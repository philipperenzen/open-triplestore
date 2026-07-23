//! HTTP API for the vocabulary search service.
//!
//! Endpoint family and response envelopes follow the LOV API v2
//! (`/api/v2/term/search`, `/api/v2/vocabulary/search`, …) so existing LOV
//! clients port with a path change, with OTS extensions: `source` filters
//! (platform vs LOV), install-state on vocabulary records, the CLARIAH-style
//! recommender, and an offline install endpoint.
//!
//! Catalog-level endpoints (vocabulary list/info/search/autocomplete/tags)
//! work on every build.  Term-level search and the recommender need the
//! `vocab-search` feature (Tantivy) and answer `503` without it, mirroring
//! the text-search feature's behaviour.

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::server::error::AppError;
use crate::server::AppState;

use super::catalog::{CatalogEntry, CatalogSource, PlatformVocab};
use super::corpus::TermType;

type MaybeUser = Option<axum::extract::Extension<crate::auth::middleware::AuthenticatedUser>>;

/// Visibility predicate for the caller: anonymous users see public entries;
/// authenticated users additionally see registry entries they can access
/// (their own private vocabularies, org models, everything for admins).
fn viewer<'a>(state: &'a AppState, user: &'a MaybeUser) -> impl Fn(&PlatformVocab) -> bool + 'a {
    move |v: &PlatformVocab| {
        if v.is_public {
            return true;
        }
        let Some(u) = user.as_deref() else {
            return false;
        };
        state
            .auth_db
            .can_access_ontology(
                Some(&u.user_id),
                v.is_public,
                v.owner_type.as_deref(),
                v.owner_id.as_deref(),
            )
            .unwrap_or(false)
    }
}

pub fn vocab_public_routes() -> Router<AppState> {
    Router::new()
        .route("/api/vocab/list", get(list_vocabs))
        .route("/api/vocab/info", get(vocab_info))
        .route("/api/vocab/tags", get(vocab_tags))
        .route("/api/vocab/search", get(vocab_search))
        .route("/api/vocab/autocomplete", get(vocab_autocomplete))
        .route("/api/vocab/status", get(vocab_status))
        .route("/api/vocab/terms/search", get(terms_search))
        .route("/api/vocab/terms/autocomplete", get(terms_autocomplete))
        .route("/api/vocab/terms/suggest", get(terms_suggest))
        .route("/api/vocab/recommend", post(recommend_terms))
}

/// Admin-gated routes (mounted behind require_admin + require_auth).
pub fn vocab_admin_routes() -> Router<AppState> {
    Router::new().route("/api/vocab/install", post(install_vocab))
}

// ─── Freshness ───────────────────────────────────────────────────────────────

/// Single-flight guard for background refreshes (process-wide: one refresh at
/// a time regardless of how many requests observe the dirty flag).
static REFRESH_IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Kick off a background rebuild of registry-derived state when models or
/// vocabularies changed.  Non-blocking: the current request is served from
/// the previous (stale) snapshot — the refresh includes full-store usage
/// scans and version-graph extraction, which must never sit on the request
/// path.  The dirty flag is only cleared after a successful refresh, and a
/// mutation that lands mid-refresh re-marks it, so staleness always heals.
pub(crate) async fn ensure_fresh(state: &AppState) {
    use std::sync::atomic::Ordering;
    if !state.vocab_registry_dirty.load(Ordering::Relaxed) {
        return;
    }
    if REFRESH_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        return; // a refresh is already running
    }
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        // Claim the current dirty generation before reading, so a write that
        // arrives during the refresh re-dirties and triggers another pass.
        state.vocab_registry_dirty.store(false, Ordering::Relaxed);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::refresh_platform_state(&state);
        }));
        if result.is_err() {
            tracing::warn!("vocab platform refresh panicked — will retry on next request");
            state.vocab_registry_dirty.store(true, Ordering::Relaxed);
        }
        REFRESH_IN_FLIGHT.store(false, Ordering::Release);
    });
}

// ─── Catalog endpoints ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct ListResponse {
    total: usize,
    source: CatalogSource,
    vocabularies: Vec<CatalogEntry>,
}

async fn list_vocabs(State(state): State<AppState>, user: MaybeUser) -> Json<ListResponse> {
    ensure_fresh(&state).await;
    let vocabularies = state.vocab_catalog.list(&viewer(&state, &user));
    Json(ListResponse {
        total: vocabularies.len(),
        source: state.vocab_catalog.source().clone(),
        vocabularies,
    })
}

#[derive(Deserialize)]
struct InfoParams {
    vocab: String,
}

async fn vocab_info(
    State(state): State<AppState>,
    user: MaybeUser,
    Query(params): Query<InfoParams>,
) -> Result<Json<CatalogEntry>, AppError> {
    ensure_fresh(&state).await;
    state
        .vocab_catalog
        .info(&params.vocab, &viewer(&state, &user))
        .map(Json)
        .ok_or_else(|| AppError::NotFound(format!("Unknown vocabulary {:?}", params.vocab)))
}

#[derive(Serialize)]
struct TagCount {
    tag: String,
    count: usize,
}

async fn vocab_tags(State(state): State<AppState>) -> Json<Vec<TagCount>> {
    Json(
        state
            .vocab_catalog
            .tags()
            .into_iter()
            .map(|(tag, count)| TagCount { tag, count })
            .collect(),
    )
}

fn comma_list(raw: &Option<String>) -> Vec<String> {
    raw.as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct VocabSearchParams {
    #[serde(default)]
    q: String,
    tag: Option<String>,
    lang: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
}

#[derive(Serialize)]
struct VocabSearchResponse {
    total_results: usize,
    page: usize,
    page_size: usize,
    #[serde(rename = "queryString")]
    query_string: String,
    results: Vec<CatalogEntry>,
}

async fn vocab_search(
    State(state): State<AppState>,
    user: MaybeUser,
    Query(params): Query<VocabSearchParams>,
) -> Json<VocabSearchResponse> {
    ensure_fresh(&state).await;
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(15).clamp(1, 100);
    let tags = comma_list(&params.tag);
    let langs = comma_list(&params.lang);
    let (total, results) = state.vocab_catalog.search(
        &params.q,
        &tags,
        &langs,
        page_size,
        (page - 1) * page_size,
        &viewer(&state, &user),
    );
    Json(VocabSearchResponse {
        total_results: total,
        page,
        page_size,
        query_string: params.q,
        results,
    })
}

#[derive(Deserialize)]
struct AutocompleteParams {
    q: String,
    page_size: Option<usize>,
    #[serde(rename = "type")]
    types: Option<String>,
}

async fn vocab_autocomplete(
    State(state): State<AppState>,
    user: MaybeUser,
    Query(params): Query<AutocompleteParams>,
) -> Json<Vec<CatalogEntry>> {
    ensure_fresh(&state).await;
    let limit = params.page_size.unwrap_or(10).clamp(1, 50);
    Json(
        state
            .vocab_catalog
            .autocomplete(&params.q, limit, &viewer(&state, &user)),
    )
}

// ─── Status ──────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct StatusResponse {
    catalog_vocabularies: usize,
    prefix_dataset_size: usize,
    corpus_available: bool,
    term_search_enabled: bool,
    #[cfg(feature = "vocab-search")]
    engine: Option<super::index::EngineStatus>,
    source: CatalogSource,
}

async fn vocab_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let corpus_available = state
        .vocab_corpus
        .read()
        .map(|p| p.is_some())
        .unwrap_or(false);
    Json(StatusResponse {
        catalog_vocabularies: state.vocab_catalog.lov_len(),
        prefix_dataset_size: state.prefix_registry.dataset_len(),
        corpus_available,
        term_search_enabled: cfg!(feature = "vocab-search"),
        #[cfg(feature = "vocab-search")]
        engine: state.vocab_engine.as_ref().map(|e| e.status()),
        source: state.vocab_catalog.source().clone(),
    })
}

// ─── Term search (feature-gated) ─────────────────────────────────────────────

fn parse_types(raw: &Option<String>) -> Vec<TermType> {
    comma_list(raw)
        .iter()
        .filter_map(|t| TermType::parse(t))
        .collect()
}

#[derive(Deserialize)]
struct TermSearchParams {
    #[serde(default)]
    q: String,
    /// Comma-separated: class,property,datatype,instance (LOV default:
    /// class,property).
    #[serde(rename = "type")]
    types: Option<String>,
    vocab: Option<String>,
    tag: Option<String>,
    /// `platform` or `lov`.
    source: Option<String>,
    page: Option<usize>,
    page_size: Option<usize>,
}

#[cfg(feature = "vocab-search")]
async fn terms_search(
    State(state): State<AppState>,
    Query(params): Query<TermSearchParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    ensure_fresh(&state).await;
    let engine = state.vocab_engine.clone().ok_or_else(|| {
        AppError::ServiceUnavailable("Vocabulary search engine unavailable".into())
    })?;
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(10).clamp(1, 100);
    let mut types = parse_types(&params.types);
    if params.types.is_none() {
        // LOV default type filter for term search.
        types = vec![TermType::Class, TermType::Property];
    }
    let tags = comma_list(&params.tag);
    let source = params.source.as_deref().filter(|s| !s.is_empty());
    if let Some(s) = source {
        if s != "platform" && s != "lov" {
            return Err(AppError::BadRequest(format!(
                "Invalid source {s:?} (expected platform or lov)"
            )));
        }
    }
    let q = params.q.clone();
    let vocab = params.vocab.clone();
    let source_owned = source.map(str::to_string);
    let outcome = tokio::task::spawn_blocking(move || {
        engine.search_terms(
            &q,
            &types,
            vocab.as_deref(),
            &tags,
            source_owned.as_deref(),
            page,
            page_size,
        )
    })
    .await
    .map_err(|_| AppError::Internal("search task panicked".into()))?;
    let mut value = serde_json::to_value(&outcome)
        .map_err(|e| AppError::Internal(format!("serialize: {e}")))?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("page".into(), page.into());
        obj.insert("page_size".into(), page_size.into());
        obj.insert("queryString".into(), params.q.clone().into());
    }
    Ok(Json(value))
}

#[cfg(not(feature = "vocab-search"))]
async fn terms_search(
    State(_state): State<AppState>,
    Query(_params): Query<TermSearchParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    Err(feature_unavailable())
}

#[cfg(feature = "vocab-search")]
async fn terms_autocomplete(
    State(state): State<AppState>,
    Query(params): Query<AutocompleteParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    ensure_fresh(&state).await;
    let engine = state.vocab_engine.clone().ok_or_else(|| {
        AppError::ServiceUnavailable("Vocabulary search engine unavailable".into())
    })?;
    let limit = params.page_size.unwrap_or(10).clamp(1, 50);
    let types = parse_types(&params.types);
    let hits = engine.autocomplete(&params.q, &types, limit);
    Ok(Json(serde_json::json!({ "results": hits })))
}

#[cfg(not(feature = "vocab-search"))]
async fn terms_autocomplete(
    State(_state): State<AppState>,
    Query(_params): Query<AutocompleteParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    Err(feature_unavailable())
}

#[derive(Deserialize)]
struct SuggestParams {
    q: String,
    page_size: Option<usize>,
}

#[cfg(feature = "vocab-search")]
async fn terms_suggest(
    State(state): State<AppState>,
    Query(params): Query<SuggestParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    let engine = state.vocab_engine.clone().ok_or_else(|| {
        AppError::ServiceUnavailable("Vocabulary search engine unavailable".into())
    })?;
    let limit = params.page_size.unwrap_or(5).clamp(1, 20);
    let suggestions: Vec<serde_json::Value> = engine
        .suggest(&params.q, limit)
        .into_iter()
        .map(|(text, score)| serde_json::json!({ "text": text, "score": score }))
        .collect();
    Ok(Json(serde_json::json!({ "suggestions": suggestions })))
}

#[cfg(not(feature = "vocab-search"))]
async fn terms_suggest(
    State(_state): State<AppState>,
    Query(_params): Query<SuggestParams>,
) -> Result<Json<serde_json::Value>, AppError> {
    Err(feature_unavailable())
}

// ─── Recommender (feature-gated) ─────────────────────────────────────────────

#[cfg(feature = "vocab-search")]
async fn recommend_terms(
    State(state): State<AppState>,
    Json(request): Json<super::recommend::RecommendRequest>,
) -> Result<Json<super::recommend::RecommendResponse>, AppError> {
    ensure_fresh(&state).await;
    if request.terms.is_empty() {
        return Err(AppError::BadRequest("terms must not be empty".into()));
    }
    if request.terms.len() > 50 {
        return Err(AppError::BadRequest("at most 50 terms per request".into()));
    }
    let engine = state.vocab_engine.clone().ok_or_else(|| {
        AppError::ServiceUnavailable("Vocabulary search engine unavailable".into())
    })?;
    let response =
        tokio::task::spawn_blocking(move || super::recommend::recommend(&engine, &request))
            .await
            .map_err(|_| AppError::Internal("recommend task panicked".into()))?;
    Ok(Json(response))
}

#[cfg(not(feature = "vocab-search"))]
async fn recommend_terms(
    State(_state): State<AppState>,
    Json(_request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, AppError> {
    Err(feature_unavailable())
}

#[cfg(not(feature = "vocab-search"))]
fn feature_unavailable() -> AppError {
    AppError::ServiceUnavailable("Vocabulary term search requires the vocab-search feature".into())
}

// ─── Install (admin) ─────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct InstallRequest {
    /// Prefix, ontology URI or namespace of the LOV vocabulary to install.
    vocab: String,
}

async fn install_vocab(
    State(state): State<AppState>,
    user: Option<axum::extract::Extension<crate::auth::middleware::AuthenticatedUser>>,
    Json(request): Json<InstallRequest>,
) -> Result<Json<super::install::InstallOutcome>, AppError> {
    ensure_fresh(&state).await;
    let corpus_path = state.vocab_corpus.read().ok().and_then(|p| p.clone());
    // Registry convention: creators are stored as user IRIs (see
    // data_models::handlers::create_data_model), not bare ids.
    let installed_by = user.map(|u| format!("{}/users/{}", state.base_url, u.user_id));
    let state_bg = state.clone();
    let vocab = request.vocab.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        super::install::install_lov_vocab(
            &state_bg.store,
            &state_bg.base_url,
            &state_bg.vocab_catalog,
            corpus_path.as_deref(),
            &vocab,
            installed_by.as_deref(),
        )
    })
    .await
    .map_err(|_| AppError::Internal("install task panicked".into()))?;

    match outcome {
        Ok(result) => {
            state.auth_db.invalidate_accessible_graphs_cache();
            // Refresh synchronously (install is a rare admin action) so the
            // response's caller immediately sees the vocabulary as installed;
            // refresh_platform_state swaps overlays atomically, so a
            // concurrent background refresh is harmless.
            state
                .vocab_registry_dirty
                .store(false, std::sync::atomic::Ordering::Relaxed);
            let state_bg = state.clone();
            let refreshed = tokio::task::spawn_blocking(move || {
                super::refresh_platform_state(&state_bg);
            })
            .await;
            if refreshed.is_err() {
                state.mark_vocab_registry_dirty();
            }
            Ok(Json(result))
        }
        Err(e) => Err(match e {
            super::install::InstallError::UnknownVocab(_) => AppError::NotFound(e.to_string()),
            super::install::InstallError::NotInCorpus(_)
            | super::install::InstallError::CorpusUnavailable => {
                AppError::ServiceUnavailable(e.to_string())
            }
            super::install::InstallError::AlreadyInstalled(_) => {
                AppError::Conflict(serde_json::json!({ "error": e.to_string() }))
            }
            super::install::InstallError::Internal(msg) => AppError::Internal(msg),
        }),
    }
}
