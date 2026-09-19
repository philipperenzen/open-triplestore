//! An asset whose bytes this node does not hold answers `404`, not `500`.
//!
//! A follower replicates the store and the identity database but not the
//! object store, so its file libraries list the leader's files — the metadata
//! travels with the identity database — while the bytes stay on the leader.
//! Asking for one produced `500 Failed to read asset …: No such file or
//! directory`, which reads as "this node is broken" rather than "those bytes
//! are not here", and told the caller nothing about where they are.
//!
//! It is a `404` now, and on a follower it names the leader. A store that is
//! genuinely misconfigured or unreachable is a different thing and stays a
//! `500`: the two must not be conflated, or a broken S3 endpoint would look
//! like an empty one.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::storage::ObjectStore;
use open_triplestore::store::replication::{Mode, ReplicationConfig, Scope};
use open_triplestore::store::TripleStore;
use std::sync::Arc;
use tower::ServiceExt as _;

const LEADER: &str = "http://leader.internal:7878";

struct Fixture {
    app: axum::Router,
    token: String,
    dataset_id: String,
    asset_id: String,
}

/// A dataset with one asset row whose bytes were never written to `store`.
fn fixture(store: ObjectStore, follower: bool) -> Fixture {
    let triples = if follower {
        TripleStore::in_memory()
            .unwrap()
            .with_replication(ReplicationConfig::follower(LEADER, Mode::Hot, Scope::All))
    } else {
        TripleStore::in_memory().unwrap()
    };
    let (mut state, admin) = admin_state_with_store(triples);
    state.object_store = Arc::new(store);
    let ds = state
        .auth_db
        .create_dataset(
            "files",
            "Files",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    let asset = state
        .auth_db
        .create_asset(
            "asset-1",
            &ds.id,
            "plan.ifc",
            "application/octet-stream",
            "assets/asset-1",
            1024,
            "adm",
            false,
            "",
        )
        .unwrap();
    Fixture {
        app: test_app(state),
        token: admin,
        dataset_id: ds.id,
        asset_id: asset.id,
    }
}

async fn download(f: &Fixture) -> (StatusCode, String) {
    let uri = format!("/api/datasets/{}/assets/{}", f.dataset_id, f.asset_id);
    let resp = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {}", f.token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

/// The bug: bytes absent from a local store read as a server fault.
#[tokio::test]
async fn an_asset_whose_bytes_are_absent_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let f = fixture(ObjectStore::local(dir.path().to_path_buf()).unwrap(), false);

    let (status, body) = download(&f).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "body was: {body}");
    assert!(
        !body.contains("No such file") && !body.to_lowercase().contains("os error"),
        "the message should describe the asset, not the filesystem: {body}"
    );
}

/// And on a follower it says where the bytes actually live.
#[tokio::test]
async fn a_follower_names_the_leader_it_replicates() {
    let dir = tempfile::tempdir().unwrap();
    let f = fixture(ObjectStore::local(dir.path().to_path_buf()).unwrap(), true);

    let (status, body) = download(&f).await;

    assert_eq!(status, StatusCode::NOT_FOUND, "body was: {body}");
    assert!(
        body.contains(LEADER),
        "a follower's 404 should point at the leader that holds the bytes: {body}"
    );
}

/// A store that is not configured at all is a fault, not an absence: it must
/// stay a 500, or a broken endpoint would be indistinguishable from an empty
/// one and nobody would go looking for the real problem.
#[tokio::test]
async fn an_unconfigured_object_store_is_still_a_server_error() {
    let f = fixture(ObjectStore::noop(), false);

    let (status, body) = download(&f).await;

    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "body was: {body}"
    );
}
