//! A deactivated SPARQL service must stop answering.
//!
//! `PUT /api/datasets/:dataset_id/services/:service_id` with `"is_active": false`
//! is how an owner switches a service off — the dataset page greys the service out
//! and stops showing its endpoint URL. The query route
//! (`/api/datasets/:dataset_id/services/:service_slug/sparql`) used to ignore the
//! flag and keep answering, so deactivating a public dataset's service had no
//! effect. An inactive service now answers exactly like a missing one — 404
//! "Service not found" — for every caller, the dataset's own writers included,
//! and whether or not a version is pinned.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt as _;

use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::dataset_versions::{models::VersionStatus, snapshot_as_version};

const MARKER: &str = "SERVICE_ACTIVE_MARKER";
const GRAPH: &str = "http://example.org/g/data";
const VERSION: &str = "1.0.0";
const QUERY: &str = "SELECT ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } }";

struct Fixture {
    app: Router,
    owner: String,
    admin: String,
}

/// A public dataset `ds1` owned by `owner`, one graph holding [`MARKER`],
/// snapshotted into version [`VERSION`].
fn setup() -> Fixture {
    let (state, admin) = admin_state();
    state
        .auth_db
        .create_user("owner", "owner", "owner@test.com", "hash", SystemRole::User)
        .unwrap();
    let ds = state
        .auth_db
        .create_dataset(
            "ds1",
            "DS1",
            None,
            OwnerType::User,
            "owner",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph(&ds.id, GRAPH).unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{GRAPH}> {{ <http://example.org/s> <http://example.org/p> \"{MARKER}\" }} }}"
        ))
        .unwrap();
    snapshot_as_version(
        &state.store,
        state.base_url.as_str(),
        &ds.id,
        VERSION,
        &[GRAPH.to_string()],
        VersionStatus::Published,
        None,
        None,
    )
    .expect("snapshot_as_version should succeed");

    Fixture {
        app: test_app(state),
        owner: mint_token("owner", "owner", "user"),
        admin,
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: String,
) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(ct) = content_type {
        req = req.header(header::CONTENT_TYPE, ct);
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn send_json(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let (status, text) = send(
        app,
        method,
        uri,
        Some(token),
        Some("application/json"),
        body.to_string(),
    )
    .await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// GET the service's SPARQL endpoint with [`QUERY`], optionally pinned to a version.
async fn query_get(
    app: &Router,
    slug: &str,
    token: Option<&str>,
    version: Option<&str>,
) -> (StatusCode, String) {
    let mut uri = format!(
        "/api/datasets/ds1/services/{slug}/sparql?query={}",
        url_encode(QUERY)
    );
    if let Some(v) = version {
        uri.push_str(&format!("&version={v}"));
    }
    send(app, Method::GET, &uri, token, None, String::new()).await
}

/// Create a service over HTTP as the owner; returns its id.
async fn create_service(fx: &Fixture, slug: &str) -> String {
    let (status, body) = send_json(
        &fx.app,
        Method::POST,
        "/api/datasets/ds1/services",
        &fx.owner,
        json!({ "name": "Public", "slug": slug }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "create service: {body}");
    assert_eq!(
        body["is_active"],
        json!(true),
        "a new service starts active"
    );
    body["id"].as_str().unwrap().to_string()
}

/// PUT the service as the owner; returns the updated service.
async fn put_service(fx: &Fixture, id: &str, body: Value) -> Value {
    let (status, updated) = send_json(
        &fx.app,
        Method::PUT,
        &format!("/api/datasets/ds1/services/{id}"),
        &fx.owner,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update service: {updated}");
    updated
}

#[tokio::test]
async fn deactivated_service_answers_like_a_missing_one_for_every_caller() {
    let fx = setup();
    let id = create_service(&fx, "pub").await;

    // Baseline: an active service on a public dataset answers anyone.
    let (status, body) = query_get(&fx.app, "pub", None, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "active service should answer: {body}"
    );
    assert!(
        body.contains(MARKER),
        "active service should return data: {body}"
    );

    let updated = put_service(&fx, &id, json!({ "name": "Public", "is_active": false })).await;
    assert_eq!(updated["is_active"], json!(false));

    // What a service that never existed looks like.
    let (missing_status, missing_body) = query_get(&fx.app, "no-such-service", None, None).await;
    assert_eq!(missing_status, StatusCode::NOT_FOUND);
    assert!(
        missing_body.contains("Service not found"),
        "missing service body: {missing_body}"
    );

    // Anonymous, the owner (a dataset writer), and a super admin all get the
    // missing-service answer — no data, and nothing that says the slug exists.
    for (who, token) in [
        ("anonymous", None),
        ("owner", Some(fx.owner.as_str())),
        ("super admin", Some(fx.admin.as_str())),
    ] {
        let (status, body) = query_get(&fx.app, "pub", token, None).await;
        assert_eq!(
            (status, body.as_str()),
            (missing_status, missing_body.as_str()),
            "{who}: an inactive service must answer like a missing one"
        );
        assert!(
            !body.contains(MARKER),
            "{who}: inactive service leaked data"
        );
    }

    // The POST forms of the protocol are refused the same way.
    let uri = "/api/datasets/ds1/services/pub/sparql";
    for (content_type, body) in [
        ("application/sparql-query", QUERY.to_string()),
        (
            "application/x-www-form-urlencoded",
            format!("query={}", url_encode(QUERY)),
        ),
    ] {
        let (status, text) = send(
            &fx.app,
            Method::POST,
            uri,
            Some(&fx.owner),
            Some(content_type),
            body,
        )
        .await;
        assert_eq!(
            (status, text.as_str()),
            (missing_status, missing_body.as_str()),
            "POST {content_type}: an inactive service must answer like a missing one"
        );
    }
}

#[tokio::test]
async fn inactive_service_refuses_pinned_version_reads() {
    let fx = setup();
    let id = create_service(&fx, "pub").await;

    // The version path resolves snapshot graphs instead of the service's live
    // graph list; it must not be a way around the flag.
    let (status, body) = query_get(&fx.app, "pub", None, Some(VERSION)).await;
    assert_eq!(status, StatusCode::OK, "version read should answer: {body}");
    assert!(
        body.contains(MARKER),
        "version read should return data: {body}"
    );

    put_service(&fx, &id, json!({ "name": "Public", "is_active": false })).await;

    for token in [None, Some(fx.owner.as_str())] {
        let (status, body) = query_get(&fx.app, "pub", token, Some(VERSION)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "pinned version: {body}");
        assert!(body.contains("Service not found"), "pinned version: {body}");
    }
}

#[tokio::test]
async fn reactivating_a_service_restores_it_and_a_rename_keeps_it_off() {
    let fx = setup();
    let id = create_service(&fx, "pub").await;
    put_service(&fx, &id, json!({ "name": "Public", "is_active": false })).await;

    // A PUT that omits `is_active` leaves the flag as it was.
    let renamed = put_service(&fx, &id, json!({ "name": "Renamed" })).await;
    assert_eq!(
        renamed["is_active"],
        json!(false),
        "rename must not reactivate"
    );
    let (status, _) = query_get(&fx.app, "pub", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    put_service(&fx, &id, json!({ "name": "Renamed", "is_active": true })).await;
    let (status, body) = query_get(&fx.app, "pub", None, None).await;
    assert_eq!(status, StatusCode::OK, "reactivated service: {body}");
    assert!(body.contains(MARKER), "reactivated service: {body}");
}
