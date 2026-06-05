//! Linked Data dereference endpoint and VoID/DCAT dataset description.
//!
//! # Routes
//! - `GET /resource/*path` — content-negotiated IRI dereference (FAIR A + I)
//! - `GET /.well-known/void` — VoID/DCAT machine-readable dataset description
//!
//! ## Content Negotiation
//! RDF Accept types (text/turtle, application/ld+json, …) → SPARQL CONSTRUCT result.
//! `text/html` → 303 See Other redirect to the SPA resource view.
//! Default (*/*, empty) → Turtle.

use axum::extract::{Extension, Path, Query, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;

use super::content_negotiation::{negotiate_graph_format, serialize_graph, GraphFormat};
use super::error::AppError;
use super::AppState;
use crate::store::engine::TripleStore;

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
/// - `text/html` → 303 See Other to `/resource?iri={full_iri}` (SPA view)
/// - `?format=turtle|jsonld|ntriples|rdfxml|nquads|trig` overrides Accept
async fn dereference_handler(
    State(state): State<AppState>,
    user: Option<Extension<crate::auth::middleware::AuthenticatedUser>>,
    Path(path): Path<String>,
    Query(params): Query<FormatParam>,
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
        let encoded = utf8_percent_encode(&full_iri, NON_ALPHANUMERIC).to_string();
        let location = format!("/resource?iri={}", encoded);
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
    let void_turtle =
        crate::dcat::generate_dcat_catalog(&state.base_url, &state.store, &state.auth_db, user_id);

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

    let format = negotiate_graph_format(&effective_accept);

    // For Turtle (the default), emit directly — no round-trip needed.
    if format == GraphFormat::Turtle {
        return axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/turtle")
            .header("vary", "Accept")
            .body(axum::body::Body::from(void_turtle))
            .map_err(|e| AppError::Internal(e.to_string()));
    }

    // For other formats: load into a temporary in-memory store and re-serialize.
    let tmp = TripleStore::in_memory().map_err(|e| AppError::Internal(e.to_string()))?;
    tmp.load_str(&void_turtle, oxigraph::io::RdfFormat::Turtle, None)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let bytes = tmp
        .dump(format.to_rdf_format(), None)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, format.content_type())
        .header("vary", "Accept")
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
    let void_turtle = crate::dcat::generate_org_dcat_catalog(
        &org,
        &state.base_url,
        &state.store,
        &state.auth_db,
        user_id,
    );

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

    let format = negotiate_graph_format(&effective_accept);

    if format == GraphFormat::Turtle {
        return axum::http::Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/turtle")
            .header("vary", "Accept")
            .body(axum::body::Body::from(void_turtle))
            .map_err(|e| AppError::Internal(e.to_string()));
    }

    let tmp = TripleStore::in_memory().map_err(|e| AppError::Internal(e.to_string()))?;
    tmp.load_str(&void_turtle, oxigraph::io::RdfFormat::Turtle, None)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let bytes = tmp
        .dump(format.to_rdf_format(), None)
        .map_err(|e| AppError::Internal(e.to_string()))?;

    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, format.content_type())
        .header("vary", "Accept")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| AppError::Internal(e.to_string()))
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
