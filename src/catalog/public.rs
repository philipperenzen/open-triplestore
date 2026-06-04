//! `GET /api/public/catalog` — unauthenticated, structured JSON catalog of
//! **public + published** data-models and vocabularies, with full published
//! version history and the publisher (organisation / user) for each entry.
//!
//! Unlike `/api/catalog` (DCAT/Turtle, access-scoped) this is a stable JSON shape
//! designed for a public "Releases" page: the client builds download URLs from
//! `{id, version, format}` rather than parsing RDF.

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::auth::db::AuthDb;
use crate::server::AppState;

#[derive(Serialize)]
pub struct Publisher {
    pub user_id: String,
    pub username: String,
    pub display_name: Option<String>,
}

#[derive(Serialize)]
pub struct Owner {
    /// "organisation" | "user"
    #[serde(rename = "type")]
    pub kind: String,
    pub id: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct PublicVersion {
    pub version: String,
    pub status: String,
    pub created_at: String,
    pub notes: Option<String>,
    pub published_by: Option<Publisher>,
}

#[derive(Serialize)]
pub struct PublicEntry {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub description: Option<String>,
    pub namespace: Option<String>,
    pub latest_published: Option<String>,
    pub owner: Option<Owner>,
    /// Distinct users who authored the published versions.
    pub publishers: Vec<Publisher>,
    /// Published (and deprecated) versions, newest first.
    pub versions: Vec<PublicVersion>,
}

#[derive(Serialize)]
pub struct PublicCatalog {
    pub models: Vec<PublicEntry>,
    pub vocabularies: Vec<PublicEntry>,
}

/// A version row as handed to the builder (registry-agnostic).
pub struct VersionRow {
    pub version: String,
    pub status: String,
    pub created_at: String,
    pub notes: Option<String>,
    pub created_by: Option<String>,
}

fn resolve_publisher(auth_db: &AuthDb, iri: Option<&str>) -> Option<Publisher> {
    let uid = iri?.rsplit('/').next()?;
    let u = auth_db.get_user_by_id(uid).ok().flatten()?;
    Some(Publisher { user_id: u.id, username: u.username, display_name: u.display_name })
}

fn resolve_owner(auth_db: &AuthDb, owner_type: Option<&str>, owner_id: Option<&str>) -> Option<Owner> {
    match (owner_type, owner_id) {
        (Some("organisation"), Some(id)) => auth_db
            .get_organisation(id)
            .ok()
            .flatten()
            .map(|o| Owner { kind: "organisation".into(), id: o.id, name: o.name }),
        (Some("user"), Some(id)) => auth_db
            .get_user_by_id(id)
            .ok()
            .flatten()
            .map(|u| Owner { kind: "user".into(), id: u.id.clone(), name: u.display_name.unwrap_or(u.username) }),
        _ => None,
    }
}

/// Build one catalog entry from already-fetched version rows (newest-first).
#[allow(clippy::too_many_arguments)]
fn build_entry(
    auth_db: &AuthDb,
    kind: &str,
    id: String,
    title: String,
    description: Option<String>,
    namespace: Option<String>,
    owner_type: Option<&str>,
    owner_id: Option<&str>,
    latest_published: Option<String>,
    rows: Vec<VersionRow>,
) -> PublicEntry {
    // Keep only published / deprecated versions for the public history.
    let published: Vec<&VersionRow> = rows
        .iter()
        .filter(|r| r.status == "published" || r.status == "deprecated")
        .collect();

    let mut seen = std::collections::HashSet::new();
    let mut publishers = Vec::new();
    for r in &published {
        if let Some(p) = resolve_publisher(auth_db, r.created_by.as_deref()) {
            if seen.insert(p.user_id.clone()) {
                publishers.push(p);
            }
        }
    }

    let versions = published
        .iter()
        .map(|r| PublicVersion {
            version: r.version.clone(),
            status: r.status.clone(),
            created_at: r.created_at.clone(),
            notes: r.notes.clone(),
            published_by: resolve_publisher(auth_db, r.created_by.as_deref()),
        })
        .collect();

    PublicEntry {
        id,
        kind: kind.to_string(),
        title,
        description,
        namespace,
        latest_published,
        owner: resolve_owner(auth_db, owner_type, owner_id),
        publishers,
        versions,
    }
}

/// GET /api/public/catalog
pub async fn serve_public_catalog(State(state): State<AppState>) -> Json<PublicCatalog> {
    let auth_db = state.auth_db.as_ref();

    let models = crate::data_models::registry::list_data_models(&state.store)
        .into_iter()
        .filter(|m| m.is_public && m.latest_published.is_some())
        .map(|m| {
            let rows = crate::data_models::registry::list_versions(&state.store, &state.base_url, &m.id)
                .into_iter()
                .map(|v| VersionRow {
                    version: v.version,
                    status: v.status.as_str().to_string(),
                    created_at: v.created_at,
                    notes: v.notes,
                    created_by: v.created_by,
                })
                .collect();
            build_entry(
                auth_db, "data-model", m.id, m.title, m.description, Some(m.namespace),
                m.owner_type.as_deref(), m.owner_id.as_deref(), m.latest_published, rows,
            )
        })
        .collect();

    let vocabularies = crate::vocabularies::registry::list_vocabularies(&state.store)
        .into_iter()
        .filter(|v| v.is_public && v.latest_published.is_some())
        .map(|voc| {
            let rows = crate::vocabularies::registry::list_versions(&state.store, &state.base_url, &voc.id)
                .into_iter()
                .map(|v| VersionRow {
                    version: v.version,
                    status: v.status.as_str().to_string(),
                    created_at: v.created_at,
                    notes: v.notes,
                    created_by: v.created_by,
                })
                .collect();
            build_entry(
                auth_db, "vocabulary", voc.id, voc.title, voc.description, Some(voc.namespace),
                voc.owner_type.as_deref(), voc.owner_id.as_deref(), voc.latest_published, rows,
            )
        })
        .collect();

    Json(PublicCatalog { models, vocabularies })
}
