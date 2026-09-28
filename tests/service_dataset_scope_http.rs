//! A service of another dataset is exactly as absent as one that does not
//! exist — through the real router.
//!
//! `tests/service_graph_authority_http.rs` shows that every
//! `/api/datasets/{id}/services/{service_id}[/graphs]` route answers `404` for
//! a service of another dataset and leaves it untouched. This suite pins the
//! rest of that contract:
//!
//! * the `404` carries the same body as one for an id that exists nowhere, on
//!   every route, so the response does not reveal that the id is in use in
//!   another dataset;
//! * an id that exists nowhere answers `404` on every route (the `DELETE` and
//!   graph routes used to report success for it, or a `500` when adding a
//!   graph);
//! * the other dataset's service endpoint still answers afterwards;
//! * a writer still manages their own dataset's services through its path.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// Alice's dataset (private).
const DS_A: &str = "ds-alice";
/// Bob's dataset (private: Alice may neither read nor write it).
const DS_B: &str = "ds-bob";
const SVC_A: &str = "svc-alice";
const SVC_B: &str = "svc-bob";
const SVC_B_NAME: &str = "Bob's service";
const SVC_B_SLUG: &str = "bob-svc";
/// An external graph registered to Alice's dataset.
const A_GRAPH: &str = "http://alice.example/g";
/// The graph Bob scoped his service to.
const B_SCOPED: &str = "http://bob.example/g/scoped";
/// A service id that exists nowhere.
const MISSING: &str = "no-such-service";

struct Fixture {
    state: AppState,
    app: Router,
    alice: String,
    bob: String,
}

fn fixture() -> Fixture {
    let state = test_state();
    for user in ["alice", "bob"] {
        state
            .auth_db
            .create_user(
                user,
                user,
                &format!("{user}@test.com"),
                "hash",
                SystemRole::User,
            )
            .unwrap();
    }
    for (ds, owner) in [(DS_A, "alice"), (DS_B, "bob")] {
        state
            .auth_db
            .create_dataset(
                ds,
                ds,
                None,
                OwnerType::User,
                owner,
                Visibility::Private,
                None,
            )
            .unwrap();
    }
    state
        .auth_db
        .create_sparql_service(SVC_A, DS_A, "Alice's service", "alice-svc", None)
        .unwrap();
    state
        .auth_db
        .create_sparql_service(SVC_B, DS_B, SVC_B_NAME, SVC_B_SLUG, None)
        .unwrap();
    state.auth_db.add_dataset_graph(DS_A, A_GRAPH).unwrap();
    state.auth_db.add_dataset_graph(DS_B, B_SCOPED).unwrap();
    state.auth_db.add_service_graph(SVC_B, B_SCOPED).unwrap();

    Fixture {
        app: test_app(state.clone()),
        state,
        alice: mint_token("alice", "alice", "user"),
        bob: mint_token("bob", "bob", "user"),
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, String) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let req = match body {
        Some(v) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// `/api/datasets/{DS_A}/services/{service_id}{suffix}` — Alice's own path.
fn via_alice(service_id: &str, suffix: &str) -> String {
    format!("/api/datasets/{DS_A}/services/{service_id}{suffix}")
}

/// The six service routes, each with a body that would succeed on a service
/// of Alice's own dataset.
fn routes() -> [(Method, &'static str, Option<Value>); 6] {
    [
        (Method::GET, "", None),
        (
            Method::PUT,
            "",
            Some(json!({ "name": "renamed by alice", "is_active": false })),
        ),
        (Method::GET, "/graphs", None),
        (
            Method::POST,
            "/graphs",
            Some(json!({ "graph_iri": A_GRAPH })),
        ),
        (
            Method::DELETE,
            "/graphs",
            Some(json!({ "graph_iri": B_SCOPED })),
        ),
        (Method::DELETE, "", None),
    ]
}

/// Bob queries his own service endpoint (`GET …/{slug}/sparql`).
async fn bob_queries_his_service(f: &Fixture) -> (StatusCode, String) {
    let uri = format!(
        "/api/datasets/{DS_B}/services/{SVC_B_SLUG}/sparql?query={}",
        url_encode("ASK { }")
    );
    send(&f.app, Method::GET, &uri, &f.bob, None).await
}

#[tokio::test]
async fn another_datasets_service_answers_exactly_like_a_missing_one() {
    let f = fixture();
    for (method, suffix, body) in routes() {
        let (status, text) = send(
            &f.app,
            method.clone(),
            &via_alice(SVC_B, suffix),
            &f.alice,
            body.clone(),
        )
        .await;
        let (missing_status, missing_text) = send(
            &f.app,
            method.clone(),
            &via_alice(MISSING, suffix),
            &f.alice,
            body,
        )
        .await;
        assert_eq!(
            missing_status,
            StatusCode::NOT_FOUND,
            "{method} {suffix} on an id that exists nowhere: {missing_text}"
        );
        assert_eq!(
            (status, text.as_str()),
            (missing_status, missing_text.as_str()),
            "{method} {suffix}: a service of another dataset must look exactly like a missing one"
        );
    }

    // Bob's service still answers, with its scope as he left it.
    let svc = f.state.auth_db.get_sparql_service(SVC_B).unwrap().unwrap();
    assert_eq!((svc.name.as_str(), svc.is_active), (SVC_B_NAME, true));
    assert_eq!(
        f.state.auth_db.list_service_graphs(SVC_B).unwrap(),
        vec![B_SCOPED.to_string()]
    );
    let (status, text) = bob_queries_his_service(&f).await;
    assert_eq!(status, StatusCode::OK, "Bob's service endpoint: {text}");
}

/// The check narrows only the cross-dataset case: a writer still manages the
/// services of their own dataset through its path, and so does Bob.
#[tokio::test]
async fn own_services_are_still_managed_through_their_dataset() {
    let f = fixture();
    let expected = [
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::OK,
        StatusCode::CREATED,
        StatusCode::NO_CONTENT,
        StatusCode::NO_CONTENT,
    ];
    for ((method, suffix, body), want) in routes().into_iter().zip(expected) {
        // Alice removes the graph she just added, not Bob's.
        let body = if method == Method::DELETE && suffix == "/graphs" {
            Some(json!({ "graph_iri": A_GRAPH }))
        } else {
            body
        };
        let (status, text) = send(
            &f.app,
            method.clone(),
            &via_alice(SVC_A, suffix),
            &f.alice,
            body,
        )
        .await;
        assert_eq!(
            status, want,
            "{method} {suffix} on Alice's own service: {text}"
        );
        if method == Method::PUT {
            let svc: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(svc["name"], "renamed by alice");
            assert_eq!(svc["is_active"], false);
        }
        if method == Method::GET && suffix == "/graphs" {
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!([]));
        }
    }
    assert!(f.state.auth_db.get_sparql_service(SVC_A).unwrap().is_none());

    // Bob reaches his own service through his own dataset.
    let (status, text) = send(
        &f.app,
        Method::GET,
        &format!("/api/datasets/{DS_B}/services/{SVC_B}"),
        &f.bob,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let (status, text) = bob_queries_his_service(&f).await;
    assert_eq!(status, StatusCode::OK, "{text}");
}
