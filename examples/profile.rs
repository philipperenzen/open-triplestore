//! Profiling harness for `open-triplestore` hot paths.
//!
//! Loads a deterministic dataset once, then runs a chosen operation in a loop so
//! an instrumentation profiler (callgrind) attributes cost to the query/eval path
//! rather than to setup. One op per process so each callgrind run is isolated.
//!
//! Usage: `profile <op> [reps]`   op ∈ count|join|group|filter|distinct|geo|shacl|load
//!
//! Build with debug info, then:
//!   valgrind --tool=callgrind --callgrind-out-file=cg.<op> \
//!     ./target/debug/examples/profile <op> 20

use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

const EX: &str = "http://example.org/";

fn persons_ttl(n: usize) -> String {
    let mut s =
        format!("@prefix ex: <{EX}> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n");
    for i in 0..n {
        let age = 18 + (i % 65);
        let score = (i as f64 * 7.13) % 100.0;
        s.push_str(&format!(
            "ex:p{i} ex:name \"Person {i}\" ; ex:age {age} ; ex:type ex:Type{} ; ex:score {score:.2} ; ex:email \"p{i}@ex.org\" .\n",
            i % 10
        ));
    }
    s
}

fn geo_ttl(n: usize) -> String {
    let mut s =
        format!("@prefix ex: <{EX}> .\n@prefix geo: <http://www.opengis.net/ont/geosparql#> .\n");
    for i in 0..n {
        let lon = (i % 100) as f64 * 0.1;
        let lat = ((i / 100) % 100) as f64 * 0.1;
        s.push_str(&format!(
            "ex:f{i} geo:hasGeometry ex:g{i} .\nex:g{i} geo:asWKT \"POINT({lon:.2} {lat:.2})\"^^geo:wktLiteral .\n"
        ));
    }
    s
}

fn shapes_ttl() -> String {
    String::from(
        "@prefix sh: <http://www.w3.org/ns/shacl#> . @prefix ex: <http://example.org/> .\n\
         ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;\n\
         sh:property [ sh:path ex:name ; sh:minCount 1 ] ;\n\
         sh:property [ sh:path ex:age ; sh:minCount 1 ; sh:minInclusive 0 ] .",
    )
}

fn shacl_persons_ttl(n: usize) -> String {
    let mut s = String::from(
        "@prefix ex: <http://example.org/> . @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n",
    );
    for i in 0..n {
        s.push_str(&format!(
            "ex:p{i} rdf:type ex:Person ; ex:name \"P{i}\" ; ex:age {} .\n",
            18 + i % 65
        ));
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

fn run_query(sparql: &str, reps: usize) {
    let store = TripleStore::in_memory().unwrap();
    // 30k persons = 150k triples — enough to make the hot functions dominate while
    // keeping each (slow) callgrind run to ~1–2 min on a debug build.
    store
        .load_str(&persons_ttl(30_000), RdfFormat::Turtle, None)
        .unwrap();
    let mut acc = 0usize;
    for _ in 0..reps {
        acc = acc.wrapping_add(consume(store.query(sparql).unwrap()));
    }
    eprintln!("rows~{acc}");
}

fn main() {
    let op = std::env::args().nth(1).unwrap_or_else(|| "count".into());
    let reps: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    match op.as_str() {
        "count" => run_query("SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }", reps),
        "join" => run_query(
            "SELECT ?n ?a WHERE { ?s <http://example.org/name> ?n . ?s <http://example.org/age> ?a }",
            reps,
        ),
        "group" => run_query(
            "SELECT ?t (COUNT(?s) AS ?c) (AVG(?a) AS ?avg) WHERE { ?s <http://example.org/type> ?t . ?s <http://example.org/age> ?a } GROUP BY ?t",
            reps,
        ),
        "filter" => run_query(
            "SELECT (COUNT(*) AS ?c) WHERE { ?s <http://example.org/age> ?a FILTER(?a >= 40 && ?a < 60) }",
            reps,
        ),
        "distinct" => run_query(
            "SELECT (COUNT(DISTINCT ?t) AS ?c) WHERE { ?s <http://example.org/type> ?t }",
            reps,
        ),
        "geo" => {
            let store = TripleStore::in_memory().unwrap();
            store.load_str(&geo_ttl(2_000), RdfFormat::Turtle, None).unwrap();
            let q = "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
                     PREFIX geof: <http://www.opengis.net/def/function/geosparql/>\n\
                     SELECT ?f WHERE { ?f geo:hasGeometry/geo:asWKT ?w \
                       FILTER(geof:sfIntersects(\"POLYGON((0 0,5 0,5 5,0 5,0 0))\"^^geo:wktLiteral, ?w)) }";
            let mut acc = 0;
            for _ in 0..reps {
                acc += consume(store.query(q).unwrap());
            }
            eprintln!("rows~{acc}");
        }
        "shacl" => {
            let store = TripleStore::in_memory().unwrap();
            store.load_str(&shacl_persons_ttl(2_000), RdfFormat::Turtle, Some("urn:data")).unwrap();
            store.load_str(&shapes_ttl(), RdfFormat::Turtle, Some("urn:shapes")).unwrap();
            let dg = vec!["urn:data".to_string()];
            for _ in 0..reps {
                let rep = open_triplestore::shacl::validate(&store, "urn:shapes", &dg).unwrap();
                std::hint::black_box(rep.conforms);
            }
        }
        "load" => {
            let ttl = persons_ttl(50_000);
            for _ in 0..reps {
                let store = TripleStore::in_memory().unwrap();
                store.load_str(&ttl, RdfFormat::Turtle, None).unwrap();
                std::hint::black_box(store.len().unwrap());
            }
        }
        other => {
            eprintln!("unknown op: {other}");
            std::process::exit(2);
        }
    }
}
