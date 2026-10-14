//! Model versions ↔ dependent datasets: a cut dataset version records the model
//! version it was pinned to; the conformance layer flags a newer published model
//! version; `/api/models/:id/dependents` lists the datasets behind it; publishing a
//! model version lands on the model's commit log.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::registry as dmr;
use open_triplestore::server::AppState;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const EX: &str = "http://ex.org/";
const G1: &str = "urn:dep:d1:instances";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = if body.is_null() {
        Body::empty()
    } else {
        b = b.header(header::CONTENT_TYPE, "application/json");
        Body::from(body.to_string())
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

fn model_version(state: &AppState, ver: &str, status: VersionStatus) {
    let base = state.base_url.to_string();
    dmr::insert_version(
        &state.store,
        &base,
        &DataModelVersion {
            data_model_id: "m1".to_string(),
            version: ver.to_string(),
            status,
            graph_iri: format!("{base}/data-model/m1/version/{ver}"),
            sub_graphs: vec![],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();
}

#[tokio::test]
async fn dataset_versions_record_the_model_pin_and_dependents_see_updates() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m1",
        "Bridges",
        &format!("{EX}def#"),
        None,
        true,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    model_version(&state, "1.0.0", VersionStatus::Published);
    dmr::update_latest_published(&state.store, &base, "m1", "1.0.0").unwrap();

    // A dataset pinned to m1@1.0.0 with one instance graph.
    let (st, v, txt) = req(&app, Method::POST, "/api/datasets", &token,
        json!({ "name": "d1", "owner_type": "user", "owner_id": "adm", "visibility": "private", "graph_role": "instances" })).await;
    assert!(st.is_success(), "{st} {txt}");
    let d1 = v["id"].as_str().unwrap().to_string();
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/graphs"),
        &token,
        json!({ "graph_iri": G1 }),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "d1", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": "1.0.0" })).await;
    assert!(st.is_success(), "{st} {txt}");

    // Cutting a version stamps the pin onto it.
    let (st, ver, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/versions"),
        &token,
        json!({ "version": "1.0.0", "notes": "first" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(ver["conforms_to_model"], "m1");
    assert_eq!(ver["conforms_to_version"], "1.0.0");
    let (st, list, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/versions"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(list[0]["conforms_to_version"], "1.0.0");
    let (st, _, _) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/versions/1.0.0/publish"),
        &token,
        Value::Null,
    )
    .await;
    assert!(st.is_success());

    // Current: the layer says so, dependents are up to date.
    let (st, layer, txt) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(layer["conforms_to_model"]["latest_published"], "1.0.0");
    assert_eq!(layer["conforms_to_model"]["pinned"], true);
    assert_eq!(layer["conforms_to_model"]["update_available"], false);
    let (st, deps, txt) = req(
        &app,
        Method::GET,
        "/api/models/m1/dependents",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(deps["datasets"][0]["dataset_id"], d1.as_str());
    assert_eq!(deps["datasets"][0]["update_available"], false);
    assert_eq!(deps["datasets"][0]["published_version"], "1.0.0");
    assert_eq!(
        deps["datasets"][0]["published_conforms_to_version"],
        "1.0.0"
    );

    // Model 2.0.0 is published through the API: the dataset is now behind.
    model_version(&state, "2.0.0", VersionStatus::Staged);
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/models/m1/versions/2.0.0/publish",
        &token,
        Value::Null,
    )
    .await;
    assert!(st.is_success(), "publish model: {st} {txt}");
    let (_, layer, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(
        layer["conforms_to_model"]["version"], "1.0.0",
        "pinned datasets do not move"
    );
    assert_eq!(layer["conforms_to_model"]["latest_published"], "2.0.0");
    assert_eq!(layer["conforms_to_model"]["update_available"], true);
    let (_, deps, _) = req(
        &app,
        Method::GET,
        "/api/models/m1/dependents",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(deps["latest_published"], "2.0.0");
    assert_eq!(deps["datasets"][0]["update_available"], true);
    assert_eq!(deps["datasets"][0]["pinned_version"], "1.0.0");

    // Publishing landed on the model's commit log.
    let (st, commits, txt) = req(
        &app,
        Method::GET,
        "/api/models/m1/commits",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let msgs: Vec<String> = commits
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|c| c["message"].as_str().map(str::to_string))
        .collect();
    assert!(
        msgs.iter().any(|m| m.contains("Published version 2.0.0")),
        "{txt}"
    );

    // The update procedure ends with a re-pin: the layer is current again and the
    // next dataset version records the new model version.
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "d1", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": "2.0.0" })).await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, ver2, _) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/versions"),
        &token,
        json!({ "version": "1.1.0", "notes": "conforms to m1@2.0.0" }),
    )
    .await;
    assert_eq!(ver2["conforms_to_version"], "2.0.0");
    let (_, layer, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(layer["conforms_to_model"]["update_available"], false);

    // A floating dataset (no pinned version) is never "behind".
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "d1", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": null })).await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, layer, _) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{d1}/conformance"),
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(layer["conforms_to_model"]["pinned"], false);
    assert_eq!(layer["conforms_to_model"]["update_available"], false);
}

#[tokio::test]
async fn dependents_are_visibility_scoped() {
    let (state, admin) = admin_state();
    let app = test_app(state.clone());
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m2",
        "Public model",
        &format!("{EX}def2#"),
        None,
        true,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    model_version(&state, "1.0.0", VersionStatus::Published);
    let (st, v, txt) = req(&app, Method::POST, "/api/datasets", &admin,
        json!({ "name": "secret", "owner_type": "user", "owner_id": "adm", "visibility": "private" })).await;
    assert!(st.is_success(), "{st} {txt}");
    let id = v["id"].as_str().unwrap().to_string();
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{id}"), &admin,
        json!({ "name": "secret", "visibility": "private", "conforms_to_model": "m2", "conforms_to_version": "1.0.0" })).await;
    assert!(st.is_success(), "{st} {txt}");
    // Another (plain) user sees the public model but not the private dataset behind it.
    let other = mint_token("u2", "someone", "user");
    let (st, deps, txt) = req(
        &app,
        Method::GET,
        "/api/models/m2/dependents",
        &other,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(deps["datasets"].as_array().map(|a| a.len()), Some(0));
    let (_, deps, _) = req(
        &app,
        Method::GET,
        "/api/models/m2/dependents",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(deps["datasets"].as_array().map(|a| a.len()), Some(1));
}

// ─── DELETE /api/models/:id/versions/:ver ─────────────────────────────────────

/// Model `m1` owned by `owner`, public, with versions 1.0.0 (published, latest)
/// and 2.0.0 (draft, latest draft), each graph holding `n` triples.
fn model_with_two_versions(state: &AppState, owner: &str) -> (String, String) {
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m1",
        "Bridges",
        &format!("{EX}def#"),
        None,
        true,
        Some("user"),
        Some(owner),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    model_version(state, "1.0.0", VersionStatus::Published);
    dmr::update_latest_published(&state.store, &base, "m1", "1.0.0").unwrap();
    model_version(state, "2.0.0", VersionStatus::Draft);
    dmr::update_latest_draft(&state.store, &base, "m1", "2.0.0").unwrap();
    let g1 = format!("{base}/data-model/m1/version/1.0.0");
    let g2 = format!("{base}/data-model/m1/version/2.0.0");
    for g in [&g1, &g2] {
        state
            .store
            .load_str(
                &format!("<{EX}A> a <http://www.w3.org/2002/07/owl#Class> . <{EX}B> a <http://www.w3.org/2002/07/owl#Class> ."),
                oxigraph::io::RdfFormat::Turtle,
                Some(g),
            )
            .unwrap();
    }
    (g1, g2)
}

fn graph_exists(state: &AppState, g: &str) -> bool {
    state
        .store
        .named_graphs()
        .unwrap()
        .iter()
        .any(|n| n.as_str() == g)
}

async fn commit_messages(app: &Router, token: &str) -> Vec<String> {
    let (_, commits, _) = req(
        app,
        Method::GET,
        "/api/models/m1/commits",
        token,
        Value::Null,
    )
    .await;
    commits
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c["message"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn deleting_a_draft_version_drops_its_graphs_and_registry_rows() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let (g1, g2) = model_with_two_versions(&state, "adm");

    let (st, out, txt) = req(
        &app,
        Method::DELETE,
        "/api/models/m1/versions/2.0.0",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(out["deleted"], true);
    assert_eq!(out["forced"], false);
    assert_eq!(out["graphs_dropped"], json!([g2.clone()]));
    assert_eq!(out["triples_removed"], 2);

    // The version is gone: record, graph, and the entry's draft pointer.
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/models/m1/versions/2.0.0",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert!(!graph_exists(&state, &g2), "the version graph is dropped");
    assert!(
        graph_exists(&state, &g1),
        "other versions keep their graphs"
    );
    let (_, model, _) = req(&app, Method::GET, "/api/models/m1", &token, Value::Null).await;
    assert!(model["latest_draft"].is_null(), "{model}");
    assert_eq!(model["latest_published"], "1.0.0");
    let (_, versions, _) = req(
        &app,
        Method::GET,
        "/api/models/m1/versions",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(versions.as_array().map(|a| a.len()), Some(1), "{versions}");
    // No row of the version is left in the registry graph.
    let ver_iri = dmr::version_record_iri(&state.base_url, "m1", "2.0.0");
    let ask = format!(
        "ASK {{ GRAPH <{}> {{ {{ <{ver_iri}> ?p ?o }} UNION {{ ?s ?p <{ver_iri}> }} }} }}",
        dmr::REGISTRY_GRAPH
    );
    assert!(matches!(
        state.store.query(&ask),
        Ok(oxigraph::sparql::QueryResults::Boolean(false))
    ));

    // Commit log and audit log both carry it.
    assert!(commit_messages(&app, &token)
        .await
        .iter()
        .any(|m| m == "Deleted version 2.0.0"),);
    let events = state
        .audit
        .list(50, 0, Some("graph_deleted"), Some("adm"), None)
        .unwrap();
    let ev = events
        .iter()
        .find(|e| e.action.as_deref() == Some("delete_model_version"))
        .expect("an audit event for the delete");
    assert_eq!(ev.resource_id.as_deref(), Some("m1/2.0.0"));
    assert_eq!(ev.details.as_ref().unwrap()["graphs"], json!([g2]));

    // Deleting it again is a 404, not a second delete.
    let (st, _, _) = req(
        &app,
        Method::DELETE,
        "/api/models/m1/versions/2.0.0",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_published_version_needs_force() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let (g1, _) = model_with_two_versions(&state, "adm");

    let (st, body, txt) = req(
        &app,
        Method::DELETE,
        "/api/models/m1/versions/1.0.0",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{txt}");
    assert_eq!(body["published"], true);
    assert_eq!(body["force_allowed"], true);
    assert_eq!(body["reasons"][0]["code"], "published");
    assert!(
        body["error"].as_str().unwrap().contains("force=true"),
        "{txt}"
    );
    assert!(
        graph_exists(&state, &g1),
        "a refused delete changes nothing"
    );

    let (st, out, txt) = req(
        &app,
        Method::DELETE,
        "/api/models/m1/versions/1.0.0?force=true",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(out["forced"], true);
    assert!(!graph_exists(&state, &g1));
    let (_, model, _) = req(&app, Method::GET, "/api/models/m1", &token, Value::Null).await;
    assert!(
        model["latest_published"].is_null(),
        "the latest pointer no longer names a deleted version: {model}"
    );
    assert!(commit_messages(&app, &token)
        .await
        .iter()
        .any(|m| m == "Deleted published version 1.0.0 (forced)"));
}

#[tokio::test]
async fn dependent_datasets_block_a_delete_even_with_force() {
    let (state, token) = admin_state();
    let app = test_app(state.clone());
    let (g1, _) = model_with_two_versions(&state, "adm");

    // A private dataset pinned to m1@1.0.0.
    let (st, v, txt) = req(&app, Method::POST, "/api/datasets", &token,
        json!({ "name": "pinned", "owner_type": "user", "owner_id": "adm", "visibility": "private", "graph_role": "instances" })).await;
    assert!(st.is_success(), "{st} {txt}");
    let d1 = v["id"].as_str().unwrap().to_string();
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/graphs"),
        &token,
        json!({ "graph_iri": "urn:dep:pinned:instances" }),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "pinned", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": "1.0.0" })).await;
    assert!(st.is_success(), "{st} {txt}");

    for uri in [
        "/api/models/m1/versions/1.0.0",
        "/api/models/m1/versions/1.0.0?force=true",
    ] {
        let (st, body, txt) = req(&app, Method::DELETE, uri, &token, Value::Null).await;
        assert_eq!(st, StatusCode::CONFLICT, "{uri}: {txt}");
        assert_eq!(body["force_allowed"], false, "{txt}");
        let deps = body["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["code"] == "dependents")
            .cloned()
            .expect("a dependents reason");
        assert_eq!(deps["datasets"][0]["dataset_id"], d1.as_str(), "{txt}");
        assert_eq!(deps["datasets"][0]["reason"], "pinned");
        assert_eq!(deps["hidden_datasets"], 0);
    }
    assert!(graph_exists(&state, &g1));

    // A floating dataset (no pin) depends on the latest published version.
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "pinned", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": null })).await;
    assert!(st.is_success(), "{st} {txt}");
    let (st, body, txt) = req(
        &app,
        Method::DELETE,
        "/api/models/m1/versions/1.0.0?force=true",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{txt}");
    assert_eq!(
        body["reasons"][0]["datasets"][0]["reason"], "floating",
        "{txt}"
    );

    // A dataset version cut while pinned keeps 1.0.0 alive after the dataset
    // re-pins to 2.0.0.
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "pinned", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": "1.0.0" })).await;
    assert!(st.is_success(), "{st} {txt}");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{d1}/versions"),
        &token,
        json!({ "version": "1.0.0", "notes": "first" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, _, txt) = req(&app, Method::PUT, &format!("/api/datasets/{d1}"), &token,
        json!({ "name": "pinned", "visibility": "private", "conforms_to_model": "m1", "conforms_to_version": "2.0.0" })).await;
    assert!(st.is_success(), "{st} {txt}");
    let (st, body, txt) = req(
        &app,
        Method::DELETE,
        "/api/models/m1/versions/1.0.0?force=true",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{txt}");
    let dep = &body["reasons"][0]["datasets"][0];
    assert_eq!(dep["reason"], "dataset_version", "{txt}");
    assert_eq!(dep["dataset_version"], "1.0.0");
    // ... and 2.0.0 is now pinned, so it is blocked too, though only a draft.
    let (st, body, txt) = req(
        &app,
        Method::DELETE,
        "/api/models/m1/versions/2.0.0",
        &token,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{txt}");
    assert_eq!(body["published"], false);
    assert_eq!(body["reasons"][0]["code"], "dependents");
}

#[tokio::test]
async fn only_admins_and_publishers_who_may_write_the_entry_may_delete() {
    let (state, admin) = admin_state();
    let app = test_app(state.clone());
    use open_triplestore::auth::models::SystemRole;
    for (id, name) in [("owner", "owner"), ("pub", "pub"), ("plain", "plain")] {
        state
            .auth_db
            .create_user(
                id,
                name,
                &format!("{name}@test.com"),
                "hash",
                SystemRole::User,
            )
            .unwrap();
    }
    state.auth_db.update_user_can_publish("pub", true).unwrap();
    model_with_two_versions(&state, "owner");
    let owner = mint_token("owner", "owner", "user");
    let publisher = mint_token("pub", "pub", "user");
    let plain = mint_token("plain", "plain", "user");
    let uri = "/api/models/m1/versions/2.0.0";

    // Anonymous: the route needs a token.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // A plain user, and a publisher who may not write this entry: 403.
    for t in [&plain, &publisher] {
        let (st, _, txt) = req(&app, Method::DELETE, uri, t, Value::Null).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{txt}");
    }
    // The entry's owner without the publish permission: 403 as well.
    let (st, _, txt) = req(&app, Method::DELETE, uri, &owner, Value::Null).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{txt}");
    assert!(dmr::version_exists(
        &state.store,
        &state.base_url,
        "m1",
        "2.0.0"
    ));

    // Granted the publish permission, the owner may.
    state
        .auth_db
        .update_user_can_publish("owner", true)
        .unwrap();
    let (st, _, txt) = req(&app, Method::DELETE, uri, &owner, Value::Null).await;
    assert_eq!(st, StatusCode::OK, "{txt}");

    // A private entry is not there for someone who may not see it.
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "secret",
        "Secret",
        &format!("{EX}secret#"),
        None,
        false,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let (st, _, txt) = req(
        &app,
        Method::DELETE,
        "/api/models/secret/versions/1.0.0",
        &publisher,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{txt}");
    // An admin may delete in an entry someone else owns.
    let (st, _, txt) = req(
        &app,
        Method::DELETE,
        "/api/models/m1/versions/1.0.0?force=true",
        &admin,
        Value::Null,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
}
