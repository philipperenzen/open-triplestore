//! Repair a broken saved query with the configured LLM endpoint.
//!
//! Like NL→SPARQL generation, the model only *rewrites text*: the returned query
//! is still run through the normal scoped executor, so repair cannot widen what
//! the caller may read. It uses the same model as NL→SPARQL ([`llm_sparql::sparql_model`]).

use crate::server::error::AppError;
use crate::server::llm_sparql;

const REPAIR_SYSTEM_PROMPT: &str = "You are a SPARQL repair assistant. You are given a SPARQL \
query, the error message it produced, and optionally the ontology prefixes in use. Return a \
corrected version of the query that fixes the error while preserving the original intent and any \
{{placeholder}} variables. Reply with ONLY the SPARQL query.";

pub struct RepairResult {
    pub sparql: String,
    pub model: String,
}

/// Ask the gateway to fix `broken` given `error` and optional `schema_hint`.
pub async fn repair_query(
    broken: &str,
    error: &str,
    schema_hint: Option<&str>,
    model: Option<&str>,
) -> Result<RepairResult, AppError> {
    if broken.trim().is_empty() {
        return Err(AppError::BadRequest("nothing to repair".to_string()));
    }
    let model = model.map(|m| m.to_string()).unwrap_or_else(llm_sparql::sparql_model);
    let mut user = format!(
        "Broken SPARQL query:\n{}\n\nError:\n{}",
        broken.trim(),
        error.trim()
    );
    if let Some(h) = schema_hint {
        if !h.trim().is_empty() {
            user.push_str(&format!("\n\nOntology / prefixes:\n{}", h.trim()));
        }
    }
    let sparql = llm_sparql::chat_completion(&model, REPAIR_SYSTEM_PROMPT, &user, 512).await?;
    Ok(RepairResult { sparql, model })
}
