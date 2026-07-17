//! Performance benchmarks for open-triplestore.
//!
//! Covers the primary hot-paths:
//! - Bulk data loading via the RDF bulk-loader
//! - SPARQL query forms: simple lookup, multi-join, aggregation,
//!   property paths, OPTIONAL, FILTER, VALUES, BIND, MINUS, NOT EXISTS,
//!   CONSTRUCT, named-graph, GROUP_CONCAT
//! - SPARQL UPDATE forms: INSERT WHERE, DELETE WHERE, batched INSERT DATA
//! - GeoSPARQL spatial relation checks and constructive functions
//! - SHACL validation (clean data and data with violations)
//! - Concurrent read throughput, concurrent writes, mixed read/write
//!
//! # Running
//!
//! ```
//! cargo bench                          # all benchmarks + HTML report
//! cargo bench -- query                 # only groups whose name contains "query"
//! cargo bench -- insert/10000          # one specific input size
//! cargo bench --bench performance -- --list   # list all benchmark IDs
//! ```
//!
//! Reports are written to `target/criterion/`. Open
//! `target/criterion/report/index.html` for an interactive summary.
//!
//! # Dataset
//!
//! All datasets are generated in-process using a deterministic pseudo-random
//! pattern so that results are reproducible across machines. The default graph
//! is used throughout unless stated otherwise; GeoSPARQL benchmarks store WKT
//! geometries.

use std::sync::{Arc, Mutex};

use criterion::{
    criterion_group, criterion_main, BenchmarkId, Criterion, SamplingMode, Throughput,
};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn fresh_store() -> TripleStore {
    TripleStore::in_memory().unwrap()
}

/// Generate N generic person triples in Turtle.
///
/// Each person has: name, age, type (one of 10 categories), score, email.
/// This gives a realistic multi-property join workload.
fn gen_persons_ttl(n: usize) -> String {
    let mut s = String::from(
        "@prefix ex: <http://example.org/> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n",
    );
    for i in 0..n {
        let kind = i % 10;
        let age = 18 + (i % 65);
        let score = (i as f64 * 7.13) % 100.0;
        s.push_str(&format!(
            "ex:p{i} ex:name \"Person {i}\" ; \
             ex:age {age} ; \
             ex:type ex:Type{kind} ; \
             ex:score {score:.2} ; \
             ex:email \"person{i}@example.org\" .\n"
        ));
    }
    s
}

/// Generate N linked-list triples for property-path benchmarks.
///
/// ex:n0 ex:next ex:n1 . ex:n1 ex:next ex:n2 . … ex:n(N-1) ex:next ex:nN .
fn gen_chain_ttl(n: usize) -> String {
    let mut s = String::from("@prefix ex: <http://example.org/> .\n");
    for i in 0..n {
        s.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
    }
    s
}

/// Generate N WKT point geometries for GeoSPARQL benchmarks.
fn gen_geo_ttl(n: usize) -> String {
    let mut s = String::from(
        "@prefix ex: <http://example.org/> .\n\
         @prefix geo: <http://www.opengis.net/ont/geosparql#> .\n\
         @prefix geof: <http://www.opengis.net/def/function/geosparql/> .\n",
    );
    for i in 0..n {
        // Scatter points across a 10×10 degree bounding box
        let lon = (i % 100) as f64 * 0.1;
        let lat = ((i / 100) % 100) as f64 * 0.1;
        s.push_str(&format!(
            "ex:feat{i} geo:hasGeometry ex:geom{i} .\n\
             ex:geom{i} geo:asWKT \"POINT({lon:.2} {lat:.2})\"^^geo:wktLiteral .\n"
        ));
    }
    s
}

/// Generate N WKT polygon geometries for GeoSPARQL complexity benchmarks.
///
/// Each polygon is a small 5-vertex square, more expensive than a point for GEOS.
fn gen_polygon_geo_ttl(n: usize) -> String {
    let mut s = String::from(
        "@prefix ex: <http://example.org/> .\n\
         @prefix geo: <http://www.opengis.net/ont/geosparql#> .\n",
    );
    for i in 0..n {
        let lon = (i % 50) as f64 * 0.2;
        let lat = ((i / 50) % 50) as f64 * 0.2;
        let d = 0.05_f64; // half-side of small square
        s.push_str(&format!(
            "ex:feat{i} geo:hasGeometry ex:geom{i} .\n\
             ex:geom{i} geo:asWKT \
             \"POLYGON(({lo} {la},{hi} {la},{hi} {ha},{lo} {ha},{lo} {la}))\"^^geo:wktLiteral .\n",
            lo = lon - d,
            hi = lon + d,
            la = lat - d,
            ha = lat + d,
        ));
    }
    s
}

/// Generate N person triples distributed across `graphs` named graphs (N-Quads).
///
/// Uses plain string literals for portability (no typed literal `^^` syntax required).
fn gen_named_graph_nq(n: usize, graphs: usize) -> String {
    let mut s = String::new();
    for i in 0..n {
        let g = i % graphs;
        let age = 18 + (i % 65);
        let kind = i % 10;
        s.push_str(&format!(
            "<http://example.org/p{i}> <http://example.org/name> \"Person {i}\" <http://example.org/g{g}> .\n\
             <http://example.org/p{i}> <http://example.org/age> \"{age}\" <http://example.org/g{g}> .\n\
             <http://example.org/p{i}> <http://example.org/type> <http://example.org/Type{kind}> <http://example.org/g{g}> .\n"
        ));
    }
    s
}

/// Generate a SHACL shapes graph (Turtle) requiring ex:name and ex:age on ex:Person nodes.
fn gen_shapes_ttl() -> String {
    String::from(
        "@prefix sh:  <http://www.w3.org/ns/shacl#> .\n\
         @prefix ex:  <http://example.org/> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
         ex:PersonShape a sh:NodeShape ;\n\
             sh:targetClass ex:Person ;\n\
             sh:property [ sh:path ex:name ; sh:minCount 1 ] ;\n\
             sh:property [ sh:path ex:age  ; sh:minCount 1 ; sh:minInclusive 0 ] .\n",
    )
}

/// Generate N person triples for SHACL benchmarks, with `rdf:type ex:Person` added.
/// `violation_rate` fraction of records intentionally omit ex:age.
fn gen_shacl_persons_ttl(n: usize, violation_rate: f64) -> String {
    let mut s = String::from(
        "@prefix ex:  <http://example.org/> .\n\
         @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
         @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n",
    );
    let violations = (n as f64 * violation_rate) as usize;
    for i in 0..n {
        let age = 18 + (i % 65);
        s.push_str(&format!(
            "ex:p{i} rdf:type ex:Person ; ex:name \"Person {i}\" .\n"
        ));
        if i >= violations {
            s.push_str(&format!("ex:p{i} ex:age {age} .\n"));
        }
    }
    s
}

/// Fully consume a `QueryResults` iterator so timing is fair.
fn consume_solutions(results: QueryResults) -> usize {
    match results {
        QueryResults::Solutions(sols) => sols.count(),
        QueryResults::Boolean(_) => 1,
        QueryResults::Graph(g) => g.count(),
    }
}

// ─── Insert benchmarks ────────────────────────────────────────────────────────

/// Measure bulk-loader throughput for various dataset sizes.
fn bench_insert_bulk(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert/bulk_loader");
    group.sample_size(20);
    group.sampling_mode(SamplingMode::Flat);

    for &n in &[100_usize, 1_000, 10_000, 100_000] {
        let ttl = gen_persons_ttl(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &ttl, |b, data| {
            b.iter(|| {
                let store = fresh_store();
                store.load_str(data, RdfFormat::Turtle, None).unwrap();
                store.len().unwrap()
            });
        });
    }
    group.finish();
}

/// Measure single-triple SPARQL UPDATE INSERT throughput.
fn bench_insert_update(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert/sparql_update");
    group.throughput(Throughput::Elements(1));
    group.bench_function("single_triple", |b| {
        let store = fresh_store();
        let mut i: usize = 0;
        b.iter(|| {
            store
                .update(&format!(
                    "INSERT DATA {{ <http://ex/s{i}> <http://ex/p> \"v{i}\" }}"
                ))
                .unwrap();
            i += 1;
        });
    });
    group.finish();
}

/// Measure batched INSERT DATA (10 triples per statement) vs. single-triple.
///
/// Batching amortises SPARQL parse overhead; cost per triple should be 3–5× lower.
fn bench_insert_update_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert/sparql_update_batch");
    group.throughput(Throughput::Elements(10));
    group.bench_function("10_triples", |b| {
        let store = fresh_store();
        let mut base: usize = 0;
        b.iter(|| {
            let triples: String = (base..base + 10)
                .map(|i| format!("<http://ex/s{i}> <http://ex/p> \"v{i}\" ."))
                .collect::<Vec<_>>()
                .join(" ");
            store
                .update(&format!("INSERT DATA {{ {triples} }}"))
                .unwrap();
            base += 10;
        });
    });
    group.finish();
}

/// Measure named-graph bulk load (N-Quads into 10 distinct named graphs).
fn bench_insert_named_graph(c: &mut Criterion) {
    let mut group = c.benchmark_group("insert/named_graph");
    group.sample_size(20);
    group.sampling_mode(SamplingMode::Flat);

    for &n in &[1_000_usize, 10_000, 100_000] {
        let nq = gen_named_graph_nq(n, 10);
        let triples = n * 3; // 3 triples per person across 10 graphs
        group.throughput(Throughput::Elements(triples as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &nq, |b, data| {
            b.iter(|| {
                let store = fresh_store();
                store.load_str(data, RdfFormat::NQuads, None).unwrap();
                store.len().unwrap()
            });
        });
    }
    group.finish();
}

// ─── Simple lookup benchmarks ─────────────────────────────────────────────────

/// Measure single-predicate SELECT on datasets of increasing size.
fn bench_query_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/simple_lookup");

    for &n in &[100_usize, 1_000, 10_000, 100_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query("SELECT ?s ?name WHERE { ?s <http://example.org/name> ?name }")
                        .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure single-predicate SELECT with LIMIT (early termination path).
fn bench_query_lookup_limit(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/lookup_with_limit");

    for &n in &[1_000_usize, 10_000, 100_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?s ?name WHERE { ?s <http://example.org/name> ?name } LIMIT 10",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ─── Join benchmarks ──────────────────────────────────────────────────────────

/// Measure two-way join (two triple patterns, shared variable).
fn bench_query_join_2way(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/join_2way");

    for &n in &[100_usize, 1_000, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?name ?age WHERE { \
                           ?s <http://example.org/name> ?name . \
                           ?s <http://example.org/age>  ?age \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure three-way join (three triple patterns, two shared variables).
fn bench_query_join_3way(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/join_3way");

    for &n in &[100_usize, 1_000, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?name ?age ?type WHERE { \
                           ?s <http://example.org/name>  ?name . \
                           ?s <http://example.org/age>   ?age  . \
                           ?s <http://example.org/type>  ?type \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ─── FILTER benchmarks ────────────────────────────────────────────────────────

/// Measure FILTER with numeric comparison.
fn bench_query_filter(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/filter");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?s ?age WHERE { \
                           ?s <http://example.org/age> ?age \
                           FILTER(?age >= 40 && ?age < 60) \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure FILTER with REGEX (string scan).
fn bench_query_regex(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/regex_filter");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?s ?name WHERE { \
                           ?s <http://example.org/name> ?name \
                           FILTER(REGEX(?name, \"^Person [0-9]$\")) \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ─── OPTIONAL benchmarks ──────────────────────────────────────────────────────

/// Measure OPTIONAL (left outer join).
fn bench_query_optional(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/optional");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?name ?email WHERE { \
                           ?s <http://example.org/name> ?name \
                           OPTIONAL { ?s <http://example.org/email> ?email } \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ─── Aggregation benchmarks ───────────────────────────────────────────────────

/// Measure COUNT(*) (full-scan aggregation).
fn bench_query_count(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/count_star");

    for &n in &[1_000_usize, 10_000, 100_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query("SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }")
                        .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure GROUP BY with COUNT and AVG over a type dimension.
fn bench_query_group_by(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/group_by");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?type (COUNT(?s) AS ?cnt) (AVG(?age) AS ?avg_age) WHERE { \
                           ?s <http://example.org/type> ?type . \
                           ?s <http://example.org/age>  ?age \
                         } GROUP BY ?type ORDER BY DESC(?cnt)",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure GROUP_CONCAT string aggregation — builds long concatenated strings per group.
///
/// Produces one row per type with a comma-separated list of names. Tests string
/// allocation overhead on top of the GROUP BY hash table.
fn bench_query_group_concat(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/group_concat");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?type (GROUP_CONCAT(?name; separator=\",\") AS ?names) WHERE { \
                           ?s <http://example.org/type> ?type . \
                           ?s <http://example.org/name> ?name \
                         } GROUP BY ?type",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ─── Property path benchmarks ─────────────────────────────────────────────────

/// Measure transitive property path `ex:next+` over a linked chain.
fn bench_query_transitive_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/transitive_path");
    for &n in &[50_usize, 100, 200] {
        let store = fresh_store();
        store
            .load_str(&gen_chain_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?a ?b WHERE { \
                           ?a <http://example.org/next>+ ?b \
                         } LIMIT 50",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure alternative property path (|) with fixed depth.
fn bench_query_alternative_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/alternative_path");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?s ?v WHERE { \
                           ?s (<http://example.org/name>|<http://example.org/email>) ?v \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure zero-or-more path `ex:next*` (includes the start node itself).
///
/// Returns all (a, b) pairs where b is reachable from a in 0 or more steps.
/// LIMIT 50 prevents runaway result materialisation on longer chains.
fn bench_query_zero_or_more_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("path/zero_or_more");
    for &n in &[50_usize, 100, 200] {
        let store = fresh_store();
        store
            .load_str(&gen_chain_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?a ?b WHERE { \
                           ?a <http://example.org/next>* ?b \
                         } LIMIT 50",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure sequence path `ex:next/ex:next` (depth-2 hop, rewritten to a join).
///
/// Equivalent to a 2-way join `?a ex:next ?mid . ?mid ex:next ?b`. This benchmark
/// verifies that the path rewriter produces an efficient join plan.
fn bench_query_sequence_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("path/sequence");
    for &n in &[100_usize, 500, 1_000] {
        let store = fresh_store();
        store
            .load_str(&gen_chain_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?a ?b WHERE { \
                           ?a (<http://example.org/next>/<http://example.org/next>) ?b \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure inverse path `^ex:next` (reverses subject/object lookup direction).
///
/// `?b ^ex:next ?a` is semantically `?a ex:next ?b` but expressed from the
/// object side. Tests whether the query engine uses the O-P-S index.
fn bench_query_inverse_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("path/inverse");
    for &n in &[100_usize, 1_000, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_chain_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?a ?b WHERE { \
                           ?b ^<http://example.org/next> ?a \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure negated property set `!(ex:name|ex:age)`.
///
/// Returns all triples whose predicate is NOT in the excluded set.
/// Rewritten internally as a full scan with predicate NOT IN filter.
fn bench_query_negated_property_set(c: &mut Criterion) {
    let mut group = c.benchmark_group("path/negated_property_set");
    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?s ?o WHERE { \
                           ?s !(<http://example.org/name>|<http://example.org/age>) ?o \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ─── Subquery benchmarks ──────────────────────────────────────────────────────

/// Measure SELECT wrapping a subquery with MAX aggregation.
fn bench_query_subquery(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/subquery");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?name ?age WHERE { \
                           ?s <http://example.org/name> ?name . \
                           ?s <http://example.org/age>  ?age . \
                           { SELECT (MAX(?a2) AS ?maxAge) WHERE { ?x <http://example.org/age> ?a2 } } \
                           FILTER(?age = ?maxAge) \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ─── SPARQL 1.1 operator benchmarks ──────────────────────────────────────────

/// Measure VALUES inline-data join.
///
/// VALUES ?type { ex:Type0 ex:Type1 ex:Type2 } acts as an equality filter driven
/// by a small inline table; the engine should produce one index probe per value.
fn bench_query_values(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/values");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?s ?name WHERE { \
                           VALUES ?type { \
                             <http://example.org/Type0> \
                             <http://example.org/Type1> \
                             <http://example.org/Type2> \
                           } \
                           ?s <http://example.org/type> ?type . \
                           ?s <http://example.org/name> ?name \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure BIND expression evaluation.
///
/// BIND does not filter rows; it extends each solution with a derived value.
/// Tests arithmetic expression evaluation throughput.
fn bench_query_bind(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/bind");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?s ?age ?doubled WHERE { \
                           ?s <http://example.org/age> ?age \
                           BIND(?age * 2 AS ?doubled) \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure MINUS set-difference operator.
///
/// MINUS builds a hash set of bindings from the right-hand side, then filters
/// the left side. Faster than NOT EXISTS for large exclusion sets.
fn bench_query_minus(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/minus");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?s ?name WHERE { \
                           ?s <http://example.org/name> ?name \
                           MINUS { ?s <http://example.org/type> <http://example.org/Type0> } \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure FILTER NOT EXISTS (correlated existence check).
///
/// NOT EXISTS re-evaluates its inner pattern once per outer row, making it
/// O(n × inner_cost). Compare to MINUS which uses a hash-set approach.
fn bench_query_not_exists(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/not_exists");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?s ?name WHERE { \
                           ?s <http://example.org/name> ?name \
                           FILTER NOT EXISTS { ?s <http://example.org/type> <http://example.org/Type0> } \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure CONSTRUCT query — builds an RDF graph from matched patterns.
///
/// CONSTRUCT materialises triples into a new graph; this adds serialisation
/// overhead vs. SELECT. The benchmark uses LIMIT 1000 to bound output.
fn bench_query_construct(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/construct");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_persons_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "CONSTRUCT { ?s <http://example.org/name> ?name } \
                         WHERE { ?s <http://example.org/name> ?name } LIMIT 1000",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure named-graph queries (GRAPH clause).
///
/// Loads triples into 10 named graphs; queries with an unbound GRAPH variable
/// to enumerate all graph–subject–name combinations.
fn bench_query_named_graph(c: &mut Criterion) {
    let mut group = c.benchmark_group("query/named_graph");

    for &n in &[1_000_usize, 10_000] {
        let store = fresh_store();
        store
            .load_str(&gen_named_graph_nq(n, 10), RdfFormat::NQuads, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "SELECT ?g ?s ?name WHERE { \
                           GRAPH ?g { ?s <http://example.org/name> ?name } \
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ─── SPARQL Update benchmarks ─────────────────────────────────────────────────

/// Measure INSERT WHERE (read-modify-write): derive a new property from existing data.
///
/// `INSERT { ?s ex:doubled ?d } WHERE { ?s ex:age ?a BIND(?a * 2 AS ?d) }` reads
/// all ages, computes a derived value, and inserts new triples in one transaction.
fn bench_update_insert_where(c: &mut Criterion) {
    let mut group = c.benchmark_group("update/insert_where");

    for &n in &[100_usize, 1_000, 10_000] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &size| {
            b.iter_batched(
                || {
                    let store = fresh_store();
                    store
                        .load_str(&gen_persons_ttl(size), RdfFormat::Turtle, None)
                        .unwrap();
                    store
                },
                |store| {
                    store
                        .update(
                            "INSERT { ?s <http://example.org/doubled> ?d } \
                             WHERE { ?s <http://example.org/age> ?a \
                                     BIND(?a * 2 AS ?d) }",
                        )
                        .unwrap();
                    store.len().unwrap()
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// Measure DELETE WHERE: selectively remove triples matching a pattern.
///
/// Deletes all triples for persons of Type0 (~10% of persons). Tests the
/// write-path under a moderately selective DELETE.
fn bench_update_delete_where(c: &mut Criterion) {
    let mut group = c.benchmark_group("update/delete_where");

    for &n in &[100_usize, 1_000, 10_000] {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &size| {
            b.iter_batched(
                || {
                    let store = fresh_store();
                    store
                        .load_str(&gen_persons_ttl(size), RdfFormat::Turtle, None)
                        .unwrap();
                    store
                },
                |store| {
                    store
                        .update(
                            "DELETE { ?s <http://example.org/type> ?t } \
                             WHERE  { ?s <http://example.org/type> ?t \
                                      FILTER(?t = <http://example.org/Type0>) }",
                        )
                        .unwrap();
                    store.len().unwrap()
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

// ─── GeoSPARQL benchmarks ─────────────────────────────────────────────────────

/// Measure GeoSPARQL sfContains over point features.
fn bench_geosparql_contains(c: &mut Criterion) {
    let mut group = c.benchmark_group("geosparql/sf_contains");

    for &n in &[50_usize, 200] {
        let store = fresh_store();
        store
            .load_str(&gen_geo_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "PREFIX geo:  <http://www.opengis.net/ont/geosparql#>\n\
                         PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                         SELECT ?f WHERE {\n\
                           ?f geo:hasGeometry ?g .\n\
                           ?g geo:asWKT ?wkt .\n\
                           FILTER(geof:sfContains(\n\
                             \"POLYGON((0 0,5 0,5 5,0 5,0 0))\"^^geo:wktLiteral,\n\
                             ?wkt\n\
                           ))\n\
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure geof:distance (Euclidean) for spatial proximity queries.
fn bench_geosparql_distance(c: &mut Criterion) {
    let mut group = c.benchmark_group("geosparql/distance");

    for &n in &[50_usize, 200] {
        let store = fresh_store();
        store
            .load_str(&gen_geo_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "PREFIX geo:  <http://www.opengis.net/ont/geosparql#>\n\
                         PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                         PREFIX uom:  <http://www.opengis.net/def/uom/OGC/1.0/>\n\
                         SELECT ?f ?dist WHERE {\n\
                           ?f geo:hasGeometry ?g .\n\
                           ?g geo:asWKT ?wkt .\n\
                           BIND(geof:distance(\n\
                             \"POINT(0 0)\"^^geo:wktLiteral,\n\
                             ?wkt,\n\
                             uom:metre\n\
                           ) AS ?dist)\n\
                           FILTER(?dist < 300000)\n\
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure geof:sfIntersects — DE-9IM relation testing for polygon/point overlap.
///
/// Intersects is slightly cheaper than Contains for most geometry pairs because
/// the early-exit condition is hit more often (more candidates intersect than are
/// fully contained). This benchmark quantifies the difference.
fn bench_geosparql_intersects(c: &mut Criterion) {
    let mut group = c.benchmark_group("geosparql/sf_intersects");

    for &n in &[50_usize, 200] {
        let store = fresh_store();
        store
            .load_str(&gen_geo_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "PREFIX geo:  <http://www.opengis.net/ont/geosparql#>\n\
                         PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                         SELECT ?f WHERE {\n\
                           ?f geo:hasGeometry ?g .\n\
                           ?g geo:asWKT ?wkt .\n\
                           FILTER(geof:sfIntersects(\n\
                             \"POLYGON((0 0,5 0,5 5,0 5,0 0))\"^^geo:wktLiteral,\n\
                             ?wkt\n\
                           ))\n\
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

/// Measure geometry complexity cost: sfContains against polygon features vs. points.
///
/// GEOS DE-9IM computation scales with the number of vertices in both geometries.
/// This benchmark compares point (2 values) vs. 5-vertex polygon geometries to
/// quantify the overhead for more complex shapes.
fn bench_geosparql_polygon_complexity(c: &mut Criterion) {
    let mut group = c.benchmark_group("geosparql/polygon_complexity");

    for &n in &[50_usize, 200] {
        let store_pts = {
            let s = fresh_store();
            s.load_str(&gen_geo_ttl(n), RdfFormat::Turtle, None)
                .unwrap();
            s
        };
        let store_poly = {
            let s = fresh_store();
            s.load_str(&gen_polygon_geo_ttl(n), RdfFormat::Turtle, None)
                .unwrap();
            s
        };

        let query = "PREFIX geo:  <http://www.opengis.net/ont/geosparql#>\n\
                     PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                     SELECT ?f WHERE {\n\
                       ?f geo:hasGeometry ?g .\n\
                       ?g geo:asWKT ?wkt .\n\
                       FILTER(geof:sfIntersects(\n\
                         \"POLYGON((0 0,5 0,5 5,0 5,0 0))\"^^geo:wktLiteral,\n\
                         ?wkt\n\
                       ))\n\
                     }";

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::new("points", n), &store_pts, |b, s| {
            b.iter(|| consume_solutions(s.query(query).unwrap()))
        });
        group.bench_with_input(BenchmarkId::new("polygons", n), &store_poly, |b, s| {
            b.iter(|| consume_solutions(s.query(query).unwrap()))
        });
    }
    group.finish();
}

/// Measure geof:buffer constructive function — creates a buffered polygon per feature.
///
/// Unlike relation-check functions (sfContains, sfIntersects), geof:buffer returns a
/// new geometry literal for every row. This measures constructive-function throughput.
fn bench_geosparql_buffer(c: &mut Criterion) {
    let mut group = c.benchmark_group("geosparql/buffer");

    for &n in &[50_usize, 200] {
        let store = fresh_store();
        store
            .load_str(&gen_geo_ttl(n), RdfFormat::Turtle, None)
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            b.iter(|| {
                consume_solutions(
                    s.query(
                        "PREFIX geo:  <http://www.opengis.net/ont/geosparql#>\n\
                         PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                         PREFIX uom:  <http://www.opengis.net/def/uom/OGC/1.0/>\n\
                         SELECT ?f ?buf WHERE {\n\
                           ?f geo:hasGeometry ?g .\n\
                           ?g geo:asWKT ?wkt .\n\
                           BIND(geof:buffer(?wkt, 0.5, uom:degree) AS ?buf)\n\
                         }",
                    )
                    .unwrap(),
                )
            });
        });
    }
    group.finish();
}

// ─── SHACL benchmarks ─────────────────────────────────────────────────────────

/// Measure SHACL validation on a fully-conformant dataset.
///
/// All N persons have the required ex:name and ex:age properties. Measures the
/// baseline overhead of shapes loading + focus-node resolution + constraint
/// evaluation when no violations are found.
fn bench_shacl_validate_clean(c: &mut Criterion) {
    let mut group = c.benchmark_group("shacl/validate_clean");
    group.sample_size(20);

    for &n in &[100_usize, 500, 1_000] {
        let data_graph = "http://example.org/data";
        let shapes_graph = "http://example.org/shapes";

        let store = fresh_store();
        // Load data into a named graph
        store
            .load_str(
                &gen_shacl_persons_ttl(n, 0.0),
                RdfFormat::Turtle,
                Some(data_graph),
            )
            .unwrap();
        // Load shapes into a separate named graph
        store
            .load_str(&gen_shapes_ttl(), RdfFormat::Turtle, Some(shapes_graph))
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            let dg = vec![data_graph.to_string()];
            b.iter(|| open_triplestore::shacl::validate(s, shapes_graph, &dg).unwrap());
        });
    }
    group.finish();
}

/// Measure SHACL validation with 20% violation rate.
///
/// One in five persons intentionally omits ex:age (violating sh:minCount 1).
/// Tests violation-accumulation overhead compared to the clean baseline.
fn bench_shacl_validate_violations(c: &mut Criterion) {
    let mut group = c.benchmark_group("shacl/validate_violations");
    group.sample_size(20);

    for &n in &[100_usize, 500, 1_000] {
        let data_graph = "http://example.org/data";
        let shapes_graph = "http://example.org/shapes";

        let store = fresh_store();
        store
            .load_str(
                &gen_shacl_persons_ttl(n, 0.2),
                RdfFormat::Turtle,
                Some(data_graph),
            )
            .unwrap();
        store
            .load_str(&gen_shapes_ttl(), RdfFormat::Turtle, Some(shapes_graph))
            .unwrap();

        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &store, |b, s| {
            let dg = vec![data_graph.to_string()];
            b.iter(|| open_triplestore::shacl::validate(s, shapes_graph, &dg).unwrap());
        });
    }
    group.finish();
}

// ─── Concurrent read benchmarks ───────────────────────────────────────────────

/// Measure read throughput with multiple concurrent threads.
fn bench_concurrent_reads(c: &mut Criterion) {
    let store = fresh_store();
    store
        .load_str(&gen_persons_ttl(10_000), RdfFormat::Turtle, None)
        .unwrap();
    let store = Arc::new(store);

    let mut group = c.benchmark_group("concurrent/reads");
    group.sample_size(20);
    group.sampling_mode(SamplingMode::Flat);

    for &threads in &[1_usize, 2, 4, 8] {
        group.throughput(Throughput::Elements(threads as u64));
        group.bench_with_input(BenchmarkId::new("threads", threads), &threads, |b, &t| {
            b.iter(|| {
                let handles: Vec<_> = (0..t)
                    .map(|_| {
                        let s = Arc::clone(&store);
                        std::thread::spawn(move || {
                            consume_solutions(
                                s.query(
                                    "SELECT ?name ?age WHERE { \
                                           ?s <http://example.org/name> ?name . \
                                           ?s <http://example.org/age>  ?age \
                                         } LIMIT 100",
                                )
                                .unwrap(),
                            )
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().unwrap())
                    .sum::<usize>()
            });
        });
    }
    group.finish();
}

/// Measure concurrent write throughput — N threads each inserting distinct triples.
///
/// Each thread inserts unique triples so there are no logical conflicts. The
/// bottleneck is write-lock contention on the shared store. The benchmark
/// measures wall-clock time for all N threads to complete their batch.
fn bench_concurrent_writes(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent/writes");
    group.sample_size(20);
    group.sampling_mode(SamplingMode::Flat);

    for &threads in &[1_usize, 2, 4] {
        group.throughput(Throughput::Elements((threads * 10) as u64));
        group.bench_with_input(BenchmarkId::new("threads", threads), &threads, |b, &t| {
            b.iter_batched(
                || {
                    let store = Arc::new(fresh_store());
                    let counter = Arc::new(Mutex::new(0usize));
                    (store, counter)
                },
                |(store, counter)| {
                    let handles: Vec<_> = (0..t)
                        .map(|_| {
                            let s = Arc::clone(&store);
                            let ctr = Arc::clone(&counter);
                            std::thread::spawn(move || {
                                // Each thread writes 10 triples with unique subjects
                                let base = {
                                    let mut c = ctr.lock().unwrap();
                                    let v = *c;
                                    *c += 10;
                                    v
                                };
                                for i in base..base + 10 {
                                    s.update(&format!(
                                        "INSERT DATA {{ <http://ex/s{i}> <http://ex/p> \"{i}\" }}"
                                    ))
                                    .unwrap();
                                }
                            })
                        })
                        .collect();
                    for h in handles {
                        h.join().unwrap();
                    }
                    store.len().unwrap()
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

/// Measure mixed read/write throughput — 4 reader threads + 1 writer thread.
///
/// The writer continuously inserts new triples while 4 readers run join queries.
/// Measures read-side throughput degradation under write-lock contention.
fn bench_concurrent_mixed(c: &mut Criterion) {
    let store = Arc::new(fresh_store());
    store
        .load_str(&gen_persons_ttl(5_000), RdfFormat::Turtle, None)
        .unwrap();

    let mut group = c.benchmark_group("concurrent/mixed");
    group.sample_size(20);
    group.sampling_mode(SamplingMode::Flat);
    group.throughput(Throughput::Elements(4)); // 4 reader threads

    group.bench_function("4r_1w", |b| {
        let write_counter = Arc::new(Mutex::new(5_000usize));
        b.iter(|| {
            let readers: Vec<_> = (0..4)
                .map(|_| {
                    let s = Arc::clone(&store);
                    std::thread::spawn(move || {
                        consume_solutions(
                            s.query(
                                "SELECT ?name ?age WHERE { \
                                   ?s <http://example.org/name> ?name . \
                                   ?s <http://example.org/age>  ?age \
                                 } LIMIT 50",
                            )
                            .unwrap(),
                        )
                    })
                })
                .collect();

            // One write during the read batch
            let s = Arc::clone(&store);
            let ctr = Arc::clone(&write_counter);
            let writer = std::thread::spawn(move || {
                let i = {
                    let mut c = ctr.lock().unwrap();
                    let v = *c;
                    *c += 1;
                    v
                };
                s.update(&format!(
                    "INSERT DATA {{ <http://ex/s{i}> <http://ex/p> \"{i}\" }}"
                ))
                .unwrap();
            });

            let total: usize = readers.into_iter().map(|h| h.join().unwrap()).sum();
            writer.join().unwrap();
            total
        });
    });
    group.finish();
}

/// Measure query THROUGHPUT under concurrency — `N` threads each run `K` join
/// queries against a shared `Arc<TripleStore>`.
///
/// Unlike `concurrent/reads` (one query per thread), this saturates cores: a
/// *single* SPARQL query is single-threaded in Oxigraph, but reads are lock-free,
/// so aggregate throughput (queries/s) scales with core count. This is the
/// realistic way to use all 24 cores — concurrent requests, not one big query.
fn bench_concurrent_throughput(c: &mut Criterion) {
    let store = Arc::new(fresh_store());
    store
        .load_str(&gen_persons_ttl(10_000), RdfFormat::Turtle, None)
        .unwrap();

    let mut group = c.benchmark_group("concurrent/throughput");
    group.sample_size(10);
    group.sampling_mode(SamplingMode::Flat);

    const PER_THREAD: usize = 8;
    let query = "SELECT ?name ?age WHERE { \
                   ?s <http://example.org/name> ?name . \
                   ?s <http://example.org/age>  ?age \
                 }";

    for &threads in &[1usize, 2, 4, 8, 16, 24] {
        group.throughput(Throughput::Elements((threads * PER_THREAD) as u64));
        group.bench_with_input(BenchmarkId::new("threads", threads), &threads, |b, &t| {
            b.iter(|| {
                let handles: Vec<_> = (0..t)
                    .map(|_| {
                        let s = Arc::clone(&store);
                        std::thread::spawn(move || {
                            let mut total = 0usize;
                            for _ in 0..PER_THREAD {
                                total += consume_solutions(s.query(query).unwrap());
                            }
                            total
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().unwrap())
                    .sum::<usize>()
            });
        });
    }
    group.finish();
}

// ─── Criterion registration ───────────────────────────────────────────────────

criterion_group!(
    name = insert;
    config = Criterion::default().sample_size(20);
    targets =
        bench_insert_bulk,
        bench_insert_update,
        bench_insert_update_batch,
        bench_insert_named_graph
);

criterion_group!(
    name = query;
    config = Criterion::default().sample_size(50);
    targets =
        bench_query_lookup,
        bench_query_lookup_limit,
        bench_query_join_2way,
        bench_query_join_3way,
        bench_query_filter,
        bench_query_regex,
        bench_query_optional,
        bench_query_count,
        bench_query_group_by,
        bench_query_group_concat,
        bench_query_transitive_path,
        bench_query_alternative_path,
        bench_query_subquery,
        bench_query_values,
        bench_query_bind,
        bench_query_minus,
        bench_query_not_exists,
        bench_query_construct,
        bench_query_named_graph
);

criterion_group!(
    name = paths;
    config = Criterion::default().sample_size(50);
    targets =
        bench_query_zero_or_more_path,
        bench_query_sequence_path,
        bench_query_inverse_path,
        bench_query_negated_property_set
);

criterion_group!(
    name = update;
    config = Criterion::default().sample_size(20);
    targets =
        bench_update_insert_where,
        bench_update_delete_where
);

criterion_group!(
    name = geosparql;
    config = Criterion::default().sample_size(30);
    targets =
        bench_geosparql_contains,
        bench_geosparql_distance,
        bench_geosparql_intersects,
        bench_geosparql_polygon_complexity,
        bench_geosparql_buffer
);

criterion_group!(
    name = shacl;
    config = Criterion::default().sample_size(20);
    targets =
        bench_shacl_validate_clean,
        bench_shacl_validate_violations
);

criterion_group!(
    name = concurrent;
    config = Criterion::default().sample_size(20);
    targets =
        bench_concurrent_reads,
        bench_concurrent_writes,
        bench_concurrent_mixed,
        bench_concurrent_throughput
);

criterion_main!(insert, query, paths, update, geosparql, shacl, concurrent);
