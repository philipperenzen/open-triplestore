//! SWRL rules stored with a dataset, managed over HTTP
//! (`/api/datasets/:id/rules`).
//!
//! A dataset's stored rules are the `swrl:Imp`s in its `entailment`- and
//! `model`-role graphs (see [`crate::entailment`]). This API keeps the ones a
//! user writes in one server-managed graph, `urn:ots:rules:<dataset>`,
//! registered with the dataset in the `entailment` role on the first write.
//! What a user writes is a *rule set*: rules in any syntax
//! `POST /api/swrl/execute` reads, stored as
//!
//! ```text
//! <urn:ots:ruleset:<uuid>> a ots:RuleSet ; rdfs:label "…" ;
//!     ots:ruleSource "…" ; ots:ruleFormat "swrlapi" ; ots:ruleOptions "{…}" ;
//!     ots:rule <urn:ots:ruleset:<uuid>:1>, … .
//! ```
//!
//! with each rule in the SWRL RDF syntax ([`crate::swrl::write`]), so the
//! entailment machinery runs it like any other stored rule, the source kept
//! for editing. Every rule is checked as `POST /api/swrl/execute` checks it
//! before anything is written; after a change the dataset's rules (and its
//! regime, in `materialize` mode) run again at once.
//!
//! Rules in the dataset's other rule graphs (uploaded Turtle, a model) are
//! listed read-only; a graph a non-writer may not see is left out.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, GraphKind};
use crate::server::AppState;

const OTS: &str = "https://opentriplestore.org/ns#";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const DCT_MODIFIED: &str = "http://purl.org/dc/terms/modified";
const DCT_CREATOR: &str = "http://purl.org/dc/terms/creator";
const SET_PREFIX: &str = "urn:ots:ruleset:";

/// The graph this API keeps a dataset's rule sets in.
pub fn managed_graph(dataset_id: &str) -> String {
    format!("urn:ots:rules:{dataset_id}")
}

type ApiErr = (StatusCode, String);

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn dataset(state: &AppState, uid: Option<&str>, id: &str) -> Result<Dataset, ApiErr> {
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

fn can_write(state: &AppState, user: Option<&AuthenticatedUser>, ds: &Dataset) -> bool {
    match user {
        Some(u) if u.is_admin() => true,
        Some(u) => {
            u.write_access
                && state
                    .auth_db
                    .can_write_dataset(&u.user_id, ds)
                    .unwrap_or(false)
        }
        None => false,
    }
}

fn writer<'a>(
    state: &AppState,
    user: &'a Option<Extension<AuthenticatedUser>>,
    ds: &Dataset,
) -> Result<&'a AuthenticatedUser, ApiErr> {
    let Some(Extension(u)) = user else {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Authentication required".to_string(),
        ));
    };
    if !can_write(state, Some(u), ds) {
        return Err((
            StatusCode::FORBIDDEN,
            "Changing a dataset's rules needs write access to the dataset".to_string(),
        ));
    }
    Ok(u)
}

/// The body of `POST …/rules` and `PUT …/rules/:set`.
#[derive(Debug, Deserialize)]
pub struct RuleSetBody {
    /// A name for the set (optional).
    #[serde(default)]
    pub label: Option<String>,
    /// The rules, in `format`.
    pub rules: String,
    /// One of `crate::swrl::FORMATS` (default `swrlapi`).
    #[serde(default)]
    pub format: Option<String>,
    /// Format `rdf`: the serialisation (default `turtle`).
    #[serde(default)]
    pub rdf_format: Option<String>,
    /// Format `rdf`: the base IRI.
    #[serde(default)]
    pub base_iri: Option<String>,
    /// Format `swrlapi`: prefixes, ahead of the server's registry.
    #[serde(default)]
    pub prefixes: std::collections::HashMap<String, String>,
}

#[cfg(feature = "swrl")]
mod imp {
    use super::*;
    use oxigraph::model::{
        GraphName, GraphNameRef, Literal, NamedNode, NamedNodeRef, NamedOrBlankNode, Quad, Term,
        Triple,
    };
    use std::collections::{HashMap, HashSet};

    fn ots(local: &str) -> NamedNode {
        NamedNode::new_unchecked(format!("{OTS}{local}"))
    }

    fn nn(iri: &str) -> NamedNodeRef<'_> {
        NamedNodeRef::new_unchecked(iri)
    }

    fn graph_node(dataset_id: &str) -> Result<NamedNode, ApiErr> {
        NamedNode::new(managed_graph(dataset_id)).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("dataset id cannot name a graph: {e}"),
            )
        })
    }

    fn objects(state: &AppState, g: &NamedNode, s: &NamedOrBlankNode, p: &str) -> Vec<Term> {
        state
            .store
            .store()
            .quads_for_pattern(
                Some(s.as_ref()),
                Some(nn(p)),
                None,
                Some(GraphNameRef::NamedNode(g.as_ref())),
            )
            .filter_map(Result::ok)
            .map(|q| q.object)
            .collect()
    }

    fn literal_of(
        state: &AppState,
        g: &NamedNode,
        s: &NamedOrBlankNode,
        p: &str,
    ) -> Option<String> {
        objects(state, g, s, p).into_iter().find_map(|o| match o {
            Term::Literal(l) => Some(l.value().to_string()),
            _ => None,
        })
    }

    /// The rule sets in the managed graph, as JSON, oldest first.
    pub(super) fn list_sets(state: &AppState, dataset_id: &str) -> Result<Vec<Value>, ApiErr> {
        let g = graph_node(dataset_id)?;
        let store = state.store.store();
        let rules: HashMap<String, crate::swrl::engine::SwrlRule> =
            crate::swrl::rdf::rules_in_graphs(store, &[g.as_str().to_string()])
                .map_err(e500)?
                .into_iter()
                .filter_map(|r| r.name.clone().map(|n| (n, r)))
                .collect();
        let rule_set = ots("RuleSet");
        let mut sets = Vec::new();
        for q in store.quads_for_pattern(
            None,
            Some(nn(RDF_TYPE)),
            Some(rule_set.as_ref().into()),
            Some(GraphNameRef::NamedNode(g.as_ref())),
        ) {
            let q = q.map_err(e500)?;
            let NamedOrBlankNode::NamedNode(set) = &q.subject else {
                continue;
            };
            let Some(id) = set.as_str().strip_prefix(SET_PREFIX) else {
                continue;
            };
            let mut members: Vec<String> = objects(state, &g, &q.subject, &format!("{OTS}rule"))
                .into_iter()
                .filter_map(|t| match t {
                    Term::NamedNode(n) => Some(n.as_str().to_string()),
                    _ => None,
                })
                .collect();
            // `…:<n>`: in the order they were written.
            members.sort_by_key(|m| {
                m.rsplit(':')
                    .next()
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(u64::MAX)
            });
            let options = literal_of(state, &g, &q.subject, &format!("{OTS}ruleOptions"))
                .and_then(|j| serde_json::from_str::<Value>(&j).ok())
                .unwrap_or_else(|| json!({}));
            sets.push(json!({
                "id": id,
                "iri": set.as_str(),
                "label": literal_of(state, &g, &q.subject, RDFS_LABEL),
                "format": literal_of(state, &g, &q.subject, &format!("{OTS}ruleFormat")),
                "source": literal_of(state, &g, &q.subject, &format!("{OTS}ruleSource")),
                "options": options,
                "updated_at": literal_of(state, &g, &q.subject, DCT_MODIFIED),
                "updated_by": literal_of(state, &g, &q.subject, DCT_CREATOR),
                "rules": members.iter().map(|m| json!({
                    "iri": m,
                    "functional": rules.get(m).map(crate::swrl::write::rule_to_functional),
                })).collect::<Vec<_>>(),
            }));
        }
        sets.sort_by(|a, b| {
            a["updated_at"]
                .as_str()
                .cmp(&b["updated_at"].as_str())
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        Ok(sets)
    }

    /// The dataset's rules in its other rule graphs, read-only: per graph its
    /// rules in functional syntax, or why the strict reader refused them.
    pub(super) fn other_rules(state: &AppState, ds: &Dataset, writer: bool) -> Vec<Value> {
        let managed = managed_graph(&ds.id);
        let private: HashSet<String> = state
            .auth_db
            .list_dataset_graph_entries(&ds.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.private)
            .map(|e| e.graph_iri)
            .collect();
        let store = state.store.store();
        crate::entailment::rule_graphs(state, ds)
            .into_iter()
            .filter(|g| *g != managed && (writer || !private.contains(g)))
            .filter(|g| crate::swrl::rdf::graphs_hold_rules(store, std::slice::from_ref(g)))
            .map(
                |g| match crate::swrl::rdf::rules_in_graphs(store, std::slice::from_ref(&g)) {
                    Ok(rules) => json!({
                        "graph": g,
                        "rules": rules.iter().map(|r| json!({
                            "iri": r.name,
                            "functional": crate::swrl::write::rule_to_functional(r),
                        })).collect::<Vec<_>>(),
                        "error": null,
                    }),
                    Err(e) => json!({ "graph": g, "rules": [], "error": e }),
                },
            )
            .collect()
    }

    /// Every quad of the set `set` in `g`: the set node, its rules, their
    /// variables (IRIs under the set's IRI) and the blank nodes they reach.
    fn set_quads(state: &AppState, g: &NamedNode, set: &NamedNode) -> Vec<Quad> {
        let store = state.store.store();
        let mut out = Vec::new();
        let mut seen: HashSet<NamedOrBlankNode> = HashSet::new();
        let mut todo: Vec<NamedOrBlankNode> = vec![set.clone().into()];
        while let Some(node) = todo.pop() {
            if !seen.insert(node.clone()) {
                continue;
            }
            for q in store
                .quads_for_pattern(
                    Some(node.as_ref()),
                    None,
                    None,
                    Some(GraphNameRef::NamedNode(g.as_ref())),
                )
                .filter_map(Result::ok)
            {
                match &q.object {
                    Term::BlankNode(b) => todo.push(b.clone().into()),
                    Term::NamedNode(n) if n.as_str().starts_with(set.as_str()) => {
                        todo.push(n.clone().into())
                    }
                    _ => {}
                }
                out.push(q);
            }
        }
        out
    }

    /// Parse, check and turn `body` into the triples of the set `set`.
    pub(super) fn build_set(
        state: &AppState,
        ds: &Dataset,
        user: &AuthenticatedUser,
        set: &NamedNode,
        body: &RuleSetBody,
    ) -> Result<(Vec<Triple>, usize), ApiErr> {
        let bad = |e: String| (StatusCode::BAD_REQUEST, e);
        let format = body
            .format
            .as_deref()
            .unwrap_or("swrlapi")
            .trim()
            .to_ascii_lowercase();
        let registry = state.prefix_registry.clone();
        let options = crate::swrl::ParseOptions {
            rdf_format: body.rdf_format.as_deref(),
            base_iri: body.base_iri.as_deref(),
            prefixes: body.prefixes.clone(),
            prefix_fallback: Some(std::sync::Arc::new(move |p: &str| {
                registry.lookup_local(p).map(|r| r.namespace)
            })),
        };
        let mut rules = crate::swrl::parse_rules(&format, &body.rules, &options).map_err(bad)?;
        if rules.is_empty() {
            return Err(bad("No rules found".to_string()));
        }
        crate::swrl::write::settle_property_atoms(&mut rules).map_err(bad)?;
        // Checked exactly as a run checks them, against the regime the
        // dataset runs its rules with now (class-expression atoms need one).
        let (sources, _) = crate::entailment::reasoning_sources(state, ds);
        let target = crate::entailment::inference_graph(&state.auth_db, &ds.id);
        let regime = crate::entailment::config(&state.auth_db, &ds.id)
            .ok()
            .flatten()
            .filter(|c| c.mode == "materialize")
            .map(|c| c.regime);
        crate::swrl::compile_rules_for_regime(
            &rules,
            Some(&target),
            Some(&sources),
            regime.as_deref(),
        )
        .map_err(bad)?;

        let mut triples = Vec::new();
        let subject = set.clone();
        let lit = |s: &str| Literal::new_simple_literal(s);
        triples.push(Triple::new(subject.clone(), nn(RDF_TYPE), ots("RuleSet")));
        if let Some(label) = body
            .label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            triples.push(Triple::new(subject.clone(), nn(RDFS_LABEL), lit(label)));
        }
        triples.push(Triple::new(
            subject.clone(),
            ots("ruleSource"),
            lit(&body.rules),
        ));
        triples.push(Triple::new(
            subject.clone(),
            ots("ruleFormat"),
            lit(&format),
        ));
        let opts = json!({
            "rdf_format": body.rdf_format,
            "base_iri": body.base_iri,
            "prefixes": body.prefixes,
        });
        triples.push(Triple::new(
            subject.clone(),
            ots("ruleOptions"),
            lit(&opts.to_string()),
        ));
        triples.push(Triple::new(
            subject.clone(),
            nn(DCT_MODIFIED),
            Literal::new_typed_literal(
                chrono::Utc::now().to_rfc3339(),
                nn("http://www.w3.org/2001/XMLSchema#dateTime"),
            ),
        ));
        let author = state
            .auth_db
            .get_user_by_id(&user.user_id)
            .ok()
            .flatten()
            .map(|u| u.username)
            .unwrap_or_else(|| user.user_id.clone());
        triples.push(Triple::new(subject.clone(), nn(DCT_CREATOR), lit(&author)));
        for (i, rule) in rules.iter().enumerate() {
            let imp = NamedNode::new(format!("{}:{}", set.as_str(), i + 1)).map_err(e500)?;
            triples.push(Triple::new(subject.clone(), ots("rule"), imp.clone()));
            triples.extend(crate::swrl::write::rule_to_rdf(rule, &imp).map_err(bad)?);
        }
        Ok((triples, rules.len()))
    }

    /// Replace the set `set` (absent: add it) with `triples`, in one
    /// transaction, registering the managed graph with the dataset first.
    pub(super) fn write_set(
        state: &AppState,
        dataset_id: &str,
        set: &NamedNode,
        triples: Vec<Triple>,
    ) -> Result<(), ApiErr> {
        let g = graph_node(dataset_id)?;
        let registered = state
            .auth_db
            .list_dataset_graph_entries(dataset_id)
            .map_err(e500)?
            .into_iter()
            .find(|e| e.graph_iri == g.as_str());
        match registered {
            None => {
                state
                    .auth_db
                    .add_dataset_graph_with_origin(dataset_id, g.as_str(), "created")
                    .map_err(e500)?;
                state
                    .auth_db
                    .set_dataset_graph_role(dataset_id, g.as_str(), Some(GraphKind::Entailment))
                    .map_err(e500)?;
            }
            Some(e) if e.graph_role != Some(GraphKind::Entailment) => {
                state
                    .auth_db
                    .set_dataset_graph_role(dataset_id, g.as_str(), Some(GraphKind::Entailment))
                    .map_err(e500)?;
            }
            Some(_) => {}
        }
        let graph = GraphName::NamedNode(g.clone());
        let mut ops: Vec<crate::store::QuadOp> = set_quads(state, &g, set)
            .into_iter()
            .map(crate::store::QuadOp::Remove)
            .collect();
        ops.extend(triples.into_iter().map(|t| {
            crate::store::QuadOp::Add(Quad::new(t.subject, t.predicate, t.object, graph.clone()))
        }));
        state.store.apply_quad_ops(&ops).map_err(e500)?;
        Ok(())
    }

    /// Remove the set `set`; false when there is no such set.
    pub(super) fn remove_set(
        state: &AppState,
        dataset_id: &str,
        set: &NamedNode,
    ) -> Result<bool, ApiErr> {
        let g = graph_node(dataset_id)?;
        let quads = set_quads(state, &g, set);
        if quads.is_empty() {
            return Ok(false);
        }
        let ops: Vec<crate::store::QuadOp> = quads
            .into_iter()
            .map(crate::store::QuadOp::Remove)
            .collect();
        state.store.apply_quad_ops(&ops).map_err(e500)?;
        Ok(true)
    }

    pub(super) fn set_exists(state: &AppState, dataset_id: &str, set: &NamedNode) -> bool {
        graph_node(dataset_id).is_ok_and(|g| {
            state
                .store
                .store()
                .quads_for_pattern(
                    Some(set.as_ref().into()),
                    Some(nn(RDF_TYPE)),
                    None,
                    Some(GraphNameRef::NamedNode(g.as_ref())),
                )
                .next()
                .is_some()
        })
    }

    pub(super) fn set_node(id: &str) -> Result<NamedNode, ApiErr> {
        uuid::Uuid::parse_str(id)
            .ok()
            .and_then(|u| NamedNode::new(format!("{SET_PREFIX}{u}")).ok())
            .ok_or_else(|| (StatusCode::NOT_FOUND, "No such rule set".to_string()))
    }
}

/// Run the dataset's rules again after a change: with its regime when it is
/// in `materialize` mode (an `owl2-dl` run is queued, as after a write),
/// else alone into its rules graph. What happened, for the response.
pub fn rerun(state: &AppState, dataset_id: &str) -> Value {
    match crate::entailment::config(&state.auth_db, dataset_id) {
        Ok(Some(c)) if c.mode == "materialize" => {
            if c.regime == "owl2-dl" {
                crate::entailment::schedule_background_run(state, dataset_id);
                return json!({ "regime": c.regime, "queued": true, "report": null, "error": null });
            }
            let result = crate::entailment::run_for_dataset(state, dataset_id, &c.regime);
            json!({
                "regime": c.regime,
                "queued": false,
                "report": crate::entailment::last_rules_report(dataset_id),
                "error": result.err().map(|e| e.message()),
            })
        }
        _ => match crate::entailment::run_rules_for_dataset(state, dataset_id, false) {
            Ok(report) => {
                json!({ "regime": null, "queued": false, "report": report, "error": null })
            }
            Err(e) => json!({ "regime": null, "queued": false, "report": null, "error": e }),
        },
    }
}

#[cfg(not(feature = "swrl"))]
fn feature_off() -> ApiErr {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "this build has no SWRL support (feature swrl)".to_string(),
    )
}

/// GET /api/datasets/:id/rules
pub async fn list_rules(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<Response, ApiErr> {
    let u = user.as_ref().map(|Extension(u)| u);
    let ds = dataset(&state, u.map(|u| u.user_id.as_str()), &dataset_id)?;
    #[cfg(not(feature = "swrl"))]
    {
        let _ = ds;
        Err(feature_off())
    }
    #[cfg(feature = "swrl")]
    {
        let w = can_write(&state, u, &ds);
        let st = state.clone();
        let (sets, other) = tokio::task::spawn_blocking(move || {
            Ok::<_, ApiErr>((imp::list_sets(&st, &ds.id)?, imp::other_rules(&st, &ds, w)))
        })
        .await
        .map_err(e500)??;
        Ok(Json(json!({
            "dataset_id": dataset_id,
            "graph": managed_graph(&dataset_id),
            "can_write": w,
            "sets": sets,
            "other": other,
            "inference_graph": crate::entailment::inference_graph(&state.auth_db, &dataset_id),
            "last_run": crate::entailment::last_rules_report(&dataset_id),
            "formats": crate::swrl::FORMATS,
        }))
        .into_response())
    }
}

#[cfg(feature = "swrl")]
async fn save(
    state: AppState,
    user: Option<Extension<AuthenticatedUser>>,
    dataset_id: String,
    set_id: Option<String>,
    body: RuleSetBody,
) -> Result<Response, ApiErr> {
    let ds = dataset(
        &state,
        user.as_ref().map(|Extension(u)| u.user_id.as_str()),
        &dataset_id,
    )?;
    let u = writer(&state, &user, &ds)?.clone();
    let creating = set_id.is_none();
    let id = set_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let set = imp::set_node(&id)?;
    let st = state.clone();
    let (count, run) = tokio::task::spawn_blocking(move || -> Result<(usize, Value), ApiErr> {
        if !creating && !imp::set_exists(&st, &ds.id, &set) {
            return Err((StatusCode::NOT_FOUND, "No such rule set".to_string()));
        }
        let (triples, count) = imp::build_set(&st, &ds, &u, &set, &body)?;
        imp::write_set(&st, &ds.id, &set, triples)?;
        Ok((count, rerun(&st, &ds.id)))
    })
    .await
    .map_err(e500)??;
    let status = if creating {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(json!({ "id": id, "rules": count, "run": run })),
    )
        .into_response())
}

/// POST /api/datasets/:id/rules — add a rule set.
pub async fn create_rules(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Json(body): Json<RuleSetBody>,
) -> Result<Response, ApiErr> {
    #[cfg(not(feature = "swrl"))]
    {
        let _ = (state, user, dataset_id, body);
        Err(feature_off())
    }
    #[cfg(feature = "swrl")]
    {
        save(state, user, dataset_id, None, body).await
    }
}

/// PUT /api/datasets/:id/rules/:set — replace a rule set.
pub async fn update_rules(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, set_id)): Path<(String, String)>,
    Json(body): Json<RuleSetBody>,
) -> Result<Response, ApiErr> {
    #[cfg(not(feature = "swrl"))]
    {
        let _ = (state, user, dataset_id, set_id, body);
        Err(feature_off())
    }
    #[cfg(feature = "swrl")]
    {
        save(state, user, dataset_id, Some(set_id), body).await
    }
}

/// DELETE /api/datasets/:id/rules/:set — remove a rule set.
pub async fn delete_rules(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, set_id)): Path<(String, String)>,
) -> Result<Response, ApiErr> {
    let ds = dataset(
        &state,
        user.as_ref().map(|Extension(u)| u.user_id.as_str()),
        &dataset_id,
    )?;
    writer(&state, &user, &ds)?;
    #[cfg(not(feature = "swrl"))]
    {
        let _ = set_id;
        Err(feature_off())
    }
    #[cfg(feature = "swrl")]
    {
        let set = imp::set_node(&set_id)?;
        let st = state.clone();
        let run = tokio::task::spawn_blocking(move || -> Result<Value, ApiErr> {
            if !imp::remove_set(&st, &ds.id, &set)? {
                return Err((StatusCode::NOT_FOUND, "No such rule set".to_string()));
            }
            Ok(rerun(&st, &ds.id))
        })
        .await
        .map_err(e500)??;
        Ok(Json(json!({ "deleted": set_id, "run": run })).into_response())
    }
}

/// POST /api/datasets/:id/rules/run — run the dataset's stored rules now.
pub async fn run_rules(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<Response, ApiErr> {
    let ds = dataset(
        &state,
        user.as_ref().map(|Extension(u)| u.user_id.as_str()),
        &dataset_id,
    )?;
    writer(&state, &user, &ds)?;
    let st = state.clone();
    let run = tokio::task::spawn_blocking(move || rerun(&st, &ds.id))
        .await
        .map_err(e500)?;
    Ok(Json(run).into_response())
}
