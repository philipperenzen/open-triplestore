//! Background scheduler: once a minute, run any pipeline whose cron schedule is
//! due. Each fired run is recorded like a manual run. A pipeline is skipped if
//! its `last_run_at` already falls in the current minute, so a run started this
//! tick is not re-fired by clock jitter.

use std::sync::Arc;

use chrono::{DateTime, Timelike, Utc};
use tracing::{debug, warn};

use crate::auth::db::AuthDb;
use crate::store::TripleStore;

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
