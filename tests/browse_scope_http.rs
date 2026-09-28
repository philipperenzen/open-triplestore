//! Browse scope resolution: `dataset_id`, `dataset_ids`, `org_id` and `org_ids`
//! name one scope together, not four competing ones.
//!
//! The Triple Browser sends `dataset_ids` *and* `org_id` whenever the user has
//! picked some datasets and an organisation. The handlers used to resolve the
//! scope with an `if let … else if let …` chain, so the organisation was dropped
//! on the floor and both the triple rows and the "Terms in scope" facets came
//! back covering the datasets alone. The scope is now the union of everything
//! named, deduplicated, with the access-control filter applied to every dataset
//! in it — including the ones reached through an organisation.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use serde_json::Value;
use std::collections::BTreeSet;
use tower::ServiceExt as _;

const G_A1: &str = "https://example.org/scope/a1";
const G_A2: &str = "https://example.org/scope/a2";
const G_B1: &str = "https://example.org/scope/b1";
const G_SOLO: &str = "https://example.org/scope/solo";
const G_SECRET: &str = "https://example.org/scope/secret";

/// Two organisations with a dataset each (`org-a` has two), plus a dataset owned
/// by a user directly, plus one dataset of `org-a` that only its members may
/// read. Every graph carries one typed subject so the facets have something to
/// count.
fn scoped_state() -> (AppState, String) {
    let (state, token) = admin_state();

    state
        .auth_db
        .create_organisation("org-a", "Org A", "org-a", None, None)
        .unwrap();
    state
        .auth_db
        .create_organisation("org-b", "Org B", "org-b", None, None)
        .unwrap();

    let datasets = [
        (
            "ds-a1",
            OwnerType::Organisation,
            "org-a",
            Visibility::Public,
            G_A1,
        ),
        (
            "ds-a2",
            OwnerType::Organisation,
            "org-a",
            Visibility::Public,
            G_A2,
        ),
        (
            "ds-b1",
            OwnerType::Organisation,
            "org-b",
            Visibility::Public,
            G_B1,
        ),
        (
            "ds-solo",
            OwnerType::User,
            "adm",
            Visibility::Public,
            G_SOLO,
        ),
        (
            "ds-secret",
            OwnerType::Organisation,
            "org-a",
            Visibility::Private,
            G_SECRET,
        ),
    ];
    for (id, owner_type, owner_id, visibility, graph) in datasets {
        state
            .auth_db
            .create_dataset(id, id, None, owner_type, owner_id, visibility, None)
            .unwrap();
        state.auth_db.add_dataset_graph(id, graph).unwrap();
        state
            .store
            .load_str(
                &format!("<urn:subject:{id}> a <urn:class:Thing> ; <urn:prop:name> \"{id}\" ."),
                RdfFormat::Turtle,
                Some(graph),
            )
            .unwrap();
    }

    (state, token)
}

async fn get_json(app: &Router, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(Method::GET).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// The distinct graphs the triple rows came from — the scope, as the caller sees it.
async fn triples_graphs(app: &Router, query: &str, token: Option<&str>) -> BTreeSet<String> {
    let (status, body) = get_json(
        app,
        &format!("/api/browse/triples?limit=500&{query}"),
        token,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "browse/triples {query} → {body}");
    body["triples"]
        .as_array()
        .expect("triples is an array")
        .iter()
        .filter_map(|t| t["graph"]["value"].as_str().map(str::to_string))
        .collect()
}

/// The graphs named by the facets endpoint for the same scope.
async fn facet_graphs(app: &Router, query: &str, token: Option<&str>) -> BTreeSet<String> {
    let (status, body) = get_json(app, &format!("/api/browse/facets?{query}"), token).await;
    assert_eq!(status, StatusCode::OK, "browse/facets {query} → {body}");
    body["graphs"]
        .as_array()
        .expect("graphs is an array")
        .iter()
        .filter_map(|g| g["iri"].as_str().map(str::to_string))
        .collect()
}

fn set(iris: &[&str]) -> BTreeSet<String> {
    iris.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn a_dataset_only_scope_is_unchanged() {
    let (state, token) = scoped_state();
    let app = test_app(state);

    assert_eq!(
        triples_graphs(&app, "dataset_id=ds-a1", Some(&token)).await,
        set(&[G_A1])
    );
    assert_eq!(
        triples_graphs(&app, "dataset_ids=ds-a1,ds-b1", Some(&token)).await,
        set(&[G_A1, G_B1])
    );
    // Whitespace and empty entries in the CSV are still tolerated.
    assert_eq!(
        triples_graphs(&app, "dataset_ids=ds-a1,%20,ds-solo", Some(&token)).await,
        set(&[G_A1, G_SOLO])
    );
}

#[tokio::test]
async fn an_org_only_scope_is_unchanged() {
    let (state, token) = scoped_state();
    let app = test_app(state);

    assert_eq!(
        triples_graphs(&app, "org_id=org-a", Some(&token)).await,
        set(&[G_A1, G_A2, G_SECRET])
    );
    assert_eq!(
        triples_graphs(&app, "org_id=org-b", Some(&token)).await,
        set(&[G_B1])
    );
}

#[tokio::test]
async fn an_empty_scope_stays_empty_rather_than_becoming_everything() {
    let (state, token) = scoped_state();
    let app = test_app(state);

    // An organisation with no datasets, a dataset that does not exist, and an
    // empty CSV all resolve to "nothing in scope" — never to the admin's
    // unscoped "every graph" branch.
    for q in [
        "org_id=org-nope",
        "dataset_ids=",
        "dataset_ids=,%20,",
        "dataset_id=ds-nope&org_id=org-nope",
        "org_ids=",
    ] {
        assert!(
            triples_graphs(&app, q, Some(&token)).await.is_empty(),
            "scope `{q}` must resolve to no graphs"
        );
        assert!(
            facet_graphs(&app, q, Some(&token)).await.is_empty(),
            "facets for scope `{q}` must be empty"
        );
    }

    // No scope params at all still means "everything the admin can see".
    assert!(triples_graphs(&app, "", Some(&token)).await.len() >= 4);
}

#[tokio::test]
async fn datasets_and_an_org_together_are_unioned() {
    let (state, token) = scoped_state();
    let app = test_app(state);

    // The regression: `org_id` used to be dropped whenever `dataset_ids` was
    // present, so this came back as {G_B1} alone.
    assert_eq!(
        triples_graphs(&app, "dataset_ids=ds-b1&org_id=org-a", Some(&token)).await,
        set(&[G_A1, G_A2, G_SECRET, G_B1])
    );
    // The same for the single-dataset form.
    assert_eq!(
        triples_graphs(&app, "dataset_id=ds-solo&org_id=org-b", Some(&token)).await,
        set(&[G_SOLO, G_B1])
    );
}

#[tokio::test]
async fn org_ids_scopes_to_several_organisations() {
    let (state, token) = scoped_state();
    let app = test_app(state);

    assert_eq!(
        triples_graphs(&app, "org_ids=org-a,org-b", Some(&token)).await,
        set(&[G_A1, G_A2, G_SECRET, G_B1])
    );
    // `org_id` and `org_ids` name one union too.
    assert_eq!(
        triples_graphs(&app, "org_id=org-b&org_ids=org-a", Some(&token)).await,
        set(&[G_A1, G_A2, G_SECRET, G_B1])
    );
    assert_eq!(
        triples_graphs(&app, "dataset_ids=ds-solo&org_ids=org-b", Some(&token)).await,
        set(&[G_SOLO, G_B1])
    );
}

#[tokio::test]
async fn a_dataset_named_twice_is_counted_once() {
    let (state, token) = scoped_state();
    let app = test_app(state);

    // ds-a1 is named directly *and* reached through org-a, so the union has it
    // twice. Naming it that way must add nothing to the organisation's own
    // scope: the same rows, the same exact count, each triple once.
    let (status, body) = get_json(
        &app,
        "/api/browse/triples?limit=500&count=true&dataset_ids=ds-a1&org_id=org-a",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let rows = body["triples"].as_array().unwrap().len();
    let org_only = get_json(
        &app,
        "/api/browse/triples?limit=500&count=true&org_id=org-a",
        Some(&token),
    )
    .await
    .1;
    assert_eq!(
        rows,
        org_only["triples"].as_array().unwrap().len(),
        "naming a dataset of the org as well must not duplicate its rows"
    );
    assert_eq!(body["total"], org_only["total"]);
}

#[tokio::test]
async fn the_facets_agree_with_the_triples_for_the_same_scope() {
    let (state, token) = scoped_state();
    let app = test_app(state);

    for q in [
        "dataset_ids=ds-b1&org_id=org-a",
        "org_ids=org-a,org-b",
        "dataset_id=ds-solo&org_ids=org-b",
        "dataset_ids=ds-a1&org_id=org-a",
    ] {
        assert_eq!(
            facet_graphs(&app, q, Some(&token)).await,
            triples_graphs(&app, q, Some(&token)).await,
            "facets and triples must resolve the same scope for `{q}`"
        );
    }
}

#[tokio::test]
async fn the_union_never_widens_what_a_caller_may_read() {
    let (state, token) = scoped_state();
    state
        .auth_db
        .create_user(
            "outsider",
            "outsider",
            "out@test.com",
            "hash",
            SystemRole::User,
        )
        .unwrap();
    let outsider = mint_token("outsider", "outsider", "user");
    let app = test_app(state);

    // The admin sees org-a's private dataset; a non-member does not — through
    // `org_id` alone, and equally when the organisation arrives as half of a
    // union. Reaching a dataset via an organisation grants nothing.
    assert!(triples_graphs(&app, "org_id=org-a", Some(&token))
        .await
        .contains(G_SECRET));
    for q in [
        "org_id=org-a",
        "org_ids=org-a",
        "dataset_ids=ds-b1&org_id=org-a",
        "dataset_ids=ds-secret&org_ids=org-b",
    ] {
        let seen = triples_graphs(&app, q, Some(&outsider)).await;
        assert!(
            !seen.contains(G_SECRET),
            "a non-member must not read the private dataset via `{q}` (saw {seen:?})"
        );
        assert_eq!(facet_graphs(&app, q, Some(&outsider)).await, seen);
    }
    // The public halves of those unions are still readable, so the assertion
    // above is about access control and not about an empty response.
    assert_eq!(
        triples_graphs(&app, "dataset_ids=ds-b1&org_id=org-a", Some(&outsider)).await,
        set(&[G_A1, G_A2, G_B1])
    );
}

#[tokio::test]
async fn the_resource_view_follows_the_same_union() {
    let (state, token) = scoped_state();
    let app = test_app(state);

    // Expanding a resource that lives in the org half of the scope must find it
    // when datasets are named alongside the organisation.
    let (status, body) = get_json(
        &app,
        "/api/browse/resource?iri=urn:subject:ds-a1&dataset_ids=ds-b1&org_id=org-a",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body["outgoing"].as_array().unwrap().is_empty(),
        "the resource is in org-a's half of the union → {body}"
    );

    // And a resource outside the union is still not readable through it.
    let (status, body) = get_json(
        &app,
        "/api/browse/resource?iri=urn:subject:ds-solo&dataset_ids=ds-b1&org_id=org-a",
        Some(&token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["outgoing"].as_array().unwrap().is_empty(),
        "ds-solo is in neither half of the union → {body}"
    );
}
