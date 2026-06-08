//! HTTP routes implementing the SPARQL Protocol and Graph Store Protocol.
//!
//! Endpoints:
//! - GET/POST /sparql — SPARQL Query and Update
//! - GET/PUT/POST/DELETE /store — Graph Store HTTP Protocol
//! - GET / — Service Description
//! - GET /health — Health check
//!
//! # Prefix auto-resolution
//!
//! Before executing a SPARQL query or update, the route handlers scan the
//! query text for prefix labels that are used but not declared.  Any undeclared
//! prefixes are looked up via the [`PrefixRegistry`] (local cache first, then
//! prefix.cc) and the resulting `PREFIX label: <IRI>` declarations are
//! prepended automatically.  This lets clients write queries with common
//! prefixes like `foaf:`, `schema:`, or `owl:` without boilerplate.

use axum::body::Bytes;
use axum::extract::{Extension, Multipart, Path, Query, State};
use axum::http::header::{ACCEPT, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use futures::StreamExt;
use oxigraph::model::Term;
use serde::Deserialize;
use tracing::debug;

use super::content_negotiation::*;
use super::error::AppError;
use super::AppState;
use crate::auth::acl::{check_graph_permission, filter_quad_indices_by_label, QuadKey};
use crate::auth::middleware::AuthenticatedUser;
use crate::prefixes::find_undeclared_prefixes;
use crate::sparql::service_description;
use spargebra::algebra::GraphTarget;
use spargebra::term::{GraphName, GraphNamePattern, NamedNodePattern};
use spargebra::GraphUpdateOperation;
use tokio::sync::{mpsc, oneshot};

/// Namespace for the asset technical-metadata predicates this server mints during
/// upload (`…asset#pointCount`, `…asset#rowCount`, …). Single source of truth for
/// BOTH the write side (deriving metadata into the asset graph) and the read side
/// (parsing it back), so the round-trip can never drift.
///
/// PLACEHOLDER default — repoint at your own domain if you have one. This is a
/// data-format constant: metadata already written under the old namespace will not
/// be read back after you change it.
const ASSET_NS: &str = "https://opentriplestore.org/ns/asset#";

// ─── Route construction ───────────────────────────────────────────────────────

pub fn sparql_routes() -> Router<AppState> {
    Router::new()
        .route("/sparql", get(sparql_query_get))
        .route("/sparql", post(sparql_post))
}

/// Batch SPARQL UPDATE routes (auth required).
pub fn sparql_batch_routes() -> Router<AppState> {
    Router::new().route("/sparql/batch", post(sparql_batch_update))
}

pub fn graph_store_read_routes() -> Router<AppState> {
    Router::new().route("/store", get(graph_store_get))
}

pub fn graph_store_write_routes() -> Router<AppState> {
    Router::new()
        .route("/store", put(graph_store_put))
        .route("/store", post(graph_store_post))
        .route("/store", delete(graph_store_delete))
}

pub fn management_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(service_description_handler))
        .route("/health", get(health_check))
}

// ─── Query parameters ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SparqlQueryParams {
    pub query: Option<String>,
    /// Entailment regime: "rdfs", "owl2-rl", "owl2-el", "owl2-ql", "owl2-dl"
    pub entailment: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct GraphStoreParams {
    pub graph: Option<String>,
    pub default: Option<String>,
}

impl GraphStoreParams {
    fn graph_iri(&self) -> Option<&str> {
        if self.default.is_some() {
            None // default graph
        } else {
            self.graph.as_deref()
        }
    }
}

// ─── SPARQL Protocol Handlers ─────────────────────────────────────────────────

/// GET /sparql?query=...
/// SPARQL Protocol §2.1.1: query via URL parameters
async fn sparql_query_get(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<SparqlQueryParams>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let query = match params.query {
        Some(q) => q,
        None => {
            // A browser hard-refresh / deep link to the `/sparql` client route
            // arrives here with no `query` param. Serve the SPA shell so the
            // workspace renders instead of a JSON error; genuine API clients
            // (no `Accept: text/html`) still get the 400 below.
            if let Some(resp) = spa_shell_response(&headers) {
                return Ok(resp);
            }
            return Err(AppError::BadRequest(
                "Missing 'query' parameter".to_string(),
            ));
        }
    };

    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/sparql-results+json");

    execute_query(
        &state,
        user.as_deref(),
        &query,
        accept,
        params.entailment.as_deref(),
    )
    .await
}

/// POST /sparql
/// Handles three content types per SPARQL Protocol:
/// - application/sparql-query: query in body
/// - application/sparql-update: update in body (requires authentication)
/// - application/x-www-form-urlencoded: query or update in form params
async fn sparql_post(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/sparql-results+json");

    let body_str = String::from_utf8(body.to_vec())
        .map_err(|_| AppError::BadRequest("Invalid UTF-8 in body".to_string()))?;

    let commit_msg = headers
        .get("x-commit-message")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    if content_type.starts_with("application/sparql-query") {
        // Direct query in body
        execute_query(&state, user.as_deref(), &body_str, accept, None).await
    } else if content_type.starts_with("application/sparql-update") {
        // Updates require authentication
        if user.is_none() {
            return Err(AppError::Unauthorized(
                "Authentication required for SPARQL updates".to_string(),
            ));
        }
        execute_update(&state, user.as_deref(), &body_str, commit_msg).await
    } else if content_type.starts_with("application/x-www-form-urlencoded") {
        // Parse form body
        let params: Vec<(String, String)> = url::form_urlencoded::parse(body_str.as_bytes())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let query = params
            .iter()
            .find(|(k, _)| k == "query")
            .map(|(_, v)| v.as_str());
        let update = params
            .iter()
            .find(|(k, _)| k == "update")
            .map(|(_, v)| v.as_str());

        if let Some(q) = query {
            execute_query(&state, user.as_deref(), q, accept, None).await
        } else if let Some(u) = update {
            if user.is_none() {
                return Err(AppError::Unauthorized(
                    "Authentication required for SPARQL updates".to_string(),
                ));
            }
            execute_update(&state, user.as_deref(), u, commit_msg).await
        } else {
            Err(AppError::BadRequest(
                "Missing 'query' or 'update' in form body".to_string(),
            ))
        }
    } else {
        Err(AppError::UnsupportedMediaType(format!(
            "Unsupported Content-Type: {}",
            content_type
        )))
    }
}

// ─── Streaming response plumbing ──────────────────────────────────────────────

/// `std::io::Write` adapter that pumps every successful write into a Tokio mpsc
/// channel as a `Bytes` chunk. Lets `spawn_blocking` SPARQL serialisation
/// stream straight through `axum::body::Body::from_stream` without first
/// buffering the whole result in a `Vec<u8>`.
struct ChannelWriter {
    tx: mpsc::Sender<Result<Bytes, std::io::Error>>,
}

impl std::io::Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let chunk = Bytes::copy_from_slice(buf);
        // `blocking_send` is correct here: we run inside `spawn_blocking`, and
        // applying back-pressure on the runtime (rather than dropping bytes) is
        // what keeps memory bounded for huge result sets.
        self.tx.blocking_send(Ok(chunk)).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "client disconnected")
        })?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Wrap an mpsc receiver as a `futures::Stream` suitable for
/// `axum::body::Body::from_stream`.
fn receiver_stream(
    rx: mpsc::Receiver<Result<Bytes, std::io::Error>>,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
    .fuse()
}

#[cfg(test)]
mod receiver_stream_tests {
    use super::receiver_stream;
    use axum::body::Bytes;
    use futures::StreamExt;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn receiver_stream_is_safe_to_poll_after_completion() {
        let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(4);
        tx.send(Ok(Bytes::from_static(b"chunk")))
            .await
            .expect("send should succeed");
        drop(tx);

        let stream = receiver_stream(rx);
        futures::pin_mut!(stream);

        let first = stream.next().await;
        assert!(first.is_some(), "first poll should return the sent chunk");

        let done = stream.next().await;
        assert!(done.is_none(), "stream should end once sender is dropped");

        let done_again = stream.next().await;
        assert!(
            done_again.is_none(),
            "stream should remain completed on repeated polls"
        );
    }
}

// ─── Core execution helpers ───────────────────────────────────────────────────

/// Execute a SPARQL query scoped to graphs accessible by the caller.
///
/// For unauthenticated requests, only graphs in public datasets are queryable.
/// For authenticated requests, graphs from accessible datasets are included.
/// Any prefix labels found in `query` that lack a `PREFIX` declaration are
/// looked up in the [`PrefixRegistry`] and prepended before the query reaches
/// the store engine.
async fn execute_query(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    query: &str,
    accept: &str,
    entailment: Option<&str>,
) -> Result<Response, AppError> {
    debug!("Executing query, Accept: {}", accept);

    // Scope query dataset to graphs the caller can access.
    let user_id = user.map(|u| u.user_id.as_str());
    let cached_graphs = state
        .auth_db
        .get_accessible_graph_iris_cached(user_id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut accessible = cached_graphs.0.clone();
    let all_registered = &cached_graphs.1;

    // Merge in any extra graphs granted via the named-graph ACL table.
    if let Some(u) = user {
        match state
            .auth_db
            .get_graph_acl_readable_iris(&u.user_id, u.role.as_str())
        {
            Ok(acl_iris) => {
                for iri in acl_iris {
                    accessible.insert(iri);
                }
            }
            Err(e) => debug!("graph_acl lookup failed: {e}"),
        }
    } else {
        // Unauthenticated — include graphs with public graph_acl grants
        if let Ok(acl_iris) = state.auth_db.get_graph_acl_readable_iris("", "public") {
            for iri in acl_iris {
                accessible.insert(iri);
            }
        }
    }

    // We always scope the query to FROM/FROM NAMED clauses so plain `WHERE { ?s ?p ?o }`
    // queries (which match only the default graph in standard SPARQL) still see triples
    // held in named graphs — otherwise querying the main store with `?s ?p ?o` would yield
    // an empty result, since this store keeps all data in named graphs.
    //
    // Admins may read every registered graph and may legitimately name additional
    // server-owned graphs (entailment regimes, system graphs) in their own FROM clauses,
    // so their dataset is preserved and all registered graphs are exposed additively.
    // Everyone else is strictly re-scoped: any FROM / FROM NAMED clause they supplied is
    // intersected with the graphs they may read, so naming a private graph in FROM NAMED
    // cannot widen what the query can see (security boundary — see scope_query_to_authorized).
    let scoped_query = if user.map(|u| u.is_admin()).unwrap_or(false) {
        if all_registered.is_empty() {
            Some(query.to_string())
        } else {
            let mut from_clauses = String::new();
            for iri in all_registered {
                from_clauses.push_str(&format!("FROM <{}>\nFROM NAMED <{}>\n", iri, iri));
            }
            Some(inject_from_clauses(query, &from_clauses))
        }
    } else {
        Some(scope_query_to_authorized(query, &accessible))
    };

    // text:search magic property preprocessing (text-search feature)
    // Rebuild Tantivy index first if the store has been written since last sync.
    #[cfg(feature = "text-search")]
    state.sync_text_index_if_dirty();
    #[cfg(feature = "text-search")]
    let query_after_text_search: String;
    #[cfg(feature = "text-search")]
    let query_after_regex_pushdown: String;
    #[cfg(feature = "text-search")]
    let query = if let Some(ref idx) = state.text_index {
        query_after_text_search = crate::text_search::sparql_fn::preprocess_text_search(
            scoped_query.as_deref().unwrap_or(query),
            idx,
        );
        // REGEX/CONTAINS → Tantivy push-down (~100x for text-heavy queries)
        query_after_regex_pushdown =
            crate::text_search::sparql_fn::preprocess_regex_pushdown(&query_after_text_search, idx);
        &query_after_regex_pushdown as &str
    } else {
        scoped_query.as_deref().unwrap_or(query)
    };
    #[cfg(not(feature = "text-search"))]
    let query = scoped_query.as_deref().unwrap_or(query);

    // Entailment regime: inject FROM <urn:entailment:...> if requested
    let entailment_query: String;
    let query = if let Some(regime) = entailment {
        let graph_iri = match regime {
            "rdfs" => Some(crate::reasoning::common::RDFS_ENTAILMENT_GRAPH),
            "owl2-rl" => Some(crate::reasoning::common::OWL2_RL_ENTAILMENT_GRAPH),
            "owl2-el" => Some(crate::reasoning::common::OWL2_EL_ENTAILMENT_GRAPH),
            "owl2-ql" => Some(crate::reasoning::common::OWL2_QL_ENTAILMENT_GRAPH),
            _ => None,
        };
        if let Some(iri) = graph_iri {
            entailment_query =
                inject_from_clauses(query, &format!("FROM <{iri}>\nFROM NAMED <{iri}>\n"));
            &entailment_query as &str
        } else {
            query
        }
    } else {
        query
    };

    let effective_query = resolve_prefixes(state, query).await;
    let effective_query_str = effective_query.as_deref().unwrap_or(query).to_string();

    // M-1: Enforce a configurable SPARQL query timeout to prevent runaway queries.
    let timeout = std::time::Duration::from_secs(state.query_timeout_secs);

    // Run query execution + serialisation inside spawn_blocking: both are CPU-bound
    // synchronous operations that would stall the Tokio async runtime if left on it.
    // The store is Arc-backed and clones cheaply.
    //
    // Serialisation streams through a bounded mpsc channel so the response body
    // is not buffered in memory — large CONSTRUCT/DESCRIBE results no longer
    // produce multi-MB allocations before the first byte is sent.
    let store = state.store.clone();
    let accept_owned = accept.to_string();
    let (ct_tx, ct_rx) = oneshot::channel::<Result<&'static str, AppError>>();
    let (chunk_tx, chunk_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(8);

    tokio::task::spawn_blocking(move || {
        let results = match store.query(&effective_query_str) {
            Ok(r) => r,
            Err(e) => {
                // Preserve `From<StoreError> for AppError` so syntax errors
                // surface as 4xx (and don't leak internal library names).
                let _ = ct_tx.send(Err(AppError::from(e)));
                return;
            }
        };

        // Determine the response format from the result kind without consuming it.
        enum Mode {
            Tabular(ResultFormat),
            Graph(GraphFormat),
        }
        let mode = match &results {
            oxigraph::sparql::QueryResults::Solutions(_)
            | oxigraph::sparql::QueryResults::Boolean(_) => {
                Mode::Tabular(negotiate_result_format(&accept_owned))
            }
            oxigraph::sparql::QueryResults::Graph(_) => {
                Mode::Graph(negotiate_graph_format(&accept_owned))
            }
        };
        let content_type = match mode {
            Mode::Tabular(f) => f.content_type(),
            Mode::Graph(f) => f.content_type(),
        };
        // Headers go out before serialisation begins; any subsequent failure
        // surfaces as a truncated stream (no way to flip a 200 to a 500).
        if ct_tx.send(Ok(content_type)).is_err() {
            return;
        }

        let mut writer = ChannelWriter {
            tx: chunk_tx.clone(),
        };
        let result = match mode {
            Mode::Tabular(f) => serialize_results_to(results, f, &mut writer),
            Mode::Graph(f) => serialize_graph_to(results, f, &mut writer),
        };
        if let Err(msg) = result {
            let _ =
                chunk_tx.blocking_send(Err(std::io::Error::new(std::io::ErrorKind::Other, msg)));
        }
    });

    // M-1: Apply the configured query timeout to the time-to-first-byte
    // (i.e. parsing + planning + first row). Once the body is streaming, the
    // response is committed and the client controls how long it reads for.
    let content_type = tokio::time::timeout(timeout, ct_rx)
        .await
        .map_err(|_| AppError::BadRequest("Query execution timed out".to_string()))?
        .map_err(|_| AppError::Internal("Query task aborted".to_string()))??;

    let body = axum::body::Body::from_stream(receiver_stream(chunk_rx));
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .body(body)
        .unwrap())
}

/// Execute a SPARQL UPDATE, auto-resolving undeclared prefixes via prefix.cc.
///
/// H-1: Before executing, the update is parsed to extract all target named
/// graph IRIs. `require_graph_write` is called for each one so that the
/// graph-level ACL is enforced for SPARQL UPDATE the same way it is for
/// the Graph Store Protocol PUT/POST/DELETE endpoints.
async fn execute_update(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    update: &str,
    message: Option<&str>,
) -> Result<Response, AppError> {
    debug!("Executing update");

    let effective_update = resolve_prefixes(state, update).await;
    let effective_str = effective_update.as_deref().unwrap_or(update);

    // Parse with spargebra to extract target graph IRIs for ACL checking.
    let parsed = spargebra::Update::parse(effective_str, None)
        .map_err(|e| AppError::BadRequest(format!("Invalid SPARQL UPDATE: {}", e)))?;

    // M-8: Enforce API token write scope — read-only tokens may not perform SPARQL UPDATE.
    if let Some(u) = user {
        if !u.write_access && !u.is_admin() {
            return Err(AppError::Unauthorized(
                "This API token does not have write scope".to_string(),
            ));
        }
    }

    // H-1: enforce the per-graph ACL on BOTH the write targets and the WHERE/USING
    // read side, and admin-gate variable-graph / SERVICE / all-graph operations.
    let (graph_iris, requires_admin) = authorize_update(state, user, &parsed)?;

    // M-5: Use targeted graph index update (only re-count affected graphs).
    // W4-21: Wrap in a configurable timeout to abort runaway UPDATE operations.
    let effective = effective_str.to_string();
    let store = state.store.clone();
    let affected = graph_iris.clone();
    let timeout = std::time::Duration::from_secs(state.query_timeout_secs);
    tokio::time::timeout(
        timeout,
        tokio::task::spawn_blocking(move || {
            store.update_targeted(&effective, &affected, requires_admin)
        }),
    )
    .await
    .map_err(|_| AppError::BadRequest("Update execution timed out".to_string()))?
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(|e| AppError::BadRequest(e.to_string()))?;
    #[cfg(feature = "text-search")]
    state.mark_text_dirty();

    {
        use crate::auth::audit::{AuditEventBuilder, AuditEventType, AuditOutcome};
        let mut b = AuditEventBuilder::new(AuditEventType::SparqlUpdate, AuditOutcome::Success)
            .details(serde_json::json!({
                "graphs": graph_iris,
                "requires_admin": requires_admin,
                "message": message,
            }));
        if let Some(u) = user {
            b.actor_id = Some(u.user_id.clone());
            b.actor_role = Some(u.role.as_str().to_string());
        }
        state.audit.log(b);
    }

    // Record the update in the linked-data provenance trail, keyed by the graphs
    // it touched (so it surfaces on dataset pages). Best-effort.
    if !graph_iris.is_empty() {
        let mut rec = crate::commit_log::CommitRecord::new(
            crate::commit_log::CommitKind::Sparql,
            message.unwrap_or("SPARQL update"),
        );
        rec.actor_iri = user.map(|u| format!("{}/users/{}", state.base_url, u.user_id));
        rec.affected_graphs = graph_iris.clone();
        if let Err(e) = crate::commit_log::insert_commit(&state.store, &state.base_url, &rec) {
            tracing::warn!("failed to record SPARQL update commit: {e}");
        }
    }

    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Graph access a SPARQL UPDATE performs, resolved for per-graph ACL enforcement.
///
/// H-1 (hardened): the previous implementation collected only ground `NamedNode`
/// *write* targets, so an UPDATE whose graph was a variable (`GRAPH ?g { … }`)
/// produced an empty target list and bypassed the ACL loop entirely, and the
/// `WHERE`/`USING` *read* side was never checked at all. That let any writer
/// rewrite/delete every graph (`DELETE { GRAPH ?g {…} } WHERE { GRAPH ?g {…} }`)
/// or copy data out of graphs they cannot read
/// (`INSERT { GRAPH <mine> {…} } WHERE { GRAPH <victim> {…} }`). This captures
/// the full picture so both sides are authorized.
#[derive(Default)]
struct UpdateGraphAccess {
    /// Ground named graphs the update writes to.
    write_iris: std::collections::BTreeSet<String>,
    /// Ground named graphs the update reads (`GRAPH <iri>` in WHERE, or `USING`).
    read_iris: std::collections::BTreeSet<String>,
    /// CLEAR/DROP targeting ALL or NAMED graphs — affects every graph; admin only.
    requires_admin: bool,
    /// A variable graph target/source (`GRAPH ?g`) or a `SERVICE` clause: the set
    /// of affected graphs cannot be bounded statically, so the operation is
    /// restricted to admins (non-admins must name explicit graphs).
    unscoped: bool,
}

/// Recursively collect the named graphs a `WHERE` pattern reads, flagging a
/// variable `GRAPH ?g` block or a `SERVICE` call as unscoped. Default-graph leaf
/// reads (`Bgp`/`Path`/`Values` outside any `GRAPH`) are harmless here: the store
/// keeps all data in named graphs and the engine uses a non-union default graph.
fn collect_where_graph_access(p: &spargebra::algebra::GraphPattern, acc: &mut UpdateGraphAccess) {
    use spargebra::algebra::GraphPattern as GP;
    match p {
        GP::Graph { name, inner } => {
            match name {
                NamedNodePattern::NamedNode(nn) => {
                    acc.read_iris.insert(nn.as_str().to_string());
                }
                NamedNodePattern::Variable(_) => acc.unscoped = true,
            }
            collect_where_graph_access(inner, acc);
        }
        GP::Service { inner, .. } => {
            // Federated read — cannot be bounded to the caller's grants.
            acc.unscoped = true;
            collect_where_graph_access(inner, acc);
        }
        GP::Bgp { .. } | GP::Path { .. } | GP::Values { .. } => {}
        GP::Join { left, right, .. }
        | GP::Lateral { left, right, .. }
        | GP::Union { left, right, .. }
        | GP::Minus { left, right, .. }
        | GP::LeftJoin { left, right, .. } => {
            collect_where_graph_access(left, acc);
            collect_where_graph_access(right, acc);
        }
        GP::Filter { inner, .. }
        | GP::Extend { inner, .. }
        | GP::OrderBy { inner, .. }
        | GP::Project { inner, .. }
        | GP::Distinct { inner, .. }
        | GP::Reduced { inner, .. }
        | GP::Slice { inner, .. }
        | GP::Group { inner, .. } => collect_where_graph_access(inner, acc),
    }
}

/// Analyse a SPARQL UPDATE for the graphs it reads and writes so the per-graph
/// ACL can be enforced on both sides (H-1).
fn analyze_update_graph_access(update: &spargebra::Update) -> UpdateGraphAccess {
    let mut acc = UpdateGraphAccess::default();

    for op in &update.operations {
        match op {
            GraphUpdateOperation::InsertData { data } => {
                for quad in data {
                    if let GraphName::NamedNode(nn) = &quad.graph_name {
                        acc.write_iris.insert(nn.as_str().to_string());
                    }
                }
            }
            GraphUpdateOperation::DeleteData { data } => {
                for quad in data {
                    if let GraphName::NamedNode(nn) = &quad.graph_name {
                        acc.write_iris.insert(nn.as_str().to_string());
                    }
                }
            }
            GraphUpdateOperation::DeleteInsert {
                delete,
                insert,
                using,
                pattern,
            } => {
                // `USING`/`WITH` sets the default graph(s) the templates write to
                // and the WHERE reads from. Collect them as reads, and remember
                // them to resolve any `DefaultGraph`-targeted template below.
                let using_default: Vec<String> = using
                    .as_ref()
                    .map(|d| d.default.iter().map(|n| n.as_str().to_string()).collect())
                    .unwrap_or_default();
                if let Some(d) = using {
                    for nn in &d.default {
                        acc.read_iris.insert(nn.as_str().to_string());
                    }
                    if let Some(named) = &d.named {
                        for nn in named {
                            acc.read_iris.insert(nn.as_str().to_string());
                        }
                    }
                }
                for graph_name in delete
                    .iter()
                    .map(|q| &q.graph_name)
                    .chain(insert.iter().map(|q| &q.graph_name))
                {
                    match graph_name {
                        GraphNamePattern::NamedNode(nn) => {
                            acc.write_iris.insert(nn.as_str().to_string());
                        }
                        GraphNamePattern::Variable(_) => acc.unscoped = true,
                        GraphNamePattern::DefaultGraph => {
                            // Resolves to the WITH/USING default graph(s); with no
                            // USING it is the unnamed default graph (allowed, like a
                            // Graph Store write with no `graph` param).
                            for iri in &using_default {
                                acc.write_iris.insert(iri.clone());
                            }
                        }
                    }
                }
                collect_where_graph_access(pattern, &mut acc);
            }
            GraphUpdateOperation::Load { destination, .. } => {
                if let GraphName::NamedNode(nn) = destination {
                    acc.write_iris.insert(nn.as_str().to_string());
                }
            }
            GraphUpdateOperation::Clear { graph, .. }
            | GraphUpdateOperation::Drop { graph, .. } => match graph {
                GraphTarget::NamedNode(nn) => {
                    acc.write_iris.insert(nn.as_str().to_string());
                }
                GraphTarget::NamedGraphs | GraphTarget::AllGraphs => {
                    acc.requires_admin = true;
                }
                GraphTarget::DefaultGraph => {}
            },
            GraphUpdateOperation::Create { graph, .. } => {
                acc.write_iris.insert(graph.as_str().to_string());
            }
        }
    }

    acc
}

/// Compute the set of named-graph IRIs `user` may READ, combining dataset-access
/// visibility with explicit `graph_acl` read grants — the exact set the query
/// path scopes to (see [`execute_query`]). Used to authorize the read
/// (`WHERE`/`USING`) side of SPARQL UPDATEs so a writer cannot copy data out of
/// graphs they cannot read.
fn accessible_read_graphs(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
) -> Result<std::collections::HashSet<String>, AppError> {
    let user_id = user.map(|u| u.user_id.as_str());
    let cached = state
        .auth_db
        .get_accessible_graph_iris_cached(user_id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut accessible = cached.0.clone();
    if let Some(u) = user {
        if let Ok(acl_iris) = state
            .auth_db
            .get_graph_acl_readable_iris(&u.user_id, u.role.as_str())
        {
            accessible.extend(acl_iris);
        }
    } else if let Ok(acl_iris) = state.auth_db.get_graph_acl_readable_iris("", "public") {
        accessible.extend(acl_iris);
    }
    Ok(accessible)
}

/// Enforce the per-graph ACL for both the write and read side of a parsed
/// SPARQL UPDATE (H-1). Returns the ground write-target IRIs (for index recount,
/// audit, and provenance) and whether the update is an all-graph operation.
fn authorize_update(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    parsed: &spargebra::Update,
) -> Result<(Vec<String>, bool), AppError> {
    let access = analyze_update_graph_access(parsed);
    let is_admin = user.map(|u| u.is_admin()).unwrap_or(false);

    // All-graph operations (CLEAR ALL / DROP ALL / …NAMED) require admin.
    if access.requires_admin && !is_admin {
        return Err(AppError::Unauthorized(
            "Clearing or dropping all graphs requires admin privileges".to_string(),
        ));
    }

    // H-1: a variable graph target/source (`GRAPH ?g`) or a SERVICE clause cannot
    // be bounded to the caller's grants, so it is admin-only. This closes the
    // `DELETE { GRAPH ?g {…} } WHERE { GRAPH ?g {…} }` whole-store rewrite.
    if access.unscoped && !is_admin {
        return Err(AppError::Forbidden(
            "SPARQL UPDATE with a variable graph target/source or a SERVICE clause requires \
             admin privileges; name the explicit graphs you have access to instead"
                .to_string(),
        ));
    }

    // Write permission for every ground target graph.
    for iri in &access.write_iris {
        require_graph_write(state, user, Some(iri.as_str()))?;
    }

    // H-1: read permission for every ground graph the WHERE/USING reads. Prevents
    // exfiltration via `INSERT { GRAPH <mine> {…} } WHERE { GRAPH <victim> {…} }`.
    if !is_admin && !access.read_iris.is_empty() {
        let readable = accessible_read_graphs(state, user)?;
        for iri in &access.read_iris {
            if !readable.contains(iri.as_str()) {
                return Err(AppError::Forbidden(format!(
                    "Read access denied for graph <{iri}> referenced in the UPDATE WHERE/USING clause"
                )));
            }
        }
    }

    Ok((
        access.write_iris.into_iter().collect(),
        access.requires_admin,
    ))
}

// ─── Batch SPARQL UPDATE ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BatchUpdateRequest {
    updates: Vec<String>,
}

/// POST /sparql/batch — execute multiple SPARQL UPDATE statements in one batch.
///
/// Amortises write-lock acquisition and SPARQL parse overhead across all
/// statements (3-7x faster than individual updates). Max 1000 statements.
async fn sparql_batch_update(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<BatchUpdateRequest>,
) -> Result<impl IntoResponse, AppError> {
    if body.updates.len() > 1000 {
        return Err(AppError::BadRequest(
            "Batch size exceeds maximum of 1000 statements".to_string(),
        ));
    }

    // Resolve prefixes for each statement
    let mut resolved: Vec<String> = Vec::with_capacity(body.updates.len());
    for stmt in &body.updates {
        let effective = resolve_prefixes(&state, stmt).await;
        resolved.push(effective.unwrap_or_else(|| stmt.clone()));
    }

    // H-1: Enforce graph-level write ACL for every statement before executing any.
    // M-8: Also enforce API token write scope.
    if !user.write_access && !user.is_admin() {
        return Err(AppError::Unauthorized(
            "This API token does not have write scope".to_string(),
        ));
    }
    for stmt in &resolved {
        let parsed = spargebra::Update::parse(stmt.as_str(), None)
            .map_err(|e| AppError::BadRequest(format!("Invalid SPARQL UPDATE: {}", e)))?;
        // H-1: per-graph read+write ACL, admin-gate variable-graph/SERVICE/all-graph ops.
        authorize_update(&state, Some(&user), &parsed)?;
    }

    let results = state.store.batch_update(&resolved)?;

    // Build per-statement status
    let statuses: Vec<serde_json::Value> = results
        .iter()
        .enumerate()
        .map(|(i, r)| match r {
            Ok(()) => serde_json::json!({ "index": i, "status": "ok" }),
            Err(e) => serde_json::json!({ "index": i, "status": "error", "error": e }),
        })
        .collect();

    let all_ok = results.iter().all(|r| r.is_ok());

    if all_ok {
        Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ok",
                "count": results.len(),
            })),
        )
            .into_response())
    } else {
        Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "partial",
                "count": results.len(),
                "results": statuses,
            })),
        )
            .into_response())
    }
}

/// Scan `sparql` for undeclared prefix labels, look them up in the registry,
/// and return a new string with the resolved `PREFIX` declarations prepended.
///
/// Returns `None` if no undeclared prefixes are found (avoids an allocation).
pub(crate) async fn resolve_prefixes(state: &AppState, sparql: &str) -> Option<String> {
    let undeclared = find_undeclared_prefixes(sparql);
    if undeclared.is_empty() {
        return None;
    }

    let mut prefix_block = String::new();
    for label in &undeclared {
        if let Some(iri) = state.prefix_registry.lookup_prefix(label).await {
            debug!("Auto-resolved prefix '{}' → <{}>", label, iri);
            prefix_block.push_str(&format!("PREFIX {}: <{}>\n", label, iri));
        }
    }

    if prefix_block.is_empty() {
        None
    } else {
        Some(format!("{}{}", prefix_block, sparql))
    }
}

// ─── Graph Store Protocol Handlers ───────────────────────────────────────────

/// GET /store?graph=... or /store?default
async fn graph_store_get(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<GraphStoreParams>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");

    let format = negotiate_graph_format(accept);

    // Graph-level access control: check visibility before serving graph data.
    if let Some(iri) = params.graph_iri() {
        let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);
        if !is_admin {
            let user_id = user.as_deref().map(|u| u.user_id.as_str());
            let allowed = check_graph_read_access(&state, user_id, iri)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            if !allowed {
                return Err(AppError::Unauthorized(
                    "Access denied to this graph".to_string(),
                ));
            }
        }
    }

    // Triple-level security label filtering: when the target graph has labels,
    // we have to load+filter+re-serialize, which fundamentally needs the bytes
    // in memory. Otherwise we can stream the dump directly through axum.
    let needs_label_filter = match params.graph_iri() {
        Some(iri) => state
            .auth_db
            .has_triple_security_labels(&[iri][..])
            .unwrap_or(false),
        None => false,
    };

    if needs_label_filter {
        let iri = params.graph_iri().expect("checked above");
        let bytes = state
            .store
            .graph_store_get(Some(iri), format.to_rdf_format())?;
        let filtered = apply_triple_label_filter(user.as_deref(), bytes, iri, format, &state)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok((
            StatusCode::OK,
            [(CONTENT_TYPE, format.content_type())],
            filtered,
        )
            .into_response());
    }

    // Stream the dump straight through the response body so multi-MB graphs
    // don't get buffered in a `Vec<u8>` before the first byte is sent.
    let store = state.store.clone();
    let graph_iri = params.graph_iri().map(|s| s.to_string());
    let rdf_format = format.to_rdf_format();
    let (chunk_tx, chunk_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let (start_tx, start_rx) = oneshot::channel::<Result<(), AppError>>();

    tokio::task::spawn_blocking(move || {
        let mut writer = ChannelWriter {
            tx: chunk_tx.clone(),
        };
        // Signal "ok to send headers" before producing data so the caller
        // can surface an error as a real 5xx if the dump cannot be initiated.
        let _ = start_tx.send(Ok(()));
        if let Err(e) = store.dump_to_writer(&mut writer, rdf_format, graph_iri.as_deref()) {
            let _ = chunk_tx.blocking_send(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                e.to_string(),
            )));
        }
    });

    start_rx
        .await
        .map_err(|_| AppError::Internal("Dump task aborted".to_string()))??;

    let body = axum::body::Body::from_stream(receiver_stream(chunk_rx));
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, format.content_type())
        .body(body)
        .unwrap())
}

/// Parse `bytes` as NTriples/Turtle in `iri`, filter triples whose security
/// labels the caller cannot read, and re-serialize in the same format.
fn apply_triple_label_filter(
    user: Option<&AuthenticatedUser>,
    bytes: Vec<u8>,
    graph_iri: &str,
    format: super::content_negotiation::GraphFormat,
    state: &AppState,
) -> anyhow::Result<Vec<u8>> {
    // Admins bypass filtering
    if user.map(|u| u.is_admin()).unwrap_or(false) {
        return Ok(bytes);
    }

    let data_str = String::from_utf8(bytes)?;

    // Parse into a temp store
    let temp = crate::store::TripleStore::in_memory()?;
    let rdf_fmt = format.to_rdf_format();
    temp.load_str(&data_str, rdf_fmt, Some(graph_iri))?;

    // Dump as NTriples for processing
    let nt_bytes = temp.dump(oxigraph::io::RdfFormat::NTriples, Some(graph_iri))?;
    let nt_str = String::from_utf8(nt_bytes)?;

    // Parse triples and build QuadKeys
    let quad_keys: Vec<QuadKey> = nt_str
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let trimmed = line.trim_end_matches('.').trim();
            let parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
            if parts.len() == 3 {
                Some(QuadKey {
                    subject: parts[0].trim().to_string(),
                    predicate: parts[1].trim().to_string(),
                    object: parts[2].trim().to_string(),
                    graph: graph_iri.to_string(),
                })
            } else {
                None
            }
        })
        .collect();

    let allowed_indices = filter_quad_indices_by_label(user, &quad_keys, &state.auth_db);

    // Build filtered NTriples string
    let filtered_lines: Vec<&str> = nt_str
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .enumerate()
        .filter_map(|(i, line)| {
            if allowed_indices.contains(&i) {
                Some(line)
            } else {
                None
            }
        })
        .collect();

    let filtered_nt = filtered_lines.join("\n");

    // Load filtered triples into another temp store and dump in original format
    let out_store = crate::store::TripleStore::in_memory()?;
    if !filtered_nt.is_empty() {
        out_store.load_str(
            &filtered_nt,
            oxigraph::io::RdfFormat::NTriples,
            Some(graph_iri),
        )?;
    }
    let result = out_store.dump(rdf_fmt, Some(graph_iri))?;
    Ok(result)
}

/// If the target graph belongs to a dataset with `shacl_on_write` enabled,
/// load the incoming data into an in-memory store and validate it against
/// the dataset's shapes graph. Returns `Err(AppError::ValidationFailed)` if
/// validation fails. Also enforces SHACL Studio write-gates (pipelines +
/// validation-layer bindings). Shared with the validate-and-commit path so a
/// commit cannot bypass the dataset's effective shapes.
pub(crate) fn validate_on_write(
    state: &AppState,
    graph_iri: Option<&str>,
    data: &str,
    format: oxigraph::io::RdfFormat,
) -> Result<(), AppError> {
    let iri = match graph_iri {
        Some(iri) => iri,
        None => return Ok(()), // default graph — no dataset association
    };

    // SHACL Studio write-gating: consult any pipelines with `gate_writes=true`
    // whose scope covers this graph. Rejection produces 422 + report via
    // `AppError::ValidationFailed`. Runs before — and independently of — the
    // legacy per-dataset `shacl_on_write` gate below, so both can coexist
    // during the transition.
    {
        let studio = crate::shacl_studio::store::ShaclStudioStore::new(state.auth_db.pool());
        if let Err(report) = crate::shacl_studio::gate::check_write_gates(
            &state.store,
            &state.auth_db,
            &studio,
            &state.base_url,
            iri,
            data,
            format,
        ) {
            return Err(AppError::ValidationFailed(report));
        }
    }

    let dataset = match state.auth_db.find_dataset_by_graph_iri(iri) {
        Ok(Some(ds)) => ds,
        _ => return Ok(()), // no owning dataset found — skip validation
    };

    if !dataset.shacl_on_write {
        return Ok(());
    }

    let shapes_graph_iri = match &dataset.shapes_graph_iri {
        Some(iri) if !iri.is_empty() => iri.clone(),
        _ => {
            // Fallback: ontology version fallback no longer supported
            return Ok(()); // no shapes graph configured
        }
    };

    // Load incoming data into a temporary in-memory store for validation
    let temp = crate::store::TripleStore::in_memory()
        .map_err(|e| AppError::Internal(format!("Failed to create temp store: {e}")))?;
    temp.load_str(data, format, graph_iri)
        .map_err(|e| AppError::BadRequest(format!("Failed to parse incoming data: {e}")))?;

    // Also load the shapes into the temp store from the main store
    let shapes_data = state
        .store
        .dump(oxigraph::io::RdfFormat::Turtle, Some(&shapes_graph_iri))
        .map_err(|e| AppError::Internal(format!("Failed to load shapes graph: {e}")))?;
    let shapes_turtle = String::from_utf8(shapes_data)
        .map_err(|_| AppError::Internal("Shapes graph is not valid UTF-8".to_string()))?;
    temp.load_str(
        &shapes_turtle,
        oxigraph::io::RdfFormat::Turtle,
        Some(&shapes_graph_iri),
    )
    .map_err(|e| AppError::Internal(format!("Failed to load shapes into temp store: {e}")))?;

    let data_graphs = vec![iri.to_string()];
    let report = crate::shacl::validate(&temp, &shapes_graph_iri, &data_graphs)
        .map_err(|e| AppError::Internal(format!("SHACL validation error: {e}")))?;

    // Continuous mode (Phase 5): record a report for this validate-on-write so it
    // shares the on-demand report history. Best-effort — a storage hiccup (or a
    // minimal report graph) must never block or fail a legitimate write. The
    // report body is data-free so there is no Turtle-escaping risk here.
    let report_ttl = format!(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n[] a sh:ValidationReport ; sh:conforms {} . # {} result(s)\n",
        report.conforms, report.results_count
    );
    let _ = crate::dataset_versions::reports::persist_report(
        state,
        &dataset.id,
        None,
        report.conforms,
        &report_ttl,
        Some(iri),
        Some(&shapes_graph_iri),
        "on-write",
        None,
    );

    if !report.conforms {
        return Err(AppError::ValidationFailed(report));
    }

    Ok(())
}

/// Check graph-level write permission for a caller.
/// Admins always pass; non-admins must have an explicit write/admin grant
/// in `graph_acl` (dataset-visibility grants read-only access to SPARQL
/// queries — explicit write grants are required for Graph Store writes).
fn require_graph_write(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    graph_iri: Option<&str>,
) -> Result<(), AppError> {
    // M-8: a read-only API token may never write, even to the default graph or a
    // graph it holds a stale grant on. SPARQL UPDATE enforces this separately too,
    // but centralising it here also covers the Graph Store Protocol PUT/POST/DELETE
    // handlers, which previously checked only the graph grant and not the scope.
    if let Some(u) = user {
        if !u.write_access && !u.is_admin() {
            return Err(AppError::Unauthorized(
                "This API token does not have write scope".to_string(),
            ));
        }
    }

    let iri = match graph_iri {
        Some(i) => i,
        None => return Ok(()), // default graph — handled by require_auth layer
    };

    // Admins bypass graph ACL
    if user.map(|u| u.is_admin()).unwrap_or(false) {
        return Ok(());
    }

    if check_graph_permission(user, iri, "write", &state.auth_db) {
        Ok(())
    } else {
        Err(AppError::Unauthorized(format!(
            "Write access denied for graph <{iri}>"
        )))
    }
}

/// PUT /store?graph=... — Replace graph contents
async fn graph_store_put(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<GraphStoreParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    require_graph_write(&state, user.as_deref(), params.graph_iri())?;

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");

    let format = parse_rdf_content_type(content_type)
        .ok_or_else(|| AppError::UnsupportedMediaType(content_type.to_string()))?;

    let data = String::from_utf8(body.to_vec())
        .map_err(|_| AppError::BadRequest("Invalid UTF-8".to_string()))?;

    validate_on_write(&state, params.graph_iri(), &data, format)?;

    state
        .store
        .graph_store_put(params.graph_iri(), &data, format)?;
    #[cfg(feature = "text-search")]
    state.mark_text_dirty();
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// POST /store?graph=... — Merge into graph
async fn graph_store_post(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<GraphStoreParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    require_graph_write(&state, user.as_deref(), params.graph_iri())?;

    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");

    let format = parse_rdf_content_type(content_type)
        .ok_or_else(|| AppError::UnsupportedMediaType(content_type.to_string()))?;

    let data = String::from_utf8(body.to_vec())
        .map_err(|_| AppError::BadRequest("Invalid UTF-8".to_string()))?;

    validate_on_write(&state, params.graph_iri(), &data, format)?;

    state
        .store
        .graph_store_post(params.graph_iri(), &data, format)?;
    #[cfg(feature = "text-search")]
    state.mark_text_dirty();
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// DELETE /store?graph=... — Remove a graph
async fn graph_store_delete(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<GraphStoreParams>,
) -> Result<Response, AppError> {
    require_graph_write(&state, user.as_deref(), params.graph_iri())?;
    state.store.graph_store_delete(params.graph_iri())?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

// ─── Management endpoints ─────────────────────────────────────────────────────

/// GET / — Service Description (Turtle), filtered to caller-accessible graphs
/// True when the client prefers HTML (a browser) over RDF — used so the root route
/// serves the web UI to browsers while RDF/SPARQL clients still get the service
/// description.
fn prefers_html(headers: &HeaderMap) -> bool {
    headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|a| a.contains("text/html"))
        .unwrap_or(false)
}

/// Mirrors the server's frontend gate (`SERVE_FRONTEND`, default on) so the root
/// route only serves the SPA shell when the web UI is actually being served.
fn serve_frontend_enabled() -> bool {
    !matches!(
        std::env::var("SERVE_FRONTEND").ok().as_deref(),
        Some("false") | Some("0") | Some("no")
    )
}

/// Returns the SPA shell (`index.html`) as a 200 response when the web UI is
/// being served and the caller is a browser (`Accept: text/html`).
///
/// A few API routes share their path with a client-side route in the
/// History-mode SPA (`/` → Home, `/sparql` → the SPARQL workspace). Those API
/// routes are registered explicitly, so they shadow the `index.html` fallback
/// configured in `mod.rs`: a hard refresh or deep link to e.g. `/sparql` reaches
/// the API handler, which would otherwise answer a browser with a JSON error
/// ("Missing 'query' parameter") instead of the page. Handlers call this first
/// so a browser navigation renders the UI, while genuine API/RDF clients (which
/// do not send `Accept: text/html`) fall through to the API behaviour.
fn spa_shell_response(headers: &HeaderMap) -> Option<Response> {
    if serve_frontend_enabled() && prefers_html(headers) {
        if let Ok(html) = std::fs::read_to_string("frontend/dist/index.html") {
            return Some(axum::response::Html(html).into_response());
        }
    }
    None
}

async fn service_description_handler(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // Content negotiation: a browser (Accept: text/html) gets the web UI; RDF/SPARQL
    // clients get the service description. The explicit `/` route shadows the SPA
    // fallback, so without this a browser at the root only ever sees Turtle.
    if let Some(resp) = spa_shell_response(&headers) {
        return Ok(resp);
    }

    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);

    // Collect accessible graph IRIs scoped to the caller's permissions.
    let accessible_graph_iris: Vec<String> = if is_admin {
        state
            .store
            .named_graphs()
            .unwrap_or_default()
            .into_iter()
            .map(|g| g.as_str().to_string())
            .collect()
    } else {
        let cached_graphs = state
            .auth_db
            .get_accessible_graph_iris_cached(user_id)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        cached_graphs.0.iter().cloned().collect()
    };

    // Default-graph triple count. Hidden (0) for anonymous/non-admin callers so the
    // unauthenticated service description never reveals default-graph size.
    let default_graph_count = if is_admin {
        state.store.count_graph(None).unwrap_or(0)
    } else {
        0
    };

    // Pair each accessible named graph with its own triple count (void:triples).
    let named_graph_counts: Vec<(&str, usize)> = accessible_graph_iris
        .iter()
        .map(|iri| {
            let count = state.store.count_graph(Some(iri.as_str())).unwrap_or(0);
            (iri.as_str(), count)
        })
        .collect();
    // Registry datasets the caller can access, each with its accessible graphs.
    // Private graphs are filtered out for non-admins so the description can't leak
    // them (datasets themselves are already scoped by list_accessible_datasets).
    let accessible_set: std::collections::HashSet<&str> =
        accessible_graph_iris.iter().map(|s| s.as_str()).collect();
    let base = state.base_url.trim_end_matches('/');
    let dataset_descs: Vec<service_description::DatasetDesc> = state
        .auth_db
        .list_accessible_datasets(user_id)
        .unwrap_or_default()
        .into_iter()
        .map(|d| {
            let graphs = state
                .auth_db
                .list_dataset_graphs(&d.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|g| is_admin || accessible_set.contains(g.as_str()))
                .collect();
            service_description::DatasetDesc {
                iri: format!("{base}/dataset/{}", d.id),
                name: d.name,
                description: d.description,
                public: matches!(d.visibility, crate::auth::models::Visibility::Public),
                graphs,
            }
        })
        .collect();

    let desc =
        service_description::generate(default_graph_count, &named_graph_counts, &dataset_descs);

    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], desc).into_response())
}

/// GET /health — detailed subsystem probe
async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    // Triplestore
    let (store_ok, store_triples, store_graphs) =
        match (state.store.len(), state.store.named_graphs()) {
            (Ok(n), Ok(g)) => (true, Some(n as u64), Some(g.len() as u64)),
            _ => (false, None, None),
        };

    // Auth / SQLite DB — lightweight read
    let db_ok = state.auth_db.count_users().is_ok();

    // Object store (S3 / local) — only "configured" counts as a health signal
    let object_store_configured = state.object_store.is_configured();

    // Backup subsystem
    let backup_enabled = state.backup.is_some();

    // Overall: unhealthy only if core services are down
    let overall = if store_ok && db_ok { "ok" } else { "degraded" };
    let http_status = if overall == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = serde_json::json!({
        "status": overall,
        "version": env!("CARGO_PKG_VERSION"),
        "services": {
            "triplestore": {
                "ok": store_ok,
                "triples": store_triples,
                "graphs": store_graphs,
            },
            "database": {
                "ok": db_ok,
            },
            "object_storage": {
                "ok": object_store_configured,
                "configured": object_store_configured,
            },
            "backup": {
                "ok": backup_enabled,
                "enabled": backup_enabled,
            },
        }
    });

    (
        http_status,
        [(CONTENT_TYPE, "application/json")],
        serde_json::to_string_pretty(&body).unwrap(),
    )
}

/// Check whether a user may read a named graph via the Graph Store Protocol.
///
/// Rules:
/// - `urn:system:*` graphs are admin-only (internal system graphs).
/// - All other graphs are checked against dataset-graph access control.
fn check_graph_read_access(
    state: &AppState,
    user_id: Option<&str>,
    iri: &str,
) -> anyhow::Result<bool> {
    // Block all system graphs for non-admins.
    if iri.starts_with("urn:system:") {
        return Ok(false);
    }

    // Dataset graphs: check against accessible graph IRIs.
    let cached_graphs = state.auth_db.get_accessible_graph_iris_cached(user_id)?;
    Ok(cached_graphs.0.contains(iri))
}

// ─── Triple Browsing API ──────────────────────────────────────────────────────

/// Validate a user-supplied IRI string to prevent SPARQL injection.
/// Rejects any string containing characters that could break out of an IRI literal.
fn validate_iri(iri: &str) -> Result<(), AppError> {
    if iri.contains('>')
        || iri.contains('<')
        || iri.contains('"')
        || iri.contains('`')
        || iri.contains('^')
        || iri.contains('\n')
        || iri.contains('\r')
    {
        return Err(AppError::BadRequest(
            "Invalid IRI: contains disallowed characters".to_string(),
        ));
    }
    // Reject relative / non-absolute strings (e.g. "bim") — they would produce invalid
    // SPARQL like FILTER(?s = <bim>) which the query engine rejects with a syntax error.
    if !iri.contains(':') {
        return Err(AppError::BadRequest(
            "IRI must be absolute (e.g. http://example.org/resource)".to_string(),
        ));
    }
    Ok(())
}

/// Validate a plain literal value used in a SPARQL string filter to prevent injection.
/// Rejects characters that cannot be safely embedded in a SPARQL double-quoted string.
fn validate_literal_value(val: &str) -> Result<(), AppError> {
    if val.contains('\\') || val.contains('\n') || val.contains('\r') || val.contains('"') {
        return Err(AppError::BadRequest(
            "Object filter value contains disallowed characters".to_string(),
        ));
    }
    Ok(())
}

/// Validate a substring used in a CONTAINS()-style filter. Stricter than
/// `validate_literal_value` — also forbids `<`, `>` and backticks so we never
/// have to worry about a clever payload escaping the surrounding quotes.
fn validate_substring(val: &str) -> Result<(), AppError> {
    if val.is_empty() {
        return Err(AppError::BadRequest(
            "Substring filter is empty".to_string(),
        ));
    }
    if val.len() > 200 {
        return Err(AppError::BadRequest(
            "Substring filter is too long".to_string(),
        ));
    }
    if val.contains('\\')
        || val.contains('\n')
        || val.contains('\r')
        || val.contains('"')
        || val.contains('<')
        || val.contains('>')
        || val.contains('`')
    {
        return Err(AppError::BadRequest(
            "Substring filter contains disallowed characters".to_string(),
        ));
    }
    Ok(())
}

/// One advanced-filter chip from the triple browser. The browser sends an array
/// of these as a JSON string in the `filters` query param. Chips on the same
/// `field` are OR-ed together; different fields are AND-ed (and AND-ed with the
/// legacy single-field params and `q`).
#[derive(Debug, Deserialize)]
struct BrowseFilterChip {
    /// `subject` | `predicate` | `object` | `graph` | `vocabulary`
    field: String,
    value: String,
    /// `exact` (full IRI/literal match, picked from autosuggest), `contains`
    /// (case-insensitive substring — the default for free-typed values), or
    /// `regex` (case-insensitive SPARQL REGEX over the term's string form).
    #[serde(default)]
    mode: Option<String>,
    /// When true the chip is negated: rows that MATCH the clause are excluded
    /// (`FILTER(!(…))`). Negated chips AND together — each is an independent
    /// exclusion — unlike positive chips on the same field, which OR together.
    #[serde(default)]
    neg: Option<bool>,
}

/// Validate a regex pattern before embedding it in a SPARQL string literal.
/// Backslashes and quotes are escaped by the caller; here we only reject control
/// characters and over-long patterns. An invalid *regex* still parses safely —
/// Oxigraph rejects it at query time, surfaced as a normal error.
fn validate_regex(val: &str) -> Result<(), AppError> {
    if val.is_empty() {
        return Err(AppError::BadRequest("Regex filter is empty".to_string()));
    }
    if val.len() > 200 {
        return Err(AppError::BadRequest("Regex filter is too long".to_string()));
    }
    if val.contains('\n') || val.contains('\r') {
        return Err(AppError::BadRequest(
            "Regex filter contains disallowed characters".to_string(),
        ));
    }
    Ok(())
}

/// Build a single SPARQL boolean expression for one chip, bound to `var`
/// (`s`/`p`/`o`/`g`). Every value is validated before interpolation so the
/// same injection guards as the legacy single-field filters apply.
fn chip_clause(var: &str, value: &str, mode: &str, is_object: bool) -> Result<String, AppError> {
    match mode {
        "exact" => {
            let is_iri = is_object
                && (value.starts_with("http://")
                    || value.starts_with("https://")
                    || value.starts_with("urn:"));
            if is_object && !is_iri {
                validate_literal_value(value)?;
                Ok(format!("str(?{var}) = \"{value}\""))
            } else {
                validate_iri(value)?;
                Ok(format!("?{var} = <{value}>"))
            }
        }
        "regex" => {
            validate_regex(value)?;
            // Escape for a SPARQL double-quoted string: `\` so `\d` survives as a
            // regex escape (not an invalid SPARQL string escape), and `"` so the
            // pattern can't break out of the literal.
            let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
            Ok(format!("REGEX(STR(?{var}), \"{escaped}\", \"i\")"))
        }
        _ => {
            validate_substring(value)?;
            Ok(format!("CONTAINS(LCASE(STR(?{var})), LCASE(\"{value}\"))"))
        }
    }
}

/// Build a clause for the `vocabulary` filter field: a namespace (`value`) is
/// considered present when it appears in the subject, predicate OR object of a
/// triple. `exact` means the term is *in* that namespace (STRSTARTS); `contains`
/// is a case-insensitive substring; `regex` applies the pattern to each term.
fn vocab_clause(value: &str, mode: &str) -> Result<String, AppError> {
    let parts = match mode {
        "exact" => {
            validate_substring(value)?;
            ["s", "p", "o"]
                .iter()
                .map(|v| format!("STRSTARTS(STR(?{v}), \"{value}\")"))
                .collect::<Vec<_>>()
        }
        "regex" => {
            validate_regex(value)?;
            let e = value.replace('\\', "\\\\").replace('"', "\\\"");
            ["s", "p", "o"]
                .iter()
                .map(|v| format!("REGEX(STR(?{v}), \"{e}\", \"i\")"))
                .collect::<Vec<_>>()
        }
        _ => {
            validate_substring(value)?;
            ["s", "p", "o"]
                .iter()
                .map(|v| format!("CONTAINS(LCASE(STR(?{v})), LCASE(\"{value}\"))"))
                .collect::<Vec<_>>()
        }
    };
    Ok(format!("({})", parts.join(" || ")))
}

/// One token of the free-text search mini-language.
#[derive(Clone, PartialEq)]
enum QTok {
    And,
    Or,
    Xor,
    Not,
    LParen,
    RParen,
    Term(String),
}

/// Tokenise a `q` search string. Uppercase `AND`/`OR`/`XOR`/`NOT` are operators;
/// parentheses group; `"…"` is a phrase (kept whole, spaces allowed); any other
/// run of non-space, non-paren characters is a term.
fn tokenize_q(q: &str) -> Vec<QTok> {
    let chars: Vec<char> = q.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            toks.push(QTok::LParen);
            i += 1;
            continue;
        }
        if c == ')' {
            toks.push(QTok::RParen);
            i += 1;
            continue;
        }
        if c == '"' {
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            toks.push(QTok::Term(chars[start..i].iter().collect()));
            if i < chars.len() {
                i += 1;
            } // skip closing quote
            continue;
        }
        let start = i;
        while i < chars.len()
            && !chars[i].is_whitespace()
            && chars[i] != '('
            && chars[i] != ')'
            && chars[i] != '"'
        {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        toks.push(match word.as_str() {
            "AND" => QTok::And,
            "OR" => QTok::Or,
            "XOR" => QTok::Xor,
            "NOT" => QTok::Not,
            _ => QTok::Term(word),
        });
    }
    toks
}

/// A leaf term matches if the substring appears in ANY column (s/p/o/g),
/// case-insensitively. The term is assumed pre-validated by `build_q_filter`.
fn q_leaf(term: &str) -> String {
    let n = format!("LCASE(\"{}\")", term);
    format!(
        "(CONTAINS(LCASE(STR(?s)), {n}) || CONTAINS(LCASE(STR(?p)), {n}) || CONTAINS(LCASE(STR(?o)), {n}) || CONTAINS(LCASE(STR(?g)), {n}))"
    )
}

/// Recursive-descent parser for the search mini-language. Precedence (lowest→
/// highest): OR, XOR, AND (adjacent terms imply AND), NOT, atom/paren.
struct QParser {
    toks: Vec<QTok>,
    pos: usize,
}
impl QParser {
    fn peek(&self) -> Option<QTok> {
        self.toks.get(self.pos).cloned()
    }
    fn parse_or(&mut self) -> Result<String, ()> {
        let mut left = self.parse_xor()?;
        while self.peek() == Some(QTok::Or) {
            self.pos += 1;
            let r = self.parse_xor()?;
            left = format!("({left} || {r})");
        }
        Ok(left)
    }
    fn parse_xor(&mut self) -> Result<String, ()> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(QTok::Xor) {
            self.pos += 1;
            let r = self.parse_and()?;
            left = format!("(({left} || {r}) && !({left} && {r}))");
        }
        Ok(left)
    }
    fn parse_and(&mut self) -> Result<String, ()> {
        let mut left = self.parse_not()?;
        loop {
            match self.peek() {
                Some(QTok::And) => {
                    self.pos += 1;
                    let r = self.parse_not()?;
                    left = format!("({left} && {r})");
                }
                // Implicit AND when an atom (term / NOT / "(") follows with no operator.
                Some(QTok::Term(_)) | Some(QTok::Not) | Some(QTok::LParen) => {
                    let r = self.parse_not()?;
                    left = format!("({left} && {r})");
                }
                _ => break,
            }
        }
        Ok(left)
    }
    fn parse_not(&mut self) -> Result<String, ()> {
        if self.peek() == Some(QTok::Not) {
            self.pos += 1;
            let a = self.parse_not()?;
            Ok(format!("(!{a})"))
        } else {
            self.parse_atom()
        }
    }
    fn parse_atom(&mut self) -> Result<String, ()> {
        match self.peek() {
            Some(QTok::LParen) => {
                self.pos += 1;
                let e = self.parse_or()?;
                if self.peek() != Some(QTok::RParen) {
                    return Err(());
                }
                self.pos += 1;
                Ok(e)
            }
            Some(QTok::Term(t)) => {
                self.pos += 1;
                Ok(q_leaf(&t))
            }
            _ => Err(()),
        }
    }
}

/// Turn a `q` search string into a SPARQL boolean expression over s/p/o/g.
/// Supports AND/OR/XOR/NOT, parentheses, and quoted phrases; adjacent terms
/// imply AND. Returns `None` for an empty query. Every term is validated as a
/// substring so nothing can break out of the generated SPARQL.
fn build_q_filter(q: &str) -> Result<Option<String>, AppError> {
    let q = q.trim();
    if q.is_empty() {
        return Ok(None);
    }
    let toks = tokenize_q(q);
    if toks.is_empty() {
        return Ok(None);
    }
    for t in &toks {
        if let QTok::Term(s) = t {
            validate_substring(s)?;
        }
    }
    let mut p = QParser { toks, pos: 0 };
    match p.parse_or() {
        Ok(expr) if p.pos == p.toks.len() => Ok(Some(expr)),
        _ => Err(AppError::BadRequest(
            "Invalid search syntax — check AND/OR/XOR/NOT operators and matching parentheses"
                .to_string(),
        )),
    }
}

/// Resolved named-graph scope for a browse/facets request.
enum ScopeGraphs {
    /// Admin with no dataset/org scope — query every graph (`GRAPH ?g`).
    All,
    /// Explicit, ACL-checked set of graph IRIs (dataset/org/accessible/single).
    Set(Vec<String>),
    /// Caller has no graphs in scope — callers should short-circuit to empty.
    Empty,
}

/// Resolve the named-graph set a browse/facets request should run over, applying
/// the same precedence and ACL rules as `browse_triples`: an explicit single
/// `graph`, else dataset/org scope (honouring version pins), else all graphs for
/// admins or the accessible set for everyone else.
fn resolve_scope_graphs(
    state: &AppState,
    params: &BrowseTripleParams,
    user_id: Option<&str>,
    is_admin: bool,
) -> Result<ScopeGraphs, AppError> {
    let versions_map = params
        .versions
        .as_deref()
        .map(parse_versions_map)
        .unwrap_or_default();

    if let Some(graph) = params.graph.as_ref() {
        validate_iri(graph)?;
        if !is_admin {
            let cached = state
                .auth_db
                .get_accessible_graph_iris_cached(user_id)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            if !cached.0.contains(graph.as_str())
                && !is_authorized_version_graph(state, graph, &versions_map, user_id)?
            {
                return Ok(ScopeGraphs::Empty);
            }
        }
        return Ok(ScopeGraphs::Set(vec![graph.clone()]));
    }

    if params.dataset_ids.is_some() || params.org_id.is_some() || params.dataset_id.is_some() {
        let ds_ids: Vec<String> = if let Some(ref ids_csv) = params.dataset_ids {
            ids_csv
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        } else if let Some(ref org_id) = params.org_id {
            state
                .auth_db
                .list_datasets_by_org(org_id)
                .map_err(|e| AppError::Internal(e.to_string()))?
                .into_iter()
                .map(|ds| ds.id.to_string())
                .collect()
        } else {
            vec![params.dataset_id.clone().unwrap()]
        };
        let scoped = scope_dataset_graphs(state, &ds_ids, &versions_map, user_id, is_admin)?;
        return Ok(if scoped.is_empty() {
            ScopeGraphs::Empty
        } else {
            ScopeGraphs::Set(scoped)
        });
    }

    if is_admin {
        Ok(ScopeGraphs::All)
    } else {
        let cached = state
            .auth_db
            .get_accessible_graph_iris_cached(user_id)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        if cached.0.is_empty() {
            Ok(ScopeGraphs::Empty)
        } else {
            Ok(ScopeGraphs::Set(cached.0.iter().cloned().collect()))
        }
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BrowseTripleParams {
    pub graph: Option<String>,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// Opt-in exact row count. When omitted, the response returns `hasMore`
    /// via a cheap LIMIT+1 probe instead of a full COUNT.
    pub count: Option<bool>,
    /// Filter mode: `"exact"` (default — match IRIs/literals verbatim) or
    /// `"contains"` (case-insensitive substring match against the term's
    /// stringified value, including the graph IRI).
    pub match_mode: Option<String>,
    /// Free-text quick-search substring. Matched case-insensitively against
    /// every column (subject, predicate, object, graph). Combined with the
    /// per-field filters via AND (a row must satisfy all per-field filters
    /// AND have `q` somewhere).
    pub q: Option<String>,
    /// Scope browse results to all named graphs belonging to this dataset ID.
    /// When `graph` is also supplied, `graph` takes precedence (single-graph
    /// drill-down). Non-admin users are additionally restricted to graphs they
    /// can access.
    pub dataset_id: Option<String>,
    /// Comma-separated list of dataset IDs. Scopes results to the union of all
    /// named graphs registered under those datasets. When present, takes
    /// precedence over `dataset_id`. Non-admin users are additionally restricted
    /// to graphs they can access.
    pub dataset_ids: Option<String>,
    /// Scope browse results to all named graphs of all datasets owned by this
    /// organisation ID. Non-admin users are additionally restricted to graphs
    /// they can access.
    pub org_id: Option<String>,
    /// Optional per-dataset version map: comma-separated `datasetId:version`
    /// pairs. For a dataset with a pinned version, results come from that
    /// version's snapshot graphs instead of its live graphs. Datasets absent
    /// from the map (or mapped to `live`) use live data.
    pub versions: Option<String>,
    /// Advanced filter chips as a JSON string: an array of
    /// `{ "field": "subject|predicate|object|graph", "value": "...",
    /// "mode": "exact|contains" }`. Chips on the same field are OR-ed; different
    /// fields are AND-ed (and AND-ed with the legacy single-field params + `q`).
    /// `mode` defaults to `contains`. Used by the triple browser's chip filters.
    pub filters: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BrowseResourceParams {
    pub iri: String,
    pub graph: Option<String>,
    /// Scope to all named graphs belonging to this dataset ID. Mirrors
    /// `BrowseTripleParams` so the graph view can expand a resource within the
    /// same scope as the initial browse load. `graph` takes precedence.
    pub dataset_id: Option<String>,
    /// Comma-separated dataset IDs; union of their named graphs. Takes
    /// precedence over `dataset_id`.
    pub dataset_ids: Option<String>,
    /// Scope to all datasets owned by this organisation ID.
    pub org_id: Option<String>,
    /// Per-dataset version pins (comma-separated `datasetId:version`). A pinned
    /// dataset is read from its version snapshot graphs instead of live graphs.
    pub versions: Option<String>,
}

/// GET /api/browse/graphs — list named graphs accessible to the caller
///
/// Uses the in-memory graph index for O(1) graph enumeration and counts
/// instead of running per-graph SPARQL COUNT queries.
pub async fn browse_graphs(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);

    let mut result = Vec::new();

    // Filter to accessible graphs
    let accessible_set: std::collections::HashSet<String> = if is_admin {
        state
            .store
            .named_graphs()?
            .iter()
            .map(|g| g.as_str().to_string())
            .collect()
    } else {
        let cached_graphs = state
            .auth_db
            .get_accessible_graph_iris_cached(user_id)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        cached_graphs.0.clone()
    };

    // Default graph (only shown to admins) — direct O(1) cache lookup,
    // no need to scan the full graph_counts list.
    if is_admin {
        let default_count = state.store.graph_count_cached(None).unwrap_or(0);
        result.push(serde_json::json!({
            "iri": null,
            "name": "Default Graph",
            "count": default_count,
        }));
    }

    // Iterate the accessible set directly (typically smaller than the full
    // graph index, especially for non-admin users) instead of scanning all
    // graphs and discarding the inaccessible ones.
    result.reserve(accessible_set.len() + 1);
    for iri in &accessible_set {
        let count = state
            .store
            .graph_count_cached(Some(iri.as_str()))
            .unwrap_or(0);
        result.push(serde_json::json!({
            "iri": iri,
            "name": iri.rsplit('/').next().unwrap_or(iri.as_str()),
            "count": count,
        }));
    }

    Ok(Json(result))
}

/// Empty `/api/browse/triples` response, used when the caller has no graphs in
/// scope. Includes an exact `total` of 0 when a count was requested so the UI
/// resolves to "0 triples" rather than leaving the count unknown.
fn empty_browse_body(limit: usize, offset: usize, want_count: bool) -> serde_json::Value {
    let mut body = serde_json::json!({
        "triples": [],
        "hasMore": false,
        "limit": limit,
        "offset": offset,
    });
    if want_count {
        body["total"] = serde_json::json!(0);
    }
    body
}

/// Parse the `versions` browse param: comma-separated `datasetId:version` pairs
/// into a map. A pair with an empty/`live` version means "live data for that dataset".
fn parse_versions_map(s: &str) -> std::collections::HashMap<String, String> {
    s.split(',')
        .filter_map(|pair| {
            let pair = pair.trim();
            if pair.is_empty() {
                return None;
            }
            let mut it = pair.splitn(2, ':');
            let k = it.next()?.trim().to_string();
            let v = it.next().unwrap_or("").trim().to_string();
            if k.is_empty() {
                None
            } else {
                Some((k, v))
            }
        })
        .collect()
}

/// True when `version` names a real snapshot (not live/latest/current/empty).
fn is_pinned_version(version: &str) -> bool {
    !matches!(version.trim(), "" | "live" | "latest" | "current")
}

/// Resolve the scoped named-graph set for a set of datasets, honouring an
/// optional per-dataset version map. For a pinned version we authorise at the
/// dataset level (the snapshot graphs are not in the live accessible-graph set)
/// and trust the version's snapshot graphs; for live datasets we intersect their
/// live graphs with the caller's accessible set (admins see all).
fn scope_dataset_graphs(
    state: &AppState,
    ds_ids: &[String],
    versions: &std::collections::HashMap<String, String>,
    user_id: Option<&str>,
    is_admin: bool,
) -> Result<Vec<String>, AppError> {
    let accessible = if is_admin {
        None
    } else {
        Some(
            state
                .auth_db
                .get_accessible_graph_iris_cached(user_id)
                .map_err(|e| AppError::Internal(e.to_string()))?
                .0
                .clone(),
        )
    };
    let mut out: Vec<String> = Vec::new();
    for id in ds_ids {
        if let Some(v) = versions.get(id) {
            if is_pinned_version(v) {
                // Authorise at the dataset level, then trust the snapshot graphs.
                let ds = state
                    .auth_db
                    .get_dataset(id)
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                let allowed = match &ds {
                    Some(d) => state
                        .auth_db
                        .can_access_dataset(user_id, d)
                        .map_err(|e| AppError::Internal(e.to_string()))?,
                    None => false,
                };
                if !allowed {
                    continue;
                }
                if let Some(snap) = version_snapshot_graphs(state, id, Some(v.as_str()))? {
                    out.extend(snap);
                }
                continue;
            }
        }
        // Live data: intersect this dataset's live graphs with the accessible set.
        let live = state
            .auth_db
            .list_dataset_graphs(id)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        match &accessible {
            None => out.extend(live),
            Some(acc) => out.extend(live.into_iter().filter(|g| acc.contains(g.as_str()))),
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

/// True when `graph` is a snapshot graph of one of the pinned dataset versions and
/// the caller can access that dataset — used to authorise single-graph drill-down
/// into a version snapshot (whose IRIs are absent from the live accessible set).
fn is_authorized_version_graph(
    state: &AppState,
    graph: &str,
    versions: &std::collections::HashMap<String, String>,
    user_id: Option<&str>,
) -> Result<bool, AppError> {
    for (ds_id, v) in versions {
        if !is_pinned_version(v) {
            continue;
        }
        if let Some(snap) = version_snapshot_graphs(state, ds_id, Some(v.as_str()))? {
            if snap.iter().any(|g| g == graph) {
                let ds = state
                    .auth_db
                    .get_dataset(ds_id)
                    .map_err(|e| AppError::Internal(e.to_string()))?;
                if let Some(d) = ds {
                    if state
                        .auth_db
                        .can_access_dataset(user_id, &d)
                        .map_err(|e| AppError::Internal(e.to_string()))?
                    {
                        return Ok(true);
                    }
                }
            }
        }
    }
    Ok(false)
}

/// GET /api/browse/triples — browse triples with optional filters, scoped to accessible graphs
pub async fn browse_triples(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<BrowseTripleParams>,
) -> Result<impl IntoResponse, AppError> {
    // Cap at 1 000 000 rows to guard against accidental memory exhaustion.
    // The previous 10 000 limit was unnecessarily restrictive for bulk data exports.
    let limit = params.limit.unwrap_or(100).min(1_000_000);
    let offset = params.offset.unwrap_or(0);

    let contains_mode = matches!(params.match_mode.as_deref(), Some("contains"));

    // The quick-search `q` is parsed as a boolean mini-language (AND/OR/XOR/NOT,
    // parentheses, quoted phrases) by build_q_filter, which validates each term.

    // Validate filter parameters. In `exact` mode, IRIs and literals are validated
    // separately; in `contains` mode every filter is treated as a free-form
    // substring with stricter validation (no quotes, no angle brackets).
    if contains_mode {
        if let Some(ref s) = params.subject {
            validate_substring(s)?;
        }
        if let Some(ref p) = params.predicate {
            validate_substring(p)?;
        }
        if let Some(ref o) = params.object {
            validate_substring(o)?;
        }
        if let Some(ref g) = params.graph {
            validate_substring(g)?;
        }
    } else {
        if let Some(ref s) = params.subject {
            validate_iri(s)?;
        }
        if let Some(ref p) = params.predicate {
            validate_iri(p)?;
        }
        if let Some(ref o) = params.object {
            if o.starts_with("http://") || o.starts_with("https://") || o.starts_with("urn:") {
                validate_iri(o)?;
            } else {
                validate_literal_value(o)?;
            }
        }
        if let Some(ref g) = params.graph {
            validate_iri(g)?;
        }
    }

    // Check graph access
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);

    // Per-dataset version pins (datasetId:version). Empty when not version-scoped.
    let versions_map = params
        .versions
        .as_deref()
        .map(parse_versions_map)
        .unwrap_or_default();

    // Advanced filter chips (subject/predicate/object/graph). Chips on the same
    // field are OR-ed; different fields are AND-ed. Each value is validated by
    // `chip_clause` before interpolation, so the same injection guards apply.
    let chips: Vec<BrowseFilterChip> = match params.filters.as_deref() {
        Some(raw) if !raw.trim().is_empty() => serde_json::from_str(raw)
            .map_err(|_| AppError::BadRequest("invalid filters parameter".to_string()))?,
        _ => Vec::new(),
    };
    if chips.len() > 64 {
        return Err(AppError::BadRequest(
            "too many filter chips (max 64)".to_string(),
        ));
    }
    let mut subj_clauses: Vec<String> = Vec::new();
    let mut pred_clauses: Vec<String> = Vec::new();
    let mut obj_clauses: Vec<String> = Vec::new();
    let mut graph_clauses: Vec<String> = Vec::new();
    let mut vocab_clauses: Vec<String> = Vec::new();
    // Negated chips (`neg: true`): rows matching the clause are excluded. Each
    // exclusion ANDs independently (drop the row if it matches ANY of them), so
    // they share one list and emit one FILTER each — unlike the OR-ed positive
    // buckets above.
    let mut neg_clauses: Vec<String> = Vec::new();
    for chip in &chips {
        if chip.value.is_empty() {
            continue;
        }
        let mode = chip.mode.as_deref().unwrap_or("contains");
        let clause = match chip.field.as_str() {
            "subject" => chip_clause("s", &chip.value, mode, false)?,
            "predicate" => chip_clause("p", &chip.value, mode, false)?,
            "object" => chip_clause("o", &chip.value, mode, true)?,
            "graph" => chip_clause("g", &chip.value, mode, false)?,
            "vocabulary" => vocab_clause(&chip.value, mode)?,
            _ => {
                return Err(AppError::BadRequest(
                    "filter field must be subject, predicate, object, graph, or vocabulary"
                        .to_string(),
                ))
            }
        };
        if chip.neg.unwrap_or(false) {
            neg_clauses.push(format!("!({clause})"));
            continue;
        }
        match chip.field.as_str() {
            "subject" => subj_clauses.push(clause),
            "predicate" => pred_clauses.push(clause),
            "object" => obj_clauses.push(clause),
            "graph" => graph_clauses.push(clause),
            "vocabulary" => vocab_clauses.push(clause),
            _ => unreachable!("field validated above"),
        }
    }
    // Graph chips scan/bind ?g, which the single-graph fast path doesn't expose,
    // so any graph chip — positive or negated — forces a ?g-binding branch (ACL
    // still enforced by the candidate ?g set in every branch).
    let has_graph_chips = chips
        .iter()
        .any(|c| c.field == "graph" && !c.value.is_empty());

    // In exact mode the graph filter is bound directly via `GRAPH <iri>` and is
    // ACL-checked here. In contains mode the same access check is enforced by
    // restricting the candidate graphs (VALUES ?g { … }) to the accessible set.
    let exact_graph = if contains_mode || has_graph_chips {
        None
    } else {
        params.graph.as_ref()
    };
    if let Some(graph) = exact_graph {
        if !is_admin {
            let cached_graphs = state
                .auth_db
                .get_accessible_graph_iris_cached(user_id)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            // Allow a live accessible graph, or a snapshot graph of a pinned
            // version on a dataset the caller can access (snapshot IRIs are not
            // part of the live accessible set).
            if !cached_graphs.0.contains(graph.as_str())
                && !is_authorized_version_graph(&state, graph, &versions_map, user_id)?
            {
                return Err(AppError::NotFound("Graph not found".to_string()));
            }
        }
    }

    let mut filters = Vec::new();
    if contains_mode {
        // Lower-case both the term's stringification and the needle so the match
        // is case-insensitive — users typing "Person" still find rdf:type Person.
        if let Some(ref s) = params.subject {
            filters.push(format!(
                "FILTER(CONTAINS(LCASE(STR(?s)), LCASE(\"{}\")))",
                s
            ));
        }
        if let Some(ref p) = params.predicate {
            filters.push(format!(
                "FILTER(CONTAINS(LCASE(STR(?p)), LCASE(\"{}\")))",
                p
            ));
        }
        if let Some(ref o) = params.object {
            filters.push(format!(
                "FILTER(CONTAINS(LCASE(STR(?o)), LCASE(\"{}\")))",
                o
            ));
        }
        if let Some(ref g) = params.graph {
            filters.push(format!(
                "FILTER(CONTAINS(LCASE(STR(?g)), LCASE(\"{}\")))",
                g
            ));
        }
    } else {
        if let Some(ref s) = params.subject {
            filters.push(format!("FILTER(?s = <{}>)", s));
        }
        if let Some(ref p) = params.predicate {
            filters.push(format!("FILTER(?p = <{}>)", p));
        }
        if let Some(ref o) = params.object {
            if o.starts_with("http://") || o.starts_with("https://") || o.starts_with("urn:") {
                filters.push(format!("FILTER(?o = <{}>)", o));
            } else {
                filters.push(format!("FILTER(str(?o) = \"{}\")", o));
            }
        }
    }

    // Free-text quick-search, parsed as a boolean mini-language (AND/OR/XOR/NOT,
    // parentheses, quoted phrases) over every bound column. ?g is bound in every
    // query branch (either as `(<iri> AS ?g)` for the single-graph case, or as the
    // GRAPH variable elsewhere) so it's safe to include in the disjunction.
    if let Some(ref q) = params.q {
        if let Some(expr) = build_q_filter(q)? {
            filters.push(format!("FILTER({})", expr));
        }
    }

    // Advanced filter chips: OR the clauses within each field, then AND the
    // per-field groups (and AND them with the legacy filters and `q` above).
    if !subj_clauses.is_empty() {
        filters.push(format!("FILTER({})", subj_clauses.join(" || ")));
    }
    if !pred_clauses.is_empty() {
        filters.push(format!("FILTER({})", pred_clauses.join(" || ")));
    }
    if !obj_clauses.is_empty() {
        filters.push(format!("FILTER({})", obj_clauses.join(" || ")));
    }
    if !graph_clauses.is_empty() {
        filters.push(format!("FILTER({})", graph_clauses.join(" || ")));
    }
    if !vocab_clauses.is_empty() {
        filters.push(format!("FILTER({})", vocab_clauses.join(" || ")));
    }
    // Negated chips: each is excluded independently (AND), so one FILTER apiece.
    for nc in &neg_clauses {
        filters.push(format!("FILTER({nc})"));
    }

    let filter_clause = filters.join("\n");

    // Build graph-scoped or accessible-graphs-scoped query.
    // ORDER BY is intentionally omitted: on large stores it forces a full sort and
    // caused 20s client timeouts. Pagination still works via stable OFFSET/LIMIT.
    //
    // By default we now skip the COUNT query entirely and issue a LIMIT+1 probe
    // to derive `hasMore`. The exact total is opt-in via ?count=true — the
    // previous unconditional COUNT(*) was the main reason /browse took >1 min.
    //
    // Multi-graph scoping uses VALUES ?g { ... } rather than one FROM NAMED per
    // graph; the Oxigraph planner handles VALUES far better when a user has many
    // accessible graphs.
    let want_count = params.count.unwrap_or(false);
    let probe_limit = limit.saturating_add(1);
    let fc = if filter_clause.is_empty() {
        String::new()
    } else {
        format!(" . {}", filter_clause)
    };

    let (query, count_query): (String, Option<String>) = if let Some(graph) = exact_graph {
        (
            format!(
                "SELECT ?s ?p ?o (<{g}> AS ?g) WHERE {{ GRAPH <{g}> {{ ?s ?p ?o{fc} }} }} LIMIT {pl} OFFSET {off}",
                g = graph, fc = fc, pl = probe_limit, off = offset,
            ),
            want_count.then(|| format!(
                "SELECT (COUNT(*) AS ?count) WHERE {{ SELECT ?s WHERE {{ GRAPH <{g}> {{ ?s ?p ?o{fc} }} }} }}",
                g = graph, fc = fc,
            )),
        )
    } else if params.dataset_ids.is_some() || params.org_id.is_some() || params.dataset_id.is_some()
    {
        // Resolve the set of dataset IDs in scope, then collect their graphs —
        // each dataset's graphs come from its pinned version snapshot (if any) or
        // its live graphs. Shared across the dataset_ids / org_id / dataset_id cases.
        let ds_ids: Vec<String> = if let Some(ref ids_csv) = params.dataset_ids {
            ids_csv
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        } else if let Some(ref org_id) = params.org_id {
            state
                .auth_db
                .list_datasets_by_org(org_id)
                .map_err(|e| AppError::Internal(e.to_string()))?
                .into_iter()
                .map(|ds| ds.id.to_string())
                .collect()
        } else {
            vec![params.dataset_id.clone().unwrap()]
        };

        let scoped = scope_dataset_graphs(&state, &ds_ids, &versions_map, user_id, is_admin)?;

        if scoped.is_empty() {
            return Ok(Json(empty_browse_body(limit, offset, want_count)));
        }

        let mut values = String::with_capacity(scoped.len() * 40);
        for iri in &scoped {
            values.push('<');
            values.push_str(iri);
            values.push_str("> ");
        }
        (
            format!(
                "SELECT ?s ?p ?o ?g WHERE {{ VALUES ?g {{ {v}}} GRAPH ?g {{ ?s ?p ?o{fc} }} }} LIMIT {pl} OFFSET {off}",
                v = values, fc = fc, pl = probe_limit, off = offset,
            ),
            want_count.then(|| format!(
                "SELECT (COUNT(*) AS ?count) WHERE {{ SELECT ?s WHERE {{ VALUES ?g {{ {v}}} GRAPH ?g {{ ?s ?p ?o{fc} }} }} }}",
                v = values, fc = fc,
            )),
        )
    } else if is_admin {
        (
            format!(
                "SELECT ?s ?p ?o ?g WHERE {{ GRAPH ?g {{ ?s ?p ?o{fc} }} }} LIMIT {pl} OFFSET {off}",
                fc = fc, pl = probe_limit, off = offset,
            ),
            want_count.then(|| format!(
                "SELECT (COUNT(*) AS ?count) WHERE {{ SELECT ?s WHERE {{ GRAPH ?g {{ ?s ?p ?o{fc} }} }} }}",
                fc = fc,
            )),
        )
    } else {
        let cached_graphs = state
            .auth_db
            .get_accessible_graph_iris_cached(user_id)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let accessible = &cached_graphs.0;
        if accessible.is_empty() {
            return Ok(Json(empty_browse_body(limit, offset, want_count)));
        }
        let mut values = String::with_capacity(accessible.len() * 40);
        for iri in accessible {
            values.push('<');
            values.push_str(iri);
            values.push_str("> ");
        }
        (
            format!(
                "SELECT ?s ?p ?o ?g WHERE {{ VALUES ?g {{ {v}}} GRAPH ?g {{ ?s ?p ?o{fc} }} }} LIMIT {pl} OFFSET {off}",
                v = values, fc = fc, pl = probe_limit, off = offset,
            ),
            want_count.then(|| format!(
                "SELECT (COUNT(*) AS ?count) WHERE {{ SELECT ?s WHERE {{ VALUES ?g {{ {v}}} GRAPH ?g {{ ?s ?p ?o{fc} }} }} }}",
                v = values, fc = fc,
            )),
        )
    };

    // Bound how many browse queries run blocking work concurrently so a burst
    // of browse traffic can't exhaust the shared spawn_blocking pool. Held until
    // both the triples and (optional) count tasks complete.
    let _browse_permit = state
        .browse_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::Internal("browse concurrency limiter closed".to_string()))?;

    let store1 = state.store.clone();
    let triples_task = tokio::task::spawn_blocking(move || {
        let results = store1.query(&query)?;
        format_sparql_results_as_triples(results)
    });

    let count_task = count_query.map(|cq| {
        let store2 = state.store.clone();
        tokio::task::spawn_blocking(move || -> usize {
            if let Ok(oxigraph::sparql::QueryResults::Solutions(mut solutions)) = store2.query(&cq)
            {
                if let Some(Ok(solution)) = solutions.next() {
                    // Use the literal's lexical value directly instead of
                    // re-parsing the term's debug/turtle string ("123"^^xsd:integer).
                    return solution
                        .get("count")
                        .and_then(|v| match v {
                            oxigraph::model::Term::Literal(lit) => {
                                lit.value().parse::<usize>().ok()
                            }
                            _ => None,
                        })
                        .unwrap_or(0);
                }
            }
            0
        })
    });

    let mut triples = triples_task
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;

    let has_more = triples.len() > limit;
    if has_more {
        triples.truncate(limit);
    }

    let mut body = serde_json::json!({
        "triples": triples,
        "hasMore": has_more,
        "limit": limit,
        "offset": offset,
    });

    if let Some(task) = count_task {
        let total = task.await.unwrap_or(0);
        body["total"] = serde_json::json!(total);
    }

    Ok(Json(body))
}

/// GET /api/browse/facets — classes, properties and named graphs *present in the
/// current scope*, with counts. Reuses the same scope resolution as
/// `browse_triples` (single `graph`, else dataset/org scope with version pins,
/// else all graphs for admins / the accessible set otherwise), so the facets a
/// caller sees always match what they can browse. The frontend derives the
/// "vocabularies / namespaces" facet from the class+property IRIs and attaches
/// graph roles itself, so no extra endpoints are needed.
pub async fn browse_facets(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<BrowseTripleParams>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);

    let empty = || serde_json::json!({ "classes": [], "properties": [], "graphs": [] });

    // Resolve the scope; `GRAPH ?g { … }` for admins with no scope, else a
    // `VALUES ?g { … }`-restricted set.
    let (open, close) = match resolve_scope_graphs(&state, &params, user_id, is_admin)? {
        ScopeGraphs::Empty => return Ok(Json(empty())),
        ScopeGraphs::All => ("GRAPH ?g { ".to_string(), " }".to_string()),
        ScopeGraphs::Set(graphs) => {
            let mut values = String::with_capacity(graphs.len() * 40);
            for iri in &graphs {
                values.push('<');
                values.push_str(iri);
                values.push_str("> ");
            }
            (
                format!("VALUES ?g {{ {values}}} GRAPH ?g {{ "),
                " }".to_string(),
            )
        }
    };

    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    let cls_q = format!(
        "SELECT ?cls (COUNT(DISTINCT ?s) AS ?c) WHERE {{ {open}?s a ?cls{close} }} GROUP BY ?cls ORDER BY DESC(?c) LIMIT 300"
    );
    let prop_q = format!(
        "SELECT ?p (COUNT(*) AS ?c) WHERE {{ {open}?s ?p ?o . FILTER(?p != <{RDF_TYPE}>){close} }} GROUP BY ?p ORDER BY DESC(?c) LIMIT 300"
    );
    let graph_q = format!(
        "SELECT ?g (COUNT(*) AS ?c) WHERE {{ {open}?s ?p ?o{close} }} GROUP BY ?g ORDER BY DESC(?c) LIMIT 1000"
    );

    let _browse_permit = state
        .browse_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::Internal("browse concurrency limiter closed".to_string()))?;

    let store = state.store.clone();
    let facets = tokio::task::spawn_blocking(move || {
        // Pull distinct IRI key + integer count rows out of an aggregate result.
        let collect = |query: &str, key: &str| -> Vec<serde_json::Value> {
            let mut out = Vec::new();
            if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(query) {
                for sol in solutions.filter_map(|s| s.ok()) {
                    let iri = match sol.get(key) {
                        Some(oxigraph::model::Term::NamedNode(n)) => n.as_str().to_string(),
                        _ => continue,
                    };
                    let count = match sol.get("c") {
                        Some(oxigraph::model::Term::Literal(l)) => {
                            l.value().parse::<u64>().unwrap_or(0)
                        }
                        _ => 0,
                    };
                    out.push(serde_json::json!({ "iri": iri, "count": count }));
                }
            }
            out
        };
        serde_json::json!({
            "classes": collect(&cls_q, "cls"),
            "properties": collect(&prop_q, "p"),
            "graphs": collect(&graph_q, "g"),
        })
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(facets))
}

/// GET /api/browse/resource — get all triples about a resource (scoped to accessible graphs)
pub async fn browse_resource(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<BrowseResourceParams>,
) -> Result<impl IntoResponse, AppError> {
    let iri = &params.iri;

    // Validate IRI parameters
    validate_iri(iri)?;
    if let Some(ref g) = params.graph {
        validate_iri(g)?;
    }

    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);

    // Build FROM + FROM NAMED clauses scoped to accessible graphs.
    //
    // Even for admins we always inject FROM clauses, because store data lives in
    // named graphs — without FROM, default-graph queries (the kind we issue here)
    // return nothing.
    // Graphs this request may read. Drives BOTH the SPARQL path (FROM clauses)
    // and the blank-node path (a low-level quad scan — SPARQL cannot address a
    // stored blank node by its label).
    let mut search_graphs: Vec<String> = if let Some(ref graph) = params.graph {
        if !is_admin {
            let cached_graphs = state
                .auth_db
                .get_accessible_graph_iris_cached(user_id)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            if !cached_graphs.0.contains(graph.as_str()) {
                return Err(AppError::NotFound("Graph not found".to_string()));
            }
        }
        vec![graph.clone()]
    } else if params.dataset_ids.is_some() || params.org_id.is_some() || params.dataset_id.is_some()
    {
        // Honour the browse scope (dataset/org + version pins), mirroring
        // browse_triples → resolve_scope_graphs, so expanding a resource in the
        // graph view reads the same (possibly version-snapshot) graphs as the
        // initial scoped load — not the broad accessible set. Access control is
        // enforced inside scope_dataset_graphs.
        let versions_map = params
            .versions
            .as_deref()
            .map(parse_versions_map)
            .unwrap_or_default();
        let ds_ids: Vec<String> = if let Some(ref ids_csv) = params.dataset_ids {
            ids_csv
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        } else if let Some(ref org_id) = params.org_id {
            state
                .auth_db
                .list_datasets_by_org(org_id)
                .map_err(|e| AppError::Internal(e.to_string()))?
                .into_iter()
                .map(|ds| ds.id.to_string())
                .collect()
        } else {
            vec![params.dataset_id.clone().unwrap()]
        };
        let scoped = scope_dataset_graphs(&state, &ds_ids, &versions_map, user_id, is_admin)?;
        if scoped.is_empty() {
            return Ok(Json(serde_json::json!({
                "iri": iri,
                "label": null,
                "outgoing": [],
                "incoming": [],
                "bnodes": {},
                "reason": "no_accessible_graphs",
            })));
        }
        scoped
    } else {
        let cached_graphs = state
            .auth_db
            .get_accessible_graph_iris_cached(user_id)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let visible: &std::collections::HashSet<String> = if is_admin {
            &cached_graphs.1
        } else {
            &cached_graphs.0
        };
        if visible.is_empty() {
            return Ok(Json(serde_json::json!({
                "iri": iri,
                "label": null,
                "outgoing": [],
                "incoming": [],
                "bnodes": {},
                "reason": "no_accessible_graphs",
            })));
        }
        visible.iter().cloned().collect()
    };

    // Blank-node subjects (`_:label`) can't be re-queried in SPARQL — a `_:x`
    // there is a fresh variable, and `<_:label>` parses as a relative IRI, so the
    // old code returned nothing and clicking a blank node opened an empty page.
    // Oxigraph preserves blank-node labels in storage, so resolve them via the
    // low-level quad API instead, scoped to the authorized graphs.
    if let Some(bnode_label) = iri.strip_prefix("_:") {
        let (outgoing, bnodes, incoming) = resolve_blank_node(
            state.store.store(),
            bnode_label,
            &search_graphs,
            MAX_BNODE_DEPTH,
        );
        return Ok(Json(serde_json::json!({
            "iri": iri,
            "label": null,
            "outgoing": outgoing,
            "incoming": incoming,
            "bnodes": bnodes,
        })));
    }

    // Dataset nodes: a dataset's DCAT description (title, members, contact),
    // its API services and its version history all live in `urn:system:*` graphs
    // that are deliberately excluded from general user SPARQL scoping. Without
    // them a clicked dataset IRI renders an empty node. So when the requested IRI
    // is a dataset IRI the caller may access, widen the scope to that dataset's
    // metadata graph plus the two shared registries — just for this browse. The
    // queries below only ever match the specific dataset IRI, so pulling in the
    // shared registries cannot leak other datasets' rows.
    if let Some(ds_id) = dataset_id_from_iri(iri, &state.base_url) {
        if let Ok(Some(ds)) = state.auth_db.get_dataset(&ds_id) {
            if state
                .auth_db
                .can_access_dataset(user_id, &ds)
                .unwrap_or(false)
            {
                search_graphs.push(crate::auth::dataset_graph::dataset_metadata_graph_iri(
                    &ds_id,
                ));
                search_graphs.push(crate::saved_queries::metadata::REGISTRY_GRAPH.to_string());
                search_graphs.push(crate::dataset_versions::registry::REGISTRY_GRAPH.to_string());
            }
        }
    }

    let from_clauses: Option<String> = if params.graph.is_some() {
        None // specific graph injected via GRAPH <…> in the query template below
    } else {
        let mut clauses = String::new();
        for g in &search_graphs {
            clauses.push_str(&format!("FROM <{}>\nFROM NAMED <{}>\n", g, g));
        }
        Some(clauses)
    };

    let build_query = |template: &str| -> String {
        if let Some(ref clauses) = from_clauses {
            inject_from_clauses(template, clauses)
        } else {
            template.to_string()
        }
    };

    // Outgoing triples (resource as subject) plus the triples of any blank nodes
    // reachable from it — a bounded Concise Bounded Description. Fetched in ONE
    // query so blank-node labels are internally consistent: a bnode that appears
    // as an object in `outgoing` is the same label that keys `bnodes`. (SPARQL
    // blank-node labels are scoped to a single result set, so we cannot re-query
    // a bnode by label across separate queries — hence the single-query closure.)
    const MAX_BNODE_DEPTH: usize = 5;
    let outgoing_query = build_query(&build_resource_closure_query(
        iri,
        params.graph.as_deref(),
        MAX_BNODE_DEPTH,
    ));
    let (outgoing, bnodes) = split_resource_closure(state.store.query(&outgoing_query)?)?;

    // Incoming triples (resource as object)
    let incoming_query = build_query(&if let Some(ref graph) = params.graph {
        format!(
            "SELECT ?s ?p WHERE {{ GRAPH <{}> {{ ?s ?p <{}> }} }} ORDER BY ?s ?p",
            graph, iri
        )
    } else {
        format!("SELECT ?s ?p WHERE {{ ?s ?p <{}> }} ORDER BY ?s ?p", iri)
    });

    let incoming_results = state.store.query(&incoming_query)?;
    let incoming = format_sparql_results_as_pairs(incoming_results, "s", "p")?;

    // Get label if available
    let label_query = build_query(&format!(
        "SELECT ?label WHERE {{ <{}> <http://www.w3.org/2000/01/rdf-schema#label> ?label }} LIMIT 1",
        iri
    ));
    let label = if let Ok(oxigraph::sparql::QueryResults::Solutions(mut solutions)) =
        state.store.query(&label_query)
    {
        solutions
            .next()
            .and_then(|r| r.ok())
            .and_then(|s| s.get("label").map(format_term))
    } else {
        None
    };

    Ok(Json(serde_json::json!({
        "iri": iri,
        "label": label,
        "outgoing": outgoing,
        "incoming": incoming,
        "bnodes": bnodes,
    })))
}

/// GET /api/browse/stats — store statistics scoped to accessible graphs
pub async fn browse_stats(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
) -> Result<impl IntoResponse, AppError> {
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);

    let (total_triples, named_graph_count) = if is_admin {
        let total = state.store.len()?;
        let graphs = state.store.named_graphs()?;
        (total, graphs.len())
    } else {
        let cached_graphs = state
            .auth_db
            .get_accessible_graph_iris_cached(user_id)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let accessible = &cached_graphs.0;
        let count = accessible.len();
        // Count triples only in accessible named graphs
        let total: usize = if accessible.is_empty() {
            0
        } else {
            let mut from_clauses = String::new();
            for iri in accessible {
                from_clauses.push_str(&format!("FROM <{}>\nFROM NAMED <{}>\n", iri, iri));
            }
            let count_query = inject_from_clauses(
                "SELECT (COUNT(*) AS ?count) WHERE { ?s ?p ?o }",
                &from_clauses,
            );
            if let Ok(oxigraph::sparql::QueryResults::Solutions(mut solutions)) =
                state.store.query(&count_query)
            {
                solutions
                    .next()
                    .and_then(|r| r.ok())
                    .and_then(|s| {
                        s.get("count").and_then(|v| {
                            let str_val = v.to_string();
                            str_val
                                .trim_matches('"')
                                .split('"')
                                .next()
                                .and_then(|n| n.parse::<usize>().ok())
                        })
                    })
                    .unwrap_or(0)
            } else {
                0
            }
        };
        (total, count)
    };

    Ok(Json(serde_json::json!({
        "total_triples": total_triples,
        "named_graphs": named_graph_count,
        "version": env!("CARGO_PKG_VERSION"),
    })))
}

// ─── Browse Suggest ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct BrowseSuggestParams {
    /// Which field to suggest values for: "subject", "predicate", "object", or "graph"
    pub field: String,
    /// Optional prefix to filter suggestions
    pub prefix: Option<String>,
    /// Maximum number of suggestions (default 30, max 200)
    pub limit: Option<usize>,
    /// Optional: restrict suggestions to a single dataset's graphs (used by the
    /// API-service variable autocomplete to stay scoped to the relevant data).
    pub dataset: Option<String>,
    /// Optional: when suggesting subjects/objects, only consider triples whose
    /// predicate is this IRI — position-aware completion for `{{param}}` values.
    pub predicate: Option<String>,
}

/// Reject anything that could break out of an `<iri>` term in injected SPARQL.
fn valid_suggest_iri(iri: &str) -> bool {
    (iri.starts_with("http://") || iri.starts_with("https://") || iri.starts_with("urn:"))
        && !iri.contains(['<', '>', '"', '{', '}', '\\', '|', '^', '`', ' '])
        && !iri.contains(char::is_control)
}

/// GET /api/browse/suggest — return distinct values for a triple-browser field
pub async fn browse_suggest(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Query(params): Query<BrowseSuggestParams>,
) -> Result<impl IntoResponse, AppError> {
    let limit = params.limit.unwrap_or(30).min(200);
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);

    // Validate prefix if it looks like an IRI
    let prefix = params.prefix.as_deref().unwrap_or("");
    if (prefix.starts_with("http://")
        || prefix.starts_with("https://")
        || prefix.starts_with("urn:"))
        && prefix.contains('>')
    {
        return Err(AppError::BadRequest("Invalid prefix".to_string()));
    }

    // The variable name in SPARQL for the selected field
    let var = match params.field.as_str() {
        "subject" => "s",
        "predicate" => "p",
        "object" => "o",
        "graph" => "g",
        _ => {
            return Err(AppError::BadRequest(
                "field must be subject, predicate, object, or graph".to_string(),
            ))
        }
    };

    // Build prefix filter. Validate (not just quote-escape) the prefix: a trailing
    // backslash would otherwise escape the closing quote we append and corrupt the
    // generated SPARQL. `validate_substring` rejects `\ " < > ` ` and control chars.
    let filter = if prefix.is_empty() {
        String::new()
    } else {
        validate_substring(prefix)?;
        format!(" FILTER(STRSTARTS(STR(?{}), \"{}\"))", var, prefix)
    };

    // Optional predicate constraint (subject/object completion only).
    let predicate_term = match params.predicate.as_deref().filter(|p| !p.is_empty()) {
        None => "?p".to_string(),
        Some(p) if params.field == "subject" || params.field == "object" => {
            if !valid_suggest_iri(p) {
                return Err(AppError::BadRequest("invalid predicate IRI".to_string()));
            }
            format!("<{p}>")
        }
        Some(_) => "?p".to_string(),
    };

    // Build the base query pattern
    let pattern = if params.field == "graph" {
        format!("GRAPH ?g {{ ?s ?p ?o }}{}", filter)
    } else {
        format!(
            "GRAPH ?g {{ ?s {pred} ?o{} }}",
            filter,
            pred = predicate_term
        )
    };

    let base_query = format!(
        "SELECT DISTINCT ?{var} WHERE {{ {pattern} }} LIMIT {limit}",
        var = var,
        pattern = pattern,
        limit = limit,
    );

    // Determine the graph set to scope to:
    //   - an explicit `dataset` (only if the caller can access it) → that dataset's graphs;
    //   - otherwise non-admins are scoped to their accessible graphs; admins see all.
    let scoped_graphs: Option<Vec<String>> = if let Some(ds_id) = params.dataset.as_deref() {
        match state
            .auth_db
            .get_dataset(ds_id)
            .map_err(|e| AppError::Internal(e.to_string()))?
        {
            Some(ds)
                if state
                    .auth_db
                    .can_access_dataset(user_id, &ds)
                    .unwrap_or(false) =>
            {
                Some(
                    state
                        .auth_db
                        .list_dataset_graphs(ds_id)
                        .map_err(|e| AppError::Internal(e.to_string()))?,
                )
            }
            // Unknown or inaccessible dataset → no suggestions (non-fatal for the UI).
            _ => return Ok(Json(serde_json::json!({ "values": [] }))),
        }
    } else if is_admin {
        None
    } else {
        Some(
            state
                .auth_db
                .get_accessible_graph_iris_cached(user_id)
                .map_err(|e| AppError::Internal(e.to_string()))?
                .0
                .iter()
                .cloned()
                .collect(),
        )
    };

    let query = match scoped_graphs {
        None => base_query,
        Some(graphs) => {
            if graphs.is_empty() {
                return Ok(Json(serde_json::json!({ "values": [] })));
            }
            let mut from_clauses = String::new();
            for iri in &graphs {
                from_clauses.push_str(&format!("FROM NAMED <{}>\n", iri));
            }
            inject_from_clauses(&base_query, &from_clauses)
        }
    };

    let mut values: Vec<serde_json::Value> = Vec::new();
    if let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = state.store.query(&query) {
        for sol in solutions.filter_map(|s| s.ok()) {
            if let Some(term) = sol.get(var) {
                values.push(format_term(term));
            }
        }
    }

    Ok(Json(serde_json::json!({ "values": values })))
}

// ─── Dataset SPARQL Service Endpoint ──────────────────────────────────────────

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DatasetSparqlParams {
    pub query: Option<String>,
    /// Optional dataset version label. When set to a real version, the query is
    /// scoped to that version's snapshot graphs instead of the live graphs.
    /// Empty / "live" / "latest" / "current" mean live data.
    pub version: Option<String>,
}

/// Resolve the snapshot graphs for an explicit dataset version label.
/// Returns `Ok(None)` for live data (empty / "live" / "latest" / "current"),
/// or `Ok(Some(graphs))` for a pinned version. Errors if the version is unknown.
fn version_snapshot_graphs(
    state: &AppState,
    dataset_id: &str,
    version: Option<&str>,
) -> Result<Option<Vec<String>>, AppError> {
    let v = match version.map(str::trim) {
        None | Some("") | Some("live") | Some("latest") | Some("current") => return Ok(None),
        Some(v) => v,
    };
    let ver = crate::dataset_versions::registry::get_version(
        &state.store,
        state.base_url.as_str(),
        dataset_id,
        v,
    )
    .ok_or_else(|| AppError::NotFound(format!("dataset version '{v}' not found")))?;
    Ok(Some(ver.snapshot_graphs))
}

/// GET /api/datasets/:dataset_id/services/:service_slug/sparql?query=...
pub async fn dataset_sparql_query(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, service_slug)): Path<(String, String)>,
    Query(params): Query<DatasetSparqlParams>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let query = params
        .query
        .ok_or_else(|| AppError::BadRequest("Missing 'query' parameter".to_string()))?;

    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/sparql-results+json");

    execute_dataset_query(
        &state,
        user.as_deref(),
        &dataset_id,
        &service_slug,
        &query,
        params.version.as_deref(),
        accept,
    )
    .await
}

/// POST /api/datasets/:dataset_id/services/:service_slug/sparql
pub async fn dataset_sparql_post(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, service_slug)): Path<(String, String)>,
    Query(params): Query<DatasetSparqlParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/sparql-results+json");

    // For POST the query lives in the body; the version (if any) is a URL query param.
    let version = params.version.as_deref();

    let body_str = String::from_utf8(body.to_vec())
        .map_err(|_| AppError::BadRequest("Invalid UTF-8".to_string()))?;

    if content_type.starts_with("application/sparql-query") {
        execute_dataset_query(
            &state,
            user.as_deref(),
            &dataset_id,
            &service_slug,
            &body_str,
            version,
            accept,
        )
        .await
    } else if content_type.starts_with("application/x-www-form-urlencoded") {
        let form: Vec<(String, String)> = url::form_urlencoded::parse(body_str.as_bytes())
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let query = form
            .iter()
            .find(|(k, _)| k == "query")
            .map(|(_, v)| v.as_str())
            .ok_or_else(|| AppError::BadRequest("Missing 'query' in form body".to_string()))?;
        // A version supplied in the form body overrides the URL one.
        let form_version = form
            .iter()
            .find(|(k, _)| k == "version")
            .map(|(_, v)| v.as_str());
        execute_dataset_query(
            &state,
            user.as_deref(),
            &dataset_id,
            &service_slug,
            query,
            form_version.or(version),
            accept,
        )
        .await
    } else {
        execute_dataset_query(
            &state,
            user.as_deref(),
            &dataset_id,
            &service_slug,
            &body_str,
            version,
            accept,
        )
        .await
    }
}

async fn execute_dataset_query(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    dataset_id: &str,
    service_slug: &str,
    query: &str,
    version: Option<&str>,
    accept: &str,
) -> Result<Response, AppError> {
    // Check dataset access
    let dataset = state
        .auth_db
        .get_dataset(dataset_id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound("Dataset not found".to_string()))?;

    let user_id = user.map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::NotFound("Dataset not found".to_string()));
    }

    // Find the service and its graphs
    let service = state
        .auth_db
        .get_sparql_service_by_slug(dataset_id, service_slug)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound("Service not found".to_string()))?;

    // When a version is pinned, scope to that version's snapshot graphs instead of
    // the service's live graphs (dataset access was already checked above, and the
    // version belongs to this dataset). Otherwise use the service graphs, falling
    // back to all dataset graphs so the service works without manual registration.
    let graphs = if let Some(snapshot) = version_snapshot_graphs(state, dataset_id, version)? {
        snapshot
    } else {
        let service_graphs = state
            .auth_db
            .list_service_graphs(&service.id)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        if service_graphs.is_empty() {
            state
                .auth_db
                .list_dataset_graphs(dataset_id)
                .map_err(|e| AppError::Internal(e.to_string()))?
        } else {
            service_graphs
        }
    };

    // Drop graphs flagged `private` for callers who cannot write the dataset, so a
    // viewer (or anonymous user on a public dataset) cannot read a graph the owner
    // marked private — matching the global /sparql path (get_accessible_graph_iris).
    let can_write = match user {
        Some(u) => state
            .auth_db
            .can_write_dataset(&u.user_id, &dataset)
            .unwrap_or(false),
        None => false,
    };
    let graphs: Vec<String> = if can_write {
        graphs
    } else {
        let private: std::collections::HashSet<String> = state
            .auth_db
            .list_dataset_graph_entries(dataset_id)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.private)
            .map(|e| e.graph_iri)
            .collect();
        graphs
            .into_iter()
            .filter(|g| !private.contains(g))
            .collect()
    };

    // Fast path: detect "list all non-empty named graphs" queries and answer from the
    // in-memory graph index.  The pattern `GRAPH ?g { ?s ?p ?o }` forces Oxigraph to
    // enumerate every triple in every accessible graph (O(total_triples)), whereas the
    // graph index gives the same answer in O(N_graphs).  This makes the common
    // fetchGraphNames call ~100x faster on large datasets.
    if let Some(graph_var) = detect_graph_listing_query(query) {
        let sorted: Vec<String> = if graphs.is_empty() {
            Vec::new()
        } else {
            let mut filtered: Vec<String> = graphs
                .iter()
                .filter(|iri| {
                    state
                        .store
                        .graph_count_cached(Some(iri.as_str()))
                        .unwrap_or(1)
                        > 0
                })
                .cloned()
                .collect();
            filtered.sort();
            filtered
        };

        let bindings: Vec<serde_json::Value> = sorted
            .iter()
            .map(|iri| serde_json::json!({ &graph_var: { "type": "uri", "value": iri } }))
            .collect();

        let body = serde_json::json!({
            "head": { "vars": [&graph_var] },
            "results": { "bindings": bindings }
        });
        let bytes = serde_json::to_vec(&body).map_err(|e| AppError::Internal(e.to_string()))?;
        return Ok((
            StatusCode::OK,
            [(CONTENT_TYPE, "application/sparql-results+json")],
            bytes,
        )
            .into_response());
    }

    // Re-scope the query to the service's graphs. Any FROM / FROM NAMED clause the caller
    // supplied is intersected with these graphs (see scope_query_to_authorized), so a query
    // cannot name a graph from outside this dataset/service to read it. When the service has
    // no graphs the helper scopes to a non-existent graph, returning empty rather than
    // scanning the entire store.
    let graph_set: std::collections::HashSet<String> = graphs.iter().cloned().collect();
    run_scoped_sparql(state, query, &graph_set, accept).await
}

/// Execute `query` restricted to `graph_set`, streaming the serialised result.
///
/// This is the shared core behind the dataset-service endpoint and the saved-query
/// API: the query is first re-scoped with [`scope_query_to_authorized`] (the read
/// boundary), prefixes are auto-resolved, and the result is streamed through a
/// bounded channel so large CONSTRUCT/DESCRIBE responses are not fully buffered.
/// A query that fails to compile or evaluate surfaces as an `Err(AppError)` before
/// any body bytes are sent, so callers can react (e.g. notify on a broken `latest`).
pub(crate) async fn run_scoped_sparql(
    state: &AppState,
    query: &str,
    graph_set: &std::collections::HashSet<String>,
    accept: &str,
) -> Result<Response, AppError> {
    let effective_query = scope_query_to_authorized(query, graph_set);

    // Auto-resolve prefixes
    let resolved = resolve_prefixes(state, &effective_query).await;
    let final_query_str = resolved.as_deref().unwrap_or(&effective_query).to_string();

    // Stream serialisation through a channel so large CONSTRUCT/DESCRIBE
    // results are not fully buffered before the first byte is sent.
    let store = state.store.clone();
    let accept_owned = accept.to_string();
    let (ct_tx, ct_rx) = oneshot::channel::<Result<&'static str, AppError>>();
    let (chunk_tx, chunk_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(8);

    tokio::task::spawn_blocking(move || {
        let results = match store.query(&final_query_str) {
            Ok(r) => r,
            Err(e) => {
                let _ = ct_tx.send(Err(AppError::from(e)));
                return;
            }
        };

        enum Mode {
            Tabular(ResultFormat),
            Graph(GraphFormat),
        }
        let mode = match &results {
            oxigraph::sparql::QueryResults::Solutions(_)
            | oxigraph::sparql::QueryResults::Boolean(_) => {
                Mode::Tabular(negotiate_result_format(&accept_owned))
            }
            oxigraph::sparql::QueryResults::Graph(_) => {
                Mode::Graph(negotiate_graph_format(&accept_owned))
            }
        };
        let content_type = match mode {
            Mode::Tabular(f) => f.content_type(),
            Mode::Graph(f) => f.content_type(),
        };
        if ct_tx.send(Ok(content_type)).is_err() {
            return;
        }

        let mut writer = ChannelWriter {
            tx: chunk_tx.clone(),
        };
        let result = match mode {
            Mode::Tabular(f) => serialize_results_to(results, f, &mut writer),
            Mode::Graph(f) => serialize_graph_to(results, f, &mut writer),
        };
        if let Err(msg) = result {
            let _ =
                chunk_tx.blocking_send(Err(std::io::Error::new(std::io::ErrorKind::Other, msg)));
        }
    });

    // M-1: bound time-to-first-byte with the configured query timeout, matching the
    // main `/sparql` endpoint. Without this the dataset-service and saved-query
    // `/run` paths (some anonymously reachable) could run pathological queries
    // unbounded.
    let timeout = std::time::Duration::from_secs(state.query_timeout_secs);
    let content_type = tokio::time::timeout(timeout, ct_rx)
        .await
        .map_err(|_| AppError::BadRequest("Query execution timed out".to_string()))?
        .map_err(|_| AppError::Internal("Query task aborted".to_string()))??;

    let body = axum::body::Body::from_stream(receiver_stream(chunk_rx));
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .body(body)
        .unwrap())
}

/// Detect queries whose sole purpose is listing named graphs that contain at least one
/// triple, e.g.:
///   SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p ?o } } ORDER BY ?g
///   select ?g where { graph ?g { ?subject ?pred ?obj } }
///
/// The pattern is: SELECT clause projects exactly one variable, WHERE clause contains a
/// single GRAPH ?<same-var> { ?<v> ?<v> ?<v> } block where the inner triple variables
/// are not projected.  Returns the projected graph variable name (without `?`) when
/// matched, or `None` otherwise.
fn detect_graph_listing_query(query: &str) -> Option<String> {
    let q_upper = query.to_ascii_uppercase();

    // Must be a SELECT query
    let select_pos = q_upper.find("SELECT")?;
    let where_pos = q_upper[select_pos..]
        .find("WHERE")
        .map(|i| select_pos + i)?;

    // Collect projected variables between SELECT and WHERE (uppercased for case-insensitivity)
    let select_clause_upper = query[select_pos + 6..where_pos].to_ascii_uppercase();
    let select_vars_upper = select_clause_upper
        .replace("DISTINCT", "")
        .replace("REDUCED", "")
        .replace("ALL", "");
    let projected: Vec<&str> = select_vars_upper
        .split_whitespace()
        .filter(|t| t.starts_with('?'))
        .collect();
    // Must project exactly one variable (not `*`, not multiple vars)
    if projected.len() != 1 {
        return None;
    }

    // Find "GRAPH" keyword in the WHERE clause
    let where_clause_upper = &q_upper[where_pos..];
    let graph_pos = where_clause_upper.find("GRAPH ")?;
    let after_graph = query[where_pos + graph_pos + 6..].trim_start();

    // Must be followed by a variable (not a named-graph IRI <...>)
    if !after_graph.starts_with('?') {
        return None;
    }

    // Extract the graph variable name (without `?`)
    let var_end = after_graph[1..]
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|i| i + 1)
        .unwrap_or(after_graph.len());
    let graph_var = &after_graph[1..var_end];

    // The projected variable must match the GRAPH variable
    if projected[0] != format!("?{}", graph_var).to_ascii_uppercase() {
        return None;
    }

    // After the graph variable, find the opening `{`
    let after_var = after_graph[var_end..].trim_start();
    if !after_var.starts_with('{') {
        return None;
    }

    // Find the matching closing `}` (simple — reject nested braces)
    let body_content = after_var[1..].trim_start();
    let close_pos = body_content.find('}')?;
    let body = body_content[..close_pos].trim();

    // Body must be empty OR exactly three `?variable` tokens (wildcard triple pattern)
    if !body.is_empty() {
        let tokens: Vec<&str> = body.split_whitespace().collect();
        if tokens.len() != 3 || !tokens.iter().all(|t| t.starts_with('?')) {
            return None;
        }
        // Inner variables must not be projected
        for tok in &tokens {
            if select_clause_upper.contains(tok.to_ascii_uppercase().as_str()) {
                return None;
            }
        }
    }

    Some(graph_var.to_string())
}

/// Byte index of the first top-level `WHERE` keyword (word-boundaried, followed
/// by whitespace or `{`), or `None` when the optional `WHERE` keyword is absent.
///
/// Case-insensitive comparison runs against the original bytes — uppercasing
/// into a separate buffer can change byte length for some Unicode (e.g. `ß` →
/// `SS`) and misalign the returned index against the source string.
fn first_top_level_where(query: &str) -> Option<usize> {
    let b = query.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i + 5 <= n {
        if b[i..i + 5].eq_ignore_ascii_case(b"WHERE") {
            let before_ok = i == 0 || b[i - 1].is_ascii_whitespace();
            let after_ok = i + 5 == n || matches!(b[i + 5], b' ' | b'\t' | b'\n' | b'\r' | b'{');
            if before_ok && after_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Where a `FROM` / `FROM NAMED` dataset clause must be inserted: before the
/// `WHERE` keyword, or — when it is omitted — before the group pattern's opening
/// `{`. Falls back to end-of-query (e.g. a bare `DESCRIBE <iri>`).
fn dataset_clause_anchor(query: &str) -> usize {
    first_top_level_where(query)
        .or_else(|| query.as_bytes().iter().position(|&c| c == b'{'))
        .unwrap_or(query.len())
}

/// Insert server-controlled `from_clauses` at the dataset-clause anchor,
/// preserving any dataset the caller already wrote. Use only for additive,
/// server-owned graphs (e.g. an entailment-regime graph) — never to enforce
/// access scoping, since it does not remove caller-supplied graphs.
fn inject_from_clauses(query: &str, from_clauses: &str) -> String {
    let at = dataset_clause_anchor(query);
    let (head, tail) = query.split_at(at);
    let sep = if head.is_empty() || head.ends_with(char::is_whitespace) {
        ""
    } else {
        "\n"
    };
    format!("{head}{sep}{from_clauses}{tail}")
}

/// Pull every top-level `FROM` / `FROM NAMED <iri>` clause out of `head` (the
/// text before the dataset-clause anchor), returning the requested IRIs and
/// `head` with those clauses removed.
///
/// `FROM` is a reserved keyword that is only legal in the dataset-clause
/// position, so scanning this region cannot misfire on `FROM` appearing inside
/// a triple pattern, filter expression, or string literal.
fn extract_and_strip_dataset(head: &str) -> (Vec<String>, String) {
    let b = head.as_bytes();
    let n = b.len();
    let mut iris = Vec::new();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let boundary_before = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        let is_from = boundary_before
            && i + 4 <= n
            && b[i..i + 4].eq_ignore_ascii_case(b"FROM")
            && (i + 4 == n || b[i + 4].is_ascii_whitespace());
        if is_from {
            let mut j = i + 4;
            while j < n && b[j].is_ascii_whitespace() {
                j += 1;
            }
            // Optional NAMED keyword.
            if j + 5 <= n
                && b[j..j + 5].eq_ignore_ascii_case(b"NAMED")
                && (j + 5 == n || b[j + 5].is_ascii_whitespace())
            {
                j += 5;
                while j < n && b[j].is_ascii_whitespace() {
                    j += 1;
                }
            }
            if j < n && b[j] == b'<' {
                if let Some(rel) = b[j + 1..].iter().position(|&c| c == b'>') {
                    iris.push(head[j + 1..j + 1 + rel].to_string());
                    i = j + 1 + rel + 1; // consume through the closing '>'
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    (
        iris,
        String::from_utf8(out).unwrap_or_else(|_| head.to_string()),
    )
}

/// Re-scope a caller-supplied query so it can only read graphs in `authorized`.
///
/// Any `FROM` / `FROM NAMED` clause the caller wrote is treated as a *request*
/// and intersected with `authorized`; graphs the caller may not read are
/// dropped. A caller that names no dataset is scoped to the full `authorized`
/// set, so a plain `?s ?p ?o` query still sees data held in named graphs
/// (this store keeps all data in named graphs).
///
/// This is the read-access security boundary: it is what stops an
/// unauthenticated or under-privileged caller from naming a private graph in
/// `FROM NAMED <…>` to exfiltrate it. Unlike [`inject_from_clauses`], it removes
/// the caller's dataset before applying the authorized scope.
pub(crate) fn scope_query_to_authorized(
    query: &str,
    authorized: &std::collections::HashSet<String>,
) -> String {
    let anchor = dataset_clause_anchor(query);
    let (head, tail) = query.split_at(anchor);
    let (requested, head_clean) = extract_and_strip_dataset(head);

    let effective: Vec<&str> = if requested.is_empty() {
        authorized.iter().map(String::as_str).collect()
    } else {
        let mut seen = std::collections::HashSet::new();
        requested
            .iter()
            .map(String::as_str)
            .filter(|g| authorized.contains(*g) && seen.insert(*g))
            .collect()
    };

    let mut from_clauses = String::new();
    if effective.is_empty() {
        // Nothing the caller may read — scope to a non-existent graph so the
        // query stays valid but returns no rows (rather than scanning the store).
        from_clauses.push_str("FROM <urn:empty:graph>\nFROM NAMED <urn:empty:graph>\n");
    } else {
        for iri in effective {
            from_clauses.push_str("FROM <");
            from_clauses.push_str(iri);
            from_clauses.push_str(">\nFROM NAMED <");
            from_clauses.push_str(iri);
            from_clauses.push_str(">\n");
        }
    }

    let sep = if head_clean.is_empty() || head_clean.ends_with(char::is_whitespace) {
        ""
    } else {
        "\n"
    };
    format!("{head_clean}{sep}{from_clauses}{tail}")
}

#[cfg(test)]
mod query_scoping_tests {
    use super::{extract_and_strip_dataset, first_top_level_where, scope_query_to_authorized};
    use std::collections::HashSet;

    fn authz(iris: &[&str]) -> HashSet<String> {
        iris.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn finds_where_case_insensitively_at_word_boundary() {
        assert_eq!(
            first_top_level_where("SELECT * WHERE { ?s ?p ?o }"),
            Some(9)
        );
        assert_eq!(first_top_level_where("select * where{?s ?p ?o}"), Some(9));
        // `?whereabouts` must not match.
        assert_eq!(
            first_top_level_where("SELECT ?whereabouts { ?s ?p ?o }"),
            None
        );
    }

    #[test]
    fn strips_caller_from_clauses_and_collects_iris() {
        let (iris, head) =
            extract_and_strip_dataset("SELECT * FROM <urn:g:a> FROM NAMED <urn:g:b> ");
        assert_eq!(iris, vec!["urn:g:a".to_string(), "urn:g:b".to_string()]);
        assert!(
            !head.contains("FROM"),
            "dataset clauses must be removed: {head:?}"
        );
        assert!(head.starts_with("SELECT *"));
    }

    // The core security property: an unauthorized graph named by the caller in
    // FROM NAMED is dropped, so the query cannot read it.
    #[test]
    fn drops_unauthorized_named_graph() {
        let scoped = scope_query_to_authorized(
            "SELECT * FROM NAMED <urn:private:grouch> WHERE { GRAPH ?g { ?s ?p ?o } }",
            &authz(&["urn:public:open"]),
        );
        assert!(
            !scoped.contains("urn:private:grouch"),
            "private graph must not survive scoping: {scoped}"
        );
        // The caller named a dataset, none of it authorized → empty sentinel scope.
        assert!(scoped.contains("urn:empty:graph"), "got: {scoped}");
    }

    #[test]
    fn keeps_authorized_graph_the_caller_requested() {
        let scoped = scope_query_to_authorized(
            "SELECT * FROM <urn:public:open> WHERE { ?s ?p ?o }",
            &authz(&["urn:public:open", "urn:public:other"]),
        );
        assert!(scoped.contains("FROM <urn:public:open>"));
        assert!(scoped.contains("FROM NAMED <urn:public:open>"));
        // Intersection: a graph the caller did NOT ask for is not added back.
        assert!(!scoped.contains("urn:public:other"), "got: {scoped}");
    }

    #[test]
    fn mixed_request_keeps_only_authorized_intersection() {
        let scoped = scope_query_to_authorized(
            "SELECT * FROM <urn:public:open> FROM NAMED <urn:private:grouch> WHERE { ?s ?p ?o }",
            &authz(&["urn:public:open"]),
        );
        assert!(scoped.contains("urn:public:open"));
        assert!(!scoped.contains("urn:private:grouch"), "got: {scoped}");
    }

    #[test]
    fn no_caller_dataset_scopes_to_all_authorized() {
        let scoped =
            scope_query_to_authorized("SELECT * WHERE { ?s ?p ?o }", &authz(&["urn:g:one"]));
        assert!(scoped.contains("FROM <urn:g:one>"));
        assert!(scoped.contains("FROM NAMED <urn:g:one>"));
        assert!(!scoped.contains("urn:empty:graph"));
    }

    #[test]
    fn empty_authorized_yields_empty_sentinel() {
        let scoped = scope_query_to_authorized("SELECT * WHERE { ?s ?p ?o }", &authz(&[]));
        assert!(scoped.contains("urn:empty:graph"), "got: {scoped}");
    }

    #[test]
    fn scopes_query_without_where_keyword() {
        // WHERE is optional; the dataset clause must still land before the group `{`.
        let scoped = scope_query_to_authorized("SELECT * { ?s ?p ?o }", &authz(&["urn:g:one"]));
        let from_pos = scoped.find("FROM <urn:g:one>").expect("FROM injected");
        let brace_pos = scoped.find('{').expect("group present");
        assert!(
            from_pos < brace_pos,
            "FROM must precede the group `{{`: {scoped}"
        );
    }

    #[test]
    fn from_inside_string_literal_is_not_stripped() {
        // A `FROM` substring inside a WHERE-clause literal must be untouched.
        let q = "SELECT * WHERE { ?s ?p \"data FROM <x>\" }";
        let scoped = scope_query_to_authorized(q, &authz(&["urn:g:one"]));
        assert!(
            scoped.contains("\"data FROM <x>\""),
            "literal corrupted: {scoped}"
        );
    }
}

#[cfg(test)]
mod blank_node_resolution_tests {
    use super::resolve_blank_node;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;
    use oxigraph::sparql::QueryResults;

    // The blank-node browse fix assumes a label observed in a SPARQL result can be
    // re-resolved through the low-level quad API — i.e. Oxigraph preserves blank
    // node labels across both APIs. This pins that end-to-end: if it regresses,
    // clicking a blank node would silently open an empty resource page again.
    #[test]
    fn resolves_blank_node_by_sparql_observed_label() {
        let store = TripleStore::in_memory().unwrap();
        let g = "urn:test:bn";
        store
            .graph_store_put(
                Some(g),
                r#"@prefix ex: <http://ex/> .
                   ex:Bridge ex:hasGeometry [ ex:asWKT "POINT(1 2)" ; a ex:Geometry ] ."#,
                RdfFormat::Turtle,
            )
            .unwrap();

        // Capture the blank node's label exactly as SPARQL hands it to the UI.
        let label = match store
            .query(&format!(
                "SELECT ?o WHERE {{ GRAPH <{g}> {{ <http://ex/Bridge> <http://ex/hasGeometry> ?o }} }}"
            ))
            .unwrap()
        {
            QueryResults::Solutions(mut sols) => {
                let sol = sols.next().expect("a solution").expect("ok solution");
                match sol.get("o").expect("bound ?o") {
                    oxigraph::model::Term::BlankNode(bn) => bn.as_str().to_string(),
                    other => panic!("expected a blank node, got {other:?}"),
                }
            }
            _ => panic!("expected SELECT solutions"),
        };

        let (outgoing, _bnodes, incoming) =
            resolve_blank_node(store.store(), &label, &[g.to_string()], 5);

        // The blank node's own properties resolve (detail page isn't empty)…
        assert!(
            outgoing
                .iter()
                .any(|p| p["p"]["value"] == "http://ex/asWKT"),
            "expected ex:asWKT in outgoing, got {outgoing:?}"
        );
        // …and the triple pointing at it is found as an incoming reference.
        assert!(
            incoming
                .iter()
                .any(|p| p["s"]["value"] == "http://ex/Bridge"
                    && p["p"]["value"] == "http://ex/hasGeometry"),
            "expected incoming from ex:Bridge, got {incoming:?}"
        );
    }

    // A graph the caller may not read must contribute nothing, even if it holds a
    // blank node with the requested label (blank-node identity is graph-local).
    #[test]
    fn respects_graph_authorization_scope() {
        let store = TripleStore::in_memory().unwrap();
        store
            .graph_store_put(
                Some("urn:test:secret"),
                r#"<http://ex/s> <http://ex/p> [ <http://ex/q> "hidden" ] ."#,
                RdfFormat::Turtle,
            )
            .unwrap();
        let label = match store
            .query("SELECT ?o WHERE { GRAPH <urn:test:secret> { <http://ex/s> <http://ex/p> ?o } }")
            .unwrap()
        {
            QueryResults::Solutions(mut sols) => {
                match sols.next().unwrap().unwrap().get("o").unwrap() {
                    oxigraph::model::Term::BlankNode(bn) => bn.as_str().to_string(),
                    _ => panic!("expected bnode"),
                }
            }
            _ => panic!("expected solutions"),
        };
        // Scope the scan to an unrelated, empty authorized graph.
        let (outgoing, _b, incoming) =
            resolve_blank_node(store.store(), &label, &["urn:test:other".to_string()], 5);
        assert!(
            outgoing.is_empty(),
            "must not leak across graphs: {outgoing:?}"
        );
        assert!(
            incoming.is_empty(),
            "must not leak across graphs: {incoming:?}"
        );
    }
}

#[cfg(test)]
mod dataset_iri_tests {
    use super::dataset_id_from_iri;

    #[test]
    fn parses_canonical_dataset_iri() {
        let b = "http://localhost:7878";
        assert_eq!(
            dataset_id_from_iri("http://localhost:7878/dataset/spatial", b),
            Some("spatial".into())
        );
        // Trailing slash on the configured base is tolerated.
        assert_eq!(
            dataset_id_from_iri(
                "http://localhost:7878/dataset/bridge-inventory",
                "http://localhost:7878/"
            ),
            Some("bridge-inventory".into())
        );
    }

    #[test]
    fn rejects_non_dataset_iris() {
        let b = "http://localhost:7878";
        // Different base.
        assert_eq!(dataset_id_from_iri("http://elsewhere/dataset/x", b), None);
        // A graph / sub-resource (extra path segment) is not the dataset node.
        assert_eq!(
            dataset_id_from_iri("http://localhost:7878/dataset/spatial/geo", b),
            None
        );
        // The aggregate VoID dataset and empty id are not per-dataset IRIs.
        assert_eq!(
            dataset_id_from_iri("http://localhost:7878/dataset", b),
            None
        );
        assert_eq!(
            dataset_id_from_iri("http://localhost:7878/dataset/", b),
            None
        );
        // A plural REST-ish path must not be mistaken for the singular IRI.
        assert_eq!(
            dataset_id_from_iri("http://localhost:7878/datasets/spatial", b),
            None
        );
    }
}

// ─── Asset helpers ───────────────────────────────────────────────────────────

fn asset_iri(base_url: &str, dataset_id: &str, asset_id: &str) -> String {
    format!("{}/datasets/{}/assets/{}", base_url, dataset_id, asset_id)
}

fn assets_graph_iri(base_url: &str, dataset_id: &str) -> String {
    format!("{}/datasets/{}/assets", base_url, dataset_id)
}

fn sparql_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Write an asset's RDF description into the dataset's assets graph. The triple
/// store is the *authority* for how an asset is modelled: in addition to the DCAT
/// distribution, the node is typed by kind (schema.org class + DCMI Type) and
/// carries kind-specific detail (image dimensions, PDF pages, 3D format, geo
/// bbox/CRS, …) derived from the bytes. See `crate::assets`.
fn insert_asset_triples(
    state: &AppState,
    asset: &crate::auth::models::Asset,
    dataset_id: &str,
    kind: crate::assets::AssetKind,
    meta: &crate::assets::AssetMetadata,
) -> Result<(), String> {
    let iri = asset_iri(&state.base_url, dataset_id, &asset.id);
    let graph = assets_graph_iri(&state.base_url, dataset_id);
    // Link the distribution to the canonical dataset IRI (singular, styleguide §3.3)
    // so the asset attaches to the same dataset node the catalogue/metadata use.
    let dataset_iri_str = format!(
        "{}/dataset/{}",
        state.base_url.trim_end_matches('/'),
        dataset_id
    );
    let display_title = asset.title.as_deref().unwrap_or(&asset.filename);
    let title_escaped = sparql_escape(display_title);
    let media_type = asset
        .content_type
        .split(';')
        .next()
        .unwrap_or(&asset.content_type)
        .trim();
    let media_iri = format!(
        "https://www.iana.org/assignments/media-types/{}",
        media_type
    );

    let desc_triple = if let Some(desc) = &asset.description {
        format!("\n        dct:description \"{}\" ;", sparql_escape(desc))
    } else {
        String::new()
    };
    let modified_triple = if let Some(upd) = &asset.updated_at {
        format!("\n        dct:modified \"{}\"^^xsd:dateTime ;", upd)
    } else {
        String::new()
    };

    // Kind typing: an extra rdf:type (schema.org class, always) + a coarse DCMI Type.
    // Full IRIs (not prefixed) because schema:3DModel is not a legal prefixed name.
    let (schema_class, dcmi_type) = kind.class_iris();
    let type_clause = format!(", <{}>", schema_class);
    let dcmi_triple = match dcmi_type {
        Some(d) => format!("\n        dct:type <{}> ;", d),
        None => String::new(),
    };

    // Per-kind detail predicates.
    let mut detail = String::new();
    if let Some(w) = meta.width {
        detail.push_str(&format!(
            "\n        <http://schema.org/width> \"{}\"^^xsd:integer ;",
            w
        ));
    }
    if let Some(h) = meta.height {
        detail.push_str(&format!(
            "\n        <http://schema.org/height> \"{}\"^^xsd:integer ;",
            h
        ));
    }
    if let Some(p) = meta.pages {
        detail.push_str(&format!(
            "\n        <http://schema.org/numberOfPages> \"{}\"^^xsd:integer ;",
            p
        ));
    }
    if let Some(d) = meta.duration_secs {
        detail.push_str(&format!(
            "\n        <http://schema.org/duration> \"PT{}S\"^^xsd:duration ;",
            d
        ));
    }
    if let Some(fmt) = &meta.format {
        detail.push_str(&format!(
            "\n        <http://schema.org/encodingFormat> \"{}\" ;",
            sparql_escape(fmt)
        ));
    }
    if let Some(pc) = meta.point_count {
        detail.push_str(&format!("\n        <{}pointCount> {} ;", ASSET_NS, pc));
    }
    if let Some(sum) = &meta.checksum_sha256 {
        // Integrity + dedup. SPDX checksum vocabulary (DCAT-AP convention).
        detail.push_str(&format!(
            "\n        <http://spdx.org/rdf/terms#checksum> [ \
                <http://spdx.org/rdf/terms#algorithm> <http://spdx.org/rdf/terms#checksumAlgorithm_sha256> ; \
                <http://spdx.org/rdf/terms#checksumValue> \"{}\" ] ;", sum));
    }
    if let Some(bbox) = &meta.bbox_wkt {
        // GeoSPARQL extended WKT: optional `<crs> ` prefix inside the literal value.
        let lit = match &meta.crs {
            Some(crs) => format!("<{}> {}", crs, bbox),
            None => bbox.clone(),
        };
        detail.push_str(&format!(
            "\n        dcat:bbox \"{}\"^^geo:wktLiteral ;",
            sparql_escape(&lit)
        ));
        if let Some(crs) = &meta.crs {
            detail.push_str(&format!("\n        dct:conformsTo <{}> ;", crs));
        }
    }
    // Archive detail.
    if let Some(n) = meta.entry_count {
        detail.push_str(&format!("\n        <{}entryCount> {} ;", ASSET_NS, n));
    }
    if let Some(sz) = meta.uncompressed_size {
        detail.push_str(&format!(
            "\n        <{}uncompressedSize> {} ;",
            ASSET_NS, sz
        ));
    }
    // Spreadsheet detail.
    if let Some(s) = meta.sheet_count {
        detail.push_str(&format!("\n        <{}sheetCount> {} ;", ASSET_NS, s));
    }
    if let Some(r) = meta.row_count {
        detail.push_str(&format!("\n        <{}rowCount> {} ;", ASSET_NS, r));
    }
    // A 360° equirectangular panorama (schema.org has no dedicated class; flag it).
    if meta.is_panorama {
        detail.push_str(&format!(
            "\n        <{}panorama> true ;\
             \n        <http://schema.org/encodingFormat> \"equirectangular\" ;",
            ASSET_NS
        ));
    }
    // Image capture timestamp (EXIF DateTimeOriginal).
    if let Some(ts) = &meta.captured_at {
        detail.push_str(&format!(
            "\n        <http://schema.org/dateCreated> \"{}\"^^xsd:dateTime ;",
            sparql_escape(ts)
        ));
    }
    // Audio sample rate (Hz).
    if let Some(sr) = meta.sample_rate {
        detail.push_str(&format!("\n        <{}sampleRate> {} ;", ASSET_NS, sr));
    }

    let update = format!(
        r#"PREFIX dcat: <http://www.w3.org/ns/dcat#>
PREFIX dct:  <http://purl.org/dc/terms/>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
INSERT DATA {{
  GRAPH <{graph}> {{
    <{iri}> a dcat:Distribution{type_clause} ;
        dct:title "{title}" ;{desc}{modified}{dcmi}
        dcat:mediaType <{media_iri}> ;
        dcat:byteSize {size} ;
        dct:created "{created}"^^xsd:dateTime ;
        dct:isPartOf <{dataset_iri}> ;{detail}
        dcat:downloadURL <{iri}> .
  }}
}}"#,
        graph = graph,
        iri = iri,
        type_clause = type_clause,
        title = title_escaped,
        desc = desc_triple,
        modified = modified_triple,
        dcmi = dcmi_triple,
        media_iri = media_iri,
        size = asset.size_bytes,
        created = asset.created_at,
        dataset_iri = dataset_iri_str,
        detail = detail,
    );
    // Targeted: only the assets graph is mutated, so skip the full GraphIndex
    // rebuild and re-count just that graph.
    state
        .store
        .update_targeted(&update, &[graph.clone()], false)
        .map_err(|e| e.to_string())
}

/// Re-derive and write an asset's typed RDF by re-reading its bytes from the
/// object store. Used after a metadata edit (which rebuilds the node) so the
/// kind class + detail survive a title/description change.
async fn rebuild_asset_triples(
    state: &AppState,
    asset: &crate::auth::models::Asset,
    dataset_id: &str,
) -> Result<(), String> {
    let (data, _ct) = state
        .object_store
        .download(&asset.s3_key)
        .await
        .map_err(|e| e.to_string())?;
    let kind = crate::assets::classify(&asset.content_type, &asset.filename, &data);
    let meta = crate::assets::extract_for(kind, &data, &asset.content_type, &asset.filename);
    insert_asset_triples(state, asset, dataset_id, kind, &meta)
}

fn remove_dcat_triples(
    state: &AppState,
    asset_iri_str: &str,
    graph_iri: &str,
) -> Result<(), String> {
    let update = format!(
        "DELETE WHERE {{ GRAPH <{}> {{ <{}> ?p ?o . }} }}",
        graph_iri, asset_iri_str
    );
    state
        .store
        .update_targeted(&update, &[graph_iri.to_string()], false)
        .map_err(|e| e.to_string())
}

#[derive(serde::Serialize, utoipa::ToSchema)]
struct AssetResponse {
    #[serde(flatten)]
    asset: crate::auth::models::Asset,
    iri: String,
}

// ─── Asset handlers ──────────────────────────────────────────────────────────

/// Hard ceiling, in bytes, for a single uploaded asset. Mirrors the form service's
/// `Settings.asset_max_bytes` so a body that the form front-door accepts is not then rejected
/// here. Enforced two ways: a streaming read in [`upload_asset`] that aborts past this, and a
/// matching `DefaultBodyLimit` on the asset route (see `server::mod`).
pub const ASSET_MAX_BYTES: usize = 50 * 1024 * 1024;

/// POST /api/datasets/:dataset_id/assets — upload an asset
pub async fn upload_asset(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    if !state.object_store.is_configured() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "S3 storage not configured".to_string(),
        ));
    }

    if let Some(mut field) = multipart
        .next_field()
        .await
        // e.status() maps a body-limit overflow to 413 (an oversized single frame trips the
        // transport limit here, before the per-chunk accumulator below runs); other parse
        // failures keep their 400.
        .map_err(|e| (e.status(), e.to_string()))?
    {
        let _filename = field.name().unwrap_or("file").to_string();
        // SECURITY: sanitize the client-supplied filename to a bare basename before it is used
        // in the storage key or persisted — prevents path traversal (e.g. "../../etc/passwd")
        // on the local-filesystem object store.
        let file_name = crate::assets::sanitize_filename(field.file_name().unwrap_or("unnamed"));
        let declared_type = field
            .content_type()
            .unwrap_or("application/octet-stream")
            .to_string();

        // Bounded read: stream the part and abort the instant it would exceed the asset
        // ceiling, so a hostile multipart body cannot be buffered unbounded in memory.
        // Mirrors the form service's streaming cap; the route also carries a matching
        // DefaultBodyLimit as a transport-level backstop.
        let mut buf: Vec<u8> = Vec::new();
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    if buf.len() + chunk.len() > ASSET_MAX_BYTES {
                        return Err((
                            StatusCode::PAYLOAD_TOO_LARGE,
                            format!("asset exceeds the maximum size of {ASSET_MAX_BYTES} bytes"),
                        ));
                    }
                    buf.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                // A body-limit overflow surfaces here as a length-limit error; e.status() maps
                // that to 413 (PAYLOAD_TOO_LARGE) rather than a misleading 400. This covers the
                // case where the body arrives as one oversized frame and the transport limit
                // trips before the per-chunk accumulator above can.
                Err(e) => return Err((e.status(), format!("Multipart read error: {e}"))),
            }
        }
        let data = Bytes::from(buf);

        // Malware scan (ClamAV INSTREAM) on the fully-buffered bytes, BEFORE anything
        // is persisted. Disabled unless `CLAMAV_ADDR` is set, in which case an empty
        // address ⇒ `Skipped`. Only an explicit `Infected` verdict blocks the upload
        // (422); Skipped/Clean/Error fall through (fail-open on a scanner outage, by
        // design — see `ScanVerdict::allows_storage`). Entirely cfg-gated so non-clamav
        // builds are byte-for-byte unchanged.
        #[cfg(feature = "asset-clamav")]
        {
            let clamd_addr = crate::assets::metadata::clamav_addr();
            let verdict = crate::assets::metadata::scan_clamav(&data, &clamd_addr);
            if !verdict.allows_storage() {
                let signature = match &verdict {
                    crate::assets::metadata::ScanVerdict::Infected(sig) => sig.as_str(),
                    _ => "unknown",
                };
                tracing::warn!(
                    "asset upload rejected by malware scan (dataset {}, file {}): {}",
                    dataset_id,
                    file_name,
                    signature
                );
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("asset rejected by malware scan: {signature}"),
                ));
            }
        }

        // The triple store is the authority for how an asset is modelled: classify
        // by kind from the bytes (a magic-byte sniff overrides a spoofed/missing
        // Content-Type) and extract per-kind detail BEFORE `data` is moved into the
        // object store. The stored content_type is corrected when it was generic.
        let kind = crate::assets::classify(&declared_type, &file_name, &data);
        let asset_meta = crate::assets::extract_for(kind, &data, &declared_type, &file_name);
        let content_type = match crate::assets::sniff_mime(&data) {
            Some(sniffed)
                if declared_type.is_empty() || declared_type == "application/octet-stream" =>
            {
                sniffed
            }
            _ => declared_type,
        };

        let asset_id = uuid::Uuid::new_v4().to_string();
        let s3_key = format!("datasets/{}/{}/{}", dataset_id, asset_id, file_name);
        let size = data.len() as i64;

        // Derive a thumbnail from image bytes BEFORE `data` is moved into the store. Stored as a
        // sibling asset (own IRI, reusing the asset download/linked-data routes) and linked from
        // the parent via schema:thumbnail. Feature-gated; best-effort (never fails the upload).
        #[cfg(feature = "asset-thumbnail")]
        let thumb_png: Option<Vec<u8>> = if kind == crate::assets::AssetKind::Image {
            crate::assets::metadata::make_thumbnail(&data, 256)
        } else {
            None
        };

        state
            .object_store
            .upload(&s3_key, data, &content_type)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let asset = state
            .auth_db
            .create_asset(
                &asset_id,
                &dataset_id,
                &file_name,
                &content_type,
                &s3_key,
                size,
                &current_user.user_id,
                false,
            )
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let graph = assets_graph_iri(&state.base_url, &dataset_id);
        if let Err(e) = insert_asset_triples(&state, &asset, &dataset_id, kind, &asset_meta) {
            tracing::warn!("Asset metadata insert failed for asset {}: {}", asset.id, e);
        }
        // Register the assets named graph in the dataset so it participates in
        // dataset-scoped SPARQL queries and is visible in the graph list.
        if let Err(e) = state.auth_db.add_dataset_graph(&dataset_id, &graph) {
            tracing::warn!(
                "Failed to register assets graph for dataset {}: {}",
                dataset_id,
                e
            );
        }
        let iri = asset_iri(&state.base_url, &dataset_id, &asset.id);

        // Store the derived thumbnail as its own (PNG) asset and link parent → thumbnail.
        #[cfg(feature = "asset-thumbnail")]
        if let Some(png) = thumb_png {
            if let Err(e) = store_thumbnail(
                &state,
                &dataset_id,
                &asset.id,
                &iri,
                png,
                &current_user.user_id,
            )
            .await
            {
                tracing::warn!("thumbnail generation failed for asset {}: {}", asset.id, e);
            }
        }
        return Ok((StatusCode::CREATED, Json(AssetResponse { asset, iri })));
    }

    Err((StatusCode::BAD_REQUEST, "No file uploaded".to_string()))
}

/// Store a derived thumbnail PNG as its own asset and link the parent image to it via
/// `schema:thumbnail`. Best-effort helper for `upload_asset` (feature `asset-thumbnail`).
#[cfg(feature = "asset-thumbnail")]
async fn store_thumbnail(
    state: &AppState,
    dataset_id: &str,
    parent_id: &str,
    parent_iri: &str,
    png: Vec<u8>,
    user_id: &str,
) -> Result<(), String> {
    let thumb_id = uuid::Uuid::new_v4().to_string();
    let filename = format!("{}-thumb.png", parent_id);
    let s3_key = format!("datasets/{}/{}/{}", dataset_id, thumb_id, filename);
    let size = png.len() as i64;
    state
        .object_store
        .upload(&s3_key, Bytes::from(png), "image/png")
        .await
        .map_err(|e| e.to_string())?;
    let thumb = state
        .auth_db
        .create_asset(
            &thumb_id,
            dataset_id,
            &filename,
            "image/png",
            &s3_key,
            size,
            user_id,
            false,
        )
        .map_err(|e| e.to_string())?;
    // Type the thumbnail node (schema:ImageObject + dimensions), then link parent → thumb.
    let kind = crate::assets::AssetKind::Image;
    let (data, _ct) = state
        .object_store
        .download(&s3_key)
        .await
        .map_err(|e| e.to_string())?;
    let meta = crate::assets::extract_for(kind, &data, "image/png", &filename);
    insert_asset_triples(state, &thumb, dataset_id, kind, &meta)?;
    let thumb_iri = asset_iri(&state.base_url, dataset_id, &thumb.id);
    let graph = assets_graph_iri(&state.base_url, dataset_id);
    let update = format!(
        "INSERT DATA {{ GRAPH <{g}> {{ <{p}> <http://schema.org/thumbnail> <{t}> . }} }}",
        g = graph,
        p = parent_iri,
        t = thumb_iri,
    );
    state
        .store
        .update_targeted(&update, &[graph], false)
        .map_err(|e| e.to_string())
}

/// GET /api/datasets/:dataset_id/assets — list assets
pub async fn list_assets(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }

    let assets = state
        .auth_db
        .list_dataset_assets(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let base_url = state.base_url.clone();
    let response: Vec<AssetResponse> = assets
        .into_iter()
        .map(|asset| {
            let iri = asset_iri(&base_url, &dataset_id, &asset.id);
            AssetResponse { asset, iri }
        })
        .collect();
    Ok(Json(response))
}

/// GET /api/datasets/:dataset_id/assets/:asset_id — download an asset
pub async fn download_asset(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((dataset_id, asset_id)): Path<(String, String)>,
) -> Result<Response, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_access_dataset(Some(&current_user.user_id), &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let asset = state
        .auth_db
        .get_asset(&asset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Asset not found".to_string()))?;

    if asset.dataset_id != dataset_id {
        return Err((
            StatusCode::NOT_FOUND,
            "Asset not in this dataset".to_string(),
        ));
    }

    let (data, content_type) = state
        .object_store
        .download(&asset.s3_key)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Active content types (SVG / HTML / XML) can execute script when a browser
    // renders them inline in the app's origin — stored XSS. Serve those as a
    // neutral octet-stream + `attachment` so they download instead of rendering.
    // Inert types (images, PDF, audio, video) keep inline rendering for UX.
    let ct_lower = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let is_active = ct_lower == "image/svg+xml"
        || ct_lower == "text/html"
        || ct_lower == "application/xhtml+xml"
        || ct_lower.ends_with("/xml")
        || ct_lower.ends_with("+xml");
    let (served_ct, disposition) = if is_active {
        (
            "application/octet-stream".to_string(),
            format!("attachment; filename=\"{}\"", asset.filename),
        )
    } else {
        (
            content_type.clone(),
            format!("inline; filename=\"{}\"", asset.filename),
        )
    };

    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, served_ct.as_str()),
            (CONTENT_DISPOSITION, disposition.as_str()),
        ],
        data,
    )
        .into_response())
}

/// DELETE /api/datasets/:dataset_id/assets/:asset_id — delete an asset
pub async fn delete_asset(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((dataset_id, asset_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    let asset = state
        .auth_db
        .get_asset(&asset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Asset not found".to_string()))?;

    if asset.dataset_id != dataset_id {
        return Err((
            StatusCode::NOT_FOUND,
            "Asset not in this dataset".to_string(),
        ));
    }

    state
        .object_store
        .delete(&asset.s3_key)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    state
        .auth_db
        .delete_asset(&asset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let iri = asset_iri(&state.base_url, &dataset_id, &asset_id);
    let graph = assets_graph_iri(&state.base_url, &dataset_id);
    if let Err(e) = remove_dcat_triples(&state, &iri, &graph) {
        tracing::warn!("DCAT remove failed for asset {}: {}", asset_id, e);
    }

    // If no assets remain for this dataset, remove the assets graph from dataset_graphs.
    match state.auth_db.list_dataset_assets(&dataset_id) {
        Ok(remaining) if remaining.is_empty() => {
            if let Err(e) = state.auth_db.remove_dataset_graph(&dataset_id, &graph) {
                tracing::warn!(
                    "Failed to unregister assets graph for dataset {}: {}",
                    dataset_id,
                    e
                );
            }
        }
        _ => {}
    }

    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/datasets/:dataset_id/assets/:asset_id — update asset metadata (title, description)
pub async fn patch_asset_metadata(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((dataset_id, asset_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    let asset = state
        .auth_db
        .get_asset(&asset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Asset not found".to_string()))?;

    if asset.dataset_id != dataset_id {
        return Err((
            StatusCode::NOT_FOUND,
            "Asset not in this dataset".to_string(),
        ));
    }

    let title: Option<&str> = body
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let description: Option<&str> = body
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    let updated = state
        .auth_db
        .update_asset_metadata(&asset_id, title, description)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Rebuild DCAT triples: delete existing, re-insert with new metadata
    let graph = assets_graph_iri(&state.base_url, &dataset_id);
    let iri = asset_iri(&state.base_url, &dataset_id, &asset_id);
    if let Err(e) = remove_dcat_triples(&state, &iri, &graph) {
        tracing::warn!(
            "DCAT remove failed during metadata update for asset {}: {}",
            asset_id,
            e
        );
    }
    if let Err(e) = rebuild_asset_triples(&state, &updated, &dataset_id).await {
        tracing::warn!(
            "Asset metadata insert failed during update for asset {}: {}",
            asset_id,
            e
        );
    }

    let iri = asset_iri(&state.base_url, &dataset_id, &asset_id);
    Ok(Json(AssetResponse {
        asset: updated,
        iri,
    }))
}

/// PUT /api/datasets/:dataset_id/assets/:asset_id/visibility — set asset public/private
pub async fn update_asset_visibility(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((dataset_id, asset_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    let asset = state
        .auth_db
        .get_asset(&asset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Asset not found".to_string()))?;

    if asset.dataset_id != dataset_id {
        return Err((
            StatusCode::NOT_FOUND,
            "Asset not in this dataset".to_string(),
        ));
    }

    let public = body
        .get("public")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "Missing 'public' boolean field".to_string(),
            )
        })?;

    // An asset in a non-public dataset cannot be made publicly accessible.
    if public {
        use crate::auth::models::Visibility;
        if dataset.visibility != Visibility::Public {
            let vis = match dataset.visibility {
                Visibility::Private => "private",
                Visibility::Members => "members-only",
                Visibility::Public => "public",
            };
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!(
                    "Cannot make an asset public in a {} dataset. Make the dataset public first.",
                    vis
                ),
            ));
        }
    }

    state
        .auth_db
        .update_asset_public(&asset_id, public)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/datasets/:dataset_id/assets/:asset_id/metadata — the asset's typed metadata as JSON.
///
/// Returns the kind-specific detail the triplestore derived on upload (dimensions, duration, page/
/// point/entry/sheet/row counts, panorama flag, SHA-256 checksum, geo bbox, thumbnail IRI, …) by
/// querying the asset node's predicates across all graphs. This is the queryable, UI-friendly view
/// of what `linked_data_asset` serves as Turtle. Same access control as the rest of the asset API.
pub async fn asset_metadata(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, asset_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    let asset = state
        .auth_db
        .get_asset(&asset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Asset not found".to_string()))?;
    if asset.dataset_id != dataset_id {
        return Err((
            StatusCode::NOT_FOUND,
            "Asset not in this dataset".to_string(),
        ));
    }
    use crate::auth::models::Visibility;
    let publicly_available = asset.public && dataset.visibility == Visibility::Public;
    if !publicly_available {
        let user_id = user.as_ref().map(|u| u.0.user_id.as_str());
        if !state
            .auth_db
            .can_access_dataset(user_id, &dataset)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Authentication required".to_string(),
            ));
        }
    }

    let iri = asset_iri(&state.base_url, &dataset_id, &asset_id);
    // SELECT the asset node's predicate/object pairs across the default + all named graphs (the
    // assets graph's base_url may differ from the current one, as in linked_data_asset). The third
    // UNION reaches the SHA-256 one hop through the `spdx:checksum [ spdx:checksumValue … ]` blank
    // node and surfaces it as a direct `spdx:checksumValue` pair so the flat matcher below sees it.
    let q = format!(
        "SELECT ?p ?o WHERE {{ \
           {{ <{iri}> ?p ?o }} \
           UNION {{ GRAPH ?g {{ <{iri}> ?p ?o }} }} \
           UNION {{ <{iri}> <{spdx}checksum> ?c . ?c <{spdx}checksumValue> ?o . \
                    BIND(<{spdx}checksumValue> AS ?p) }} \
           UNION {{ GRAPH ?g2 {{ <{iri}> <{spdx}checksum> ?c . ?c <{spdx}checksumValue> ?o . }} \
                    BIND(<{spdx}checksumValue> AS ?p) }} \
         }}",
        iri = iri,
        spdx = "http://spdx.org/rdf/terms#"
    );
    let results = state
        .store
        .query(&q)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    const SCHEMA: &str = "http://schema.org/";
    const SPDX: &str = "http://spdx.org/rdf/terms#";
    const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    const DCAT: &str = "http://www.w3.org/ns/dcat#";

    let mut out = serde_json::Map::new();
    out.insert("iri".into(), serde_json::Value::String(iri.clone()));
    let mut classes: Vec<String> = Vec::new();

    if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
        for sol in solutions.flatten() {
            let (Some(p), Some(o)) = (sol.get("p"), sol.get("o")) else {
                continue;
            };
            let pred = match p {
                Term::NamedNode(n) => n.as_str().to_string(),
                _ => continue,
            };
            // Literal lexical value, or IRI string for named-node objects.
            let lit = match o {
                Term::Literal(l) => l.value().to_string(),
                Term::NamedNode(n) => n.as_str().to_string(),
                _ => continue,
            };
            let as_u64 = || lit.parse::<u64>().ok();
            match pred.as_str() {
                RDF_TYPE if pred_starts(&lit, SCHEMA) || lit == format!("{}3DModel", SCHEMA) => {
                    classes.push(lit.clone());
                }
                _ if pred == format!("{}width", SCHEMA) => {
                    insert_u64(&mut out, "width", as_u64());
                }
                _ if pred == format!("{}height", SCHEMA) => {
                    insert_u64(&mut out, "height", as_u64());
                }
                _ if pred == format!("{}numberOfPages", SCHEMA) => {
                    insert_u64(&mut out, "pages", as_u64());
                }
                _ if pred == format!("{}duration", SCHEMA) => {
                    out.insert("duration".into(), lit.clone().into());
                }
                _ if pred == format!("{}encodingFormat", SCHEMA) => {
                    out.insert("format".into(), lit.clone().into());
                }
                _ if pred == format!("{}thumbnail", SCHEMA) => {
                    out.insert("thumbnail".into(), lit.clone().into());
                }
                _ if pred == format!("{}pointCount", ASSET_NS) => {
                    insert_u64(&mut out, "point_count", as_u64());
                }
                _ if pred == format!("{}entryCount", ASSET_NS) => {
                    insert_u64(&mut out, "entry_count", as_u64());
                }
                _ if pred == format!("{}uncompressedSize", ASSET_NS) => {
                    insert_u64(&mut out, "uncompressed_size", as_u64());
                }
                _ if pred == format!("{}sheetCount", ASSET_NS) => {
                    insert_u64(&mut out, "sheet_count", as_u64());
                }
                _ if pred == format!("{}rowCount", ASSET_NS) => {
                    insert_u64(&mut out, "row_count", as_u64());
                }
                _ if pred == format!("{}panorama", ASSET_NS) => {
                    out.insert("panorama".into(), (lit == "true").into());
                }
                _ if pred == format!("{}sampleRate", ASSET_NS) => {
                    insert_u64(&mut out, "sample_rate", as_u64());
                }
                _ if pred == format!("{}dateCreated", SCHEMA) => {
                    out.insert("captured_at".into(), lit.clone().into());
                }
                _ if pred == format!("{}checksumValue", SPDX) => {
                    out.insert("sha256".into(), lit.clone().into());
                }
                _ if pred == format!("{}bbox", DCAT) => {
                    out.insert("bbox".into(), lit.clone().into());
                }
                _ if pred == format!("{}byteSize", DCAT) => {
                    insert_u64(&mut out, "byte_size", as_u64());
                }
                _ => {}
            }
        }
    }
    if let Some(c) = classes
        .into_iter()
        .find(|c| c != &format!("{}MediaObject", SCHEMA))
    {
        out.insert("schema_class".into(), c.into());
    }
    Ok(Json(serde_json::Value::Object(out)))
}

fn pred_starts(s: &str, prefix: &str) -> bool {
    s.starts_with(prefix)
}

fn insert_u64(map: &mut serde_json::Map<String, serde_json::Value>, key: &str, v: Option<u64>) {
    if let Some(n) = v {
        map.insert(key.to_string(), serde_json::Value::Number(n.into()));
    }
}

/// GET /datasets/:dataset_id/assets/:asset_id — linked data endpoint with content negotiation.
///
/// - RDF Accept types (text/turtle, application/n-triples, etc.) → DCAT description from triplestore
/// - All other Accept types → stream the actual file (inline so browsers can display PDFs, images, etc.)
/// - Public datasets: accessible without authentication
/// - Non-public datasets: authentication required
pub async fn linked_data_asset(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, asset_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    let asset = state
        .auth_db
        .get_asset(&asset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Asset not found".to_string()))?;

    if asset.dataset_id != dataset_id {
        return Err((
            StatusCode::NOT_FOUND,
            "Asset not in this dataset".to_string(),
        ));
    }

    // Access control: public assets on public datasets need no auth.
    // All other cases require the caller to be able to access the dataset.
    use crate::auth::models::Visibility;
    let publicly_available = asset.public && dataset.visibility == Visibility::Public;
    if !publicly_available {
        let user_id = user.as_ref().map(|u| u.0.user_id.as_str());
        if !state
            .auth_db
            .can_access_dataset(user_id, &dataset)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Authentication required".to_string(),
            ));
        }
    }

    let accept = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();

    let wants_rdf = accept.contains("text/turtle")
        || accept.contains("application/x-turtle")
        || accept.contains("application/n-triples")
        || accept.contains("application/rdf+xml")
        || accept.contains("application/ld+json")
        || accept.contains("application/trig")
        || accept.contains("application/n-quads");

    if wants_rdf {
        let graph_fmt = negotiate_graph_format(&accept);
        let asset_iri_str = asset_iri(&state.base_url, &dataset_id, &asset_id);
        // Query across all named graphs so the description is found regardless of
        // which base_url was in effect when the asset was uploaded.
        let sparql = format!(
            "CONSTRUCT {{ <{iri}> ?p ?o }} WHERE {{ \
                {{ <{iri}> ?p ?o }} \
                UNION \
                {{ GRAPH ?g {{ <{iri}> ?p ?o }} }} \
            }}",
            iri = asset_iri_str
        );
        let results = state
            .store
            .query(&sparql)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let body = serialize_graph(results, graph_fmt)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        let link_header = format!("<{}>; rel=\"describedby\"", asset_iri_str);
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, graph_fmt.content_type())
            .header("vary", "Accept")
            .header("link", link_header)
            .body(axum::body::Body::from(body))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    } else {
        let (data, content_type) = state
            .object_store
            .download(&asset.s3_key)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let disposition = format!("attachment; filename=\"{}\"", asset.filename);
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, content_type.as_str())
            .header(CONTENT_DISPOSITION, disposition)
            .header("vary", "Accept")
            .body(axum::body::Body::from(data))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
    }
}

// ─── SHACL handlers ──────────────────────────────────────────────────────────

/// POST /api/datasets/:dataset_id/validate — validate dataset against SHACL shapes
///
/// Uses the dataset's own shapes graph if configured, otherwise falls back to
/// SHACL shapes found in the linked ontology version graph.
pub async fn validate_dataset(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Query(q): Query<ValidateQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Bound concurrent expensive operations (SHACL validation) — see expensive_semaphore.
    let _permit = state.expensive_semaphore.acquire().await.map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Server overloaded".to_string(),
        )
    })?;
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_access_dataset(Some(&current_user.user_id), &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    // Resolve shapes graph: dataset-specific shapes, or from linked ontology version
    let shapes_graph_iri = if let Some(ref iri) = dataset.shapes_graph_iri {
        iri.clone()
    } else if let (Some(ref onto_id), Some(ref onto_ver)) =
        (&dataset.conforms_to_ontology, &dataset.conforms_to_version)
    {
        return Err((StatusCode::BAD_REQUEST, format!(
            "Linked ontology {onto_id} version {onto_ver}: ontology registry is no longer supported. Configure a shapes_graph_iri on the dataset instead."
        )));
    } else {
        return Err((
            StatusCode::BAD_REQUEST,
            "No shapes graph configured and no ontology linked".to_string(),
        ));
    };

    // Get data graphs for this dataset
    let data_graphs = state
        .auth_db
        .list_dataset_graphs(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Run SHACL validation
    let report = crate::shacl::validate(&state.store, &shapes_graph_iri, &data_graphs)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // A test run validates but records nothing — no run row, so it doesn't
    // count officially and the dataset's stored status is left unchanged.
    if q.test.unwrap_or(false) {
        return Ok(Json(serde_json::json!({
            "report": report,
            "run_id": null,
            "ran_at": null,
            "test": true,
        })));
    }

    // Best-effort private usage telemetry. Only real (non-test) runs count.
    let _ =
        state
            .auth_db
            .record_dataset_usage(&dataset_id, Some(&current_user.user_id), "validate");

    // Persist this run so status + history survive reloads.
    let summary = persist_validation_run(
        &state,
        &dataset_id,
        &report,
        Some(current_user.user_id.as_str()),
    )
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "report": report,
        "run_id": summary.id,
        "ran_at": summary.run_timestamp,
    })))
}

/// Query params for `validate_dataset`. `test=true` runs a non-persisted
/// (unofficial) validation.
#[derive(Debug, Deserialize)]
pub struct ValidateQuery {
    pub test: Option<bool>,
}

/// Count severities, serialize the report, and store a validation run.
fn persist_validation_run(
    state: &AppState,
    dataset_id: &str,
    report: &crate::shacl::report::ValidationReport,
    triggered_by: Option<&str>,
) -> anyhow::Result<crate::auth::models::ShaclRunSummary> {
    use crate::shacl::report::Severity;
    let mut violation_count = 0i64;
    let mut warning_count = 0i64;
    let mut info_count = 0i64;
    for r in &report.results {
        match r.severity {
            Severity::Violation => violation_count += 1,
            Severity::Warning => warning_count += 1,
            Severity::Info => info_count += 1,
        }
    }
    let report_json = serde_json::to_string(report)?;
    state.auth_db.insert_validation_run(
        dataset_id,
        report.conforms,
        report.results_count as i64,
        violation_count,
        warning_count,
        info_count,
        &report_json,
        triggered_by,
    )
}

/// GET /api/datasets/:dataset_id/validation/latest — latest stored run (full report) or null
pub async fn get_latest_validation_run(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state
        .auth_db
        .can_access_dataset(Some(&current_user.user_id), &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }
    let run = state
        .auth_db
        .get_latest_validation_run(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(run))
}

/// GET /api/datasets/:dataset_id/validation/history?limit=N — run summaries, newest first
pub async fn get_validation_history(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state
        .auth_db
        .can_access_dataset(Some(&current_user.user_id), &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50)
        .clamp(1, 50);
    let runs = state
        .auth_db
        .list_validation_run_summaries(&dataset_id, limit)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(runs))
}

/// GET /api/datasets/:dataset_id/validation/runs/:run_id — one stored run (full report)
pub async fn get_validation_run(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((dataset_id, run_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state
        .auth_db
        .can_access_dataset(Some(&current_user.user_id), &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }
    let run = state
        .auth_db
        .get_validation_run(&run_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .filter(|r| r.dataset_id == dataset_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Run not found".to_string()))?;
    Ok(Json(run))
}

#[derive(Deserialize)]
pub struct LatestRunsRequest {
    pub dataset_ids: Vec<String>,
}

/// POST /api/shacl/validation/latest — latest run summary per requested dataset
pub async fn list_latest_validation_runs(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(req): Json<LatestRunsRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Only include datasets the caller may access.
    let mut accessible: Vec<String> = Vec::new();
    for id in &req.dataset_ids {
        if let Some(ds) = state
            .auth_db
            .get_dataset(id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        {
            if state
                .auth_db
                .can_access_dataset(Some(&current_user.user_id), &ds)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            {
                accessible.push(id.clone());
            }
        }
    }
    let runs = state
        .auth_db
        .list_latest_run_summaries(&accessible)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(runs))
}

/// GET /api/datasets/:dataset_id/shapes — get shapes graph
///
/// Supports `Accept: text/shaclc` or `?format=shaclc` to return SHACLC compact syntax.
/// Default is Turtle.
pub async fn get_shapes(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Query(fmt_params): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_access_dataset(Some(&current_user.user_id), &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let shapes_iri = dataset.shapes_graph_iri.as_deref().ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "No shapes graph configured".to_string(),
        )
    })?;

    // Detect SHACLC format request via query param or Accept header
    let want_shaclc = fmt_params
        .get("format")
        .map(|v| v == "shaclc")
        .unwrap_or(false)
        || headers
            .get(ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(|a| a.contains("text/shaclc"))
            .unwrap_or(false);

    if want_shaclc {
        let shaclc = crate::shaclc::serialize(&state.store, shapes_iri)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
        return Ok((StatusCode::OK, [(CONTENT_TYPE, "text/shaclc")], shaclc).into_response());
    }

    let data = state
        .store
        .graph_store_get(Some(shapes_iri), oxigraph::io::RdfFormat::Turtle)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], data).into_response())
}

/// PUT /api/datasets/:dataset_id/shapes — upload shapes graph
pub async fn put_shapes(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    _headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    // Create a shapes graph IRI if not yet set
    let shapes_iri = dataset
        .shapes_graph_iri
        .clone()
        .unwrap_or_else(|| format!("urn:dataset:{}:shapes", dataset_id));

    let raw = String::from_utf8(body.to_vec())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid UTF-8".to_string()))?;

    // Detect SHACLC input via Content-Type
    let content_type = _headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");
    let data = if content_type.contains("shaclc") {
        crate::shaclc::parse(&raw)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("SHACLC parse error: {e}")))?
    } else {
        raw
    };

    state
        .store
        .graph_store_put(Some(&shapes_iri), &data, oxigraph::io::RdfFormat::Turtle)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Update dataset to point to shapes graph
    state
        .auth_db
        .update_dataset_shacl(&dataset_id, dataset.shacl_on_write, Some(&shapes_iri))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /api/datasets/:dataset_id/infer — run SHACL rules to materialize inferred triples
pub async fn infer_dataset(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    // Bound concurrent expensive operations (SHACL inference) — see expensive_semaphore.
    let _permit = state.expensive_semaphore.acquire().await.map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Server overloaded".to_string(),
        )
    })?;
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    let shapes_graph_iri = dataset.shapes_graph_iri.as_deref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "No shapes graph configured".to_string(),
        )
    })?;

    let data_graphs = state
        .auth_db
        .list_dataset_graphs(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let count = crate::shacl::infer(&state.store, shapes_graph_iri, &data_graphs)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "inferred_triples": count,
    })))
}

// ─── SHACLC standalone endpoints ─────────────────────────────────────────────

/// POST /api/shaclc/parse — convert SHACLC text → Turtle
///
/// Body: SHACLC text (Content-Type: text/shaclc or text/plain)
/// Response: Turtle (Content-Type: text/turtle)
pub async fn shaclc_parse(body: Bytes) -> Result<Response, (StatusCode, String)> {
    let input = String::from_utf8(body.to_vec())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid UTF-8".to_string()))?;
    let turtle = crate::shaclc::parse(&input).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], turtle).into_response())
}

/// POST /api/shaclc/serialize — convert a shapes graph (by IRI) from the store → SHACLC
///
/// Body: JSON `{"shapesGraphIri": "urn:..."}` or the IRI directly as plain text
pub async fn shaclc_serialize(
    State(state): State<AppState>,
    body: Bytes,
) -> Result<Response, (StatusCode, String)> {
    let body_str = String::from_utf8(body.to_vec())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid UTF-8".to_string()))?;

    // Accept both plain IRI text and JSON {"shapesGraphIri": "..."}
    let shapes_iri = if body_str.trim().starts_with('{') {
        let v: serde_json::Value = serde_json::from_str(&body_str)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid JSON: {e}")))?;
        v["shapesGraphIri"]
            .as_str()
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "Missing shapesGraphIri field".to_string(),
                )
            })?
            .to_string()
    } else {
        body_str.trim().to_string()
    };

    let shaclc = crate::shaclc::serialize(&state.store, &shapes_iri)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/shaclc")], shaclc).into_response())
}

// ─── RML endpoints ───────────────────────────────────────────────────────────

/// PUT /api/datasets/:dataset_id/mappings — store an RML mapping document
pub async fn put_rml_mapping(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    body: Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    let mapping_turtle = String::from_utf8(body.to_vec())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid UTF-8".to_string()))?;

    // Validate that it parses as a valid RML mapping
    crate::rml::parse_rml(&mapping_turtle)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid RML mapping: {e}")))?;

    // Store in a named graph: urn:dataset:{id}:rml-mappings
    let mapping_graph = format!("urn:dataset:{}:rml-mappings", dataset_id);
    state
        .store
        .graph_store_put(
            Some(&mapping_graph),
            &mapping_turtle,
            oxigraph::io::RdfFormat::Turtle,
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Register the mapping graph in dataset_graphs
    let _ = state.auth_db.add_dataset_graph(&dataset_id, &mapping_graph);

    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/datasets/:dataset_id/mappings — retrieve the stored RML mapping
pub async fn get_rml_mapping(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_access_dataset(Some(&current_user.user_id), &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let mapping_graph = format!("urn:dataset:{}:rml-mappings", dataset_id);
    let data = state
        .store
        .graph_store_get(Some(&mapping_graph), oxigraph::io::RdfFormat::Turtle)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if data.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            "No RML mapping stored for this dataset".to_string(),
        ));
    }

    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], data).into_response())
}

/// POST /api/datasets/:dataset_id/mappings/execute
///
/// Execute the stored RML mapping with source file(s) provided as multipart.
/// Multipart parts:
///   - Optional: `mapping` (text/turtle) — override the stored mapping
///   - One or more: named parts whose name matches the rml:source value
///
/// Query params:
/// - `?preview=true` — return generated triples without persisting
/// - `?graph=<iri>` — override target named graph (default: dataset default graph)
pub async fn execute_rml_mapping(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    mut multipart: Multipart,
) -> Result<Response, (StatusCode, String)> {
    // Bound concurrent expensive operations (RML execution) — see expensive_semaphore.
    let _permit = state.expensive_semaphore.acquire().await.map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Server overloaded".to_string(),
        )
    })?;
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_write_dataset(&current_user.user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }

    let preview = params.get("preview").map(|v| v == "true").unwrap_or(false);
    let target_graph = params
        .get("graph")
        .cloned()
        .unwrap_or_else(|| format!("urn:dataset:{}:rml-output", dataset_id));

    // Cross-tenant write boundary (fail fast, before any work). `can_write_dataset`
    // only authorizes writing *into this dataset*, not which graph the RML output
    // targets. A non-admin may therefore only write the dataset's own namespaced
    // graphs: gate the `?graph=` target here, and every `rml:graphMap` override at
    // execution. Without this a writer of any dataset could inject triples into
    // another tenant's graph. Admins are unrestricted.
    if !current_user.is_admin() {
        if let Err(msg) = crate::auth::dataset_graph::authorize_dataset_graph_target(
            &state.auth_db,
            &state.base_url,
            &dataset_id,
            &target_graph,
        ) {
            state.audit.log_denied(
                Some(current_user.user_id.clone()),
                None,
                "dataset_graph",
                &dataset_id,
                "rml_execute",
                None,
            );
            return Err((StatusCode::FORBIDDEN, msg));
        }
    }

    // Parse multipart: collect mapping override and source files
    let mut mapping_turtle_override: Option<String> = None;
    let mut source_data: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        let bytes = field.bytes().await.map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Multipart read error: {e}"),
            )
        })?;
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|_| (StatusCode::BAD_REQUEST, "Non-UTF-8 field".to_string()))?;

        if name == "mapping" {
            mapping_turtle_override = Some(text);
        } else {
            source_data.insert(name, text);
        }
    }

    // Load the mapping: override or stored
    let mapping_turtle = if let Some(override_turtle) = mapping_turtle_override {
        override_turtle
    } else {
        let mapping_graph = format!("urn:dataset:{}:rml-mappings", dataset_id);
        let data = state
            .store
            .graph_store_get(Some(&mapping_graph), oxigraph::io::RdfFormat::Turtle)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if data.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "No RML mapping stored. Upload one first or provide it as the 'mapping' part."
                    .to_string(),
            ));
        }
        String::from_utf8(data).map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Stored mapping is not valid UTF-8".to_string(),
            )
        })?
    };

    let mapping = crate::rml::parse_rml(&mapping_turtle)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid RML mapping: {e}")))?;

    if preview {
        // Execute into a temporary in-memory store and return the triples
        let temp = crate::store::TripleStore::in_memory().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Temp store error: {e}"),
            )
        })?;
        let count = crate::rml::execute(&mapping, &source_data, &temp, Some(&target_graph))
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        let turtle_bytes = temp
            .dump(oxigraph::io::RdfFormat::Turtle, Some(&target_graph))
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let turtle = String::from_utf8(turtle_bytes).unwrap_or_default();
        return Ok(Json(serde_json::json!({
            "preview": true,
            "triples_count": count,
            "turtle": turtle,
        }))
        .into_response());
    }

    // Execute into the real store, enforcing the same boundary on every effective
    // (graphMap-overridden) destination graph — a mapping's `rml:graphMap` can name
    // a target other than `?graph=`, so the gate must cover the resolved set too.
    let is_admin = current_user.is_admin();
    let authz_base = state.base_url.clone();
    let authz_db = state.auth_db.clone();
    let authz_ds = dataset_id.clone();
    let count = crate::rml::execute_authorized(
        &mapping,
        &source_data,
        &state.store,
        Some(&target_graph),
        move |g: &str| {
            if is_admin {
                Ok(())
            } else {
                crate::auth::dataset_graph::authorize_dataset_graph_target(
                    &authz_db,
                    &authz_base,
                    &authz_ds,
                    g,
                )
            }
        },
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Register target graph in dataset
    let _ = state.auth_db.add_dataset_graph(&dataset_id, &target_graph);

    Ok(Json(serde_json::json!({
        "triples_inserted": count,
        "target_graph": target_graph,
    }))
    .into_response())
}

/// POST /api/rml/preview — dry-run RML mapping without persisting
///
/// Multipart: `mapping` (Turtle) + named source file parts.
pub async fn rml_preview(mut multipart: Multipart) -> Result<Response, (StatusCode, String)> {
    let mut mapping_turtle: Option<String> = None;
    let mut source_data: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        let bytes = field.bytes().await.map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Multipart read error: {e}"),
            )
        })?;
        let text = String::from_utf8(bytes.to_vec())
            .map_err(|_| (StatusCode::BAD_REQUEST, "Non-UTF-8 field".to_string()))?;
        if name == "mapping" {
            mapping_turtle = Some(text);
        } else {
            source_data.insert(name, text);
        }
    }

    let turtle = mapping_turtle.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "Missing 'mapping' part".to_string(),
        )
    })?;

    let mapping = crate::rml::parse_rml(&turtle)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid RML mapping: {e}")))?;

    let temp = crate::store::TripleStore::in_memory().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Temp store error: {e}"),
        )
    })?;
    let count = crate::rml::execute(&mapping, &source_data, &temp, None)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let turtle_bytes = temp
        .dump(oxigraph::io::RdfFormat::Turtle, None)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "triples_count": count,
        "turtle": String::from_utf8(turtle_bytes).unwrap_or_default(),
    }))
    .into_response())
}

// ─── Helper functions ────────────────────────────────────────────────────────

/// Format a SPARQL term for JSON output.
fn format_term(term: &Term) -> serde_json::Value {
    use oxigraph::model::Term as ModelTerm;
    match term {
        ModelTerm::NamedNode(nn) => serde_json::json!({
            "type": "uri",
            "value": nn.as_str(),
        }),
        ModelTerm::BlankNode(bn) => serde_json::json!({
            "type": "bnode",
            "value": bn.as_str(),
        }),
        ModelTerm::Literal(lit) => {
            let mut obj = serde_json::json!({
                "type": "literal",
                "value": lit.value(),
            });
            if let Some(lang) = lit.language() {
                obj["xml:lang"] = serde_json::json!(lang);
                obj["language"] = serde_json::json!(lang);
            }
            let dt = lit.datatype();
            if dt.as_str() != "http://www.w3.org/2001/XMLSchema#string" {
                obj["datatype"] = serde_json::json!(dt.as_str());
            }
            obj
        }
        #[cfg(feature = "rdf-12")]
        ModelTerm::Triple(t) => crate::sparql::rdf12_functions::triple_term_to_json(t),
        #[cfg(not(feature = "rdf-12"))]
        _ => serde_json::json!({"type": "unknown", "value": term.to_string()}),
    }
}

fn format_sparql_results_as_triples(
    results: oxigraph::sparql::QueryResults,
) -> Result<Vec<serde_json::Value>, AppError> {
    if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
        let triples: Vec<serde_json::Value> = solutions
            .filter_map(|s| s.ok())
            .map(|solution| {
                let s = solution
                    .get("s")
                    .map(format_term)
                    .unwrap_or(serde_json::Value::Null);
                let p = solution
                    .get("p")
                    .map(format_term)
                    .unwrap_or(serde_json::Value::Null);
                let o = solution
                    .get("o")
                    .map(format_term)
                    .unwrap_or(serde_json::Value::Null);
                let g = solution
                    .get("g")
                    .map(format_term)
                    .unwrap_or(serde_json::Value::Null);
                serde_json::json!({ "subject": s, "predicate": p, "object": o, "graph": g })
            })
            .collect();
        Ok(triples)
    } else {
        Err(AppError::Internal("Expected SELECT results".to_string()))
    }
}

/// Build a query that returns a resource's own triples plus the triples of every
/// blank node reachable from it through a chain of blank nodes (a bounded
/// Concise Bounded Description). All rows are projected as `?s ?p ?o`, where `?s`
/// is the resource IRI for its own triples and a blank node for nested ones.
///
/// Everything is gathered in a single query on purpose: SPARQL blank-node labels
/// are only stable within one result set, so the only way to correlate "the
/// bnode I saw as an object" with "that bnode's properties" is to read them
/// together. `max_depth` bounds how many blank-node hops we follow (cycles and
/// long RDF lists are therefore naturally truncated rather than looping).
fn build_resource_closure_query(iri: &str, graph: Option<&str>, max_depth: usize) -> String {
    let mut branches: Vec<String> = Vec::new();
    // Depth 0 — the resource's own triples.
    branches.push(format!("{{ <{iri}> ?p ?o . BIND(<{iri}> AS ?s) }}"));
    // Depth d — a chain of d blank-node hops ending at the described bnode `?s`.
    for d in 1..=max_depth {
        let mut pat = format!("<{iri}> ?cp0 ");
        for k in 1..d {
            pat.push_str(&format!("?cb{k} . FILTER(isBlank(?cb{k})) ?cb{k} ?cp{k} "));
        }
        pat.push_str("?s . FILTER(isBlank(?s)) ?s ?p ?o .");
        branches.push(format!("{{ {pat} }}"));
    }
    let body = branches.join("\n  UNION ");
    let where_body = match graph {
        Some(g) => format!("GRAPH <{g}> {{\n  {body}\n}}"),
        None => body,
    };
    format!("SELECT DISTINCT ?s ?p ?o WHERE {{\n  {where_body}\n}} ORDER BY ?s ?p ?o")
}

/// Split the rows of a `build_resource_closure_query` result into the resource's
/// own outgoing `{p, o}` pairs and a map of blank-node id → its `{p, o}` pairs.
fn split_resource_closure(
    results: oxigraph::sparql::QueryResults,
) -> Result<
    (
        Vec<serde_json::Value>,
        serde_json::Map<String, serde_json::Value>,
    ),
    AppError,
> {
    use oxigraph::model::Term;
    let mut outgoing: Vec<serde_json::Value> = Vec::new();
    let mut bnodes: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
        for sol in solutions.filter_map(|s| s.ok()) {
            let p = sol
                .get("p")
                .map(format_term)
                .unwrap_or(serde_json::Value::Null);
            let o = sol
                .get("o")
                .map(format_term)
                .unwrap_or(serde_json::Value::Null);
            let pair = serde_json::json!({ "p": p, "o": o });
            match sol.get("s") {
                Some(Term::BlankNode(bn)) => {
                    bnodes
                        .entry(bn.as_str().to_string())
                        .or_insert_with(|| serde_json::Value::Array(Vec::new()))
                        .as_array_mut()
                        .expect("bnode closure entry is always an array")
                        .push(pair);
                }
                Some(_) => outgoing.push(pair),
                None => {}
            }
        }
        Ok((outgoing, bnodes))
    } else {
        Err(AppError::Internal("Expected SELECT results".to_string()))
    }
}

/// Extract the dataset id from a canonical dataset IRI `{base}/dataset/{id}`
/// (styleguide §3.3). Returns `None` when `iri` is not a dataset IRI — a
/// different base, or extra path segments (a sub-resource), or empty id.
fn dataset_id_from_iri(iri: &str, base_url: &str) -> Option<String> {
    let prefix = format!("{}/dataset/", base_url.trim_end_matches('/'));
    let id = iri.strip_prefix(&prefix)?;
    if id.is_empty() || id.contains('/') {
        return None;
    }
    Some(id.to_string())
}

/// Resolve a blank-node subject (addressed by its stored label) through the
/// low-level quad API, because SPARQL cannot reference a stored blank node by
/// label. Oxigraph preserves blank-node labels in storage, so the label the UI
/// captured from an earlier browse is the same label stored here.
///
/// Returns the same `(outgoing, bnodes, incoming)` shape the named-resource path
/// produces: the requested node's own `{p, o}` pairs, a map of nested blank-node
/// label → its `{p, o}` pairs (a bounded description, so geometry/contact chains
/// still expand inline), and `{s, p}` pairs for triples pointing at it. The scan
/// is restricted to `graphs` (the caller's authorized set) so blank-node identity
/// stays graph-local and access control is respected.
fn resolve_blank_node(
    store: &oxigraph::store::Store,
    label: &str,
    graphs: &[String],
    max_depth: usize,
) -> (
    Vec<serde_json::Value>,
    serde_json::Map<String, serde_json::Value>,
    Vec<serde_json::Value>,
) {
    use oxigraph::model::{BlankNode, GraphNameRef, NamedNode, SubjectRef, Term, TermRef};
    use std::collections::{HashSet, VecDeque};

    let mut outgoing: Vec<serde_json::Value> = Vec::new();
    let mut bnodes: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    let mut incoming: Vec<serde_json::Value> = Vec::new();

    // Authorized graphs to scan (skip any unparsable IRI defensively).
    let graph_nodes: Vec<NamedNode> = graphs
        .iter()
        .filter_map(|g| NamedNode::new(g).ok())
        .collect();
    if graph_nodes.is_empty() {
        return (outgoing, bnodes, incoming);
    }

    // BFS over blank-node hops. Depth 0 is the requested node (its pairs are the
    // resource's own `outgoing`); deeper hops are nested anonymous nodes keyed in
    // `bnodes`. `seen` guards cycles and `max_depth` bounds the description.
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    seen.insert(label.to_string());
    queue.push_back((label.to_string(), 0));

    while let Some((bn_label, depth)) = queue.pop_front() {
        let bn = match BlankNode::new(&bn_label) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mut pairs: Vec<serde_json::Value> = Vec::new();
        for g in &graph_nodes {
            let subj = SubjectRef::BlankNode(bn.as_ref());
            for quad in store.quads_for_pattern(
                Some(subj),
                None,
                None,
                Some(GraphNameRef::NamedNode(g.as_ref())),
            ) {
                let quad = match quad {
                    Ok(q) => q,
                    Err(_) => continue,
                };
                // Follow nested blank nodes so the inline expander gets the closure.
                if let Term::BlankNode(child) = &quad.object {
                    let cl = child.as_str().to_string();
                    if depth < max_depth && !seen.contains(&cl) {
                        seen.insert(cl.clone());
                        queue.push_back((cl, depth + 1));
                    }
                }
                let p = format_term(&Term::from(quad.predicate));
                let o = format_term(&quad.object);
                pairs.push(serde_json::json!({ "p": p, "o": o }));
            }
        }
        if depth == 0 {
            outgoing = pairs;
        } else {
            bnodes
                .entry(bn_label)
                .or_insert_with(|| serde_json::Value::Array(pairs));
        }
    }

    // Incoming: subjects that reference the requested blank node as an object.
    if let Ok(target) = BlankNode::new(label) {
        for g in &graph_nodes {
            let obj = TermRef::BlankNode(target.as_ref());
            for quad in store.quads_for_pattern(
                None,
                None,
                Some(obj),
                Some(GraphNameRef::NamedNode(g.as_ref())),
            ) {
                let quad = match quad {
                    Ok(q) => q,
                    Err(_) => continue,
                };
                let s = format_term(&Term::from(quad.subject));
                let p = format_term(&Term::from(quad.predicate));
                incoming.push(serde_json::json!({ "s": s, "p": p }));
            }
        }
    }

    (outgoing, bnodes, incoming)
}

fn format_sparql_results_as_pairs(
    results: oxigraph::sparql::QueryResults,
    key1: &str,
    key2: &str,
) -> Result<Vec<serde_json::Value>, AppError> {
    if let oxigraph::sparql::QueryResults::Solutions(solutions) = results {
        let pairs: Vec<serde_json::Value> = solutions
            .filter_map(|s| s.ok())
            .map(|solution| {
                let v1 = solution
                    .get(key1)
                    .map(format_term)
                    .unwrap_or(serde_json::Value::Null);
                let v2 = solution
                    .get(key2)
                    .map(format_term)
                    .unwrap_or(serde_json::Value::Null);
                serde_json::json!({ key1: v1, key2: v2 })
            })
            .collect();
        Ok(pairs)
    } else {
        Err(AppError::Internal("Expected SELECT results".to_string()))
    }
}

// ─── Reasoning API ────────────────────────────────────────────────────────────

pub fn reasoning_routes() -> Router<AppState> {
    Router::new()
        .route("/api/reasoning/materialize", post(reasoning_materialize))
        .route("/api/reasoning/status", get(reasoning_status))
        .route("/api/reasoning/rewrite", post(reasoning_rewrite))
        .route("/api/text-search/reindex", post(text_search_reindex))
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
struct MaterializeRequest {
    regime: String,
    source_graphs: Option<Vec<String>>,
    target_graph: Option<String>,
}

/// POST /api/reasoning/materialize — run an entailment regime.
async fn reasoning_materialize(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<MaterializeRequest>,
) -> Result<Response, AppError> {
    let target = body
        .target_graph
        .unwrap_or_else(|| match body.regime.as_str() {
            "rdfs" => crate::reasoning::common::RDFS_ENTAILMENT_GRAPH.to_string(),
            "owl2-rl" => crate::reasoning::common::OWL2_RL_ENTAILMENT_GRAPH.to_string(),
            "owl2-el" => crate::reasoning::common::OWL2_EL_ENTAILMENT_GRAPH.to_string(),
            "owl2-ql" => crate::reasoning::common::OWL2_QL_ENTAILMENT_GRAPH.to_string(),
            "owl2-dl" => crate::reasoning::common::OWL2_DL_ENTAILMENT_GRAPH.to_string(),
            _ => format!("urn:entailment:{}", body.regime),
        });

    // Materialization writes derived triples into `target` — previously with NO
    // authorization, so any authenticated caller could write arbitrary (incl.
    // shared entailment / other tenants') graphs. Require write permission on the
    // target graph (admins bypass; server-owned entailment graphs therefore need
    // an explicit grant or admin).
    require_graph_write(&state, Some(&user), Some(target.as_str()))?;

    // Bound concurrent expensive operations so a burst of reasoning calls can't
    // occupy every Tokio worker and starve the runtime (held until handler return).
    let _permit = state
        .expensive_semaphore
        .acquire()
        .await
        .map_err(|_| AppError::Internal("Server overloaded".to_string()))?;
    // Extract source_graphs unconditionally so the struct field is always read.
    let _sources = body.source_graphs.unwrap_or_default();
    // Silence unused-variable warnings for the case where no reasoning feature is
    // compiled in (only the `_ => Err(...)` arm fires, leaving state/target unused).
    let _ = (&state, &target);

    // Match returns Some(report) for a recognised regime or None for an unknown one.
    // Both branches are always present in the match so no unreachable-code warning fires.
    let report: Option<crate::reasoning::ReasoningReport> = match body.regime.as_str() {
        #[cfg(feature = "rdfs-entailment")]
        "rdfs" => {
            let m = crate::reasoning::rdfs::RdfsMaterializer::with_target(&state.store, &target);
            Some(
                m.materialize()
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )
        }
        #[cfg(feature = "owl2-rl")]
        "owl2-rl" => {
            let m =
                crate::reasoning::owl2_rl::Owl2RLReasoner::new(&state.store).with_target(&target);
            Some(
                m.materialize()
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )
        }
        #[cfg(feature = "owl2-el")]
        "owl2-el" => {
            let m =
                crate::reasoning::owl2_el::El2Classifier::new(&state.store).with_target(&target);
            Some(
                m.classify()
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )
        }
        #[cfg(feature = "owl2-ql")]
        "owl2-ql" => {
            let rw = crate::reasoning::owl2_ql::QLQueryRewriter::new(&state.store);
            Some(
                rw.materialize_tbox()
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )
        }
        #[cfg(feature = "owl2-dl")]
        "owl2-dl" => {
            use crate::reasoning::owl2_dl::{ExternalReasonerBridge, NativeTableauStub};
            let bridge = ExternalReasonerBridge::new(Box::new(NativeTableauStub));
            Some(
                bridge
                    .materialize(&state.store, &_sources, &target)
                    .map_err(|e| AppError::Internal(e.to_string()))?,
            )
        }
        _ => None,
    };

    match report {
        Some(r) => Ok((
            StatusCode::OK,
            Json(serde_json::json!({
                "regime": r.regime,
                "triples_added": r.triples_added,
                "iterations": r.iterations,
                "elapsed_ms": r.elapsed_ms,
                "target_graph": r.target_graph,
            })),
        )
            .into_response()),
        None => Err(AppError::BadRequest(format!(
            "Unknown reasoning regime: {}",
            body.regime
        ))),
    }
}

/// GET /api/reasoning/status — counts of entailed triples per graph.
async fn reasoning_status(State(state): State<AppState>) -> Result<Response, AppError> {
    let graphs = [
        crate::reasoning::common::RDFS_ENTAILMENT_GRAPH,
        crate::reasoning::common::OWL2_RL_ENTAILMENT_GRAPH,
        crate::reasoning::common::OWL2_EL_ENTAILMENT_GRAPH,
        crate::reasoning::common::OWL2_QL_ENTAILMENT_GRAPH,
        crate::reasoning::common::OWL2_DL_ENTAILMENT_GRAPH,
    ];
    let mut result = serde_json::json!({});
    for g in &graphs {
        let count = crate::reasoning::common::count_graph(&state.store, g).unwrap_or(0);
        result[*g] = serde_json::json!(count);
    }
    Ok((StatusCode::OK, Json(result)).into_response())
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
struct RewriteRequest {
    query: String,
    regime: Option<String>,
}

/// POST /api/reasoning/rewrite — debug endpoint: return the rewritten query.
async fn reasoning_rewrite(
    State(state): State<AppState>,
    Json(body): Json<RewriteRequest>,
) -> Result<Response, AppError> {
    // Silence unused-variable warning when no reasoning features are compiled in.
    let _ = &state;
    let regime = body.regime.as_deref().unwrap_or("owl2-ql");
    let rewritten = match regime {
        #[cfg(feature = "owl2-ql")]
        "owl2-ql" => {
            let rw = crate::reasoning::owl2_ql::QLQueryRewriter::new(&state.store);
            rw.rewrite_query(&body.query)
                .map_err(|e| AppError::Internal(e.to_string()))?
        }
        _ => body.query.clone(),
    };
    Ok((
        StatusCode::OK,
        Json(serde_json::json!({ "rewritten": rewritten })),
    )
        .into_response())
}

/// POST /api/text-search/reindex — rebuild the Tantivy index from the store.
///
/// A full reindex is a global, store-wide, expensive operation, so it is
/// restricted to admins (previously any authenticated caller could trigger it,
/// an easy CPU/IO DoS lever).
async fn text_search_reindex(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Response, AppError> {
    if !user.is_admin() {
        return Err(AppError::Forbidden(
            "Admin access required to reindex".to_string(),
        ));
    }
    // Suppress unused-variable warning when text-search feature is disabled.
    let _ = &state;
    #[cfg(feature = "text-search")]
    {
        if let Some(ref idx) = state.text_index {
            let count = idx
                .reindex_from_store(&state.store)
                .map_err(|e| AppError::Internal(e.to_string()))?;
            return Ok((
                StatusCode::OK,
                Json(serde_json::json!({ "indexed": count })),
            )
                .into_response());
        }
    }
    Ok((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "text-search feature not enabled or index not available"
        })),
    )
        .into_response())
}

// ─── ShEx validation routes ─────────────────────────────────────────────

/// ShEx validation routes (auth required).
#[cfg(feature = "shex")]
pub fn shex_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/datasets/:dataset_id/shex/validate",
            post(shex_validate),
        )
        .route("/api/shex/validate", post(shex_validate_inline))
}

#[cfg(feature = "shex")]
#[derive(Debug, Deserialize)]
struct ShExValidateRequest {
    /// ShExC schema text
    schema: String,
    /// Shape map: shape IRI → list of focus node IRIs
    #[serde(default)]
    shape_map: std::collections::HashMap<String, Vec<String>>,
}

/// POST /api/datasets/:dataset_id/shex/validate — validate dataset using ShEx
#[cfg(feature = "shex")]
async fn shex_validate(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
    Json(body): Json<ShExValidateRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;

    if !state
        .auth_db
        .can_access_dataset(Some(&current_user.user_id), &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }

    let schema =
        crate::shex::parse_shexc(&body.schema).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let report = crate::shex::validate(&state.store, &schema, &body.shape_map);
    Ok(Json(report))
}

/// POST /api/shex/validate — validate inline (no dataset context)
#[cfg(feature = "shex")]
async fn shex_validate_inline(
    State(state): State<AppState>,
    Json(body): Json<ShExValidateRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let schema =
        crate::shex::parse_shexc(&body.schema).map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let report = crate::shex::validate(&state.store, &schema, &body.shape_map);
    Ok(Json(report))
}

// ─── SWRL rule execution routes ─────────────────────────────────────────

/// SWRL rule execution routes (auth required).
#[cfg(feature = "swrl")]
pub fn swrl_routes() -> Router<AppState> {
    Router::new().route("/api/swrl/execute", post(swrl_execute))
}

#[cfg(feature = "swrl")]
#[derive(Debug, Deserialize)]
struct SwrlExecuteRequest {
    /// SWRL rules in text format (simple) or XML (OWL)
    rules: String,
    /// Format: "text" (default) or "xml"
    #[serde(default = "default_swrl_format")]
    format: String,
    /// Maximum fixed-point iterations (default: 100)
    #[serde(default = "default_max_iterations")]
    max_iterations: usize,
    /// Target named graph for inferred triples
    target_graph: Option<String>,
}

#[cfg(feature = "swrl")]
fn default_swrl_format() -> String {
    "text".to_string()
}

#[cfg(feature = "swrl")]
fn default_max_iterations() -> usize {
    100
}

/// POST /api/swrl/execute — parse and execute SWRL rules
#[cfg(feature = "swrl")]
async fn swrl_execute(
    State(state): State<AppState>,
    Json(body): Json<SwrlExecuteRequest>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let rules = match body.format.as_str() {
        "xml" => crate::swrl::parser::parse_swrl(&body.rules)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?,
        _ => crate::swrl::parser::parse_swrl_text(&body.rules)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?,
    };

    if rules.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No valid rules found".to_string()));
    }

    let max_iter = body.max_iterations.min(1000);
    let result =
        crate::swrl::execute_rules(&state.store, &rules, max_iter, body.target_graph.as_deref())
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(result))
}

// ─── SHACL detect-shapes ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DetectShapesParams {
    pub graph: String,
}

/// GET /api/shacl/detect-shapes?graph=<iri>
///
/// Checks whether the named graph identified by `graph` contains any SHACL
/// NodeShape or PropertyShape declarations.  Also returns the count of shapes
/// found and a list of datasets whose `shapes_graph_iri` is not yet set — these
/// are candidates the caller can offer to link.
pub async fn detect_shapes(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Query(params): Query<DetectShapesParams>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let graph_iri = &params.graph;

    // Count SHACL shapes in the graph
    let count_query = format!(
        "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph_iri}> {{ \
         {{ ?s a <http://www.w3.org/ns/shacl#NodeShape> }} \
         UNION {{ ?s a <http://www.w3.org/ns/shacl#PropertyShape> }} }} }}"
    );
    let shape_count: usize = if let Ok(oxigraph::sparql::QueryResults::Solutions(mut sols)) =
        state.store.query(count_query.as_str())
    {
        sols.next()
            .and_then(|r| r.ok())
            .and_then(|s| s.get("n").map(|v| v.to_string()))
            .and_then(|s| {
                // Oxigraph returns typed literals like `"5"^^<...integer>`
                s.trim_matches('"')
                    .split('"')
                    .next()
                    .and_then(|n| n.parse::<usize>().ok())
            })
            .unwrap_or(0)
    } else {
        0
    };

    let shapes_detected = shape_count > 0;

    // Return datasets accessible to this user that have no shapes graph yet —
    // these are the candidates to which the detected shapes could be linked.
    let accessible_datasets = state
        .auth_db
        .list_accessible_datasets(Some(&current_user.user_id))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let suggested: Vec<serde_json::Value> = accessible_datasets
        .into_iter()
        .filter(|d| d.shapes_graph_iri.is_none())
        .map(|d| serde_json::json!({ "id": d.id, "name": d.name }))
        .collect();

    Ok(Json(serde_json::json!({
        "shapes_detected": shapes_detected,
        "shape_count": shape_count,
        "suggested_datasets": suggested,
    })))
}

/// GET /api/shacl/shape-graphs
///
/// Returns all datasets accessible to the current user that have a
/// `shapes_graph_iri` configured.  This powers the shape-graph selector in the
/// Validation page so users only see shapes for data they can access.
pub async fn list_accessible_shape_graphs(
    Extension(current_user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let datasets = state
        .auth_db
        .list_accessible_datasets(Some(&current_user.user_id))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let shape_graphs: Vec<serde_json::Value> = datasets
        .into_iter()
        .filter_map(|d| {
            d.shapes_graph_iri.clone().map(|iri| {
                serde_json::json!({
                    "dataset_id": d.id,
                    "dataset_name": d.name,
                    "shapes_graph_iri": iri,
                })
            })
        })
        .collect();

    Ok(Json(serde_json::json!({ "shape_graphs": shape_graphs })))
}
