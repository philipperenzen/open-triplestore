//! SHACL-AF rules may only read and write what the dataset they run for holds.
//!
//! A shapes graph is *caller-supplied data*: any writer of a dataset can PUT one
//! (`PUT /api/datasets/:id/shapes`) and then run it (`POST /api/datasets/:id/infer`),
//! and a SHACL Studio pipeline reaches the same engine. The rule bodies inside it
//! are therefore untrusted text, and the engine used to hand them to
//! `TripleStore::update` — which has no authorization and no graph guard — after a
//! purely textual rewrite. That made every rule body a full SPARQL UPDATE running
//! with the store's own authority:
//!
//! * with exactly one data graph the body was prefixed with `WITH <g>`, which does
//!   NOT confine it — `WITH <g> INSERT { GRAPH <any> { … } } WHERE { GRAPH <any> { … } }`
//!   is valid SPARQL, so a rule could copy any graph in the store (another tenant's
//!   private graph, `urn:system:*`, the model registry) into the caller's own;
//! * with zero, or two or more, data graphs the body ran verbatim, so `DROP ALL`
//!   was a rule;
//! * `$this` was substituted *textually* (`<{focus}>`), and a focus node may be a
//!   literal (`sh:targetNode "…"`), so even a `sh:TripleRule` — which has no query
//!   at all — could inject extra operations through its focus node.
//!
//! Each test below drives the real router as a non-admin who owns nothing but
//! their own dataset, and asserts the victim graph is untouched.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use open_triplestore::server::AppState;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use tower::ServiceExt as _;

mod common;
use common::{admin_state, mint_token, test_app};

/// A graph owned by someone else that the attacker may neither read nor write.
const VICTIM: &str = "urn:victim:secret";
const VICTIM_TRIPLE: &str = "<urn:victim:patient> <urn:victim:diagnosis> \"confidential\"";

/// The attacker: a plain user with a private dataset of their own.
const DS: &str = "ds";
const DATA: &str = "urn:dataset:ds:data";
const DATA2: &str = "urn:dataset:ds:more";

/// `(state, mallory's token)` — a non-admin user owning dataset `ds` with
/// `graphs` registered, and a victim graph they have no grant on.
fn tenant(graphs: &[&str]) -> (AppState, String) {
    let (state, _admin) = admin_state();
    state
        .auth_db
        .create_user("mal", "mallory", "mal@test.com", "hash", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .create_dataset(
            DS,
            "DS",
            None,
            OwnerType::User,
            "mal",
            Visibility::Private,
            None,
        )
        .unwrap();
    for g in graphs {
        state.auth_db.add_dataset_graph(DS, g).unwrap();
        state
            .store
            .load_str(
                "<urn:dataset:ds:thing> a <http://example.org/Person> .",
                RdfFormat::Turtle,
                Some(g),
            )
            .unwrap();
    }
    state
        .store
        .load_str(
            &format!("{VICTIM_TRIPLE} ."),
            RdfFormat::NTriples,
            Some(VICTIM),
        )
        .unwrap();
    (state, mint_token("mal", "mallory", "user"))
}

fn ask(state: &AppState, q: &str) -> bool {
    matches!(state.store.query(q), Ok(QueryResults::Boolean(true)))
}

fn victim_intact(state: &AppState) -> bool {
    ask(
        state,
        &format!("ASK {{ GRAPH <{VICTIM}> {{ {VICTIM_TRIPLE} }} }}"),
    )
}

/// PUT the shapes graph as the dataset's writer, then POST /infer. Returns the
/// infer response's status and body text (JSON on success, the error otherwise).
async fn upload_shapes_and_infer(
    state: &AppState,
    token: &str,
    shapes: &str,
) -> (StatusCode, String) {
    let put = Request::builder()
        .method("PUT")
        .uri(format!("/api/datasets/{DS}/shapes"))
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "text/turtle")
        .body(Body::from(shapes.to_string()))
        .unwrap();
    let resp = test_app(state.clone()).oneshot(put).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "a dataset writer may upload shapes: {}",
        common::body_text(resp.into_body()).await
    );

    let post = Request::builder()
        .method("POST")
        .uri(format!("/api/datasets/{DS}/infer"))
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = test_app(state.clone()).oneshot(post).await.unwrap();
    let status = resp.status();
    (status, common::body_text(resp.into_body()).await)
}

/// Exfiltration. One data graph, so the rule is prefixed with `WITH <g>` — which
/// leaves `GRAPH <victim>` in the WHERE clause free to read the victim graph, and
/// `GRAPH <mine>` in the template free to land it where the attacker can read it.
#[tokio::test]
async fn rule_cannot_copy_a_graph_the_caller_may_not_read() {
    let (state, token) = tenant(&[DATA]);
    let shapes = format!(
        r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix ex: <http://example.org/> .
        ex:Steal a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ; sh:construct
                "INSERT {{ GRAPH <{DATA}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{VICTIM}> {{ ?s ?p ?o }} }}" ] ."#
    );
    let (status, body) = upload_shapes_and_infer(&state, &token, &shapes).await;

    assert!(
        !ask(
            &state,
            &format!("ASK {{ GRAPH <{DATA}> {{ {VICTIM_TRIPLE} }} }}")
        ),
        "a rule copied a graph the caller may not read into their own ({status}, {body})"
    );
    assert!(
        !ask(&state, &format!("ASK {{ {VICTIM_TRIPLE} }}")),
        "the victim's triple must not reach the default graph either ({status}, {body})"
    );
}

/// Destruction. Two data graphs, so the body used to run verbatim — every SPARQL
/// UPDATE operation included.
#[tokio::test]
async fn rule_cannot_drop_graphs() {
    let (state, token) = tenant(&[DATA, DATA2]);
    let shapes = r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix ex: <http://example.org/> .
        ex:Wipe a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ; sh:construct "DROP ALL" ] ."#;
    let (status, body) = upload_shapes_and_infer(&state, &token, shapes).await;

    assert!(
        victim_intact(&state),
        "a rule dropped every graph in the store ({status}, {body})"
    );
    assert_ne!(
        status,
        StatusCode::OK,
        "a rule body that is not a CONSTRUCT query must be refused, not run: {body}"
    );
    assert!(
        body.contains("CONSTRUCT"),
        "the refusal must say what a rule body has to be: {body}"
    );
}

/// Injection through the focus node. A `sh:TripleRule` carries no query, but the
/// focus node was pasted into the generated update as `<{focus}>` — and
/// `sh:targetNode` may be a *literal*, whose lexical form the attacker writes.
#[tokio::test]
async fn focus_node_cannot_inject_update_operations() {
    let (state, token) = tenant(&[DATA]);
    // Closes the generated `INSERT DATA { GRAPH <g> { <focus> … } }`, appends a
    // DROP of the victim graph, and reopens an INSERT DATA so the whole string
    // still parses as SPARQL.
    let payload = format!(
        "urn:zz> <urn:p> <urn:o> }} }} ; DROP GRAPH <{VICTIM}> ; INSERT DATA {{ GRAPH <{DATA}> {{ <urn:done"
    );
    let shapes = format!(
        r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix ex: <http://example.org/> .
        ex:Inject a sh:NodeShape ;
            sh:targetNode "{payload}" ;
            sh:rule [ a sh:TripleRule ;
                sh:subject sh:this ; sh:predicate ex:p ; sh:object ex:o ] ."#
    );
    let (status, body) = upload_shapes_and_infer(&state, &token, &shapes).await;

    assert!(
        victim_intact(&state),
        "a literal focus node injected extra update operations ({status}, {body})"
    );
}

/// The honest path still works: a CONSTRUCT rule materialises its triples, and
/// over several data graphs they land in a graph the dataset holds rather than
/// in the store's global default graph, where nothing can read them.
#[tokio::test]
async fn construct_rule_materialises_into_a_graph_the_dataset_holds() {
    let (state, token) = tenant(&[DATA, DATA2]);
    let shapes = r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix ex: <http://example.org/> .
        ex:Tag a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ; sh:construct
                "CONSTRUCT { $this <http://example.org/tagged> true } WHERE { }" ] ."#;
    let (status, body) = upload_shapes_and_infer(&state, &token, shapes).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let inferred: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        inferred["inferred_triples"].as_u64().unwrap_or(0) >= 1,
        "the rule must materialise its triple: {body}"
    );

    let derived = "<urn:dataset:ds:thing> <http://example.org/tagged> true";
    assert!(
        !ask(&state, &format!("ASK {{ {derived} }}")),
        "derived triples must not land in the store's default graph: {body}"
    );
    let graphs: Vec<String> = state.auth_db.list_dataset_graphs(DS).unwrap();
    let landed: Vec<&String> = graphs
        .iter()
        .filter(|g| ask(&state, &format!("ASK {{ GRAPH <{g}> {{ {derived} }} }}")))
        .collect();
    assert!(
        !landed.is_empty(),
        "derived triples must land in a graph the dataset holds; dataset graphs: {graphs:?}"
    );
}
