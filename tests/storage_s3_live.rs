//! The S3 asset store against a real S3-compatible server (Versity S3 Gateway
//! in CI).
//!
//! S3 is the documented store for assets on a replicated deployment, yet the
//! only storage tests were the local backend's path-safety checks: nothing ever
//! sent a byte to an S3 endpoint. These tests do, through `ObjectStore` and
//! through the asset routes: bucket creation, a binary round trip with its
//! content type, overwrite, delete, an absent key reading as absent (not as a
//! fault), and wrong credentials failing loudly.
//!
//! They skip at run time unless `OTS_TEST_S3_ENDPOINT` is set. The CI job that
//! starts the S3 server also sets `OTS_TEST_LIVE_REQUIRED`, so a missing or renamed
//! variable fails there instead of skipping. The variables:
//!
//! - `OTS_TEST_S3_ENDPOINT` — e.g. `http://127.0.0.1:7070`
//! - `OTS_TEST_S3_ACCESS_KEY`, `OTS_TEST_S3_SECRET_KEY` — credentials allowed
//!   to create buckets
//! - `OTS_TEST_S3_REGION` — default `us-east-1`
//!
//! Every test works in a bucket of its own named `ots-test-…`; the objects are
//! deleted again, the (empty) buckets are left behind.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use bytes::Bytes;
use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::storage::{AssetMissing, ObjectStore};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tower::ServiceExt as _;

struct Target {
    endpoint: String,
    access_key: String,
    secret_key: String,
    region: String,
}

fn target() -> Option<Target> {
    let Ok(endpoint) = std::env::var("OTS_TEST_S3_ENDPOINT") else {
        assert!(
            std::env::var_os("OTS_TEST_LIVE_REQUIRED").is_none(),
            "OTS_TEST_LIVE_REQUIRED is set but OTS_TEST_S3_ENDPOINT is not"
        );
        eprintln!("skipped: set OTS_TEST_S3_ENDPOINT to run the S3 storage tests");
        return None;
    };
    let var = |name: &str| {
        std::env::var(name)
            .unwrap_or_else(|_| panic!("{name} must be set with OTS_TEST_S3_ENDPOINT"))
    };
    Some(Target {
        endpoint,
        access_key: var("OTS_TEST_S3_ACCESS_KEY"),
        secret_key: var("OTS_TEST_S3_SECRET_KEY"),
        region: std::env::var("OTS_TEST_S3_REGION").unwrap_or_else(|_| "us-east-1".into()),
    })
}

/// A bucket name no other test (or earlier run) uses.
fn fresh_bucket(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("ots-test-{tag}-{}-{nanos}", std::process::id())
}

/// Connect, creating the bucket. The server may still be starting when a CI
/// job reaches this test, so a failure is retried for up to a minute.
async fn connect(t: &Target, bucket: &str) -> ObjectStore {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match ObjectStore::new(&t.endpoint, bucket, &t.access_key, &t.secret_key, &t.region).await {
            Ok(store) => return store,
            Err(e) if Instant::now() < deadline => {
                eprintln!("waiting for the S3 endpoint: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(e) => panic!("S3 endpoint {}: {e}", t.endpoint),
        }
    }
}

/// Every byte value, repeated past a megabyte: binary-clean, and big enough
/// that the body is not one small frame.
fn binary_body() -> Bytes {
    let pattern: Vec<u8> = (0..=255u8).collect();
    Bytes::from(pattern.repeat(4 * 1024 + 7))
}

/// Every test runs on this one runtime. The store's HTTPS client is a process
/// global with a connection pool, and a pooled connection is driven by a task
/// on the runtime that opened it: with a runtime per test (`#[tokio::test]`),
/// a test that reuses a connection opened by another test fails with
/// "dispatch failure" as soon as that test's runtime shuts down.
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("test runtime")
        })
        .block_on(fut)
}

fn is_missing(e: &anyhow::Error) -> bool {
    e.downcast_ref::<AssetMissing>().is_some()
}

#[test]
fn objects_round_trip_through_s3() {
    block_on(async {
        let Some(t) = target() else { return };
        let store = connect(&t, &fresh_bucket("rt")).await;
        assert!(store.is_configured());

        let key = "datasets/ds-1/0f6e/plan with spaces.ifc";
        let body = binary_body();
        store
            .upload(key, body.clone(), "application/x-step")
            .await
            .expect("upload");

        let (got, content_type) = store.download(key).await.expect("download");
        assert_eq!(
            got.len(),
            body.len(),
            "the object came back a different size"
        );
        assert!(got == body, "the object came back with different bytes");
        assert_eq!(content_type, "application/x-step");

        // An upload to the same key replaces the object and its content type.
        store
            .upload(key, Bytes::from_static(b"second version"), "text/plain")
            .await
            .expect("overwrite");
        let (got, content_type) = store.download(key).await.expect("download after overwrite");
        assert_eq!(&got[..], b"second version");
        assert_eq!(content_type, "text/plain");

        store.delete(key).await.expect("delete");
        let err = store
            .download(key)
            .await
            .expect_err("a deleted object must not download");
        assert!(
            is_missing(&err),
            "a deleted object reads as absent, not as a fault: {err:#}"
        );
        // Deleting what is already gone is not an error (S3 semantics).
        store.delete(key).await.expect("second delete");
    })
}

#[test]
fn an_absent_key_is_missing_not_a_fault() {
    block_on(async {
        let Some(t) = target() else { return };
        let store = connect(&t, &fresh_bucket("absent")).await;

        let err = store
            .download("datasets/nothing/here.bin")
            .await
            .expect_err("nothing was uploaded");
        assert!(
            is_missing(&err),
            "an absent key must surface as AssetMissing (the routes turn it into a 404): {err:#}"
        );
    })
}

#[test]
fn an_existing_bucket_is_reused() {
    block_on(async {
        let Some(t) = target() else { return };
        let bucket = fresh_bucket("reuse");
        let first = connect(&t, &bucket).await;
        first
            .upload("kept.txt", Bytes::from_static(b"still here"), "text/plain")
            .await
            .expect("upload");

        // A second store on the same bucket (a restart) finds it and its objects.
        let second = connect(&t, &bucket).await;
        let (got, _) = second.download("kept.txt").await.expect("download");
        assert_eq!(&got[..], b"still here");
        second.delete("kept.txt").await.expect("delete");
    })
}

#[test]
fn wrong_credentials_fail_loudly() {
    block_on(async {
        let Some(t) = target() else { return };
        // Make sure the endpoint is up, so the failure below is the credentials.
        let bucket = fresh_bucket("creds");
        connect(&t, &bucket).await;

        let unknown = fresh_bucket("creds-new");
        for name in [bucket.as_str(), unknown.as_str()] {
            let result = ObjectStore::new(
                &t.endpoint,
                name,
                &t.access_key,
                "not-the-secret-key",
                &t.region,
            )
            .await;
            assert!(
                result.is_err(),
                "a store with a wrong secret key must refuse to start (bucket {name}), not \
             come up and fail every upload later"
            );
        }
    })
}

/// The same round trip through the HTTP asset routes: multipart upload,
/// download with the stored content type, delete, and a `404` afterwards.
#[test]
fn assets_round_trip_over_http_through_s3() {
    block_on(async {
        let Some(t) = target() else { return };
        let store = connect(&t, &fresh_bucket("http")).await;

        let (mut state, token) = admin_state();
        state.object_store = Arc::new(store.clone());
        let ds = state
            .auth_db
            .create_dataset(
                "s3-files",
                "S3 files",
                None,
                OwnerType::User,
                "adm",
                Visibility::Private,
                None,
            )
            .unwrap();
        let app = test_app(state);

        let body = binary_body();
        let boundary = "OtsS3Boundary";
        let mut multipart = Vec::new();
        multipart.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"probe.bin\"\r\nContent-Type: application/x-ots-probe\r\n\r\n"
            )
            .as_bytes(),
        );
        multipart.extend_from_slice(&body);
        multipart.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/datasets/{}/assets", ds.id))
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(multipart))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let created = body_json(resp.into_body()).await;
        assert_eq!(status, StatusCode::CREATED, "upload: {created}");
        let asset_id = created["id"].as_str().expect("asset id").to_string();
        let s3_key = created["s3_key"].as_str().expect("s3 key").to_string();

        // The bytes are in the bucket under the key the asset row names.
        let (in_bucket, _) = store.download(&s3_key).await.expect("object in the bucket");
        assert!(in_bucket == body, "the bucket holds different bytes");

        let get = |app: axum::Router| {
            let uri = format!("/api/datasets/{}/assets/{asset_id}", ds.id);
            let token = token.clone();
            async move {
                app.oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(uri)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
            }
        };

        let resp = get(app.clone()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()[header::CONTENT_TYPE],
            "application/x-ots-probe"
        );
        let downloaded = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        assert!(downloaded == body, "the download differs from the upload");

        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/datasets/{}/assets/{asset_id}", ds.id))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "delete answered {}",
            resp.status()
        );
        let err = store
            .download(&s3_key)
            .await
            .expect_err("deleting the asset deletes its object");
        assert!(is_missing(&err), "{err:#}");

        let resp = get(app).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    })
}
