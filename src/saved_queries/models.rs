//! Data types for saved, versioned SPARQL queries.
//!
//! A [`SavedQuery`] is a reusable SPARQL query owned by a dataset, organisation
//! or group. It carries its own edit history ([`SavedQueryRevision`]) and a
//! record of how each revision behaved against successive dataset versions
//! ([`QueryTest`]). When exposed as an API it accepts typed [`ParamSpec`]
//! variables that are safely injected into the query text.

use serde::{Deserialize, Serialize};

/// The kind of owner a saved query is scoped to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QueryScope {
    Dataset,
    Organisation,
    Group,
}

impl QueryScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            QueryScope::Dataset => "dataset",
            QueryScope::Organisation => "organisation",
            QueryScope::Group => "group",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "dataset" => Some(QueryScope::Dataset),
            "organisation" => Some(QueryScope::Organisation),
            "group" => Some(QueryScope::Group),
            _ => None,
        }
    }
}

/// Datatype of an API variable. Determines how a supplied value is rendered into
/// the SPARQL text (see [`crate::saved_queries::params`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParamType {
    /// An absolute IRI, rendered as `<iri>`.
    Iri,
    /// A plain string literal, rendered as an escaped `"..."`.
    String,
    Integer,
    Decimal,
    Boolean,
    /// `xsd:date` (`YYYY-MM-DD`).
    Date,
    /// `xsd:dateTime` (ISO-8601).
    DateTime,
}

impl Default for ParamType {
    fn default() -> Self {
        ParamType::String
    }
}

/// One typed variable a saved query exposes when run as an API.
///
/// The placeholder substituted in the SPARQL text is `{{name}}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSpec {
    pub name: String,
    #[serde(rename = "type", default)]
    pub param_type: ParamType,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A saved query (head state). `sparql` carries the head revision's text when an
/// accessor joins it in; it is `None` for list views that omit the body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQuery {
    pub id: String,
    pub scope: QueryScope,
    pub owner_id: String,
    pub name: String,
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub current_revision: i64,
    pub parameters: Vec<ParamSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub test_parameters: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
    pub is_active: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sparql: Option<String>,
}

/// One immutable revision of a saved query's SPARQL text.
///
/// Like a commit, a revision can carry a human-friendly `name` (e.g. "v2 — add
/// language filter") and a longer `note`, both optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedQueryRevision {
    pub query_id: String,
    pub revision: i64,
    /// Optional custom version name (the revision's short title).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub sparql: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// `manual` | `llm_repair` | `import`.
    pub origin: String,
    pub created_by: String,
    pub created_at: String,
}

/// Outcome of running a saved-query revision against one dataset version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryTest {
    pub id: String,
    pub query_id: String,
    pub revision: i64,
    pub dataset_id: String,
    pub dataset_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_version: Option<String>,
    /// `ok` | `changed` | `error`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_rowcount: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub acknowledged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<String>,
    pub created_at: String,
}

// ─── Request bodies ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateSavedQueryRequest {
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub sparql: String,
    #[serde(default)]
    pub parameters: Vec<ParamSpec>,
    #[serde(default)]
    pub test_parameters: Option<serde_json::Value>,
    #[serde(default)]
    pub visibility: Option<String>,
    /// Optional custom version name + note for the initial revision (commit-style).
    #[serde(default)]
    pub version_name: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateSavedQueryRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// When present, a new revision is created with this text.
    #[serde(default)]
    pub sparql: Option<String>,
    /// Custom version name for the new revision (commit-style title).
    #[serde(default)]
    pub version_name: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub parameters: Option<Vec<ParamSpec>>,
    #[serde(default)]
    pub test_parameters: Option<serde_json::Value>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub is_active: Option<bool>,
}

/// Turn a free-text name into a URL-safe slug (`[a-z0-9-]`).
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "query".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Cities by Population!"), "cities-by-population");
        assert_eq!(slugify("  weird __ name  "), "weird-name");
        assert_eq!(slugify("***"), "query");
    }

    #[test]
    fn param_type_serde_camel() {
        assert_eq!(
            serde_json::to_string(&ParamType::DateTime).unwrap(),
            "\"dateTime\""
        );
        let t: ParamType = serde_json::from_str("\"iri\"").unwrap();
        assert_eq!(t, ParamType::Iri);
    }
}
