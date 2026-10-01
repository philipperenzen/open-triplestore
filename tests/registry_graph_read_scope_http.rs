//! The graphs of a published model-registry version are readable by whoever
//! may see the entry — over `/sparql`, the Graph Store Protocol and the
//! model profile, exactly as `GET /api/models/:id/versions/:v/data` already
//! serves them: a public entry to everyone, anonymous callers included; a
//! private entry to its owner organisation's members (and admins). Writing
//! such a graph stays the registry's business: a signed-in caller who does not
//! own the entry is refused, whether or not they may read it.
//!
//! Drives the real Axum router via `tower::ServiceExt::oneshot`.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{Role, SystemRole};
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::{registry, seed_vocab};
use open_triplestore::server::AppState;
use tower::ServiceExt as _;

const PRIVATE_MODEL: &str = "priv-model";
const PRIVATE_VERSION: &str = "1.0.0";

/// An instance with the bundled vocabularies seeded (public entries), one
/// private entry owned by `org-a` with a published version, an org member
/// (`member`) and a signed-in outsider (`outsider`).
fn instance() -> AppState {
    let (state, _admin) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);

    let db = &state.auth_db;
    db.create_organisation("org-a", "Org A", "org-a", None, None)
        .unwrap();
    db.create_user(
        "member",
        "member",
        "member@test.com",
        "hash",
        SystemRole::User,
    )
    .unwrap();
    db.add_org_member("member", "org-a", Role::Member).unwrap();
    db.create_user(
        "outsider",
        "outsider",
        "outsider@test.com",
        "hash",
        SystemRole::User,
    )
    .unwrap();

    let base = state.base_url.to_string();
    registry::insert_data_model(
        &state.store,
        &base,
        PRIVATE_MODEL,
        "Private model",
        "http://example.org/private#",
        None,
        false,
        Some("organisation"),
        Some("org-a"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let graph = private_graph(&state);
    registry::insert_version(
        &state.store,
        &base,
        &DataModelVersion {
            data_model_id: PRIVATE_MODEL.to_string(),
            version: PRIVATE_VERSION.to_string(),
            status: VersionStatus::Published,
            graph_iri: graph.clone(),
            sub_graphs: vec![graph.clone()],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();
    registry::update_latest_published(&state.store, &base, PRIVATE_MODEL, PRIVATE_VERSION).unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{graph}> {{ <http://example.org/private#Thing> a \
             <http://www.w3.org/2002/07/owl#Class> }} }}"
        ))
        .unwrap();
    state.auth_db.invalidate_accessible_graphs_cache();
    state
}

fn private_graph(state: &AppState) -> String {
    format!(
        "{}/data-model/{PRIVATE_MODEL}/version/{PRIVATE_VERSION}",
        state.base_url
    )
}

/// The graph holding the seeded SKOS vocabulary's published version.
fn skos_graph(state: &AppState) -> String {
    registry::get_version(&state.store, &state.base_url, "skos", "2009-08-18")
        .expect("skos is seeded")
        .graph_iri
}

fn bearer(
    builder: axum::http::request::Builder,
    token: Option<&str>,
) -> axum::http::request::Builder {
    match token {
        Some(t) => builder.header(header::AUTHORIZATION, format!("Bearer {t}")),
        None => builder,
    }
}

/// `SELECT (COUNT(*) AS ?n) WHERE { GRAPH <graph> { ?s ?p ?o } }` over
/// `POST /sparql`, as the caller `token` (`None`: anonymous).
async fn count_over_sparql(
    state: &AppState,
    token: Option<&str>,
    graph: &str,
) -> (StatusCode, u64) {
    let query = format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}");
    let req = bearer(
        Request::builder().method(Method::POST).uri("/sparql"),
        token,
    )
    .header(header::CONTENT_TYPE, "application/sparql-query")
    .header(header::ACCEPT, "application/sparql-results+json")
    .body(Body::from(query))
    .unwrap();
    let resp = test_app(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    let json = body_json(resp.into_body()).await;
    let n = json["results"]["bindings"][0]["n"]["value"]
        .as_str()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    (status, n)
}

async fn graph_store_get(state: &AppState, token: Option<&str>, graph: &str) -> StatusCode {
    let req = bearer(
        Request::builder()
            .method(Method::GET)
            .uri(format!("/store?graph={}", url_encode(graph))),
        token,
    )
    .header(header::ACCEPT, "text/turtle")
    .body(Body::empty())
    .unwrap();
    test_app(state.clone()).oneshot(req).await.unwrap().status()
}

async fn get_status(state: &AppState, token: Option<&str>, uri: &str) -> StatusCode {
    let req = bearer(Request::builder().method(Method::GET).uri(uri), token)
        .body(Body::empty())
        .unwrap();
    test_app(state.clone()).oneshot(req).await.unwrap().status()
}

async fn insert_over_sparql(state: &AppState, token: &str, graph: &str) -> StatusCode {
    let update = format!(
        "INSERT DATA {{ GRAPH <{graph}> {{ <http://example.org/intruder> \
         <http://example.org/wrote> \"this\" }} }}"
    );
    let req = Request::builder()
        .method(Method::POST)
        .uri("/sparql")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/sparql-update")
        .body(Body::from(update))
        .unwrap();
    test_app(state.clone()).oneshot(req).await.unwrap().status()
}

fn triples_in(state: &AppState, graph: &str) -> usize {
    state.store.graph_count_cached(Some(graph)).unwrap_or(0)
}

#[tokio::test]
async fn a_public_registry_graph_is_readable_anonymously_over_sparql_and_the_graph_store() {
    let state = instance();
    let skos = skos_graph(&state);
    assert!(
        triples_in(&state, &skos) > 0,
        "skos was seeded into <{skos}>"
    );

    // The behaviour /data already has …
    assert_eq!(
        get_status(&state, None, "/api/models/skos/versions/2009-08-18/data").await,
        StatusCode::OK
    );
    // … and now the protocol endpoints agree with it.
    let (status, n) = count_over_sparql(&state, None, &skos).await;
    assert_eq!(status, StatusCode::OK);
    assert!(n > 0, "anonymous GRAPH <{skos}> count is {n}");
    assert_eq!(graph_store_get(&state, None, &skos).await, StatusCode::OK);

    // A signed-in caller who owns nothing reads it too.
    let outsider = mint_token("outsider", "outsider", "user");
    let (status, n) = count_over_sparql(&state, Some(&outsider), &skos).await;
    assert_eq!(status, StatusCode::OK);
    assert!(n > 0, "signed-in GRAPH <{skos}> count is {n}");
    assert_eq!(
        graph_store_get(&state, Some(&outsider), &skos).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_public_model_profile_is_readable_by_anyone_who_may_read_the_entry() {
    let state = instance();
    let profile = "/api/models/skos/versions/2009-08-18/profile";
    assert_eq!(get_status(&state, None, profile).await, StatusCode::OK);
    let outsider = mint_token("outsider", "outsider", "user");
    assert_eq!(
        get_status(&state, Some(&outsider), profile).await,
        StatusCode::OK
    );
    // A private entry's profile answers as its data does: not found to whoever
    // may not see the entry, served to a member of the owner organisation.
    let private = format!("/api/models/{PRIVATE_MODEL}/versions/{PRIVATE_VERSION}/profile");
    assert_eq!(
        get_status(&state, None, &private).await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_status(&state, Some(&outsider), &private).await,
        StatusCode::NOT_FOUND
    );
    let member = mint_token("member", "member", "user");
    assert_eq!(
        get_status(&state, Some(&member), &private).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_private_registry_graph_is_read_only_by_who_may_see_the_entry() {
    let state = instance();
    let graph = private_graph(&state);
    assert_eq!(triples_in(&state, &graph), 1);

    // Anonymous: nothing over /sparql, refused over /store.
    let (status, n) = count_over_sparql(&state, None, &graph).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(n, 0, "anonymous caller reads a private registry graph");
    let status = graph_store_get(&state, None, &graph).await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::NOT_FOUND,
        "anonymous /store read of a private registry graph answers {status}"
    );

    // A signed-in non-member: the same.
    let outsider = mint_token("outsider", "outsider", "user");
    let (status, n) = count_over_sparql(&state, Some(&outsider), &graph).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(n, 0, "an outsider reads a private registry graph");
    let status = graph_store_get(&state, Some(&outsider), &graph).await;
    assert!(
        status == StatusCode::UNAUTHORIZED
            || status == StatusCode::FORBIDDEN
            || status == StatusCode::NOT_FOUND,
        "outsider /store read of a private registry graph answers {status}"
    );

    // A member of the owner organisation reads it, as /data serves it to them.
    let member = mint_token("member", "member", "user");
    assert_eq!(
        get_status(
            &state,
            Some(&member),
            &format!("/api/models/{PRIVATE_MODEL}/versions/{PRIVATE_VERSION}/data")
        )
        .await,
        StatusCode::OK
    );
    let (status, n) = count_over_sparql(&state, Some(&member), &graph).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        n, 1,
        "the owner org's member reads the private registry graph"
    );
    assert_eq!(
        graph_store_get(&state, Some(&member), &graph).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn reading_a_public_registry_graph_grants_no_write_to_it() {
    let state = instance();
    let skos = skos_graph(&state);
    let before = triples_in(&state, &skos);

    let outsider = mint_token("outsider", "outsider", "user");
    let status = insert_over_sparql(&state, &outsider, &skos).await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "a non-owner's INSERT DATA into a public registry graph answers {status}"
    );
    assert_eq!(triples_in(&state, &skos), before, "nothing was written");

    // Reading it is unaffected by the refusal.
    let (status, n) = count_over_sparql(&state, Some(&outsider), &skos).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(n as usize, before);
}
