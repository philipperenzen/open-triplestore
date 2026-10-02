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
//! Not supported: `SERVICE ?var` (a variable endpoint) and pushing local
//! bindings into the remote query; each SERVICE is evaluated once, on its own.

use std::sync::Arc;

use oxigraph::model::NamedNode;
use oxigraph::sparql::results::{
    QueryResultsFormat, QueryResultsParser, ReaderQueryResultsParserOutput,
};
use oxigraph::sparql::{DefaultServiceHandler, QuerySolution, QuerySolutionIter, Variable};
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
}

/// The default service handler: allowlist, timeout, body limit, row cap.
#[derive(Debug, Clone, Default)]
pub struct AllowlistedServiceHandler {
    /// The identity the query acts for (captured on the request thread), for
    /// federation assertions towards the remote.
    pub identity: Option<std::sync::Arc<crate::federation::Identity>>,
    /// Whom the query acts for, captured the same way: decides whether
    /// `SERVICE <urn:source:id>` may use the source's account. `None`
    /// resolves no source.
    pub source_caller: Option<std::sync::Arc<crate::sources::virtual_source::SourceCaller>>,
}

impl DefaultServiceHandler for AllowlistedServiceHandler {
    type Error = FederationError;

    fn handle(
        &self,
        service_name: &NamedNode,
        pattern: &GraphPattern,
        base_iri: Option<&Iri<String>>,
    ) -> Result<QuerySolutionIter<'static>, Self::Error> {
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
        let query = spargebra::Query::Select {
            dataset: None,
            pattern: pattern.clone(),
            base_iri: base_iri.cloned(),
        }
        .to_string();
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
        let body = crate::remote::post_sparql_blocking(
            endpoint,
            &query,
            "application/sparql-results+json",
            auth,
        )?;
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
                let mut rows: Vec<QuerySolution> = Vec::new();
                for row in iter {
                    if rows.len() >= cap {
                        return Err(FederationError::TooManyRows {
                            url: endpoint.to_string(),
                            cap,
                        });
                    }
                    rows.push(row.map_err(|e| FederationError::Parse {
                        url: endpoint.to_string(),
                        reason: e.to_string(),
                    })?);
                }
                Ok(QuerySolutionIter::new(variables, rows.into_iter().map(Ok)))
            }
            ReaderQueryResultsParserOutput::Boolean(_) => {
                Err(FederationError::NotSolutions(endpoint.to_string()))
            }
        }
    }
}
