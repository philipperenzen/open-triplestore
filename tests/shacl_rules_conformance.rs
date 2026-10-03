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
//!   * A `sh:SPARQLRule`'s `sh:construct` accepts both the spec CONSTRUCT-template
//!     form (`CONSTRUCT { … } WHERE { … }`) and the `INSERT { … } WHERE { … }`
//!     convenience form, with `$this` substituted by the focus node IRI.
//!   * A `sh:TripleRule` binds `sh:this` to each focus node.
//!   * `infer` re-resolves targets every iteration and runs to the true fixed
//!     point — measured by the store's triple-count delta per round — so rules
//!     chain transitively and the reported inferred-triple count is exact.
//!
//! The two spec features below were gaps pinned by `limitation_*` sentinels on the
//! standards branch; this branch implements them and the tests now assert the
//! correct behaviour:
//!   1. `sh:construct` CONSTRUCT-template query form (`construct_query_form_materialises`).
//!   2. `sh:TripleRule` focus-node binding via `sh:this` (`triple_rule_binds_focus_node`).

use open_triplestore::shacl::{infer, infer_into};
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

/// The reported count is the EXACT number of newly-materialised triples, not
/// inflated by the fixed-point iteration cap. Regression for the convergence bug
/// where `apply_rule` returned 1 per (rule × focus) every round, so the count was
/// ~100× the focus-node count and the loop never early-exited.
#[test]
fn inferred_count_is_exact_not_inflated() {
    let shapes = r#"
        ex:AdultShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "INSERT { $this <http://example.org/category> <http://example.org/Adult> } WHERE { $this <http://example.org/age> ?a . FILTER(?a >= 18) }" ] ."#;
    // Three adults + one minor ⇒ exactly three derived `ex:category ex:Adult`.
    let data = r#"
        ex:alice a ex:Person ; ex:age 30 .
        ex:bob   a ex:Person ; ex:age 40 .
        ex:carol a ex:Person ; ex:age 21 .
        ex:dan   a ex:Person ; ex:age 12 ."#;
    let store = store_with(shapes, data);

    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert_eq!(
        n, 3,
        "exactly three classifications inferred — count must not be inflated by the iteration cap",
    );
    assert_eq!(
        rows(&store, "SELECT ?s WHERE { ?s ex:category ex:Adult }"),
        3,
    );
}

// ───────────── Blank-node heads (the repair layer's baseline) ─────────────

const BRIDGES: &str = "ex:b1 a ex:Bridge . ex:b2 a ex:Bridge . ex:b3 a ex:Bridge .";

/// Distinct deck nodes hanging off a bridge.
fn decks(store: &TripleStore) -> usize {
    rows(
        store,
        "SELECT DISTINCT ?w WHERE { ?b ex:hasDeck ?w . ?w a ex:Deck }",
    )
}

/// A rule whose head has a blank node and whose body does not check that the
/// head already holds mints a fresh node on every round: the run stops at its
/// 100-round cap with 100 decks per bridge, two triples each. This is the
/// behaviour the repair layer's labelled nulls replace
/// (`docs/notes/repair-layer-design.md` §2.2); `/infer` keeps it.
#[test]
fn unguarded_blank_node_head_mints_a_witness_per_round() {
    let shapes = r#"
        ex:BridgeShape a sh:NodeShape ;
            sh:targetClass ex:Bridge ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "CONSTRUCT { $this <http://example.org/hasDeck> _:w . _:w a <http://example.org/Deck> } WHERE { $this a <http://example.org/Bridge> }" ] ."#;
    let store = store_with(shapes, BRIDGES);
    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert_eq!(n, 100 * 3 * 2, "two new triples per bridge per round");
    assert_eq!(decks(&store), 300);
}

/// The same rule guarded by `FILTER NOT EXISTS` on its own head fires once
/// per bridge, and a second run derives nothing.
#[test]
fn guarded_blank_node_head_mints_one_witness_and_is_idempotent() {
    let shapes = r#"
        ex:BridgeShape a sh:NodeShape ;
            sh:targetClass ex:Bridge ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "CONSTRUCT { $this <http://example.org/hasDeck> _:w . _:w a <http://example.org/Deck> } WHERE { $this a <http://example.org/Bridge> . FILTER NOT EXISTS { $this <http://example.org/hasDeck> ?w0 . ?w0 a <http://example.org/Deck> } }" ] ."#;
    let store = store_with(shapes, BRIDGES);
    assert_eq!(infer(&store, "urn:shapes", &[]).unwrap(), 6);
    assert_eq!(infer(&store, "urn:shapes", &[]).unwrap(), 0);
    assert_eq!(decks(&store), 3);
}

// ──────────────── SHACL-AF features implemented on this branch ────────────────

/// `sh:construct` accepts the spec **CONSTRUCT-template** query form
/// (`CONSTRUCT { template } WHERE { pattern }`), materialising its output exactly
/// like the `INSERT { … } WHERE { … }` convenience form. (Was the
/// `limitation_construct_query_form_not_materialised` sentinel.)
#[test]
fn construct_query_form_materialises() {
    let shapes = r#"
        ex:CShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "CONSTRUCT { $this <http://example.org/x> true } WHERE { $this a <http://example.org/Person> }" ] ."#;
    let data = r#"ex:alice a ex:Person ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(
        ask(&store, "ASK { ex:alice ex:x true }"),
        "CONSTRUCT-template form must materialise the derived triple",
    );
}

/// A `sh:TripleRule` binds the focus node: `sh:subject sh:this` is substituted by
/// each focus node (SHACL-AF §4.3), so the derived triple is focus-aware rather
/// than the literal `sh:this` IRI. (Was the
/// `limitation_triple_rule_does_not_bind_focus_node` sentinel.)
#[test]
fn triple_rule_binds_focus_node() {
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
        ask(&store, "ASK { ex:alice ex:self ex:marker }"),
        "focus node must be substituted for sh:this in the triple rule",
    );
    assert!(
        !ask(&store, "ASK { sh:this ex:self ex:marker }"),
        "the literal sh:this IRI must NOT be inserted",
    );
}

/// A `sh:TripleRule` with `sh:this` as the **object** also binds the focus node,
/// and a per-focus self-edge is materialised once per focus node.
#[test]
fn triple_rule_binds_focus_node_in_object_position() {
    let shapes = r#"
        ex:RegSelf a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:TripleRule ;
                sh:subject ex:Registry ;
                sh:predicate ex:member ;
                sh:object sh:this ] ."#;
    let data = r#"
        ex:alice a ex:Person .
        ex:bob   a ex:Person ."#;
    let store = store_with(shapes, data);

    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:Registry ex:member ex:alice }"));
    assert!(ask(&store, "ASK { ex:Registry ex:member ex:bob }"));
    assert_eq!(
        rows(&store, "SELECT ?m WHERE { ex:Registry ex:member ?m }"),
        2,
        "one membership edge per focus node",
    );
}

/// A `sh:SPARQLRule` carrying `sh:prefixes` run against **one** data graph. With a
/// single data graph the engine materialises into that graph via `WITH <g>`; the
/// prologue the rule's `sh:prefixes` expands to must stay *before* `WITH`
/// (SPARQL 1.1 Update grammar: `Prologue ( Update1 … )`, `WITH` being part of
/// `Modify`), otherwise the update fails to parse and `infer` errors out. This
/// is the `ex:InspectionPriorityRule` shape of `tests/fixtures/example-bridge/shapes-af.ttl`.
#[test]
fn sparql_rule_with_prefixes_infers_into_single_named_graph() {
    let shapes = r#"
        ex:prefixes sh:declare [ sh:prefix "ex" ; sh:namespace "http://example.org/"^^xsd:anyURI ] .
        ex:AdultShape a sh:NodeShape ;
            sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ;
                sh:prefixes ex:prefixes ;
                sh:construct "CONSTRUCT { $this ex:category ex:Adult } WHERE { $this ex:age ?a . FILTER(?a >= 18) }" ] ."#;
    let data = r#"
        ex:alice a ex:Person ; ex:age 30 .
        ex:bob   a ex:Person ; ex:age 12 ."#;
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    store
        .load_str(&format!("{PFX}{data}"), RdfFormat::Turtle, Some("urn:data"))
        .unwrap();

    let n = infer(&store, "urn:shapes", &["urn:data".to_string()])
        .expect("a prefixed rule must parse when the engine targets one graph");
    assert_eq!(n, 1, "exactly alice is classified");
    assert!(
        ask(
            &store,
            "ASK { GRAPH <urn:data> { ex:alice ex:category ex:Adult } }"
        ),
        "the derived triple lands in the data graph the rule inferred over",
    );
    assert!(
        !ask(&store, "ASK { ex:alice ex:category ex:Adult }"),
        "nothing leaks into the store's default graph",
    );
    assert!(!ask(
        &store,
        "ASK { GRAPH <urn:data> { ex:bob ex:category ex:Adult } }"
    ));
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

        // The rule materialises INTO the dataset's data graph. It used to land
        // in the default graph — outside every registered, ACL'd graph, and
        // invisible to the data graph it was inferring over.
        let in_data_graph = matches!(
            state.store.query(
                "ASK { GRAPH <urn:data> { <http://example.org/Registry> <http://example.org/status> <http://example.org/Active> } }"
            ),
            Ok(QueryResults::Boolean(true))
        );
        assert!(
            in_data_graph,
            "derived triple must land in the data graph it was inferred over"
        );

        let leaked_to_default = matches!(
            state.store.query(
                "ASK { <http://example.org/Registry> <http://example.org/status> <http://example.org/Active> }"
            ),
            Ok(QueryResults::Boolean(true))
        );
        assert!(
            !leaked_to_default,
            "inferred triples must not be written to the default graph"
        );
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

// ─── sh:order, sh:condition, sh:deactivated (SHACL-AF §4.1–4.2) ───────────────

/// Rules run in ascending `sh:order`: the second rule consumes what the first
/// produced within one iteration, so a rule chain converges in one round when
/// ordered and would need the fixed-point loop otherwise. Observable here: a
/// rule with a higher order that deletes-nothing-but-marks sees the earlier
/// rule's triple in the same pass.
#[test]
fn rules_run_in_sh_order() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:TripleRule ; sh:order 2 ;
            sh:subject sh:this ; sh:predicate ex:second ; sh:object true ;
            sh:condition ex:HasFirst ] ;
  sh:rule [ a sh:TripleRule ; sh:order 1 ;
            sh:subject sh:this ; sh:predicate ex:first ; sh:object true ] .
ex:HasFirst a sh:NodeShape ; sh:property [ sh:path ex:first ; sh:minCount 1 ] .
"#;
    let store = store_with(shapes, "ex:t a ex:Thing .");
    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:t ex:first true }"), "order 1 fired");
    assert!(
        ask(&store, "ASK { ex:t ex:second true }"),
        "order 2 fired after order 1 (its condition needs ex:first)"
    );
    assert_eq!(n, 2);
}

/// `sh:condition`: a rule fires only for focus nodes that conform to the
/// condition shape.
#[test]
fn rule_condition_filters_focus_nodes() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:rule [ a sh:TripleRule ; sh:condition ex:Adult ;
            sh:subject sh:this ; sh:predicate ex:mayVote ; sh:object true ] .
ex:Adult a sh:NodeShape ; sh:property [ sh:path ex:age ; sh:minInclusive 18 ] .
"#;
    let data = "ex:ann a ex:Person ; ex:age 34 . ex:bob a ex:Person ; ex:age 12 .";
    let store = store_with(shapes, data);
    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert!(
        ask(&store, "ASK { ex:ann ex:mayVote true }"),
        "ann conforms to ex:Adult"
    );
    assert!(
        !ask(&store, "ASK { ex:bob ex:mayVote true }"),
        "bob does not"
    );
    assert_eq!(n, 1);
}

/// A deactivated rule, or a rule on a deactivated shape, does not fire.
#[test]
fn deactivated_rules_do_not_fire() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:TripleRule ; sh:deactivated true ;
            sh:subject sh:this ; sh:predicate ex:fromRule ; sh:object true ] .
ex:Off a sh:NodeShape ; sh:targetClass ex:Thing ; sh:deactivated true ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:fromShape ; sh:object true ] .
"#;
    let store = store_with(shapes, "ex:t a ex:Thing .");
    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert!(
        !ask(&store, "ASK { ex:t ex:fromRule ?x }"),
        "deactivated rule"
    );
    assert!(
        !ask(&store, "ASK { ex:t ex:fromShape ?x }"),
        "rule of a deactivated shape"
    );
    assert_eq!(n, 0);
}

// ─── Node expressions in triple rules (SHACL-AF §6, §8.5) ─────────────────────

/// SHACL-AF makes `sh:subject`/`sh:predicate`/`sh:object` node expressions: a
/// blank node there (`sh:object [ sh:path ex:p ]`) is an expression the rule
/// evaluates per focus node, and the rule derives one triple per combination
/// of the three result sets. It used to be loaded as a fixed term, so every
/// focus node got a triple pointing at the shapes graph's own blank node,
/// written into the data graph; then it was refused at load.
#[test]
fn triple_rule_node_expressions_compute_their_terms() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:copy ; sh:object [ sh:path ex:p ] ] ;
  sh:rule [ a sh:TripleRule ; sh:subject [ sh:path ex:p ] ; sh:predicate ex:backTo ; sh:object sh:this ] ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate [ sh:path ex:pred ] ; sh:object true ] ."#;
    let store = store_with(
        shapes,
        "ex:t a ex:Thing ; ex:p ex:v1, ex:v2 ; ex:pred ex:q . ex:v1 ex:name \"one\" .",
    );
    infer(&store, "urn:shapes", &[]).unwrap();
    for q in [
        "ASK { ex:t ex:copy ex:v1 }",
        "ASK { ex:t ex:copy ex:v2 }",
        "ASK { ex:v1 ex:backTo ex:t }",
        "ASK { ex:v2 ex:backTo ex:t }",
        "ASK { ex:t ex:q true }",
    ] {
        assert!(ask(&store, q), "missing: {q}");
    }
    assert!(
        !ask(
            &store,
            "ASK { ?s ?p ?o FILTER(isBlank(?o) || isBlank(?s)) }"
        ),
        "no triple may point at the shapes graph's expression node"
    );
    assert_eq!(rows(&store, "SELECT ?o WHERE { ex:t ex:copy ?o }"), 2);
}

/// A function expression calls its SHACL function once per combination of
/// its arguments' outputs (§6.4): two values of `ex:p1`, one of `ex:p2` and
/// three of `ex:p3` derive six triples. (After TopQuadrant's
/// `rules/triple/functions-permutations` test.)
#[test]
fn triple_rule_function_expression_takes_every_argument_combination() {
    let shapes = r#"
ex:concat3 a sh:SPARQLFunction ;
  sh:parameter [ sh:path ex:arg1 ] ; sh:parameter [ sh:path ex:arg2 ] ; sh:parameter [ sh:path ex:arg3 ] ;
  sh:select "SELECT ?result WHERE { BIND (CONCAT($arg1, $arg2, $arg3) AS ?result) }" .
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:target ;
    sh:object [ ex:concat3 ( [ sh:path ex:p1 ] [ sh:path ex:p2 ] [ sh:path ex:p3 ] ) ] ] ."#;
    let store = store_with(
        shapes,
        r#"ex:t a ex:Thing ; ex:p1 "1a ", "1b " ; ex:p2 "2a " ; ex:p3 "3a ", "3b ", "3c " ."#,
    );
    infer(&store, "urn:shapes", &[]).unwrap();
    assert_eq!(rows(&store, "SELECT ?o WHERE { ex:t ex:target ?o }"), 6);
    assert!(ask(&store, r#"ASK { ex:t ex:target "1b 2a 3c " }"#));
    // An argument with no value: the call is not made at all.
    let store = store_with(shapes, r#"ex:t a ex:Thing ; ex:p1 "1a " ; ex:p3 "3a " ."#);
    infer(&store, "urn:shapes", &[]).unwrap();
    assert_eq!(rows(&store, "SELECT ?o WHERE { ex:t ex:target ?o }"), 0);
}

/// Union, intersection, filter-shape and chained path (`sh:nodes`)
/// expressions (§6.3, §6.5–6.7).
#[test]
fn node_expression_kinds_union_intersection_filter_and_nodes() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:any ;
    sh:object [ sh:union ( [ sh:path ex:a ] [ sh:path ex:b ] ) ] ] ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:both ;
    sh:object [ sh:intersection ( [ sh:path ex:a ] [ sh:path ex:b ] ) ] ] ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:goodA ;
    sh:object [ sh:filterShape [ sh:class ex:Good ] ; sh:nodes [ sh:path ex:a ] ] ] ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:aName ;
    sh:object [ sh:path ex:name ; sh:nodes [ sh:path ex:a ] ] ] ."#;
    let data = r#"
ex:t a ex:Thing ; ex:a ex:x, ex:y ; ex:b ex:y, ex:z .
ex:x a ex:Good ; ex:name "x" .
ex:y ex:name "y" ."#;
    let store = store_with(shapes, data);
    infer(&store, "urn:shapes", &[]).unwrap();
    assert_eq!(rows(&store, "SELECT ?o WHERE { ex:t ex:any ?o }"), 3);
    assert_eq!(rows(&store, "SELECT ?o WHERE { ex:t ex:both ?o }"), 1);
    assert!(ask(&store, "ASK { ex:t ex:both ex:y }"));
    assert_eq!(rows(&store, "SELECT ?o WHERE { ex:t ex:goodA ?o }"), 1);
    assert!(ask(&store, "ASK { ex:t ex:goodA ex:x }"));
    assert_eq!(rows(&store, "SELECT ?o WHERE { ex:t ex:aName ?o }"), 2);
}

/// A node expression may not contain itself (§6): such a rule fails the run
/// at load, before anything is written.
#[test]
fn a_node_expression_that_contains_itself_is_refused() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:copy ; sh:object _:loop ] .
_:loop sh:path ex:p ; sh:nodes _:loop ."#;
    let store = store_with(shapes, "ex:t a ex:Thing ; ex:p ex:v .");
    let r = infer(&store, "urn:shapes", &[]);
    assert!(r.as_ref().is_err_and(|e| e.contains("itself")), "got {r:?}");
    assert!(!ask(&store, "ASK { ?s ex:copy ?o }"), "nothing is written");
}

/// A blank node that is none of the seven node-expression kinds (here two
/// list-valued triples) is not a term to copy into the data either: the run
/// fails at load.
#[test]
fn an_ill_formed_node_expression_is_refused() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:copy ;
    sh:object [ ex:f ( 1 ) ; ex:g ( 2 ) ] ] ."#;
    let store = store_with(shapes, "ex:t a ex:Thing .");
    let r = infer(&store, "urn:shapes", &[]);
    assert!(
        r.as_ref().is_err_and(|e| e.contains("node expression")),
        "got {r:?}"
    );
    assert!(!ask(&store, "ASK { ?s ex:copy ?o }"), "nothing is written");
}

// ─── sh:SPARQLFunction: sh:ask, arguments as terms, data access ───────────────

/// An `sh:ask` body returns the ASK result as `xsd:boolean` (SHACL-AF §5.4).
#[test]
fn sparql_function_with_an_ask_body() {
    let shapes = r#"
ex:isBig a sh:SPARQLFunction ; sh:parameter [ sh:path ex:x ] ;
  sh:returnType xsd:boolean ; sh:ask "ASK { FILTER ($x > 10) }" .
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { $this <http://example.org/big> true } WHERE { $this <http://example.org/v> ?v FILTER (<http://example.org/isBig>(?v)) }" ] ."#;
    let store = store_with(
        shapes,
        "ex:t1 a ex:Thing ; ex:v 20 . ex:t2 a ex:Thing ; ex:v 5 .",
    );
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:t1 ex:big true }"));
    assert!(!ask(&store, "ASK { ex:t2 ex:big true }"));
}

/// Arguments are bound to the body's variables as RDF terms. Textual
/// replacement of `$x` also rewrote `$xy`, and pasted an argument's lexical
/// form into the query, where a literal could close a string and add clauses.
#[test]
fn sparql_function_arguments_are_bound_as_terms() {
    let shapes = r#"
ex:pair a sh:SPARQLFunction ;
  sh:parameter [ sh:path ex:x ; sh:order 0 ] ; sh:parameter [ sh:path ex:xy ; sh:order 1 ] ;
  sh:select "SELECT (CONCAT(STR($x), '|', STR($xy)) AS ?r) WHERE {}" .
ex:same a sh:SPARQLFunction ; sh:parameter [ sh:path ex:x ] ;
  sh:select "SELECT ?r WHERE { BIND ($x AS ?r) }" .
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { $this <http://example.org/pair> ?p . $this <http://example.org/same> ?s } WHERE { $this <http://example.org/label> ?l . BIND (<http://example.org/pair>('a', 'b') AS ?p) BIND (<http://example.org/same>(?l) AS ?s) }" ] ."#;
    let hostile = r#"x") AS ?r) WHERE {} #"#;
    let data = format!(
        "ex:t a ex:Thing ; ex:label \"{}\" .",
        hostile.replace('"', "\\\"")
    );
    let store = store_with(shapes, &data);
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(
        ask(&store, r#"ASK { ex:t ex:pair "a|b" }"#),
        "$x and $xy are different variables"
    );
    assert!(
        ask(&store, "ASK { ex:t ex:same ?s . ex:t ex:label ?s }"),
        "a literal argument comes back unchanged"
    );
}

/// Without `sh:order`, parameters are ordered by the local names of their
/// paths (SHACL-AF §5.2), whatever order the shapes graph lists them in.
#[test]
fn sparql_function_parameters_without_order_sort_by_local_name() {
    let shapes = r#"
ex:cat a sh:SPARQLFunction ; sh:parameter [ sh:path ex:b ] ; sh:parameter [ sh:path ex:a ] ;
  sh:select "SELECT (CONCAT($a, $b) AS ?r) WHERE {}" .
ex:S a sh:NodeShape ; sh:targetNode ex:t ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { $this <http://example.org/cat> ?c } WHERE { BIND (<http://example.org/cat>('1', '2') AS ?c) }" ] ."#;
    let store = store_with(shapes, "ex:t a ex:Thing .");
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, r#"ASK { ex:t ex:cat "12" }"#));
}

/// A function body reads the run's data graphs — and nothing else. It used to
/// run against an empty store, so any body that queried data returned
/// unbound. A `GRAPH` or `FROM NAMED` in the body reaches no other graph.
#[test]
fn sparql_function_body_reads_the_run_data_graphs_only() {
    let shapes = r#"
ex:labelOf a sh:SPARQLFunction ; sh:parameter [ sh:path ex:node ] ;
  sh:select "SELECT ?l WHERE { $node <http://www.w3.org/2000/01/rdf-schema#label> ?l }" .
ex:peek a sh:SPARQLFunction ;
  sh:select "SELECT ?l FROM NAMED <urn:other> WHERE { GRAPH ?g { ?s <http://example.org/secret> ?l } }" .
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { $this <http://example.org/label> ?l . $this <http://example.org/peeked> ?p } WHERE { BIND (<http://example.org/labelOf>($this) AS ?l) BIND (<http://example.org/peek>() AS ?p) }" ] ."#;
    let store = TripleStore::in_memory().unwrap();
    for (graph, ttl) in [
        ("urn:shapes", shapes),
        ("urn:data", "ex:t a ex:Thing ; rdfs:label \"in data\" ."),
        (
            "urn:other",
            "ex:t rdfs:label \"elsewhere\" . ex:x ex:secret \"SECRET\" .",
        ),
    ] {
        store
            .load_str(&format!("{PFX}{ttl}"), RdfFormat::Turtle, Some(graph))
            .unwrap();
    }
    infer(&store, "urn:shapes", &["urn:data".to_string()]).unwrap();
    assert!(ask(
        &store,
        r#"ASK { GRAPH <urn:data> { ex:t ex:label "in data" } }"#
    ));
    assert!(
        !ask(&store, r#"ASK { GRAPH ?g { ex:t ex:label "elsewhere" } }"#),
        "the body read a graph outside the run"
    );
    assert!(
        !ask(&store, "ASK { GRAPH ?g { ?s ex:peeked ?p } }"),
        "the body escaped the run's data graphs"
    );
}

/// A function that calls itself stops at the recursion bound instead of
/// exhausting the stack: the call is unbound and the run completes.
#[test]
fn a_recursive_sparql_function_is_bounded() {
    let shapes = r#"
ex:loop a sh:SPARQLFunction ; sh:parameter [ sh:path ex:x ] ;
  sh:select "SELECT (<http://example.org/loop>($x) AS ?r) WHERE {}" .
ex:S a sh:NodeShape ; sh:targetNode ex:t ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { $this <http://example.org/r> ?r } WHERE { BIND (<http://example.org/loop>(1) AS ?r) }" ] ."#;
    let store = store_with(shapes, "ex:t a ex:Thing .");
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(!ask(&store, "ASK { ?s ex:r ?o }"));
}

/// `sh:prefixes` of a function follows `owl:imports` (SHACL §5.2.1), as it
/// does for constraints and rules.
#[test]
fn sparql_function_prefixes_follow_owl_imports() {
    let shapes = r#"
ex:onto owl:imports ex:base .
ex:base sh:declare [ sh:prefix "q" ; sh:namespace "http://example.org/q#"^^xsd:anyURI ] .
ex:ns a sh:SPARQLFunction ; sh:prefixes ex:onto ;
  sh:select "SELECT (STR(q:thing) AS ?r) WHERE {}" .
ex:S a sh:NodeShape ; sh:targetNode ex:t ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { $this <http://example.org/ns> ?n } WHERE { BIND (<http://example.org/ns>() AS ?n) }" ] ."#;
    let store = store_with(
        &format!("@prefix owl: <http://www.w3.org/2002/07/owl#> .\n{shapes}"),
        "ex:t a ex:Thing .",
    );
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(
        &store,
        r#"ASK { ex:t ex:ns "http://example.org/q#thing" }"#
    ));
}

/// A function whose body does not parse fails the run of the shapes graph
/// that declares it, as a constraint or rule body does; it used to be dropped,
/// and every call to it was silently unbound.
#[test]
fn a_sparql_function_whose_body_does_not_parse_fails_the_run() {
    let shapes = r#"
ex:broken a sh:SPARQLFunction ; sh:select "SELECT ?r WHERE { THIS IS NOT SPARQL" .
ex:S a sh:NodeShape ; sh:targetNode ex:t ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:seen ; sh:object true ] ."#;
    let store = store_with(shapes, "ex:t a ex:Thing .");
    let r = infer(&store, "urn:shapes", &[]);
    assert!(r.as_ref().is_err_and(|e| e.contains("broken")), "got {r:?}");
}

/// A rule shape whose `sh:target` cannot select focus nodes used to fall back
/// to no targets (`unwrap_or_default`), so the rule silently never fired.
#[test]
fn a_rule_whose_target_cannot_be_loaded_fails_the_run() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:target [ sh:select "THIS IS NOT SPARQL" ] ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:seen ; sh:object true ] ."#;
    let store = store_with(shapes, "ex:t a ex:Thing .");
    let r = infer(&store, "urn:shapes", &[]);
    assert!(r.is_err(), "got {r:?}");
}

/// A deactivated `sh:condition` shape is one every node conforms to, so the
/// rule fires (SHACL §2.1.6).
#[test]
fn a_deactivated_condition_does_not_block_its_rule() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:TripleRule ; sh:subject sh:this ; sh:predicate ex:seen ; sh:object true ;
            sh:condition [ sh:class ex:Missing ; sh:deactivated true ] ] ."#;
    let store = store_with(shapes, "ex:t a ex:Thing .");
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:t ex:seen true }"));
}

// ─── Conditions read the run the way validation does ────────────────────────

/// Load `shapes` into `urn:shapes` and each `(graph, turtle)` into its graph.
fn store_with_graphs(shapes: &str, graphs: &[(&str, &str)]) -> TripleStore {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            &format!("{PFX}{shapes}"),
            RdfFormat::Turtle,
            Some("urn:shapes"),
        )
        .unwrap();
    for (g, data) in graphs {
        store
            .load_str(&format!("{PFX}{data}"), RdfFormat::Turtle, Some(g))
            .unwrap();
    }
    store
}

/// A rule condition evaluates its `sh:path` over the merge of the run's data
/// graphs (SHACL §3.4). It used to walk an IRI focus node's path inside each
/// graph in turn, so a condition whose path crossed from one graph into
/// another was never met and the rule never fired.
#[test]
fn a_rule_condition_follows_a_path_across_data_graphs() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Bridge ;
  sh:rule [ a sh:TripleRule ; sh:condition ex:HasDeckWidth ;
            sh:subject sh:this ; sh:predicate ex:measured ; sh:object true ] .
ex:HasDeckWidth a sh:NodeShape ;
  sh:property [ sh:path ( ex:hasDeck ex:width ) ; sh:minCount 1 ] .
"#;
    let store = store_with_graphs(
        shapes,
        &[
            (
                "urn:instances",
                "ex:b1 a ex:Bridge ; ex:hasDeck ex:d1 . ex:b2 a ex:Bridge ; ex:hasDeck ex:d2 .",
            ),
            ("urn:details", "ex:d1 ex:width 12 ."),
        ],
    );
    let n = infer_into(
        &store,
        "urn:shapes",
        &["urn:instances".to_string(), "urn:details".to_string()],
        Some("urn:inferred"),
    )
    .unwrap();
    assert!(
        ask(
            &store,
            "ASK { GRAPH <urn:inferred> { ex:b1 ex:measured true } }"
        ),
        "b1's deck width lives in urn:details, which the run reads"
    );
    assert!(
        !ask(&store, "ASK { GRAPH ?g { ex:b2 ex:measured true } }"),
        "b2's deck has no width in any graph"
    );
    assert_eq!(n, 1);
}

/// A `sh:sparql` condition checks a blank-node focus node. Blank nodes could
/// not be pre-bound, so the constraint was skipped and every blank node met
/// the condition: the rule fired for the very node the condition excludes.
#[test]
fn a_sparql_condition_checks_a_blank_node_focus() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Person ;
  sh:rule [ a sh:TripleRule ; sh:condition ex:Adult ;
            sh:subject sh:this ; sh:predicate ex:mayVote ; sh:object true ] .
ex:Adult a sh:NodeShape ;
  sh:sparql [ sh:select """SELECT $this WHERE { $this <http://example.org/age> ?a . FILTER (?a < 18) }""" ] .
"#;
    let store = store_with(
        shapes,
        "[ a ex:Person ; ex:age 12 ; ex:name \"minor\" ] . [ a ex:Person ; ex:age 40 ; ex:name \"adult\" ] .",
    );
    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert!(
        ask(&store, "ASK { ?p ex:name \"adult\" ; ex:mayVote true }"),
        "the adult meets the condition"
    );
    assert!(
        !ask(&store, "ASK { ?p ex:name \"minor\" ; ex:mayVote true }"),
        "the minor does not"
    );
    assert_eq!(n, 1);
}

/// `$this` reaches a filter that no triple pattern of the rule binds it in.
/// The query optimizer took such a `$this` for a variable that is never bound
/// and dropped the filter's group, so the rule never fired.
#[test]
fn a_sparql_rule_sees_this_in_a_filter_only_scope() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct """CONSTRUCT { $this <http://example.org/checked> true } WHERE { FILTER (isIRI($this) && bound($this)) }""" ] .
"#;
    let store = store_with(shapes, "ex:t a ex:Thing .");
    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:t ex:checked true }"));
    assert_eq!(n, 1);
}

// ─── Pre-bound $this in rule expressions (SHACL §5.6.1, SHACL-AF §8.3) ───────
//
// A rule's `$this` is pre-bound: in an expression it stands for the focus node
// exactly as a constant would. The query optimizer was never told the variable
// is bound, so it treated it as unbound and rewrote the expressions that
// mention it — these rules derived nothing, or the wrong thing.

/// `BIND ($this AS ?x)` copies the focus node; the optimizer dropped the
/// `BIND` as binding nothing, at the top of the WHERE clause and in a nested
/// group alike, for an IRI and a blank-node focus node alike.
#[test]
fn sparql_rule_bind_of_this_copies_the_focus_node() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { ?x <http://example.org/self> ?x } WHERE { BIND ($this AS ?x) }" ] ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { $this <http://example.org/tag> ?y } WHERE { { BIND ($this AS ?y) } UNION { $this <http://example.org/alias> ?y } }" ] ."#;
    let store = store_with(
        shapes,
        r#"ex:t a ex:Thing . [] a ex:Thing ; ex:name "anon" ."#,
    );
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:t ex:self ex:t }"), "top-level BIND");
    assert!(
        ask(&store, "ASK { ex:t ex:tag ex:t }"),
        "BIND in a UNION branch"
    );
    assert!(
        ask(
            &store,
            r#"ASK { ?b ex:name "anon" ; ex:self ?b ; ex:tag ?b }"#
        ),
        "a blank-node focus node is copied too"
    );
}

/// `BOUND ($this)` is true: the optimizer folded it to false.
#[test]
fn sparql_rule_this_is_bound() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetClass ex:Thing ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { $this <http://example.org/bound> true } WHERE { FILTER (BOUND($this)) }" ] ."#;
    let store = store_with(
        shapes,
        r#"ex:t a ex:Thing . [] a ex:Thing ; ex:name "anon" ."#,
    );
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:t ex:bound true }"));
    assert!(ask(&store, r#"ASK { [] ex:name "anon" ; ex:bound true }"#));
}

/// `?v = $this` compares values: a literal focus node `1` equals `1.0`. When
/// `$this` occurs in the expression only, the optimizer turned `=` into
/// `sameTerm`, which compares the terms. (Two lexical forms of one `xsd:int`
/// would not show it: the store keeps derived integer types as canonical
/// `xsd:integer`.)
#[test]
fn sparql_rule_compares_a_literal_focus_node_by_value() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetObjectsOf ex:code ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { ?b <http://example.org/sameCode> $this } WHERE { ?b <http://example.org/alt> ?v . FILTER (?v = $this) }" ] ."#;
    let data = r#"
ex:a ex:code "1"^^xsd:int .
ex:b ex:alt "1.0"^^xsd:decimal .
ex:c ex:alt "2"^^xsd:int ."#;
    let store = store_with(shapes, data);
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:b ex:sameCode 1 }"), "1.0 = 1");
    assert!(!ask(&store, "ASK { ex:c ex:sameCode 1 }"), "2 != 1");
}

/// A triple-term focus node has no constant form in an expression; the rule
/// still sees it bound.
#[test]
fn sparql_rule_bind_of_a_triple_term_focus_node() {
    let shapes = r#"
ex:S a sh:NodeShape ; sh:targetObjectsOf ex:about ;
  sh:rule [ a sh:SPARQLRule ;
    sh:construct "CONSTRUCT { ?s <http://example.org/mentioned> true } WHERE { BIND ($this AS ?t) BIND (SUBJECT(?t) AS ?s) }" ] ."#;
    let store = store_with(shapes, "ex:note ex:about <<( ex:s ex:p ex:o )>> .");
    infer(&store, "urn:shapes", &[]).unwrap();
    assert!(ask(&store, "ASK { ex:s ex:mentioned true }"));
}

// ──────────────────── sh:entailment sh:Rules (SHACL-AF §8.3) ────────────────────

/// Validating under `sh:entailment sh:Rules` reports what running the rules
/// with `/infer` and then validating reports — but leaves the data alone,
/// while `/infer` still materialises (the declaration does not change it).
#[test]
fn entailment_rules_validation_matches_infer_then_validate() {
    let rules_and_shapes = r#"
        ex:R a sh:NodeShape ; sh:targetClass ex:Person ;
            sh:rule [ a sh:SPARQLRule ;
                sh:construct "CONSTRUCT { $this a ex:Adult } WHERE { $this ex:age ?a . FILTER(?a >= 18) }" ;
                sh:prefixes ex: ] ;
            sh:rule [ a sh:TripleRule ; sh:order 1 ;
                sh:condition ex:AdultShape ;
                sh:subject sh:this ; sh:predicate ex:mayVote ; sh:object true ] .
        ex: sh:declare [ sh:prefix "ex" ; sh:namespace "http://example.org/"^^xsd:anyURI ] .
        ex:AdultShape a sh:NodeShape ; sh:class ex:Adult .
        ex:VoterShape a sh:NodeShape ; sh:targetSubjectsOf ex:mayVote ;
            sh:property [ sh:path ex:registered ; sh:minCount 1 ] ."#;
    let data = r#"
        ex:alice a ex:Person ; ex:age 30 .
        ex:carol a ex:Person ; ex:age 50 ; ex:registered true .
        ex:bob   a ex:Person ; ex:age 12 ."#;
    let focus = |r: &open_triplestore::shacl::report::ValidationReport| {
        let mut f: Vec<String> = r.results.iter().map(|v| v.focus_node.clone()).collect();
        f.sort();
        f
    };

    // Under the regime: the rules' output is validated, nothing is stored.
    let store = store_with(
        &format!("<urn:shapes> sh:entailment sh:Rules .\n{rules_and_shapes}"),
        data,
    );
    let len = store.len().unwrap();
    let entailed = open_triplestore::shacl::validate(&store, "urn:shapes", &[]).unwrap();
    assert_eq!(store.len().unwrap(), len, "validation stored nothing");
    assert!(!ask(&store, "ASK { ?s ex:mayVote true }"));

    // The same rules materialised first, then validated without the regime.
    let reference = store_with(rules_and_shapes, data);
    infer(&reference, "urn:shapes", &[]).unwrap();
    assert!(ask(&reference, "ASK { ex:alice ex:mayVote true }"));
    let expected = open_triplestore::shacl::validate(&reference, "urn:shapes", &[]).unwrap();
    assert_eq!(focus(&entailed), focus(&expected));
    assert_eq!(
        focus(&entailed),
        vec!["http://example.org/alice".to_string()]
    );

    // `/infer` over a shapes graph that declares the regime still materialises.
    let n = infer(&store, "urn:shapes", &[]).unwrap();
    assert!(n >= 1);
    assert!(ask(&store, "ASK { ex:alice ex:mayVote true }"));
}
