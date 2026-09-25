//! Two more ways into a model-registry graph are closed: a bulk import and an
//! LDES sync may not name one as their target — admins included — because
//! models change through the data-model API, which keeps their licence
//! records true and refuses altered copies of content whose licence allows
//! none.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
use serde_json::json;
use tower::ServiceExt as _;

fn multipart(boundary: &str, parts: &[(&str, &str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, content_type, filename, bytes) in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match filename {
            Some(f) => out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\n")
                    .as_bytes(),
            ),
            None => out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n").as_bytes(),
            ),
        }
        out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

#[tokio::test]
async fn a_bulk_import_may_not_write_a_model_registry_graph() {
    let (state, token) = admin_state();
    let target = format!("{}/data-model/some-model/version/1.0.0", state.base_url);
    let meta = json!({ "default_target_graph": target, "replace": true }).to_string();
    let boundary = "REGISTRYGUARD";
    let body = multipart(
        boundary,
        &[
            ("meta", "application/json", None, meta.as_bytes()),
            (
                "file",
                "text/turtle",
                Some("a.ttl"),
                b"<http://ex.org/s> <http://ex.org/p> <http://ex.org/o> .",
            ),
        ],
    );
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/import/bulk")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    assert!(
        !status.is_success(),
        "an admin bulk import into a registry graph is refused: {status} {text}"
    );
    assert!(text.contains("model registry"), "{text}");
    assert_eq!(
        state.store.graph_count_cached(Some(&target)).unwrap_or(0),
        0,
        "nothing was written"
    );
}

#[tokio::test]
async fn an_ldes_sync_may_not_target_a_model_registry_graph() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "mirror",
            "Mirror",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    let target = format!("{}/data-model/some-model/version/1.0.0", state.base_url);
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/ldes/sync")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({
                        "url": "https://example.org/stream",
                        "dataset_id": "mirror",
                        "graph_iri": target,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(
        !state
            .auth_db
            .list_dataset_graphs("mirror")
            .unwrap()
            .contains(&target),
        "the registry graph was not registered to the dataset"
    );
}
