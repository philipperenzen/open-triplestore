//! HTTP API for the internal prefix service.
//!
//! Public, read-only endpoints over the local prefix tiers (platform
//! registry, bundled prefix.cc/LOV snapshot, confirmed cache).  The URL
//! ergonomics follow prefix.cc where they help existing tooling (comma
//! multi-lookup, bulk export formats, a JSON-LD context) but always return a
//! body directly instead of prefix.cc's redirect responses.
//!
//! Mounted with the anonymous SPARQL rate-limit tier — everything here is
//! in-memory and cheap, but unauthenticated.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::server::error::AppError;
use crate::server::AppState;

use super::{is_valid_label, ResolvedPrefix};

pub fn prefix_routes() -> Router<AppState> {
    Router::new()
        .route("/api/prefixes", get(search_prefixes))
        .route("/api/prefixes/all", get(export_all))
        .route("/api/prefixes/context.jsonld", get(jsonld_context))
        .route("/api/prefixes/reverse", get(reverse_lookup))
        .route("/api/prefixes/expand", get(expand_curie))
        .route("/api/prefixes/shrink", get(shrink_iri))
        .route("/api/prefixes/:label", get(lookup_label))
}

// ─── Search ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SearchParams {
    /// Substring to match against labels and namespaces; empty lists by rank.
    #[serde(default)]
    q: String,
    limit: Option<usize>,
}

#[derive(Serialize)]
pub struct PrefixSearchResponse {
    pub total_known: usize,
    pub results: Vec<ResolvedPrefix>,
}

async fn search_prefixes(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Json<PrefixSearchResponse> {
    crate::vocab_search::routes::ensure_fresh(&state).await;
    let limit = params.limit.unwrap_or(25).min(200);
    Json(PrefixSearchResponse {
        total_known: state.prefix_registry.dataset_len(),
        results: state.prefix_registry.search(&params.q, limit),
    })
}

// ─── Forward lookup ──────────────────────────────────────────────────────────

async fn lookup_label(
    State(state): State<AppState>,
    Path(label): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    crate::vocab_search::routes::ensure_fresh(&state).await;
    // Comma multi-lookup, mirroring prefix.cc's /rdf,foaf,dc ergonomics.
    if label.contains(',') {
        let mut map = serde_json::Map::new();
        for part in label.split(',').filter(|p| !p.is_empty()).take(50) {
            if !is_valid_label(part) {
                continue;
            }
            if let Some(resolved) = state.prefix_registry.lookup_local(part) {
                map.insert(
                    part.to_string(),
                    serde_json::Value::String(resolved.namespace),
                );
            }
        }
        if map.is_empty() {
            return Err(AppError::NotFound("No matching prefixes".into()));
        }
        return Ok(Json(serde_json::Value::Object(map)));
    }

    if !is_valid_label(&label) {
        return Err(AppError::BadRequest(format!(
            "Invalid prefix label {label:?}"
        )));
    }
    // Local tiers first; the async path adds the opt-in prefix.cc fallback.
    let resolved = match state.prefix_registry.lookup_local(&label) {
        Some(r) => r,
        None => match state.prefix_registry.lookup_prefix(&label).await {
            Some(ns) => ResolvedPrefix {
                prefix: label.clone(),
                namespace: ns,
                source: super::PrefixSource::Cache,
            },
            None => return Err(AppError::NotFound(format!("Unknown prefix {label:?}"))),
        },
    };
    Ok(Json(serde_json::to_value(resolved).unwrap_or_default()))
}

// ─── Reverse lookup ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ReverseParams {
    uri: String,
}

async fn reverse_lookup(
    State(state): State<AppState>,
    Query(params): Query<ReverseParams>,
) -> Result<Json<ResolvedPrefix>, AppError> {
    crate::vocab_search::routes::ensure_fresh(&state).await;
    if let Some(resolved) = state.prefix_registry.reverse_local(&params.uri) {
        return Ok(Json(resolved));
    }
    // Namespace unknown locally: try the longest-prefix match so term IRIs
    // (…/foaf/0.1/name) resolve to their namespace, like prefix.cc /reverse.
    if let Some((resolved, _local)) = state.prefix_registry.shrink_iri(&params.uri) {
        return Ok(Json(resolved));
    }
    if let Some((label, ns)) = state.prefix_registry.reverse_lookup(&params.uri).await {
        return Ok(Json(ResolvedPrefix {
            prefix: label,
            namespace: ns,
            source: super::PrefixSource::Cache,
        }));
    }
    Err(AppError::NotFound(format!(
        "No registered prefix for {:?}",
        params.uri
    )))
}

// ─── CURIE expand / shrink ───────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ExpandParams {
    curie: String,
}

#[derive(Serialize)]
pub struct ExpandResponse {
    pub curie: String,
    pub iri: String,
}

async fn expand_curie(
    State(state): State<AppState>,
    Query(params): Query<ExpandParams>,
) -> Result<Json<ExpandResponse>, AppError> {
    match state.prefix_registry.expand_curie(&params.curie) {
        Some(iri) => Ok(Json(ExpandResponse {
            curie: params.curie,
            iri,
        })),
        None => Err(AppError::NotFound(format!(
            "Cannot expand {:?}",
            params.curie
        ))),
    }
}

#[derive(Deserialize)]
pub struct ShrinkParams {
    iri: String,
}

#[derive(Serialize)]
pub struct ShrinkResponse {
    pub iri: String,
    pub curie: String,
    pub prefix: String,
    pub namespace: String,
}

async fn shrink_iri(
    State(state): State<AppState>,
    Query(params): Query<ShrinkParams>,
) -> Result<Json<ShrinkResponse>, AppError> {
    match state.prefix_registry.shrink_iri(&params.iri) {
        Some((resolved, local)) => Ok(Json(ShrinkResponse {
            curie: format!("{}:{}", resolved.prefix, local),
            prefix: resolved.prefix,
            namespace: resolved.namespace,
            iri: params.iri,
        })),
        None => Err(AppError::NotFound(format!(
            "No known namespace covers {:?}",
            params.iri
        ))),
    }
}

// ─── Bulk export ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ExportParams {
    /// json | jsonld | ttl | sparql | csv | txt
    format: Option<String>,
}

async fn export_all(
    State(state): State<AppState>,
    Query(params): Query<ExportParams>,
) -> Result<Response, AppError> {
    crate::vocab_search::routes::ensure_fresh(&state).await;
    let entries = state.prefix_registry.all_prefixes();
    let format = params.format.as_deref().unwrap_or("json");
    let (body, content_type) = match format {
        "json" => {
            let map: serde_json::Map<String, serde_json::Value> = entries
                .iter()
                .map(|e| {
                    (
                        e.prefix.clone(),
                        serde_json::Value::String(e.namespace.clone()),
                    )
                })
                .collect();
            (
                serde_json::to_string_pretty(&map).unwrap_or_default(),
                "application/json",
            )
        }
        "jsonld" => (jsonld_context_body(&entries), "application/ld+json"),
        "ttl" => (
            entries
                .iter()
                .map(|e| format!("@prefix {}: <{}> .\n", e.prefix, e.namespace))
                .collect(),
            "text/turtle",
        ),
        "sparql" => (
            entries
                .iter()
                .map(|e| format!("PREFIX {}: <{}>\n", e.prefix, e.namespace))
                .collect(),
            "text/plain; charset=utf-8",
        ),
        "csv" => {
            // RFC 4180: quote fields containing delimiters (commas are legal,
            // un-encoded characters in namespace IRIs).
            fn csv_field(s: &str) -> std::borrow::Cow<'_, str> {
                if s.contains([',', '"', '\n', '\r']) {
                    std::borrow::Cow::Owned(format!("\"{}\"", s.replace('"', "\"\"")))
                } else {
                    std::borrow::Cow::Borrowed(s)
                }
            }
            let mut s = String::from("prefix,namespace,source\n");
            for e in &entries {
                let source = serde_json::to_value(e.source)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                s.push_str(&format!(
                    "{},{},{}\n",
                    csv_field(&e.prefix),
                    csv_field(&e.namespace),
                    csv_field(&source)
                ));
            }
            (s, "text/csv")
        }
        "txt" => (
            entries
                .iter()
                .map(|e| format!("{}\t{}\n", e.prefix, e.namespace))
                .collect(),
            "text/plain; charset=utf-8",
        ),
        other => {
            return Err(AppError::BadRequest(format!(
                "Unsupported format {other:?} (expected json, jsonld, ttl, sparql, csv or txt)"
            )))
        }
    };
    Ok(([(axum::http::header::CONTENT_TYPE, content_type)], body).into_response())
}

fn jsonld_context_body(entries: &[ResolvedPrefix]) -> String {
    let ctx: serde_json::Map<String, serde_json::Value> = entries
        .iter()
        .map(|e| {
            (
                e.prefix.clone(),
                serde_json::Value::String(e.namespace.clone()),
            )
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "@context": ctx })).unwrap_or_default()
}

async fn jsonld_context(State(state): State<AppState>) -> Response {
    let entries = state.prefix_registry.all_prefixes();
    (
        [(axum::http::header::CONTENT_TYPE, "application/ld+json")],
        jsonld_context_body(&entries),
    )
        .into_response()
}
