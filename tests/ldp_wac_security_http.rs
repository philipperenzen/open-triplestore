//! Per-resource access control for `/ldp/*` with Web Access Control (WAC).
//!
//! Drives the real router with real tokens: two users (Alice, Bob) and an
//! admin. The eleven tests follow `docs/notes/ldp-wac-plan.md`, "Tests to
//! write", in order; the number is in each test's doc comment.

#![cfg(feature = "ldp")]

mod common;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::ldp::wac;
use open_triplestore::server::AppState;
use open_triplestore::store::TripleStore;
use oxigraph::sparql::QueryResults;
use tower::ServiceExt as _;

const BASE: &str = "http://localhost:7878";
const ACL: &str = "http://www.w3.org/ns/auth/acl#";
const TURTLE: &str = "text/turtle";
const SPARQL_UPDATE: &str = "application/sparql-update";

struct Env {
    state: AppState,
    app: Router,
    admin: String,
    alice: String,
    bob: String,
}

fn env() -> Env {
    env_over(TripleStore::in_memory().unwrap())
}

fn env_over(store: TripleStore) -> Env {
    let (state, admin) = admin_state_with_store(store);
    for (id, name) in [("alice", "alice"), ("bob", "bob")] {
        state
            .auth_db
            .create_user(
                id,
                name,
                &format!("{id}@test.com"),
                "hash",
                SystemRole::User,
            )
            .unwrap();
    }
    let app = test_app(state.clone());
    Env {
        state,
        app,
        admin,
        alice: mint_token("alice", "alice", "user"),
        bob: mint_token("bob", "bob", "user"),
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
    body: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    let req = b.body(Body::from(body.to_string())).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let hdrs = resp.headers().clone();
    let text = body_text(resp.into_body()).await;
    (status, hdrs, text)
}

/// `PUT` a one-triple Turtle resource at `path` (`/ldp/…`).
async fn put_doc(app: &Router, token: &str, path: &str) -> (StatusCode, String) {
    let (st, _, body) = send(
        app,
        Method::PUT,
        path,
        Some(token),
        &[("Content-Type", TURTLE)],
        &format!("<{BASE}{path}> <http://example.org/p> \"v\" ."),
    )
    .await;
    (st, body)
}

async fn get(app: &Router, token: Option<&str>, path: &str) -> (StatusCode, HeaderMap, String) {
    send(app, Method::GET, path, token, &[("Accept", TURTLE)], "").await
}

/// `PATCH` that only touches the resource itself.
async fn patch_self(app: &Router, token: &str, path: &str) -> (StatusCode, String) {
    let (st, _, body) = send(
        app,
        Method::PATCH,
        path,
        Some(token),
        &[("Content-Type", SPARQL_UPDATE)],
        &format!("INSERT DATA {{ <{BASE}{path}> <http://example.org/patched> \"yes\" }}"),
    )
    .await;
    (st, body)
}

async fn delete(app: &Router, token: &str, path: &str) -> (StatusCode, String) {
    let (st, _, body) = send(app, Method::DELETE, path, Some(token), &[], "").await;
    (st, body)
}

/// `PUT` an ACL body (Turtle, relative to the ACL resource) at `acl_path`.
async fn put_acl(app: &Router, token: &str, acl_path: &str, turtle: &str) -> (StatusCode, String) {
    let (st, _, body) = send(
        app,
        Method::PUT,
        acl_path,
        Some(token),
        &[("Content-Type", TURTLE)],
        &format!("@prefix acl: <{ACL}> .\n{turtle}"),
    )
    .await;
    (st, body)
}

/// An authorization node granting `user` Read, Write and Control on
/// `resource` (`accessTo`, plus `default` when `container`).
fn grant_all(fragment: &str, resource: &str, container: bool, user: &str) -> String {
    let default = if container {
        format!("acl:default <{resource}> ;")
    } else {
        String::new()
    };
    format!(
        "<#{fragment}> a acl:Authorization ; acl:accessTo <{resource}> ; {default}\n\
           acl:agent <urn:ots:user:{user}> ; acl:mode acl:Read, acl:Write, acl:Control .\n"
    )
}

fn grant_mode(fragment: &str, resource: &str, agent_line: &str, mode: &str) -> String {
    format!(
        "<#{fragment}> a acl:Authorization ; acl:accessTo <{resource}> ; {agent_line} ; acl:mode acl:{mode} .\n"
    )
}

fn ask(state: &AppState, q: &str) -> bool {
    matches!(state.store.query(q), Ok(QueryResults::Boolean(true)))
}

fn acl_graph_has_subject_prefix(state: &AppState, prefix: &str) -> bool {
    ask(
        state,
        &format!(
            "ASK {{ GRAPH <{}> {{ ?s ?p ?o }} FILTER(STRSTARTS(STR(?s), \"{prefix}\")) }}",
            wac::ACL_GRAPH
        ),
    )
}

fn link_has(headers: &HeaderMap, needle: &str) -> bool {
    headers
        .get_all("link")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| v.contains(needle))
}

fn wac_allow(headers: &HeaderMap) -> String {
    headers
        .get("wac-allow")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

// ─── 1 ─────────────────────────────────────────────────────────────────────────

/// Plan test 1. Alice creates `/ldp/a/`; while the root's grant is inherited,
/// Bob may write inside. Once Alice writes an ACL for the container that grants
/// only herself, Bob (signed in, write scope) gets 403 on PUT, PATCH and DELETE
/// inside it; Alice and the admin get 2xx.
#[tokio::test]
async fn security_owner_can_close_a_container_to_other_users() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/a/x").await.0,
        StatusCode::NO_CONTENT
    );

    // Inherited from the open root: Bob writes inside Alice's container.
    assert_eq!(
        put_doc(app, &e.bob, "/ldp/a/y").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        patch_self(app, &e.bob, "/ldp/a/x").await.0,
        StatusCode::NO_CONTENT
    );

    // Alice removes the inherited grant by giving the container an ACL of its own.
    let (st, body) = put_acl(
        app,
        &e.alice,
        "/ldp/a/.acl",
        &grant_all("me", &format!("{BASE}/ldp/a"), true, "alice"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "alice writes a/.acl: {body}");

    let (st, body) = put_doc(app, &e.bob, "/ldp/a/z").await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "bob PUT inside closed container: {body}"
    );
    assert!(
        !ask(&e.state, &format!("ASK {{ <{BASE}/ldp/a/z> ?p ?o }}")),
        "a refused PUT must create nothing"
    );
    let (st, body) = patch_self(app, &e.bob, "/ldp/a/x").await;
    assert_eq!(st, StatusCode::FORBIDDEN, "bob PATCH: {body}");
    let (st, body) = delete(app, &e.bob, "/ldp/a/x").await;
    assert_eq!(st, StatusCode::FORBIDDEN, "bob DELETE: {body}");
    assert!(ask(&e.state, &format!("ASK {{ <{BASE}/ldp/a/x> ?p ?o }}")));
    // Bob still owns what he made before the container was closed.
    assert_eq!(
        delete(app, &e.bob, "/ldp/a/y").await.0,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        put_doc(app, &e.alice, "/ldp/a/z").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        patch_self(app, &e.alice, "/ldp/a/x").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        delete(app, &e.alice, "/ldp/a/z").await.0,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        put_doc(app, &e.admin, "/ldp/a/w").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        patch_self(app, &e.admin, "/ldp/a/x").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        delete(app, &e.admin, "/ldp/a/w").await.0,
        StatusCode::NO_CONTENT
    );
}

// ─── 2 ─────────────────────────────────────────────────────────────────────────

/// Plan test 2. In a closed container, Alice grants Bob `acl:Read` on one
/// resource: Bob can GET it, not its sibling, and cannot PUT it.
#[tokio::test]
async fn security_read_grant_on_one_resource_does_not_reach_siblings() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/b/r").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/b/s").await.0,
        StatusCode::NO_CONTENT
    );
    let (st, _) = put_acl(
        app,
        &e.alice,
        "/ldp/b/.acl",
        &grant_all("me", &format!("{BASE}/ldp/b"), true, "alice"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/b/r").await.0,
        StatusCode::FORBIDDEN
    );

    let (st, body) = put_acl(
        app,
        &e.alice,
        "/ldp/b/r.acl",
        &grant_mode(
            "bob",
            &format!("{BASE}/ldp/b/r"),
            "acl:agent <urn:ots:user:bob>",
            "Read",
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");

    let (st, _, body) = get(app, Some(&e.bob), "/ldp/b/r").await;
    assert_eq!(st, StatusCode::OK, "bob reads r: {body}");
    assert!(body.contains("http://example.org/p"));
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/b/s").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/b/").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        put_doc(app, &e.bob, "/ldp/b/r").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        patch_self(app, &e.bob, "/ldp/b/r").await.0,
        StatusCode::FORBIDDEN
    );
    // The owner is unaffected by the resource's own ACL naming only Bob.
    assert_eq!(get(app, Some(&e.alice), "/ldp/b/r").await.0, StatusCode::OK);
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/b/r").await.0,
        StatusCode::NO_CONTENT
    );
}

// ─── 3 ─────────────────────────────────────────────────────────────────────────

/// Plan test 3. `acl:default` on a container reaches grandchildren; a child's
/// own ACL overrides it.
#[tokio::test]
async fn security_default_reaches_grandchildren_and_own_acl_overrides() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/c/x").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/c/d/e").await.0,
        StatusCode::NO_CONTENT
    );
    let c = format!("{BASE}/ldp/c");
    let (st, body) = put_acl(
        app,
        &e.alice,
        "/ldp/c/.acl",
        &format!(
            "{}<#readers> a acl:Authorization ; acl:accessTo <{c}> ; acl:default <{c}> ;\n\
               acl:agent <urn:ots:user:bob> ; acl:mode acl:Read .\n",
            grant_all("me", &c, true, "alice")
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");

    let (st, _, body) = get(app, Some(&e.bob), "/ldp/c/d/e").await;
    assert_eq!(st, StatusCode::OK, "grandchild inherits Read: {body}");
    assert_eq!(get(app, Some(&e.bob), "/ldp/c/d/").await.0, StatusCode::OK);
    assert_eq!(
        put_doc(app, &e.bob, "/ldp/c/d/e").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        put_doc(app, &e.bob, "/ldp/c/d/f").await.0,
        StatusCode::FORBIDDEN
    );

    // The grandchild's own ACL replaces the inherited policy.
    let (st, body) = put_acl(
        app,
        &e.alice,
        "/ldp/c/d/e.acl",
        &grant_all("me", &format!("{BASE}/ldp/c/d/e"), false, "alice"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/c/d/e").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(get(app, Some(&e.bob), "/ldp/c/x").await.0, StatusCode::OK);
    assert_eq!(get(app, Some(&e.bob), "/ldp/c/d/").await.0, StatusCode::OK);

    // Deleting that ACL restores inheritance.
    assert_eq!(
        delete(app, &e.alice, "/ldp/c/d/e.acl").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(get(app, Some(&e.bob), "/ldp/c/d/e").await.0, StatusCode::OK);
}

// ─── 4 ─────────────────────────────────────────────────────────────────────────

/// Plan test 4. A `foaf:Agent` Read grant makes a resource readable without a
/// token; without that grant anonymous gets 401 as today. Nothing else opens.
#[tokio::test]
async fn security_public_read_needs_a_foaf_agent_grant() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/p/doc").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get(app, None, "/ldp/p/doc").await.0,
        StatusCode::UNAUTHORIZED
    );

    let (st, body) = put_acl(
        app,
        &e.alice,
        "/ldp/p/doc.acl",
        &grant_mode(
            "public",
            &format!("{BASE}/ldp/p/doc"),
            "acl:agentClass <http://xmlns.com/foaf/0.1/Agent>",
            "Read",
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");

    let (st, headers, body) = get(app, None, "/ldp/p/doc").await;
    assert_eq!(st, StatusCode::OK, "anonymous read: {body}");
    assert!(body.contains("http://example.org/p"));
    assert_eq!(wac_allow(&headers), "user=\"read\", public=\"read\"");
    let (st, _, _) = send(app, Method::HEAD, "/ldp/p/doc", None, &[], "").await;
    assert_eq!(st, StatusCode::OK);

    // The grant is on the document only, and only for reading.
    assert_eq!(get(app, None, "/ldp/p/").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        get(app, None, "/ldp/p/doc.acl").await.0,
        StatusCode::UNAUTHORIZED
    );
    let (st, _, _) = send(
        app,
        Method::PUT,
        "/ldp/p/doc",
        None,
        &[("Content-Type", TURTLE)],
        "<http://example.org/x> <http://example.org/p> \"anon\" .",
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, _) = send(app, Method::DELETE, "/ldp/p/doc", None, &[], "").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Without the grant: 401 again.
    assert_eq!(
        delete(app, &e.alice, "/ldp/p/doc.acl").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get(app, None, "/ldp/p/doc").await.0,
        StatusCode::UNAUTHORIZED
    );
}

// ─── 7 ─────────────────────────────────────────────────────────────────────────

/// Plan test 7. Writing a `.acl` needs `acl:Control`: the owner can, a
/// Write-only grantee cannot; an invalid ACL body is refused with 400 and the
/// old ACL stays.
#[tokio::test]
async fn security_writing_an_acl_needs_control_and_a_valid_body() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/w/doc").await.0,
        StatusCode::NO_CONTENT
    );
    let doc = format!("{BASE}/ldp/w/doc");
    let bob_write = grant_mode("bob", &doc, "acl:agent <urn:ots:user:bob>", "Write");
    let (st, body) = put_acl(app, &e.alice, "/ldp/w/doc.acl", &bob_write).await;
    assert_eq!(st, StatusCode::CREATED, "{body}");

    // Bob may write the document, not its ACL, and may not read the ACL.
    assert_eq!(
        put_doc(app, &e.bob, "/ldp/w/doc").await.0,
        StatusCode::NO_CONTENT
    );
    let (st, body) = put_acl(
        app,
        &e.bob,
        "/ldp/w/doc.acl",
        &grant_all("bob", &doc, false, "bob"),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "bob grants himself control: {body}"
    );
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/w/doc.acl").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        delete(app, &e.bob, "/ldp/w/doc.acl").await.0,
        StatusCode::FORBIDDEN
    );

    let (st, _, body) = get(app, Some(&e.alice), "/ldp/w/doc.acl").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("urn:ots:user:bob"), "{body}");
    assert!(body.contains("doc.acl#owner"), "{body}");
    assert!(body.contains("urn:ots:user:alice"), "{body}");

    // Invalid bodies: refused whole, and the old ACL stays.
    for (why, bad) in [
        (
            "another resource",
            grant_mode(
                "x",
                &format!("{BASE}/ldp/w/other"),
                "acl:agent <urn:ots:user:bob>",
                "Read",
            ),
        ),
        (
            "reserved owner node",
            grant_all("owner", &doc, false, "bob"),
        ),
        (
            "foreign agent",
            grant_mode(
                "x",
                &doc,
                "acl:agent <https://alice.example/profile#me>",
                "Read",
            ),
        ),
        (
            "unknown mode",
            grant_mode("x", &doc, "acl:agent <urn:ots:user:bob>", "Fly"),
        ),
        (
            "trusted app",
            format!(
                "<#x> a acl:Authorization ; acl:accessTo <{doc}> ; acl:agent <urn:ots:user:bob> ;\n\
                   acl:mode acl:Read ; acl:origin <https://app.example> .\n"
            ),
        ),
        ("not turtle at all", "this is not turtle {{{".to_string()),
    ] {
        let (st, body) = put_acl(app, &e.alice, "/ldp/w/doc.acl", &bad).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{why} must be a 400: {body}");
        let (_, _, acl) = get(app, Some(&e.alice), "/ldp/w/doc.acl").await;
        assert!(acl.contains("doc.acl#bob"), "{why}: old ACL gone: {acl}");
        assert!(!acl.contains("doc.acl#x"), "{why}: partial write: {acl}");
    }
    assert_eq!(
        put_doc(app, &e.bob, "/ldp/w/doc").await.0,
        StatusCode::NO_CONTENT
    );

    // The admin can always repair an ACL.
    let (st, body) = put_acl(
        app,
        &e.admin,
        "/ldp/w/doc.acl",
        &grant_all("bob", &doc, false, "bob"),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/w/doc.acl").await.0,
        StatusCode::OK
    );
}

// ─── 8 ─────────────────────────────────────────────────────────────────────────

/// Plan test 8. Deleting a resource deletes its ACL; recreating it at the same
/// path gives the new creator ownership, not the old one.
#[tokio::test]
async fn security_deleting_a_resource_deletes_its_acl_and_recreation_changes_owner() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/doc8").await.0,
        StatusCode::NO_CONTENT
    );
    let doc = format!("{BASE}/ldp/doc8");
    let (st, _) = put_acl(
        app,
        &e.alice,
        "/ldp/doc8.acl",
        &grant_mode("bob", &doc, "acl:agent <urn:ots:user:bob>", "Read"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let prefix = format!("{doc}.acl#");
    assert!(acl_graph_has_subject_prefix(&e.state, &prefix));

    assert_eq!(
        delete(app, &e.alice, "/ldp/doc8").await.0,
        StatusCode::NO_CONTENT
    );
    assert!(
        !acl_graph_has_subject_prefix(&e.state, &prefix),
        "the ACL must go with the resource"
    );
    // Control went with the owner grant (403 before 404: no existence leak);
    // the admin, who passes WAC, sees that the ACL is gone.
    assert_eq!(
        get(app, Some(&e.alice), "/ldp/doc8.acl").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(app, Some(&e.admin), "/ldp/doc8.acl").await.0,
        StatusCode::NOT_FOUND
    );

    // Bob recreates the path: he owns it, Alice does not.
    assert_eq!(
        put_doc(app, &e.bob, "/ldp/doc8").await.0,
        StatusCode::NO_CONTENT
    );
    let (st, _, body) = get(app, Some(&e.bob), "/ldp/doc8.acl").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("urn:ots:user:bob"), "{body}");
    assert!(!body.contains("urn:ots:user:alice"), "{body}");
    assert_eq!(
        get(app, Some(&e.alice), "/ldp/doc8.acl").await.0,
        StatusCode::FORBIDDEN
    );
    let (st, _) = put_acl(
        app,
        &e.alice,
        "/ldp/doc8.acl",
        &grant_all("me", &doc, false, "alice"),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

// ─── 9 ─────────────────────────────────────────────────────────────────────────

/// Plan test 9. Upgrade: a store with LDP data and no ACL graph gets the open
/// root ACL at start, and every request that worked before still succeeds.
#[tokio::test]
async fn security_upgrade_seeds_the_open_root_acl_and_keeps_existing_requests_working() {
    use open_triplestore::ldp::container;
    let store = TripleStore::in_memory().unwrap();
    let old = format!("{BASE}/ldp/old/");
    let item = format!("{BASE}/ldp/old/item");
    container::ensure_container(&store, &old).unwrap();
    container::add_member(&store, &old, &item).unwrap();
    store
        .update(&format!(
            "INSERT DATA {{ <{item}> <http://example.org/p> \"pre-upgrade\" . \
             <{item}> a <http://www.w3.org/ns/ldp#RDFSource> }}"
        ))
        .unwrap();
    assert!(!matches!(
        store.query(&format!(
            "ASK {{ GRAPH <{}> {{ ?s ?p ?o }} }}",
            wac::ACL_GRAPH
        )),
        Ok(QueryResults::Boolean(true))
    ));

    let e = env_over(store);
    let app = &e.app;
    assert!(
        acl_graph_has_subject_prefix(&e.state, &format!("{BASE}/ldp/.acl#authenticated")),
        "the open root ACL is seeded when the router is built"
    );
    assert!(wac::root_acl_seeded(&e.state.store, BASE).unwrap());

    // A plain user does today's things to yesterday's data.
    let (st, _, body) = get(app, Some(&e.bob), "/ldp/old/item").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("pre-upgrade"));
    let (st, _, body) = get(app, Some(&e.bob), "/ldp/old/").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("ldp#contains"));
    assert_eq!(
        patch_self(app, &e.bob, "/ldp/old/item").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put_doc(app, &e.bob, "/ldp/old/item2").await.0,
        StatusCode::NO_CONTENT
    );
    let (st, _, body) = send(
        app,
        Method::POST,
        "/ldp/old/",
        Some(&e.bob),
        &[("Content-Type", TURTLE), ("Slug", "posted")],
        "<> <http://example.org/p> \"posted\" .",
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(
        delete(app, &e.bob, "/ldp/old/item2").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        delete(app, &e.alice, "/ldp/old/item").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get(app, None, "/ldp/old/").await.0,
        StatusCode::UNAUTHORIZED
    );
    // Anyone signed in reads the root ACL? No: Control stays with admins.
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/.acl").await.0,
        StatusCode::FORBIDDEN
    );
    let (st, _, body) = get(app, Some(&e.admin), "/ldp/.acl").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(body.contains("AuthenticatedAgent"), "{body}");
}

// ─── 11 ────────────────────────────────────────────────────────────────────────

/// Plan test 11. `Link: rel="acl"` and `WAC-Allow` are present and correct on
/// GET and HEAD, and the ACL link is on every LDP response.
#[tokio::test]
async fn security_acl_link_and_wac_allow_headers() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/h/doc").await.0,
        StatusCode::NO_CONTENT
    );
    let doc_acl = format!("<{BASE}/ldp/h/doc.acl>; rel=\"acl\"");

    let (st, headers, _) = get(app, Some(&e.alice), "/ldp/h/doc").await;
    assert_eq!(st, StatusCode::OK);
    assert!(link_has(&headers, &doc_acl), "{headers:?}");
    assert_eq!(
        wac_allow(&headers),
        "user=\"read write append control\", public=\"\"",
        "the owner"
    );
    let (st, head_headers, _) =
        send(app, Method::HEAD, "/ldp/h/doc", Some(&e.alice), &[], "").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(wac_allow(&head_headers), wac_allow(&headers));
    assert!(link_has(&head_headers, &doc_acl));

    let (_, headers, _) = get(app, Some(&e.bob), "/ldp/h/doc").await;
    assert_eq!(
        wac_allow(&headers),
        "user=\"read write append\", public=\"\"",
        "inherited from the open root"
    );
    let (_, headers, _) = get(app, Some(&e.admin), "/ldp/h/doc").await;
    assert_eq!(
        wac_allow(&headers),
        "user=\"read write append control\", public=\"\""
    );

    // Containers, the root, the ACL itself, and write responses.
    let (_, headers, _) = get(app, Some(&e.alice), "/ldp/h/").await;
    assert!(
        link_has(&headers, &format!("<{BASE}/ldp/h.acl>; rel=\"acl\"")),
        "{headers:?}"
    );
    let (_, headers, _) = get(app, Some(&e.alice), "/ldp/").await;
    assert!(
        link_has(&headers, &format!("<{BASE}/ldp/.acl>; rel=\"acl\"")),
        "{headers:?}"
    );
    let (st, headers, _) = get(app, Some(&e.alice), "/ldp/h/doc.acl").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        link_has(&headers, &doc_acl),
        "an ACL's ACL is itself: {headers:?}"
    );
    let (st, headers, _) = send(
        app,
        Method::POST,
        "/ldp/h/",
        Some(&e.alice),
        &[("Content-Type", TURTLE), ("Slug", "posted")],
        "<> <http://example.org/p> \"posted\" .",
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert!(
        link_has(&headers, &format!("<{BASE}/ldp/h/posted.acl>; rel=\"acl\"")),
        "{headers:?}"
    );
    let (_, headers, _) = send(app, Method::OPTIONS, "/ldp/h/doc", Some(&e.alice), &[], "").await;
    assert!(link_has(&headers, &doc_acl), "{headers:?}");

    // Public modes show up once granted.
    let (st, _) = put_acl(
        app,
        &e.alice,
        "/ldp/h/doc.acl",
        &grant_mode(
            "public",
            &format!("{BASE}/ldp/h/doc"),
            "acl:agentClass <http://xmlns.com/foaf/0.1/Agent>",
            "Read",
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (_, headers, _) = get(app, Some(&e.bob), "/ldp/h/doc").await;
    assert_eq!(wac_allow(&headers), "user=\"read\", public=\"read\"");
}
