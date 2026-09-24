//! Background scheduler: once a minute, run any pipeline whose cron schedule is
//! due. Each fired run is recorded like a manual run, and reads with its
//! creator's authority: a pipeline whose creator may no longer read its scope
//! is skipped. A pipeline is also skipped if its `last_run_at` already falls in
//! the current minute, so a run started this tick is not re-fired by clock
//! jitter.

use std::sync::Arc;

use chrono::{DateTime, Timelike, Utc};
use tracing::{debug, warn};

use crate::auth::db::AuthDb;
use crate::store::TripleStore;

use super::read_scope::{pipeline_unreadable, ReadScope};
use super::store::ShaclStudioStore;

/// Spawn the pipeline scheduler. Cheap; holds clones of the store + auth db.
pub fn spawn_scheduler(store: TripleStore, auth_db: Arc<AuthDb>, base_url: String) {
    tokio::spawn(async move {
        let tick = std::time::Duration::from_secs(60);
        loop {
            tokio::time::sleep(tick).await;
            let store = store.clone();
            let auth_db = auth_db.clone();
            let base_url = base_url.clone();
            // Validation is blocking (rayon + SPARQL); keep it off the async pool.
            let _ = tokio::task::spawn_blocking(move || run_due(&store, &auth_db, &base_url)).await;
        }
    });
}

fn run_due(store: &TripleStore, auth_db: &AuthDb, base_url: &str) {
    let studio = ShaclStudioStore::new(auth_db.pool());
    let now = Utc::now();
    let pipelines = match studio.list_scheduled_pipelines() {
        Ok(p) => p,
        Err(e) => {
            warn!("scheduler: failed to list scheduled pipelines: {e}");
            return;
        }
    };
    for p in pipelines {
        let Some(cron) = p.schedule_cron.as_deref() else {
            continue;
        };
        if !super::cron::is_due(cron, now) {
            continue;
        }
        if already_ran_this_minute(p.last_run_at.as_deref(), now) {
            continue;
        }
        // A scheduled run reads with its creator's authority, checked at every
        // run as a manual run checks its caller's: a creator who has lost read
        // access to the pipeline's scope, or is no longer a user, gets no run,
        // and so no report for the pipeline's viewers to open.
        let unreadable = ReadScope::for_creator(auth_db, p.created_by.as_deref())
            .and_then(|reader| pipeline_unreadable(auth_db, &studio, &p, &reader));
        match unreadable {
            Ok(None) => {}
            Ok(Some(what)) => {
                warn!(
                    "scheduler: pipeline {} skipped: its creator may not read {what}",
                    p.id
                );
                continue;
            }
            Err(e) => {
                warn!(
                    "scheduler: pipeline {} skipped: its read scope could not be checked: {e}",
                    p.id
                );
                continue;
            }
        }
        debug!("scheduler: running pipeline {} ({})", p.id, p.name);
        if let Err(e) =
            super::exec::execute_pipeline(store, auth_db, &studio, base_url, &p, "schedule", None)
        {
            warn!("scheduler: pipeline {} failed: {e}", p.id);
        }
    }
}

fn already_ran_this_minute(last_run_at: Option<&str>, now: DateTime<Utc>) -> bool {
    match last_run_at.and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
        Some(prev) => {
            let prev = prev.with_timezone(&Utc);
            prev.date_naive() == now.date_naive()
                && prev.hour() == now.hour()
                && prev.minute() == now.minute()
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::{OwnerType, SystemRole, Visibility};
    use crate::shacl_studio::models::{
        ResultsTarget, SeverityThreshold, ValidationPipeline, WriteTarget,
    };

    const GRAPH: &str = "http://alice.example/private";

    fn scheduled(id: &str, created_by: Option<&str>) -> ValidationPipeline {
        ValidationPipeline {
            id: id.into(),
            name: id.into(),
            description: None,
            owner_type: OwnerType::User,
            owner_id: created_by.unwrap_or("nobody").into(),
            visibility: Visibility::Public,
            targets: vec![],
            dataset_ids: vec![],
            graph_iris: vec![GRAPH.into()],
            target_classes: vec![],
            shape_graph_ids: vec![],
            severity_threshold: SeverityThreshold::Violation,
            run_inference: false,
            max_results: None,
            trigger_on_write: false,
            schedule_cron: Some("* * * * *".into()),
            gate_writes: false,
            retention: 50,
            inferred_target: WriteTarget::InPlace,
            inferred_target_graph: None,
            results_target: ResultsTarget::None,
            results_target_graph: None,
            last_run_at: None,
            last_conforms: None,
            created_by: created_by.map(String::from),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// A scheduled run reads with its creator's authority, checked at every
    /// run: a pipeline over a graph its creator may not read (made before the
    /// Studio checked, or whose grant was since revoked) records no run, and
    /// so no report for the pipeline's viewers to open.
    #[test]
    fn a_scheduled_run_needs_its_creator_to_read_the_scope() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        for id in ["alice", "mallory"] {
            auth.create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
                .unwrap();
        }
        auth.create_dataset(
            "alice-ds",
            "alice-ds",
            None,
            OwnerType::User,
            "alice",
            Visibility::Private,
            None,
        )
        .unwrap();
        store
            .load_str(
                "<http://ex.org/p1> <http://ex.org/ssn> \"123-45-6789\" .",
                oxigraph::io::RdfFormat::Turtle,
                Some(GRAPH),
            )
            .unwrap();
        auth.add_dataset_graph("alice-ds", GRAPH).unwrap();

        let studio = ShaclStudioStore::new(auth.pool());
        for (id, by) in [
            ("alice-p", Some("alice")),
            ("mallory-p", Some("mallory")),
            ("gone-p", Some("deleted-user")),
            ("anonymous-p", None),
        ] {
            studio.insert_pipeline(&scheduled(id, by)).unwrap();
        }

        run_due(&store, &auth, "http://localhost:7878");

        let runs = |id: &str| studio.list_pipeline_runs(id, 10).unwrap().len();
        assert_eq!(runs("alice-p"), 1, "alice may read her graph");
        for id in ["mallory-p", "gone-p", "anonymous-p"] {
            assert_eq!(
                runs(id),
                0,
                "{id} ran over a graph its creator may not read"
            );
        }
    }
}
