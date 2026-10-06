//! Runner for the buildingSMART IDS 1.0 test corpus
//! (`Documentation/ImplementersDocumentation/TestCases` of
//! https://github.com/buildingSMART/IDS): 334 IDS + IFC pairs in nine
//! folders, each named `pass-`, `fail-` or `invalid-` after the outcome every
//! IDS implementation must reach on it.
//!
//! The corpus is **fetched at test time, never vendored**. It is licensed
//! CC BY-ND 4.0 (© buildingSMART International Ltd.), and its IFC files came
//! from IfcOpenShell's ifctester work, so the repository keeps only
//! `tests/fixtures/buildingsmart-ids/MANIFEST.sha256` — the path and SHA-256 of
//! every file at the pinned commit below — plus its PROVENANCE.md. The runner
//! downloads each file from that commit (or reuses an earlier download in
//! `target/buildingsmart-ids/<commit>/`, or `$OTS_IDS_CORPUS_DIR`), checks its
//! SHA-256 against the manifest, and refuses to run on any mismatch. Offline it
//! skips with a message, unless `OTS_TEST_IDS_CORPUS_REQUIRED=1` (set in CI),
//! which turns a failed download into a failure.
//!
//! Each case runs the path a user takes: the IFC file through the built-in IFC
//! lift's IDS projection (the graph an IFC import writes to `{building}/ids`),
//! the IDS through the IDS importer (`POST /api/shacl/import/ids`), and the
//! resulting shapes through the SHACL validator over the projection. The
//! outcome is satisfied when
//!
//! - `pass-`: the importer accepts the IDS and the lifted model conforms;
//! - `fail-`: the importer rejects the IDS, or the model does not conform;
//! - `invalid-`: the same as `fail-` (an invalid IDS "could not be satisfied,
//!   regardless of IFC contents"; rejecting it at import counts).
//!
//! These are development and regression results on the corpus, not a
//! buildingSMART certification, and no score is published from them (see
//! docs/conformance/ids.md).
//!
//! Gap policy (two-way ratchet, as in `w3c_shacl_conformance.rs`): every case
//! NOT in `KNOWN_FAILURES` must be satisfied and every listed case must still
//! be unsatisfied, so silent regressions and silent fixes both turn the suite
//! red. Run with `OTS_IDS_PRINT_FAILURES=1` to print the current list.

use open_triplestore::ifc::{convert_layers, ConvertOptions};
use open_triplestore::shacl::validate;
use open_triplestore::spec_import::importer;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const CORPUS_REPO: &str = "buildingSMART/IDS";
const CORPUS_COMMIT: &str = "870f9c4e6e8f414e737b4d84ca1aa9b46fc6c8f3";
const CORPUS_DIR: &str = "Documentation/ImplementersDocumentation/TestCases";
const MANIFEST: &str = include_str!("fixtures/buildingsmart-ids/MANIFEST.sha256");

const SHAPES_GRAPH: &str = "urn:ids-corpus:shapes";
const DATA_GRAPH: &str = "http://example.org/ids-corpus/model/";

/// Cases that are currently unsatisfied, with the gap they sit behind. Keys
/// are `<folder>/<file stem>`. Keep sorted. Removing an entry requires the
/// case to be satisfied (the ratchet asserts both directions).
///
/// Baseline (develop @ 570a7a3, before any IDS work): 183 of 334 satisfied,
/// most of them vacuously — the building-topology lift leaves uncontained
/// elements out, so nothing is targeted and every specification conforms.
/// With typed values, negated prohibited facets and the existence check: 147,
/// and no longer vacuous — a required specification now fails when the
/// building-topology layer has none of the model's applicable elements.
/// With the IDS projection, the SHACL-SPARQL converter and the IDS audit: all
/// 334, and the list is empty — keep it that way.
const KNOWN_FAILURES: &[(&str, &str)] = &[];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    Pass,
    Fail,
    Invalid,
}

struct Case {
    key: String,
    expect: Expect,
    ids: PathBuf,
    ifc: PathBuf,
}

fn manifest() -> Vec<(String, String)> {
    MANIFEST
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let (hash, path) = l.split_once("  ").expect("`<sha256>  <path>` lines");
            (hash.to_string(), path.to_string())
        })
        .collect()
}

fn cache_dir() -> PathBuf {
    match std::env::var_os("OTS_IDS_CORPUS_DIR") {
        Some(d) => PathBuf::from(d),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("buildingsmart-ids")
            .join(CORPUS_COMMIT),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Make every manifest file present and verified under `dir`. `Err` means the
/// corpus could not be fetched (offline, or the host refused); a file whose
/// bytes do not match the manifest is a panic, never a skip.
fn ensure_corpus(dir: &Path) -> Result<(), String> {
    let entries = manifest();
    let mut missing = Vec::new();
    for (hash, rel) in &entries {
        let path = dir.join(rel);
        match std::fs::read(&path) {
            Ok(bytes) if sha256_hex(&bytes) == *hash => {}
            Ok(_) => {
                // A stale or corrupted cache entry: fetch it again.
                let _ = std::fs::remove_file(&path);
                missing.push((hash.clone(), rel.clone()));
            }
            Err(_) => missing.push((hash.clone(), rel.clone())),
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    let dir = dir.to_path_buf();
    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("http client: {e}"))?;
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
        let mut tasks = tokio::task::JoinSet::new();
        for (hash, rel) in missing {
            let client = client.clone();
            let sem = sem.clone();
            let dir = dir.clone();
            tasks.spawn(async move {
                let _permit = sem.acquire_owned().await.map_err(|e| e.to_string())?;
                let url = format!(
                    "https://raw.githubusercontent.com/{CORPUS_REPO}/{CORPUS_COMMIT}/{CORPUS_DIR}/{rel}"
                );
                let resp = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| format!("{url}: {e}"))?;
                if !resp.status().is_success() {
                    return Err(format!("{url}: HTTP {}", resp.status()));
                }
                let bytes = resp.bytes().await.map_err(|e| format!("{url}: {e}"))?;
                let got = sha256_hex(&bytes);
                assert_eq!(
                    got, hash,
                    "{rel}: the downloaded bytes do not match the pinned SHA-256 \
                     (tests/fixtures/buildingsmart-ids/MANIFEST.sha256) — refusing to run on them"
                );
                let path = dir.join(&rel);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
                Ok::<(), String>(())
            });
        }
        while let Some(r) = tasks.join_next().await {
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Err(e),
                Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    })
}

fn cases(dir: &Path) -> Vec<Case> {
    let mut stems: BTreeMap<String, (bool, bool)> = BTreeMap::new();
    for (_, rel) in manifest() {
        if let Some(stem) = rel.strip_suffix(".ids") {
            stems.entry(stem.to_string()).or_default().0 = true;
        } else if let Some(stem) = rel.strip_suffix(".ifc") {
            stems.entry(stem.to_string()).or_default().1 = true;
        }
    }
    stems
        .into_iter()
        .map(|(stem, (has_ids, has_ifc))| {
            assert!(
                has_ids && has_ifc,
                "{stem}: the corpus pairs every IDS with an IFC"
            );
            let name = stem.rsplit('/').next().unwrap_or(&stem);
            let expect = if name.starts_with("pass-") {
                Expect::Pass
            } else if name.starts_with("fail-") {
                Expect::Fail
            } else if name.starts_with("invalid-") {
                Expect::Invalid
            } else {
                panic!("{stem}: unknown outcome prefix");
            };
            Case {
                ids: dir.join(format!("{stem}.ids")),
                ifc: dir.join(format!("{stem}.ifc")),
                key: stem,
                expect,
            }
        })
        .collect()
}

/// What the platform made of one case: `Rejected` (the importer refused the
/// IDS) or the validator's verdict on the lifted model.
enum Verdict {
    Rejected(String),
    Conforms,
    Violates(usize),
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Rejected(why) => write!(f, "rejected at import: {why}"),
            Verdict::Conforms => write!(f, "conforms"),
            Verdict::Violates(n) => write!(f, "{n} violation(s)"),
        }
    }
}

fn run_case(case: &Case) -> Result<Verdict, String> {
    let ids = std::fs::read(&case.ids).map_err(|e| format!("read ids: {e}"))?;
    let ifc = std::fs::read_to_string(&case.ifc).map_err(|e| format!("read ifc: {e}"))?;
    let imported = match importer("ids").expect("ids importer").import(&ids) {
        Ok(i) => i,
        Err(e) => return Ok(Verdict::Rejected(e.to_string())),
    };
    let store = TripleStore::in_memory().map_err(|e| format!("store: {e}"))?;
    // The IDS projection is the data an IDS is checked against; the
    // building-topology and ifcOWL outputs are not needed here.
    let mut data = String::new();
    convert_layers(
        &ifc,
        &ConvertOptions {
            inst_base: DATA_GRAPH.to_string(),
            include_ids: true,
            ..Default::default()
        },
        &mut |_| {},
        &mut |_| {},
        &mut |c| data.push_str(c),
    )
    .map_err(|e| format!("ifc lift: {e}"))?;
    store
        .load_str(&data, RdfFormat::NTriples, Some(DATA_GRAPH))
        .map_err(|e| format!("load data: {e}"))?;
    store
        .load_str(&imported.turtle, RdfFormat::Turtle, Some(SHAPES_GRAPH))
        .map_err(|e| format!("load shapes: {e}\n{}", imported.turtle))?;
    let report = validate(&store, SHAPES_GRAPH, &[DATA_GRAPH.to_string()])
        .map_err(|e| format!("validate: {e}"))?;
    Ok(if report.conforms {
        Verdict::Conforms
    } else {
        Verdict::Violates(report.results_count)
    })
}

fn satisfied(expect: Expect, verdict: &Verdict) -> bool {
    match (expect, verdict) {
        (Expect::Pass, Verdict::Conforms) => true,
        (Expect::Pass, _) => false,
        (Expect::Fail | Expect::Invalid, Verdict::Conforms) => false,
        (Expect::Fail | Expect::Invalid, _) => true,
    }
}

#[test]
fn buildingsmart_ids_corpus() {
    let dir = cache_dir();
    if let Err(e) = ensure_corpus(&dir) {
        assert!(
            std::env::var_os("OTS_TEST_IDS_CORPUS_REQUIRED").is_none(),
            "OTS_TEST_IDS_CORPUS_REQUIRED is set but the buildingSMART IDS corpus could not be fetched: {e}"
        );
        eprintln!(
            "SKIP: the buildingSMART IDS corpus could not be fetched ({e}); it is downloaded from \
             github.com/{CORPUS_REPO} at {CORPUS_COMMIT} on first run"
        );
        return;
    }

    let all = cases(&dir);
    assert_eq!(all.len(), 334, "the pinned corpus has 334 cases");
    let mut unexpected_failures = Vec::new();
    let mut unexpected_passes = Vec::new();
    let mut still_failing = Vec::new();
    let mut seen_known = 0usize;
    let mut satisfied_count = 0usize;
    let trace = std::env::var_os("OTS_IDS_TRACE").is_some();
    for case in &all {
        if trace {
            eprintln!("case {}", case.key);
        }
        let known = KNOWN_FAILURES.iter().find(|(k, _)| *k == case.key);
        if known.is_some() {
            seen_known += 1;
        }
        let (ok, detail) = match run_case(case) {
            Ok(v) => (satisfied(case.expect, &v), v.to_string()),
            Err(e) => (false, format!("error: {e}")),
        };
        if ok {
            satisfied_count += 1;
            if let Some((k, why)) = known {
                unexpected_passes.push(format!("{k} (listed as: {why})"));
            }
        } else {
            still_failing.push((case.key.clone(), detail.clone()));
            if known.is_none() {
                unexpected_failures.push(format!("{}: {detail}", case.key));
            }
        }
    }
    println!(
        "buildingSMART IDS corpus @ {}: {satisfied_count} satisfied, {} known-unsatisfied, {} cases",
        &CORPUS_COMMIT[..7],
        KNOWN_FAILURES.len(),
        all.len()
    );
    if std::env::var_os("OTS_IDS_PRINT_FAILURES").is_some() {
        for (k, d) in &still_failing {
            let d: String = d.chars().take(160).collect();
            println!(
                "    (\"{k}\", \"{}\"),",
                d.replace('\\', "\\\\").replace('"', "\\\"")
            );
        }
    }
    assert_eq!(
        seen_known,
        KNOWN_FAILURES.len(),
        "every KNOWN_FAILURES key must name a corpus case (stale entries?)"
    );
    assert!(
        unexpected_failures.is_empty(),
        "IDS corpus cases unsatisfied that are not in KNOWN_FAILURES:\n  {}",
        unexpected_failures.join("\n  ")
    );
    assert!(
        unexpected_passes.is_empty(),
        "KNOWN_FAILURES entries are now satisfied — remove them to ratchet forward:\n  {}",
        unexpected_passes.join("\n  ")
    );
}

/// The manifest pins the whole corpus: every case's IDS and IFC, nothing
/// else, with well-formed SHA-256 digests. Runs offline.
#[test]
fn the_manifest_pins_every_corpus_file() {
    let entries = manifest();
    assert_eq!(entries.len(), 668, "334 IDS + IFC pairs");
    for (hash, rel) in &entries {
        assert_eq!(hash.len(), 64, "{rel}");
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()), "{rel}");
        assert!(
            rel.ends_with(".ids") || rel.ends_with(".ifc"),
            "{rel}: only test-case files are pinned"
        );
        assert!(!rel.contains(".."), "{rel}");
    }
    let mut sorted = entries.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), entries.len(), "no duplicate paths");
}
