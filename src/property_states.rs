//! Time-evolving properties: an OPM-style profile for any dataset.
//!
//! A property value that changes over time — a bridge's load rating, a
//! patient's weight, a device's firmware — is recorded as a chain of
//! `opm:PropertyState`s in the dataset's *states graph* (role `provenance`),
//! each with its value, `ots:validFrom`, recording time, agent, reliability
//! and note. The data graph always carries the **current** value as a plain
//! triple, so SPARQL, SHACL and reasoning see nothing new; the history and
//! "as of" views read the states.
//!
//! * `POST /api/datasets/:id/properties/state`   — set a new state
//! * `GET  /api/datasets/:id/properties/history` — every state, newest first
//! * `GET  /api/datasets/:id/properties/as-of`   — the state valid at a time
//!
//! Vocabulary: OPM (<https://w3id.org/opm#>) for the property/state model
//! and reliability classes, `schema:value` for the value, PROV for time and
//! attribution. Domain vocabularies (material passports, clinical records)
//! supply the property IRIs; nothing here knows them.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use oxigraph::model::{NamedNode, Term};
use oxigraph::sparql::QueryResults;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, GraphKind};
use crate::server::AppState;
use crate::store::escape_sparql_iri;

pub const OPM: &str = "https://w3id.org/opm#";
pub const SCHEMA: &str = "https://schema.org/";
pub const PROV: &str = "http://www.w3.org/ns/prov#";
pub const OTS: &str = "https://opentriplestore.org/ns#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";

type ApiErr = (StatusCode, String);

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
fn bad(msg: impl Into<String>) -> ApiErr {
    (StatusCode::BAD_REQUEST, msg.into())
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

fn esc_lit(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// The value as a SPARQL term.
fn value_term(
    value: &str,
    datatype: Option<&str>,
    language: Option<&str>,
) -> Result<String, ApiErr> {
    if let Some(lang) = language {
        return Ok(format!("\"{}\"@{lang}", esc_lit(value)));
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
                esc_lit(value),
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
                Ok(format!("\"{}\"", esc_lit(value)))
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

fn visible_dataset(state: &AppState, uid: Option<&str>, id: &str) -> Result<Dataset, ApiErr> {
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

/// The graph a state's current value goes to when the caller names none:
/// the dataset's instances graph, else its first graph without a model-layer
/// role, else its first graph.
fn default_data_graph(state: &AppState, dataset_id: &str) -> Option<String> {
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

/// Register the states graph (role `provenance`) on first use.
fn ensure_states_graph(state: &AppState, dataset_id: &str) -> Result<String, ApiErr> {
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

#[derive(Debug, Deserialize)]
pub struct SetStateBody {
    pub entity: String,
    pub property: String,
    pub value: String,
    /// An XSD datatype (`xsd:decimal` or a full IRI), or `iri` for an IRI value.
    pub datatype: Option<String>,
    pub language: Option<String>,
    /// The data graph holding the current value (default: see
    /// [`default_data_graph`]).
    pub graph: Option<String>,
    /// When the value became true (default: now).
    pub valid_from: Option<String>,
    /// `assumed` | `confirmed` | `derived` (OPM reliability).
    pub reliability: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StateView {
    pub state: String,
    pub value: String,
    pub datatype: Option<String>,
    pub language: Option<String>,
    pub valid_from: String,
    pub recorded_at: String,
    pub attributed_to: Option<String>,
    pub reliability: Option<String>,
    pub note: Option<String>,
    pub current: bool,
}

/// POST /api/datasets/:id/properties/state
pub async fn set_state(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<SetStateBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = visible_dataset(&state, Some(&user.user_id), &dataset_id)?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    NamedNode::new(&body.entity).map_err(|e| bad(format!("entity: {e}")))?;
    NamedNode::new(&body.property).map_err(|e| bad(format!("property: {e}")))?;
    let graphs = state
        .auth_db
        .list_dataset_graphs(&dataset_id)
        .map_err(e500)?;
    let data_graph = match &body.graph {
        Some(g) => {
            if !graphs.iter().any(|x| x == g) {
                return Err(bad(format!(
                    "graph <{g}> is not registered to dataset {dataset_id}"
                )));
            }
            g.clone()
        }
        None => default_data_graph(&state, &dataset_id).ok_or_else(|| {
            bad("the dataset has no graph to hold the current value; register one or pass `graph`")
        })?,
    };
    let states = ensure_states_graph(&state, &dataset_id)?;
    let term = value_term(
        &body.value,
        body.datatype.as_deref(),
        body.language.as_deref(),
    )?;
    let now = chrono::Utc::now().to_rfc3339();
    let valid_from = match &body.valid_from {
        Some(v) => parse_time(v)?,
        None => now.clone(),
    };
    let reliability = match body
        .reliability
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None => None,
        Some("assumed") => Some("Assumed"),
        Some("confirmed") => Some("Confirmed"),
        Some("derived") => Some("Derived"),
        Some(other) => {
            return Err(bad(format!(
                "reliability `{other}` is not assumed|confirmed|derived"
            )))
        }
    };
    let prop = property_iri(&body.entity, &body.property);
    let st = format!("urn:ots:property-state:{}", uuid::Uuid::new_v4());
    let e = escape_sparql_iri(&body.entity);
    let p = escape_sparql_iri(&body.property);
    let agent = format!(
        "{}/users/{}",
        state.base_url.trim_end_matches('/'),
        user.user_id
    );
    let mut state_lines = vec![
        format!("<{st}> a opm:PropertyState, opm:CurrentPropertyState"),
        format!("<{st}> schema:value {term}"),
        format!("<{st}> ots:validFrom \"{valid_from}\"^^xsd:dateTime"),
        format!("<{st}> prov:generatedAtTime \"{now}\"^^xsd:dateTime"),
        format!("<{st}> prov:wasAttributedTo <{agent}>"),
    ];
    if let Some(r) = reliability {
        state_lines.push(format!("<{st}> a opm:{r}"));
    }
    if let Some(n) = &body.note {
        state_lines.push(format!("<{st}> rdfs:comment \"{}\"", esc_lit(n)));
    }
    let state_block = state_lines.join(" .\n        ");
    let update = format!(
        r#"PREFIX opm: <{OPM}>
PREFIX schema: <{SCHEMA}>
PREFIX prov: <{PROV}>
PREFIX ots: <{OTS}>
PREFIX xsd: <{XSD}>
PREFIX rdfs: <{RDFS}>
DELETE {{
    GRAPH <{data_graph}> {{ <{e}> <{p}> ?old }}
    GRAPH <{states}> {{ ?cur a opm:CurrentPropertyState }}
}}
INSERT {{
    GRAPH <{data_graph}> {{ <{e}> <{p}> {term} }}
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
    OPTIONAL {{ GRAPH <{data_graph}> {{ <{e}> <{p}> ?old }} }}
    OPTIONAL {{ GRAPH <{states}> {{ <{prop}> opm:hasPropertyState ?cur . ?cur a opm:CurrentPropertyState }} }}
}}"#
    );
    state
        .store
        .update(&update)
        .map_err(|e| bad(format!("state update failed: {e}")))?;
    {
        let st = state.clone();
        let g = vec![data_graph.clone()];
        let _ = tokio::task::spawn_blocking(move || crate::entailment::after_write(&st, &g)).await;
    }
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Dataset,
        format!(
            "Property state: <{}> <{}> = {}",
            body.entity, body.property, body.value
        ),
        Some(&user.user_id),
        Some(format!(
            "{}/dataset/{}",
            state.base_url.trim_end_matches('/'),
            dataset_id
        )),
        vec![data_graph.clone(), states.clone()],
        1,
        0,
        None,
    );
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
            "reliability": reliability.map(|r| r.to_ascii_lowercase()),
        })),
    ))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub entity: String,
    pub property: String,
    /// as-of only.
    pub at: Option<String>,
}

fn states_of(
    state: &AppState,
    dataset_id: &str,
    entity: &str,
    property: &str,
    at: Option<&str>,
) -> Result<Vec<StateView>, ApiErr> {
    let prop = property_iri(entity, property);
    let states = states_graph(dataset_id);
    let filter = match at {
        Some(t) => format!("FILTER(?vf <= \"{t}\"^^xsd:dateTime)"),
        None => String::new(),
    };
    let q = format!(
        r#"PREFIX opm: <{OPM}>
PREFIX schema: <{SCHEMA}>
PREFIX prov: <{PROV}>
PREFIX ots: <{OTS}>
PREFIX xsd: <{XSD}>
PREFIX rdfs: <{RDFS}>
SELECT ?s ?v ?vf ?rec ?who ?note ?rel ?cur WHERE {{
  GRAPH <{states}> {{
    <{prop}> opm:hasPropertyState ?s .
    ?s schema:value ?v ; ots:validFrom ?vf ; prov:generatedAtTime ?rec .
    OPTIONAL {{ ?s prov:wasAttributedTo ?who }}
    OPTIONAL {{ ?s rdfs:comment ?note }}
    OPTIONAL {{ ?s a ?rel . FILTER(?rel IN (opm:Assumed, opm:Confirmed, opm:Derived)) }}
    BIND(EXISTS {{ ?s a opm:CurrentPropertyState }} AS ?cur)
    {filter}
  }}
}} ORDER BY DESC(?vf) DESC(?rec)"#
    );
    let mut out = Vec::new();
    if let QueryResults::Solutions(sols) = state.store.query(&q).map_err(e500)? {
        for row in sols {
            let row = row.map_err(e500)?;
            let s = |k: &str| row.get(k).map(term_string);
            let (value, datatype, language) = match row.get("v") {
                Some(Term::Literal(l)) => (
                    l.value().to_string(),
                    if l.language().is_some() {
                        None
                    } else {
                        Some(l.datatype().as_str().to_string())
                    },
                    l.language().map(str::to_string),
                ),
                Some(Term::NamedNode(n)) => (n.as_str().to_string(), Some("iri".to_string()), None),
                Some(other) => (other.to_string(), None, None),
                None => continue,
            };
            out.push(StateView {
                state: s("s").unwrap_or_default(),
                value,
                datatype,
                language,
                valid_from: s("vf").unwrap_or_default(),
                recorded_at: s("rec").unwrap_or_default(),
                attributed_to: s("who"),
                reliability: s("rel").map(|r| r.trim_start_matches(OPM).to_ascii_lowercase()),
                note: s("note"),
                current: matches!(row.get("cur"), Some(Term::Literal(l)) if l.value() == "true"),
            });
        }
    }
    Ok(out)
}

fn term_string(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        other => other.to_string(),
    }
}

/// GET /api/datasets/:id/properties/history?entity=&property=
pub async fn history(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    visible_dataset(&state, uid, &dataset_id)?;
    let states = states_of(&state, &dataset_id, &q.entity, &q.property, None)?;
    Ok(Json(serde_json::json!({
        "entity": q.entity,
        "property": q.property,
        "property_iri": property_iri(&q.entity, &q.property),
        "states": states,
    })))
}

/// GET /api/datasets/:id/properties/as-of?entity=&property=&at=
pub async fn as_of(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    visible_dataset(&state, uid, &dataset_id)?;
    let at = parse_time(q.at.as_deref().ok_or_else(|| bad("`at` is required"))?)?;
    let mut states = states_of(&state, &dataset_id, &q.entity, &q.property, Some(&at))?;
    if states.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("no state of <{}> <{}> valid at {at}", q.entity, q.property),
        ));
    }
    let s = states.remove(0);
    Ok(Json(serde_json::json!({
        "entity": q.entity,
        "property": q.property,
        "at": at,
        "state": s,
    })))
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
            value_term("Waalbrug", None, Some("nl")).unwrap(),
            "\"Waalbrug\"@nl"
        );
        assert_eq!(
            value_term("12", Some("xsd:integer"), None).unwrap(),
            "\"12\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        );
        assert_eq!(value_term("urn:x", Some("iri"), None).unwrap(), "<urn:x>");
        assert!(value_term("not an iri", Some("iri"), None).is_err());
    }

    #[test]
    fn times_accept_rfc3339_and_dates() {
        assert_eq!(
            parse_time("2026-03-01").unwrap(),
            "2026-03-01T00:00:00+00:00"
        );
        assert!(parse_time("2026-03-01T10:00:00Z").is_ok());
        assert!(parse_time("yesterday").is_err());
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
}
