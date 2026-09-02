//! Constraint-specification importers: turn a domain's exchange-requirement
//! format into SHACL shapes that live in SHACL Studio like any other shape
//! graph. The interface is generic — an importer takes bytes and returns
//! Turtle plus a report — and buildingSMART IDS ([`ids`]) is the first
//! implementation; a FHIR-profile or any other importer is the same shape.
//!
//! * `GET  /api/shacl/importers`            — the registered formats
//! * `POST /api/shacl/import/:format`        — convert (and, with
//!   `?create=true`, create a shape graph from the result)

pub mod ids;

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
