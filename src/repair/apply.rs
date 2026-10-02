//! Applying a patch against a base marker (`docs/notes/repair-layer-design.md`
//! §8.3): the opt-in preconditions and gate of `POST
//! /api/datasets/:dataset_id/patch`, and `POST
//! /api/datasets/:dataset_id/repair/proposals/:proposal_id/apply`.
//!
//! On the patch route everything here is opt-in. Without a query parameter
//! or `If-Match` the request reaches the handler untouched; with them:
//!
//! * `?if-base-commit=<iri>` (or `If-Match: "<iri>"`): the newest commit
//!   touching any graph the patch names must still be that one (an empty
//!   value: no commit touched them yet), else 409;
//! * `?if-base-sequence=<n>` (and `&if-base-epoch=` when the client has it):
//!   no change-log row after `n` may touch those graphs, else 409. It needs
//!   change capture (400 without it), and unlike the commit check it also
//!   sees writes that record no commit;
//! * `?validate=true`: the SHACL write gates of every graph the patch touches
//!   run over what the graph would hold after it, and a refusal is the 422 a
//!   Graph Store write's gate refusal is.
//!
//! Both routes take the dataset's patch lock ([`lock_dataset`]) from the
//! precondition through the write, so two patch applies to one dataset never
//! interleave between check and write. Other writers do not take it: a
//! SPARQL update or Graph Store write landing between the check and the
//! write is not detected.

use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, OnceLock};

use axum::body::{Body, Bytes};
use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json, RequestExt};
use dashmap::DashMap;
use oxigraph::model::{GraphName, GraphNameRef, NamedNode, Quad};
use serde::Deserialize;

use super::handlers::{newest_commit, writable_dataset};
use super::persist;
use crate::auth::middleware::AuthenticatedUser;
use crate::server::error::AppError;
use crate::server::AppState;

type Locks = DashMap<String, Arc<tokio::sync::Mutex<()>>>;

static PATCH_LOCKS: OnceLock<Locks> = OnceLock::new();

/// The lock the two patch entry points hold from their precondition through
/// their write: a `tokio` mutex, held across the handler's awaits.
pub struct PatchLock {
    dataset_id: String,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

/// Take the patch lock of `dataset_id`.
pub async fn lock_dataset(dataset_id: &str) -> PatchLock {
    let mutex = PATCH_LOCKS
        .get_or_init(DashMap::new)
        .entry(dataset_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    PatchLock {
        dataset_id: dataset_id.to_string(),
        guard: Some(mutex.lock_owned().await),
    }
}

impl Drop for PatchLock {
    /// Release the lock, and drop its entry when nobody holds or waits on it,
    /// so requests naming datasets that do not exist leave nothing behind.
    fn drop(&mut self) {
        self.guard.take();
        if let Some(map) = PATCH_LOCKS.get() {
            map.remove_if(&self.dataset_id, |_, m| Arc::strong_count(m) == 1);
        }
    }
}

/// The opt-in query parameters of a gated apply.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ApplyOptions {
    /// `true` / `1` / `yes` / `on` turn the gate on; any other value leaves
    /// it off, as the route did before it read the parameter.
    pub validate: Option<String>,
    #[serde(rename = "if-base-commit")]
    pub if_base_commit: Option<String>,
    #[serde(rename = "if-base-sequence")]
    pub if_base_sequence: Option<i64>,
    #[serde(rename = "if-base-epoch")]
    pub if_base_epoch: Option<String>,
}

impl ApplyOptions {
    /// `If-Match` names a base commit when the query does not.
    fn with_headers(mut self, headers: &HeaderMap) -> Self {
        if self.if_base_commit.is_none() {
            if let Some(v) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) {
                let v = v.trim().trim_start_matches("W/").trim_matches('"');
                if v != "*" {
                    self.if_base_commit = Some(v.to_string());
                }
            }
        }
        self
    }

    fn validate(&self) -> bool {
        self.validate.as_deref().is_some_and(|v| {
            ["true", "1", "yes", "on"]
                .iter()
                .any(|t| v.trim().eq_ignore_ascii_case(t))
        })
    }

    fn any(&self) -> bool {
        self.validate() || self.if_base_commit.is_some() || self.if_base_sequence.is_some()
    }
}

/// A commit IRI from what a client sends: the IRI itself, or the bare id.
fn commit_iri(state: &AppState, v: &str) -> String {
    if v.contains("://") || v.starts_with("urn:") {
        v.to_string()
    } else {
        format!("{}/commit/{v}", state.base_url.trim_end_matches('/'))
    }
}

fn stale(precondition: &str, message: String, current: serde_json::Value) -> Response {
    AppError::Conflict(serde_json::json!({
        "error": "stale_base",
        "message": message,
        "precondition": precondition,
        "current": current,
    }))
    .into_response()
}

/// Check the preconditions (§8.3 step 1): `Err` is the 409, or a 400 for a
/// precondition the server cannot check.
fn check_preconditions(
    state: &AppState,
    graphs: &[String],
    opts: &ApplyOptions,
) -> Result<(), Box<Response>> {
    if let Some(expected) = &opts.if_base_commit {
        let newest = newest_commit(state, graphs);
        let expected = (!expected.trim().is_empty()).then(|| commit_iri(state, expected.trim()));
        if newest != expected {
            return Err(Box::new(stale(
                "if-base-commit",
                format!(
                    "the graphs changed since the base commit: the newest commit touching them is {}",
                    newest.as_deref().unwrap_or("none")
                ),
                serde_json::json!(newest),
            )));
        }
    }
    if let Some(seq) = opts.if_base_sequence {
        let changes = state.store.changes();
        if !changes.enabled() {
            return Err(Box::new(
                (
                    StatusCode::BAD_REQUEST,
                    "if-base-sequence needs the change log (OTS_CHANGE_CAPTURE=on)".to_string(),
                )
                    .into_response(),
            ));
        }
        let head = changes.last_seq();
        let current = serde_json::json!({ "sequence": head, "epoch": changes.epoch() });
        if opts
            .if_base_epoch
            .as_deref()
            .is_some_and(|e| e != changes.epoch())
        {
            return Err(Box::new(stale(
                "if-base-sequence",
                "the change log started a new epoch after the base sequence".to_string(),
                current,
            )));
        }
        if seq > head {
            return Err(Box::new(stale(
                "if-base-sequence",
                format!("the base sequence {seq} is ahead of the change log ({head})"),
                current,
            )));
        }
        if persist::touched_since(state, graphs, seq) {
            return Err(Box::new(stale(
                "if-base-sequence",
                format!("the graphs changed after sequence {seq} (the log is at {head})"),
                current,
            )));
        }
    }
    Ok(())
}

/// The write gates of `graphs` over what each would hold after `rows`
/// (§8.3 step 2): the gates a Graph Store write to the same graph passes.
/// `Err` is the first refusal's report, as `writer` may see it; a failed
/// gate lookup refuses.
fn check_gates(
    state: &AppState,
    writer: &AuthenticatedUser,
    rows: &[(bool, Quad)],
    graphs: &[String],
) -> Result<(), crate::shacl::report::ValidationReport> {
    use crate::shacl_studio::gate;
    let studio = crate::shacl_studio::store::ShaclStudioStore::new(state.auth_db.pool());
    let ctx = gate::GateContext {
        main_store: &state.store,
        auth_db: &state.auth_db,
        studio: &studio,
        base_url: &state.base_url,
        writer: Some(writer),
    };
    for g in graphs {
        if !gate::import_gates_apply(ctx, g) {
            continue;
        }
        let name = NamedNode::new(g).map_err(gate::gate_error)?;
        let mut future: HashSet<Quad> = state
            .store
            .quads_for_graph(GraphNameRef::NamedNode(name.as_ref()))
            .map_err(gate::gate_error)?
            .into_iter()
            .collect();
        for (add, q) in rows {
            if q.graph_name.as_ref() != GraphNameRef::NamedNode(name.as_ref()) {
                continue;
            }
            if *add {
                future.insert(q.clone());
            } else {
                future.remove(q);
            }
        }
        let future: Vec<Quad> = future.into_iter().collect();
        gate::check_import_gates(ctx, g, &future)?;
    }
    Ok(())
}

/// A patch's rows as quads, in order, `true` for an add: `None` when a row
/// names no graph. The one place that reads `rdf_patch`'s row type.
fn patch_rows(patch: &crate::rdf_patch::Patch) -> Result<Option<Vec<(bool, Quad)>>, String> {
    use crate::rdf_patch::Op;
    let mut rows = Vec::with_capacity(patch.ops.len());
    for op in &patch.ops {
        let (add, q) = match op {
            Op::Add(q) => (true, q),
            Op::Delete(q) => (false, q),
        };
        let Some(g) = &q.g else {
            return Ok(None);
        };
        let line = format!("{} {} {} {g} .", q.s, q.p, q.o);
        let quad = parse_nquad(&line)?;
        rows.push((add, quad));
    }
    Ok(Some(rows))
}

fn parse_nquad(line: &str) -> Result<Quad, String> {
    oxigraph::io::RdfParser::from_format(oxigraph::io::RdfFormat::NQuads)
        .for_reader(line.as_bytes())
        .next()
        .ok_or_else(|| format!("not a quad: {line}"))?
        .map_err(|e| e.to_string())
}

/// The named graphs `rows` touch, sorted.
fn graphs_of(rows: &[(bool, Quad)]) -> Vec<String> {
    rows.iter()
        .filter_map(|(_, q)| match &q.graph_name {
            GraphName::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// The layer on `POST /api/datasets/:dataset_id/patch`. A request without an
/// option goes to the handler as it is, under the dataset's patch lock. A
/// request with one has its access checked as the handler checks it, its
/// preconditions and gate evaluated, and then reaches the handler with the
/// lock still held. A patch this layer cannot read (no graph on a row, an
/// unregistered graph, a parse error) also goes to the handler, which
/// refuses it as it always has.
pub async fn patch_route_layer(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let opts = match Query::<ApplyOptions>::try_from_uri(req.uri()) {
        Ok(Query(o)) => o.with_headers(req.headers()),
        Err(e) => return (StatusCode::BAD_REQUEST, e.body_text()).into_response(),
    };
    let (mut parts, body) = req.into_parts();
    let dataset_id = match Path::<String>::from_request_parts(&mut parts, &state).await {
        Ok(Path(id)) => id,
        Err(e) => return e.into_response(),
    };
    let user = parts.extensions.get::<AuthenticatedUser>().cloned();
    let (Some(user), true) = (user, opts.any()) else {
        let _held = lock_dataset(&dataset_id).await;
        return next.run(Request::from_parts(parts, body)).await;
    };
    if let Err(e) = writable_dataset(&state, &user, &dataset_id) {
        return e.into_response();
    }
    // The body under the route's own size limit, as the handler reads it.
    let (parts, body) = Request::from_parts(parts, body)
        .with_limited_body()
        .into_parts();
    let bytes: Bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("Failed to buffer the request body: {e}"),
            )
                .into_response()
        }
    };
    let pass = |parts, bytes: Bytes| Request::from_parts(parts, Body::from(bytes));
    let rows = std::str::from_utf8(&bytes)
        .ok()
        .and_then(|t| crate::rdf_patch::parse(t).ok())
        .and_then(|p| patch_rows(&p).ok().flatten());
    let registered: HashSet<String> = state
        .auth_db
        .list_dataset_graphs(&dataset_id)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let readable = |r: &Vec<(bool, Quad)>| {
        !r.is_empty() && graphs_of(r).iter().all(|g| registered.contains(g))
    };
    let Some(rows) = rows.filter(readable) else {
        let _held = lock_dataset(&dataset_id).await;
        return next.run(pass(parts, bytes)).await;
    };
    let graphs = graphs_of(&rows);
    let _held = lock_dataset(&dataset_id).await;
    if let Err(resp) = check_preconditions(&state, &graphs, &opts) {
        return *resp;
    }
    if opts.validate() {
        let st = state.clone();
        let gs = graphs.clone();
        let gated = tokio::task::spawn_blocking(move || check_gates(&st, &user, &rows, &gs)).await;
        match gated {
            Ok(Ok(())) => {}
            Ok(Err(report)) => return AppError::ValidationFailed(report).into_response(),
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        }
    }
    next.run(pass(parts, bytes)).await
}

/// A stored proposal's rows, in patch order: the patch is ours, so every
/// `A` / `D` line is one N-Quads statement after its code.
pub(crate) fn proposal_rows(patch: &str) -> Result<Vec<(bool, Quad)>, String> {
    let mut rows = Vec::new();
    for line in patch.lines() {
        let (add, rest) = match (line.strip_prefix("A "), line.strip_prefix("D ")) {
            (Some(rest), _) => (true, rest),
            (_, Some(rest)) => (false, rest),
            _ => continue,
        };
        rows.push((add, parse_nquad(rest)?));
    }
    Ok(rows)
}

/// A ground update that deletes, then inserts, `rows`. Every term went
/// through the N-Quads parser, so its `Display` form is a complete term.
pub(crate) fn ground_update(rows: &[(bool, Quad)]) -> String {
    let block = |add: bool| {
        let mut by_graph: std::collections::BTreeMap<String, String> = Default::default();
        for (a, q) in rows {
            if *a != add {
                continue;
            }
            by_graph
                .entry(q.graph_name.to_string())
                .or_default()
                .push_str(&format!(
                    "    {} {} {} .\n",
                    q.subject, q.predicate, q.object
                ));
        }
        if by_graph.is_empty() {
            return None;
        }
        let body: String = by_graph
            .iter()
            .map(|(g, t)| format!("  GRAPH {g} {{\n{t}  }}\n"))
            .collect();
        Some(format!(
            "{} {{\n{body}}}",
            if add { "INSERT DATA" } else { "DELETE DATA" }
        ))
    };
    [block(false), block(true)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ;\n")
}

/// `POST /api/datasets/:dataset_id/repair/proposals/:proposal_id/apply` —
/// apply a kept proposal (§8.2, §8.3).
///
/// The patch is read from the proposal, so a proposal larger than the
/// request body limit is applied here. Its own base marker is the
/// precondition — the change-log sequence when it carries one and the log
/// is in the same epoch, its base commit otherwise — plus any the request
/// adds; a proposal computed from a state the dataset has left becomes
/// `superseded` (409). The write gates always run: this route is new, so
/// gating it changes no existing contract, and a write the dataset's own
/// gates refuse on the Graph Store route must not go through here
/// (`?validate=true` is accepted and changes nothing). The write is one
/// ground update, so the count and text indexes take the exact delta; the
/// commit records the proposal in its metadata, and the apply is audited as
/// the SPARQL update it is.
pub async fn apply_proposal(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((dataset_id, proposal_id)): Path<(String, String)>,
    Query(query): Query<ApplyOptions>,
    headers: HeaderMap,
) -> Response {
    if let Err(e) = writable_dataset(&state, &user, &dataset_id) {
        return e.into_response();
    }
    let _held = lock_dataset(&dataset_id).await;
    let Some(stored) = persist::get(&state, &dataset_id, &proposal_id) else {
        return (StatusCode::NOT_FOUND, "Proposal not found".to_string()).into_response();
    };
    if stored.status != "proposed" {
        return AppError::Conflict(serde_json::json!({
            "error": "not_proposed",
            "message": format!("the proposal is {}; only a proposed one can be applied", stored.status),
            "status": stored.status,
        }))
        .into_response();
    }
    let rows = match proposal_rows(stored.patch()) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("the stored proposal does not read as a patch: {e}"),
            )
                .into_response()
        }
    };
    let graphs = graphs_of(&rows);
    let id = stored.id.clone();
    let supersede = |state: &AppState, stored: persist::Stored, reason: String| {
        if let Err(e) = persist::set_status(state, stored, "superseded", Some(reason), None) {
            tracing::warn!("could not mark proposal {id} superseded: {e}");
        }
    };
    let registered: Vec<String> = match state.auth_db.list_dataset_graphs(&dataset_id) {
        Ok(g) => g
            .into_iter()
            .filter(|g| !g.starts_with("urn:system:reports:"))
            .collect(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if let Some(g) = graphs.iter().find(|g| !registered.contains(*g)) {
        let message = format!("graph <{g}> is no longer registered to the dataset");
        supersede(&state, stored, message.clone());
        return stale("registration", message, serde_json::Value::Null);
    }
    // The proposal's own marker, over the graphs its run read; then the
    // request's, over the graphs the patch touches.
    let base = stored.base().clone();
    let base_graphs = persist::base_graphs(&stored, &registered);
    let changes = state.store.changes();
    let mut own = ApplyOptions::default();
    match (base["sequence"].as_i64(), base["epoch"].as_str()) {
        (Some(seq), Some(epoch)) if changes.enabled() && changes.epoch() == epoch => {
            own.if_base_sequence = Some(seq);
            own.if_base_epoch = Some(epoch.to_string());
        }
        _ => own.if_base_commit = Some(base["commit"].as_str().unwrap_or("").to_string()),
    }
    if let Err(resp) = check_preconditions(&state, &base_graphs, &own) {
        let resp = *resp;
        supersede(
            &state,
            stored,
            "the dataset changed after the proposal was computed".to_string(),
        );
        return resp;
    }
    let opts = query.with_headers(&headers);
    if let Err(resp) = check_preconditions(&state, &graphs, &opts) {
        return *resp;
    }
    let heuristic = stored.report["rules"]["heuristic"].as_u64().unwrap_or(0) > 0;
    if rows.is_empty() {
        return Json(serde_json::json!({
            "applied": false,
            "proposal_id": id,
            "status": stored.status,
            "added": 0,
            "removed": 0,
            "reason": "the proposal has no actions",
        }))
        .into_response();
    }
    {
        let st = state.clone();
        let (u, r, g) = (user.clone(), rows.clone(), graphs.clone());
        match tokio::task::spawn_blocking(move || check_gates(&st, &u, &r, &g)).await {
            Ok(Ok(())) => {}
            Ok(Err(report)) => return AppError::ValidationFailed(report).into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    }

    // The write (§8.3 step 3), stamped with the commit it is recorded as.
    let base_url = state.base_url.trim_end_matches('/').to_string();
    let mut rec =
        crate::commit_log::CommitRecord::new(crate::commit_log::CommitKind::Sparql, String::new());
    let commit = format!("{base_url}/commit/{}", rec.commit_id);
    let actor = format!("{base_url}/users/{}", user.user_id);
    let update = ground_update(&rows);
    let st = state.clone();
    let gs = graphs.clone();
    let ctx = crate::store::changes::WriteContext {
        actor_iri: Some(actor.clone()),
        commit_iri: Some(commit.clone()),
        kind: Some(crate::commit_log::CommitKind::Sparql.as_str().to_string()),
    };
    let written = tokio::task::spawn_blocking(move || {
        let before = crate::ldes::capture::before(&st, &gs);
        let result = {
            let _ctx = crate::store::changes::WriteContextGuard::set(ctx);
            st.store.update_targeted_delta(&update, &gs, false)
        };
        if result.is_ok() {
            crate::ldes::capture::after(&st, before);
            crate::entailment::after_write(&st, &gs);
        }
        result
    })
    .await;
    let delta = match written {
        Ok(Ok(d)) => d,
        Ok(Err(e @ crate::store::engine::StoreError::ReadOnly(_))) => {
            return AppError::from(e).into_response()
        }
        Ok(Err(e)) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("applying the proposal failed: {e}"),
            )
                .into_response()
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let adds = rows.iter().filter(|(a, _)| *a).count();
    let (added, removed) = match &delta {
        Some((inserted, deleted)) => (inserted.len(), deleted.len()),
        None => (adds, rows.len() - adds),
    };
    {
        let st = state.clone();
        let gs = graphs.clone();
        let _ = tokio::task::spawn_blocking(move || match delta {
            Some((inserted, deleted)) => st.text_index_apply_delta(&inserted, &deleted, &gs),
            None => st.refresh_text_index_graphs(&gs),
        })
        .await;
    }
    rec.message = format!("Repair {id}: +{added} −{removed}");
    rec.actor_iri = Some(actor);
    rec.subject_iri = Some(format!("{base_url}/dataset/{dataset_id}"));
    rec.affected_graphs = graphs.clone();
    rec.added = added;
    rec.removed = removed;
    rec.metadata = Some(serde_json::json!({
        "repair": {
            "proposal": id,
            "engine": super::proposal::ENGINE,
            "base_commit": base["commit"],
            "base_sequence": base["sequence"],
            "base_epoch": base["epoch"],
            "rules": stored.report["rules"]["digest"],
            "heuristic": heuristic,
        }
    }));
    if let Err(e) = crate::commit_log::insert_commit(&state.store, &state.base_url, &rec) {
        tracing::warn!("failed to record the commit of repair proposal {id}: {e}");
    }
    {
        use crate::auth::audit::{AuditEventBuilder, AuditEventType, AuditOutcome};
        let mut b = AuditEventBuilder::new(AuditEventType::SparqlUpdate, AuditOutcome::Success)
            .details(serde_json::json!({
                "graphs": graphs,
                "message": format!("Repair {id}"),
                "proposal": id,
            }));
        b.actor_id = Some(user.user_id.clone());
        b.actor_role = Some(user.role.as_str().to_string());
        state.audit.log(b);
    }
    let status = match persist::set_status(&state, stored, "applied", None, Some(commit.clone())) {
        Ok(s) => s.status,
        Err(e) => {
            tracing::warn!("could not mark proposal {id} applied: {e}");
            "applied".to_string()
        }
    };
    Json(serde_json::json!({
        "applied": true,
        "proposal_id": id,
        "status": status,
        "commit": commit,
        "added": added,
        "removed": removed,
        "graphs": graphs,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_rows_read_back_what_the_emitter_writes() {
        let patch = "H id <urn:ots:proposal:1> .\nTX .\n# rule=<urn:r>\n\
                     D <urn:a> <urn:p> \"x\\\"y\"@en <urn:g> .\n\
                     A <urn:b> <urn:p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> <urn:g> .\nTC .\n";
        let rows = proposal_rows(patch).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].0 && rows[1].0);
        assert_eq!(graphs_of(&rows), vec!["urn:g".to_string()]);
        let update = ground_update(&rows);
        assert!(update.starts_with(
            "DELETE DATA {\n  GRAPH <urn:g> {\n    <urn:a> <urn:p> \"x\\\"y\"@en .\n"
        ));
        assert!(update.contains(" ;\nINSERT DATA {"));
        spargebra::SparqlParser::new()
            .parse_update(&update)
            .unwrap();
    }

    /// A second taker waits for the first, and the entry goes with the
    /// last holder.
    #[tokio::test]
    async fn the_patch_lock_serialises_and_leaves_no_entry_behind() {
        let first = lock_dataset("lock-test").await;
        let waiter = tokio::spawn(async {
            let _second = lock_dataset("lock-test").await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());
        drop(first);
        waiter.await.unwrap();
        assert!(PATCH_LOCKS
            .get()
            .is_none_or(|m| !m.contains_key("lock-test")));
    }

    #[test]
    fn if_match_names_the_base_commit_unless_the_query_does() {
        let mut h = HeaderMap::new();
        h.insert(header::IF_MATCH, "W/\"urn:c:1\"".parse().unwrap());
        let o = ApplyOptions::default().with_headers(&h);
        assert_eq!(o.if_base_commit.as_deref(), Some("urn:c:1"));
        assert!(o.any() && !o.validate());
        let q = ApplyOptions {
            if_base_commit: Some("urn:c:2".into()),
            ..Default::default()
        }
        .with_headers(&h);
        assert_eq!(q.if_base_commit.as_deref(), Some("urn:c:2"));
        h.insert(header::IF_MATCH, "*".parse().unwrap());
        assert!(!ApplyOptions::default().with_headers(&h).any());
    }
}
