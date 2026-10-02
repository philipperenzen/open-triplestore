//! SPARQL federation behind an allowlist (6.4). `SERVICE` used to error
//! unconditionally (oxigraph built without its HTTP client, as an SSRF
//! mitigation). It now reaches endpoints whose prefix is in
//! `OTS_REMOTE_ALLOWLIST`, with a timeout, a row cap and a byte cap (both a
//! failed invocation when exceeded, never a truncation), and nothing else.
//!
//! The "remote" is a second instance of this server on a local listener,
//! holding a public dataset; the local store federates to it. `SERVICE ?var`
//! (an endpoint named by the data), the per-query caps and deadline, and the
//! caller check on a variable naming `urn:source:*` run against the same
//! listeners, a second copy of the remote and a deliberately slow endpoint.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use open_triplestore::sources::virtual_source::{SourceCaller, SourceCallerGuard};
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use std::sync::OnceLock;
use tower::ServiceExt as _;

const G: &str = "urn:fed:graph";

/// `app` on a local listener of its own; its origin.
fn serve(app: axum::Router) -> String {
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
}

/// A server with a public dataset of three triples.
fn remote_server() -> String {
    let (state, _token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "fed",
            "Federated",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
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
    serve(test_app(state))
}

/// A second server on a local listener with a public dataset of three triples.
fn remote() -> String {
    static ADDR: OnceLock<String> = OnceLock::new();
    ADDR.get_or_init(remote_server).clone()
}

/// Another one, with the same data: a second endpoint.
fn remote_b() -> String {
    static ADDR: OnceLock<String> = OnceLock::new();
    ADDR.get_or_init(remote_server).clone()
}

/// An endpoint that answers an empty result after three seconds.
fn slow() -> String {
    static ADDR: OnceLock<String> = OnceLock::new();
    ADDR.get_or_init(|| {
        serve(axum::Router::new().route(
            "/sparql",
            axum::routing::post(|| async {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                (
                    [(header::CONTENT_TYPE, "application/sparql-results+json")],
                    r#"{"head":{"vars":[]},"results":{"bindings":[]}}"#,
                )
            }),
        ))
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

    // 4. A result over the row cap is a failed invocation, not a truncated
    //    answer: an error naming the knob, and Ω0 under SERVICE SILENT.
    std::env::set_var("OTS_REMOTE_ALLOWLIST", format!("{origin}/"));
    std::env::set_var("OTS_SERVICE_MAX_ROWS", "2");
    let err = count(&local, &q).expect_err("three rows exceed a cap of two");
    assert!(
        err.contains("OTS_SERVICE_MAX_ROWS"),
        "the error names the knob: {err}"
    );
    assert_eq!(
        count(&local, &silent).unwrap(),
        1,
        "SERVICE SILENT over the cap yields the single empty solution"
    );
    std::env::set_var("OTS_SERVICE_MAX_ROWS", "3");
    assert_eq!(count(&local, &q).unwrap(), 3, "a result at the cap passes");
    std::env::remove_var("OTS_SERVICE_MAX_ROWS");

    // 4b. The same for a response body over OTS_REMOTE_MAX_BYTES.
    std::env::set_var("OTS_REMOTE_MAX_BYTES", "64");
    let err = count(&local, &q).expect_err("the results document exceeds 64 bytes");
    assert!(
        err.contains("OTS_REMOTE_MAX_BYTES"),
        "the error names the knob: {err}"
    );
    assert_eq!(
        count(&local, &silent).unwrap(),
        1,
        "SERVICE SILENT over the byte cap yields the single empty solution"
    );
    std::env::remove_var("OTS_REMOTE_MAX_BYTES");
    assert_eq!(count(&local, &q).unwrap(), 3, "the default limit admits it");

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
        !describe(app.clone(), &token)
            .await
            .contains("BasicFederatedQuery"),
        "not advertised without an allowlist"
    );

    // 6. SERVICE ?var: an endpoint named by the data is called once it is
    //    bound by the pattern before the SERVICE (Federated Query §4).
    let origin_b = remote_b();
    let endpoint_b = format!("{origin_b}/sparql");
    let slow_endpoint = format!("{}/sparql", slow());
    std::env::set_var(
        "OTS_REMOTE_ALLOWLIST",
        format!("{origin}/,{origin_b}/,{slow_endpoint}"),
    );
    local
        .store
        .load_str(
            &format!(
                "<urn:cat:x> <urn:cat:endpoint> <{endpoint}> . <urn:cat:y> <urn:cat:endpoint> <{endpoint}> .\n\
                 <urn:cat:x> <urn:cat:mirror> <{endpoint}> . <urn:cat:y> <urn:cat:mirror> <{endpoint_b}> ."
            ),
            RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let by_var =
        "SELECT ?c ?s WHERE { ?c <urn:cat:endpoint> ?ep SERVICE ?ep { ?s <urn:fed:p> ?o } }";
    assert_eq!(
        count(&local, by_var).unwrap(),
        6,
        "three remote rows per catalogue row"
    );
    // The explicit form it is rewritten to.
    assert_eq!(
        count(
            &local,
            "SELECT ?c ?s WHERE { ?c <urn:cat:endpoint> ?ep LATERAL { SERVICE ?ep { ?s <urn:fed:p> ?o } } }"
        )
        .unwrap(),
        6,
        "LATERAL passes the bound endpoint to the SERVICE"
    );
    assert_eq!(
        count(
            &local,
            "SELECT ?c ?s WHERE { ?c <urn:cat:endpoint> ?ep OPTIONAL { SERVICE ?ep { ?s <urn:fed:p> \"one\" } } }"
        )
        .unwrap(),
        2,
        "OPTIONAL {{ SERVICE ?var }} keeps every catalogue row"
    );
    assert_eq!(
        count(
            &local,
            "SELECT ?c ?s WHERE { ?c <urn:cat:mirror> ?ep SERVICE ?ep { ?s <urn:fed:p> ?o } }"
        )
        .unwrap(),
        6,
        "two endpoints, one call each"
    );

    // 7. An unbound endpoint variable is a failed invocation: an error, and
    //    Ω0 under SILENT.
    let err = count(
        &local,
        "SELECT * WHERE { SERVICE ?nope { ?s <urn:fed:p> ?o } }",
    )
    .expect_err("nothing binds ?nope");
    assert!(err.contains("unbound"), "{err}");
    assert_eq!(
        count(
            &local,
            "SELECT * WHERE { SERVICE SILENT ?nope { ?s <urn:fed:p> ?o } }"
        )
        .unwrap(),
        1,
        "SERVICE SILENT ?unbound yields the single empty solution"
    );
    assert_eq!(
        count(
            &local,
            "SELECT ?s WHERE { ?s <urn:local:known> true SERVICE SILENT ?nope { ?x <urn:fed:p> ?o } }"
        )
        .unwrap(),
        1,
        "and the local row survives"
    );

    // 8. Per-query caps: an answer is fetched once per (endpoint, pattern) and
    //    reused, and a query may contact OTS_SERVICE_MAX_ENDPOINTS endpoints
    //    with OTS_SERVICE_MAX_CALLS requests. Over a cap is a failed invocation.
    std::env::set_var("OTS_SERVICE_MAX_CALLS", "1");
    assert_eq!(
        count(&local, by_var).unwrap(),
        6,
        "the second catalogue row reuses the first row's answer"
    );
    let two_patterns = format!(
        "SELECT * WHERE {{ SERVICE <{endpoint}> {{ ?s <urn:fed:p> ?o }} SERVICE <{endpoint}> {{ ?s <urn:fed:p> ?o2 }} }}"
    );
    let err = count(&local, &two_patterns).expect_err("two requests, a cap of one");
    assert!(
        err.contains("OTS_SERVICE_MAX_CALLS"),
        "the error names the knob: {err}"
    );
    std::env::remove_var("OTS_SERVICE_MAX_CALLS");
    assert_eq!(count(&local, &two_patterns).unwrap(), 3);
    std::env::set_var("OTS_SERVICE_MAX_ENDPOINTS", "1");
    let err = count(
        &local,
        "SELECT ?c ?s WHERE { ?c <urn:cat:mirror> ?ep SERVICE ?ep { ?s <urn:fed:p> ?o } }",
    )
    .expect_err("two endpoints, a cap of one");
    assert!(
        err.contains("OTS_SERVICE_MAX_ENDPOINTS"),
        "the error names the knob: {err}"
    );
    assert_eq!(
        count(
            &local,
            "SELECT ?c ?s WHERE { ?c <urn:cat:mirror> ?ep SERVICE SILENT ?ep { ?s <urn:fed:p> ?o } }",
        )
        .unwrap(),
        4,
        "under SILENT the endpoint over the cap is Ω0: three rows and one empty"
    );
    assert_eq!(
        count(&local, by_var).unwrap(),
        6,
        "one endpoint is within the cap"
    );
    std::env::remove_var("OTS_SERVICE_MAX_ENDPOINTS");

    // 9. The per-query deadline cuts a slow call short, and cancels the query:
    //    SERVICE SILENT does not turn it into Ω0.
    std::env::set_var("OTS_SERVICE_DEADLINE_SECS", "1");
    let started = std::time::Instant::now();
    let err = count(
        &local,
        &format!("SELECT * WHERE {{ SERVICE <{slow_endpoint}> {{ ?s ?p ?o }} }}"),
    )
    .expect_err("the endpoint answers after the deadline");
    assert!(
        err.contains("OTS_SERVICE_DEADLINE_SECS"),
        "the error names the knob: {err}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "cut short"
    );
    let err = count(
        &local,
        &format!(
            "SELECT ?s WHERE {{ ?s <urn:local:known> true SERVICE SILENT <{slow_endpoint}> {{ ?x ?p ?o }} }}"
        ),
    )
    .expect_err("past the deadline the query fails, SILENT or not");
    assert!(err.contains("OTS_SERVICE_DEADLINE_SECS"), "{err}");
    std::env::remove_var("OTS_SERVICE_DEADLINE_SECS");

    // 10. A variable naming `urn:source:<id>` is resolved for the caller like
    //     a written-out `SERVICE <urn:source:id>`: a caller who may not use the
    //     source's account finds it exactly as if it were not registered.
    std::env::set_var("OTS_REMOTE_ALLOWLIST", format!("{origin}/"));
    local
        .auth_db
        .create_dataset(
            "srcds",
            "Source-bound",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    let port = origin.rsplit(':').next().unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/sources")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"id":"fedsrc","dialect":"sparql","host":"127.0.0.1","port":{port},"database":"/sparql","dataset":"srcds"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{}",
        body_text(resp.into_body()).await
    );
    local
        .store
        .load_str(
            "<urn:cat:z> <urn:cat:source> <urn:source:fedsrc> . <urn:cat:w> <urn:cat:unknown> <urn:source:nosuch> .",
            RdfFormat::Turtle,
            None,
        )
        .unwrap();
    local
        .auth_db
        .create_user(
            "stranger",
            "stranger",
            "stranger@test.com",
            "hash",
            SystemRole::User,
        )
        .unwrap();
    let as_caller = |user: &str, is_admin: bool, q: &str| {
        let _caller = SourceCallerGuard::set(Some(std::sync::Arc::new(SourceCaller {
            user_id: Some(user.to_string()),
            is_admin,
            auth_db: local.auth_db.clone(),
        })));
        count(&local, q)
    };
    let via_source =
        "SELECT ?s WHERE { ?c <urn:cat:source> ?ep SERVICE ?ep { ?s <urn:fed:p> ?o } }";
    let via_nothing =
        "SELECT ?s WHERE { ?c <urn:cat:unknown> ?ep SERVICE ?ep { ?s <urn:fed:p> ?o } }";
    assert_eq!(
        as_caller("adm", true, via_source).unwrap(),
        3,
        "the administrator may use the source"
    );
    let refused = as_caller("stranger", false, via_source).expect_err("not the stranger's to use");
    let unknown = as_caller("stranger", false, via_nothing).expect_err("no such source");
    assert_eq!(
        refused.replace("fedsrc", "<id>"),
        unknown.replace("nosuch", "<id>"),
        "a refused source looks like an unregistered one"
    );
    assert!(
        count(&local, via_source).is_err(),
        "evaluation without a caller resolves no source"
    );
}
