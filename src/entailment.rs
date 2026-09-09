//! Selectable entailment regimes per dataset, with a materialisation toggle.
//!
//! A dataset picks a regime (`rdfs`, `owl2-rl`, `owl2-el`, `owl2-ql`,
//! `owl2-dl`) and a mode:
//!
//! * `materialize` — after every write to one of the dataset's graphs, the
//!   regime is re-run over the dataset's conformance layer (its instance,
//!   model, vocabulary, domain-value and linkset graphs) into the dataset's
//!   own entailment graph `urn:entailment:<regime>:<dataset>`, so tenants
//!   never share inferred triples and consequences of deleted data never
//!   linger;
//! * `off` — the entailment graph is cleared and no longer maintained.
//!
//! Queries opt in with `?entailment_dataset=<id>` (plus `?entailment=<regime>`
//! to pick a regime other than the configured one): the dataset's entailment
//! graph joins the query's default graph, exactly as the global
//! `?entailment=` graphs do.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::db::AuthDb;
use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;

pub const REGIMES: &[&str] = &["rdfs", "owl2-rl", "owl2-el", "owl2-ql", "owl2-dl"];

#[derive(Debug, Clone, Serialize)]
pub struct EntailmentConfig {
    pub dataset_id: String,
    pub regime: String,
    /// `materialize` | `off`
    pub mode: String,
    pub graph: String,
    pub updated_at: String,
    pub last_run_at: Option<String>,
    pub last_triples: Option<i64>,
}

pub fn dataset_entailment_graph(regime: &str, dataset_id: &str) -> String {
    format!("urn:entailment:{regime}:{dataset_id}")
}

pub fn config(db: &AuthDb, dataset_id: &str) -> anyhow::Result<Option<EntailmentConfig>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            "SELECT regime, mode, updated_at, last_run_at, last_triples FROM dataset_entailment WHERE dataset_id = ?1",
            params![dataset_id],
            |r| {
                let regime: String = r.get(0)?;
                Ok(EntailmentConfig {
                    dataset_id: dataset_id.to_string(),
                    graph: dataset_entailment_graph(&regime, dataset_id),
                    regime,
                    mode: r.get(1)?,
                    updated_at: r.get(2)?,
                    last_run_at: r.get(3)?,
                    last_triples: r.get(4)?,
                })
            },
        )
        .optional()?)
}

fn set_config(db: &AuthDb, dataset_id: &str, regime: &str, mode: &str) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    conn.execute(
        "INSERT INTO dataset_entailment (dataset_id, regime, mode, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(dataset_id) DO UPDATE SET regime = excluded.regime, mode = excluded.mode, updated_at = excluded.updated_at",
        params![dataset_id, regime, mode, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

fn record_run(db: &AuthDb, dataset_id: &str, triples: i64) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    conn.execute(
        "UPDATE dataset_entailment SET last_run_at = ?2, last_triples = ?3 WHERE dataset_id = ?1",
        params![dataset_id, chrono::Utc::now().to_rfc3339(), triples],
    )?;
    Ok(())
}

/// Datasets in `materialize` mode that own any of `graphs`.
fn materialized_datasets_for(
    db: &AuthDb,
    graphs: &[String],
) -> anyhow::Result<Vec<(String, String)>> {
    if graphs.is_empty() {
        return Ok(Vec::new());
    }
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT e.dataset_id, e.regime FROM dataset_entailment e \
         JOIN dataset_graphs g ON g.dataset_id = e.dataset_id \
         WHERE e.mode = 'materialize' AND g.graph_iri = ?1",
    )?;
    let mut out: Vec<(String, String)> = Vec::new();
    for g in graphs {
        // A dataset's own entailment graph is never a trigger.
        if g.starts_with("urn:entailment:") {
            continue;
        }
        for row in stmt.query_map(params![g], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let row = row?;
            if !out.contains(&row) {
                out.push(row);
            }
        }
    }
    Ok(out)
}

/// Re-materialise `regime` for `dataset_id` into its entailment graph.
/// Returns the number of triples in the entailment graph afterwards.
pub fn run_for_dataset(state: &AppState, dataset_id: &str, regime: &str) -> Result<i64, String> {
    let ds = state
        .auth_db
        .get_dataset(dataset_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("dataset {dataset_id} not found"))?;
    let sources = crate::conformance::resolve(state, &ds).reasoning_sources;
    let target = dataset_entailment_graph(regime, dataset_id);
    state
        .store
        .update(&format!("CLEAR SILENT GRAPH <{target}>"))
        .map_err(|e| format!("clearing <{target}>: {e}"))?;
    crate::server::routes::run_regime(state, regime, Some(sources), &target)
        .map_err(|e| format!("{e:?}"))?;
    let n = state.store.graph_count_cached(Some(&target)).unwrap_or(0) as i64;
    let _ = record_run(&state.auth_db, dataset_id, n);
    Ok(n)
}

/// After a write to `graphs`: re-materialise every dataset in `materialize`
/// mode that owns one of them. Synchronous — call it from a blocking context
/// (the write paths already sit in one for the LDES capture). Best-effort;
/// failures are logged.
pub fn after_write(state: &AppState, graphs: &[String]) {
    let targets = match materialized_datasets_for(&state.auth_db, graphs) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("entailment: lookup failed: {e}");
            return;
        }
    };
    for (ds, regime) in targets {
        match run_for_dataset(state, &ds, &regime) {
            Ok(n) => tracing::debug!("entailment: re-materialised {regime} for {ds}: {n} triples"),
            Err(e) => tracing::warn!("entailment: re-materialising {regime} for {ds} failed: {e}"),
        }
    }
}

// ── HTTP ────────────────────────────────────────────────────────────────────

type ApiErr = (StatusCode, String);

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn visible(
    state: &AppState,
    uid: Option<&str>,
    id: &str,
) -> Result<crate::auth::models::Dataset, ApiErr> {
    let ds = state
        .auth_db
        .get_dataset(id)
        .map_err(e500)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state.auth_db.can_access_dataset(uid, &ds).map_err(e500)? {
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }
    Ok(ds)
}

/// GET /api/datasets/:id/entailment
pub async fn get_entailment(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    visible(&state, uid, &dataset_id)?;
    match config(&state.auth_db, &dataset_id).map_err(e500)? {
        Some(c) => Ok(Json(serde_json::to_value(c).unwrap())),
        None => Ok(Json(serde_json::json!({
            "dataset_id": dataset_id,
            "regime": null,
            "mode": "off",
            "regimes": REGIMES,
        }))),
    }
}

#[derive(Debug, Deserialize)]
pub struct EntailmentBody {
    pub regime: String,
    /// `materialize` (default) | `off`
    #[serde(default)]
    pub mode: Option<String>,
}

/// PUT /api/datasets/:id/entailment — select a regime and mode; in
/// `materialize` mode the regime runs immediately.
pub async fn put_entailment(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<EntailmentBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = visible(&state, Some(&user.user_id), &dataset_id)?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    let regime = body.regime.trim().to_ascii_lowercase();
    if !REGIMES.contains(&regime.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("unknown regime `{regime}`; one of {}", REGIMES.join(", ")),
        ));
    }
    let mode = body
        .mode
        .as_deref()
        .unwrap_or("materialize")
        .trim()
        .to_ascii_lowercase();
    if mode != "materialize" && mode != "off" {
        return Err((
            StatusCode::BAD_REQUEST,
            "mode must be `materialize` or `off`".to_string(),
        ));
    }
    set_config(&state.auth_db, &dataset_id, &regime, &mode).map_err(e500)?;
    let graph = dataset_entailment_graph(&regime, &dataset_id);
    let st = state.clone();
    let id = dataset_id.clone();
    let r = regime.clone();
    let m = mode.clone();
    let g = graph.clone();
    let triples = tokio::task::spawn_blocking(move || -> Result<i64, String> {
        if m == "off" {
            st.store
                .update(&format!("CLEAR SILENT GRAPH <{g}>"))
                .map_err(|e| e.to_string())?;
            let _ = record_run(&st.auth_db, &id, 0);
            return Ok(0);
        }
        run_for_dataset(&st, &id, &r)
    })
    .await
    .map_err(e500)?
    .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "regime": regime,
        "mode": mode,
        "graph": graph,
        "triples": triples,
    })))
}
