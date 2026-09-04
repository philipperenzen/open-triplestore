//! Federated access control (7.7): instance A acts for its user at instance
//! B through `SERVICE`; B verifies A's identity assertion against A's JWKS,
//! provisions a read-only federated user whose organisation memberships
//! follow the assertion, and authorises locally — the user sees B's
//! member-only data of the shared organisation, an anonymous call sees
//! nothing, and the assertion cannot write.
//!
//! Own binary: the remote allowlist, the outbound mode and the signer are
//! process-wide.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, Role, Visibility};
use open_triplestore::auth::oidc_rs::{AuthExt, OidcVerifier};
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use serde_json::Value;
use tower::ServiceExt as _;

const G: &str = "https://example.org/fed/members-only";

/// Serve `state` on a loopback listener; returns its origin.
fn serve(state: &mut AppState) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    state.base_url = Arc::new(origin.clone());
    origin
}

fn start(listener_origin: &str, state: AppState) {
    let app = test_app(state);
    let addr = listener_origin.trim_start_matches("http://").to_string();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::bind(&addr).await.unwrap();
            axum::serve(l, app).await.unwrap();
        });
    });
}

async fn query(app: &Router, token: &str, q: &str) -> (StatusCode, usize, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sparql")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/sparql-query")
                .header(header::ACCEPT, "application/sparql-results+json")
                .body(Body::from(q.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let txt = body_text(resp.into_body()).await;
    let rows = serde_json::from_str::<Value>(&txt)
        .ok()
        .and_then(|v| v["results"]["bindings"].as_array().map(|a| a.len()))
        .unwrap_or(0);
    (st, rows, txt)
}

#[tokio::test]
async fn identity_assertions_cross_instances_and_authorise_locally() {
    // ── B: the peer. An organisation with a member-only dataset. ──
    let (mut b, _b_token) = admin_state();
    let origin_b = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let o = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        o
    };
    b.base_url = Arc::new(origin_b.clone());
    b.auth_db
        .create_organisation("wsv-b", "Waterschap Voorbeeld", "wsv", None, None)
        .unwrap();
    b.auth_db
        .create_dataset(
            "secrets",
            "Members only",
            None,
            OwnerType::Organisation,
            "wsv-b",
            Visibility::Members,
            None,
        )
        .unwrap();
    b.auth_db.add_dataset_graph("secrets", G).unwrap();
    b.store
        .load_str(
            "<urn:s:1> <urn:p> \"one\" . <urn:s:2> <urn:p> \"two\" .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();

    // ── A: the caller's home. Same organisation slug; adm is a member. ──
    let (mut a, a_token) = admin_state();
    // The test state ships no OIDC-provider key; A needs one to sign and to serve its JWKS.
    a.oidc_provider = Some(Arc::new(
        open_triplestore::auth::oidc_provider::ProviderKeys::load_or_generate(
            &a.auth_db,
            "test-secret",
        )
        .unwrap(),
    ));
    let origin_a = serve(&mut a);
    a.auth_db
        .create_organisation("wsv-a", "Waterschap Voorbeeld", "wsv", None, None)
        .unwrap();
    a.auth_db
        .add_org_member("adm", "wsv-a", Role::Member)
        .unwrap();
    let a_app = test_app(a.clone());
    start(&origin_a, a.clone());

    // B trusts A; A may call B.
    let mut ext = AuthExt::disabled();
    ext.trusted_issuers = vec![OidcVerifier::new(origin_a.clone(), Some(origin_b.clone()))];
    b.auth_ext = Arc::new(ext);
    let b_app = test_app(b.clone());
    start(&origin_b, b.clone());
    std::env::set_var("OTS_REMOTE_ALLOWLIST", format!("{origin_b}/"));
    open_triplestore::federation::init(
        a.oidc_provider
            .clone()
            .expect("test state has provider keys"),
        &origin_a,
    );

    let q = format!("SELECT ?s ?o WHERE {{ SERVICE <{origin_b}/sparql> {{ GRAPH <{G}> {{ ?s <urn:p> ?o }} }} }}");

    // 1. Anonymous federation (the default): B shows the member-only graph to no one.
    std::env::set_var("OTS_REMOTE_AUTH", "none");
    let (st, rows, txt) = query(&a_app, &a_token, &q).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        rows, 0,
        "without an identity B sees an anonymous caller: {txt}"
    );

    // 2. With assertions: B recognises A's user as a member of wsv and shows the data.
    std::env::set_var("OTS_REMOTE_AUTH", "assert");
    // First directly at B, so a verification or provisioning failure shows its status.
    let direct = {
        let _g = open_triplestore::federation::IdentityGuard::set(
            open_triplestore::federation::identity_for(&a, "adm"),
        );
        open_triplestore::federation::assertion_for(&format!("{origin_b}/sparql"))
            .expect("assertion")
    };
    let (st, rows, txt) = query(
        &b_app,
        &direct,
        &format!("SELECT ?s WHERE {{ GRAPH <{G}> {{ ?s <urn:p> ?o }} }}"),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "B must accept A's assertion: {txt}");
    // The assertion authenticates as a federated user, not as B's own admin.
    let me = b_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {direct}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let me_status = me.status();
    let me_body = body_text(me.into_body()).await;
    assert_eq!(
        me_status,
        StatusCode::OK,
        "B authenticates the assertion: {me_body}"
    );
    assert!(
        !me_body.contains("\"id\":\"adm\""),
        "a federated user, never B's own account: {me_body}"
    );
    // Diagnostics: what the assertion carried and what B made of it.
    let payload = {
        use base64::Engine as _;
        let p = direct.split('.').nth(1).unwrap();
        String::from_utf8(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(p)
                .unwrap(),
        )
        .unwrap()
    };
    let b_users: Vec<String> = b
        .auth_db
        .list_users()
        .unwrap_or_default()
        .into_iter()
        .map(|u| format!("{}:{}:{}", u.id, u.username, u.email))
        .collect();
    let b_members: Vec<String> = b
        .auth_db
        .list_org_members("wsv-b")
        .unwrap()
        .into_iter()
        .map(|(u, r)| format!("{}:{:?}", u.username, r))
        .collect();
    assert_eq!(rows, 2, "the federated user is a wsv member on B: {txt}\nassertion: {payload}\nB users: {b_users:?}\nB wsv members: {b_members:?}");
    let (st, rows, txt) = query(&a_app, &a_token, &format!("# as the user\n{q}")).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        rows, 2,
        "the federated identity is a wsv member on B: {txt}"
    );
    // B provisioned a federated user and put it in the organisation.
    let members: Vec<String> = b
        .auth_db
        .list_org_members("wsv-b")
        .unwrap()
        .into_iter()
        .map(|(u, _)| u.id)
        .collect();
    assert!(
        members.iter().any(|id| id != "adm"),
        "a federated member exists: {members:?}"
    );

    // 3. An assertion is read-only at B: a SPARQL update with it is refused.
    let assertion = {
        let _g = open_triplestore::federation::IdentityGuard::set(
            open_triplestore::federation::identity_for(&a, "adm"),
        );
        open_triplestore::federation::assertion_for(&format!("{origin_b}/sparql"))
            .expect("assertion")
    };
    let resp = b_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sparql")
                .header(header::AUTHORIZATION, format!("Bearer {assertion}"))
                .header(header::CONTENT_TYPE, "application/sparql-update")
                .body(Body::from(format!(
                    "INSERT DATA {{ GRAPH <{G}> {{ <urn:s:3> <urn:p> \"three\" }} }}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.status() == StatusCode::FORBIDDEN || resp.status() == StatusCode::UNAUTHORIZED,
        "federated principals cannot write: {}",
        resp.status()
    );
    // …but the same assertion reads directly too.
    let (st, rows, txt) = query(
        &b_app,
        &assertion,
        &format!("SELECT ?s WHERE {{ GRAPH <{G}> {{ ?s <urn:p> ?o }} }}"),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(rows, 2, "{txt}");

    // 4. An assertion for another audience is refused.
    let wrong = {
        let _g = open_triplestore::federation::IdentityGuard::set(
            open_triplestore::federation::identity_for(&a, "adm"),
        );
        open_triplestore::federation::assertion_for("http://127.0.0.1:1/sparql").expect("assertion")
    };
    let me = b_app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {wrong}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        me.status(),
        StatusCode::UNAUTHORIZED,
        "an assertion for another audience is refused"
    );
}
