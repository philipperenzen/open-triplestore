//! The datasource, mapping and run records, and their JSON shapes.
//!
//! These are *views* over RDF: [`registry`](super::registry) is the storage
//! layer and the `urn:system:sources` graph is the system of record. A record
//! never holds a resolved secret — only the [`SecretRef`] that points at one.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::secrets::SecretRef;

/// The datasource vocabulary. Provisional base IRI, versioned with the
/// platform vocabularies under `w3id.org`.
pub const DS: &str = "https://w3id.org/open-triplestore/datasource#";
/// Named graph holding every datasource, mapping and run record.
pub const SOURCES_GRAPH: &str = "urn:system:sources";

pub fn source_iri(id: &str) -> String {
    format!("urn:source:{id}")
}
pub fn mapping_iri(id: &str) -> String {
    format!("urn:mapping:{id}")
}
/// A mapping version IRI is *also* the named graph holding that version's RML.
pub fn mapping_version_iri(id: &str, version: u32) -> String {
    format!("urn:mapping:{id}:version:{version}")
}
/// A run IRI is *also* the named graph holding the triples it produced.
pub fn run_graph_iri(run_id: &str) -> String {
    format!("urn:run:{run_id}")
}
/// The PROV activity that produced a run's graph. Distinct from the graph:
/// an activity does not generate itself.
pub fn run_activity_iri(run_id: &str) -> String {
    format!("urn:run:{run_id}:activity")
}

/// An identifier that is safe as an IRI segment and as a path parameter.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Which graph a source currently serves from, and the run that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RunPointer {
    pub graph: String,
    pub run: String,
}

/// A registered SQL datasource.
#[derive(Debug, Clone, Default)]
pub struct SqlSource {
    pub id: String,
    pub name: String,
    pub dialect: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    /// Database name, or the file path for a file-backed dialect.
    pub database: String,
    pub username: Option<String>,
    /// The *reference*. The value it points at is never stored or returned.
    pub credential: Option<SecretRef>,
    pub read_only: bool,
    pub statement_timeout_ms: u64,
    pub watermark_column: Option<String>,
    /// Whether the proposer may send this source's schema metadata to a model.
    pub allow_model_assist: bool,
    pub tls: bool,
    pub options: BTreeMap<String, String>,
    /// Dataset the run graphs are registered to, when one is bound.
    pub dataset: Option<String>,
    pub owner: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub production: Option<RunPointer>,
    pub previous: Option<RunPointer>,
}

impl SqlSource {
    pub fn iri(&self) -> String {
        source_iri(&self.id)
    }

    /// Connection parameters with the credential resolved at the moment of
    /// use. The resolved value lives only in the returned struct, which
    /// redacts it in `Debug` and is dropped with the connection.
    pub fn connect_params(
        &self,
    ) -> Result<ots_plugin_api::sources::ConnectParams, crate::secrets::SecretError> {
        let password = match &self.credential {
            Some(r) => Some(ots_plugin_api::sources::SecretString::new(
                crate::secrets::resolve(r)?.expose(),
            )),
            None => None,
        };
        Ok(ots_plugin_api::sources::ConnectParams {
            dialect: self.dialect.clone(),
            host: self.host.clone(),
            port: self.port,
            database: self.database.clone(),
            username: self.username.clone(),
            password,
            read_only: true,
            statement_timeout_ms: self.statement_timeout_ms,
            tls: self.tls,
            options: self.options.clone(),
        })
    }

    /// The endpoint the egress allowlist is checked against, for a networked
    /// dialect. `None` for a file-backed one.
    pub fn egress_url(&self) -> Option<String> {
        let host = self.host.as_deref()?;
        let scheme = if self.tls { "https" } else { "http" };
        Some(match self.port {
            Some(p) => format!("{scheme}://{host}:{p}"),
            None => format!("{scheme}://{host}"),
        })
    }
}

/// What a caller sends to register or update a datasource.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceRequest {
    pub id: Option<String>,
    pub name: Option<String>,
    pub dialect: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: String,
    pub username: Option<String>,
    /// A secret **reference** (`env:` / `file:` / `vault:`). A raw value is
    /// refused in the production posture.
    pub credential: Option<String>,
    #[serde(default = "default_true")]
    pub read_only: bool,
    pub statement_timeout_ms: Option<u64>,
    pub watermark_column: Option<String>,
    #[serde(default)]
    pub allow_model_assist: bool,
    #[serde(default)]
    pub tls: bool,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    pub dataset: Option<String>,
}

fn default_true() -> bool {
    true
}

/// The API view of a datasource: the credential **reference**, never a value.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceResponse {
    pub id: String,
    pub iri: String,
    pub name: String,
    pub dialect: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub database: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The reference string, e.g. `vault:secret/data/sources/legacy#password`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    pub read_only: bool,
    pub statement_timeout_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watermark_column: Option<String>,
    pub allow_model_assist: bool,
    pub tls: bool,
    /// Whether the source's endpoint passes the federation egress allowlist
    /// (always true for a file-backed dialect).
    pub allowlisted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dataset: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production: Option<RunPointer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<RunPointer>,
}

impl From<&SqlSource> for SourceResponse {
    fn from(s: &SqlSource) -> Self {
        SourceResponse {
            id: s.id.clone(),
            iri: s.iri(),
            name: s.name.clone(),
            dialect: s.dialect.clone(),
            host: s.host.clone(),
            port: s.port,
            database: s.database.clone(),
            username: s.username.clone(),
            credential: s.credential.as_ref().map(|r| r.to_string()),
            read_only: s.read_only,
            statement_timeout_ms: s.statement_timeout_ms,
            watermark_column: s.watermark_column.clone(),
            allow_model_assist: s.allow_model_assist,
            tls: s.tls,
            allowlisted: super::egress_allowed(s),
            dataset: s.dataset.clone(),
            owner: s.owner.clone(),
            created_at: s.created_at.clone(),
            updated_at: s.updated_at.clone(),
            production: s.production.clone(),
            previous: s.previous.clone(),
        }
    }
}

/// Lifecycle state of a mapping. `proposed` is what the external proposer
/// writes; only a human moves it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MappingState {
    Draft,
    Proposed,
    Approved,
}

impl MappingState {
    pub fn as_str(self) -> &'static str {
        match self {
            MappingState::Draft => "draft",
            MappingState::Proposed => "proposed",
            MappingState::Approved => "approved",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "draft" => Some(MappingState::Draft),
            "proposed" => Some(MappingState::Proposed),
            "approved" => Some(MappingState::Approved),
            _ => None,
        }
    }
}

/// A registered RML mapping: metadata here, the RML itself in the version
/// graph named by [`mapping_version_iri`].
#[derive(Debug, Clone)]
pub struct MappingRecord {
    pub id: String,
    pub title: String,
    pub source_id: String,
    /// The newest frozen version. Version 1 is created with the mapping.
    pub version: u32,
    pub state: MappingState,
    /// Shapes graph this mapping's output is gated against (`dct:conformsTo`).
    pub shapes_graph: Option<String>,
    /// Model registry id and version the mapping targets.
    pub model: Option<String>,
    pub model_version: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl MappingRecord {
    pub fn iri(&self) -> String {
        mapping_iri(&self.id)
    }
    pub fn version_iri(&self) -> String {
        mapping_version_iri(&self.id, self.version)
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MappingRequest {
    pub id: Option<String>,
    pub title: Option<String>,
    pub source: Option<String>,
    /// RML in Turtle. Mutually exclusive with `yarrrml`.
    pub rml: Option<String>,
    /// YARRRML source; translated to RML on the way in.
    pub yarrrml: Option<String>,
    pub shapes_graph: Option<String>,
    pub model: Option<String>,
    pub model_version: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MappingResponse {
    pub id: String,
    pub iri: String,
    pub title: String,
    pub source: String,
    pub version: u32,
    pub version_iri: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shapes_graph: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
    /// Triples maps in the newest version, for a list view.
    pub triples_maps: usize,
    /// `rr:parentTriplesMap` edges in the newest version: which triples map
    /// joins to which. Drives the Studio matrix view.
    pub joins: Vec<MappingJoin>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    /// Materialised, gated and swapped into production.
    Succeeded,
    /// Materialised but refused by the SHACL gate; the candidate graph is kept.
    Rejected,
    /// Did not complete; nothing was swapped.
    Failed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Succeeded => "succeeded",
            RunStatus::Rejected => "rejected",
            RunStatus::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "succeeded" => Some(RunStatus::Succeeded),
            "rejected" => Some(RunStatus::Rejected),
            "failed" => Some(RunStatus::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    /// Re-materialise everything the mapping selects.
    Full,
    /// Only rows newer than the last watermark (phase 3).
    Watermark,
}

impl RunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Full => "full",
            RunMode::Watermark => "watermark",
        }
    }
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunRequest {
    /// Mapping id, or its IRI.
    pub mapping: String,
    pub model_version: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    /// Rows per batch handed to the term-map evaluator.
    pub batch_size: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct RunRecord {
    pub id: String,
    pub source_id: String,
    pub mapping_id: String,
    pub mapping_version: u32,
    pub model_version: Option<String>,
    pub mode: String,
    pub status: Option<RunStatus>,
    pub graph: String,
    pub previous_graph: Option<String>,
    pub rows_extracted: u64,
    pub triples_produced: u64,
    pub duration_ms: u64,
    pub started_at: String,
    pub ended_at: String,
    pub actor: Option<String>,
    /// `None` when no gate applied.
    pub conforms: Option<bool>,
    pub violations: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunResponse {
    pub id: String,
    pub activity: String,
    pub source: String,
    pub graph: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_graph: Option<String>,
    pub status: String,
    pub mode: String,
    pub mapping: MappingRef,
    pub rows_extracted: u64,
    pub triples_produced: u64,
    /// Triples the run's graph holds *now*. Differs from `triplesProduced`
    /// once a graph has been deleted, and is how a caller tells a kept
    /// candidate from a collected one without dataset-scoped SPARQL.
    pub graph_triples: u64,
    pub duration_ms: u64,
    pub started_at: String,
    pub ended_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shacl: Option<ShaclSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MappingJoin {
    pub child: String,
    pub parent: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MappingRef {
    pub id: String,
    pub version: u32,
    pub iri: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShaclSummary {
    pub conforms: bool,
    pub violations: u64,
}

impl RunResponse {
    /// Build the API view, reading the run graph's current size from `store`.
    pub fn of(store: &crate::store::TripleStore, r: &RunRecord) -> Self {
        let mut view = RunResponse::from(r);
        view.graph_triples = store.count_graph(Some(&r.graph)).unwrap_or(0) as u64;
        view
    }
}

impl From<&RunRecord> for RunResponse {
    fn from(r: &RunRecord) -> Self {
        RunResponse {
            id: r.id.clone(),
            activity: run_activity_iri(&r.id),
            source: source_iri(&r.source_id),
            graph: r.graph.clone(),
            previous_graph: r.previous_graph.clone(),
            status: r.status.map(|s| s.as_str()).unwrap_or("failed").to_string(),
            mode: r.mode.clone(),
            mapping: MappingRef {
                id: r.mapping_id.clone(),
                version: r.mapping_version,
                iri: mapping_version_iri(&r.mapping_id, r.mapping_version),
            },
            rows_extracted: r.rows_extracted,
            triples_produced: r.triples_produced,
            // Filled in by `RunResponse::of`, which has the store.
            graph_triples: 0,
            duration_ms: r.duration_ms,
            started_at: r.started_at.clone(),
            ended_at: r.ended_at.clone(),
            actor: r.actor.clone(),
            shacl: r.conforms.map(|conforms| ShaclSummary {
                conforms,
                violations: r.violations,
            }),
            error: r.error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iris_are_derived_consistently() {
        assert_eq!(source_iri("legacy"), "urn:source:legacy");
        assert_eq!(mapping_version_iri("m", 3), "urn:mapping:m:version:3");
        assert_eq!(run_graph_iri("r1"), "urn:run:r1");
        assert_eq!(run_activity_iri("r1"), "urn:run:r1:activity");
        assert_ne!(run_graph_iri("r1"), run_activity_iri("r1"));
    }

    #[test]
    fn ids_that_could_break_out_of_an_iri_are_refused() {
        for ok in ["legacy", "legacy-assets", "a.b_c", "A1"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", "a b", "a/b", "a:b", "a>b", "a#b", "../x", &"x".repeat(129)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_source_never_serialises_a_secret_value() {
        std::env::set_var("OTS_MODEL_TEST_SECRET", "top-secret-value");
        let s = SqlSource {
            id: "s".into(),
            name: "s".into(),
            dialect: "sqlite".into(),
            database: "/tmp/x.db".into(),
            credential: Some(SecretRef::parse("env:OTS_MODEL_TEST_SECRET").unwrap()),
            read_only: true,
            statement_timeout_ms: 1000,
            ..Default::default()
        };
        let json = serde_json::to_string(&SourceResponse::from(&s)).unwrap();
        assert!(json.contains("env:OTS_MODEL_TEST_SECRET"));
        assert!(!json.contains("top-secret-value"));
        // …and the resolved parameters redact it too.
        let params = s.connect_params().unwrap();
        assert_eq!(params.password.as_ref().unwrap().expose(), "top-secret-value");
        assert!(!format!("{params:?}").contains("top-secret-value"));
    }

    #[test]
    fn egress_url_is_scheme_host_port_or_none_for_a_file() {
        let mut s = SqlSource {
            dialect: "postgresql".into(),
            host: Some("db.internal".into()),
            port: Some(5432),
            ..Default::default()
        };
        assert_eq!(s.egress_url().as_deref(), Some("http://db.internal:5432"));
        s.tls = true;
        assert_eq!(s.egress_url().as_deref(), Some("https://db.internal:5432"));
        s.host = None;
        assert_eq!(s.egress_url(), None);
    }
}
