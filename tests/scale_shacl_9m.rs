//! The 9M-quad SHACL measurement the P2 notes gate the analytical-layer
//! decision on (`docs/notes/analytical-mirror-design.md` §1.5): whole-dataset
//! validation of 1M OTL assets (~9M quads) against the six OTL property
//! shapes, on a persistent store, in the deployment's real configuration.
//!
//! **It runs by default at a size that fits a test run** — 20 000 assets,
//! about 180 000 quads — and asserts what the measurement is for: the load
//! lands, the mirror publishes, whole-dataset validation finds exactly the
//! violations the fixture plants, twice, and a validation straight after a
//! write still does. That is the path a pipeline run takes after an edit, and
//! nothing else covers it end to end on a persistent store.
//!
//! **The 9M measurement is the same harness with the size turned up.** It
//! loads nine million quads and runs for minutes, so it is run on purpose,
//! with the configuration under test in the environment:
//!
//! ```text
//! # A — mirror on (needs ~40 GB of container budget, or the override):
//! OTS_SCALE_ASSETS=1000000 OTS_SCALE_SETTLE_SECS=150 OTS_PARALLEL_QUERY_MAX_TRIPLES=12000000 \
//!   cargo test --release --test scale_shacl_9m -- --nocapture
//! # B — the shipped 4g container (mirror off, run index capped):
//! docker run -m 4g … OTS_SCALE_ASSETS=1000000 OTS_SCALE_SETTLE_SECS=150 \
//!   cargo test --release --test scale_shacl_9m -- --nocapture
//! ```
//!
//! Knobs: `OTS_SCALE_ASSETS` (default 20 000), `OTS_SCALE_SETTLE_SECS`
//! (default 60; the 9M mirror needs about 150). Every phase still prints one
//! JSON line on stdout, including the report's `metrics` (data source, run
//! index, duration) that the telemetry item added, so a run says which path it
//! measured — the measurement is unchanged, only its default size and the
//! assertions around it are new.
//!
//! At the 9M size configuration B never publishes a mirror, on purpose: the
//! store is over the cap. The assertions account for that — the mirror is only
//! required to publish when the data fits under the configured cap.

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

fn validate(store: &TripleStore, label: &str) -> (f64, usize, bool) {
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
    (secs, report.results.len(), report.conforms)
}

#[test]
fn whole_dataset_validation_on_a_persistent_store() {
    let assets = env_usize("OTS_SCALE_ASSETS", 20_000);
    let settle = env_usize("OTS_SCALE_SETTLE_SECS", 60);
    // The fixture plants one bad code every 10 000 assets.
    let planted = assets / 10_000;
    assert!(
        planted > 0,
        "OTS_SCALE_ASSETS must be at least 10 000 for the fixture to plant a violation"
    );
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
    let mut probe = 0usize;
    while store.mirror_full_copy().is_none() && t.elapsed() < Duration::from_secs(settle as u64) {
        std::thread::sleep(Duration::from_millis(200));
        // Each probe has to be a *new* query. The result cache answers a repeat
        // without reaching the mirror at all, so polling with one fixed query
        // builds the mirror at most once — and never, if that first probe lands
        // inside the post-write quiet window and is then served from the cache.
        probe += 1;
        let _ = store.query(&format!("SELECT ?s WHERE {{ ?s ?p ?o }} LIMIT 1 # {probe}"));
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

    // The load landed: every asset carries eight triples plus the optional
    // `partOf`, so this is a floor rather than an equality.
    assert!(
        total >= assets * 8,
        "expected at least {} quads from {assets} assets, got {total}",
        assets * 8
    );
    // The mirror publishes whenever the data fits under the cap. At the 9M
    // size in the shipped container it does not, and that is the measurement.
    let cap = std::env::var("OTS_PARALLEL_QUERY_MAX_TRIPLES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2_000_000);
    if total <= cap {
        assert!(
            store.mirror_full_copy().is_some(),
            "{total} quads is under the {cap} cap, so the mirror should have published \
             within {settle}s"
        );
    }

    // 3. Whole-dataset validation, twice (the second on warm caches).
    let (a1, n1, conforms1) = validate(&store, "validate_1");
    let (a2, n2, _) = validate(&store, "validate_2");
    assert_eq!(
        n1, planted,
        "the fixture plants one bad code per 10 000 assets"
    );
    assert!(
        !conforms1,
        "with {planted} planted violations it cannot conform"
    );
    assert_eq!(n2, n1, "a second run over unchanged data must agree");

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
    let (c, n3, _) = validate(&store, "validate_after_write");
    // The 500 added triples use a predicate no shape constrains, so the
    // violation count is unchanged — but the run has to see the new data
    // rather than a stale mirror, which is the path this phase exists for.
    assert_eq!(
        n3, planted,
        "a validation straight after a write must see the store as it now is"
    );
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
