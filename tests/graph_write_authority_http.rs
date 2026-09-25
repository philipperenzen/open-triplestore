//! Who may write or delete a graph follows the graph, not the Library entry or
//! dataset that names it — through the real router.
//!
//! SHACL Studio:
//!
//! * registering an existing graph as a Library shape graph needs the right to
//!   change it (a graph-ACL read grant is not enough), and every Studio write
//!   (save, restore, import, a visibility change of an adopted graph) checks
//!   that right again, so an entry made before the rule gives no write;
//! * org members still edit their dataset's shapes graph in the Studio (an
//!   org viewer, who may not write the dataset, no longer can);
//! * `PATCH /api/datasets/{id}/graphs` with role `shapes` adopts only a graph
//!   registered to the dataset.
//!
//! Datasets:
//!
//! * a graph that already holds data is attached (and so becomes writable by
//!   the dataset's editors) only by a caller who may write it directly;
//!   server-named graphs (the Studio Library, sources, runs, other datasets'
//!   assets, entailment and property-state graphs) are never attached;
//! * a detach or dataset delete deletes only a graph the dataset created (or
//!   one the caller may delete directly): an old registration of someone
//!   else's graph only loses its row;
//! * linking a shapes graph is a read — `PUT /shapes` writes a linked graph
//!   only for a caller who may write it;
//! * a version restore and a CityJSON import stay inside the dataset.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, Role, SystemRole, Visibility};
use open_triplestore::data_models::content_digest;
use open_triplestore::server::AppState;
use open_triplestore::shacl_studio::models::ShapeSource;
use open_triplestore::shacl_studio::store::ShaclStudioStore;
use open_triplestore::store::TripleStore;
use serde_json::{json, Value};
use tower::ServiceExt as _;

/// A graph an admin loaded over the Graph Store, registered to no dataset.
const ADMIN_SHAPES: &str = "http://admin.example/shapes";
const ADMIN_DATA: &str = "http://admin.example/data";

fn shapes_ttl(class: &str) -> String {
    format!(
        "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
         <http://ex.org/{class}Shape> a sh:NodeShape ;\n\
           sh:targetClass <http://ex.org/{class}> ;\n\
           sh:property [ sh:path <http://ex.org/name> ; sh:minCount 1 ] .\n"
    )
}

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
        token,
        "application/json",
        body.to_string(),
    )
    .await
}

async fn turtle(state: &AppState, method: Method, uri: &str, token: &str, ttl: &str) -> StatusCode {
    send(state, method, uri, token, "text/turtle", ttl.to_string())
        .await
        .0
}

fn make_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

fn make_dataset(state: &AppState, id: &str, owner_type: OwnerType, owner: &str, vis: Visibility) {
    state
        .auth_db
        .create_dataset(id, id, None, owner_type, owner, vis, None)
        .unwrap();
}

fn load(store: &TripleStore, graph: &str, ttl: &str) {
    store
        .load_str(ttl, oxigraph::io::RdfFormat::Turtle, Some(graph))
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

fn grant(state: &AppState, graph: &str, user: &str, permission: &str) {
    state
        .auth_db
        .grant_graph_permission(
            &uuid::Uuid::new_v4().to_string(),
            graph,
            "user",
            user,
            permission,
            "adm",
        )
        .unwrap();
}

fn studio(state: &AppState) -> ShaclStudioStore {
    ShaclStudioStore::new(state.auth_db.pool())
}

fn library_entry(state: &AppState, graph: &str) -> Option<String> {
    studio(state)
        .get_shape_graph_by_iri(graph)
        .unwrap()
        .map(|s| s.id)
}

/// A Library entry made the way `register_shape_graph` made one before it
/// required write access: owned by `owner`, over someone else's graph.
fn adopted_entry(state: &AppState, graph: &str, owner: &str) -> String {
    studio(state)
        .create_shape_graph(
            "Adopted",
            None,
            OwnerType::User,
            owner,
            Visibility::Private,
            graph,
            &[],
            ShapeSource::Imported,
            Some(owner),
        )
        .unwrap()
        .id
}

// ─── SHACL Studio ────────────────────────────────────────────────────────────

/// A graph-ACL read grant used to be enough to register a graph as one's own
/// Library shape graph, and its owner then saved over the graph. Registering
/// now needs write access; an admin and a holder of a write grant still can.
#[tokio::test]
async fn registering_a_shape_graph_needs_write_access_to_it() {
    let (state, admin) = admin_state();
    let reader = make_user(&state, "reader");
    let writer = make_user(&state, "writer");
    load(&state.store, ADMIN_SHAPES, &shapes_ttl("Pump"));
    grant(&state, ADMIN_SHAPES, "reader", "read");
    grant(&state, ADMIN_SHAPES, "writer", "write");
    let before = digest(&state, ADMIN_SHAPES);
    let body = json!({ "graph_iri": ADMIN_SHAPES, "name": "Pumps", "visibility": "public" });

    let (status, text) = json(
        &state,
        Method::POST,
        "/api/shacl/register-shape-graph",
        &reader,
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(
        library_entry(&state, ADMIN_SHAPES).is_none(),
        "no entry was made"
    );

    let (status, text) = json(
        &state,
        Method::POST,
        "/api/shacl/register-shape-graph",
        &writer,
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let id = library_entry(&state, ADMIN_SHAPES).unwrap();
    // The reader now meets the entry the writer made: they may see it (it is
    // public), and gain nothing by asking again.
    let (status, _) = json(
        &state,
        Method::POST,
        "/api/shacl/register-shape-graph",
        &reader,
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let status = turtle(
        &state,
        Method::PUT,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        &reader,
        &shapes_ttl("Hijacked"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        digest(&state, ADMIN_SHAPES),
        before,
        "the graph is unchanged"
    );

    // The writer edits it in place.
    let status = turtle(
        &state,
        Method::PUT,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        &writer,
        &shapes_ttl("Valve"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(digest(&state, ADMIN_SHAPES), before);

    // An admin registers any graph that holds shapes.
    load(&state.store, ADMIN_DATA, &shapes_ttl("Pipe"));
    let (status, text) = json(
        &state,
        Method::POST,
        "/api/shacl/register-shape-graph",
        &admin,
        json!({ "graph_iri": ADMIN_DATA, "name": "Pipes" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
}

/// An entry that already names someone else's graph — made before the rule,
/// or with a grant since revoked — gives its owner no write: save, restore,
/// import and a visibility change are refused, and the graph stays as it
/// was. The minted graphs of the Studio work as before.
#[tokio::test]
async fn a_library_entry_over_someone_elses_graph_gives_no_write() {
    let (state, _admin) = admin_state();
    let user = make_user(&state, "u1");
    load(&state.store, ADMIN_SHAPES, &shapes_ttl("Pump"));
    let before = digest(&state, ADMIN_SHAPES);
    let id = adopted_entry(&state, ADMIN_SHAPES, "u1");
    studio(&state)
        .save_shape_graph_revision(&id, &shapes_ttl("Old"), &[], 1, Some("seed"), Some("u1"))
        .unwrap();

    let base = format!("/api/shacl/shape-graphs/{id}");
    let status = turtle(
        &state,
        Method::PUT,
        &format!("{base}/turtle"),
        &user,
        &shapes_ttl("Hijacked"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "save");
    let (status, _) = json(
        &state,
        Method::POST,
        &format!("{base}/restore/1"),
        &user,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "restore");

    // Import copies shapes from a graph the user may read into the entry's.
    let (status, created) = json(
        &state,
        Method::POST,
        "/api/shacl/shape-graphs",
        &user,
        json!({ "name": "Mine", "turtle": shapes_ttl("Mine") }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let mine: Value = serde_json::from_str(&created).unwrap();
    let mine_graph = mine["graph_iri"].as_str().unwrap().to_string();
    let mine_id = mine["id"].as_str().unwrap().to_string();
    grant(&state, &mine_graph, "u1", "read");
    let (status, _) = json(
        &state,
        Method::POST,
        &format!("{base}/import-shapes"),
        &user,
        json!({ "shapes": [{ "source_graph": mine_graph, "shape": "http://ex.org/MineShape" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "import");

    let (status, _) = json(
        &state,
        Method::PUT,
        &base,
        &user,
        json!({ "name": "Adopted", "visibility": "public" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "making it world-readable");
    // Renaming it changes no one's access and stays allowed.
    let (status, _) = json(
        &state,
        Method::PUT,
        &base,
        &user,
        json!({ "name": "Renamed", "visibility": "private" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "renaming");
    assert_eq!(
        digest(&state, ADMIN_SHAPES),
        before,
        "the graph is unchanged"
    );

    // Deleting the entry drops only its rows.
    let (status, _) = json(&state, Method::DELETE, &base, &user, json!({})).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(digest(&state, ADMIN_SHAPES), before);

    // The user's own minted shape graph: edit and delete (which clears it).
    let status = turtle(
        &state,
        Method::PUT,
        &format!("/api/shacl/shape-graphs/{mine_id}/turtle"),
        &user,
        &shapes_ttl("Edited"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = json(
        &state,
        Method::DELETE,
        &format!("/api/shacl/shape-graphs/{mine_id}"),
        &user,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(triple_count(&state, &mine_graph), 0);
}

/// A graph named like one the Studio mints is registered only by an admin
/// (the Studio clears such a graph with its entry), and an entry a caller may
/// not see is not handed out by asking to register its graph again.
#[tokio::test]
async fn studio_named_graphs_and_private_entries_stay_with_their_owners() {
    let (state, _admin) = admin_state();
    let owner = make_user(&state, "owner");
    let other = make_user(&state, "other");
    let orphan = "urn:shapes:0000-orphan";
    load(&state.store, orphan, &shapes_ttl("Orphan"));
    grant(&state, orphan, "other", "write");
    let (status, _) = json(
        &state,
        Method::POST,
        "/api/shacl/register-shape-graph",
        &other,
        json!({ "graph_iri": orphan, "name": "Orphan" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, created) = json(
        &state,
        Method::POST,
        "/api/shacl/shape-graphs",
        &owner,
        json!({ "name": "Private", "turtle": shapes_ttl("Private") }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let graph = serde_json::from_str::<Value>(&created).unwrap()["graph_iri"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, text) = json(
        &state,
        Method::POST,
        "/api/shacl/register-shape-graph",
        &other,
        json!({ "graph_iri": graph, "name": "Mine now" }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert!(
        !text.contains("Private"),
        "the entry is not disclosed: {text}"
    );
}

/// The existing flow: an organisation's dataset keeps its shapes in a graph
/// of its own, adopted into the Library with the organisation as owner. Its
/// members who may write the dataset edit it in the Studio without any
/// graph-ACL grant; an org viewer, who may not write the dataset, may not.
#[tokio::test]
async fn org_members_edit_their_datasets_shapes_graph_in_the_studio() {
    let (state, _admin) = admin_state();
    let member = make_user(&state, "member");
    let viewer = make_user(&state, "viewer");
    let db = &state.auth_db;
    db.create_organisation("acme", "Acme", "acme", None, None)
        .unwrap();
    db.add_org_member("member", "acme", Role::Member).unwrap();
    db.add_org_member("viewer", "acme", Role::Viewer).unwrap();
    make_dataset(
        &state,
        "orgds",
        OwnerType::Organisation,
        "acme",
        Visibility::Members,
    );

    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/orgds/shapes",
        &member,
        &shapes_ttl("Asset"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let shapes = "urn:dataset:orgds:shapes";
    let id = library_entry(&state, shapes).expect("adopted into the Library");

    let status = turtle(
        &state,
        Method::PUT,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        &member,
        &shapes_ttl("Bridge"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "an org member edits it");
    let after_member = digest(&state, shapes);

    let status = turtle(
        &state,
        Method::PUT,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        &viewer,
        &shapes_ttl("Viewer"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "an org viewer may not");
    assert_eq!(digest(&state, shapes), after_member);
}

/// "Set role: shapes" adopted any graph holding a shape into the Library,
/// owned by the dataset's owner — a graph the dataset never registered
/// included. It now answers 404 for such a graph and adopts nothing.
#[tokio::test]
async fn setting_the_shapes_role_adopts_only_a_registered_graph() {
    let (state, _admin) = admin_state();
    let user = make_user(&state, "u1");
    make_dataset(&state, "mine", OwnerType::User, "u1", Visibility::Private);
    load(&state.store, ADMIN_SHAPES, &shapes_ttl("Pump"));
    let (status, text) = json(
        &state,
        Method::PATCH,
        "/api/datasets/mine/graphs",
        &user,
        json!({ "graph_iri": ADMIN_SHAPES, "graph_role": "shapes" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{text}");
    assert!(library_entry(&state, ADMIN_SHAPES).is_none());

    // A graph the dataset made is adopted as before.
    let own = format!("{}/dataset/mine/shapes", state.base_url);
    let (status, _) = json(
        &state,
        Method::POST,
        "/api/datasets/mine/graphs",
        &user,
        json!({ "graph_iri": own }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    load(&state.store, &own, &shapes_ttl("Own"));
    let (status, text) = json(
        &state,
        Method::PATCH,
        "/api/datasets/mine/graphs",
        &user,
        json!({ "graph_iri": own, "graph_role": "shapes" }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
    assert!(library_entry(&state, &own).is_some());
}

// ─── Datasets ────────────────────────────────────────────────────────────────

/// A non-admin attached any unclaimed graph to their dataset — an admin's
/// graph, another user's Studio shape graph — and could then overwrite it
/// through the dataset and delete it with a detach. Now a graph that holds
/// data needs direct write access, and server-named graphs are refused.
#[tokio::test]
async fn a_dataset_cannot_take_over_a_graph_it_did_not_make() {
    let (state, _admin) = admin_state();
    let base = state.base_url.to_string();
    let user = make_user(&state, "u1");
    let victim = make_user(&state, "victim");
    make_dataset(&state, "mine", OwnerType::User, "u1", Visibility::Private);
    make_dataset(
        &state,
        "other",
        OwnerType::User,
        "victim",
        Visibility::Private,
    );
    put_triple(&state.store, ADMIN_DATA, "admin");
    let (status, created) = json(
        &state,
        Method::POST,
        "/api/shacl/shape-graphs",
        &victim,
        json!({ "name": "Victim", "turtle": shapes_ttl("Victim") }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let victim_shapes = serde_json::from_str::<Value>(&created).unwrap()["graph_iri"]
        .as_str()
        .unwrap()
        .to_string();
    let victim_before = digest(&state, &victim_shapes);

    for g in [
        ADMIN_DATA.to_string(),
        victim_shapes.clone(),
        "urn:run:some-run".to_string(),
        "urn:mapping:m:version:1".to_string(),
        "urn:config:mapping-gates".to_string(),
        "urn:entailment:rdfs:other".to_string(),
        "urn:ots:property-states:other".to_string(),
        format!("{base}/datasets/other/assets"),
    ] {
        let (status, text) = json(
            &state,
            Method::POST,
            "/api/datasets/mine/graphs",
            &user,
            json!({ "graph_iri": g }),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "attach <{g}>: {text}");
    }
    let (status, text) = json(
        &state,
        Method::PUT,
        "/api/datasets/mine/shacl",
        &user,
        json!({ "shacl_on_write": false, "shapes_graph_iri": victim_shapes }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "link a private Library graph: {text}"
    );
    let (status, text) = json(
        &state,
        Method::PUT,
        "/api/datasets/mine/shacl",
        &user,
        json!({ "shacl_on_write": false, "shapes_graph_iri": ADMIN_DATA }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "link an unreadable graph: {text}"
    );
    let (status, text) = send(
        &state,
        Method::POST,
        &format!(
            "/api/datasets/mine/mappings/execute?graph={}",
            url_encode(ADMIN_DATA)
        ),
        &user,
        "multipart/form-data; boundary=X",
        "--X--\r\n".to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "RML into it: {text}");
    assert!(state
        .auth_db
        .list_dataset_graphs("mine")
        .unwrap()
        .is_empty());
    assert_eq!(triple_count(&state, ADMIN_DATA), 1);
    assert_eq!(digest(&state, &victim_shapes), victim_before);

    // Still allowed: the dataset's own graphs, its own server-kept graphs, and
    // a new external graph, which the dataset creates.
    for g in [
        format!("{base}/dataset/mine/g"),
        "urn:ots:property-states:mine".to_string(),
        format!("{base}/datasets/mine/assets"),
        "http://my.example/new-graph".to_string(),
    ] {
        let (status, text) = json(
            &state,
            Method::POST,
            "/api/datasets/mine/graphs",
            &user,
            json!({ "graph_iri": g }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "attach <{g}>: {text}");
    }
}

/// A caller who may write a graph directly may attach it; the dataset then
/// holds it, but a detach by an editor without that right keeps its data. The
/// writer's own detach, and a detach of a graph the dataset created, delete it.
#[tokio::test]
async fn only_a_graphs_writer_or_creator_deletes_it_with_a_detach() {
    let (state, _admin) = admin_state();
    let writer = make_user(&state, "writer");
    let editor = make_user(&state, "editor");
    let db = &state.auth_db;
    db.create_organisation("acme", "Acme", "acme", None, None)
        .unwrap();
    db.add_org_member("writer", "acme", Role::Member).unwrap();
    db.add_org_member("editor", "acme", Role::Member).unwrap();
    make_dataset(
        &state,
        "orgds",
        OwnerType::Organisation,
        "acme",
        Visibility::Members,
    );
    put_triple(&state.store, ADMIN_DATA, "admin");
    grant(&state, ADMIN_DATA, "writer", "write");
    let attach = |g: &str| json!({ "graph_iri": g });

    let (status, text) = json(
        &state,
        Method::POST,
        "/api/datasets/orgds/graphs",
        &editor,
        attach(ADMIN_DATA),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let (status, text) = json(
        &state,
        Method::POST,
        "/api/datasets/orgds/graphs",
        &writer,
        attach(ADMIN_DATA),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");

    let (status, _) = json(
        &state,
        Method::DELETE,
        "/api/datasets/orgds/graphs",
        &editor,
        attach(ADMIN_DATA),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        triple_count(&state, ADMIN_DATA),
        1,
        "the editor's detach keeps it"
    );

    let (status, _) = json(
        &state,
        Method::POST,
        "/api/datasets/orgds/graphs",
        &writer,
        attach(ADMIN_DATA),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = json(
        &state,
        Method::DELETE,
        "/api/datasets/orgds/graphs",
        &writer,
        attach(ADMIN_DATA),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        triple_count(&state, ADMIN_DATA),
        0,
        "the writer's detach deletes it"
    );

    // A graph the dataset created goes with any editor's detach.
    let created = "http://acme.example/new";
    let (status, _) = json(
        &state,
        Method::POST,
        "/api/datasets/orgds/graphs",
        &editor,
        attach(created),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    put_triple(&state.store, created, "made by the dataset");
    let (status, _) = json(
        &state,
        Method::DELETE,
        "/api/datasets/orgds/graphs",
        &editor,
        attach(created),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(triple_count(&state, created), 0);
}

/// Registrations made before the rule — an admin's graph, another user's
/// Studio shape graph — are detached and deleted with the dataset without
/// their data going: only the dataset's rows do. Its own graph still goes.
#[tokio::test]
async fn old_registrations_of_foreign_graphs_are_never_deleted() {
    let (state, _admin) = admin_state();
    let base = state.base_url.to_string();
    let user = make_user(&state, "u1");
    let victim = make_user(&state, "victim");
    make_dataset(&state, "mine", OwnerType::User, "u1", Visibility::Private);
    let db = &state.auth_db;
    put_triple(&state.store, ADMIN_DATA, "admin");
    let (_, created) = json(
        &state,
        Method::POST,
        "/api/shacl/shape-graphs",
        &victim,
        json!({ "name": "Victim", "turtle": shapes_ttl("Victim") }),
    )
    .await;
    let victim_shapes = serde_json::from_str::<Value>(&created).unwrap()["graph_iri"]
        .as_str()
        .unwrap()
        .to_string();
    let victim_before = digest(&state, &victim_shapes);

    for g in [ADMIN_DATA, victim_shapes.as_str()] {
        db.add_dataset_graph("mine", g).unwrap();
        let (status, text) = json(
            &state,
            Method::DELETE,
            "/api/datasets/mine/graphs",
            &user,
            json!({ "graph_iri": g }),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
    }
    assert_eq!(triple_count(&state, ADMIN_DATA), 1);
    assert_eq!(digest(&state, &victim_shapes), victim_before);

    let own = format!("{base}/dataset/mine/g");
    put_triple(&state.store, &own, "own");
    db.add_dataset_graph("mine", &own).unwrap();
    db.add_dataset_graph("mine", ADMIN_DATA).unwrap();
    db.update_dataset_shacl("mine", false, Some(&victim_shapes))
        .unwrap();
    let (status, text) = json(
        &state,
        Method::DELETE,
        "/api/datasets/mine",
        &user,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
    assert_eq!(triple_count(&state, &own), 0, "its own graph goes");
    assert_eq!(triple_count(&state, ADMIN_DATA), 1);
    assert_eq!(digest(&state, &victim_shapes), victim_before);
}

/// Linking a shapes graph the caller may read is allowed — validation reads
/// it — but `PUT /shapes` does not write it, and deleting the dataset keeps
/// it. A new shapes graph the dataset makes is its own.
#[tokio::test]
async fn a_linked_shapes_graph_is_read_not_written() {
    let (state, _admin) = admin_state();
    let user = make_user(&state, "u1");
    make_dataset(&state, "mine", OwnerType::User, "u1", Visibility::Private);
    load(&state.store, ADMIN_SHAPES, &shapes_ttl("Pump"));
    grant(&state, ADMIN_SHAPES, "u1", "read");
    let before = digest(&state, ADMIN_SHAPES);

    let (status, text) = json(
        &state,
        Method::PUT,
        "/api/datasets/mine/shacl",
        &user,
        json!({ "shacl_on_write": false, "shapes_graph_iri": ADMIN_SHAPES }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/mine/shapes",
        &user,
        &shapes_ttl("Hijacked"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Nor through the Library entry the link adopted it as.
    if let Some(id) = library_entry(&state, ADMIN_SHAPES) {
        let status = turtle(
            &state,
            Method::PUT,
            &format!("/api/shacl/shape-graphs/{id}/turtle"),
            &user,
            &shapes_ttl("Hijacked"),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    assert_eq!(digest(&state, ADMIN_SHAPES), before);

    // A new external shapes graph is the dataset's once it writes it.
    let own = "http://my.example/shapes";
    let (status, _) = json(
        &state,
        Method::PUT,
        "/api/datasets/mine/shacl",
        &user,
        json!({ "shacl_on_write": false, "shapes_graph_iri": own }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    for class in ["First", "Second"] {
        let status = turtle(
            &state,
            Method::PUT,
            "/api/datasets/mine/shapes",
            &user,
            &shapes_ttl(class),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{class}");
    }
    assert!(triple_count(&state, own) > 0);

    // The dataset goes with its own shapes graph; the linked one stays.
    state
        .auth_db
        .update_dataset_shacl("mine", false, Some(ADMIN_SHAPES))
        .unwrap();
    let (status, _) = json(
        &state,
        Method::DELETE,
        "/api/datasets/mine",
        &user,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(digest(&state, ADMIN_SHAPES), before);
    assert_eq!(triple_count(&state, own), 0);
}

/// A version restore overwrites the graphs its snapshot names: one that has
/// since left the dataset and holds someone else's data is skipped and
/// reported, and the rest is restored.
#[tokio::test]
async fn a_version_restore_does_not_overwrite_a_graph_the_dataset_let_go() {
    let (state, admin) = admin_state();
    let user = make_user(&state, "u1");
    make_dataset(&state, "mine", OwnerType::User, "u1", Visibility::Private);
    let g = "http://my.example/restorable";
    let own = format!("{}/dataset/mine/kept", state.base_url);
    for graph in [g, own.as_str()] {
        let (status, _) = json(
            &state,
            Method::POST,
            "/api/datasets/mine/graphs",
            &user,
            json!({ "graph_iri": graph }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        put_triple(&state.store, graph, "v1");
    }
    let (status, text) = json(
        &state,
        Method::POST,
        "/api/datasets/mine/versions",
        &user,
        json!({ "version": "1.0.0" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{text}");
    let (status, _) = json(
        &state,
        Method::DELETE,
        "/api/datasets/mine/graphs",
        &user,
        json!({ "graph_iri": g }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        triple_count(&state, g),
        0,
        "it created the graph, so it went"
    );

    // Someone else's data now lives there, and the dataset's own graph moved on.
    put_triple(&state.store, g, "admin's");
    put_triple(&state.store, &own, "v2");
    let before = digest(&state, g);
    let (status, text) = json(
        &state,
        Method::POST,
        "/api/datasets/mine/versions/1.0.0/restore",
        &user,
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let body: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(body["skipped"][0]["graph"], g, "{text}");
    assert_eq!(body["restored"], json!([own]), "{text}");
    assert_eq!(
        digest(&state, g),
        before,
        "the graph it let go is untouched"
    );
    assert_eq!(triple_count(&state, &own), 1, "its own graph is back at v1");
    assert!(!state
        .auth_db
        .list_dataset_graphs("mine")
        .unwrap()
        .contains(&g.to_string()));

    // An admin may restore it.
    let (status, text) = json(
        &state,
        Method::POST,
        "/api/datasets/mine/versions/1.0.0/restore",
        &admin,
        json!({}),
    )
    .await;
    assert!(status.is_success(), "{status}: {text}");
}

/// The CityJSON write boundary compared the target with the dataset's IRI
/// without the trailing slash, so dataset `bridge` could write
/// `{base}/dataset/bridge-inventory/…`.
#[tokio::test]
async fn a_cityjson_import_stays_inside_its_own_dataset_namespace() {
    let (state, _admin) = admin_state();
    let base = state.base_url.to_string();
    let user = make_user(&state, "u1");
    make_dataset(&state, "bridge", OwnerType::User, "u1", Visibility::Private);
    let foreign = format!("{base}/dataset/bridge-inventory/objects");
    let boundary = "CJ";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"target_graph\"\r\n\r\n{foreign}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.city.json\"\r\n\
         Content-Type: application/json\r\n\r\n{{\"type\":\"CityJSON\",\"version\":\"2.0\",\
         \"CityObjects\":{{}},\"vertices\":[]}}\r\n--{boundary}--\r\n"
    );
    let (status, text) = send(
        &state,
        Method::POST,
        "/api/datasets/bridge/ingest/cityjson",
        &user,
        &format!("multipart/form-data; boundary={boundary}"),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    assert_eq!(triple_count(&state, &foreign), 0);
}

/// A graph the graph ACL already hands to someone is theirs even before it
/// holds data: another user cannot claim it for their dataset, or link it.
#[tokio::test]
async fn a_graph_granted_to_someone_is_not_claimed_while_it_is_empty() {
    let (state, _admin) = admin_state();
    let user = make_user(&state, "u1");
    make_user(&state, "grantee");
    make_dataset(&state, "mine", OwnerType::User, "u1", Visibility::Private);
    let granted = "http://grantee.example/g";
    grant(&state, granted, "grantee", "write");
    let (status, text) = json(
        &state,
        Method::POST,
        "/api/datasets/mine/graphs",
        &user,
        json!({ "graph_iri": granted }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let (status, text) = json(
        &state,
        Method::PUT,
        "/api/datasets/mine/shacl",
        &user,
        json!({ "shacl_on_write": false, "shapes_graph_iri": granted }),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "link: {text}");
}

/// A mapping whose `rml:graphMap` names a new external graph can run again:
/// the first run registers the graph it created, so the second finds it
/// held by the dataset.
#[tokio::test]
async fn an_rml_graph_map_target_is_registered_and_runs_again() {
    let (state, _admin) = admin_state();
    let user = make_user(&state, "u1");
    make_dataset(&state, "mine", OwnerType::User, "u1", Visibility::Private);
    let people = "http://example.org/people";
    let mapping = format!(
        "@prefix rr: <http://www.w3.org/ns/r2rml#> .\n\
         @prefix rml: <http://semweb.mmlab.be/ns/rml#> .\n\
         @prefix ql: <http://semweb.mmlab.be/ns/ql#> .\n\
         @prefix ex: <http://example.org/> .\n\
         ex:M a rr:TriplesMap ; rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;\n\
           rr:predicateObjectMap ex:POM ; rr:graphMap ex:GM .\n\
         ex:Src rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV .\n\
         ex:Subj rr:template \"http://example.org/r/{{id}}\" .\n\
         ex:POM rr:predicate ex:name ; rr:objectMap [ rml:reference \"name\" ] .\n\
         ex:GM rr:constant <{people}> .\n"
    );
    let boundary = "RML";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"mapping\"\r\n\r\n{mapping}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"d.csv\"\r\n\r\nid,name\n1,Al\n\r\n\
         --{boundary}--\r\n"
    );
    for run in 1..=2 {
        let (status, text) = send(
            &state,
            Method::POST,
            "/api/datasets/mine/mappings/execute",
            &user,
            &format!("multipart/form-data; boundary={boundary}"),
            body.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "run {run}: {text}");
    }
    assert!(triple_count(&state, people) > 0);
    assert!(state
        .auth_db
        .list_dataset_graphs("mine")
        .unwrap()
        .contains(&people.to_string()));
}

/// Sharing shapes: a dataset writes its own external shapes graph, a co-editor
/// writes it too, and a user who may read that dataset links the same graph
/// to theirs (read only). Resending an unchanged link (toggling
/// SHACL-on-write) is not gated again, so a co-editor who cannot see the
/// linked Library graph can still toggle it.
#[tokio::test]
async fn shapes_graphs_are_shared_and_existing_links_stay_editable() {
    let (state, _admin) = admin_state();
    let owner = make_user(&state, "owner");
    let editor = make_user(&state, "editor");
    let db = &state.auth_db;
    db.create_organisation("acme", "Acme", "acme", None, None)
        .unwrap();
    db.add_org_member("owner", "acme", Role::Member).unwrap();
    db.add_org_member("editor", "acme", Role::Member).unwrap();
    make_dataset(
        &state,
        "a",
        OwnerType::Organisation,
        "acme",
        Visibility::Members,
    );
    make_dataset(&state, "b", OwnerType::User, "owner", Visibility::Private);
    let shared = "https://acme.example/shapes";

    let (status, _) = json(
        &state,
        Method::PUT,
        "/api/datasets/a/shacl",
        &owner,
        json!({ "shacl_on_write": false, "shapes_graph_iri": shared }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/a/shapes",
        &owner,
        &shapes_ttl("Asset"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/a/shapes",
        &editor,
        &shapes_ttl("Pipe"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "a co-editor writes the dataset's shapes graph"
    );

    let (status, text) = json(
        &state,
        Method::PUT,
        "/api/datasets/b/shacl",
        &owner,
        json!({ "shacl_on_write": false, "shapes_graph_iri": shared }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "sharing a's shapes with b: {text}"
    );
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/b/shapes",
        &owner,
        &shapes_ttl("Other"),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "b only links it");

    // A Library shape graph the owner links to the org dataset; the co-editor
    // cannot see that entry, yet toggles SHACL-on-write on the link.
    let (status, created) = json(
        &state,
        Method::POST,
        "/api/shacl/shape-graphs",
        &owner,
        json!({ "name": "Owner's", "turtle": shapes_ttl("Owned") }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let library = serde_json::from_str::<Value>(&created).unwrap()["graph_iri"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = json(
        &state,
        Method::PUT,
        "/api/datasets/a/shacl",
        &owner,
        json!({ "shacl_on_write": false, "shapes_graph_iri": library }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, text) = json(
        &state,
        Method::PUT,
        "/api/datasets/a/shacl",
        &editor,
        json!({ "shacl_on_write": true, "shapes_graph_iri": library }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
}

/// An admin's write of a dataset's linked shapes graph makes it the
/// dataset's, so the dataset's editors may write it afterwards, there and in
/// the Studio.
#[tokio::test]
async fn an_admins_shapes_write_leaves_the_graph_to_the_dataset() {
    let (state, admin) = admin_state();
    let member = make_user(&state, "member");
    let db = &state.auth_db;
    db.create_organisation("acme", "Acme", "acme", None, None)
        .unwrap();
    db.add_org_member("member", "acme", Role::Member).unwrap();
    make_dataset(
        &state,
        "orgds",
        OwnerType::Organisation,
        "acme",
        Visibility::Members,
    );
    let linked = "https://data.example.org/shapes";
    let (status, _) = json(
        &state,
        Method::PUT,
        "/api/datasets/orgds/shacl",
        &admin,
        json!({ "shacl_on_write": false, "shapes_graph_iri": linked }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/orgds/shapes",
        &admin,
        &shapes_ttl("Asset"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/orgds/shapes",
        &member,
        &shapes_ttl("Pipe"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let id = library_entry(&state, linked).expect("adopted into the Library");
    let status = turtle(
        &state,
        Method::PUT,
        &format!("/api/shacl/shape-graphs/{id}/turtle"),
        &member,
        &shapes_ttl("Valve"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

/// An admin's write for a dataset registers a graph to it only when a
/// non-admin could have claimed the graph as new: a linked graph that already
/// held data, and a reserved graph a stored mapping names, stay out of the
/// dataset, so its editors gain no write to them.
#[tokio::test]
async fn an_admins_write_hands_the_dataset_only_graphs_it_could_claim() {
    let (state, admin) = admin_state();
    let member = make_user(&state, "member");
    let db = &state.auth_db;
    db.create_organisation("acme", "Acme", "acme", None, None)
        .unwrap();
    db.add_org_member("member", "acme", Role::Member).unwrap();
    make_dataset(
        &state,
        "orgds",
        OwnerType::Organisation,
        "acme",
        Visibility::Members,
    );

    // A reference shapes graph the member may only read, linked by them.
    load(&state.store, ADMIN_SHAPES, &shapes_ttl("Reference"));
    grant(&state, ADMIN_SHAPES, "member", "read");
    let (status, _) = json(
        &state,
        Method::PUT,
        "/api/datasets/orgds/shacl",
        &member,
        json!({ "shacl_on_write": false, "shapes_graph_iri": ADMIN_SHAPES }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/orgds/shapes",
        &admin,
        &shapes_ttl("Admin"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!db
        .list_dataset_graphs("orgds")
        .unwrap()
        .contains(&ADMIN_SHAPES.to_string()));
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/orgds/shapes",
        &member,
        &shapes_ttl("Member"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the member still may not write it"
    );

    // A stored mapping that routes a triple into a server graph: the admin's
    // run writes it, but does not make it the dataset's.
    let gates = "urn:config:mapping-gates";
    let mapping = format!(
        "@prefix rr: <http://www.w3.org/ns/r2rml#> .\n\
         @prefix rml: <http://semweb.mmlab.be/ns/rml#> .\n\
         @prefix ql: <http://semweb.mmlab.be/ns/ql#> .\n\
         @prefix ex: <http://example.org/> .\n\
         ex:M a rr:TriplesMap ; rml:logicalSource ex:Src ; rr:subjectMap ex:Subj ;\n\
           rr:predicateObjectMap ex:POM ; rr:graphMap ex:GM .\n\
         ex:Src rml:source \"d.csv\" ; rml:referenceFormulation ql:CSV .\n\
         ex:Subj rr:template \"http://example.org/r/{{id}}\" .\n\
         ex:POM rr:predicate ex:name ; rr:objectMap [ rml:reference \"name\" ] .\n\
         ex:GM rr:constant <{gates}> .\n"
    );
    let boundary = "RML";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"mapping\"\r\n\r\n{mapping}\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"d.csv\"\r\n\r\nid,name\n1,Al\n\r\n\
         --{boundary}--\r\n"
    );
    let (status, text) = send(
        &state,
        Method::POST,
        "/api/datasets/orgds/mappings/execute",
        &admin,
        &format!("multipart/form-data; boundary={boundary}"),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert!(!db
        .list_dataset_graphs("orgds")
        .unwrap()
        .contains(&gates.to_string()));
}

/// Two datasets sharing one external shapes graph: while it is still empty
/// it is the first dataset's pending shapes graph, and a second link would
/// stop that dataset from writing them, so it waits until they exist. A
/// dataset's default shapes graph is shared with those who may read it.
#[tokio::test]
async fn a_pending_shapes_graph_is_linked_once_its_shapes_exist() {
    let (state, _admin) = admin_state();
    let owner = make_user(&state, "owner");
    make_dataset(&state, "a", OwnerType::User, "owner", Visibility::Private);
    make_dataset(&state, "b", OwnerType::User, "owner", Visibility::Private);
    let shared = "https://owner.example/shapes";
    let link = |ds: &'static str, g: &str| {
        let state = state.clone();
        let owner = owner.clone();
        let g = g.to_string();
        async move {
            json(
                &state,
                Method::PUT,
                &format!("/api/datasets/{ds}/shacl"),
                &owner,
                json!({ "shacl_on_write": false, "shapes_graph_iri": g }),
            )
            .await
        }
    };
    assert_eq!(link("a", shared).await.0, StatusCode::NO_CONTENT);
    let (status, text) = link("b", shared).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{text}");
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/a/shapes",
        &owner,
        &shapes_ttl("Asset"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "a writes its shapes");
    assert_eq!(
        link("b", shared).await.0,
        StatusCode::NO_CONTENT,
        "then b shares them"
    );

    // The default shapes graph of a dataset the caller may read.
    make_dataset(&state, "c", OwnerType::User, "owner", Visibility::Private);
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/c/shapes",
        &owner,
        &shapes_ttl("Pipe"),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, text) = link("b", "urn:dataset:c:shapes").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
    let status = turtle(
        &state,
        Method::PUT,
        "/api/datasets/b/shapes",
        &owner,
        &shapes_ttl("Other"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "b only links c's shapes graph"
    );
}
