//! Review decisions on a mapping, as PROV, and the calibration they feed.
//!
//! A proposer writes a mapping in the `proposed` state with a confidence it
//! computed; a reviewer decides. **Approve**, **edit** and **reject** are three
//! distinct outcomes — an edit says the proposal was close but not right,
//! which is the training signal a proposer needs most — and each is recorded
//! as its own `ds:ReviewDecision` activity in `urn:system:sources`, naming the
//! mapping version it judged, the reviewer, the confidence the proposal
//! carried and the note the reviewer left. The list of decisions is what the
//! proposer's training job reads back; the mapping's PROV trail serves the
//! same records as Turtle.
//!
//! **Calibration.** A confidence is only as good as its track record. Given
//! decisions with a confidence (or points supplied directly), the calibration
//! endpoint fits a monotone map from stated confidence to observed acceptance
//! rate — isotonic regression by pool-adjacent-violators — which the proposer
//! applies before it compares a score with the gates. One-class data cannot
//! be calibrated: a set of only approvals says the proposer was never wrong,
//! and a curve fitted to it would say so for every confidence. It is refused.

use axum::extract::{Path, State};
use axum::http::{header::CONTENT_TYPE, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use oxigraph::sparql::QueryResults;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;
use crate::store::{escape_sparql_iri, escape_sparql_literal, TripleStore};

use super::model::*;
use super::registry;

const PROV: &str = "http://www.w3.org/ns/prov#";
const DCT: &str = "http://purl.org/dc/terms/";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn prefixes() -> String {
    format!("PREFIX ds: <{DS}>\nPREFIX prov: <{PROV}>\nPREFIX dct: <{DCT}>\nPREFIX xsd: <{XSD}>\n")
}

fn lex(row: &oxigraph::sparql::QuerySolution, var: &str) -> Option<String> {
    use oxigraph::model::Term;
    match row.get(var)? {
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        Term::Literal(l) => Some(l.value().to_string()),
        Term::BlankNode(b) => Some(format!("_:{}", b.as_str())),
        #[cfg(feature = "rdf-12")]
        Term::Triple(_) => None,
    }
}

// ───────────────────────────── Decisions ─────────────────────────────

/// What a reviewer decided about a proposed mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The proposal stands as written; the mapping is approved.
    Approve,
    /// The proposal was close: the reviewer changed it before accepting.
    /// The mapping's state is left to the edit itself (`PUT /api/mappings/:id`).
    Edit,
    /// The proposal is wrong; the mapping is rejected.
    Reject,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Approve => "approve",
            Outcome::Edit => "edit",
            Outcome::Reject => "reject",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "approve" | "approved" => Some(Outcome::Approve),
            "edit" | "edited" => Some(Outcome::Edit),
            "reject" | "rejected" => Some(Outcome::Reject),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionRequest {
    /// `approve`, `edit` or `reject`.
    pub decision: String,
    /// What the decision is about, when narrower than the whole mapping: a
    /// triples map IRI, a predicate, a column.
    pub target: Option<String>,
    /// The confidence the proposal carried, in `0..=1`. Calibration reads it.
    pub confidence: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub id: String,
    pub iri: String,
    /// The mapping id.
    pub mapping: String,
    /// The version the decision judged.
    pub mapping_version: u32,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    pub decided_at: String,
}

pub fn decision_iri(mapping_id: &str, id: &str) -> String {
    format!("urn:mapping:{mapping_id}:decision:{id}")
}

fn decision_select(filter: &str) -> String {
    format!(
        "{}SELECT ?d ?id ?mapping ?version ?outcome ?target ?confidence ?note ?actor ?at ?seq \
         WHERE {{ GRAPH <{SOURCES_GRAPH}> {{\n\
           ?d a ds:ReviewDecision ; ds:id ?id ; ds:mapping ?mapping ; ds:outcome ?outcome ;\n\
              ds:sequence ?seq ; prov:endedAtTime ?at ; prov:used ?v .\n\
           ?v ds:version ?version .\n\
           {filter}\n\
           OPTIONAL {{ ?d ds:target ?target }}\n\
           OPTIONAL {{ ?d ds:confidence ?confidence }}\n\
           OPTIONAL {{ ?d ds:note ?note }}\n\
           OPTIONAL {{ ?d prov:wasAssociatedWith ?actor }}\n\
         }} }} ORDER BY DESC(?seq)",
        prefixes()
    )
}

fn decisions(store: &TripleStore, filter: &str) -> Vec<Decision> {
    let Ok(QueryResults::Solutions(rows)) = store.query(&decision_select(filter)) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|row| {
            let id = lex(&row, "id")?;
            let mapping = lex(&row, "mapping")?
                .trim_start_matches("urn:mapping:")
                .to_string();
            Some(Decision {
                iri: decision_iri(&mapping, &id),
                mapping_version: lex(&row, "version").and_then(|v| v.parse().ok())?,
                outcome: lex(&row, "outcome")?,
                target: lex(&row, "target"),
                confidence: lex(&row, "confidence").and_then(|c| c.parse().ok()),
                note: lex(&row, "note"),
                actor: lex(&row, "actor"),
                decided_at: lex(&row, "at")?,
                mapping,
                id,
            })
        })
        .collect()
}

/// Every decision on a mapping, newest first.
pub fn list(store: &TripleStore, mapping_id: &str) -> Vec<Decision> {
    decisions(
        store,
        &format!(
            "FILTER(?mapping = <{}>)",
            escape_sparql_iri(&mapping_iri(mapping_id))
        ),
    )
}

/// Every decision in the store that carries a confidence, as
/// `(confidence, accepted)`: the calibration set. Only an approval counts as
/// accepted — an edited proposal was not right as proposed.
pub fn calibration_points(store: &TripleStore) -> Vec<(f64, bool)> {
    decisions(store, "")
        .into_iter()
        .filter_map(|d| Some((d.confidence?, d.outcome == "approve")))
        .collect()
}

/// Record a decision as a PROV activity that `prov:used` the mapping version
/// it judged. Returns the record.
pub fn record(
    store: &TripleStore,
    mapping: &MappingRecord,
    outcome: Outcome,
    target: Option<&str>,
    confidence: Option<f64>,
    note: Option<&str>,
    actor: Option<&str>,
) -> Result<Decision, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let iri = decision_iri(&mapping.id, &id);
    let now = registry::now();
    // Decisions are listed newest first; a sequence number orders two taken
    // within the same clock tick.
    let sequence = list(store, &mapping.id).len() + 1;
    let mut body = format!(
        "  <{}> a prov:Activity, ds:ReviewDecision ;\n    ds:id \"{}\" ;\n    ds:mapping <{}> ;\n    ds:outcome \"{}\" ;\n    ds:sequence \"{sequence}\"^^xsd:integer ;\n    prov:used <{}> ;\n    prov:startedAtTime \"{now}\"^^xsd:dateTime ;\n    prov:endedAtTime \"{now}\"^^xsd:dateTime ;\n",
        escape_sparql_iri(&iri),
        escape_sparql_literal(&id),
        escape_sparql_iri(&mapping.iri()),
        outcome.as_str(),
        escape_sparql_iri(&mapping.version_iri()),
        now = escape_sparql_literal(&now),
    );
    if let Some(t) = target.map(str::trim).filter(|t| !t.is_empty()) {
        // An IRI target is stored as one, so a query can join it; anything
        // else (a column name, a label) is a literal.
        let is_iri = t.contains(':') && oxigraph::model::NamedNode::new(t).is_ok();
        if is_iri {
            body.push_str(&format!("    ds:target <{}> ;\n", escape_sparql_iri(t)));
        } else {
            body.push_str(&format!(
                "    ds:target \"{}\" ;\n",
                escape_sparql_literal(t)
            ));
        }
    }
    if let Some(c) = confidence {
        body.push_str(&format!("    ds:confidence \"{c}\"^^xsd:decimal ;\n"));
    }
    if let Some(n) = note.map(str::trim).filter(|n| !n.is_empty()) {
        body.push_str(&format!("    ds:note \"{}\" ;\n", escape_sparql_literal(n)));
    }
    if let Some(a) = actor {
        body.push_str(&format!(
            "    prov:wasAssociatedWith <{}> ;\n",
            escape_sparql_iri(a)
        ));
    }
    body.push_str(&format!(
        "    dct:created \"{}\" .\n",
        escape_sparql_literal(&now)
    ));
    let sparql = format!(
        "{}INSERT DATA {{ GRAPH <{SOURCES_GRAPH}> {{\n{body}}} }}",
        prefixes()
    );
    store.update(&sparql).map_err(|e| e.to_string())?;
    Ok(Decision {
        id,
        iri,
        mapping: mapping.id.clone(),
        mapping_version: mapping.version,
        outcome: outcome.as_str().to_string(),
        target: target
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string),
        confidence,
        note: note
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string),
        actor: actor.map(str::to_string),
        decided_at: now,
    })
}

/// Every decision on a mapping, for the delete cascade.
pub fn delete_for_mapping(store: &TripleStore, mapping_id: &str) -> Result<(), String> {
    let mapping = escape_sparql_iri(&mapping_iri(mapping_id));
    store
        .update(&format!(
            "{}DELETE {{ GRAPH <{SOURCES_GRAPH}> {{ ?d ?p ?o }} }} WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ ?d a ds:ReviewDecision ; ds:mapping <{mapping}> ; ?p ?o }} }}",
            prefixes()
        ))
        .map_err(|e| e.to_string())
}

/// A mapping's PROV-O trail as Turtle: the mapping, its frozen versions, the
/// runs that used them and the decisions taken on them.
///
/// The records live in `urn:system:sources`, outside a caller's SPARQL scope,
/// so this is how a client — the proposer's training job above all — follows
/// a mapping's history.
pub fn provenance_turtle(store: &TripleStore, mapping_id: &str) -> String {
    let mapping = escape_sparql_iri(&mapping_iri(mapping_id));
    let query = format!(
        "PREFIX ds: <{DS}>\n\
         CONSTRUCT {{ ?s ?p ?o }} WHERE {{ GRAPH <{SOURCES_GRAPH}> {{\n\
           {{ ?s ?p ?o FILTER(?s = <{mapping}>) }}\n\
           UNION {{ ?s ds:mapping <{mapping}> ; ?p ?o }}\n\
         }} }}"
    );
    super::runs::prov_turtle(store, &query)
}

// ───────────────────────────── Calibration ─────────────────────────────

#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationPoint {
    /// The confidence the proposal carried, in `0..=1`.
    pub confidence: f64,
    /// Whether the reviewer accepted it as proposed.
    pub accepted: bool,
}

#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CalibrationRequest {
    /// Points to fit. Absent: every recorded decision that carries a
    /// confidence, an approval counting as accepted.
    pub points: Option<Vec<CalibrationPoint>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CurvePoint {
    pub confidence: f64,
    /// The acceptance rate the fit assigns to this confidence.
    pub calibrated: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Calibration {
    /// `body` or `decisions`.
    pub source: &'static str,
    pub points: usize,
    pub positives: usize,
    pub negatives: usize,
    /// One point per distinct confidence, ascending; `calibrated` never
    /// decreases along it. A confidence between two points takes the lower
    /// point's value.
    pub curve: Vec<CurvePoint>,
    /// Mean squared error of the stated confidences against the outcomes,
    /// before and after the fit.
    pub brier_score: BrierScore,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BrierScore {
    pub raw: f64,
    pub calibrated: f64,
}

/// Why a set of points cannot be calibrated.
#[derive(Debug, PartialEq)]
pub enum CalibrationError {
    /// Fewer than two points.
    TooFew(usize),
    /// Every point has the same outcome.
    OneClass { positives: usize, negatives: usize },
}

impl std::fmt::Display for CalibrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CalibrationError::TooFew(0) => f.write_str(
                "no decisions with a confidence are recorded yet; calibration needs both \
                 accepted and refused proposals",
            ),
            CalibrationError::TooFew(n) => {
                write!(f, "calibration needs at least two points, got {n}")
            }
            CalibrationError::OneClass {
                positives,
                negatives,
            } => write!(
                f,
                "one-class data: {positives} accepted and {negatives} refused; a curve fitted \
                 to a single outcome would assign it to every confidence — calibration needs both"
            ),
        }
    }
}

/// Isotonic regression by pool-adjacent-violators: the non-decreasing step
/// function of confidence that best fits the outcomes in least squares.
/// Returns `(confidence, fitted)` per distinct confidence, ascending.
pub fn isotonic(points: &[(f64, bool)]) -> Vec<(f64, f64)> {
    let mut sorted: Vec<(f64, f64)> = points
        .iter()
        .map(|(c, a)| (*c, if *a { 1.0 } else { 0.0 }))
        .collect();
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // Blocks of (sum, count, lowest confidence, highest confidence).
    let mut blocks: Vec<(f64, f64, f64, f64)> = Vec::with_capacity(sorted.len());
    for (c, y) in sorted {
        blocks.push((y, 1.0, c, c));
        // Merge backwards while the mean would decrease.
        while blocks.len() >= 2 {
            let n = blocks.len();
            let (s1, k1, lo1, _) = blocks[n - 2];
            let (s2, k2, _, hi2) = blocks[n - 1];
            if s1 / k1 <= s2 / k2 {
                break;
            }
            blocks.truncate(n - 2);
            blocks.push((s1 + s2, k1 + k2, lo1, hi2));
        }
    }
    let mut curve: Vec<(f64, f64)> = Vec::new();
    for (s, k, lo, hi) in blocks {
        let mean = s / k;
        for c in [lo, hi] {
            if curve.last().is_none_or(|(prev, _)| *prev != c) {
                curve.push((c, mean));
            }
        }
    }
    curve
}

/// The fitted value at `confidence`: that of the highest curve point at or
/// below it, else the first point's.
pub fn calibrated(curve: &[(f64, f64)], confidence: f64) -> f64 {
    curve
        .iter()
        .take_while(|(c, _)| *c <= confidence)
        .last()
        .or(curve.first())
        .map(|(_, y)| *y)
        .unwrap_or(confidence)
}

pub fn calibrate(
    source: &'static str,
    points: &[(f64, bool)],
) -> Result<Calibration, CalibrationError> {
    if points.len() < 2 {
        return Err(CalibrationError::TooFew(points.len()));
    }
    let positives = points.iter().filter(|(_, a)| *a).count();
    let negatives = points.len() - positives;
    if positives == 0 || negatives == 0 {
        return Err(CalibrationError::OneClass {
            positives,
            negatives,
        });
    }
    let curve = isotonic(points);
    let n = points.len() as f64;
    let brier = |f: &dyn Fn(f64) -> f64| {
        points
            .iter()
            .map(|(c, a)| {
                let y = if *a { 1.0 } else { 0.0 };
                (f(*c) - y).powi(2)
            })
            .sum::<f64>()
            / n
    };
    Ok(Calibration {
        source,
        points: points.len(),
        positives,
        negatives,
        brier_score: BrierScore {
            raw: brier(&|c| c),
            calibrated: brier(&|c| calibrated(&curve, c)),
        },
        curve: curve
            .into_iter()
            .map(|(confidence, calibrated)| CurvePoint {
                confidence,
                calibrated,
            })
            .collect(),
    })
}

// ───────────────────────────── HTTP ─────────────────────────────

type ApiErr = (StatusCode, String);

fn not_found(kind: &str, id: &str) -> ApiErr {
    (StatusCode::NOT_FOUND, format!("{kind} '{id}' not found"))
}

fn actor_iri(state: &AppState, user: &AuthenticatedUser) -> String {
    format!(
        "{}/users/{}",
        state.base_url.trim_end_matches('/'),
        user.user_id
    )
}

fn check_confidence(confidence: Option<f64>) -> Result<(), ApiErr> {
    match confidence {
        Some(c) if c.is_nan() || !(0.0..=1.0).contains(&c) => Err((
            StatusCode::BAD_REQUEST,
            format!("confidence must be within 0 and 1, got {c}"),
        )),
        _ => Ok(()),
    }
}

/// `POST /api/mappings/:id/decisions`
pub async fn decide(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<DecisionRequest>,
) -> Result<Response, ApiErr> {
    let mapping =
        registry::get_mapping(&state.store, &id).ok_or_else(|| not_found("mapping", &id))?;
    let outcome = Outcome::parse(&body.decision).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "unknown decision '{}'; expected approve, edit or reject",
                body.decision
            ),
        )
    })?;
    check_confidence(body.confidence)?;

    // The decision moves the mapping: an approval also re-baselines drift to
    // the profile of the moment, as an approval through PUT does. An edit is
    // recorded here and carried out by the edit itself.
    let mut record = mapping.clone();
    match outcome {
        Outcome::Approve => {
            record.state = MappingState::Approved;
            record.profile_version =
                super::profile::latest_version(&state.store, &record.source_id)
                    .or(record.profile_version);
        }
        Outcome::Reject => record.state = MappingState::Rejected,
        Outcome::Edit => {}
    }
    if record.state != mapping.state || record.profile_version != mapping.profile_version {
        record.updated_at = registry::now();
        registry::put_mapping(&state.store, &record)
            .map_err(|m| (StatusCode::INTERNAL_SERVER_ERROR, m))?;
    }
    let actor = actor_iri(&state, &user);
    let decision = record_decision(&state, &record, outcome, &body, &actor)?;
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Source,
        format!(
            "mapping '{}' v{}: review decision '{}'{}",
            record.id,
            record.version,
            outcome.as_str(),
            body.confidence
                .map(|c| format!(" (confidence {c})"))
                .unwrap_or_default()
        ),
        Some(&user.user_id),
        Some(record.iri()),
        vec![SOURCES_GRAPH.to_string()],
        0,
        0,
        Some(record.version.to_string()),
    );
    Ok((StatusCode::CREATED, Json(decision)).into_response())
}

fn record_decision(
    state: &AppState,
    mapping: &MappingRecord,
    outcome: Outcome,
    body: &DecisionRequest,
    actor: &str,
) -> Result<Decision, ApiErr> {
    record(
        &state.store,
        mapping,
        outcome,
        body.target.as_deref(),
        body.confidence,
        body.note.as_deref(),
        Some(actor),
    )
    .map_err(|m| (StatusCode::INTERNAL_SERVER_ERROR, m))
}

/// `GET /api/mappings/:id/reviews`
pub async fn list_reviews(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<Decision>>, ApiErr> {
    if registry::get_mapping(&state.store, &id).is_none() {
        return Err(not_found("mapping", &id));
    }
    Ok(Json(list(&state.store, &id)))
}

/// `GET /api/mappings/:id/provenance`
pub async fn mapping_provenance(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiErr> {
    if registry::get_mapping(&state.store, &id).is_none() {
        return Err(not_found("mapping", &id));
    }
    let turtle = provenance_turtle(&state.store, &id);
    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], turtle).into_response())
}

/// `POST /api/sources/calibration`
pub async fn calibration(
    State(state): State<AppState>,
    Json(body): Json<CalibrationRequest>,
) -> Result<Json<Calibration>, ApiErr> {
    let (source, points): (&'static str, Vec<(f64, bool)>) = match body.points {
        Some(points) => {
            for p in &points {
                check_confidence(Some(p.confidence))?;
            }
            (
                "body",
                points.iter().map(|p| (p.confidence, p.accepted)).collect(),
            )
        }
        None => ("decisions", calibration_points(&state.store)),
    };
    calibrate(source, &points)
        .map(Json)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/mappings/:id/decisions", post(decide))
        .route("/api/mappings/:id/reviews", get(list_reviews))
        .route("/api/mappings/:id/provenance", get(mapping_provenance))
        .route("/api/sources/calibration", post(calibration))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(id: &str) -> MappingRecord {
        MappingRecord {
            id: id.into(),
            title: id.into(),
            source_id: "s".into(),
            version: 2,
            state: MappingState::Proposed,
            shapes_graph: None,
            model: None,
            model_version: None,
            profile_version: None,
            created_by: None,
            created_at: registry::now(),
            updated_at: registry::now(),
        }
    }

    #[test]
    fn outcomes_parse_leniently_and_print_canonically() {
        assert_eq!(Outcome::parse("Approved"), Some(Outcome::Approve));
        assert_eq!(Outcome::parse(" edit "), Some(Outcome::Edit));
        assert_eq!(Outcome::parse("rejected"), Some(Outcome::Reject));
        assert_eq!(Outcome::parse("maybe"), None);
        assert_eq!(Outcome::Reject.as_str(), "reject");
    }

    #[test]
    fn decisions_round_trip_newest_first_with_their_version() {
        let store = TripleStore::in_memory().unwrap();
        let m = mapping("m");
        registry::put_mapping(&store, &m).unwrap();
        record(
            &store,
            &m,
            Outcome::Reject,
            None,
            Some(0.4),
            Some("wrong table"),
            Some("http://x/users/adm"),
        )
        .unwrap();
        record(
            &store,
            &m,
            Outcome::Edit,
            Some("http://example.org/map#Products"),
            Some(0.7),
            None,
            None,
        )
        .unwrap();
        record(
            &store,
            &m,
            Outcome::Approve,
            Some("price column"),
            None,
            Some(""),
            None,
        )
        .unwrap();

        let listed = list(&store, "m");
        let outcomes: Vec<&str> = listed.iter().map(|d| d.outcome.as_str()).collect();
        assert_eq!(outcomes, ["approve", "edit", "reject"]);
        assert!(listed.iter().all(|d| d.mapping_version == 2));
        assert_eq!(listed[2].confidence, Some(0.4));
        assert_eq!(listed[2].note.as_deref(), Some("wrong table"));
        assert_eq!(listed[2].actor.as_deref(), Some("http://x/users/adm"));
        assert_eq!(
            listed[1].target.as_deref(),
            Some("http://example.org/map#Products")
        );
        assert_eq!(listed[0].target.as_deref(), Some("price column"));
        assert_eq!(listed[0].note, None, "an empty note is no note");
        assert!(list(&store, "other").is_empty());

        // The calibration set: only an approval is an acceptance, and only
        // decisions with a confidence count.
        let mut points = calibration_points(&store);
        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        assert_eq!(points, vec![(0.4, false), (0.7, false)]);

        // The trail carries the mapping, its versions and the decisions.
        let ttl = provenance_turtle(&store, "m");
        assert!(ttl.contains("ds:ReviewDecision"), "{ttl}");
        assert!(ttl.contains("<urn:mapping:m:version:2>"), "{ttl}");
        assert!(ttl.contains("ds:Mapping"), "{ttl}");

        delete_for_mapping(&store, "m").unwrap();
        assert!(list(&store, "m").is_empty());
        assert!(provenance_turtle(&store, "m").contains("ds:Mapping"));
    }

    #[test]
    fn isotonic_regression_pools_violators_into_a_monotone_step() {
        let points = [
            (0.95, true),
            (0.9, true),
            (0.8, false),
            (0.85, true),
            (0.5, false),
            (0.3, false),
            (0.6, true),
        ];
        let curve = isotonic(&points);
        let mut last = -1.0;
        for (c, y) in &curve {
            assert!((0.0..=1.0).contains(y));
            assert!(*y >= last, "not monotone at {c}: {curve:?}");
            last = *y;
        }
        assert_eq!(curve.first().unwrap().0, 0.3);
        assert_eq!(curve.last().unwrap().0, 0.95);
        // Below every observation the fit is the lowest block's rate; at the
        // top it is the highest one's.
        assert_eq!(calibrated(&curve, 0.0), curve[0].1);
        assert_eq!(calibrated(&curve, 1.0), curve.last().unwrap().1);
        // 0.8 (refused) sits between 0.6 and 0.85 (accepted): the violator
        // is pooled, so the fit there is strictly between 0 and 1.
        let at = calibrated(&curve, 0.8);
        assert!(at > 0.0 && at < 1.0, "{curve:?}");
    }

    #[test]
    fn perfectly_separated_data_gives_a_step_and_the_fit_is_never_worse() {
        let points = [(0.2, false), (0.4, false), (0.7, true), (0.9, true)];
        let c = calibrate("body", &points).unwrap();
        assert_eq!(c.positives, 2);
        assert_eq!(c.negatives, 2);
        assert_eq!(calibrated(&isotonic(&points), 0.4), 0.0);
        assert_eq!(calibrated(&isotonic(&points), 0.7), 1.0);
        assert!(c.brier_score.calibrated <= c.brier_score.raw);
    }

    #[test]
    fn one_class_and_too_few_are_refused() {
        assert_eq!(
            calibrate("body", &[(0.9, true), (0.6, true)]),
            Err(CalibrationError::OneClass {
                positives: 2,
                negatives: 0
            })
        );
        assert_eq!(
            calibrate("body", &[(0.9, false)]),
            Err(CalibrationError::TooFew(1))
        );
        assert_eq!(
            calibrate("decisions", &[]),
            Err(CalibrationError::TooFew(0))
        );
        let msg = CalibrationError::OneClass {
            positives: 0,
            negatives: 3,
        }
        .to_string();
        assert!(msg.contains("one-class"), "{msg}");
    }
}
