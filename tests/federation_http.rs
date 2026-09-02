//! SPARQL federation behind an allowlist (6.4). `SERVICE` used to error
//! unconditionally (oxigraph built without its HTTP client, as an SSRF
//! mitigation). It now reaches endpoints whose prefix is in
//! `OTS_REMOTE_ALLOWLIST`, with a timeout and a row cap, and nothing else.
//!
//! The "remote" is a second instance of this server on a local listener,
//! holding a public dataset; the local store federates to it.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use std::sync::OnceLock;
use tower::ServiceExt as _;

const G: &str = "urn:fed:graph";

/// A second server on a local listener with a public dataset of three triples.
fn remote() -> String {
    static ADDR: OnceLock<String> = OnceLock::new();
    ADDR.get_or_init(|| {
        let (state, _token) = admin_state();
        state
            .auth_db
            .create_dataset("fed", "Federated", None, OwnerType::User, "adm", Visibility::Public, None)
            .unwrap();
        state.auth_db.add_dataset_graph("fed", G).unwrap();
        state
            .store
            .load_str(
                "<urn:fed:a> <urn:fed:p> \"one\" . <urn:fed:b> <urn:fed:p> \"two\" . <urn:fed:c> <urn:fed:p> \"three\" .",
                RdfFormat::Turtle,
                Some(G),
            )
            .unwrap();
        let app = test_app(state);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        let addr = rx.recv().unwrap();
        format!("http://{addr}")
    })
    .clone()
}

/// Rows of `q`, or the first error — a SERVICE failure surfaces as an `Err`
/// item inside the solution iterator, so counting items would hide it.
fn count(state: &AppState, q: &str) -> Result<usize, String> {
    match state.store.query(q) {
        Ok(QueryResults::Solutions(s)) => {
            let mut n = 0;
            for row in s {
                row.map_err(|e| e.to_string())?;
                n += 1;
            }
            Ok(n)
        }
        Ok(_) => Ok(0),
        Err(e) => Err(e.to_string()),
    }
}

/// One test, sequential: the allowlist is a process-wide environment variable.
#[tokio::test]
async fn service_is_allowlisted_timed_and_capped() {
    let origin = remote();
    let endpoint = format!("{origin}/sparql");
    let (local, token) = admin_state();
    let q = format!("SELECT ?s WHERE {{ SERVICE <{endpoint}> {{ ?s <urn:fed:p> ?o }} }}");

    // 1. No allowlist: SERVICE errors, and SERVICE SILENT yields nothing.
    std::env::set_var("OTS_REMOTE_ALLOWLIST", "");
    let err = count(&local, &q).expect_err("federation is off without an allowlist");
    assert!(
        err.contains("OTS_REMOTE_ALLOWLIST") || err.to_lowercase().contains("not allowed"),
        "the error names the knob: {err}"
    );
    let silent =
        format!("SELECT ?s WHERE {{ SERVICE SILENT <{endpoint}> {{ ?s <urn:fed:p> ?o }} }}");
    // SPARQL 1.1: a failed SERVICE SILENT yields a single solution with no bindings.
    assert!(
        count(&local, &silent).unwrap() <= 1,
        "SERVICE SILENT swallows the refusal"
    );

    // 2. Allowlisted: the remote rows come back and join locally.
    std::env::set_var("OTS_REMOTE_ALLOWLIST", format!("{origin}/"));
    assert_eq!(count(&local, &q).unwrap(), 3, "three remote rows");
    local
        .store
        .load_str(
            "<urn:fed:a> <urn:local:known> true .",
            RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let joined = format!(
        "SELECT ?s WHERE {{ ?s <urn:local:known> true . SERVICE <{endpoint}> {{ ?s <urn:fed:p> ?o }} }}"
    );
    assert_eq!(
        count(&local, &joined).unwrap(),
        1,
        "local bindings join with the remote result"
    );

    // 3. A prefix that does not match is still refused.
    std::env::set_var("OTS_REMOTE_ALLOWLIST", "https://sparql.example.org/");
    assert!(
        count(&local, &q).is_err(),
        "an endpoint outside the allowlist is refused"
    );

    // 4. The row cap truncates.
    std::env::set_var("OTS_REMOTE_ALLOWLIST", format!("{origin}/"));
    std::env::set_var("OTS_SERVICE_MAX_ROWS", "2");
    assert_eq!(
        count(&local, &q).unwrap(),
        2,
        "capped at OTS_SERVICE_MAX_ROWS"
    );
    std::env::remove_var("OTS_SERVICE_MAX_ROWS");

    // 5. The service description advertises federation only with an allowlist.
    async fn describe(app: axum::Router, token: &str) -> String {
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .header(header::ACCEPT, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        body_text(resp.into_body()).await
    }
    let app = test_app(local.clone());
    assert!(
        describe(app.clone(), &token)
            .await
            .contains("BasicFederatedQuery"),
        "advertised while allowlisted"
    );
    std::env::set_var("OTS_REMOTE_ALLOWLIST", "");
    assert!(
        !describe(app, &token).await.contains("BasicFederatedQuery"),
        "not advertised without an allowlist"
    );
}
