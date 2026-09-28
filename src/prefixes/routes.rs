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
//!
//! `/api/prefixes/all` and `/api/prefixes/context.jsonld` re-serve the whole
//! bundled snapshot, so every deployment passes on third-party data: prefix.cc
//! mappings (no licence published; the operator has stated they are considered
//! CC0) and LOV-derived ones (CC BY 4.0, which asks for credit, the licence URI
//! and a note of modification). The Turtle and SPARQL exports open with the
//! snapshot's credit lines as comments, and the CSV export names each row's
//! source; JSON, JSON-LD and plain text have nowhere to put a credit without
//! changing what clients parse, so the docs and NOTICE carry it for those.

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::server::error::AppError;
use crate::server::AppState;

use super::{is_valid_label, PrefixRegistry, PrefixSource, ResolvedPrefix};

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
    let format = params.format.as_deref().unwrap_or("json");
    let (body, content_type) = render_export(&state.prefix_registry, format)?;
    Ok(([(axum::http::header::CONTENT_TYPE, content_type)], body).into_response())
}

/// `#` comment lines crediting the bundled third-party sources that `entries`
/// draw on, empty when none do. Turtle and SPARQL both allow comments, so the
/// credit travels with a saved export.
fn credit_comment(registry: &PrefixRegistry, entries: &[ResolvedPrefix]) -> String {
    let mut out = String::new();
    for source in [PrefixSource::PrefixCc, PrefixSource::Lov] {
        if !entries.iter().any(|e| e.source == source) {
            continue;
        }
        if let Some(credit) = registry.source_credit(source) {
            if out.is_empty() {
                out.push_str("# Includes prefix mappings from:\n");
            }
            // A line break would end the comment and leave the rest as syntax.
            out.push_str(&format!("# - {}\n", credit.replace(['\r', '\n'], " ")));
        }
    }
    out
}

/// Body and content type of one bulk-export format.
fn render_export(
    registry: &PrefixRegistry,
    format: &str,
) -> Result<(String, &'static str), AppError> {
    let entries = registry.all_prefixes();
    Ok(match format {
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
        "ttl" => {
            let mut s = credit_comment(registry, &entries);
            for e in &entries {
                s.push_str(&format!("@prefix {}: <{}> .\n", e.prefix, e.namespace));
            }
            (s, "text/turtle")
        }
        "sparql" => {
            let mut s = credit_comment(registry, &entries);
            for e in &entries {
                s.push_str(&format!("PREFIX {}: <{}>\n", e.prefix, e.namespace));
            }
            (s, "text/plain; charset=utf-8")
        }
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
    })
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

// ─── Administration ──────────────────────────────────────────────────────────
//
// What a prefix means *here*. The read side above is the community snapshot
// plus what this instance derives from its own datasets; this is a deployment
// stating, for example, that `geo` is its geo namespace and not the one
// prefix.cc lists. Overrides are stored in the identity database, so they
// survive a restart and reach a follower with the rest of it, and the label is
// that table's primary key — two prefixes cannot share a shorthand.
//
// Mounted admin-only. Repointing a prefix changes what every stored CURIE
// expands to, which is not a thing an ordinary user should be able to do to
// everyone else.

/// Reload the in-memory overlay from storage. Called after every write so the
/// process never disagrees with what is stored.
fn refresh_admin_overlay(state: &AppState) -> Result<(), AppError> {
    let stored = state
        .auth_db
        .list_prefix_overrides()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    state
        .prefix_registry
        .set_admin_prefixes(stored.into_iter().map(|o| (o.label, o.namespace)));
    Ok(())
}

/// Reject a label or namespace the registry would refuse anyway, with a reason
/// rather than a silent no-op.
fn validate(label: &str, namespace: &str) -> Result<(), AppError> {
    if !is_valid_label(label) {
        return Err(AppError::BadRequest(format!(
            "{label:?} is not a prefix label: start with a letter, then letters, \
             digits, '_' or '-'"
        )));
    }
    if !super::is_valid_iri(namespace) {
        return Err(AppError::BadRequest(format!(
            "{namespace:?} is not a usable namespace: an http or https IRI"
        )));
    }
    Ok(())
}

pub fn prefix_admin_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/admin/prefixes",
            get(list_overrides).post(create_override),
        )
        .route(
            "/api/admin/prefixes/:label",
            axum::routing::put(put_override).delete(delete_override),
        )
}

#[derive(Deserialize)]
pub struct CreateOverride {
    label: String,
    namespace: String,
}

#[derive(Deserialize)]
pub struct PutOverride {
    namespace: String,
}

/// GET /api/admin/prefixes — the overrides this deployment has set.
async fn list_overrides(State(state): State<AppState>) -> Result<Response, AppError> {
    let rows = state
        .auth_db
        .list_prefix_overrides()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Json(rows).into_response())
}

/// POST /api/admin/prefixes — claim a label.
///
/// Refuses a label that already has an override, and says what it currently
/// means: two prefixes with the same shorthand cannot both be right, and
/// repointing an established one silently would change what stored CURIEs
/// expand to. `PUT` is the way to repoint, which says so by being a different
/// request.
async fn create_override(
    State(state): State<AppState>,
    axum::Extension(user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    Json(body): Json<CreateOverride>,
) -> Result<Response, AppError> {
    validate(&body.label, &body.namespace)?;
    if let Some(existing) = state
        .auth_db
        .get_prefix_override(&body.label)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Conflict(serde_json::json!({
            "error": format!(
                "the prefix {:?} already resolves to {} here; PUT to repoint it",
                existing.label, existing.namespace
            ),
            "label": existing.label,
            "namespace": existing.namespace,
        })));
    }
    let row = state
        .auth_db
        .create_prefix_override(&body.label, &body.namespace, Some(&user.user_id))
        .map_err(|e| AppError::Internal(e.to_string()))?;
    refresh_admin_overlay(&state)?;
    Ok((axum::http::StatusCode::CREATED, Json(row)).into_response())
}

/// PUT /api/admin/prefixes/:label — set or repoint a label.
async fn put_override(
    State(state): State<AppState>,
    Path(label): Path<String>,
    axum::Extension(user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    Json(body): Json<PutOverride>,
) -> Result<Response, AppError> {
    validate(&label, &body.namespace)?;
    let (row, created) = state
        .auth_db
        .put_prefix_override(&label, &body.namespace, Some(&user.user_id))
        .map_err(|e| AppError::Internal(e.to_string()))?;
    refresh_admin_overlay(&state)?;
    let status = if created {
        axum::http::StatusCode::CREATED
    } else {
        axum::http::StatusCode::OK
    };
    Ok((status, Json(row)).into_response())
}

/// DELETE /api/admin/prefixes/:label — drop the override.
///
/// The label is not deleted, only this deployment's opinion of it: it falls
/// back to the platform overlay, an installed bundle's seeds, or the community
/// snapshot, whichever answers first.
async fn delete_override(
    State(state): State<AppState>,
    Path(label): Path<String>,
) -> Result<Response, AppError> {
    let removed = state
        .auth_db
        .delete_prefix_override(&label)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    if !removed {
        return Err(AppError::NotFound(format!(
            "no override for the prefix {label:?}"
        )));
    }
    refresh_admin_overlay(&state)?;
    Ok(axum::http::StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turtle_and_sparql_exports_open_with_the_source_credits() {
        let registry = PrefixRegistry::bundled_only();
        for (format, directive) in [("ttl", "@prefix "), ("sparql", "PREFIX ")] {
            let (body, _) = render_export(&registry, format).unwrap();
            assert!(body.starts_with("# Includes prefix mappings from:\n"));
            assert!(body.contains("prefix.cc (https://prefix.cc/)"));
            assert!(body.contains("https://github.com/cygri/prefix.cc/issues/13"));
            assert!(body.contains("CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/)"));
            assert!(body.contains(&format!("{directive}foaf: <http://xmlns.com/foaf/0.1/>")));
        }
    }

    #[test]
    fn credit_header_parses_as_turtle_and_sparql() {
        // Comments only: a client reading the export as Turtle or as a SPARQL
        // prologue must not notice it.
        let registry = PrefixRegistry::bundled_only();
        let header = credit_comment(&registry, &registry.all_prefixes());
        assert!(header.lines().count() >= 3, "{header}");
        let ttl = format!("{header}@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n");
        let parsed: Result<Vec<_>, _> =
            oxigraph::io::RdfParser::from_format(oxigraph::io::RdfFormat::Turtle)
                .for_reader(ttl.as_bytes())
                .collect();
        assert!(parsed.is_ok(), "{parsed:?}");
        spargebra::SparqlParser::new()
            .parse_query(&format!(
                "{header}PREFIX foaf: <http://xmlns.com/foaf/0.1/>\n\
                 SELECT * WHERE {{ ?s foaf:name ?o }}"
            ))
            .expect("header works in a SPARQL prologue");
    }

    #[test]
    fn the_whole_turtle_export_parses() {
        // Every bundled namespace is an IRI (the loader leaves out the one
        // prefix.cc entry that is not), so a strict parser reads it all.
        let registry = PrefixRegistry::bundled_only();
        let (body, _) = render_export(&registry, "ttl").unwrap();
        let parsed: Result<Vec<_>, _> =
            oxigraph::io::RdfParser::from_format(oxigraph::io::RdfFormat::Turtle)
                .for_reader(body.as_bytes())
                .collect();
        assert!(parsed.is_ok(), "{:?}", parsed.err());
    }

    #[test]
    fn json_export_stays_a_plain_prefix_map() {
        // The frontend reads this as prefix → namespace; a credit key would
        // turn into a bogus prefix there.
        let registry = PrefixRegistry::bundled_only();
        let (body, content_type) = render_export(&registry, "json").unwrap();
        assert_eq!(content_type, "application/json");
        let map: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&body).unwrap();
        assert!(map
            .values()
            .all(|v| v.as_str().is_some_and(|ns| ns.starts_with("http"))));
        assert_eq!(map["foaf"], "http://xmlns.com/foaf/0.1/");
    }

    #[test]
    fn no_credit_header_without_bundled_entries() {
        let registry = PrefixRegistry::empty();
        registry.set_admin_prefixes([("ex".to_string(), "https://example.org/".to_string())]);
        let (body, _) = render_export(&registry, "ttl").unwrap();
        assert_eq!(body, "@prefix ex: <https://example.org/> .\n");
    }

    #[test]
    fn unknown_export_format_is_refused() {
        let registry = PrefixRegistry::empty();
        assert!(matches!(
            render_export(&registry, "xml"),
            Err(AppError::BadRequest(_))
        ));
    }
}
