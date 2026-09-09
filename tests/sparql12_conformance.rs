//! SPARQL 1.2 / RDF 1.2 (triple-term) conformance tests.
//!
//! Open Triplestore pins **oxigraph 0.5**, which implements the **RDF 1.2 /
//! SPARQL 1.2** model rather than the older RDF-star CG one:
//!
//! * a **triple term** is written `<<( s p o )>>` and may appear in **object
//!   position only**;
//! * it is attached to a statement through `rdf:reifies`, whose subject (the
//!   *reifier*) is an ordinary IRI or blank node — the reifier is NOT itself a
//!   triple term, so `isTRIPLE` is false for it;
//! * `<< s p o >>` is now **reifier** shorthand: it mints a blank node with
//!   `rdf:reifies <<( s p o )>>` and does not assert the base triple;
//! * `s p o {| … |}` asserts the base triple AND attaches a reifier carrying
//!   the annotations.
//!
//! Quoting is still not asserting, and referential opacity, per-graph
//! isolation, OPTIONAL / NOT EXISTS over a triple pattern, nested quoting and
//! the `TRIPLE()` constructor all hold.
//!
//! Spec refs: <https://www.w3.org/TR/sparql12-query/>,
//!            <https://www.w3.org/TR/rdf12-concepts/>

#![cfg(feature = "rdf-12")]

use open_triplestore::store::TripleStore;
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;

const PFX: &str = "PREFIX : <http://ex/>\n\
PREFIX owl: <http://www.w3.org/2002/07/owl#>\n\
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n\
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\n";

fn ts() -> TripleStore {
    TripleStore::in_memory().unwrap()
}

fn upd(s: &TripleStore, body: &str) {
    s.update(&format!("{PFX}{body}")).unwrap();
}

fn sel(s: &TripleStore, body: &str) -> Vec<Vec<String>> {
    match s.query(&format!("{PFX}{body}")).unwrap() {
        QueryResults::Solutions(sols) => {
            let vars: Vec<String> = sols
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            sols.into_iter()
                .map(|sol| {
                    let sol = sol.unwrap();
                    vars.iter()
                        .map(|v| {
                            sol.get(v.as_str())
                                .map(|t| t.to_string())
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .collect()
        }
        _ => panic!("expected SELECT solutions"),
    }
}

fn ask(s: &TripleStore, body: &str) -> bool {
    match s.query(&format!("{PFX}{body}")).unwrap() {
        QueryResults::Boolean(b) => b,
        _ => panic!("expected ASK boolean"),
    }
}

fn construct(s: &TripleStore, body: &str) -> Vec<oxigraph::model::Triple> {
    match s.query(&format!("{PFX}{body}")).unwrap() {
        QueryResults::Graph(g) => g.map(|t| t.unwrap()).collect(),
        _ => panic!("expected CONSTRUCT graph"),
    }
}

// ═══════════════════════════════════════════════════════════
// Quoting semantics: a quoted triple is NOT asserted (tt-01)
// ═══════════════════════════════════════════════════════════

#[test]
fn star_quoted_triple_is_not_asserted() {
    let s = ts();
    upd(
        &s,
        r#"INSERT DATA { << :alice :age "30"^^xsd:integer >> :source :HR }"#,
    );
    // The metadata triple is matchable...
    let src = sel(
        &s,
        r#"SELECT ?src WHERE { << :alice :age "30"^^xsd:integer >> :source ?src }"#,
    );
    assert_eq!(src.len(), 1);
    assert!(src[0][0].contains("HR"));
    // ...but the inner triple is NOT asserted by quoting alone.
    assert!(
        !ask(&s, r#"ASK { :alice :age "30"^^xsd:integer }"#),
        "quoting a triple must not assert it"
    );
}

// ═══════════════════════════════════════════════════════════
// Referential opacity (tt-03 / tt-15): quoted triples are distinct
// even when their components are owl:sameAs / numerically equal.
// ═══════════════════════════════════════════════════════════

#[test]
fn star_referential_opacity_distinct_terms() {
    let s = ts();
    upd(
        &s,
        r#"INSERT DATA {
            :clark owl:sameAs :superman .
            << :superman :can :fly >> :source :Lois .
        }"#,
    );
    // Without substitution into the quoted triple, querying clark's variant matches nothing.
    let r = sel(
        &s,
        r#"SELECT ?src WHERE { << :clark :can :fly >> :source ?src }"#,
    );
    assert_eq!(
        r.len(),
        0,
        "owl:sameAs must not substitute inside a quoted triple"
    );
}

// Opacity holds across datatypes: an integer and a value-equal decimal are
// distinct RDF terms, so they do not match inside a quoted triple.
#[test]
fn star_referential_opacity_cross_datatype() {
    let s = ts();
    upd(
        &s,
        r#"INSERT DATA { << :alice :age "30"^^xsd:integer >> :source :HR }"#,
    );
    let r = sel(
        &s,
        r#"SELECT ?src WHERE { << :alice :age "30.0"^^xsd:decimal >> :source ?src }"#,
    );
    assert_eq!(
        r.len(),
        0,
        "value-equal but differently-typed literals are distinct quoted-triple terms"
    );
}

// Documented oxigraph behavior: same-datatype xsd:integer lexical forms ARE
// canonicalized ("030" == "30"), so they DO match inside a quoted triple. Strict
// RDF-1.2 triple-term opacity would keep them distinct; oxigraph normalizes the
// integer lexical form per its RDF 1.1 term handling.
#[test]
fn star_integer_lexical_canonicalization_in_quoted_triple() {
    let s = ts();
    upd(
        &s,
        r#"INSERT DATA { << :alice :age "30"^^xsd:integer >> :source :HR }"#,
    );
    let r = sel(
        &s,
        r#"SELECT ?src WHERE { << :alice :age "030"^^xsd:integer >> :source ?src }"#,
    );
    assert_eq!(
        r.len(),
        1,
        "oxigraph canonicalizes xsd:integer lexical forms; 030 == 30"
    );
    assert!(r[0][0].contains("HR"));
}

// ═══════════════════════════════════════════════════════════
// Triple-term accessor functions (tt-02 / tt-04): SUBJECT / PREDICATE
// / OBJECT / isTRIPLE / TRIPLE constructor.
// ═══════════════════════════════════════════════════════════

#[test]
fn star_accessor_functions() {
    let s = ts();
    // RDF 1.2: a triple term appears in OBJECT position, reached through
    // `rdf:reifies`. It is the triple term — not the reifier that points at it —
    // that satisfies isTRIPLE and carries the accessors.
    upd(
        &s,
        r#"INSERT DATA {
            _:r rdf:reifies <<( :alice :knows :bob )>> .
            _:r :certainty "0.9"^^xsd:decimal .
            :plainStmt :certainty "0.5"^^xsd:decimal .
        }"#,
    );
    let r = sel(
        &s,
        "SELECT ?s ?p ?o WHERE { \
           ?r rdf:reifies ?t . FILTER(isTRIPLE(?t)) \
           BIND(SUBJECT(?t) AS ?s) BIND(PREDICATE(?t) AS ?p) BIND(OBJECT(?t) AS ?o) }",
    );
    assert_eq!(r.len(), 1, "one triple term is reified");
    assert!(r[0][0].contains("alice"));
    assert!(r[0][1].contains("knows"));
    assert!(r[0][2].contains("bob"));

    // The reifier itself is a blank node, not a triple term.
    let not_triples = sel(
        &s,
        "SELECT ?r WHERE { ?r :certainty ?c . FILTER(isTRIPLE(?r)) }",
    );
    assert!(
        not_triples.is_empty(),
        "a reifier is not a triple term, got {not_triples:?}"
    );
}

#[test]
fn star_triple_constructor() {
    let s = ts();
    let r = sel(
        &s,
        "SELECT ?t WHERE { BIND(TRIPLE(:a, :b, :c) AS ?t) FILTER(isTRIPLE(?t)) }",
    );
    assert_eq!(r.len(), 1, "TRIPLE() constructs a triple term");
}

// ═══════════════════════════════════════════════════════════
// Nested quoted triples (tt-04, object/subject nesting that oxigraph allows)
// ═══════════════════════════════════════════════════════════

#[test]
fn star_nested_quoted_triple() {
    let s = ts();
    upd(
        &s,
        r#"INSERT DATA { << << :alice :trusts :bob >> :since "2020"^^xsd:gYear >> :confidence "0.9"^^xsd:decimal }"#,
    );
    let r = sel(
        &s,
        r#"SELECT ?conf WHERE { << << :alice :trusts ?x >> :since ?y >> :confidence ?conf }"#,
    );
    assert_eq!(r.len(), 1);
    assert!(r[0][0].contains("0.9"));
}

// ═══════════════════════════════════════════════════════════
// Aggregation over triple-term keys (tt-09, minus ORDER-BY determinism
// which SPARQL 1.2 leaves undefined for triple terms).
// ═══════════════════════════════════════════════════════════

#[test]
fn star_group_by_quoted_triple() {
    let s = ts();
    // Group by the TRIPLE TERM. Under RDF 1.2 each reifier is a distinct blank
    // node, so grouping by the reifier would count three groups of one; the
    // triple term is what two of these statements share.
    upd(
        &s,
        r#"INSERT DATA {
            _:r1 rdf:reifies <<( :alice :knows :bob )>> .   _:r1 :src :S1 .
            _:r2 rdf:reifies <<( :alice :knows :bob )>> .   _:r2 :src :S2 .
            _:r3 rdf:reifies <<( :alice :knows :carol )>> . _:r3 :src :S1 .
        }"#,
    );
    let r = sel(
        &s,
        "SELECT ?t (COUNT(DISTINCT ?src) AS ?cnt) WHERE { \
           ?r rdf:reifies ?t . ?r :src ?src . FILTER(isTRIPLE(?t)) } GROUP BY ?t",
    );
    assert_eq!(r.len(), 2, "two distinct triple-term groups");
    // Term equality on triple terms is defined; the bob-group must count 2.
    let counts: Vec<&str> = r.iter().map(|row| row[1].as_str()).collect();
    assert!(
        counts.iter().any(|c| c.contains("\"2\"")),
        "bob group counts 2, got {:?}",
        counts
    );
    assert!(
        counts.iter().any(|c| c.contains("\"1\"")),
        "carol group counts 1, got {:?}",
        counts
    );
}

// ═══════════════════════════════════════════════════════════
// OPTIONAL / NOT EXISTS with a triple-term pattern parameterized by an
// outer variable (tt-08 / tt-10).
// ═══════════════════════════════════════════════════════════

#[test]
fn star_optional_quoted_pattern() {
    let s = ts();
    upd(
        &s,
        r#"INSERT DATA {
            :alice :knows :bob .
            :alice :knows :carol .
            << :alice :knows :carol >> :certainty "0.8"^^xsd:decimal .
        }"#,
    );
    let r = sel(
        &s,
        "SELECT ?person ?cert WHERE { \
           :alice :knows ?person . \
           OPTIONAL { << :alice :knows ?person >> :certainty ?cert } } ORDER BY ?person",
    );
    assert_eq!(r.len(), 2);
    // :bob (no annotation) -> cert unbound ; :carol -> 0.8
    assert!(r[0][0].contains("bob"));
    assert!(r[0][1].is_empty());
    assert!(r[1][0].contains("carol"));
    assert!(r[1][1].contains("0.8"));
}

#[test]
fn star_not_exists_quoted_pattern() {
    let s = ts();
    upd(
        &s,
        r#"INSERT DATA {
            :alice :knows :bob .
            :alice :knows :carol .
            << :alice :knows :carol >> :certainty "0.8"^^xsd:decimal .
        }"#,
    );
    let r = sel(
        &s,
        "SELECT ?person WHERE { \
           :alice :knows ?person . \
           FILTER NOT EXISTS { << :alice :knows ?person >> :certainty ?c } }",
    );
    assert_eq!(r.len(), 1, "only :bob lacks an annotation");
    assert!(r[0][0].contains("bob"));
}

// ═══════════════════════════════════════════════════════════
// Property path traversal that reaches quoted-triple metadata (tt-11)
// ═══════════════════════════════════════════════════════════

#[test]
fn star_property_path_over_chain_to_quoted() {
    let s = ts();
    // Triple terms sit in object position, so `:describes` can carry one
    // directly — no reifier needed for this shape.
    upd(
        &s,
        r#"INSERT DATA {
            :chain :next :r1 . :r1 :next :r2 .
            :r1 :describes <<( :alice :trusts :bob )>> .
            :r2 :describes <<( :bob :trusts :carol )>> .
        }"#,
    );
    let r = sel(
        &s,
        "SELECT ?stmt ?t WHERE { :chain (:next)+ ?stmt . ?stmt :describes ?t . FILTER(isTRIPLE(?t)) } ORDER BY ?stmt",
    );
    assert_eq!(r.len(), 2, "path reaches r1 and r2 without looping");
    assert!(r[0][0].contains("r1"));
    assert!(r[1][0].contains("r2"));
}

// ═══════════════════════════════════════════════════════════
// CONSTRUCT with a quoted-triple template (tt-12)
// ═══════════════════════════════════════════════════════════

#[test]
fn star_construct_quoted_template() {
    let s = ts();
    upd(
        &s,
        r#"INSERT DATA { :r1 :describes <<( :alice :age "30"^^xsd:integer )>> }"#,
    );
    let triples = construct(
        &s,
        "CONSTRUCT { :r1copy :describes ?tt . :r1copy :derivedFrom :r1 } WHERE { :r1 :describes ?tt }",
    );
    assert_eq!(triples.len(), 2);
    let has_quoted_object = triples.iter().any(|t| {
        matches!(t.object, Term::Triple(_)) && t.predicate.as_str() == "http://ex/describes"
    });
    assert!(
        has_quoted_object,
        "the quoted triple must appear verbatim in the output graph"
    );
}

// ═══════════════════════════════════════════════════════════
// Multi-tenant named-graph isolation for quoted triples (tt-14, security)
// ═══════════════════════════════════════════════════════════

#[test]
fn star_named_graph_isolation() {
    let s = ts();
    upd(
        &s,
        r#"INSERT DATA {
            GRAPH <urn:tenant:A> {
                << :alice :salary "50000"^^xsd:integer >> :source :Payroll .
                :alice :salary "50000"^^xsd:integer .
            }
            GRAPH <urn:tenant:B> { :bob :role :Engineer . }
        }"#,
    );
    let b = sel(
        &s,
        "SELECT ?s ?p ?o WHERE { GRAPH <urn:tenant:B> { ?s ?p ?o } }",
    );
    assert_eq!(b.len(), 1, "tenant B sees only its own triple");
    assert!(b[0][0].contains("bob"));
    // Tenant A's quoted-triple metadata must not be visible inside tenant B.
    assert!(
        !ask(&s, "ASK { GRAPH <urn:tenant:B> { << :alice :salary \"50000\"^^xsd:integer >> :source ?x } }"),
        "quoted triples must not bleed across named-graph boundaries"
    );
}

// ═══════════════════════════════════════════════════════════
// (Was a tracked gap.) The RDF 1.2 triple-term surface syntax `<<( )>>` with
// The RDF 1.2 triple-term syntax and `rdf:reifies` are supported. This test
// asserted the opposite while the engine was on oxigraph 0.4.
// ═══════════════════════════════════════════════════════════

#[test]
fn star_new_triple_term_syntax_is_supported() {
    let s = ts();
    s.update(
        "PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> \
         INSERT DATA { _:r rdf:reifies <<( <http://ex/a> <http://ex/p> <http://ex/o> )>> }",
    )
    .expect("RDF 1.2 triple-term syntax must parse");

    // Stored shape: the reifier points at the triple term, and the base triple
    // is NOT asserted — quoting is not asserting.
    let r = sel(
        &s,
        "SELECT ?t WHERE { ?r rdf:reifies ?t . FILTER(isTRIPLE(?t)) }",
    );
    assert_eq!(r.len(), 1, "the triple term is stored and reachable");

    let asserted = sel(&s, "SELECT ?o WHERE { <http://ex/a> <http://ex/p> ?o }");
    assert!(
        asserted.is_empty(),
        "reifying a triple must not assert it, got {asserted:?}"
    );
}
