//! Pipelines that run when their data changes (`trigger_on_write`).
//!
//! The flag was stored and returned but nothing ever read it. It is now a
//! change-log consumer (delta note §8.3): every tick the trigger reads the
//! rows past its cursor (`studio-trigger`), and each pipeline whose data
//! graphs those rows touched is revalidated —
//!
//! * **incrementally** when it can be: only the focus nodes the rows touched
//!   — their subjects, the IRI or blank-node objects they point at, and the
//!   nodes pointing at those subjects (a `sh:class` or `sh:node` check on a
//!   referring node depends on the node it refers to) — merged into the
//!   pipeline's last report;
//! * **in full** when it cannot: no earlier report to merge into, a row that
//!   does not carry its quads, a change to the pipeline's shapes, shapes that
//!   use SPARQL (a SPARQL constraint or target reaches what no focus set
//!   bounds), a pipeline that infers, or more than
//!   `OTS_STUDIO_INCREMENTAL_MAX` (10 000) touched nodes.
//!
//! A run reads with its creator's authority, exactly as a scheduled run does,
//! and is recorded as `triggered_by = "write"`. The cursor moves past what
//! was handled only, so a restart resumes where it stopped; a pipeline that
//! gets the trigger switched on starts from the log's head, not its history.
//! Nothing happens while change capture is off — there is nothing to read.

use std::collections::HashSet;

use oxigraph::model::{GraphNameRef, NamedNodeRef, NamedOrBlankNode, Quad, Term};
use tracing::{debug, warn};

use crate::auth::db::AuthDb;
use crate::store::changes::{decode_text, ChangeRow, Extent};
use crate::store::TripleStore;

use super::models::ValidationPipeline;
use super::read_scope::{pipeline_unreadable, ReadScope};
use super::store::ShaclStudioStore;

/// The change-log cursor the trigger keeps.
pub const CURSOR: &str = "studio-trigger";
const BATCH: usize = 2_000;

fn incremental_max() -> usize {
    crate::config::STUDIO_INCREMENTAL_MAX.get()
}

/// What one tick did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tick {
    pub incremental: usize,
    pub full: usize,
}

/// Run the write-triggered pipelines the change log calls for. Blocking.
pub fn tick(store: &TripleStore, auth_db: &AuthDb, base_url: &str) -> Tick {
    let mut done = Tick::default();
    let log = store.changes();
    if !log.enabled() {
        return done;
    }
    let studio = ShaclStudioStore::new(auth_db.pool());
    let pipelines = match studio.list_triggered_pipelines() {
        Ok(p) => p,
        Err(e) => {
            warn!("studio trigger: listing pipelines failed: {e}");
            return done;
        }
    };
    let head = log.last_seq();
    let since = match log.cursor(CURSOR) {
        Some(c) if !pipelines.is_empty() => c.seq,
        // No triggered pipeline, or the first tick: start from the head.
        _ => {
            let _ = log.set_cursor(CURSOR, head, Some("system"));
            return done;
        }
    };
    let rows = log.rows_after(since, BATCH);
    let Some(last) = rows.last().and_then(|r| r.seq) else {
        // Nothing new; keep the cursor alive.
        let _ = log.set_cursor(CURSOR, since, Some("system"));
        return done;
    };
    for p in &pipelines {
        match run_one(store, auth_db, &studio, base_url, p, &rows) {
            Ok(Some(true)) => done.incremental += 1,
            Ok(Some(false)) => done.full += 1,
            Ok(None) => {}
            Err(e) => warn!("studio trigger: pipeline {} failed: {e}", p.id),
        }
    }
    let _ = log.set_cursor(CURSOR, last, Some("system"));
    done
}

/// `Some(true)` for an incremental run, `Some(false)` for a full one,
/// `None` when the rows do not concern the pipeline (or its creator may not
/// read its scope).
fn run_one(
    store: &TripleStore,
    auth_db: &AuthDb,
    studio: &ShaclStudioStore,
    base_url: &str,
    p: &ValidationPipeline,
    rows: &[ChangeRow],
) -> anyhow::Result<Option<bool>> {
    let data_graphs: HashSet<String> = super::exec::resolve_data_graphs(auth_db, studio, p)
        .into_iter()
        .collect();
    let shape_graphs: HashSet<String> =
        super::exec::resolve_shape_graphs(store, auth_db, studio, base_url, p)
            .into_iter()
            .collect();
    let mine: Vec<&ChangeRow> = rows
        .iter()
        .filter(|r| match r.graph_iri.as_deref() {
            Some(g) => data_graphs.contains(g) || shape_graphs.contains(g),
            None => r.scope == "store",
        })
        .collect();
    if mine.is_empty() {
        return Ok(None);
    }
    let unreadable = ReadScope::for_creator(auth_db, p.created_by.as_deref())
        .and_then(|reader| pipeline_unreadable(store, auth_db, studio, base_url, p, &reader));
    match unreadable {
        Ok(None) => {}
        Ok(Some(what)) => {
            warn!(
                "studio trigger: pipeline {} skipped: its creator may not read {what}",
                p.id
            );
            return Ok(None);
        }
        Err(e) => {
            warn!("studio trigger: pipeline {} skipped: {e}", p.id);
            return Ok(None);
        }
    }
    let previous = studio
        .list_pipeline_runs(&p.id, 1)?
        .into_iter()
        .next()
        .and_then(|r| studio.get_pipeline_run(&r.id).ok().flatten())
        .and_then(|r| r.report);
    let incremental_focus = previous.as_ref().and_then(|_| {
        if p.run_inference
            || mine.iter().any(|r| {
                r.scope != "graph"
                    || r.extent != Extent::Full
                    || r.graph_iri
                        .as_deref()
                        .is_some_and(|g| shape_graphs.contains(g))
            })
            || shape_graphs.iter().any(|g| uses_sparql(store, g))
        {
            return None;
        }
        let graphs: Vec<String> = data_graphs.iter().cloned().collect();
        let focus = focus_of(store, &graphs, &mine);
        (focus.len() <= incremental_max()).then_some(focus)
    });
    match (incremental_focus, previous) {
        (Some(focus), Some(previous)) => {
            debug!(
                "studio trigger: pipeline {} over {} touched node(s)",
                p.id,
                focus.len()
            );
            super::exec::execute_pipeline_incremental(
                store, auth_db, studio, base_url, p, &previous, &focus,
            )?;
            Ok(Some(true))
        }
        _ => {
            debug!("studio trigger: pipeline {} in full", p.id);
            super::exec::execute_pipeline(store, auth_db, studio, base_url, p, "write", None)?;
            Ok(Some(false))
        }
    }
}

/// The nodes a change touched: the subjects of its quads, the IRI and
/// blank-node objects they point at, and — one hop back, in the data graphs
/// as they are now — the subjects pointing at those subjects.
fn focus_of(store: &TripleStore, graphs: &[String], rows: &[&ChangeRow]) -> HashSet<Term> {
    let quads: Vec<Quad> = rows
        .iter()
        .flat_map(|r| {
            decode_text(r.added.as_deref())
                .into_iter()
                .chain(decode_text(r.removed.as_deref()))
        })
        .collect();
    let mut focus: HashSet<Term> = HashSet::new();
    let mut subjects: Vec<NamedOrBlankNode> = Vec::new();
    for q in &quads {
        let s: Term = q.subject.clone().into();
        if focus.insert(s) {
            subjects.push(q.subject.clone());
        }
        if matches!(q.object, Term::NamedNode(_) | Term::BlankNode(_)) {
            focus.insert(q.object.clone());
        }
    }
    for s in subjects {
        let object: Term = s.into();
        for g in graphs {
            let Ok(name) = NamedNodeRef::new(g) else {
                continue;
            };
            for q in store
                .store()
                .quads_for_pattern(
                    None,
                    None,
                    Some(object.as_ref()),
                    Some(GraphNameRef::NamedNode(name)),
                )
                .flatten()
            {
                focus.insert(q.subject.into());
            }
        }
    }
    focus
}

/// Whether a shapes graph uses SPARQL-based constraints or targets.
fn uses_sparql(store: &TripleStore, shapes_graph: &str) -> bool {
    let q = format!(
        "PREFIX sh: <http://www.w3.org/ns/shacl#>
         ASK {{ GRAPH <{}> {{ {{ ?s sh:sparql ?c }} UNION {{ ?s sh:target ?t }} UNION {{ ?s a sh:SPARQLTarget }} UNION {{ ?s a sh:SPARQLConstraint }} }} }}",
        crate::store::escape_sparql_iri(shapes_graph)
    );
    !matches!(
        store.query(&q),
        Ok(oxigraph::sparql::QueryResults::Boolean(false))
    )
}
