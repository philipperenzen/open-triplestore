//! Live-path multi-core measurement: `TripleStore::query()` with the subject-
//! sharded mirror **OFF vs ON**, on ~500k triples, for the aggregate shapes the
//! live `/sparql` path accelerates. Prints a markdown table (median of N paced
//! runs), mirroring the Fuseki-comparison methodology in `docs/performance.md`.
//!
//! This measures the *same* `TripleStore` the HTTP server uses (not the standalone
//! `opengraph::parallel::ParallelStore`), so it is the before/after for the live
//! engine. Run: `cargo bench --bench parallel_live`.

use std::time::Instant;

use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

const EX: &str = "http://example.org/";

/// `n` persons × 3 triples (name/age/type) as N-Triples in the default graph —
/// the same deterministic workload the criterion suite and Fuseki comparison use.
fn gen_persons_nt(n: usize) -> String {
    let mut s = String::with_capacity(n * 160);
    for i in 0..n {
        s.push_str(&format!("<{EX}p{i}> <{EX}name> \"Person {i}\" .\n"));
        s.push_str(&format!(
            "<{EX}p{i}> <{EX}age> \"{}\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
            18 + i % 65
        ));
        s.push_str(&format!("<{EX}p{i}> <{EX}type> <{EX}Type{}> .\n", i % 10));
    }
    s
}

fn consume(r: QueryResults) -> usize {
    match r {
        QueryResults::Solutions(s) => s.count(),
        QueryResults::Boolean(_) => 1,
        QueryResults::Graph(g) => g.count(),
    }
}

/// Median wall-clock (ms) over `runs` timed iterations, after one warm-up call
/// (which builds the mirror so the measured runs hit warm shards).
fn median_ms(store: &TripleStore, q: &str, runs: usize) -> f64 {
    consume(store.query(q).unwrap()); // warm-up (builds the mirror)
    let mut times: Vec<f64> = Vec::with_capacity(runs);
    for _ in 0..runs {
        let t = Instant::now();
        consume(store.query(q).unwrap());
        times.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    times[times.len() / 2]
}

fn build(enabled: bool, shards: usize, data: &str) -> TripleStore {
    let store = TripleStore::in_memory()
        .unwrap()
        .with_parallel_query(enabled, shards, 100_000_000)
        // Measure *engine compute*, not the result cache — otherwise the warm-up call
        // populates the cache and every timed run is a ~0 ms cache hit.
        .with_query_cache(false, 256, 10_000);
    store.load_str(data, RdfFormat::NTriples, None).unwrap();
    store
}

fn main() {
    let n: usize = 167_000; // ~500k triples
    let data = gen_persons_nt(n);
    let shards = std::thread::available_parallelism()
        .map(|x| x.get())
        .unwrap_or(8)
        .min(16);

    let single = build(false, 1, &data);
    let parallel = build(true, shards, &data);

    let queries: &[(&str, String)] = &[
        (
            "2-way join `COUNT`",
            format!("SELECT (COUNT(*) AS ?c) WHERE {{ ?s <{EX}name> ?n . ?s <{EX}age> ?a }}"),
        ),
        (
            "`FILTER` + `COUNT`",
            format!(
                "SELECT (COUNT(*) AS ?c) WHERE {{ ?s <{EX}age> ?a FILTER(?a >= 40 && ?a < 60) }}"
            ),
        ),
        (
            "single-pattern `COUNT`",
            format!("SELECT (COUNT(*) AS ?c) WHERE {{ ?s <{EX}name> ?n }}"),
        ),
        (
            "`GROUP BY` + `COUNT`",
            format!("SELECT ?t (COUNT(*) AS ?c) WHERE {{ ?s <{EX}type> ?t }} GROUP BY ?t"),
        ),
        (
            "`GROUP BY` + `AVG` (join)",
            format!(
                "SELECT ?t (AVG(?a) AS ?v) WHERE {{ ?s <{EX}type> ?t . ?s <{EX}age> ?a }} GROUP BY ?t"
            ),
        ),
        (
            "`COUNT(DISTINCT)`",
            format!("SELECT (COUNT(DISTINCT ?t) AS ?c) WHERE {{ ?s <{EX}type> ?t }}"),
        ),
        (
            "global `AVG`",
            format!("SELECT (AVG(?a) AS ?v) WHERE {{ ?s <{EX}age> ?a }}"),
        ),
    ];

    println!(
        "\n## Live /sparql path — single-core vs {shards}-shard mirror ({} triples)\n",
        single.len().unwrap()
    );
    println!(
        "| Query (~500k triples, in-process) | single-core | {shards}-shard mirror | speedup |"
    );
    println!("|---|--:|--:|--:|");
    for (label, q) in queries {
        let s = median_ms(&single, q, 9);
        let p = median_ms(&parallel, q, 9);
        println!("| {label} | {s:.1} ms | {p:.1} ms | **{:.1}×** |", s / p);
    }
    println!();
}
