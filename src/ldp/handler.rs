//! LDP request handlers — GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use uuid::Uuid;

use super::container::{self, ContainerType};
use crate::server::AppState;

// ─── Link header values ────────────────────────────────────────────────────────

const LDP_NS: &str = "http://www.w3.org/ns/ldp#";

fn link_type(suffix: &str) -> String {
    format!("<{LDP_NS}{suffix}>; rel=\"type\"")
}

fn constrained_by_link(base_url: &str) -> String {
    format!("<{base_url}/ldp/constraints>; rel=\"http://www.w3.org/ns/ldp#constrainedBy\"")
}

/// Build type Link header values for the given container type.
fn type_links(ct: &ContainerType) -> Vec<String> {
    let mut links = vec![link_type("Resource")];
    match ct {
        ContainerType::NonRdfSource => {
            links.push(link_type("NonRDFSource"));
        }
        ContainerType::Basic => {
            links.push(link_type("RDFSource"));
            links.push(link_type("BasicContainer"));
        }
        ContainerType::Direct => {
            links.push(link_type("RDFSource"));
            links.push(link_type("DirectContainer"));
        }
        ContainerType::Indirect => {
            links.push(link_type("RDFSource"));
            links.push(link_type("IndirectContainer"));
        }
        _ => {
            links.push(link_type("RDFSource"));
        }
    }
    links
}

/// Build a joined Link header value (type links + constrained-by).
fn build_link_header(ct: &ContainerType, base_url: &str) -> String {
    let mut parts = type_links(ct);
    parts.push(constrained_by_link(base_url));
    parts.join(", ")
}

// ─── Query parameters ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LdpPageParams {
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

// ─── Prefer header parsing ────────────────────────────────────────────────────

const LDP_PREFER_CONTAINMENT: &str = "http://www.w3.org/ns/ldp#PreferContainment";
const LDP_PREFER_MEMBERSHIP: &str = "http://www.w3.org/ns/ldp#PreferMembership";
const LDP_PREFER_MINIMAL_CONTAINER: &str = "http://www.w3.org/ns/ldp#PreferMinimalContainer";

#[derive(Debug, PartialEq)]
enum PreferReturn {
    Minimal,
    Representation,
}

/// Parsed Prefer header state.
#[derive(Debug)]
struct PreferState {
    ret: PreferReturn,
    /// IRIs listed in `omit="..."` parameter.
    omit: Vec<String>,
    /// IRIs listed in `include="..."` parameter.
    include: Vec<String>,
}

impl PreferState {
    /// Whether ldp:contains (containment) triples should be included.
    fn include_containment(&self) -> bool {
        if self.ret == PreferReturn::Minimal {
            return false;
        }
        // omit=PreferContainment or omit=PreferMinimalContainer → exclude containment
        if self
            .omit
            .iter()
            .any(|u| u == LDP_PREFER_CONTAINMENT || u == LDP_PREFER_MINIMAL_CONTAINER)
        {
            return false;
        }
        // include=PreferMinimalContainer → exclude containment (same as minimal)
        if self
            .include
            .iter()
            .any(|u| u == LDP_PREFER_MINIMAL_CONTAINER)
        {
            return false;
        }
        true
    }

    /// Whether membership triples should be included.
    fn include_membership(&self) -> bool {
        if self.ret == PreferReturn::Minimal {
            return false;
        }
        if self
            .omit
            .iter()
            .any(|u| u == LDP_PREFER_MEMBERSHIP || u == LDP_PREFER_MINIMAL_CONTAINER)
        {
            return false;
        }
        if self
            .include
            .iter()
            .any(|u| u == LDP_PREFER_MINIMAL_CONTAINER)
        {
            return false;
        }
        true
    }

    /// Preference-Applied value for response header.
    fn applied_header(&self) -> &'static str {
        match self.ret {
            PreferReturn::Minimal => "return=minimal",
            PreferReturn::Representation => "return=representation",
        }
    }
}

/// Parse the `Prefer` request header into a `PreferState`.
///
/// Handles:
/// - `return=minimal` / `return=representation`
/// - `omit="<iri> [<iri>]"` — space-separated IRIs in double quotes
/// - `include="<iri> [<iri>]"` — same
fn parse_prefer(headers: &HeaderMap) -> PreferState {
    let val = match headers.get("prefer").and_then(|v| v.to_str().ok()) {
        Some(v) => v.to_string(),
        None => {
            return PreferState {
                ret: PreferReturn::Representation,
                omit: vec![],
                include: vec![],
            }
        }
    };

    let ret = if val.contains("return=minimal") {
        PreferReturn::Minimal
    } else {
        PreferReturn::Representation
    };

    fn extract_quoted_iris(haystack: &str, key: &str) -> Vec<String> {
        // Match: key="..." or key=<...>
        let prefix = format!("{}=\"", key);
        if let Some(start) = haystack.find(&prefix) {
            let rest = &haystack[start + prefix.len()..];
            if let Some(end) = rest.find('"') {
                return rest[..end]
                    .split_whitespace()
                    .map(|s| s.trim_start_matches('<').trim_end_matches('>').to_string())
                    .collect();
            }
        }
        vec![]
    }

    let omit = extract_quoted_iris(&val, "omit");
    let include = extract_quoted_iris(&val, "include");

    PreferState { ret, omit, include }
}

// ─── Content negotiation ─────────────────────────────────────────────────────

/// Negotiate the LDP response format from an Accept header.
/// Defaults to Turtle (preferred by LDP spec).
fn negotiate_ldp_format(headers: &HeaderMap) -> (oxigraph::io::RdfFormat, &'static str) {
    let accept = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");
    let a = accept.to_lowercase();
    if a.contains("application/n-triples") {
        (oxigraph::io::RdfFormat::NTriples, "application/n-triples")
    } else if a.contains("application/rdf+xml") {
        (oxigraph::io::RdfFormat::RdfXml, "application/rdf+xml")
    } else if a.contains("application/ld+json") {
        (
            oxigraph::io::RdfFormat::JsonLd {
                profile: oxigraph::io::JsonLdProfileSet::empty(),
            },
            "application/ld+json",
        )
    } else {
        // Default: Turtle (W3C LDP preferred)
        (oxigraph::io::RdfFormat::Turtle, "text/turtle")
    }
}

/// Convert N-Triples bytes to another RDF format via a temp in-memory store.
fn reserialize_ntriples(nt: &[u8], target_format: oxigraph::io::RdfFormat) -> Vec<u8> {
    if target_format == oxigraph::io::RdfFormat::NTriples {
        return nt.to_vec();
    }
    let Ok(nt_str) = std::str::from_utf8(nt) else {
        return nt.to_vec();
    };
    let Ok(tmp) = crate::store::TripleStore::in_memory() else {
        return nt.to_vec();
    };
    if tmp
        .load_str(nt_str, oxigraph::io::RdfFormat::NTriples, None)
        .is_err()
    {
        return nt.to_vec();
    }
    tmp.dump(target_format, None)
        .unwrap_or_else(|_| nt.to_vec())
}

// ─── Helper ───────────────────────────────────────────────────────────────────

fn resource_iri(base_url: &str, path: &str) -> String {
    let path = path.trim_start_matches('/');
    format!("{base_url}/ldp/{path}")
}

fn container_iri_for(base_url: &str, path: &str) -> String {
    let path = path.trim_start_matches('/');
    let parent = match path.rfind('/') {
        Some(i) => &path[..i + 1],
        None => "",
    };
    if parent.is_empty() {
        format!("{base_url}/ldp/")
    } else {
        format!("{base_url}/ldp/{parent}")
    }
}

// ─── GET ──────────────────────────────────────────────────────────────────────

/// GET /ldp/*path — Fetch an LDP resource or list a container.
pub async fn ldp_get(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    Query(params): Query<LdpPageParams>,
) -> Response {
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);

    let page_size = params.page_size.unwrap_or(100).min(1000);
    let page = params.page.unwrap_or(0);
    let offset = page * page_size;

    let ct = container::get_container_type(&state.store, &iri);

    // Non-RDF Source: return binary bytes directly
    if ct == ContainerType::NonRdfSource {
        return match container::get_binary_resource(&state.store, &iri) {
            Ok(Some((content_type, data))) => {
                let mut resp_headers = HeaderMap::new();
                resp_headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_str(&content_type)
                        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
                );
                let link_val = build_link_header(&ct, base);
                resp_headers.insert(
                    HeaderName::from_static("link"),
                    HeaderValue::from_str(&link_val)
                        .unwrap_or_else(|_| HeaderValue::from_static("")),
                );
                (StatusCode::OK, resp_headers, data).into_response()
            }
            Ok(None) => StatusCode::NOT_FOUND.into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        };
    }

    let member_count = container::count_members(&state.store, &iri).unwrap_or(0);
    let exists = member_count > 0 || container::resource_exists(&state.store, &iri);

    // Prefer header (return=minimal/representation + include/omit parameters)
    let prefer = parse_prefer(&headers);

    // Accept-based content negotiation (default: Turtle per LDP spec)
    let (out_format, out_content_type) = negotiate_ldp_format(&headers);

    // Describe the resource itself (N-Triples used as working format, re-serialized later)
    let raw_body = match container::describe_resource(&state.store, &iri) {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    // Strip ldp:contains lines from the stored describe output (we add them back selectively)
    let contains_marker = format!("<{}>", container::LDP_CONTAINS);
    let mut body: Vec<u8> = raw_body
        .split(|&b| b == b'\n')
        .filter(|line| !String::from_utf8_lossy(line).contains(&contains_marker))
        .flat_map(|line| line.iter().copied().chain(std::iter::once(b'\n')))
        .collect();

    // Append ldp:contains triples if containment is included per Prefer
    if member_count > 0 && prefer.include_containment() {
        match container::list_members(&state.store, &iri, offset, page_size) {
            Ok(members) => {
                for m in &members {
                    body.extend_from_slice(
                        format!("<{iri}> <{}> <{m}> .\n", container::LDP_CONTAINS).as_bytes(),
                    );
                }
            }
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        }
    }

    // Append membership triples if requested via Prefer: include=PreferMembership
    if prefer.include_membership() {
        if let Ok(Some(info)) = container::get_membership_info(&state.store, &iri) {
            match container::list_members(&state.store, &iri, 0, usize::MAX) {
                Ok(members) => {
                    for m in &members {
                        body.extend_from_slice(
                            format!(
                                "<{}> <{}> <{m}> .\n",
                                info.membership_resource, info.has_member_relation
                            )
                            .as_bytes(),
                        );
                    }
                }
                Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
            }
        }
    }

    if body.is_empty() && !exists {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Re-serialize from N-Triples to the negotiated format
    let body = reserialize_ntriples(&body, out_format);

    let etag = container::compute_etag(&body);
    let link_val = build_link_header(&ct, base);

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_str(out_content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("text/turtle")),
    );
    resp_headers.insert(
        HeaderName::from_static("etag"),
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"x\"")),
    );
    resp_headers.insert(
        HeaderName::from_static("vary"),
        HeaderValue::from_static("Accept, Prefer"),
    );
    resp_headers.insert(
        HeaderName::from_static("preference-applied"),
        HeaderValue::from_static(prefer.applied_header()),
    );
    resp_headers.insert(
        HeaderName::from_static("link"),
        HeaderValue::from_str(&link_val).unwrap_or_else(|_| HeaderValue::from_static("")),
    );

    // Pagination next link
    if member_count > offset + page_size {
        let next_url = format!("{iri}?page={}&page_size={page_size}", page + 1);
        let next_link = format!("<{next_url}>; rel=\"next\"");
        let cur = resp_headers
            .get("link")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let combined = format!("{cur}, {next_link}");
        resp_headers.insert(
            HeaderName::from_static("link"),
            HeaderValue::from_str(&combined).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
    }

    (StatusCode::OK, resp_headers, body).into_response()
}

// ─── HEAD ─────────────────────────────────────────────────────────────────────

/// HEAD /ldp/*path — Headers only (no body).
pub async fn ldp_head(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);

    if !container::resource_exists(&state.store, &iri) {
        return StatusCode::NOT_FOUND.into_response();
    }

    let body = container::describe_resource(&state.store, &iri).unwrap_or_default();
    let etag = container::compute_etag(&body);
    let ct = container::get_container_type(&state.store, &iri);
    let link_val = build_link_header(&ct, base);

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/n-triples"),
    );
    headers.insert(
        HeaderName::from_static("etag"),
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"x\"")),
    );
    headers.insert(
        HeaderName::from_static("link"),
        HeaderValue::from_str(&link_val).unwrap_or_else(|_| HeaderValue::from_static("")),
    );

    (StatusCode::OK, headers).into_response()
}

// ─── POST ─────────────────────────────────────────────────────────────────────

/// POST /ldp/*path — Create a new member resource in the container.
///
/// Supports `Slug` header, Turtle / JSON-LD / binary bodies, and Direct /
/// Indirect Container membership triple creation.
pub async fn ldp_post(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let base = state.base_url.as_ref();
    let container_iri = resource_iri(base, &path);

    // Determine container type before creating member
    let container_ct = container::get_container_type(&state.store, &container_iri);

    // Ensure container exists (creates as Basic if unknown)
    if container_ct == ContainerType::Unknown {
        if let Err(e) = container::ensure_container(&state.store, &container_iri) {
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }
    }

    // Determine member IRI from Slug or UUID
    let slug = headers
        .get("slug")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().replace(' ', "-"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let container_base = container_iri.trim_end_matches('/');
    let member_iri = format!("{container_base}/{slug}");

    // Determine content type from request header
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");

    // Load the body based on content type
    if !body.is_empty() {
        let is_turtle = content_type.contains("text/turtle");
        let is_jsonld = content_type.contains("application/ld+json");
        let is_rdf_xml = content_type.contains("application/rdf+xml");

        if is_turtle || is_jsonld || is_rdf_xml {
            let text = match std::str::from_utf8(&body) {
                Ok(s) => s,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Body must be valid UTF-8").into_response()
                }
            };

            let fmt = if is_jsonld {
                oxigraph::io::RdfFormat::JsonLd {
                    profile: Default::default(),
                }
            } else if is_rdf_xml {
                oxigraph::io::RdfFormat::RdfXml
            } else {
                oxigraph::io::RdfFormat::Turtle
            };

            if let Err(e) = state.store.load_str(text, fmt, None) {
                return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
            }
        } else {
            // Binary / Non-RDF Source
            if let Err(e) =
                container::store_binary_resource(&state.store, &member_iri, content_type, &body)
            {
                return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
            }
            // Still add ldp:contains
            if let Err(e) = container::add_member(&state.store, &container_iri, &member_iri) {
                return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
            }
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert(
                axum::http::header::LOCATION,
                HeaderValue::from_str(&member_iri).unwrap_or_else(|_| HeaderValue::from_static("")),
            );
            let link_val = build_link_header(&ContainerType::NonRdfSource, base);
            resp_headers.insert(
                HeaderName::from_static("link"),
                HeaderValue::from_str(&link_val).unwrap_or_else(|_| HeaderValue::from_static("")),
            );
            return (StatusCode::CREATED, resp_headers).into_response();
        }
    }

    // Tag the new RDF resource with type triples
    let type_q = format!(
        "INSERT DATA {{ \
           <{member_iri}> <{rdf_type}> <{rdf_source}> . \
           <{member_iri}> <{rdf_type}> <{ldp_resource}> . \
         }}",
        rdf_type = container::RDF_TYPE,
        rdf_source = container::LDP_RDF_SOURCE,
        ldp_resource = container::LDP_RESOURCE,
    );
    if let Err(e) = state.store.update(&type_q) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Add ldp:contains triple
    if let Err(e) = container::add_member(&state.store, &container_iri, &member_iri) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }

    // Handle Direct Container membership triple
    let re_read_ct = container::get_container_type(&state.store, &container_iri);
    if re_read_ct == ContainerType::Direct || re_read_ct == ContainerType::Indirect {
        if let Ok(Some(info)) = container::get_membership_info(&state.store, &container_iri) {
            if re_read_ct == ContainerType::Indirect {
                // For Indirect Containers, the membership triple object is the value
                // of the insertedContentRelation property on the new member.
                if let Some(icr) = &info.inserted_content_relation {
                    let icr_val_q = format!("SELECT ?val WHERE {{ <{member_iri}> <{icr}> ?val }}");
                    if let Ok(oxigraph::sparql::QueryResults::Solutions(sols)) =
                        state.store.query(&icr_val_q)
                    {
                        for sol in sols.flatten() {
                            if let Some(oxigraph::model::Term::NamedNode(nn)) = sol.get("val") {
                                let _ = container::add_direct_membership_triple(
                                    &state.store,
                                    &info.membership_resource,
                                    &info.has_member_relation,
                                    nn.as_str(),
                                );
                                break;
                            }
                        }
                    }
                }
            } else {
                // Direct Container: membership triple points to the new member
                let _ = container::add_direct_membership_triple(
                    &state.store,
                    &info.membership_resource,
                    &info.has_member_relation,
                    &member_iri,
                );
            }
        }
    }

    let member_ct = container::get_container_type(&state.store, &member_iri);
    let link_val = build_link_header(&member_ct, base);
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        axum::http::header::LOCATION,
        HeaderValue::from_str(&member_iri).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    resp_headers.insert(
        HeaderName::from_static("link"),
        HeaderValue::from_str(&link_val).unwrap_or_else(|_| HeaderValue::from_static("")),
    );

    (StatusCode::CREATED, resp_headers).into_response()
}

// ─── PUT ──────────────────────────────────────────────────────────────────────

/// PUT /ldp/*path — Replace (or create) an LDP RDF Source.
///
/// Supports `If-Match` ETag for optimistic concurrency.
pub async fn ldp_put(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);

    // If-Match check
    if let Some(if_match) = headers.get("if-match") {
        let current = container::describe_resource(&state.store, &iri).unwrap_or_default();
        let current_etag = container::compute_etag(&current);
        let client_etag = if_match.to_str().unwrap_or("");
        if client_etag != "*" && client_etag != current_etag {
            return StatusCode::PRECONDITION_FAILED.into_response();
        }
    }

    // Delete existing triples for this resource
    let del_q = format!("DELETE WHERE {{ <{iri}> ?p ?o }}");
    if let Err(e) = state.store.update(&del_q) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Load new body
    if !body.is_empty() {
        let content_type = headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/turtle");

        if content_type.contains("application/ld+json") {
            let text = match std::str::from_utf8(&body) {
                Ok(s) => s,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Body must be valid UTF-8").into_response()
                }
            };
            if let Err(e) = container::load_resource_jsonld(&state.store, &iri, text) {
                return (StatusCode::BAD_REQUEST, e).into_response();
            }
        } else if !content_type.contains("application/octet-stream") {
            let turtle = match std::str::from_utf8(&body) {
                Ok(s) => s,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Body must be valid UTF-8").into_response()
                }
            };
            if let Err(e) = container::load_resource_turtle(&state.store, &iri, turtle) {
                return (StatusCode::BAD_REQUEST, e).into_response();
            }
        } else {
            // Binary PUT
            if let Err(e) =
                container::store_binary_resource(&state.store, &iri, content_type, &body)
            {
                return (StatusCode::BAD_REQUEST, e).into_response();
            }
        }
    }

    // Re-tag with type triples
    let type_q = format!(
        "INSERT DATA {{ \
           <{iri}> <{rdf_type}> <{rdf_source}> . \
           <{iri}> <{rdf_type}> <{ldp_resource}> . \
         }}",
        rdf_type = container::RDF_TYPE,
        rdf_source = container::LDP_RDF_SOURCE,
        ldp_resource = container::LDP_RESOURCE,
    );
    if let Err(e) = state.store.update(&type_q) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Ensure container relationship
    let container = container_iri_for(base, &path);
    let _ = container::ensure_container(&state.store, &container);
    let _ = container::add_member(&state.store, &container, &iri);

    let new_body = container::describe_resource(&state.store, &iri).unwrap_or_default();
    let etag = container::compute_etag(&new_body);

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        HeaderName::from_static("etag"),
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"x\"")),
    );

    (StatusCode::NO_CONTENT, resp_headers).into_response()
}

// ─── PATCH ────────────────────────────────────────────────────────────────────

/// PATCH /ldp/*path — Apply a SPARQL Update body to an LDP RDF Source.
///
/// `Content-Type` must be `application/sparql-update`.
pub async fn ldp_patch(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);

    // 415 if wrong Content-Type
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !content_type.contains("application/sparql-update") {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "PATCH requires Content-Type: application/sparql-update",
        )
            .into_response();
    }

    // 404 if resource doesn't exist
    if !container::resource_exists(&state.store, &iri) {
        return StatusCode::NOT_FOUND.into_response();
    }

    // If-Match ETag check
    if let Some(if_match) = headers.get("if-match") {
        let current = container::describe_resource(&state.store, &iri).unwrap_or_default();
        let current_etag = container::compute_etag(&current);
        let client_etag = if_match.to_str().unwrap_or("");
        if client_etag != "*" && client_etag != current_etag {
            return StatusCode::PRECONDITION_FAILED.into_response();
        }
    }

    // Apply the SPARQL Update
    let sparql = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "Body must be valid UTF-8").into_response(),
    };

    if let Err(e) = state.store.update(sparql) {
        return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
    }

    // Return 204 with new ETag
    let new_body = container::describe_resource(&state.store, &iri).unwrap_or_default();
    let etag = container::compute_etag(&new_body);

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        HeaderName::from_static("etag"),
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"x\"")),
    );

    (StatusCode::NO_CONTENT, resp_headers).into_response()
}

// ─── DELETE ───────────────────────────────────────────────────────────────────

/// DELETE /ldp/*path — Remove an LDP resource and its containment triple.
///
/// Also removes Direct / Indirect Container membership triples.
pub async fn ldp_delete(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);
    let container = container_iri_for(base, &path);

    if !container::resource_exists(&state.store, &iri) {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Clean up Direct/Indirect Container membership triple
    let parent_ct = container::get_container_type(&state.store, &container);
    if parent_ct == ContainerType::Direct || parent_ct == ContainerType::Indirect {
        if let Ok(Some(info)) = container::get_membership_info(&state.store, &container) {
            let _ = container::remove_direct_membership_triple(
                &state.store,
                &info.membership_resource,
                &info.has_member_relation,
                &iri,
            );
        }
    }

    if let Err(e) = container::remove_member(&state.store, &container, &iri) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }

    StatusCode::NO_CONTENT.into_response()
}

// ─── OPTIONS ──────────────────────────────────────────────────────────────────

/// OPTIONS /ldp/*path — Advertise allowed methods, Accept-Post, and Accept-Patch.
pub async fn ldp_options(State(state): State<AppState>, Path(path): Path<String>) -> Response {
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);
    let ct = container::get_container_type(&state.store, &iri);
    let link_val = build_link_header(&ct, base);

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::ALLOW,
        HeaderValue::from_static("GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS"),
    );
    headers.insert(
        HeaderName::from_static("accept-post"),
        HeaderValue::from_static("text/turtle, application/ld+json"),
    );
    headers.insert(
        HeaderName::from_static("accept-patch"),
        HeaderValue::from_static("application/sparql-update"),
    );
    headers.insert(
        HeaderName::from_static("link"),
        HeaderValue::from_str(&link_val).unwrap_or_else(|_| HeaderValue::from_static("")),
    );

    (StatusCode::OK, headers).into_response()
}

/// OPTIONS /ldp/ — root container options (no path param).
pub async fn ldp_options_root(State(state): State<AppState>) -> Response {
    let base = state.base_url.as_ref();
    let link_val = build_link_header(&ContainerType::Basic, base);

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::ALLOW,
        HeaderValue::from_static("GET, HEAD, POST, PUT, PATCH, DELETE, OPTIONS"),
    );
    headers.insert(
        HeaderName::from_static("accept-post"),
        HeaderValue::from_static("text/turtle, application/ld+json"),
    );
    headers.insert(
        HeaderName::from_static("accept-patch"),
        HeaderValue::from_static("application/sparql-update"),
    );
    headers.insert(
        HeaderName::from_static("link"),
        HeaderValue::from_str(&link_val).unwrap_or_else(|_| HeaderValue::from_static("")),
    );

    (StatusCode::OK, headers).into_response()
}
