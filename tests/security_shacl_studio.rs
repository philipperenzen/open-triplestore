//! Security regression tests for SHACL Studio cross-tenant access control.
//!
//! These cover findings where Studio tooling read or enforced against graphs the
//! caller had no rights to:
//!   * import-shapes copied from a caller-named *source* graph without a read check
//!     (only the destination was manage-gated);
//!   * model-context (`?graphs=`) and derive (`body.graphs`) returned/derived from
//!     caller-named graphs without a read check (the `?dataset=` paths already gate);
//!   * binding a shape graph as an enforcing validator required only *read* on the
//!     shape graph, not *manage*.
//!
//! The last section covers who sees the authors of a shape graph (the entry's
//! creator, each revision's, and the commit actors) under
//! `OTS_ACTOR_VISIBILITY`.
//!
//! All run against an in-memory AppState driven via the shared HTTP harness — a
//! plain authenticated (non-admin) user is denied read on any graph lacking a
//! `graph_acl` grant, which is exactly the cross-tenant boundary under test.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{Role, SystemRole};
use open_triplestore::server::AppState;
use serde_json::Value;
use tower::ServiceExt as _;

/// Send a request with an optional bearer token and JSON-ish body, returning the
/// status and decoded body text.
async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: &str,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b = b.header(header::CONTENT_TYPE, "application/json");
    let req = b.body(Body::from(body.to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    (status, text)
}

/// Create a regular (non-admin) user and return a token for them.
fn make_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

/// Create a shape graph as `token`'s user with the given visibility; return its id.
async fn create_shape_graph(app: &Router, token: &str, name: &str, visibility: &str) -> String {
    let body = serde_json::json!({ "name": name, "visibility": visibility }).to_string();
    let (st, txt) = send(
        app,
        Method::POST,
        "/api/shacl/shape-graphs",
        Some(token),
        &body,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "shape graph create failed: {txt}");
    let v: Value = serde_json::from_str(&txt).unwrap();
    v["id"].as_str().expect("shape graph id").to_string()
}

// import-shapes must reject a source graph the caller cannot read, even though the
// caller owns (and may manage) the destination shape graph.
#[tokio::test]
async fn import_shapes_denies_unreadable_source_graph() {
    let state = test_state();
    let token = make_user(&state, "alice");
    let app = test_app(state);

    let dest = create_shape_graph(&app, &token, "dest", "private").await;
    let body = serde_json::json!({
        "shapes": [ { "source_graph": "urn:secret:other-tenant", "shape": "http://ex/Shape" } ]
    })
    .to_string();
    let (st, txt) = send(
        &app,
        Method::POST,
        &format!("/api/shacl/shape-graphs/{dest}/import-shapes"),
        Some(&token),
        &body,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "import from an unreadable source graph must be 403, got {st}: {txt}"
    );
}

/// model-context and derive with NO scope at all must not read outside the
/// caller's grants. `resolve_scope` returned an empty graph list, which the
/// introspection query builder turned into "no GRAPH wrapper" — an unscoped BGP.
/// That reads the default graph, which no per-graph ACL covers and which holds
/// LDP resources and anything loaded without a target graph, so any
/// authenticated caller could enumerate it by omitting `?dataset=`/`?graphs=`.
/// Scope now defaults to the caller's readable graphs instead.
#[tokio::test]
async fn unscoped_introspection_does_not_read_the_whole_store() {
    let state = test_state();
    // Default-graph content: what an unscoped BGP actually reaches.
    state
        .store
        .load_str(
            "<http://other/s> a <http://other/TopSecretClass> .",
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let token = make_user(&state, "alice");
    let app = test_app(state);

    let (st, txt) = send(
        &app,
        Method::GET,
        "/api/shacl/model-context",
        Some(&token),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "unscoped model-context should answer");
    assert!(
        !txt.contains("TopSecretClass"),
        "unscoped model-context must not expose another tenant's classes: {txt}"
    );

    let (st, txt) = send(
        &app,
        Method::POST,
        "/api/shacl/derive",
        Some(&token),
        "{\"graphs\":[]}",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "unscoped derive should answer");
    assert!(
        !txt.contains("TopSecretClass"),
        "unscoped derive must not read another tenant's graphs: {txt}"
    );
}

// model-context with caller-named ?graphs= must reject graphs the caller cannot read.
#[tokio::test]
async fn model_context_denies_unreadable_graph() {
    let state = test_state();
    let token = make_user(&state, "alice");
    let app = test_app(state);

    let (st, txt) = send(
        &app,
        Method::GET,
        &format!(
            "/api/shacl/model-context?graphs={}",
            url_encode("urn:secret:other-tenant")
        ),
        Some(&token),
        "",
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "model-context over an unreadable graph must be 403, got {st}: {txt}"
    );
}

// derive with caller-named body.graphs must reject graphs the caller cannot read.
#[tokio::test]
async fn derive_shapes_denies_unreadable_graph() {
    let state = test_state();
    let token = make_user(&state, "alice");
    let app = test_app(state);

    let body = serde_json::json!({ "graphs": ["urn:secret:other-tenant"] }).to_string();
    let (st, txt) = send(&app, Method::POST, "/api/shacl/derive", Some(&token), &body).await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "derive over an unreadable graph must be 403, got {st}: {txt}"
    );
}

// Binding a shape graph as an enforcing validator requires *manage* on it, not
// merely read: a user who can only *access* (a public shape graph they do not own)
// must be refused.
#[tokio::test]
async fn binding_requires_manage_on_shape_graph() {
    let state = test_state();
    let owner_token = make_user(&state, "owner");
    let other_token = make_user(&state, "other");
    let app = test_app(state);

    // owner publishes a *public* shape graph: any authenticated user can read it,
    // but only the owner (or an admin) may manage it.
    let sg = create_shape_graph(&app, &owner_token, "public-shapes", "public").await;

    // The non-owner can read it (sanity: the access gate would have passed before
    // the fix) — GET succeeds.
    let (read_st, _) = send(
        &app,
        Method::GET,
        &format!("/api/shacl/shape-graphs/{sg}"),
        Some(&other_token),
        "",
    )
    .await;
    assert_eq!(
        read_st,
        StatusCode::OK,
        "a public shape graph must be readable by other users"
    );

    // But binding it (which makes it enforce against a target) must require manage.
    let body = serde_json::json!({
        "target": { "kind": "graph", "id": "urn:data:some-graph" },
        "shape_graph_id": sg,
    })
    .to_string();
    let (st, txt) = send(
        &app,
        Method::POST,
        "/api/shacl/bindings",
        Some(&other_token),
        &body,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "binding a shape graph the caller can only read must be 403, got {st}: {txt}"
    );
}

// ── Who sees the authors of a shape graph ─────────────────────────────────────
//
// A Library entry names who created it and who saved each revision, and its
// commit trail names who made each change. `OTS_ACTOR_VISIBILITY` (default
// `writers`) decides how far down the entry's readers those names go, as it
// does for a dataset's history. A writer is an account that manages the
// entry: its owner, a member of the owning organisation, an administrator.

/// The account behind every revision and commit. It has left the owning
/// organisation since, so nothing but who-did-what names it.
const SG_AUTHOR: &str = "sg-author";
const SG_AUTHOR_NAME: &str = "quillfeather";

const SG_SHAPES: &str =
    "@prefix sh: <http://www.w3.org/ns/shacl#> . <urn:sg:Shape> a sh:NodeShape .";

fn sg_names_the_author(body: &str) -> bool {
    body.contains(SG_AUTHOR) || body.contains(SG_AUTHOR_NAME)
}

async fn sg_put_turtle(app: &Router, token: &str, uri: &str, turtle: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "text/turtle")
        .body(Body::from(turtle.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

struct StudioWorld {
    app: Router,
    admin: String,
    /// A member of the owning organisation: manages its entries.
    writer: String,
    /// An account outside it: reads its public entry only.
    outsider: String,
    /// The organisation's public and private shape graphs.
    public: String,
    private: String,
}

impl StudioWorld {
    /// `(label, token)` of the four callers, lowest first.
    fn callers(&self) -> [(&'static str, Option<&str>); 4] {
        [
            ("anonymous", None),
            ("outsider", Some(self.outsider.as_str())),
            ("writer", Some(self.writer.as_str())),
            ("admin", Some(self.admin.as_str())),
        ]
    }

    async fn set_actor_visibility(&self, value: &str) {
        let (st, body) = send(
            &self.app,
            Method::PUT,
            "/api/admin/settings/OTS_ACTOR_VISIBILITY",
            Some(&self.admin),
            &serde_json::json!({ "value": value }).to_string(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "OTS_ACTOR_VISIBILITY={value}: {body}");
    }
}

/// An organisation's public and private shape graph, each created and then
/// saved once by the author (two revisions, two commits), who then left the
/// organisation. The public one is bound to graph `urn:sg:bound`.
async fn studio_world() -> StudioWorld {
    // The matrix sends more requests than the limiter's burst allows one
    // client, and a 429 says nothing about who may read what.
    std::env::set_var("OTS_RATE_LIMIT_ENABLED", "false");
    let (state, admin) = admin_state();
    let db = &state.auth_db;
    db.create_user(
        SG_AUTHOR,
        SG_AUTHOR_NAME,
        "quill@sg.example",
        "h",
        SystemRole::User,
    )
    .unwrap();
    let author = mint_token(SG_AUTHOR, SG_AUTHOR_NAME, "user");
    let writer = make_user(&state, "sg-writer");
    let outsider = make_user(&state, "sg-outsider");
    db.create_organisation("sg-org", "Sandgrouse Works", "sg-org", None, None)
        .unwrap();
    db.add_org_member(SG_AUTHOR, "sg-org", Role::Member)
        .unwrap();
    db.add_org_member("sg-writer", "sg-org", Role::Member)
        .unwrap();
    let app = test_app(state.clone());

    let mut ids = Vec::new();
    for visibility in ["public", "private"] {
        let body = serde_json::json!({
            "name": format!("sg {visibility}"), "visibility": visibility,
            "owner_type": "organisation", "owner_id": "sg-org",
        })
        .to_string();
        let (st, txt) = send(
            &app,
            Method::POST,
            "/api/shacl/shape-graphs",
            Some(&author),
            &body,
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "create {visibility}: {txt}");
        let id = serde_json::from_str::<Value>(&txt).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (st, txt) = sg_put_turtle(
            &app,
            &author,
            &format!("/api/shacl/shape-graphs/{id}/turtle"),
            SG_SHAPES,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "save {visibility}: {txt}");
        ids.push(id);
    }
    let private = ids.pop().unwrap();
    let public = ids.pop().unwrap();
    let body = serde_json::json!({
        "target": { "kind": "graph", "id": "urn:sg:bound" },
        "shape_graph_id": public,
    })
    .to_string();
    let (st, txt) = send(
        &app,
        Method::POST,
        "/api/shacl/bindings",
        Some(&admin),
        &body,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "bind: {txt}");
    db.remove_org_member(SG_AUTHOR, "sg-org").unwrap();

    StudioWorld {
        app,
        admin,
        writer,
        outsider,
        public,
        private,
    }
}

/// The routes of shape graph `id` that can name who made it and its
/// revisions, and a string each answer holds whoever asks.
fn shape_graph_author_routes(id: &str) -> Vec<(String, String)> {
    let revision = "\"revision\":2".to_string();
    vec![
        (
            format!("/api/shacl/shape-graphs/{id}"),
            format!("\"id\":\"{id}\""),
        ),
        (
            format!("/api/shacl/shape-graphs/{id}/revisions"),
            revision.clone(),
        ),
        (
            format!("/api/shacl/shape-graphs/{id}/revisions/2"),
            revision,
        ),
        (
            format!("/api/shacl/shape-graphs/{id}/commits"),
            "\"commit_id\"".to_string(),
        ),
    ]
}

/// Every author route of shape graph `id` as `token`: answered and naming
/// the author (`Some(true)`), answered without (`Some(false)`), or refused
/// (`None`).
async fn shape_graph_author_seen(
    w: &StudioWorld,
    id: &str,
    who: &str,
    token: Option<&str>,
) -> Option<bool> {
    let mut seen: Option<bool> = None;
    for (i, (uri, always)) in shape_graph_author_routes(id).into_iter().enumerate() {
        let (st, body) = send(&w.app, Method::GET, &uri, token, "").await;
        let this = if st == StatusCode::OK {
            assert!(body.contains(&always), "{who} {uri}: {body}");
            Some(sg_names_the_author(&body))
        } else {
            assert!(
                matches!(st, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
                "{who} {uri}: {st} {body}"
            );
            assert!(!sg_names_the_author(&body), "{who} {uri}: {body}");
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
async fn a_shape_graphs_authors_are_shown_to_its_writers_unless_the_administrator_widens_it() {
    let w = studio_world().await;
    // `(setting, [anonymous, outsider, writer, admin] on the public shape
    //   graph, the same on the private one)`; `None` is a refused request.
    // The Studio takes no anonymous caller, so `readers` is `signed-in` here.
    // A private entry is read by its owner's side only, who all manage it:
    // the Studio's read check lets no administrator in on their role alone.
    let yes = Some(true);
    let no = Some(false);
    let table = [
        ("writers", [None, no, yes, yes], [None, None, yes, None]),
        ("signed-in", [None, yes, yes, yes], [None, None, yes, None]),
        ("readers", [None, yes, yes, yes], [None, None, yes, None]),
    ];
    for (i, (level, on_public, on_private)) in table.into_iter().enumerate() {
        // The first row is the default: nothing is set for it.
        if i > 0 {
            w.set_actor_visibility(level).await;
        }
        for (id, want) in [(&w.public, on_public), (&w.private, on_private)] {
            for ((who, token), want) in w.callers().into_iter().zip(want) {
                let got = shape_graph_author_seen(&w, id, who, token).await;
                assert_eq!(got, want, "{level}: {who} on shape graph {id}");
            }
        }
        // The same record wherever else it is served: the Library listing
        // and the shape graphs bound to a target.
        for uri in [
            "/api/shacl/shape-graphs",
            "/api/shacl/bindings?target_kind=graph&target_id=urn%3Asg%3Abound",
        ] {
            for ((who, token), want) in w.callers().into_iter().zip(on_public).skip(1) {
                let (st, body) = send(&w.app, Method::GET, uri, token, "").await;
                assert_eq!(st, StatusCode::OK, "{level}: {who} {uri}: {body}");
                assert!(
                    body.contains(&format!("\"id\":\"{}\"", w.public)),
                    "{level}: {who} {uri}: {body}"
                );
                assert_eq!(
                    Some(sg_names_the_author(&body)),
                    want,
                    "{level}: {who} {uri}: {body}"
                );
            }
        }
    }
}

/// Below the audience a commit names nobody at all and keeps its message.
#[tokio::test]
async fn a_shape_graphs_commit_trail_keeps_its_messages_without_its_authors() {
    let w = studio_world().await;
    let uri = format!("/api/shacl/shape-graphs/{}/commits", w.public);
    let (st, body) = send(&w.app, Method::GET, &uri, Some(&w.outsider), "").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let commits: Value = serde_json::from_str(&body).unwrap();
    let commits = commits.as_array().expect("a list of commits");
    assert!(commits.len() >= 3, "created, saved and bound: {body}");
    for c in commits {
        for field in ["actor_iri", "actor_username", "actor_display_name"] {
            assert!(c.get(field).is_none(), "{field} in {c}");
        }
        assert!(
            c["message"].is_string() && c["created_at"].is_string(),
            "{c}"
        );
    }
    assert!(body.contains("Bound to urn:sg:bound"), "{body}");

    let (st, body) = send(&w.app, Method::GET, &uri, Some(&w.writer), "").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        body.contains(&format!("\"actor_username\":\"{SG_AUTHOR_NAME}\"")),
        "{body}"
    );
}
