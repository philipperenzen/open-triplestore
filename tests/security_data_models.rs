//! Security regression tests for the data-model versioning API.
//!
//! Covers two fixes:
//!   * CB6 — `PATCH /api/models/:id/versions/:ver/data` must escape user-supplied
//!     literal values so a SPARQL break-out payload cannot inject extra triples,
//!     and must reject a per-triple graph override that points outside the
//!     version's own graphs.
//!   * CB7 — re-uploading an *existing* version must be rejected WITHOUT first
//!     wiping the existing version's graph data.
//!
//! Both drive the real Axum router via `tower::ServiceExt::oneshot`.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::registry as dmr;
use oxigraph::sparql::QueryResults;
use tower::ServiceExt as _;

/// `(state, admin_token, base_graph_iri)` plus a freshly-registered draft version
/// on data model `m1` whose single base graph is
/// `{base}/data-model/m1/version/1.0`. The model is owned by the admin user so the
/// admin has write access to it.
fn model_with_draft() -> (open_triplestore::server::AppState, String, String) {
    let (state, token) = admin_state();
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m1",
        "M1",
        "http://ex.org/m1#",
        None,
        false,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let graph_iri = format!("{base}/data-model/m1/version/1.0");
    dmr::insert_version(
        &state.store,
        &base,
        &DataModelVersion {
            data_model_id: "m1".to_string(),
            version: "1.0".to_string(),
            status: VersionStatus::Draft,
            graph_iri: graph_iri.clone(),
            sub_graphs: vec![],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();
    (state, token, graph_iri)
}

/// Returns true if the store contains the given triple in any graph.
fn ask_triple(state: &open_triplestore::server::AppState, s: &str, p: &str, o: &str) -> bool {
    let q = format!("ASK {{ GRAPH ?g {{ <{s}> <{p}> <{o}> }} }}");
    matches!(state.store.query(&q), Ok(QueryResults::Boolean(true)))
}

// ── CB6: PATCH literal escaping ───────────────────────────────────────────────

/// A PATCH whose object-literal value contains a SPARQL break-out payload must
/// NOT inject any extra triples: the value is escaped and stays a single literal.
#[tokio::test]
async fn patch_literal_breakout_does_not_inject_triples() {
    let (state, token, graph_iri) = model_with_draft();

    // The value tries to close the literal and the GRAPH/INSERT-DATA blocks, then
    // append its own triples. Properly escaped, the quotes/braces are inert.
    let payload = "pwned\" . <urn:evil:s> <urn:evil:p> <urn:evil:o> } } ; \
         INSERT DATA { GRAPH <urn:evil:g> { <urn:evil:s2> <urn:evil:p2> <urn:evil:o2> } } #";
    let body = serde_json::json!({
        "add": [{
            "s": "http://ex.org/subject",
            "p": "http://ex.org/label",
            "o": { "value": payload }
        }],
        "remove": []
    });

    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/models/m1/versions/1.0/data")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;

    // The escaped literal is valid SPARQL, so the patch itself succeeds.
    assert_eq!(status, StatusCode::OK, "escaped PATCH should apply: {text}");

    // Neither breakout triple may have been injected into ANY graph.
    assert!(
        !ask_triple(&state, "urn:evil:s", "urn:evil:p", "urn:evil:o"),
        "SPARQL injection must not create urn:evil:s triple"
    );
    assert!(
        !ask_triple(&state, "urn:evil:s2", "urn:evil:p2", "urn:evil:o2"),
        "SPARQL injection must not create urn:evil:s2 triple"
    );

    // The intended triple landed in the authorized version graph, stored as a
    // plain literal carrying the raw (still-dangerous-looking) text verbatim.
    let q = format!(
        "ASK {{ GRAPH <{graph_iri}> {{ <http://ex.org/subject> <http://ex.org/label> ?o \
         FILTER(isLiteral(?o)) }} }}"
    );
    assert!(
        matches!(state.store.query(&q), Ok(QueryResults::Boolean(true))),
        "the intended literal triple must be stored in the version graph"
    );
}

/// A per-triple `graph` override pointing at an unrelated (other-tenant) absolute
/// IRI must be rejected — a writer may only target this version's own graphs.
#[tokio::test]
async fn patch_rejects_foreign_graph_override() {
    let (state, token, _graph_iri) = model_with_draft();

    let body = serde_json::json!({
        "add": [{
            "s": "http://ex.org/s",
            "p": "http://ex.org/p",
            "o": "http://ex.org/o",
            "graph": "http://localhost:7878/data-model/other/version/1.0"
        }],
        "remove": []
    });

    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::PATCH)
                .uri("/api/models/m1/versions/1.0/data")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "writing into a graph outside this version must be rejected"
    );
    assert!(
        !ask_triple(
            &state,
            "http://ex.org/s",
            "http://ex.org/p",
            "http://ex.org/o"
        ),
        "no triple may be written into the foreign graph"
    );
}

// ── CB7: duplicate upload must not wipe existing data ──────────────────────────

/// Build a multipart/form-data body from `(name, content_type, optional_filename,
/// bytes)` parts.
fn multipart_body(boundary: &str, parts: &[(&str, &str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, content_type, filename, bytes) in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        if let Some(fname) = filename {
            out.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n"
                )
                .as_bytes(),
            );
        } else {
            out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n").as_bytes(),
            );
        }
        out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

async fn upload_version(
    state: &open_triplestore::server::AppState,
    token: &str,
    boundary: &str,
    file: &[u8],
) -> StatusCode {
    let body = multipart_body(
        boundary,
        &[
            ("version", "text/plain", None, b"1.0.0"),
            ("file", "text/turtle", Some("m.ttl"), file),
        ],
    );
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/models/dup/versions")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

/// Re-uploading an already-existing version is rejected and, critically, leaves
/// the previously-uploaded version's data intact (it is not deleted-then-failed).
#[tokio::test]
async fn duplicate_upload_does_not_wipe_existing_version() {
    let (state, token) = admin_state();
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "dup",
        "Dup",
        "http://ex.org/dup#",
        None,
        false,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();

    // First upload of version 1.0.0 establishes a distinctive triple.
    let first = b"<http://ex.org/keep> <http://ex.org/p> \"original\" .";
    let st = upload_version(&state, &token, "BOUNDARY1", first).await;
    assert_eq!(st, StatusCode::CREATED, "first upload should succeed");

    let version_graph = format!("{base}/data-model/dup/version/1.0.0");
    let original_present = |state: &open_triplestore::server::AppState| {
        let q = format!(
            "ASK {{ GRAPH <{version_graph}> {{ <http://ex.org/keep> <http://ex.org/p> \"original\" }} }}"
        );
        matches!(state.store.query(&q), Ok(QueryResults::Boolean(true)))
    };
    assert!(
        original_present(&state),
        "first upload must store its triple"
    );

    // Re-upload the SAME version with different content; must be rejected.
    let second = b"<http://ex.org/new> <http://ex.org/p> \"replacement\" .";
    let st = upload_version(&state, &token, "BOUNDARY2", second).await;
    assert!(
        st.is_client_error(),
        "duplicate version upload must be rejected, got {st}"
    );

    // The original data must survive — the duplicate upload must NOT have wiped it
    // before failing, and the replacement content must NOT have been written.
    assert!(
        original_present(&state),
        "existing version data must NOT be wiped by a rejected duplicate upload"
    );
    let q_new = format!(
        "ASK {{ GRAPH <{version_graph}> {{ <http://ex.org/new> <http://ex.org/p> \"replacement\" }} }}"
    );
    assert!(
        !matches!(state.store.query(&q_new), Ok(QueryResults::Boolean(true))),
        "rejected duplicate upload must not write replacement content"
    );
}

/// Uploads are ownership-scoped, not publisher-gated: a signed-in user who
/// does NOT own the model must be rejected (and nothing written). Registry
/// writes follow the dataset rules — any account may version models it owns,
/// while foreign models stay closed.
#[tokio::test]
async fn upload_requires_write_access_to_the_model() {
    let (state, _admin_token, graph_iri) = model_with_draft(); // m1 owned by "adm"
    state
        .auth_db
        .create_user("u_joe", "joe", "j@t.com", "h", SystemRole::User)
        .unwrap();
    let tok = mint_token("u_joe", "joe", "user");
    let body = multipart_body(
        "BNDX",
        &[
            ("version", "text/plain", None, b"2.0".as_slice()),
            (
                "file",
                "text/turtle",
                Some("m.ttl"),
                b"<http://ex.org/s> <http://ex.org/p> <http://ex.org/o> .",
            ),
        ],
    );
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/models/m1/versions")
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=BNDX".to_string(),
                )
                .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "a user without write access to the model must not reach the upload handler"
    );
    // The foreign upload wrote nothing anywhere near the model's graphs.
    let count = match state
        .store
        .query(&format!(
            "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph_iri}> {{ ?s ?p ?o }} }}"
        ))
        .unwrap()
    {
        QueryResults::Solutions(mut sols) => sols
            .next()
            .and_then(|r| r.ok())
            .and_then(|s| s.get("n").map(|v| v.to_string()))
            .unwrap_or_default(),
        _ => String::new(),
    };
    assert!(
        count.starts_with("\"0\""),
        "rejected upload must write nothing, got {count}"
    );
}

/// Publishing or deprecating a data-model version is admin-gated. The gate
/// answered 401 `Unauthorized` — which means "not authenticated" and which
/// clients treat as a session expiry — to a logged-in user who merely lacks the
/// role. It is 403 now, like the dataset-version gate.
#[tokio::test]
async fn version_lifecycle_admin_gate_is_forbidden_not_unauthorized() {
    let (state, _admin, _base) = model_with_draft();
    state
        .auth_db
        .create_user("mallory", "mallory", "m@t.com", "hash", SystemRole::User)
        .unwrap();
    let mallory = mint_token("mallory", "mallory", "user");
    let app = test_app(state);
    for action in ["publish", "deprecate"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/models/m1/versions/1.0/{action}"))
                    .header(header::AUTHORIZATION, format!("Bearer {mallory}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "{action}: an authenticated non-admin is forbidden, not unauthenticated"
        );
    }
}
