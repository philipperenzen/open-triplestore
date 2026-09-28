//! A dataset's SPARQL service serves only graphs its dataset holds — through
//! the real router.
//!
//! * `/api/datasets/{id}/services/{service_id}/…` reaches a service only when
//!   it belongs to `{id}`: a writer of one dataset cannot read, rename,
//!   delete or re-scope another dataset's service through their own (404);
//! * adding a graph to a service needs the dataset to hold it (its namespace,
//!   its well-known graphs, or registered to it), unless the caller is an
//!   admin: another tenant's graph or a `urn:system:` graph answers 403;
//! * at query time the service drops every graph its dataset does not hold,
//!   so rows made before the rule, and rows whose graph was detached since,
//!   serve nothing — and a service left with none serves nothing rather than
//!   widening to the whole dataset.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// Alice's dataset: public, so an anonymous caller may query its services.
const DS_A: &str = "tenant-a";
/// Bob's dataset: private.
const DS_B: &str = "tenant-b";

/// Inside tenant-a's namespace (held without registration).
const A_OWN: &str = "http://localhost:7878/dataset/tenant-a/own";
/// An external graph registered to tenant-a.
const A_EXTERNAL: &str = "http://alice.example/data";
/// An external graph registered to tenant-b only.
const B_SECRET: &str = "http://bob.example/secret";
/// A system graph, registered to no dataset.
const SYSTEM: &str = "urn:system:service-authority-test";

/// Every graph carries one triple whose object names it, so a response body
/// shows which graphs a query reached.
fn seed(state: &AppState) {
    for (graph, label) in [
        (A_OWN, "alice-own"),
        (A_EXTERNAL, "alice-external"),
        (B_SECRET, "bob-secret"),
        (SYSTEM, "system-secret"),
    ] {
        state
            .store
            .update(&format!(
                "INSERT DATA {{ GRAPH <{graph}> {{ <http://ex.org/s> <http://ex.org/p> \"{label}\" }} }}"
            ))
            .unwrap();
    }
}

/// `(state, admin, alice, bob)`: alice owns public tenant-a (with
/// `A_EXTERNAL` registered), bob owns private tenant-b (with `B_SECRET`).
fn setup() -> (AppState, String, String, String) {
    let (state, admin) = admin_state();
    for id in ["alice", "bob"] {
        state
            .auth_db
            .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
            .unwrap();
    }
    state
        .auth_db
        .create_dataset(
            DS_A,
            DS_A,
            None,
            OwnerType::User,
            "alice",
            Visibility::Public,
            None,
        )
        .unwrap();
    state
        .auth_db
        .create_dataset(
            DS_B,
            DS_B,
            None,
            OwnerType::User,
            "bob",
            Visibility::Private,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph(DS_A, A_EXTERNAL).unwrap();
    state.auth_db.add_dataset_graph(DS_B, B_SECRET).unwrap();
    seed(&state);
    let alice = mint_token("alice", "alice", "user");
    let bob = mint_token("bob", "bob", "user");
    (state, admin, alice, bob)
}

async fn call(
    state: &AppState,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = test_app(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// Create service `slug` on `dataset`; returns its id.
async fn create_service(state: &AppState, dataset: &str, slug: &str, token: &str) -> String {
    let (status, body) = call(
        state,
        Method::POST,
        &format!("/api/datasets/{dataset}/services"),
        Some(token),
        Some(json!({ "name": slug, "slug": slug })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create service: {body}");
    serde_json::from_str::<Value>(&body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn add_graph(
    state: &AppState,
    dataset: &str,
    service_id: &str,
    graph: &str,
    token: &str,
) -> (StatusCode, String) {
    call(
        state,
        Method::POST,
        &format!("/api/datasets/{dataset}/services/{service_id}/graphs"),
        Some(token),
        Some(json!({ "graph_iri": graph })),
    )
    .await
}

/// Every object in every graph the service reaches.
const ALL_OBJECTS: &str = "SELECT ?g ?o WHERE { GRAPH ?g { ?s ?p ?o } }";
/// The graph-listing shape the endpoint answers from its graph index.
const LIST_GRAPHS: &str = "SELECT ?g WHERE { GRAPH ?g { ?s ?p ?o } }";

async fn query(
    state: &AppState,
    dataset: &str,
    slug: &str,
    token: Option<&str>,
    sparql: &str,
) -> String {
    let (status, body) = call(
        state,
        Method::GET,
        &format!(
            "/api/datasets/{dataset}/services/{slug}/sparql?query={}",
            url_encode(sparql)
        ),
        token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "query {dataset}/{slug}: {body}");
    body
}

fn assert_no_foreign_data(body: &str) {
    for secret in ["bob-secret", "system-secret", B_SECRET, SYSTEM] {
        assert!(!body.contains(secret), "service leaked {secret}: {body}");
    }
}

#[tokio::test]
async fn a_writer_cannot_scope_their_service_to_another_tenants_or_a_system_graph() {
    let (state, _admin, alice, _bob) = setup();
    let svc = create_service(&state, DS_A, "leak", &alice).await;

    for graph in [B_SECRET, SYSTEM] {
        let (status, body) = add_graph(&state, DS_A, &svc, graph, &alice).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "a writer of {DS_A} scoped a service to <{graph}>: {body}"
        );
    }
    assert!(state.auth_db.list_service_graphs(&svc).unwrap().is_empty());

    // Neither alice nor an anonymous reader of her public dataset reaches it.
    for token in [Some(alice.as_str()), None] {
        assert_no_foreign_data(&query(&state, DS_A, "leak", token, ALL_OBJECTS).await);
        assert_no_foreign_data(&query(&state, DS_A, "leak", token, LIST_GRAPHS).await);
    }
}

#[tokio::test]
async fn another_datasets_service_is_not_reachable_through_your_own_dataset() {
    let (state, _admin, alice, bob) = setup();
    let bob_svc = create_service(&state, DS_B, "b-svc", &bob).await;
    let (status, body) = add_graph(&state, DS_B, &bob_svc, B_SECRET, &bob).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "bob scopes their own service: {body}"
    );

    let svc_uri = format!("/api/datasets/{DS_A}/services/{bob_svc}");
    let graphs_uri = format!("{svc_uri}/graphs");
    let attempts: [(Method, &str, Option<Value>); 6] = [
        (Method::GET, &svc_uri, None),
        (Method::GET, &graphs_uri, None),
        (
            Method::POST,
            &graphs_uri,
            Some(json!({ "graph_iri": A_OWN })),
        ),
        (
            Method::DELETE,
            &graphs_uri,
            Some(json!({ "graph_iri": B_SECRET })),
        ),
        (
            Method::PUT,
            &svc_uri,
            Some(json!({ "name": "hijacked", "is_active": false })),
        ),
        (Method::DELETE, &svc_uri, None),
    ];
    for (method, uri, body) in attempts {
        let label = format!("{method} {uri}");
        let (status, text) = call(&state, method, uri, Some(&alice), body).await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "{label} reached tenant-b's service through tenant-a: {text}"
        );
        assert!(!text.contains(B_SECRET), "{label} leaked: {text}");
    }

    // Bob's service is untouched.
    let svc = state.auth_db.get_sparql_service(&bob_svc).unwrap().unwrap();
    assert_eq!(svc.name, "b-svc");
    assert!(svc.is_active);
    assert_eq!(
        state.auth_db.list_service_graphs(&bob_svc).unwrap(),
        vec![B_SECRET.to_string()]
    );
}

#[tokio::test]
async fn a_service_serves_the_graphs_its_dataset_holds() {
    let (state, _admin, alice, _bob) = setup();
    let svc = create_service(&state, DS_A, "mine", &alice).await;

    let metadata = format!("urn:system:metadata:dataset:{DS_A}");
    for graph in [A_OWN, A_EXTERNAL, metadata.as_str()] {
        let (status, body) = add_graph(&state, DS_A, &svc, graph, &alice).await;
        assert_eq!(status, StatusCode::CREATED, "<{graph}>: {body}");
    }

    let body = query(&state, DS_A, "mine", Some(&alice), ALL_OBJECTS).await;
    assert!(body.contains("alice-own"), "{body}");
    assert!(body.contains("alice-external"), "{body}");
    assert_no_foreign_data(&body);
}

#[tokio::test]
async fn stale_service_graphs_serve_nothing() {
    let (state, _admin, alice, _bob) = setup();

    // Rows made before the rule: a foreign and a system graph.
    let stale = create_service(&state, DS_A, "stale", &alice).await;
    state.auth_db.add_service_graph(&stale, B_SECRET).unwrap();
    state.auth_db.add_service_graph(&stale, SYSTEM).unwrap();
    for token in [Some(alice.as_str()), None] {
        for sparql in [ALL_OBJECTS, LIST_GRAPHS] {
            let body = query(&state, DS_A, "stale", token, sparql).await;
            assert_no_foreign_data(&body);
            // A service whose graphs are all stale serves nothing; it does
            // not fall back to every graph of the dataset.
            assert!(!body.contains("alice-own"), "{body}");
            assert!(!body.contains("alice-external"), "{body}");
        }
    }
    // The owner may still clear such a row.
    let (status, body) = call(
        &state,
        Method::DELETE,
        &format!("/api/datasets/{DS_A}/services/{stale}/graphs"),
        Some(&alice),
        Some(json!({ "graph_iri": B_SECRET })),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        state.auth_db.list_service_graphs(&stale).unwrap(),
        vec![SYSTEM.to_string()]
    );

    // A graph detached from the dataset stops serving through the service.
    let detach = create_service(&state, DS_A, "detach", &alice).await;
    let (status, body) = add_graph(&state, DS_A, &detach, A_EXTERNAL, &alice).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let body = query(&state, DS_A, "detach", Some(&alice), ALL_OBJECTS).await;
    assert!(body.contains("alice-external"), "{body}");
    state
        .auth_db
        .remove_dataset_graph(DS_A, A_EXTERNAL)
        .unwrap();
    let body = query(&state, DS_A, "detach", Some(&alice), ALL_OBJECTS).await;
    assert!(
        !body.contains("alice-external"),
        "detached graph served: {body}"
    );
}

#[tokio::test]
async fn an_admin_may_name_any_graph_but_it_serves_once_the_dataset_holds_it() {
    let (state, admin, alice, _bob) = setup();
    let svc = create_service(&state, DS_A, "curated", &alice).await;

    let (status, body) = add_graph(&state, DS_A, &svc, B_SECRET, &admin).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_no_foreign_data(&query(&state, DS_A, "curated", Some(&alice), ALL_OBJECTS).await);

    // Registering the graph to the dataset is the admin's decision to expose
    // it to the dataset's readers; from then on the service serves it.
    let (status, body) = call(
        &state,
        Method::POST,
        &format!("/api/datasets/{DS_A}/graphs"),
        Some(&admin),
        Some(json!({ "graph_iri": B_SECRET })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let body = query(&state, DS_A, "curated", Some(&alice), ALL_OBJECTS).await;
    assert!(body.contains("bob-secret"), "{body}");
}
