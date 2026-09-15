//! Constraint-specification importers: turn a domain's exchange-requirement
//! format into SHACL shapes that live in SHACL Studio like any other shape
//! graph. The interface is generic — an importer takes bytes and returns
//! Turtle plus a report — and buildingSMART IDS ([`ids`]) is the first
//! implementation; a FHIR-profile or any other importer is the same shape.
//!
//! The same interface runs in reverse: an exporter takes the typed shapes of
//! a shape graph and writes the exchange format back out, reporting everything
//! it could not express rather than emitting a document that quietly says less
//! than the shapes did.
//!
//! * `GET  /api/shacl/importers`             — the registered import formats
//! * `POST /api/shacl/import/:format`        — convert (and, with
//!   `?create=true`, create a shape graph from the result)
//! * `GET  /api/shacl/exporters`             — the registered export formats
//! * `POST /api/shacl/export/:format`        — Turtle in, the format out

pub mod ids;
pub mod ids_export;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;

/// One specification of the imported document, for the report.
#[derive(Debug, Clone, Serialize)]
pub struct SpecSummary {
    pub name: String,
    pub shape: String,
    pub target_classes: Vec<String>,
    pub requirements: usize,
}

/// What an importer produces.
#[derive(Debug, Clone, Serialize)]
pub struct ImportedShapes {
    pub title: String,
    pub description: Option<String>,
    pub turtle: String,
    pub shape_count: usize,
    pub specifications: Vec<SpecSummary>,
    /// What could not be expressed or had to be approximated.
    pub warnings: Vec<String>,
}

pub trait SpecImporter: Send + Sync {
    /// Route segment and registry key, e.g. `ids`.
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    /// Media types the format is usually served as.
    fn media_types(&self) -> &'static [&'static str];
    fn import(&self, bytes: &[u8]) -> anyhow::Result<ImportedShapes>;
}

pub fn importers() -> &'static [&'static dyn SpecImporter] {
    static IDS: ids::IdsImporter = ids::IdsImporter;
    static ALL: [&dyn SpecImporter; 1] = [&IDS];
    &ALL
}

pub fn importer(id: &str) -> Option<&'static dyn SpecImporter> {
    importers().iter().copied().find(|i| i.id() == id)
}

/// What an exporter produces: the document, how many specifications it
/// carried, and — the part that matters — everything that could not be
/// expressed in the target format.
#[derive(Debug, Clone, Serialize)]
pub struct ExportedSpec {
    pub document: String,
    pub specification_count: usize,
    /// Constraints, paths and targets with no counterpart in the target
    /// format. Never empty by accident: an exporter that drops something
    /// records it here, and one that can express nothing at all errors.
    pub losses: Vec<String>,
}

pub trait SpecExporter: Send + Sync {
    /// Route segment and registry key, e.g. `ids`.
    fn id(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn media_type(&self) -> &'static str;
    fn file_extension(&self) -> &'static str;
    fn export(
        &self,
        shapes: &[crate::shacl::shapes::Shape],
        title: &str,
    ) -> anyhow::Result<ExportedSpec>;
}

pub fn exporters() -> &'static [&'static dyn SpecExporter] {
    static IDS: ids_export::IdsExporter = ids_export::IdsExporter;
    static ALL: [&dyn SpecExporter; 1] = [&IDS];
    &ALL
}

pub fn exporter(id: &str) -> Option<&'static dyn SpecExporter> {
    exporters().iter().copied().find(|e| e.id() == id)
}

#[derive(Debug, Serialize)]
pub struct ExporterInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub media_type: &'static str,
    pub file_extension: &'static str,
}

/// GET /api/shacl/exporters
pub async fn list_exporters() -> impl IntoResponse {
    Json(
        exporters()
            .iter()
            .map(|e| ExporterInfo {
                id: e.id(),
                label: e.label(),
                media_type: e.media_type(),
                file_extension: e.file_extension(),
            })
            .collect::<Vec<_>>(),
    )
}

#[derive(Debug, Default, Deserialize)]
pub struct ExportQuery {
    /// Return the document itself rather than the JSON report.
    #[serde(default)]
    pub raw: bool,
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExportResponse {
    pub format: &'static str,
    #[serde(flatten)]
    pub spec: ExportedSpec,
}

/// POST /api/shacl/export/:format — body is a shapes graph in Turtle.
///
/// The report is the default representation because the losses are the point:
/// an exporter that silently produced a thinner document than the shapes it
/// was given would be worse than useless for a delivery contract. `?raw=true`
/// returns the bare document for callers that have already read them.
pub async fn export_spec(
    Path(format): Path<String>,
    Query(q): Query<ExportQuery>,
    body: Bytes,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let exp = exporter(&format).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!(
                "unknown specification format `{format}`; known: {}",
                exporters()
                    .iter()
                    .map(|e| e.id())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })?;
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty body".to_string()));
    }
    let turtle = std::str::from_utf8(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, "body is not UTF-8".to_string()))?;
    let store = crate::store::TripleStore::in_memory()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    store
        .load_str(turtle, oxigraph::io::RdfFormat::Turtle, Some("urn:export"))
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("shapes do not parse: {e}")))?;
    let shapes = crate::shacl::engine::load_shapes(&store, "urn:export")
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e))?;
    let title = q.title.clone().unwrap_or_else(|| "Exported shapes".into());
    let spec = exp
        .export(&shapes, &title)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, format!("{format}: {e}")))?;
    if q.raw {
        return Ok((
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, exp.media_type())],
            spec.document,
        )
            .into_response());
    }
    Ok(Json(ExportResponse {
        format: exp.id(),
        spec,
    })
    .into_response())
}

#[derive(Debug, Serialize)]
pub struct ImporterInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub media_types: &'static [&'static str],
}

/// GET /api/shacl/importers
pub async fn list_importers() -> impl IntoResponse {
    Json(
        importers()
            .iter()
            .map(|i| ImporterInfo {
                id: i.id(),
                label: i.label(),
                media_types: i.media_types(),
            })
            .collect::<Vec<_>>(),
    )
}

#[derive(Debug, Default, Deserialize)]
pub struct ImportQuery {
    /// Create a SHACL Studio shape graph from the result.
    #[serde(default)]
    pub create: bool,
    pub name: Option<String>,
    pub visibility: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ImportResponse {
    pub format: &'static str,
    #[serde(flatten)]
    pub shapes: ImportedShapes,
    pub shape_graph: Option<crate::shacl_studio::models::ShapeGraph>,
}

/// POST /api/shacl/import/:format — body is the specification document.
pub async fn import_spec(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(format): Path<String>,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let imp = importer(&format).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!(
                "unknown specification format `{format}`; known: {}",
                importers()
                    .iter()
                    .map(|i| i.id())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })?;
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty body".to_string()));
    }
    let shapes = imp
        .import(&body)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, format!("{format}: {e}")))?;
    let shape_graph = if q.create {
        let name = q.name.clone().unwrap_or_else(|| shapes.title.clone());
        let (owner_type, owner_id) =
            crate::shacl_studio::handlers::resolve_owner(&state, &user, &None, &None)?;
        Some(
            crate::shacl_studio::handlers::create_shape_graph_from_turtle(
                &state,
                &user,
                &name,
                shapes.description.as_deref(),
                owner_type,
                &owner_id,
                crate::shacl_studio::handlers::parse_visibility(&q.visibility),
                std::slice::from_ref(&format),
                crate::shacl_studio::models::ShapeSource::Imported,
                &shapes.turtle,
                &format!("Imported from {}", imp.label()),
            )?,
        )
    } else {
        None
    };
    Ok((
        if shape_graph.is_some() {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(ImportResponse {
            format: imp.id(),
            shapes,
            shape_graph,
        }),
    ))
}
