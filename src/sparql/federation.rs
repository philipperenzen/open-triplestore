//! SPARQL 1.1 Federated Query (`SERVICE`) behind the remote allowlist.
//!
//! oxigraph is built without its HTTP client, so `SERVICE` used to error
//! unconditionally (an SSRF mitigation, but also no federation at all). This
//! handler is registered as the evaluator's default service handler: it
//! forwards the SERVICE pattern as a stand-alone SELECT to the named endpoint
//! — only if `crate::remote::is_allowed` says so — with the module's timeout
//! and body limit, and hands the solutions back to the local evaluator, which
//! joins them with the rest of the query.
//!
//! A remote result with more than `OTS_SERVICE_MAX_ROWS` rows is a failed
//! invocation, like a refused endpoint or a timeout: the query errors, or
//! under `SERVICE SILENT` the evaluator substitutes the empty solution (Ω0).
//! It is never truncated, because a truncated SERVICE result silently changes
//! the answer of the query around it.
//!
//! One query shares a [`QueryBudget`] across its `SERVICE` calls: the answer
//! of an (endpoint, pattern) pair is fetched once and reused (a `SERVICE`
//! inside a loop, or `SERVICE ?var` with the same endpoint on many rows, asks
//! the remote once), and the query may contact at most `OTS_SERVICE_MAX_ENDPOINTS`
//! endpoints with at most `OTS_SERVICE_MAX_CALLS` requests, within
//! `OTS_SERVICE_DEADLINE_SECS` of its start. Over an endpoint or call cap the
//! call fails like any other. Past the deadline the call fails and the
//! budget's cancellation token is cancelled: `TripleStore::query` evaluates a
//! federated query with that token, so the query then fails even where
//! `SERVICE SILENT` swallowed the call.
//!
//! `SERVICE ?var` is evaluated per binding of the variable through the
//! lateral-join rewrite in `opengraph::service_var`, applied by
//! `TripleStore::query`. Local bindings are not pushed into the remote query:
//! the pattern goes out as written and the answer is joined locally.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oxigraph::model::NamedNode;
use oxigraph::sparql::results::{
    QueryResultsFormat, QueryResultsParser, ReaderQueryResultsParserOutput,
};
use oxigraph::sparql::{
    CancellationToken, DefaultServiceHandler, QueryEvaluationError, QueryResults, QuerySolution,
    QuerySolutionIter, QueryTripleIter, Variable,
};
use oxiri::Iri;
use spargebra::algebra::GraphPattern;

#[derive(Debug, thiserror::Error)]
pub enum FederationError {
    #[error(transparent)]
    Remote(#[from] crate::remote::RemoteError),
    #[error("remote SPARQL results from <{url}> could not be parsed: {reason}")]
    Parse { url: String, reason: String },
    #[error("remote <{0}> answered a boolean result where solutions were expected")]
    NotSolutions(String),
    #[error(
        "remote <{url}> answered more than {cap} rows; raise {} or narrow the SERVICE pattern",
        crate::remote::MAX_ROWS_ENV
    )]
    TooManyRows { url: String, cap: usize },
    #[error(
        "this query would contact more than {cap} SERVICE endpoints (<{url}> is one too many); \
         raise {} or name fewer endpoints",
        crate::remote::MAX_ENDPOINTS_ENV
    )]
    TooManyEndpoints { url: String, cap: usize },
    #[error(
        "this query would make more than {cap} SERVICE requests; raise {} or name fewer endpoints",
        crate::remote::MAX_CALLS_ENV
    )]
    TooManyCalls { cap: usize },
    #[error(
        "this query's SERVICE calls ran past its {secs} s deadline; raise {}",
        crate::remote::DEADLINE_ENV
    )]
    DeadlineExceeded { secs: u64 },
    /// A failure kept for the rest of the query: every call with the same
    /// endpoint and pattern gets it, and the remote is not asked again.
    #[error(transparent)]
    Repeated(Arc<FederationError>),
}

/// A remote answer kept for the rest of the query.
#[derive(Debug)]
struct Rows {
    variables: Arc<[Variable]>,
    /// Each row's values, in `variables` order.
    rows: Vec<Vec<Option<oxigraph::model::Term>>>,
}

type Answer = Result<Arc<Rows>, Arc<FederationError>>;

/// What one query's `SERVICE` calls share: answers already fetched, the
/// endpoints and requests used against their caps, and the deadline.
pub struct QueryBudget {
    started: Instant,
    deadline: Duration,
    token: CancellationToken,
    state: Mutex<BudgetState>,
}

#[derive(Default)]
struct BudgetState {
    /// Keyed by (SERVICE IRI as written, request text).
    answers: HashMap<(String, String), Answer>,
    endpoints: HashSet<String>,
    calls: usize,
}

impl Default for QueryBudget {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            deadline: crate::remote::deadline(),
            token: CancellationToken::new(),
            state: Mutex::new(BudgetState::default()),
        }
    }
}

impl std::fmt::Debug for QueryBudget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryBudget")
            .field("started", &self.started)
            .field("deadline", &self.deadline)
            .field("cancelled", &self.token.is_cancelled())
            .finish_non_exhaustive()
    }
}

impl QueryBudget {
    /// The token cancelled when the deadline passes; hand it to the
    /// evaluator (`SparqlEvaluator::with_cancellation_token`) to stop the
    /// whole query then.
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    fn time_left(&self) -> Duration {
        self.deadline.saturating_sub(self.started.elapsed())
    }

    fn deadline_exceeded(&self) -> FederationError {
        self.token.cancel();
        FederationError::DeadlineExceeded {
            secs: self.deadline.as_secs(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, BudgetState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Count a request to `endpoint` against the caps, or refuse it.
    fn admit(&self, endpoint: &str) -> Result<(), FederationError> {
        let mut st = self.state();
        let cap = crate::remote::max_endpoints();
        if !st.endpoints.contains(endpoint) && st.endpoints.len() >= cap {
            return Err(FederationError::TooManyEndpoints {
                url: endpoint.to_string(),
                cap,
            });
        }
        let cap = crate::remote::max_calls();
        if st.calls >= cap {
            return Err(FederationError::TooManyCalls { cap });
        }
        st.endpoints.insert(endpoint.to_string());
        st.calls += 1;
        Ok(())
    }

    /// Wrap a federated query's results so that, once the deadline has
    /// passed, the query ends with that error — also when `SERVICE SILENT`
    /// turned the failing call into Ω0 and the evaluator went on.
    pub fn guard(self: Arc<Self>, results: QueryResults<'static>) -> QueryResults<'static> {
        let error = {
            let budget = Arc::clone(&self);
            move || budget.deadline_error()
        };
        match results {
            QueryResults::Solutions(iter) => {
                let variables: Arc<[Variable]> = Arc::from(iter.variables());
                QueryResults::Solutions(QuerySolutionIter::new(
                    variables,
                    until_cancelled(iter, self.token.clone(), error),
                ))
            }
            QueryResults::Graph(iter) => QueryResults::Graph(QueryTripleIter::new(
                until_cancelled(iter, self.token.clone(), error),
            )),
            QueryResults::Boolean(b) => QueryResults::Boolean(b),
        }
    }

    /// Whether the deadline cancelled the query (a boolean result cannot
    /// carry the error, so the caller asks).
    pub fn cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// The error a query cancelled at the deadline ends with.
    pub fn deadline_error(&self) -> QueryEvaluationError {
        QueryEvaluationError::Service(Box::new(FederationError::DeadlineExceeded {
            secs: self.deadline.as_secs(),
        }))
    }
}

/// `inner`, ending with `error()` as soon as `token` is cancelled.
fn until_cancelled<T>(
    mut inner: impl Iterator<Item = Result<T, QueryEvaluationError>>,
    token: CancellationToken,
    error: impl Fn() -> QueryEvaluationError,
) -> impl Iterator<Item = Result<T, QueryEvaluationError>> {
    let mut done = false;
    std::iter::from_fn(move || {
        if done {
            return None;
        }
        let item = inner.next();
        if token.is_cancelled() {
            done = true;
            return Some(Err(error()));
        }
        done = item.is_none();
        item
    })
}

/// The default service handler: allowlist, timeout, body limit, row cap,
/// and the per-query budget.
#[derive(Debug, Clone, Default)]
pub struct AllowlistedServiceHandler {
    /// The identity the query acts for (captured on the request thread), for
    /// federation assertions towards the remote.
    pub identity: Option<std::sync::Arc<crate::federation::Identity>>,
    /// Whom the query acts for, captured the same way: decides whether
    /// `SERVICE <urn:source:id>` may use the source's account. `None`
    /// resolves no source.
    pub source_caller: Option<std::sync::Arc<crate::sources::virtual_source::SourceCaller>>,
    /// Shared by every `SERVICE` call of one query.
    pub budget: Arc<QueryBudget>,
}

impl DefaultServiceHandler for AllowlistedServiceHandler {
    type Error = FederationError;

    fn handle(
        &self,
        service_name: &NamedNode,
        pattern: &GraphPattern,
        base_iri: Option<&Iri<String>>,
    ) -> Result<QuerySolutionIter<'static>, Self::Error> {
        let query = spargebra::Query::Select {
            dataset: None,
            pattern: pattern.clone(),
            base_iri: base_iri.cloned(),
        }
        .to_string();
        let key = (service_name.as_str().to_string(), query);
        let known = self.budget.state().answers.get(&key).cloned();
        let answer = match known {
            Some(answer) => answer,
            None => {
                let answer = self
                    .fetch(service_name, &key.1)
                    .map(Arc::new)
                    .map_err(Arc::new);
                self.budget.state().answers.insert(key, answer.clone());
                answer
            }
        };
        match answer {
            Ok(rows) => {
                let variables = Arc::clone(&rows.variables);
                let n = rows.rows.len();
                Ok(QuerySolutionIter::new(
                    Arc::clone(&variables),
                    (0..n).map(move |i| {
                        Ok(QuerySolution::from((
                            Arc::clone(&variables),
                            rows.rows[i].clone(),
                        )))
                    }),
                ))
            }
            Err(e) => Err(FederationError::Repeated(e)),
        }
    }
}

impl AllowlistedServiceHandler {
    /// Ask the remote: resolve, allowlist, caps and deadline, then one request.
    fn fetch(&self, service_name: &NamedNode, query: &str) -> Result<Rows, FederationError> {
        let left = self.budget.time_left();
        if left.is_zero() {
            return Err(self.budget.deadline_exceeded());
        }
        // `SERVICE <urn:source:id>` names a registered virtual datasource:
        // its endpoint and its account stand in, and the allowlist applies
        // to the endpoint exactly as to a URL written out. A source the
        // caller may not use resolves to nothing, and its IRI then fails the
        // allowlist like any IRI that names no source.
        let resolved = match crate::sources::virtual_source::resolve(
            service_name.as_str(),
            self.source_caller.as_deref(),
        ) {
            Some(Ok(r)) => Some(r),
            Some(Err(reason)) => {
                return Err(FederationError::Parse {
                    url: service_name.as_str().to_string(),
                    reason,
                })
            }
            None => None,
        };
        let endpoint: &str = resolved
            .as_ref()
            .map(|r| r.endpoint.as_str())
            .unwrap_or_else(|| service_name.as_str());
        if !crate::remote::is_allowed(endpoint) {
            return Err(crate::remote::RemoteError::NotAllowed(endpoint.to_string()).into());
        }
        self.budget.admit(endpoint)?;
        let bearer = crate::federation::assertion_for_identity(endpoint, self.identity.as_ref());
        let auth = match resolved.as_ref() {
            Some(r) => match (&r.username, &r.secret) {
                (Some(u), Some(s)) => crate::remote::Auth::Basic(u, s.expose()),
                _ => crate::remote::Auth::None,
            },
            None => match bearer.as_deref() {
                Some(b) => crate::remote::Auth::Bearer(b),
                None => crate::remote::Auth::None,
            },
        };
        let body = crate::remote::post_sparql_blocking_within(
            endpoint,
            query,
            "application/sparql-results+json",
            auth,
            crate::remote::timeout().min(left),
        );
        let body = match body {
            Ok(body) => body,
            // A request cut short by the query's deadline reports the
            // deadline, not a plain timeout.
            Err(_) if self.budget.time_left().is_zero() => {
                return Err(self.budget.deadline_exceeded())
            }
            Err(e) => return Err(e.into()),
        };
        let parsed = QueryResultsParser::from_format(QueryResultsFormat::Json)
            .for_reader(body.as_bytes())
            .map_err(|e| FederationError::Parse {
                url: endpoint.to_string(),
                reason: e.to_string(),
            })?;
        match parsed {
            ReaderQueryResultsParserOutput::Solutions(iter) => {
                let variables: Arc<[Variable]> = Arc::from(iter.variables().to_vec());
                let cap = crate::remote::max_rows();
                let mut rows = Vec::new();
                for row in iter {
                    if rows.len() >= cap {
                        return Err(FederationError::TooManyRows {
                            url: endpoint.to_string(),
                            cap,
                        });
                    }
                    let row = row.map_err(|e| FederationError::Parse {
                        url: endpoint.to_string(),
                        reason: e.to_string(),
                    })?;
                    rows.push(row.values().to_vec());
                }
                Ok(Rows { variables, rows })
            }
            ReaderQueryResultsParserOutput::Boolean(_) => {
                Err(FederationError::NotSolutions(endpoint.to_string()))
            }
        }
    }
}
