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
//! It also covers who sees the authors of a registry entry — the creator of
//! the entry and of its versions, branch owners, commit actors and the
//! collaborators list — under `OTS_ACTOR_VISIBILITY`.
//!
//! All drive the real Axum router via `tower::ServiceExt::oneshot`.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{Role, SystemRole};
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

// ── Who sees the authors of a model ───────────────────────────────────────────
//
// A registry entry names who registered it, who made each version, who owns
// each branch tip and who made each commit; its collaborators route lists the
// version authors beside the owner. `OTS_ACTOR_VISIBILITY` (default
// `writers`) decides how far down the entry's readers those names go, as it
// does for a dataset's history. A writer is an account that may write the
// entry (`can_write_ontology`), administrators included.

/// The account behind every version and commit, by id, username and e-mail.
/// It has left the owning organisation since, so nothing but who-did-what
/// names it: none of the three may reach a caller below the audience.
const AV_AUTHOR: &str = "av-author";
const AV_AUTHOR_NAME: &str = "quillfeather";
const AV_AUTHOR_MAIL: &str = "quill@av.example";

async fn av_send(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(ct) = content_type {
        b = b.header(header::CONTENT_TYPE, ct);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn av_get(app: &axum::Router, uri: &str, token: Option<&str>) -> (StatusCode, String) {
    av_send(app, Method::GET, uri, token, None, Vec::new()).await
}

struct ModelWorld {
    app: axum::Router,
    admin: String,
    /// An administrator of the owning organisation: may write its entries.
    writer: String,
    /// A plain member of it: reads its private entry, writes neither.
    member: String,
    /// An account outside it: reads its public entry only.
    outsider: String,
}

impl ModelWorld {
    /// `(label, token)` of the five callers, lowest first.
    fn callers(&self) -> [(&'static str, Option<&str>); 5] {
        [
            ("anonymous", None),
            ("outsider", Some(self.outsider.as_str())),
            ("member", Some(self.member.as_str())),
            ("writer", Some(self.writer.as_str())),
            ("admin", Some(self.admin.as_str())),
        ]
    }

    async fn set_actor_visibility(&self, value: &str) {
        let (st, body) = av_send(
            &self.app,
            Method::PUT,
            "/api/admin/settings/OTS_ACTOR_VISIBILITY",
            Some(&self.admin),
            Some("application/json"),
            serde_json::json!({ "value": value })
                .to_string()
                .into_bytes(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "OTS_ACTOR_VISIBILITY={value}: {body}");
    }
}

/// An organisation's public entry `av-pub` (one published version) and its
/// private entry `av-priv` (one draft), every version and commit made by the
/// author, who then left the organisation.
async fn model_world() -> ModelWorld {
    // The matrix sends more requests than the limiter's burst allows one
    // client, and a 429 says nothing about who may read what.
    std::env::set_var("OTS_RATE_LIMIT_ENABLED", "false");
    let (state, admin) = admin_state();
    let app = test_app(state.clone());
    let base = state.base_url.to_string();
    let db = &state.auth_db;
    for (id, name, mail) in [
        (AV_AUTHOR, AV_AUTHOR_NAME, AV_AUTHOR_MAIL),
        ("av-writer", "av-writer", "writer@av.example"),
        ("av-member", "av-member", "member@av.example"),
        ("av-outsider", "av-outsider", "outsider@av.example"),
    ] {
        db.create_user(id, name, mail, "h", SystemRole::User)
            .unwrap();
    }
    db.create_organisation("av-org", "Avocet Works", "av-org", None, None)
        .unwrap();
    db.add_org_member(AV_AUTHOR, "av-org", Role::Admin).unwrap();
    db.add_org_member("av-writer", "av-org", Role::Admin)
        .unwrap();
    db.add_org_member("av-member", "av-org", Role::Member)
        .unwrap();
    let author = mint_token(AV_AUTHOR, AV_AUTHOR_NAME, "user");

    let creator = format!("{base}/users/{AV_AUTHOR}");
    for (id, public) in [("av-pub", true), ("av-priv", false)] {
        dmr::insert_data_model(
            &state.store,
            &base,
            id,
            id,
            &format!("http://ex.org/{id}#"),
            None,
            public,
            Some("organisation"),
            Some("av-org"),
            Some(&creator),
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        // An upload: a version, a branch tip and a commit of the author's.
        let body = multipart_body(
            "AVB",
            &[
                ("version", "text/plain", None, b"1.0.0".as_slice()),
                (
                    "file",
                    "text/turtle",
                    Some("m.ttl"),
                    b"<http://ex.org/s> <http://ex.org/p> <http://ex.org/o> .".as_slice(),
                ),
            ],
        );
        let (st, text) = av_send(
            &app,
            Method::POST,
            &format!("/api/models/{id}/versions"),
            Some(&author),
            Some("multipart/form-data; boundary=AVB"),
            body,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "upload to {id}: {text}");
    }
    // Published, so the public catalogue lists the entry and its version.
    let (st, text) = av_send(
        &app,
        Method::POST,
        "/api/models/av-pub/versions/1.0.0/publish",
        Some(&admin),
        None,
        Vec::new(),
    )
    .await;
    assert!(st.is_success(), "publish: {st} {text}");
    db.remove_org_member(AV_AUTHOR, "av-org").unwrap();

    ModelWorld {
        app,
        admin,
        writer: mint_token("av-writer", "av-writer", "user"),
        member: mint_token("av-member", "av-member", "user"),
        outsider: mint_token("av-outsider", "av-outsider", "user"),
    }
}

/// The routes of entry `m` that can name who made it, its versions and its
/// commits, and a string each answer holds whoever asks — so "no author" is
/// never an empty answer.
fn model_author_routes(m: &str) -> Vec<(String, String)> {
    let version = "\"version\":\"1.0.0\"".to_string();
    vec![
        (format!("/api/models/{m}"), format!("\"id\":\"{m}\"")),
        (format!("/api/models/{m}/versions"), version.clone()),
        (format!("/api/models/{m}/versions/1.0.0"), version),
        (
            format!("/api/models/{m}/branches"),
            "\"tip_version\"".to_string(),
        ),
        (
            format!("/api/models/{m}/commits"),
            "\"commit_id\"".to_string(),
        ),
        (format!("/api/models/{m}/collaborators"), "[".to_string()),
    ]
}

fn av_names_the_author(body: &str) -> bool {
    body.contains(AV_AUTHOR) || body.contains(AV_AUTHOR_NAME) || body.contains(AV_AUTHOR_MAIL)
}

/// Every author route of entry `m` as `token`: answered and naming the author
/// (`Some(true)`), answered without (`Some(false)`), or refused (`None`).
async fn model_author_seen(
    w: &ModelWorld,
    m: &str,
    who: &str,
    token: Option<&str>,
) -> Option<bool> {
    let mut seen: Option<bool> = None;
    for (i, (uri, always)) in model_author_routes(m).into_iter().enumerate() {
        let (st, body) = av_get(&w.app, &uri, token).await;
        let this = if st == StatusCode::OK {
            assert!(body.contains(&always), "{who} {uri}: {body}");
            Some(av_names_the_author(&body))
        } else {
            assert_eq!(st, StatusCode::NOT_FOUND, "{who} {uri}: {body}");
            assert!(!av_names_the_author(&body), "{who} {uri}: {body}");
            None
        };
        if i == 0 {
            seen = this;
        } else {
            assert_eq!(this, seen, "{who} {uri}: the routes disagree");
        }
    }
    seen
}

#[tokio::test]
async fn a_models_authors_are_shown_to_its_writers_unless_the_administrator_widens_it() {
    let w = model_world().await;
    // `(setting, [anonymous, outsider, member, writer, admin] on the public
    //   entry, the same on the private one)`; `None` is a refused request.
    let yes = Some(true);
    let no = Some(false);
    let table = [
        (
            "writers",
            [no, no, no, yes, yes],
            [None, None, no, yes, yes],
        ),
        (
            "signed-in",
            [no, yes, yes, yes, yes],
            [None, None, yes, yes, yes],
        ),
        (
            "readers",
            [yes, yes, yes, yes, yes],
            [None, None, yes, yes, yes],
        ),
    ];
    for (i, (level, on_public, on_private)) in table.into_iter().enumerate() {
        // The first row is the default: nothing is set for it.
        if i > 0 {
            w.set_actor_visibility(level).await;
        }
        for (m, want) in [("av-pub", on_public), ("av-priv", on_private)] {
            for ((who, token), want) in w.callers().into_iter().zip(want) {
                let got = model_author_seen(&w, m, who, token).await;
                assert_eq!(got, want, "{level}: {who} on entry {m}");
            }
        }
        // The listing carries each entry's `created_by` by the same rule; the
        // public entry is in everyone's.
        for ((who, token), want) in w.callers().into_iter().zip(on_public) {
            let (st, body) = av_get(&w.app, "/api/models", token).await;
            assert_eq!(st, StatusCode::OK, "{level}: {who}: {body}");
            assert!(body.contains("\"id\":\"av-pub\""), "{level}: {who}: {body}");
            assert_eq!(
                Some(av_names_the_author(&body)),
                want,
                "{level}: {who} listing the registry: {body}"
            );
        }
        // The public catalogue has no caller to ask about: it names who made
        // a published version only where every reader may know. The owning
        // organisation is named at every setting.
        let (st, body) = av_get(&w.app, "/api/public/catalog", None).await;
        assert_eq!(st, StatusCode::OK, "{level}: {body}");
        assert!(body.contains("\"id\":\"av-pub\""), "{level}: {body}");
        assert!(body.contains("Avocet Works"), "{level}: {body}");
        assert!(!body.contains("av-priv"), "{level}: {body}");
        assert_eq!(
            av_names_the_author(&body),
            level == "readers",
            "{level}: the public catalogue: {body}"
        );
    }
}

/// Below the audience a commit names nobody at all, the administrator who
/// published the version included, and keeps everything else.
#[tokio::test]
async fn a_models_commit_trail_keeps_its_messages_without_its_authors() {
    let w = model_world().await;
    let (st, body) = av_get(&w.app, "/api/models/av-pub/commits", None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let commits: serde_json::Value = serde_json::from_str(&body).unwrap();
    let commits = commits.as_array().expect("a list of commits");
    assert!(commits.len() >= 2, "the upload and the publication: {body}");
    for c in commits {
        for field in ["actor_iri", "actor_username", "actor_display_name"] {
            assert!(c.get(field).is_none(), "{field} in {c}");
        }
        assert!(
            c["message"].is_string() && c["created_at"].is_string(),
            "{c}"
        );
    }
    assert!(body.contains("Published version 1.0.0"), "{body}");

    let (st, body) = av_get(&w.app, "/api/models/av-pub/commits", Some(&w.writer)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        body.contains(&format!("\"actor_username\":\"{AV_AUTHOR_NAME}\"")),
        "{body}"
    );
    assert!(body.contains("\"actor_username\":\"admin\""), "{body}");
}

/// The collaborators route lists accounts. Who made a version follows
/// `OTS_ACTOR_VISIBILITY`; the members of the owning organisation are listed
/// to its members and to administrators only, as the organisation's own
/// member list is; and nobody else is handed an e-mail address.
#[tokio::test]
async fn a_models_collaborators_are_listed_without_members_or_addresses_to_outsiders() {
    let w = model_world().await;
    let uri = "/api/models/av-pub/collaborators";
    for (i, level) in ["writers", "signed-in", "readers"].into_iter().enumerate() {
        if i > 0 {
            w.set_actor_visibility(level).await;
        }
        for (who, token) in [("anonymous", None), ("outsider", Some(w.outsider.as_str()))] {
            let (st, body) = av_get(&w.app, uri, token).await;
            assert_eq!(st, StatusCode::OK, "{level}: {who}: {body}");
            assert!(
                !body.contains('@'),
                "{level}: {who} gets an address: {body}"
            );
            for member in ["av-writer", "av-member"] {
                assert!(
                    !body.contains(member),
                    "{level}: {who} gets the organisation's members: {body}"
                );
            }
        }
        // Its members read the list the organisation's own route gives them.
        for (who, token) in [
            ("member", &w.member),
            ("writer", &w.writer),
            ("admin", &w.admin),
        ] {
            let (st, body) = av_get(&w.app, uri, Some(token)).await;
            assert_eq!(st, StatusCode::OK, "{level}: {who}: {body}");
            for mail in ["writer@av.example", "member@av.example"] {
                assert!(body.contains(mail), "{level}: {who}: {body}");
            }
        }
    }

    // An entry a user owns: the owner is named to every reader (the entry's
    // record names them already), their address only to whoever may write it.
    let (st, body) = av_send(
        &w.app,
        Method::POST,
        "/api/models",
        Some(&w.admin),
        Some("application/json"),
        serde_json::json!({
            "title": "av solo", "namespace": "http://ex.org/av-solo#", "is_public": true,
            "owner_type": "user", "owner_id": "av-writer"
        })
        .to_string()
        .into_bytes(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    let uri = "/api/models/av-solo/collaborators";
    for (who, token) in [("anonymous", None), ("outsider", Some(w.outsider.as_str()))] {
        let (st, body) = av_get(&w.app, uri, token).await;
        assert_eq!(st, StatusCode::OK, "{who}: {body}");
        assert!(body.contains("\"username\":\"av-writer\""), "{who}: {body}");
        assert!(body.contains("\"role\":\"owner\""), "{who}: {body}");
        assert!(!body.contains('@'), "{who} gets an address: {body}");
    }
    let (st, body) = av_get(&w.app, uri, Some(&w.writer)).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("writer@av.example"), "{body}");
}
