//! Materialisation runs: source → RDF, gate, atomic promotion, rollback.
//!
//! A run never writes into a live graph. It materialises into a fresh
//! `urn:run:<id>`, the SHACL write gate validates *that* graph, and only a
//! passing candidate takes the production role — in one update, so a reader
//! sees either the old graph or the new one. A failing gate leaves production
//! exactly as it was and keeps the candidate for inspection.
//!
//! Rollback re-points the source at the graph it served before. It never
//! re-runs the mapping, so it cannot fail on a source that has since changed
//! or gone away.

use std::collections::HashSet;

use crate::auth::db::AuthDb;
use crate::auth::models::GraphKind;
use crate::secrets::Secret;
use crate::shacl::report::ValidationReport;
use crate::store::TripleStore;

use super::model::*;
use super::{connector, mappings, registry};

/// What a run needs from the running instance.
#[derive(Clone, Copy)]
pub struct RunContext<'a> {
    pub store: &'a TripleStore,
    pub auth_db: &'a AuthDb,
    pub base_url: &'a str,
}

#[derive(Debug)]
pub enum RunError {
    /// The request cannot be carried out as asked (400).
    BadRequest(String),
    /// Materialised but refused by the write gate (422). The candidate graph
    /// named by `run.graph` is kept for inspection.
    Gate {
        run: Box<RunRecord>,
        report: Box<ValidationReport>,
    },
    /// Something failed while running (500). Already scrubbed.
    Failed(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::BadRequest(m) | RunError::Failed(m) => f.write_str(m),
            RunError::Gate { .. } => f.write_str("the SHACL write gate refused the run"),
        }
    }
}

/// Remove everything about a datasource's location from a message before it
/// reaches a caller: the credential value, the database name or path, the
/// host and the account. A driver's error text is written for an operator
/// reading server logs, not for an API client.
pub fn scrub(message: &str, source: &SqlSource, secret: Option<&Secret>) -> String {
    let mut out = message.to_string();
    if let Some(s) = secret {
        out = crate::secrets::redact(&out, &[s]);
    }
    for part in [
        Some(source.database.as_str()),
        source.host.as_deref(),
        source.username.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        for candidate in location_forms(part) {
            if candidate.len() >= 3 && out.contains(&candidate) {
                out = out.replace(&candidate, "[redacted]");
            }
        }
    }
    out
}

/// A path can appear whole, or only as its file name / stem — SQLite reports
/// "unable to open database file" with the name alone.
fn location_forms(value: &str) -> Vec<String> {
    let mut forms = vec![value.to_string()];
    let path = std::path::Path::new(value);
    for part in [path.file_name(), path.file_stem()].into_iter().flatten() {
        forms.push(part.to_string_lossy().into_owned());
    }
    forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
    forms.dedup();
    forms
}

/// Open a connection to `source`, scrubbing any failure.
pub fn connect(
    source: &SqlSource,
) -> Result<Box<dyn ots_plugin_api::sources::SourceConnection>, String> {
    let params = source
        .connect_params()
        .map_err(|e| format!("the credential could not be resolved: {e}"))?;
    // Kept only to scrub it out of an error the driver may have embedded it in.
    let resolved = source
        .credential
        .as_ref()
        .and_then(|r| crate::secrets::resolve(r).ok());
    connector::connect(&params).map_err(|e| {
        let scrubbed = scrub(&e.to_string(), source, resolved.as_ref());
        format!(
            "could not connect to datasource '{}': {scrubbed}",
            source.id
        )
    })
}

/// Compare two watermark values the way their column orders them.
///
/// Numeric when both parse as numbers — a lexicographic `"9" > "10"` would
/// stall an integer-keyed cursor forever — and lexicographic otherwise, which
/// is correct for ISO-8601 timestamps and for zero-padded identifiers.
fn watermark_max(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => {
            let greater = match (a.parse::<f64>(), b.parse::<f64>()) {
                (Ok(x), Ok(y)) => x >= y,
                _ => a >= b,
            };
            Some(if greater { a } else { b })
        }
        (some, None) | (None, some) => some,
    }
}

/// The tables a mapping reads that actually carry `column`, and the highest
/// value the source currently holds for it.
///
/// One introspection call, so the cursor is read from the catalogue rather than
/// guessed from the mapping.
fn watermark_state(
    mapping: &crate::rml::model::RmlMapping,
    conn: &mut dyn ots_plugin_api::sources::SourceConnection,
    column: &str,
) -> Result<(HashSet<String>, Option<String>), String> {
    let wanted: HashSet<String> = mapping
        .triples_maps
        .iter()
        .filter(|tm| tm.logical_source.query.is_none())
        .filter_map(|tm| tm.logical_source.table_name.clone())
        .collect();
    let tables = conn
        .introspect()
        .map_err(|e| format!("reading the schema to locate the watermark column: {e}"))?;

    let mut carrying = HashSet::new();
    let mut high: Option<String> = None;
    for t in tables.iter().filter(|t| wanted.contains(&t.name)) {
        if !t.columns.iter().any(|c| c.name == column) {
            continue;
        }
        carrying.insert(t.name.clone());
        high = watermark_max(high, conn.max_watermark(&t.name, column).ok().flatten());
    }
    if carrying.is_empty() {
        return Err(format!(
            "no table this mapping reads has a column named '{column}', so there is nothing to \
             track incrementally"
        ));
    }
    Ok((carrying, high))
}

/// Replace, per IRI subject, whatever `delta` holds into `target`.
///
/// An incremental run re-maps a changed row in full, so the row's previous
/// triples have to go: merging would leave the old value of every field the
/// update changed sitting beside the new one. Blank-node subjects are not
/// matched across graphs, so an entity identified only by a blank node is
/// appended rather than replaced.
fn apply_delta(store: &TripleStore, target: &str, delta: &str) -> Result<(), String> {
    let t = crate::store::escape_sparql_iri(target);
    let d = crate::store::escape_sparql_iri(delta);
    store
        .update(&format!(
            "DELETE {{ GRAPH <{t}> {{ ?s ?p ?o }} }} \
             WHERE {{ GRAPH <{d}> {{ ?s ?x ?y }} GRAPH <{t}> {{ ?s ?p ?o }} }};\n\
             INSERT {{ GRAPH <{t}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{d}> {{ ?s ?p ?o }} }}"
        ))
        .map_err(|e| format!("applying the increment into <{target}>: {e}"))
}

/// Copy every triple of `from` into `to`.
fn copy_graph(store: &TripleStore, from: &str, to: &str) -> Result<(), String> {
    let f = crate::store::escape_sparql_iri(from);
    let t = crate::store::escape_sparql_iri(to);
    store
        .update(&format!(
            "INSERT {{ GRAPH <{t}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{f}> {{ ?s ?p ?o }} }}"
        ))
        .map_err(|e| format!("copying <{from}> into <{to}>: {e}"))
}

/// The IRI subjects a graph names.
fn subjects_of(store: &TripleStore, graph: &str) -> Vec<String> {
    use oxigraph::sparql::QueryResults;
    let g = crate::store::escape_sparql_iri(graph);
    let Ok(QueryResults::Solutions(sols)) = store.query(&format!(
        "SELECT DISTINCT ?s WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} FILTER(isIRI(?s)) }}"
    )) else {
        return Vec::new();
    };
    sols.flatten()
        .filter_map(|b| match b.get("s") {
            Some(oxigraph::model::Term::NamedNode(n)) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect()
}

/// Publish the entities a run changed to the dataset's LDES stream.
///
/// Gated on the stream being enabled, so a deployment that does not publish
/// pays nothing. Only the changed entities are published: a run graph is new
/// every time, so the store's own before/after capture — which diffs one graph
/// against itself — cannot see what actually moved.
fn publish_members(
    ctx: RunContext<'_>,
    source: &SqlSource,
    graph: &str,
    entities: &[String],
) -> u64 {
    let Some(dataset_id) = source.dataset.as_deref() else {
        return 0;
    };
    match crate::ldes::store::stream(ctx.auth_db, dataset_id) {
        Ok(Some(cfg)) if cfg.enabled => {}
        _ => return 0,
    }
    let now = registry::now();
    let mut published = 0u64;
    for entity in entities {
        let nt = crate::ldes::capture::describe_entity(ctx.store, graph, entity);
        if nt.is_empty() {
            continue;
        }
        if crate::ldes::store::insert_member(
            ctx.auth_db,
            dataset_id,
            entity,
            graph,
            &now,
            false,
            &nt,
        )
        .is_ok()
        {
            published += 1;
        }
    }
    published
}

/// Build an incremental candidate: the graph in production, with the entities
/// whose rows moved past the cursor re-mapped over the top.
///
/// Returns the row/triple counts of the INCREMENT (not of the whole graph), the
/// entities it replaced, and the cursor the next run resumes from.
#[allow(clippy::too_many_arguments)]
fn materialise_increment(
    ctx: RunContext<'_>,
    rml: &crate::rml::model::RmlMapping,
    conn: &mut dyn ots_plugin_api::sources::SourceConnection,
    quote: &dyn Fn(&str) -> String,
    graph: &str,
    previous: &RunPointer,
    column: &str,
    cursor: &str,
    batch_size: usize,
    run_id: &str,
) -> Result<(crate::rml::sql::SqlOutcome, Vec<String>, Option<String>), String> {
    let (tables, high) = watermark_state(rml, conn, column)?;
    let scratch = format!("{graph}:delta");
    let filter = crate::rml::sql::RowFilter {
        column: column.to_string(),
        greater_than: cursor.to_string(),
        tables,
    };

    let outcome = crate::rml::sql::execute_relational_filtered(
        rml,
        conn,
        quote,
        ctx.store,
        &scratch,
        batch_size,
        run_id,
        Some(&filter),
    );
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => {
            let _ = ctx.store.bulk_delete_graphs(&[scratch.as_str()]);
            return Err(e);
        }
    };

    let changed = subjects_of(ctx.store, &scratch);
    let result = copy_graph(ctx.store, &previous.graph, graph)
        .and_then(|()| apply_delta(ctx.store, graph, &scratch));
    let _ = ctx.store.bulk_delete_graphs(&[scratch.as_str()]);
    result?;

    // The cursor only advances past rows this run actually consumed.
    Ok((
        outcome,
        changed,
        watermark_max(Some(cursor.to_string()), high),
    ))
}

/// Run `mapping` against `source` and, if the gate passes, promote the result.
#[allow(clippy::too_many_arguments)]
pub fn execute(
    ctx: RunContext<'_>,
    source: &SqlSource,
    mapping: Option<&MappingRecord>,
    mode: RunMode,
    model_version: Option<String>,
    batch_size: usize,
    actor: Option<&str>,
) -> Result<RunRecord, RunError> {
    if let Some(mapping) = mapping {
        if mapping.source_id != source.id {
            return Err(RunError::BadRequest(format!(
                "mapping '{}' is registered against datasource '{}', not '{}'",
                mapping.id, mapping.source_id, source.id
            )));
        }
    }
    // A snapshot is a virtual source's own graph, whole; every other mode
    // runs a mapping.
    if mode == RunMode::Snapshot {
        if !super::virtual_source::is_virtual(source) {
            return Err(RunError::BadRequest(format!(
                "datasource '{}' is a {} database; a snapshot run materialises a virtual \
                 (sparql) source's graph — run a mapping instead",
                source.id, source.dialect
            )));
        }
        return snapshot(ctx, source, model_version, actor);
    }
    let mapping = mapping.ok_or_else(|| {
        RunError::BadRequest(
            "a run needs a mapping; only mode 'snapshot' runs without one".to_string(),
        )
    })?;

    // An incremental run builds on the graph currently in production and needs
    // a cursor to resume from. Both come from the run log, so the first run of
    // a source is always a full one.
    let increment = if mode == RunMode::Watermark {
        let column = source.watermark_column.clone().ok_or_else(|| {
            RunError::BadRequest(format!(
                "datasource '{}' declares no watermarkColumn, so there is nothing to read \
                 incrementally",
                source.id
            ))
        })?;
        let previous = source.production.clone().ok_or_else(|| {
            RunError::BadRequest(
                "an incremental run builds on the graph in production, and this datasource has \
                 none yet; run mode 'full' first"
                    .to_string(),
            )
        })?;
        let cursor = registry::last_watermark(ctx.store, &source.id).ok_or_else(|| {
            RunError::BadRequest(
                "no previous run recorded a watermark to resume from; run mode 'full' first"
                    .to_string(),
            )
        })?;
        Some((column, previous, cursor))
    } else {
        None
    };

    let rml = mappings::load(ctx.store, &mapping.id, mapping.version)
        .map_err(|e| RunError::BadRequest(e.to_string()))?;
    let connector = connector::get(&source.dialect).ok_or_else(|| {
        RunError::BadRequest(format!(
            "no connector for dialect '{}'; this build supports: {}",
            source.dialect,
            connector::dialects().join(", ")
        ))
    })?;
    let mut conn = connect(source).map_err(RunError::Failed)?;

    let run_id = uuid::Uuid::new_v4().to_string();
    let graph = run_graph_iri(&run_id);
    let started_at = registry::now();
    let clock = std::time::Instant::now();

    let quote = |ident: &str| connector.quote_identifier(ident);
    // A full run materialises everything into the fresh graph. An incremental
    // one copies the graph in production and replaces only the entities whose
    // rows moved — so the candidate is still a complete graph, and the gate,
    // the swap and rollback all keep working exactly as they do for a full run.
    let materialised = match &increment {
        None => {
            let watermark = source.watermark_column.as_deref().and_then(|c| {
                watermark_state(&rml, conn.as_mut(), c)
                    .map(|(_, high)| high)
                    .unwrap_or(None)
            });
            crate::rml::execute_relational(
                &rml,
                conn.as_mut(),
                &quote,
                ctx.store,
                &graph,
                batch_size,
                &run_id,
            )
            .map(|o| (o, Vec::new(), watermark))
        }
        Some((column, previous, cursor)) => materialise_increment(
            ctx,
            &rml,
            conn.as_mut(),
            &quote,
            &graph,
            previous,
            column,
            cursor,
            batch_size,
            &run_id,
        ),
    };

    let mut record = RunRecord {
        id: run_id.clone(),
        source_id: source.id.clone(),
        mapping_id: mapping.id.clone(),
        mapping_version: mapping.version,
        model_version: model_version.or_else(|| mapping.model_version.clone()),
        mode: mode.as_str().to_string(),
        graph: graph.clone(),
        previous_graph: source.production.as_ref().map(|p| p.graph.clone()),
        started_at,
        actor: actor.map(str::to_string),
        ..Default::default()
    };

    let (outcome, changed, watermark) = match materialised {
        Ok(o) => o,
        Err(e) => {
            // A partial candidate graph is not evidence of anything; drop it.
            let _ = ctx.store.bulk_delete_graphs(&[graph.as_str()]);
            let message = scrub(&e, source, None);
            record.status = Some(RunStatus::Failed);
            record.ended_at = registry::now();
            record.duration_ms = clock.elapsed().as_millis() as u64;
            record.error = Some(message.clone());
            let _ = registry::put_run(ctx.store, &record);
            return Err(RunError::Failed(message));
        }
    };
    record.rows_extracted = outcome.rows;
    record.triples_produced = outcome.triples;
    record.watermark = watermark;

    // ── The write gate applies to the candidate graph, not to every batch ──
    let report = gate(ctx, source, Some(mapping), &graph).map_err(RunError::Failed)?;
    if let Some(report) = &report {
        record.conforms = Some(report.conforms);
        record.violations = report.results_count as u64;
    }
    record.ended_at = registry::now();
    record.duration_ms = clock.elapsed().as_millis() as u64;

    if let Some(report) = report {
        if !report.conforms {
            record.status = Some(RunStatus::Rejected);
            registry::put_run(ctx.store, &record).map_err(RunError::Failed)?;
            commit(ctx, source, &record, "rejected by the SHACL write gate");
            // The refusal becomes work: one review item per subject. A queue
            // that cannot be written does not turn the refusal into a failure.
            match super::review::open_items(ctx.store, source, &record, &report) {
                Ok(n) => tracing::info!(run = %record.id, items = n, "review items opened"),
                Err(e) => tracing::warn!(run = %record.id, "review items not opened: {e}"),
            }
            return Err(RunError::Gate {
                run: Box::new(record),
                report: Box::new(report),
            });
        }
    }

    record.status = Some(RunStatus::Succeeded);
    promote(ctx, source, &record).map_err(RunError::Failed)?;
    // Published after the swap, so a member never points at a graph that is not
    // yet the one being served. A full run publishes every entity it wrote; an
    // incremental one publishes only what moved.
    let entities = if increment.is_some() {
        changed
    } else {
        subjects_of(ctx.store, &graph)
    };
    record.ldes_members = publish_members(ctx, source, &graph, &entities);
    registry::put_run(ctx.store, &record).map_err(RunError::Failed)?;
    commit(ctx, source, &record, "materialised and promoted");
    Ok(record)
}

/// Materialise a virtual source's whole graph as a run: the endpoint's
/// `CONSTRUCT`, then the same gate, swap and record as a mapped run.
fn snapshot(
    ctx: RunContext<'_>,
    source: &SqlSource,
    model_version: Option<String>,
    actor: Option<&str>,
) -> Result<RunRecord, RunError> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let graph = run_graph_iri(&run_id);
    let started_at = registry::now();
    let clock = std::time::Instant::now();
    let mut record = RunRecord {
        id: run_id.clone(),
        source_id: source.id.clone(),
        mapping_id: String::new(),
        mapping_version: 0,
        model_version,
        mode: RunMode::Snapshot.as_str().to_string(),
        graph: graph.clone(),
        previous_graph: source.production.as_ref().map(|p| p.graph.clone()),
        started_at,
        actor: actor.map(str::to_string),
        ..Default::default()
    };
    let triples = match super::virtual_source::snapshot(source, ctx.store, &graph) {
        Ok(n) => n,
        Err(e) => {
            let _ = ctx.store.bulk_delete_graphs(&[graph.as_str()]);
            let message = scrub(&e, source, None);
            record.status = Some(RunStatus::Failed);
            record.ended_at = registry::now();
            record.duration_ms = clock.elapsed().as_millis() as u64;
            record.error = Some(message.clone());
            let _ = registry::put_run(ctx.store, &record);
            return Err(RunError::Failed(message));
        }
    };
    record.triples_produced = triples;
    let report = gate(ctx, source, None, &graph).map_err(RunError::Failed)?;
    if let Some(report) = &report {
        record.conforms = Some(report.conforms);
        record.violations = report.results_count as u64;
    }
    record.ended_at = registry::now();
    record.duration_ms = clock.elapsed().as_millis() as u64;
    if let Some(report) = report {
        if !report.conforms {
            record.status = Some(RunStatus::Rejected);
            registry::put_run(ctx.store, &record).map_err(RunError::Failed)?;
            commit(ctx, source, &record, "rejected by the SHACL write gate");
            match super::review::open_items(ctx.store, source, &record, &report) {
                Ok(n) => tracing::info!(run = %record.id, items = n, "review items opened"),
                Err(e) => tracing::warn!(run = %record.id, "review items not opened: {e}"),
            }
            return Err(RunError::Gate {
                run: Box::new(record),
                report: Box::new(report),
            });
        }
    }
    record.status = Some(RunStatus::Succeeded);
    promote(ctx, source, &record).map_err(RunError::Failed)?;
    let entities = subjects_of(ctx.store, &graph);
    record.ldes_members = publish_members(ctx, source, &graph, &entities);
    registry::put_run(ctx.store, &record).map_err(RunError::Failed)?;
    commit(ctx, source, &record, "snapshot taken and promoted");
    Ok(record)
}

/// Validate the candidate graph against every shapes graph that applies: the
/// one the mapping declares it conforms to, and the bound dataset's own
/// `shacl_on_write` shapes. `None` means no gate applied.
fn gate(
    ctx: RunContext<'_>,
    source: &SqlSource,
    mapping: Option<&MappingRecord>,
    candidate: &str,
) -> Result<Option<ValidationReport>, String> {
    let mut shape_graphs: Vec<String> = Vec::new();
    if let Some(g) = mapping.and_then(|m| m.shapes_graph.clone()) {
        shape_graphs.push(g);
    }
    if let Some(dataset_id) = &source.dataset {
        if let Ok(Some(ds)) = ctx.auth_db.get_dataset(dataset_id) {
            if ds.shacl_on_write {
                if let Some(g) = ds.shapes_graph_iri.filter(|g| !g.is_empty()) {
                    shape_graphs.push(g);
                }
            }
        }
    }
    shape_graphs.sort();
    shape_graphs.dedup();
    if shape_graphs.is_empty() {
        return Ok(None);
    }

    let data = vec![candidate.to_string()];
    let mut combined = ValidationReport {
        conforms: true,
        results: Vec::new(),
        results_count: 0,
        metrics: None,
    };
    for shapes in &shape_graphs {
        // A gate that cannot be evaluated must refuse the promotion, not wave
        // it through: the same rule the Graph Store write gate follows.
        let report = crate::shacl::engine::validate(ctx.store, shapes, &data).map_err(|e| {
            format!("the write gate could not be evaluated against <{shapes}>: {e}")
        })?;
        combined.conforms &= report.conforms;
        combined.results_count += report.results_count;
        combined.results.extend(report.results);
        // One shapes graph at a time, folded the way the validate route does.
        combined.metrics = match (combined.metrics.take(), report.metrics) {
            (Some(a), Some(b)) => Some(a.merge(b)),
            (a, b) => a.or(b),
        };
    }
    Ok(Some(combined))
}

/// Give the run's graph the production role for its source, demoting the
/// previous one. The pointer swap is a single update; the dataset
/// registration follows it, so the store is never without a production graph.
fn promote(ctx: RunContext<'_>, source: &SqlSource, run: &RunRecord) -> Result<(), String> {
    let new = RunPointer {
        graph: run.graph.clone(),
        run: run.id.clone(),
    };
    let previous = source.production.clone();
    registry::set_production(ctx.store, &source.id, Some(&new), previous.as_ref())?;
    if let Some(dataset_id) = &source.dataset {
        attach(ctx, dataset_id, &new.graph);
        if let Some(old) = &previous {
            detach(ctx, dataset_id, &old.graph);
        }
    }
    Ok(())
}

/// Why a kept candidate could not be promoted.
#[derive(Debug)]
pub enum PromoteError {
    /// The run is in production already, or has no graph to promote (409).
    Conflict(String),
    /// The gate still refuses the graph as it now stands (422).
    Gate(Box<ValidationReport>),
    /// Something failed while promoting (500).
    Failed(String),
}

/// What a promotion after review produced.
#[derive(Debug, Clone)]
pub struct Promotion {
    pub id: String,
    pub run: RunRecord,
    pub source: SqlSource,
    pub actor: Option<String>,
    pub at: String,
}

/// Promote a kept candidate — a run the gate refused, since corrected — after
/// gating it again as it now stands. The same swap a passing run makes, with
/// its own `ds:Promotion` activity naming who released it.
pub fn promote_candidate(
    ctx: RunContext<'_>,
    source: &SqlSource,
    run: &RunRecord,
    actor: Option<&str>,
) -> Result<Promotion, PromoteError> {
    if source.production.as_ref().is_some_and(|p| p.run == run.id) {
        return Err(PromoteError::Conflict(format!(
            "run '{}' is in production for datasource '{}' already",
            run.id, source.id
        )));
    }
    if run.status == Some(RunStatus::Failed)
        || ctx.store.count_graph(Some(&run.graph)).unwrap_or(0) == 0
    {
        return Err(PromoteError::Conflict(format!(
            "run '{}' has no candidate graph to promote; trigger a new run instead",
            run.id
        )));
    }
    // A snapshot has no mapping; a mapped run's mapping must still exist for
    // its shapes graph to be found.
    let mapping = match run.mapping() {
        Some((id, _)) => Some(registry::get_mapping(ctx.store, id).ok_or_else(|| {
            PromoteError::Conflict(format!(
                "mapping '{id}' no longer exists, so the candidate cannot be gated"
            ))
        })?),
        None => None,
    };

    // The gate, over the graph as it stands after correction. Refused stays
    // refused: production is untouched, the candidate is kept.
    let report = gate(ctx, source, mapping.as_ref(), &run.graph).map_err(PromoteError::Failed)?;
    if let Some(report) = &report {
        if !report.conforms {
            return Err(PromoteError::Gate(Box::new(report.clone())));
        }
    }

    let mut record = run.clone();
    record.status = Some(RunStatus::Succeeded);
    record.conforms = report.as_ref().map(|r| r.conforms);
    record.violations = 0;
    record.error = None;
    record.previous_graph = source.production.as_ref().map(|p| p.graph.clone());
    promote(ctx, source, &record).map_err(PromoteError::Failed)?;
    let entities = subjects_of(ctx.store, &record.graph);
    record.ldes_members = publish_members(ctx, source, &record.graph, &entities);
    registry::put_run(ctx.store, &record).map_err(PromoteError::Failed)?;
    let at = registry::now();
    let id = registry::record_promotion(ctx.store, &source.id, &record, actor, &at)
        .map_err(PromoteError::Failed)?;
    crate::commit_log::record(
        ctx.store,
        ctx.base_url,
        crate::commit_log::CommitKind::Source,
        format!(
            "run {} of mapping '{}' v{} against datasource '{}' promoted after review ({} triples)",
            record.id,
            record.mapping_id,
            record.mapping_version,
            source.id,
            record.triples_produced
        ),
        actor_id(actor).as_deref(),
        Some(source.iri()),
        vec![record.graph.clone()],
        record.triples_produced as usize,
        0,
        None,
    );
    let source = registry::get_source(ctx.store, &source.id).ok_or_else(|| {
        PromoteError::Failed("the datasource disappeared during promotion".to_string())
    })?;
    Ok(Promotion {
        id,
        run: record,
        source,
        actor: actor.map(str::to_string),
        at,
    })
}

/// Re-point a source at the graph it served before `run`.
pub fn rollback(
    ctx: RunContext<'_>,
    source: &SqlSource,
    run: &RunRecord,
    actor: Option<&str>,
) -> Result<SqlSource, RunError> {
    let current = source.production.as_ref().ok_or_else(|| {
        RunError::BadRequest(format!(
            "datasource '{}' has no production graph",
            source.id
        ))
    })?;
    if current.run != run.id {
        return Err(RunError::BadRequest(format!(
            "run '{}' is not the one in production; roll back the current run '{}' instead",
            run.id, current.run
        )));
    }
    let target = source.previous.clone().ok_or_else(|| {
        RunError::BadRequest(format!(
            "run '{}' is the first for this datasource; there is nothing to roll back to",
            run.id
        ))
    })?;
    if ctx.store.count_graph(Some(&target.graph)).unwrap_or(0) == 0 {
        return Err(RunError::BadRequest(format!(
            "the previous graph <{}> is empty or was deleted; rollback re-points, it does not \
             re-run — trigger a new run instead",
            target.graph
        )));
    }

    // The swap, then the dataset registration, then the provenance.
    registry::set_production(ctx.store, &source.id, Some(&target), Some(current))
        .map_err(RunError::Failed)?;
    if let Some(dataset_id) = &source.dataset {
        attach(ctx, dataset_id, &target.graph);
        detach(ctx, dataset_id, &current.graph);
    }
    registry::record_rollback(ctx.store, &source.id, current, &target, actor)
        .map_err(RunError::Failed)?;
    crate::commit_log::record(
        ctx.store,
        ctx.base_url,
        crate::commit_log::CommitKind::Source,
        format!(
            "rolled datasource '{}' back from run {} to run {}",
            source.id, current.run, target.run
        ),
        actor_id(actor).as_deref(),
        Some(source.iri()),
        vec![target.graph.clone(), current.graph.clone()],
        0,
        0,
        None,
    );

    registry::get_source(ctx.store, &source.id)
        .ok_or_else(|| RunError::Failed("the datasource disappeared during rollback".to_string()))
}

/// Delete a run's graph and record. Refuses the graph a source is currently
/// serving from.
pub fn delete(ctx: RunContext<'_>, run: &RunRecord) -> Result<(), RunError> {
    let source = registry::get_source(ctx.store, &run.source_id);
    if let Some(s) = &source {
        if s.production.as_ref().is_some_and(|p| p.run == run.id) {
            return Err(RunError::BadRequest(format!(
                "run '{}' is in production for datasource '{}'; roll back first",
                run.id, s.id
            )));
        }
        // Deleting the graph a rollback would return to must not leave a
        // pointer at nothing.
        if s.previous.as_ref().is_some_and(|p| p.run == run.id) {
            registry::set_production(ctx.store, &s.id, s.production.as_ref(), None)
                .map_err(RunError::Failed)?;
        }
        if let Some(dataset_id) = &s.dataset {
            detach(ctx, dataset_id, &run.graph);
        }
    }
    // Its review items point at a graph that is about to go.
    super::review::delete_for_run(ctx.store, &run.source_id, &run.id).map_err(RunError::Failed)?;
    registry::delete_run(ctx.store, run).map_err(RunError::Failed)
}

fn attach(ctx: RunContext<'_>, dataset_id: &str, graph: &str) {
    if let Err(e) = ctx.auth_db.add_dataset_graph(dataset_id, graph) {
        tracing::warn!(
            dataset = dataset_id,
            graph,
            "could not register run graph: {e}"
        );
        return;
    }
    if let Err(e) =
        ctx.auth_db
            .set_dataset_graph_role(dataset_id, graph, Some(GraphKind::Instances))
    {
        tracing::warn!(
            dataset = dataset_id,
            graph,
            "could not tag run graph role: {e}"
        );
    }
}

fn detach(ctx: RunContext<'_>, dataset_id: &str, graph: &str) {
    if let Err(e) = ctx.auth_db.remove_dataset_graph(dataset_id, graph) {
        tracing::warn!(
            dataset = dataset_id,
            graph,
            "could not unregister run graph: {e}"
        );
    }
}

/// The user id inside an actor IRI (`{base}/users/{id}`), which the commit log
/// re-derives the IRI from.
fn actor_id(actor: Option<&str>) -> Option<String> {
    actor?.rsplit('/').next().map(str::to_string)
}

fn commit(ctx: RunContext<'_>, source: &SqlSource, run: &RunRecord, what: &str) {
    crate::commit_log::record(
        ctx.store,
        ctx.base_url,
        crate::commit_log::CommitKind::Source,
        match run.mapping() {
            Some((id, version)) => format!(
                "run {} of mapping '{id}' v{version} against datasource '{}' {what} ({} rows, {} triples)",
                run.id, source.id, run.rows_extracted, run.triples_produced
            ),
            None => format!(
                "snapshot run {} of datasource '{}' {what} ({} triples)",
                run.id, source.id, run.triples_produced
            ),
        },
        actor_id(run.actor.as_deref()).as_deref(),
        Some(source.iri()),
        vec![run.graph.clone()],
        run.triples_produced as usize,
        0,
        None,
    );
}

/// A run's PROV-O trail as Turtle: the activity, the graph it generated, the
/// datasource and the mapping version it used, and the agent.
///
/// The records live in `urn:system:sources`, which the SPARQL endpoint scopes
/// out of a caller's dataset (a system graph belongs to no dataset), so this
/// is how a client follows a run's provenance — the same arrangement
/// `GET /api/datasets/:id/provenance` uses for the commit log.
pub fn provenance_turtle(store: &TripleStore, run: &RunRecord) -> String {
    let mut subjects = vec![
        run_activity_iri(&run.id),
        run.graph.clone(),
        source_iri(&run.source_id),
    ];
    if let Some((id, version)) = run.mapping() {
        subjects.push(mapping_version_iri(id, version));
        subjects.push(mapping_iri(id));
    }
    let subjects = subjects
        .iter()
        .map(|s| format!("<{}>", crate::store::escape_sparql_iri(s)))
        .collect::<Vec<_>>()
        .join(" ");

    // Whatever later used the run's graph — a promotion after review, a
    // rollback to it — is part of the run's story too.
    let query = format!(
        "PREFIX prov: <http://www.w3.org/ns/prov#>\n\
         CONSTRUCT {{ ?s ?p ?o }} WHERE {{ GRAPH <{graph}> {{\n\
           {{ VALUES ?s {{ {subjects} }} ?s ?p ?o }}\n\
           UNION {{ ?s prov:used <{run_graph}> ; ?p ?o }}\n\
         }} }}",
        graph = SOURCES_GRAPH,
        run_graph = crate::store::escape_sparql_iri(&run.graph),
    );
    prov_turtle(store, &query)
}

/// The vocabularies a served PROV trail is written against.
const PROV_PREFIXES: [(&str, &str); 6] = [
    ("prov", "http://www.w3.org/ns/prov#"),
    ("dct", "http://purl.org/dc/terms/"),
    ("ds", crate::sources::model::DS),
    (
        crate::sources::profile::PROF_LABEL,
        crate::sources::profile::PROF,
    ),
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
];

/// Run a CONSTRUCT over the system graph and serialise it as prefixed Turtle.
pub(crate) fn prov_turtle(store: &TripleStore, construct: &str) -> String {
    use oxigraph::sparql::QueryResults;
    let triples: Vec<oxigraph::model::Triple> = match store.query(construct) {
        Ok(QueryResults::Graph(triples)) => triples.flatten().collect(),
        _ => Vec::new(),
    };
    crate::sources::turtle::turtle_of(&triples, crate::sources::turtle::fixed(&PROV_PREFIXES))
}

/// A datasource's whole PROV-O trail as Turtle: every run activity and every
/// rollback that names it, plus the graphs those runs generated.
///
/// The counterpart of [`provenance_turtle`] for a single run, and the reason
/// both exist: these records live in `urn:system:sources`, which belongs to no
/// dataset and is therefore outside a caller's SPARQL scope.
pub fn source_provenance_turtle(store: &TripleStore, source_id: &str) -> String {
    let source = format!(
        "<{}>",
        crate::store::escape_sparql_iri(&source_iri(source_id))
    );
    let ds = crate::sources::model::DS;
    let query = format!(
        "PREFIX ds: <{ds}>\n\
         PREFIX prov: <http://www.w3.org/ns/prov#>\n\
         CONSTRUCT {{ ?s ?p ?o }} WHERE {{ GRAPH <{graph}> {{\n\
           {{ ?s ds:source {source} ; ?p ?o }}\n\
           UNION {{ ?a ds:source {source} ; prov:generated ?s . ?s ?p ?o }}\n\
         }} }}",
        graph = SOURCES_GRAPH,
    );
    prov_turtle(store, &query)
}

/// Operational counters for the metrics endpoint.
pub fn metrics(store: &TripleStore) -> serde_json::Value {
    let sources = registry::list_sources(store);
    let runs = registry::list_runs(store, None);
    let mut succeeded = 0u64;
    let mut rejected = 0u64;
    let mut failed = 0u64;
    let mut rows = 0u64;
    let mut triples = 0u64;
    let mut duration = 0u64;
    for r in &runs {
        match r.status {
            Some(RunStatus::Succeeded) => succeeded += 1,
            Some(RunStatus::Rejected) => rejected += 1,
            _ => failed += 1,
        }
        rows += r.rows_extracted;
        triples += r.triples_produced;
        duration += r.duration_ms;
    }
    let gated: Vec<&RunRecord> = runs.iter().filter(|r| r.conforms.is_some()).collect();
    let pass_rate = if gated.is_empty() {
        None
    } else {
        Some(gated.iter().filter(|r| r.conforms == Some(true)).count() as f64 / gated.len() as f64)
    };
    serde_json::json!({
        "sources": sources.len(),
        "sourcesInProduction": sources.iter().filter(|s| s.production.is_some()).count(),
        "mappings": registry::list_mappings(store, None).len(),
        "runs": {
            "total": runs.len(),
            "succeeded": succeeded,
            "rejected": rejected,
            "failed": failed,
        },
        "rowsExtracted": rows,
        "triplesProduced": triples,
        "totalDurationMs": duration,
        "shaclPassRate": pass_rate,
        "reviewQueueDepth": super::review::queue_depth(store),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubbing_removes_the_credential_and_the_location() {
        let source = SqlSource {
            id: "legacy".into(),
            dialect: "postgresql".into(),
            host: Some("db.internal".into()),
            database: "assets_production".into(),
            username: Some("reader_account".into()),
            ..Default::default()
        };
        let secret = Secret::new("hunter2-the-password");
        let raw = "FATAL: password authentication failed for user \"reader_account\" \
                   connecting to assets_production at db.internal with hunter2-the-password";
        let out = scrub(raw, &source, Some(&secret));
        for leak in [
            "hunter2-the-password",
            "db.internal",
            "assets_production",
            "reader_account",
        ] {
            assert!(!out.contains(leak), "{leak} survived: {out}");
        }
        assert!(out.contains("password authentication failed"), "{out}");
    }

    #[test]
    fn scrubbing_catches_a_file_name_on_its_own() {
        let source = SqlSource {
            dialect: "sqlite".into(),
            database: "/var/lib/ots/sources/legacy-9f1c.db".into(),
            ..Default::default()
        };
        let out = scrub("unable to open database file legacy-9f1c.db", &source, None);
        assert!(!out.contains("9f1c"), "{out}");
    }

    #[test]
    fn an_actor_iri_yields_the_user_id_the_commit_log_wants() {
        assert_eq!(actor_id(Some("http://x/users/adm")).as_deref(), Some("adm"));
        assert_eq!(actor_id(None), None);
    }

    #[test]
    fn provenance_turtle_carries_the_activity_its_inputs_and_its_output() {
        let store = TripleStore::in_memory().unwrap();
        let run = RunRecord {
            id: "r1".into(),
            source_id: "legacy".into(),
            mapping_id: "m".into(),
            mapping_version: 2,
            mode: "full".into(),
            status: Some(RunStatus::Succeeded),
            graph: run_graph_iri("r1"),
            rows_extracted: 3,
            triples_produced: 9,
            started_at: registry::now(),
            ended_at: registry::now(),
            actor: Some("http://x/users/adm".into()),
            ..Default::default()
        };
        registry::put_run(&store, &run).unwrap();
        let ttl = provenance_turtle(&store, &run);
        // Prefixed Turtle: the PROV vocabulary reads as CURIEs, the store's
        // own `urn:` identifiers stay whole.
        for expected in [
            "@prefix prov: <http://www.w3.org/ns/prov#>",
            "<urn:run:r1:activity>",
            "prov:Activity",
            "prov:used",
            "<urn:source:legacy>",
            "<urn:mapping:m:version:2>",
            "prov:generated",
            "<urn:run:r1>",
            "http://x/users/adm",
        ] {
            assert!(ttl.contains(expected), "missing {expected} in:\n{ttl}");
        }
        assert!(!ttl.contains("prov#Activity>"), "written in full: {ttl}");
        // A run nobody recorded produces nothing at all.
        let empty = provenance_turtle(
            &store,
            &RunRecord {
                id: "nope".into(),
                graph: run_graph_iri("nope"),
                ..Default::default()
            },
        );
        assert!(empty.is_empty(), "{empty}");
    }

    #[test]
    fn source_provenance_covers_runs_and_rollbacks() {
        let store = TripleStore::in_memory().unwrap();
        let run = RunRecord {
            id: "r1".into(),
            source_id: "legacy".into(),
            mapping_id: "m".into(),
            mapping_version: 1,
            mode: "full".into(),
            status: Some(RunStatus::Succeeded),
            graph: run_graph_iri("r1"),
            started_at: registry::now(),
            ended_at: registry::now(),
            ..Default::default()
        };
        registry::put_run(&store, &run).unwrap();
        registry::record_rollback(
            &store,
            "legacy",
            &RunPointer {
                graph: run_graph_iri("r2"),
                run: "r2".into(),
            },
            &RunPointer {
                graph: run_graph_iri("r1"),
                run: "r1".into(),
            },
            Some("http://x/users/adm"),
        )
        .unwrap();

        let ttl = source_provenance_turtle(&store, "legacy");
        assert!(ttl.contains("ds:Run"), "{ttl}");
        assert!(ttl.contains("ds:Rollback"), "{ttl}");
        assert!(ttl.contains("<urn:run:r1:activity>"), "{ttl}");
        // The generated graph's own entity triples come along.
        assert!(
            ttl.contains("<urn:run:r1>") && ttl.contains("prov:wasGeneratedBy"),
            "{ttl}"
        );
        // Another source's trail is not mixed in.
        assert!(!source_provenance_turtle(&store, "other").contains("ds:Run"));
    }

    #[test]
    fn metrics_are_zero_on_an_empty_store_and_never_divide_by_zero() {
        let store = TripleStore::in_memory().unwrap();
        let m = metrics(&store);
        assert_eq!(m["sources"], 0);
        assert_eq!(m["runs"]["total"], 0);
        assert_eq!(m["rowsExtracted"], 0);
        assert!(m["shaclPassRate"].is_null(), "no gated runs means no rate");
    }

    #[test]
    fn metrics_summarise_run_outcomes() {
        let store = TripleStore::in_memory().unwrap();
        let base = RunRecord {
            source_id: "s".into(),
            mapping_id: "m".into(),
            mapping_version: 1,
            mode: "full".into(),
            started_at: registry::now(),
            ended_at: registry::now(),
            rows_extracted: 10,
            triples_produced: 40,
            duration_ms: 5,
            ..Default::default()
        };
        registry::put_mapping(
            &store,
            &MappingRecord {
                id: "m".into(),
                title: "m".into(),
                source_id: "s".into(),
                version: 1,
                state: MappingState::Approved,
                shapes_graph: None,
                model: None,
                model_version: None,
                profile_version: None,
                created_by: None,
                created_at: registry::now(),
                updated_at: registry::now(),
            },
        )
        .unwrap();
        registry::put_run(
            &store,
            &RunRecord {
                id: "a".into(),
                graph: run_graph_iri("a"),
                status: Some(RunStatus::Succeeded),
                conforms: Some(true),
                ..base.clone()
            },
        )
        .unwrap();
        registry::put_run(
            &store,
            &RunRecord {
                id: "b".into(),
                graph: run_graph_iri("b"),
                status: Some(RunStatus::Rejected),
                conforms: Some(false),
                violations: 3,
                ..base
            },
        )
        .unwrap();
        let m = metrics(&store);
        assert_eq!(m["runs"]["total"], 2);
        assert_eq!(m["runs"]["succeeded"], 1);
        assert_eq!(m["runs"]["rejected"], 1);
        assert_eq!(m["rowsExtracted"], 20);
        assert_eq!(m["triplesProduced"], 80);
        assert_eq!(m["shaclPassRate"], 0.5);
    }
}
