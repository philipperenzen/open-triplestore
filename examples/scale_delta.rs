//! Delta-versions measurements: the list of
//! `docs/notes/delta-versioning-design.md` §8.2, taken on the data of
//! `examples/scale_otl.rs` (the same generator, included below) in a
//! persistent (RocksDB) store.
//!
//! One command per measurement; each prints one JSON document on stdout and
//! its progress on stderr. Change capture and its caps come from the
//! environment exactly as in the server (`OTS_CHANGE_CAPTURE_ENABLED`,
//! `OTS_CHANGE_CAPTURE_MAX_SCAN`, `OTS_CHANGE_CAPTURE_MAX_PAYLOAD`,
//! `OTS_APPLY_DELTA_TX_MAX_QUADS`, `OTS_LDES_BNODE_DEPTH`, …). A command that
//! compares capture off with capture on opens the store with capture on and
//! runs its "off" side on a clone whose log is switched off
//! (`with_change_capture_disabled`): both sides share one RocksDB, one
//! process and one build, and they alternate run by run.
//!
//! ```text
//! scale_delta load <dir> <graph> <from> <assets> [batch=50000] [--with-model]
//! scale_delta scan <dir> <graph> <from> <assets> <reps>          §8.2 item 1
//! scale_delta probe <dir> <quads> <reps>                         item 2
//! scale_delta put <dir> <assets> <reps>                          item 4
//! scale_delta txrss <dir> <quads> <atomic|chunked> <on|off>      item 5, one process per size
//! scale_delta restore <dir> <graph> <from> <assets> <k> <reps> [--skip-builds] [--skip-tier2]  item 5
//! scale_delta census <dir-or-file>...                            item 6
//! scale_delta annotations <dir> <assets> <batch> <off|minimal|full>  item 6
//! scale_delta crash <dir> <iterations> <reset-every>             item 7 (kills a child with SIGKILL)
//! scale_delta payload <dir> <reps> <quads>...                    item 9
//! scale_delta bulk <dir> <assets> <off|on>                       item 9, one process per side
//! scale_delta coldread <dir> <vgraph> <vfrom> <vassets> <ograph> <ofrom> <oassets> <k> <secs>  item 10
//! scale_delta ldes <dir> <assets> <depth> <changes> <reps>       item 11
//! ```
//!
//! `graph` is an IRI or one of the short names `data` (the OTL instance
//! graph) and `other`. Asset `from` offsets keep the IRIs of different graphs
//! apart. Item 3 (the mixed phase) is `scale_otl` itself, run with capture
//! off and on.
//!
//! Build in release mode, with `test-utils` (the provenance and LDES
//! commands drive the real consumers through a test `AppState`):
//!   cargo build --release --features full,test-utils --example scale_delta

#[path = "scale_otl.rs"]
#[allow(dead_code)]
mod otl;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{BufRead, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use open_triplestore::dataset_versions::models::{DatasetVersion, VersionStatus};
use open_triplestore::dataset_versions::{cut, materialise, registry, snapshot};
use open_triplestore::store::changes::{self, WriteContext, WriteContextGuard};
use open_triplestore::store::set_hash::SetHash;
use open_triplestore::store::{QuadOp, TripleStore};
use otl::{assets_ttl, model_ttl, shapes_ttl, DATA, EX, MODEL, SHAPES};
use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::{
    GraphName, GraphNameRef, Literal, NamedNode, NamedNodeRef, NamedOrBlankNode, Quad, Term,
};
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};

const OTHER: &str = "https://example.org/otl/other";
const BASE_URL: &str = "http://localhost";

// ─── small helpers ──────────────────────────────────────────────────────────

fn graph_arg(s: &str) -> String {
    match s {
        "data" => DATA.to_string(),
        "other" => OTHER.to_string(),
        iri => iri.to_string(),
    }
}

fn arg<T: std::str::FromStr>(args: &[String], i: usize, what: &str) -> T {
    args.get(i)
        .and_then(|a| a.parse().ok())
        .unwrap_or_else(|| panic!("missing or invalid argument {i}: {what}"))
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let out = f();
    (out, ms(t.elapsed()))
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

fn percentile(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let i = ((s.len() as f64 * p).ceil() as usize).clamp(1, s.len()) - 1;
    s[i]
}

/// Median, spread and every value, in milliseconds.
fn summary(v: &[f64]) -> Value {
    let min = v.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = v.iter().cloned().fold(0.0, f64::max);
    json!({
        "n": v.len(),
        "median_ms": round(median(v)),
        "min_ms": round(if v.is_empty() { 0.0 } else { min }),
        "max_ms": round(max),
        "values_ms": v.iter().map(|x| round(*x)).collect::<Vec<_>>(),
    })
}

fn round(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Resident set size of this process (Linux; 0 elsewhere).
fn rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map(|pages| pages * 4096)
        .unwrap_or(0)
}

/// Return freed heap memory to the kernel (glibc; a no-op elsewhere).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim_heap() {
    extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    // SAFETY: glibc's malloc_trim takes no pointers and only releases
    // memory the allocator holds free.
    unsafe {
        malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim_heap() {}

fn mib(b: u64) -> f64 {
    round(b as f64 / (1024.0 * 1024.0))
}

/// Samples the resident set every 2 ms until stopped; the peak is what an
/// operation needed on top of what was resident when it started.
struct PeakRss {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    start: u64,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PeakRss {
    fn start() -> Self {
        // Hand freed heap back to the kernel first: memory an earlier step
        // freed but the allocator kept would otherwise be reused without the
        // resident set growing, and the operation would look free.
        trim_heap();
        let start = rss_bytes();
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(start));
        let (s, p) = (stop.clone(), peak.clone());
        let handle = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                p.fetch_max(rss_bytes(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Self {
            stop,
            peak,
            start,
            handle: Some(handle),
        }
    }

    /// `(resident at start, peak above it)` in bytes.
    fn stop(mut self) -> (u64, u64) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        self.peak.fetch_max(rss_bytes(), Ordering::Relaxed);
        let peak = self.peak.load(Ordering::Relaxed);
        (self.start, peak.saturating_sub(self.start))
    }
}

fn dir_bytes(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else {
        return std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    };
    rd.flatten()
        .map(|e| {
            let path = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => dir_bytes(&path),
                _ => e.metadata().map(|m| m.len()).unwrap_or(0),
            }
        })
        .sum()
}

/// The `OTS_` settings this run read, for the record.
fn env_record() -> Value {
    let keys = [
        "OTS_CHANGE_CAPTURE_ENABLED",
        "OTS_CHANGE_CAPTURE_MAX_SCAN",
        "OTS_CHANGE_CAPTURE_MAX_PAYLOAD",
        "OTS_APPLY_DELTA_TX_MAX_QUADS",
        "OTS_LDES_BNODE_DEPTH",
        "OTS_LDES_BNODE_SUBJECTS_MAX",
        "OTS_PARALLEL_QUERY_ENABLED",
        "OTS_PARALLEL_QUERY_MAX_TRIPLES",
        "OTS_QUERY_CACHE_ENABLED",
        "OTS_MATERIALISATION_MAX_QUADS",
    ];
    let mut m = serde_json::Map::new();
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            m.insert(k.to_string(), json!(v));
        }
    }
    Value::Object(m)
}

fn emit(command: &str, results: Value) {
    let out = json!({
        "command": command,
        "args": std::env::args().skip(1).collect::<Vec<_>>(),
        "env": env_record(),
        "results": results,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}

fn open(dir: &str) -> TripleStore {
    TripleStore::open(Path::new(dir)).expect("open store")
}

/// The capture-off side of a paired measurement: the same store, its log
/// switched off.
fn off_side(on: &TripleStore) -> TripleStore {
    on.clone().with_change_capture_disabled()
}

/// `assets` OTL assets from `from` as quads of `graph`.
fn asset_quads(from: usize, assets: usize, graph: &str) -> Vec<Quad> {
    let ttl = assets_ttl(from, assets);
    parse_into(&ttl, RdfFormat::Turtle, graph)
}

fn parse_into(text: &str, format: RdfFormat, graph: &str) -> Vec<Quad> {
    let g = GraphName::NamedNode(NamedNode::new_unchecked(graph));
    RdfParser::from_format(format)
        .for_reader(text.as_bytes())
        .map(|q| {
            let q = q.expect("generated data parses");
            Quad::new(q.subject, q.predicate, q.object, g.clone())
        })
        .collect()
}

fn status_of(i: usize) -> &'static str {
    ["in-service", "planned", "decommissioned"][i % 3]
}

fn asset_iri(i: usize) -> String {
    format!("{EX}asset{i}")
}

fn lit_quad(s: &str, p: &str, o: &str, g: &str) -> Quad {
    Quad::new(
        NamedNode::new_unchecked(s),
        NamedNode::new_unchecked(p),
        Literal::new_simple_literal(o),
        GraphName::NamedNode(NamedNode::new_unchecked(g)),
    )
}

/// The original value of `field` (status, name or code) of asset `i`, as
/// `assets_ttl` writes it.
fn original(field: &str, i: usize) -> String {
    match field {
        "status" => status_of(i).to_string(),
        "name" => format!("Asset {i}"),
        "code" => {
            if i % 10_000 == 9_999 {
                format!("bad-{i}")
            } else {
                format!("AB-{i}")
            }
        }
        other => panic!("no field {other}"),
    }
}

/// `n` quads changed in `graph`: `n / 2` assets of `ids` get `field`
/// replaced by a marked value — one removal and one addition each. With
/// `back`, the inverse.
fn field_ops(graph: &str, field: &str, ids: &[usize], mark: &str, back: bool) -> Vec<QuadOp> {
    let p = format!("{EX}{field}");
    let mut ops = Vec::with_capacity(ids.len() * 2);
    for &i in ids {
        let s = asset_iri(i);
        let old = lit_quad(&s, &p, &original(field, i), graph);
        let new = lit_quad(&s, &p, &format!("{}{mark}", original(field, i)), graph);
        if back {
            ops.push(QuadOp::Remove(new));
            ops.push(QuadOp::Add(old));
        } else {
            ops.push(QuadOp::Remove(old));
            ops.push(QuadOp::Add(new));
        }
    }
    ops
}

fn changes_db(dir: &str) -> PathBuf {
    Path::new(dir).join("changes").join("changes.db")
}

/// Compressed payload bytes of the row with sequence number `seq`.
fn row_blob_bytes(dir: &str, seq: i64) -> (i64, i64) {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        changes_db(dir),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return (0, 0);
    };
    conn.query_row(
        "SELECT COALESCE(LENGTH(added), 0), COALESCE(LENGTH(removed), 0) FROM changes WHERE seq = ?1",
        [seq],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap_or((0, 0))
}

/// The rows a write made, as `{extent, added_n, removed_n, seq}`.
fn rows_since(store: &TripleStore, seq: i64) -> Vec<Value> {
    store
        .changes()
        .rows_after(seq, 100)
        .iter()
        .map(|r| {
            json!({
                "seq": r.seq,
                "graph": r.graph_iri,
                "extent": format!("{:?}", r.extent).to_lowercase(),
                "state": format!("{:?}", r.state).to_lowercase(),
                "added_n": r.added_n,
                "removed_n": r.removed_n,
            })
        })
        .collect()
}

// ─── load ───────────────────────────────────────────────────────────────────

fn cmd_load(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let graph = graph_arg(&arg::<String>(args, 3, "graph"));
    let from: usize = arg(args, 4, "from");
    let assets: usize = arg(args, 5, "assets");
    let batch: usize = args.get(6).and_then(|a| a.parse().ok()).unwrap_or(50_000);
    let with_model = args.iter().any(|a| a == "--with-model");
    let store = open(&dir);
    if with_model {
        store
            .load_str(&model_ttl(), RdfFormat::Turtle, Some(MODEL))
            .expect("model");
        store
            .load_str(&shapes_ttl(), RdfFormat::Turtle, Some(SHAPES))
            .expect("shapes");
    }
    let t = Instant::now();
    let mut done = 0;
    while done < assets {
        let n = batch.min(assets - done);
        store
            .load_str(&assets_ttl(from + done, n), RdfFormat::Turtle, Some(&graph))
            .expect("load batch");
        done += n;
        if done % 500_000 == 0 || done == assets {
            eprintln!("  {done} assets, {:.1}s", t.elapsed().as_secs_f64());
        }
    }
    let secs = t.elapsed().as_secs_f64();
    let quads = store.graph_count_cached(Some(&graph)).unwrap_or(0);
    store.store().flush().ok();
    emit(
        "load",
        json!({
            "graph": graph, "assets": assets, "quads": quads, "seconds": round(secs),
            "quads_per_s": round(quads as f64 / secs),
            "store_quads": store.graph_counts().iter().map(|(_, n)| n).sum::<usize>(),
            "dir_mib": mib(dir_bytes(Path::new(&dir))),
        }),
    );
}

// ─── item 1: the before-image of a WHERE update ─────────────────────────────

fn cmd_scan(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let graph = graph_arg(&arg::<String>(args, 3, "graph"));
    let from: usize = arg(args, 4, "from");
    let assets: usize = arg(args, 5, "assets");
    let reps: usize = arg(args, 6, "reps");
    let on = open(&dir);
    let off = off_side(&on);
    let n = on.graph_count_cached(Some(&graph)).unwrap_or(0);
    // 100 assets spread over the graph: 100 quads removed, 100 added.
    let ids: Vec<usize> = (0..100).map(|k| from + k * (assets / 100)).collect();
    let values: String = ids
        .iter()
        .map(|i| format!("<{}> ", asset_iri(*i)))
        .collect();
    let pairs: String = ids
        .iter()
        .map(|i| format!("(<{}> \"{}\") ", asset_iri(*i), status_of(*i)))
        .collect();
    let st = format!("{EX}status");
    let fwd = format!(
        "DELETE {{ GRAPH <{graph}> {{ ?a <{st}> ?s }} }} INSERT {{ GRAPH <{graph}> {{ ?a <{st}> \"scan-x\" }} }} \
         WHERE {{ VALUES ?a {{ {values} }} GRAPH <{graph}> {{ ?a <{st}> ?s }} }}"
    );
    let back = format!(
        "DELETE {{ GRAPH <{graph}> {{ ?a <{st}> \"scan-x\" }} }} INSERT {{ GRAPH <{graph}> {{ ?a <{st}> ?o }} }} \
         WHERE {{ VALUES (?a ?o) {{ {pairs} }} GRAPH <{graph}> {{ ?a <{st}> \"scan-x\" }} }}"
    );
    // Warm both paths once.
    off.update(&fwd).unwrap();
    off.update(&back).unwrap();
    let (mut t_off, mut t_on, mut t_back_on) = (Vec::new(), Vec::new(), Vec::new());
    let mut peaks = Vec::new();
    let mut extents = Vec::new();
    for rep in 0..reps {
        let order: [bool; 2] = if rep % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        };
        for capture in order {
            let s = if capture { &on } else { &off };
            let before = on.changes().last_seq();
            let peak = PeakRss::start();
            let (_, t) = timed(|| s.update(&fwd).unwrap());
            let (_, p) = peak.stop();
            let (_, tb) = timed(|| s.update(&back).unwrap());
            if capture {
                t_on.push(t);
                t_back_on.push(tb);
                peaks.push(mib(p));
                extents.push(rows_since(&on, before));
            } else {
                t_off.push(t);
            }
            eprintln!("  rep {rep} capture={capture}: {t:.1} ms");
        }
    }
    // The before-image alone: read the graph, and diff two copies of it.
    let g = NamedNodeRef::new(&graph).unwrap();
    let (pre, collect_ms) = timed(|| {
        on.store()
            .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(g)))
            .flatten()
            .collect::<Vec<Quad>>()
    });
    let post = pre.clone();
    let (_, diff_ms) = timed(|| changes::diff_quads(&pre, &post));
    emit(
        "scan",
        json!({
            "graph": graph, "graph_quads": n, "max_scan": on.changes().max_scan(),
            "changed_quads": 200,
            "off": summary(&t_off), "on": summary(&t_on), "on_undo": summary(&t_back_on),
            "on_peak_rss_mib": peaks,
            "on_rows_first_rep": extents.first(),
            "ratio_on_off": round(median(&t_on) / median(&t_off)),
            "added_ms": round(median(&t_on) - median(&t_off)),
            "before_image_collect_ms": round(collect_ms),
            "diff_ms": round(diff_ms),
        }),
    );
}

// ─── item 2: probes ─────────────────────────────────────────────────────────

fn cmd_probe(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let quads_n: usize = arg(args, 3, "quads");
    let reps: usize = arg(args, 4, "reps");
    let on = open(&dir);
    let off = off_side(&on);
    let g = "https://example.org/otl/probe";
    let assets = quads_n / 9;
    let ttl = assets_ttl(30_000_000, assets);
    let quads = parse_into(&ttl, RdfFormat::Turtle, g);
    eprintln!("probe: {} quads", quads.len());
    let mut res: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut extents: BTreeMap<String, Value> = BTreeMap::new();
    for rep in 0..reps {
        let order = if rep % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        };
        for path in ["bulk_insert_quads", "load_named"] {
            for capture in order {
                let s = if capture { &on } else { &off };
                let before = on.changes().last_seq();
                let batch = quads.clone();
                let (_, t) = timed(|| match path {
                    "bulk_insert_quads" => s.bulk_insert_quads(batch, &[g.to_string()]).unwrap(),
                    _ => s.load_str(&ttl, RdfFormat::Turtle, Some(g)).unwrap(),
                });
                let key = format!("{path}_{}", if capture { "on" } else { "off" });
                if capture {
                    extents.insert(key.clone(), json!(rows_since(&on, before)));
                }
                res.entry(key).or_default().push(t);
                eprintln!("  rep {rep} {path} capture={capture}: {t:.0} ms");
                off.bulk_delete_graphs(&[g]).unwrap();
            }
        }
    }
    let r: serde_json::Map<String, Value> =
        res.iter().map(|(k, v)| (k.clone(), summary(v))).collect();
    emit(
        "probe",
        json!({ "quads": quads.len(), "results": r, "rows": extents }),
    );
}

// ─── item 4: Graph Store PUT replace ────────────────────────────────────────

fn cmd_put(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let assets: usize = arg(args, 3, "assets");
    let reps: usize = arg(args, 4, "reps");
    let on = open(&dir);
    let off = off_side(&on);
    let g = "https://example.org/otl/put";
    let a = assets_ttl(40_000_000, assets);
    // B: every 100th asset's status changed — a 1 % replace.
    let b: String = a
        .lines()
        .enumerate()
        .map(|(n, line)| {
            if n >= 2 && (n - 2) % 100 == 0 {
                line.replacen("ex:status \"", "ex:status \"b-", 1)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let (_, first) = timed(|| on.graph_store_put(Some(g), &a, RdfFormat::Turtle).unwrap());
    eprintln!("put: first PUT (empty target) {first:.0} ms");
    let mut current_a = true;
    let (mut t_off, mut t_on) = (Vec::new(), Vec::new());
    let (mut p_off, mut p_on) = (Vec::new(), Vec::new());
    let mut rows = Vec::new();
    for rep in 0..reps {
        let order = if rep % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        };
        for capture in order {
            let s = if capture { &on } else { &off };
            let body = if current_a { &b } else { &a };
            let before = on.changes().last_seq();
            let peak = PeakRss::start();
            let (_, t) = timed(|| s.graph_store_put(Some(g), body, RdfFormat::Turtle).unwrap());
            let (_, p) = peak.stop();
            current_a = !current_a;
            if capture {
                t_on.push(t);
                p_on.push(mib(p));
                rows.push(json!(rows_since(&on, before)));
            } else {
                t_off.push(t);
                p_off.push(mib(p));
            }
            eprintln!("  rep {rep} capture={capture}: {t:.0} ms");
        }
    }
    off.bulk_delete_graphs(&[g]).unwrap();
    emit(
        "put",
        json!({
            "graph_quads": assets * 9, "max_scan": on.changes().max_scan(),
            "max_payload": on.changes().max_payload(),
            "first_put_empty_target_ms": round(first),
            "off": summary(&t_off), "on": summary(&t_on),
            "off_peak_rss_mib": p_off, "on_peak_rss_mib": p_on,
            "ratio_on_off": round(median(&t_on) / median(&t_off)),
            "on_rows": rows,
        }),
    );
}

// ─── item 5: transaction memory ─────────────────────────────────────────────

fn cmd_txrss(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let n: usize = arg(args, 3, "quads");
    let mode: String = arg(args, 4, "atomic|chunked");
    let side: String = arg(args, 5, "on|off");
    let on = open(&dir);
    let store = if side == "off" {
        off_side(&on)
    } else {
        on.clone()
    };
    let g = "https://example.org/otl/tx";
    let quads = asset_quads(50_000_000, n.div_ceil(9), g);
    let ops: Vec<QuadOp> = quads.into_iter().take(n).map(QuadOp::Add).collect();
    let resident = rss_bytes();
    let peak = PeakRss::start();
    let (r, t) = timed(|| {
        if mode == "chunked" {
            store.apply_quad_ops_chunked(&ops)
        } else {
            store.apply_quad_ops(&ops)
        }
    });
    let (start, above) = peak.stop();
    let inserted = r.expect("apply").0.len();
    eprintln!(
        "txrss {n} {mode} capture={side}: {t:.0} ms, peak +{:.0} MiB",
        mib(above)
    );
    emit(
        "txrss",
        json!({
            "quads": n, "mode": mode, "capture": side, "inserted": inserted,
            "ms": round(t),
            "resident_before_mib": mib(resident), "resident_at_start_mib": mib(start),
            "peak_above_start_mib": mib(above),
            "bytes_per_staged_quad": round(above as f64 / n as f64),
            "tx_max_quads": open_triplestore::store::engine::apply_delta_tx_max_quads(),
        }),
    );
    store.bulk_delete_graphs(&[g]).unwrap();
}

// ─── item 5: restore and materialisation ────────────────────────────────────

fn cut_version(
    store: &TripleStore,
    graph: &str,
    version: &str,
    checkpoint: bool,
) -> DatasetVersion {
    let req = cut::CutRequest {
        dataset_id: "ds-delta".into(),
        version: version.into(),
        source_graphs: vec![graph.to_string()],
        status: VersionStatus::Draft,
        created_by: None,
        notes: None,
        branch: None,
        derived_from: None,
        force_checkpoint: checkpoint,
        shared: Default::default(),
        conforms_to_model: None,
        conforms_to_version: None,
    };
    let c = cut::cut(store, BASE_URL, &req).expect("cut");
    if let Some(reason) = &c.checkpoint_reason {
        eprintln!("  {version}: checkpoint ({reason})");
    }
    registry::get_version(store, BASE_URL, "ds-delta", version).expect("version recorded")
}

/// The net change of `graph` from `anchor` to now, from the change log.
fn net_since(
    store: &TripleStore,
    graph: &str,
    epoch: &str,
    from: i64,
) -> (Vec<Quad>, Vec<Quad>, bool) {
    let log = store.changes();
    let chain = log.chain(&[graph.to_string()], epoch, from, log.head_seq());
    let fold = changes::fold_rows(&chain.rows);
    let net = fold
        .get(&Some(graph.to_string()))
        .cloned()
        .unwrap_or_default();
    (net.added, net.removed, chain.intact)
}

fn cmd_restore(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let graph = graph_arg(&arg::<String>(args, 3, "graph"));
    let from: usize = arg(args, 4, "from");
    let assets: usize = arg(args, 5, "assets");
    let k: usize = arg(args, 6, "k changed quads");
    let reps: usize = arg(args, 7, "reps");
    let skip_tier2 = args.iter().any(|a| a == "--skip-tier2");
    let skip_builds = args.iter().any(|a| a == "--skip-builds");
    let store = open(&dir);
    assert!(
        store.changes().enabled(),
        "restore needs OTS_CHANGE_CAPTURE_ENABLED=on"
    );
    let tag = format!("k{k}-{}", chrono_stamp());
    let half = k / 2;
    let set = |off: usize, n: usize| -> Vec<usize> {
        (0..n).map(|j| from + (off + j * 7) % assets).collect()
    };
    // A, B, C change status, name and code of their assets: disjoint quads.
    let a_ids = set(0, half + 5);
    let b_ids = set(1, half);
    let c_ids = set(2, half + 5);
    let (v1, cut1) = timed(|| cut_version(&store, &graph, &format!("{tag}-v1"), true));
    eprintln!("restore: checkpoint cut {cut1:.0} ms");
    let anchor = v1.anchor.clone().expect("anchored checkpoint");
    let (_, ta) = timed(|| {
        store
            .apply_quad_ops(&field_ops(&graph, "status", &a_ids, "*a", false))
            .unwrap()
    });
    let (v2, cut2) = timed(|| cut_version(&store, &graph, &format!("{tag}-v2"), false));
    store
        .apply_quad_ops(&field_ops(&graph, "name", &b_ids, "*b", false))
        .unwrap();
    let (v3, cut3) = timed(|| cut_version(&store, &graph, &format!("{tag}-v3"), false));
    store
        .apply_quad_ops(&field_ops(&graph, "code", &c_ids, "*c", false))
        .unwrap();
    eprintln!(
        "  apply A {ta:.0} ms; delta cuts {cut2:.0} / {cut3:.0} ms; v2 delta={} v3 delta={}",
        v2.is_delta(),
        v3.is_delta()
    );
    // Builds, both directions: v2 is cheaper from its base (A) than from
    // now (B + C); v3 cheaper from now (C) than from its base (A + B).
    let mut builds: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut build_peaks: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for _ in 0..if skip_builds { 0 } else { reps } {
        for (label, v) in [("forward_v2", &v2), ("backward_v3", &v3)] {
            let peak = PeakRss::start();
            let (r, t) = timed(|| materialise::build(&store, BASE_URL, v));
            let (_, p) = peak.stop();
            r.unwrap_or_else(|e| panic!("build {label}: {e}"));
            builds.entry(label).or_default().push(t);
            build_peaks.entry(label).or_default().push(mib(p));
            eprintln!("  build {label}: {t:.0} ms");
            materialise::release(&store, v, false);
        }
    }
    // Restore to v1. Tier 1: the inverse of the chain since v1, one
    // transaction, then the content hash checked as the handler does.
    let expected = v1.source_map[0].content_hash.clone().unwrap_or_default();
    let (fwd_added, fwd_removed, intact) = net_since(&store, &graph, &anchor.epoch, anchor.seq);
    let inverse_size = fwd_added.len() + fwd_removed.len();
    let (mut tier1, mut tier1_apply, mut tier1_hash, mut tier1_peak) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut tier1_plan = Vec::new();
    let mut chain_after_tier1 = Vec::new();
    for _ in 0..reps {
        let peak = PeakRss::start();
        let t0 = Instant::now();
        // The plan reads the chain as the handler does. A tier-1 restore
        // whose rows exceed the payload cap is recorded as a `counts` row,
        // which closes the chain behind it: later runs then apply the same
        // inverse from the plan of the first.
        let (added, removed, ok) = net_since(&store, &graph, &anchor.epoch, anchor.seq);
        chain_after_tier1.push(ok);
        let (added, removed) = if ok {
            (added, removed)
        } else {
            (fwd_added.clone(), fwd_removed.clone())
        };
        let plan_ms = ms(t0.elapsed());
        tier1_plan.push(plan_ms);
        let (_, apply_ms) = timed(|| {
            store
                .apply_quad_ops(&QuadOp::from_delta(removed, added))
                .unwrap()
        });
        let (hash, hash_ms) =
            timed(|| SetHash::of_graphs(&store, std::slice::from_ref(&graph)).to_hex());
        let total = ms(t0.elapsed());
        let (_, p) = peak.stop();
        assert_eq!(hash, expected, "tier 1 restored the version");
        tier1.push(total);
        tier1_apply.push(apply_ms);
        tier1_hash.push(hash_ms);
        tier1_peak.push(mib(p));
        eprintln!(
            "  tier 1: {total:.0} ms (plan {plan_ms:.0}, apply {apply_ms:.0}, hash {hash_ms:.0})"
        );
        // Back to the changed state for the next run.
        store
            .apply_quad_ops(&QuadOp::from_delta(fwd_added.clone(), fwd_removed.clone()))
            .unwrap();
    }
    // Tier 2: copy v1's snapshot into a staging graph, MOVE it over the
    // live graph (per graph, as the handler does for a physical version).
    let (mut tier2, mut tier2_peak, mut tier2_log) = (Vec::new(), Vec::new(), Vec::new());
    let log_bytes = || dir_bytes(&Path::new(&dir).join("changes"));
    if !skip_tier2 {
        for _ in 0..reps {
            let log_before = log_bytes();
            let peak = PeakRss::start();
            let (_, t) = timed(|| snapshot::restore(&store, &v1.source_map).unwrap());
            let (_, p) = peak.stop();
            tier2_log.push(mib(log_bytes().saturating_sub(log_before)));
            let hash = SetHash::of_graphs(&store, std::slice::from_ref(&graph)).to_hex();
            assert_eq!(hash, expected, "tier 2 restored the version");
            tier2.push(t);
            tier2_peak.push(mib(p));
            eprintln!("  tier 2: {t:.0} ms");
            store
                .apply_quad_ops(&QuadOp::from_delta(fwd_added.clone(), fwd_removed.clone()))
                .unwrap();
        }
    }
    let b: serde_json::Map<String, Value> = builds
        .iter()
        .map(|(k, v)| (k.to_string(), summary(v)))
        .collect();
    let bp: serde_json::Map<String, Value> = build_peaks
        .iter()
        .map(|(k, v)| (k.to_string(), json!(v)))
        .collect();
    emit(
        "restore",
        json!({
            "graph": graph, "graph_quads": store.graph_count_cached(Some(&graph)),
            "k_changed": k, "inverse_ops": inverse_size, "chain_intact": intact,
            "tx_max_quads": open_triplestore::store::engine::apply_delta_tx_max_quads(),
            "cut_checkpoint_ms": round(cut1), "cut_delta_ms": [round(cut2), round(cut3)],
            "v2_is_delta": v2.is_delta(), "v3_is_delta": v3.is_delta(),
            "v2_changes": v2.change_count, "v3_changes": v3.change_count,
            "build": b, "build_peak_rss_mib": bp,
            "tier1_total": summary(&tier1), "tier1_apply": summary(&tier1_apply),
            "tier1_plan": summary(&tier1_plan), "chain_intact_before_each_tier1": chain_after_tier1,
            "max_payload": store.changes().max_payload(),
            "tier1_hash_check": summary(&tier1_hash), "tier1_peak_rss_mib": tier1_peak,
            "tier2_copy_move": summary(&tier2), "tier2_peak_rss_mib": tier2_peak,
            "tier2_change_log_growth_mib": tier2_log,
        }),
    );
}

fn chrono_stamp() -> String {
    format!(
        "{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    )
}

// ─── item 6: blank-node census ──────────────────────────────────────────────

fn census_of(path: &Path) -> Option<Value> {
    let ext = path.extension()?.to_str()?;
    let format = match ext {
        "ttl" => RdfFormat::Turtle,
        "trig" => RdfFormat::TriG,
        "nq" => RdfFormat::NQuads,
        "nt" => RdfFormat::NTriples,
        _ => return None,
    };
    let text = std::fs::read_to_string(path)
        .ok()?
        .replace("{base_url}", BASE_URL);
    let quads: Vec<Quad> = RdfParser::from_format(format)
        .for_reader(text.as_bytes())
        .flatten()
        .collect();
    let first = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
    let rest = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
    let mut bnodes: HashSet<String> = HashSet::new();
    let mut parent: HashMap<String, NamedOrBlankNode> = HashMap::new();
    let (mut subj, mut obj, mut either) = (0usize, 0usize, 0usize);
    let mut list_cells: HashSet<String> = HashSet::new();
    let mut next: HashMap<String, String> = HashMap::new();
    for q in &quads {
        let s_b = matches!(q.subject, NamedOrBlankNode::BlankNode(_));
        let o_b = matches!(q.object, Term::BlankNode(_));
        subj += s_b as usize;
        obj += o_b as usize;
        either += (s_b || o_b) as usize;
        if let NamedOrBlankNode::BlankNode(b) = &q.subject {
            bnodes.insert(b.as_str().to_string());
            if q.predicate.as_str() == first {
                list_cells.insert(b.as_str().to_string());
            }
            if q.predicate.as_str() == rest {
                if let Term::BlankNode(n) = &q.object {
                    next.insert(b.as_str().to_string(), n.as_str().to_string());
                }
            }
        }
        if let Term::BlankNode(b) = &q.object {
            bnodes.insert(b.as_str().to_string());
            parent
                .entry(b.as_str().to_string())
                .or_insert_with(|| q.subject.clone());
        }
    }
    // Steps from each blank node up to the IRI that owns it — what the LDES
    // capture walks (`OTS_LDES_BNODE_DEPTH`); an orphan has no owner.
    let mut max_depth = 0usize;
    let mut orphans = 0usize;
    let mut depth_hist: BTreeMap<usize, usize> = BTreeMap::new();
    for b in &bnodes {
        let mut cur = b.clone();
        let mut steps = 0usize;
        let mut owned = false;
        while steps < 10_000 {
            steps += 1;
            match parent.get(&cur) {
                Some(NamedOrBlankNode::NamedNode(_)) => {
                    owned = true;
                    break;
                }
                Some(NamedOrBlankNode::BlankNode(p)) => cur = p.as_str().to_string(),
                None => break,
            }
        }
        if owned {
            max_depth = max_depth.max(steps);
            *depth_hist.entry(steps).or_default() += 1;
        } else {
            orphans += 1;
        }
    }
    // Longest RDF list.
    let heads: Vec<&String> = list_cells
        .iter()
        .filter(|c| !next.values().any(|n| n == *c))
        .collect();
    let longest = heads
        .iter()
        .map(|h| {
            let mut n = 1;
            let mut cur = (*h).clone();
            while let Some(nx) = next.get(&cur) {
                n += 1;
                cur = nx.clone();
                if n > 100_000 {
                    break;
                }
            }
            n
        })
        .max()
        .unwrap_or(0);
    Some(json!({
        "file": path.display().to_string(),
        "quads": quads.len(),
        "quads_with_bnode": either, "bnode_subject": subj, "bnode_object": obj,
        "share_with_bnode": round(either as f64 / quads.len().max(1) as f64),
        "distinct_bnodes": bnodes.len(), "orphan_bnodes": orphans,
        "max_steps_to_owner": max_depth, "steps_histogram": depth_hist,
        "rdf_list_cells": list_cells.len(), "longest_list": longest,
    }))
}

fn walk(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        entries.sort();
        for e in entries {
            walk(&e, out);
        }
    } else {
        out.push(path.to_path_buf());
    }
}

fn cmd_census(args: &[String]) {
    let mut files = Vec::new();
    for a in &args[2..] {
        walk(Path::new(a), &mut files);
    }
    let rows: Vec<Value> = files.iter().filter_map(|f| census_of(f)).collect();
    let total_q: u64 = rows.iter().map(|r| r["quads"].as_u64().unwrap_or(0)).sum();
    let total_b: u64 = rows
        .iter()
        .map(|r| r["quads_with_bnode"].as_u64().unwrap_or(0))
        .sum();
    let max_steps = rows
        .iter()
        .map(|r| r["max_steps_to_owner"].as_u64().unwrap_or(0))
        .max()
        .unwrap_or(0);
    emit(
        "census",
        json!({
            "files": rows.len(), "quads": total_q, "quads_with_bnode": total_b,
            "share_with_bnode": round(total_b as f64 / total_q.max(1) as f64),
            "max_steps_to_owner": max_steps, "per_file": rows,
        }),
    );
}

// ─── item 6: statement-provenance storage ───────────────────────────────────

fn cmd_annotations(args: &[String]) {
    use open_triplestore::auth::models::{OwnerType, Visibility};
    use open_triplestore::dataset_settings::{self, DatasetSettings, ProvenanceProfile};
    let dir: String = arg(args, 2, "dir");
    let assets: usize = arg(args, 3, "assets");
    let batch: usize = arg(args, 4, "batch");
    let profile = ProvenanceProfile::parse(&arg::<String>(args, 5, "profile")).expect("profile");
    let store = open(&dir);
    assert!(store.changes().enabled(), "annotations needs capture on");
    let state = open_triplestore::server::AppState::test_default_with_store(store.clone());
    let g = "https://example.org/otl/prov-data";
    state
        .auth_db
        .create_dataset(
            "ds-prov",
            "provenance",
            None,
            OwnerType::User,
            "u1",
            Visibility::Private,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("ds-prov", g).unwrap();
    dataset_settings::set(
        &state.auth_db,
        "ds-prov",
        &DatasetSettings {
            skolem: false,
            provenance: profile,
        },
    )
    .unwrap();
    // The first tick puts the consumer's cursor at the head.
    open_triplestore::statement_provenance::tick(&state);
    let t = Instant::now();
    let mut done = 0;
    let mut load_ms = 0.0;
    let mut tick_ms = 0.0;
    let mut annotations = 0usize;
    while done < assets {
        let n = batch.min(assets - done);
        let ttl = assets_ttl(60_000_000 + done, n);
        let (_, t1) = timed(|| store.load_str(&ttl, RdfFormat::Turtle, Some(g)).unwrap());
        load_ms += t1;
        done += n;
        // Drain the log as the consumer loop would.
        loop {
            let (w, t2) = timed(|| open_triplestore::statement_provenance::tick(&state));
            tick_ms += t2;
            annotations += w;
            let cur = store
                .changes()
                .cursor(open_triplestore::statement_provenance::CURSOR)
                .map(|c| c.seq)
                .unwrap_or(0);
            if cur >= store.changes().last_seq() || w == 0 && cur == 0 {
                break;
            }
        }
    }
    let secs = t.elapsed().as_secs_f64();
    store.store().flush().ok();
    let total = dir_bytes(Path::new(&dir));
    let changes = dir_bytes(&Path::new(&dir).join("changes"));
    let (_, opt_ms) = timed(|| store.store().optimize().ok());
    let total_opt = dir_bytes(Path::new(&dir));
    let prov_g = open_triplestore::provenance::statement_provenance_graph("ds-prov");
    emit(
        "annotations",
        json!({
            "profile": profile.as_str(), "assets": assets, "batch_assets": batch,
            "data_quads": store.graph_count_cached(Some(g)),
            "annotation_quads": store.graph_count_cached(Some(&prov_g)).unwrap_or(0),
            "annotation_quads_written": annotations,
            "seconds": round(secs), "load_ms": round(load_ms), "consumer_ms": round(tick_ms),
            "dir_mib": mib(total), "changes_db_mib": mib(changes),
            "rocksdb_mib": mib(total - changes),
            "after_optimize": { "dir_mib": mib(total_opt), "rocksdb_mib": mib(total_opt.saturating_sub(changes)), "optimize_ms": round(opt_ms) },
        }),
    );
}

// ─── item 9: payload serialisation ──────────────────────────────────────────

fn encode(quads: &[Quad]) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut w = RdfSerializer::from_format(RdfFormat::NQuads).for_writer(&mut buf);
    for q in quads {
        w.serialize_quad(q.as_ref()).unwrap();
    }
    w.finish().unwrap();
    buf
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap()
}

fn cmd_payload(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let reps: usize = arg(args, 3, "reps");
    let sizes: Vec<usize> = args[4..].iter().map(|s| s.parse().unwrap()).collect();
    let on = open(&dir);
    let off = off_side(&on);
    let g = "https://example.org/otl/payload";
    let mut out = Vec::new();
    for &n in &sizes {
        let ttl = assets_ttl(70_000_000, n / 9);
        let quads = parse_into(&ttl, RdfFormat::Turtle, g);
        // The row's own work, isolated: N-Quads, gzip, and the reader's way back.
        let (mut enc_ms, mut gz_ms, mut dec_ms) = (Vec::new(), Vec::new(), Vec::new());
        let (mut text_len, mut gz_len) = (0, 0);
        for _ in 0..3 {
            let (text, t1) = timed(|| encode(&quads));
            let (z, t2) = timed(|| gzip(&text));
            let (back, t3) = timed(|| changes::decode_quads(Some(z.as_slice())));
            assert_eq!(back.len(), quads.len());
            enc_ms.push(t1);
            gz_ms.push(t2);
            dec_ms.push(t3);
            text_len = text.len();
            gz_len = z.len();
        }
        // End to end: a PUT into an empty graph (the free path: the row is
        // every quad of the payload), off against on.
        let (mut t_off, mut t_on) = (Vec::new(), Vec::new());
        let mut rows = Vec::new();
        let mut blob = (0, 0);
        for rep in 0..reps {
            let order = if rep % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            };
            for capture in order {
                let s = if capture { &on } else { &off };
                let before = on.changes().last_seq();
                let (_, t) = timed(|| s.graph_store_put(Some(g), &ttl, RdfFormat::Turtle).unwrap());
                if capture {
                    t_on.push(t);
                    let r = rows_since(&on, before);
                    if let Some(seq) = r.first().and_then(|x| x["seq"].as_i64()) {
                        blob = row_blob_bytes(&dir, seq);
                    }
                    rows.push(json!(r));
                } else {
                    t_off.push(t);
                }
                eprintln!("  payload {n} rep {rep} capture={capture}: {t:.0} ms");
                off.bulk_delete_graphs(&[g]).unwrap();
            }
        }
        out.push(json!({
            "quads": quads.len(),
            "nquads_bytes": text_len, "gzip_bytes": gz_len,
            "bytes_per_quad_gz": round(gz_len as f64 / quads.len() as f64),
            "serialise_ms": round(median(&enc_ms)), "gzip_ms": round(median(&gz_ms)),
            "decode_ms": round(median(&dec_ms)),
            "put_empty_off": summary(&t_off), "put_empty_on": summary(&t_on),
            "ratio_on_off": round(median(&t_on) / median(&t_off)),
            "added_ms": round(median(&t_on) - median(&t_off)),
            "row_blob_bytes": [blob.0, blob.1], "on_rows_first_rep": rows.first(),
        }));
    }
    emit(
        "payload",
        json!({ "max_payload": on.changes().max_payload(), "sizes": out, "changes_db_mib": mib(dir_bytes(&Path::new(&dir).join("changes"))) }),
    );
}

fn cmd_bulk(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let assets: usize = arg(args, 3, "assets");
    let side: String = arg(args, 4, "off|on");
    let on = open(&dir);
    let store = if side == "off" {
        off_side(&on)
    } else {
        on.clone()
    };
    let g = "https://example.org/otl/bulk";
    eprintln!("bulk: generating {assets} assets");
    let mut quads = Vec::with_capacity(assets * 9);
    let mut done = 0;
    while done < assets {
        let n = 100_000.min(assets - done);
        quads.extend(asset_quads(80_000_000 + done, n, g));
        done += n;
    }
    let n = quads.len();
    let before = on.changes().last_seq();
    let changes_before = dir_bytes(&Path::new(&dir).join("changes"));
    let peak = PeakRss::start();
    let (_, t) = timed(|| store.bulk_insert_quads(quads, &[g.to_string()]).unwrap());
    let (start, above) = peak.stop();
    let rows = rows_since(&on, before);
    let blob = rows
        .first()
        .and_then(|r| r["seq"].as_i64())
        .map(|s| row_blob_bytes(&dir, s))
        .unwrap_or((0, 0));
    eprintln!("bulk {n} capture={side}: {t:.0} ms");
    emit(
        "bulk",
        json!({
            "quads": n, "capture": side, "ms": round(t),
            "quads_per_s": round(n as f64 / (t / 1000.0)),
            "max_payload": on.changes().max_payload(),
            "resident_at_start_mib": mib(start), "peak_above_start_mib": mib(above),
            "rows": rows, "row_blob_bytes": [blob.0, blob.1],
            "changes_db_growth_mib": mib(dir_bytes(&Path::new(&dir).join("changes")).saturating_sub(changes_before)),
        }),
    );
}

// ─── item 10: a cold read, as seen by other readers ─────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Before,
    During,
    Rebuild,
    After,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Before => "before",
            Phase::During => "during_build",
            Phase::Rebuild => "until_mirror_republished",
            Phase::After => "after",
        }
    }
}

fn consume(r: QueryResults<'_>) -> usize {
    match r {
        QueryResults::Solutions(s) => s.count(),
        QueryResults::Boolean(_) => 1,
        QueryResults::Graph(g) => g.count(),
    }
}

fn cmd_coldread(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let vgraph = graph_arg(&arg::<String>(args, 3, "vgraph"));
    let vfrom: usize = arg(args, 4, "vfrom");
    let vassets: usize = arg(args, 5, "vassets");
    let ograph = graph_arg(&arg::<String>(args, 6, "ograph"));
    let ofrom: usize = arg(args, 7, "ofrom");
    let oassets: usize = arg(args, 8, "oassets");
    let k: usize = arg(args, 9, "k");
    let secs: u64 = arg(args, 10, "phase seconds");
    let store = open(&dir);
    assert!(store.changes().enabled(), "coldread needs capture on");
    let tag = format!("cold-{}", chrono_stamp());
    let set = |off: usize, n: usize| -> Vec<usize> {
        (0..n).map(|j| vfrom + (off + j * 7) % vassets).collect()
    };
    let _v1 = cut_version(&store, &vgraph, &format!("{tag}-v1"), true);
    store
        .apply_quad_ops(&field_ops(
            &vgraph,
            "status",
            &set(0, k / 2),
            "*cold",
            false,
        ))
        .unwrap();
    let v2 = cut_version(&store, &vgraph, &format!("{tag}-v2"), false);
    store
        .apply_quad_ops(&field_ops(
            &vgraph,
            "name",
            &set(1, k / 2 + 5),
            "*cold",
            false,
        ))
        .unwrap();
    assert!(v2.is_delta(), "v2 is a delta");
    let total: usize = store.graph_counts().iter().map(|(_, n)| n).sum();
    eprintln!("coldread: store {total} quads; settling the mirror …");
    let ticker_stop = Arc::new(AtomicBool::new(false));
    let ticker = {
        let (s, stop) = (store.clone(), ticker_stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                s.accelerator_tick();
                std::thread::sleep(Duration::from_millis(500));
            }
        })
    };
    let mirror_on = std::env::var("OTS_PARALLEL_QUERY_ENABLED")
        .map(|v| v != "false" && v != "off")
        .unwrap_or(true);
    let settle = Instant::now();
    if mirror_on {
        while store.mirror_full_copy().is_none() && settle.elapsed() < Duration::from_secs(900) {
            std::thread::sleep(Duration::from_millis(250));
        }
    }
    let settled_s = settle.elapsed().as_secs_f64();
    let published_before = store.mirror_full_copy().is_some();
    eprintln!("  mirror published: {published_before} after {settled_s:.0}s");
    let phase = Arc::new(Mutex::new(Phase::Before));
    let stop = Arc::new(AtomicBool::new(false));
    type Samples = Arc<Mutex<Vec<(Phase, &'static str, f64)>>>;
    let samples: Samples = Arc::new(Mutex::new(Vec::new()));
    let mut readers = Vec::new();
    for r in 0..5usize {
        let (s, stop, phase, samples, og) = (
            store.clone(),
            stop.clone(),
            phase.clone(),
            samples.clone(),
            ograph.clone(),
        );
        readers.push(std::thread::spawn(move || {
            let mut i = r;
            while !stop.load(Ordering::Relaxed) {
                let (kind, q) = if r < 4 {
                    let id = ofrom + (i * 7919) % oassets.max(1);
                    (
                        "lookup",
                        format!("SELECT ?p ?o WHERE {{ GRAPH <{og}> {{ <{EX}asset{id}> ?p ?o }} }}"),
                    )
                } else {
                    (
                        "filter_count",
                        format!("SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{og}> {{ ?a <{EX}length> ?l FILTER(?l > 40.0 && ?l < 42.0) }} }}"),
                    )
                };
                let ph = *phase.lock().unwrap();
                let t = Instant::now();
                let _ = consume(s.query(&q).expect("reader query"));
                samples.lock().unwrap().push((ph, kind, ms(t.elapsed())));
                i += 1;
            }
        }));
    }
    std::thread::sleep(Duration::from_secs(secs));
    *phase.lock().unwrap() = Phase::During;
    let peak = PeakRss::start();
    let (built, build_ms) = timed(|| materialise::build(&store, BASE_URL, &v2));
    let (_, build_peak) = peak.stop();
    let built = built.expect("build");
    eprintln!("  build: {build_ms:.0} ms, {built} quads");
    *phase.lock().unwrap() = Phase::Rebuild;
    let rebuild = Instant::now();
    if mirror_on {
        while store.mirror_full_copy().is_none() && rebuild.elapsed() < Duration::from_secs(900) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let republish_s = rebuild.elapsed().as_secs_f64();
    eprintln!("  mirror republished after {republish_s:.1}s");
    *phase.lock().unwrap() = Phase::After;
    std::thread::sleep(Duration::from_secs(secs));
    stop.store(true, Ordering::Relaxed);
    for h in readers {
        h.join().unwrap();
    }
    ticker_stop.store(true, Ordering::Relaxed);
    ticker.join().unwrap();
    let samples = samples.lock().unwrap();
    let mut per: BTreeMap<String, Value> = BTreeMap::new();
    for ph in [Phase::Before, Phase::During, Phase::Rebuild, Phase::After] {
        for kind in ["lookup", "filter_count"] {
            let v: Vec<f64> = samples
                .iter()
                .filter(|(p, k, _)| *p == ph && *k == kind)
                .map(|(_, _, t)| *t)
                .collect();
            per.insert(
                format!("{}.{kind}", ph.name()),
                json!({
                    "n": v.len(), "p50_ms": round(percentile(&v, 0.5)),
                    "p95_ms": round(percentile(&v, 0.95)), "max_ms": round(v.iter().cloned().fold(0.0, f64::max)),
                }),
            );
        }
    }
    emit(
        "coldread",
        json!({
            "store_quads": total, "version_graph_quads": store.graph_count_cached(Some(&vgraph)),
            "built_quads": built, "build_ms": round(build_ms), "build_peak_rss_mib": mib(build_peak),
            "mirror_enabled": mirror_on, "mirror_published_before": published_before,
            "settle_s": round(settled_s), "mirror_republish_s": round(republish_s),
            "phase_seconds": secs, "latency": per,
        }),
    );
    materialise::release(&store, &v2, false);
}

// ─── item 11: LDES and blank-node owners ────────────────────────────────────

/// `assets` entities, each with a chain of `depth` blank nodes below it, the
/// last carrying a value — the shape of a nested measurement or property set.
fn bnode_ttl(from: usize, assets: usize, depth: usize) -> String {
    let mut s = format!("@prefix ex: <{EX}> .\n");
    for i in from..from + assets {
        s.push_str(&format!(
            "ex:asset{i} a ex:Asset ; ex:name \"Asset {i}\" ; ex:spec _:b{i}x1 .\n"
        ));
        for l in 1..=depth {
            if l < depth {
                s.push_str(&format!(
                    "_:b{i}x{l} ex:level {l} ; ex:inner _:b{i}x{} .\n",
                    l + 1
                ));
            } else {
                s.push_str(&format!("_:b{i}x{l} ex:level {l} ; ex:value \"v{i}\" .\n"));
            }
        }
    }
    s
}

/// The quad carrying asset `i`'s value, found by walking its blank-node
/// chain in the store (so its blank nodes are the store's own).
fn value_quad(store: &TripleStore, g: &str, i: usize) -> Option<Quad> {
    let gn = NamedNodeRef::new(g).ok()?;
    let spec = NamedNode::new_unchecked(format!("{EX}spec"));
    let inner = NamedNode::new_unchecked(format!("{EX}inner"));
    let value = NamedNode::new_unchecked(format!("{EX}value"));
    let a = NamedNode::new_unchecked(asset_iri(i));
    let mut cur: NamedOrBlankNode = a.into();
    let mut via = spec;
    loop {
        let next = store
            .store()
            .quads_for_pattern(
                Some(cur.as_ref()),
                Some(via.as_ref()),
                None,
                Some(GraphNameRef::NamedNode(gn)),
            )
            .flatten()
            .next()?;
        let Term::BlankNode(b) = next.object else {
            return None;
        };
        cur = b.into();
        if let Some(q) = store
            .store()
            .quads_for_pattern(
                Some(cur.as_ref()),
                Some(value.as_ref()),
                None,
                Some(GraphNameRef::NamedNode(gn)),
            )
            .flatten()
            .next()
        {
            return Some(q);
        }
        via = inner.clone();
    }
}

fn cmd_ldes(args: &[String]) {
    use open_triplestore::auth::models::{OwnerType, Visibility};
    use open_triplestore::ldes::capture;
    let dir: String = arg(args, 2, "dir");
    let assets: usize = arg(args, 3, "assets");
    let depth: usize = arg(args, 4, "depth");
    let m: usize = arg(args, 5, "changes per write");
    let reps: usize = arg(args, 6, "reps");
    let store = open(&dir);
    assert!(store.changes().enabled(), "ldes needs capture on");
    let state = open_triplestore::server::AppState::test_default_with_store(store.clone());
    let g = format!("https://example.org/otl/ldes-d{depth}");
    state
        .auth_db
        .create_dataset(
            "ds-ldes",
            "ldes",
            None,
            OwnerType::User,
            "u1",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("ds-ldes", &g).unwrap();
    let mut loaded = 0;
    while loaded < assets {
        let n = 20_000.min(assets - loaded);
        store
            .load_str(&bnode_ttl(loaded, n, depth), RdfFormat::Turtle, Some(&g))
            .unwrap();
        loaded += n;
    }
    // The stream is switched on after the load, as a deployment would.
    open_triplestore::ldes::store::set_stream(&state.auth_db, "ds-ldes", true, 100).unwrap();
    let quads = store.graph_count_cached(Some(&g)).unwrap_or(0);
    eprintln!("ldes: {quads} quads at depth {depth}");
    let (_, publish_all_ms) =
        timed(|| capture::publish_all(&state, "ds-ldes", std::slice::from_ref(&g)));
    let mut next_asset = 0usize;
    let mut change = |round: usize| -> (Vec<QuadOp>, Vec<usize>) {
        let ids: Vec<usize> = (0..m).map(|j| (next_asset + j * 13) % assets).collect();
        next_asset += 1;
        let ops = ids
            .iter()
            .flat_map(|&i| {
                let old = value_quad(&store, &g, i).expect("value quad");
                let new = Quad::new(
                    old.subject.clone(),
                    old.predicate.clone(),
                    Literal::new_simple_literal(format!("v{i}-{round}")),
                    old.graph_name.clone(),
                );
                [QuadOp::Remove(old), QuadOp::Add(new)]
            })
            .collect();
        (ops, ids)
    };
    let (mut scan_ms, mut rows_ms, mut write_ms) = (Vec::new(), Vec::new(), Vec::new());
    let (mut scan_members, mut rows_members) = (Vec::new(), Vec::new());
    let tracked =
        open_triplestore::ldes::store::tracked(&state.auth_db, std::slice::from_ref(&g)).unwrap();
    for rep in 0..reps {
        // The scan path: index the graph before and after the write.
        let (ops, _) = change(rep * 2);
        let (b, t_before) = timed(|| capture::before(&state, std::slice::from_ref(&g)));
        let (_, tw) = timed(|| store.apply_quad_ops(&ops).unwrap());
        let (n_scan, t_after) = timed(|| capture::after(&state, b));
        scan_ms.push(t_before + t_after);
        write_ms.push(tw);
        scan_members.push(n_scan);
        // The rows path: the write's own rows, owners walked up the tree.
        let (ops, _) = change(rep * 2 + 1);
        let stamp = format!("urn:ldes-measure:{rep}");
        {
            let _ctx = WriteContextGuard::set(WriteContext {
                actor_iri: None,
                commit_iri: Some(stamp.clone()),
                kind: None,
            });
            store.apply_quad_ops(&ops).unwrap();
        }
        let rows = store.changes().rows_for_commit(&stamp);
        let (n_rows, t_rows) = timed(|| capture::after_rows(&state, &tracked, &rows));
        rows_ms.push(t_rows);
        rows_members.push(n_rows);
        eprintln!(
            "  rep {rep}: scan {:.0} ms ({n_scan} members), rows {t_rows:.0} ms ({n_rows} members)",
            t_before + t_after
        );
    }
    emit(
        "ldes",
        json!({
            "graph_quads": quads, "assets": assets, "depth": depth, "changed_entities_per_write": m,
            "bnode_depth_setting": open_triplestore::config::LDES_BNODE_DEPTH.get(),
            "bnode_subjects_max_setting": open_triplestore::config::LDES_BNODE_SUBJECTS_MAX.get(),
            "scan_path": summary(&scan_ms), "rows_path": summary(&rows_ms), "write": summary(&write_ms),
            "scan_members": scan_members, "rows_members": rows_members,
            "publish_all_ms": round(publish_all_ms),
        }),
    );
}

// ─── item 7: kill -9 in the middle of writes ────────────────────────────────

const CRASH_P: &str = "urn:crash:p";

fn crash_graph(w: &str) -> String {
    format!("urn:crash:g:{w}")
}

/// `(workload, quads per batch)`; every quad of batch `i` of run `r` has the
/// subject `urn:crash:{r}:{w}:{i}:{j}`.
const WORKLOADS: [(&str, usize); 6] = [
    ("txn", 10_000),
    ("bulk", 50_000),
    ("ops", 20_000),
    ("put", 20_000),
    ("sst", 30_000),
    ("stream", 30_000),
];

fn crash_batch(run: usize, w: &str, i: usize, n: usize) -> Vec<Quad> {
    let g = GraphName::NamedNode(NamedNode::new_unchecked(crash_graph(w)));
    (0..n)
        .map(|j| {
            Quad::new(
                NamedNode::new_unchecked(format!("urn:crash:{run}:{w}:{i}:{j}")),
                NamedNode::new_unchecked(CRASH_P),
                Literal::new_simple_literal(format!("{j}")),
                g.clone(),
            )
        })
        .collect()
}

fn nquads(quads: &[Quad]) -> String {
    String::from_utf8(encode(quads)).unwrap()
}

fn cmd_crash_writer(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let run: usize = arg(args, 3, "run");
    let store = open(&dir);
    assert!(store.changes().enabled());
    let out = Arc::new(Mutex::new(std::io::stdout()));
    {
        let mut o = out.lock().unwrap();
        writeln!(o, "ready").unwrap();
        o.flush().unwrap();
    }
    let mut handles = Vec::new();
    for (w, n) in WORKLOADS {
        let (s, out) = (store.clone(), out.clone());
        handles.push(std::thread::spawn(move || {
            let g = crash_graph(w);
            // Until the process is killed: that is the experiment.
            for i in 0..=usize::MAX {
                let batch = crash_batch(run, w, i, n);
                let _ctx = WriteContextGuard::set(WriteContext {
                    actor_iri: None,
                    commit_iri: Some(format!("urn:crash:{run}:{w}:{i}")),
                    kind: None,
                });
                match w {
                    "txn" => {
                        let body: String = batch
                            .iter()
                            .map(|q| format!("{} {} {} .\n", q.subject, q.predicate, q.object))
                            .collect();
                        s.update(&format!("INSERT DATA {{ GRAPH <{g}> {{ {body} }} }}"))
                            .unwrap();
                    }
                    "bulk" => {
                        s.load_str(&nquads(&batch), RdfFormat::NQuads, Some(&g))
                            .unwrap();
                    }
                    "ops" => {
                        let ops: Vec<QuadOp> = batch.into_iter().map(QuadOp::Add).collect();
                        s.apply_quad_ops(&ops).unwrap();
                    }
                    "put" => {
                        s.graph_store_put(Some(&g), &nquads(&batch), RdfFormat::NQuads)
                            .unwrap();
                    }
                    "sst" => {
                        s.bulk_insert_quads(batch, std::slice::from_ref(&g))
                            .unwrap();
                    }
                    _ => {
                        let text = nquads(&batch);
                        s.load_reader(
                            std::io::BufReader::new(text.as_bytes()),
                            RdfFormat::NQuads,
                            None,
                        )
                        .unwrap();
                    }
                }
                let mut o = out.lock().unwrap();
                writeln!(o, "done {w} {i}").unwrap();
                o.flush().unwrap();
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
}

/// One row of the change log, read straight from its database.
struct RawRow {
    seq: Option<i64>,
    graph: Option<String>,
    origin: String,
    commit: Option<String>,
    extent: String,
    state: String,
}

fn raw_rows(dir: &str) -> Vec<RawRow> {
    let conn = rusqlite::Connection::open_with_flags(
        changes_db(dir),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("changes.db");
    let mut stmt = conn
        .prepare(
            "SELECT seq, graph_iri, origin, commit_iri, extent, state FROM changes ORDER BY row",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok(RawRow {
            seq: r.get(0)?,
            graph: r.get(1)?,
            origin: r.get(2)?,
            commit: r.get(3)?,
            extent: r.get(4)?,
            state: r.get(5)?,
        })
    })
    .unwrap()
    .flatten()
    .collect()
}

fn orphan_ssts(dir: &str) -> (usize, u64) {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.starts_with("bulk-") && n.ends_with(".sst")
                })
                .fold((0, 0), |(c, b), e| {
                    (c + 1, b + e.metadata().map(|m| m.len()).unwrap_or(0))
                })
        })
        .unwrap_or((0, 0))
}

/// Open the store after a kill (which settles what the crash left pending)
/// and check what it holds against what the writer said it finished and
/// what the change log says.
fn crash_check(dir: &str, run: usize, last_done: &HashMap<String, i64>) -> Value {
    let (orphans_before, orphan_bytes_before) = orphan_ssts(dir);
    let (store, open_ms) = timed(|| open(dir));
    let (orphans_after, _) = orphan_ssts(dir);
    let rows = raw_rows(dir);
    let mut violations: Vec<String> = Vec::new();
    let mut outcomes: BTreeMap<String, usize> = BTreeMap::new();
    let pending = rows.iter().filter(|r| r.state == "pending").count();
    if pending > 0 {
        violations.push(format!("{pending} rows still pending after open"));
    }
    // Sequence numbers are dense.
    let mut seqs: Vec<i64> = rows.iter().filter_map(|r| r.seq).collect();
    seqs.sort();
    for w in seqs.windows(2) {
        if w[1] != w[0] + 1 {
            violations.push(format!("sequence gap {} → {}", w[0], w[1]));
        }
    }
    let reconciled: HashSet<String> = rows
        .iter()
        .filter(|r| r.origin == "reconcile")
        .filter_map(|r| r.graph.clone())
        .collect();
    let epoch = store.changes().epoch().to_string();
    for (w, n) in WORKLOADS {
        let g = crash_graph(w);
        let gn = NamedNodeRef::new(&g).unwrap();
        // Batches present in the graph, counted through three index families.
        let mut by_batch: HashMap<(usize, usize), usize> = HashMap::new();
        let mut total = 0usize;
        for q in store
            .store()
            .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(gn)))
            .flatten()
        {
            total += 1;
            if let NamedOrBlankNode::NamedNode(s) = &q.subject {
                let parts: Vec<&str> = s.as_str().split(':').collect();
                if parts.len() == 6 {
                    let r: usize = parts[2].parse().unwrap_or(usize::MAX);
                    let i: usize = parts[4].parse().unwrap_or(usize::MAX);
                    *by_batch.entry((r, i)).or_default() += 1;
                }
            }
        }
        let by_p = store
            .store()
            .quads_for_pattern(
                None,
                Some(NamedNodeRef::new(CRASH_P).unwrap()),
                None,
                Some(GraphNameRef::NamedNode(gn)),
            )
            .count();
        if by_p != total {
            violations.push(format!(
                "{w}: {total} quads by graph scan, {by_p} by predicate"
            ));
        }
        let cached = store.graph_count_cached(Some(&g)).unwrap_or(0);
        if cached != total {
            violations.push(format!("{w}: count index {cached}, graph holds {total}"));
        }
        // Spot-check the subject index for every batch present.
        for &(r, i) in by_batch.keys() {
            for j in [0, n - 1] {
                let s = NamedNode::new_unchecked(format!("urn:crash:{r}:{w}:{i}:{j}"));
                let found = store
                    .store()
                    .quads_for_pattern(Some(s.as_ref().into()), None, None, None)
                    .next()
                    .is_some();
                if !found && by_batch[&(r, i)] == n {
                    violations.push(format!("{w}: batch {r}/{i} missing from the subject index"));
                }
            }
        }
        // Atomicity, durability and what the log says, batch by batch of
        // this run. A write's rows all carry its commit IRI; a streamed load
        // has a store-scoped intent row beside its graph rows, so the most
        // definite state of the write's rows is the write's state.
        let done = last_done.get(w).copied().unwrap_or(-1);
        let state_of = |i: usize| -> String {
            let commit = format!("urn:crash:{run}:{w}:{i}");
            let rank = |s: &str| match s {
                "committed" => 4,
                "unknown" => 3,
                "pending" => 2,
                "aborted" => 1,
                _ => 0,
            };
            rows.iter()
                .filter(|r| r.commit.as_deref() == Some(commit.as_str()))
                .map(|r| r.state.as_str())
                .max_by_key(|s| rank(s))
                .unwrap_or("none")
                .to_string()
        };
        let in_flight = (done + 1) as usize;
        for i in 0..in_flight {
            let state = state_of(i);
            if state != "committed" {
                violations.push(format!("{w} {i}: returned, but its row is {state}"));
            }
        }
        let flight_state = state_of(in_flight);
        if w == "put" {
            // The graph holds exactly one batch: the last returned, or the
            // one in flight if its commit landed.
            let held: Vec<((usize, usize), usize)> = by_batch
                .iter()
                .filter(|(_, c)| **c > 0)
                .map(|(k, c)| (*k, *c))
                .collect();
            if held.len() > 1 {
                violations.push(format!("put: the graph mixes {} batches", held.len()));
            }
            let mut flight_data = "none";
            match held.first() {
                Some(((r, i), c)) => {
                    if *c != n {
                        violations.push(format!("put {r}/{i}: {c}/{n} quads — not atomic"));
                    }
                    if *r == run && *i == in_flight {
                        flight_data = "all";
                    } else if done >= 0 && !(*r == run && *i as i64 == done) {
                        violations.push(format!(
                            "put: holds batch {r}/{i}, but batch {done} returned last"
                        ));
                    }
                }
                None if done >= 0 => {
                    violations.push("put: the graph is empty after a PUT returned".into())
                }
                None => {}
            }
            if flight_data == "all" && (flight_state == "aborted" || flight_state == "none") {
                violations.push(format!("put {in_flight}: data present, row {flight_state}"));
            }
            if flight_data == "none" && flight_state == "committed" {
                violations.push(format!("put {in_flight}: row committed, data absent"));
            }
            if flight_data != "none" || flight_state != "none" {
                *outcomes
                    .entry(format!(
                        "{w} in flight: data {flight_data}, row {flight_state}"
                    ))
                    .or_default() += 1;
            }
        } else {
            for i in 0..=in_flight + 1 {
                let present = by_batch.get(&(run, i)).copied().unwrap_or(0);
                let data = if present == n {
                    "all"
                } else if present == 0 {
                    "none"
                } else {
                    "partial"
                };
                if data == "partial" {
                    violations.push(format!("{w} {i}: {present}/{n} quads — not atomic"));
                }
                if i < in_flight && data != "all" {
                    violations.push(format!("{w} {i}: returned, but its data is {data}"));
                }
                if i > in_flight && data != "none" {
                    violations.push(format!("{w} {i}: written though it was never started"));
                }
                if i == in_flight {
                    if data == "all" && (flight_state == "aborted" || flight_state == "none") {
                        violations.push(format!(
                            "{w} {i}: data present, row {flight_state} (reconcile row: {})",
                            reconciled.contains(&g)
                        ));
                    }
                    if data == "none" && flight_state == "committed" {
                        violations.push(format!("{w} {i}: row committed, data absent"));
                    }
                    if data != "none" || flight_state != "none" {
                        *outcomes
                            .entry(format!("{w} in flight: data {data}, row {flight_state}"))
                            .or_default() += 1;
                    }
                }
            }
        }
        // A chain the log calls intact must replay to exactly what the
        // graph holds (every graph started empty with this store).
        let chain = store.changes().chain(
            std::slice::from_ref(&g),
            &epoch,
            0,
            store.changes().head_seq(),
        );
        if chain.intact {
            let fold = changes::fold_rows(&chain.rows);
            let net = fold.get(&Some(g.clone())).cloned().unwrap_or_default();
            let live: HashSet<Quad> = store
                .store()
                .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(gn)))
                .flatten()
                .collect();
            let replay: HashSet<Quad> = net.added.into_iter().collect();
            if !net.removed.is_empty() || replay != live {
                violations.push(format!(
                    "{w}: the chain is intact but replays to {} quads, the graph holds {}",
                    replay.len(),
                    live.len()
                ));
            }
            *outcomes.entry(format!("{w}: chain intact")).or_default() += 1;
        } else {
            *outcomes
                .entry(format!(
                    "{w}: chain closed ({})",
                    chain
                        .reason
                        .as_deref()
                        .unwrap_or("?")
                        .split(" (")
                        .next()
                        .unwrap_or("?")
                ))
                .or_default() += 1;
        }
    }
    let unknown_rows = rows
        .iter()
        .filter(|r| r.state == "unknown" && r.origin != "reconcile")
        .count();
    let reconcile_rows = rows.iter().filter(|r| r.origin == "reconcile").count();
    let full_rows = rows.iter().filter(|r| r.extent == "full").count();
    drop(store);
    json!({
        "run": run, "open_ms": round(open_ms),
        "violations": violations, "outcomes": outcomes,
        "rows": rows.len(), "full_rows": full_rows, "unknown_rows": unknown_rows,
        "reconcile_rows": reconcile_rows,
        "orphan_bulk_ssts_before_open": orphans_before, "orphan_bulk_sst_mib": mib(orphan_bytes_before),
        "orphan_bulk_ssts_after_open": orphans_after,
    })
}

/// A small xorshift: the kill delay needs no more randomness than this.
fn next_rand(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn cmd_crash(args: &[String]) {
    let dir: String = arg(args, 2, "dir");
    let iterations: usize = arg(args, 3, "iterations");
    let reset_every: usize = arg(args, 4, "reset every");
    let logs = PathBuf::from(format!("{dir}-logs"));
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(42)
        | 1;
    let mut results = Vec::new();
    let mut total_violations = 0usize;
    let mut outcome_totals: BTreeMap<String, u64> = BTreeMap::new();
    let mut orphans_total = 0usize;
    for it in 0..iterations {
        if it % reset_every == 0 {
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_dir_all(&logs);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::create_dir_all(&logs).unwrap();
        }
        let log = logs.join(format!("writer-{it}.log"));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["crash-writer", &dir, &it.to_string()])
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn writer");
        // Wait for the writer to have opened the store.
        let started = Instant::now();
        loop {
            let ready = std::fs::read_to_string(&log)
                .map(|s| s.starts_with("ready"))
                .unwrap_or(false);
            if ready || started.elapsed() > Duration::from_secs(300) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let delay = 300 + next_rand(&mut seed) % 2700;
        std::thread::sleep(Duration::from_millis(delay));
        // SIGKILL on Unix: no destructor, no flush, no cleanup.
        child.kill().expect("kill writer");
        let _ = child.wait();
        let mut last_done: HashMap<String, i64> = HashMap::new();
        if let Ok(f) = std::fs::File::open(&log) {
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() == 3 && parts[0] == "done" {
                    let i: i64 = parts[2].parse().unwrap_or(-1);
                    let e = last_done.entry(parts[1].to_string()).or_insert(-1);
                    *e = (*e).max(i);
                }
            }
        }
        let mut v = crash_check(&dir, it, &last_done);
        v["kill_after_ms"] = json!(delay);
        v["batches_returned"] = json!(last_done.values().map(|i| i + 1).sum::<i64>());
        total_violations += v["violations"].as_array().map(|a| a.len()).unwrap_or(0);
        orphans_total += v["orphan_bulk_ssts_before_open"].as_u64().unwrap_or(0) as usize;
        if let Some(o) = v["outcomes"].as_object() {
            for (k, n) in o {
                *outcome_totals.entry(k.clone()).or_default() += n.as_u64().unwrap_or(0);
            }
        }
        eprintln!(
            "crash {it}: killed after {delay} ms, {} violations, {} orphan SSTs",
            v["violations"].as_array().map(|a| a.len()).unwrap_or(0),
            v["orphan_bulk_ssts_before_open"]
        );
        results.push(v);
    }
    emit(
        "crash",
        json!({
            "iterations": iterations, "violations": total_violations,
            "orphan_bulk_ssts_seen": orphans_total,
            "outcome_totals": outcome_totals, "runs": results,
        }),
    );
}

// ─── main ───────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let started = Instant::now();
    match args.get(1).map(String::as_str) {
        Some("load") => cmd_load(&args),
        Some("scan") => cmd_scan(&args),
        Some("probe") => cmd_probe(&args),
        Some("put") => cmd_put(&args),
        Some("txrss") => cmd_txrss(&args),
        Some("restore") => cmd_restore(&args),
        Some("census") => cmd_census(&args),
        Some("annotations") => cmd_annotations(&args),
        Some("payload") => cmd_payload(&args),
        Some("bulk") => cmd_bulk(&args),
        Some("coldread") => cmd_coldread(&args),
        Some("ldes") => cmd_ldes(&args),
        Some("crash") => cmd_crash(&args),
        Some("crash-writer") => cmd_crash_writer(&args),
        _ => {
            eprintln!("usage: see the module documentation of examples/scale_delta.rs");
            std::process::exit(2);
        }
    }
    eprintln!("({:.1}s)", started.elapsed().as_secs_f64());
}
