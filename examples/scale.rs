//! Large-scale ("extra large") benchmark harness: 1M–100M triples.
//!
//! At these sizes an in-memory store would exhaust RAM, so this uses the
//! persistent (RocksDB) backend and streams the dataset from a file. It reports
//! wall-clock for the load and for representative query shapes, so the doc's
//! extra-large tier is measured, not extrapolated.
//!
//! Usage: `scale <ntriples_file> <data_dir>`

use open_triplestore::store::TripleStore;
use oxigraph::sparql::QueryResults;
use std::path::Path;
use std::time::Instant;

fn consume(r: QueryResults) -> usize {
    match r {
        QueryResults::Solutions(s) => s.count(),
        QueryResults::Boolean(_) => 1,
        QueryResults::Graph(g) => g.count(),
    }
}

/// Median wall-clock of `runs` timed executions (after one warm-up), in ms.
fn timed(store: &TripleStore, sparql: &str, runs: usize) -> f64 {
    let _ = consume(store.query(sparql).unwrap()); // warm
    let mut ms: Vec<f64> = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        let _ = consume(store.query(sparql).unwrap());
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ms[ms.len() / 2]
}

fn main() {
    let file = std::env::args()
        .nth(1)
        .expect("usage: scale <file> <data_dir>");
    let dir = std::env::args()
        .nth(2)
        .expect("usage: scale <file> <data_dir>");

    let store = TripleStore::open(Path::new(&dir)).expect("open store");
    let t = Instant::now();
    store.load_file(Path::new(&file)).expect("load");
    let load = t.elapsed();
    let total = store.count_graph(None).unwrap();
    let mt_s = (total as f64 / 1e6) / load.as_secs_f64();
    println!(
        "TRIPLES={total} LOAD_s={:.1} LOAD_Mt_s={:.2}",
        load.as_secs_f64(),
        mt_s
    );

    let n = "http://example.org/";
    let q_count = "SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }";
    let q_lookup = &format!("SELECT ?s ?v WHERE {{ ?s <{n}name> ?v }} LIMIT 1000");
    let q_filter =
        &format!("SELECT (COUNT(*) AS ?c) WHERE {{ ?s <{n}age> ?a FILTER(?a >= 40 && ?a < 60) }}");
    let q_group = &format!(
        "SELECT ?t (COUNT(?s) AS ?c) (AVG(?a) AS ?avg) WHERE {{ ?s <{n}type> ?t . ?s <{n}age> ?a }} GROUP BY ?t"
    );

    println!("COUNT_ms={:.3}", timed(&store, q_count, 5));
    println!("LOOKUP1000_ms={:.3}", timed(&store, q_lookup, 5));
    println!("FILTER_COUNT_ms={:.1}", timed(&store, q_filter, 3));
    println!("GROUP_BY_ms={:.1}", timed(&store, q_group, 3));
}
