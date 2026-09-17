//! QLever as a read backend (P5): a QLever instance kept current from the
//! change log answers the analytical queries, configurable and on by
//! default once a URL is set.
//!
//! Two halves. The **feeder** is a consumer of this node's own change log
//! (the same rows a replication follower reads): `full` rows become
//! `DELETE DATA` / `INSERT DATA`, rows that only say a graph changed —
//! `counts`, `unknown`, store-scoped, and rows with blank nodes, whose
//! labels do not survive a round trip — become a graph replace from the
//! local store, an epoch change replaces everything, and the feeder
//! bookmarks its position as the cursor `qlever` on the log, so retention
//! waits for it. The **router** sends a query to QLever only while the
//! feeder is caught up (its cursor is the log's newest sequence number and
//! no write is in flight) — the mirror's never-stale rule — and only for
//! the shapes the route policy names: `analytical` (an `ASK`, or a query
//! with an aggregate anywhere in it — the default), `all` (every `SELECT`/`ASK`
//! after the in-memory copies), `first` (before the copies; the setting for
//! a head-to-head measurement), `off`. Anything else, and any error, falls
//! through to the next exit.
//!
//! QLever's index is built from a dump (`qlever index`); the feeder assumes
//! an index that already holds this store's data and keeps it current
//! through SPARQL Update. `docs/operations.md` has the set-up.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use opengraph::parallel::ParClass;
use oxigraph::model::{GraphName, GraphNameRef, NamedNodeRef, Quad};
use oxigraph::sparql::results::{
    QueryResultsFormat, QueryResultsParser, ReaderQueryResultsParserOutput,
};
use oxigraph::sparql::{QueryResults, QuerySolution, QuerySolutionIter};

use crate::store::changes::{self, Extent, State};
use crate::store::engine::TripleStore;

// ─── Configuration ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// An `ASK`, or a query with an aggregate anywhere in it (`GROUP BY`,
    /// `COUNT`, `SUM`, … — whether or not the shards can decompose it),
    /// after the in-memory copies. The default.
    Analytical,
    /// Every `SELECT` / `ASK`, after the in-memory copies.
    All,
    /// Every `SELECT` / `ASK`, before the in-memory copies (measurements).
    First,
    Off,
}

#[derive(Clone, Debug)]
pub struct QleverConfig {
    pub url: Option<String>,
    pub enabled: bool,
    pub access_token: Option<String>,
    pub route: Route,
    /// Quads per `INSERT DATA` / `DELETE DATA` statement.
    pub batch: usize,
    pub poll: Duration,
    pub timeout: Duration,
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl QleverConfig {
    /// `OTS_QLEVER_URL`, `OTS_QLEVER_ENABLED` (default on), `OTS_QLEVER_ACCESS_TOKEN`,
    /// `OTS_QLEVER_ROUTE` (`analytical` | `all` | `first` | `off`),
    /// `OTS_QLEVER_BATCH` (5000), `OTS_QLEVER_POLL_MS` (500), `OTS_QLEVER_TIMEOUT_SECS` (30).
    pub fn from_env() -> Self {
        Self::parse(
            env_opt("OTS_QLEVER_URL").as_deref(),
            env_opt("OTS_QLEVER_ENABLED").as_deref(),
            env_opt("OTS_QLEVER_ACCESS_TOKEN").as_deref(),
            env_opt("OTS_QLEVER_ROUTE").as_deref(),
            env_opt("OTS_QLEVER_BATCH").as_deref(),
            env_opt("OTS_QLEVER_POLL_MS").as_deref(),
            env_opt("OTS_QLEVER_TIMEOUT_SECS").as_deref(),
        )
    }

    pub fn parse(
        url: Option<&str>,
        enabled: Option<&str>,
        token: Option<&str>,
        route: Option<&str>,
        batch: Option<&str>,
        poll_ms: Option<&str>,
        timeout_secs: Option<&str>,
    ) -> Self {
        let enabled = !enabled
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off" | "no"
                )
            })
            .unwrap_or(false);
        let route = match route.map(|r| r.trim().to_ascii_lowercase()).as_deref() {
            Some("all") => Route::All,
            Some("first") => Route::First,
            Some("off") | Some("none") => Route::Off,
            _ => Route::Analytical,
        };
        Self {
            url: url.map(|u| u.trim_end_matches('/').to_string()),
            enabled,
            access_token: token.map(str::to_string),
            route,
            batch: batch
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(5000)
                .clamp(100, 100_000),
            poll: Duration::from_millis(
                poll_ms
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(500)
                    .clamp(50, 60_000),
            ),
            timeout: Duration::from_secs(
                timeout_secs
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(30)
                    .clamp(1, 600),
            ),
        }
    }

    pub fn none() -> Self {
        Self::parse(None, None, None, None, None, None, None)
    }
}

// ─── The endpoint ───────────────────────────────────────────────────────────

/// A SPARQL 1.1 Protocol endpoint that also takes updates: QLever, or a
/// stand-in in tests.
pub trait QleverEndpoint: Send + Sync {
    fn query(&self, sparql: &str) -> Result<QueryResults<'static>, String>;
    fn update(&self, sparql: &str) -> Result<(), String>;
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("qlever runtime")
    })
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(concat!(
                "open-triplestore-qlever/",
                env!("CARGO_PKG_VERSION")
            ))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest client")
    })
}

fn blocking<F, T>(fut: F) -> T
where
    F: std::future::Future<Output = T> + Send,
    T: Send,
{
    std::thread::scope(|s| {
        s.spawn(|| runtime().block_on(fut))
            .join()
            .expect("qlever request thread")
    })
}

/// QLever over HTTP: queries as `application/sparql-query`, updates as
/// `application/sparql-update` with the access token.
pub struct HttpQlever {
    url: String,
    token: Option<String>,
    timeout: Duration,
}

impl HttpQlever {
    pub fn new(url: &str, token: Option<&str>, timeout: Duration) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            token: token.map(str::to_string),
            timeout,
        }
    }

    fn post(&self, content_type: &str, accept: &str, body: String) -> Result<Vec<u8>, String> {
        let url = self.url.clone();
        let token = self.token.clone();
        let timeout = self.timeout;
        blocking(async move {
            let mut req = client()
                .post(&url)
                .timeout(timeout)
                .header("Content-Type", content_type)
                .header("Accept", accept)
                .body(body);
            if let Some(t) = &token {
                req = req.bearer_auth(t);
            }
            let resp = req.send().await.map_err(|e| format!("{url}: {e}"))?;
            let status = resp.status();
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| format!("{url}: {e}"))?
                .to_vec();
            if !status.is_success() {
                let text = String::from_utf8_lossy(&bytes);
                return Err(format!(
                    "{url}: HTTP {} {}",
                    status.as_u16(),
                    text.chars().take(300).collect::<String>()
                ));
            }
            Ok(bytes)
        })
    }
}

impl QleverEndpoint for HttpQlever {
    fn query(&self, sparql: &str) -> Result<QueryResults<'static>, String> {
        let bytes = self.post(
            "application/sparql-query",
            "application/sparql-results+json",
            sparql.to_string(),
        )?;
        parse_results_json(&bytes)
    }

    fn update(&self, sparql: &str) -> Result<(), String> {
        self.post(
            "application/sparql-update",
            "application/json",
            sparql.to_string(),
        )
        .map(|_| ())
    }
}

/// SPARQL JSON results into owned `QueryResults`.
pub fn parse_results_json(bytes: &[u8]) -> Result<QueryResults<'static>, String> {
    let parsed = QueryResultsParser::from_format(QueryResultsFormat::Json)
        .for_reader(bytes)
        .map_err(|e| e.to_string())?;
    match parsed {
        ReaderQueryResultsParserOutput::Boolean(b) => Ok(QueryResults::Boolean(b)),
        ReaderQueryResultsParserOutput::Solutions(solutions) => {
            let vars: Arc<[oxigraph::model::Variable]> = Arc::from(solutions.variables().to_vec());
            let mut rows: Vec<QuerySolution> = Vec::new();
            for s in solutions {
                rows.push(s.map_err(|e| e.to_string())?);
            }
            let row_vars = vars.clone();
            let iter = rows.into_iter().map(move |s| {
                let values: Vec<Option<oxigraph::model::Term>> = row_vars
                    .iter()
                    .map(|v| s.get(v.as_str()).cloned())
                    .collect();
                Ok(QuerySolution::from((row_vars.clone(), values)))
            });
            Ok(QueryResults::Solutions(QuerySolutionIter::new(vars, iter)))
        }
    }
}

/// A stand-in for tests: an in-memory store that takes the same updates
/// and answers the same queries. Capture is off on it — it is a mirror.
#[cfg(any(test, feature = "test-utils"))]
#[allow(dead_code)]
pub struct FakeQlever {
    pub store: TripleStore,
}

#[cfg(any(test, feature = "test-utils"))]
#[allow(dead_code)]
impl FakeQlever {
    pub fn new() -> Self {
        Self {
            store: TripleStore::in_memory()
                .expect("in-memory store")
                .with_change_capture_disabled()
                .with_parallel_query(false, 1, 0),
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Default for FakeQlever {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl QleverEndpoint for FakeQlever {
    fn query(&self, sparql: &str) -> Result<QueryResults<'static>, String> {
        self.store.query(sparql).map_err(|e| e.to_string())
    }

    fn update(&self, sparql: &str) -> Result<(), String> {
        self.store.update(sparql).map_err(|e| e.to_string())
    }
}

// ─── The feeder and the router ──────────────────────────────────────────────

#[derive(Default)]
struct FeedState {
    epoch: Option<String>,
    applied_seq: i64,
    last_sync_at: Option<String>,
    last_error: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Status {
    pub configured: bool,
    pub enabled: bool,
    pub url: Option<String>,
    pub route: Option<Route>,
    pub epoch: Option<String>,
    pub applied_seq: i64,
    pub caught_up: bool,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    pub applied_rows: u64,
    pub replaced_graphs: u64,
    pub resyncs: u64,
    pub queries_served: u64,
    pub queries_failed: u64,
}

/// What one feeding round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub resyncs: usize,
    pub applied_rows: usize,
    pub replaced_graphs: usize,
    pub applied_seq: i64,
}

pub struct Qlever {
    config: QleverConfig,
    endpoint: Option<Arc<dyn QleverEndpoint>>,
    state: Mutex<FeedState>,
    applied_rows: AtomicU64,
    replaced_graphs: AtomicU64,
    resyncs: AtomicU64,
    queries_served: AtomicU64,
    queries_failed: AtomicU64,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

const CURSOR: &str = "qlever";

impl Qlever {
    pub fn from_env() -> Self {
        let config = QleverConfig::from_env();
        let endpoint: Option<Arc<dyn QleverEndpoint>> = match (&config.url, config.enabled) {
            (Some(url), true) => Some(Arc::new(HttpQlever::new(
                url,
                config.access_token.as_deref(),
                config.timeout,
            ))),
            _ => None,
        };
        Self::with_endpoint(config, endpoint)
    }

    pub fn with_endpoint(config: QleverConfig, endpoint: Option<Arc<dyn QleverEndpoint>>) -> Self {
        Self {
            config,
            endpoint,
            state: Mutex::new(FeedState::default()),
            applied_rows: AtomicU64::new(0),
            replaced_graphs: AtomicU64::new(0),
            resyncs: AtomicU64::new(0),
            queries_served: AtomicU64::new(0),
            queries_failed: AtomicU64::new(0),
        }
    }

    pub fn disabled() -> Self {
        Self::with_endpoint(QleverConfig::none(), None)
    }

    pub fn config(&self) -> &QleverConfig {
        &self.config
    }

    /// An endpoint is set, the switch is on, and the route is not `off`.
    pub fn active(&self) -> bool {
        self.endpoint.is_some() && self.config.enabled && self.config.route != Route::Off
    }

    /// The feeder is exactly at the log's newest sequence number, on the
    /// log's epoch, with nothing in flight.
    pub fn caught_up(&self, store: &TripleStore) -> bool {
        let log = store.changes();
        if !log.enabled() {
            return false;
        }
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.epoch.as_deref() == Some(log.epoch())
            && s.applied_seq == log.last_seq()
            && store.writes_in_flight() == 0
    }

    pub fn status(&self, store: &TripleStore) -> Status {
        let caught_up = self.active() && self.caught_up(store);
        let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        Status {
            configured: self.endpoint.is_some(),
            enabled: self.config.enabled,
            url: self.config.url.clone(),
            route: self.endpoint.as_ref().map(|_| self.config.route),
            epoch: s.epoch.clone(),
            applied_seq: s.applied_seq,
            caught_up,
            last_sync_at: s.last_sync_at.clone(),
            last_error: s.last_error.clone(),
            applied_rows: self.applied_rows.load(Ordering::Relaxed),
            replaced_graphs: self.replaced_graphs.load(Ordering::Relaxed),
            resyncs: self.resyncs.load(Ordering::Relaxed),
            queries_served: self.queries_served.load(Ordering::Relaxed),
            queries_failed: self.queries_failed.load(Ordering::Relaxed),
        }
    }

    /// Route `sparql` to QLever if the policy names it and the feeder is
    /// caught up. `before_copies` says which of the two consultation points
    /// this is.
    pub fn try_query(
        &self,
        store: &TripleStore,
        sparql: &str,
        class: Option<ParClass>,
        before_copies: bool,
    ) -> Option<QueryResults<'static>> {
        let endpoint = self.endpoint.as_ref()?;
        if !self.active() {
            return None;
        }
        let wanted = match self.config.route {
            Route::Off => false,
            Route::First => before_copies,
            Route::All => !before_copies,
            Route::Analytical => {
                !before_copies && (class == Some(ParClass::Aggregate) || is_aggregate_query(sparql))
            }
        };
        if !wanted || !is_select_or_ask(sparql) || !self.caught_up(store) {
            return None;
        }
        match endpoint.query(sparql) {
            Ok(r) => {
                self.queries_served.fetch_add(1, Ordering::Relaxed);
                Some(r)
            }
            Err(e) => {
                self.queries_failed.fetch_add(1, Ordering::Relaxed);
                self.state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .last_error = Some(e);
                None
            }
        }
    }

    /// One feeding round: adopt the log's epoch (replacing everything on a
    /// change), then apply the rows after the bookmark, page by page.
    pub fn feed_once(&self, store: &TripleStore) -> Result<Progress, String> {
        let outcome = self.feed_inner(store);
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.last_sync_at = Some(now());
        match &outcome {
            Ok(_) => s.last_error = None,
            Err(e) => s.last_error = Some(e.clone()),
        }
        outcome
    }

    fn feed_inner(&self, store: &TripleStore) -> Result<Progress, String> {
        let endpoint = self
            .endpoint
            .as_ref()
            .ok_or_else(|| "no QLever endpoint configured".to_string())?;
        let log = store.changes();
        if !log.enabled() {
            return Err("change capture is off; the QLever feeder needs it".to_string());
        }
        let mut progress = Progress::default();
        let epoch = log.epoch().to_string();
        let mut after = {
            let s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            if s.epoch.as_deref() == Some(epoch.as_str()) {
                Some(s.applied_seq)
            } else {
                None
            }
        };
        if after.is_none() {
            // A fresh or re-epoched store: everything, whole.
            let newest = log.last_seq();
            let replaced = self.resync_all(store, endpoint.as_ref())?;
            progress.resyncs += 1;
            progress.replaced_graphs += replaced;
            self.resyncs.fetch_add(1, Ordering::Relaxed);
            self.bookmark(&epoch, newest, log);
            after = Some(newest);
        }
        let mut position = after.unwrap_or(0);
        loop {
            let page = log.rows_after(position, 500);
            let n = page.len();
            for row in &page {
                let Some(seq) = row.seq else { continue };
                if row.state == State::Aborted || row.state == State::Pending {
                    continue;
                }
                if row.scope == "store" {
                    progress.replaced_graphs += self.resync_all(store, endpoint.as_ref())?;
                    progress.resyncs += 1;
                    self.resyncs.fetch_add(1, Ordering::Relaxed);
                } else if row.extent == Extent::Full
                    && row.state == State::Committed
                    && !row.has_bnode
                {
                    let added = changes::decode_text(row.added.as_deref());
                    let removed = changes::decode_text(row.removed.as_deref());
                    self.apply_delta(endpoint.as_ref(), &removed, &added)?;
                    progress.applied_rows += 1;
                    self.applied_rows.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.replace_graph(store, endpoint.as_ref(), row.graph_iri.as_deref())?;
                    progress.replaced_graphs += 1;
                    self.replaced_graphs.fetch_add(1, Ordering::Relaxed);
                }
                position = seq;
            }
            if n > 0 {
                self.bookmark(&epoch, position, log);
            }
            if n < 500 {
                break;
            }
        }
        progress.applied_seq = position;
        Ok(progress)
    }

    fn bookmark(&self, epoch: &str, seq: i64, log: &changes::ChangeLog) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        s.epoch = Some(epoch.to_string());
        s.applied_seq = seq;
        drop(s);
        let _ = log.set_cursor(CURSOR, seq, Some("qlever feeder"));
    }

    fn resync_all(
        &self,
        store: &TripleStore,
        endpoint: &dyn QleverEndpoint,
    ) -> Result<usize, String> {
        let graphs: Vec<Option<String>> =
            store.graph_counts().into_iter().map(|(g, _)| g).collect();
        for g in &graphs {
            self.replace_graph(store, endpoint, g.as_deref())?;
        }
        Ok(graphs.len())
    }

    /// Replace one graph on the endpoint with its local contents.
    fn replace_graph(
        &self,
        store: &TripleStore,
        endpoint: &dyn QleverEndpoint,
        graph: Option<&str>,
    ) -> Result<(), String> {
        let name = match graph {
            Some(iri) => {
                GraphNameRef::NamedNode(NamedNodeRef::new(iri).map_err(|e| e.to_string())?)
            }
            None => GraphNameRef::DefaultGraph,
        };
        let quads = store.quads_for_graph(name).map_err(|e| e.to_string())?;
        let clear = match graph {
            Some(iri) => format!("DELETE WHERE {{ GRAPH <{iri}> {{ ?s ?p ?o }} }}"),
            None => "DELETE WHERE { ?s ?p ?o }".to_string(),
        };
        endpoint.update(&clear)?;
        for chunk in quads.chunks(self.config.batch) {
            endpoint.update(&format!("INSERT DATA {{ {} }}", quads_as_sparql(chunk)))?;
        }
        Ok(())
    }

    fn apply_delta(
        &self,
        endpoint: &dyn QleverEndpoint,
        removed: &[Quad],
        added: &[Quad],
    ) -> Result<(), String> {
        for chunk in removed.chunks(self.config.batch) {
            endpoint.update(&format!("DELETE DATA {{ {} }}", quads_as_sparql(chunk)))?;
        }
        for chunk in added.chunks(self.config.batch) {
            endpoint.update(&format!("INSERT DATA {{ {} }}", quads_as_sparql(chunk)))?;
        }
        Ok(())
    }
}

/// Quads as the body of `INSERT DATA` / `DELETE DATA`: bare triples for the
/// default graph, `GRAPH <g> { … }` blocks otherwise.
fn quads_as_sparql(quads: &[Quad]) -> String {
    use std::collections::BTreeMap;
    let mut by_graph: BTreeMap<Option<String>, Vec<&Quad>> = BTreeMap::new();
    for q in quads {
        let g = match &q.graph_name {
            GraphName::DefaultGraph => None,
            GraphName::NamedNode(n) => Some(n.as_str().to_string()),
            GraphName::BlankNode(b) => Some(format!("_:{}", b.as_str())),
        };
        by_graph.entry(g).or_default().push(q);
    }
    let mut out = String::new();
    for (g, qs) in by_graph {
        if let Some(g) = &g {
            out.push_str(&format!("GRAPH <{g}> {{ "));
        }
        for q in qs {
            out.push_str(&format!("{} {} {} . ", q.subject, q.predicate, q.object));
        }
        if g.is_some() {
            out.push_str("} ");
        }
    }
    out
}

fn is_select_or_ask(sparql: &str) -> bool {
    let upper = sparql.to_ascii_uppercase();
    let body = upper.trim_start();
    // Skip PREFIX / BASE declarations.
    let mut rest = body;
    loop {
        let t = rest.trim_start();
        if t.starts_with("PREFIX") || t.starts_with("BASE") {
            match t.find('>') {
                Some(i) => rest = &t[i + 1..],
                None => return false,
            }
        } else {
            return t.starts_with("SELECT") || t.starts_with("ASK");
        }
    }
}

/// Start the feeder thread when the environment configures QLever. Called
/// once by the store constructors.
pub(crate) fn spawn_feeder_if_configured(store: &TripleStore) {
    let q = store.qlever();
    if q.endpoint.is_none() || !q.config.enabled {
        return;
    }
    let poll = q.config.poll;
    let store = store.clone();
    let spawned = std::thread::Builder::new()
        .name("qlever-feeder".into())
        .spawn(move || loop {
            match store.qlever().feed_once(&store) {
                Ok(p) if p.applied_rows > 0 || p.replaced_graphs > 0 => tracing::info!(
                    "qlever: seq {} ({} rows, {} graphs replaced, {} resyncs)",
                    p.applied_seq,
                    p.applied_rows,
                    p.replaced_graphs,
                    p.resyncs
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!("qlever: feed failed: {e}"),
            }
            std::thread::sleep(poll);
        });
    if let Err(e) = spawned {
        tracing::error!("qlever: could not start the feeder thread: {e}");
    }
}

/// An `ASK`, or a `SELECT` with an aggregate anywhere in its algebra — the
/// `analytical` route's shape, wider than the shard classifier's verdict,
/// which is `None` for anything the shards cannot decompose (a variable
/// `GRAPH`, a subquery, …).
fn is_aggregate_query(sparql: &str) -> bool {
    use spargebra::algebra::GraphPattern as P;
    fn has_group(p: &P) -> bool {
        match p {
            P::Group { .. } => true,
            P::Join { left, right } | P::Union { left, right } | P::Minus { left, right } => {
                has_group(left) || has_group(right)
            }
            P::LeftJoin { left, right, .. } => has_group(left) || has_group(right),
            P::Lateral { left, right } => has_group(left) || has_group(right),
            P::Filter { inner, .. }
            | P::Graph { inner, .. }
            | P::Extend { inner, .. }
            | P::OrderBy { inner, .. }
            | P::Project { inner, .. }
            | P::Distinct { inner }
            | P::Reduced { inner }
            | P::Slice { inner, .. }
            | P::Service { inner, .. } => has_group(inner),
            _ => false,
        }
    }
    match spargebra::SparqlParser::new().parse_query(sparql) {
        Ok(spargebra::Query::Ask { .. }) => true,
        Ok(spargebra::Query::Select { pattern, .. }) => has_group(&pattern),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_parse_with_their_defaults() {
        let c = QleverConfig::parse(
            Some("http://qlever:7001/"),
            None,
            Some("t"),
            None,
            None,
            None,
            None,
        );
        assert_eq!(c.url.as_deref(), Some("http://qlever:7001"));
        assert!(c.enabled);
        assert_eq!(c.route, Route::Analytical);
        assert_eq!(c.batch, 5000);
        let off = QleverConfig::parse(
            Some("http://q"),
            Some("off"),
            None,
            Some("first"),
            Some("10"),
            None,
            None,
        );
        assert!(!off.enabled);
        assert_eq!(off.route, Route::First);
        assert_eq!(off.batch, 100, "the batch floor");
        assert!(QleverConfig::none().url.is_none());
    }

    #[test]
    fn the_analytical_shape_is_an_ask_or_an_aggregate_anywhere() {
        assert!(is_aggregate_query("ASK { ?s ?p ?o }"));
        assert!(is_aggregate_query(
            "SELECT (COUNT(?s) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }"
        ));
        assert!(is_aggregate_query(
            "SELECT ?s ?n WHERE { ?s ?p ?o { SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?q ?r } GROUP BY ?s } } ORDER BY ?n LIMIT 5"
        ));
        assert!(!is_aggregate_query("SELECT ?s WHERE { ?s ?p ?o } LIMIT 10"));
        assert!(!is_aggregate_query("CONSTRUCT WHERE { ?s ?p ?o }"));
        assert!(!is_aggregate_query("not sparql"));
    }

    #[test]
    fn only_select_and_ask_are_routed() {
        assert!(is_select_or_ask("SELECT * WHERE { ?s ?p ?o }"));
        assert!(is_select_or_ask(
            "PREFIX ex: <http://x/> ask { ?s ex:p ?o }"
        ));
        assert!(!is_select_or_ask(
            "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }"
        ));
        assert!(!is_select_or_ask("INSERT DATA { <a> <b> <c> }"));
    }

    #[test]
    fn quads_become_graph_blocks() {
        use oxigraph::model::{Literal, NamedNode};
        let q1 = Quad::new(
            NamedNode::new_unchecked("http://x/s"),
            NamedNode::new_unchecked("http://x/p"),
            Literal::from(1i64),
            GraphName::DefaultGraph,
        );
        let q2 = Quad::new(
            NamedNode::new_unchecked("http://x/s"),
            NamedNode::new_unchecked("http://x/p"),
            Literal::from("a"),
            GraphName::NamedNode(NamedNode::new_unchecked("http://x/g")),
        );
        let s = quads_as_sparql(&[q1, q2]);
        assert!(s.starts_with("<http://x/s> <http://x/p> \"1\"^^"), "{s}");
        assert!(
            s.contains("GRAPH <http://x/g> { <http://x/s> <http://x/p> \"a\" . }"),
            "{s}"
        );
    }
}
