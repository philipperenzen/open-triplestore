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
//!
//! A second test runs the workloads the first measurement left out
//! (`docs/notes/delta-versioning-design.md` §8.3) on the same data with change
//! capture on: the whole-dataset pipeline run, the write-triggered run that
//! revalidates only what an edit touched, the revalidation after a shape
//! update (which no change log can narrow), and the write gate's sandbox for
//! a 1 000-quad POST. Same knobs, same JSON lines.

mod common;

use std::time::{Duration, Instant};

use common::admin_state_with_store;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::shacl::report::ValidationResult;
use open_triplestore::shacl_studio::exec::execute_pipeline;
use open_triplestore::shacl_studio::gate::{check_write_gates, GateContext, WriteMode};
use open_triplestore::shacl_studio::models::{
    ResultsTarget, SeverityThreshold, ShapeSource, ValidationPipeline, WriteTarget,
};
use open_triplestore::shacl_studio::store::ShaclStudioStore;
use open_triplestore::shacl_studio::trigger::{self, Tick};
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
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

fn secs(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 100.0).round() / 100.0
}

/// Model and shapes, then `assets` assets in 50 000-asset chunks; prints the
/// phase line and returns the store's quad count.
fn load(store: &TripleStore, assets: usize, phase: &str) -> usize {
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
            "phase": phase,
            "assets": assets,
            "quads": total,
            "seconds": secs(t),
            "change_capture": store.changes().enabled(),
            "mirror_max_triples": std::env::var("OTS_PARALLEL_QUERY_MAX_TRIPLES").ok(),
        })
    );
    total
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

/// One test at a time. Both load and validate the same data, and run side by
/// side they compete for the CPU the first one's mirror build needs inside its
/// settle window — on a small CI runner, enough to miss it.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn whole_dataset_validation_on_a_persistent_store() {
    let _serial = serial();
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
    let total = load(&store, assets, "load");

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

fn pipeline(
    shape_graph_id: String,
    trigger_on_write: bool,
    gate_writes: bool,
) -> ValidationPipeline {
    let now = chrono::Utc::now().to_rfc3339();
    ValidationPipeline {
        id: uuid::Uuid::new_v4().to_string(),
        name: if gate_writes { "gate" } else { "on write" }.into(),
        description: None,
        owner_type: OwnerType::User,
        owner_id: "adm".into(),
        visibility: Visibility::Private,
        targets: vec![],
        dataset_ids: vec![],
        graph_iris: vec![MODEL.to_string(), DATA.to_string()],
        target_classes: vec![],
        shape_graph_ids: vec![shape_graph_id],
        severity_threshold: SeverityThreshold::Violation,
        run_inference: false,
        max_results: None,
        inferred_target: WriteTarget::InPlace,
        inferred_target_graph: None,
        results_target: ResultsTarget::None,
        results_target_graph: None,
        trigger_on_write,
        schedule_cron: None,
        gate_writes,
        retention: 10,
        last_run_at: None,
        last_conforms: None,
        created_by: Some("adm".into()),
        created_at: now.clone(),
        updated_at: now,
    }
}

/// A report's results as comparable keys, sorted.
fn keys(results: &[ValidationResult]) -> Vec<(String, Option<String>, String)> {
    let mut k: Vec<_> = results
        .iter()
        .map(|r| {
            (
                r.focus_node.clone(),
                r.path.clone(),
                r.source_constraint.clone(),
            )
        })
        .collect();
    k.sort();
    k
}

/// The workloads of §8.3 the first measurement did not run, on the same data
/// with change capture on:
///
/// 1. the whole-dataset pipeline run (the Studio path, in-process);
/// 2. an edit of a few hundred quads, and the write-triggered run that
///    revalidates only the nodes it touched, merged into the last report —
///    which must say exactly what a full validation says;
/// 3. a shape update (`sh:minInclusive` of one property shape), after which
///    every target is revalidated in full: workload (ii), which no change log
///    narrows;
/// 4. the write gate for a 1 000-quad POST into the instance graph, which
///    copies the whole graph into a sandbox before it validates.
#[tokio::test]
async fn write_triggered_shape_update_and_gate_runs() {
    let _serial = serial();
    let assets = env_usize("OTS_SCALE_ASSETS", 20_000);
    let planted = assets / 10_000;
    assert!(
        planted > 0,
        "OTS_SCALE_ASSETS must be at least 10 000 for the fixture to plant a violation"
    );
    let dir = tempfile::tempdir().unwrap();
    let store = TripleStore::open(dir.path())
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD);
    let total = load(&store, assets, "load_with_capture");
    let (state, _token) = admin_state_with_store(store);
    let store = &state.store;
    let studio = ShaclStudioStore::new(state.auth_db.pool());
    let shapes = studio
        .create_shape_graph(
            "otl",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            SHAPES,
            &[],
            ShapeSource::Manual,
            Some("adm"),
        )
        .unwrap();
    let on_write = pipeline(shapes.id.clone(), true, false);
    studio.insert_pipeline(&on_write).unwrap();
    let tick = || trigger::tick(store, &state.auth_db, &state.base_url);
    // The first tick only takes the trigger's cursor to the log's head.
    assert_eq!(tick(), Tick::default());

    // 1. The whole-dataset pipeline run.
    let t = Instant::now();
    let run = execute_pipeline(
        store,
        &state.auth_db,
        &studio,
        &state.base_url,
        &on_write,
        "manual",
        Some("adm"),
    )
    .unwrap();
    let pipeline_s = secs(t);
    assert_eq!(run.results_count as usize, planted);
    println!(
        "{}",
        json!({ "phase": "pipeline_full", "seconds": pipeline_s, "results": run.results_count })
    );

    // 2. An edit: every planted code fixed, and the status of 100 assets
    //    dropped — 100 new `sh:minCount` violations.
    let codes: Vec<usize> = (0..assets).filter(|i| i % 10_000 == 9_999).collect();
    let dropped: Vec<usize> = (0..100).map(|k| k * (assets / 100) + 1).collect();
    let mut removed = String::new();
    let mut added = String::new();
    for &i in &codes {
        removed.push_str(&format!("<{EX}asset{i}> <{EX}code> \"bad-{i}\" . "));
        added.push_str(&format!("<{EX}asset{i}> <{EX}code> \"AB-{i}\" . "));
    }
    for &i in &dropped {
        let status = ["in-service", "planned", "decommissioned"][i % 3];
        removed.push_str(&format!("<{EX}asset{i}> <{EX}status> \"{status}\" . "));
    }
    let edit = format!(
        "DELETE DATA {{ GRAPH <{DATA}> {{ {removed} }} }} ; INSERT DATA {{ GRAPH <{DATA}> {{ {added} }} }}"
    );
    let t = Instant::now();
    store.update(&edit).unwrap();
    let edit_s = secs(t);
    let t = Instant::now();
    assert_eq!(
        tick(),
        Tick {
            incremental: 1,
            full: 0
        },
        "a small edit of the instances is revalidated incrementally"
    );
    let incremental_s = secs(t);
    let latest = |studio: &ShaclStudioStore| {
        let summary = studio
            .list_pipeline_runs(&on_write.id, 1)
            .unwrap()
            .pop()
            .expect("a run");
        studio
            .get_pipeline_run(&summary.id)
            .unwrap()
            .unwrap()
            .report
            .unwrap()
    };
    let merged = latest(&studio);
    let t = Instant::now();
    let full =
        open_triplestore::shacl::validate(store, SHAPES, &[MODEL.to_string(), DATA.to_string()])
            .unwrap();
    let full_s = secs(t);
    assert_eq!(
        full.results.len(),
        100,
        "the planted codes are fixed, 100 statuses gone"
    );
    assert_eq!(
        keys(&merged.results),
        keys(&full.results),
        "the merged report says exactly what a full validation says"
    );
    println!(
        "{}",
        json!({
            "phase": "trigger_incremental",
            "edit_quads": 2 * codes.len() + dropped.len(),
            "edit_s": edit_s,
            "incremental_s": incremental_s,
            "full_validate_s": full_s,
            "results": merged.results.len(),
        })
    );

    // 3. A shape update: lengths below 2 now violate. Every target is
    //    revalidated, so the run is a full one.
    let t = Instant::now();
    store
        .update(&format!(
            "PREFIX sh: <http://www.w3.org/ns/shacl#> PREFIX ex: <{EX}> \
             DELETE {{ GRAPH <{SHAPES}> {{ ?ps sh:minInclusive 0 }} }} \
             INSERT {{ GRAPH <{SHAPES}> {{ ?ps sh:minInclusive 2 }} }} \
             WHERE {{ GRAPH <{SHAPES}> {{ ?ps sh:path ex:length ; sh:minInclusive 0 }} }}"
        ))
        .unwrap();
    assert_eq!(
        tick(),
        Tick {
            incremental: 0,
            full: 1
        },
        "a shape change revalidates every target"
    );
    let shape_update_s = secs(t);
    // Lengths are (i % 500) / 10 + 1, so below 2 for i % 500 < 10.
    let short = (assets / 500) * 10 + (assets % 500).min(10);
    let after_shape = latest(&studio);
    assert_eq!(after_shape.results.len(), 100 + short);
    println!(
        "{}",
        json!({
            "phase": "shape_update_full",
            "seconds": shape_update_s,
            "results": after_shape.results.len(),
        })
    );

    // 4. The write gate: a 1 000-quad POST into the instances, validated
    //    against the graph it merges into. The graph does not conform (the
    //    shape update above), so the gate refuses — after the full sandbox
    //    cost, which is what is measured.
    let gate = pipeline(shapes.id.clone(), false, true);
    studio.insert_pipeline(&gate).unwrap();
    let post = assets_ttl(assets, 111);
    let t = Instant::now();
    let verdict = check_write_gates(
        GateContext {
            main_store: store,
            auth_db: &state.auth_db,
            studio: &studio,
            base_url: &state.base_url,
            writer: None,
        },
        DATA,
        &post,
        RdfFormat::Turtle,
        WriteMode::Merge,
        None,
    );
    let gate_s = secs(t);
    assert!(verdict.is_err(), "the merged graph does not conform");
    println!(
        "{}",
        json!({ "phase": "gate_sandbox", "post_quads": 111 * 9, "seconds": gate_s })
    );
    println!(
        "{}",
        json!({
            "phase": "summary_change_log",
            "assets": assets,
            "quads": total,
            "pipeline_full_s": pipeline_s,
            "trigger_incremental_s": incremental_s,
            "full_validate_s": full_s,
            "shape_update_s": shape_update_s,
            "gate_sandbox_s": gate_s,
        })
    );
}
