//! Runner for the RDF 1.2 syntax test suites of w3c/rdf-tests (`rdf/rdf12`,
//! with the `rdf/rdf11` suites each RDF 1.2 manifest includes), vendored
//! unmodified under `tests/fixtures/w3c-rdf12` (see PROVENANCE.md and
//! LICENSE.md there).
//!
//! The vendored files are a subset of a W3C test suite, used under the W3C
//! 3-clause BSD licence for development and bug tracking only. W3C's
//! test-suite licence policy allows no public performance claims on a subset,
//! so the runner states no pass count: the known-failure list and the pass
//! floor below drive the ratchet, and no score is published (see
//! `scripts/conformance_table.py` and docs/conformance/rdf12.md).
//!
//! The top-level `rdf12/manifest.ttl` is walked through `mf:include`, and so
//! is every manifest it includes (each RDF 1.2 format manifest includes its
//! own syntax/eval/c14n sections and the RDF 1.1 suite of the same format).
//! Every entry loads its input through `TripleStore::load_str_with_base` — the
//! path uploads and the Graph Store protocol take — into a fresh in-memory
//! store, with the file's own IRI as the base (the suites' rule for relative
//! IRI resolution):
//!
//! - `rdft:Test*PositiveSyntax` / `rdft:Test*NegativeSyntax`: the load must
//!   succeed / must fail.
//! - `rdft:TestTurtleEval`, `rdft:TestTrigEval`, `rdft:TestXMLEval`: the
//!   store's contents after the load must be isomorphic to `mf:result`
//!   (blank nodes matched structurally, also inside triple terms; literals
//!   compared exactly, lexical form, language tag, base direction and
//!   datatype).
//! - `rdft:TestNTriplesPositiveC14N`, `rdft:TestNQuadsPositiveC14N`: the
//!   store's N-Triples / N-Quads export of the loaded data must be, line for
//!   line, the canonical form in `mf:result` (lines compared as a set, since
//!   the canonical form fixes each line, not their order).
//!
//! `rdf12/manifest.ttl` also includes `rdf-semantics/manifest.ttl` (RDF 1.2
//! Semantics entailment tests, which include the RDF 1.1 `rdf-mt` suite).
//! Those are entailment-regime tests, not syntax tests; they are not vendored,
//! and the runner records that include as not run (see PROVENANCE.md).
//!
//! Gap policy (two-way ratchet, as in `w3c_sparql11_manifests.rs`): every
//! entry NOT in `KNOWN_FAILURES` must pass, and every listed entry must still
//! fail — silent regressions and silent fixes both turn the suite red. A pass
//! floor guards against a loader regression turning passes into skips.

#![cfg(feature = "rdf-12")]

use open_triplestore::store::engine::BlankNodeMode;
use open_triplestore::store::TripleStore;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::dataset::CanonicalizationAlgorithm;
use oxigraph::model::vocab::rdf;
use oxigraph::model::{
    Dataset, Graph, NamedNode, NamedOrBlankNodeRef, Quad, Term, TermRef, TripleRef,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const SUITE_ROOT: &str = "tests/fixtures/w3c-rdf12";
/// The suites' published location: `rdf12/…` and `rdf11/…` sit under it, the
/// manifests' relative IRIs resolve against it, and each test file's own IRI
/// under it is the base for parsing that file.
const BASE: &str = "https://w3c.github.io/rdf-tests/rdf/";
const TOP: &str = "rdf12/manifest.ttl";
/// Includes of the top manifest that are not vendored and not run.
const NOT_RUN: &[&str] = &["rdf12/rdf-semantics/manifest.ttl"];

const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const RDFT: &str = "http://www.w3.org/ns/rdftest#";

/// Entries that currently fail, with the gap they sit behind. Keep sorted.
/// Removing an entry requires the entry to actually pass (the ratchet asserts
/// both directions). One cause, below the platform layer: oxrdfxml's
/// `rdf:parseType="Literal"` output, which the suite changed on 2026-04-20
/// while the canonical form of `rdf:XMLLiteral` is an open W3C issue
/// (w3c/rdf-xml#97). The numeric lexical-form entries listed here before pass
/// now that the store keeps literals as written (vendor/README.md).
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("rdf11/rdf-xml/manifest.ttl#xml-canon-test001", "`rdf:parseType=\"Literal\"`: oxrdfxml 0.2.4 keeps the in-scope namespace declarations on the literal's root element; the suite expects `\"<br></br>\"` again since rdf-tests d974697 (2026-04-20). The canonical form of `rdf:XMLLiteral` is open W3C issue w3c/rdf-xml#97 (opened 2026-04-20)"),
    ("rdf11/rdf-xml/manifest.ttl#xml-canon-test002", "as xml-canon-test001"),
    ("rdf12/rdf-xml/eval#rdf12-xml-an-13", "as rdf11 xml-canon-test001, inside a triple term (rdf-tests 7633586, 2026-05-31; w3c/rdf-xml#97)"),
    ("rdf12/rdf-xml/eval#rdf12-xml-an-14", "as rdf12-xml-an-13"),
];

/// Pass floor: a loader or parser regression turns passes into skips or
/// failures; the two ratchet asserts alone would not notice a wholesale skip.
/// It sits below the current count, with headroom for corpus churn.
const PASS_FLOOR: usize = 1250;

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

fn nn(ns: &str, local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{ns}{local}"))
}

/// Local file for an IRI under [`BASE`].
fn local_path(iri: &str) -> Option<PathBuf> {
    let rel = iri.strip_prefix(BASE)?;
    let rel = rel.split('#').next().unwrap_or(rel);
    Some(Path::new(SUITE_ROOT).join(rel))
}

fn read_bytes(iri: &str) -> Result<Vec<u8>, String> {
    let path = local_path(iri).ok_or_else(|| format!("<{iri}> is outside the corpus"))?;
    std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn format_for(iri: &str) -> Option<RdfFormat> {
    RdfFormat::from_extension(iri.rsplit('.').next()?)
}

/// Parse a corpus file with the raw parser (manifests and expected results).
fn parse_file(iri: &str) -> Result<Vec<Quad>, String> {
    let bytes = read_bytes(iri)?;
    let format = format_for(iri).ok_or_else(|| format!("<{iri}>: unknown RDF format"))?;
    RdfParser::from_format(format)
        .with_base_iri(iri)
        .map_err(|e| format!("base <{iri}>: {e}"))?
        .for_slice(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("<{iri}>: {e}"))
}

fn graph_of(iri: &str) -> Result<Graph, String> {
    let mut g = Graph::new();
    for q in parse_file(iri)? {
        g.insert(TripleRef::new(&q.subject, &q.predicate, &q.object));
    }
    Ok(g)
}

fn as_node(t: TermRef<'_>) -> Option<NamedOrBlankNodeRef<'_>> {
    match t {
        TermRef::NamedNode(n) => Some(n.into()),
        TermRef::BlankNode(b) => Some(b.into()),
        _ => None,
    }
}

fn iri_of(t: TermRef<'_>) -> Option<String> {
    match t {
        TermRef::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    }
}

/// Items of an RDF collection starting at `head`.
fn list_items(g: &Graph, head: TermRef<'_>) -> Vec<Term> {
    let mut out = Vec::new();
    let mut cur = head.into_owned();
    loop {
        if cur == Term::NamedNode(rdf::NIL.into_owned()) {
            break;
        }
        let Some(node) = as_node(cur.as_ref()) else {
            break;
        };
        if let Some(first) = g.object_for_subject_predicate(node, rdf::FIRST) {
            out.push(first.into_owned());
        }
        match g.object_for_subject_predicate(node, rdf::REST) {
            Some(rest) => cur = rest.into_owned(),
            None => break,
        }
    }
    out
}

fn object(g: &Graph, subject: NamedOrBlankNodeRef<'_>, predicate: &NamedNode) -> Option<Term> {
    g.object_for_subject_predicate(subject, predicate)
        .map(|t| t.into_owned())
}

/// The list items of `predicate` on the file's `mf:Manifest` node (the file
/// itself, or a node such as `trs:manifest` that the file types as one).
fn manifest_list(g: &Graph, manifest_iri: &str, predicate: &str) -> Vec<Term> {
    let file = NamedNode::new_unchecked(manifest_iri);
    let manifest = nn(MF, "Manifest");
    let mut roots: Vec<NamedOrBlankNodeRef<'_>> = g
        .subjects_for_predicate_object(rdf::TYPE, manifest.as_ref())
        .collect();
    if roots.is_empty() {
        roots.push(file.as_ref().into());
    }
    let predicate = nn(MF, predicate);
    let heads: Vec<TermRef<'_>> = roots
        .into_iter()
        .flat_map(|root| g.objects_for_subject_predicate(root, &predicate))
        .collect();
    heads
        .into_iter()
        .flat_map(|head| list_items(g, head))
        .collect()
}

struct Entry {
    /// `rdf12/rdf-turtle/syntax#turtle12-syntax-basic-01`
    id: String,
    kind: String,
    action: String,
    result: Option<String>,
}

fn entries(g: &Graph, manifest_iri: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    for item in manifest_list(g, manifest_iri, "entries") {
        let Some(node) = as_node(item.as_ref()) else {
            continue;
        };
        let id = match item.as_ref() {
            TermRef::NamedNode(n) => n.as_str().strip_prefix(BASE).unwrap_or(n.as_str()),
            _ => continue,
        }
        .to_string();
        let kind = object(g, node, &rdf::TYPE.into_owned())
            .and_then(|t| iri_of(t.as_ref()))
            .and_then(|t| t.strip_prefix(RDFT).map(str::to_string))
            .unwrap_or_default();
        let Some(action) = object(g, node, &nn(MF, "action")).and_then(|t| iri_of(t.as_ref()))
        else {
            continue;
        };
        let result = object(g, node, &nn(MF, "result")).and_then(|t| iri_of(t.as_ref()));
        out.push(Entry {
            id,
            kind,
            action,
            result,
        });
    }
    out
}

fn fresh_store() -> TripleStore {
    TripleStore::in_memory()
        .unwrap()
        .with_blank_node_mode(BlankNodeMode::Preserve)
        .with_query_cache(false, 1, 1)
        .with_parallel_query(false, 1, usize::MAX)
}

/// Load an entry's action into a fresh store the way an upload does.
fn load(entry: &Entry, format: RdfFormat) -> Result<Result<TripleStore, String>, String> {
    let bytes = read_bytes(&entry.action)?;
    // A file that is not UTF-8 is a load failure like any other syntax error.
    let text = match String::from_utf8(bytes) {
        Ok(t) => t,
        Err(e) => return Ok(Err(format!("not UTF-8: {e}"))),
    };
    let store = fresh_store();
    Ok(store
        .load_str_with_base(&text, format, &entry.action, None)
        .map(|()| store)
        .map_err(|e| e.to_string()))
}

fn store_dataset(store: &TripleStore) -> Result<Dataset, String> {
    let mut ds = Dataset::new();
    for q in store.store().iter() {
        ds.insert(&q.map_err(|e| format!("store read: {e}"))?);
    }
    Ok(ds)
}

fn sorted_quads(ds: &Dataset, limit: usize) -> String {
    let mut lines: Vec<String> = ds.iter().map(|q| q.to_string()).collect();
    lines.sort();
    let n = lines.len();
    lines.truncate(limit);
    let mut s = lines.join("\n    ");
    if n > limit {
        s.push_str(&format!("\n    … ({n} quads)"));
    }
    s
}

fn run_syntax(entry: &Entry, format: RdfFormat, positive: bool) -> Outcome {
    match (positive, load(entry, format)) {
        (_, Err(e)) => Outcome::Skip(e),
        (true, Ok(Ok(_))) | (false, Ok(Err(_))) => Outcome::Pass,
        (true, Ok(Err(e))) => Outcome::Fail(format!("must load: {e}")),
        (false, Ok(Ok(_))) => Outcome::Fail("must not load, but did".into()),
    }
}

fn run_eval(entry: &Entry, format: RdfFormat) -> Outcome {
    let Some(result) = &entry.result else {
        return Outcome::Skip("no mf:result".into());
    };
    let store = match load(entry, format) {
        Err(e) => return Outcome::Skip(e),
        Ok(Err(e)) => return Outcome::Fail(format!("must load: {e}")),
        Ok(Ok(s)) => s,
    };
    let mut expected = Dataset::new();
    match parse_file(result) {
        Ok(quads) => {
            for q in quads {
                expected.insert(&q);
            }
        }
        Err(e) => return Outcome::Skip(format!("expected result: {e}")),
    }
    let mut actual = match store_dataset(&store) {
        Ok(ds) => ds,
        Err(e) => return Outcome::Fail(e),
    };
    actual.canonicalize(CanonicalizationAlgorithm::Unstable);
    expected.canonicalize(CanonicalizationAlgorithm::Unstable);
    if actual == expected {
        Outcome::Pass
    } else {
        Outcome::Fail(format!(
            "stored data differs ({} vs {} expected quads)\n  expected:\n    {}\n  actual:\n    {}",
            actual.len(),
            expected.len(),
            sorted_quads(&expected, 12),
            sorted_quads(&actual, 12)
        ))
    }
}

/// The non-empty lines of an N-Triples / N-Quads text with every blank-node
/// label masked: the canonical form fixes how each term is written, not which
/// label a blank node gets, and a store relabels blank nodes on load (each
/// load's blank nodes are fresh). The labels are checked separately, by
/// isomorphism.
fn masked_lines(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut out = String::with_capacity(l.len());
            let mut rest = l;
            // Blank-node labels sit outside string literals; a `_:` inside a
            // literal is escaped data and stays.
            let mut in_literal = false;
            while let Some(c) = rest.chars().next() {
                if !in_literal && rest.starts_with("_:") {
                    out.push_str("_:*");
                    rest = rest[2..].trim_start_matches(|ch: char| {
                        !ch.is_whitespace() && ch != ')' && ch != '>'
                    });
                    continue;
                }
                if c == '"' {
                    in_literal = !in_literal;
                } else if c == '\\' && in_literal {
                    out.push(c);
                    rest = &rest[1..];
                    if let Some(n) = rest.chars().next() {
                        out.push(n);
                        rest = &rest[n.len_utf8()..];
                    }
                    continue;
                }
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
            out
        })
        .collect()
}

fn dataset_of(text: &str, format: RdfFormat) -> Result<Dataset, String> {
    let mut ds = Dataset::new();
    for q in RdfParser::from_format(format).for_slice(text.as_bytes()) {
        ds.insert(&q.map_err(|e| e.to_string())?);
    }
    ds.canonicalize(CanonicalizationAlgorithm::Unstable);
    Ok(ds)
}

fn run_c14n(entry: &Entry, format: RdfFormat) -> Outcome {
    let Some(result) = &entry.result else {
        return Outcome::Skip("no mf:result".into());
    };
    let store = match load(entry, format) {
        Err(e) => return Outcome::Skip(e),
        Ok(Err(e)) => return Outcome::Fail(format!("must load: {e}")),
        Ok(Ok(s)) => s,
    };
    let expected = match read_bytes(result).map(String::from_utf8) {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return Outcome::Skip(format!("expected result: {e}")),
        Err(e) => return Outcome::Skip(format!("expected result: {e}")),
    };
    let exported = if format == RdfFormat::NQuads {
        let mut out = Vec::new();
        store.dump_all_nquads(&mut out).map(|_| out)
    } else {
        store.dump(RdfFormat::NTriples, None)
    };
    let exported = match exported.map(String::from_utf8) {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return Outcome::Fail(format!("export is not UTF-8: {e}")),
        Err(e) => return Outcome::Fail(format!("export: {e}")),
    };
    let (want, got) = (masked_lines(&expected), masked_lines(&exported));
    if want == got {
        match (dataset_of(&expected, format), dataset_of(&exported, format)) {
            (Ok(a), Ok(b)) if a == b => Outcome::Pass,
            (Ok(_), Ok(_)) => {
                Outcome::Fail("export is not isomorphic to the canonical form".into())
            }
            (Err(e), _) => Outcome::Skip(format!("expected result: {e}")),
            (_, Err(e)) => Outcome::Fail(format!("export does not parse: {e}")),
        }
    } else {
        Outcome::Fail(format!(
            "export is not the canonical form\n  expected:\n    {}\n  actual:\n    {}",
            want.iter().cloned().collect::<Vec<_>>().join("\n    "),
            got.iter().cloned().collect::<Vec<_>>().join("\n    ")
        ))
    }
}

fn run_entry(entry: &Entry) -> Outcome {
    use RdfFormat::*;
    let (format, rest) = match entry.kind.as_str() {
        k if k.starts_with("TestNTriples") => (NTriples, &k["TestNTriples".len()..]),
        k if k.starts_with("TestNQuads") => (NQuads, &k["TestNQuads".len()..]),
        k if k.starts_with("TestTurtle") => (Turtle, &k["TestTurtle".len()..]),
        k if k.starts_with("TestTrig") => (TriG, &k["TestTrig".len()..]),
        k if k.starts_with("TestXML") => (RdfXml, &k["TestXML".len()..]),
        other => return Outcome::Skip(format!("unsupported test type rdft:{other}")),
    };
    match rest {
        "PositiveSyntax" => run_syntax(entry, format, true),
        "NegativeSyntax" => run_syntax(entry, format, false),
        "Eval" => run_eval(entry, format),
        "NegativeEval" => run_syntax(entry, format, false),
        "PositiveC14N" => run_c14n(entry, format),
        other => Outcome::Skip(format!("unsupported test type rdft:{}{other}", entry.kind)),
    }
}

#[derive(Default)]
struct Tally {
    pass: usize,
    known_fail: usize,
    total: usize,
    manifests: usize,
    skips: Vec<String>,
    unexpected_failures: Vec<String>,
    unexpected_passes: Vec<String>,
    not_run: Vec<String>,
}

/// Walk a manifest: its entries, then (depth first) its includes.
fn run_manifest(manifest_iri: &str, tally: &mut Tally, seen: &mut BTreeSet<String>) {
    if !seen.insert(manifest_iri.to_string()) {
        return;
    }
    let rel = manifest_iri.strip_prefix(BASE).unwrap_or(manifest_iri);
    if NOT_RUN.contains(&rel) {
        tally.not_run.push(rel.to_string());
        return;
    }
    let g = graph_of(manifest_iri).unwrap_or_else(|e| panic!("{rel}: {e}"));
    tally.manifests += 1;
    for entry in entries(&g, manifest_iri) {
        tally.total += 1;
        let known = KNOWN_FAILURES.iter().find(|(k, _)| *k == entry.id);
        match run_entry(&entry) {
            Outcome::Pass => {
                tally.pass += 1;
                if let Some((k, why)) = known {
                    tally
                        .unexpected_passes
                        .push(format!("{k} (listed as: {why})"));
                }
            }
            Outcome::Fail(reason) => {
                if known.is_some() {
                    tally.known_fail += 1;
                } else {
                    tally
                        .unexpected_failures
                        .push(format!("{} [{}]: {reason}", entry.id, entry.kind));
                }
            }
            Outcome::Skip(reason) => tally.skips.push(format!("{}: {reason}", entry.id)),
        }
    }
    for include in manifest_list(&g, manifest_iri, "include") {
        if let Some(iri) = iri_of(include.as_ref()) {
            run_manifest(&iri, tally, seen);
        }
    }
}

#[test]
fn w3c_rdf12_syntax_suites() {
    let mut tally = Tally::default();
    run_manifest(&format!("{BASE}{TOP}"), &mut tally, &mut BTreeSet::new());

    for s in &tally.skips {
        println!("  SKIP {s}");
    }
    println!(
        "W3C RDF 1.2 syntax suites: {} manifests, {} entries — {} passed, {} known-fail, {} skipped; not run: {:?}",
        tally.manifests,
        tally.total,
        tally.pass,
        tally.known_fail,
        tally.skips.len(),
        tally.not_run
    );
    assert_eq!(
        tally.not_run.len(),
        NOT_RUN.len(),
        "a NOT_RUN include is no longer in the corpus: {:?}",
        tally.not_run
    );
    let listed_seen = tally.known_fail + tally.unexpected_passes.len();
    assert_eq!(
        listed_seen,
        KNOWN_FAILURES.len(),
        "KNOWN_FAILURES lists {} entries but {listed_seen} were encountered (stale ids?)",
        KNOWN_FAILURES.len()
    );
    assert!(
        tally.unexpected_failures.is_empty(),
        "{} entries failing that are not in KNOWN_FAILURES:\n  {}",
        tally.unexpected_failures.len(),
        tally.unexpected_failures.join("\n  ")
    );
    assert!(
        tally.unexpected_passes.is_empty(),
        "KNOWN_FAILURES entries now pass — remove them to ratchet forward:\n  {}",
        tally.unexpected_passes.join("\n  ")
    );
    assert!(
        tally.skips.is_empty(),
        "{} entries were skipped:\n  {}",
        tally.skips.len(),
        tally.skips.join("\n  ")
    );
    assert!(
        tally.pass >= PASS_FLOOR,
        "only {} W3C RDF entries passed (floor {PASS_FLOOR})",
        tally.pass
    );
}
