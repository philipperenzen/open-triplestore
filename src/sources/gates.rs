//! The mapping gates: the thresholds a proposal is judged by, as a config
//! graph an administrator edits.
//!
//! `urn:config:mapping-gates` holds one subject with the numbers: the
//! confidence bands (`auto` / `review` / `needs-expert`), the datatype-mismatch
//! cap, the ambiguity margin, the enumeration match minimum, the deterministic
//! lexical scorer's weights, and the two numbers the dry-run classifier reads
//! — what share of a type's subjects a violation must hit, over how many
//! subjects at least, to count as a mapping defect rather than a data issue.
//!
//! They live in the store rather than in a config file because the mapping
//! proposer is a separate service that runs offline against the store and
//! reads them from here, and because changing a gate is a decision worth a
//! commit-log entry. Absent, the defaults apply and `GET` says so.
//!
//! The graph is a system graph: it belongs to no dataset, so it is outside a
//! caller's SPARQL scope and is served through `/api/sources/gates`.

use axum::extract::State;
use axum::http::{header::ACCEPT, header::CONTENT_TYPE, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use oxigraph::sparql::QueryResults;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;
use crate::store::{escape_sparql_literal, TripleStore};

use super::model::DS;
use super::registry;

/// The config graph, and the subject inside it.
pub const GATES_GRAPH: &str = "urn:config:mapping-gates";

const DCT: &str = "http://purl.org/dc/terms/";
const PROV: &str = "http://www.w3.org/ns/prov#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The deterministic lexical scorer's parameters. The proposer scores a
/// (column, property) pair from name similarity, description similarity and
/// datatype compatibility; these are the weights it combines them with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LexicalScorer {
    /// Weight of name similarity: the column name against the property's
    /// local name and labels.
    pub name_weight: f64,
    /// Weight of description similarity: the column comment against
    /// `rdfs:comment` / `skos:definition`.
    pub comment_weight: f64,
    /// Weight of datatype compatibility: the column's generic type against the
    /// property's range or `sh:datatype`.
    pub type_weight: f64,
    /// A candidate scoring below this is not proposed at all.
    pub minimum_score: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MappingGates {
    /// Confidence at or above which a proposal is accepted without review.
    pub auto_threshold: f64,
    /// Confidence at or above which a proposal goes to a reviewer; below it,
    /// to an expert.
    pub review_threshold: f64,
    /// The largest fraction of sampled values that may fail the target
    /// datatype before a candidate is refused.
    pub datatype_mismatch_cap: f64,
    /// The smallest score margin between the best candidate and the runner-up
    /// for the best to count as unambiguous.
    pub ambiguity_margin: f64,
    /// The fraction of a code list's values that must match an enumeration's
    /// members for the enumeration to be proposed.
    pub enum_match_minimum: f64,
    /// The share of a type's subjects a violation must hit for the dry-run to
    /// classify it as a mapping defect.
    pub systematic_share: f64,
    /// ...and the smallest number of subjects that can make one. One subject
    /// is never a pattern.
    pub systematic_min_subjects: u64,
    /// KL divergence between two profile versions of a code list above which
    /// drift is reported.
    pub drift_kl_threshold: f64,
    pub lexical: LexicalScorer,
}

impl Default for MappingGates {
    fn default() -> Self {
        MappingGates {
            auto_threshold: 0.90,
            review_threshold: 0.70,
            datatype_mismatch_cap: 0.10,
            ambiguity_margin: 0.05,
            enum_match_minimum: 0.80,
            systematic_share: 0.90,
            systematic_min_subjects: 2,
            drift_kl_threshold: 0.10,
            lexical: LexicalScorer {
                name_weight: 0.60,
                comment_weight: 0.25,
                type_weight: 0.15,
                minimum_score: 0.40,
            },
        }
    }
}

impl MappingGates {
    /// Refuse a configuration that cannot be applied: a threshold outside
    /// `[0, 1]`, bands that cross, a weight set that sums to nothing.
    pub fn validate(&self) -> Result<(), String> {
        let unit = |name: &str, v: f64| {
            if !(0.0..=1.0).contains(&v) || v.is_nan() {
                Err(format!("{name} must be between 0 and 1, not {v}"))
            } else {
                Ok(())
            }
        };
        unit("autoThreshold", self.auto_threshold)?;
        unit("reviewThreshold", self.review_threshold)?;
        unit("datatypeMismatchCap", self.datatype_mismatch_cap)?;
        unit("ambiguityMargin", self.ambiguity_margin)?;
        unit("enumMatchMinimum", self.enum_match_minimum)?;
        unit("systematicShare", self.systematic_share)?;
        unit("lexical.nameWeight", self.lexical.name_weight)?;
        unit("lexical.commentWeight", self.lexical.comment_weight)?;
        unit("lexical.typeWeight", self.lexical.type_weight)?;
        unit("lexical.minimumScore", self.lexical.minimum_score)?;
        if self.review_threshold > self.auto_threshold {
            return Err(format!(
                "reviewThreshold ({}) is above autoThreshold ({}); the review band would be empty",
                self.review_threshold, self.auto_threshold
            ));
        }
        if self.systematic_min_subjects == 0 {
            return Err("systematicMinSubjects must be at least 1".to_string());
        }
        if self.drift_kl_threshold.is_nan() || self.drift_kl_threshold < 0.0 {
            return Err(format!(
                "driftKlThreshold must be zero or more, not {}",
                self.drift_kl_threshold
            ));
        }
        let weights =
            self.lexical.name_weight + self.lexical.comment_weight + self.lexical.type_weight;
        if weights <= 0.0 {
            return Err("the lexical weights sum to zero; nothing would score".to_string());
        }
        Ok(())
    }
}

/// A partial update: every field optional, so an administrator changes one
/// number without restating the rest. An unknown field is refused rather than
/// ignored — a misspelled gate silently keeping its old value is the failure
/// mode this exists to prevent.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GatesPatch {
    pub auto_threshold: Option<f64>,
    pub review_threshold: Option<f64>,
    pub datatype_mismatch_cap: Option<f64>,
    pub ambiguity_margin: Option<f64>,
    pub enum_match_minimum: Option<f64>,
    pub systematic_share: Option<f64>,
    pub systematic_min_subjects: Option<u64>,
    pub drift_kl_threshold: Option<f64>,
    pub lexical: Option<LexicalPatch>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LexicalPatch {
    pub name_weight: Option<f64>,
    pub comment_weight: Option<f64>,
    pub type_weight: Option<f64>,
    pub minimum_score: Option<f64>,
}

impl GatesPatch {
    pub fn apply(self, mut gates: MappingGates) -> MappingGates {
        macro_rules! take {
            ($field:ident) => {
                if let Some(v) = self.$field {
                    gates.$field = v;
                }
            };
        }
        take!(auto_threshold);
        take!(review_threshold);
        take!(datatype_mismatch_cap);
        take!(ambiguity_margin);
        take!(enum_match_minimum);
        take!(systematic_share);
        take!(systematic_min_subjects);
        take!(drift_kl_threshold);
        if let Some(l) = self.lexical {
            if let Some(v) = l.name_weight {
                gates.lexical.name_weight = v;
            }
            if let Some(v) = l.comment_weight {
                gates.lexical.comment_weight = v;
            }
            if let Some(v) = l.type_weight {
                gates.lexical.type_weight = v;
            }
            if let Some(v) = l.minimum_score {
                gates.lexical.minimum_score = v;
            }
        }
        gates
    }
}

/// The `(property, value)` pairs one configuration writes. Decimals are
/// written with a fixed precision so a read-back compares equal to what was
/// saved and two saves of the same numbers produce the same graph.
fn pairs(g: &MappingGates) -> Vec<(&'static str, String)> {
    let dec = |v: f64| format!("\"{v:.4}\"^^xsd:decimal");
    let int = |v: u64| format!("\"{v}\"^^xsd:integer");
    vec![
        ("ds:autoThreshold", dec(g.auto_threshold)),
        ("ds:reviewThreshold", dec(g.review_threshold)),
        ("ds:datatypeMismatchCap", dec(g.datatype_mismatch_cap)),
        ("ds:ambiguityMargin", dec(g.ambiguity_margin)),
        ("ds:enumMatchMinimum", dec(g.enum_match_minimum)),
        ("ds:systematicShare", dec(g.systematic_share)),
        ("ds:systematicMinSubjects", int(g.systematic_min_subjects)),
        ("ds:driftKlThreshold", dec(g.drift_kl_threshold)),
        ("ds:lexicalNameWeight", dec(g.lexical.name_weight)),
        ("ds:lexicalCommentWeight", dec(g.lexical.comment_weight)),
        ("ds:lexicalTypeWeight", dec(g.lexical.type_weight)),
        ("ds:lexicalMinimumScore", dec(g.lexical.minimum_score)),
    ]
}

fn prologue() -> String {
    format!("PREFIX ds: <{DS}>\nPREFIX dct: <{DCT}>\nPREFIX prov: <{PROV}>\nPREFIX xsd: <{XSD}>\n")
}

/// Replace the config graph with `gates`, in one update.
pub fn save(store: &TripleStore, gates: &MappingGates, actor: Option<&str>) -> Result<(), String> {
    gates.validate()?;
    let mut body = String::from("  <urn:config:mapping-gates> a ds:MappingGates ;\n");
    for (p, v) in pairs(gates) {
        body.push_str(&format!("    {p} {v} ;\n"));
    }
    if let Some(a) = actor {
        body.push_str(&format!(
            "    prov:wasAttributedTo <{}> ;\n",
            crate::store::escape_sparql_iri(a)
        ));
    }
    body.push_str(&format!(
        "    dct:modified \"{}\" .\n",
        escape_sparql_literal(&registry::now())
    ));
    let sparql = format!(
        "{}DROP SILENT GRAPH <{GATES_GRAPH}>;\nINSERT DATA {{ GRAPH <{GATES_GRAPH}> {{\n{body}}} }}",
        prologue()
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

/// What the graph currently says, over the defaults for anything it does not
/// say. The flag tells whether anything was configured at all.
pub fn load(store: &TripleStore) -> (MappingGates, bool) {
    let query = format!(
        "SELECT ?p ?o WHERE {{ GRAPH <{GATES_GRAPH}> {{ <urn:config:mapping-gates> ?p ?o }} }}"
    );
    let mut values: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(&query) {
        for solution in solutions.flatten() {
            if let (
                Some(oxigraph::model::Term::NamedNode(p)),
                Some(oxigraph::model::Term::Literal(o)),
            ) = (solution.get("p"), solution.get("o"))
            {
                if let Some(local) = p.as_str().strip_prefix(DS) {
                    values.insert(local.to_string(), o.value().to_string());
                }
            }
        }
    }
    if values.is_empty() {
        return (MappingGates::default(), false);
    }
    let d = MappingGates::default();
    let f = |name: &str, default: f64| {
        values
            .get(name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let gates = MappingGates {
        auto_threshold: f("autoThreshold", d.auto_threshold),
        review_threshold: f("reviewThreshold", d.review_threshold),
        datatype_mismatch_cap: f("datatypeMismatchCap", d.datatype_mismatch_cap),
        ambiguity_margin: f("ambiguityMargin", d.ambiguity_margin),
        enum_match_minimum: f("enumMatchMinimum", d.enum_match_minimum),
        systematic_share: f("systematicShare", d.systematic_share),
        systematic_min_subjects: values
            .get("systematicMinSubjects")
            .and_then(|v| v.parse().ok())
            .unwrap_or(d.systematic_min_subjects),
        drift_kl_threshold: f("driftKlThreshold", d.drift_kl_threshold),
        lexical: LexicalScorer {
            name_weight: f("lexicalNameWeight", d.lexical.name_weight),
            comment_weight: f("lexicalCommentWeight", d.lexical.comment_weight),
            type_weight: f("lexicalTypeWeight", d.lexical.type_weight),
            minimum_score: f("lexicalMinimumScore", d.lexical.minimum_score),
        },
    };
    (gates, true)
}

/// The configuration as Turtle, for a reader that wants the graph itself.
pub fn turtle(gates: &MappingGates) -> String {
    let mut out = format!(
        "@prefix ds:  <{DS}> .\n@prefix xsd: <{XSD}> .\n\n<{GATES_GRAPH}> a ds:MappingGates ;\n"
    );
    let all = pairs(gates);
    for (i, (p, v)) in all.iter().enumerate() {
        let end = if i + 1 == all.len() { " ." } else { " ;" };
        out.push_str(&format!("    {p} {v}{end}\n"));
    }
    out
}

// ─────────────────────────────── HTTP ───────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GatesResponse {
    pub graph: &'static str,
    /// `configured` when the graph holds a saved configuration, `default`
    /// when the built-in numbers apply.
    pub source: &'static str,
    #[serde(flatten)]
    pub gates: MappingGates,
}

fn response(store: &TripleStore) -> GatesResponse {
    let (gates, configured) = load(store);
    GatesResponse {
        graph: GATES_GRAPH,
        source: if configured { "configured" } else { "default" },
        gates,
    }
}

/// `GET /api/sources/gates` — JSON, or Turtle with `Accept: text/turtle`.
pub async fn get_gates(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let wants_turtle = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/turtle"));
    if wants_turtle {
        let (gates, _) = load(&state.store);
        return (
            StatusCode::OK,
            [(CONTENT_TYPE, "text/turtle")],
            turtle(&gates),
        )
            .into_response();
    }
    Json(response(&state.store)).into_response()
}

/// `PUT /api/sources/gates` — a partial update over what is configured.
pub async fn put_gates(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(patch): Json<GatesPatch>,
) -> Result<Json<GatesResponse>, (StatusCode, String)> {
    let (current, _) = load(&state.store);
    let next = patch.apply(current);
    next.validate().map_err(|m| (StatusCode::BAD_REQUEST, m))?;
    let actor = format!(
        "{}/users/{}",
        state.base_url.trim_end_matches('/'),
        user.user_id
    );
    save(&state.store, &next, Some(&actor)).map_err(|m| (StatusCode::INTERNAL_SERVER_ERROR, m))?;
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Source,
        "mapping gates updated".to_string(),
        Some(&user.user_id),
        Some(GATES_GRAPH.to_string()),
        vec![GATES_GRAPH.to_string()],
        0,
        0,
        None,
    );
    Ok(Json(response(&state.store)))
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/sources/gates", get(get_gates).put(put_gates))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_documented_bands() {
        let g = MappingGates::default();
        g.validate().unwrap();
        assert_eq!((g.auto_threshold, g.review_threshold), (0.90, 0.70));
        assert_eq!((g.systematic_share, g.systematic_min_subjects), (0.90, 2));
    }

    #[test]
    fn a_configuration_that_cannot_be_applied_is_refused() {
        let crossed = MappingGates {
            review_threshold: 0.95,
            ..Default::default()
        };
        assert!(crossed.validate().unwrap_err().contains("reviewThreshold"));
        let over = MappingGates {
            systematic_share: 1.5,
            ..Default::default()
        };
        assert!(over.validate().unwrap_err().contains("systematicShare"));
        let none = MappingGates {
            systematic_min_subjects: 0,
            ..Default::default()
        };
        assert!(none.validate().is_err());
        let weightless = MappingGates {
            lexical: LexicalScorer {
                name_weight: 0.0,
                comment_weight: 0.0,
                type_weight: 0.0,
                minimum_score: 0.4,
            },
            ..Default::default()
        };
        assert!(weightless.validate().unwrap_err().contains("weights"));
    }

    #[test]
    fn a_patch_changes_only_what_it_names() {
        let patch: GatesPatch =
            serde_json::from_str(r#"{"systematicShare": 0.8, "lexical": {"nameWeight": 0.5}}"#)
                .unwrap();
        let g = patch.apply(MappingGates::default());
        assert_eq!(g.systematic_share, 0.8);
        assert_eq!(g.lexical.name_weight, 0.5);
        assert_eq!(g.lexical.comment_weight, 0.25, "untouched");
        assert_eq!(g.auto_threshold, 0.9, "untouched");
        // A misspelled gate is refused, not ignored.
        assert!(serde_json::from_str::<GatesPatch>(r#"{"autoTreshold": 0.5}"#).is_err());
    }

    #[test]
    fn the_graph_round_trips_and_says_whether_it_was_configured() {
        let store = TripleStore::in_memory().unwrap();
        let (defaults, configured) = load(&store);
        assert!(!configured);
        assert_eq!(defaults, MappingGates::default());

        let mut g = MappingGates {
            auto_threshold: 0.85,
            systematic_min_subjects: 3,
            lexical: LexicalScorer {
                minimum_score: 0.33,
                ..MappingGates::default().lexical
            },
            ..Default::default()
        };
        save(&store, &g, Some("http://x/users/adm")).unwrap();
        let (back, configured) = load(&store);
        assert!(configured);
        assert_eq!(back, g);

        // A second save replaces, never accumulates.
        g.auto_threshold = 0.95;
        save(&store, &g, None).unwrap();
        assert_eq!(load(&store).0.auto_threshold, 0.95);
        assert_eq!(
            store.count_graph(Some(GATES_GRAPH)).unwrap(),
            pairs(&g).len() + 2,
            "type and modified, one triple each"
        );
        assert!(turtle(&g).contains("ds:autoThreshold \"0.9500\"^^xsd:decimal"));
    }
}
