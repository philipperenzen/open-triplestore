//! `GET /api/models/:id/versions/:ver/profile` — the ontology profile an
//! external mapping proposer reads instead of re-implementing the traversal.
//!
//! Acceptance checks, written against the contract in
//! `src/data_models/profile.rs`:
//! * the superclass chain is **transitive** — a class three levels down
//!   reports every ancestor, not just its declared parent;
//! * properties carry domain, range and the literal datatype (and an object
//!   property carries none);
//! * SHACL property shapes come back flattened: a shape reached through
//!   `sh:node` is its own entry with the full path from the root shape, so the
//!   reader never walks `sh:property`/`sh:node`;
//! * enumerations arrive from all three sources — `owl:oneOf`, a SKOS concept
//!   scheme, and an `sh:in` list — in declaration order;
//! * shapes found by validation-layer binding are included *and* attributed,
//!   so a caller can tell where a constraint came from;
//! * an unknown model or version is a clean 404, and a private model is not
//!   readable by someone who may not read it.
//!
//! The fixture ontology is deliberately domain-neutral: four nested classes
//! called LevelA…LevelD and properties named after nothing.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::profile;
use open_triplestore::data_models::registry as dmr;
use open_triplestore::server::AppState;
use serde_json::Value;
use tower::ServiceExt as _;

const NS: &str = "http://example.org/profile-fixture#";

/// The TBox: a four-deep class chain, typed properties, an `owl:oneOf` class
/// and a SKOS scheme. `{BASE}` is substituted rather than formatted, so the
/// `sh:pattern` braces further down need no escaping.
const TBOX: &str = r#"
PREFIX ex:   <http://example.org/profile-fixture#>
PREFIX owl:  <http://www.w3.org/2002/07/owl#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
INSERT DATA {
  GRAPH <{BASE}> {
    ex:LevelA a owl:Class ;
      rdfs:label "Level A"@en ;
      rdfs:comment "The top of the fixture hierarchy."@en .
    ex:LevelB a owl:Class ; rdfs:subClassOf ex:LevelA ; rdfs:label "Level B"@en .
    ex:LevelC a owl:Class ; rdfs:subClassOf ex:LevelB ; rdfs:label "Level C"@en .
    ex:LevelD a owl:Class ; rdfs:subClassOf ex:LevelC ; rdfs:label "Level D"@en .

    ex:relatesTo a owl:ObjectProperty ;
      rdfs:domain ex:LevelC ; rdfs:range ex:LevelD ;
      rdfs:label "relates to"@en ; rdfs:comment "A link between levels."@en .
    ex:code a owl:DatatypeProperty ;
      rdfs:domain ex:LevelC ; rdfs:range xsd:token ; skos:prefLabel "code"@en .
    ex:count a owl:DatatypeProperty ;
      rdfs:domain ex:LevelD ; rdfs:range xsd:integer .
    ex:note a owl:AnnotationProperty .

    ex:Priority a owl:Class ; owl:oneOf ( ex:PriorityLow ex:PriorityHigh ) .
    ex:PriorityLow rdfs:label "Low"@en .
    ex:PriorityHigh rdfs:label "High"@en .

    ex:Scheme a skos:ConceptScheme ; skos:hasTopConcept ex:ConceptOne .
    ex:ConceptOne a skos:Concept ;
      skos:inScheme ex:Scheme ; skos:prefLabel "One"@en ; skos:notation "1" .
    ex:ConceptTwo a skos:Concept ;
      skos:inScheme ex:Scheme ; skos:prefLabel "Two"@en .
  }
}
"#;

/// The shapes sub-graph: a targeted node shape with three property shapes (two
/// anonymous, one reached through `sh:node`) and a named property shape one
/// level down.
const SHAPES: &str = r#"
PREFIX ex:  <http://example.org/profile-fixture#>
PREFIX sh:  <http://www.w3.org/ns/shacl#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
INSERT DATA {
  GRAPH <{SHAPES_GRAPH}> {
    ex:LevelCShape a sh:NodeShape ;
      sh:targetClass ex:LevelC ;
      sh:property [
        sh:path ex:code ;
        sh:datatype xsd:token ;
        sh:minCount 1 ;
        sh:maxCount 1 ;
        sh:pattern "^[A-Z]{2}-[0-9]+$" ;
        sh:name "code" ;
        sh:message "A code is two letters, a dash and digits."
      ] ;
      sh:property [ sh:path ex:state ; sh:in ( ex:StateOne ex:StateTwo ) ] ;
      sh:property [ sh:path ex:relatesTo ; sh:class ex:LevelD ; sh:node ex:LevelDShape ] .

    ex:LevelDShape a sh:NodeShape ; sh:property ex:CountShape .
    ex:CountShape a sh:PropertyShape ;
      sh:path ex:count ; sh:datatype xsd:integer ; sh:minCount 1 .
  }
}
"#;

/// A shape graph that belongs to no model version — reachable only through a
/// validation-layer binding.
const EXTERNAL_SHAPES: &str = r#"
PREFIX ex: <http://example.org/profile-fixture#>
PREFIX sh: <http://www.w3.org/ns/shacl#>
INSERT DATA {
  GRAPH <urn:test:external-shapes> {
    ex:LevelAShape a sh:NodeShape ;
      sh:targetClass ex:LevelA ;
      sh:property [ sh:path ex:note ; sh:maxCount 1 ] .
  }
}
"#;

/// `(state, admin_token)` with data model `m1` at version `1.0.0`. The model is
/// **private**, owned by the admin user, so the happy path also proves an
/// authorised reader still gets it.
fn fixture() -> (AppState, String) {
    let (state, token) = admin_state();
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m1",
        "Fixture model",
        NS,
        None,
        false,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let graph_iri = format!("{base}/data-model/m1/version/1.0.0");
    let shapes_graph = format!("{graph_iri}/shapes");
    dmr::insert_version(
        &state.store,
        &base,
        &DataModelVersion {
            data_model_id: "m1".to_string(),
            version: "1.0.0".to_string(),
            status: VersionStatus::Published,
            graph_iri: graph_iri.clone(),
            sub_graphs: vec![graph_iri.clone(), shapes_graph.clone()],
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();
    state
        .store
        .update(&TBOX.replace("{BASE}", &graph_iri))
        .unwrap();
    state
        .store
        .update(&SHAPES.replace("{SHAPES_GRAPH}", &shapes_graph))
        .unwrap();
    (state, token)
}

async fn get_profile(state: &AppState, token: &str, path: &str) -> (StatusCode, Value) {
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

async fn ok_profile(state: &AppState, token: &str) -> Value {
    let (status, body) = get_profile(state, token, "/api/models/m1/versions/1.0.0/profile").await;
    assert_eq!(status, StatusCode::OK, "profile should be served: {body}");
    body
}

fn term(local: &str) -> String {
    format!("{NS}{local}")
}

/// The class entry for `local`, or a failure naming what was there instead.
fn class_of<'a>(profile: &'a Value, local: &str) -> &'a Value {
    let iri = term(local);
    profile["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["iri"] == Value::String(iri.clone()))
        .unwrap_or_else(|| panic!("no class {iri} in {}", profile["classes"]))
}

fn property_of<'a>(profile: &'a Value, local: &str) -> &'a Value {
    let iri = term(local);
    profile["properties"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["iri"] == Value::String(iri.clone()))
        .unwrap_or_else(|| panic!("no property {iri} in {}", profile["properties"]))
}

/// Every property-shape entry whose own `sh:path` is `local`.
fn shapes_on<'a>(profile: &'a Value, local: &str) -> Vec<&'a Value> {
    let iri = term(local);
    profile["property_shapes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["path"] == Value::String(iri.clone()))
        .collect()
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|x| x.as_str().unwrap_or_default().to_string())
        .collect()
}

// ── Classes ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn superclass_chain_is_complete_not_just_direct_parents() {
    let (state, token) = fixture();
    let profile = ok_profile(&state, &token).await;

    let level_d = class_of(&profile, "LevelD");
    assert_eq!(
        strings(&level_d["direct_super_classes"]),
        vec![term("LevelC")],
        "only one edge is declared directly"
    );
    assert_eq!(
        strings(&level_d["super_classes"]),
        vec![term("LevelC"), term("LevelB"), term("LevelA")],
        "the whole chain, nearest ancestor first — this is what the endpoint exists for"
    );

    // The middle of the chain still reports what is above it, and the top
    // reports nothing.
    assert_eq!(
        strings(&class_of(&profile, "LevelB")["super_classes"]),
        vec![term("LevelA")]
    );
    assert!(strings(&class_of(&profile, "LevelA")["super_classes"]).is_empty());
}

#[tokio::test]
async fn classes_carry_their_labels_and_descriptions() {
    let (state, token) = fixture();
    let profile = ok_profile(&state, &token).await;

    let level_a = class_of(&profile, "LevelA");
    assert_eq!(level_a["labels"][0]["value"], "Level A");
    assert_eq!(level_a["labels"][0]["lang"], "en");
    assert_eq!(
        level_a["descriptions"][0]["value"],
        "The top of the fixture hierarchy."
    );
}

// ── Properties ────────────────────────────────────────────────────────────

#[tokio::test]
async fn properties_carry_domain_range_and_datatype() {
    let (state, token) = fixture();
    let profile = ok_profile(&state, &token).await;

    let code = property_of(&profile, "code");
    assert_eq!(code["kind"], "datatype");
    assert_eq!(strings(&code["domains"]), vec![term("LevelC")]);
    assert_eq!(
        strings(&code["ranges"]),
        vec!["http://www.w3.org/2001/XMLSchema#token"]
    );
    assert_eq!(code["datatype"], "http://www.w3.org/2001/XMLSchema#token");
    // skos:prefLabel counts as a label, like rdfs:label and dct:title.
    assert_eq!(code["labels"][0]["value"], "code");

    let relates = property_of(&profile, "relatesTo");
    assert_eq!(relates["kind"], "object");
    assert_eq!(strings(&relates["ranges"]), vec![term("LevelD")]);
    assert!(
        relates["datatype"].is_null(),
        "an object property has no datatype: {relates}"
    );
    assert_eq!(relates["labels"][0]["value"], "relates to");

    assert_eq!(property_of(&profile, "note")["kind"], "annotation");
    assert_eq!(
        profile["counts"]["properties"], 4,
        "four properties are declared: {}",
        profile["properties"]
    );
}

// ── SHACL ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn property_shapes_are_flattened_with_every_constraint() {
    let (state, token) = fixture();
    let profile = ok_profile(&state, &token).await;

    let code = shapes_on(&profile, "code");
    assert_eq!(code.len(), 1, "one shape constrains ex:code: {code:?}");
    let code = code[0];
    assert_eq!(code["datatype"], "http://www.w3.org/2001/XMLSchema#token");
    assert_eq!(code["min_count"], 1);
    assert_eq!(code["max_count"], 1);
    assert_eq!(code["pattern"], "^[A-Z]{2}-[0-9]+$");
    assert_eq!(code["name"], "code");
    assert_eq!(code["message"], "A code is two letters, a dash and digits.");
    assert_eq!(code["node_shape"], term("LevelCShape"));
    assert_eq!(strings(&code["target_classes"]), vec![term("LevelC")]);
    assert_eq!(strings(&code["path_chain"]), vec![term("code")]);
    assert!(
        code["shape"].as_str().unwrap().starts_with("_:"),
        "an anonymous shape is reported by its blank-node label: {code}"
    );

    // sh:class and the sh:node link survive on the linking shape itself.
    let link = shapes_on(&profile, "relatesTo");
    assert_eq!(link.len(), 1);
    assert_eq!(link[0]["class"], term("LevelD"));
    assert_eq!(link[0]["node"], term("LevelDShape"));

    // The shape underneath sh:node is its own entry, with the composed path —
    // the caller never walks sh:property or sh:node.
    let nested = shapes_on(&profile, "count");
    assert_eq!(
        nested.len(),
        1,
        "the nested shape appears exactly once: {nested:?}"
    );
    let nested = nested[0];
    assert_eq!(nested["shape"], term("CountShape"));
    assert_eq!(
        strings(&nested["path_chain"]),
        vec![term("relatesTo"), term("count")],
        "the flattened path runs from the root shape down"
    );
    assert_eq!(
        nested["node_shape"],
        term("LevelCShape"),
        "attributed to the root shape, not to the nested one"
    );
    assert_eq!(strings(&nested["target_classes"]), vec![term("LevelC")]);
    assert_eq!(nested["min_count"], 1);
}

#[tokio::test]
async fn shapes_inside_the_version_are_attributed_to_their_graph() {
    let (state, token) = fixture();
    let profile = ok_profile(&state, &token).await;
    let shapes_graph = format!(
        "{}/data-model/m1/version/1.0.0/shapes",
        state.base_url.as_str()
    );

    assert_eq!(profile["shape_discovery"], profile::SHAPE_DISCOVERY);
    let sources = profile["shape_sources"].as_array().unwrap();
    let found = sources
        .iter()
        .find(|s| s["graph_iri"] == Value::String(shapes_graph.clone()))
        .unwrap_or_else(|| panic!("shapes sub-graph missing from {sources:?}"));
    assert_eq!(found["origin"], "version-graph");
    assert!(
        found["bound_to"].is_null(),
        "a graph found by its content was not bound: {found}"
    );
    // The TBox graph holds no SHACL, so it is not reported as a shape source.
    assert!(
        !sources.iter().any(|s| s["graph_iri"]
            == Value::String(format!(
                "{}/data-model/m1/version/1.0.0",
                state.base_url.as_str()
            ))),
        "a graph without shapes must not be listed: {sources:?}"
    );
    assert_eq!(shapes_on(&profile, "code")[0]["graph_iri"], shapes_graph);
}

#[tokio::test]
async fn shapes_bound_in_the_validation_layer_are_included_and_attributed() {
    let (state, token) = fixture();
    let version_graph = format!("{}/data-model/m1/version/1.0.0", state.base_url.as_str());
    state.store.update(EXTERNAL_SHAPES).unwrap();
    open_triplestore::shacl_studio::bindings::add_binding(
        &state.store,
        &version_graph,
        "urn:test:external-shapes",
    )
    .unwrap();

    let profile = ok_profile(&state, &token).await;
    let sources = profile["shape_sources"].as_array().unwrap();
    let bound = sources
        .iter()
        .find(|s| s["graph_iri"] == "urn:test:external-shapes")
        .unwrap_or_else(|| panic!("bound shape graph missing from {sources:?}"));
    assert_eq!(bound["origin"], "validation-binding");
    assert_eq!(
        bound["bound_to"], version_graph,
        "the caller must be able to tell which IRI carried the binding"
    );

    let note = shapes_on(&profile, "note");
    assert_eq!(note.len(), 1, "the bound shape is profiled too: {note:?}");
    assert_eq!(note[0]["graph_iri"], "urn:test:external-shapes");
    assert_eq!(note[0]["max_count"], 1);
    assert_eq!(strings(&note[0]["target_classes"]), vec![term("LevelA")]);
}

// ── Enumerations ──────────────────────────────────────────────────────────

#[tokio::test]
async fn enumerations_come_from_owl_one_of_skos_and_sh_in() {
    let (state, token) = fixture();
    let profile = ok_profile(&state, &token).await;
    let enums = profile["enumerations"].as_array().unwrap();
    let of_kind =
        |kind: &str| -> Vec<&Value> { enums.iter().filter(|e| e["kind"] == kind).collect() };

    let one_of = of_kind("owl_one_of");
    assert_eq!(one_of.len(), 1, "one owl:oneOf class: {enums:?}");
    assert_eq!(one_of[0]["iri"], term("Priority"));
    let values: Vec<String> = one_of[0]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        values,
        vec![term("PriorityLow"), term("PriorityHigh")],
        "declaration order is part of what an enumeration says"
    );
    assert_eq!(one_of[0]["values"][0]["labels"][0]["value"], "Low");

    let scheme = of_kind("skos_scheme");
    assert_eq!(scheme.len(), 1, "one concept scheme: {enums:?}");
    assert_eq!(scheme[0]["iri"], term("Scheme"));
    let concepts: Vec<String> = scheme[0]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(concepts, vec![term("ConceptOne"), term("ConceptTwo")]);
    assert_eq!(scheme[0]["values"][0]["notation"], "1");
    assert_eq!(scheme[0]["values"][1]["labels"][0]["value"], "Two");

    let sh_in = of_kind("sh_in");
    assert_eq!(sh_in.len(), 1, "one sh:in list: {enums:?}");
    assert_eq!(sh_in[0]["path"], term("state"));
    let states: Vec<String> = sh_in[0]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(states, vec![term("StateOne"), term("StateTwo")]);

    // The same list is on the shape it constrains, so a reader working from the
    // shapes never needs to join back to `enumerations`.
    assert_eq!(
        strings(&Value::Array(
            shapes_on(&profile, "state")[0]["in_list"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v["value"].clone())
                .collect()
        )),
        states
    );
}

// ── Contract stability ────────────────────────────────────────────────────

#[tokio::test]
async fn the_response_is_stable_and_self_describing() {
    let (state, token) = fixture();
    let first = ok_profile(&state, &token).await;
    let second = ok_profile(&state, &token).await;

    assert_eq!(first["profile"], profile::PROFILE_CONTRACT);
    assert_eq!(first["model"]["id"], "m1");
    assert_eq!(first["model"]["namespace"], NS);
    assert_eq!(first["version"]["version"], "1.0.0");
    assert_eq!(first["version"]["status"], "published");
    assert_eq!(
        first["counts"]["classes"],
        first["classes"].as_array().unwrap().len()
    );
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "unchanged data must profile byte-identically"
    );
}

// ── Not found ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_model_and_unknown_version_are_clean_404s() {
    let (state, token) = fixture();

    for path in [
        "/api/models/nope/versions/1.0.0/profile",
        "/api/models/m1/versions/9.9.9/profile",
        // A version string that could otherwise reach the registry's SPARQL.
        "/api/models/m1/versions/1.0.0%3E%20%7D/profile",
    ] {
        let (status, body) = get_profile(&state, &token, path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path} should 404: {body}");
    }
}

// ── Visibility ────────────────────────────────────────────────────────────

/// The registry's own rule, applied by the handler: a private model answers as
/// if it did not exist. Asserted directly on the gate because the route is
/// additionally admin-gated by the router it is merged into, which would deny a
/// non-admin before the handler ever runs — the gate must hold on its own.
#[tokio::test]
async fn a_private_model_is_not_readable_by_someone_who_may_not_read_it() {
    let (state, _token) = fixture();
    state
        .auth_db
        .create_user("mallory", "mallory", "m@test.com", "hash", SystemRole::User)
        .unwrap();

    let denied = profile::readable_model(&state, Some("mallory"), "m1");
    let (status, message) = denied.expect_err("a private model must not be readable");
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        !message.contains("private") && !message.contains("forbidden"),
        "the message must not reveal that the model exists: {message}"
    );
    assert!(
        profile::readable_model(&state, None, "m1").is_err(),
        "an anonymous caller may not read a private model either"
    );
    // The owner still can — the gate denies, it does not just refuse everyone.
    assert!(profile::readable_model(&state, Some("adm"), "m1").is_ok());
}

#[tokio::test]
async fn a_non_admin_never_reaches_the_profile() {
    let (state, _token) = fixture();
    state
        .auth_db
        .create_user("mallory", "mallory", "m@test.com", "hash", SystemRole::User)
        .unwrap();
    let mallory = mint_token("mallory", "mallory", "user");

    let (status, body) =
        get_profile(&state, &mallory, "/api/models/m1/versions/1.0.0/profile").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the router this endpoint is merged into is admin-gated: {body}"
    );
    let text = body.to_string();
    assert!(
        !text.contains(NS),
        "no part of the ontology may leak in the denial: {text}"
    );
}

#[tokio::test]
async fn a_public_model_is_readable_by_anyone_the_router_lets_through() {
    let (state, _token) = fixture();
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "m_pub",
        "Public fixture",
        NS,
        None,
        true,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    assert!(profile::readable_model(&state, None, "m_pub").is_ok());
}

// ── What the review found ─────────────────────────────────────────────────

/// A shape graph the Studio knows carries an owner and a visibility, and being
/// bound to a readable model version is not consent to read it.
#[tokio::test]
async fn a_private_bound_shape_graph_is_not_handed_out_with_the_profile() {
    let (state, token) = fixture();
    let version_graph = format!("{}/data-model/m1/version/1.0.0", state.base_url.as_str());
    state.store.update(EXTERNAL_SHAPES).unwrap();
    open_triplestore::shacl_studio::bindings::add_binding(
        &state.store,
        &version_graph,
        "urn:test:external-shapes",
    )
    .unwrap();

    // Registered to somebody else, privately.
    let studio = open_triplestore::shacl_studio::store::ShaclStudioStore::new(state.auth_db.pool());
    studio
        .create_shape_graph(
            "Somebody else's shapes",
            None,
            open_triplestore::auth::models::OwnerType::User,
            "other-user",
            open_triplestore::auth::models::Visibility::Private,
            "urn:test:external-shapes",
            &[],
            open_triplestore::shacl_studio::models::ShapeSource::Manual,
            Some("other-user"),
        )
        .unwrap();

    let profile = ok_profile(&state, &token).await;
    let sources = profile["shape_sources"].as_array().unwrap();
    assert!(
        !sources
            .iter()
            .any(|s| s["graph_iri"] == "urn:test:external-shapes"),
        "a private shape set must not be listed: {sources:?}"
    );
    assert!(
        shapes_on(&profile, "note").is_empty(),
        "nor may its constraints be profiled"
    );
    // The version's own shapes are untouched by the filter.
    assert!(!shapes_on(&profile, "code").is_empty() || !sources.is_empty());
}

/// Flattening emits one entry per path to a shape, so a shape shared by two
/// roots used to contribute its value set twice and inflate the count.
#[tokio::test]
async fn a_shape_reached_from_two_roots_yields_one_enumeration() {
    let (state, token) = fixture();
    let shapes_graph = format!(
        "{}/data-model/m1/version/1.0.0/shapes",
        state.base_url.as_str()
    );
    state
        .store
        .update(
            &r#"
PREFIX ex: <http://example.org/profile-fixture#>
PREFIX sh: <http://www.w3.org/ns/shacl#>
INSERT DATA {
  GRAPH <{SHAPES_GRAPH}> {
    ex:SharedShape a sh:NodeShape ;
      sh:property [ sh:path ex:sharedCode ; sh:in ( "a" "b" ) ] .
    ex:RootOneShape a sh:NodeShape ; sh:targetClass ex:LevelA ;
      sh:property [ sh:path ex:one ; sh:node ex:SharedShape ] .
    ex:RootTwoShape a sh:NodeShape ; sh:targetClass ex:LevelB ;
      sh:property [ sh:path ex:two ; sh:node ex:SharedShape ] .
  }
}
"#
            .replace("{SHAPES_GRAPH}", &shapes_graph),
        )
        .unwrap();

    let profile = ok_profile(&state, &token).await;
    let shared: Vec<&Value> = profile["enumerations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "sh_in" && e["path"] == term("sharedCode"))
        .collect();
    assert_eq!(
        shared.len(),
        1,
        "one shape, one value set, however many roots reach it: {shared:?}"
    );
    let values: Vec<String> = shared[0]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["value"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(values, vec!["a", "b"]);

    // …and the count the proposer reads agrees with the list it is given.
    assert_eq!(
        profile["counts"]["enumerations"],
        profile["enumerations"].as_array().unwrap().len()
    );
}

/// The module promises byte-identical JSON for unchanged data, and a SELECT
/// returns bound graphs in no particular order.
#[tokio::test]
async fn two_shape_graphs_bound_to_one_target_come_back_in_a_stable_order() {
    let (state, token) = fixture();
    let version_graph = format!("{}/data-model/m1/version/1.0.0", state.base_url.as_str());
    for g in ["urn:test:bound-b", "urn:test:bound-a"] {
        state
            .store
            .update(&format!(
                "PREFIX ex: <http://example.org/profile-fixture#> \
                 PREFIX sh: <http://www.w3.org/ns/shacl#> \
                 INSERT DATA {{ GRAPH <{g}> {{ ex:S{} a sh:NodeShape ; sh:targetClass ex:LevelA }} }}",
                g.len()
            ))
            .unwrap();
        open_triplestore::shacl_studio::bindings::add_binding(&state.store, &version_graph, g)
            .unwrap();
    }

    let first = ok_profile(&state, &token).await;
    let second = ok_profile(&state, &token).await;
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap(),
        "two calls on unchanged data must agree byte for byte"
    );
    let bound: Vec<String> = first["shape_sources"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["origin"] == "validation-binding")
        .map(|s| s["graph_iri"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(bound, vec!["urn:test:bound-a", "urn:test:bound-b"]);
}
