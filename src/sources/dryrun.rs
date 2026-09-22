//! Dry-run: a sample of a mapping, materialised into a scratch graph,
//! validated, and every violation classified.
//!
//! **Two kinds of violation.** A shape that fails on nearly every subject of
//! a type is not telling you about the data; it is telling you the mapping
//! produced the wrong term for that property — a literal where the shape
//! wants an IRI, `xsd:string` where it wants `xsd:decimal`. One that fails on
//! a few subjects is a fact about those rows. The classifier separates the
//! two by share: a violation hitting at least `systematicShare` of a type's
//! subjects, over at least `systematicMinSubjects` of them, is a **mapping
//! defect**; anything sparser is a **data issue**. Both numbers come from the
//! mapping gates ([`super::gates`]), so the proposer and the reviewer read
//! the same rule.
//!
//! **Why the classifier is here.** It needs the shapes graph, the sample graph
//! and the mapping in one scope, and it needs the sample closed under its
//! joins ([`crate::rml::sample`]) — otherwise a one-row preview of a child
//! table fakes an `sh:class` violation on every reference. The store has all
//! three; nothing outside it does.
//!
//! **Scratch graphs.** A dry-run writes `urn:dryrun:<id>` and keeps it for
//! `OTS_DRYRUN_TTL_SECS` (default fifteen minutes) so a reviewer can look at
//! the produced triples, then drops it. Nothing about a dry-run is promoted,
//! recorded as a run, or published.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::middleware::AuthenticatedUser;
use crate::rml::sample::{execute_sample, SampleSpec, SampledMap, MAX_SAMPLE_ROWS};
use crate::server::AppState;
use crate::shacl::report::{Severity, ValidationReport, ValidationResult};
use crate::store::{escape_sparql_iri, TripleStore};

use super::gates::MappingGates;
use super::model::*;
use super::{connector, mappings, registry, runs};

/// Every scratch graph a dry-run writes starts with this.
pub const SCRATCH_PREFIX: &str = "urn:dryrun:";
const TTL_ENV: &str = "OTS_DRYRUN_TTL_SECS";
const DEFAULT_TTL_SECS: u64 = 15 * 60;
const DEFAULT_SAMPLE: usize = 20;
/// Focus nodes listed per finding; the rest are counted.
const FOCUS_NODES_LISTED: usize = 20;
/// Entities described in the response; the graph holds them all.
const ENTITIES_LISTED: usize = 500;

fn ttl() -> Duration {
    Duration::from_secs(
        std::env::var(TTL_ENV)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(DEFAULT_TTL_SECS),
    )
}

pub fn scratch_graph_iri(id: &str) -> String {
    format!("{SCRATCH_PREFIX}{id}")
}

// ───────────────────────────── Expiry ─────────────────────────────

/// Scratch graphs this process wrote, with the moment each expires.
static SCRATCH: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

fn remember(graph: &str) {
    let mut held = SCRATCH.lock().unwrap_or_else(|e| e.into_inner());
    held.push((graph.to_string(), Instant::now() + ttl()));
}

/// Drop every scratch graph whose time is up. Called on each dry-run, so a
/// deployment that never dry-runs pays nothing, and one that does keeps at
/// most a TTL's worth of scratch.
pub fn sweep(store: &TripleStore) {
    let expired: Vec<String> = {
        let mut held = SCRATCH.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let (gone, kept): (Vec<_>, Vec<_>) = held.drain(..).partition(|(_, at)| *at <= now);
        *held = kept;
        gone.into_iter().map(|(g, _)| g).collect()
    };
    if expired.is_empty() {
        return;
    }
    let refs: Vec<&str> = expired.iter().map(String::as_str).collect();
    if let Err(e) = store.bulk_delete_graphs(&refs) {
        tracing::warn!("dry-run scratch graphs not dropped: {e}");
    }
}

/// Drop every scratch graph left behind by an earlier process. A scratch
/// graph is a system graph nobody can reach through a dataset, so one that
/// outlived its TTL because the server restarted is only occupying space.
pub fn sweep_leftovers(store: &TripleStore) {
    let leftovers: Vec<String> = match store.named_graphs() {
        Ok(graphs) => graphs
            .into_iter()
            .map(|g| g.as_str().to_string())
            .filter(|g| g.starts_with(SCRATCH_PREFIX))
            .collect(),
        Err(e) => {
            tracing::warn!("could not list graphs to sweep dry-run scratch: {e}");
            return;
        }
    };
    if leftovers.is_empty() {
        return;
    }
    let refs: Vec<&str> = leftovers.iter().map(String::as_str).collect();
    match store.bulk_delete_graphs(&refs) {
        Ok(()) => tracing::info!(
            "dropped {} dry-run scratch graph(s) left by an earlier process",
            leftovers.len()
        ),
        Err(e) => tracing::warn!("dry-run scratch graphs not dropped: {e}"),
    }
}

// ───────────────────────────── Request ─────────────────────────────

/// What to dry-run. Exactly one of `mapping`, `mappingGraph`, `rml` and
/// `yarrrml` names the mapping; the rest narrows the sample and names the
/// shapes.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DryRunRequest {
    /// A registered mapping, by id or IRI; its newest version unless
    /// `version` says otherwise.
    pub mapping: Option<String>,
    pub version: Option<u32>,
    /// A mapping version graph, `urn:mapping:<id>:version:<n>`.
    pub mapping_graph: Option<String>,
    /// An unregistered mapping — what the proposer sends before it writes a
    /// proposal, and what the Studio editor sends between saves.
    pub rml: Option<String>,
    pub yarrrml: Option<String>,
    /// The shapes to validate against. Explicit, else the registered
    /// mapping's, else the model version's.
    pub shapes_graph: Option<String>,
    pub model: Option<String>,
    pub model_version: Option<String>,
    /// Sample only the triples maps reading this table…
    pub table: Option<String>,
    /// …or these triples maps, by IRI. Rows they reference through a join are
    /// pulled in regardless.
    #[serde(default)]
    pub triples_maps: Vec<String>,
    /// Rows per sampled triples map. Default 20, at most 1 000.
    pub sample_size: Option<usize>,
}

// ───────────────────────────── Response ─────────────────────────────

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub conforms: bool,
    pub results_count: usize,
    pub results: Vec<ValidationResult>,
}

/// One kind of violation, with how much of its type it covers.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub shape: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub constraint: String,
    pub severity: Severity,
    pub message: String,
    /// Distinct focus nodes the violation hit.
    pub affected: u64,
    /// Subjects of the affected nodes' types in the sample.
    pub population: u64,
    pub share: f64,
    /// The first few focus nodes; `affected` counts them all.
    pub focus_nodes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    /// The rule applied, from the mapping gates.
    pub systematic_share: f64,
    pub systematic_min_subjects: u64,
    /// Violations that hit a type wholesale: the mapping is wrong for them.
    pub mapping_defects: Vec<Finding>,
    /// Violations on a few subjects: facts about those rows.
    pub data_issues: Vec<Finding>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Entity {
    pub subject: String,
    pub types: Vec<String>,
    /// The entity's own triples and its blank-node closure, as Turtle.
    pub turtle: String,
    pub violations: Vec<ValidationResult>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DryRunResponse {
    pub id: String,
    /// The scratch graph, readable until `expiresAt`.
    pub graph: String,
    pub expires_at: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping: Option<MappingRef>,
    pub sample_size: usize,
    pub rows: u64,
    pub triples: u64,
    pub maps: Vec<SampledMap>,
    pub shapes_graphs: Vec<String>,
    /// Absent when no shapes graph applied — then nothing was validated,
    /// and the classification is empty for that reason, not because the
    /// sample conforms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<ReportSummary>,
    pub classification: Classification,
    pub entities: Vec<Entity>,
    pub warnings: Vec<String>,
}

// ───────────────────────────── Classification ─────────────────────────────

/// Split a report's results into mapping defects and data issues.
///
/// Results are grouped by what fired — shape, path and constraint — and each
/// group is measured against the subjects of its focus nodes' types in the
/// sample graph. A group with no typed focus node is measured against every
/// IRI subject in the graph, which is the widest reading and therefore the
/// one least likely to call a sparse problem systematic.
pub fn classify(
    store: &TripleStore,
    graph: &str,
    results: &[ValidationResult],
    gates: &MappingGates,
) -> Classification {
    let mut classification = Classification {
        systematic_share: gates.systematic_share,
        systematic_min_subjects: gates.systematic_min_subjects,
        ..Default::default()
    };
    let mut groups: BTreeMap<(String, Option<String>, String), Vec<&ValidationResult>> =
        BTreeMap::new();
    for r in results {
        groups
            .entry((
                r.source_shape.clone(),
                r.path.clone(),
                r.source_constraint.clone(),
            ))
            .or_default()
            .push(r);
    }
    let all_subjects = subjects(store, graph, None);
    for ((shape, path, constraint), hits) in groups {
        let focus: BTreeSet<&str> = hits.iter().map(|r| r.focus_node.as_str()).collect();
        let types = types_of(store, graph, focus.iter().copied());
        let population: BTreeSet<String> = if types.is_empty() {
            all_subjects.clone()
        } else {
            subjects(store, graph, Some(&types))
        };
        let mut population: BTreeSet<&str> = population.iter().map(String::as_str).collect();
        population.extend(focus.iter().copied());
        let affected = focus.len() as u64;
        let total = population.len().max(1) as u64;
        let share = affected as f64 / total as f64;
        let systematic =
            affected >= gates.systematic_min_subjects && share >= gates.systematic_share;
        let first = hits[0];
        let finding = Finding {
            shape,
            path,
            constraint,
            severity: first.severity.clone(),
            message: first.message.clone(),
            affected,
            population: total,
            share,
            focus_nodes: focus
                .iter()
                .take(FOCUS_NODES_LISTED)
                .map(|s| s.to_string())
                .collect(),
        };
        if systematic {
            classification.mapping_defects.push(finding);
        } else {
            classification.data_issues.push(finding);
        }
    }
    classification
}

/// The `rdf:type`s of `nodes` in `graph`.
fn types_of<'a>(
    store: &TripleStore,
    graph: &str,
    nodes: impl Iterator<Item = &'a str>,
) -> BTreeSet<String> {
    use oxigraph::sparql::QueryResults;
    let values: Vec<String> = nodes
        .filter_map(|n| oxigraph::model::NamedNode::new(n).ok())
        .map(|n| n.to_string())
        .collect();
    if values.is_empty() {
        return BTreeSet::new();
    }
    let q = format!(
        "SELECT DISTINCT ?t WHERE {{ GRAPH <{}> {{ VALUES ?f {{ {} }} ?f a ?t }} }}",
        escape_sparql_iri(graph),
        values.join(" ")
    );
    let Ok(QueryResults::Solutions(sols)) = store.query(&q) else {
        return BTreeSet::new();
    };
    sols.flatten()
        .filter_map(|b| match b.get("t") {
            Some(oxigraph::model::Term::NamedNode(n)) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect()
}

/// The IRI subjects of `graph`: those of `types`, or every one.
fn subjects(
    store: &TripleStore,
    graph: &str,
    types: Option<&BTreeSet<String>>,
) -> BTreeSet<String> {
    use oxigraph::sparql::QueryResults;
    let g = escape_sparql_iri(graph);
    let q = match types {
        Some(types) if !types.is_empty() => format!(
            "SELECT DISTINCT ?s WHERE {{ GRAPH <{g}> {{ VALUES ?t {{ {} }} ?s a ?t }} FILTER(isIRI(?s)) }}",
            types
                .iter()
                .map(|t| format!("<{}>", escape_sparql_iri(t)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        _ => format!("SELECT DISTINCT ?s WHERE {{ GRAPH <{g}> {{ ?s ?p ?o }} FILTER(isIRI(?s)) }}"),
    };
    let Ok(QueryResults::Solutions(sols)) = store.query(&q) else {
        return BTreeSet::new();
    };
    sols.flatten()
        .filter_map(|b| match b.get("s") {
            Some(oxigraph::model::Term::NamedNode(n)) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect()
}

/// The entities of the scratch graph, each with its own Turtle and the
/// violations that name it.
fn entities(state: &AppState, graph: &str, results: &[ValidationResult]) -> Vec<Entity> {
    let mut by_focus: HashMap<&str, Vec<ValidationResult>> = HashMap::new();
    for r in results {
        by_focus
            .entry(r.focus_node.as_str())
            .or_default()
            .push(r.clone());
    }
    let mut out = Vec::new();
    for subject in subjects(&state.store, graph, None)
        .into_iter()
        .take(ENTITIES_LISTED)
    {
        let nt = crate::ldes::capture::describe_entity(&state.store, graph, &subject);
        let turtle =
            super::turtle::ntriples_to_turtle(&nt, |ns| state.prefix_registry.declaration_for(ns));
        let types = types_of(&state.store, graph, std::iter::once(subject.as_str()))
            .into_iter()
            .collect();
        out.push(Entity {
            violations: by_focus.remove(subject.as_str()).unwrap_or_default(),
            subject,
            types,
            turtle,
        });
    }
    out
}

// ───────────────────────────── The mapping ─────────────────────────────

/// The mapping to run, in one of its four forms.
struct Chosen {
    rml: crate::rml::model::RmlMapping,
    record: Option<MappingRecord>,
    version: Option<u32>,
}

fn choose(
    state: &AppState,
    source: &SqlSource,
    body: &DryRunRequest,
) -> Result<Chosen, (StatusCode, String)> {
    let bad = |m: String| (StatusCode::BAD_REQUEST, m);
    let named = [
        body.mapping.is_some(),
        body.mapping_graph.is_some(),
        body.rml.as_deref().is_some_and(|r| !r.trim().is_empty()),
        body.yarrrml
            .as_deref()
            .is_some_and(|y| !y.trim().is_empty()),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if named != 1 {
        return Err(bad(
            "name the mapping to dry-run in exactly one of 'mapping', 'mappingGraph', 'rml' or \
             'yarrrml'"
                .to_string(),
        ));
    }

    // A registered mapping, by id or by version graph.
    let registered: Option<(String, Option<u32>)> = if let Some(m) = &body.mapping {
        Some((
            m.trim().trim_start_matches("urn:mapping:").to_string(),
            body.version,
        ))
    } else if let Some(g) = &body.mapping_graph {
        let rest = g
            .trim()
            .strip_prefix("urn:mapping:")
            .ok_or_else(|| bad(format!("'{g}' is not a mapping version graph")))?;
        let (id, v) = rest
            .rsplit_once(":version:")
            .ok_or_else(|| bad(format!("'{g}' names no version")))?;
        let v: u32 = v
            .parse()
            .map_err(|_| bad(format!("'{g}' names no version")))?;
        Some((id.to_string(), Some(v)))
    } else {
        None
    };

    if let Some((id, version)) = registered {
        let record = registry::get_mapping(&state.store, &id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, format!("mapping '{id}' not found")))?;
        if record.source_id != source.id {
            return Err(bad(format!(
                "mapping '{id}' is registered against datasource '{}', not '{}'",
                record.source_id, source.id
            )));
        }
        let version = version.unwrap_or(record.version);
        if version == 0 || version > record.version {
            return Err(bad(format!(
                "mapping '{id}' has versions 1..{}",
                record.version
            )));
        }
        let rml = mappings::load(&state.store, &id, version).map_err(|e| bad(e.to_string()))?;
        return Ok(Chosen {
            rml,
            record: Some(record),
            version: Some(version),
        });
    }

    // Inline: what the proposer sends before it writes a proposal.
    let turtle = match (&body.rml, &body.yarrrml) {
        (Some(r), _) if !r.trim().is_empty() => r.clone(),
        (_, Some(y)) => super::yarrrml::to_rml(y, Some(&source.id))
            .map_err(|e| bad(mappings::MappingError::Yarrrml(e.to_string()).to_string()))?,
        _ => unreachable!("counted above"),
    };
    let rml = mappings::validate_rml(&turtle, &source.id).map_err(|e| bad(e.to_string()))?;
    Ok(Chosen {
        rml,
        record: None,
        version: None,
    })
}

/// The shapes graphs the sample is validated against, with a warning for
/// each one that resolved to nothing.
fn shapes_for(
    state: &AppState,
    user: &AuthenticatedUser,
    body: &DryRunRequest,
    record: Option<&MappingRecord>,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(g) = body
        .shapes_graph
        .as_deref()
        .map(str::trim)
        .filter(|g| !g.is_empty())
    {
        out.push(g.to_string());
    } else if let Some(g) = record.and_then(|r| r.shapes_graph.clone()) {
        out.push(g);
    }
    let model = body
        .model
        .clone()
        .or_else(|| record.and_then(|r| r.model.clone()));
    let version = body
        .model_version
        .clone()
        .or_else(|| record.and_then(|r| r.model_version.clone()));
    if let (Some(model), Some(version)) = (model, version) {
        match crate::data_models::registry::get_version(
            &state.store,
            &state.base_url,
            &model,
            &version,
        ) {
            Some(v) => {
                let sources = crate::data_models::profile::shape_sources(
                    state,
                    Some(&user.user_id),
                    &model,
                    &v,
                );
                if sources.is_empty() {
                    warnings.push(format!(
                        "model '{model}' version '{version}' has no shapes graph bound to it"
                    ));
                }
                out.extend(sources.into_iter().map(|s| s.graph_iri));
            }
            None => warnings.push(format!(
                "model '{model}' has no version '{version}'; its shapes were not applied"
            )),
        }
    }
    out.sort();
    out.dedup();
    out.retain(|g| {
        let present = state.store.count_graph(Some(g)).unwrap_or(0) > 0;
        if !present {
            warnings.push(format!(
                "shapes graph <{g}> is empty or absent; nothing validated against it"
            ));
        }
        present
    });
    out
}

// ───────────────────────────── Handler ─────────────────────────────

/// `POST /api/sources/:id/dry-run`
pub async fn dry_run(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DryRunRequest>,
) -> Result<Json<DryRunResponse>, (StatusCode, String)> {
    let _permit = state.expensive_semaphore.acquire().await.map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Server overloaded".to_string(),
        )
    })?;
    let source = registry::get_source(&state.store, &id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("datasource '{id}' not found"),
        )
    })?;
    let sample_size = body
        .sample_size
        .unwrap_or(DEFAULT_SAMPLE)
        .clamp(1, MAX_SAMPLE_ROWS);
    let chosen = choose(&state, &source, &body)?;
    let mut warnings = Vec::new();
    let shapes_graphs = shapes_for(&state, &user, &body, chosen.record.as_ref(), &mut warnings);
    if shapes_graphs.is_empty() {
        warnings.push(
            "no shapes graph applies: name one in 'shapesGraph', or register the mapping with a \
             shapes graph or a model version"
                .to_string(),
        );
    }

    sweep(&state.store);
    let dry_id = uuid::Uuid::new_v4().to_string();
    let graph = scratch_graph_iri(&dry_id);
    let spec = SampleSpec {
        limit: sample_size,
        tables: body
            .table
            .iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        triples_maps: body.triples_maps.clone(),
    };

    let blocking_state = state.clone();
    let blocking_source = source.clone();
    let rml = chosen.rml;
    let blocking_graph = graph.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let connector = connector::get(&blocking_source.dialect).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("no connector for dialect '{}'", blocking_source.dialect),
            )
        })?;
        let mut conn = runs::connect(&blocking_source).map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
        let quote = |ident: &str| connector.quote_identifier(ident);
        execute_sample(
            &rml,
            conn.as_mut(),
            &quote,
            &blocking_state.store,
            &blocking_graph,
            &dry_id,
            &spec,
        )
        .map(|o| (o, dry_id))
        .map_err(|e| {
            let _ = blocking_state
                .store
                .bulk_delete_graphs(&[blocking_graph.as_str()]);
            let scrubbed = runs::scrub(&e, &blocking_source, None);
            let status = if scrubbed.contains("no triples map reads") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, scrubbed)
        })
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let (outcome, dry_id) = outcome?;
    remember(&graph);

    // ── Validate, then classify ──
    let (gates, _) = super::gates::load(&state.store);
    let data = vec![graph.clone()];
    let mut report: Option<ValidationReport> = None;
    for shapes in &shapes_graphs {
        match crate::shacl::engine::validate(&state.store, shapes, &data) {
            Ok(r) => {
                let merged = report.take().map_or_else(
                    || r.clone(),
                    |mut m| {
                        m.conforms &= r.conforms;
                        m.results_count += r.results_count;
                        m.results.extend(r.results.iter().cloned());
                        m
                    },
                );
                report = Some(merged);
            }
            Err(e) => warnings.push(format!(
                "shapes graph <{shapes}> could not be evaluated: {e}"
            )),
        }
    }
    let results: Vec<ValidationResult> = report
        .as_ref()
        .map(|r| r.results.clone())
        .unwrap_or_default();
    let classification = classify(&state.store, &graph, &results, &gates);
    let entities = entities(&state, &graph, &results);

    let expires_at =
        (chrono::Utc::now() + chrono::Duration::from_std(ttl()).unwrap_or_default()).to_rfc3339();
    Ok(Json(DryRunResponse {
        id: dry_id,
        graph,
        expires_at,
        source: source.iri(),
        mapping: chosen.record.as_ref().map(|r| MappingRef {
            id: r.id.clone(),
            version: chosen.version.unwrap_or(r.version),
            iri: mapping_version_iri(&r.id, chosen.version.unwrap_or(r.version)),
        }),
        sample_size,
        rows: outcome.rows,
        triples: outcome.triples,
        maps: outcome.maps,
        shapes_graphs,
        report: report.map(|r| ReportSummary {
            conforms: r.conforms,
            results_count: r.results_count,
            results: r.results,
        }),
        classification,
        entities,
        warnings,
    }))
}

pub fn routes() -> axum::Router<AppState> {
    axum::Router::new().route("/api/sources/:id/dry-run", axum::routing::post(dry_run))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::RdfFormat;

    fn result(shape: &str, path: &str, focus: &str) -> ValidationResult {
        ValidationResult {
            severity: Severity::Violation,
            focus_node: focus.to_string(),
            path: Some(format!("<{path}>")),
            value: None,
            source_shape: shape.to_string(),
            source_constraint: "sh:datatype".to_string(),
            message: "wrong datatype".to_string(),
        }
    }

    fn graph_with(n_products: usize, n_suppliers: usize) -> TripleStore {
        let store = TripleStore::in_memory().unwrap();
        let mut ttl = String::new();
        for i in 0..n_products {
            ttl.push_str(&format!(
                "<http://x/p{i}> a <http://x/Product> ; <http://x/name> \"p{i}\" .\n"
            ));
        }
        for i in 0..n_suppliers {
            ttl.push_str(&format!(
                "<http://x/s{i}> a <http://x/Supplier> ; <http://x/name> \"s{i}\" .\n"
            ));
        }
        store
            .load_str(&ttl, RdfFormat::Turtle, Some("urn:dryrun:t"))
            .unwrap();
        store
    }

    #[test]
    fn a_violation_on_every_subject_of_a_type_is_a_mapping_defect() {
        let store = graph_with(10, 3);
        let results: Vec<ValidationResult> = (0..10)
            .map(|i| {
                result(
                    "http://x/PriceShape",
                    "http://x/price",
                    &format!("http://x/p{i}"),
                )
            })
            .collect();
        let c = classify(&store, "urn:dryrun:t", &results, &MappingGates::default());
        assert_eq!(c.mapping_defects.len(), 1, "{c:?}");
        assert!(c.data_issues.is_empty());
        let f = &c.mapping_defects[0];
        assert_eq!((f.affected, f.population), (10, 10));
        assert_eq!(f.share, 1.0);
        assert_eq!(f.path.as_deref(), Some("<http://x/price>"));
    }

    #[test]
    fn a_violation_on_a_few_subjects_is_a_data_issue() {
        let store = graph_with(10, 3);
        let results = vec![
            result("http://x/NameShape", "http://x/name", "http://x/p3"),
            result("http://x/NameShape", "http://x/name", "http://x/p7"),
        ];
        let c = classify(&store, "urn:dryrun:t", &results, &MappingGates::default());
        assert!(c.mapping_defects.is_empty(), "{c:?}");
        assert_eq!(c.data_issues.len(), 1);
        let f = &c.data_issues[0];
        assert_eq!((f.affected, f.population), (2, 10));
        assert!((f.share - 0.2).abs() < 1e-9);
    }

    #[test]
    fn one_subject_is_never_systematic_even_when_it_is_the_whole_type() {
        // One row of a table: its one violation covers 100% of the type, and
        // the minimum-subjects rule keeps that from reading as a defect.
        let store = graph_with(1, 0);
        let results = vec![result("http://x/S", "http://x/name", "http://x/p0")];
        let c = classify(&store, "urn:dryrun:t", &results, &MappingGates::default());
        assert!(c.mapping_defects.is_empty(), "{c:?}");
        assert_eq!(c.data_issues.len(), 1);
    }

    #[test]
    fn the_share_is_measured_against_the_focus_nodes_own_type() {
        // Three suppliers all failing is systematic for Supplier, whatever
        // the ten products are doing.
        let store = graph_with(10, 3);
        let results: Vec<ValidationResult> = (0..3)
            .map(|i| {
                result(
                    "http://x/SupShape",
                    "http://x/name",
                    &format!("http://x/s{i}"),
                )
            })
            .collect();
        let c = classify(&store, "urn:dryrun:t", &results, &MappingGates::default());
        assert_eq!(c.mapping_defects.len(), 1, "{c:?}");
        assert_eq!(c.mapping_defects[0].population, 3);
    }

    #[test]
    fn the_thresholds_come_from_the_gates() {
        let store = graph_with(10, 0);
        let results: Vec<ValidationResult> = (0..5)
            .map(|i| result("http://x/S", "http://x/name", &format!("http://x/p{i}")))
            .collect();
        assert!(
            classify(&store, "urn:dryrun:t", &results, &MappingGates::default())
                .mapping_defects
                .is_empty()
        );
        let lowered = MappingGates {
            systematic_share: 0.5,
            ..Default::default()
        };
        assert_eq!(
            classify(&store, "urn:dryrun:t", &results, &lowered)
                .mapping_defects
                .len(),
            1
        );
    }

    #[test]
    fn scratch_graphs_expire_and_leftovers_are_swept_at_boot() {
        let store = TripleStore::in_memory().unwrap();
        for g in ["urn:dryrun:old", "urn:dryrun:older", "urn:run:keep"] {
            store
                .load_str(
                    "<http://x/s> <http://x/p> <http://x/o> .",
                    RdfFormat::Turtle,
                    Some(g),
                )
                .unwrap();
        }
        sweep_leftovers(&store);
        assert_eq!(store.count_graph(Some("urn:dryrun:old")).unwrap(), 0);
        assert_eq!(store.count_graph(Some("urn:dryrun:older")).unwrap(), 0);
        assert_eq!(
            store.count_graph(Some("urn:run:keep")).unwrap(),
            1,
            "not scratch"
        );

        // An in-process registration expires by TTL.
        store
            .load_str(
                "<http://x/s> <http://x/p> <http://x/o> .",
                RdfFormat::Turtle,
                Some("urn:dryrun:ttl"),
            )
            .unwrap();
        {
            let mut held = SCRATCH.lock().unwrap();
            held.push((
                "urn:dryrun:ttl".to_string(),
                Instant::now() - Duration::from_secs(1),
            ));
            held.push((
                "urn:dryrun:fresh".to_string(),
                Instant::now() + Duration::from_secs(3600),
            ));
        }
        sweep(&store);
        assert_eq!(store.count_graph(Some("urn:dryrun:ttl")).unwrap(), 0);
        let held = SCRATCH.lock().unwrap();
        assert!(held.iter().any(|(g, _)| g == "urn:dryrun:fresh"));
        assert!(!held.iter().any(|(g, _)| g == "urn:dryrun:ttl"));
    }
}
