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

// ─── 5 ─────────────────────────────────────────────────────────────────────────

/// Plan test 5. A resource body that contains ACL triples changes no ACL: with
/// the resource as subject they land in the default graph as ordinary triples;
/// naming the ACL resource itself is refused as any other resource under
/// `/ldp/` is; a body that names the ACL graph is refused outright.
#[tokio::test]
async fn security_acl_triples_in_a_resource_body_change_no_acl() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/e/doc").await.0,
        StatusCode::NO_CONTENT
    );
    let (st, _) = put_acl(
        app,
        &e.alice,
        "/ldp/e/.acl",
        &grant_all("me", &format!("{BASE}/ldp/e"), true, "alice"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/e/doc").await.0,
        StatusCode::FORBIDDEN
    );
    let doc = format!("{BASE}/ldp/e/doc");
    let bob_all = "acl:agent <urn:ots:user:bob> ; acl:mode acl:Read, acl:Write, acl:Control";
    let acl_graph_before = {
        let (_, _, acl) = get(app, Some(&e.alice), "/ldp/e/doc.acl").await;
        acl
    };

    // The resource as subject: accepted as plain triples in the default graph.
    let (st, _, body) = send(
        app,
        Method::PUT,
        "/ldp/e/doc",
        Some(&e.alice),
        &[("Content-Type", TURTLE)],
        &format!("@prefix acl: <{ACL}> .\n<> a acl:Authorization ; acl:accessTo <> ; {bob_all} ."),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");
    assert!(
        ask(
            &e.state,
            &format!("ASK {{ <{doc}> <{ACL}agent> <urn:ots:user:bob> }}")
        ),
        "an ordinary default-graph triple"
    );
    assert!(
        !ask(
            &e.state,
            &format!(
                "ASK {{ GRAPH <{}> {{ ?s <{ACL}agent> <urn:ots:user:bob> }} }}",
                wac::ACL_GRAPH
            )
        ),
        "nothing reached the ACL graph"
    );
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/e/doc").await.0,
        StatusCode::FORBIDDEN
    );

    // The ACL resource as subject: refused, like any other resource under /ldp/.
    for (ct, body) in [
        (
            TURTLE,
            format!("@prefix acl: <{ACL}> .\n<{doc}.acl#grant> a acl:Authorization ; acl:accessTo <{doc}> ; {bob_all} ."),
        ),
        (
            "application/ld+json",
            format!(
                r#"{{"@id":"{doc}.acl#grant","@type":"{ACL}Authorization","{ACL}accessTo":{{"@id":"{doc}"}},"{ACL}agent":{{"@id":"urn:ots:user:bob"}},"{ACL}mode":{{"@id":"{ACL}Control"}}}}"#
            ),
        ),
        // The ACL graph named outright (a JSON-LD named graph): refused before anything is written.
        (
            "application/ld+json",
            format!(
                r#"{{"@id":"{}","@graph":[{{"@id":"{doc}.acl#grant","@type":"{ACL}Authorization","{ACL}accessTo":{{"@id":"{doc}"}},"{ACL}agent":{{"@id":"urn:ots:user:bob"}},"{ACL}mode":{{"@id":"{ACL}Control"}}}}]}}"#,
                wac::ACL_GRAPH
            ),
        ),
    ] {
        for (method, path) in [(Method::PUT, "/ldp/e/doc"), (Method::POST, "/ldp/e/")] {
            let (st, _, resp) = send(
                app,
                method.clone(),
                path,
                Some(&e.alice),
                &[("Content-Type", ct), ("Slug", "planted")],
                &body,
            )
            .await;
            assert!(
                st == StatusCode::FORBIDDEN || st == StatusCode::BAD_REQUEST,
                "{method} {ct} body naming the ACL must be refused, got {st}: {resp}"
            );
        }
    }
    let (_, _, acl_graph_after) = get(app, Some(&e.alice), "/ldp/e/doc.acl").await;
    assert_eq!(acl_graph_after, acl_graph_before, "the ACL is untouched");
    assert!(!acl_graph_has_subject_prefix(
        &e.state,
        &format!("{doc}.acl#grant")
    ));
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/e/doc").await.0,
        StatusCode::FORBIDDEN
    );
}

// ─── 6 ─────────────────────────────────────────────────────────────────────────

/// Plan test 6. A `PATCH` that names another resource's subject, or uses
/// `GRAPH`, `CLEAR`, `DROP`, `LOAD`, `SERVICE`, `WITH` or `USING`, is refused
/// and changes nothing; one confined to the resource works.
#[tokio::test]
async fn security_patch_is_confined_to_the_target_resource() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/q/one").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/q/two").await.0,
        StatusCode::NO_CONTENT
    );
    let one = format!("{BASE}/ldp/q/one");
    let two = format!("{BASE}/ldp/q/two");
    let victim = "http://victim.example/private";
    e.state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{victim}> {{ <http://s/seed> <http://p/> \"SEED\" }} . \
             <http://other.example/subject> <http://p/> \"DEFAULT_GRAPH_SEED\" }}"
        ))
        .unwrap();
    let intact = |state: &AppState| {
        ask(
            state,
            &format!("ASK {{ <{two}> <http://example.org/p> \"v\" }}"),
        ) && ask(
            state,
            &format!("ASK {{ GRAPH <{victim}> {{ <http://s/seed> <http://p/> \"SEED\" }} }}"),
        ) && ask(
            state,
            "ASK { <http://other.example/subject> <http://p/> \"DEFAULT_GRAPH_SEED\" }",
        ) && ask(
            state,
            &format!("ASK {{ <{one}> a <http://www.w3.org/ns/ldp#RDFSource> }}"),
        ) && ask(
            state,
            &format!("ASK {{ <{BASE}/ldp/q/> <http://www.w3.org/ns/ldp#contains> <{one}> }}"),
        )
    };
    assert!(intact(&e.state));

    // Bob holds acl:Write on `one` through the open root, and nothing more.
    for evil in [
        format!("INSERT DATA {{ <{two}> <http://example.org/p> \"smuggled\" }}"),
        format!("DELETE WHERE {{ <{two}> ?p ?o }}"),
        format!("INSERT DATA {{ GRAPH <{victim}> {{ <{one}> <http://p/> \"x\" }} }}"),
        format!("INSERT {{ <{one}> <http://p/> ?o }} WHERE {{ GRAPH <{victim}> {{ ?s ?p ?o }} }}"),
        format!("INSERT {{ <{one}> <http://p/> ?o }} WHERE {{ GRAPH ?g {{ ?s ?p ?o }} }}"),
        format!("INSERT {{ <{one}> <http://p/> ?o }} WHERE {{ SERVICE <http://x.example/sparql> {{ ?s ?p ?o }} }}"),
        format!("WITH <{victim}> DELETE {{ ?s ?p ?o }} WHERE {{ ?s ?p ?o }}"),
        format!("DELETE {{ <{one}> ?p ?o }} USING <{victim}> WHERE {{ ?s ?p ?o }}"),
        "CLEAR ALL".to_string(),
        "CLEAR DEFAULT".to_string(),
        "DROP ALL".to_string(),
        format!("DROP GRAPH <{victim}>"),
        "LOAD <http://example.org/data.ttl>".to_string(),
        "CREATE GRAPH <http://new.example/>".to_string(),
        format!("DELETE DATA {{ <{one}> a <http://www.w3.org/ns/ldp#RDFSource> }}"),
        format!("INSERT DATA {{ <{one}> <http://www.w3.org/ns/ldp#contains> <{two}> }}"),
        format!("INSERT {{ ?other <http://example.org/p> \"x\" }} WHERE {{ VALUES ?other {{ <{two}> }} }}"),
    ] {
        let (st, _, body) = send(
            app,
            Method::PATCH,
            "/ldp/q/one",
            Some(&e.bob),
            &[("Content-Type", SPARQL_UPDATE)],
            &evil,
        )
        .await;
        assert_eq!(st, StatusCode::FORBIDDEN, "`{evil}` must be refused: {body}");
        assert!(intact(&e.state), "`{evil}` changed something it must not");
        assert!(
            ask(&e.state, &format!("ASK {{ <{one}> <http://example.org/p> \"v\" }}")),
            "`{evil}` changed the resource although refused"
        );
    }
    let (st, _, _) = send(
        app,
        Method::PATCH,
        "/ldp/q/one",
        Some(&e.bob),
        &[("Content-Type", SPARQL_UPDATE)],
        "not sparql at all",
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // A subject outside /ldp/ is the resource's to describe (as in a PUT body),
    // and a WHERE clause about one sees nothing: a no-op, not a breach.
    let (st, _, body) = send(
        app,
        Method::PATCH,
        "/ldp/q/one",
        Some(&e.bob),
        &[("Content-Type", SPARQL_UPDATE)],
        "DELETE WHERE { <http://other.example/subject> ?p ?o }",
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");
    assert!(intact(&e.state));

    // PUT and POST bodies are held to the same line: no triples about another
    // resource under /ldp/, whatever the target.
    for (method, path) in [(Method::PUT, "/ldp/q/mine"), (Method::POST, "/ldp/q/")] {
        let (st, _, body) = send(
            app,
            method.clone(),
            path,
            Some(&e.bob),
            &[("Content-Type", TURTLE), ("Slug", "mine")],
            &format!(
                "<> <http://example.org/p> \"ok\" . <{two}> <http://example.org/p> \"smuggled\" ."
            ),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "{method} body about `two`: {body}"
        );
        assert!(intact(&e.state));
        assert!(!ask(
            &e.state,
            &format!("ASK {{ <{two}> <http://example.org/p> \"smuggled\" }}")
        ));
        let (st, _, body) = send(
            app,
            method.clone(),
            path,
            Some(&e.bob),
            &[("Content-Type", TURTLE), ("Slug", "mine")],
            "<> <http://example.org/p> \"ok\" . <http://example.org/related> <http://example.org/p> \"fine\" .",
        )
        .await;
        assert!(
            st.is_success(),
            "{method} body about the target and an outside subject: {st} {body}"
        );
    }

    // Confined to the resource: allowed, and visible on GET.
    let (st, _, body) = send(
        app,
        Method::PATCH,
        "/ldp/q/one",
        Some(&e.bob),
        &[("Content-Type", SPARQL_UPDATE)],
        "DELETE { <> <http://example.org/p> ?o } INSERT { <> <http://example.org/p> \"new\" } \
         WHERE { <> <http://example.org/p> ?o }",
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");
    let (st, _, body) = get(app, Some(&e.bob), "/ldp/q/one").await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body.contains("\"new\"") && !body.contains("\"v\""),
        "{body}"
    );
    assert!(intact(&e.state));
    // A wildcard delete empties the resource and nothing else; it stays an LDP resource.
    let (st, _, _) = send(
        app,
        Method::PATCH,
        "/ldp/q/one",
        Some(&e.bob),
        &[("Content-Type", SPARQL_UPDATE)],
        "DELETE { ?s ?p ?o } WHERE { ?s ?p ?o }",
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(intact(&e.state));
    assert_eq!(get(app, Some(&e.bob), "/ldp/q/one").await.0, StatusCode::OK);
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
        !wac::root_acl_seeded(&e.state.store, BASE).unwrap(),
        "building the router writes nothing to the store"
    );

    // A plain user does today's things to yesterday's data; the first LDP
    // request seeds the open root ACL (at startup, the boot seed does).
    let (st, _, body) = get(app, Some(&e.bob), "/ldp/old/item").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert!(
        acl_graph_has_subject_prefix(&e.state, &format!("{BASE}/ldp/.acl#authenticated")),
        "the open root ACL is seeded by the first LDP request"
    );
    assert!(wac::root_acl_seeded(&e.state.store, BASE).unwrap());
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

// ─── 10 ────────────────────────────────────────────────────────────────────────

/// Plan test 10. A lookup failure fails closed: when the membership lookup
/// cannot run, a non-admin gets 403 on every verb and nothing is written;
/// admins, who pass before any lookup, are unaffected.
#[tokio::test]
async fn security_acl_lookup_failure_fails_closed() {
    let e = env();
    let app = &e.app;
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/f/doc").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(get(app, Some(&e.bob), "/ldp/f/doc").await.0, StatusCode::OK);

    // Every group lookup now fails, as a database error would: the in-memory
    // pool holds one connection. (Only WAC reads `groups`; the endpoint ACL
    // reads the membership tables, which stay.)
    e.state
        .auth_db
        .pool()
        .get()
        .unwrap()
        .execute_batch("DROP TABLE groups")
        .unwrap();

    let (st, _, body) = get(app, Some(&e.bob), "/ldp/f/doc").await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.contains("lookup failed"),
        "the refusal names the failed lookup: {body}"
    );
    assert_eq!(
        put_doc(app, &e.bob, "/ldp/f/new").await.0,
        StatusCode::FORBIDDEN
    );
    assert!(!ask(
        &e.state,
        &format!("ASK {{ <{BASE}/ldp/f/new> ?p ?o }}")
    ));
    assert_eq!(
        patch_self(app, &e.bob, "/ldp/f/doc").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        delete(app, &e.bob, "/ldp/f/doc").await.0,
        StatusCode::FORBIDDEN
    );
    let (st, _, _) = send(
        app,
        Method::POST,
        "/ldp/f/",
        Some(&e.bob),
        &[("Content-Type", TURTLE), ("Slug", "posted")],
        "<> <http://example.org/p> \"posted\" .",
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/f/doc.acl").await.0,
        StatusCode::FORBIDDEN
    );
    assert!(ask(
        &e.state,
        &format!("ASK {{ <{BASE}/ldp/f/doc> <http://example.org/p> \"v\" }}")
    ));
    assert_eq!(
        get(app, Some(&e.admin), "/ldp/f/doc").await.0,
        StatusCode::OK
    );
}

// ─── LDP_ROOT_ACL=owners ───────────────────────────────────────────────────────

/// The `owners` seed (`LDP_ROOT_ACL=owners`) starts a fresh install closed:
/// users reach only what they create or are granted; admins everything. The
/// policy is applied through the seed function rather than the environment
/// variable, which is process-wide and would race other tests.
#[tokio::test]
async fn security_owners_root_policy_starts_closed() {
    let store = TripleStore::in_memory().unwrap();
    assert!(wac::ensure_root_acl(&store, BASE, wac::RootAclPolicy::Owners).unwrap());
    let e = env_over(store);
    let app = &e.app;
    assert!(
        !acl_graph_has_subject_prefix(&e.state, &format!("{BASE}/ldp/.acl#authenticated")),
        "neither the router build nor a request re-seeds an already seeded root"
    );
    assert!(!wac::root_acl_is_open(&e.state.store, BASE).unwrap());

    assert_eq!(
        get(app, Some(&e.alice), "/ldp/").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/mine").await.0,
        StatusCode::FORBIDDEN
    );
    let (st, _, _) = send(
        app,
        Method::POST,
        "/ldp/",
        Some(&e.alice),
        &[("Content-Type", TURTLE), ("Slug", "x")],
        "<> <http://example.org/p> \"x\" .",
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // The admin opens one container to Alice; she owns what she creates there.
    assert_eq!(
        put_doc(app, &e.admin, "/ldp/team/readme").await.0,
        StatusCode::NO_CONTENT
    );
    let team = format!("{BASE}/ldp/team");
    let (st, body) = put_acl(
        app,
        &e.admin,
        "/ldp/team/.acl",
        &format!(
            "<#alice> a acl:Authorization ; acl:accessTo <{team}> ; acl:default <{team}> ;\n\
               acl:agent <urn:ots:user:alice> ; acl:mode acl:Read, acl:Append .\n"
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(
        get(app, Some(&e.alice), "/ldp/team/readme").await.0,
        StatusCode::OK
    );
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/team/readme").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/team/notes").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/team/notes").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get(app, Some(&e.bob), "/ldp/team/notes").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        get(app, Some(&e.alice), "/ldp/").await.0,
        StatusCode::FORBIDDEN
    );
}

// ─── 11 ────────────────────────────────────────────────────────────────────────

/// Plan test 11. `Link: rel="acl"` and `WAC-Allow` are present and correct on
/// GET and HEAD, and the ACL link is on every LDP response — a 404 for a
/// resource that does not exist included.
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

    // A resource that does not exist is a 404 that still carries the ACL link
    // and the caller's modes for that path, so ACL discovery and creating it
    // with PUT keep working: never created, mentioned by another resource
    // (an object of `doc`), deleted, and an intermediate path.
    let (st, _, _) = send(
        app,
        Method::PUT,
        "/ldp/h/linker",
        Some(&e.alice),
        &[("Content-Type", TURTLE)],
        &format!("<{BASE}/ldp/h/linker> <http://example.org/p> <{BASE}/ldp/h/ghost> ."),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/deep/a/b").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/h/gone").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        delete(app, &e.alice, "/ldp/h/gone").await.0,
        StatusCode::NO_CONTENT
    );
    // Alice owns `/ldp/h/` (its default reaches her missing paths there) but
    // not the intermediate `/ldp/deep/`, where only the open root reaches.
    for (path, modes) in [
        ("/ldp/h/never", "read write append control"),
        ("/ldp/h/ghost", "read write append control"),
        ("/ldp/h/gone", "read write append control"),
        ("/ldp/deep/", "read write append"),
    ] {
        let acl = format!("<{BASE}{}.acl>; rel=\"acl\"", path.trim_end_matches('/'));
        for method in [Method::GET, Method::HEAD] {
            let (st, headers, body) = send(
                app,
                method.clone(),
                path,
                Some(&e.alice),
                &[("Accept", TURTLE)],
                "",
            )
            .await;
            assert_eq!(st, StatusCode::NOT_FOUND, "{method} {path}: {body}");
            assert!(link_has(&headers, &acl), "{method} {path}: {headers:?}");
            assert!(
                link_has(&headers, "rel=\"http://www.w3.org/ns/ldp#constrainedBy\""),
                "{method} {path}: {headers:?}"
            );
            assert_eq!(
                wac_allow(&headers),
                format!("user=\"{modes}\", public=\"\""),
                "{method} {path}: the modes alice would hold, inherited"
            );
        }
    }
    // Bob's modes on a missing path are his, inherited from the open root.
    let (st, headers, _) = get(app, Some(&e.bob), "/ldp/h/never").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        wac_allow(&headers),
        "user=\"read write append\", public=\"\""
    );
    // Creating where the 404 pointed works.
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/h/never").await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get(app, Some(&e.alice), "/ldp/h/never").await.0,
        StatusCode::OK
    );
    // The ACL of a resource that does not exist is a 404 that links the ACL
    // to itself.
    let (st, headers, _) = get(app, Some(&e.alice), "/ldp/h/missing.acl").await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(
        link_has(
            &headers,
            &format!("<{BASE}/ldp/h/missing.acl>; rel=\"acl\"")
        ),
        "{headers:?}"
    );
    // A caller who may not read the path learns nothing about whether it
    // exists: 403 either way.
    assert_eq!(
        put_doc(app, &e.alice, "/ldp/closed/x").await.0,
        StatusCode::NO_CONTENT
    );
    let (st, _) = put_acl(
        app,
        &e.alice,
        "/ldp/closed/.acl",
        &grant_all("me", &format!("{BASE}/ldp/closed"), true, "alice"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    for path in ["/ldp/closed/x", "/ldp/closed/missing"] {
        let (st, headers, _) = get(app, Some(&e.bob), path).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{path}");
        assert!(headers.get("wac-allow").is_none(), "{path}: {headers:?}");
    }
}
