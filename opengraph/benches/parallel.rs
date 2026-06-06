//! Benchmark: subject-sharded parallel execution vs single-store evaluation.
//!
//! Demonstrates the multi-core speedup of `opengraph::parallel::ParallelStore`
//! for shard-decomposable queries (global COUNT, subject-star COUNT, filtered
//! COUNT) against a single Oxigraph store (the current `TripleStore` behaviour:
//! one query → one core).
//!
//! Run:  cargo bench --bench parallel
//!       cargo bench --bench parallel -- count_star

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, SamplingMode};
use opengraph::oxigraph::store::Store;
use opengraph::oxrdf::vocab::xsd;
use opengraph::oxrdf::{GraphName, Literal, NamedNode, Quad, Subject, Term};
use opengraph::parallel::ParallelStore;

fn iri(s: &str) -> NamedNode {
    NamedNode::new(s).unwrap()
}

/// `n` persons × 3 properties (name/age/type) — same shape as the criterion suite.
fn persons(n: usize) -> Vec<Quad> {
    let ex = "http://example.org/";
    let mut q = Vec::with_capacity(n * 3);
    for i in 0..n {
        let s = Subject::NamedNode(iri(&format!("{ex}p{i}")));
        q.push(Quad::new(
            s.clone(),
            iri(&format!("{ex}name")),
            Term::Literal(Literal::new_simple_literal(format!("Person {i}"))),
            GraphName::DefaultGraph,
        ));
        q.push(Quad::new(
            s.clone(),
            iri(&format!("{ex}age")),
            Term::Literal(Literal::new_typed_literal(
                (18 + i % 65).to_string(),
                xsd::INTEGER,
            )),
            GraphName::DefaultGraph,
        ));
        q.push(Quad::new(
            s,
            iri(&format!("{ex}type")),
            Term::NamedNode(iri(&format!("{ex}Type{}", i % 10))),
            GraphName::DefaultGraph,
        ));
    }
    q
}

fn single_count(store: &Store, sparql: &str) -> usize {
    match store.query(sparql).unwrap() {
        opengraph::oxigraph::sparql::QueryResults::Solutions(s) => s.count(),
        _ => 0,
    }
}

fn bench_query(c: &mut Criterion, label: &str, sparql: &str, n_persons: usize) {
    let quads = persons(n_persons);

    let single = Store::new().unwrap();
    single
        .bulk_loader()
        .load_quads(quads.iter().cloned())
        .unwrap();

    let mut group = c.benchmark_group(label);
    group.sample_size(20);
    group.sampling_mode(SamplingMode::Flat);

    group.bench_function(BenchmarkId::new("shards", 1), |b| {
        b.iter(|| single_count(&single, sparql))
    });

    for &n in &[2usize, 4, 8, 16] {
        let ps = ParallelStore::new(n);
        ps.load_quads(quads.iter().cloned()).unwrap();
        group.bench_with_input(BenchmarkId::new("shards", n), &ps, |b, ps| {
            b.iter(|| ps.query(sparql).unwrap().unwrap().len())
        });
    }
    group.finish();
}

fn count_star(c: &mut Criterion) {
    bench_query(
        c,
        "parallel/count_star",
        "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }",
        200_000,
    );
}

fn join_count(c: &mut Criterion) {
    bench_query(
        c,
        "parallel/join_count",
        "SELECT (COUNT(*) AS ?c) WHERE { ?s <http://example.org/name> ?n . ?s <http://example.org/age> ?a }",
        200_000,
    );
}

fn filter_count(c: &mut Criterion) {
    bench_query(
        c,
        "parallel/filter_count",
        "SELECT (COUNT(*) AS ?c) WHERE { ?s <http://example.org/age> ?a FILTER(?a >= 40 && ?a < 60) }",
        200_000,
    );
}

criterion_group!(
    name = parallel;
    config = Criterion::default().sample_size(20);
    targets = count_star, join_count, filter_count
);
criterion_main!(parallel);
