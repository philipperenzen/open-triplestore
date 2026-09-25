//! SHACL Studio and the model registry's write guard, through the real router.
//!
//! A seed bundle binds a model's graph as a Studio shape graph in place: the
//! nen2660-imbor bundle binds CROW's IMBOR Kern, whose licence allows no
//! altered copies. The Studio applies the registry's rules to such a graph,
//! for admins too:
//!
//! * no save, restore, import or delete-clear writes a no-derivatives graph
//!   (403), and delete drops only the Library's rows of an adopted graph;
//! * no clone or import copies it into an editable graph (403);
//! * the Studio serves it only while it is a checked, unchanged copy, and its
//!   stored revisions only to those who may write the entry;
//! * a pipeline never infers into it or writes a report into it;
//! * a write into any other attributed version is allowed, and its licence
//!   record stops calling the content unchanged first.

mod common;

use std::borrow::Cow;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{GraphKind, OwnerType, SystemRole, Visibility};
use open_triplestore::data_models::{content_digest, registry, vocab_files};
use open_triplestore::seed_bundles::{
    apply_bundle, Bundle, BundleDataModel, BundleDataset, BundleGraph, BundleLicense, Fmt, OrgSpec,
};
use open_triplestore::server::AppState;
use open_triplestore::shacl_studio::models::{
    ResultsTarget, SeverityThreshold, ValidationPipeline, WriteTarget,
};
use open_triplestore::shacl_studio::store::ShaclStudioStore;
use serde_json::Value;
use tower::ServiceExt as _;

/// The no-derivatives model's graph (bound in place as a shape graph).
const ND_GRAPH: &str = "https://example.org/otl/def/";
/// A CC BY model's graph (bound in place as a shape graph too).
const BY_GRAPH: &str = "https://example.org/open/def/";

const ND_TTL: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
    <https://example.org/otl/def/Boom> a rdfs:Class, sh:NodeShape ;\n\
      rdfs:label \"Boom\"@nl ;\n\
      sh:property [ sh:path <https://example.org/otl/def/hoogte> ; sh:maxCount 1 ] .\n";

const BY_TTL: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
    <https://example.org/open/def/Mast> a rdfs:Class, sh:NodeShape ;\n\
      rdfs:label \"Mast\"@nl ;\n\
      sh:property [ sh:path <https://example.org/open/def/hoogte> ; sh:maxCount 1 ] .\n";

/// A SHACL-AF rule that tags every `rdfs:Class`: it fires on both models.
const RULE_TTL: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
    <urn:test:RuleShape> a sh:NodeShape ;\n\
      sh:targetClass <http://www.w3.org/2000/01/rdf-schema#Class> ;\n\
      sh:rule [ a sh:TripleRule ; sh:subject sh:this ;\n\
        sh:predicate <urn:test:tagged> ; sh:object \"inferred\" ] .\n";

fn licence(no_derivatives: bool, file: &str) -> BundleLicense {
    BundleLicense {
        licenses: vec![(
            "CC BY 4.0".into(),
            "https://creativecommons.org/licenses/by/4.0/".into(),
        )],
        copyright: vec!["© Example Rights Holder".into()],
        source: "https://example.org/release".into(),
        notice: None,
        changes: None,
        remarks: None,
        notice_url: "https://example.org/release".into(),
        no_derivatives,
        files: vec![file.into()],
    }
}

fn model(id: &str, graph: &str, ttl: &'static str, lic: BundleLicense) -> BundleDataModel {
    BundleDataModel {
        id: id.into(),
        title: id.into(),
        namespace: graph.into(),
        description: None,
        kind: open_triplestore::kind_detector::RegistryKind::DataModel,
        version: "2025".into(),
        graphs: vec![BundleGraph {
            iri: graph.into(),
            role: Some(GraphKind::Model),
            data: Some((Cow::Borrowed(ttl), Fmt::Turtle)),
        }],
        license: Some(lic),
        public: true,
    }
}

/// A bundle like nen2660-imbor: a no-derivatives model and a CC BY model,
/// both bound in place as the public sample dataset's shape graphs.
fn bundle() -> Bundle {
    Bundle {
        id: "guarded".into(),
        opt_out_env: None,
        org: OrgSpec {
            slug: "guarded-org".into(),
            name: "Guarded".into(),
            description: None,
        },
        datasets: vec![BundleDataset {
            slug: "guarded-sample".into(),
            name: "Guarded sample".into(),
            description: None,
            visibility: Visibility::Public,
            graphs: vec![BundleGraph {
                iri: "https://example.org/guarded/instances".into(),
                role: Some(GraphKind::Instances),
                data: Some((
                    Cow::Borrowed(
                        "<https://example.org/guarded/b1> a <https://example.org/otl/def/Boom> .",
                    ),
                    Fmt::Turtle,
                )),
            }],
            quads: vec![],
            saved_queries: vec![],
            conforms_to: None,
            shape_graphs: vec![ND_GRAPH.into(), BY_GRAPH.into()],
        }],
        prefixes: Default::default(),
        data_models: vec![
            model("nd-otl", ND_GRAPH, ND_TTL, licence(true, "otl.ttl")),
            model("open-otl", BY_GRAPH, BY_TTL, licence(false, "open.ttl")),
        ],
    }
}

/// `(state, admin token, member token)`: the bundle applied, and a plain user.
fn setup() -> (AppState, String, String) {
    let (state, admin) = admin_state();
    apply_bundle(&state, &bundle()).unwrap();
    state
        .auth_db
        .create_user("u1", "u1", "u1@test.com", "hash", SystemRole::User)
        .unwrap();
    let user = mint_token("u1", "u1", "user");
    (state, admin, user)
}

fn studio(state: &AppState) -> ShaclStudioStore {
    ShaclStudioStore::new(state.auth_db.pool())
}

fn set_id(state: &AppState, graph: &str) -> String {
    studio(state)
        .get_shape_graph_by_iri(graph)
        .unwrap()
        .expect("the bundle bound the graph as a shape graph")
        .id
}

fn digest(state: &AppState, graph: &str) -> String {
    content_digest::triples_digest(&content_digest::graph_triples(&state.store, graph).unwrap())
}

fn attribution(
    state: &AppState,
    id: &str,
) -> open_triplestore::data_models::models::ContentAttribution {
    registry::get_attribution(
        &state.store,
        &registry::version_record_iri(&state.base_url, id, "2025"),
    )
    .unwrap()
}

async fn send(
    state: &AppState,
    method: Method,
    uri: &str,
    token: Option<&str>,
    content_type: &str,
    body: &str,
) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = test_app(state.clone())
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn json(
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
        Some(token),
        "application/json",
        &body.to_string(),
    )
    .await
}

fn ask(state: &AppState, q: &str) -> bool {
    matches!(
        state.store.query(q),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    )
}

/// Save, restore and import into the bound no-derivatives graph are refused
/// for an admin; the graph and its record stay as they were, and the version
/// is still served as the unchanged copy. Deleting the Library entry drops
/// only its rows: the graph keeps its triples.
#[tokio::test]
async fn studio_writes_into_a_no_derivatives_model_graph_are_refused() {
    let (state, admin, _) = setup();
    let id = set_id(&state, ND_GRAPH);
    let before = digest(&state, ND_GRAPH);
    assert!(attribution(&state, "nd-otl").unchanged);

    let (status, body) = send(
        &state,
        Method::PUT,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        Some(&admin),
        "text/turtle",
        "<urn:x> <urn:y> \"edited\" .",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("allows no altered copies"), "{body}");

    let (status, body) = send(
        &state,
        Method::POST,
        &format!("/api/shacl/shape-graphs/{id}/restore/1"),
        Some(&admin),
        "application/json",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let by = set_id(&state, BY_GRAPH);
    let (status, body) = json(
        &state,
        Method::POST,
        &format!("/api/shacl/shape-graphs/{id}/import-shapes"),
        &admin,
        serde_json::json!({ "shapes": [
            { "source_graph": BY_GRAPH, "shape": "https://example.org/open/def/Mast" }
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(!by.is_empty());

    assert_eq!(digest(&state, ND_GRAPH), before, "the graph is untouched");
    let a = attribution(&state, "nd-otl");
    assert!(a.unchanged && a.no_derivatives, "{a:?}");
    let (status, _) = send(
        &state,
        Method::GET,
        "/api/models/nd-otl/versions/2025/data?format=nt",
        None,
        "text/plain",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Delete: the Library entry goes, the adopted graph keeps its data.
    let (status, body) = send(
        &state,
        Method::DELETE,
        &format!("/api/shacl/shape-graphs/{id}"),
        Some(&admin),
        "application/json",
        "",
    )
    .await;
    assert!(status.is_success(), "{status}: {body}");
    assert!(studio(&state).get_shape_graph(&id).unwrap().is_none());
    assert_eq!(digest(&state, ND_GRAPH), before, "delete never clears it");
    assert!(attribution(&state, "nd-otl").unchanged);
}

/// A clone of the bound no-derivatives shape graph, or an import of its
/// shapes into a graph of one's own, would be an editable copy: refused.
#[tokio::test]
async fn copies_of_a_no_derivatives_model_graph_are_refused() {
    let (state, admin, user) = setup();
    let id = set_id(&state, ND_GRAPH);

    for token in [&admin, &user] {
        let (status, body) = json(
            &state,
            Method::POST,
            &format!("/api/shacl/shape-graphs/{id}/clone"),
            token,
            serde_json::json!({ "name": "mine" }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert!(body.contains("allows no altered copies"), "{body}");
    }

    let (status, body) = json(
        &state,
        Method::POST,
        "/api/shacl/shape-graphs",
        &admin,
        serde_json::json!({ "name": "own" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let own: Value = serde_json::from_str(&body).unwrap();
    let own_id = own["id"].as_str().unwrap().to_string();
    let own_graph = own["graph_iri"].as_str().unwrap().to_string();
    let (status, body) = json(
        &state,
        Method::POST,
        &format!("/api/shacl/shape-graphs/{own_id}/import-shapes"),
        &admin,
        serde_json::json!({ "shapes": [
            { "source_graph": ND_GRAPH, "shape": "https://example.org/otl/def/Boom" }
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(!ask(
        &state,
        &format!("ASK {{ GRAPH <{own_graph}> {{ <https://example.org/otl/def/Boom> ?p ?o }} }}")
    ));

    // Shapes of the CC BY model may be imported.
    let (status, body) = json(
        &state,
        Method::POST,
        &format!("/api/shacl/shape-graphs/{own_id}/import-shapes"),
        &admin,
        serde_json::json!({ "shapes": [
            { "source_graph": BY_GRAPH, "shape": "https://example.org/open/def/Mast" }
        ]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// A Studio save into the bound CC BY model's graph is allowed; its licence
/// record stops calling the copy unchanged first. The no-derivatives model's
/// record is left alone.
#[tokio::test]
async fn a_studio_write_into_an_attributed_copy_marks_its_record() {
    let (state, admin, _) = setup();
    let id = set_id(&state, BY_GRAPH);
    assert!(attribution(&state, "open-otl").unchanged);
    let edited = format!("{BY_TTL}<https://example.org/open/def/Mast> <http://www.w3.org/2000/01/rdf-schema#comment> \"edited\" .\n");
    let (status, body) = send(
        &state,
        Method::PUT,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        Some(&admin),
        "text/turtle",
        &edited,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let a = attribution(&state, "open-otl");
    assert!(!a.unchanged);
    assert!(
        a.stored_copy.contains("written directly"),
        "{}",
        a.stored_copy
    );
    assert!(attribution(&state, "nd-otl").unchanged);

    // Delete keeps the adopted graph's data here too.
    let before = digest(&state, BY_GRAPH);
    let (status, _) = send(
        &state,
        Method::DELETE,
        &format!("/api/shacl/shape-graphs/{id}"),
        Some(&admin),
        "application/json",
        "",
    )
    .await;
    assert!(status.is_success());
    assert_eq!(digest(&state, BY_GRAPH), before);
}

/// The Studio serves the bound no-derivatives graph to every user who may
/// read the shape graph only while its record calls it a checked, unchanged
/// copy; otherwise only to those who may write the registry entry. A stored
/// revision of it, which no check vouches for, goes only to those users.
#[tokio::test]
async fn a_no_derivatives_graph_is_served_by_the_studio_only_while_unchanged() {
    let (state, admin, user) = setup();
    let id = set_id(&state, ND_GRAPH);
    let turtle = format!("/api/shacl/shape-graphs/{id}/turtle");
    let revision = format!("/api/shacl/shape-graphs/{id}/revisions/1");

    let (status, body) = send(&state, Method::GET, &turtle, Some(&user), "text/turtle", "").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("Boom"));
    let (status, body) = send(
        &state,
        Method::GET,
        &revision,
        Some(&user),
        "application/json",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, _) = send(
        &state,
        Method::GET,
        &revision,
        Some(&admin),
        "application/json",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The copy is altered outside the registry (an admin's wildcard update)
    // and the re-check marked it.
    registry::mark_possibly_modified(
        &state.store,
        &registry::version_record_iri(&state.base_url, "nd-otl", "2025"),
        vocab_files::written_stored_copy,
    )
    .unwrap();
    let (status, body) = send(&state, Method::GET, &turtle, Some(&user), "text/turtle", "").await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(body.contains("withheld"), "{body}");
    let (status, _) = send(
        &state,
        Method::GET,
        &format!("{turtle}?format=shaclc"),
        Some(&user),
        "text/turtle",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = send(
        &state,
        Method::GET,
        &turtle,
        Some(&admin),
        "text/turtle",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // The CC BY graph is served whatever its record says.
    let by = set_id(&state, BY_GRAPH);
    let (status, _) = send(
        &state,
        Method::GET,
        &format!("/api/shacl/shape-graphs/{by}/revisions/1"),
        Some(&user),
        "application/json",
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

fn pipeline(
    state: &AppState,
    graph: &str,
    rules: &str,
    results_graph: Option<&str>,
) -> ValidationPipeline {
    let now = chrono::Utc::now().to_rfc3339();
    let p = ValidationPipeline {
        id: uuid::Uuid::new_v4().to_string(),
        name: "p".into(),
        description: None,
        owner_type: OwnerType::User,
        owner_id: "adm".into(),
        visibility: Visibility::Private,
        targets: vec![],
        dataset_ids: vec![],
        graph_iris: vec![graph.to_string()],
        target_classes: vec![],
        shape_graph_ids: vec![rules.to_string()],
        severity_threshold: SeverityThreshold::Violation,
        run_inference: true,
        max_results: None,
        inferred_target: WriteTarget::InPlace,
        inferred_target_graph: None,
        results_target: if results_graph.is_some() {
            ResultsTarget::NewGraph
        } else {
            ResultsTarget::None
        },
        results_target_graph: results_graph.map(str::to_string),
        trigger_on_write: false,
        schedule_cron: None,
        gate_writes: false,
        retention: 10,
        last_run_at: None,
        last_conforms: None,
        created_by: Some("adm".into()),
        created_at: now.clone(),
        updated_at: now,
    };
    studio(state).insert_pipeline(&p).unwrap();
    p
}

fn run(state: &AppState, p: &ValidationPipeline) {
    open_triplestore::shacl_studio::exec::execute_pipeline(
        &state.store,
        &state.auth_db,
        &studio(state),
        &state.base_url,
        p,
        "manual",
        Some("adm"),
    )
    .unwrap();
}

/// A pipeline cannot be set up to write the no-derivatives graph (in-place
/// inference over it, or its report into it), not even by an admin; and a
/// pipeline stored with such a scope never writes it when it runs. The same
/// pipeline over the CC BY graph infers into it and marks its record.
#[tokio::test]
async fn pipelines_never_write_a_no_derivatives_model_graph() {
    let (state, admin, _) = setup();
    let (status, body) = json(
        &state,
        Method::POST,
        "/api/shacl/shape-graphs",
        &admin,
        serde_json::json!({ "name": "rules", "turtle": RULE_TTL }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let rules: Value = serde_json::from_str(&body).unwrap();
    let rules = rules["id"].as_str().unwrap().to_string();

    for body in [
        serde_json::json!({ "name": "infer", "run_inference": true,
            "targets": [{ "kind": "graph", "id": ND_GRAPH }], "shape_graph_ids": [rules] }),
        serde_json::json!({ "name": "report", "results_target": "new_graph",
            "results_target_graph": ND_GRAPH,
            "targets": [{ "kind": "graph", "id": BY_GRAPH }], "shape_graph_ids": [rules] }),
    ] {
        let (status, text) = json(&state, Method::POST, "/api/shacl/pipelines", &admin, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
        assert!(text.contains("allows no altered copies"), "{text}");
    }

    // Stored before this build (no check at set-up): the run does not write.
    let before = digest(&state, ND_GRAPH);
    run(&state, &pipeline(&state, ND_GRAPH, &rules, None));
    run(&state, &pipeline(&state, BY_GRAPH, &rules, Some(ND_GRAPH)));
    assert_eq!(digest(&state, ND_GRAPH), before, "no inference, no report");
    assert!(attribution(&state, "nd-otl").unchanged);

    // The rule does fire: over the CC BY graph it infers in place, and that
    // version's record stopped calling the copy unchanged before it ran.
    assert!(ask(
        &state,
        &format!("ASK {{ GRAPH <{BY_GRAPH}> {{ ?c <urn:test:tagged> \"inferred\" }} }}")
    ));
    assert!(!attribution(&state, "open-otl").unchanged);
}
