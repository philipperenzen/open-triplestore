//! Dataset graph attach, detach and delete never reach the model registry's
//! graphs, through the real router:
//!
//! * detaching a graph a dataset never registered is refused (404) and deletes
//!   nothing — not the registry graph, not IMBOR, not a copy the seeder keeps
//!   aside, not another user's private model version, not a seed bundle's
//!   graphs;
//! * none of those graphs can be registered to a dataset, set as its shapes
//!   graph or written by its RML mapping, by a user or an admin (403);
//! * a registration made before that refusal existed only loses its row on a
//!   detach, a dataset delete or an organisation delete: the stored graph
//!   stays;
//! * a one-time boot cleanup releases those old registrations, keeping a
//!   shapes-role graph bound for validation;
//! * a dataset still deletes its own graphs, and keeps a graph another dataset
//!   still uses.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::dataset_graph;
use open_triplestore::auth::middleware::AuthenticatedUser;
use open_triplestore::auth::models::{GraphKind, OwnerType, Role, SystemRole, Visibility};
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::{content_digest, registry, seed_vocab};
use open_triplestore::server::AppState;
use open_triplestore::store::TripleStore;
use tower::ServiceExt as _;

const IMBOR_TERM: &str = "https://data.crow.nl/imbor/term/74e825e1-9b93-4dd5-8fab-52783fdb758b";
const BUNDLE_GRAPH: &str = "https://bundle.example/def";
const BUNDLE_SUB_GRAPH: &str = "https://bundle.example/shapes";

async fn send(state: &AppState, req: Request<Body>) -> (StatusCode, String) {
    let resp = test_app(state.clone()).oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

fn json(method: Method, uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn delete(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn rml_execute(dataset: &str, graph: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!(
            "/api/datasets/{dataset}/mappings/execute?graph={}",
            url_encode(graph)
        ))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=X")
        .body(Body::from("--X--\r\n"))
        .unwrap()
}

fn make_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

fn make_dataset(state: &AppState, id: &str, owner_type: OwnerType, owner: &str) {
    state
        .auth_db
        .create_dataset(id, id, None, owner_type, owner, Visibility::Private, None)
        .unwrap();
}

fn put_triple(store: &TripleStore, graph: &str, label: &str) {
    store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{graph}> {{ <http://ex.org/s> <http://ex.org/p> \"{label}\" }} }}"
        ))
        .unwrap();
}

fn triple_count(state: &AppState, graph: &str) -> usize {
    content_digest::graph_triples(&state.store, graph)
        .unwrap()
        .len()
}

fn digest(state: &AppState, graph: &str) -> String {
    content_digest::triples_digest(&content_digest::graph_triples(&state.store, graph).unwrap())
}

/// A registry entry `id` with one published version `version` whose base
/// graph is `graph` and whose sub-graphs are `sub_graphs`, holding a triple.
fn add_model(
    state: &AppState,
    id: &str,
    owner: &str,
    is_public: bool,
    version: &str,
    graph: &str,
    sub_graphs: &[&str],
) {
    registry::insert_data_model(
        &state.store,
        &state.base_url,
        id,
        id,
        &format!("http://{id}.example/ns#"),
        None,
        is_public,
        Some("user"),
        Some(owner),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    registry::insert_version(
        &state.store,
        &state.base_url,
        &DataModelVersion {
            data_model_id: id.to_string(),
            version: version.to_string(),
            status: VersionStatus::Published,
            graph_iri: graph.to_string(),
            sub_graphs: sub_graphs.iter().map(|s| s.to_string()).collect(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: Vec::new(),
        },
    )
    .unwrap();
    put_triple(&state.store, graph, id);
    for g in sub_graphs {
        put_triple(&state.store, g, id);
    }
}

/// The model registry's graphs a dataset must never claim, write or delete,
/// with each one's content digest: the registry graph itself; IMBOR 2025 and
/// the copy of it the seeder kept aside after an edit (made the way the
/// seeder makes it: an unscoped admin write, then the next start); another
/// user's private model version; and the graphs a seed-bundle-like version
/// names outside `{base}/data-model/`.
struct Fixture {
    state: AppState,
    admin: String,
    user: String,
    protected: Vec<(String, String)>,
}

async fn fixture() -> Fixture {
    let (state, admin) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);
    let base = state.base_url.to_string();

    let (status, body) = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/sparql")
            .header(header::CONTENT_TYPE, "application/sparql-update")
            .header(header::AUTHORIZATION, format!("Bearer {admin}"))
            .body(Body::from(format!(
                "INSERT {{ GRAPH ?g {{ <{IMBOR_TERM}> <http://www.w3.org/2000/01/rdf-schema#comment> \"edited\" }} }} \
                 WHERE {{ GRAPH ?g {{ <{IMBOR_TERM}> a ?t }} }}"
            )))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    seed_vocab::seed_standard_vocabularies(&state);
    let kept = registry::get_version(&state.store, &base, "imbor", "2025-kept-1")
        .expect("the seeder kept the edited IMBOR copy aside")
        .graph_iri;

    make_user(&state, "victim");
    let victim_graph = registry::version_record_iri(&base, "victim-model", "1.0.0");
    add_model(
        &state,
        "victim-model",
        "victim",
        false,
        "1.0.0",
        &victim_graph,
        &[],
    );
    add_model(
        &state,
        "bundle-model",
        "adm",
        true,
        "1.0",
        BUNDLE_GRAPH,
        &[BUNDLE_SUB_GRAPH],
    );

    let user = make_user(&state, "attacker");
    make_dataset(&state, "mine", OwnerType::User, "attacker");

    let graphs = [
        registry::REGISTRY_GRAPH.to_string(),
        registry::version_record_iri(&base, "imbor", "2025"),
        kept,
        victim_graph,
        BUNDLE_GRAPH.to_string(),
        BUNDLE_SUB_GRAPH.to_string(),
    ];
    let protected = graphs
        .iter()
        .map(|g| {
            assert!(triple_count(&state, g) > 0, "<{g}> holds content");
            (g.clone(), digest(&state, g))
        })
        .collect();
    Fixture {
        state,
        admin,
        user,
        protected,
    }
}

impl Fixture {
    fn assert_untouched(&self, when: &str) {
        for (g, before) in &self.protected {
            assert_eq!(
                &digest(&self.state, g),
                before,
                "{when}: <{g}> is untouched"
            );
        }
    }
}

/// `DELETE /api/datasets/{mine}/graphs` naming a graph the dataset never
/// registered deleted any graph no other dataset claimed. It now answers 404
/// and deletes nothing, for a user and an admin alike.
#[tokio::test]
async fn detaching_a_graph_the_dataset_never_registered_deletes_nothing() {
    let f = fixture().await;
    for (g, _) in &f.protected {
        for token in [&f.user, &f.admin] {
            let (status, body) = send(
                &f.state,
                json(
                    Method::DELETE,
                    "/api/datasets/mine/graphs",
                    token,
                    serde_json::json!({ "graph_iri": g }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::NOT_FOUND, "<{g}>: {body}");
        }
    }
    f.assert_untouched("after the detach attempts");
    // The IMBOR copy is still served as the checked, unchanged file.
    let (status, _) = send(
        &f.state,
        Request::builder()
            .uri("/api/models/imbor/versions/2025/data?format=nt")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// No model-registry graph can become a graph of a dataset — registered, set
/// as its shapes graph or named as its RML output — by a user or an admin.
#[tokio::test]
async fn registry_graphs_cannot_be_attached_to_a_dataset_by_anyone() {
    let f = fixture().await;
    for (g, _) in &f.protected {
        for token in [&f.user, &f.admin] {
            let (status, body) = send(
                &f.state,
                json(
                    Method::POST,
                    "/api/datasets/mine/graphs",
                    token,
                    serde_json::json!({ "graph_iri": g }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "register <{g}>: {body}");

            let (status, body) = send(
                &f.state,
                json(
                    Method::PUT,
                    "/api/datasets/mine/shacl",
                    token,
                    serde_json::json!({ "shacl_on_write": false, "shapes_graph_iri": g }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::FORBIDDEN, "shapes <{g}>: {body}");

            let (status, body) = send(&f.state, rml_execute("mine", g, token)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "RML into <{g}>: {body}");
        }
    }
    assert!(f
        .state
        .auth_db
        .list_dataset_graphs("mine")
        .unwrap()
        .is_empty());
    let ds = f.state.auth_db.get_dataset("mine").unwrap().unwrap();
    assert!(ds.shapes_graph_iri.is_none());
    f.assert_untouched("after the attach attempts");

    // The refusal says what the graph is, not that it belongs to a dataset.
    let (_, body) = send(
        &f.state,
        json(
            Method::POST,
            "/api/datasets/mine/graphs",
            &f.admin,
            serde_json::json!({ "graph_iri": BUNDLE_GRAPH }),
        ),
    )
    .await;
    assert!(body.contains("belongs to the model registry"), "{body}");
}

/// Registrations made before registry graphs were refused (or through a path
/// that does not gate registration) only lose their row: a detach, a dataset
/// delete (its shapes graph included) and an organisation delete leave every
/// registry graph in the store, while the dataset's own graphs go.
#[tokio::test]
async fn existing_registrations_of_registry_graphs_never_delete_them() {
    let f = fixture().await;
    let base = f.state.base_url.to_string();
    let db = &f.state.auth_db;

    // Detach, one by one.
    for (g, _) in &f.protected {
        db.add_dataset_graph("mine", g).unwrap();
        let (status, body) = send(
            &f.state,
            json(
                Method::DELETE,
                "/api/datasets/mine/graphs",
                &f.user,
                serde_json::json!({ "graph_iri": g }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "<{g}>: {body}");
    }
    assert!(db.list_dataset_graphs("mine").unwrap().is_empty());
    f.assert_untouched("after detaching the old registrations");

    // Dataset delete: every registry graph registered, the kept copy as the
    // shapes graph, and a graph of the dataset's own.
    let own = format!("{base}/dataset/mine/instances");
    put_triple(&f.state.store, &own, "mine");
    db.add_dataset_graph("mine", &own).unwrap();
    for (g, _) in &f.protected {
        db.add_dataset_graph("mine", g).unwrap();
    }
    db.update_dataset_shacl("mine", false, Some(&f.protected[2].0))
        .unwrap();
    let (status, body) = send(&f.state, delete("/api/datasets/mine", &f.user)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert!(db.get_dataset("mine").unwrap().is_none());
    assert_eq!(
        triple_count(&f.state, &own),
        0,
        "the dataset's own graph goes"
    );
    f.assert_untouched("after the dataset delete");

    // Organisation delete, with the registry graph as a dataset's shapes graph.
    db.create_organisation("acme", "Acme", "acme", None, None)
        .unwrap();
    db.add_org_member("attacker", "acme", Role::Admin).unwrap();
    make_dataset(&f.state, "orgds", OwnerType::Organisation, "acme");
    let org_own = format!("{base}/dataset/orgds/instances");
    put_triple(&f.state.store, &org_own, "orgds");
    db.add_dataset_graph("orgds", &org_own).unwrap();
    for (g, _) in &f.protected {
        db.add_dataset_graph("orgds", g).unwrap();
    }
    db.update_dataset_shacl("orgds", false, Some(registry::REGISTRY_GRAPH))
        .unwrap();
    let (status, body) = send(&f.state, delete("/api/organisations/acme", &f.user)).await;
    assert!(status.is_success(), "{status}: {body}");
    assert!(db.get_dataset("orgds").unwrap().is_none());
    assert_eq!(triple_count(&f.state, &org_own), 0);
    f.assert_untouched("after the organisation delete");
}

/// The legitimate flows keep working: a dataset detaches and deletes its own
/// graphs (its validation-report graph included), a graph another dataset
/// still uses (registered, or as its shapes graph) stays until the last user
/// lets go of it.
#[tokio::test]
async fn a_dataset_still_deletes_its_own_graphs() {
    let (state, _admin) = admin_state();
    let base = state.base_url.to_string();
    let user = make_user(&state, "owner");
    make_dataset(&state, "mine", OwnerType::User, "owner");
    make_dataset(&state, "theirs", OwnerType::User, "owner");
    let db = &state.auth_db;

    // Register through the API, then detach: the graph goes. The external
    // graph is new, so the dataset creates it (a graph that already held data
    // would need the caller's direct write access).
    let own = format!("{base}/dataset/mine/g1");
    let external = "http://external.example/g";
    for g in [own.as_str(), external] {
        let (status, body) = send(
            &state,
            json(
                Method::POST,
                "/api/datasets/mine/graphs",
                &user,
                serde_json::json!({ "graph_iri": g }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "<{g}>: {body}");
        put_triple(&state.store, g, "data");
    }
    let (status, body) = send(
        &state,
        json(
            Method::DELETE,
            "/api/datasets/mine/graphs",
            &user,
            serde_json::json!({ "graph_iri": own }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        triple_count(&state, &own),
        0,
        "a detached own graph is deleted"
    );

    // A graph another dataset also registered stays on detach, and goes with
    // the last dataset that holds it — the one that created it.
    db.add_dataset_graph("theirs", external).unwrap();
    let (status, _) = send(
        &state,
        json(
            Method::DELETE,
            "/api/datasets/theirs/graphs",
            &user,
            serde_json::json!({ "graph_iri": external }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(triple_count(&state, external), 1, "still used by 'mine'");

    // A shapes graph two datasets share stays until the second is deleted
    // (its namespace's dataset is gone by then); the dataset's report graph
    // goes with it.
    let shapes = format!("{base}/dataset/theirs/shapes");
    put_triple(&state.store, &shapes, "shape");
    db.update_dataset_shacl("theirs", false, Some(&shapes))
        .unwrap();
    make_dataset(&state, "third", OwnerType::User, "owner");
    db.update_dataset_shacl("third", false, Some(&shapes))
        .unwrap();
    let reports = "urn:system:reports:dataset:theirs";
    put_triple(&state.store, reports, "report");
    db.add_dataset_graph("theirs", reports).unwrap();
    let (status, body) = send(&state, delete("/api/datasets/theirs", &user)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(
        triple_count(&state, reports),
        0,
        "its own report graph goes"
    );
    assert_eq!(triple_count(&state, &shapes), 1, "'third' still uses it");
    let (status, body) = send(&state, delete("/api/datasets/third", &user)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(triple_count(&state, &shapes), 0);

    // Another dataset's system graphs are never deleted with this one; the
    // external graph it created goes with it, as its last holder.
    let foreign_system = "urn:system:reports:dataset:elsewhere";
    put_triple(&state.store, foreign_system, "report");
    db.add_dataset_graph("mine", foreign_system).unwrap();
    let (status, body) = send(&state, delete("/api/datasets/mine", &user)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(triple_count(&state, foreign_system), 1);
    assert_eq!(triple_count(&state, external), 0, "the last holder took it");
}

/// The one-time boot cleanup: registrations of registry graphs made before
/// they were refused are released. The graphs stop being dataset-scoped for
/// reads, a shapes-role row stays bound for validation, the dataset's shapes
/// graph setting (which scopes no reads) is kept for its validation, other
/// registrations are untouched, and the sweep runs once.
#[tokio::test]
async fn the_boot_cleanup_releases_old_registrations_of_registry_graphs() {
    let f = fixture().await;
    let base = f.state.base_url.to_string();
    let db = &f.state.auth_db;
    let own = format!("{base}/dataset/mine/instances");
    db.add_dataset_graph("mine", &own).unwrap();
    for (g, _) in &f.protected {
        db.add_dataset_graph("mine", g).unwrap();
    }
    db.set_dataset_graph_role("mine", BUNDLE_SUB_GRAPH, Some(GraphKind::Shapes))
        .unwrap();
    db.update_dataset_shacl("mine", true, Some(BUNDLE_GRAPH))
        .unwrap();
    let (_, all_registered) = db.get_accessible_graph_iris(None).unwrap();
    assert!(all_registered.contains(BUNDLE_GRAPH));
    assert!(!dataset_graph::model_registry_claims_released(db));

    let released = dataset_graph::release_model_registry_claims(&f.state.store, db, &base).unwrap();
    assert_eq!(released, f.protected.len(), "every registry row");
    assert!(dataset_graph::model_registry_claims_released(db));
    assert_eq!(db.list_dataset_graphs("mine").unwrap(), vec![own.clone()]);
    let ds = db.get_dataset("mine").unwrap().unwrap();
    assert_eq!(ds.shapes_graph_iri.as_deref(), Some(BUNDLE_GRAPH));
    assert!(ds.shacl_on_write);
    let bound = open_triplestore::shacl_studio::bindings::bindings_for_target(
        &f.state.store,
        &open_triplestore::shacl_studio::bindings::dataset_target_iri(&base, "mine"),
    );
    assert!(bound.contains(&BUNDLE_SUB_GRAPH.to_string()), "{bound:?}");
    let (_, all_registered) = db.get_accessible_graph_iris(None).unwrap();
    assert!(!all_registered.contains(BUNDLE_GRAPH));
    f.assert_untouched("after the cleanup");

    // Once: a row made afterwards (by a path that bypasses the gate) stays for
    // the gate and the delete filter to deal with.
    db.add_dataset_graph("mine", BUNDLE_GRAPH).unwrap();
    assert_eq!(
        dataset_graph::release_model_registry_claims(&f.state.store, db, &base).unwrap(),
        0
    );
    assert!(db
        .list_dataset_graphs("mine")
        .unwrap()
        .contains(&BUNDLE_GRAPH.to_string()));
}

/// The registry lookup names a version's base graph and sub-graphs, and fails
/// closed on a name that is not an IRI.
#[test]
fn graph_held_by_version_names_base_and_sub_graphs_and_fails_closed() {
    let state = test_state();
    add_model(
        &state,
        "bundle-model",
        "someone",
        true,
        "1.0",
        BUNDLE_GRAPH,
        &[BUNDLE_SUB_GRAPH],
    );
    assert!(registry::graph_held_by_version(&state.store, BUNDLE_GRAPH));
    assert!(registry::graph_held_by_version(
        &state.store,
        BUNDLE_SUB_GRAPH
    ));
    assert!(!registry::graph_held_by_version(
        &state.store,
        "http://unrelated.example/g"
    ));
    assert!(registry::graph_held_by_version(&state.store, "not an iri"));
    assert!(registry::graph_held_by_version(&state.store, ""));
}

/// The gate every attach and dataset write path calls: registry graphs and
/// invalid names are refused for admins too; admins are otherwise
/// unrestricted and non-admins keep the namespace boundary.
#[test]
fn the_dataset_graph_gate_refuses_registry_graphs_for_admins_too() {
    let state = test_state();
    let base = state.base_url.to_string();
    add_model(
        &state,
        "bundle-model",
        "someone",
        true,
        "1.0",
        BUNDLE_GRAPH,
        &[BUNDLE_SUB_GRAPH],
    );
    make_user(&state, "u");
    make_dataset(&state, "mine", OwnerType::User, "u");
    let gate = |g: &str, admin: bool| {
        let user = AuthenticatedUser {
            user_id: if admin { "adm" } else { "u" }.to_string(),
            role: if admin {
                SystemRole::SuperAdmin
            } else {
                SystemRole::User
            },
            can_publish: admin,
            write_access: true,
            can_mint_api_tokens: true,
            scopes: Vec::new(),
        };
        dataset_graph::gate_dataset_graph_target(
            &state.store,
            &state.auth_db,
            &base,
            "mine",
            g,
            &user,
        )
    };
    for admin in [false, true] {
        for g in [
            registry::REGISTRY_GRAPH.to_string(),
            format!("{base}/data-model/anything/version/1.0.0"),
            format!("{base}/data-model/imbor/version/2025-kept-3"),
            BUNDLE_GRAPH.to_string(),
            BUNDLE_SUB_GRAPH.to_string(),
            "not an iri".to_string(),
        ] {
            assert!(gate(&g, admin).is_err(), "<{g}> admin={admin}");
        }
        assert!(gate(&format!("{base}/dataset/mine/g"), admin).is_ok());
        assert!(gate("urn:dataset:mine:rml-output", admin).is_ok());
        assert!(gate("http://unclaimed.example/g", admin).is_ok());
    }
    // Non-admins keep the namespace boundary; admins keep their reach.
    assert!(gate("urn:system:audit", false).is_err());
    assert!(gate("urn:system:audit", true).is_ok());
    assert!(gate(&format!("{base}/dataset/other/g"), false).is_err());

    // The delete filter: system and registry graphs stay, except the
    // dataset's own metadata and report graphs.
    let kept =
        |g: &str| dataset_graph::graph_kept_on_dataset_delete(&state.store, &base, "mine", g);
    assert!(kept(registry::REGISTRY_GRAPH));
    assert!(kept("urn:system:audit"));
    assert!(kept("urn:system:reports:dataset:other"));
    assert!(kept(BUNDLE_GRAPH));
    assert!(kept(&format!("{base}/data-model/x/version/1")));
    assert!(kept("not an iri"));
    assert!(!kept("urn:system:reports:dataset:mine"));
    assert!(!kept("urn:system:metadata:dataset:mine"));
    assert!(!kept(&format!("{base}/dataset/mine/g")));
    assert!(!kept("http://unclaimed.example/g"));
    // A SHACL Studio Library graph goes only with its Library entry.
    assert!(kept("urn:shapes:0000"));
}
