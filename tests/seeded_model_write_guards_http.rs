//! Direct writes (SPARQL Update, `/sparql/batch`, the Graph Store Protocol)
//! into the graphs of registered model versions, through the real router:
//!
//! * a version whose licence allows no altered copies (the seeded IMBOR) is
//!   never written directly, not even by an admin (403);
//! * a write into any other seeded version is allowed, and its licence record
//!   stops calling the copy unchanged; the next start keeps the edit;
//! * an admin's write that names no graph (`GRAPH ?g`) is followed by a
//!   re-check, so an altered IMBOR copy is withheld at once, and the next
//!   start keeps the altered copy aside (private) and restores CROW's file;
//! * a model a user created under the id `imbor` stays theirs to edit.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::data_models::{content_digest, registry, seed_vocab};
use tower::ServiceExt as _;

async fn send(
    state: &open_triplestore::server::AppState,
    req: Request<Body>,
) -> (StatusCode, String) {
    let resp = test_app(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

fn sparql_update(token: &str, update: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/sparql")
        .header(header::CONTENT_TYPE, "application/sparql-update")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(update.to_string()))
        .unwrap()
}

fn gsp(method: Method, token: &str, graph: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(format!("/store?graph={}", url_encode(graph)))
        .header(header::CONTENT_TYPE, "text/turtle")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn get(uri: &str, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b.body(Body::empty()).unwrap()
}

fn digest(state: &open_triplestore::server::AppState, graph: &str) -> String {
    content_digest::triples_digest(&content_digest::graph_triples(&state.store, graph).unwrap())
}

fn attribution(
    state: &open_triplestore::server::AppState,
    id: &str,
    ver: &str,
) -> open_triplestore::data_models::models::ContentAttribution {
    registry::get_attribution(
        &state.store,
        &registry::version_record_iri(&state.base_url, id, ver),
    )
    .unwrap()
}

const IMBOR_TERM: &str = "https://data.crow.nl/imbor/term/74e825e1-9b93-4dd5-8fab-52783fdb758b";

/// No direct write reaches IMBOR: SPARQL Update (named graph, `WITH`,
/// `DROP`), `/sparql/batch` and Graph Store PUT/POST/DELETE all answer 403,
/// for an admin too, and the graph and its record stay as they were.
#[tokio::test]
async fn direct_writes_into_imbor_are_refused_for_everyone() {
    let (state, token) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);
    let base = state.base_url.to_string();
    let graph = registry::version_record_iri(&base, "imbor", "2025");
    let before = digest(&state, &graph);
    let triple =
        format!("<{IMBOR_TERM}> <http://www.w3.org/2004/02/skos/core#prefLabel> \"edited\"");

    for update in [
        format!("INSERT DATA {{ GRAPH <{graph}> {{ {triple} }} }}"),
        format!("WITH <{graph}> DELETE {{ ?s ?p ?o }} WHERE {{ ?s ?p ?o }}"),
        format!("DROP GRAPH <{graph}>"),
        format!("INSERT {{ GRAPH <{graph}/extra> {{ {triple} }} }} WHERE {{}}"),
    ] {
        let (status, body) = send(&state, sparql_update(&token, &update)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{update}: {body}");
        assert!(body.contains("allows no altered copies"), "{body}");
    }

    let batch = serde_json::json!({ "updates": [
        "INSERT DATA { GRAPH <http://example.org/mine> { <http://ex.org/a> <http://ex.org/b> \"c\" } }",
        format!("INSERT DATA {{ GRAPH <{graph}> {{ {triple} }} }}"),
    ]});
    let (status, body) = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/sparql/batch")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(batch.to_string()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    for method in [Method::PUT, Method::POST, Method::DELETE] {
        let (status, body) = send(
            &state,
            gsp(method.clone(), &token, &graph, &format!("{triple} .")),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}: {body}");
        assert!(body.contains("allows no altered copies"), "{body}");
    }

    assert_eq!(digest(&state, &graph), before, "the graph is untouched");
    let a = attribution(&state, "imbor", "2025");
    assert!(a.unchanged && a.no_derivatives);
    assert_eq!(
        send(
            &state,
            get("/api/models/imbor/versions/2025/data?format=nt", None)
        )
        .await
        .0,
        StatusCode::OK
    );
}

/// An admin's SPARQL edit of a seeded copy whose licence allows other copies
/// (BOT) is allowed. Its record stops calling it unchanged before the write
/// runs, downloads say it may have been modified, and the next start keeps
/// both the edit and the label. A Graph Store PUT does the same.
#[tokio::test]
async fn a_direct_write_into_a_seeded_copy_is_kept_and_labelled() {
    let (state, token) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);
    let base = state.base_url.to_string();
    let bot = registry::version_record_iri(&base, "bot", "0.3.2");
    let edit = "<https://w3id.org/bot#Building> <http://www.w3.org/2004/02/skos/core#altLabel> \"gebouw\"@nl";

    let (status, body) = send(
        &state,
        sparql_update(
            &token,
            &format!("INSERT DATA {{ GRAPH <{bot}> {{ {edit} }} }}"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let a = attribution(&state, "bot", "0.3.2");
    assert!(!a.unchanged);
    assert!(
        a.stored_copy.contains("written directly"),
        "{}",
        a.stored_copy
    );

    let (status, body) = send(
        &state,
        get("/api/models/bot/versions/0.3.2/data?format=turtle", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("gebouw"));
    assert!(body.contains("may have been modified"));
    assert!(!body
        .lines()
        .any(|l| l.starts_with("# ---- Served") && l.contains("unchanged")));

    // The next start keeps the edit and the label.
    seed_vocab::seed_standard_vocabularies(&state);
    let ask = format!("ASK {{ GRAPH <{bot}> {{ {edit} }} }}");
    assert!(matches!(
        state.store.query(&ask),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    ));
    assert!(!attribution(&state, "bot", "0.3.2").unchanged);

    // A Graph Store PUT into RDF 1.1 is allowed and labelled.
    let rdf = registry::version_record_iri(&base, "rdf", "1.1");
    let (status, body) = send(
        &state,
        gsp(
            Method::PUT,
            &token,
            &rdf,
            "<http://ex.org/a> <http://ex.org/b> \"c\" .",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(!attribution(&state, "rdf", "1.1").unchanged);
    // Other seeded copies keep their label.
    assert!(attribution(&state, "rdf", "1.0").unchanged);
}

/// An admin update that names no graph (`GRAPH ?g`) cannot be refused up
/// front; it is followed by a re-check of every checked copy. An altered
/// IMBOR copy is then withheld from everyone who may not write the entry
/// straight away. The next start keeps the altered copy aside, as a private,
/// withheld version only writers can read, and restores CROW's file.
#[tokio::test]
async fn an_unscoped_admin_write_into_imbor_is_caught_and_repaired() {
    let (state, token) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);
    let base = state.base_url.to_string();
    let graph = registry::version_record_iri(&base, "imbor", "2025");
    let before = digest(&state, &graph);

    let (status, body) = send(
        &state,
        sparql_update(
            &token,
            &format!(
                "INSERT {{ GRAPH ?g {{ <{IMBOR_TERM}> <http://www.w3.org/2000/01/rdf-schema#comment> \"edited\" }} }} \
                 WHERE {{ GRAPH ?g {{ <{IMBOR_TERM}> a ?t }} }}"
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_ne!(digest(&state, &graph), before);
    assert!(!attribution(&state, "imbor", "2025").unchanged);
    let uri = "/api/models/imbor/versions/2025/data?format=nt";
    assert_eq!(send(&state, get(uri, None)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&state, get(uri, Some(&token))).await.0, StatusCode::OK);
    // Other copies are unaffected.
    assert!(attribution(&state, "rdf", "1.1").unchanged);

    // The next start.
    seed_vocab::seed_standard_vocabularies(&state);
    assert_eq!(digest(&state, &graph), before, "CROW's file again");
    assert!(attribution(&state, "imbor", "2025").unchanged);
    assert_eq!(send(&state, get(uri, None)).await.0, StatusCode::OK);

    let kept = registry::get_version(&state.store, &base, "imbor", "2025-kept-1")
        .expect("the altered copy is kept");
    assert!(kept.created_by.is_none());
    assert_eq!(
        kept.status,
        open_triplestore::data_models::models::VersionStatus::Deprecated
    );
    let kept_uri = "/api/models/imbor/versions/2025-kept-1/data?format=nt";
    assert_eq!(
        send(&state, get(kept_uri, None)).await.0,
        StatusCode::FORBIDDEN
    );
    let (status, body) = send(&state, get(kept_uri, Some(&token))).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"edited\""));
    // The kept copy cannot be written either.
    let (status, _) = send(
        &state,
        sparql_update(
            &token,
            &format!(
                "DELETE WHERE {{ GRAPH <{}> {{ ?s ?p ?o }} }}",
                kept.graph_iri
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let entry = body_json(
        test_app(state.clone())
            .oneshot(get("/api/models/imbor", None))
            .await
            .unwrap()
            .into_body(),
    )
    .await;
    assert_eq!(entry["latest_published"], "2025");
}

/// A model a user created through the API that happens to take the id
/// `imbor` stays theirs: the seeder adds nothing to it and writes no licence
/// record there, so they can still upload into it.
#[tokio::test]
async fn a_user_model_named_imbor_is_never_locked() {
    let (state, token) = admin_state();
    let (status, body) = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/models")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(
                serde_json::json!({ "title": "IMBOR", "namespace": "https://example.org/imbor#" })
                    .to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    seed_vocab::seed_standard_vocabularies(&state);
    let base = state.base_url.to_string();
    assert!(!registry::version_exists(
        &state.store,
        &base,
        "imbor",
        "2025"
    ));
    assert!(registry::no_derivatives_attribution(&state.store, &base, "imbor").is_none());

    let boundary = "XUSERX";
    let upload_body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"version\"\r\n\r\n1.0.0\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"m.ttl\"\r\n\
         Content-Type: text/turtle\r\n\r\n\
         <https://example.org/imbor#x> <http://www.w3.org/2000/01/rdf-schema#label> \"x\" .\r\n\
         --{boundary}--\r\n"
    );
    let (status, body) = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/models/imbor/versions")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(upload_body))
            .unwrap(),
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
}

/// The shipped nen2660-imbor bundle's IMBOR Kern model (CROW's release,
/// fetched by fetch.sh) carries CROW's licence record with no altered
/// copies, checked against the files; a direct write into its graphs is
/// refused. Skipped when the payloads are not fetched.
#[tokio::test]
async fn the_imbor_kern_bundle_model_is_guarded() {
    let bundles = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/seed-bundles");
    if !bundles
        .join("nen2660-imbor/imbor-release/imbor2025-kern.ttl")
        .exists()
    {
        assert!(
            std::env::var_os("OTS_TEST_SEED_PAYLOADS_REQUIRED").is_none(),
            "OTS_TEST_SEED_PAYLOADS_REQUIRED is set but the IMBOR release is not present"
        );
        eprintln!("SKIP: run examples/seed-bundles/nen2660-imbor/fetch.sh");
        return;
    }
    let (state, token) = admin_state();
    open_triplestore::seed_bundles::load_seed_dir(&state, &bundles);
    let base = state.base_url.to_string();
    let a = attribution(&state, "imbor-otl", "2025");
    assert!(a.no_derivatives, "{a:?}");
    assert!(
        a.unchanged,
        "the stored graphs are CROW's files: {}",
        a.stored_copy
    );
    assert!(registry::no_derivatives_attribution(&state.store, &base, "imbor-otl").is_some());
    let (status, body) = send(
        &state,
        sparql_update(
            &token,
            "INSERT DATA { GRAPH <https://data.crow.nl/imbor/def/> { \
             <https://data.crow.nl/imbor/def/x> <http://www.w3.org/2000/01/rdf-schema#label> \"x\" } }",
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/models/imbor-otl/versions/2025/draft")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(
                serde_json::json!({ "target_version": "2025-edit" }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The vocabularies seeded after the bundle keep their own records.
    seed_vocab::seed_standard_vocabularies(&state);
    assert!(attribution(&state, "imbor", "2025").unchanged);
}
