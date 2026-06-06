//! SHACL Advanced Features (SHACL-AF) — inference **rules** conformance.
//!
//! Grounded in the W3C *SHACL Advanced Features* Note (§4 SHACL Rules):
//! `sh:rule` with `sh:SPARQLRule`/`sh:construct` and `sh:TripleRule`
//! (`sh:subject`/`sh:predicate`/`sh:object`), executed by
//! [`open_triplestore::shacl::infer`] and exposed over HTTP at
//! `POST /api/datasets/:id/infer`.
//!
//! Engine model (verified against `src/shacl/engine.rs`):
//!   * Shapes load into `urn:shapes`; data loads into the **default graph** and
//!     `infer(store, "urn:shapes", &[])` is called. With an empty `data_graphs`
//!     list every lookup (target resolution, SPARQL-rule `WHERE`, `INSERT`)
//!     evaluates against the default graph, so the rule pipeline is internally
//!     consistent. (Named-graph target resolution is covered by the HTTP tests.)
//!   * A `sh:SPARQLRule`'s `sh:construct` is executed as a SPARQL **Update** with
//!     the literal `$this` substituted by the focus node IRI — i.e. the
//!     `INSERT { … } WHERE { … }` form drives focus-aware inference.
//!   * `infer` re-resolves targets every iteration and runs to a fixed point, so
//!     rules chain transitively.
//!
//! Two genuine spec gaps are pinned by `limitation_*` sentinel tests (they assert
//! *current* behaviour and are tracked for the perf/correctness follow-up):
//!   1. `sh:construct` does not accept the spec CONSTRUCT-template query form.
//!   2. `sh:TripleRule` does not bind the focus node (`sh:this`).

use open_triplestore::shacl::infer;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

mod common;

/// Turtle prefixes shared by every shapes/data fragment.
const PFX: &str = "@prefix ex: <http://example.org/> .\n\
@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n";

/// PREFIX header for the verification ASK/SELECT queries (query strings are not
/// run through the Turtle prefix map, so they carry their own).
const QPFX: &str = "PREFIX ex: <http://example.org/> \
PREFIX sh: <http://www.w3.org/ns/shacl#> ";

/// Load `shapes` into `urn:shapes`, `data` into the default graph, run inference.
fn store_with(shapes: &str, data: &str) -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    if !data.trim().is_empty() {
        store
            .load_str(&format!("{PFX}{data}"), RdfFormat::Turtle, None)
            .unwrap();
    }
    store
}

fn ask(store: &TripleStore, q: &str) -> bool {
    matches!(
        store.query(&format!("{QPFX}{q}")),
        Ok(QueryResults::Boolean(true))
    )
}

/// Number of solution rows for a SELECT (used to assert set-semantics / no dups).
fn rows(store: &TripleStore, q: &str) -> usize {
    match store.query(&format!("{QPFX}{q}")) {
        Ok(QueryResults::Solutions(s)) => s.filter_map(|r| r.ok()).count(),
        _ => 0,
    }
}

// ───────────────────────── Triple rules (sh:TripleRule) ─────────────────────────

/// A `sh:TripleRule` with concrete subject/predicate/object materialises that
/// fixed triple. The rule fires once per focus node, but RDF set semantics keep
/// the result a single triple.
#[test]
fn triple_rule_materialises_concrete_triple() {
    let shapes = r#"
        ex:RegShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:TripleRule ;
                sh:subject ex:Registry ;
                sh:predicate ex:status ;
                sh:object ex:Active ] ."#;
    let data = r#"
        ex:alice a ex:Person .
        ex:bob   a ex:Person ."#;
    let store = store_with(shapes, data);

    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert!(n >= 1, "rule with focus nodes must report inferred work");
    assert!(
        ask(&store, "ASK { ex:Registry ex:status ex:Active }"),
        "the concrete triple must be materialised",
    );
    assert_eq!(
        rows(&store, "SELECT ?s WHERE { ?s ex:status ex:Active }"),
        1,
        "firing once per focus node must not duplicate the triple",
    );
}

// ──────────────────── SPARQL rules (sh:SPARQLRule / sh:construct) ────────────────────

/// The canonical focus-aware rule: only `ex:Person` instances whose `ex:age`
/// satisfies the `FILTER` get the derived classification.
#[test]
fn sparql_rule_derives_focus_aware_triple() {
    let shapes = r#"
        ex:AdultShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "INSERT { $this <http://example.org/category> <http://example.org/Adult> } WHERE { $this <http://example.org/age> ?a . FILTER(?a >= 18) }" ] ."#;
    let data = r#"
        ex:alice a ex:Person ; ex:age 30 .
        ex:bob   a ex:Person ; ex:age 12 ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(
        ask(&store, "ASK { ex:alice ex:category ex:Adult }"),
        "alice (30) satisfies the FILTER and must be classified Adult",
    );
    assert!(
        !ask(&store, "ASK { ex:bob ex:category ex:Adult }"),
        "bob (12) fails the FILTER and must NOT be classified",
    );
}

/// `sh:targetNode` restricts a rule to a single focus node.
#[test]
fn sparql_rule_target_node_limits_scope() {
    let shapes = r#"
        ex:OnlyAlice a sh:NodeShape ;
            sh:targetNode ex:alice ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "INSERT { $this <http://example.org/flagged> true } WHERE { $this a <http://example.org/Person> }" ] ."#;
    let data = r#"
        ex:alice a ex:Person .
        ex:bob   a ex:Person ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:alice ex:flagged true }"), "targeted");
    assert!(
        !ask(&store, "ASK { ex:bob ex:flagged true }"),
        "non-targeted node must be untouched",
    );
}

/// `sh:targetSubjectsOf` resolves focus nodes as the subjects of a predicate.
#[test]
fn sparql_rule_target_subjects_of() {
    let shapes = r#"
        ex:ContactShape a sh:NodeShape ;
            sh:targetSubjectsOf ex:email ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "INSERT { $this <http://example.org/hasContact> true } WHERE { $this a <http://example.org/Person> }" ] ."#;
    let data = r#"
        ex:alice a ex:Person ; ex:email "a@x.org" .
        ex:bob   a ex:Person ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:alice ex:hasContact true }"));
    assert!(
        !ask(&store, "ASK { ex:bob ex:hasContact true }"),
        "bob has no ex:email so is not a focus node",
    );
}

/// `sh:targetObjectsOf` resolves focus nodes as the objects of a predicate.
#[test]
fn sparql_rule_target_objects_of() {
    let shapes = r#"
        ex:MentionedShape a sh:NodeShape ;
            sh:targetObjectsOf ex:knows ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "INSERT { $this <http://example.org/popular> true } WHERE { $this a <http://example.org/Person> }" ] ."#;
    let data = r#"
        ex:alice a ex:Person ; ex:knows ex:bob .
        ex:bob   a ex:Person ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(
        ask(&store, "ASK { ex:bob ex:popular true }"),
        "bob is the object of ex:knows",
    );
    assert!(
        !ask(&store, "ASK { ex:alice ex:popular true }"),
        "alice is only a subject, never an object of ex:knows",
    );
}

// ─────────────────────── Iteration / fixed point ───────────────────────

/// Rules chain transitively: rule B consumes the triples produced by rule A.
/// `infer` re-resolves targets every round, so a single `infer` call reaches the
/// transitive closure.
#[test]
fn rules_chain_to_fixed_point() {
    let shapes = r#"
        ex:S1 a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "INSERT { $this a <http://example.org/Adult> } WHERE { $this <http://example.org/age> ?a . FILTER(?a >= 18) }" ] .
        ex:S2 a sh:NodeShape ;
            sh:targetClass ex:Adult ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "INSERT { $this <http://example.org/canVote> true } WHERE { $this a <http://example.org/Adult> }" ] ."#;
    let data = r#"
        ex:alice a ex:Person ; ex:age 30 .
        ex:bob   a ex:Person ; ex:age 12 ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:alice a ex:Adult }"), "rule A fired");
    assert!(
        ask(&store, "ASK { ex:alice ex:canVote true }"),
        "rule B consumed rule A's output transitively",
    );
    assert!(
        !ask(&store, "ASK { ex:bob ex:canVote true }"),
        "bob never became an Adult so rule B must not fire for bob",
    );
}

/// Re-running inference is idempotent — RDF set semantics keep derived triples
/// unique even though the engine iterates to a fixed point.
#[test]
fn inference_is_idempotent() {
    let shapes = r#"
        ex:AdultShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "INSERT { $this <http://example.org/category> <http://example.org/Adult> } WHERE { $this <http://example.org/age> ?a . FILTER(?a >= 18) }" ] ."#;
    let data = r#"ex:alice a ex:Person ; ex:age 30 ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    infer(&store, "urn:shapes", &[]).unwrap(); // second pass must not duplicate
    assert_eq!(
        rows(&store, "SELECT ?s WHERE { ?s ex:category ex:Adult }"),
        1,
        "derived triple must exist exactly once after repeated inference",
    );
}

/// A rule whose target class has no instances infers nothing and reports zero.
#[test]
fn no_focus_nodes_infers_nothing() {
    let shapes = r#"
        ex:GhostShape a sh:NodeShape ;
            sh:targetClass ex:Ghost ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "INSERT { $this <http://example.org/x> true } WHERE { $this a <http://example.org/Ghost> }" ] ."#;
    let data = r#"ex:alice a ex:Person ."#;
    let store = store_with(shapes, data);

    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert_eq!(n, 0, "no focus nodes ⇒ zero inferred triples");
    assert!(!ask(&store, "ASK { ?s ex:x true }"));
}

// ───────────────────── Documented limitations (sentinels) ─────────────────────

/// LIMITATION: `sh:construct` is executed as a SPARQL **Update**; the spec
/// CONSTRUCT-template query form is rejected by the update parser and produces no
/// triples. Use the `INSERT { … } WHERE { … }` form. Tracked for the follow-up
/// branch — flip this assertion once the CONSTRUCT-template form is supported.
#[test]
fn limitation_construct_query_form_not_materialised() {
    let shapes = r#"
        ex:CShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "CONSTRUCT { $this <http://example.org/x> true } WHERE { $this a <http://example.org/Person> }" ] ."#;
    let data = r#"ex:alice a ex:Person ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(
        !ask(&store, "ASK { ex:alice ex:x true }"),
        "CURRENT behaviour: CONSTRUCT-template form does not materialise",
    );
}

/// LIMITATION: a `sh:TripleRule` does not bind the focus node — `sh:subject sh:this`
/// is inserted verbatim as the `sh:this` IRI rather than substituted per focus
/// node. Use a SPARQL rule with `$this` for focus-aware triples. Tracked for the
/// follow-up branch.
#[test]
fn limitation_triple_rule_does_not_bind_focus_node() {
    let shapes = r#"
        ex:SelfShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:TripleRule ;
                sh:subject sh:this ;
                sh:predicate ex:self ;
                sh:object ex:marker ] ."#;
    let data = r#"ex:alice a ex:Person ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(
        !ask(&store, "ASK { ex:alice ex:self ex:marker }"),
        "CURRENT behaviour: focus node is NOT substituted in triple rules",
    );
    assert!(
        ask(&store, "ASK { sh:this ex:self ex:marker }"),
        "CURRENT behaviour: sh:this is inserted as a literal IRI",
    );
}

// ─────────────────────────── HTTP endpoint ───────────────────────────

/// `POST /api/datasets/:id/infer` — exercises the real Axum router (auth, write
/// ACL, shapes-graph config, named-graph target resolution, materialisation).
mod http {
    use crate::common::{admin_state, body_json, mint_token, test_app};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use open_triplestore::auth::models::{OwnerType, Visibility};
    use oxigraph::io::RdfFormat;
    use oxigraph::sparql::QueryResults;
    use tower::ServiceExt as _;

    fn post(uri: &str, token: Option<&str>) -> Request<Body> {
        let mut b = Request::builder().method("POST").uri(uri);
        if let Some(t) = token {
            b = b.header("Authorization", format!("Bearer {t}"));
        }
        b.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn infer_endpoint_materialises_triples_for_writer() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_dataset(
                "ds",
                "DS",
                None,
                OwnerType::User,
                "adm",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .update_dataset_shacl("ds", false, Some("urn:shapes"))
            .unwrap();
        state.auth_db.add_dataset_graph("ds", "urn:data").unwrap();

        // Shapes in urn:shapes; instance data in the registered named graph.
        state
            .store
            .load_str(
                r#"@prefix ex: <http://example.org/> . @prefix sh: <http://www.w3.org/ns/shacl#> .
                ex:RegShape a sh:NodeShape ; sh:targetClass ex:Person ;
                    sh:rule [ a sh:TripleRule ; sh:subject ex:Registry ; sh:predicate ex:status ; sh:object ex:Active ] ."#,
                RdfFormat::Turtle,
                Some("urn:shapes"),
            )
            .unwrap();
        state
            .store
            .load_str(
                "@prefix ex: <http://example.org/> . ex:alice a ex:Person . ex:bob a ex:Person .",
                RdfFormat::Turtle,
                Some("urn:data"),
            )
            .unwrap();

        let resp = test_app(state.clone())
            .oneshot(post("/api/datasets/ds/infer", Some(&token)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let j = body_json(resp.into_body()).await;
        assert!(
            j["inferred_triples"].as_u64().unwrap() >= 1,
            "endpoint must report inferred triples: {j}",
        );

        // The triple rule writes to the default graph — verify via the store.
        let materialised = matches!(
            state.store.query(
                "ASK { <http://example.org/Registry> <http://example.org/status> <http://example.org/Active> }"
            ),
            Ok(QueryResults::Boolean(true))
        );
        assert!(materialised, "derived triple must be queryable after infer");
    }

    #[tokio::test]
    async fn infer_endpoint_requires_authentication() {
        let (state, _token) = admin_state();
        let resp = test_app(state)
            .oneshot(post("/api/datasets/ds/infer", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn infer_endpoint_400_when_no_shapes_graph() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_dataset(
                "ds2",
                "DS2",
                None,
                OwnerType::User,
                "adm",
                Visibility::Private,
                None,
            )
            .unwrap();
        // No update_dataset_shacl ⇒ shapes_graph_iri is NULL.
        let resp = test_app(state)
            .oneshot(post("/api/datasets/ds2/infer", Some(&token)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn infer_endpoint_403_for_non_writer() {
        let (state, _admin) = admin_state();
        state
            .auth_db
            .create_user(
                "viewer",
                "viewer",
                "v@test.com",
                "hash",
                open_triplestore::auth::models::SystemRole::User,
            )
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds3",
                "DS3",
                None,
                OwnerType::User,
                "adm",
                Visibility::Private,
                None,
            )
            .unwrap();
        let viewer = mint_token("viewer", "viewer", "user");
        let resp = test_app(state)
            .oneshot(post("/api/datasets/ds3/infer", Some(&viewer)))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
