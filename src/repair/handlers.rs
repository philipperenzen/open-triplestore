//! HTTP: `POST /api/datasets/:dataset_id/repair` (§8.1).
//!
//! Mounted beside `/validate` and `/infer` (token required, endpoint ACL).
//! Write permission on the dataset, because a proposal quotes the triples it
//! would delete, and a graph the dataset holds as private is readable only by
//! its writers. Status codes copy the neighbours: 404 for a dataset the
//! caller cannot see, 403 without write access, 400 for a rule set that does
//! not load or stratify, no shapes to compile, or premises over the quad
//! cap, 503 when either semaphore is full or the copy ran out of time, and
//! 200 — with `summary.exhausted` — when a budget ran out mid-chase, since
//! what was proposed by then is still a valid partial repair.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::Deserialize;

use super::compile::Policies;
use super::rules::Origin;
use super::run::{self, Job, RepairRequest, RunError};
use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;

type ApiErr = (StatusCode, String);

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn bad(msg: impl Into<String>) -> ApiErr {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn overloaded() -> ApiErr {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "Server overloaded".to_string(),
    )
}

/// The repair routes (mounted with `require_auth` and the endpoint ACL).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/datasets/:dataset_id/repair", post(repair_dataset))
        .route(
            "/api/datasets/:dataset_id/repair/proposals",
            get(list_proposals),
        )
        .route(
            "/api/datasets/:dataset_id/repair/proposals/:proposal_id",
            get(get_proposal),
        )
        .route(
            "/api/datasets/:dataset_id/repair/proposals/:proposal_id/reject",
            post(reject_proposal),
        )
}

/// An IRI a caller names: absolute, and nothing that could leave `<…>`.
fn check_iri(iri: &str, what: &str) -> Result<(), ApiErr> {
    let bad_char = iri.chars().any(|c| {
        matches!(c, '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\') || c.is_whitespace()
    });
    if bad_char || oxigraph::model::NamedNode::new(iri).is_err() {
        return Err(bad(format!("{what} <{iri}> is not a valid IRI")));
    }
    Ok(())
}

/// The dataset, if the caller may write it: 404 when they cannot see it,
/// 403 when they can see but not write it.
pub(crate) fn writable_dataset(
    state: &AppState,
    user: &AuthenticatedUser,
    dataset_id: &str,
) -> Result<crate::auth::models::Dataset, ApiErr> {
    let ds = state
        .auth_db
        .get_dataset(dataset_id)
        .map_err(e500)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state
        .auth_db
        .can_access_dataset(Some(&user.user_id), &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    Ok(ds)
}

/// Whether `user` may read `iri`: the `/sparql` rule, or a graph-ACL grant.
fn may_read_graph(state: &AppState, user: &AuthenticatedUser, iri: &str) -> Result<bool, ApiErr> {
    if user.is_admin() {
        return Ok(true);
    }
    let readable = crate::server::routes::accessible_read_graphs(state, Some(user))
        .map_err(|e| e500(e.message()))?;
    Ok(readable.contains(iri)
        || crate::auth::acl::check_graph_permission(Some(user), iri, "read", &state.auth_db))
}

/// The newest commit touching `graphs`, as its IRI.
pub(crate) fn newest_commit(state: &AppState, graphs: &[String]) -> Option<String> {
    crate::commit_log::list_commits(
        &state.store,
        &crate::commit_log::CommitScope::Graphs(graphs.to_vec()),
        &crate::commit_log::CommitQuery {
            limit: Some(1),
            ..Default::default()
        },
    )
    .into_iter()
    .next()
    .map(|c| {
        format!(
            "{}/commit/{}",
            state.base_url.trim_end_matches('/'),
            c.commit_id
        )
    })
}

/// Parse heuristic rules (Turtle `ots:Rule`s) into specs.
pub(crate) fn heuristic_rules(turtle: &str) -> Result<Vec<super::rules::RuleSpec>, ApiErr> {
    const GRAPH: &str = "urn:ots:heuristic-rules";
    let temp = crate::store::TripleStore::in_memory().map_err(e500)?;
    temp.load_str(turtle, oxigraph::io::RdfFormat::Turtle, Some(GRAPH))
        .map_err(|e| bad(format!("heuristic_rules is not Turtle: {e}")))?;
    let mut rules = super::vocab::load_rules(&temp, GRAPH, Origin::Heuristic)
        .map_err(|e| bad(e.to_string()))?;
    if rules.is_empty() {
        return Err(bad("heuristic_rules holds no ots:Rule"));
    }
    for r in &mut rules {
        r.confidence = super::rules::Confidence::Heuristic;
        r.source_graph = None;
    }
    Ok(rules)
}

/// Everything the handler resolves before the blocking run: the job, and
/// whether the caller asked for the patch only and to keep the proposal.
pub(crate) struct Prepared {
    pub job: Job,
    pub timeout: std::time::Duration,
    pub persist: bool,
}

/// Resolve a request into a job, with the caller's rights (§5.2, §8.1).
pub(crate) fn prepare_job(
    state: &AppState,
    user: &AuthenticatedUser,
    dataset_id: &str,
    req: RepairRequest,
) -> Result<Prepared, ApiErr> {
    let dataset = writable_dataset(state, user, dataset_id)?;
    let policies = Policies::parse(&req.policies).map_err(bad)?;
    let heuristic = match req.heuristic_rules.as_deref() {
        Some(t) if !t.trim().is_empty() => Some(heuristic_rules(t)?),
        _ => None,
    };

    // Shapes: an explicit graph the caller may read, else the dataset's
    // own (resolved as for /validate and /infer), less the private graphs
    // of other datasets the caller may not read.
    let (shapes, shapes_withheld) =
        match req.shapes_graph.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(iri) => {
                check_iri(iri, "shapes_graph")?;
                if !may_read_graph(state, user, iri)? {
                    return Err((
                        StatusCode::FORBIDDEN,
                        format!("Read access denied for shapes graph <{iri}>"),
                    ));
                }
                (vec![iri.to_string()], 0)
            }
            None => {
                let all = crate::server::routes::resolve_shapes_graphs(
                    crate::server::routes::dataset_shapes_sources(state, &dataset),
                );
                let withheld =
                    crate::auth::acl::withheld_private_graphs(&state.auth_db, Some(user))
                        .map_err(e500)?;
                let kept: Vec<String> = all
                    .iter()
                    .filter(|g| !withheld.contains(*g))
                    .cloned()
                    .collect();
                let n = all.len() - kept.len();
                (kept, n)
            }
        };
    let needs_shapes = heuristic.is_none() && req.rules.is_empty() && req.derive.from_shapes;
    if needs_shapes && shapes.is_empty() {
        return Err(bad(if shapes_withheld > 0 {
            crate::server::routes::NO_READABLE_SHAPES_GRAPH_MSG
        } else {
            crate::server::routes::NO_SHAPES_GRAPH_MSG
        }));
    }

    // Authored rule graphs: each one the caller may read.
    let mut authored = Vec::new();
    for g in &req.rules {
        check_iri(g, "rule graph")?;
        if !may_read_graph(state, user, g)? {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Read access denied for rule graph <{g}>"),
            ));
        }
        let rules = super::vocab::load_rules(&state.store, g, Origin::Authored)
            .map_err(|e| bad(e.to_string()))?;
        if rules.is_empty() {
            return Err(bad(format!("rule graph <{g}> holds no ots:Rule")));
        }
        authored.extend(rules);
    }

    if let Some(scope) = &req.scope.graphs {
        let registered = state
            .auth_db
            .list_dataset_graphs(dataset_id)
            .map_err(e500)?;
        for g in scope {
            if !registered.contains(g) {
                return Err(bad(format!(
                    "scope.graphs: <{g}> is not a graph of dataset {dataset_id}"
                )));
            }
        }
    }
    let focus = match &req.scope.focus {
        Some(list) => {
            let mut terms = Vec::new();
            for f in list {
                check_iri(f, "scope.focus")?;
                terms.push(oxigraph::model::Term::NamedNode(
                    oxigraph::model::NamedNode::new_unchecked(f.as_str()),
                ));
            }
            Some(terms)
        }
        None => None,
    };

    let premises = super::sandbox::resolve(super::sandbox::ResolveInput {
        state,
        user,
        dataset: &dataset,
        shapes,
        shapes_withheld,
        scope_graphs: req.scope.graphs.as_deref(),
    })
    .map_err(e500)?;
    let cap = super::runtime(&state.store).max_quads();
    let estimate = premises.quad_estimate(&state.store);
    if estimate > cap {
        return Err(bad(format!(
            "the graphs a repair of this dataset reads hold {estimate} quads, over OTS_REPAIR_MAX_QUADS ({cap}); \
             narrow scope.graphs or raise the limit"
        )));
    }
    let base_commit = newest_commit(state, &premises.dataset_graphs);
    let timeout_secs = req.budget.timeout_secs.unwrap_or(state.query_timeout_secs);
    let (deadline, timeout) = run::deadline(timeout_secs);
    let partial = premises.withheld > 0 || premises.shapes_withheld > 0;
    let rule_graphs = req.rules.clone();
    Ok(Prepared {
        job: Job {
            main: state.store.clone(),
            dataset_id: dataset_id.to_string(),
            base_url: state.base_url.to_string(),
            actor_iri: Some(format!(
                "{}/users/{}",
                state.base_url.trim_end_matches('/'),
                user.user_id
            )),
            premises,
            authored,
            rule_graphs,
            heuristic,
            derive: req.derive.clone(),
            policies,
            focus,
            rounds: req
                .budget
                .rounds
                .unwrap_or(run::DEFAULT_ROUNDS)
                .clamp(1, run::MAX_ROUNDS),
            nulls: req
                .budget
                .nulls
                .unwrap_or(run::DEFAULT_NULLS)
                .min(run::MAX_NULLS),
            ops: req.budget.ops.unwrap_or(run::DEFAULT_OPS).min(run::MAX_OPS),
            deadline,
            validate: req.validate,
            semi_naive: req.semi_naive.unwrap_or(true),
            base_commit,
            partial,
        },
        timeout,
        persist: req.persist,
    })
}

/// Run a prepared job on a blocking thread, holding both permits.
pub(crate) async fn execute(
    state: &AppState,
    prepared: Prepared,
) -> Result<super::proposal::Proposal, ApiErr> {
    // The dedicated repair permit first (cheap to refuse), then the
    // expensive-operation permit every heavy handler takes.
    let rt = super::runtime(&state.store);
    let _repair = rt
        .semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| overloaded())?;
    let _expensive = state
        .expensive_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| overloaded())?;
    // The in-task deadline is what stops the run; this outer bound only
    // keeps the response from waiting on a run that ignores it.
    let outer = prepared.timeout + std::time::Duration::from_secs(15);
    let task = tokio::task::spawn_blocking(move || run::run(prepared.job));
    let result = tokio::time::timeout(outer, task)
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Repair timed out".to_string(),
            )
        })?
        .map_err(e500)?;
    result.map_err(|e| match e {
        RunError::BadRequest(m) => bad(m),
        RunError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m),
        RunError::Internal(m) => e500(m),
    })
}

/// POST /api/datasets/:dataset_id/repair — propose a repair (§8.1). JSON
/// report by default; `Accept: application/rdf-patch` returns the patch
/// text only.
pub async fn repair_dataset(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiErr> {
    let req: RepairRequest = if body.iter().all(u8::is_ascii_whitespace) {
        RepairRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| bad(format!("invalid repair request: {e}")))?
    };
    let prepared = prepare_job(&state, &user, &dataset_id, req)?;
    let persist = prepared.persist;
    let proposal = execute(&state, prepared).await?;
    let mut report = proposal.report.clone();
    if persist {
        super::persist::save(&state, &dataset_id, &proposal).map_err(e500)?;
        report["persisted"] = serde_json::Value::Bool(true);
    }
    let wants_patch = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains(crate::rdf_patch::MEDIA_TYPE));
    if wants_patch {
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, crate::rdf_patch::MEDIA_TYPE)],
            proposal.patch,
        )
            .into_response());
    }
    Ok(Json(report).into_response())
}

/// The dataset graphs a proposal's staleness is judged against.
fn dataset_graphs(state: &AppState, dataset_id: &str) -> Result<Vec<String>, ApiErr> {
    Ok(state
        .auth_db
        .list_dataset_graphs(dataset_id)
        .map_err(e500)?
        .into_iter()
        .filter(|g| !g.starts_with("urn:system:reports:"))
        .collect())
}

/// A `proposed` proposal whose base moved is `superseded` (§7.3); the
/// returned flag says whether it is stale.
fn refresh_staleness(
    state: &AppState,
    stored: super::persist::Stored,
    graphs: &[String],
) -> Result<(super::persist::Stored, bool), ApiErr> {
    if stored.status != "proposed" {
        let stale = stored_is_stale_status(&stored);
        return Ok((stored, stale));
    }
    if !super::persist::is_stale(state, &stored, graphs) {
        return Ok((stored, false));
    }
    let s = super::persist::set_status(
        state,
        stored,
        "superseded",
        Some("the dataset changed after the proposal was computed".into()),
        None,
    )
    .map_err(e500)?;
    Ok((s, true))
}

fn stored_is_stale_status(s: &super::persist::Stored) -> bool {
    s.status == "superseded"
}

/// GET /api/datasets/:dataset_id/repair/proposals — the kept proposals,
/// newest first.
pub async fn list_proposals(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    writable_dataset(&state, &user, &dataset_id)?;
    let graphs = dataset_graphs(&state, &dataset_id)?;
    let mut out = Vec::new();
    for s in super::persist::list(&state, &dataset_id) {
        let (s, stale) = refresh_staleness(&state, s, &graphs)?;
        let mut v = s.summary();
        v["stale"] = serde_json::Value::Bool(stale);
        out.push(v);
    }
    Ok(Json(serde_json::json!({ "proposals": out })))
}

#[derive(Debug, Default, Deserialize)]
pub struct Paging {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// GET /api/datasets/:dataset_id/repair/proposals/:proposal_id — report,
/// patch, a page of actions (`?offset=&limit=`) and `stale`.
pub async fn get_proposal(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((dataset_id, proposal_id)): Path<(String, String)>,
    Query(page): Query<Paging>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    writable_dataset(&state, &user, &dataset_id)?;
    let stored = super::persist::get(&state, &dataset_id, &proposal_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Proposal not found".to_string()))?;
    let graphs = dataset_graphs(&state, &dataset_id)?;
    let (stored, stale) = refresh_staleness(&state, stored, &graphs)?;
    let wants_patch = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains(crate::rdf_patch::MEDIA_TYPE));
    if wants_patch {
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, crate::rdf_patch::MEDIA_TYPE)],
            stored.patch().to_string(),
        )
            .into_response());
    }
    let offset = page.offset.unwrap_or(0);
    let limit = page
        .limit
        .unwrap_or(super::proposal::MAX_INLINE_ACTIONS)
        .clamp(1, super::proposal::MAX_INLINE_ACTIONS);
    let actions: Vec<serde_json::Value> = stored
        .actions
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect();
    let mut report = stored.report.clone();
    report["status"] = serde_json::json!(stored.status);
    report["stale"] = serde_json::Value::Bool(stale);
    report["created_at"] = serde_json::json!(stored.created_at);
    report["updated_at"] = serde_json::json!(stored.updated_at);
    if let Some(r) = &stored.status_reason {
        report["status_reason"] = serde_json::json!(r);
    }
    if let Some(c) = &stored.applied_commit {
        report["applied_commit"] = serde_json::json!(c);
    }
    report["actions"] = serde_json::Value::Array(actions);
    report["actions_offset"] = serde_json::json!(offset);
    report["actions_total"] = serde_json::json!(stored.actions.len());
    Ok(Json(report).into_response())
}

/// POST /api/datasets/:dataset_id/repair/proposals/:proposal_id/reject
pub async fn reject_proposal(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((dataset_id, proposal_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    writable_dataset(&state, &user, &dataset_id)?;
    let stored = super::persist::get(&state, &dataset_id, &proposal_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Proposal not found".to_string()))?;
    if stored.status == "applied" {
        return Err((
            StatusCode::CONFLICT,
            "The proposal was applied; reverse it with its inverse patch instead".to_string(),
        ));
    }
    let stored = if stored.status == "rejected" {
        stored
    } else {
        super::persist::set_status(
            &state,
            stored,
            "rejected",
            Some(format!("rejected by {}", user.user_id)),
            None,
        )
        .map_err(e500)?
    };
    Ok(Json(serde_json::json!({
        "proposal_id": stored.id,
        "status": stored.status,
    })))
}
