//! LDP request handlers — GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use serde::Deserialize;
use uuid::Uuid;

use super::container::{self, ContainerType};
use super::wac;
use crate::auth::middleware::AuthenticatedUser;
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

/// Build a joined Link header value: type links, constrained-by, and the
/// resource's ACL (`rel="acl"`, WAC discovery).
fn build_link_header(ct: &ContainerType, base_url: &str, resource_iri: &str) -> String {
    let mut parts = type_links(ct);
    parts.push(constrained_by_link(base_url));
    parts.push(acl_link(resource_iri));
    parts.join(", ")
}

fn acl_link(resource_iri: &str) -> String {
    // An ACL resource's ACL is itself.
    let acl = match wac::governed_resource(resource_iri) {
        Some(_) => resource_iri.to_string(),
        None => wac::acl_iri(resource_iri),
    };
    format!("<{acl}>; rel=\"acl\"")
}

// ─── Security helpers ───────────────────────────────────────────────────────────

/// Validate that a computed resource IRI is a syntactically valid IRI before it is
/// interpolated into any SPARQL string. A crafted request path (or `Slug`) could
/// otherwise smuggle SPARQL/IRI-breaking characters (`<` `>` `"` `{` `}` space,
/// control chars) into `INSERT DATA` / `DELETE WHERE`; `NamedNode::new` rejects them.
fn valid_iri(iri: &str) -> bool {
    oxigraph::model::NamedNode::new(iri).is_ok()
}

/// Sanitise a client-supplied `Slug` into a safe IRI path segment: only
/// `[A-Za-z0-9._-]` survive; every other character (whitespace and SPARQL/IRI
/// metacharacters like `>` `<` `{` `}` `"` `;`) collapses to `-`, so the slug
/// cannot break out of the `<member_iri>` context in the member triples.
fn sanitize_slug(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Clamp the stored `Content-Type` of a Non-RDF Source to a safe family before
/// reflecting it on GET. Anything outside the allow-list (notably `text/html` and
/// `image/svg+xml`, which execute script in the browser) degrades to
/// `application/octet-stream`, defusing stored XSS via an uploaded binary resource.
fn safe_binary_content_type(ct: &str) -> String {
    let base = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let safe = matches!(
        base.as_str(),
        "application/pdf" | "application/octet-stream" | "text/plain"
    ) || (base.starts_with("image/") && base != "image/svg+xml")
        || base.starts_with("video/")
        || base.starts_with("audio/");
    if safe {
        base
    } else {
        "application/octet-stream".to_string()
    }
}

/// Record `user` as the owner of a resource this request created.
#[allow(clippy::result_large_err)] // Err is an axum Response, returned on the cold deny path
fn own_created(state: &AppState, iri: &str, user: &AuthenticatedUser) -> Result<(), Response> {
    wac::write_owner_acl(&state.store, iri, &user.user_id, is_container(state, iri)).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("recording the owner of <{iri}>: {e}"),
        )
            .into_response()
    })
}

// ─── WAC ───────────────────────────────────────────────────────────────────────

fn forbidden(msg: String) -> Response {
    (StatusCode::FORBIDDEN, msg).into_response()
}

/// The caller as WAC sees them. A failed membership lookup refuses the
/// request: access control that cannot be evaluated grants nothing.
#[allow(clippy::result_large_err)] // Err is an axum Response, returned on the cold deny path
fn caller_agent(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
) -> Result<wac::Agent, Response> {
    wac::Agent::resolve(&state.auth_db, user).map_err(|e| {
        tracing::error!("ldp: WAC membership lookup failed, refusing the request: {e}");
        forbidden("access control lookup failed (memberships); the request is refused".to_string())
    })
}

/// Refuse unless `agent` holds `mode` on `iri` under its effective ACL.
#[allow(clippy::result_large_err)] // Err is an axum Response, returned on the cold deny path
fn require_mode(
    state: &AppState,
    agent: &wac::Agent,
    iri: &str,
    mode: wac::Mode,
) -> Result<(), Response> {
    match wac::allowed(
        &state.store,
        agent,
        iri,
        &wac::root_iri(&state.base_url),
        mode,
    ) {
        Ok(true) => Ok(()),
        Ok(false) => Err(forbidden(format!(
            "acl:{} on <{iri}> is not granted by its access control list (Link rel=\"acl\")",
            capitalize(mode.label())
        ))),
        Err(e) => {
            tracing::error!("ldp: WAC lookup for <{iri}> failed, refusing the request: {e}");
            Err(forbidden(
                "access control lookup failed; the request is refused".to_string(),
            ))
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The `WAC-Allow` header for `iri`: the caller's modes and the public ones.
fn wac_allow_value(state: &AppState, agent: &wac::Agent, iri: &str) -> Option<HeaderValue> {
    let root = wac::root_iri(&state.base_url);
    let user_modes = wac::granted_modes(&state.store, agent, iri, &root).ok()?;
    let public_modes =
        wac::granted_modes(&state.store, &wac::Agent::anonymous(), iri, &root).ok()?;
    HeaderValue::from_str(&wac::wac_allow_header(&user_modes, &public_modes)).ok()
}

const WAC_ALLOW: &str = "wac-allow";

/// Refuse a `PUT`/`POST` body that describes another LDP resource. The body
/// is authorized for `target` only; a triple whose subject is another IRI
/// under `/ldp/` would land in that resource past its ACL. Subjects outside
/// `/ldp/` stay allowed: an LDP-RS may describe related things.
#[allow(clippy::result_large_err)] // Err is an axum Response, returned on the cold deny path
fn body_confined_to(
    state: &AppState,
    text: &str,
    format: oxigraph::io::RdfFormat,
    target: &str,
) -> Result<(), Response> {
    let root = wac::root_iri(&state.base_url);
    let parser = oxigraph::io::RdfParser::from_format(format)
        .with_base_iri(target)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()).into_response())?;
    for quad in parser.for_reader(text.as_bytes()) {
        let quad = quad.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()).into_response())?;
        if let oxigraph::model::NamedOrBlankNode::NamedNode(s) = &quad.subject {
            if super::patch::other_ldp_subject(s.as_str(), target, &root) {
                return Err(forbidden(format!(
                    "the body describes another resource, <{}>; a write to <{target}> may only \
                     describe <{target}> and things outside /ldp/",
                    s.as_str()
                )));
            }
        }
    }
    Ok(())
}

fn acl_method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(axum::http::header::ALLOW, "GET, HEAD, PUT, DELETE, OPTIONS")],
        "an ACL resource is read with GET and replaced with PUT; POST and PATCH are not supported",
    )
        .into_response()
}

/// Whether the resource an ACL governs is there to be governed. The root
/// container always is.
fn governed_exists(state: &AppState, governed: &str) -> bool {
    let canonical = wac::canonical(governed);
    canonical == wac::root_iri(&state.base_url)
        || container::resource_exists(&state.store, canonical)
        || container::resource_exists(&state.store, &format!("{canonical}/"))
}

/// Whether `iri` is an LDP container, under either spelling.
fn is_container(state: &AppState, iri: &str) -> bool {
    let canonical = wac::canonical(iri);
    let is = |i: &str| {
        matches!(
            container::get_container_type(&state.store, i),
            ContainerType::Basic | ContainerType::Direct | ContainerType::Indirect
        )
    };
    canonical == wac::root_iri(&state.base_url) || is(canonical) || is(&format!("{canonical}/"))
}

/// GET `R.acl`: the authorizations of `R`, for a caller with `acl:Control` on `R`.
fn acl_get(state: &AppState, agent: &wac::Agent, governed: &str, headers: &HeaderMap) -> Response {
    if let Err(r) = require_mode(state, agent, governed, wac::Mode::Control) {
        return r;
    }
    if !governed_exists(state, governed) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let nt = match wac::acl_ntriples(&state.store, governed) {
        Ok(nt) => nt,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    if nt.is_empty() {
        // No ACL of its own: the resource inherits (Solid clients read a 404 so).
        return StatusCode::NOT_FOUND.into_response();
    }
    let (out_format, out_content_type) = negotiate_ldp_format(headers);
    let etag = container::compute_etag(&nt);
    let body = reserialize_ntriples(&nt, out_format);
    let acl = wac::acl_iri(governed);
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_str(out_content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("text/turtle")),
    );
    resp_headers.insert(axum::http::header::VARY, HeaderValue::from_static("Accept"));
    resp_headers.insert(
        HeaderName::from_static("etag"),
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("\"x\"")),
    );
    resp_headers.insert(
        HeaderName::from_static("link"),
        HeaderValue::from_str(&build_link_header(
            &ContainerType::RdfSource,
            &state.base_url,
            &acl,
        ))
        .unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    (StatusCode::OK, resp_headers, body).into_response()
}

/// PUT `R.acl`: replace the client-written authorizations of `R`. Needs
/// `acl:Control` on `R`; the body is validated whole before anything is written.
fn acl_put(
    state: &AppState,
    agent: &wac::Agent,
    governed: &str,
    headers: &HeaderMap,
    body: &Bytes,
) -> Response {
    if let Err(r) = require_mode(state, agent, governed, wac::Mode::Control) {
        return r;
    }
    if !governed_exists(state, governed) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");
    let format = if content_type.contains("application/ld+json") {
        oxigraph::io::RdfFormat::JsonLd {
            profile: Default::default(),
        }
    } else if content_type.contains("application/rdf+xml") {
        oxigraph::io::RdfFormat::RdfXml
    } else if content_type.contains("text/turtle") || content_type.contains("application/n-triples")
    {
        oxigraph::io::RdfFormat::Turtle
    } else {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "an ACL is written as text/turtle, application/ld+json or application/rdf+xml",
        )
            .into_response();
    };
    let text = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return (StatusCode::BAD_REQUEST, "Body must be valid UTF-8").into_response(),
    };
    let triples =
        match wac::validate_acl_body(text, format, governed, is_container(state, governed)) {
            Ok(t) => t,
            Err(e) => {
                return (StatusCode::BAD_REQUEST, format!("invalid ACL: {e}")).into_response()
            }
        };
    let had_own = match wac::has_client_acl(&state.store, governed) {
        Ok(b) => b,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };
    if let Err(e) = wac::replace_client_acl(&state.store, governed, &triples) {
        return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
    }
    let acl = wac::acl_iri(governed);
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        HeaderName::from_static("link"),
        HeaderValue::from_str(&build_link_header(
            &ContainerType::RdfSource,
            &state.base_url,
            &acl,
        ))
        .unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    let status = if had_own {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };
    (status, resp_headers).into_response()
}

/// DELETE `R.acl`: drop the client-written authorizations of `R`, which then
/// inherits again. The owner grant stays; it goes with the resource.
fn acl_delete(state: &AppState, agent: &wac::Agent, governed: &str) -> Response {
    if let Err(r) = require_mode(state, agent, governed, wac::Mode::Control) {
        return r;
    }
    match wac::has_client_acl(&state.store, governed) {
        Ok(true) => {}
        Ok(false) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
    match wac::delete_client_acl(&state.store, governed) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

// ─── Query parameters ─────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
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
///
/// `path` is optional: the bare root container `/ldp/` has no `*path` segment, so
/// it resolves to the empty path (the container IRI `{base}/ldp/`).
pub async fn ldp_get(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    path: Option<Path<String>>,
    headers: HeaderMap,
    Query(params): Query<LdpPageParams>,
) -> Response {
    let user = user.map(|Extension(u)| u);
    let path = path.map(|p| p.0).unwrap_or_default();
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);
    if !valid_iri(&iri) {
        return (StatusCode::BAD_REQUEST, "Invalid resource path").into_response();
    }

    // WAC: `acl:Read` on the resource, `acl:Control` to read its ACL. An
    // anonymous caller only gets here when `require_auth` found a
    // `foaf:Agent` grant; the check is repeated so the two cannot disagree.
    let agent = match caller_agent(&state, user.as_ref()) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Some(governed) = wac::governed_resource(&iri) {
        return acl_get(&state, &agent, governed, &headers);
    }
    if let Err(r) = require_mode(&state, &agent, &iri, wac::Mode::Read) {
        return r;
    }
    let wac_allow = wac_allow_value(&state, &agent, &iri);

    let page_size = params.page_size.unwrap_or(100).min(1000);
    let page = params.page.unwrap_or(0);
    let offset = page * page_size;

    let ct = container::get_container_type(&state.store, &iri);

    // Non-RDF Source: return binary bytes directly
    if ct == ContainerType::NonRdfSource {
        return match container::get_binary_resource(&state.store, &iri) {
            Ok(Some((content_type, data))) => {
                let mut resp_headers = HeaderMap::new();
                // Clamp the reflected Content-Type to a safe family and forbid MIME
                // sniffing — prevents stored XSS via an uploaded text/html or
                // image/svg+xml Non-RDF Source.
                let safe_ct = safe_binary_content_type(&content_type);
                resp_headers.insert(
                    axum::http::header::CONTENT_TYPE,
                    HeaderValue::from_str(&safe_ct)
                        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
                );
                resp_headers.insert(
                    HeaderName::from_static("x-content-type-options"),
                    HeaderValue::from_static("nosniff"),
                );
                let link_val = build_link_header(&ct, base, &iri);
                resp_headers.insert(
                    HeaderName::from_static("link"),
                    HeaderValue::from_str(&link_val)
                        .unwrap_or_else(|_| HeaderValue::from_static("")),
                );
                if let Some(v) = wac_allow {
                    resp_headers.insert(HeaderName::from_static(WAC_ALLOW), v);
                }
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

    // One ETag per resource STATE, shared with HEAD/PUT/PATCH — never a hash of
    // this particular negotiated, Prefer-filtered representation.
    let etag = container::resource_etag(&state.store, &iri);
    let link_val = build_link_header(&ct, base, &iri);

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_str(out_content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("text/turtle")),
    );
    // The representation varies with content negotiation and with Prefer.
    resp_headers.insert(
        axum::http::header::VARY,
        HeaderValue::from_static("Accept, Prefer"),
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
    if let Some(v) = wac_allow {
        resp_headers.insert(HeaderName::from_static(WAC_ALLOW), v);
    }
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

/// HEAD /ldp/*path — Headers only (no body). `path` is optional so the bare root
/// container `/ldp/` (no `*path` segment) resolves to the empty path.
pub async fn ldp_head(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    path: Option<Path<String>>,
    headers: HeaderMap,
) -> Response {
    // RFC 9110: HEAD's headers must be the ones GET would send. This used to be
    // a separate implementation that hard-coded application/n-triples, hashed
    // the unfiltered DESCRIBE, omitted Vary, and knew nothing about binary
    // resources — so HEAD and GET disagreed on Content-Type and ETag for the
    // same resource. Delegating makes divergence impossible.
    let mut resp = ldp_get(
        State(state),
        user,
        path,
        headers,
        Query(LdpPageParams::default()),
    )
    .await;
    *resp.body_mut() = axum::body::Body::empty();
    resp
}

/// GET /ldp/constraints — the document every LDP response's `constrainedBy`
/// Link points at. It was advertised on every response and served by no route.
pub async fn ldp_constraints() -> Response {
    const DOC: &str = "\
# Open Triplestore — LDP server constraints\n\
\n\
This document is the target of the `Link: rel=\"http://www.w3.org/ns/ldp#constrainedBy\"`\n\
header on every `/ldp/` response (LDP 1.0 §4.2.1.6).\n\
\n\
- Authentication is required for every `/ldp/` request, except `GET`/`HEAD` of\n\
  a resource whose access control list grants `foaf:Agent` `acl:Read`.\n\
- Access control is Web Access Control (WAC): every resource `R` has an ACL at\n\
  `R.acl` (`C/.acl` for a container), linked with `Link: rel=\"acl\"`. `GET`/`HEAD`\n\
  need `acl:Read`, `PUT`/`PATCH`/`DELETE` `acl:Write`, `POST` `acl:Append` on the\n\
  container, and reading or writing an ACL `acl:Control` on the resource it\n\
  governs. A resource without an ACL of its own inherits the nearest container's\n\
  `acl:default`. The creator of a resource holds Read, Write and Control on it\n\
  (`R.acl#owner`, server-managed). Names ending in `.acl` are reserved.\n\
- `WAC-Allow: user=\"…\", public=\"…\"` on `GET`/`HEAD` lists the caller's and the\n\
  public modes.\n\
- `POST` creates a member of the target container. `Slug` is sanitised to a\n\
  URL-safe path segment; a missing or empty Slug yields a UUID.\n\
- `POST` and `PUT` accept `text/turtle`, `application/ld+json` and\n\
  `application/rdf+xml`; any other Content-Type is stored as a Non-RDF Source.\n\
- A member is created as a container by sending `Link: <http://www.w3.org/ns/ldp#BasicContainer>; rel=\"type\"`\n\
  (or `DirectContainer` / `IndirectContainer`). A Direct container's body must\n\
  carry `ldp:membershipResource` and `ldp:hasMemberRelation`; an Indirect\n\
  container additionally `ldp:insertedContentRelation`. Missing ones are a 400.\n\
- `PATCH` takes `application/sparql-update` and is confined to the target\n\
  resource: `INSERT DATA`, `DELETE DATA` and `DELETE/INSERT … WHERE` on the\n\
  default graph only (no `GRAPH`, `LOAD`, `CLEAR`, `CREATE`, `DROP`, `SERVICE`,\n\
  `WITH`, `USING`). The `WHERE` clause sees only the resource's own triples. A\n\
  result describing another resource under `/ldp/`, or touching a\n\
  server-managed triple (`ldp:*`, `rdf:type ldp:*`), is refused with 403 and\n\
  nothing changes. Likewise a `PUT`/`POST` body may describe the target and\n\
  things outside `/ldp/`, not another resource.\n\
- `If-Match` is honoured on `PUT` and `PATCH`; the ETag identifies the resource\n\
  state and is the same for `GET` and `HEAD` regardless of the negotiated format.\n\
- The path `/ldp/constraints` is reserved for this document.\n";
    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/markdown; charset=utf-8",
        )],
        DOC,
    )
        .into_response()
}

// ─── POST ─────────────────────────────────────────────────────────────────────

/// POST /ldp/*path — Create a new member resource in the container.
///
/// Supports `Slug` header, Turtle / JSON-LD / binary bodies, and Direct /
/// Indirect Container membership triple creation.
pub async fn ldp_post(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Option<Path<String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = path.map(|p| p.0).unwrap_or_default();
    let base = state.base_url.as_ref();
    let container_iri = resource_iri(base, &path);
    if !valid_iri(&container_iri) {
        return (StatusCode::BAD_REQUEST, "Invalid container path").into_response();
    }
    if wac::governed_resource(&container_iri).is_some() {
        return acl_method_not_allowed();
    }

    // WAC: creating a member needs `acl:Append` (or Write) on the container.
    let agent = match caller_agent(&state, Some(&user)) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_mode(&state, &agent, &container_iri, wac::Mode::Append) {
        return r;
    }

    // Determine container type before creating member
    let container_ct = container::get_container_type(&state.store, &container_iri);

    // Ensure container exists (creates as Basic if unknown). Whoever caused a
    // container to come into being owns it, like any other created resource.
    if container_ct == ContainerType::Unknown {
        if let Err(e) = container::ensure_container(&state.store, &container_iri) {
            return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
        }
        if let Err(e) = own_created(&state, &container_iri, &user) {
            return e;
        }
    }

    // Determine member IRI from Slug or UUID
    let slug = headers
        .get("slug")
        .and_then(|v| v.to_str().ok())
        .map(sanitize_slug)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let container_base = container_iri.trim_end_matches('/');
    let member_iri = format!("{container_base}/{slug}");
    if !valid_iri(&member_iri) {
        return (StatusCode::BAD_REQUEST, "Invalid member IRI").into_response();
    }
    if wac::governed_resource(&member_iri).is_some() {
        return (
            StatusCode::BAD_REQUEST,
            "names ending in .acl are reserved for access control lists",
        )
            .into_response();
    }

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

            if let Err(r) = body_confined_to(&state, text, fmt, &member_iri) {
                return r;
            }

            // Parse with the new member's IRI as base so an idiomatic relative
            // `<>` subject resolves to it instead of being rejected as schemeless.
            // Triples-only: an LDP RDF Source is a single graph, so a body that
            // names its own graph (e.g. a JSON-LD `@graph` with an `@id`) must not
            // write into another named graph past the graph ACL.
            if let Err(e) = state
                .store
                .load_str_triples_only(text, fmt, Some(&member_iri))
            {
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
            if let Err(e) = own_created(&state, &member_iri, &user) {
                return e;
            }
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert(
                axum::http::header::LOCATION,
                HeaderValue::from_str(&member_iri).unwrap_or_else(|_| HeaderValue::from_static("")),
            );
            let link_val = build_link_header(&ContainerType::NonRdfSource, base, &member_iri);
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

    // LDP 1.0 §5.2.3.4: a client creates a container by sending
    // `Link: <http://www.w3.org/ns/ldp#DirectContainer>; rel="type"` (or the
    // Basic / Indirect variants). The header was never read — every POSTed
    // member became a plain RDFSource, and Direct/Indirect containers could only
    // be created from Rust code, so the README's "Direct, Indirect Containers"
    // was reachable by no client.
    if let Some(requested) = requested_container_type(&headers) {
        if let Err(msg) = apply_requested_container_type(&state.store, &member_iri, requested) {
            return (StatusCode::BAD_REQUEST, msg).into_response();
        }
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

    // The creator owns the new resource (Read, Write, Control), whatever the
    // container's policy says about everyone else.
    if let Err(e) = own_created(&state, &member_iri, &user) {
        return e;
    }

    let member_ct = container::get_container_type(&state.store, &member_iri);
    let link_val = build_link_header(&member_ct, base, &member_iri);
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

/// The container type a client asked for with `Link: <ldp:…>; rel="type"`.
/// When several are sent, the most specific wins (Indirect > Direct > Basic).
fn requested_container_type(headers: &HeaderMap) -> Option<ContainerType> {
    let mut found: Option<ContainerType> = None;
    for value in headers.get_all(axum::http::header::LINK) {
        let Ok(s) = value.to_str() else { continue };
        for link in s.split(',') {
            let link = link.trim();
            let is_type = link
                .split(';')
                .skip(1)
                .any(|p| p.trim().trim_matches('"') == "rel=type" || p.trim() == "rel=\"type\"");
            if !is_type {
                continue;
            }
            let target = link
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>');
            let ct = match target.strip_prefix(LDP_NS) {
                Some("IndirectContainer") => ContainerType::Indirect,
                Some("DirectContainer") => ContainerType::Direct,
                Some("BasicContainer") => ContainerType::Basic,
                _ => continue,
            };
            let rank = |c: &ContainerType| match c {
                ContainerType::Indirect => 3,
                ContainerType::Direct => 2,
                ContainerType::Basic => 1,
                _ => 0,
            };
            if found.as_ref().map(rank).unwrap_or(0) < rank(&ct) {
                found = Some(ct);
            }
        }
    }
    found
}

/// Type the freshly created member as the requested container, reading the
/// membership configuration from the triples the client supplied in the body.
fn apply_requested_container_type(
    store: &crate::store::TripleStore,
    iri: &str,
    ct: ContainerType,
) -> Result<(), String> {
    let need = |pred: &str, name: &str| {
        container::object_iri(store, iri, pred).ok_or_else(|| {
            format!(
                "creating a {} requires an IRI value for ldp:{name} in the request body \
                 (see the constrainedBy document at /ldp/constraints)",
                if ct == ContainerType::Indirect {
                    "IndirectContainer"
                } else {
                    "DirectContainer"
                }
            )
        })
    };
    match ct {
        ContainerType::Basic => container::ensure_container(store, iri),
        ContainerType::Direct => {
            let mr = need(container::LDP_MEMBERSHIP_RESOURCE, "membershipResource")?;
            let hmr = need(container::LDP_HAS_MEMBER_RELATION, "hasMemberRelation")?;
            container::ensure_direct_container(store, iri, &mr, &hmr, None)
        }
        ContainerType::Indirect => {
            let mr = need(container::LDP_MEMBERSHIP_RESOURCE, "membershipResource")?;
            let hmr = need(container::LDP_HAS_MEMBER_RELATION, "hasMemberRelation")?;
            let icr = need(
                container::LDP_INSERTED_CONTENT_REL,
                "insertedContentRelation",
            )?;
            container::ensure_indirect_container(store, iri, &mr, &hmr, &icr)
        }
        _ => Ok(()),
    }
}

// ─── PUT ──────────────────────────────────────────────────────────────────────

/// PUT /ldp/*path — Replace (or create) an LDP RDF Source.
///
/// Supports `If-Match` ETag for optimistic concurrency.
pub async fn ldp_put(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Option<Path<String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = path.map(|p| p.0).unwrap_or_default();
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);
    if !valid_iri(&iri) {
        return (StatusCode::BAD_REQUEST, "Invalid resource path").into_response();
    }
    let agent = match caller_agent(&state, Some(&user)) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Some(governed) = wac::governed_resource(&iri) {
        return acl_put(&state, &agent, governed, &headers, &body);
    }
    let container = container_iri_for(base, &path);
    let existed = container::resource_exists(&state.store, &iri);
    let container_existed = container == iri
        || container::get_container_type(&state.store, &container) != ContainerType::Unknown;

    // WAC: replacing needs `acl:Write` on the resource; creating needs
    // `acl:Append` (or Write) on the container it lands in.
    let gate = if existed || container == iri {
        require_mode(&state, &agent, &iri, wac::Mode::Write)
    } else {
        require_mode(&state, &agent, &container, wac::Mode::Append)
    };
    if let Err(r) = gate {
        return r;
    }

    // If-Match check
    if let Some(if_match) = headers.get("if-match") {
        let current_etag = container::resource_etag(&state.store, &iri);
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
            let fmt = oxigraph::io::RdfFormat::JsonLd {
                profile: Default::default(),
            };
            if let Err(r) = body_confined_to(&state, text, fmt, &iri) {
                return r;
            }
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
            if let Err(r) = body_confined_to(&state, turtle, oxigraph::io::RdfFormat::Turtle, &iri)
            {
                return r;
            }
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

    // Ensure container relationship. A PUT to the root `/ldp/` has the resource and
    // its parent container as the same IRI, so skip the self-membership triple.
    let _ = container::ensure_container(&state.store, &container);
    if container != iri {
        let _ = container::add_member(&state.store, &container, &iri);
    }

    // A PUT that created something makes the caller its owner; a replace
    // leaves ownership where it was.
    if !existed {
        if let Err(e) = own_created(&state, &iri, &user) {
            return e;
        }
    }
    if !container_existed {
        if let Err(e) = own_created(&state, &container, &user) {
            return e;
        }
    }

    let etag = container::resource_etag(&state.store, &iri);

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
    Extension(user): Extension<AuthenticatedUser>,
    path: Option<Path<String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let path = path.map(|p| p.0).unwrap_or_default();
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);
    if !valid_iri(&iri) {
        return (StatusCode::BAD_REQUEST, "Invalid resource path").into_response();
    }

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
    if wac::governed_resource(&iri).is_some() {
        return acl_method_not_allowed();
    }

    // WAC: `acl:Write` on the resource.
    let agent = match caller_agent(&state, Some(&user)) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Err(r) = require_mode(&state, &agent, &iri, wac::Mode::Write) {
        return r;
    }

    // 404 if resource doesn't exist
    if !container::resource_exists(&state.store, &iri) {
        return StatusCode::NOT_FOUND.into_response();
    }

    // If-Match ETag check
    if let Some(if_match) = headers.get("if-match") {
        let current_etag = container::resource_etag(&state.store, &iri);
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

    // The body is arbitrary SPARQL Update from a caller who holds acl:Write on
    // this one resource. It is not run against the store: `patch::apply`
    // restricts its shape, evaluates it on a scratch store holding only the
    // resource's own triples and writes back the difference, so whatever the
    // body says it can change the target resource and nothing else. (It used
    // to go through the `/sparql` gate, which could still clear the whole
    // default graph, where every LDP resource lives.)
    match super::patch::apply(&state.store, &iri, &wac::root_iri(base), sparql) {
        Ok(_) => {}
        Err(super::patch::PatchError::BadRequest(m)) => {
            return (StatusCode::BAD_REQUEST, m).into_response()
        }
        Err(super::patch::PatchError::NotAllowed(m)) => {
            return (StatusCode::FORBIDDEN, m).into_response()
        }
        Err(super::patch::PatchError::Internal(m)) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, m).into_response()
        }
    }

    // Return 204 with new ETag
    let etag = container::resource_etag(&state.store, &iri);

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
pub async fn ldp_delete(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    path: Option<Path<String>>,
) -> Response {
    let path = path.map(|p| p.0).unwrap_or_default();
    let base = state.base_url.as_ref();
    let iri = resource_iri(base, &path);
    if !valid_iri(&iri) {
        return (StatusCode::BAD_REQUEST, "Invalid resource path").into_response();
    }
    let agent = match caller_agent(&state, Some(&user)) {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Some(governed) = wac::governed_resource(&iri) {
        return acl_delete(&state, &agent, governed);
    }
    // WAC: `acl:Write` on the resource.
    if let Err(r) = require_mode(&state, &agent, &iri, wac::Mode::Write) {
        return r;
    }
    let container = container_iri_for(base, &path);

    // REVIEW: a DELETE to the bare root container `/ldp/` (empty path) wipes the
    // root's own triples like any other container delete (it does not cascade to
    // members). If the root should be undeletable, reject the empty path here with
    // `StatusCode::METHOD_NOT_ALLOWED` instead of proceeding.
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

    // The ACL goes with the resource: whoever recreates the path owns it.
    if let Err(e) = wac::delete_acl(&state.store, &iri) {
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
    let link_val = build_link_header(&ct, base, &iri);

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
    let link_val = build_link_header(&ContainerType::Basic, base, &wac::root_iri(base));

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
