//! Scale benchmark at OTL scale: millions of quads of asset-shaped data, deep
//! SHACL over all of it, and concurrent writers next to readers.
//!
//! Generates a synthetic object-type-library dataset in-process (no download):
//! `N` assets, each typed against one of 40 object types with a label, six
//! typed properties, a part-of link and a location literal — ~10 quads per
//! asset — into a persistent (RocksDB) store, then measures:
//!
//! 1. load throughput (quads/s) in batches of 50 000;
//! 2. six query shapes (median of 5 after a warm-up): lookup, 2-way join,
//!    filter, group-by, property path, whole-store count;
//! 3. SHACL validation of every asset against a shape set with datatype,
//!    minCount, class and pattern constraints (six property shapes);
//! 4. a 20-second mixed phase: `W` writer threads inserting 500-quad batches
//!    while `R` reader threads run lookups — throughput and p95 latencies.
//!
//! Usage: `scale_otl <assets> <data_dir> [writers=4] [readers=4] [seconds=20]
//!         [--skip-load] [--shacl-only] [--settle <secs>] [--repair] [--patch]`
//!
//! * `--skip-load` reuses a data dir an earlier run loaded (nothing is
//!   generated or written before the measured phases);
//! * `--shacl-only` runs only the SHACL phase (and `--repair`, `--patch`);
//! * `--settle <secs>` waits, ticking the query accelerator, until its copy
//!   of the store is published or the time is up, so the SHACL phase does
//!   not race a background rebuild; the JSON says which source the run read
//!   (`source`: `mirror`, `snapshot` or `live`, and `run_index`);
//! * `--repair` adds the repair layer's own phase
//!   (`docs/notes/repair-layer-design.md` §5.5): copy the dataset's graphs
//!   into an in-memory sandbox by quad copy, validate the sandbox, then
//!   compile the shapes and run the chase — timing the three spans
//!   separately, which is what one `POST …/repair` with `validate: true`
//!   costs on top of its request handling;
//! * `--patch` times one 1 000-line patch (500 `D`, 500 `A`) applied the
//!   way `POST …/patch` writes it (`update`, which recounts the graph) and
//!   the way a repair proposal's apply writes it (`update_targeted_delta`,
//!   which adjusts the count by the exact delta): median of 5, each apply
//!   undone before the next.
//!
//! Output: one JSON document on stdout (plus progress on stderr), suitable for
//! docs/performance.md. Build in release mode for meaningful numbers:
//!   cargo run --release --example scale_otl -- 100000 /tmp/scale-otl
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

const EX: &str = "https://example.org/otl/";
const DATA: &str = "https://example.org/otl/instances";
const MODEL: &str = "https://example.org/otl/model";
const SHAPES: &str = "https://example.org/otl/shapes";

fn consume(r: QueryResults) -> usize {
    match r {
        QueryResults::Solutions(s) => s.count(),
        QueryResults::Boolean(_) => 1,
        QueryResults::Graph(g) => g.count(),
    }
}

fn median_ms(store: &TripleStore, q: &str, runs: usize) -> (f64, usize) {
    let mut n = consume(store.query(q).expect("warm-up query"));
    let mut ms: Vec<f64> = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        n = consume(store.query(q).expect("query"));
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (ms[ms.len() / 2], n)
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

/// `count` assets starting at `from`, ~10 quads each; every 10 000th asset
/// violates a shape (a bad code), so the validator has something to find.
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

/// The repair layer's phase: seed, validate the sandbox, compile and chase.
fn repair_phase(store: &TripleStore) -> serde_json::Value {
    use open_triplestore::repair::{chase, compile, rules, sandbox, stratify};
    let graphs = vec![DATA.to_string(), MODEL.to_string(), SHAPES.to_string()];
    let t = Instant::now();
    let seeded = sandbox::seed(store, &graphs, Instant::now() + Duration::from_secs(3600))
        .unwrap_or_else(|_| panic!("seeding the sandbox"));
    let seed_s = t.elapsed().as_secs_f64();
    eprintln!("  seed: {seed_s:.1}s, {} quads", seeded.quads);
    let data = vec![DATA.to_string(), MODEL.to_string()];
    let t = Instant::now();
    let report = open_triplestore::shacl::engine::validate(&seeded.store, SHAPES, &data)
        .expect("sandbox shacl");
    let validate_s = t.elapsed().as_secs_f64();
    eprintln!(
        "  sandbox validate: {validate_s:.1}s, {} results",
        report.results.len()
    );
    let t = Instant::now();
    let compiled =
        compile::compile_shapes(&seeded.store, SHAPES, &data, Default::default()).expect("compile");
    let prepared: Vec<rules::PreparedRule> = compiled
        .rules
        .into_iter()
        .map(|r| rules::prepare(r).expect("rule"))
        .collect();
    let strata = stratify::stratify(&prepared).expect("strata");
    let premises = data.clone();
    let targets: std::collections::BTreeSet<String> = [DATA.to_string()].into_iter().collect();
    let prefixes = open_triplestore::repair::run::null_prefixes("http://localhost");
    let outcome = chase::chase(&chase::ChaseInput {
        sandbox: &seeded.store,
        rules: &prepared,
        strata: &strata,
        premises: &premises,
        targets: &targets,
        null_base: "http://localhost",
        null_prefixes: &prefixes,
        focus: None,
        budget: chase::Budget {
            rounds: 100,
            nulls: 10_000,
            ops: 50_000,
            deadline: Instant::now() + Duration::from_secs(3600),
        },
        same_as_seeds: &[],
        semi_naive: true,
    })
    .expect("chase");
    let chase_s = t.elapsed().as_secs_f64();
    eprintln!(
        "  compile + chase: {chase_s:.1}s, {} rules, {} adds, {} rounds",
        prepared.len(),
        outcome.added.len(),
        outcome.rounds
    );
    serde_json::json!({
        "seed_seconds": seed_s,
        "sandbox_quads": seeded.quads,
        "sandbox_validate_seconds": validate_s,
        "sandbox_results": report.results.len(),
        "chase_seconds": chase_s,
        "rules": prepared.len(),
        "report_only": compiled.report_only.len(),
        "adds": outcome.added.len(),
        "deletes": outcome.deleted.len(),
        "rounds": outcome.rounds,
        "evaluations": outcome.evaluations,
    })
}

/// The `--patch` phase: one 1 000-line ground patch, applied as the patch
/// route writes it and as a repair proposal's apply writes it.
fn patch_phase(store: &TripleStore, assets: usize) -> serde_json::Value {
    let n = 500.min(assets);
    let lines = |from: &str, to: &str| -> String {
        let mut d = String::from("DELETE DATA { GRAPH <");
        d.push_str(DATA);
        d.push_str("> {\n");
        let mut a = d.replace("DELETE", "INSERT");
        for i in 0..n {
            d.push_str(&format!(
                "<{EX}asset{i}> <{EX}name> \"Asset {i}{from}\" .\n"
            ));
            a.push_str(&format!("<{EX}asset{i}> <{EX}name> \"Asset {i}{to}\" .\n"));
        }
        format!("{d}}} }} ;\n{a}}} }}")
    };
    let forward = lines("", " (renamed)");
    let back = lines(" (renamed)", "");
    let graphs = vec![DATA.to_string()];
    let mut update_ms = Vec::new();
    let mut delta_ms = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        store.update(&forward).expect("patch via update");
        update_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        store.update(&back).expect("undo");
        let t = Instant::now();
        store
            .update_targeted_delta(&forward, &graphs, false)
            .expect("patch via the exact delta");
        delta_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        store
            .update_targeted_delta(&back, &graphs, false)
            .expect("undo");
    }
    let median = |v: &mut Vec<f64>| {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    };
    let (u, d) = (median(&mut update_ms), median(&mut delta_ms));
    eprintln!("  1000-line patch: update {u:.1} ms, exact delta {d:.1} ms");
    serde_json::json!({ "lines": 2 * n, "update_ms": u, "targeted_delta_ms": d })
}

fn main() {
    let raw: Vec<String> = std::env::args().collect();
    let flag = |name: &str| raw.iter().any(|a| a == name);
    let skip_load = flag("--skip-load");
    let shacl_only = flag("--shacl-only");
    let repair = flag("--repair");
    let patch = flag("--patch");
    let settle: u64 = raw
        .iter()
        .position(|a| a == "--settle")
        .and_then(|i| raw.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let args: Vec<String> = raw
        .iter()
        .enumerate()
        .filter(|(i, a)| !a.starts_with("--") && !(*i > 0 && raw[*i - 1] == "--settle"))
        .map(|(_, a)| a.clone())
        .collect();
    let assets: usize = args
        .get(1)
        .and_then(|a| a.parse().ok())
        .expect("usage: scale_otl <assets> <data_dir> [writers] [readers] [seconds] [--skip-load] [--shacl-only] [--settle <secs>] [--repair] [--patch]");
    let dir = args.get(2).expect("usage: scale_otl <assets> <data_dir>");
    let writers: usize = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(4);
    let readers: usize = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(4);
    let seconds: u64 = args.get(5).and_then(|a| a.parse().ok()).unwrap_or(20);

    let store = TripleStore::open(Path::new(dir)).expect("open store");
    if !skip_load {
        store
            .load_str(&model_ttl(), RdfFormat::Turtle, Some(MODEL))
            .expect("model");
        store
            .load_str(&shapes_ttl(), RdfFormat::Turtle, Some(SHAPES))
            .expect("shapes");
    }

    // Optionally keep the generated data (Turtle, one file) so another store
    // can load the same dataset: SCALE_DUMP=/path/to/otl.ttl
    let mut dump = std::env::var("SCALE_DUMP")
        .ok()
        .map(|p| std::io::BufWriter::new(std::fs::File::create(p).expect("dump file")));
    if let Some(d) = dump.as_mut() {
        use std::io::Write as _;
        d.write_all(model_ttl().as_bytes()).unwrap();
        d.write_all(shapes_ttl().as_bytes()).unwrap();
    }

    // 1. Load.
    eprintln!("loading {assets} assets …");
    let t = Instant::now();
    let batch = 50_000;
    let mut done = if skip_load { assets } else { 0 };
    while done < assets {
        let n = batch.min(assets - done);
        let ttl = assets_ttl(done, n);
        if let Some(d) = dump.as_mut() {
            use std::io::Write as _;
            d.write_all(ttl.as_bytes()).unwrap();
        }
        store
            .load_str(&ttl, RdfFormat::Turtle, Some(DATA))
            .expect("load batch");
        done += n;
        if done % 500_000 == 0 || done == assets {
            eprintln!("  {done} assets, {:.1}s", t.elapsed().as_secs_f64());
        }
    }
    let load_s = t.elapsed().as_secs_f64();
    let quads = store.count_graph(Some(DATA)).unwrap_or(0);
    let load_rate = quads as f64 / load_s;

    // Settle: let the accelerator publish its copy before anything reads.
    if settle > 0 {
        let deadline = Instant::now() + Duration::from_secs(settle);
        while Instant::now() < deadline && store.mirror_full_copy().is_none() {
            store.accelerator_tick();
            std::thread::sleep(Duration::from_millis(500));
        }
        eprintln!(
            "settled: mirror {}",
            if store.mirror_full_copy().is_some() {
                "published"
            } else {
                "not published"
            }
        );
    }

    // 2. Queries.
    eprintln!("queries …");
    let probe = assets / 2;
    let queries = [
        ("lookup", format!("SELECT ?p ?o WHERE {{ GRAPH <{DATA}> {{ <{EX}asset{probe}> ?p ?o }} }}")),
        ("join_2way", format!("SELECT ?a ?pn WHERE {{ GRAPH <{DATA}> {{ ?a <{EX}partOf> ?p . ?p <{EX}name> ?pn }} }} LIMIT 10000")),
        ("filter", format!("SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{DATA}> {{ ?a <{EX}length> ?l FILTER(?l > 40.0 && ?l < 42.0) }} }}")),
        ("group_by", format!("SELECT ?t (COUNT(?a) AS ?c) (AVG(?l) AS ?avg) WHERE {{ GRAPH <{DATA}> {{ ?a a ?t ; <{EX}length> ?l }} }} GROUP BY ?t")),
        ("path", format!("SELECT (COUNT(?anc) AS ?c) WHERE {{ GRAPH <{DATA}> {{ <{EX}asset{probe}> <{EX}partOf>+ ?anc }} }}")),
        ("count_all", format!("SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{DATA}> {{ ?s ?p ?o }} }}")),
    ];
    let mut qres = serde_json::Map::new();
    for (name, q) in queries.iter().filter(|_| !shacl_only) {
        let (ms, n) = median_ms(&store, q, 5);
        eprintln!("  {name}: {ms:.1} ms ({n} rows)");
        qres.insert(
            name.to_string(),
            serde_json::json!({ "median_ms": ms, "rows": n }),
        );
    }

    // 3. SHACL over everything.
    eprintln!("SHACL …");
    let t = Instant::now();
    let report = open_triplestore::shacl::engine::validate(&store, SHAPES, &[DATA.to_string()])
        .expect("shacl");
    let shacl_s = t.elapsed().as_secs_f64();
    eprintln!(
        "  {:.1}s, conforms={}, results={}",
        shacl_s,
        report.conforms,
        report.results.len()
    );
    let (shacl_source, shacl_index) = report
        .metrics
        .as_ref()
        .map(|m| (m.source.clone(), m.run_index))
        .unwrap_or_default();
    let repair_json = if repair {
        eprintln!("repair sandbox …");
        repair_phase(&store)
    } else {
        serde_json::Value::Null
    };
    let patch_json = if patch {
        eprintln!("patch …");
        patch_phase(&store, assets)
    } else {
        serde_json::Value::Null
    };
    if shacl_only {
        let out = serde_json::json!({
            "assets": assets,
            "quads_loaded": quads,
            "shacl": { "seconds": shacl_s, "conforms": report.conforms, "results": report.results.len(), "quads_validated": quads, "source": shacl_source, "run_index": shacl_index },
            "repair": repair_json,
            "patch": patch_json,
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return;
    }

    // 4. Concurrent writers + readers.
    eprintln!("mixed phase: {writers} writers, {readers} readers, {seconds}s …");
    let store = Arc::new(store);
    let stop = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let write_lat = Arc::new(Mutex::new(Vec::<f64>::new()));
    let read_lat = Arc::new(Mutex::new(Vec::<f64>::new()));
    let mut handles = Vec::new();
    for w in 0..writers {
        let (store, stop, written, lat) = (
            store.clone(),
            stop.clone(),
            written.clone(),
            write_lat.clone(),
        );
        handles.push(std::thread::spawn(move || {
            let mut i = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let from = 10_000_000 + w * 1_000_000 + i * 50;
                let ttl = assets_ttl(from, 50);
                let t = Instant::now();
                store
                    .load_str(&ttl, RdfFormat::Turtle, Some(DATA))
                    .expect("concurrent write");
                lat.lock().unwrap().push(t.elapsed().as_secs_f64() * 1000.0);
                written.fetch_add(500, Ordering::Relaxed);
                i += 1;
            }
        }));
    }
    for r in 0..readers {
        let (store, stop, reads, lat) =
            (store.clone(), stop.clone(), reads.clone(), read_lat.clone());
        handles.push(std::thread::spawn(move || {
            let mut i = r;
            while !stop.load(Ordering::Relaxed) {
                let id = (i * 7919) % assets.max(1);
                let q =
                    format!("SELECT ?p ?o WHERE {{ GRAPH <{DATA}> {{ <{EX}asset{id}> ?p ?o }} }}");
                let t = Instant::now();
                let _ = consume(store.query(&q).expect("concurrent read"));
                lat.lock().unwrap().push(t.elapsed().as_secs_f64() * 1000.0);
                reads.fetch_add(1, Ordering::Relaxed);
                i += 1;
            }
        }));
    }
    std::thread::sleep(Duration::from_secs(seconds));
    stop.store(true, Ordering::Relaxed);
    for h in handles {
        h.join().unwrap();
    }
    let p95 = |v: &Mutex<Vec<f64>>| -> f64 {
        let mut v = v.lock().unwrap().clone();
        if v.is_empty() {
            return 0.0;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[(v.len() as f64 * 0.95) as usize % v.len()]
    };
    let out = serde_json::json!({
        "assets": assets,
        "quads_loaded": quads,
        "load": { "seconds": load_s, "quads_per_s": load_rate },
        "queries": qres,
        "shacl": { "seconds": shacl_s, "conforms": report.conforms, "results": report.results.len(), "quads_validated": quads, "source": shacl_source, "run_index": shacl_index },
        "repair": repair_json,
        "patch": patch_json,
        "mixed": {
            "seconds": seconds, "writers": writers, "readers": readers,
            "quads_written": written.load(Ordering::Relaxed),
            "quads_written_per_s": written.load(Ordering::Relaxed) as f64 / seconds as f64,
            "reads": reads.load(Ordering::Relaxed),
            "reads_per_s": reads.load(Ordering::Relaxed) as f64 / seconds as f64,
            "write_p95_ms": p95(&write_lat), "read_p95_ms": p95(&read_lat),
        },
        "store_quads_final": store.len().unwrap_or(0),
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
