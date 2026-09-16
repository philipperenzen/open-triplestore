//! The 9M-quad SHACL measurement the P2 notes gate the analytical-layer
//! decision on (`docs/notes/analytical-mirror-design.md` §1.5): whole-dataset
//! validation of 1M OTL assets (~9M quads) against the six OTL property
//! shapes, on a persistent store, in the deployment's real configuration.
//!
//! Ignored: it loads nine million quads and runs for minutes. Run it on
//! purpose, with the configuration under test set in the environment:
//!
//! ```text
//! # A — mirror on (needs ~40 GB of container budget, or the override):
//! OTS_PARALLEL_QUERY_MAX_TRIPLES=12000000 cargo test --release --test scale_shacl_9m -- --ignored --nocapture
//! # B — the shipped 4g container (mirror off, run index capped):
//! docker run -m 4g … cargo test --release --test scale_shacl_9m -- --ignored --nocapture
//! ```
//!
//! Knobs: `OTS_SCALE_ASSETS` (default 1 000 000), `OTS_SCALE_SETTLE_SECS`
//! (default 150, the quiet window the 9M mirror needs to publish). The
//! result is one JSON line per phase on stdout, including the report's
//! `metrics` (data source, run index, duration) that the telemetry item
//! added, so a run says which path it measured.

use std::time::{Duration, Instant};

use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use serde_json::json;

const EX: &str = "https://example.org/otl/";
const DATA: &str = "https://example.org/otl/instances";
const MODEL: &str = "https://example.org/otl/model";
const SHAPES: &str = "https://example.org/otl/shapes";

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn model_ttl() -> String {
    let mut s = String::new();
    s.push_str("@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n");
    s.push_str(&format!(
        "<{EX}Asset> a owl:Class ; rdfs:label \"Asset\" .\n"
    ));
    for t in 0..40 {
        s.push_str(&format!(
            "<{EX}Type{t}> a owl:Class ; rdfs:subClassOf <{EX}Asset> ; rdfs:label \"Object type {t}\" .\n"
        ));
    }
    s
}

/// The six OTL property shapes of `examples/scale_otl.rs`.
fn shapes_ttl() -> String {
    format!(
        r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <{EX}> .
ex:AssetShape a sh:NodeShape ; sh:targetClass ex:Asset ;
  sh:property [ sh:path ex:name ; sh:minCount 1 ; sh:maxCount 1 ; sh:datatype xsd:string ] ,
              [ sh:path ex:code ; sh:minCount 1 ; sh:datatype xsd:string ; sh:pattern "^[A-Z]{{2}}-[0-9]+$" ] ,
              [ sh:path ex:length ; sh:minCount 1 ; sh:datatype xsd:decimal ; sh:minInclusive 0 ] ,
              [ sh:path ex:installed ; sh:minCount 1 ; sh:datatype xsd:gYear ] ,
              [ sh:path ex:status ; sh:minCount 1 ; sh:in ( "in-service" "planned" "decommissioned" ) ] ,
              [ sh:path ex:partOf ; sh:maxCount 1 ; sh:class ex:Asset ] .
"#
    )
}

/// `count` assets from `from`, ~9 quads each; every 10 000th asset carries a
/// bad code, so the validator has something to find.
fn assets_ttl(from: usize, count: usize) -> String {
    let mut s = String::with_capacity(count * 420);
    s.push_str(&format!(
        "@prefix ex: <{EX}> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n"
    ));
    for i in from..from + count {
        let t = i % 40;
        let code = if i % 10_000 == 9_999 {
            format!("bad-{i}")
        } else {
            format!("AB-{i}")
        };
        let status = ["in-service", "planned", "decommissioned"][i % 3];
        let parent = if i > 0 {
            format!(" ; ex:partOf ex:asset{}", i / 10)
        } else {
            String::new()
        };
        s.push_str(&format!(
            "ex:asset{i} a ex:Type{t}, ex:Asset ; ex:name \"Asset {i}\" ; ex:code \"{code}\" ; ex:length \"{}\"^^xsd:decimal ; ex:installed \"{}\"^^xsd:gYear ; ex:status \"{status}\" ; ex:location \"POINT({} {})\"{parent} .\n",
            (i % 500) as f64 / 10.0 + 1.0,
            1950 + (i % 76),
            4.0 + (i % 1000) as f64 / 1000.0,
            51.0 + (i % 777) as f64 / 1000.0,
        ));
    }
    s
}

fn validate(store: &TripleStore, label: &str) -> f64 {
    let t = Instant::now();
    let report =
        open_triplestore::shacl::validate(store, SHAPES, &[MODEL.to_string(), DATA.to_string()])
            .expect("validation runs");
    let secs = t.elapsed().as_secs_f64();
    let m = report.metrics.clone().unwrap_or_default();
    println!(
        "{}",
        json!({
            "phase": label,
            "seconds": (secs * 100.0).round() / 100.0,
            "conforms": report.conforms,
            "results": report.results.len(),
            "source": m.source,
            "run_index": m.run_index,
            "quads": m.quads,
            "graphs": m.graphs,
            "engine_ms": m.duration_ms,
        })
    );
    secs
}

#[test]
#[ignore = "loads ~9M quads and runs for minutes; the P2 §1.5 measurement, run on purpose"]
fn whole_dataset_validation_at_nine_million_quads() {
    let assets = env_usize("OTS_SCALE_ASSETS", 1_000_000);
    let settle = env_usize("OTS_SCALE_SETTLE_SECS", 150);
    let dir = tempfile::tempdir().unwrap();
    let store = TripleStore::open(dir.path()).unwrap();

    // 1. Load: model and shapes, then the assets in 50 000-asset chunks.
    let t = Instant::now();
    store
        .load_str(&model_ttl(), RdfFormat::Turtle, Some(MODEL))
        .unwrap();
    store
        .load_str(&shapes_ttl(), RdfFormat::Turtle, Some(SHAPES))
        .unwrap();
    let chunk = 50_000;
    let mut from = 0;
    while from < assets {
        let n = chunk.min(assets - from);
        store
            .load_str(&assets_ttl(from, n), RdfFormat::Turtle, Some(DATA))
            .unwrap();
        from += n;
    }
    let total = store.len().unwrap();
    println!(
        "{}",
        json!({
            "phase": "load",
            "assets": assets,
            "quads": total,
            "seconds": (t.elapsed().as_secs_f64() * 100.0).round() / 100.0,
            "mirror_max_triples": std::env::var("OTS_PARALLEL_QUERY_MAX_TRIPLES").ok(),
        })
    );

    // 2. Settle. The mirror rebuilds on the first query after writes go
    //    quiet (the server also pokes it from a periodic task; a test has
    //    to issue the query itself), on a background thread; wait for the
    //    full copy to be published, up to `settle` seconds. Configuration B
    //    (over the cap) never publishes and runs the timeout out.
    let t = Instant::now();
    std::thread::sleep(Duration::from_secs(10));
    let _ = store.query("SELECT ?s WHERE { ?s ?p ?o } LIMIT 1");
    while store.mirror_full_copy().is_none() && t.elapsed() < Duration::from_secs(settle as u64) {
        std::thread::sleep(Duration::from_secs(5));
        let _ = store.query("SELECT ?s WHERE { ?s ?p ?o } LIMIT 1");
    }
    println!(
        "{}",
        json!({
            "phase": "settled",
            "seconds": (t.elapsed().as_secs_f64() * 100.0).round() / 100.0,
            "mirror_builds": store.parallel_build_count(),
            "mirror_full_copy_published": store.mirror_full_copy().is_some(),
        })
    );

    // 3. Whole-dataset validation, twice (the second on warm caches).
    let a1 = validate(&store, "validate_1");
    let a2 = validate(&store, "validate_2");

    // 4. A 500-quad write, then validation straight away: the mirror is dirty,
    //    so this is the path a pipeline run after an edit takes.
    let mut ins = String::from("INSERT DATA { GRAPH <");
    ins.push_str(DATA);
    ins.push_str("> { ");
    for i in 0..500 {
        ins.push_str(&format!(
            "<{EX}asset{}> <{EX}note> \"edit {i}\" . ",
            assets + i
        ));
    }
    ins.push_str("} }");
    store.update(&ins).unwrap();
    let c = validate(&store, "validate_after_write");
    println!(
        "{}",
        json!({
            "phase": "summary",
            "assets": assets,
            "quads": total,
            "validate_1_s": (a1 * 100.0).round() / 100.0,
            "validate_2_s": (a2 * 100.0).round() / 100.0,
            "after_write_s": (c * 100.0).round() / 100.0,
        })
    );
}
