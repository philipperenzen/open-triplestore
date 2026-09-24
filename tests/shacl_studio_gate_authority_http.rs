//! A SHACL Studio pipeline with `gate_writes` refuses (422) every write to the
//! graphs it covers that its shapes reject, whoever makes the write — the
//! graph's owners and editors included. So setting one up is a write-side
//! power over those graphs, and it needs the authority a validation-layer
//! binding (which gates writes the same way) already needs: write access to
//! each dataset and graph it would gate. Read access, which is all a plain
//! validation pipeline needs, is not enough — through the real router.
//!
//! * creating or updating a gating pipeline checks the author's write access
//!   to every dataset (dataset targets, legacy `dataset_ids`) and graph (graph
//!   targets, legacy `graph_iris`) it covers; admins pass;
//! * the gate itself runs with its creator's authority, checked at every
//!   write: a gating pipeline stored before this check, or one whose creator
//!   has since lost write access, no longer gates.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use open_triplestore::shacl_studio::models::{
    ResultsTarget, SeverityThreshold, TargetKind, ValidationPipeline, ValidationTarget, WriteTarget,
};
use open_triplestore::shacl_studio::store::ShaclStudioStore;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// In alice's public dataset: every signed-in user may read it, only alice
/// (the owner, with a graph-ACL write grant for the Graph Store) writes it.
const DATA_GRAPH: &str = "http://pub.example/data";
const DATASET: &str = "pub-ds";

/// Shapes that reject any subject with a type: every write of typed data fails.
const BLOCKING_SHAPES: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
    <http://mallory.example/Block> a sh:NodeShape ;\n\
      sh:targetSubjectsOf rdf:type ;\n\
      sh:property [ sh:path rdf:type ; sh:maxCount 0 ] .\n";

/// An ordinary data-quality rule: a person has a name.
const PERSON_SHAPES: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix ex: <http://ex.org/> .\n\
    ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;\n\
      sh:property [ sh:path ex:name ; sh:minCount 1 ] .\n";

const NAMED: &str = "<http://ex.org/p1> a <http://ex.org/Person> ; <http://ex.org/name> \"Ada\" .";
const UNNAMED: &str = "<http://ex.org/p2> a <http://ex.org/Person> .";

async fn send(
    state: &AppState,
    method: Method,
    uri: &str,
    token: &str,
    content_type: &str,
    body: String,
) -> (StatusCode, String) {
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn json_req(
    state: &AppState,
    method: Method,
    uri: &str,
    token: &str,
    body: Value,
) -> (StatusCode, String) {
    send(
        state,
        method,
        uri,
        token,
        "application/json",
        body.to_string(),
    )
    .await
}

/// A Graph Store PUT of `turtle` into [`DATA_GRAPH`].
async fn put_data(state: &AppState, token: &str, turtle: &str) -> (StatusCode, String) {
    send(
        state,
        Method::PUT,
        &format!("/store?graph={}", url_encode(DATA_GRAPH)),
        token,
        "text/turtle",
        turtle.to_string(),
    )
    .await
}

fn make_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

/// Alice's public dataset holding [`DATA_GRAPH`], which alice may also write
/// over the Graph Store. Returns `(admin, alice, mallory)`.
fn setup() -> (AppState, String, String, String) {
    let (state, admin) = admin_state();
    let alice = make_user(&state, "alice");
    let mallory = make_user(&state, "mallory");
    state
        .auth_db
        .create_dataset(
            DATASET,
            DATASET,
            None,
            OwnerType::User,
            "alice",
            Visibility::Public,
            None,
        )
        .unwrap();
    state
        .auth_db
        .add_dataset_graph(DATASET, DATA_GRAPH)
        .unwrap();
    state
        .auth_db
        .grant_graph_permission("alice-w", DATA_GRAPH, "user", "alice", "write", "adm")
        .unwrap();
    (state, admin, alice, mallory)
}

async fn create_shape_graph(state: &AppState, token: &str, ttl: &str) -> String {
    let (st, body) = json_req(
        state,
        Method::POST,
        "/api/shacl/shape-graphs",
        token,
        json!({ "name": "shapes", "visibility": "private", "turtle": ttl }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "create shape graph: {body}");
    id_of(&body)
}

fn id_of(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap();
    v["id"].as_str().unwrap().to_string()
}

async fn create_pipeline(state: &AppState, token: &str, body: Value) -> (StatusCode, String) {
    json_req(state, Method::POST, "/api/shacl/pipelines", token, body).await
}

/// The four ways a pipeline's scope can cover [`DATA_GRAPH`] for the gate.
fn gating_scopes(sg: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "a graph target",
            json!({ "name": "gate", "gate_writes": true, "shape_graph_ids": [sg],
                    "targets": [{ "kind": "graph", "id": DATA_GRAPH }] }),
        ),
        (
            "a dataset target",
            json!({ "name": "gate", "gate_writes": true, "shape_graph_ids": [sg],
                    "targets": [{ "kind": "dataset", "id": DATASET }] }),
        ),
        (
            "legacy graph_iris",
            json!({ "name": "gate", "gate_writes": true, "shape_graph_ids": [sg],
                    "graph_iris": [DATA_GRAPH] }),
        ),
        (
            "legacy dataset_ids",
            json!({ "name": "gate", "gate_writes": true, "shape_graph_ids": [sg],
                    "dataset_ids": [DATASET] }),
        ),
    ]
}

fn graph_len(state: &AppState) -> usize {
    state.store.count_graph(Some(DATA_GRAPH)).unwrap()
}

// ─── Creating and updating a gate ────────────────────────────────────────────

/// Mallory may read alice's public dataset, so she may validate it — but not
/// gate it: a pipeline of hers with shapes that reject everything would
/// otherwise refuse every write alice makes to her own graph.
#[tokio::test]
async fn a_reader_may_not_gate_writes_to_a_graph_it_may_not_write() {
    let (state, _admin, alice, mallory) = setup();
    let sg = create_shape_graph(&state, &mallory, BLOCKING_SHAPES).await;

    for (how, body) in gating_scopes(&sg) {
        let (st, created) = create_pipeline(&state, &mallory, body).await;

        // The owner's write is what the gate would take away.
        let (put, put_body) = put_data(&state, &alice, NAMED).await;
        assert!(
            put.is_success(),
            "a reader's pipeline over {how} must not refuse the owner's write: {put} {put_body}"
        );
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "gating {how} needs write access to it, not read: {created}"
        );
        assert!(
            created.contains("Write access denied"),
            "the refusal says what is missing: {created}"
        );
    }
    assert_eq!(graph_len(&state), 2, "alice's data landed");
}

/// Read access still suffices for a pipeline that only validates; switching
/// its gate on is the step that needs write access.
#[tokio::test]
async fn a_reader_may_validate_but_not_switch_the_gate_on() {
    let (state, _admin, alice, mallory) = setup();
    let sg = create_shape_graph(&state, &mallory, BLOCKING_SHAPES).await;
    let body = json!({ "name": "check", "shape_graph_ids": [sg],
                       "targets": [{ "kind": "dataset", "id": DATASET }] });
    let (st, created) = create_pipeline(&state, &mallory, body.clone()).await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "a validation-only pipeline needs read access alone: {created}"
    );
    let id = id_of(&created);

    let mut gating = body;
    gating["gate_writes"] = json!(true);
    let (st, updated) = json_req(
        &state,
        Method::PUT,
        &format!("/api/shacl/pipelines/{id}"),
        &mallory,
        gating,
    )
    .await;
    let (put, put_body) = put_data(&state, &alice, NAMED).await;
    assert!(
        put.is_success(),
        "the owner's write lands: {put} {put_body}"
    );
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "switching the gate on needs write access: {updated}"
    );
}

/// A gate over a dataset that does not exist is refused, as a binding to one
/// is: its id could otherwise be claimed for someone else's dataset later.
#[tokio::test]
async fn a_gate_over_a_missing_dataset_is_refused() {
    let (state, _admin, _alice, mallory) = setup();
    let sg = create_shape_graph(&state, &mallory, BLOCKING_SHAPES).await;
    let (st, body) = create_pipeline(
        &state,
        &mallory,
        json!({ "name": "gate", "gate_writes": true, "shape_graph_ids": [sg],
                "targets": [{ "kind": "dataset", "id": "not-yet" }] }),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{body}");
}

/// Whoever may write a graph or dataset still gates it: its owner (dataset
/// target), a graph-ACL writer (graph target), an admin (anything).
#[tokio::test]
async fn a_writer_may_gate_what_it_may_write() {
    let (state, admin, alice, _mallory) = setup();
    let erin = make_user(&state, "erin");
    state
        .auth_db
        .grant_graph_permission("erin-w", DATA_GRAPH, "user", "erin", "write", "adm")
        .unwrap();

    for (who, token) in [("alice", &alice), ("erin", &erin), ("admin", &admin)] {
        let sg = create_shape_graph(&state, token, PERSON_SHAPES).await;
        for (how, body) in gating_scopes(&sg) {
            // erin writes the graph, not the dataset: only graph scopes.
            if who == "erin" && how.contains("dataset") {
                let (st, created) = create_pipeline(&state, token, body).await;
                assert_eq!(st, StatusCode::FORBIDDEN, "{who} over {how}: {created}");
                continue;
            }
            let (st, created) = create_pipeline(&state, token, body).await;
            assert_eq!(st, StatusCode::CREATED, "{who} over {how}: {created}");
            let id = id_of(&created);

            // The gate is live: an unnamed person is refused, even to alice.
            let (put, put_body) = put_data(&state, &alice, UNNAMED).await;
            assert_eq!(
                put,
                StatusCode::UNPROCESSABLE_ENTITY,
                "{who}'s gate over {how} gates: {put_body}"
            );
            let (put, put_body) = put_data(&state, &alice, NAMED).await;
            assert!(put.is_success(), "conforming data lands: {put_body}");

            let (st, _) = send(
                &state,
                Method::DELETE,
                &format!("/api/shacl/pipelines/{id}"),
                token,
                "application/json",
                String::new(),
            )
            .await;
            assert!(st.is_success(), "delete {who}'s pipeline");
        }
    }
}

// ─── The gate runs with its creator's authority ──────────────────────────────

fn studio(state: &AppState) -> ShaclStudioStore {
    ShaclStudioStore::new(state.auth_db.pool())
}

/// A gating pipeline as one stored before its author's write access was
/// checked.
fn stored_gate(id: &str, creator: &str, target: ValidationTarget, sg: &str) -> ValidationPipeline {
    let now = chrono::Utc::now().to_rfc3339();
    ValidationPipeline {
        id: id.into(),
        name: id.into(),
        description: None,
        owner_type: OwnerType::User,
        owner_id: creator.into(),
        visibility: Visibility::Private,
        targets: vec![target],
        dataset_ids: vec![],
        graph_iris: vec![],
        target_classes: vec![],
        shape_graph_ids: vec![sg.into()],
        severity_threshold: SeverityThreshold::Violation,
        run_inference: false,
        max_results: None,
        trigger_on_write: false,
        schedule_cron: None,
        gate_writes: true,
        retention: 10,
        inferred_target: WriteTarget::InPlace,
        inferred_target_graph: None,
        results_target: ResultsTarget::None,
        results_target_graph: None,
        last_run_at: None,
        last_conforms: None,
        created_by: Some(creator.into()),
        created_at: now.clone(),
        updated_at: now,
    }
}

/// Gates a reader stored before the check existed refuse nothing: the gate
/// asks, at every write, whether its creator may write what it gates.
#[tokio::test]
async fn a_gate_stored_by_a_reader_before_the_check_gates_nothing() {
    let (state, _admin, alice, mallory) = setup();
    let sg = create_shape_graph(&state, &mallory, BLOCKING_SHAPES).await;
    for (id, target) in [
        (
            "old-graph-gate",
            ValidationTarget {
                kind: TargetKind::Graph,
                id: DATA_GRAPH.into(),
            },
        ),
        (
            "old-dataset-gate",
            ValidationTarget {
                kind: TargetKind::Dataset,
                id: DATASET.into(),
            },
        ),
    ] {
        studio(&state)
            .insert_pipeline(&stored_gate(id, "mallory", target, &sg))
            .unwrap();
    }

    let (put, body) = put_data(&state, &alice, NAMED).await;
    assert!(
        put.is_success(),
        "a gate its creator may not write refuses nothing: {put} {body}"
    );
    assert_eq!(graph_len(&state), 2);
}

/// A gate made by someone who could write the graph stops gating once they no
/// longer can — a revoked grant, a deactivated account — as a pipeline's
/// derived writes do (`exec::owner_can_write`).
#[tokio::test]
async fn a_gate_stops_gating_when_its_creator_loses_write_access() {
    let (state, admin, alice, _mallory) = setup();
    let erin = make_user(&state, "erin");
    state
        .auth_db
        .grant_graph_permission("erin-w", DATA_GRAPH, "user", "erin", "write", "adm")
        .unwrap();
    let sg = create_shape_graph(&state, &erin, PERSON_SHAPES).await;
    let (st, created) = create_pipeline(
        &state,
        &erin,
        json!({ "name": "gate", "gate_writes": true, "shape_graph_ids": [sg],
                "targets": [{ "kind": "graph", "id": DATA_GRAPH }] }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");

    let (put, body) = put_data(&state, &alice, UNNAMED).await;
    assert_eq!(
        put,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the gate is live: {body}"
    );

    state.auth_db.revoke_graph_permission("erin-w").unwrap();
    let (put, body) = put_data(&state, &alice, UNNAMED).await;
    assert!(
        put.is_success(),
        "a revoked grant gates nothing: {put} {body}"
    );

    // Alice's own gate over her dataset holds while she is active…
    let sg = create_shape_graph(&state, &alice, PERSON_SHAPES).await;
    let (st, created) = create_pipeline(
        &state,
        &alice,
        json!({ "name": "gate", "gate_writes": true, "shape_graph_ids": [sg],
                "targets": [{ "kind": "dataset", "id": DATASET }] }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    let (put, body) = put_data(&state, &alice, UNNAMED).await;
    assert_eq!(
        put,
        StatusCode::UNPROCESSABLE_ENTITY,
        "the gate is live: {body}"
    );

    // …and not once her account is deactivated (an admin writes now).
    state.auth_db.set_user_active("alice", false).unwrap();
    let (put, body) = put_data(&state, &admin, UNNAMED).await;
    assert!(
        put.is_success(),
        "a deactivated creator's gate gates nothing: {put} {body}"
    );
}
