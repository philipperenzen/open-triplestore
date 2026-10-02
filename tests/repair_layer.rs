//! The repair layer over HTTP (`docs/repair.md`,
//! `docs/notes/repair-layer-design.md` §8 and its test plan): who may ask
//! for a proposal, what the proposal says, that it is deterministic and
//! idempotent, and how it is kept, reviewed and applied.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{GraphKind, OwnerType, ResourceRole, SystemRole, Visibility};
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const DATA: &str = "http://example.org/graph/instances";
const SHAPES: &str = "http://example.org/graph/shapes";
const RULES: &str = "http://example.org/graph/rules";

const SHAPES_TTL: &str = r#"
@prefix ex: <http://example.org/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:BridgeShape a sh:NodeShape ; sh:targetClass ex:Bridge ;
  sh:property [ sh:path ex:hasDeck ; sh:minCount 1 ; sh:class ex:Deck ] ;
  sh:property [ sh:path ex:status ; sh:hasValue ex:Active ] ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:datatype xsd:string ] .
"#;

const DATA_TTL: &str = r#"
@prefix ex: <http://example.org/> .
ex:b1 a ex:Bridge ; ex:name "One" .
ex:b2 a ex:Bridge ; ex:name "Two" ; ex:hasDeck ex:d2 ; ex:status ex:Active .
ex:b3 a ex:Bridge .
"#;

/// An admin state with dataset `ds` (owned by `adm`): an instances graph
/// and a shapes-role graph.
fn bridge_state() -> (AppState, String) {
    bridge_state_over(open_triplestore::store::TripleStore::in_memory().unwrap())
}

/// As [`bridge_state`], around a store the test built.
fn bridge_state_over(store: open_triplestore::store::TripleStore) -> (AppState, String) {
    let (state, token) = admin_state_with_store(store);
    state
        .auth_db
        .create_dataset(
            "ds",
            "Bridges",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    for g in [DATA, SHAPES] {
        state.auth_db.add_dataset_graph("ds", g).unwrap();
    }
    state
        .auth_db
        .set_dataset_graph_role("ds", SHAPES, Some(GraphKind::Shapes))
        .unwrap();
    state
        .store
        .load_str(SHAPES_TTL, RdfFormat::Turtle, Some(SHAPES))
        .unwrap();
    state
        .store
        .load_str(DATA_TTL, RdfFormat::Turtle, Some(DATA))
        .unwrap();
    (state, token)
}

fn user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@test.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
    accept: Option<&str>,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(a) = accept {
        b = b.header(header::ACCEPT, a);
    }
    let req = match body {
        Some(v) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

async fn repair(app: &Router, token: &str, body: Value) -> (StatusCode, Value) {
    let (s, text) = send(
        app,
        Method::POST,
        "/api/datasets/ds/repair",
        Some(token),
        Some(body),
        None,
    )
    .await;
    (
        s,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

fn ask(state: &AppState, q: &str) -> bool {
    matches!(state.store.query(q), Ok(QueryResults::Boolean(true)))
}

// ── Access ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_anonymous_caller_is_refused_with_401() {
    let (state, _) = bridge_state();
    let app = test_app(state);
    let (s, _) = send(
        &app,
        Method::POST,
        "/api/datasets/ds/repair",
        None,
        Some(json!({})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = send(
        &app,
        Method::GET,
        "/api/datasets/ds/repair/proposals",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_unknown_or_invisible_dataset_is_404() {
    let (state, token) = bridge_state();
    let outsider = user(&state, "outsider");
    let app = test_app(state);
    let (s, _) = send(
        &app,
        Method::POST,
        "/api/datasets/nope/repair",
        Some(&token),
        Some(json!({})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    // A private dataset the caller has no grant on does not exist for them.
    let (s, _) = repair(&app, &outsider, json!({})).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// A proposal quotes the triples it would delete, and private graphs are
/// read only by a dataset's writers: a viewer gets 403, and so does a
/// read-scoped API token (a POST is a mutation to the token check).
#[tokio::test]
async fn a_viewer_or_a_read_scoped_token_is_refused_with_403() {
    use open_triplestore::auth::jwt::{generate_api_token, hash_token};
    use open_triplestore::auth::models::ApiScope;
    let (state, _) = bridge_state();
    let viewer = user(&state, "viewer");
    state
        .auth_db
        .set_resource_grant(
            "dataset",
            "ds",
            "user",
            "viewer",
            ResourceRole::Viewer,
            "adm",
        )
        .unwrap();
    // A writer of the dataset holding a read-scoped token (an admin's token
    // is exempt from the scope check by design).
    let _ = user(&state, "editor");
    state
        .auth_db
        .set_resource_grant(
            "dataset",
            "ds",
            "user",
            "editor",
            ResourceRole::Editor,
            "adm",
        )
        .unwrap();
    let raw = generate_api_token();
    state
        .auth_db
        .create_api_token(
            "tok",
            "editor",
            "read-only",
            &hash_token(&raw),
            &format!("{}...", &raw[..11]),
            &[ApiScope::Read],
            None,
        )
        .unwrap();
    let app = test_app(state);
    let (s, body) = repair(&app, &viewer, json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body, Value::String("Write access required".into()));
    let (s, _) = repair(&app, &raw, json!({})).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

// ── Refusals ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn no_shapes_is_400_like_validate_and_infer() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "ds",
            "Empty",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("ds", DATA).unwrap();
    let app = test_app(state);
    let (s, body) = repair(&app, &token, json!({})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(
        body.as_str().unwrap().starts_with("No shapes graph found"),
        "{body}"
    );
    // Without shapes, OWL-only runs are still possible when asked for.
    let (s, _) = repair(&app, &token, json!({ "derive": { "from_shapes": false } })).await;
    assert_eq!(s, StatusCode::OK);
}

async fn rules_graph(state: &AppState, ttl: &str) {
    state
        .store
        .load_str(
            &format!("@prefix ots: <https://opentriplestore.org/ns#> .\n@prefix ex: <http://example.org/> .\n{ttl}"),
            RdfFormat::Turtle,
            Some(RULES),
        )
        .unwrap();
}

/// §3.4: a cycle through negation, or through a retract, is a load-time
/// 400 naming the rules.
#[tokio::test]
async fn an_unstratifiable_rule_set_is_400_naming_the_rules() {
    let (state, token) = bridge_state();
    rules_graph(
        &state,
        r#"
<urn:rule:a> a ots:Rule ; ots:construct "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:p true } WHERE { ?x a ex:Bridge FILTER NOT EXISTS { ?x ex:q true } }" .
<urn:rule:b> a ots:Rule ; ots:construct "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:q true } WHERE { ?x a ex:Bridge FILTER NOT EXISTS { ?x ex:p true } }" .
"#,
    )
    .await;
    let app = test_app(state);
    let (s, body) = repair(
        &app,
        &token,
        json!({ "rules": [RULES], "derive": { "from_shapes": false, "from_owl": false } }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    let msg = body.as_str().unwrap();
    assert!(
        msg.contains("cannot be stratified")
            && msg.contains("urn:rule:a")
            && msg.contains("urn:rule:b"),
        "{msg}"
    );
}

#[tokio::test]
async fn a_retract_cycle_is_400() {
    let (state, token) = bridge_state();
    rules_graph(
        &state,
        r#"
<urn:rule:closure> a ots:Rule ; ots:construct "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:hasPart ?z } WHERE { ?x ex:hasPart ?y . ?y ex:hasPart ?z }" .
<urn:rule:prune> a ots:Rule ; ots:construct "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:pruned true } WHERE { ?x ex:hasPart ?y . ?y a ex:Scrap }" ;
  ots:retract "?x ex:hasPart ?y" .
"#,
    )
    .await;
    let app = test_app(state);
    let (s, body) = repair(
        &app,
        &token,
        json!({ "rules": [RULES], "derive": { "from_shapes": false, "from_owl": false } }),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.as_str().unwrap().contains("urn:rule:prune"), "{body}");
}

#[tokio::test]
async fn a_malformed_rule_or_request_is_400() {
    let (state, token) = bridge_state();
    rules_graph(
        &state,
        r#"<urn:rule:bad> a ots:Rule ; ots:construct "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:hasDeck _:w } WHERE { ?x a ex:Bridge }" ."#,
    )
    .await;
    let app = test_app(state);
    let (s, body) = repair(&app, &token, json!({ "rules": [RULES] })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body.as_str().unwrap().contains("ots:nulls"), "{body}");
    let (s, body) = repair(&app, &token, json!({ "policies": ["delete-everything"] })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(body.as_str().unwrap().contains("unknown policy"), "{body}");
    let (s, _) = send(
        &app,
        Method::POST,
        "/api/datasets/ds/repair",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "an empty body is the default request");
    let mut b = Request::builder()
        .method(Method::POST)
        .uri("/api/datasets/ds/repair")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json");
    b = b.header(header::ACCEPT, "application/json");
    let resp = app
        .clone()
        .oneshot(b.body(Body::from("{ not json")).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// §5.4: premises over the quad cap are refused before anything is copied.
#[tokio::test]
async fn premises_over_the_cap_are_400_naming_the_limit() {
    let (state, token) = bridge_state();
    open_triplestore::repair::runtime(&state.store).set_max_quads(3);
    let app = test_app(state);
    let (s, body) = repair(&app, &token, json!({})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(
        body.as_str().unwrap().contains("OTS_REPAIR_MAX_QUADS"),
        "{body}"
    );
}

/// §5.4: the dedicated repair semaphore (default one run) answers 503 when
/// it is held.
#[tokio::test]
async fn a_full_repair_semaphore_is_503() {
    let (state, token) = bridge_state();
    let rt = open_triplestore::repair::runtime(&state.store);
    let _held = rt.semaphore.clone().try_acquire_owned().unwrap();
    let app = test_app(state);
    let (s, body) = repair(&app, &token, json!({})).await;
    assert_eq!(s, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body, Value::String("Server overloaded".into()));
}

// ── The proposal ────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_proposal_carries_a_patch_and_explains_every_line() {
    let (state, token) = bridge_state();
    let app = test_app(state);
    let (s, r) = repair(&app, &token, json!({ "validate": true })).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["schema"], 1);
    assert_eq!(r["status"], "proposed");
    assert_eq!(r["partial"], false);
    let id = r["proposal_id"].as_str().unwrap();
    assert!(id.starts_with("urn:ots:proposal:"));
    // b1 and b3 need a deck (a typed witness) and the status; b2's deck
    // needs its type; b3 also lacks a name, which no rule can supply (a null
    // is never a literal).
    let adds = r["summary"]["adds"].as_u64().unwrap();
    assert_eq!(adds, 3 + 1 + 3, "{r:#}");
    assert_eq!(r["summary"]["deletes"], 0);
    assert_eq!(r["summary"]["nulls"], 2);
    assert!(r["summary"]["exhausted"].is_null());
    // Before: b1 (deck, status), b2 (deck class), b3 (deck, status, name).
    // After: b3's name.
    assert_eq!(r["validation"]["before"]["violation"], 6);
    assert_eq!(r["validation"]["after"]["violation"], 1);
    assert_eq!(
        r["validation"]["residual"][0]["focus_node"],
        "http://example.org/b3"
    );
    let report_only = r["report_only"].as_array().unwrap();
    let name = report_only
        .iter()
        .find(|e| {
            e["path"] == "<http://example.org/name>"
                && e["source_constraint_component"]
                    .as_str()
                    .unwrap()
                    .ends_with("MinCountConstraintComponent")
        })
        .unwrap();
    assert_eq!(name["triggers"], 1, "the residual is attributed: {name}");
    assert!(name["reason"].as_str().unwrap().contains("never a literal"));
    // Every action names its rule, trigger, premises, the violation it
    // answers and why.
    for a in r["actions"].as_array().unwrap() {
        assert_eq!(a["op"], "A");
        assert_eq!(a["graph"], DATA);
        assert!(a["rule"].as_str().unwrap().starts_with("urn:ots:rule:"));
        assert_eq!(a["confidence"], "certain");
        assert!(!a["trigger"]["premises"].as_array().unwrap().is_empty());
        assert!(a["violation"]["source_constraint_component"]
            .as_str()
            .unwrap()
            .starts_with("http://www.w3.org/ns/shacl#"));
        assert!(a["explanation"].as_str().is_some());
    }
    // The compiled set is returned, and as Turtle that loads back as rules.
    assert!(r["rules"]["compiled"].as_u64().unwrap() >= 3);
    let ttl = r["rules"]["turtle"].as_str().unwrap();
    assert!(ttl.contains("a ots:Rule"));
    // The patch parses, is appliable and says it is complete.
    let patch = r["patch"].as_str().unwrap();
    let p = open_triplestore::rdf_patch::parse(patch).unwrap();
    assert_eq!(p.id(), Some(id));
    assert!(patch.contains("H complete \"true\" ."));
    assert!(patch.contains("H engine \"ots-chase/0.1\" ."));
    assert!(r["prov"].as_str().unwrap().contains("prov:Activity"));
}

#[tokio::test]
async fn accept_rdf_patch_returns_the_patch_text() {
    let (state, token) = bridge_state();
    let app = test_app(state);
    let (s, text) = send(
        &app,
        Method::POST,
        "/api/datasets/ds/repair",
        Some(&token),
        Some(json!({})),
        Some("application/rdf-patch"),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(text.starts_with("H id <urn:ots:proposal:"), "{text}");
    assert!(open_triplestore::rdf_patch::parse(&text).is_ok());
}

/// §9 test plan: byte-identical patch text, and the report minus its
/// volatile fields, across two runs over one snapshot.
#[tokio::test]
async fn two_runs_over_one_state_agree_byte_for_byte() {
    let (state, token) = bridge_state();
    let app = test_app(state);
    let (_, a) = repair(&app, &token, json!({ "validate": true })).await;
    let (_, b) = repair(&app, &token, json!({ "validate": true })).await;
    assert_eq!(a["patch"], b["patch"]);
    assert_eq!(
        open_triplestore::repair::proposal::stable_view(&a),
        open_triplestore::repair::proposal::stable_view(&b)
    );
}

/// §9 test plan: applying a proposal and asking again proposes nothing.
#[tokio::test]
async fn an_applied_proposal_leaves_nothing_to_propose() {
    let (state, token) = bridge_state();
    let app = test_app(state.clone());
    let (_, r) = repair(&app, &token, json!({})).await;
    let patch = r["patch"].as_str().unwrap().to_string();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/datasets/ds/patch")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/rdf-patch")
        .body(Body::from(patch))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(ask(&state, &format!("ASK {{ GRAPH <{DATA}> {{ <http://example.org/b1> <http://example.org/status> <http://example.org/Active> }} }}")));
    let (s, again) = repair(&app, &token, json!({})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(again["summary"]["adds"], 0, "{again:#}");
    assert_eq!(again["summary"]["deletes"], 0);
}

/// A budget that runs out is not an error: what was proposed by then is a
/// valid partial repair, marked incomplete.
#[tokio::test]
async fn an_exhausted_budget_is_a_partial_proposal() {
    let (state, token) = bridge_state();
    rules_graph(
        &state,
        r#"<urn:rule:chain> a ots:Rule ;
  ots:construct "PREFIX ex: <http://example.org/> CONSTRUCT { ?x ex:next ?w . ?w a ex:Bridge } WHERE { ?x a ex:Bridge }" ;
  ots:nulls ( "w" ) ."#,
    )
    .await;
    let app = test_app(state);
    let (s, r) = repair(
        &app,
        &token,
        json!({
            "rules": [RULES],
            "derive": { "from_shapes": false, "from_owl": false },
            "budget": { "nulls": 7 }
        }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["summary"]["exhausted"], "nulls");
    assert!(r["patch"]
        .as_str()
        .unwrap()
        .contains("H complete \"false\" ."));
    assert!(r["summary"]["adds"].as_u64().unwrap() > 0);
}

/// `scope.focus` limits the focus nodes, not the copy.
#[tokio::test]
async fn scope_focus_limits_the_focus_nodes() {
    let (state, token) = bridge_state();
    let app = test_app(state);
    let (s, r) = repair(
        &app,
        &token,
        json!({ "scope": { "focus": ["http://example.org/b3"] } }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    for a in r["actions"].as_array().unwrap() {
        assert_eq!(
            a["trigger"]["bindings"]["this"], "<http://example.org/b3>",
            "{a}"
        );
    }
    assert_eq!(r["summary"]["adds"], 3);
}

// ── Kept proposals ──────────────────────────────────────────────────────────

async fn get_json(app: &Router, token: &str, uri: &str) -> (StatusCode, Value) {
    let (s, text) = send(app, Method::GET, uri, Some(token), None, None).await;
    (
        s,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// §7.3 and §8.2: a kept proposal is listed, read page by page, goes stale
/// when the dataset moves, and can be rejected.
#[tokio::test]
async fn a_kept_proposal_is_listed_paged_superseded_and_rejected() {
    let (state, token) = bridge_state();
    let app = test_app(state.clone());
    let (s, r) = repair(&app, &token, json!({ "persist": true })).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["persisted"], true);
    let pid = r["proposal_id"].as_str().unwrap().to_string();
    let hash = pid.trim_start_matches("urn:ots:proposal:");

    let (s, list) = get_json(&app, &token, "/api/datasets/ds/repair/proposals").await;
    assert_eq!(s, StatusCode::OK);
    let first = &list["proposals"][0];
    assert_eq!(first["proposal_id"], pid.as_str());
    assert_eq!(first["status"], "proposed");
    assert_eq!(first["stale"], false);

    // Either form of the id addresses it; actions are paged.
    let (s, got) = get_json(
        &app,
        &token,
        &format!("/api/datasets/ds/repair/proposals/{hash}?offset=1&limit=2"),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{got}");
    assert_eq!(got["actions"].as_array().unwrap().len(), 2);
    assert_eq!(got["actions_offset"], 1);
    assert_eq!(got["actions_total"], r["actions_total"]);
    assert_eq!(got["actions"][0], r["actions"][1]);
    assert_eq!(got["patch"], r["patch"]);
    let (s, text) = send(
        &app,
        Method::GET,
        &format!("/api/datasets/ds/repair/proposals/{pid}"),
        Some(&token),
        None,
        Some("application/rdf-patch"),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(text, r["patch"].as_str().unwrap());
    let (s, _) = get_json(
        &app,
        &token,
        "/api/datasets/ds/repair/proposals/urn:ots:proposal:0123456789abcdef0123456789abcdef",
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = get_json(
        &app,
        &token,
        "/api/datasets/ds/repair/proposals/..%2F..%2Fetc",
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // A write to the dataset after the proposal was computed supersedes it.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/datasets/ds/patch")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/rdf-patch")
        .body(Body::from(format!(
            "TX .\nA <http://example.org/b9> <http://example.org/name> \"Nine\" <{DATA}> .\nTC .\n"
        )))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
    let (_, got) = get_json(
        &app,
        &token,
        &format!("/api/datasets/ds/repair/proposals/{pid}"),
    )
    .await;
    assert_eq!(got["stale"], true);
    assert_eq!(got["status"], "superseded");

    let (s, rej) = send(
        &app,
        Method::POST,
        &format!("/api/datasets/ds/repair/proposals/{pid}/reject"),
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{rej}");
    let (_, got) = get_json(
        &app,
        &token,
        &format!("/api/datasets/ds/repair/proposals/{pid}"),
    )
    .await;
    assert_eq!(got["status"], "rejected");
}

/// Kept proposals are files under the configured directory, a `.json` and a
/// `.patch` per proposal, never store writes.
#[tokio::test]
async fn kept_proposals_are_files_beside_the_store() {
    let (state, token) = bridge_state();
    let dir = tempfile::tempdir().unwrap();
    open_triplestore::repair::configure(&state.store, dir.path());
    let quads_before = state.store.len().unwrap();
    let app = test_app(state.clone());
    let (s, r) = repair(&app, &token, json!({ "persist": true })).await;
    assert_eq!(s, StatusCode::OK);
    let hash = r["proposal_id"]
        .as_str()
        .unwrap()
        .trim_start_matches("urn:ots:proposal:")
        .to_string();
    let base = dir.path().join("repair-proposals").join("ds");
    assert!(base.join(format!("{hash}.json")).exists());
    let patch = std::fs::read_to_string(base.join(format!("{hash}.patch"))).unwrap();
    assert_eq!(patch, r["patch"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(base.join("dataset.txt")).unwrap(),
        "ds"
    );
    assert_eq!(
        state.store.len().unwrap(),
        quads_before,
        "a proposal is never written to the store"
    );
}

// ── Applying (§8.3) ─────────────────────────────────────────────────────────

async fn post_patch(
    app: &Router,
    token: &str,
    query: &str,
    if_match: Option<&str>,
    patch: String,
) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(format!("/api/datasets/ds/patch{query}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/rdf-patch");
    if let Some(m) = if_match {
        b = b.header(header::IF_MATCH, m);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(patch)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let text = body_text(resp.into_body()).await;
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// A patch adding `ex:b{n} ex:name "N{n}"` to the instances graph.
fn name_patch(n: u32) -> String {
    format!(
        "TX .\nA <http://example.org/b{n}> <http://example.org/name> \"N{n}\" <{DATA}> .\nTC .\n"
    )
}

fn has_name(state: &AppState, n: u32) -> bool {
    ask(state, &format!("ASK {{ GRAPH <{DATA}> {{ <http://example.org/b{n}> <http://example.org/name> \"N{n}\" }} }}"))
}

fn newest_commit(state: &AppState) -> Option<open_triplestore::commit_log::CommitRecord> {
    use open_triplestore::commit_log::{list_commits, CommitQuery, CommitScope};
    list_commits(
        &state.store,
        &CommitScope::Graphs(vec![DATA.to_string()]),
        &CommitQuery {
            limit: Some(1),
            ..Default::default()
        },
    )
    .into_iter()
    .next()
}

fn commit_iri(state: &AppState, c: &open_triplestore::commit_log::CommitRecord) -> String {
    format!(
        "{}/commit/{}",
        state.base_url.trim_end_matches('/'),
        c.commit_id
    )
}

async fn apply(app: &Router, token: &str, pid: &str) -> (StatusCode, Value) {
    let (s, text) = send(
        app,
        Method::POST,
        &format!("/api/datasets/ds/repair/proposals/{pid}/apply"),
        Some(token),
        None,
        None,
    )
    .await;
    (
        s,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn kept(app: &Router, token: &str, body: Value) -> String {
    let mut body = body;
    body["persist"] = json!(true);
    let (s, r) = repair(app, token, body).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    r["proposal_id"].as_str().unwrap().to_string()
}

/// §8.2, §8.3 and the test plan's idempotence: a kept proposal applies
/// once, the commit names it, the proposal records the commit, a second
/// apply is refused and nothing is left to propose.
#[tokio::test]
async fn a_kept_proposal_applies_once_and_the_commit_names_it() {
    let (state, token) = bridge_state();
    let app = test_app(state.clone());
    let (_, r) = repair(&app, &token, json!({ "persist": true })).await;
    let pid = r["proposal_id"].as_str().unwrap().to_string();
    let (s, a) = apply(&app, &token, &pid).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert_eq!(a["applied"], true);
    assert_eq!(a["status"], "applied");
    assert_eq!(a["added"], r["summary"]["adds"]);
    assert_eq!(a["removed"], 0);
    assert_eq!(a["graphs"], json!([DATA]));
    assert!(ask(&state, &format!("ASK {{ GRAPH <{DATA}> {{ <http://example.org/b1> <http://example.org/status> <http://example.org/Active> }} }}")));

    let c = newest_commit(&state).unwrap();
    assert_eq!(a["commit"], commit_iri(&state, &c));
    assert!(
        c.message.starts_with(&format!("Repair {pid}: +")),
        "{}",
        c.message
    );
    let meta = c.metadata.unwrap();
    assert_eq!(meta["repair"]["proposal"], pid.as_str());
    assert_eq!(meta["repair"]["engine"], "ots-chase/0.1");
    assert_eq!(meta["repair"]["rules"], r["rules"]["digest"]);

    let (_, got) = get_json(
        &app,
        &token,
        &format!("/api/datasets/ds/repair/proposals/{pid}"),
    )
    .await;
    assert_eq!(got["status"], "applied");
    assert_eq!(got["applied_commit"], a["commit"]);

    let (s, again) = apply(&app, &token, &pid).await;
    assert_eq!(s, StatusCode::CONFLICT, "{again}");
    assert_eq!(again["error"], "not_proposed");
    let (_, next) = repair(&app, &token, json!({})).await;
    assert_eq!(next["summary"]["adds"], 0, "{next:#}");
}

/// §9: the base moved before the apply → 409, and the proposal is
/// superseded; nothing is written.
#[tokio::test]
async fn a_proposal_whose_base_moved_is_409_and_superseded() {
    let (state, token) = bridge_state();
    let app = test_app(state.clone());
    let pid = kept(&app, &token, json!({})).await;
    assert_eq!(
        post_patch(&app, &token, "", None, name_patch(9)).await.0,
        StatusCode::OK
    );
    let (s, body) = apply(&app, &token, &pid).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "stale_base");
    assert_eq!(body["precondition"], "if-base-commit");
    let (_, got) = get_json(
        &app,
        &token,
        &format!("/api/datasets/ds/repair/proposals/{pid}"),
    )
    .await;
    assert_eq!(got["status"], "superseded");
    assert!(!ask(&state, &format!("ASK {{ GRAPH <{DATA}> {{ <http://example.org/b1> <http://example.org/status> <http://example.org/Active> }} }}")));
}

/// The apply runs the dataset's write gates (always: a Graph Store write to
/// the same graph would). A refusal is the gate's 422 and leaves the
/// proposal `proposed`; once the residual is gone the next proposal applies.
#[tokio::test]
async fn the_apply_runs_the_write_gates_and_a_refusal_keeps_the_proposal() {
    let (state, token) = bridge_state();
    state
        .auth_db
        .update_dataset_shacl("ds", true, Some(SHAPES))
        .unwrap();
    let app = test_app(state.clone());
    // b3 has no name, which no rule supplies: the repaired graph still
    // violates the shapes the gate runs.
    let pid = kept(&app, &token, json!({})).await;
    let (s, body) = apply(&app, &token, &pid).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert!(!body["results"].as_array().unwrap().is_empty(), "{body}");
    let (_, got) = get_json(
        &app,
        &token,
        &format!("/api/datasets/ds/repair/proposals/{pid}"),
    )
    .await;
    assert_eq!(got["status"], "proposed");

    state
        .store
        .load_str(
            "<http://example.org/b3> <http://example.org/name> \"Three\" .",
            RdfFormat::NTriples,
            Some(DATA),
        )
        .unwrap();
    let pid = kept(&app, &token, json!({})).await;
    let (s, body) = apply(&app, &token, &pid).await;
    assert_eq!(s, StatusCode::OK, "{body}");
}

/// The patch route's opt-in base-commit precondition, by query or
/// `If-Match`, over the dataset's graphs. Without it the route is
/// unchanged.
#[tokio::test]
async fn the_patch_route_checks_a_base_commit_when_asked() {
    let (state, token) = bridge_state();
    let app = test_app(state.clone());
    // No commit has touched the graph: an empty base matches.
    let (s, body) = post_patch(&app, &token, "?if-base-commit=", None, name_patch(10)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    // Now one has.
    let newest = commit_iri(&state, &newest_commit(&state).unwrap());
    let (s, body) = post_patch(&app, &token, "?if-base-commit=", None, name_patch(11)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "stale_base");
    assert_eq!(body["current"], newest.as_str());
    assert!(!has_name(&state, 11), "a refused patch writes nothing");
    let (s, body) = post_patch(
        &app,
        &token,
        &format!("?if-base-commit={newest}"),
        None,
        name_patch(12),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let newest = commit_iri(&state, &newest_commit(&state).unwrap());
    let (s, _) = post_patch(
        &app,
        &token,
        "",
        Some(&format!("\"{newest}\"")),
        name_patch(13),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = post_patch(
        &app,
        &token,
        "",
        Some(&format!("\"{newest}\"")),
        name_patch(14),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, body) = post_patch(&app, &token, "", None, name_patch(15)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["applied"], true);
    assert!(
        has_name(&state, 10)
            && has_name(&state, 12)
            && has_name(&state, 13)
            && has_name(&state, 15)
    );
    // A caller who cannot see the dataset learns nothing from the option.
    let outsider = user(&state, "outsider");
    let (s, _) = post_patch(&app, &outsider, "?if-base-commit=", None, name_patch(16)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// The durable precondition: change-log sequence (and epoch). It sees a
/// write that records no commit, and ignores writes to other graphs.
#[tokio::test]
async fn the_patch_route_checks_a_base_sequence_against_the_change_log() {
    use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
    let (state, token) = bridge_state();
    let app = test_app(state);
    let (s, body) = post_patch(&app, &token, "?if-base-sequence=0", None, name_patch(20)).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(
        body.as_str().unwrap().contains("OTS_CHANGE_CAPTURE"),
        "{body}"
    );

    let store = open_triplestore::store::TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD);
    let (state, token) = bridge_state_over(store);
    let app = test_app(state.clone());
    let changes = state.store.changes();
    let (seq, epoch) = (changes.last_seq(), changes.epoch().to_string());
    let q = |seq: i64, epoch: &str| format!("?if-base-sequence={seq}&if-base-epoch={epoch}");
    let (s, body) = post_patch(&app, &token, &q(seq, &epoch), None, name_patch(21)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, body) = post_patch(&app, &token, &q(seq, &epoch), None, name_patch(22)).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["precondition"], "if-base-sequence");
    assert!(!has_name(&state, 22));

    let seq = changes.last_seq();
    let (s, _) = post_patch(&app, &token, &q(seq, "another-epoch"), None, name_patch(23)).await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = post_patch(&app, &token, &q(seq + 100, &epoch), None, name_patch(24)).await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "a base ahead of the log is not this log's"
    );
    // A write to another graph does not move the base of this one.
    state
        .store
        .load_str(
            "<http://example.org/x> <http://example.org/p> \"1\" .",
            RdfFormat::NTriples,
            Some("http://example.org/graph/other"),
        )
        .unwrap();
    let (s, body) = post_patch(&app, &token, &q(seq, &epoch), None, name_patch(25)).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    // A write that records no commit does.
    let seq = changes.last_seq();
    state
        .store
        .load_str(
            "<http://example.org/x> <http://example.org/p> \"2\" .",
            RdfFormat::NTriples,
            Some(DATA),
        )
        .unwrap();
    let (s, _) = post_patch(&app, &token, &q(seq, &epoch), None, name_patch(26)).await;
    assert_eq!(s, StatusCode::CONFLICT);
}

/// The test plan's last item: a `Rewrite` merge applied through the
/// proposal re-materialises the dataset's entailment, under the
/// `sameas-narrow` and the `sameas-off` identity policy alike — the merge
/// writes no `owl:sameAs`, so neither policy has a link to follow.
#[tokio::test]
async fn a_rewrite_merge_rematerialises_entailment_under_narrow_and_off() {
    for identity in ["sameas-narrow", "sameas-off"] {
        let (state, token) = bridge_state();
        state
            .store
            .load_str(
                r#"@prefix ex: <http://example.org/> . @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
                   ex:Bridge rdfs:subClassOf ex:Structure .
                   ex:k1 ex:code "K1" .
                   ex:k2 ex:code "K1" ; a ex:Bridge ."#,
                RdfFormat::Turtle,
                Some(DATA),
            )
            .unwrap();
        rules_graph(
            &state,
            r#"<urn:rule:code-key> a ots:Rule ;
  ots:construct "PREFIX ex: <http://example.org/> CONSTRUCT {} WHERE { ?x ex:code ?k . ?y ex:code ?k }" ;
  ots:equate ( "x" "y" ) ;
  ots:mergeMode ots:Rewrite ."#,
        )
        .await;
        let app = test_app(state.clone());
        let (s, body) = send(
            &app,
            Method::PUT,
            "/api/datasets/ds/entailment",
            Some(&token),
            Some(json!({ "regime": "rdfs", "mode": "materialize", "identity": identity })),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let entailed = |s: &str| {
            ask(&state, &format!("ASK {{ GRAPH <urn:entailment:rdfs:ds> {{ <http://example.org/{s}> a <http://example.org/Structure> }} }}"))
        };
        assert!(entailed("k2") && !entailed("k1"), "{identity}");

        let pid = kept(
            &app,
            &token,
            json!({ "rules": [RULES], "derive": { "from_shapes": false, "from_owl": false } }),
        )
        .await;
        let (s, a) = apply(&app, &token, &pid).await;
        assert_eq!(s, StatusCode::OK, "{identity}: {a}");
        assert!(ask(&state, &format!("ASK {{ GRAPH <{DATA}> {{ <http://example.org/k1> a <http://example.org/Bridge> }} }}")));
        assert!(
            !ask(
                &state,
                &format!("ASK {{ GRAPH <{DATA}> {{ <http://example.org/k2> ?p ?o }} }}")
            ),
            "{identity}: the loser is gone"
        );
        assert!(!ask(
            &state,
            "ASK { ?a <http://www.w3.org/2002/07/owl#sameAs> ?b }"
        ));
        assert!(
            entailed("k1") && !entailed("k2"),
            "{identity}: re-materialised after the apply"
        );
    }
}

// ── The assistant (§9) ──────────────────────────────────────────────────────

/// A scripted OpenAI-compatible gateway: each completion pops the next reply
/// and keeps the request. One per test binary, on its own runtime thread,
/// with `LLM_GATEWAY_URL` pointing at it.
mod gateway {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};

    use axum::extract::State;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::{json, Value};

    pub struct Gateway {
        pub replies: Mutex<VecDeque<String>>,
        pub prompts: Mutex<Vec<Value>>,
    }

    async fn completions(
        State(gw): State<&'static Gateway>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        gw.prompts.lock().unwrap().push(body);
        let reply = gw.replies.lock().unwrap().pop_front().unwrap_or_default();
        Json(json!({ "choices": [{ "message": { "role": "assistant", "content": reply } }] }))
    }

    pub fn get() -> &'static Gateway {
        static GW: OnceLock<&'static Gateway> = OnceLock::new();
        GW.get_or_init(|| {
            let gw: &'static Gateway = Box::leak(Box::new(Gateway {
                replies: Mutex::new(VecDeque::new()),
                prompts: Mutex::new(Vec::new()),
            }));
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async move {
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    tx.send(listener.local_addr().unwrap()).unwrap();
                    let app = Router::new()
                        .route("/v1/chat/completions", post(completions))
                        .with_state(gw);
                    axum::serve(listener, app).await.unwrap();
                });
            });
            std::env::set_var("LLM_GATEWAY_URL", format!("http://{}", rx.recv().unwrap()));
            gw
        })
    }

    pub fn script(gw: &Gateway, replies: &[&str]) {
        *gw.replies.lock().unwrap() = replies.iter().map(|r| r.to_string()).collect();
        gw.prompts.lock().unwrap().clear();
    }
}

/// The residual of a deterministic run goes to the model; its answer runs
/// as heuristic rules through the chase and comes back as a kept proposal
/// that applies through the gated apply. A rule set the heuristic guard
/// refuses comes back as `rejected`, with no proposal.
#[tokio::test]
async fn the_assistant_answers_the_residual_with_heuristic_rules() {
    let gw = gateway::get();
    let (state, token) = bridge_state();
    let app = test_app(state.clone());
    gateway::script(
        gw,
        &[
            "```turtle\n@prefix ots: <https://opentriplestore.org/ns#> .\n<urn:rule:name> a ots:Rule ;\n  ots:construct \"PREFIX ex: <http://example.org/> CONSTRUCT { ?this ex:name \\\"Unnamed\\\" } WHERE { ?this a ex:Bridge FILTER NOT EXISTS { ?this ex:name ?n } }\" ;\n  ots:message \"{?this} gets a placeholder name\" .\n```",
            "@prefix ots: <https://opentriplestore.org/ns#> .\n<urn:rule:rename> a ots:Rule ;\n  ots:construct \"PREFIX ex: <http://example.org/> CONSTRUCT { ?this ex:name \\\"X\\\" } WHERE { ?this ex:name ?n }\" ;\n  ots:retract \"?this <http://example.org/name> ?n\" .",
        ],
    );
    let (s, text) = send(
        &app,
        Method::POST,
        "/api/llm/shacl",
        Some(&token),
        Some(json!({ "task": "repair", "dataset_id": "ds" })),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{text}");
    let r: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(r["task"], "repair");
    assert!(r["rejected"].is_null(), "{r:#}");
    // The model was shown the residual: b3's missing name.
    let prompt = gw.prompts.lock().unwrap()[0].to_string();
    assert!(
        prompt.contains("http://example.org/b3") && prompt.contains("MinCountConstraintComponent"),
        "{prompt}"
    );
    let p = &r["proposal"];
    assert_eq!(p["rules"]["heuristic"], 1, "{p:#}");
    assert_eq!(p["persisted"], true);
    // The model's rules run on their own: the proposal is theirs alone, one
    // name for the one bridge without one.
    let actions = p["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1, "{p:#}");
    assert_eq!(actions[0]["rule"], "urn:rule:name");
    assert_eq!(actions[0]["confidence"], "heuristic");
    assert_eq!(p["validation"]["before"]["violation"], 6, "{p:#}");
    assert_eq!(p["validation"]["after"]["violation"], 5, "{p:#}");
    let (s, a) = apply(&app, &token, p["proposal_id"].as_str().unwrap()).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert!(ask(&state, &format!("ASK {{ GRAPH <{DATA}> {{ <http://example.org/b3> <http://example.org/name> \"Unnamed\" }} }}")));

    let (s, text) = send(
        &app,
        Method::POST,
        "/api/llm/shacl",
        Some(&token),
        Some(json!({ "task": "repair", "dataset_id": "ds", "residual": { "residual": [] } })),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{text}");
    let r: Value = serde_json::from_str(&text).unwrap();
    assert!(r["proposal"].is_null(), "{r:#}");
    assert!(
        r["rejected"]
            .as_str()
            .unwrap()
            .contains("may not be destructive"),
        "{r:#}"
    );
    // A viewer may not ask.
    let viewer = user(&state, "viewer");
    state
        .auth_db
        .set_resource_grant(
            "dataset",
            "ds",
            "user",
            "viewer",
            ResourceRole::Viewer,
            "adm",
        )
        .unwrap();
    let (s, _) = send(
        &app,
        Method::POST,
        "/api/llm/shacl",
        Some(&viewer),
        Some(json!({ "task": "repair", "dataset_id": "ds" })),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}
