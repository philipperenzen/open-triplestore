//! Drift: what changed in a datasource between two profile versions, and
//! whether the model a mapping targets has moved on.
//!
//! A profile version is a graph whose nodes have stable IRIs, so two versions
//! line up table for table and column for column. Drift is read off that
//! alignment: a column present in one version and not the other, a native or
//! generic type that differs, a code list whose value distribution moved —
//! measured as the KL divergence of the newer distribution from the older,
//! against a threshold from the mapping gates — and a structural hash that
//! moved. Separately, a mapping registered against a model version is behind
//! when that model has published a newer one.
//!
//! **The baseline.** Drift is always relative to something. Given a mapping,
//! the baseline is the profile version the mapping was registered or approved
//! against (`ds:profileVersion` on the record); without one, the previous
//! profile version. Either can be overridden by the caller.
//!
//! **Tickets.** A finding is only useful if someone acts on it. A drift run
//! with anything to report opens one re-map ticket for the (datasource,
//! mapping) pair — a model bump opens one batched ticket listing every table,
//! never one per table — and a later run updates that ticket rather than
//! opening another beside it. Tickets are RDF in `urn:system:sources`, with
//! the run and rollback records, and close explicitly.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use oxigraph::sparql::QueryResults;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;
use crate::store::{escape_sparql_iri, escape_sparql_literal, TripleStore};

use super::model::*;
use super::profile::{self, PROF, PROF_LABEL};
use super::registry;

const CSVW: &str = "http://www.w3.org/ns/csvw#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const DCT: &str = "http://purl.org/dc/terms/";
const PROV: &str = "http://www.w3.org/ns/prov#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn prefixes() -> String {
    format!(
        "PREFIX csvw: <{CSVW}>\nPREFIX {PROF_LABEL}: <{PROF}>\nPREFIX rdf: <{RDF}>\n\
         PREFIX dct: <{DCT}>\nPREFIX prov: <{PROV}>\nPREFIX xsd: <{XSD}>\nPREFIX ds: <{DS}>\n"
    )
}

// ───────────────────────────── Reading a profile ─────────────────────────────

/// One table of one profile version, as much of it as drift compares.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableShape {
    pub structural_hash: String,
    /// column → (native type, XSD datatype)
    pub columns: BTreeMap<String, (String, Option<String>)>,
    /// column → value → occurrences, for code-list columns only
    pub top_values: BTreeMap<String, BTreeMap<String, u64>>,
}

/// Every table of a profile version, by name.
pub fn read_profile(
    store: &TripleStore,
    source_id: &str,
    version: u32,
) -> BTreeMap<String, TableShape> {
    let graph = escape_sparql_iri(&profile::profile_version_graph_iri(source_id, version));
    let mut out: BTreeMap<String, TableShape> = BTreeMap::new();

    let structure = format!(
        "{}SELECT ?table ?hash ?name ?native ?dt WHERE {{ GRAPH <{graph}> {{\n\
           ?t a csvw:Table ; dct:title ?table ; {PROF_LABEL}:structuralHash ?hash ; csvw:tableSchema ?schema .\n\
           OPTIONAL {{ ?schema csvw:column ?c . ?c csvw:name ?name ; {PROF_LABEL}:nativeType ?native .\n\
                      OPTIONAL {{ ?c csvw:datatype ?dt }} }}\n\
         }} }}",
        prefixes()
    );
    if let Ok(QueryResults::Solutions(rows)) = store.query(&structure) {
        for row in rows.flatten() {
            let (Some(table), Some(hash)) = (lex(&row, "table"), lex(&row, "hash")) else {
                continue;
            };
            let entry = out.entry(table).or_default();
            entry.structural_hash = hash;
            if let (Some(name), Some(native)) = (lex(&row, "name"), lex(&row, "native")) {
                entry.columns.insert(name, (native, lex(&row, "dt")));
            }
        }
    }

    let values = format!(
        "{}SELECT ?table ?name ?value ?n WHERE {{ GRAPH <{graph}> {{\n\
           ?t a csvw:Table ; dct:title ?table ; csvw:tableSchema/csvw:column ?c .\n\
           ?c csvw:name ?name ; {PROF_LABEL}:topValue ?tv . ?tv rdf:value ?value ; {PROF_LABEL}:occurrences ?n\n\
         }} }}",
        prefixes()
    );
    if let Ok(QueryResults::Solutions(rows)) = store.query(&values) {
        for row in rows.flatten() {
            let (Some(table), Some(name), Some(value), Some(n)) = (
                lex(&row, "table"),
                lex(&row, "name"),
                lex(&row, "value"),
                lex(&row, "n").and_then(|n| n.parse::<u64>().ok()),
            ) else {
                continue;
            };
            out.entry(table)
                .or_default()
                .top_values
                .entry(name)
                .or_default()
                .insert(value, n);
        }
    }
    out
}

fn lex(row: &oxigraph::sparql::QuerySolution, var: &str) -> Option<String> {
    match row.get(var)? {
        oxigraph::model::Term::NamedNode(n) => Some(n.as_str().to_string()),
        oxigraph::model::Term::Literal(l) => Some(l.value().to_string()),
        _ => None,
    }
}

// ───────────────────────────── The comparison ─────────────────────────────

/// KL divergence `D(P ‖ Q)` of the newer distribution from the older, both
/// smoothed by one count over the union of their values, so a value one side
/// never saw contributes a large but finite term rather than infinity.
pub fn kl_divergence(newer: &BTreeMap<String, u64>, older: &BTreeMap<String, u64>) -> f64 {
    let values: BTreeSet<&String> = newer.keys().chain(older.keys()).collect();
    if values.is_empty() {
        return 0.0;
    }
    let smooth = |counts: &BTreeMap<String, u64>, v: &String| -> f64 {
        let total: u64 = counts.values().sum();
        (counts.get(v).copied().unwrap_or(0) as f64 + 1.0) / (total as f64 + values.len() as f64)
    };
    values
        .iter()
        .map(|v| {
            let p = smooth(newer, v);
            let q = smooth(older, v);
            p * (p / q).ln()
        })
        .sum()
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TypeChange {
    pub column: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DistributionShift {
    pub column: String,
    pub kl_divergence: f64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CodeListChange {
    pub column: String,
    /// `gained` when the column became a code list, `lost` when it stopped
    /// being one — its cardinality grew past what a value map covers.
    pub change: &'static str,
}

#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TableDrift {
    pub table: String,
    pub structural_hash_changed: bool,
    pub new_columns: Vec<String>,
    pub removed_columns: Vec<String>,
    pub type_changes: Vec<TypeChange>,
    pub distribution_shifts: Vec<DistributionShift>,
    pub code_list_changes: Vec<CodeListChange>,
    /// Whether anything above applies: the table needs a look.
    pub affected: bool,
}

/// Compare one table across two versions.
pub fn compare_table(
    name: &str,
    older: &TableShape,
    newer: &TableShape,
    kl_threshold: f64,
) -> TableDrift {
    let mut d = TableDrift {
        table: name.to_string(),
        structural_hash_changed: older.structural_hash != newer.structural_hash,
        ..Default::default()
    };
    for (column, (native, dt)) in &newer.columns {
        match older.columns.get(column) {
            None => d.new_columns.push(column.clone()),
            Some((old_native, old_dt)) if old_native != native || old_dt != dt => {
                d.type_changes.push(TypeChange {
                    column: column.clone(),
                    from: describe_type(old_native, old_dt.as_deref()),
                    to: describe_type(native, dt.as_deref()),
                });
            }
            _ => {}
        }
    }
    for column in older.columns.keys() {
        if !newer.columns.contains_key(column) {
            d.removed_columns.push(column.clone());
        }
    }
    for (column, values) in &newer.top_values {
        match older.top_values.get(column) {
            Some(old_values) => {
                let kl = kl_divergence(values, old_values);
                if kl > kl_threshold {
                    d.distribution_shifts.push(DistributionShift {
                        column: column.clone(),
                        kl_divergence: kl,
                    });
                }
            }
            None if older.columns.contains_key(column) => {
                d.code_list_changes.push(CodeListChange {
                    column: column.clone(),
                    change: "gained",
                })
            }
            None => {}
        }
    }
    for column in older.top_values.keys() {
        if !newer.top_values.contains_key(column) && newer.columns.contains_key(column) {
            d.code_list_changes.push(CodeListChange {
                column: column.clone(),
                change: "lost",
            });
        }
    }
    d.affected = d.structural_hash_changed
        || !d.new_columns.is_empty()
        || !d.removed_columns.is_empty()
        || !d.type_changes.is_empty()
        || !d.distribution_shifts.is_empty()
        || !d.code_list_changes.is_empty();
    d
}

fn describe_type(native: &str, dt: Option<&str>) -> String {
    match dt {
        Some(dt) => format!("{native} ({})", dt.rsplit('#').next().unwrap_or(dt)),
        None => native.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelBump {
    pub model: String,
    /// The version the mapping targets.
    pub mapping_version: String,
    /// The newest published version of the model.
    pub latest_version: String,
}

/// A bump is a published model version newer than the one the mapping targets.
pub fn model_bump(
    model: &str,
    mapping_version: &str,
    latest_published: Option<&str>,
) -> Option<ModelBump> {
    let latest = latest_published?;
    (latest != mapping_version).then(|| ModelBump {
        model: model.to_string(),
        mapping_version: mapping_version.to_string(),
        latest_version: latest.to_string(),
    })
}

// ───────────────────────────── Tickets ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Ticket {
    pub id: String,
    pub iri: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping: Option<String>,
    /// `open` or `closed`.
    pub status: String,
    /// `schema-drift`, `model-version-bump` or `both`.
    pub reason: String,
    pub affected_tables: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_profile: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_profile: Option<u32>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
}

pub fn ticket_iri(id: &str) -> String {
    format!("urn:ticket:{id}")
}

fn ticket_select(filter: &str) -> String {
    format!(
        "{}SELECT ?t ?id ?source ?mapping ?status ?reason ?baseline ?candidate ?created ?modified ?by \
         (GROUP_CONCAT(?table; separator=\"\\u001f\") AS ?tables) WHERE {{ GRAPH <{SOURCES_GRAPH}> {{\n\
           ?t a ds:RemapTicket ; ds:id ?id ; ds:source ?source ; ds:status ?status ; ds:reason ?reason ;\n\
              dct:created ?created ; dct:modified ?modified .\n\
           {filter}\n\
           OPTIONAL {{ ?t ds:mapping ?mapping }}\n\
           OPTIONAL {{ ?t ds:affectedTable ?table }}\n\
           OPTIONAL {{ ?t ds:baselineProfile ?baseline }}\n\
           OPTIONAL {{ ?t ds:candidateProfile ?candidate }}\n\
           OPTIONAL {{ ?t prov:wasAttributedTo ?by }}\n\
         }} }} GROUP BY ?t ?id ?source ?mapping ?status ?reason ?baseline ?candidate ?created ?modified ?by \
         ORDER BY DESC(?modified)",
        prefixes()
    )
}

fn tickets(store: &TripleStore, filter: &str) -> Vec<Ticket> {
    let Ok(QueryResults::Solutions(rows)) = store.query(&ticket_select(filter)) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|row| {
            let id = lex(&row, "id")?;
            let mut affected: Vec<String> = lex(&row, "tables")
                .unwrap_or_default()
                .split('\u{1f}')
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect();
            affected.sort();
            affected.dedup();
            Some(Ticket {
                iri: ticket_iri(&id),
                source: lex(&row, "source")?,
                mapping: lex(&row, "mapping"),
                status: lex(&row, "status")?,
                reason: lex(&row, "reason")?,
                affected_tables: affected,
                baseline_profile: lex(&row, "baseline").and_then(|v| v.parse().ok()),
                candidate_profile: lex(&row, "candidate").and_then(|v| v.parse().ok()),
                created_at: lex(&row, "created")?,
                updated_at: lex(&row, "modified")?,
                created_by: lex(&row, "by"),
                id,
            })
        })
        .collect()
}

pub fn list_tickets(store: &TripleStore, source_id: &str) -> Vec<Ticket> {
    tickets(
        store,
        &format!(
            "FILTER(?source = <{}>)",
            escape_sparql_iri(&source_iri(source_id))
        ),
    )
}

pub fn get_ticket(store: &TripleStore, id: &str) -> Option<Ticket> {
    if !valid_id(id) {
        return None;
    }
    tickets(
        store,
        &format!("FILTER(?t = <{}>)", escape_sparql_iri(&ticket_iri(id))),
    )
    .into_iter()
    .next()
}

fn write_ticket(store: &TripleStore, t: &Ticket) -> Result<(), String> {
    let iri = escape_sparql_iri(&t.iri);
    let mut body = format!(
        "  <{iri}> a ds:RemapTicket ;\n    ds:id \"{}\" ;\n    ds:source <{}> ;\n    ds:status \"{}\" ;\n    ds:reason \"{}\" ;\n    dct:created \"{}\" ;\n    dct:modified \"{}\" ;\n",
        escape_sparql_literal(&t.id),
        escape_sparql_iri(&t.source),
        escape_sparql_literal(&t.status),
        escape_sparql_literal(&t.reason),
        escape_sparql_literal(&t.created_at),
        escape_sparql_literal(&t.updated_at),
    );
    if let Some(m) = &t.mapping {
        body.push_str(&format!("    ds:mapping <{}> ;\n", escape_sparql_iri(m)));
    }
    for table in &t.affected_tables {
        body.push_str(&format!(
            "    ds:affectedTable \"{}\" ;\n",
            escape_sparql_literal(table)
        ));
    }
    if let Some(b) = t.baseline_profile {
        body.push_str(&format!("    ds:baselineProfile \"{b}\"^^xsd:integer ;\n"));
    }
    if let Some(c) = t.candidate_profile {
        body.push_str(&format!("    ds:candidateProfile \"{c}\"^^xsd:integer ;\n"));
    }
    if let Some(by) = &t.created_by {
        body.push_str(&format!(
            "    prov:wasAttributedTo <{}> ;\n",
            escape_sparql_iri(by)
        ));
    }
    body.push_str("    ds:kind \"ticket\" .\n");
    let sparql = format!(
        "{}DELETE WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ <{iri}> ?p ?o }} }};\n\
         INSERT DATA {{ GRAPH <{SOURCES_GRAPH}> {{\n{body}}} }}",
        prefixes()
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

/// Open a ticket for `(source, mapping)`, or fold this run's findings into the
/// one already open for it. Returns the ticket as it now stands.
#[allow(clippy::too_many_arguments)]
pub fn open_or_update_ticket(
    store: &TripleStore,
    source_id: &str,
    mapping_id: Option<&str>,
    affected: &[String],
    reason: &str,
    baseline: u32,
    candidate: u32,
    actor: Option<&str>,
) -> Result<Ticket, String> {
    let now = registry::now();
    let mapping_iri = mapping_id.map(mapping_iri);
    let existing = list_tickets(store, source_id)
        .into_iter()
        .find(|t| t.status == "open" && t.mapping == mapping_iri);
    let ticket = match existing {
        Some(mut t) => {
            let mut tables: BTreeSet<String> = t.affected_tables.iter().cloned().collect();
            tables.extend(affected.iter().cloned());
            t.affected_tables = tables.into_iter().collect();
            t.reason = if t.reason == reason || reason == "both" {
                reason.to_string()
            } else {
                "both".to_string()
            };
            t.candidate_profile = Some(candidate);
            t.updated_at = now;
            t
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            Ticket {
                iri: ticket_iri(&id),
                id,
                source: source_iri(source_id),
                mapping: mapping_iri,
                status: "open".to_string(),
                reason: reason.to_string(),
                affected_tables: affected.to_vec(),
                baseline_profile: Some(baseline),
                candidate_profile: Some(candidate),
                created_at: now.clone(),
                updated_at: now,
                created_by: actor.map(str::to_string),
            }
        }
    };
    write_ticket(store, &ticket)?;
    Ok(ticket)
}

pub fn close_ticket(store: &TripleStore, id: &str) -> Result<Option<Ticket>, String> {
    let Some(mut t) = get_ticket(store, id) else {
        return Ok(None);
    };
    t.status = "closed".to_string();
    t.updated_at = registry::now();
    write_ticket(store, &t)?;
    Ok(Some(t))
}

/// Every ticket of a datasource, for the delete cascade.
pub fn delete_tickets(store: &TripleStore, source_id: &str) -> Result<(), String> {
    let source = escape_sparql_iri(&source_iri(source_id));
    store
        .update(&format!(
            "{}DELETE {{ GRAPH <{SOURCES_GRAPH}> {{ ?t ?p ?o }} }} WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ ?t a ds:RemapTicket ; ds:source <{source}> ; ?p ?o }} }}",
            prefixes()
        ))
        .map_err(|e| e.to_string())
}

// ───────────────────────────── HTTP ─────────────────────────────

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriftRequest {
    /// The mapping whose baseline and model version to check, by id or IRI.
    pub mapping: Option<String>,
    /// Profile version to compare against; defaults to the mapping's
    /// baseline, else the previous version.
    pub baseline: Option<u32>,
    /// Profile version to compare; defaults to the newest.
    pub candidate: Option<u32>,
    /// Overrides the gates' `driftKlThreshold`.
    pub kl_threshold: Option<f64>,
    /// Open (or update) a re-map ticket when there is anything to report.
    /// Default true.
    #[serde(default = "default_true")]
    pub open_ticket: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DriftReport {
    pub source: String,
    pub baseline: u32,
    pub candidate: u32,
    pub kl_threshold: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping: Option<MappingRef>,
    pub tables: Vec<TableDrift>,
    pub new_tables: Vec<String>,
    pub removed_tables: Vec<String>,
    /// Every table that needs a look, new and removed ones included.
    pub affected_tables: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_version_bump: Option<ModelBump>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket: Option<Ticket>,
}

/// `POST /api/sources/:id/drift`
pub async fn drift(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DriftRequest>,
) -> Result<Json<DriftReport>, (StatusCode, String)> {
    let bad = |m: String| (StatusCode::BAD_REQUEST, m);
    let source = registry::get_source(&state.store, &id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("datasource '{id}' not found"),
        )
    })?;
    let mapping = match &body.mapping {
        Some(m) => {
            let mid = m.trim().trim_start_matches("urn:mapping:");
            let record = registry::get_mapping(&state.store, mid)
                .ok_or_else(|| (StatusCode::NOT_FOUND, format!("mapping '{mid}' not found")))?;
            if record.source_id != source.id {
                return Err(bad(format!(
                    "mapping '{mid}' is registered against datasource '{}', not '{}'",
                    record.source_id, source.id
                )));
            }
            Some(record)
        }
        None => None,
    };

    let versions = profile::versions(&state.store, &source.id);
    let candidate = match body.candidate {
        Some(c) => c,
        None => *versions.last().ok_or_else(|| {
            bad(format!(
                "datasource '{id}' has not been profiled; POST /api/sources/{id}/profile first"
            ))
        })?,
    };
    let baseline = match body.baseline {
        Some(b) => b,
        None => mapping
            .as_ref()
            .and_then(|m| m.profile_version)
            .filter(|b| *b != candidate)
            .or_else(|| versions.iter().rev().find(|v| **v < candidate).copied())
            .ok_or_else(|| {
                bad(format!(
                    "datasource '{id}' has one profile version; drift needs two — profile it again"
                ))
            })?,
    };
    for v in [baseline, candidate] {
        if !versions.contains(&v) {
            return Err(bad(format!(
                "datasource '{id}' has no profile version {v}; it has {:?}",
                versions
            )));
        }
    }
    if baseline == candidate {
        return Err(bad(
            "baseline and candidate are the same version".to_string()
        ));
    }
    let (gates, _) = super::gates::load(&state.store);
    let kl_threshold = body.kl_threshold.unwrap_or(gates.drift_kl_threshold);

    let older = read_profile(&state.store, &source.id, baseline);
    let newer = read_profile(&state.store, &source.id, candidate);
    let mut tables = Vec::new();
    let mut affected: BTreeSet<String> = BTreeSet::new();
    for (name, shape) in &newer {
        if let Some(old) = older.get(name) {
            let d = compare_table(name, old, shape, kl_threshold);
            if d.affected {
                affected.insert(name.clone());
            }
            tables.push(d);
        }
    }
    let new_tables: Vec<String> = newer
        .keys()
        .filter(|t| !older.contains_key(*t))
        .cloned()
        .collect();
    let removed_tables: Vec<String> = older
        .keys()
        .filter(|t| !newer.contains_key(*t))
        .cloned()
        .collect();
    affected.extend(new_tables.iter().cloned());
    affected.extend(removed_tables.iter().cloned());

    let model_version_bump = mapping.as_ref().and_then(|m| {
        let model = m.model.as_deref()?;
        let version = m.model_version.as_deref()?;
        let record =
            crate::data_models::registry::get_data_model(&state.store, &state.base_url, model)?;
        model_bump(model, version, record.latest_published.as_deref())
    });
    if model_version_bump.is_some() {
        // A model bump re-maps everything the mapping reads: one ticket, every
        // table on it.
        affected.extend(newer.keys().cloned());
    }

    let affected: Vec<String> = affected.into_iter().collect();
    let ticket = if body.open_ticket && !affected.is_empty() {
        let schema_drift = tables.iter().any(|t| t.affected)
            || !new_tables.is_empty()
            || !removed_tables.is_empty();
        let reason = match (model_version_bump.is_some(), schema_drift) {
            (true, true) => "both",
            (true, false) => "model-version-bump",
            (false, _) => "schema-drift",
        };
        let actor = format!(
            "{}/users/{}",
            state.base_url.trim_end_matches('/'),
            user.user_id
        );
        Some(
            open_or_update_ticket(
                &state.store,
                &source.id,
                mapping.as_ref().map(|m| m.id.as_str()),
                &affected,
                reason,
                baseline,
                candidate,
                Some(&actor),
            )
            .map_err(|m| (StatusCode::INTERNAL_SERVER_ERROR, m))?,
        )
    } else {
        None
    };

    Ok(Json(DriftReport {
        source: source.iri(),
        baseline,
        candidate,
        kl_threshold,
        mapping: mapping.as_ref().map(|m| MappingRef {
            id: m.id.clone(),
            version: m.version,
            iri: m.version_iri(),
        }),
        tables,
        new_tables,
        removed_tables,
        affected_tables: affected,
        model_version_bump,
        ticket,
    }))
}

/// `GET /api/sources/:id/tickets`
pub async fn list_source_tickets(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Ticket>>, (StatusCode, String)> {
    if registry::get_source(&state.store, &id).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("datasource '{id}' not found"),
        ));
    }
    Ok(Json(list_tickets(&state.store, &id)))
}

/// `GET /api/tickets/:id`
pub async fn get_ticket_handler(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Ticket>, (StatusCode, String)> {
    get_ticket(&state.store, &id)
        .map(Json)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("ticket '{id}' not found")))
}

/// `POST /api/tickets/:id/close`
pub async fn close_ticket_handler(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Ticket>, (StatusCode, String)> {
    let closed = close_ticket(&state.store, &id)
        .map_err(|m| (StatusCode::INTERNAL_SERVER_ERROR, m))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("ticket '{id}' not found")))?;
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Source,
        format!("re-map ticket {} closed", closed.id),
        Some(&user.user_id),
        Some(closed.source.clone()),
        vec![SOURCES_GRAPH.to_string()],
        0,
        0,
        None,
    );
    Ok(Json(closed))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sources/:id/drift", post(drift))
        .route("/api/sources/:id/tickets", get(list_source_tickets))
        .route("/api/tickets/:id", get(get_ticket_handler))
        .route("/api/tickets/:id/close", post(close_ticket_handler))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ots_plugin_api::sources::{ColumnProfile, TableKind, TableProfile, ValueCount, ValueKind};

    fn column(name: &str, native: &str, kind: ValueKind, top: &[(&str, u64)]) -> ColumnProfile {
        ColumnProfile {
            name: name.into(),
            position: 1,
            native_type: native.into(),
            generic_type: kind,
            nullable: true,
            primary_key: false,
            distinct_count: None,
            null_count: None,
            cardinality_ratio: None,
            top_values: top
                .iter()
                .map(|(v, n)| ValueCount {
                    value: v.to_string(),
                    count: *n,
                })
                .collect(),
            numeric: None,
            mean_length: None,
            pattern: None,
            pattern_confidence: None,
        }
    }

    fn table(name: &str, columns: Vec<ColumnProfile>) -> TableProfile {
        TableProfile {
            table: name.into(),
            kind: TableKind::Table,
            row_count: Some(10),
            columns,
            primary_key: vec![],
            unique_keys: vec![],
            foreign_keys: vec![],
            sampled_rows: 10,
        }
    }

    #[test]
    fn kl_is_zero_for_the_same_distribution_and_grows_with_the_difference() {
        let a: BTreeMap<String, u64> = [("x", 50), ("y", 50)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let b: BTreeMap<String, u64> = [("x", 90), ("y", 10)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        let c: BTreeMap<String, u64> = [("x", 50), ("z", 50)]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        assert!(kl_divergence(&a, &a).abs() < 1e-12);
        let skew = kl_divergence(&b, &a);
        let new_value = kl_divergence(&c, &a);
        assert!(skew > 0.1, "{skew}");
        assert!(
            new_value > skew,
            "a value the baseline never saw is a bigger move: {new_value}"
        );
        assert!(kl_divergence(&BTreeMap::new(), &BTreeMap::new()) == 0.0);
    }

    #[test]
    fn a_profile_diff_reports_columns_types_and_distributions() {
        let store = TripleStore::in_memory().unwrap();
        let v1 = table(
            "entry",
            vec![
                column("id", "INTEGER", ValueKind::Integer, &[]),
                column(
                    "state",
                    "TEXT",
                    ValueKind::Text,
                    &[("alpha", 50), ("beta", 50)],
                ),
                column("old", "TEXT", ValueKind::Text, &[]),
                column("amount", "INTEGER", ValueKind::Integer, &[]),
            ],
        );
        let v2 = table(
            "entry",
            vec![
                column("id", "INTEGER", ValueKind::Integer, &[]),
                column(
                    "state",
                    "TEXT",
                    ValueKind::Text,
                    &[("alpha", 95), ("beta", 5)],
                ),
                column("added", "TEXT", ValueKind::Text, &[]),
                column("amount", "REAL", ValueKind::Float, &[]),
            ],
        );
        profile::write_profile(&store, "s", 1, &[v1, table("gone", vec![])]).unwrap();
        profile::write_profile(&store, "s", 2, &[v2, table("fresh", vec![])]).unwrap();

        let older = read_profile(&store, "s", 1);
        let newer = read_profile(&store, "s", 2);
        assert_eq!(older.len(), 2);
        assert!(older.contains_key("gone") && newer.contains_key("fresh"));
        let d = compare_table("entry", &older["entry"], &newer["entry"], 0.1);
        assert!(d.structural_hash_changed);
        assert_eq!(d.new_columns, vec!["added"]);
        assert_eq!(d.removed_columns, vec!["old"]);
        assert_eq!(d.type_changes.len(), 1);
        assert_eq!(d.type_changes[0].column, "amount");
        assert_eq!(d.type_changes[0].from, "INTEGER (integer)");
        assert_eq!(d.type_changes[0].to, "REAL (double)");
        assert_eq!(d.distribution_shifts.len(), 1, "{d:?}");
        assert_eq!(d.distribution_shifts[0].column, "state");
        assert!(d.affected);

        // A higher threshold keeps the same skew below the line.
        let calm = compare_table("entry", &older["entry"], &newer["entry"], 5.0);
        assert!(calm.distribution_shifts.is_empty());

        // The same profile twice is no drift at all.
        let same = compare_table("entry", &older["entry"], &older["entry"], 0.1);
        assert!(!same.affected, "{same:?}");
    }

    #[test]
    fn a_code_list_that_appears_or_disappears_is_reported() {
        let older = TableShape {
            structural_hash: "h".into(),
            columns: [("c".to_string(), ("TEXT".to_string(), None))]
                .into_iter()
                .collect(),
            top_values: [(
                "c".to_string(),
                [("x".to_string(), 3u64)].into_iter().collect(),
            )]
            .into_iter()
            .collect(),
        };
        let newer = TableShape {
            structural_hash: "h".into(),
            columns: older.columns.clone(),
            top_values: BTreeMap::new(),
        };
        let d = compare_table("t", &older, &newer, 0.1);
        assert_eq!(d.code_list_changes.len(), 1);
        assert_eq!(d.code_list_changes[0].change, "lost");
        let back = compare_table("t", &newer, &older, 0.1);
        assert_eq!(back.code_list_changes[0].change, "gained");
    }

    #[test]
    fn a_model_bump_is_a_newer_published_version() {
        assert!(model_bump("m", "1.0.0", Some("1.0.0")).is_none());
        assert!(
            model_bump("m", "1.0.0", None).is_none(),
            "nothing published"
        );
        let bump = model_bump("m", "1.0.0", Some("2.0.0")).unwrap();
        assert_eq!(
            (bump.mapping_version.as_str(), bump.latest_version.as_str()),
            ("1.0.0", "2.0.0")
        );
    }

    #[test]
    fn one_open_ticket_per_mapping_that_folds_later_findings_in() {
        let store = TripleStore::in_memory().unwrap();
        let t1 = open_or_update_ticket(
            &store,
            "s",
            Some("m"),
            &["a".into()],
            "schema-drift",
            1,
            2,
            Some("http://x/users/adm"),
        )
        .unwrap();
        assert_eq!(t1.status, "open");
        assert_eq!(t1.affected_tables, vec!["a"]);
        let t2 = open_or_update_ticket(
            &store,
            "s",
            Some("m"),
            &["b".into()],
            "model-version-bump",
            1,
            3,
            None,
        )
        .unwrap();
        assert_eq!(t2.id, t1.id, "the same ticket, updated");
        assert_eq!(t2.affected_tables, vec!["a", "b"]);
        assert_eq!(t2.reason, "both");
        assert_eq!(t2.candidate_profile, Some(3));
        assert_eq!(t2.created_by.as_deref(), Some("http://x/users/adm"), "kept");
        assert_eq!(list_tickets(&store, "s").len(), 1);

        // Another mapping gets its own.
        let other = open_or_update_ticket(
            &store,
            "s",
            Some("n"),
            &["a".into()],
            "schema-drift",
            1,
            2,
            None,
        )
        .unwrap();
        assert_ne!(other.id, t1.id);
        assert_eq!(list_tickets(&store, "s").len(), 2);

        // Closed is closed; the next finding opens a fresh one.
        let closed = close_ticket(&store, &t1.id).unwrap().unwrap();
        assert_eq!(closed.status, "closed");
        assert_eq!(get_ticket(&store, &t1.id).unwrap().status, "closed");
        let t3 = open_or_update_ticket(
            &store,
            "s",
            Some("m"),
            &["c".into()],
            "schema-drift",
            2,
            3,
            None,
        )
        .unwrap();
        assert_ne!(t3.id, t1.id);
        assert_eq!(list_tickets(&store, "s").len(), 3);
        assert!(close_ticket(&store, "nope").unwrap().is_none());

        delete_tickets(&store, "s").unwrap();
        assert!(list_tickets(&store, "s").is_empty());
    }
}
