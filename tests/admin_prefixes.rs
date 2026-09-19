//! An administrator can repoint a prefix, and no two prefixes share a label.
//!
//! The registry already had a tier above the bundled prefix.cc/LOV snapshot and
//! already refused a second mapping for a label it knew — but only for the
//! lifetime of the process, and only for seeds read out of an installed bundle.
//! There was no way to change a mapping on a running store, and nothing
//! survived a restart.
//!
//! What a deployment needs is the opposite of the community list: `geo` means
//! *their* `geo`. So an override is stored in the identity database (which also
//! means it replicates to followers with the rest of it), it outranks every
//! other tier, and the label is its primary key — which is what makes "no two
//! of the same shorthand" a property of the storage rather than a check
//! somebody has to remember to run.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::SystemRole;
use serde_json::Value;
use tower::ServiceExt as _;

/// A label the bundled snapshot already knows, pointed somewhere else.
const LABEL: &str = "geo";
const OURS: &str = "https://data.example.org/geo/def/";
const ALSO_OURS: &str = "https://data.example.org/geo/v2/";

async fn call(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if body.is_some() {
        b = b.header(header::CONTENT_TYPE, "application/json");
    }
    let req = b
        .body(
            body.map(|s| Body::from(s.to_string()))
                .unwrap_or(Body::empty()),
        )
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

fn put_body(ns: &str) -> String {
    format!(r#"{{"namespace":"{ns}"}}"#)
}

/// Only an administrator may change what a prefix means for everyone.
#[tokio::test]
async fn changing_a_prefix_needs_an_admin() {
    let (state, _admin) = admin_state();
    state
        .auth_db
        .create_user("u", "user", "u@test.com", "hash", SystemRole::User)
        .unwrap();
    let plain = mint_token("u", "user", "user");
    let app = test_app(state);

    let (anon, _) = call(
        &app,
        Method::PUT,
        &format!("/api/admin/prefixes/{LABEL}"),
        None,
        Some(&put_body(OURS)),
    )
    .await;
    assert_eq!(anon, StatusCode::UNAUTHORIZED);

    let (user, _) = call(
        &app,
        Method::PUT,
        &format!("/api/admin/prefixes/{LABEL}"),
        Some(&plain),
        Some(&put_body(OURS)),
    )
    .await;
    assert_eq!(user, StatusCode::FORBIDDEN);
}

/// The point of the feature: a deployment's own mapping wins.
#[tokio::test]
async fn an_override_outranks_the_bundled_snapshot() {
    let (state, admin) = admin_state_over(test_state_with_bundled_prefixes());
    let app = test_app(state);

    // What the community list says before we touch it.
    let (status, before) = call(
        &app,
        Method::GET,
        &format!("/api/prefixes/{LABEL}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let bundled: Value = serde_json::from_str(&before).unwrap();
    let bundled_ns = bundled["namespace"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(bundled_ns, OURS, "the fixture assumes the snapshot differs");

    let (status, body) = call(
        &app,
        Method::PUT,
        &format!("/api/admin/prefixes/{LABEL}"),
        Some(&admin),
        Some(&put_body(OURS)),
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::CREATED,
        "{status} {body}"
    );

    let (_, after) = call(
        &app,
        Method::GET,
        &format!("/api/prefixes/{LABEL}"),
        None,
        None,
    )
    .await;
    let resolved: Value = serde_json::from_str(&after).unwrap();
    assert_eq!(resolved["namespace"].as_str().unwrap(), OURS);

    // And a CURIE expands through it, which is what actually matters.
    let (_, expanded) = call(
        &app,
        Method::GET,
        &format!("/api/prefixes/expand?curie={LABEL}:Thing"),
        None,
        None,
    )
    .await;
    assert!(
        expanded.contains(&format!("{OURS}Thing")),
        "expand did not follow the override: {expanded}"
    );
}

/// "Do not allow two of the same prefix shorthand."
#[tokio::test]
async fn a_label_cannot_be_claimed_twice() {
    let (state, admin) = admin_state_over(test_state_with_bundled_prefixes());
    let app = test_app(state);

    let (first, first_body) = call(
        &app,
        Method::POST,
        "/api/admin/prefixes",
        Some(&admin),
        Some(&format!(r#"{{"label":"{LABEL}","namespace":"{OURS}"}}"#)),
    )
    .await;
    assert_eq!(first, StatusCode::CREATED, "{first_body}");

    let (second, body) = call(
        &app,
        Method::POST,
        "/api/admin/prefixes",
        Some(&admin),
        Some(&format!(
            r#"{{"label":"{LABEL}","namespace":"{ALSO_OURS}"}}"#
        )),
    )
    .await;
    assert_eq!(second, StatusCode::CONFLICT, "{body}");
    assert!(
        body.contains(OURS),
        "the refusal should say what the label already means: {body}"
    );

    // The first mapping is untouched.
    let (_, after) = call(
        &app,
        Method::GET,
        &format!("/api/prefixes/{LABEL}"),
        None,
        None,
    )
    .await;
    assert!(after.contains(OURS), "{after}");
}

/// An update is explicit, so repointing is possible without being accidental.
#[tokio::test]
async fn an_admin_can_repoint_and_remove_an_override() {
    let (state, admin) = admin_state_over(test_state_with_bundled_prefixes());
    let app = test_app(state);
    let uri = format!("/api/admin/prefixes/{LABEL}");

    call(&app, Method::PUT, &uri, Some(&admin), Some(&put_body(OURS))).await;
    let (status, _) = call(
        &app,
        Method::PUT,
        &uri,
        Some(&admin),
        Some(&put_body(ALSO_OURS)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, after) = call(
        &app,
        Method::GET,
        &format!("/api/prefixes/{LABEL}"),
        None,
        None,
    )
    .await;
    assert!(after.contains(ALSO_OURS), "{after}");

    let (status, _) = call(&app, Method::DELETE, &uri, Some(&admin), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // …and the bundled mapping is back, rather than the label disappearing.
    let (status, restored) = call(
        &app,
        Method::GET,
        &format!("/api/prefixes/{LABEL}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert!(
        !restored.contains(ALSO_OURS),
        "the override outlived its deletion: {restored}"
    );
}

/// An override is configuration, so it survives a restart — and, being in the
/// identity database, it reaches a follower with the rest of it.
#[tokio::test]
async fn an_override_survives_a_restart() {
    let (state, admin) = admin_state_over(test_state_with_bundled_prefixes());
    let auth_db = state.auth_db.clone();
    let app = test_app(state);
    call(
        &app,
        Method::PUT,
        &format!("/api/admin/prefixes/{LABEL}"),
        Some(&admin),
        Some(&put_body(OURS)),
    )
    .await;

    // A fresh process over the same identity database.
    let rebooted = test_app(test_state_with_auth_db(auth_db));

    let (status, body) = call(
        &rebooted,
        Method::GET,
        &format!("/api/prefixes/{LABEL}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains(OURS),
        "the override did not come back after a restart: {body}"
    );
}

/// Labels and namespaces are validated before anything is stored.
#[tokio::test]
async fn a_bad_label_or_namespace_is_refused() {
    let (state, admin) = admin_state();
    let app = test_app(state);

    for (label, ns) in [
        ("not a label", OURS),
        ("9lead", OURS),
        ("ok", "javascript:alert(1)"),
        ("ok", "file:///etc/passwd"),
        ("ok", "not-a-url"),
    ] {
        let (status, body) = call(
            &app,
            Method::POST,
            "/api/admin/prefixes",
            Some(&admin),
            Some(&format!(r#"{{"label":"{label}","namespace":"{ns}"}}"#)),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "{label:?} → {ns:?} was accepted: {body}"
        );
    }
}
