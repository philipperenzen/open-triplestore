//! In-app documentation system: a small, DB-backed, admin-editable docs store.
//!
//! Docs live in the `docs` table (created in `AuthDb::migrate`). Built-in docs
//! are seeded from the repo's `docs/*.md` with `source = "builtin"` and are
//! re-seeded on boot **unless** an admin has edited them (which flips `source`
//! to `"user"`, so user edits are never clobbered).
//!
//! **Role gating is backend-enforced**: `admin_only = 1` docs are filtered out of
//! the public listing and return `404` (not `403`, so their existence isn't even
//! revealed) for non-admins. Create/update/delete require an admin/super_admin.
//! This is the single, reusable docs mechanism the frontend renders from.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Extension, Json, Router,
};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;

type ApiErr = (StatusCode, String);
type OptUser = Option<Extension<AuthenticatedUser>>;

fn is_admin(user: &OptUser) -> bool {
    user.as_ref().map(|u| u.is_admin()).unwrap_or(false)
}

/// A documentation page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Doc {
    pub slug: String,
    pub title: String,
    pub category: Option<String>,
    /// Markdown body. Omitted (empty) in list responses for size.
    #[serde(default)]
    pub body_md: String,
    pub admin_only: bool,
    /// "builtin" (seeded, re-seedable) or "user" (edited — never clobbered).
    pub source: String,
    pub sort_order: i64,
    pub updated_by: Option<String>,
    pub updated_at: String,
}

/// SQLite-backed docs persistence (reuses the auth DB pool).
pub struct DocStore {
    pool: Pool<SqliteConnectionManager>,
}

fn read_doc(row: &rusqlite::Row, with_body: bool) -> rusqlite::Result<Doc> {
    Ok(Doc {
        slug: row.get(0)?,
        title: row.get(1)?,
        category: row.get(2)?,
        body_md: if with_body { row.get(3)? } else { String::new() },
        admin_only: row.get::<_, i64>(4)? != 0,
        source: row.get(5)?,
        sort_order: row.get(6)?,
        updated_by: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

const COLS: &str = "slug, title, category, body_md, admin_only, source, sort_order, updated_by, updated_at";

impl DocStore {
    pub fn new(pool: Pool<SqliteConnectionManager>) -> Self {
        Self { pool }
    }

    /// List docs (bodies omitted). Admin-only docs are included only for admins.
    pub fn list(&self, include_admin: bool) -> anyhow::Result<Vec<Doc>> {
        let conn = self.pool.get()?;
        let sql = format!(
            "SELECT {COLS} FROM docs {} ORDER BY sort_order, title",
            if include_admin { "" } else { "WHERE admin_only = 0" }
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| read_doc(r, false))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Fetch one doc (with body). Returns `None` if absent — or, for an
    /// admin-only doc requested by a non-admin, also `None` (callers map this to
    /// a 404 so existence isn't leaked).
    pub fn get(&self, slug: &str, include_admin: bool) -> anyhow::Result<Option<Doc>> {
        let conn = self.pool.get()?;
        let doc = conn
            .query_row(&format!("SELECT {COLS} FROM docs WHERE slug = ?1"), params![slug], |r| read_doc(r, true))
            .optional()?;
        Ok(doc.filter(|d| include_admin || !d.admin_only))
    }

    /// Create or replace a user-authored doc (sets `source = "user"`).
    pub fn upsert(&self, doc: &Doc, updated_by: Option<&str>) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO docs (slug, title, category, body_md, admin_only, source, sort_order, updated_by, created_at, updated_at) \
             VALUES (?1,?2,?3,?4,?5,'user',?6,?7,?8,?8) \
             ON CONFLICT(slug) DO UPDATE SET title=excluded.title, category=excluded.category, \
               body_md=excluded.body_md, admin_only=excluded.admin_only, source='user', \
               sort_order=excluded.sort_order, updated_by=excluded.updated_by, updated_at=excluded.updated_at",
            params![
                doc.slug, doc.title, doc.category, doc.body_md, doc.admin_only as i64,
                doc.sort_order, updated_by, now
            ],
        )?;
        Ok(())
    }

    pub fn delete(&self, slug: &str) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        conn.execute("DELETE FROM docs WHERE slug = ?1", params![slug])?;
        Ok(())
    }

    /// Seed a built-in doc: insert when absent, or refresh it when it is still
    /// `source = "builtin"` (i.e. a user hasn't taken it over). User edits win.
    fn seed_one(&self, slug: &str, title: &str, category: &str, body: &str, admin_only: bool, sort: i64) -> anyhow::Result<()> {
        let conn = self.pool.get()?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO docs (slug, title, category, body_md, admin_only, source, sort_order, updated_by, created_at, updated_at) \
             VALUES (?1,?2,?3,?4,?5,'builtin',?6,NULL,?7,?7) \
             ON CONFLICT(slug) DO UPDATE SET title=excluded.title, category=excluded.category, \
               body_md=excluded.body_md, admin_only=excluded.admin_only, sort_order=excluded.sort_order, \
               updated_at=excluded.updated_at \
             WHERE docs.source = 'builtin'",
            params![slug, title, category, body, admin_only as i64, sort, now],
        )?;
        Ok(())
    }
}

// ── Built-in docs (compiled in from the repo's docs/*.md) ──────────────────────

struct Builtin {
    slug: &'static str,
    title: &'static str,
    category: &'static str,
    body: &'static str,
    admin_only: bool,
    sort: i64,
}

/// The admin/super-admin-only governance doc: describes the dataset-structure
/// SHACL model, the standards shapes/pipelines, derived-data write targets, and
/// the metadata audit. Hidden (404) for non-admins.
const DATASET_GOVERNANCE_MD: &str = include_str!("dataset-governance.md");

// Ordering: docs are grouped by `category` (the UI orders groups by their
// lowest `sort`) and then by `sort` within a group. Keep the sort values
// spaced so new docs can be slotted in without renumbering.
const BUILTINS: &[Builtin] = &[
    // ── Introduction ──
    Builtin { slug: "overview", title: "Platform Overview", category: "Introduction",
              body: include_str!("../../docs/overview.md"), admin_only: false, sort: 0 },

    // ── Concepts ──
    Builtin { slug: "named-graphs", title: "Named Graphs", category: "Concepts",
              body: include_str!("../../docs/named-graphs.md"), admin_only: false, sort: 10 },
    Builtin { slug: "datasets", title: "Datasets", category: "Concepts",
              body: include_str!("../../docs/datasets.md"), admin_only: false, sort: 12 },
    Builtin { slug: "organisations", title: "Organisations", category: "Concepts",
              body: include_str!("../../docs/organisations.md"), admin_only: false, sort: 14 },
    Builtin { slug: "versioning", title: "Dataset Versioning & Sharing", category: "Concepts",
              body: include_str!("../../docs/versioning.md"), admin_only: false, sort: 16 },

    // ── Modelling ──
    Builtin { slug: "modelling", title: "Linked Data Modelling", category: "Modelling",
              body: include_str!("../../docs/modelling.md"), admin_only: false, sort: 30 },
    Builtin { slug: "data-modeling", title: "Data Modeling Architecture", category: "Modelling",
              body: include_str!("../../docs/data-modeling.md"), admin_only: false, sort: 32 },
    Builtin { slug: "linked-data-modelling-styleguide", title: "Modelling Styleguide", category: "Modelling",
              body: include_str!("../../docs/linked-data-modelling-styleguide.md"), admin_only: false, sort: 34 },
    Builtin { slug: "models", title: "Model & Vocabulary Versioning", category: "Modelling",
              body: include_str!("../../docs/models.md"), admin_only: false, sort: 36 },
    Builtin { slug: "dcat", title: "DCAT Catalogue", category: "Modelling",
              body: include_str!("../../docs/dcat.md"), admin_only: false, sort: 38 },

    // ── Data Exchange ──
    Builtin { slug: "formats", title: "RDF Formats", category: "Data Exchange",
              body: include_str!("../../docs/formats.md"), admin_only: false, sort: 50 },
    Builtin { slug: "import", title: "Import Auto-Detection", category: "Data Exchange",
              body: include_str!("../../docs/import.md"), admin_only: false, sort: 52 },

    // ── Query & Search ──
    Builtin { slug: "search-syntax", title: "Browse & Search Syntax", category: "Query & Search",
              body: include_str!("../../docs/search-syntax.md"), admin_only: false, sort: 60 },
    Builtin { slug: "full-text-search", title: "Full-text Search", category: "Query & Search",
              body: include_str!("../../docs/full-text-search.md"), admin_only: false, sort: 62 },
    Builtin { slug: "geosparql", title: "GeoSPARQL", category: "Query & Search",
              body: include_str!("../../docs/geosparql.md"), admin_only: false, sort: 64 },

    // ── Reasoning & Validation ──
    Builtin { slug: "reasoning", title: "OWL Reasoning", category: "Reasoning & Validation",
              body: include_str!("../../docs/reasoning.md"), admin_only: false, sort: 80 },
    Builtin { slug: "shacl", title: "SHACL Validation", category: "Reasoning & Validation",
              body: include_str!("../../docs/shacl.md"), admin_only: false, sort: 82 },

    // ── Security ──
    Builtin { slug: "auth", title: "Authentication & API Tokens", category: "Security",
              body: include_str!("../../docs/auth.md"), admin_only: false, sort: 100 },
    Builtin { slug: "security", title: "Security & Access Control", category: "Security",
              body: include_str!("../../docs/security.md"), admin_only: false, sort: 102 },

    // ── API & Operations ──
    Builtin { slug: "api-services", title: "API Services & AI Queries", category: "API & Operations",
              body: include_str!("../../docs/api-services.md"), admin_only: false, sort: 110 },
    Builtin { slug: "api-reference", title: "API Reference", category: "API & Operations",
              body: include_str!("../../docs/api-reference.md"), admin_only: false, sort: 112 },
    Builtin { slug: "operations", title: "Operations", category: "API & Operations",
              body: include_str!("../../docs/operations.md"), admin_only: false, sort: 114 },

    // ── Reference ──
    Builtin { slug: "standards", title: "Supported Standards", category: "Reference",
              body: include_str!("../../docs/standards.md"), admin_only: false, sort: 130 },
    Builtin { slug: "datatypes", title: "Datatypes", category: "Reference",
              body: include_str!("../../docs/datatypes.md"), admin_only: false, sort: 132 },
    Builtin { slug: "faq", title: "Frequently Asked Questions", category: "Reference",
              body: include_str!("../../docs/faq.md"), admin_only: false, sort: 140 },

    // ── Administration (admin-only; 404 for non-admins) ──
    Builtin { slug: "dataset-governance", title: "Dataset Governance & SHACL Model (Admin)",
              category: "Administration", body: DATASET_GOVERNANCE_MD, admin_only: true, sort: 200 },
];

/// Idempotently seed the built-in docs. User-edited docs are preserved.
pub fn seed_builtin_docs(auth_db: &crate::auth::db::AuthDb) -> anyhow::Result<()> {
    let store = DocStore::new(auth_db.pool());
    for b in BUILTINS {
        store.seed_one(b.slug, b.title, b.category, b.body, b.admin_only, b.sort)?;
    }
    tracing::info!("docs: seeded {} built-in docs", BUILTINS.len());
    Ok(())
}

// ── HTTP handlers (optional auth; admin enforced in-handler) ────────────────────

fn store_of(state: &AppState) -> DocStore {
    DocStore::new(state.auth_db.pool())
}

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    // Log the detail server-side but return a generic body — never echo raw
    // rusqlite/anyhow text (which can leak schema/constraint names) to clients.
    tracing::error!("docs handler internal error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
}

pub async fn list_docs(State(state): State<AppState>, user: OptUser) -> Result<Json<Vec<Doc>>, ApiErr> {
    let docs = store_of(&state).list(is_admin(&user)).map_err(e500)?;
    Ok(Json(docs))
}

pub async fn get_doc(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    user: OptUser,
) -> Result<Json<Doc>, ApiErr> {
    match store_of(&state).get(&slug, is_admin(&user)).map_err(e500)? {
        Some(d) => Ok(Json(d)),
        // 404 (not 403) so an admin-only doc's existence is not revealed.
        None => Err((StatusCode::NOT_FOUND, "Doc not found".into())),
    }
}

#[derive(Deserialize)]
pub struct DocInput {
    pub title: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub body_md: String,
    #[serde(default)]
    pub admin_only: bool,
    #[serde(default)]
    pub sort_order: Option<i64>,
}

fn require_admin(user: &OptUser) -> Result<&AuthenticatedUser, ApiErr> {
    match user.as_ref() {
        Some(Extension(u)) if u.is_admin() => Ok(u),
        Some(_) => Err((StatusCode::FORBIDDEN, "Admin access required".into())),
        None => Err((StatusCode::UNAUTHORIZED, "Authentication required".into())),
    }
}

pub async fn create_doc(
    State(state): State<AppState>,
    user: OptUser,
    Path(slug): Path<String>,
    Json(body): Json<DocInput>,
) -> Result<Json<Doc>, ApiErr> {
    let admin = require_admin(&user)?;
    let doc = Doc {
        slug,
        title: body.title,
        category: body.category,
        body_md: body.body_md,
        admin_only: body.admin_only,
        source: "user".into(),
        sort_order: body.sort_order.unwrap_or(100),
        updated_by: Some(admin.user_id.clone()),
        updated_at: String::new(),
    };
    let store = store_of(&state);
    store.upsert(&doc, Some(&admin.user_id)).map_err(e500)?;
    let saved = store.get(&doc.slug, true).map_err(e500)?.ok_or_else(|| e500("doc vanished after upsert"))?;
    Ok(Json(saved))
}

pub async fn delete_doc(
    State(state): State<AppState>,
    user: OptUser,
    Path(slug): Path<String>,
) -> Result<StatusCode, ApiErr> {
    require_admin(&user)?;
    store_of(&state).delete(&slug).map_err(e500)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Router for the docs API. GET is optional-auth (public sees non-admin docs);
/// create/update/delete enforce admin in-handler.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/docs", get(list_docs))
        .route("/api/docs/:slug", get(get_doc).put(create_doc).post(create_doc).delete(delete_doc))
}
