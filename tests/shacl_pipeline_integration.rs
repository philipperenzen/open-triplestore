//! HTTP-level integration matrix for the SHACL pipeline:
//! bulk-import shapes auto-detection → Studio Library auto-registration →
//! validation-layer bindings → `POST /api/datasets/:id/validate` union
//! resolution → write-gating → boot backfill.
//!
//! Each lettered test mirrors one row of the regression matrix (A–L). The
//! harness drives the real Axum router via `tower::ServiceExt::oneshot`
//! (see `tests/common/mod.rs`), with datasets created directly through
//! `AuthDb` where the HTTP path is not itself under test.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::{admin_state, mint_token, test_app, test_state, url_encode};
use http_body_util::BodyExt as _;
use open_triplestore::auth::models::{GraphKind, OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use open_triplestore::shacl_studio::bindings::{bindings_for_target, dataset_target_iri};
use open_triplestore::shacl_studio::store::ShaclStudioStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

// ─── Fixtures ─────────────────────────────────────────────────────────────────

/// SHACL-only person shapes. Signals for `kind_detector::detect`: 12 sh:*
/// quads, zero OWL and zero typed instances → file-level verdict `Shapes`.
const PERSON_SHAPES_TTL: &str = r#"
@prefix sh:   <http://www.w3.org/ns/shacl#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex:   <http://example.org/people#> .

ex:PersonShape a sh:NodeShape ;
    rdfs:label "Person shape" ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name ; sh:minCount 1 ] ;
    sh:property [ sh:path ex:age ; sh:datatype xsd:integer ; sh:minInclusive 0 ] ;
    sh:property [ sh:path ex:status ; sh:in ( "active" "retired" ) ] .
"#;

/// Instances with violations: bob (no name → minCount; age -5 → minInclusive),
/// carol (age "not-a-number" → sh:datatype). 5 typed subjects so the merged
/// shapes+instances file is NOT shapes-dominant (12 sh-signals < 5×3).
const PEOPLE_BAD_TTL: &str = r#"
@prefix ex: <http://example.org/people#> .
ex:alice a ex:Person ; ex:name "Alice" ; ex:age 34 ; ex:status "active" .
ex:bob   a ex:Person ; ex:age -5 .
ex:carol a ex:Person ; ex:name "Carol" ; ex:age "not-a-number" .
ex:dave  a ex:Person ; ex:name "Dave" ; ex:age 51 ; ex:status "retired" .
ex:erin  a ex:Person ; ex:name "Erin" ; ex:age 28 .
"#;

/// Fully conforming instances (for the write-gate pass case).
const PEOPLE_OK_TTL: &str = r#"
@prefix ex: <http://example.org/people#> .
ex:alice a ex:Person ; ex:name "Alice" ; ex:age 34 ; ex:status "active" .
ex:dave  a ex:Person ; ex:name "Dave" ; ex:age 51 ; ex:status "retired" .
ex:erin  a ex:Person ; ex:name "Erin" ; ex:age 28 .
"#;

/// Same minimal name-shape as JSON-LD (expanded form — no @context games).
const PERSON_SHAPES_JSONLD: &str = r#"{
  "@id": "http://example.org/people#PersonShapeJson",
  "@type": "http://www.w3.org/ns/shacl#NodeShape",
  "http://www.w3.org/ns/shacl#targetClass": {"@id": "http://example.org/people#Person"},
  "http://www.w3.org/ns/shacl#property": {
    "http://www.w3.org/ns/shacl#path": {"@id": "http://example.org/people#name"},
    "http://www.w3.org/ns/shacl#minCount": 1
  }
}"#;

/// Same minimal name-shape as N-Triples.
const PERSON_SHAPES_NT: &str = "\
<http://example.org/people#PersonShapeNt> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/ns/shacl#NodeShape> .\n\
<http://example.org/people#PersonShapeNt> <http://www.w3.org/ns/shacl#targetClass> <http://example.org/people#Person> .\n\
<http://example.org/people#PersonShapeNt> <http://www.w3.org/ns/shacl#property> _:b1 .\n\
_:b1 <http://www.w3.org/ns/shacl#path> <http://example.org/people#name> .\n\
_:b1 <http://www.w3.org/ns/shacl#minCount> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n";

/// Strict shapes for the override test: every person needs an ex:email.
const STRICT_EMAIL_SHAPES_TTL: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/people#> .
ex:StrictPersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:email ; sh:minCount 1 ] .
"#;

/// Lenient shapes for the override test: only ex:name is required.
const LENIENT_NAME_SHAPES_TTL: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/people#> .
ex:LenientPersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name ; sh:minCount 1 ] .
"#;

fn merged_shapes_and_people() -> String {
    format!("{PERSON_SHAPES_TTL}\n{PEOPLE_BAD_TTL}")
}

// ─── Request helpers ──────────────────────────────────────────────────────────

fn req(method: Method, uri: &str, token: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b.body(Body::empty()).unwrap()
}

fn json_req(method: Method, uri: &str, token: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn send(state: &AppState, request: Request<Body>) -> (StatusCode, String) {
    let resp = test_app(state.clone()).oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

fn jv(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or(Value::Null)
}

/// Multipart body builder (same shape the wizard sends): each part is
/// `(name, content_type, optional filename, bytes)`.
fn multipart_body(boundary: &str, parts: &[(&str, &str, Option<&str>, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, content_type, filename, bytes) in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        if let Some(fname) = filename {
            out.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n"
                )
                .as_bytes(),
            );
        } else {
            out.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n").as_bytes(),
            );
        }
        out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// POST /api/import/bulk with N files + a `meta` JSON part.
async fn bulk_import(
    state: &AppState,
    token: &str,
    files: &[(&str, &str, &str)], // (filename, content_type, body)
    meta: Value,
) -> (StatusCode, String) {
    let boundary = "XSHACLPIPELINEBND";
    let meta_str = meta.to_string();
    let mut parts: Vec<(&str, &str, Option<&str>, &[u8])> = files
        .iter()
        .map(|(name, ct, body)| ("file", *ct, Some(*name), body.as_bytes()))
        .collect();
    parts.push(("meta", "application/json", None, meta_str.as_bytes()));
    let body = multipart_body(boundary, &parts);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/import/bulk")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap();
    send(state, request).await
}

/// POST /api/datasets/:id/validate (optionally with a JSON body / ?test=true).
async fn validate(
    state: &AppState,
    token: Option<&str>,
    dataset_id: &str,
    body: Option<&Value>,
    test_run: bool,
) -> (StatusCode, String) {
    let uri = if test_run {
        format!("/api/datasets/{dataset_id}/validate?test=true")
    } else {
        format!("/api/datasets/{dataset_id}/validate")
    };
    let request = match (body, token) {
        (Some(b), Some(t)) => json_req(Method::POST, &uri, t, b),
        _ => req(Method::POST, &uri, token),
    };
    send(state, request).await
}

// ─── State helpers ────────────────────────────────────────────────────────────

fn mk_dataset(state: &AppState, id: &str, owner: &str, visibility: Visibility) {
    state
        .auth_db
        .create_dataset(id, id, None, OwnerType::User, owner, visibility, None)
        .unwrap();
}

fn load_graph(state: &AppState, data: &str, graph: &str) {
    state
        .store
        .load_str(data, RdfFormat::Turtle, Some(graph))
        .unwrap();
}

fn studio(state: &AppState) -> ShaclStudioStore {
    ShaclStudioStore::new(state.auth_db.pool())
}

fn graph_role(state: &AppState, dataset_id: &str, graph_iri: &str) -> Option<GraphKind> {
    state
        .auth_db
        .list_dataset_graph_entries(dataset_id)
        .unwrap()
        .into_iter()
        .find(|e| e.graph_iri == graph_iri)
        .and_then(|e| e.graph_role)
}

fn ask(state: &AppState, query: &str) -> bool {
    matches!(state.store.query(query), Ok(QueryResults::Boolean(true)))
}

fn count(state: &AppState, graph: &str) -> usize {
    state.store.count_graph(Some(graph)).unwrap()
}

/// A non-admin user + bearer token.
fn mk_user(state: &AppState, id: &str) -> String {
    state
        .auth_db
        .create_user(id, id, &format!("{id}@test.com"), "hash", SystemRole::User)
        .unwrap();
    mint_token(id, id, "user")
}

// ─── A. Shapes-only import: full discovery pipeline ───────────────────────────

#[tokio::test]
async fn a_shapes_only_import_registers_and_validates() {
    let (state, token) = admin_state();
    mk_dataset(&state, "dsa", "adm", Visibility::Private);
    let shapes_graph = "urn:test:a:shapes";

    let (status, body) = bulk_import(
        &state,
        &token,
        &[("shapes.ttl", "text/turtle", PERSON_SHAPES_TTL)],
        json!({ "dataset_id": "dsa", "default_target_graph": shapes_graph }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "import failed: {body}");
    assert_eq!(jv(&body)["success"], json!(true), "{body}");

    // Role auto-detected as shapes.
    assert_eq!(
        graph_role(&state, "dsa", shapes_graph),
        Some(GraphKind::Shapes),
        "imported shapes graph must carry graph_role='shapes'"
    );

    // Studio Library record exists with source 'imported'.
    let (status, body) = send(
        &state,
        req(Method::GET, "/api/shacl/shape-graphs", Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let sets = jv(&body);
    let set = sets
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["graph_iri"] == json!(shapes_graph))
        .unwrap_or_else(|| panic!("auto-registered shape graph missing from Library: {body}"));
    assert_eq!(set["source"], json!("imported"), "{set}");

    // Effective shapes resolve through the binding.
    let (status, body) = send(
        &state,
        req(
            Method::GET,
            "/api/datasets/dsa/effective-shapes",
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let eff = jv(&body);
    assert!(
        eff.as_array().is_some_and(|a| !a.is_empty()),
        "effective-shapes must be non-empty: {body}"
    );

    // The legacy Validation-page selector lists the dataset×graph pair with the
    // new `source` field (auto-registration creates a binding, which outranks
    // the graph_role source in the resolution order).
    let (status, body) = send(
        &state,
        req(Method::GET, "/api/shacl/dataset-shape-graphs", Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed = jv(&body);
    let entry = listed["shape_graphs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["dataset_id"] == json!("dsa") && e["shapes_graph_iri"] == json!(shapes_graph))
        .unwrap_or_else(|| panic!("dataset-shape-graphs entry missing: {body}"));
    assert_eq!(entry["source"], json!("binding"), "{entry}");

    // Official validation run: 200 envelope, conforms (no instance data).
    let (status, body) = validate(&state, Some(&token), "dsa", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let j = jv(&body);
    assert_eq!(j["report"]["conforms"], json!(true), "{j}");
    assert!(j["run_id"].is_string(), "run_id non-null: {j}");
    assert!(j["ran_at"].is_string(), "ran_at non-null: {j}");

    // Latest stored run is non-null.
    let (status, body) = send(
        &state,
        req(
            Method::GET,
            "/api/datasets/dsa/validation/latest",
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !jv(&body).is_null(),
        "validation/latest must be non-null: {body}"
    );
}

// ─── B. Merged file, NO auto-split: 400 → role set → validates ───────────────

#[tokio::test]
async fn b_merged_file_without_autosplit_400_then_role_set_validates() {
    let (state, token) = admin_state();
    mk_dataset(&state, "dsb1", "adm", Visibility::Private);
    let merged_graph = "urn:test:b:merged";

    let merged = merged_shapes_and_people();
    let (status, body) = bulk_import(
        &state,
        &token,
        &[("merged.ttl", "text/turtle", &merged)],
        json!({ "dataset_id": "dsb1", "default_target_graph": merged_graph }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Mixed shapes+instances content: detection is inconclusive, no role set.
    assert_eq!(
        graph_role(&state, "dsb1", merged_graph),
        None,
        "mixed-content graph must not be auto-marked as shapes"
    );

    // No shapes source anywhere → 400 with an actionable message.
    let (status, body) = validate(&state, Some(&token), "dsb1", None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("No shapes graph"),
        "400 message must be actionable: {body}"
    );

    // Set the graph's role to shapes through the same endpoint the UI uses.
    let (status, body) = send(
        &state,
        json_req(
            Method::PATCH,
            "/api/datasets/dsb1/graphs",
            &token,
            &json!({ "graph_iri": merged_graph, "graph_role": "shapes" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // Role change auto-registered the graph into the Studio Library.
    assert!(
        studio(&state)
            .get_shape_graph_by_iri(merged_graph)
            .unwrap()
            .is_some(),
        "role=shapes must adopt the graph into the Library"
    );

    // Validation now resolves the shapes and runs.
    let (status, body) = validate(&state, Some(&token), "dsb1", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let j = jv(&body);
    assert!(j["run_id"].is_string(), "{j}");
    assert_eq!(
        j["report"]["conforms"],
        json!(false),
        "instances merged into the shapes graph must still be validated: {j}"
    );
    assert!(
        j["report"]["results_count"].as_u64().unwrap_or(0) >= 3,
        "bob (minCount name + minInclusive age) and carol (datatype) violations expected: {j}"
    );
}

// ─── C. Merged file WITH auto_split: subject-tree split + immediate validate ──

#[tokio::test]
async fn c_merged_file_with_autosplit_splits_registers_and_validates() {
    let (state, token) = admin_state();
    mk_dataset(&state, "dsc", "adm", Visibility::Private);
    let target = "urn:test:c:data";
    let shapes_sub = format!("{target}/shapes");
    let instances_sub = format!("{target}/instances");

    let merged = merged_shapes_and_people();
    let (status, body) = bulk_import(
        &state,
        &token,
        &[("merged.ttl", "text/turtle", &merged)],
        json!({
            "dataset_id": "dsc",
            "default_target_graph": target,
            "auto_split_files": ["merged.ttl"]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Split sub-graphs registered with their detected roles.
    assert_eq!(
        graph_role(&state, "dsc", &shapes_sub),
        Some(GraphKind::Shapes)
    );
    assert_eq!(
        graph_role(&state, "dsc", &instances_sub),
        Some(GraphKind::Instances)
    );

    // Shapes sub-graph auto-registered in the Library.
    assert!(
        studio(&state)
            .get_shape_graph_by_iri(&shapes_sub)
            .unwrap()
            .is_some(),
        "auto-split shapes sub-graph must be adopted into the Library"
    );

    // Subject-tree splitting: the sh:in RDF list spine stays with its shape…
    assert!(
        ask(
            &state,
            &format!(
                "ASK {{ GRAPH <{shapes_sub}> {{ ?cell <http://www.w3.org/1999/02/22-rdf-syntax-ns#first> \"active\" }} }}"
            )
        ),
        "rdf:first list cell of sh:in must live in the /shapes sub-graph"
    );
    assert!(
        !ask(
            &state,
            &format!(
                "ASK {{ GRAPH <{instances_sub}> {{ ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#first> ?o }} }}"
            )
        ),
        "no severed rdf list cells may leak into /instances"
    );
    // …and so does the shape's rdfs:label annotation.
    assert!(
        ask(
            &state,
            &format!(
                "ASK {{ GRAPH <{shapes_sub}> {{ <http://example.org/people#PersonShape> <http://www.w3.org/2000/01/rdf-schema#label> ?l }} }}"
            )
        ),
        "rdfs:label on the shape must live in the /shapes sub-graph"
    );

    // The people landed in /instances.
    assert!(
        ask(
            &state,
            &format!(
                "ASK {{ GRAPH <{instances_sub}> {{ <http://example.org/people#alice> ?p ?o }} }}"
            )
        ),
        "instance data must live in the /instances sub-graph"
    );

    // Validation works immediately and finds the seeded violations.
    let (status, body) = validate(&state, Some(&token), "dsc", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let j = jv(&body);
    assert_eq!(j["report"]["conforms"], json!(false), "{j}");
    assert!(
        j["report"]["results_count"].as_u64().unwrap_or(0) >= 3,
        "expected ≥3 violations (bob name, bob age, carol datatype): {j}"
    );
    assert!(j["run_id"].is_string(), "{j}");
}

// ─── D. Multi-file import, shapes not first ───────────────────────────────────

#[tokio::test]
async fn d_multifile_import_shapes_not_first_still_registers() {
    let (state, token) = admin_state();
    mk_dataset(&state, "dsd", "adm", Visibility::Private);
    let g_inst = "urn:test:d:instances";
    let g_shapes = "urn:test:d:shapes";

    let (status, body) = bulk_import(
        &state,
        &token,
        &[
            ("instances.ttl", "text/turtle", PEOPLE_BAD_TTL),
            ("shapes.ttl", "text/turtle", PERSON_SHAPES_TTL),
        ],
        json!({
            "dataset_id": "dsd",
            "targets": { "instances.ttl": g_inst, "shapes.ttl": g_shapes }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    assert_eq!(graph_role(&state, "dsd", g_shapes), Some(GraphKind::Shapes));
    assert_eq!(
        graph_role(&state, "dsd", g_inst),
        Some(GraphKind::Instances)
    );
    assert!(
        studio(&state)
            .get_shape_graph_by_iri(g_shapes)
            .unwrap()
            .is_some(),
        "shapes file ordered after the instance file must still auto-register"
    );

    let (status, body) = validate(&state, Some(&token), "dsd", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let j = jv(&body);
    assert_eq!(j["report"]["conforms"], json!(false), "{j}");
    assert!(j["run_id"].is_string(), "{j}");
}

// ─── E. Format coverage: JSON-LD, N-Triples, TriG ─────────────────────────────

async fn assert_format_registers_and_validates(
    filename: &str,
    content_type: &str,
    body_str: &str,
    dataset_id: &str,
    shapes_graph: &str,
) {
    let (state, token) = admin_state();
    mk_dataset(&state, dataset_id, "adm", Visibility::Private);

    let (status, body) = bulk_import(
        &state,
        &token,
        &[(filename, content_type, body_str)],
        json!({ "dataset_id": dataset_id, "default_target_graph": shapes_graph }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{filename}: {body}");
    assert_eq!(jv(&body)["success"], json!(true), "{filename}: {body}");

    assert_eq!(
        graph_role(&state, dataset_id, shapes_graph),
        Some(GraphKind::Shapes),
        "{filename}: role auto-detected from stored quads regardless of upload format"
    );
    assert!(
        studio(&state)
            .get_shape_graph_by_iri(shapes_graph)
            .unwrap()
            .is_some(),
        "{filename}: Studio record must exist"
    );

    let (status, body) = validate(&state, Some(&token), dataset_id, None, false).await;
    assert_eq!(status, StatusCode::OK, "{filename}: {body}");
    assert!(jv(&body)["run_id"].is_string(), "{filename}: {body}");
}

#[tokio::test]
async fn e_jsonld_shapes_auto_register_and_validate() {
    assert_format_registers_and_validates(
        "shapes.jsonld",
        "application/ld+json",
        PERSON_SHAPES_JSONLD,
        "dsej",
        "urn:test:e:jsonld:shapes",
    )
    .await;
}

#[tokio::test]
async fn e_ntriples_shapes_auto_register_and_validate() {
    assert_format_registers_and_validates(
        "shapes.nt",
        "application/n-triples",
        PERSON_SHAPES_NT,
        "dsen",
        "urn:test:e:nt:shapes",
    )
    .await;
}

#[tokio::test]
async fn e_trig_embedded_named_graph_registers_to_dataset() {
    let (state, token) = admin_state();
    mk_dataset(&state, "dse", "adm", Visibility::Private);
    // The embedded graph sits under the dataset's canonical IRI namespace.
    let embedded = format!("{}/dataset/dse/shapes-trig", state.base_url.as_str());
    let trig = format!(
        r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/people#> .
<{embedded}> {{
    ex:PersonShapeTrig a sh:NodeShape ;
        sh:targetClass ex:Person ;
        sh:property [ sh:path ex:name ; sh:minCount 1 ] .
}}
"#
    );

    // Quad format with merge off: the file's own graph name is the write target.
    let (status, body) = bulk_import(
        &state,
        &token,
        &[("shapes.trig", "application/trig", &trig)],
        json!({ "dataset_id": "dse" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Contract (verified here): the embedded graph is registered to the
    // dataset and role detection DOES fire for quad-format uploads, because
    // detection runs on the stored quads per registered graph — the upload
    // format is irrelevant. No explicit meta.graph_roles entry is needed.
    assert_eq!(
        graph_role(&state, "dse", &embedded),
        Some(GraphKind::Shapes),
        "embedded TriG graph must be registered with the shapes role"
    );
    assert!(
        studio(&state)
            .get_shape_graph_by_iri(&embedded)
            .unwrap()
            .is_some(),
        "embedded TriG shapes graph must be adopted into the Library"
    );

    let (status, body) = validate(&state, Some(&token), "dse", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(jv(&body)["run_id"].is_string(), "{body}");
}

// ─── F. Legacy sync: PUT /api/datasets/:id/shacl ──────────────────────────────

#[tokio::test]
async fn f_put_shacl_syncs_studio_registration_and_get_shapes() {
    let (state, token) = admin_state();
    mk_dataset(&state, "dsf", "adm", Visibility::Private);
    let shapes_graph = "urn:test:f:shapes";
    let data_graph = "urn:test:f:data";
    load_graph(&state, PERSON_SHAPES_TTL, shapes_graph);
    load_graph(&state, PEOPLE_BAD_TTL, data_graph);
    state.auth_db.add_dataset_graph("dsf", data_graph).unwrap();

    // No Studio record yet — the graph was loaded behind the API's back.
    assert!(studio(&state)
        .get_shape_graph_by_iri(shapes_graph)
        .unwrap()
        .is_none());

    let (status, body) = send(
        &state,
        json_req(
            Method::PUT,
            "/api/datasets/dsf/shacl",
            &token,
            &json!({ "shacl_on_write": false, "shapes_graph_iri": shapes_graph }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // Studio record + dataset binding now exist.
    assert!(
        studio(&state)
            .get_shape_graph_by_iri(shapes_graph)
            .unwrap()
            .is_some(),
        "PUT shacl must adopt the configured shapes graph into the Library"
    );
    let ds_target = dataset_target_iri(&state.base_url, "dsf");
    assert!(
        bindings_for_target(&state.store, &ds_target).contains(&shapes_graph.to_string()),
        "PUT shacl must bind the shapes graph to the dataset in the validation layer"
    );

    // GET shapes returns merged Turtle containing the shape.
    let (status, body) = send(
        &state,
        req(Method::GET, "/api/datasets/dsf/shapes", Some(&token)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        body.contains("PersonShape"),
        "GET shapes must return the configured shapes Turtle: {body}"
    );

    // And validate resolves them (violating people → conforms=false).
    let (status, body) = validate(&state, Some(&token), "dsf", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let j = jv(&body);
    assert_eq!(j["report"]["conforms"], json!(false), "{j}");
}

// ─── G. Explicit body override ────────────────────────────────────────────────

#[tokio::test]
async fn g_explicit_shapes_graph_override() {
    let (state, token) = admin_state();
    // Public so the non-admin caller can reach the dataset (their 403 must come
    // from the shapes-graph read check, not from dataset access).
    mk_dataset(&state, "dsg", "adm", Visibility::Public);
    let strict = "urn:test:g:strict";
    let lenient = "urn:test:g:lenient";
    let data_graph = "urn:test:g:data";
    load_graph(&state, STRICT_EMAIL_SHAPES_TTL, strict);
    load_graph(&state, LENIENT_NAME_SHAPES_TTL, lenient);
    load_graph(
        &state,
        r#"@prefix ex: <http://example.org/people#> .
           ex:gina a ex:Person ; ex:name "Gina" ."#,
        data_graph,
    );
    state.auth_db.add_dataset_graph("dsg", data_graph).unwrap();
    state
        .auth_db
        .update_dataset_shacl("dsg", false, Some(strict))
        .unwrap();

    // Default resolution → the configured strict shapes → email violation.
    let (status, body) = validate(&state, Some(&token), "dsg", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(jv(&body)["report"]["conforms"], json!(false), "{body}");

    // Explicit override is EXCLUSIVE: only the lenient graph is used → conforms.
    let (status, body) = validate(
        &state,
        Some(&token),
        "dsg",
        Some(&json!({ "shapes_graph": lenient })),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        jv(&body)["report"]["conforms"],
        json!(true),
        "override must validate against the named graph only: {body}"
    );

    // A non-admin without a read grant on the override graph gets 403.
    let u2 = mk_user(&state, "u2");
    let (status, body) = validate(
        &state,
        Some(&u2),
        "dsg",
        Some(&json!({ "shapes_graph": lenient })),
        false,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "override graph without read access must 403: {body}"
    );

    // With a graph ACL read grant the same call succeeds.
    state
        .auth_db
        .grant_graph_permission("acl-g-1", lenient, "user", "u2", "read", "adm")
        .unwrap();
    let (status, body) = validate(
        &state,
        Some(&u2),
        "dsg",
        Some(&json!({ "shapes_graph": lenient })),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "granted read must pass: {body}");
    assert_eq!(jv(&body)["report"]["conforms"], json!(true), "{body}");

    // Garbage IRI → 400.
    let (status, body) = validate(
        &state,
        Some(&token),
        "dsg",
        Some(&json!({ "shapes_graph": "not an iri" })),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("Invalid shapes_graph IRI"),
        "actionable message expected: {body}"
    );
}

// ─── H. Role × visibility ─────────────────────────────────────────────────────

#[tokio::test]
async fn h_validate_respects_role_and_visibility() {
    let (state, _admin) = admin_state();
    let u1 = mk_user(&state, "u1");
    let u2 = mk_user(&state, "u2");

    // Private dataset owned by u1, with a shapes-role graph.
    mk_dataset(&state, "dsp", "u1", Visibility::Private);
    let priv_shapes = "urn:test:h:shapes";
    load_graph(&state, PERSON_SHAPES_TTL, priv_shapes);
    state.auth_db.add_dataset_graph("dsp", priv_shapes).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("dsp", priv_shapes, Some(GraphKind::Shapes))
        .unwrap();

    // Owner: 200.
    let (status, body) = validate(&state, Some(&u1), "dsp", None, false).await;
    assert_eq!(status, StatusCode::OK, "owner must validate: {body}");

    // Unrelated authenticated user: 403.
    let (status, body) = validate(&state, Some(&u2), "dsp", None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "unrelated user: {body}");

    // Anonymous: 401 (the SHACL routes require auth).
    let (status, body) = validate(&state, None, "dsp", None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "anonymous: {body}");

    // Public dataset: an unrelated user may validate.
    mk_dataset(&state, "dspub", "u1", Visibility::Public);
    let pub_shapes = "urn:test:h:pub:shapes";
    load_graph(&state, PERSON_SHAPES_TTL, pub_shapes);
    state
        .auth_db
        .add_dataset_graph("dspub", pub_shapes)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_role("dspub", pub_shapes, Some(GraphKind::Shapes))
        .unwrap();
    let (status, body) = validate(&state, Some(&u2), "dspub", None, false).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "public dataset, unrelated user: {body}"
    );
}

#[tokio::test]
async fn h_studio_listing_hides_private_auto_registered_shapes() {
    let (state, _admin) = admin_state();
    let u1 = mk_user(&state, "u1");
    let u2 = mk_user(&state, "u2");

    mk_dataset(&state, "dsp", "u1", Visibility::Private);
    let priv_shapes = "urn:test:h:lib:shapes";
    load_graph(&state, PERSON_SHAPES_TTL, priv_shapes);
    state.auth_db.add_dataset_graph("dsp", priv_shapes).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("dsp", priv_shapes, Some(GraphKind::Shapes))
        .unwrap();

    // Owner validates once: the self-healing path adopts the graph into the
    // Library (record inherits the dataset's owner + PRIVATE visibility).
    let (status, body) = validate(&state, Some(&u1), "dsp", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(studio(&state)
        .get_shape_graph_by_iri(priv_shapes)
        .unwrap()
        .is_some());

    let in_listing = |body: &str| {
        jv(body)
            .as_array()
            .is_some_and(|a| a.iter().any(|s| s["graph_iri"] == json!(priv_shapes)))
    };

    // Owner sees it.
    let (status, body) = send(
        &state,
        req(Method::GET, "/api/shacl/shape-graphs", Some(&u1)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        in_listing(&body),
        "owner must see their private shape graph: {body}"
    );

    // SECURE behavior: an unrelated user must NOT see a shape graph that was
    // auto-registered from a private dataset (access.rs::can_access_set —
    // private ⇒ owner/org-member only).
    let (status, body) = send(
        &state,
        req(Method::GET, "/api/shacl/shape-graphs", Some(&u2)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !in_listing(&body),
        "private auto-registered shape graph leaked to an unrelated user: {body}"
    );
}

// ─── I. Write gates on bulk import ────────────────────────────────────────────

#[tokio::test]
async fn i_legacy_shacl_on_write_gates_bulk_import() {
    let (state, token) = admin_state();
    mk_dataset(&state, "dsw", "adm", Visibility::Private);
    let shapes_graph = "urn:test:i1:shapes";
    let data_graph = "urn:test:i1:data";
    load_graph(&state, PERSON_SHAPES_TTL, shapes_graph);
    // The gate discovers the owning dataset via the registered graph, so the
    // target graph must be registered before the import.
    state.auth_db.add_dataset_graph("dsw", data_graph).unwrap();
    state
        .auth_db
        .update_dataset_shacl("dsw", true, Some(shapes_graph))
        .unwrap();

    // Violating instances: rejected with HTTP 400, nothing committed.
    assert_eq!(count(&state, data_graph), 0);
    let (status, body) = bulk_import(
        &state,
        &token,
        &[("people.ttl", "text/turtle", PEOPLE_BAD_TTL)],
        json!({ "dataset_id": "dsw", "default_target_graph": data_graph }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("SHACL write gate rejected import into graph"),
        "gate failure must be actionable: {body}"
    );
    assert_eq!(
        count(&state, data_graph),
        0,
        "a gated rejection must commit nothing"
    );

    // Conforming instances: pass the same gate.
    let (status, body) = bulk_import(
        &state,
        &token,
        &[("people.ttl", "text/turtle", PEOPLE_OK_TTL)],
        json!({ "dataset_id": "dsw", "default_target_graph": data_graph }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        count(&state, data_graph) > 0,
        "conforming import must commit"
    );
}

#[tokio::test]
async fn i_studio_binding_alone_gates_bulk_import() {
    let (state, token) = admin_state();
    mk_dataset(&state, "dsb2", "adm", Visibility::Private);
    let shapes_graph = "urn:test:i2:shapes";
    let data_graph = "urn:test:i2:data";
    // Pre-register the data graph so gate discovery can find the owning dataset.
    state.auth_db.add_dataset_graph("dsb2", data_graph).unwrap();

    // Import the shapes over HTTP: this is the e2e path that auto-registers the
    // Library record AND creates the dataset-level validation-layer binding.
    let (status, body) = bulk_import(
        &state,
        &token,
        &[("shapes.ttl", "text/turtle", PERSON_SHAPES_TTL)],
        json!({ "dataset_id": "dsb2", "default_target_graph": shapes_graph }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let ds_target = dataset_target_iri(&state.base_url, "dsb2");
    assert!(
        bindings_for_target(&state.store, &ds_target).contains(&shapes_graph.to_string()),
        "import must create the dataset-level binding"
    );

    // Note shacl_on_write stays FALSE: per gate.rs::discover_gates, a plain
    // validation-layer binding (dataset- or graph-level) gates the import path
    // on its own at the Violation threshold — no gate_writes pipeline and no
    // legacy flag needed.
    let ds = state.auth_db.get_dataset("dsb2").unwrap().unwrap();
    assert!(!ds.shacl_on_write);

    let (status, body) = bulk_import(
        &state,
        &token,
        &[("people.ttl", "text/turtle", PEOPLE_BAD_TTL)],
        json!({ "dataset_id": "dsb2", "default_target_graph": data_graph }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("SHACL write gate rejected import into graph"),
        "{body}"
    );
    assert_eq!(
        count(&state, data_graph),
        0,
        "rejected batch commits nothing"
    );

    let (status, body) = bulk_import(
        &state,
        &token,
        &[("people.ttl", "text/turtle", PEOPLE_OK_TTL)],
        json!({ "dataset_id": "dsb2", "default_target_graph": data_graph }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(count(&state, data_graph) > 0);
}

// ─── J. Boot backfill ─────────────────────────────────────────────────────────

#[tokio::test]
async fn j_boot_backfill_sweeps_existing_shapes_idempotently() {
    let state = test_state();
    // Built entirely behind the API: configured shapes_graph_iri + a second
    // graph with the shapes role, neither registered in the Studio.
    mk_dataset(&state, "dsj", "adm", Visibility::Private);
    let configured = "urn:test:j:shapes1";
    let role_graph = "urn:test:j:shapes2";
    load_graph(&state, PERSON_SHAPES_TTL, configured);
    load_graph(&state, LENIENT_NAME_SHAPES_TTL, role_graph);
    state
        .auth_db
        .update_dataset_shacl("dsj", false, Some(configured))
        .unwrap();
    state.auth_db.add_dataset_graph("dsj", role_graph).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("dsj", role_graph, Some(GraphKind::Shapes))
        .unwrap();

    let st = studio(&state);
    assert!(st.get_shape_graph_by_iri(configured).unwrap().is_none());
    assert!(st.get_shape_graph_by_iri(role_graph).unwrap().is_none());

    let swept = open_triplestore::shacl_studio::migrate::backfill_dataset_shapes(&state);
    assert!(swept >= 2, "both shapes graphs swept, got {swept}");

    // Records + dataset bindings exist for both sources.
    assert!(st.get_shape_graph_by_iri(configured).unwrap().is_some());
    assert!(st.get_shape_graph_by_iri(role_graph).unwrap().is_some());
    let ds_target = dataset_target_iri(&state.base_url, "dsj");
    let bound = bindings_for_target(&state.store, &ds_target);
    assert!(bound.contains(&configured.to_string()), "{bound:?}");
    assert!(bound.contains(&role_graph.to_string()), "{bound:?}");

    // Second sweep: idempotent — no duplicate Library records or bindings.
    open_triplestore::shacl_studio::migrate::backfill_dataset_shapes(&state);
    let all = st.list_shape_graphs().unwrap();
    assert_eq!(
        all.iter().filter(|s| s.graph_iri == configured).count(),
        1,
        "no duplicate record for the configured graph"
    );
    assert_eq!(
        all.iter().filter(|s| s.graph_iri == role_graph).count(),
        1,
        "no duplicate record for the role graph"
    );
    assert_eq!(
        bindings_for_target(&state.store, &ds_target)
            .iter()
            .filter(|b| b.as_str() == configured)
            .count(),
        1,
        "binding stays single after the second sweep"
    );
}

// ─── K. detect-shapes ─────────────────────────────────────────────────────────

#[tokio::test]
async fn k_detect_shapes_flags_datasets_that_already_have_shapes() {
    let (state, token) = admin_state();

    // dsk has shapes (role graph); dsk2 has none.
    mk_dataset(&state, "dsk", "adm", Visibility::Private);
    let shapes_graph = "urn:test:k:shapes";
    load_graph(&state, PERSON_SHAPES_TTL, shapes_graph);
    state
        .auth_db
        .add_dataset_graph("dsk", shapes_graph)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_role("dsk", shapes_graph, Some(GraphKind::Shapes))
        .unwrap();
    mk_dataset(&state, "dsk2", "adm", Visibility::Private);

    let (status, body) = send(
        &state,
        req(
            Method::GET,
            &format!(
                "/api/shacl/detect-shapes?graph={}",
                url_encode(shapes_graph)
            ),
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let j = jv(&body);
    assert_eq!(j["shapes_detected"], json!(true), "{j}");
    assert!(j["shape_count"].as_u64().unwrap_or(0) >= 1, "{j}");

    let suggested = j["suggested_datasets"].as_array().unwrap();
    let dsk = suggested
        .iter()
        .find(|d| d["id"] == json!("dsk"))
        .unwrap_or_else(|| panic!("dataset with shapes must still be suggested: {j}"));
    assert_eq!(dsk["has_shapes"], json!(true), "{dsk}");
    let dsk2 = suggested
        .iter()
        .find(|d| d["id"] == json!("dsk2"))
        .unwrap_or_else(|| panic!("shape-less dataset must be suggested: {j}"));
    assert_eq!(dsk2["has_shapes"], json!(false), "{dsk2}");
}

// ─── L. Report persistence + test runs ────────────────────────────────────────

#[tokio::test]
async fn l_official_run_persists_rdf_report_and_test_run_does_not_count() {
    let (state, token) = admin_state();
    mk_dataset(&state, "dsl", "adm", Visibility::Private);
    let shapes_graph = "urn:test:l:shapes";
    let data_graph = "urn:test:l:data";
    load_graph(&state, PERSON_SHAPES_TTL, shapes_graph);
    // Conforming data: the persisted-RDF assertion below must hold for this run.
    load_graph(&state, PEOPLE_OK_TTL, data_graph);
    state
        .auth_db
        .add_dataset_graph("dsl", shapes_graph)
        .unwrap();
    state
        .auth_db
        .set_dataset_graph_role("dsl", shapes_graph, Some(GraphKind::Shapes))
        .unwrap();
    state.auth_db.add_dataset_graph("dsl", data_graph).unwrap();

    // Official run.
    let (status, body) = validate(&state, Some(&token), "dsl", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(jv(&body)["run_id"].is_string(), "{body}");

    // The per-dataset report graph holds a standard sh:ValidationReport.
    assert!(
        ask(
            &state,
            "PREFIX sh: <http://www.w3.org/ns/shacl#> \
             ASK { GRAPH <urn:system:reports:dataset:dsl> { ?r a sh:ValidationReport } }"
        ),
        "official run must persist the report as queryable RDF"
    );

    let history_len = |body: &str| jv(body).as_array().map(|a| a.len()).unwrap_or(0);
    let (status, body) = send(
        &state,
        req(
            Method::GET,
            "/api/datasets/dsl/validation/history",
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let before = history_len(&body);
    assert_eq!(before, 1, "exactly one official run recorded: {body}");

    // ?test=true validates but records nothing.
    let (status, body) = validate(&state, Some(&token), "dsl", None, true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let j = jv(&body);
    assert!(
        j["run_id"].is_null(),
        "test run must not mint a run id: {j}"
    );
    assert!(j["ran_at"].is_null(), "{j}");
    assert_eq!(j["test"], json!(true), "{j}");

    let (status, body) = send(
        &state,
        req(
            Method::GET,
            "/api/datasets/dsl/validation/history",
            Some(&token),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        history_len(&body),
        before,
        "a ?test=true run must not appear in history: {body}"
    );

    // A NON-conforming official run must also persist its report as RDF.
    let bad_data = "urn:test:l:bad:data";
    load_graph(&state, PEOPLE_BAD_TTL, bad_data);
    state.auth_db.add_dataset_graph("dsl", bad_data).unwrap();
    let (status, body) = validate(&state, Some(&token), "dsl", None, false).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(jv(&body)["report"]["conforms"], json!(false), "{body}");
    // Regression: engine path strings arrive angle-wrapped ("<iri>"); report
    // persistence must not double-wrap them into invalid Turtle, which used to
    // silently skip persistence for exactly the non-conforming runs dashboards
    // need (report_rdf::term now unwraps before deciding IRI vs literal).
    assert!(
        ask(
            &state,
            "PREFIX sh: <http://www.w3.org/ns/shacl#> \
             ASK { GRAPH <urn:system:reports:dataset:dsl> { ?r a sh:ValidationReport ; \
                   sh:conforms false } }"
        ),
        "non-conforming official run must persist its report as queryable RDF"
    );
}
