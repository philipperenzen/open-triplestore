//! Runner for the W3C SPARQL 1.2 test suite (w3c/rdf-tests `sparql/sparql12`),
//! vendored unmodified under `tests/fixtures/w3c-sparql12` (see PROVENANCE.md
//! and LICENSE.md there).
//!
//! The vendored files are a subset of a W3C test suite (its implementation
//! reports and cover-page template are left out), used under the W3C 3-clause
//! BSD licence for development and bug tracking only. W3C's test-suite licence
//! policy allows no public performance claims on a subset, so the runner
//! states no pass count: the known-failure list and the pass floor below drive
//! the ratchet, and no score is published (see `scripts/conformance_table.py`
//! and docs/conformance/sparql12.md).
//!
//! The top-level `manifest.ttl` is walked through `mf:include`; every entry
//! runs through `TripleStore` (the evaluation path `/sparql` uses, with the
//! result cache and the parallel mirror off so each case is one plain
//! evaluation against a fresh in-memory store). Every query-evaluation entry
//! then runs a second time with the in-memory mirror on (four subject shards,
//! the columnar copy, the full copy; no rebuild debounce) and must end the same
//! way: the mirror answers the server's reads by default, so the corpus has to
//! hold for it too.
//!
//! - `mf:QueryEvaluationTest`: `qt:data` into the default graph (a TriG or
//!   N-Quads file keeps its named graphs), each `qt:graphData` into the named
//!   graph of its resolved IRI, the query run with `BASE <query IRI>` and
//!   compared to `mf:result`. ASK by boolean; SELECT by result-set isomorphism
//!   (both sides encoded as an RDF graph in the DAWG result-set vocabulary and
//!   canonicalised, so blank nodes are matched structurally, also inside
//!   triple terms; `rs:index` is added only when the query has an outer
//!   `ORDER BY`); CONSTRUCT/DESCRIBE by graph isomorphism.
//! - `mf:UpdateEvaluationTest`: `ut:data`/`ut:graphData` loaded, the request
//!   run with `BASE <request IRI>`, the whole resulting dataset compared with
//!   the expected one by isomorphism.
//! - `mf:PositiveSyntaxTest` / `mf:NegativeSyntaxTest` and the update
//!   variants: the file must parse / must not parse.
//!
//! Numeric literals are compared by value (`"2.0"^^xsd:decimal` equals
//! `"2"^^xsd:decimal`), at any depth inside triple terms, because the engine
//! stores numerics natively and writes them back in canonical form; every
//! other literal by lexical form, language tag, base direction and datatype.
//! An entry that declares `mf:requires mf:NoCanonicalizationOfNumerics` is
//! compared exactly instead.
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
use oxigraph::model::vocab::{rdf, rdfs, xsd};
use oxigraph::model::{
    BlankNode, Dataset, Graph, GraphName, Literal, NamedNode, NamedOrBlankNodeRef, Quad, Term,
    TermRef, Triple, TripleRef,
};
use oxigraph::sparql::results::{
    QueryResultsFormat, QueryResultsParser, ReaderQueryResultsParserOutput,
};
use oxigraph::sparql::QueryResults;
use spargebra::algebra::GraphPattern;
use spargebra::{Query, SparqlParser};
use std::path::{Path, PathBuf};

const SUITE_ROOT: &str = "tests/fixtures/w3c-sparql12";
/// The corpus' published location: every relative IRI in the manifests and
/// test files resolves against it, and expected results (graph names,
/// CONSTRUCT output) are expressed in it, so both sides of every comparison
/// use the same IRIs.
const BASE: &str = "https://w3c.github.io/rdf-tests/sparql/sparql12/";
/// The manifests' own entry namespace (`@prefix : <…/sparql12/<dir>/manifest#>`);
/// stripped for readable test ids such as `eval-triple-terms/manifest#basic-2`.
const ENTRY_NS: &str = BASE;
const TOP: &str = "manifest.ttl";

const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const QT: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#";
const UT: &str = "http://www.w3.org/2009/sparql/tests/test-update#";
const RS: &str = "http://www.w3.org/2001/sw/DataAccess/tests/result-set#";

/// Entries that currently fail, with the gap they sit behind. Keep sorted.
/// Removing an entry requires the entry to actually pass (the ratchet asserts
/// both directions). None is in the platform layer: three are the oxigraph
/// 0.5.11 SPARQL parser (spargebra 0.4.7) behind grammar decisions the
/// Working Group has since closed, and one asks for a feature
/// (`mf:NoCanonicalizationOfNumerics`) the oxigraph 0.5 storage lacks. The
/// vendored spargebra already refuses nested aggregates
/// (`syntax#nested-aggregate-functions`; vendor/README.md).
const KNOWN_FAILURES: &[(&str, &str)] = &[
    ("grouping/manifest#group01", "requires mf:NoCanonicalizationOfNumerics: storage keeps numerics as values, so `\"001\"^^xsd:integer` reads back as `\"1\"` and groups with it; fixed by the lexical-form storage change"),
    ("grouping/manifest#select-variable-reuse", "spargebra 0.4.7 rejects a SELECT expression that uses a variable an earlier SELECT expression of an aggregating query binds (`(COUNT(?v) AS ?count) (?count + 1 AS ?p)`); allowed since w3c/sparql-query PR #380 (closed 2026-05-28)"),
    ("syntax-triple-terms-negative/manifest#tripleterm-subject-03", "spargebra 0.4.7 accepts a triple term as the subject of a triple-term expression (`<<( <<( :s :p :o )>> :q :z )>>`); the grammar forbids it since w3c/sparql-query#282/#283 closed 2025-12-26"),
    ("syntax-triple-terms-negative/manifest#tripleterm-subject-06", "as tripleterm-subject-03, with a literal subject (`<<( \"literal\" :q :z )>>`)"),
];

/// Pass floor: a loader or parser regression turns passes into skips or
/// failures; the two ratchet asserts alone would not notice a wholesale skip.
/// It sits below the current count, with headroom for corpus churn.
const PASS_FLOOR: usize = 250;

/// Mirror floor: query-evaluation entries that must run with the in-memory
/// mirror actually built, so the parity check cannot pass by never building
/// it. Below the current count, with the same headroom.
const MIRROR_FLOOR: usize = 35;

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
    iri.strip_prefix(BASE)
        .map(|rel| Path::new(SUITE_ROOT).join(rel))
}

fn read_text(iri: &str) -> Result<String, String> {
    let path = local_path(iri).ok_or_else(|| format!("<{iri}> is outside the corpus"))?;
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn rdf_format_for(iri: &str) -> Option<RdfFormat> {
    let ext = iri.rsplit('.').next()?;
    RdfFormat::from_extension(ext)
}

/// Parse an RDF file of the corpus into quads (relative IRIs resolved against
/// the file's own IRI).
fn parse_rdf_file(iri: &str) -> Result<Vec<Quad>, String> {
    let path = local_path(iri).ok_or_else(|| format!("<{iri}> is outside the corpus"))?;
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let format = rdf_format_for(iri).ok_or_else(|| format!("<{iri}>: unknown RDF format"))?;
    RdfParser::from_format(format)
        .with_base_iri(iri)
        .map_err(|e| format!("base <{iri}>: {e}"))?
        .for_slice(&bytes)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("<{iri}>: {e}"))
}

fn graph_of(iri: &str) -> Result<Graph, String> {
    let mut g = Graph::new();
    for q in parse_rdf_file(iri)? {
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

fn lexical(t: TermRef<'_>) -> Option<String> {
    match t {
        TermRef::Literal(l) => Some(l.value().to_string()),
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

fn objects(g: &Graph, subject: NamedOrBlankNodeRef<'_>, predicate: &NamedNode) -> Vec<Term> {
    g.objects_for_subject_predicate(subject, predicate)
        .map(|t| t.into_owned())
        .collect()
}

fn object(g: &Graph, subject: NamedOrBlankNodeRef<'_>, predicate: &NamedNode) -> Option<Term> {
    g.object_for_subject_predicate(subject, predicate)
        .map(|t| t.into_owned())
}

/// The heads of the `predicate` lists on a manifest file's `mf:Manifest`
/// node: the file itself (`<>`), or a node such as `trs:manifest` that the
/// file types as one.
fn manifest_heads(g: &Graph, manifest_iri: &str, predicate: &str) -> Vec<Term> {
    let file = NamedNode::new_unchecked(manifest_iri);
    let manifest = nn(MF, "Manifest");
    let mut roots: Vec<NamedOrBlankNodeRef<'_>> = g
        .subjects_for_predicate_object(rdf::TYPE, manifest.as_ref())
        .collect();
    if roots.is_empty() {
        roots.push(file.as_ref().into());
    }
    roots
        .into_iter()
        .flat_map(|root| objects(g, root, &nn(MF, predicate)))
        .collect()
}

/// `mf:include` targets of a manifest, in order.
fn includes(manifest_iri: &str) -> Result<Vec<String>, String> {
    let g = graph_of(manifest_iri)?;
    let mut out = Vec::new();
    for head in manifest_heads(&g, manifest_iri, "include") {
        for item in list_items(&g, head.as_ref()) {
            if let Some(iri) = iri_of(item.as_ref()) {
                out.push(iri);
            }
        }
    }
    Ok(out)
}

struct Entry {
    /// `aggregates/manifest#agg01`
    id: String,
    kind: String,
    action: Term,
    result: Option<Term>,
    /// `mf:requires mf:NoCanonicalizationOfNumerics`
    exact_numerics: bool,
}

/// `mf:entries` of a manifest, in order.
fn entries(manifest_iri: &str) -> Result<(Graph, Vec<Entry>), String> {
    let g = graph_of(manifest_iri)?;
    let mut out = Vec::new();
    for head in manifest_heads(&g, manifest_iri, "entries") {
        for item in list_items(&g, head.as_ref()) {
            let Some(node) = as_node(item.as_ref()) else {
                continue;
            };
            let id = match item.as_ref() {
                TermRef::NamedNode(n) => n
                    .as_str()
                    .strip_prefix(ENTRY_NS)
                    .unwrap_or(n.as_str())
                    .to_string(),
                other => other.to_string(),
            };
            let kind = object(&g, node, &rdf::TYPE.into_owned())
                .and_then(|t| iri_of(t.as_ref()))
                .and_then(|t| t.strip_prefix(MF).map(str::to_string))
                .unwrap_or_default();
            let Some(action) = object(&g, node, &nn(MF, "action")) else {
                continue;
            };
            let result = object(&g, node, &nn(MF, "result"));
            let exact_numerics = objects(&g, node, &nn(MF, "requires")).iter().any(|t| {
                iri_of(t.as_ref()).as_deref() == Some(&format!("{MF}NoCanonicalizationOfNumerics"))
            });
            out.push(Entry {
                id,
                kind,
                action,
                result,
                exact_numerics,
            });
        }
    }
    Ok((g, out))
}

/// Which evaluation path a store answers queries through.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Route {
    /// The engine alone: the in-memory mirror is off.
    Engine,
    /// The in-memory mirror in front of the engine, as the server runs it:
    /// subject shards, the columnar copy and the full copy, each answering
    /// what it accepts. Four shards, and no debounce, so the first query after
    /// the load builds the mirror instead of falling through to the engine.
    Mirror,
}

fn fresh_store() -> TripleStore {
    fresh_store_on(Route::Engine)
}

fn fresh_store_on(route: Route) -> TripleStore {
    let store = TripleStore::in_memory()
        .unwrap()
        .with_blank_node_mode(BlankNodeMode::Preserve)
        .with_query_cache(false, 1, 1);
    match route {
        Route::Engine => store.with_parallel_query(false, 1, usize::MAX),
        Route::Mirror => store
            .with_parallel_query(true, 4, usize::MAX)
            .with_parallel_rebuild_quiet_ms(0),
    }
}

/// Prepend `BASE <iri>` unless the text declares its own base, so relative
/// IRIs in the query resolve exactly as the suite expects.
fn with_base(text: &str, iri: &str) -> String {
    let declares_base = text
        .lines()
        .map(str::trim_start)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .is_some_and(|l| l.len() >= 4 && l[..4].eq_ignore_ascii_case("base"));
    if declares_base {
        text.to_string()
    } else {
        format!("BASE <{iri}>\n{text}")
    }
}

fn load_into(store: &TripleStore, file_iri: &str, graph: Option<&str>) -> Result<(), String> {
    let text = read_text(file_iri)?;
    let format = rdf_format_for(file_iri).ok_or_else(|| format!("<{file_iri}>: unknown format"))?;
    store
        .load_str_with_base(&text, format, file_iri, graph)
        .map_err(|e| format!("load <{file_iri}>: {e}"))
}

// ─── Literal normalisation ────────────────────────────────────────────────────

fn canonical_integer(v: &str) -> String {
    let v = v.trim();
    let (neg, digits) = match v.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let digits = digits.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    if neg && digits != "0" {
        format!("-{digits}")
    } else {
        digits.to_string()
    }
}

fn canonical_decimal(v: &str) -> String {
    let v = v.trim();
    let (neg, rest) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, f),
        None => (rest, ""),
    };
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int };
    let frac = frac.trim_end_matches('0');
    let zero = int == "0" && frac.is_empty();
    let mut s = String::new();
    if neg && !zero {
        s.push('-');
    }
    s.push_str(int);
    if !frac.is_empty() {
        s.push('.');
        s.push_str(frac);
    }
    s
}

fn canonical_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".to_string()
    } else if f.is_infinite() {
        if f > 0.0 { "INF" } else { "-INF" }.to_string()
    } else if f == 0.0 {
        "0".to_string()
    } else {
        format!("{f}")
    }
}

thread_local! {
    /// Set while an entry that declares `mf:requires
    /// mf:NoCanonicalizationOfNumerics` runs: its literals compare exactly.
    static EXACT_NUMERICS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn normalize_literal(l: &Literal) -> Literal {
    if EXACT_NUMERICS.with(|e| e.get()) {
        return l.clone();
    }
    let dt = l.datatype();
    let v = l.value();
    if dt == xsd::DECIMAL {
        return Literal::new_typed_literal(canonical_decimal(v), xsd::DECIMAL);
    }
    if dt == xsd::DOUBLE || dt == xsd::FLOAT {
        if let Ok(f) = v.trim().parse::<f64>() {
            return Literal::new_typed_literal(canonical_float(f), dt.into_owned());
        }
        return l.clone();
    }
    if dt == xsd::BOOLEAN {
        let b = match v.trim() {
            "1" | "true" => "true",
            "0" | "false" => "false",
            other => other,
        };
        return Literal::new_typed_literal(b, xsd::BOOLEAN);
    }
    if dt == xsd::INTEGER
        || dt == xsd::INT
        || dt == xsd::LONG
        || dt == xsd::SHORT
        || dt == xsd::BYTE
        || dt == xsd::NON_NEGATIVE_INTEGER
        || dt == xsd::NON_POSITIVE_INTEGER
        || dt == xsd::POSITIVE_INTEGER
        || dt == xsd::NEGATIVE_INTEGER
        || dt == xsd::UNSIGNED_INT
        || dt == xsd::UNSIGNED_LONG
        || dt == xsd::UNSIGNED_SHORT
        || dt == xsd::UNSIGNED_BYTE
    {
        return Literal::new_typed_literal(canonical_integer(v), dt.into_owned());
    }
    l.clone()
}

fn normalize_term(t: Term) -> Term {
    match t {
        Term::Literal(l) => Term::Literal(normalize_literal(&l)),
        // Literals inside a triple term are compared the same way.
        Term::Triple(tt) => Term::Triple(Box::new(Triple::new(
            tt.subject,
            tt.predicate,
            normalize_term(tt.object),
        ))),
        other => other,
    }
}

fn normalize_quad(q: Quad) -> Quad {
    Quad::new(
        q.subject,
        q.predicate,
        normalize_term(q.object),
        q.graph_name,
    )
}

// ─── Result comparison ────────────────────────────────────────────────────────

type Row = Vec<(String, Term)>;

enum Expected {
    Bool(bool),
    Solutions {
        vars: Vec<String>,
        rows: Vec<Row>,
        /// The file carries an order (`rs:index`, or document order in SRX/SRJ).
        ordered: bool,
    },
    Graph(Box<Dataset>),
}

/// Expected result file → structured expectation.
fn expected_result(result_iri: &str, query_is_graph: bool) -> Result<Expected, String> {
    let ext = result_iri.rsplit('.').next().unwrap_or("");
    if let Some(fmt) = QueryResultsFormat::from_extension(ext) {
        let path =
            local_path(result_iri).ok_or_else(|| format!("<{result_iri}> outside corpus"))?;
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        return match QueryResultsParser::from_format(fmt)
            .for_reader(bytes.as_slice())
            .map_err(|e| format!("<{result_iri}>: {e}"))?
        {
            ReaderQueryResultsParserOutput::Boolean(b) => Ok(Expected::Bool(b)),
            ReaderQueryResultsParserOutput::Solutions(iter) => {
                let vars: Vec<String> = iter
                    .variables()
                    .iter()
                    .map(|v| v.as_str().to_string())
                    .collect();
                let mut rows = Vec::new();
                for sol in iter {
                    let sol = sol.map_err(|e| format!("<{result_iri}>: {e}"))?;
                    rows.push(
                        sol.iter()
                            .map(|(v, t)| (v.as_str().to_string(), t.clone()))
                            .collect(),
                    );
                }
                Ok(Expected::Solutions {
                    vars,
                    rows,
                    ordered: true,
                })
            }
        };
    }
    // RDF file: either a CONSTRUCT/DESCRIBE result graph, or a SELECT/ASK
    // result set in the DAWG result-set vocabulary.
    let quads = parse_rdf_file(result_iri)?;
    if query_is_graph {
        let mut ds = Dataset::new();
        for q in quads {
            ds.insert(&normalize_quad(q));
        }
        return Ok(Expected::Graph(Box::new(ds)));
    }
    let mut g = Graph::new();
    for q in &quads {
        g.insert(TripleRef::new(&q.subject, &q.predicate, &q.object));
    }
    let rs_type = nn(RS, "ResultSet");
    let root = g
        .subjects_for_predicate_object(rdf::TYPE, rs_type.as_ref())
        .next()
        .ok_or_else(|| format!("<{result_iri}>: no rs:ResultSet"))?;
    if let Some(b) = object(&g, root, &nn(RS, "boolean")) {
        return Ok(Expected::Bool(
            lexical(b.as_ref()).as_deref() == Some("true"),
        ));
    }
    let vars: Vec<String> = objects(&g, root, &nn(RS, "resultVariable"))
        .iter()
        .filter_map(|t| lexical(t.as_ref()))
        .collect();
    let mut indexed: Vec<(Option<i64>, Row)> = Vec::new();
    for sol in objects(&g, root, &nn(RS, "solution")) {
        let Some(sol) = as_node(sol.as_ref()) else {
            continue;
        };
        let index = object(&g, sol, &nn(RS, "index"))
            .and_then(|t| lexical(t.as_ref()))
            .and_then(|s| s.parse::<i64>().ok());
        let mut row = Vec::new();
        for b in objects(&g, sol, &nn(RS, "binding")) {
            let Some(b) = as_node(b.as_ref()) else {
                continue;
            };
            let var = object(&g, b, &nn(RS, "variable")).and_then(|t| lexical(t.as_ref()));
            let val = object(&g, b, &nn(RS, "value"));
            if let (Some(var), Some(val)) = (var, val) {
                row.push((var, val));
            }
        }
        indexed.push((index, row));
    }
    let ordered = !indexed.is_empty() && indexed.iter().all(|(i, _)| i.is_some());
    indexed.sort_by_key(|(i, _)| *i);
    Ok(Expected::Solutions {
        vars,
        rows: indexed.into_iter().map(|(_, r)| r).collect(),
        ordered,
    })
}

/// Encode a result set as an RDF graph (DAWG result-set vocabulary) so that
/// two result sets can be compared by graph isomorphism — blank nodes in the
/// bindings become part of the structure and are matched, not compared by
/// label.
fn encode_solutions(vars: &[String], rows: &[Row], ordered: bool) -> Dataset {
    let mut ds = Dataset::new();
    let g = GraphName::DefaultGraph;
    let root = BlankNode::default();
    ds.insert(&Quad::new(
        root.clone(),
        rdf::TYPE,
        nn(RS, "ResultSet"),
        g.clone(),
    ));
    for v in vars {
        ds.insert(&Quad::new(
            root.clone(),
            nn(RS, "resultVariable"),
            Literal::new_simple_literal(v),
            g.clone(),
        ));
    }
    for (i, row) in rows.iter().enumerate() {
        let sol = BlankNode::default();
        ds.insert(&Quad::new(
            root.clone(),
            nn(RS, "solution"),
            sol.clone(),
            g.clone(),
        ));
        if ordered {
            ds.insert(&Quad::new(
                sol.clone(),
                nn(RS, "index"),
                Literal::from(i as i64),
                g.clone(),
            ));
        }
        for (var, term) in row {
            let b = BlankNode::default();
            ds.insert(&Quad::new(
                sol.clone(),
                nn(RS, "binding"),
                b.clone(),
                g.clone(),
            ));
            ds.insert(&Quad::new(
                b.clone(),
                nn(RS, "variable"),
                Literal::new_simple_literal(var),
                g.clone(),
            ));
            ds.insert(&Quad::new(
                b,
                nn(RS, "value"),
                normalize_term(term.clone()),
                g.clone(),
            ));
        }
    }
    ds
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

fn datasets_equal(mut actual: Dataset, mut expected: Dataset, what: &str) -> Outcome {
    actual.canonicalize(CanonicalizationAlgorithm::Unstable);
    expected.canonicalize(CanonicalizationAlgorithm::Unstable);
    if actual == expected {
        Outcome::Pass
    } else {
        Outcome::Fail(format!(
            "{what} differ ({} vs {} expected quads)\n  expected:\n    {}\n  actual:\n    {}",
            actual.len(),
            expected.len(),
            sorted_quads(&expected, 24),
            sorted_quads(&actual, 24)
        ))
    }
}

fn row_summary(vars: &[String], rows: &[Row], limit: usize) -> String {
    let mut lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let mut cells: Vec<String> = r.iter().map(|(v, t)| format!("?{v}={t}")).collect();
            cells.sort();
            cells.join(" ")
        })
        .collect();
    lines.sort();
    let n = lines.len();
    lines.truncate(limit);
    format!(
        "vars {vars:?}, {n} rows\n    {}{}",
        lines.join("\n    "),
        if n > limit { "\n    …" } else { "" }
    )
}

fn compare_solutions(
    vars: &[String],
    rows: &[Row],
    exp_vars: &[String],
    exp_rows: &[Row],
    ordered: bool,
) -> Outcome {
    let mut a: Vec<&String> = vars.iter().collect();
    let mut e: Vec<&String> = exp_vars.iter().collect();
    a.sort();
    e.sort();
    if a != e {
        return Outcome::Fail(format!(
            "projected variables differ: expected {exp_vars:?}, got {vars:?}"
        ));
    }
    let actual = encode_solutions(vars, rows, ordered);
    let expected = encode_solutions(exp_vars, exp_rows, ordered);
    match datasets_equal(actual, expected, "result sets") {
        Outcome::Pass => Outcome::Pass,
        _ => Outcome::Fail(format!(
            "result sets differ{}\n  expected: {}\n  actual:   {}",
            if ordered { " (ordered)" } else { "" },
            row_summary(exp_vars, exp_rows, 16),
            row_summary(vars, rows, 16)
        )),
    }
}

/// Whether the query's outer solution sequence is ordered (an `ORDER BY`
/// directly under the projection / slice / distinct), in which case the
/// expected result order is significant.
fn is_ordered(query: &Query) -> bool {
    fn walk(p: &GraphPattern) -> bool {
        match p {
            GraphPattern::OrderBy { .. } => true,
            GraphPattern::Project { inner, .. }
            | GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. } => walk(inner),
            _ => false,
        }
    }
    match query {
        Query::Select { pattern, .. } => walk(pattern),
        _ => false,
    }
}

// ─── Entry runners ────────────────────────────────────────────────────────────

fn run_query_evaluation(g: &Graph, entry: &Entry) -> Outcome {
    run_query_evaluation_on(g, entry, Route::Engine).0
}

/// Run a query-evaluation entry on a store taking `route`; also says whether
/// the in-memory mirror was built (an empty dataset never builds one).
fn run_query_evaluation_on(g: &Graph, entry: &Entry, route: Route) -> (Outcome, bool) {
    let store = fresh_store_on(route);
    let outcome = evaluate_entry(g, entry, &store);
    (outcome, store.parallel_build_count() > 0)
}

fn evaluate_entry(g: &Graph, entry: &Entry, store: &TripleStore) -> Outcome {
    let Some(action) = as_node(entry.action.as_ref()) else {
        return Outcome::Skip("action is not a node".into());
    };
    let Some(query_iri) = object(g, action, &nn(QT, "query")).and_then(|t| iri_of(t.as_ref()))
    else {
        return Outcome::Skip("no qt:query".into());
    };
    let Some(result_iri) = entry.result.as_ref().and_then(|t| iri_of(t.as_ref())) else {
        return Outcome::Skip("no mf:result file".into());
    };
    let query_text = match read_text(&query_iri) {
        Ok(t) => t,
        Err(e) => return Outcome::Skip(e),
    };
    let parsed = match SparqlParser::new()
        .with_base_iri(query_iri.as_str())
        .map_err(|e| e.to_string())
        .and_then(|p| p.parse_query(&query_text).map_err(|e| e.to_string()))
    {
        Ok(q) => q,
        Err(e) => return Outcome::Fail(format!("query does not parse: {e}")),
    };
    let query_is_graph = matches!(parsed, Query::Construct { .. } | Query::Describe { .. });
    let ordered = is_ordered(&parsed);

    for d in objects(g, action, &nn(QT, "data")) {
        if let Some(iri) = iri_of(d.as_ref()) {
            if let Err(e) = load_into(store, &iri, None) {
                return Outcome::Skip(e);
            }
        }
    }
    for d in objects(g, action, &nn(QT, "graphData")) {
        if let Some(iri) = iri_of(d.as_ref()) {
            if let Err(e) = load_into(store, &iri, Some(&iri)) {
                return Outcome::Skip(e);
            }
        }
    }

    let expected = match expected_result(&result_iri, query_is_graph) {
        Ok(e) => e,
        Err(e) => return Outcome::Skip(format!("expected result: {e}")),
    };

    let results = match store.query(&with_base(&query_text, &query_iri)) {
        Ok(r) => r,
        Err(e) => return Outcome::Fail(format!("evaluation error: {e}")),
    };
    match (results, expected) {
        (QueryResults::Boolean(b), Expected::Bool(want)) => {
            if b == want {
                Outcome::Pass
            } else {
                Outcome::Fail(format!("ASK: expected {want}, got {b}"))
            }
        }
        (
            QueryResults::Solutions(sols),
            Expected::Solutions {
                vars,
                rows,
                ordered: exp_ordered,
            },
        ) => {
            let actual_vars: Vec<String> = sols
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            let mut actual_rows: Vec<Row> = Vec::new();
            for sol in sols {
                match sol {
                    Ok(sol) => actual_rows.push(
                        sol.iter()
                            .map(|(v, t)| (v.as_str().to_string(), t.clone()))
                            .collect(),
                    ),
                    Err(e) => return Outcome::Fail(format!("evaluation error: {e}")),
                }
            }
            compare_solutions(
                &actual_vars,
                &actual_rows,
                &vars,
                &rows,
                ordered && exp_ordered,
            )
        }
        (QueryResults::Graph(triples), Expected::Graph(expected)) => {
            let mut actual = Dataset::new();
            for t in triples {
                match t {
                    Ok(t) => {
                        actual.insert(&normalize_quad(t.in_graph(GraphName::DefaultGraph)));
                    }
                    Err(e) => return Outcome::Fail(format!("evaluation error: {e}")),
                }
            }
            datasets_equal(actual, *expected, "graphs")
        }
        (got, _) => Outcome::Fail(format!(
            "result kind mismatch: got {}",
            match got {
                QueryResults::Boolean(_) => "boolean",
                QueryResults::Solutions(_) => "solutions",
                QueryResults::Graph(_) => "graph",
            }
        )),
    }
}

fn run_query_syntax(entry: &Entry, positive: bool) -> Outcome {
    let Some(iri) = iri_of(entry.action.as_ref()) else {
        return Outcome::Skip("action is not a file".into());
    };
    let text = match read_text(&iri) {
        Ok(t) => t,
        Err(e) => return Outcome::Skip(e),
    };
    let parsed = SparqlParser::new()
        .with_base_iri(iri.as_str())
        .map_err(|e| e.to_string())
        .and_then(|p| p.parse_query(&text).map_err(|e| e.to_string()));
    match (positive, parsed) {
        (true, Ok(_)) | (false, Err(_)) => Outcome::Pass,
        (true, Err(e)) => Outcome::Fail(format!("must parse: {e}")),
        (false, Ok(_)) => Outcome::Fail("must not parse, but did".into()),
    }
}

fn run_update_syntax(entry: &Entry, positive: bool) -> Outcome {
    let Some(iri) = iri_of(entry.action.as_ref()) else {
        return Outcome::Skip("action is not a file".into());
    };
    let text = match read_text(&iri) {
        Ok(t) => t,
        Err(e) => return Outcome::Skip(e),
    };
    let parsed = SparqlParser::new()
        .with_base_iri(iri.as_str())
        .map_err(|e| e.to_string())
        .and_then(|p| p.parse_update(&text).map_err(|e| e.to_string()));
    match (positive, parsed) {
        (true, Ok(_)) | (false, Err(_)) => Outcome::Pass,
        (true, Err(e)) => Outcome::Fail(format!("must parse: {e}")),
        (false, Ok(_)) => Outcome::Fail("must not parse, but did".into()),
    }
}

/// `(file IRI, graph name)` pairs of a `ut:graphData` set.
fn graph_data(g: &Graph, node: NamedOrBlankNodeRef<'_>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for gd in objects(g, node, &nn(UT, "graphData")) {
        let Some(gd) = as_node(gd.as_ref()) else {
            continue;
        };
        let file = object(g, gd, &nn(UT, "graph")).and_then(|t| iri_of(t.as_ref()));
        let label = object(g, gd, &rdfs::LABEL.into_owned()).and_then(|t| lexical(t.as_ref()));
        if let (Some(file), Some(label)) = (file, label) {
            out.push((file, label));
        }
    }
    out
}

fn expected_dataset(g: &Graph, node: NamedOrBlankNodeRef<'_>) -> Result<Dataset, String> {
    let mut ds = Dataset::new();
    for d in objects(g, node, &nn(UT, "data")) {
        if let Some(iri) = iri_of(d.as_ref()) {
            // A TriG / N-Quads file states the whole dataset, named graphs
            // included; a graph format is the default graph.
            for q in parse_rdf_file(&iri)? {
                ds.insert(&normalize_quad(q));
            }
        }
    }
    for (file, label) in graph_data(g, node) {
        let graph = NamedNode::new(&label).map_err(|e| format!("graph <{label}>: {e}"))?;
        for q in parse_rdf_file(&file)? {
            ds.insert(&normalize_quad(Quad::new(
                q.subject,
                q.predicate,
                q.object,
                graph.clone(),
            )));
        }
    }
    Ok(ds)
}

fn run_update_evaluation(g: &Graph, entry: &Entry) -> Outcome {
    let Some(action) = as_node(entry.action.as_ref()) else {
        return Outcome::Skip("action is not a node".into());
    };
    let Some(request_iri) = object(g, action, &nn(UT, "request")).and_then(|t| iri_of(t.as_ref()))
    else {
        return Outcome::Skip("no ut:request".into());
    };
    let Some(result) = entry.result.as_ref().and_then(|t| as_node(t.as_ref())) else {
        return Outcome::Skip("no mf:result".into());
    };
    let request = match read_text(&request_iri) {
        Ok(t) => t,
        Err(e) => return Outcome::Skip(e),
    };
    let store = fresh_store();
    for d in objects(g, action, &nn(UT, "data")) {
        if let Some(iri) = iri_of(d.as_ref()) {
            if let Err(e) = load_into(&store, &iri, None) {
                return Outcome::Skip(e);
            }
        }
    }
    for (file, label) in graph_data(g, action) {
        if let Err(e) = load_into(&store, &file, Some(&label)) {
            return Outcome::Skip(e);
        }
    }
    let expected = match expected_dataset(g, result) {
        Ok(ds) => ds,
        Err(e) => return Outcome::Skip(format!("expected dataset: {e}")),
    };
    if let Err(e) = store.update(&with_base(&request, &request_iri)) {
        return Outcome::Fail(format!("update error: {e}"));
    }
    let mut actual = Dataset::new();
    for q in store.store().iter() {
        match q {
            Ok(q) => {
                actual.insert(&normalize_quad(q));
            }
            Err(e) => return Outcome::Fail(format!("store read: {e}")),
        }
    }
    datasets_equal(actual, expected, "datasets")
}

fn run_entry(g: &Graph, entry: &Entry) -> Outcome {
    match entry.kind.as_str() {
        "QueryEvaluationTest" => run_query_evaluation(g, entry),
        "PositiveSyntaxTest11" | "PositiveSyntaxTest" => run_query_syntax(entry, true),
        "NegativeSyntaxTest11" | "NegativeSyntaxTest" => run_query_syntax(entry, false),
        "UpdateEvaluationTest" => run_update_evaluation(g, entry),
        "PositiveUpdateSyntaxTest11" | "PositiveUpdateSyntaxTest" => run_update_syntax(entry, true),
        "NegativeUpdateSyntaxTest11" | "NegativeUpdateSyntaxTest" => {
            run_update_syntax(entry, false)
        }
        other => Outcome::Skip(format!("unsupported test type mf:{other}")),
    }
}

#[derive(Default)]
struct Tally {
    pass: usize,
    known_fail: usize,
    total: usize,
    skips: Vec<String>,
    unexpected_failures: Vec<String>,
    unexpected_passes: Vec<String>,
    /// Query-evaluation entries answered with the in-memory mirror built.
    mirror_built: usize,
    /// Entries whose outcome through the mirror differs from the engine's.
    mirror_divergences: Vec<String>,
}

/// Run one entry, turning an engine panic into a failure of that entry
/// rather than of the whole run.
fn guarded(run: impl FnOnce() -> Outcome) -> Outcome {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|p| {
        let msg = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        Outcome::Fail(format!("evaluation panicked: {msg}"))
    })
}

fn run_manifest(top: &str, tally: &mut Tally) {
    let top_iri = format!("{BASE}{top}");
    let manifests = includes(&top_iri).unwrap_or_else(|e| panic!("{top}: {e}"));
    assert!(
        !manifests.is_empty(),
        "{top}: no mf:include entries — is the corpus vendored?"
    );
    for m in &manifests {
        let (g, entries) = match entries(m) {
            Ok(x) => x,
            Err(e) => {
                tally.skips.push(format!("{m}: {e}"));
                continue;
            }
        };
        for entry in &entries {
            tally.total += 1;
            let known = KNOWN_FAILURES.iter().find(|(k, _)| *k == entry.id);
            EXACT_NUMERICS.with(|e| e.set(entry.exact_numerics));
            let outcome = guarded(|| run_entry(&g, entry));
            // Mirror parity: the same entry through the shards, the columnar
            // copy and the full copy must end the way it ends on the engine —
            // a pass stays a pass, a known failure stays a failure.
            if entry.kind == "QueryEvaluationTest" {
                let mut built = false;
                let mirrored = guarded(|| {
                    let (outcome, b) = run_query_evaluation_on(&g, entry, Route::Mirror);
                    built = b;
                    outcome
                });
                tally.mirror_built += usize::from(built);
                if std::mem::discriminant(&mirrored) != std::mem::discriminant(&outcome) {
                    tally.mirror_divergences.push(format!(
                        "{}: engine {outcome:?}, mirror {mirrored:?}",
                        entry.id
                    ));
                }
            }
            EXACT_NUMERICS.with(|e| e.set(false));
            match outcome {
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
    }
    println!(
        "W3C SPARQL 1.2 {top}: {} manifests, {} entries — {} pass, {} known-fail, {} skipped",
        manifests.len(),
        tally.total,
        tally.pass,
        tally.known_fail,
        tally.skips.len()
    );
}

#[test]
fn w3c_sparql12_suite() {
    let mut tally = Tally::default();
    run_manifest(TOP, &mut tally);

    for s in &tally.skips {
        println!("  SKIP {s}");
    }
    println!(
        "W3C SPARQL 1.2 total: {} passed, {} known-fail, {} skipped, {} entries",
        tally.pass,
        tally.known_fail,
        tally.skips.len(),
        tally.total
    );
    println!(
        "Mirror parity: {} query-evaluation entries ran with the mirror built",
        tally.mirror_built
    );

    // Every KNOWN_FAILURES id must exist in the corpus, else the list is stale.
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
        tally.mirror_divergences.is_empty(),
        "{} entries end differently through the in-memory mirror than on the engine:\n  {}",
        tally.mirror_divergences.len(),
        tally.mirror_divergences.join("\n  ")
    );
    assert!(
        tally.skips.is_empty(),
        "{} entries were skipped:\n  {}",
        tally.skips.len(),
        tally.skips.join("\n  ")
    );
    // A mirror that never builds would make the parity check vacuous.
    assert!(
        tally.mirror_built >= MIRROR_FLOOR,
        "only {} entries ran with the mirror built (floor {MIRROR_FLOOR})",
        tally.mirror_built
    );
    assert!(
        tally.pass >= PASS_FLOOR,
        "only {} W3C SPARQL 1.2 entries passed (floor {PASS_FLOOR}); skips: {}",
        tally.pass,
        tally.skips.len()
    );
}
