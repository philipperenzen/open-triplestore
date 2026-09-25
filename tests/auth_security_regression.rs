//! Cross-tenant authorization regression tests — CI gate (`cargo test … security`).
//!
//! Locks in the dataset-graph write/registration boundary and the dataset-create
//! owner gate against the cross-tenant IDOR vectors found in the 2026-06 review
//! follow-up:
//!   * **graph-claim read escalation** via `POST /api/datasets/:id/graphs` — a
//!     writer attaching another tenant's private graph IRI to their own dataset,
//!     which `get_accessible_graph_iris` would then expose to them;
//!   * **foreign-graph write** via `POST /api/datasets/:id/mappings/execute` — RML
//!     `?graph=` / `rml:graphMap` targeting another tenant's graph;
//!   * **owner forgery** on dataset creation.
//!
//! Also locks in two later authorization fixes:
//!   * **cross-org group-member IDOR** — the group-member routes only checked the
//!     caller's role in the path's `org_id`, not that the group belonged to it, so
//!     an admin of any org could list/add/remove members of another org's group;
//!   * **publish-gate bypass** — `update_dataset` gated a visibility change on
//!     manage rights but not publisher rights, so a non-publisher could create a
//!     dataset private and then `PUT` it public.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_state, mint_token, test_app};
use open_triplestore::auth::dataset_graph::authorize_dataset_graph_target;
use open_triplestore::auth::db::AuthDb;
use open_triplestore::auth::models::{OwnerType, Role, SystemRole, Visibility};
use open_triplestore::data_models::registry;
use open_triplestore::server::AppState;
use tower::ServiceExt as _;

const BASE: &str = "http://localhost:7878";

fn make_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

fn make_dataset(state: &AppState, id: &str, owner: &str) {
    state
        .auth_db
        .create_dataset(
            id,
            id,
            None,
            OwnerType::User,
            owner,
            Visibility::Private,
            None,
        )
        .unwrap();
}

fn post_json(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

// ───────────────── HIGH-1: graph-claim cross-tenant read escalation ─────────────────

#[tokio::test]
async fn cannot_register_another_datasets_graph_security() {
    let (state, _admin) = admin_state();
    make_user(&state, "victim");
    make_dataset(&state, "victimds", "victim");
    let victim_graph = "http://victim.example/private-data";
    // The victim owns this graph (DB-level setup, as the owner/admin path would).
    state
        .auth_db
        .add_dataset_graph("victimds", victim_graph)
        .unwrap();

    let attacker = make_user(&state, "attacker");
    make_dataset(&state, "attackerds", "attacker");

    // Attacker (a writer of their OWN dataset) tries to attach the victim's graph.
    let resp = test_app(state.clone())
        .oneshot(post_json(
            "/api/datasets/attackerds/graphs",
            &attacker,
            serde_json::json!({ "graph_iri": victim_graph }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "registering another dataset's graph must be rejected"
    );

    // …and it must NOT have been attached to the attacker's dataset.
    let attacker_graphs = state.auth_db.list_dataset_graphs("attackerds").unwrap();
    assert!(
        !attacker_graphs.iter().any(|g| g == victim_graph),
        "victim graph must not be registered to attacker dataset: {attacker_graphs:?}"
    );
}

#[tokio::test]
async fn cannot_register_foreign_reserved_namespace_security() {
    let (state, _admin) = admin_state();
    make_user(&state, "victim2");
    make_dataset(&state, "victimds2", "victim2");
    let attacker = make_user(&state, "attacker2");
    make_dataset(&state, "attackerds2", "attacker2");

    let base_graph = format!("{BASE}/dataset/victimds2/instances");
    for foreign in [
        "urn:dataset:victimds2:shapes",
        base_graph.as_str(),
        "urn:system:metadata:dataset:victimds2",
    ] {
        let resp = test_app(state.clone())
            .oneshot(post_json(
                "/api/datasets/attackerds2/graphs",
                &attacker,
                serde_json::json!({ "graph_iri": foreign }),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "foreign/reserved graph <{foreign}> must be rejected"
        );
    }
}

#[tokio::test]
async fn can_register_own_and_unclaimed_graphs_security() {
    let (state, _admin) = admin_state();
    let owner = make_user(&state, "owner3");
    make_dataset(&state, "ds3", "owner3");

    // Own namespaced graphs (both schemes) and an unclaimed external graph are OK —
    // the boundary must not break legitimate registration.
    for g in [
        format!("{BASE}/dataset/ds3/instances"),
        "urn:dataset:ds3:rml-output".to_string(),
        "http://my.example/new-graph".to_string(),
    ] {
        let resp = test_app(state.clone())
            .oneshot(post_json(
                "/api/datasets/ds3/graphs",
                &owner,
                serde_json::json!({ "graph_iri": g }),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "legitimate graph <{g}> must be allowed"
        );
    }
}

// ───────────────────────── HIGH-2: RML foreign-graph write ─────────────────────────

#[tokio::test]
async fn rml_execute_cannot_target_foreign_graph_security() {
    let (state, _admin) = admin_state();
    make_user(&state, "rvictim");
    make_dataset(&state, "rvictimds", "rvictim");
    let attacker = make_user(&state, "rattacker");
    make_dataset(&state, "rattackerds", "rattacker");

    // A foreign `?graph=` target is rejected before any mapping work (403).
    let uri = format!(
        "/api/datasets/rattackerds/mappings/execute?graph={}",
        common::url_encode("urn:dataset:rvictimds:rml-output")
    );
    let req = Request::builder()
        .method("POST")
        .uri(&uri)
        .header("Authorization", format!("Bearer {attacker}"))
        .header("content-type", "multipart/form-data; boundary=X")
        .body(Body::from("--X--\r\n"))
        .unwrap();
    let resp = test_app(state.clone()).oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "RML output into another tenant's graph must be rejected"
    );
}

// ─────────────────── dataset-create owner forgery (can_act_as_owner) ───────────────────

#[tokio::test]
async fn cannot_forge_dataset_owner_security() {
    let (state, _admin) = admin_state();
    make_user(&state, "alice");
    let bob = make_user(&state, "bob");

    // Bob tries to create a dataset OWNED BY alice → rejected.
    let resp = test_app(state.clone())
        .oneshot(post_json(
            "/api/datasets",
            &bob,
            serde_json::json!({ "name": "Forged", "owner_type": "user", "owner_id": "alice" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "attributing ownership to another user must be rejected"
    );

    // Bob creating a dataset owned by himself → allowed.
    let resp = test_app(state.clone())
        .oneshot(post_json(
            "/api/datasets",
            &bob,
            serde_json::json!({ "name": "Mine", "owner_type": "user", "owner_id": "bob" }),
        ))
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "a self-owned dataset must be creatable, got {}",
        resp.status()
    );
}

// ───────────────────── the boundary helper directly (invariant) ─────────────────────

#[test]
fn authorize_dataset_graph_target_invariants_security() {
    let db = AuthDb::in_memory().unwrap();
    db.create_user("u", "u", "u@t.com", "h", SystemRole::User)
        .unwrap();
    db.create_dataset(
        "mine",
        "mine",
        None,
        OwnerType::User,
        "u",
        Visibility::Private,
        None,
    )
    .unwrap();
    db.create_dataset(
        "other",
        "other",
        None,
        OwnerType::User,
        "u",
        Visibility::Private,
        None,
    )
    .unwrap();
    db.add_dataset_graph("other", "http://shared.example/claimed")
        .unwrap();

    let ok = |g: &str| authorize_dataset_graph_target(&db, BASE, "mine", g).is_ok();

    // Own namespaces (both schemes) and unclaimed external graphs are allowed.
    assert!(ok(&format!("{BASE}/dataset/mine/instances")));
    assert!(ok("urn:dataset:mine:shapes"));
    assert!(ok("http://my.example/g"));
    // Foreign reserved namespaces are rejected.
    assert!(!ok("urn:dataset:other:shapes"));
    assert!(!ok(&format!("{BASE}/dataset/other/instances")));
    assert!(!ok("urn:system:metadata:dataset:other"));
    // A graph already claimed by another dataset is rejected.
    assert!(!ok("http://shared.example/claimed"));
    // Prefix-collision guard: `mine` must not match `mine2`.
    assert!(!ok(&format!("{BASE}/dataset/mine2/instances")));
    assert!(!ok("urn:dataset:mine2:shapes"));
}

// ───────── HIGH: registry-promotion cross-owner injection (PR #70 follow-up) ─────────
//
// Setting a dataset's graph role to `model`/`vocabulary` promotes it into the model
// registry under `slugify(dataset.name)`. Because that id is derived from the
// free-form, non-unique dataset name, a same-slug registry entry may belong to a
// different owner. The promote path must apply the same `can_write_ontology` gate as
// every other registry write, or a dataset writer could publish their RDF as another
// owner's model version.

fn put_json(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Create a registry entry owned by `owner` with NO published version yet — the
/// vulnerable state (e.g. a freshly created entry from the UI).
fn make_versionless_model(state: &AppState, id: &str, title: &str, owner: &str) {
    registry::insert_data_model(
        &state.store,
        &state.base_url,
        id,
        title,
        "",
        None,
        false,
        Some("user"),
        Some(owner),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
}

#[tokio::test]
async fn cannot_promote_into_another_owners_registry_entry_security() {
    let (state, _admin) = admin_state();
    make_user(&state, "victim");
    make_versionless_model(&state, "customer-model", "Customer Model", "victim");
    assert!(!registry::version_exists(
        &state.store,
        &state.base_url,
        "customer-model",
        "1.0.0"
    ));

    // Attacker owns a dataset whose NAME slugifies to the victim's registry id.
    let attacker = make_user(&state, "attacker");
    state
        .auth_db
        .create_dataset(
            "attackerds",
            "Customer Model",
            None,
            OwnerType::User,
            "attacker",
            Visibility::Private,
            None,
        )
        .unwrap();

    // The role update succeeds (promotion is best-effort), ...
    let resp = test_app(state.clone())
        .oneshot(put_json(
            "/api/datasets/attackerds/role",
            &attacker,
            serde_json::json!({ "graph_role": "model" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // ... but the attacker's data must NOT have been injected as the victim entry's
    // published 1.0.0 version.
    assert!(
        !registry::version_exists(&state.store, &state.base_url, "customer-model", "1.0.0"),
        "attacker injected a 1.0.0 version into the victim's registry entry"
    );
}

#[tokio::test]
async fn owner_can_still_promote_into_own_registry_entry_security() {
    // Positive control: the fix must not over-restrict the owner's own promotion.
    let (state, _admin) = admin_state();
    let owner = make_user(&state, "owner");
    make_versionless_model(&state, "my-model", "My Model", "owner");
    state
        .auth_db
        .create_dataset(
            "ownerds",
            "My Model",
            None,
            OwnerType::User,
            "owner",
            Visibility::Private,
            None,
        )
        .unwrap();

    let resp = test_app(state.clone())
        .oneshot(put_json(
            "/api/datasets/ownerds/role",
            &owner,
            serde_json::json!({ "graph_role": "model" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(
        registry::version_exists(&state.store, &state.base_url, "my-model", "1.0.0"),
        "owner's own promotion should have created the 1.0.0 version"
    );
}

// ─────────────── HIGH: cross-org group-member IDOR (org/group scope) ───────────────
//
// GET/POST `/api/organisations/:org_id/groups/:group_id/members` and
// DELETE `…/members/:user_id` only checked the caller's role in the path's
// `org_id` — the segment the caller controls — never that `group_id` actually
// belongs to `org_id`. So an admin of ANY org could list, add (including
// themselves → privilege escalation) or remove members of another org's group by
// naming that group under their own org's path. Each verb must `404` on the
// mismatch (matching `get_group`/`update_group`/`delete_group`), and the target
// group's membership must be untouched.

fn get_auth(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

fn del_auth(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// Two orgs owned by different admins: `alice_x` admins org A, `bob_x` admins
/// org B, and org B owns `groupB` whose sole member is `victim_x`. Returns
/// alice's token (a *full org-A admin*, so only the org/group-scope guard — not
/// the per-org admin check — can keep her out of org B's group).
fn two_orgs_one_group(state: &AppState) -> String {
    let alice = make_user(state, "alice_x");
    make_user(state, "bob_x");
    make_user(state, "victim_x");
    state
        .auth_db
        .create_organisation("orgA", "Org A", "org-a", None, None)
        .unwrap();
    state
        .auth_db
        .create_organisation("orgB", "Org B", "org-b", None, None)
        .unwrap();
    state
        .auth_db
        .add_org_member("alice_x", "orgA", Role::Admin)
        .unwrap();
    state
        .auth_db
        .add_org_member("bob_x", "orgB", Role::Admin)
        .unwrap();
    state
        .auth_db
        .create_group("groupB", "orgB", "Group B", None)
        .unwrap();
    state
        .auth_db
        .add_group_member("victim_x", "groupB", Role::Member)
        .unwrap();
    alice
}

#[tokio::test]
async fn group_member_ops_scoped_to_path_org_security() {
    let (state, _admin) = admin_state();
    let alice = two_orgs_one_group(&state);

    // LIST org B's group through org A's path → 404.
    let resp = test_app(state.clone())
        .oneshot(get_auth(
            "/api/organisations/orgA/groups/groupB/members",
            &alice,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "listing another org's group members must 404"
    );

    // ADD herself into org B's group (privilege escalation) → 404.
    let resp = test_app(state.clone())
        .oneshot(post_json(
            "/api/organisations/orgA/groups/groupB/members",
            &alice,
            serde_json::json!({ "user_id": "alice_x", "role": "admin" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "adding a member to another org's group must 404"
    );

    // REMOVE org B's group member → 404.
    let resp = test_app(state.clone())
        .oneshot(del_auth(
            "/api/organisations/orgA/groups/groupB/members/victim_x",
            &alice,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "removing a member from another org's group must 404"
    );

    // The group's membership is exactly as seeded: victim only, and no alice.
    let ids: Vec<String> = state
        .auth_db
        .list_group_members("groupB")
        .unwrap()
        .into_iter()
        .map(|(u, _)| u.id)
        .collect();
    assert_eq!(
        ids,
        vec!["victim_x".to_string()],
        "org B's group membership must be unchanged: {ids:?}"
    );
}

#[tokio::test]
async fn group_member_ops_work_for_legitimate_org_admin_security() {
    // Positive control: the scope guard must not break the owning org's admin
    // operating on that org's own group.
    let (state, _admin) = admin_state();
    let _alice = two_orgs_one_group(&state);
    let bob = mint_token("bob_x", "bob_x", "user");

    // LIST through the correct org path → 200.
    let resp = test_app(state.clone())
        .oneshot(get_auth(
            "/api/organisations/orgB/groups/groupB/members",
            &bob,
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "the owning org's admin must still list its group members"
    );

    // ADD through the correct org path → 201.
    make_user(&state, "newbie_x");
    let resp = test_app(state.clone())
        .oneshot(post_json(
            "/api/organisations/orgB/groups/groupB/members",
            &bob,
            serde_json::json!({ "user_id": "newbie_x", "role": "member" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "the owning org's admin must still add members"
    );
    assert!(
        state
            .auth_db
            .list_group_members("groupB")
            .unwrap()
            .iter()
            .any(|(u, _)| u.id == "newbie_x"),
        "a legitimate add must take effect"
    );
}

// ─────────────── MEDIUM: publish-gate bypass via update_dataset ───────────────
//
// `create_dataset` gates public creation on `is_publisher()`, but `update_dataset`
// gated a visibility change on `can_manage()` alone. A user with manage but not
// publish rights could therefore create a dataset private and then `PUT` it
// public, sidestepping the publisher gate. Only the transition *into* public is
// gated: an unchanged or narrowing visibility must still succeed.

#[tokio::test]
async fn non_publisher_cannot_publish_via_update_dataset_security() {
    let (state, _admin) = admin_state();
    // `np` owns (hence manages) the dataset but has no publish capability.
    let np = make_user(&state, "np");
    make_dataset(&state, "npds", "np"); // private

    let resp = test_app(state.clone())
        .oneshot(put_json(
            "/api/datasets/npds",
            &np,
            serde_json::json!({ "name": "npds", "visibility": "public" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "a non-publisher must not be able to make a dataset public"
    );

    // The visibility must be untouched by the rejected request.
    let ds = state.auth_db.get_dataset("npds").unwrap().unwrap();
    assert_eq!(
        ds.visibility,
        Visibility::Private,
        "the dataset must remain private"
    );
}

#[tokio::test]
async fn publisher_can_publish_via_update_dataset_security() {
    // Positive control: a genuine publisher (not a platform admin) may widen to
    // public.
    let (state, _admin) = admin_state();
    let pubu = make_user(&state, "pubu");
    state.auth_db.update_user_can_publish("pubu", true).unwrap();
    make_dataset(&state, "pubds", "pubu"); // private

    let resp = test_app(state.clone())
        .oneshot(put_json(
            "/api/datasets/pubds",
            &pubu,
            serde_json::json!({ "name": "pubds", "visibility": "public" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "a publisher must be able to make their dataset public"
    );
    let ds = state.auth_db.get_dataset("pubds").unwrap().unwrap();
    assert_eq!(
        ds.visibility,
        Visibility::Public,
        "the dataset must now be public"
    );
}

#[tokio::test]
async fn non_publisher_can_edit_already_public_dataset_security() {
    // Regression guard: the gate only blocks the transition INTO public. The
    // metadata dialog resends the dataset's current visibility on every save, so a
    // non-publisher owner editing an already-public dataset must still succeed.
    let (state, _admin) = admin_state();
    let np = make_user(&state, "np2");
    state
        .auth_db
        .create_dataset(
            "pubalready",
            "Public Already",
            None,
            OwnerType::User,
            "np2",
            Visibility::Public,
            None,
        )
        .unwrap();

    let resp = test_app(state.clone())
        .oneshot(put_json(
            "/api/datasets/pubalready",
            &np,
            serde_json::json!({ "name": "Renamed", "visibility": "public" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "editing an already-public dataset must not require publisher rights"
    );
    let ds = state.auth_db.get_dataset("pubalready").unwrap().unwrap();
    assert_eq!(ds.name, "Renamed", "the metadata edit must have applied");
    assert_eq!(
        ds.visibility,
        Visibility::Public,
        "visibility must remain public"
    );
}
