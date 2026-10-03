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

/// The two read paths every pin runs on: the engine alone, and the in-memory
/// mirror (four subject shards, the columnar copy, the full copy; no rebuild
/// debounce, so the first read after a write builds it) that answers the
/// server's reads by default.
fn stores() -> [TripleStore; 2] {
    [
        TripleStore::in_memory()
            .unwrap()
            .with_parallel_query(false, 1, usize::MAX),
        TripleStore::in_memory()
            .unwrap()
            .with_parallel_query(true, 4, usize::MAX)
            .with_parallel_rebuild_quiet_ms(0),
    ]
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
    for s in stores() {
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
}

// ═══════════════════════════════════════════════════════════
// Referential opacity (tt-03 / tt-15): quoted triples are distinct
// even when their components are owl:sameAs / numerically equal.
// ═══════════════════════════════════════════════════════════

#[test]
fn star_referential_opacity_distinct_terms() {
    for s in stores() {
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
}

// Opacity holds across datatypes: an integer and a value-equal decimal are
// distinct RDF terms, so they do not match inside a quoted triple.
#[test]
fn star_referential_opacity_cross_datatype() {
    for s in stores() {
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
}

// Documented oxigraph behavior: same-datatype xsd:integer lexical forms ARE
// canonicalized ("030" == "30"), so they DO match inside a quoted triple. Strict
// RDF-1.2 triple-term opacity would keep them distinct; oxigraph normalizes the
// integer lexical form per its RDF 1.1 term handling.
#[test]
fn star_integer_lexical_canonicalization_in_quoted_triple() {
    for s in stores() {
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
}

// ═══════════════════════════════════════════════════════════
// Triple-term accessor functions (tt-02 / tt-04): SUBJECT / PREDICATE
// / OBJECT / isTRIPLE / TRIPLE constructor.
// ═══════════════════════════════════════════════════════════

#[test]
fn star_accessor_functions() {
    for s in stores() {
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
}

#[test]
fn star_triple_constructor() {
    for s in stores() {
        let r = sel(
            &s,
            "SELECT ?t WHERE { BIND(TRIPLE(:a, :b, :c) AS ?t) FILTER(isTRIPLE(?t)) }",
        );
        assert_eq!(r.len(), 1, "TRIPLE() constructs a triple term");
    }
}

// ═══════════════════════════════════════════════════════════
// Nested quoted triples (tt-04, object/subject nesting that oxigraph allows)
// ═══════════════════════════════════════════════════════════

#[test]
fn star_nested_quoted_triple() {
    for s in stores() {
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
}

// ═══════════════════════════════════════════════════════════
// Aggregation over triple-term keys (tt-09, minus ORDER-BY determinism
// which SPARQL 1.2 leaves undefined for triple terms).
// ═══════════════════════════════════════════════════════════

#[test]
fn star_group_by_quoted_triple() {
    for s in stores() {
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
}

// ═══════════════════════════════════════════════════════════
// OPTIONAL / NOT EXISTS with a triple-term pattern parameterized by an
// outer variable (tt-08 / tt-10).
// ═══════════════════════════════════════════════════════════

#[test]
fn star_optional_quoted_pattern() {
    for s in stores() {
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
}

#[test]
fn star_not_exists_quoted_pattern() {
    for s in stores() {
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
}

// ═══════════════════════════════════════════════════════════
// Property path traversal that reaches quoted-triple metadata (tt-11)
// ═══════════════════════════════════════════════════════════

#[test]
fn star_property_path_over_chain_to_quoted() {
    for s in stores() {
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
}

// ═══════════════════════════════════════════════════════════
// CONSTRUCT with a quoted-triple template (tt-12)
// ═══════════════════════════════════════════════════════════

#[test]
fn star_construct_quoted_template() {
    for s in stores() {
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
}

// ═══════════════════════════════════════════════════════════
// Multi-tenant named-graph isolation for quoted triples (tt-14, security)
// ═══════════════════════════════════════════════════════════

#[test]
fn star_named_graph_isolation() {
    for s in stores() {
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
}

// ═══════════════════════════════════════════════════════════
// (Was a tracked gap.) The RDF 1.2 triple-term surface syntax `<<( )>>` with
// The RDF 1.2 triple-term syntax and `rdf:reifies` are supported. This test
// asserted the opposite while the engine was on oxigraph 0.4.
// ═══════════════════════════════════════════════════════════

#[test]
fn star_new_triple_term_syntax_is_supported() {
    for s in stores() {
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
}

// ═══════════════════════════════════════════════════════════
// Base direction survives the in-memory mirror
// ═══════════════════════════════════════════════════════════

/// A literal's RDF 1.2 base direction is part of the term: `DATATYPE` is
/// `rdf:dirLangString`, and `"hello"@en--ltr` is not `"hello"@en`. The same
/// answers must come back when the in-memory mirror (shards, columnar copy,
/// full copy) serves the read, as it does by default on the server.
#[test]
fn base_direction_survives_the_mirror() {
    let data = "INSERT DATA { :d1 :dlabel \"hello\"@en--ltr ; :label \"hello\"@en . \
                :d2 :dlabel \"مرحبا\"@ar--rtl ; :label \"مرحبا\"@ar . }";
    let engine = TripleStore::in_memory()
        .unwrap()
        .with_parallel_query(false, 1, usize::MAX);
    let mirror = TripleStore::in_memory()
        .unwrap()
        .with_parallel_query(true, 4, usize::MAX)
        .with_parallel_rebuild_quiet_ms(0);
    upd(&engine, data);
    upd(&mirror, data);

    let datatypes = "SELECT ?s (DATATYPE(?l) AS ?d) WHERE { ?s :dlabel ?l }";
    let mut want = sel(&engine, datatypes);
    want.sort();
    assert_eq!(
        want,
        vec![
            vec![
                "<http://ex/d1>".to_string(),
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString>".to_string()
            ],
            vec![
                "<http://ex/d2>".to_string(),
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString>".to_string()
            ],
        ]
    );
    for q in [
        datatypes,
        "SELECT ?s ?l WHERE { ?s :dlabel ?l }",
        "SELECT ?a ?b WHERE { ?a :dlabel ?x . ?b :label ?y FILTER(?x = ?y) }",
        "SELECT ?s WHERE { ?s :dlabel ?l FILTER(LANG(?l) = \"en\") }",
        "SELECT (COUNT(*) AS ?c) WHERE { ?s :dlabel ?l FILTER(sameTerm(?l, \"hello\"@en)) }",
    ] {
        let mut got = sel(&mirror, q);
        let mut expected = sel(&engine, q);
        got.sort();
        expected.sort();
        assert_eq!(
            got, expected,
            "the mirror diverged from the engine for: {q}"
        );
    }
    assert!(
        mirror.parallel_build_count() > 0,
        "the mirror must have been built, else this compared the engine with itself"
    );
}

// ═══════════════════════════════════════════════════════════
// SPARQL 1.2 (WD 2026-10-01) syntax and functions, and the SEP extensions
// oxigraph enables. The W3C suite (tests/w3c_sparql12_manifests.rs) covers
// the WD; these pin what the docs promise, on both read paths.
// ═══════════════════════════════════════════════════════════

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn typed(lexical: &str, local: &str) -> String {
    format!("\"{lexical}\"^^<{XSD}{local}>")
}

/// LATERAL (SEP-0006; not part of the 1.2 WD): the sub-select sees the outer
/// `?s`, so `LIMIT 1` applies per outer row. A plain sub-select is evaluated
/// once, so the same `LIMIT 1` keeps one row overall.
#[test]
fn lateral_applies_limit_per_outer_row() {
    for s in stores() {
        upd(
            &s,
            "INSERT DATA { :a a :T ; :score 3, 1, 2 . :b a :T ; :score 5, 4 . }",
        );
        let mut lateral = sel(
            &s,
            "SELECT ?s ?min WHERE { ?s a :T . \
               LATERAL { SELECT ?s ?min WHERE { ?s :score ?min } ORDER BY ?min LIMIT 1 } }",
        );
        lateral.sort();
        assert_eq!(
            lateral,
            vec![
                vec!["<http://ex/a>".to_string(), typed("1", "integer")],
                vec!["<http://ex/b>".to_string(), typed("4", "integer")],
            ]
        );
        let joined = sel(
            &s,
            "SELECT ?s ?min WHERE { ?s a :T . \
               { SELECT ?s ?min WHERE { ?s :score ?min } ORDER BY ?min LIMIT 1 } }",
        );
        assert_eq!(joined.len(), 1, "without LATERAL the LIMIT is global");
    }
}

/// ADJUST (SEP-0002; not part of the 1.2 WD) takes the new timezone as an
/// `xsd:dayTimeDuration`; any other second argument gives unbound.
#[test]
fn adjust_takes_a_day_time_duration() {
    for s in stores() {
        let r = sel(
            &s,
            r#"SELECT ?west ?bad WHERE {
                 BIND(ADJUST("2002-03-07T10:00:00-05:00"^^xsd:dateTime, "-PT10H"^^xsd:dayTimeDuration) AS ?west)
                 BIND(ADJUST("2002-03-07T10:00:00-05:00"^^xsd:dateTime, "+05:00"^^xsd:string) AS ?bad)
               }"#,
        );
        assert_eq!(
            r,
            vec![vec![
                typed("2002-03-07T05:00:00-10:00", "dateTime"),
                String::new()
            ]]
        );
    }
}

/// `VERSION "1.2"` is accepted in the prologue of a query and of an update.
#[test]
fn version_declaration_is_accepted() {
    for s in stores() {
        upd(&s, "VERSION \"1.2\"\nINSERT DATA { :a :p :o }");
        let r = sel(&s, "VERSION \"1.2\"\nSELECT ?o WHERE { :a :p ?o }");
        assert_eq!(r, vec![vec!["<http://ex/o>".to_string()]]);
    }
}

/// LANGDIR, hasLANG, hasLANGDIR and STRLANGDIR, and base direction as part of
/// the term: `"x"@en--ltr` is neither `"x"@en` nor `"x"@en--rtl`.
#[test]
fn langdir_function_family() {
    for s in stores() {
        let r = sel(
            &s,
            r#"SELECT ?dir ?nodir ?lang ?h1 ?h2 ?h3 ?h4 ?built ?dt ?same WHERE {
                 BIND(LANGDIR("abc"@en--rtl) AS ?dir)
                 BIND(LANGDIR("abc"@en) AS ?nodir)
                 BIND(LANG("abc"@en--rtl) AS ?lang)
                 BIND(hasLANG("abc"@en) AS ?h1)
                 BIND(hasLANG("abc") AS ?h2)
                 BIND(hasLANGDIR("abc"@en--ltr) AS ?h3)
                 BIND(hasLANGDIR("abc"@en) AS ?h4)
                 BIND(STRLANGDIR("abc", "en", "ltr") AS ?built)
                 BIND(DATATYPE("abc"@en--ltr) AS ?dt)
                 BIND(sameTerm("abc"@en--ltr, "abc"@en) AS ?same)
               }"#,
        );
        let t = typed("true", "boolean");
        let f = typed("false", "boolean");
        assert_eq!(
            r,
            vec![vec![
                "\"rtl\"".to_string(),
                "\"\"".to_string(),
                "\"en\"".to_string(),
                t.clone(),
                f.clone(),
                t,
                f.clone(),
                "\"abc\"@en--ltr".to_string(),
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString>".to_string(),
                f,
            ]]
        );
    }
}

/// `~ reifier` and `{| … |}` annotations in INSERT DATA: the base triple is
/// asserted, the reifier points at its triple term and carries the
/// annotations; without `~` the annotation block mints a blank reifier.
#[test]
fn reifier_and_annotation_syntax_in_insert_data() {
    for s in stores() {
        upd(
            &s,
            "INSERT DATA { :alice :knows :bob ~ :r1 {| :since 2020 |} . \
                           :carol :knows :dave {| :since 2021 |} . }",
        );
        assert!(ask(&s, "ASK { :alice :knows :bob . :carol :knows :dave }"));
        assert!(ask(
            &s,
            "ASK { :r1 rdf:reifies <<( :alice :knows :bob )>> ; :since 2020 }"
        ));
        let blank = sel(
            &s,
            "SELECT ?y (isBLANK(?r) AS ?b) WHERE { ?r rdf:reifies <<( :carol :knows :dave )>> ; :since ?y }",
        );
        assert_eq!(
            blank,
            vec![vec![typed("2021", "integer"), typed("true", "boolean")]]
        );
    }
}

/// The same forms in a query pattern: `~ ?r` binds the reifier, `{| |}`
/// matches its annotations, and `<< s p o ~ id >>` names it.
#[test]
fn reifier_and_annotation_syntax_in_queries() {
    for s in stores() {
        upd(
            &s,
            "INSERT DATA { :alice :knows :bob ~ :r1 {| :since 2020 |} . :alice :knows :carol . }",
        );
        let r = sel(
            &s,
            "SELECT ?r ?y WHERE { :alice :knows :bob ~ ?r {| :since ?y |} }",
        );
        assert_eq!(
            r,
            vec![vec!["<http://ex/r1>".to_string(), typed("2020", "integer")]]
        );
        let named = sel(
            &s,
            "SELECT ?y WHERE { << :alice :knows :bob ~ :r1 >> :since ?y }",
        );
        assert_eq!(named, vec![vec![typed("2020", "integer")]]);
        // :alice :knows :carol has no reifier, so the annotation pattern drops it.
        let annotated = sel(&s, "SELECT ?o WHERE { :alice :knows ?o {| :since ?y |} }");
        assert_eq!(annotated, vec![vec!["<http://ex/bob>".to_string()]]);
    }
}

/// SPARQL 1.2 forbids a variable twice in one VALUES block.
#[test]
fn duplicate_values_variables_are_rejected() {
    let s = &stores()[0];
    for q in [
        "SELECT * WHERE { VALUES (?x ?x) { (1 2) } }",
        "SELECT * WHERE { ?s ?p ?o } VALUES (?o ?o) { (1 2) }",
    ] {
        assert!(
            s.query(&format!("{PFX}{q}")).is_err(),
            "duplicate VALUES variables must not parse: {q}"
        );
    }
    assert!(s
        .query(&format!(
            "{PFX}SELECT * WHERE {{ VALUES (?x ?y) {{ (1 2) }} }}"
        ))
        .is_ok());
}

/// RDF 1.2 term equality for literals with a base direction: `=` between two
/// of them is true exactly when lexical form, language tag and direction all
/// match. spareval 0.2.7 (oxigraph 0.5.11) hit an `unreachable!()` here — its
/// `ExpressionTerm` equality had no arm for directional strings — so the
/// query panicked (a 500 over HTTP); the vendored copy carries the fix
/// (`vendor/spareval/UPSTREAM-PR-dir-lang-string-equality.md`).
#[test]
fn directional_literal_equality() {
    for s in stores() {
        let r = sel(
            &s,
            r#"SELECT ?same ?dir ?nodir ?lang ?ne ?in WHERE {
                 BIND("abc"@en--ltr = "abc"@en--ltr AS ?same)
                 BIND("abc"@en--ltr = "abc"@en--rtl AS ?dir)
                 BIND("abc"@en--ltr = "abc"@en AS ?nodir)
                 BIND("abc"@en--ltr = "abc"@fr--ltr AS ?lang)
                 BIND("abc"@en--ltr != "abc"@en--rtl AS ?ne)
                 BIND("abc"@en--rtl IN ("abc"@en--ltr, "abc"@en--rtl) AS ?in)
               }"#,
        );
        let t = typed("true", "boolean");
        let f = typed("false", "boolean");
        assert_eq!(
            r,
            vec![vec![t.clone(), f.clone(), f.clone(), f, t.clone(), t]]
        );
        upd(
            &s,
            r#"INSERT DATA { :a :label "x"@en--ltr . :b :label "x"@en--ltr . :c :label "x"@en--rtl . }"#,
        );
        let mut joined = sel(
            &s,
            r#"SELECT ?s WHERE { ?s :label ?l FILTER(?l = "x"@en--ltr) }"#,
        );
        joined.sort();
        assert_eq!(
            joined,
            vec![
                vec!["<http://ex/a>".to_string()],
                vec!["<http://ex/b>".to_string()]
            ]
        );
    }
}

/// SPARQL 1.2 (and the W3C sparql12 test `nested-aggregate-functions`): an
/// aggregate cannot appear inside another aggregate's argument. The vendored
/// spargebra refuses it at parse time; aggregates side by side, in a HAVING, or
/// over a sub-select's aggregate stay valid.
#[test]
fn nested_aggregates_are_a_syntax_error() {
    for s in stores() {
        upd(&s, "INSERT DATA { :a :p 1 . :b :p 2 }");
        for q in [
            "SELECT (SUM(COUNT(?x)) AS ?s) WHERE { ?x :p ?o }",
            "SELECT (MAX(1 + AVG(?o)) AS ?m) WHERE { ?x :p ?o }",
            "SELECT ?p WHERE { ?x ?p ?o } GROUP BY ?p HAVING (SUM(COUNT(?x)) > 1)",
        ] {
            assert!(
                s.query(&format!("{PFX}{q}")).is_err(),
                "must not parse: {q}"
            );
        }
        assert_eq!(
            sel(
                &s,
                "SELECT (SUM(?o) / COUNT(?o) AS ?avg) WHERE { ?x :p ?o }"
            )
            .len(),
            1
        );
        assert_eq!(
            sel(
                &s,
                "SELECT (SUM(?c) AS ?n) WHERE { { SELECT (COUNT(?x) AS ?c) WHERE { ?x :p ?o } GROUP BY ?o } }"
            ),
            vec![vec!["\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>".to_string()]]
        );
    }
}

/// SPARQL 1.2 grammar: `ExprTripleTermSubject ::= iri | Var` (W3C sparql12
/// tests `tripleterm-subject-03` and `-06`). spargebra 0.4.7 accepted a literal
/// or a triple term there; the vendored copy refuses both. Literal and
/// triple-term objects, and a variable subject, stay valid.
#[test]
fn triple_term_expression_subject_is_iri_or_var() {
    for s in stores() {
        for q in [
            r#"SELECT * WHERE { BIND(<<( "literal" :q :z )>> AS ?X) }"#,
            "SELECT * WHERE { BIND(<<( 42 :q :z )>> AS ?X) }",
            "SELECT * WHERE { BIND(<<( <<( :s :p :o )>> :q :z )>> AS ?X) }",
        ] {
            assert!(
                s.query(&format!("{PFX}{q}")).is_err(),
                "must not parse: {q}"
            );
        }
        let r = sel(
            &s,
            r#"SELECT ?X ?Y WHERE {
                 VALUES ?s { :a }
                 BIND(<<( :s :p "o" )>> AS ?X)
                 BIND(<<( ?s :p <<( :b :c :d )>> )>> AS ?Y)
               }"#,
        );
        assert_eq!(
            r,
            vec![vec![
                "<<( <http://ex/s> <http://ex/p> \"o\" )>>".to_string(),
                "<<( <http://ex/a> <http://ex/p> <<( <http://ex/b> <http://ex/c> <http://ex/d> )>> )>>"
                    .to_string(),
            ]]
        );
    }
}

/// SPARQL 1.2 (w3c/sparql-query PR #380; W3C sparql12 test
/// `select-variable-reuse`): in an aggregating query a SELECT expression may
/// use the variable an earlier SELECT expression binds. spargebra 0.4.7
/// refused it; the vendored copy accepts it. A variable used before it is
/// bound, or neither grouped nor bound by the SELECT, is still refused.
#[test]
fn select_expression_reuses_an_earlier_select_variable() {
    for s in stores() {
        let r = sel(
            &s,
            "SELECT (COUNT(?v) AS ?c) (?c * 2 AS ?d) (?d + ?c AS ?e) WHERE { VALUES ?v { 0 1 2 3 } }",
        );
        assert_eq!(
            r,
            vec![vec![
                typed("4", "integer"),
                typed("8", "integer"),
                typed("12", "integer")
            ]]
        );
        let grouped = sel(
            &s,
            "SELECT ?k (SUM(?v) AS ?t) (?t + 1 AS ?u) WHERE { VALUES (?k ?v) { (1 1) (1 2) } } GROUP BY ?k",
        );
        assert_eq!(
            grouped,
            vec![vec![
                typed("1", "integer"),
                typed("3", "integer"),
                typed("4", "integer")
            ]]
        );
        for q in [
            "SELECT (?x + 1 AS ?y) (COUNT(?v) AS ?x) WHERE { VALUES ?v { 0 1 } }",
            "SELECT (COUNT(?v) AS ?c) (?w AS ?d) WHERE { VALUES (?v ?w) { (0 1) } }",
            "SELECT (COUNT(?v) AS ?c) (?c + 1 AS ?c) WHERE { VALUES ?v { 0 1 } }",
        ] {
            assert!(
                s.query(&format!("{PFX}{q}")).is_err(),
                "must not parse: {q}"
            );
        }
    }
}
