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
    let (state, token) = admin_state();
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
