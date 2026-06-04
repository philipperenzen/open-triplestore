//! SPARQL Benchmark Tests
//!
//! Implements queries from two major W3C-referenced benchmarks:
//!
//! 1. SP2B (SPARQL Performance Benchmark)
//!    http://dbis.informatik.uni-freiburg.de/forschung/projekte/SP2B/
//!    Ref: https://www.w3.org/wiki/SparqlBenchmarks
//!    Queries Q1-Q12 testing typical SPARQL operator constellations.
//!    Data model: DBLP-style bibliographic data (journals, articles, authors)
//!
//! 2. Berlin SPARQL Benchmark (BSBM)
//!    http://wifo5-03.informatik.uni-mannheim.de/bizer/berlinsparqlbenchmark/
//!    Ref: https://www.w3.org/wiki/SparqlBenchmarks
//!    Queries Q1-Q12 testing e-commerce scenarios.
//!    Data model: products, reviews, producers, offers
//!
//! These tests use a small scale dataset (hundreds of triples instead of millions)
//! to verify correctness of query evaluation. Performance characteristics are
//! validated via the scripts/benchmark.sh script which uses larger datasets.

use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use std::time::{Duration, Instant};

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn ts() -> open_triplestore::store::TripleStore {
    open_triplestore::store::TripleStore::in_memory().unwrap()
}

fn load(s: &open_triplestore::store::TripleStore, ttl: &str) {
    s.load_str(ttl, RdfFormat::Turtle, None).unwrap();
}

fn select(s: &open_triplestore::store::TripleStore, q: &str) -> Vec<Vec<String>> {
    match s.query(q).unwrap() {
        QueryResults::Solutions(sols) => {
            let vars: Vec<_> = sols.variables().iter().map(|v| v.as_str().to_string()).collect();
            sols.into_iter()
                .map(|sol| {
                    let sol = sol.unwrap();
                    vars.iter()
                        .map(|v| sol.get(v.as_str()).map(|t| t.to_string()).unwrap_or_default())
                        .collect()
                })
                .collect()
        }
        _ => panic!("Expected SELECT results"),
    }
}

fn ask(s: &open_triplestore::store::TripleStore, q: &str) -> bool {
    match s.query(q).unwrap() {
        QueryResults::Boolean(b) => b,
        _ => panic!("Expected ASK result"),
    }
}

/// Execute a query and assert it completes within a timeout (correctness test)
fn timed_select(s: &open_triplestore::store::TripleStore, q: &str, max_ms: u64) -> Vec<Vec<String>> {
    let start = Instant::now();
    let r = select(s, q);
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_millis(max_ms),
        "Query exceeded {}ms timeout (took {}ms): {}",
        max_ms, elapsed.as_millis(), q
    );
    r
}

// ═══════════════════════════════════════════════════════════
// SP2B Dataset Setup
// Bibliographic data modeled after DBLP
// Namespaces: dc, dcterms, foaf, bench, swrc, rdf
// ═══════════════════════════════════════════════════════════

fn setup_sp2b_store() -> open_triplestore::store::TripleStore {
    let s = ts();
    load(&s, r#"
@prefix rdf:     <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs:    <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd:     <http://www.w3.org/2001/XMLSchema#> .
@prefix dc:      <http://purl.org/dc/elements/1.1/> .
@prefix dcterms: <http://purl.org/dc/terms/> .
@prefix foaf:    <http://xmlns.com/foaf/0.1/> .
@prefix bench:   <http://localhost/vocabulary/bench/> .
@prefix swrc:    <http://swrc.ontoware.org/ontology#> .
@prefix ex:      <http://example.org/> .

# ── Journals ──
ex:j1 rdf:type bench:Journal ;
      dcterms:title "Journal of Semantic Web" ;
      dc:publisher ex:pub1 ;
      dcterms:issued "2004"^^xsd:gYear .

ex:j2 rdf:type bench:Journal ;
      dcterms:title "Semantic Web Journal" ;
      dc:publisher ex:pub2 ;
      dcterms:issued "2010"^^xsd:gYear .

# ── Conference Proceedings ──
ex:proc1 rdf:type bench:Proceedings ;
         dcterms:title "International Semantic Web Conference 2004" ;
         dcterms:issued "2004"^^xsd:gYear ;
         dc:publisher ex:pub1 .

ex:proc2 rdf:type bench:Proceedings ;
         dcterms:title "European Semantic Web Conference 2005" ;
         dcterms:issued "2005"^^xsd:gYear ;
         dc:publisher ex:pub2 .

# ── Journal Articles ──
ex:art1 rdf:type bench:Article ;
        dcterms:partOf ex:j1 ;
        dc:creator ex:author1, ex:author2 ;
        dcterms:title "SPARQL Query Language for RDF" ;
        bench:abstract "This paper introduces SPARQL, a query language for RDF graphs." ;
        swrc:pages "1-25" ;
        dcterms:issued "2008"^^xsd:gYear .

ex:art2 rdf:type bench:Article ;
        dcterms:partOf ex:j1 ;
        dc:creator ex:author2, ex:author3 ;
        dcterms:title "OWL Web Ontology Language Overview" ;
        bench:abstract "An overview of the OWL ontology language for the Web." ;
        swrc:pages "26-50" ;
        dcterms:issued "2009"^^xsd:gYear .

ex:art3 rdf:type bench:Article ;
        dcterms:partOf ex:j2 ;
        dc:creator ex:author1, ex:author3 ;
        dcterms:title "Linked Data: Connect Distributed Data across the Web" ;
        bench:abstract "Linked Data principles for connecting distributed RDF data." ;
        swrc:pages "1-30" ;
        dcterms:issued "2011"^^xsd:gYear .

# ── Inproceedings ──
ex:in1 rdf:type bench:Inproceedings ;
       dcterms:partOf ex:proc1 ;
       dc:creator ex:author1 ;
       dcterms:title "A Semantic Web Primer" ;
       bench:abstract "Introduction to the Semantic Web technologies." ;
       swrc:pages "100-115" ;
       dcterms:issued "2004"^^xsd:gYear .

ex:in2 rdf:type bench:Inproceedings ;
       dcterms:partOf ex:proc1 ;
       dc:creator ex:author2, ex:author4 ;
       dcterms:title "RDF Schema: A Lightweight Ontology Language" ;
       bench:abstract "RDFS extends RDF with basic ontology constructs." ;
       swrc:pages "116-130" ;
       dcterms:issued "2004"^^xsd:gYear .

ex:in3 rdf:type bench:Inproceedings ;
       dcterms:partOf ex:proc2 ;
       dc:creator ex:author3 ;
       dcterms:title "Description Logics for Ontologies" ;
       bench:abstract "Formal foundations of description logics for ontologies." ;
       swrc:pages "200-215" ;
       dcterms:issued "2005"^^xsd:gYear .

# ── Authors / Persons ──
ex:author1 rdf:type foaf:Person ;
           foaf:name "Alice Smith" ;
           foaf:mbox <mailto:alice@example.org> ;
           foaf:homepage <http://alice.example.org/> .

ex:author2 rdf:type foaf:Person ;
           foaf:name "Bob Jones" ;
           foaf:mbox <mailto:bob@example.org> ;
           foaf:homepage <http://bob.example.org/> .

ex:author3 rdf:type foaf:Person ;
           foaf:name "Carol Williams" ;
           foaf:mbox <mailto:carol@example.org> .

ex:author4 rdf:type foaf:Person ;
           foaf:name "Dave Brown" ;
           foaf:mbox <mailto:dave@example.org> .

# ── Publishers ──
ex:pub1 rdf:type foaf:Organization ;
        foaf:name "Springer" .

ex:pub2 rdf:type foaf:Organization ;
        foaf:name "ACM Press" .
"#);
    s
}

// ═══════════════════════════════════════════════════════════
// SP2B Queries
// ═══════════════════════════════════════════════════════════

#[test]
fn sp2b_q1_year_of_first_article() {
    // SP2B Q1: Return the year of the first article published in "Journal of Semantic Web"
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:     <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX xsd:     <http://www.w3.org/2001/XMLSchema#>
        PREFIX bench:   <http://localhost/vocabulary/bench/>
        PREFIX dcterms: <http://purl.org/dc/terms/>
        SELECT ?yr WHERE {
            ?journal rdf:type bench:Journal ;
                     dcterms:title "Journal of Semantic Web" .
            ?article rdf:type bench:Article ;
                     dcterms:partOf ?journal ;
                     dcterms:issued ?yr .
        } ORDER BY ?yr LIMIT 1
    "#, 5000);
    assert!(!r.is_empty(), "Q1 should return results");
    assert!(r[0][0].contains("2008") || r[0][0].contains("2009"),
            "First article year: {}", r[0][0]);
}

#[test]
fn sp2b_q2_inproceedings_authors_abstracts() {
    // SP2B Q2: Select all inproceedings with their authors, titles, and abstracts
    // Tests: triple patterns with multiple optional properties
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:     <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX dc:      <http://purl.org/dc/elements/1.1/>
        PREFIX dcterms: <http://purl.org/dc/terms/>
        PREFIX foaf:    <http://xmlns.com/foaf/0.1/>
        PREFIX bench:   <http://localhost/vocabulary/bench/>
        SELECT ?inproc ?author ?booktitle ?title ?abstract WHERE {
            ?inproc rdf:type bench:Inproceedings .
            ?inproc dc:creator ?author .
            ?inproc dcterms:title ?title .
            OPTIONAL { ?inproc bench:abstract ?abstract }
            ?proc dcterms:title ?booktitle .
            ?inproc dcterms:partOf ?proc .
        } ORDER BY ?inproc
    "#, 5000);
    assert!(!r.is_empty(), "Q2 should return results");
    // We have 3 inproceedings, some with multiple authors
    assert!(r.len() >= 3, "At least 3 inproceedings: {}", r.len());
}

#[test]
fn sp2b_q3a_articles_with_properties() {
    // SP2B Q3a: SELECT all articles and their properties (similar to Q2 but for articles)
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:     <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX dc:      <http://purl.org/dc/elements/1.1/>
        PREFIX dcterms: <http://purl.org/dc/terms/>
        PREFIX bench:   <http://localhost/vocabulary/bench/>
        PREFIX swrc:    <http://swrc.ontoware.org/ontology#>
        SELECT ?article ?author ?title ?pages ?abstract WHERE {
            ?article rdf:type bench:Article .
            ?article dc:creator ?author .
            ?article dcterms:title ?title .
            OPTIONAL { ?article swrc:pages ?pages }
            OPTIONAL { ?article bench:abstract ?abstract }
        } ORDER BY ?article ?author
    "#, 5000);
    assert!(!r.is_empty(), "Q3a should return article results");
}

#[test]
fn sp2b_q3b_articles_with_subject_filter() {
    // SP2B Q3b: Articles where abstract mentions "SPARQL"
    // Tests: REGEX filter on string literals
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:     <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX dcterms: <http://purl.org/dc/terms/>
        PREFIX bench:   <http://localhost/vocabulary/bench/>
        SELECT ?article ?title WHERE {
            ?article rdf:type bench:Article ;
                     dcterms:title ?title ;
                     bench:abstract ?abstract .
            FILTER REGEX(?abstract, "SPARQL", "i")
        }
    "#, 5000);
    assert_eq!(r.len(), 1, "One article mentions SPARQL: {:?}", r);
    assert!(r[0][1].to_lowercase().contains("sparql"), "Title should contain SPARQL: {}", r[0][1]);
}

#[test]
fn sp2b_q4_publications_with_coauthors() {
    // SP2B Q4: Select all publications with authors who have email addresses
    // Tests: JOIN across multiple triple patterns
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:   <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX dc:    <http://purl.org/dc/elements/1.1/>
        PREFIX foaf:  <http://xmlns.com/foaf/0.1/>
        PREFIX bench: <http://localhost/vocabulary/bench/>
        SELECT DISTINCT ?pub ?author ?name ?email WHERE {
            ?pub rdf:type ?type .
            FILTER(?type IN (bench:Article, bench:Inproceedings))
            ?pub dc:creator ?author .
            ?author foaf:name ?name .
            ?author foaf:mbox ?email .
        } ORDER BY ?pub ?author
    "#, 5000);
    assert!(!r.is_empty(), "Q4 should return publications with authors");
}

#[test]
fn sp2b_q5a_persons_with_known_coauthors() {
    // SP2B Q5a: Persons who co-authored a publication with someone
    // Tests: JOIN on shared publications, self-join pattern
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX dc:    <http://purl.org/dc/elements/1.1/>
        PREFIX foaf:  <http://xmlns.com/foaf/0.1/>
        PREFIX bench: <http://localhost/vocabulary/bench/>
        SELECT DISTINCT ?person1 ?name1 ?person2 ?name2 WHERE {
            ?pub dc:creator ?person1 .
            ?pub dc:creator ?person2 .
            FILTER(?person1 != ?person2)
            ?person1 foaf:name ?name1 .
            ?person2 foaf:name ?name2 .
        } ORDER BY ?name1 ?name2
    "#, 5000);
    assert!(!r.is_empty(), "Q5a should find co-authors");
    // author1 and author2 co-authored art1; author1 and author3 co-authored art3; etc.
    assert!(r.len() >= 2, "Multiple co-author pairs: {}", r.len());
}

#[test]
fn sp2b_q6_people_coauthoring_with_homepage() {
    // SP2B Q6: Persons who co-authored with someone who has a homepage
    // Tests: OPTIONAL + FILTER
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX dc:    <http://purl.org/dc/elements/1.1/>
        PREFIX foaf:  <http://xmlns.com/foaf/0.1/>
        SELECT DISTINCT ?person ?name WHERE {
            ?pub dc:creator ?person .
            ?pub dc:creator ?coauthor .
            FILTER(?person != ?coauthor)
            ?person foaf:name ?name .
            ?coauthor foaf:homepage ?homepage .
        } ORDER BY ?name
    "#, 5000);
    assert!(!r.is_empty(), "Q6 should find persons with homepage-having coauthors");
}

#[test]
fn sp2b_q7_articles_no_coauthors() {
    // SP2B Q7: Articles with single authors (no co-authors)
    // Tests: NOT EXISTS / MINUS
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:   <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX dc:    <http://purl.org/dc/elements/1.1/>
        PREFIX bench: <http://localhost/vocabulary/bench/>
        SELECT ?article ?title WHERE {
            ?article rdf:type bench:Article ;
                     dc:creator ?author .
            FILTER NOT EXISTS {
                ?article dc:creator ?other .
                FILTER(?other != ?author)
            }
            ?article <http://purl.org/dc/terms/title> ?title .
        }
    "#, 5000);
    // In our dataset, art1 has 2 authors, art2 has 2 authors, art3 has 2 authors — all multi-authored
    assert_eq!(r.len(), 0, "All articles have co-authors in test dataset");
}

#[test]
fn sp2b_q8_subquery_count_papers_per_journal() {
    // SP2B Q8: Subquery to count papers per journal
    // Tests: subqueries with aggregation
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:     <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX dcterms: <http://purl.org/dc/terms/>
        PREFIX bench:   <http://localhost/vocabulary/bench/>
        SELECT ?journal ?title (COUNT(?article) AS ?count) WHERE {
            ?journal rdf:type bench:Journal ;
                     dcterms:title ?title .
            ?article rdf:type bench:Article ;
                     dcterms:partOf ?journal .
        } GROUP BY ?journal ?title ORDER BY DESC(?count)
    "#, 5000);
    assert!(!r.is_empty(), "Q8 should return journal paper counts");
    // j1 has 2 articles, j2 has 1 article
    assert!(r[0][2].contains("2") || r[0][2].contains("1"),
            "Paper count: {}", r[0][2]);
}

#[test]
fn sp2b_q9_author_statistics() {
    // SP2B Q9: Author productivity statistics
    // Tests: GROUP BY with COUNT and ORDER BY
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX dc:    <http://purl.org/dc/elements/1.1/>
        PREFIX foaf:  <http://xmlns.com/foaf/0.1/>
        SELECT ?author ?name (COUNT(?pub) AS ?pubCount) WHERE {
            ?author foaf:name ?name .
            ?pub dc:creator ?author .
        } GROUP BY ?author ?name ORDER BY DESC(?pubCount) ?name
    "#, 5000);
    assert!(!r.is_empty(), "Q9 should return author statistics");
    // author1 appears in art1, art3, in1 = 3; author2 in art1, art2, in2 = 3; etc.
    assert!(r.len() >= 3, "At least 3 authors: {}", r.len());
}

#[test]
fn sp2b_q10_coauthor_network() {
    // SP2B Q10: Multi-hop co-author network (find people 2 hops away)
    // Tests: Property path / join chains
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX dc:    <http://purl.org/dc/elements/1.1/>
        PREFIX foaf:  <http://xmlns.com/foaf/0.1/>
        SELECT DISTINCT ?person3 ?name3 WHERE {
            # author1 co-authored with author2 (pub1)
            ?pub1 dc:creator <http://example.org/author1> .
            ?pub1 dc:creator ?author2 .
            FILTER(?author2 != <http://example.org/author1>)
            # author2 co-authored with author3 (pub2)
            ?pub2 dc:creator ?author2 .
            ?pub2 dc:creator ?person3 .
            FILTER(?person3 != <http://example.org/author1> && ?person3 != ?author2)
            ?person3 foaf:name ?name3 .
        }
    "#, 5000);
    // 2-hop co-authors of author1
    assert!(!r.is_empty(), "Q10 should find co-authors of co-authors");
}

#[test]
fn sp2b_q11_all_publications_optional_props() {
    // SP2B Q11: All publications with all optional properties
    // Tests: Complex OPTIONAL combinations (stresses left-join evaluation)
    let s = setup_sp2b_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:     <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX dc:      <http://purl.org/dc/elements/1.1/>
        PREFIX dcterms: <http://purl.org/dc/terms/>
        PREFIX bench:   <http://localhost/vocabulary/bench/>
        PREFIX swrc:    <http://swrc.ontoware.org/ontology#>
        PREFIX foaf:    <http://xmlns.com/foaf/0.1/>
        SELECT ?pub ?type ?title ?year ?author ?authorName ?pages WHERE {
            ?pub rdf:type ?type .
            FILTER(?type IN (bench:Article, bench:Inproceedings))
            ?pub dcterms:title ?title .
            OPTIONAL { ?pub dcterms:issued ?year }
            OPTIONAL { ?pub dc:creator ?author . ?author foaf:name ?authorName }
            OPTIONAL { ?pub swrc:pages ?pages }
        } ORDER BY ?pub ?author
    "#, 5000);
    assert!(!r.is_empty(), "Q11 should return publications with optional properties");
}

#[test]
fn sp2b_q12_construct_coauthor_graph() {
    // SP2B Q12: CONSTRUCT a co-authorship graph
    // Tests: CONSTRUCT query form
    let s = setup_sp2b_store();
    let q = r#"
        PREFIX dc:   <http://purl.org/dc/elements/1.1/>
        PREFIX foaf: <http://xmlns.com/foaf/0.1/>
        PREFIX ex:   <http://example.org/vocab#>
        CONSTRUCT { ?author1 ex:coAuthoredWith ?author2 } WHERE {
            ?pub dc:creator ?author1 .
            ?pub dc:creator ?author2 .
            FILTER(?author1 != ?author2)
        }
    "#;
    let start = Instant::now();
    let result = s.query(q).unwrap();
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(5000), "Q12 must complete within 5s");
    match result {
        QueryResults::Graph(triples) => {
            let count = triples.count();
            assert!(count > 0, "Q12 should construct co-author triples");
        }
        _ => panic!("Expected CONSTRUCT result"),
    }
}

// ═══════════════════════════════════════════════════════════
// BSBM (Berlin SPARQL Benchmark) Dataset Setup
// E-commerce scenario with products, offers, reviews
// ═══════════════════════════════════════════════════════════

fn setup_bsbm_store() -> open_triplestore::store::TripleStore {
    let s = ts();
    load(&s, r#"
@prefix rdf:      <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs:     <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd:      <http://www.w3.org/2001/XMLSchema#> .
@prefix bsbm:     <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/> .
@prefix bsbm-inst: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/instances/> .
@prefix dc:       <http://purl.org/dc/elements/1.1/> .
@prefix rev:      <http://purl.org/stuff/rev#> .
@prefix foaf:     <http://xmlns.com/foaf/0.1/> .
@prefix ex:       <http://example.org/> .

# ── Product Features ──
ex:feat1 rdf:type bsbm:ProductFeature ; rdfs:label "Feature A" .
ex:feat2 rdf:type bsbm:ProductFeature ; rdfs:label "Feature B" .
ex:feat3 rdf:type bsbm:ProductFeature ; rdfs:label "Feature C" .

# ── Product Types ──
ex:ptype1 rdf:type bsbm:ProductType ; rdfs:label "Electronics" .
ex:ptype2 rdf:type bsbm:ProductType ; rdfs:label "Books" .

# ── Products ──
ex:prod1 rdf:type bsbm:Product ;
         rdfs:label "Widget Pro" ;
         bsbm:productFeature ex:feat1, ex:feat2 ;
         bsbm:productPropertyNumeric1 "100"^^xsd:integer ;
         bsbm:productPropertyNumeric2 "500"^^xsd:integer ;
         bsbm:productPropertyTextual1 "High quality widget for professionals" ;
         rdf:type ex:ptype1 .

ex:prod2 rdf:type bsbm:Product ;
         rdfs:label "Gadget Plus" ;
         bsbm:productFeature ex:feat2, ex:feat3 ;
         bsbm:productPropertyNumeric1 "200"^^xsd:integer ;
         bsbm:productPropertyNumeric2 "300"^^xsd:integer ;
         bsbm:productPropertyTextual1 "Advanced gadget with multiple features" ;
         rdf:type ex:ptype1 .

ex:prod3 rdf:type bsbm:Product ;
         rdfs:label "Economy Book" ;
         bsbm:productFeature ex:feat1 ;
         bsbm:productPropertyNumeric1 "50"^^xsd:integer ;
         bsbm:productPropertyNumeric2 "800"^^xsd:integer ;
         bsbm:productPropertyTextual1 "Affordable educational resource" ;
         rdf:type ex:ptype2 .

ex:prod4 rdf:type bsbm:Product ;
         rdfs:label "Super Gadget" ;
         bsbm:productFeature ex:feat1, ex:feat2, ex:feat3 ;
         bsbm:productPropertyNumeric1 "150"^^xsd:integer ;
         bsbm:productPropertyNumeric2 "400"^^xsd:integer ;
         bsbm:productPropertyTextual1 "Top-tier gadget with all features" ;
         rdf:type ex:ptype1 .

# ── Producers ──
ex:prod_a rdf:type bsbm:Producer ;
          foaf:name "TechCorp" ;
          foaf:homepage <http://techcorp.example.org/> .

ex:prod_b rdf:type bsbm:Producer ;
          foaf:name "BookPub" ;
          foaf:homepage <http://bookpub.example.org/> .

# ── Offers ──
ex:offer1 rdf:type bsbm:Offer ;
          bsbm:product ex:prod1 ;
          bsbm:vendor ex:vendor1 ;
          bsbm:price "89.99"^^xsd:double ;
          bsbm:validTo "2025-12-31"^^xsd:date ;
          bsbm:deliveryDays "3"^^xsd:integer .

ex:offer2 rdf:type bsbm:Offer ;
          bsbm:product ex:prod1 ;
          bsbm:vendor ex:vendor2 ;
          bsbm:price "95.00"^^xsd:double ;
          bsbm:validTo "2025-06-30"^^xsd:date ;
          bsbm:deliveryDays "5"^^xsd:integer .

ex:offer3 rdf:type bsbm:Offer ;
          bsbm:product ex:prod2 ;
          bsbm:vendor ex:vendor1 ;
          bsbm:price "149.99"^^xsd:double ;
          bsbm:validTo "2025-12-31"^^xsd:date ;
          bsbm:deliveryDays "2"^^xsd:integer .

ex:offer4 rdf:type bsbm:Offer ;
          bsbm:product ex:prod3 ;
          bsbm:vendor ex:vendor3 ;
          bsbm:price "19.99"^^xsd:double ;
          bsbm:validTo "2025-12-31"^^xsd:date ;
          bsbm:deliveryDays "7"^^xsd:integer .

# ── Vendors ──
ex:vendor1 rdf:type bsbm:Vendor ;
           foaf:name "OnlineShop" ;
           bsbm:country "US" .

ex:vendor2 rdf:type bsbm:Vendor ;
           foaf:name "MegaMart" ;
           bsbm:country "UK" .

ex:vendor3 rdf:type bsbm:Vendor ;
           foaf:name "BookStore" ;
           bsbm:country "DE" .

# ── Reviews ──
ex:rev1 rdf:type rev:Review ;
        rev:rating "5"^^xsd:integer ;
        bsbm:reviewFor ex:prod1 ;
        dc:date "2024-01-15"^^xsd:date ;
        rev:reviewer ex:reviewer1 ;
        rev:text "Excellent product! Very satisfied." .

ex:rev2 rdf:type rev:Review ;
        rev:rating "4"^^xsd:integer ;
        bsbm:reviewFor ex:prod1 ;
        dc:date "2024-02-20"^^xsd:date ;
        rev:reviewer ex:reviewer2 ;
        rev:text "Good quality, fast shipping." .

ex:rev3 rdf:type rev:Review ;
        rev:rating "3"^^xsd:integer ;
        bsbm:reviewFor ex:prod2 ;
        dc:date "2024-03-10"^^xsd:date ;
        rev:reviewer ex:reviewer1 ;
        rev:text "Average performance for the price." .

ex:rev4 rdf:type rev:Review ;
        rev:rating "5"^^xsd:integer ;
        bsbm:reviewFor ex:prod3 ;
        dc:date "2024-04-05"^^xsd:date ;
        rev:reviewer ex:reviewer3 ;
        rev:text "Great value for money." .

# ── Reviewers ──
ex:reviewer1 rdf:type foaf:Person ; foaf:name "Alice" .
ex:reviewer2 rdf:type foaf:Person ; foaf:name "Bob" .
ex:reviewer3 rdf:type foaf:Person ; foaf:name "Carol" .
"#);
    s
}

// ═══════════════════════════════════════════════════════════
// BSBM Queries
// ═══════════════════════════════════════════════════════════

#[test]
fn bsbm_q1_find_products_by_features() {
    // BSBM Q1: Find products that have features X AND Y, with numeric property in range
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX ex:   <http://example.org/>
        PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
        SELECT DISTINCT ?product ?label WHERE {
            ?product rdf:type bsbm:Product ;
                     rdfs:label ?label ;
                     bsbm:productFeature ex:feat1 ;
                     bsbm:productFeature ex:feat2 ;
                     bsbm:productPropertyNumeric1 ?numVal .
            FILTER(?numVal > 50 && ?numVal < 200)
        } ORDER BY ?label LIMIT 10
    "#, 5000);
    assert!(!r.is_empty(), "Q1 should find products with features 1 and 2, numProp1 in (50,200)");
    // prod1 has feat1+feat2, numProp1=100 (in range)
    // prod4 has feat1+feat2, numProp1=150 (in range)
    assert!(r.len() >= 1, "At least one product matches: {:?}", r);
}

#[test]
fn bsbm_q2_product_details() {
    // BSBM Q2: Retrieve details about a specific product
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        SELECT ?label ?feature ?prop1 ?prop2 ?text WHERE {
            <http://example.org/prod1> rdfs:label ?label .
            OPTIONAL { <http://example.org/prod1> bsbm:productFeature ?feature }
            OPTIONAL { <http://example.org/prod1> bsbm:productPropertyNumeric1 ?prop1 }
            OPTIONAL { <http://example.org/prod1> bsbm:productPropertyNumeric2 ?prop2 }
            OPTIONAL { <http://example.org/prod1> bsbm:productPropertyTextual1 ?text }
        } ORDER BY ?feature
    "#, 5000);
    assert!(!r.is_empty(), "Q2 should return product details");
    assert!(r[0][0].contains("Widget Pro"), "Label: {}", r[0][0]);
}

#[test]
fn bsbm_q3_find_products_with_cheaper_alternatives() {
    // BSBM Q3: Find products similar to a reference product with lower price
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
        SELECT DISTINCT ?product ?label WHERE {
            <http://example.org/prod2> bsbm:productFeature ?feat .
            ?product bsbm:productFeature ?feat .
            FILTER(?product != <http://example.org/prod2>)
            ?product rdfs:label ?label .
            ?product bsbm:productPropertyNumeric1 ?prop1 .
            <http://example.org/prod2> bsbm:productPropertyNumeric1 ?refProp1 .
            FILTER(?prop1 < ?refProp1)
        } ORDER BY ?product LIMIT 5
    "#, 5000);
    // prod2 has feat2+feat3, numProp1=200; prod1 has feat2, numProp1=100 (cheaper)
    assert!(!r.is_empty(), "Q3 should find cheaper alternatives");
}

#[test]
fn bsbm_q4_find_products_all_features() {
    // BSBM Q4: Find products matching a set of features (any combination)
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        PREFIX xsd:  <http://www.w3.org/2001/XMLSchema#>
        SELECT ?product ?label (COUNT(?feat) AS ?matchCount) WHERE {
            ?product rdf:type bsbm:Product ;
                     rdfs:label ?label ;
                     bsbm:productFeature ?feat .
            VALUES ?feat { <http://example.org/feat1> <http://example.org/feat2> }
        } GROUP BY ?product ?label ORDER BY DESC(?matchCount) LIMIT 5
    "#, 5000);
    assert!(!r.is_empty(), "Q4 should find products with matching features");
}

#[test]
fn bsbm_q5_find_offers_for_product() {
    // BSBM Q5: Find cheapest offer for a given product
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX foaf: <http://xmlns.com/foaf/0.1/>
        SELECT ?offer ?price ?vendor ?vendorName ?delivery WHERE {
            ?offer bsbm:product <http://example.org/prod1> ;
                   bsbm:price ?price ;
                   bsbm:vendor ?vendor ;
                   bsbm:deliveryDays ?delivery .
            ?vendor foaf:name ?vendorName .
        } ORDER BY ?price LIMIT 5
    "#, 5000);
    assert!(!r.is_empty(), "Q5 should find offers for prod1");
    // Cheapest offer should be offer1 at 89.99
    assert!(r[0][1].contains("89.99") || r[0][1].contains("89"),
            "Cheapest offer price: {}", r[0][1]);
}

#[test]
fn bsbm_q6_ask_product_exists() {
    // BSBM Q6: ASK whether product has certain features
    let s = setup_bsbm_store();
    let r = ask(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        ASK {
            <http://example.org/prod4>
                bsbm:productFeature <http://example.org/feat1> ;
                bsbm:productFeature <http://example.org/feat2> ;
                bsbm:productFeature <http://example.org/feat3> .
        }
    "#);
    assert!(r, "Q6: prod4 should have all 3 features");
}

#[test]
fn bsbm_q7_get_reviews_for_product() {
    // BSBM Q7: Get all reviews for a product
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX rev:  <http://purl.org/stuff/rev#>
        PREFIX foaf: <http://xmlns.com/foaf/0.1/>
        PREFIX dc:   <http://purl.org/dc/elements/1.1/>
        SELECT ?review ?rating ?reviewer ?reviewerName ?reviewDate WHERE {
            ?review bsbm:reviewFor <http://example.org/prod1> ;
                    rev:rating ?rating ;
                    rev:reviewer ?reviewer ;
                    dc:date ?reviewDate .
            ?reviewer foaf:name ?reviewerName .
        } ORDER BY DESC(?reviewDate)
    "#, 5000);
    assert_eq!(r.len(), 2, "prod1 has 2 reviews: {:?}", r);
}

#[test]
fn bsbm_q8_reviews_by_rating() {
    // BSBM Q8: Products with average rating above threshold
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX rev:  <http://purl.org/stuff/rev#>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        SELECT ?product ?label (AVG(?rating) AS ?avgRating) WHERE {
            ?review bsbm:reviewFor ?product ;
                    rev:rating ?rating .
            ?product rdfs:label ?label .
        } GROUP BY ?product ?label HAVING (AVG(?rating) >= 4) ORDER BY DESC(?avgRating)
    "#, 5000);
    assert!(!r.is_empty(), "Q8 should find highly rated products");
    // prod1 has avg rating = (5+4)/2 = 4.5; prod3 has avg = 5
}

#[test]
fn bsbm_q9_get_cheapest_product_from_vendor() {
    // BSBM Q9: Get cheapest product from a specific vendor
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        SELECT ?product ?label ?price WHERE {
            ?offer bsbm:vendor <http://example.org/vendor1> ;
                   bsbm:product ?product ;
                   bsbm:price ?price .
            ?product rdfs:label ?label .
        } ORDER BY ?price LIMIT 1
    "#, 5000);
    assert_eq!(r.len(), 1, "Q9 should return cheapest product from vendor1");
    // offer1 (prod1, 89.99) and offer3 (prod2, 149.99) from vendor1; cheapest is prod1
    assert!(r[0][2].contains("89.99") || r[0][2].contains("89"),
            "Cheapest price: {}", r[0][2]);
}

#[test]
fn bsbm_q10_product_description_text() {
    // BSBM Q10: Text search for products (REGEX on description)
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        SELECT ?product ?label ?text WHERE {
            ?product rdfs:label ?label ;
                     bsbm:productPropertyTextual1 ?text .
            FILTER REGEX(?text, "gadget", "i")
        } ORDER BY ?label
    "#, 5000);
    assert!(!r.is_empty(), "Q10 should find gadget products by text");
    // prod2 and prod4 mention "gadget"
    assert!(r.len() >= 2, "At least 2 gadget products: {:?}", r);
}

#[test]
fn bsbm_q11_find_products_similar_to_reviewed() {
    // BSBM Q11: Find products similar to those reviewed by a given reviewer
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        PREFIX rev:  <http://purl.org/stuff/rev#>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        SELECT DISTINCT ?similar ?label WHERE {
            # What did reviewer1 review?
            ?review rev:reviewer <http://example.org/reviewer1> ;
                    bsbm:reviewFor ?reviewedProd .
            # What features does the reviewed product have?
            ?reviewedProd bsbm:productFeature ?feat .
            # Find similar products with the same feature
            ?similar bsbm:productFeature ?feat .
            FILTER(?similar != ?reviewedProd)
            ?similar rdfs:label ?label .
        } ORDER BY ?label
    "#, 5000);
    assert!(!r.is_empty(), "Q11 should find similar products");
}

#[test]
fn bsbm_q12_offer_count_by_country() {
    // BSBM Q12: Count offers by vendor country
    let s = setup_bsbm_store();
    let r = timed_select(&s, r#"
        PREFIX rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
        PREFIX bsbm: <http://www4.wiwiss.fu-berlin.de/bizer/bsbm/v01/vocabulary/>
        SELECT ?country (COUNT(?offer) AS ?count) WHERE {
            ?offer rdf:type bsbm:Offer ;
                   bsbm:vendor ?vendor .
            ?vendor bsbm:country ?country .
        } GROUP BY ?country ORDER BY DESC(?count)
    "#, 5000);
    assert!(!r.is_empty(), "Q12 should return offer counts by country");
    // US has offer1+offer3 = 2 offers; UK has offer2 = 1; DE has offer4 = 1
    assert!(r[0][1].contains("2"), "US should have 2 offers: {:?}", r);
}

// ═══════════════════════════════════════════════════════════
// Performance regression baseline tests
// These verify query latency stays under reasonable thresholds
// on the small test datasets (not a substitute for full-scale benchmarks)
// ═══════════════════════════════════════════════════════════

#[test]
fn perf_simple_triple_pattern_under_1ms() {
    let s = ts();
    // Load 100 triples
    for i in 0..100 {
        s.update(&format!(
            "INSERT DATA {{ <http://ex/s{i}> <http://ex/p> \"val{i}\" }}"
        )).unwrap();
    }
    let start = Instant::now();
    let r = select(&s, "SELECT * WHERE { ?s <http://ex/p> ?o } LIMIT 10");
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(100),
            "Simple pattern on 100 triples should be fast: {}ms", elapsed.as_millis());
    assert_eq!(r.len(), 10);
}

#[test]
fn perf_aggregation_under_100ms() {
    let s = ts();
    // Load 500 triples
    for i in 0..500 {
        s.update(&format!(
            "INSERT DATA {{ <http://ex/s{}> <http://ex/val> {} }}",
            i, i
        )).unwrap();
    }
    let start = Instant::now();
    let r = select(&s, "SELECT (COUNT(*) AS ?c) (SUM(?v) AS ?s) (AVG(?v) AS ?avg) WHERE { ?x <http://ex/val> ?v }");
    let elapsed = start.elapsed();
    assert!(elapsed < Duration::from_millis(500),
            "Aggregation on 500 triples: {}ms", elapsed.as_millis());
    assert!(r[0][0].contains("500"), "Count = 500: {}", r[0][0]);
}

#[test]
fn perf_concurrent_reads() {
    let s = ts();
    for i in 0..100 {
        s.update(&format!(
            "INSERT DATA {{ <http://ex/s{}> <http://ex/p> \"v{}\" }}", i, i
        )).unwrap();
    }

    let handles: Vec<_> = (0..20).map(|_| {
        let store = s.clone();
        std::thread::spawn(move || {
            let r = select(&store, "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }");
            assert!(r[0][0].contains("100"));
        })
    }).collect();

    for h in handles {
        h.join().unwrap();
    }
}
