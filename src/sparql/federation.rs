//! SPARQL 1.1 Federated Query (`SERVICE`) behind the remote allowlist.
//!
//! oxigraph is built without its HTTP client, so `SERVICE` used to error
//! unconditionally (an SSRF mitigation, but also no federation at all). This
//! handler is registered as the evaluator's default service handler: it
//! forwards the SERVICE pattern as a stand-alone SELECT to the named endpoint
//! — only if `crate::remote::is_allowed` says so — with the module's timeout,
//! caps the rows at `OTS_SERVICE_MAX_ROWS`, and hands the solutions back to
//! the local evaluator, which joins them with the rest of the query.
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
}

/// The default service handler: allowlist, timeout, row cap.
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowlistedServiceHandler;

impl DefaultServiceHandler for AllowlistedServiceHandler {
    type Error = FederationError;

    fn handle(
        &self,
        service_name: &NamedNode,
        pattern: &GraphPattern,
        base_iri: Option<&Iri<String>>,
    ) -> Result<QuerySolutionIter<'static>, Self::Error> {
        let endpoint = service_name.as_str();
        if !crate::remote::is_allowed(endpoint) {
            return Err(crate::remote::RemoteError::NotAllowed(endpoint.to_string()).into());
        }
        let query = spargebra::Query::Select {
            dataset: None,
            pattern: pattern.clone(),
            base_iri: base_iri.cloned(),
        }
        .to_string();
        let body = crate::remote::post_sparql_blocking(endpoint, &query)?;
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
                        break;
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
