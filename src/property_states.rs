//! Time-evolving properties: the Ontology for Property Management (OPM) for
//! any dataset.
//!
//! A property value that changes over time — a bridge's load rating, a
//! patient's weight, a device's firmware — is recorded as a chain of
//! `opm:PropertyState`s in the dataset's *states graph* (role `provenance`),
//! each with its value, `ots:validFrom`, recording time, agent, reliability,
//! documentation and note. The data graph always carries the **current**
//! value as a plain triple, so SPARQL, SHACL and reasoning see nothing new;
//! the history, listing and "as of" views read the states.
//!
//! * `POST /api/datasets/:id/properties/state`   — set a new state
//! * `POST /api/datasets/:id/properties/delete`  — a new `opm:Deleted` state
//! * `POST /api/datasets/:id/properties/restore` — undo the deletion
//! * `GET  /api/datasets/:id/properties`         — list and filter, an item snapshot
//! * `GET  /api/datasets/:id/properties/history` — every state, newest first
//! * `GET  /api/datasets/:id/properties/as-of`   — the state valid at a time
//! * `GET  /api/datasets/:id/properties/export`  — canonical OPM
//! * `POST /api/datasets/:id/properties/import`  — canonical OPM in
//! * `GET  /api/datasets/:id/properties/validate` — the OPM profile shapes
//!
//! **Storage.** A property node is linked to its item with
//! `ots:propertyOf` / `ots:propertyPredicate`, not with OPM's
//! `<item> <predicate> <property>`: the data graph keeps the plain value, and
//! a canonical link stored in any graph of the union default graph would add
//! the property node to the answers of `<item> <predicate> ?v`. Export writes
//! the canonical link, import reads it, and every read here also accepts
//! canonical OPM loaded straight into a dataset graph.
//!
//! Vocabulary: OPM (<https://w3id.org/opm#>) for the property/state model,
//! reliability classes and deletion, `schema:value` for the value, PROV for
//! time and attribution. Domain vocabularies (material passports, clinical
//! records) supply the property IRIs; nothing here knows them.

mod exchange;

pub use exchange::{export_states, import_states};

use std::collections::{BTreeMap, BTreeSet, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use oxigraph::model::{NamedNode, Term};
use oxigraph::sparql::QuerySolution;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, GraphKind};
use crate::server::AppState;
use crate::store::{escape_sparql_iri, escape_sparql_literal, validate_language_tag};

pub const OPM: &str = "https://w3id.org/opm#";
pub const SCHEMA: &str = "https://schema.org/";
/// `opm.ttl` itself declares `schema:` as `http://schema.org/`; reads and
/// import accept both forms of `schema:value`.
pub const SCHEMA_HTTP: &str = "http://schema.org/";
pub const PROV: &str = "http://www.w3.org/ns/prov#";
pub const OTS: &str = "https://opentriplestore.org/ns#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// The OPM profile: SHACL shapes for a well-formed OPM states graph (one
/// current state per property, current/outdated and assumed/confirmed
/// disjoint, a generation time on every state, a value on every state that is
/// not deleted, the structure of derived states and calculations). Also
/// shipped as the `opm-profile` seed bundle.
pub const OPM_PROFILE_SHAPES: &str =
    include_str!("../examples/seed-bundles/opm-profile/shapes.ttl");

/// Most solutions a single read evaluates; more is a 422 that asks for a
/// narrower request rather than a silently truncated answer.
const MAX_READ_ROWS: usize = 200_000;
/// Most properties one listing returns.
const MAX_LIST_LIMIT: usize = 10_000;

type ApiErr = (StatusCode, String);

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn bad(msg: impl Into<String>) -> ApiErr {
    (StatusCode::BAD_REQUEST, msg.into())
}
fn conflict(msg: impl Into<String>) -> ApiErr {
    (StatusCode::CONFLICT, msg.into())
}

/// The dataset's states graph.
pub fn states_graph(dataset_id: &str) -> String {
    format!("urn:ots:property-states:{dataset_id}")
}

/// A stable IRI for "property `p` of entity `e`".
pub fn property_iri(entity: &str, property: &str) -> String {
    let h = Sha256::digest(format!("{entity}\n{property}").as_bytes());
    let hex: String = h[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("urn:ots:property:{hex}")
}

/// OPM's reliability classes (`opm:Required` included: a requirement on the
/// property rather than a value it has).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reliability {
    Assumed,
    Confirmed,
    Derived,
    Required,
}

impl Reliability {
    fn parse(s: &str) -> Result<Self, ApiErr> {
        match s.to_ascii_lowercase().as_str() {
            "assumed" => Ok(Self::Assumed),
            "confirmed" => Ok(Self::Confirmed),
            "derived" => Ok(Self::Derived),
            "required" => Ok(Self::Required),
            other => Err(bad(format!(
                "reliability `{other}` is not assumed|confirmed|derived|required"
            ))),
        }
    }
    fn class(self) -> &'static str {
        match self {
            Self::Assumed => "Assumed",
            Self::Confirmed => "Confirmed",
            Self::Derived => "Derived",
            Self::Required => "Required",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Assumed => "assumed",
            Self::Confirmed => "confirmed",
            Self::Derived => "derived",
            Self::Required => "required",
        }
    }
    fn from_iri(iri: &str) -> Option<Self> {
        match iri.strip_prefix(OPM)? {
            "Assumed" => Some(Self::Assumed),
            "Confirmed" => Some(Self::Confirmed),
            "Derived" => Some(Self::Derived),
            "Required" => Some(Self::Required),
            _ => None,
        }
    }
}

/// The value as a SPARQL term.
///
/// Everything the caller sent is checked before it is spliced into the
/// update: the literal text is escaped, the language tag must be a tag
/// (`@` ends a literal, so an unchecked tag is a way out of the string), and
/// datatype / IRI values must be IRIs.
fn value_term(
    value: &str,
    datatype: Option<&str>,
    language: Option<&str>,
) -> Result<String, ApiErr> {
    if let Some(lang) = language {
        validate_language_tag(lang).map_err(bad)?;
        return Ok(format!("\"{}\"@{lang}", escape_sparql_literal(value)));
    }
    match datatype {
        Some("iri") => {
            NamedNode::new(value).map_err(|e| bad(format!("value is not an IRI: {e}")))?;
            Ok(format!("<{}>", escape_sparql_iri(value)))
        }
        Some(dt) => {
            let dt = dt
                .strip_prefix("xsd:")
                .map(|l| format!("{XSD}{l}"))
                .unwrap_or_else(|| dt.to_string());
            NamedNode::new(&dt).map_err(|e| bad(format!("datatype is not an IRI: {e}")))?;
            Ok(format!(
                "\"{}\"^^<{}>",
                escape_sparql_literal(value),
                escape_sparql_iri(&dt)
            ))
        }
        None => {
            let v = value.trim();
            if v == "true" || v == "false" || v.parse::<i64>().is_ok() {
                Ok(v.to_string())
            } else if v.contains('.') && v.parse::<f64>().is_ok() {
                Ok(format!("\"{v}\"^^<{XSD}decimal>"))
            } else {
                Ok(format!("\"{}\"", escape_sparql_literal(value)))
            }
        }
    }
}

/// RFC 3339, or a bare date (midnight UTC).
fn parse_time(s: &str) -> Result<String, ApiErr> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(t.with_timezone(&chrono::Utc).to_rfc3339());
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).unwrap().and_utc().to_rfc3339());
    }
    Err(bad(format!(
        "`{s}` is not an RFC 3339 timestamp or a YYYY-MM-DD date"
    )))
}

/// An `xsd:dateTime` lexical form as an instant, for ordering. A form
/// without a timezone is read as UTC.
fn instant(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&chrono::Utc));
    }
    chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .ok()
        .map(|t| t.and_utc())
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|t| t.and_utc())
        })
}

fn iri(s: &str, what: &str) -> Result<NamedNode, ApiErr> {
    NamedNode::new(s).map_err(|e| bad(format!("{what}: {e}")))
}

pub(crate) fn visible_dataset(
    state: &AppState,
    uid: Option<&str>,
    id: &str,
) -> Result<Dataset, ApiErr> {
    let ds = state
        .auth_db
        .get_dataset(id)
        .map_err(e500)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state.auth_db.can_access_dataset(uid, &ds).map_err(e500)? {
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }
    Ok(ds)
}

/// The dataset, if the caller may write it.
pub(crate) fn writable_dataset(
    state: &AppState,
    user: &AuthenticatedUser,
    id: &str,
) -> Result<Dataset, ApiErr> {
    let ds = visible_dataset(state, Some(&user.user_id), id)?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    Ok(ds)
}

/// What a caller reads of a dataset: the graphs a read is confined to, and
/// the dataset graphs withheld from them (private graphs, for a caller who
/// may not write the dataset). A state whose value lives in a withheld graph
/// is withheld with it — the states graph must not mirror a private value to
/// every viewer.
pub(crate) struct ReadScope {
    pub graphs: Vec<String>,
    pub hidden: HashSet<String>,
}

pub(crate) fn read_scope(
    state: &AppState,
    uid: Option<&str>,
    ds: &Dataset,
) -> Result<ReadScope, ApiErr> {
    let graphs = state
        .auth_db
        .list_readable_dataset_graphs(uid, ds)
        .map_err(e500)?;
    let readable: HashSet<&String> = graphs.iter().collect();
    let hidden = state
        .auth_db
        .list_dataset_graphs(&ds.id)
        .map_err(e500)?
        .into_iter()
        .filter(|g| !readable.contains(g))
        .collect();
    Ok(ReadScope { graphs, hidden })
}

/// The graph a state's current value goes to when the caller names none:
/// the dataset's instances graph, else its first graph without a model-layer
/// role, else its first graph.
pub(crate) fn default_data_graph(state: &AppState, dataset_id: &str) -> Option<String> {
    let entries = state.auth_db.list_dataset_graph_entries(dataset_id).ok()?;
    let states = states_graph(dataset_id);
    let candidates: Vec<_> = entries.iter().filter(|e| e.graph_iri != states).collect();
    candidates
        .iter()
        .find(|e| e.graph_role == Some(GraphKind::Instances))
        .or_else(|| candidates.iter().find(|e| e.graph_role.is_none()))
        .or_else(|| candidates.first())
        .map(|e| e.graph_iri.clone())
}

/// `graph` if it is one of the dataset's graphs (and not its states graph).
pub(crate) fn registered_data_graph(
    state: &AppState,
    dataset_id: &str,
    graph: &str,
) -> Result<String, ApiErr> {
    let graphs = state
        .auth_db
        .list_dataset_graphs(dataset_id)
        .map_err(e500)?;
    if graph == states_graph(dataset_id) || !graphs.iter().any(|x| x == graph) {
        return Err(bad(format!(
            "graph <{graph}> is not a data graph registered to dataset {dataset_id}"
        )));
    }
    Ok(graph.to_string())
}

/// Register the states graph (role `provenance`) on first use.
pub(crate) fn ensure_states_graph(state: &AppState, dataset_id: &str) -> Result<String, ApiErr> {
    let g = states_graph(dataset_id);
    let known = state
        .auth_db
        .list_dataset_graphs(dataset_id)
        .map_err(e500)?
        .iter()
        .any(|x| x == &g);
    if !known {
        state
            .auth_db
            .add_dataset_graph(dataset_id, &g)
            .map_err(e500)?;
        state
            .auth_db
            .set_dataset_graph_role(dataset_id, &g, Some(GraphKind::Provenance))
            .map_err(e500)?;
    }
    Ok(g)
}

pub(crate) fn agent_iri(state: &AppState, user_id: &str) -> String {
    format!("{}/users/{}", state.base_url.trim_end_matches('/'), user_id)
}

// ─── Reading ─────────────────────────────────────────────────────────────────

/// One stored state, as the reads see it.
#[derive(Clone, Debug)]
pub(crate) struct StoredState {
    pub iri: String,
    pub prop: String,
    pub entity: String,
    pub property: String,
    pub value: Option<Term>,
    pub valid_from: String,
    pub recorded_at: String,
    pub attributed_to: Option<String>,
    pub note: Option<String>,
    pub reliability: BTreeSet<Reliability>,
    pub current: bool,
    pub deleted: bool,
    pub documentation: BTreeSet<String>,
    pub data_graph: Option<String>,
    pub expression: Option<String>,
    pub derived_from: Option<String>,
    pub calculation: Option<String>,
    pub revision_of: Option<String>,
    /// Found through a canonical `<item> <predicate> <property>` link in a
    /// dataset graph rather than the server's own storage.
    pub canonical: bool,
}

impl StoredState {
    fn order_key(
        &self,
    ) -> (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) {
        (instant(&self.valid_from), instant(&self.recorded_at))
    }
    fn recorded_key(
        &self,
    ) -> (
        Option<chrono::DateTime<chrono::Utc>>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) {
        (instant(&self.recorded_at), instant(&self.valid_from))
    }
    pub fn view(&self) -> StateView {
        let (value, datatype, language) = match &self.value {
            Some(t) => {
                let (v, d, l) = value_parts(t);
                (Some(v), d, l)
            }
            None => (None, None, None),
        };
        StateView {
            state: self.iri.clone(),
            value,
            datatype,
            language,
            valid_from: self.valid_from.clone(),
            recorded_at: self.recorded_at.clone(),
            attributed_to: self.attributed_to.clone(),
            reliability: self.reliability.iter().next().map(|r| r.name().to_string()),
            note: self.note.clone(),
            current: self.current,
            deleted: self.deleted,
            documentation: self.documentation.iter().cloned().collect(),
            expression: self.expression.clone(),
            derived_from: self.derived_from.clone(),
            calculation: self.calculation.clone(),
            revision_of: self.revision_of.clone(),
            canonical: self.canonical,
        }
    }
}

fn value_parts(t: &Term) -> (String, Option<String>, Option<String>) {
    match t {
        Term::Literal(l) => (
            l.value().to_string(),
            if l.language().is_some() {
                None
            } else {
                Some(l.datatype().as_str().to_string())
            },
            l.language().map(str::to_string),
        ),
        Term::NamedNode(n) => (n.as_str().to_string(), Some("iri".to_string()), None),
        other => (other.to_string(), None, None),
    }
}

#[derive(Debug, Serialize)]
pub struct StateView {
    pub state: String,
    /// `null` for a deleted state.
    pub value: Option<String>,
    pub datatype: Option<String>,
    pub language: Option<String>,
    pub valid_from: String,
    pub recorded_at: String,
    pub attributed_to: Option<String>,
    pub reliability: Option<String>,
    pub note: Option<String>,
    pub current: bool,
    pub deleted: bool,
    pub documentation: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expression: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calculation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision_of: Option<String>,
    pub canonical: bool,
}

/// Every state of the properties the bindings select (`e` = item, `p` =
/// property kind; either, both or neither), read from the server's storage
/// and from canonical OPM in the scope's graphs. States whose value lives in
/// a hidden graph are left out.
pub(crate) fn read_states(
    state: &AppState,
    scope: &ReadScope,
    entity: Option<&str>,
    property: Option<&str>,
) -> Result<Vec<StoredState>, ApiErr> {
    if scope.graphs.is_empty() {
        return Ok(Vec::new());
    }
    let parsed = crate::sparql::parser()
        .parse_query(&states_select())
        .map_err(e500)?;
    let mut bindings: Vec<(&str, Term)> = Vec::new();
    if let Some(e) = entity {
        bindings.push(("e", iri(e, "entity")?.into()));
    }
    if let Some(p) = property {
        bindings.push(("p", iri(p, "property")?.into()));
    }
    let rows = state
        .store
        .select_confined(
            &parsed,
            &scope.graphs,
            &bindings,
            MAX_READ_ROWS,
            std::time::Duration::from_secs(state.query_timeout_secs.max(1)),
        )
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("{e}; narrow the request with `entity` or `property`"),
            )
        })?;
    Ok(collect_states(&rows)
        .into_iter()
        .filter(|s| {
            s.data_graph
                .as_ref()
                .is_none_or(|g| !scope.hidden.contains(g))
        })
        .collect())
}

/// The SELECT every read of states runs: property nodes the server links
/// with `ots:propertyOf` and canonical `<item> <kind> <property>` links, with
/// each state's value (`schema:value` in either scheme, or OPM's deprecated
/// `opm:valueAtState`) and metadata. `?e` / `?p` may be bound.
pub(crate) fn states_select() -> String {
    format!(
        r#"PREFIX opm: <{OPM}>
PREFIX schema: <{SCHEMA}>
PREFIX schemahttp: <{SCHEMA_HTTP}>
PREFIX prov: <{PROV}>
PREFIX ots: <{OTS}>
PREFIX rdfs: <{RDFS}>
SELECT ?e ?p ?prop ?s ?v ?vf ?rec ?who ?note ?cls ?doc ?dg ?expr ?seq ?calc ?rev ?canon WHERE {{
  {{ ?prop ots:propertyOf ?e ; ots:propertyPredicate ?p . BIND(false AS ?canon) }}
  UNION
  {{ ?e ?p ?prop . FILTER(!isLiteral(?prop)) FILTER EXISTS {{ ?prop opm:hasPropertyState ?any }} BIND(true AS ?canon) }}
  ?prop opm:hasPropertyState ?s .
  OPTIONAL {{ ?s schema:value ?v1 }}
  OPTIONAL {{ ?s schemahttp:value ?v2 }}
  OPTIONAL {{ ?s opm:valueAtState ?v3 }}
  BIND(COALESCE(?v1, ?v2, ?v3) AS ?v)
  OPTIONAL {{ ?s prov:generatedAtTime ?rec }}
  OPTIONAL {{ ?s ots:validFrom ?vf0 }}
  BIND(COALESCE(?vf0, ?rec) AS ?vf)
  OPTIONAL {{ ?s prov:wasAttributedTo ?who }}
  OPTIONAL {{ ?s rdfs:comment ?note }}
  OPTIONAL {{ ?s a ?cls }}
  OPTIONAL {{ ?s opm:documentation ?doc }}
  OPTIONAL {{ ?s ots:dataGraph ?dg }}
  OPTIONAL {{ ?s opm:expression ?expr }}
  OPTIONAL {{ ?s prov:wasDerivedFrom ?seq }}
  OPTIONAL {{ ?s ots:calculation ?calc }}
  OPTIONAL {{ ?s prov:wasRevisionOf ?rev }}
}}"#
    )
}

/// The rows of [`states_select`] as one [`StoredState`] per (property node,
/// state).
pub(crate) fn collect_states(rows: &[QuerySolution]) -> Vec<StoredState> {
    let mut by_state: BTreeMap<(String, String), StoredState> = BTreeMap::new();
    let get = |row: &QuerySolution, k: &str| row.get(k).map(term_string);
    for row in rows {
        let (Some(s), Some(prop), Some(e), Some(p)) = (
            get(row, "s"),
            get(row, "prop"),
            get(row, "e"),
            get(row, "p"),
        ) else {
            continue;
        };
        let canonical = matches!(row.get("canon"), Some(Term::Literal(l)) if l.value() == "true");
        let entry = by_state
            .entry((prop.clone(), s.clone()))
            .or_insert_with(|| StoredState {
                iri: s.clone(),
                prop: prop.clone(),
                entity: e.clone(),
                property: p.clone(),
                value: row.get("v").cloned(),
                valid_from: get(row, "vf").unwrap_or_default(),
                recorded_at: get(row, "rec").unwrap_or_default(),
                attributed_to: get(row, "who"),
                note: get(row, "note"),
                reliability: BTreeSet::new(),
                current: false,
                deleted: false,
                documentation: BTreeSet::new(),
                data_graph: get(row, "dg"),
                expression: get(row, "expr"),
                derived_from: get(row, "seq"),
                calculation: get(row, "calc"),
                revision_of: get(row, "rev"),
                canonical,
            });
        // The server's own link wins over a canonical one for the same node.
        entry.canonical &= canonical;
        if let Some(cls) = get(row, "cls") {
            if cls == format!("{OPM}CurrentPropertyState") {
                entry.current = true;
            } else if cls == format!("{OPM}Deleted") {
                entry.deleted = true;
            } else if let Some(r) = Reliability::from_iri(&cls) {
                entry.reliability.insert(r);
            }
        }
        if let Some(doc) = get(row, "doc") {
            entry.documentation.insert(doc);
        }
    }
    by_state.into_values().collect()
}

fn term_string(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        other => other.to_string(),
    }
}

/// One property: its item, kind, node and states (newest first).
pub(crate) struct PropertyStates {
    pub entity: String,
    pub property: String,
    pub prop: String,
    pub states: Vec<StoredState>,
}

impl PropertyStates {
    /// The current state: the newest-recorded state marked current, else the
    /// newest-recorded state (canonical data without current markers).
    pub fn current(&self) -> Option<&StoredState> {
        self.states
            .iter()
            .filter(|s| s.current)
            .max_by_key(|s| s.recorded_key())
            .or_else(|| self.states.iter().max_by_key(|s| s.recorded_key()))
    }
    /// The state valid at `at`: the latest `validFrom` not after it.
    pub fn at(&self, at: &str) -> Option<&StoredState> {
        let at = instant(at)?;
        self.states
            .iter()
            .filter(|s| instant(&s.valid_from).is_some_and(|v| v <= at))
            .max_by_key(|s| s.order_key())
    }
}

/// Group states by property node, each newest first.
pub(crate) fn group_states(states: Vec<StoredState>) -> Vec<PropertyStates> {
    let mut by_prop: BTreeMap<(String, String, String), Vec<StoredState>> = BTreeMap::new();
    for s in states {
        by_prop
            .entry((s.entity.clone(), s.property.clone(), s.prop.clone()))
            .or_default()
            .push(s);
    }
    by_prop
        .into_iter()
        .map(|((entity, property, prop), mut states)| {
            states.sort_by_key(|s| std::cmp::Reverse(s.order_key()));
            PropertyStates {
                entity,
                property,
                prop,
                states,
            }
        })
        .collect()
}

/// The states of one (item, property kind), merged across property nodes
/// (the server's own and any canonical one), newest first.
fn states_of_property(
    state: &AppState,
    scope: &ReadScope,
    entity: &str,
    property: &str,
) -> Result<(String, Vec<StoredState>), ApiErr> {
    let groups = group_states(read_states(state, scope, Some(entity), Some(property))?);
    let prop = groups
        .iter()
        .find(|g| g.states.iter().any(|s| !s.canonical))
        .or(groups.first())
        .map(|g| g.prop.clone())
        .unwrap_or_else(|| property_iri(entity, property));
    let mut states: Vec<StoredState> = groups.into_iter().flat_map(|g| g.states).collect();
    states.sort_by_key(|s| std::cmp::Reverse(s.order_key()));
    Ok((prop, states))
}

// ─── Writing ─────────────────────────────────────────────────────────────────

/// One new state of one property, as the write path takes it.
pub(crate) struct StateWrite {
    pub entity: String,
    pub property: String,
    /// The value as SPARQL term text; `None` writes an `opm:Deleted` state.
    pub value: Option<String>,
    pub valid_from: String,
    pub recorded_at: String,
    pub agent: String,
    pub reliability: Option<Reliability>,
    pub note: Option<String>,
    pub documentation: Vec<String>,
    pub data_graph: String,
    pub revision_of: Option<String>,
    /// Further `<state> …` statements for the states graph, as SPARQL text
    /// built from validated terms (a derived state's expression and
    /// arguments).
    pub extra: Vec<String>,
}

/// The update that records `w` as state `st` of property node `prop`: the
/// previous current state becomes outdated, the data graph's plain value is
/// replaced (or removed, for a deletion). A canonical property node linked
/// from the data graph is left alone — only plain values are replaced.
pub(crate) fn state_update(states: &str, prop: &str, st: &str, w: &StateWrite) -> String {
    let e = escape_sparql_iri(&w.entity);
    let p = escape_sparql_iri(&w.property);
    let dg = escape_sparql_iri(&w.data_graph);
    let prop = escape_sparql_iri(prop);
    let st = escape_sparql_iri(st);
    let mut lines = vec![
        format!("<{st}> a opm:PropertyState, opm:CurrentPropertyState"),
        format!("<{st}> ots:validFrom \"{}\"^^xsd:dateTime", w.valid_from),
        format!(
            "<{st}> prov:generatedAtTime \"{}\"^^xsd:dateTime",
            w.recorded_at
        ),
        format!(
            "<{st}> prov:wasAttributedTo <{}>",
            escape_sparql_iri(&w.agent)
        ),
        format!("<{st}> ots:dataGraph <{dg}>"),
    ];
    match &w.value {
        Some(term) => lines.push(format!("<{st}> schema:value {term}")),
        None => lines.push(format!("<{st}> a opm:Deleted")),
    }
    if let Some(r) = w.reliability {
        lines.push(format!("<{st}> a opm:{}", r.class()));
    }
    if let Some(n) = &w.note {
        lines.push(format!(
            "<{st}> rdfs:comment \"{}\"",
            escape_sparql_literal(n)
        ));
    }
    for d in &w.documentation {
        lines.push(format!(
            "<{st}> opm:documentation <{}>",
            escape_sparql_iri(d)
        ));
    }
    if let Some(r) = &w.revision_of {
        lines.push(format!(
            "<{st}> prov:wasRevisionOf <{}>",
            escape_sparql_iri(r)
        ));
    }
    lines.extend(w.extra.iter().cloned());
    let state_block = lines.join(" .\n        ");
    let insert_value = match &w.value {
        Some(term) => format!("GRAPH <{dg}> {{ <{e}> <{p}> {term} }}"),
        None => String::new(),
    };
    format!(
        r#"PREFIX opm: <{OPM}>
PREFIX schema: <{SCHEMA}>
PREFIX prov: <{PROV}>
PREFIX ots: <{OTS}>
PREFIX xsd: <{XSD}>
PREFIX rdfs: <{RDFS}>
PREFIX rdf: <{RDF}>
DELETE {{
    GRAPH <{dg}> {{ <{e}> <{p}> ?old }}
    GRAPH <{states}> {{ ?cur a opm:CurrentPropertyState }}
}}
INSERT {{
    {insert_value}
    GRAPH <{states}> {{
        ?cur a opm:OutdatedPropertyState .
        <{prop}> a opm:Property ;
            ots:propertyOf <{e}> ;
            ots:propertyPredicate <{p}> ;
            opm:hasPropertyState <{st}> .
        {state_block} .
    }}
}}
WHERE {{
    OPTIONAL {{ GRAPH <{dg}> {{ <{e}> <{p}> ?old FILTER NOT EXISTS {{ ?old opm:hasPropertyState ?anyState }} }} }}
    OPTIONAL {{ GRAPH <{states}> {{ <{prop}> opm:hasPropertyState ?cur . ?cur a opm:CurrentPropertyState }} }}
}}"#
    )
}

/// The property node the server keeps for (item, kind): an existing one
/// (an imported node keeps its IRI), else the stable hash IRI.
pub(crate) fn managed_property_node(
    state: &AppState,
    states: &str,
    entity: &str,
    property: &str,
) -> Result<String, ApiErr> {
    let q = format!(
        "PREFIX ots: <{OTS}> SELECT ?prop ?e ?p WHERE {{ ?prop ots:propertyOf ?e ; ots:propertyPredicate ?p }}"
    );
    let parsed = crate::sparql::parser().parse_query(&q).map_err(e500)?;
    let rows = state
        .store
        .select_confined(
            &parsed,
            &[states.to_string()],
            &[
                ("e", iri(entity, "entity")?.into()),
                ("p", iri(property, "property")?.into()),
            ],
            1000,
            std::time::Duration::from_secs(state.query_timeout_secs.max(1)),
        )
        .map_err(e500)?;
    let mut nodes: Vec<String> = rows
        .iter()
        .filter_map(|r| match r.get("prop") {
            Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect();
    nodes.sort();
    let hash = property_iri(entity, property);
    if nodes.iter().any(|n| n == &hash) {
        return Ok(hash);
    }
    Ok(nodes.into_iter().next().unwrap_or(hash))
}

/// Run the statements as one transaction, re-derive entailments for the
/// touched data graphs and record one commit.
pub(crate) async fn apply_writes(
    state: &AppState,
    dataset_id: &str,
    user_id: &str,
    statements: Vec<String>,
    data_graphs: Vec<String>,
    message: String,
    added: usize,
) -> Result<(), ApiErr> {
    if statements.is_empty() {
        return Ok(());
    }
    let results = state
        .store
        .batch_update(&statements)
        .map_err(|e| bad(format!("state update failed: {e}")))?;
    if let Some(crate::store::engine::BatchStatement::Failed(e)) = results
        .iter()
        .find(|r| matches!(r, crate::store::engine::BatchStatement::Failed(_)))
    {
        return Err(bad(format!("state update failed: {e}")));
    }
    let states = states_graph(dataset_id);
    {
        let st = state.clone();
        let g = data_graphs.clone();
        let _ = tokio::task::spawn_blocking(move || crate::entailment::after_write(&st, &g)).await;
    }
    let mut affected = data_graphs;
    affected.push(states);
    affected.sort();
    affected.dedup();
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Dataset,
        message,
        Some(user_id),
        Some(format!(
            "{}/dataset/{}",
            state.base_url.trim_end_matches('/'),
            dataset_id
        )),
        affected,
        added,
        0,
        None,
    );
    Ok(())
}

fn documentation_iris(docs: Option<&Vec<String>>) -> Result<Vec<String>, ApiErr> {
    let mut out = Vec::new();
    for d in docs.into_iter().flatten() {
        iri(d, "documentation")?;
        out.push(d.clone());
    }
    Ok(out)
}

/// Where a write to (item, kind) puts the plain value: the caller's graph,
/// else the graph the current state's value is in, else the default.
fn write_graph(
    state: &AppState,
    dataset_id: &str,
    requested: Option<&str>,
    current: Option<&StoredState>,
) -> Result<String, ApiErr> {
    if let Some(g) = requested {
        return registered_data_graph(state, dataset_id, g);
    }
    if let Some(g) = current.and_then(|c| c.data_graph.as_deref()) {
        if let Ok(g) = registered_data_graph(state, dataset_id, g) {
            return Ok(g);
        }
    }
    default_data_graph(state, dataset_id).ok_or_else(|| {
        bad("the dataset has no graph to hold the current value; register one or pass `graph`")
    })
}

#[derive(Debug, Deserialize)]
pub struct SetStateBody {
    pub entity: String,
    pub property: String,
    pub value: String,
    /// An XSD datatype (`xsd:decimal` or a full IRI), or `iri` for an IRI value.
    pub datatype: Option<String>,
    pub language: Option<String>,
    /// The data graph holding the current value (default: the graph the
    /// current value is in, else see [`default_data_graph`]).
    pub graph: Option<String>,
    /// When the value became true (default: now).
    pub valid_from: Option<String>,
    /// `assumed` | `confirmed` | `derived` | `required` (OPM reliability).
    pub reliability: Option<String>,
    pub note: Option<String>,
    /// IRIs of documents backing the state (`opm:documentation`).
    pub documentation: Option<Vec<String>>,
}

/// POST /api/datasets/:id/properties/state
pub async fn set_state(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<SetStateBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = writable_dataset(&state, &user, &dataset_id)?;
    iri(&body.entity, "entity")?;
    iri(&body.property, "property")?;
    let term = value_term(
        &body.value,
        body.datatype.as_deref(),
        body.language.as_deref(),
    )?;
    let reliability = body
        .reliability
        .as_deref()
        .map(Reliability::parse)
        .transpose()?;
    let documentation = documentation_iris(body.documentation.as_ref())?;
    let now = chrono::Utc::now().to_rfc3339();
    let valid_from = match &body.valid_from {
        Some(v) => parse_time(v)?,
        None => now.clone(),
    };
    let scope = read_scope(&state, Some(&user.user_id), &ds)?;
    let (_, existing) = states_of_property(&state, &scope, &body.entity, &body.property)?;
    let current = existing.iter().find(|s| !s.canonical && s.current);
    let data_graph = write_graph(&state, &dataset_id, body.graph.as_deref(), current)?;
    let states = ensure_states_graph(&state, &dataset_id)?;
    let prop = managed_property_node(&state, &states, &body.entity, &body.property)?;
    let st = format!("urn:ots:property-state:{}", uuid::Uuid::new_v4());
    let write = StateWrite {
        entity: body.entity.clone(),
        property: body.property.clone(),
        value: Some(term),
        valid_from: valid_from.clone(),
        recorded_at: now.clone(),
        agent: agent_iri(&state, &user.user_id),
        reliability,
        note: body.note.clone(),
        documentation: documentation.clone(),
        data_graph: data_graph.clone(),
        revision_of: None,
        extra: Vec::new(),
    };
    apply_writes(
        &state,
        &dataset_id,
        &user.user_id,
        vec![state_update(&states, &prop, &st, &write)],
        vec![data_graph.clone()],
        format!(
            "Property state: <{}> <{}> = {}",
            body.entity, body.property, body.value
        ),
        1,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "dataset_id": dataset_id,
            "entity": body.entity,
            "property": body.property,
            "property_iri": prop,
            "state": st,
            "value": body.value,
            "valid_from": valid_from,
            "recorded_at": now,
            "data_graph": data_graph,
            "states_graph": states,
            "reliability": reliability.map(Reliability::name),
            "documentation": documentation,
        })),
    ))
}

#[derive(Debug, Deserialize)]
pub struct LifecycleBody {
    pub entity: String,
    pub property: String,
    /// When the deletion / restoration took effect (default: now).
    pub valid_from: Option<String>,
    pub note: Option<String>,
    pub documentation: Option<Vec<String>>,
    /// The data graph the plain value is removed from / restored to
    /// (default: the graph the value was in).
    pub graph: Option<String>,
}

/// POST /api/datasets/:id/properties/delete
///
/// OPM deletes a property without removing it: a new current state typed
/// `opm:Deleted`, with no value, ends the chain; the plain triple leaves the
/// data graph. History and as-of report the deletion; restore undoes it.
pub async fn delete_property(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<LifecycleBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = writable_dataset(&state, &user, &dataset_id)?;
    iri(&body.entity, "entity")?;
    iri(&body.property, "property")?;
    let documentation = documentation_iris(body.documentation.as_ref())?;
    let scope = read_scope(&state, Some(&user.user_id), &ds)?;
    let (_, existing) = states_of_property(&state, &scope, &body.entity, &body.property)?;
    let current = existing.iter().find(|s| !s.canonical && s.current);
    if current.is_some_and(|c| c.deleted) {
        return Err(conflict(format!(
            "<{}> <{}> is already deleted",
            body.entity, body.property
        )));
    }
    let data_graph = write_graph(&state, &dataset_id, body.graph.as_deref(), current)?;
    if current.is_none() {
        // Nothing recorded yet: deleting a plain value still records the
        // deletion; deleting nothing at all is a 404.
        let has_plain = state
            .store
            .store()
            .quads_for_pattern(
                Some(iri(&body.entity, "entity")?.as_ref().into()),
                Some(iri(&body.property, "property")?.as_ref()),
                None,
                Some(iri(&data_graph, "graph")?.as_ref().into()),
            )
            .next()
            .is_some();
        if !has_plain {
            return Err((
                StatusCode::NOT_FOUND,
                format!(
                    "<{}> has no <{}> to delete in <{data_graph}>",
                    body.entity, body.property
                ),
            ));
        }
    }
    let now = chrono::Utc::now().to_rfc3339();
    let valid_from = match &body.valid_from {
        Some(v) => parse_time(v)?,
        None => now.clone(),
    };
    let states = ensure_states_graph(&state, &dataset_id)?;
    let prop = managed_property_node(&state, &states, &body.entity, &body.property)?;
    let st = format!("urn:ots:property-state:{}", uuid::Uuid::new_v4());
    let write = StateWrite {
        entity: body.entity.clone(),
        property: body.property.clone(),
        value: None,
        valid_from: valid_from.clone(),
        recorded_at: now.clone(),
        agent: agent_iri(&state, &user.user_id),
        reliability: None,
        note: body.note.clone(),
        documentation,
        data_graph: data_graph.clone(),
        revision_of: None,
        extra: Vec::new(),
    };
    apply_writes(
        &state,
        &dataset_id,
        &user.user_id,
        vec![state_update(&states, &prop, &st, &write)],
        vec![data_graph.clone()],
        format!("Property deleted: <{}> <{}>", body.entity, body.property),
        1,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "dataset_id": dataset_id,
            "entity": body.entity,
            "property": body.property,
            "property_iri": prop,
            "state": st,
            "deleted": true,
            "valid_from": valid_from,
            "recorded_at": now,
            "data_graph": data_graph,
        })),
    ))
}

/// POST /api/datasets/:id/properties/restore
///
/// Undo a deletion: a new current state carrying the value, reliability and
/// documentation of the last state before the deletion
/// (`prov:wasRevisionOf` it), and the plain triple back in the data graph.
pub async fn restore_property(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<LifecycleBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = writable_dataset(&state, &user, &dataset_id)?;
    iri(&body.entity, "entity")?;
    iri(&body.property, "property")?;
    let extra_docs = documentation_iris(body.documentation.as_ref())?;
    let scope = read_scope(&state, Some(&user.user_id), &ds)?;
    let (_, existing) = states_of_property(&state, &scope, &body.entity, &body.property)?;
    let Some(current) = existing.iter().find(|s| !s.canonical && s.current) else {
        return Err((
            StatusCode::NOT_FOUND,
            format!(
                "<{}> <{}> has no recorded state",
                body.entity, body.property
            ),
        ));
    };
    if !current.deleted {
        return Err(conflict(format!(
            "<{}> <{}> is not deleted",
            body.entity, body.property
        )));
    }
    // The newest state with a value recorded before the deletion.
    let cut = current.recorded_key();
    let Some(previous) = existing
        .iter()
        .filter(|s| {
            !s.deleted && s.value.is_some() && s.recorded_key() <= cut && s.iri != current.iri
        })
        .max_by_key(|s| s.recorded_key())
    else {
        return Err(conflict(format!(
            "<{}> <{}> has no earlier value to restore",
            body.entity, body.property
        )));
    };
    let data_graph = write_graph(
        &state,
        &dataset_id,
        body.graph.as_deref(),
        Some(previous)
            .filter(|p| p.data_graph.is_some())
            .or(Some(current)),
    )?;
    let now = chrono::Utc::now().to_rfc3339();
    let valid_from = match &body.valid_from {
        Some(v) => parse_time(v)?,
        None => now.clone(),
    };
    let mut documentation: Vec<String> = previous.documentation.iter().cloned().collect();
    documentation.extend(extra_docs);
    documentation.sort();
    documentation.dedup();
    let value = previous.value.clone().expect("filtered on a value");
    let states = ensure_states_graph(&state, &dataset_id)?;
    let prop = managed_property_node(&state, &states, &body.entity, &body.property)?;
    let st = format!("urn:ots:property-state:{}", uuid::Uuid::new_v4());
    let write = StateWrite {
        entity: body.entity.clone(),
        property: body.property.clone(),
        value: Some(value.to_string()),
        valid_from: valid_from.clone(),
        recorded_at: now.clone(),
        agent: agent_iri(&state, &user.user_id),
        reliability: previous.reliability.iter().next().copied(),
        note: body.note.clone().or_else(|| previous.note.clone()),
        documentation,
        data_graph: data_graph.clone(),
        revision_of: Some(previous.iri.clone()),
        extra: Vec::new(),
    };
    apply_writes(
        &state,
        &dataset_id,
        &user.user_id,
        vec![state_update(&states, &prop, &st, &write)],
        vec![data_graph.clone()],
        format!("Property restored: <{}> <{}>", body.entity, body.property),
        1,
    )
    .await?;
    let (v, datatype, language) = value_parts(&value);
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "dataset_id": dataset_id,
            "entity": body.entity,
            "property": body.property,
            "property_iri": prop,
            "state": st,
            "restored_from": previous.iri,
            "value": v,
            "datatype": datatype,
            "language": language,
            "valid_from": valid_from,
            "recorded_at": now,
            "data_graph": data_graph,
        })),
    ))
}

// ─── Read endpoints ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub entity: String,
    pub property: String,
    /// as-of only.
    pub at: Option<String>,
}

/// GET /api/datasets/:id/properties/history?entity=&property=
pub async fn history(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = visible_dataset(&state, uid, &dataset_id)?;
    let scope = read_scope(&state, uid, &ds)?;
    let (prop, states) = states_of_property(&state, &scope, &q.entity, &q.property)?;
    let states: Vec<StateView> = states.iter().map(StoredState::view).collect();
    Ok(Json(serde_json::json!({
        "entity": q.entity,
        "property": q.property,
        "property_iri": prop,
        "states": states,
    })))
}

/// GET /api/datasets/:id/properties/as-of?entity=&property=&at=
///
/// The state valid at `at`. A property deleted by then answers with its
/// `opm:Deleted` state (`deleted: true`, `value: null`); one with no state
/// valid yet is a 404.
pub async fn as_of(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = visible_dataset(&state, uid, &dataset_id)?;
    let at = parse_time(q.at.as_deref().ok_or_else(|| bad("`at` is required"))?)?;
    let scope = read_scope(&state, uid, &ds)?;
    let (prop, states) = states_of_property(&state, &scope, &q.entity, &q.property)?;
    let group = PropertyStates {
        entity: q.entity.clone(),
        property: q.property.clone(),
        prop,
        states,
    };
    let Some(s) = group.at(&at) else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("no state of <{}> <{}> valid at {at}", q.entity, q.property),
        ));
    };
    Ok(Json(serde_json::json!({
        "entity": q.entity,
        "property": q.property,
        "at": at,
        "state": s.view(),
    })))
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Only this item's properties.
    pub entity: Option<String>,
    /// Only properties of this kind.
    pub property: Option<String>,
    /// `assumed` | `confirmed` | `derived` | `required`.
    pub reliability: Option<String>,
    /// `false` (default for latest and as-of), `true` (only deleted) or `any`.
    pub deleted: Option<String>,
    /// `true`: only derived states (`opm:Derived` or with an expression);
    /// `false`: none of them.
    pub derived: Option<String>,
    /// `latest` (default): one state per property; `full`: every state.
    pub history: Option<String>,
    /// A snapshot at this time: per property the state valid then.
    pub at: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

fn tri_bool(v: Option<&str>, name: &str) -> Result<Option<bool>, ApiErr> {
    match v.map(str::to_ascii_lowercase).as_deref() {
        None | Some("any") => Ok(None),
        Some("true") => Ok(Some(true)),
        Some("false") => Ok(Some(false)),
        Some(other) => Err(bad(format!(
            "`{name}` must be true, false or any, not `{other}`"
        ))),
    }
}

/// GET /api/datasets/:id/properties
///
/// Every property of the dataset — of one item (`entity`), of one kind
/// (`property`), or both — with its latest state, its full history
/// (`history=full`) or its state at a time (`at`; with `entity` this is the
/// item's snapshot). Filters apply to the reported states.
pub async fn list_properties(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(q): Query<ListQuery>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = visible_dataset(&state, uid, &dataset_id)?;
    let full = match q.history.as_deref() {
        None | Some("latest") => false,
        Some("full") => true,
        Some(other) => {
            return Err(bad(format!(
                "`history` must be latest or full, not `{other}`"
            )))
        }
    };
    let at = q.at.as_deref().map(parse_time).transpose()?;
    let reliability = q
        .reliability
        .as_deref()
        .map(Reliability::parse)
        .transpose()?;
    let deleted = match q.deleted.as_deref() {
        // A snapshot shows what the item has; a full history shows it all.
        None if !full => Some(false),
        v => tri_bool(v, "deleted")?,
    };
    let derived = tri_bool(q.derived.as_deref(), "derived")?;
    let limit = q.limit.unwrap_or(1000).clamp(1, MAX_LIST_LIMIT);
    let offset = q.offset.unwrap_or(0);
    let scope = read_scope(&state, uid, &ds)?;
    let groups = group_states(read_states(
        &state,
        &scope,
        q.entity.as_deref(),
        q.property.as_deref(),
    )?);
    let keep = |s: &StoredState| {
        reliability.is_none_or(|r| s.reliability.contains(&r))
            && deleted.is_none_or(|d| s.deleted == d)
            && derived.is_none_or(|d| {
                (s.reliability.contains(&Reliability::Derived) || s.expression.is_some()) == d
            })
    };
    let mut out = Vec::new();
    for g in &groups {
        let chosen: Vec<&StoredState> = if full {
            g.states
                .iter()
                .filter(|s| {
                    at.as_deref().is_none_or(|t| {
                        instant(&s.valid_from).is_some_and(|v| Some(v) <= instant(t))
                    })
                })
                .collect()
        } else {
            match at.as_deref() {
                Some(t) => g.at(t).into_iter().collect(),
                None => g.current().into_iter().collect(),
            }
        };
        let views: Vec<StateView> = chosen
            .into_iter()
            .filter(|s| keep(s))
            .map(StoredState::view)
            .collect();
        if views.is_empty() {
            continue;
        }
        out.push(serde_json::json!({
            "entity": g.entity,
            "property": g.property,
            "property_iri": g.prop,
            "states": views,
        }));
    }
    let total = out.len();
    let page: Vec<_> = out.into_iter().skip(offset).take(limit).collect();
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "history": if full { "full" } else { "latest" },
        "at": at,
        "total": total,
        "offset": offset,
        "limit": limit,
        "properties": page,
    })))
}

/// GET /api/datasets/:id/properties/validate
///
/// Validate the dataset's states graph against the OPM profile shapes
/// ([`OPM_PROFILE_SHAPES`]), in a scratch store: the states graph is copied,
/// nothing is written.
pub async fn validate_states(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = visible_dataset(&state, uid, &dataset_id)?;
    let states = states_graph(&dataset_id);
    let scope = read_scope(&state, uid, &ds)?;
    let quads = if scope.graphs.contains(&states) {
        state
            .store
            .quads_for_graph(iri(&states, "graph")?.as_ref().into())
            .map_err(e500)?
    } else {
        Vec::new()
    };
    let report = tokio::task::spawn_blocking(move || validate_profile(quads))
        .await
        .map_err(e500)??;
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "states_graph": states,
        "report": report,
    })))
}

/// The OPM profile over `quads` (any graph), in a scratch in-memory store.
pub fn validate_profile(
    quads: Vec<oxigraph::model::Quad>,
) -> Result<crate::shacl::report::ValidationReport, ApiErr> {
    const SHAPES: &str = "urn:ots:opm-profile:shapes";
    const DATA: &str = "urn:ots:opm-profile:data";
    let scratch = crate::store::TripleStore::in_memory().map_err(e500)?;
    scratch
        .load_str(
            OPM_PROFILE_SHAPES,
            oxigraph::io::RdfFormat::Turtle,
            Some(SHAPES),
        )
        .map_err(e500)?;
    let data = NamedNode::new(DATA).map_err(e500)?;
    let quads: Vec<_> = quads
        .into_iter()
        .map(|q| oxigraph::model::Quad::new(q.subject, q.predicate, q.object, data.clone()))
        .collect();
    if !quads.is_empty() {
        scratch.insert_quads(quads).map_err(e500)?;
    }
    crate::shacl::validate(&scratch, SHAPES, &[DATA.to_string()]).map_err(e500)
}

/// GET /api/properties/profile — the OPM profile shapes as Turtle.
pub async fn profile_shapes() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/turtle; charset=utf-8")],
        OPM_PROFILE_SHAPES,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_terms_are_inferred_or_explicit() {
        assert_eq!(value_term("true", None, None).unwrap(), "true");
        assert_eq!(value_term("42", None, None).unwrap(), "42");
        assert_eq!(
            value_term("2.5", None, None).unwrap(),
            "\"2.5\"^^<http://www.w3.org/2001/XMLSchema#decimal>"
        );
        assert_eq!(value_term("REI60", None, None).unwrap(), "\"REI60\"");
        assert_eq!(
            value_term("Voorbeeldbrug", None, Some("nl")).unwrap(),
            "\"Voorbeeldbrug\"@nl"
        );
        assert_eq!(
            value_term("12", Some("xsd:integer"), None).unwrap(),
            "\"12\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        );
        assert_eq!(value_term("urn:x", Some("iri"), None).unwrap(), "<urn:x>");
        assert!(value_term("not an iri", Some("iri"), None).is_err());
    }

    /// The language tag is the one piece of a literal that is written raw
    /// after the closing quote; it must be a tag and nothing else.
    #[test]
    fn language_tags_and_datatypes_are_validated() {
        assert_eq!(
            value_term("Bridge", None, Some("en-GB")).unwrap(),
            "\"Bridge\"@en-GB"
        );
        let (st, msg) = value_term(
            "v",
            None,
            Some(
                "en } } WHERE { } ; INSERT DATA { GRAPH <urn:probe> { <urn:s> <urn:p> <urn:o> } }",
            ),
        )
        .unwrap_err();
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(msg.contains("language tag"), "{msg}");
        assert!(value_term("v", None, Some("")).is_err());
        assert!(value_term("v", Some("xsd:int> <urn:p> <urn:o"), None).is_err());
        // Tabs are escaped too (the old local escaper let them through raw).
        assert_eq!(value_term("a\tb", None, None).unwrap(), "\"a\\tb\"");
    }

    #[test]
    fn times_accept_rfc3339_and_dates() {
        assert_eq!(
            parse_time("2026-03-01").unwrap(),
            "2026-03-01T00:00:00+00:00"
        );
        assert!(parse_time("2026-03-01T10:00:00Z").is_ok());
        assert!(parse_time("yesterday").is_err());
        assert!(instant("2018-02-03T13:35:23Z").is_some());
        assert!(instant("2018-02-03T13:35:23").is_some());
    }

    #[test]
    fn property_iris_are_stable() {
        assert_eq!(
            property_iri("urn:e", "urn:p"),
            property_iri("urn:e", "urn:p")
        );
        assert_ne!(
            property_iri("urn:e", "urn:p"),
            property_iri("urn:e", "urn:q")
        );
    }

    #[test]
    fn reliability_names_round_trip() {
        for r in [
            Reliability::Assumed,
            Reliability::Confirmed,
            Reliability::Derived,
            Reliability::Required,
        ] {
            assert_eq!(Reliability::parse(r.name()).unwrap(), r);
            assert_eq!(
                Reliability::from_iri(&format!("{OPM}{}", r.class())),
                Some(r)
            );
        }
        assert!(Reliability::parse("guessed").is_err());
    }

    /// The profile shapes parse and an empty states graph conforms.
    #[test]
    fn profile_shapes_load() {
        let report = validate_profile(Vec::new()).unwrap();
        assert!(report.conforms, "{:?}", report.results);
    }
}
