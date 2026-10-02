//! Linked Data dereference endpoint and VoID/DCAT dataset description.
//!
//! # Routes
//! - `GET /resource/*path` — content-negotiated IRI dereference (FAIR A + I)
//! - `GET /.well-known/void` — VoID/DCAT machine-readable dataset description
//!
//! ## Content Negotiation
//! RDF Accept types (text/turtle, application/ld+json, …) → SPARQL CONSTRUCT result.
//! `text/html` → 303 See Other redirect to the SPA resource view, with a
//! relative `Location` so it lands right behind a reverse proxy that serves the
//! instance under a path prefix (see docs/operations.md).
//! Default (*/*, empty) → Turtle.

use axum::extract::{Extension, Path, Query, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;

use super::content_negotiation::{negotiate_graph_format, serialize_graph, GraphFormat};
use super::error::AppError;
use super::AppState;

/// Builds the IRI dereference routes.
pub fn dereference_routes() -> Router<AppState> {
    Router::new().route("/resource/*path", get(dereference_handler))
}

/// Builds the well-known VoID/DCAT dataset description routes.
pub fn well_known_routes() -> Router<AppState> {
    Router::new().route("/.well-known/void", get(void_handler))
}

/// Builds organisation-scoped well-known VoID/DCAT routes.
pub fn well_known_org_routes() -> Router<AppState> {
    Router::new().route("/:org_id/.well-known/void", get(org_void_handler))
}

/// Optional `?format=` query parameter to override Accept header negotiation.
/// Allows browser links like `/resource/foo?format=turtle` to work without
/// custom Accept headers.
#[derive(Deserialize)]
struct FormatParam {
    format: Option<String>,
}

/// GET /resource/*path
///
/// Dereferences a local IRI by running a bidirectional SPARQL CONSTRUCT against
/// the triplestore and returning the result in the negotiated RDF format.
///
/// - RDF Accept headers → CONSTRUCT result (outgoing + incoming triples)
/// - `text/html` → 303 See Other to the SPA view `resource?iri={full_iri}`, as a
///   reference relative to the request (see [`spa_resource_location`])
/// - `?format=turtle|jsonld|ntriples|rdfxml|nquads|trig` overrides Accept
async fn dereference_handler(
    State(state): State<AppState>,
    user: Option<Extension<crate::auth::middleware::AuthenticatedUser>>,
    Path(path): Path<String>,
    Query(params): Query<FormatParam>,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let full_iri = format!("{}/resource/{}", state.base_url, path);

    // Reject characters that could break out of the `<…>` IRI in the CONSTRUCT
    // template built below. Reads are already FROM-scoped to readable graphs, so
    // this is defense-in-depth against query corruption / reflected-IRI noise.
    if full_iri.contains(|c: char| {
        matches!(
            c,
            '<' | '>' | '"' | '`' | '\\' | '{' | '}' | ' ' | '\n' | '\r' | '\t'
        )
    }) {
        return Err(AppError::BadRequest("Invalid resource IRI".to_string()));
    }

    // Resolve format: query param takes priority over Accept header.
    let accept_from_header = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("*/*")
        .to_lowercase();

    let effective_accept: String = match params.format.as_deref() {
        Some("turtle") => "text/turtle".to_string(),
        Some("jsonld") | Some("json-ld") => "application/ld+json".to_string(),
        Some("ntriples") | Some("n-triples") => "application/n-triples".to_string(),
        Some("rdfxml") | Some("rdf-xml") => "application/rdf+xml".to_string(),
        Some("nquads") | Some("n-quads") => "application/n-quads".to_string(),
        Some("trig") => "application/trig".to_string(),
        _ => accept_from_header.clone(),
    };

    // HTML clients get a 303 redirect to the SPA page (unless ?format= was given).
    if params.format.is_none()
        && effective_accept.contains("text/html")
        && !effective_accept.contains("text/turtle")
    {
        let location = spa_resource_location(uri.path(), &full_iri);
        return axum::http::Response::builder()
            .status(StatusCode::SEE_OTHER)
            .header("location", location)
            .header("vary", "Accept")
            .body(axum::body::Body::empty())
            .map_err(|e| AppError::Internal(e.to_string()));
    }

    let format = negotiate_graph_format(&effective_accept);

    // Scope dereference to the graphs the caller can read. Without this, an
    // anonymous user could see triples that live exclusively inside private
    // dataset, ontology, or vocabulary version graphs.
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let is_admin = user.as_deref().map(|u| u.is_admin()).unwrap_or(false);
    let allowed = if is_admin {
        // Admins see every named graph. Enumerate them so FROM/FROM NAMED can
        // make the default graph the union of all data — the CONSTRUCT pattern
        // queries an unnamed default and would otherwise miss everything.
        let mut s = std::collections::HashSet::new();
        if let Ok(named) = state.store.named_graphs() {
            for nn in named {
                s.insert(nn.as_str().to_string());
            }
        }
        s
    } else {
        compute_dereference_allowed_graphs(&state, user_id)
            .map_err(|e| AppError::Internal(e.to_string()))?
    };
    if allowed.is_empty() {
        return Err(AppError::NotFound(format!(
            "No triples found for <{full_iri}>"
        )));
    }
    let mut from_clauses = String::new();
    for iri in &allowed {
        from_clauses.push_str(&format!("FROM <{iri}>\nFROM NAMED <{iri}>\n"));
    }

    let sparql = format!(
        "CONSTRUCT {{ <{iri}> ?p ?o . ?s ?p2 <{iri}> . }} \
         {from_clauses}\
         WHERE {{ {{ <{iri}> ?p ?o }} UNION {{ ?s ?p2 <{iri}> }} }}",
        iri = full_iri
    );

    let results = state
        .store
        .query(&sparql)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let body = serialize_graph(results, format).map_err(AppError::Internal)?;

    if body.is_empty() {
        return Err(AppError::NotFound(format!(
            "No triples found for <{}>",
            full_iri
        )));
    }

    let link_sparql = format!(
        "<{}/sparql>; rel=\"http://www.w3.org/ns/sparql-service-description#endpoint\"",
        state.base_url
    );
    let link_void = format!(
        "<{}/.well-known/void>; rel=\"http://rdfs.org/ns/void#inDataset\"",
        state.base_url
    );

    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, format.content_type())
        .header("vary", "Accept")
        .header("link", &link_sparql)
        .header("link", &link_void)
        .body(axum::body::Body::from(body))
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// GET /.well-known/void
///
/// Returns a machine-readable VoID/DCAT dataset description. Content-negotiable:
/// defaults to Turtle, also supports JSON-LD, N-Triples, RDF/XML via Accept header
/// or `?format=` query param.
async fn void_handler(
    State(state): State<AppState>,
    user: Option<Extension<crate::auth::middleware::AuthenticatedUser>>,
    Query(params): Query<FormatParam>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let format = catalog_format(&params, &headers);
    let bytes = crate::dcat::generate_catalog_bytes(
        &state.base_url,
        &state.store,
        &state.auth_db,
        user_id,
        None,
        format.to_rdf_format(),
    )
    .map_err(AppError::Internal)?;
    catalog_response(format, bytes)
}

/// `?format=` wins over `Accept`; Turtle is the default.
fn catalog_format(params: &FormatParam, headers: &HeaderMap) -> GraphFormat {
    let accept_from_header = headers
        .get(ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("*/*")
        .to_lowercase();
    let effective_accept: String = match params.format.as_deref() {
        Some("turtle") => "text/turtle".to_string(),
        Some("jsonld") | Some("json-ld") => "application/ld+json".to_string(),
        Some("ntriples") | Some("n-triples") => "application/n-triples".to_string(),
        Some("rdfxml") | Some("rdf-xml") => "application/rdf+xml".to_string(),
        _ => accept_from_header,
    };
    negotiate_graph_format(&effective_accept)
}

fn catalog_response(format: GraphFormat, bytes: Vec<u8>) -> Result<Response, AppError> {
    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, format.content_type())
        .header("vary", "Accept")
        .header("cache-control", "public, max-age=60")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// GET /:org_id/.well-known/void
///
/// Returns a DCAT/VoID catalog scoped to the given organisation. The `org_id`
/// segment is matched against the organisation slug first, then by UUID.
///
/// - No bearer token → only `Public` datasets for this org.
/// - With bearer token → public + any non-public datasets the caller can access.
async fn org_void_handler(
    State(state): State<AppState>,
    user: Option<Extension<crate::auth::middleware::AuthenticatedUser>>,
    Path(org_id): Path<String>,
    Query(params): Query<FormatParam>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    // Resolve org by slug first, then by id.
    let org = state
        .auth_db
        .get_organisation_by_slug(&org_id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .or_else(|| state.auth_db.get_organisation(&org_id).unwrap_or(None))
        .ok_or_else(|| AppError::NotFound(format!("Organisation '{org_id}' not found")))?;
    let user_id = user.as_deref().map(|u| u.user_id.as_str());
    let format = catalog_format(&params, &headers);
    let bytes = crate::dcat::generate_catalog_bytes(
        &state.base_url,
        &state.store,
        &state.auth_db,
        user_id,
        Some(&org),
        format.to_rdf_format(),
    )
    .map_err(AppError::Internal)?;
    catalog_response(format, bytes)
}

/// Compute the set of named graph IRIs a NON-ADMIN caller may read when
/// dereferencing an IRI:
///   - graphs registered to datasets the user can access, and
///   - version graphs of data-models / vocabularies the user can access.
///
/// Unmanaged named graphs (registered to no dataset or ontology) and system
/// graphs (`urn:system:*`) are deliberately NOT included: this mirrors the
/// `/sparql` access boundary (`get_accessible_graph_iris`), so a graph is never
/// anonymously readable here unless it is also visible via SPARQL. (Admins
/// bypass this and see every graph — see the caller.)
fn compute_dereference_allowed_graphs(
    state: &AppState,
    user_id: Option<&str>,
) -> anyhow::Result<std::collections::HashSet<String>> {
    use std::collections::HashSet;

    let cached_graphs = state.auth_db.get_accessible_graph_iris_cached(user_id)?;
    let mut allowed: HashSet<String> = cached_graphs.0.clone();

    for d in crate::data_models::registry::list_data_models(&state.store) {
        let can = state
            .auth_db
            .can_access_ontology(
                user_id,
                d.is_public,
                d.owner_type.as_deref(),
                d.owner_id.as_deref(),
            )
            .unwrap_or(false);
        if !can {
            continue;
        }
        for ver in crate::data_models::registry::list_versions(&state.store, &state.base_url, &d.id)
        {
            allowed.insert(ver.graph_iri.clone());
            for g in &ver.sub_graphs {
                allowed.insert(g.clone());
            }
        }
    }

    Ok(allowed)
}

/// The `Location` of the 303 that sends a browser from `GET /resource/{path}`
/// to the SPA's resource page, `resource?iri=…`.
///
/// It is relative to the request: one `../` per segment of `{path}` climbs from
/// `…/resource/a/b` back to the directory that holds `resource/`. A
/// root-absolute `/resource?iri=` would leave a deployment that a reverse proxy
/// serves under a prefix (`https://example.org/ots/resource/a` → the proxy
/// strips `/ots`) for the host root, where nothing answers. Relative references
/// are allowed in `Location` (RFC 9110 §10.2.2) and every browser resolves them.
fn spa_resource_location(request_path: &str, full_iri: &str) -> String {
    // The raw (still percent-encoded) path, so an encoded `/` inside a segment
    // does not count as a separator — the browser resolves the same string.
    // The route is `/resource/*path`: the first `/resource/` is the route's,
    // anything after it (another `resource/` included) is the IRI's path.
    let tail = request_path
        .split_once("/resource/")
        .map(|(_, t)| t)
        .unwrap_or("");
    let depth = tail.split('/').count();
    let encoded = utf8_percent_encode(full_iri, NON_ALPHANUMERIC);
    format!("{}resource?iri={}", "../".repeat(depth), encoded)
}

#[cfg(test)]
mod tests {
    use super::spa_resource_location;

    /// Resolve a relative reference against a request path the way a browser
    /// does (RFC 3986 §5.2, enough of it for `../` chains plus a query).
    fn resolve(request_path: &str, reference: &str) -> String {
        let mut dir: Vec<&str> = request_path.split('/').collect();
        dir.pop(); // drop the last segment: references resolve from the directory
        let (path_part, query) = reference.split_once('?').unwrap_or((reference, ""));
        for seg in path_part.split('/') {
            match seg {
                ".." => {
                    if dir.len() > 1 {
                        dir.pop();
                    }
                }
                "." | "" => {}
                s => dir.push(s),
            }
        }
        format!("{}?{}", dir.join("/"), query)
    }

    #[test]
    fn spa_redirect_lands_on_the_resource_page_at_any_depth_and_prefix() {
        let iri = "https://example.org/ots/resource/a/b";
        let enc = "https%3A%2F%2Fexample%2Eorg%2Fots%2Fresource%2Fa%2Fb";
        for (served_at, prefix) in [("", ""), ("/ots", "/ots"), ("/tools/ots", "/tools/ots")] {
            for tail in ["a", "a/b", "a/b/c", "a/", "a%2Fb", "x/resource/y"] {
                // What the browser asked for (before a proxy strips the prefix) …
                let browser_path = format!("{served_at}/resource/{tail}");
                // … and what the backend sees after it did.
                let backend_path = format!("/resource/{tail}");
                let loc = spa_resource_location(&backend_path, iri);
                assert!(!loc.starts_with('/'), "Location must be relative: {loc}");
                assert_eq!(
                    resolve(&browser_path, &loc),
                    format!("{prefix}/resource?iri={enc}"),
                    "{browser_path} -> {loc}"
                );
            }
        }
    }
}
