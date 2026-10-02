//! Runner for the federation sections of the W3C SPARQL 1.1 test suite
//! (w3c/rdf-tests `service/` and `syntax-fed/`, through
//! `manifest-sparql11-fed.ttl`), vendored unmodified under
//! `tests/fixtures/w3c-sparql11` (see PROVENANCE.md and LICENSE.md there).
//!
//! Those sections are a subset of a W3C test suite, used under the W3C 3-clause
//! BSD licence for development and bug tracking only. W3C's test-suite licence
//! policy allows no public performance claims on a subset, so the runner
//! states no pass count: the known-failure list and the pass floor below drive
//! the ratchet, and no score is published (see `scripts/conformance_table.py`
//! and docs/conformance/sparql11.md).
//!
//! The suite names its remote endpoints by fixed IRIs (`http://example.org/sparql`,
//! `http://example1.org/sparql`, …) and gives each one's data in a
//! `qt:serviceData` block. Each of those IRIs gets a local listener here: a
//! small SPARQL endpoint that answers `POST application/sparql-query` from a
//! `TripleStore` holding that block's data. The endpoint IRIs are replaced by
//! the listeners' URLs **in memory** — in the query, in the local and remote
//! data and in the expected results — so the files on disk stay byte-identical
//! to upstream. Only the listeners are on `OTS_REMOTE_ALLOWLIST`, so an
//! endpoint the suite expects to fail (`http://invalid.endpoint.org/sparql`)
//! is refused, as any endpoint outside the allowlist is. Each listener
//! evaluates through `TripleStore` too, so a nested `SERVICE` inside a remote
//! pattern (`service03`, `service06`) is federated again from that listener.
//!
//! - `mf:QueryEvaluationTest`: `qt:data` into the local store's default graph,
//!   each `qt:serviceData` into its listener's store, the query run through
//!   `TripleStore::query` (the evaluation path `/sparql` uses) with
//!   `BASE <query IRI>`, and the solutions compared to `mf:result` by
//!   result-set isomorphism.
//! - `mf:PositiveSyntaxTest11`: the file must parse.
//!
//! Gap policy (two-way ratchet, as in `w3c_sparql11_manifests.rs`): every
//! entry NOT in `KNOWN_FAILURES` must pass, and every listed entry must still
//! fail. A pass floor guards against a loader regression turning passes into
//! skips. One test function: the allowlist is a process-wide environment
//! variable.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use open_triplestore::store::engine::BlankNodeMode;
use open_triplestore::store::TripleStore;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::dataset::CanonicalizationAlgorithm;
use oxigraph::model::vocab::rdf;
use oxigraph::model::{
    BlankNode, Dataset, Graph, GraphName, Literal, NamedNode, NamedOrBlankNodeRef, Quad, Term,
    TermRef, TripleRef,
};
use oxigraph::sparql::results::{
    QueryResultsFormat, QueryResultsParser, QueryResultsSerializer, ReaderQueryResultsParserOutput,
};
use oxigraph::sparql::QueryResults;
use spargebra::SparqlParser;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

const SUITE_ROOT: &str = "tests/fixtures/w3c-sparql11";
/// The corpus' published location; relative IRIs resolve against it.
const BASE: &str = "https://w3c.github.io/rdf-tests/sparql/sparql11/";
const TOP: &str = "manifest-sparql11-fed.ttl";
/// The manifests' entry namespace, stripped for readable ids.
const ENTRY_NS: &str = "http://www.w3.org/2009/sparql/docs/tests/data-sparql11/";

const MF: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#";
const QT: &str = "http://www.w3.org/2001/sw/DataAccess/tests/test-query#";
const RS: &str = "http://www.w3.org/2001/sw/DataAccess/tests/result-set#";

/// Entries that currently fail, with the gap they sit behind. Keep sorted.
/// (`service/manifest#service5`, `SERVICE ?service` with the endpoint taken
/// from the local data, was one until `SERVICE ?var` became a lateral join.)
const KNOWN_FAILURES: &[(&str, &str)] = &[];

/// Pass floor: below the current count, so a loader regression that turns
/// passes into skips fails the run.
const PASS_FLOOR: usize = 9;

#[derive(Debug, PartialEq)]
enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

fn nn(ns: &str, local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{ns}{local}"))
}

fn local_path(iri: &str) -> Option<PathBuf> {
    iri.strip_prefix(BASE)
        .map(|rel| Path::new(SUITE_ROOT).join(rel))
}

fn read_text(iri: &str) -> Result<String, String> {
    let path = local_path(iri).ok_or_else(|| format!("<{iri}> is outside the corpus"))?;
    std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
}

fn graph_of(iri: &str) -> Result<Graph, String> {
    let text = read_text(iri)?;
    let mut g = Graph::new();
    for q in RdfParser::from_format(RdfFormat::Turtle)
        .with_base_iri(iri)
        .map_err(|e| e.to_string())?
        .for_slice(text.as_bytes())
    {
        let q = q.map_err(|e| format!("<{iri}>: {e}"))?;
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

fn objects(g: &Graph, s: NamedOrBlankNodeRef<'_>, p: &NamedNode) -> Vec<Term> {
    g.objects_for_subject_predicate(s, p)
        .map(|t| t.into_owned())
        .collect()
}

fn object(g: &Graph, s: NamedOrBlankNodeRef<'_>, p: &NamedNode) -> Option<Term> {
    g.object_for_subject_predicate(s, p).map(|t| t.into_owned())
}

fn list_items(g: &Graph, head: TermRef<'_>) -> Vec<Term> {
    let mut out = Vec::new();
    let mut cur = head.into_owned();
    while let Some(node) = as_node(cur.as_ref()) {
        if cur.as_ref() == TermRef::NamedNode(rdf::NIL) {
            break;
        }
        let Some(first) = object(g, node, &rdf::FIRST.into_owned()) else {
            break;
        };
        out.push(first);
        let Some(rest) = object(g, node, &rdf::REST.into_owned()) else {
            break;
        };
        cur = rest;
    }
    out
}

struct Entry {
    /// `service/manifest#service1`
    id: String,
    kind: String,
    action: Term,
    result: Option<Term>,
}

/// The included manifests' entries, in order, each with its manifest graph.
fn all_entries() -> Vec<(Graph, Entry)> {
    let top = format!("{BASE}{TOP}");
    let g = graph_of(&top).expect("top manifest");
    let root = NamedNode::new_unchecked(top.as_str());
    let mut out = Vec::new();
    for head in objects(&g, root.as_ref().into(), &nn(MF, "include")) {
        for inc in list_items(&g, head.as_ref()) {
            let Some(manifest) = iri_of(inc.as_ref()) else {
                continue;
            };
            let mg = graph_of(&manifest).expect("included manifest");
            let mroot = NamedNode::new_unchecked(manifest.as_str());
            for head in objects(&mg, mroot.as_ref().into(), &nn(MF, "entries")) {
                for item in list_items(&mg, head.as_ref()) {
                    let Some(node) = as_node(item.as_ref()) else {
                        continue;
                    };
                    let id = iri_of(item.as_ref())
                        .map(|i| i.strip_prefix(ENTRY_NS).unwrap_or(&i).to_string())
                        .unwrap_or_else(|| item.to_string());
                    let kind = object(&mg, node, &rdf::TYPE.into_owned())
                        .and_then(|t| iri_of(t.as_ref()))
                        .and_then(|t| t.strip_prefix(MF).map(str::to_string))
                        .unwrap_or_default();
                    let Some(action) = object(&mg, node, &nn(MF, "action")) else {
                        continue;
                    };
                    let result = object(&mg, node, &nn(MF, "result"));
                    out.push((
                        mg.clone(),
                        Entry {
                            id,
                            kind,
                            action,
                            result,
                        },
                    ));
                }
            }
        }
    }
    out
}

fn fresh_store() -> TripleStore {
    TripleStore::in_memory()
        .unwrap()
        .with_blank_node_mode(BlankNodeMode::Preserve)
        .with_parallel_query(false, 1, usize::MAX)
        .with_query_cache(false, 1, 1)
}

// ─── Local endpoints ──────────────────────────────────────────────────────────

/// A local SPARQL endpoint whose data the runner swaps per test.
struct Endpoint {
    url: String,
    data: Arc<RwLock<TripleStore>>,
}

async fn answer(State(data): State<Arc<RwLock<TripleStore>>>, query: String) -> Response {
    let store = data.read().unwrap().clone();
    let out = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let QueryResults::Solutions(sols) = store.query(&query).map_err(|e| e.to_string())? else {
            return Err("only SELECT is served".into());
        };
        let mut w = QueryResultsSerializer::from_format(QueryResultsFormat::Json)
            .serialize_solutions_to_writer(Vec::new(), sols.variables().to_vec())
            .map_err(|e| e.to_string())?;
        for sol in sols {
            let sol = sol.map_err(|e| e.to_string())?;
            w.serialize(sol.iter()).map_err(|e| e.to_string())?;
        }
        w.finish().map_err(|e| e.to_string())
    })
    .await
    .unwrap();
    match out {
        Ok(body) => (
            [(header::CONTENT_TYPE, "application/sparql-results+json")],
            body,
        )
            .into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, e).into_response(),
    }
}

fn endpoint() -> Endpoint {
    let data = Arc::new(RwLock::new(fresh_store()));
    let app = Router::new()
        .route("/sparql", post(answer))
        .with_state(Arc::clone(&data));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    let addr = rx.recv().unwrap();
    Endpoint {
        url: format!("http://{addr}/sparql"),
        data,
    }
}

/// Every `qt:endpoint` the entries name, in a stable order.
fn endpoint_iris(entries: &[(Graph, Entry)]) -> Vec<String> {
    let mut iris: Vec<String> = Vec::new();
    for (g, e) in entries {
        let Some(action) = as_node(e.action.as_ref()) else {
            continue;
        };
        for sd in objects(g, action, &nn(QT, "serviceData")) {
            let Some(sd) = as_node(sd.as_ref()) else {
                continue;
            };
            if let Some(iri) = object(g, sd, &nn(QT, "endpoint")).and_then(|t| iri_of(t.as_ref())) {
                if !iris.contains(&iri) {
                    iris.push(iri);
                }
            }
        }
    }
    iris.sort();
    iris
}

/// Replace every suite endpoint IRI by its listener's URL.
fn substitute(text: &str, map: &BTreeMap<String, String>) -> String {
    map.iter().fold(text.to_string(), |t, (from, to)| {
        t.replace(from.as_str(), to)
    })
}

// ─── Comparison ───────────────────────────────────────────────────────────────

type Row = Vec<(String, Term)>;

/// A result set as an RDF graph (DAWG result-set vocabulary), canonicalised,
/// so two result sets compare by isomorphism (blank nodes structurally).
fn encode(vars: &[String], rows: &[Row]) -> Dataset {
    let mut ds = Dataset::new();
    let g = GraphName::DefaultGraph;
    let root = BlankNode::default();
    ds.insert(&Quad::new(
        root.clone(),
        rdf::TYPE,
        nn(RS, "ResultSet"),
        g.clone(),
    ));
    let mut vars = vars.to_vec();
    vars.sort();
    for v in vars {
        ds.insert(&Quad::new(
            root.clone(),
            nn(RS, "resultVariable"),
            Literal::new_simple_literal(v),
            g.clone(),
        ));
    }
    for row in rows {
        let sol = BlankNode::default();
        ds.insert(&Quad::new(
            root.clone(),
            nn(RS, "solution"),
            sol.clone(),
            g.clone(),
        ));
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
            ds.insert(&Quad::new(b, nn(RS, "value"), term.clone(), g.clone()));
        }
    }
    ds.canonicalize(CanonicalizationAlgorithm::Unstable);
    ds
}

fn summary(rows: &[Row]) -> String {
    let mut lines: Vec<String> = rows
        .iter()
        .map(|r| {
            let mut cells: Vec<String> = r.iter().map(|(v, t)| format!("?{v}={t}")).collect();
            cells.sort();
            cells.join(" ")
        })
        .collect();
    lines.sort();
    lines.join("\n    ")
}

fn solutions(results: QueryResults<'_>) -> Result<(Vec<String>, Vec<Row>), String> {
    let QueryResults::Solutions(sols) = results else {
        return Err("expected solutions".into());
    };
    let vars = sols
        .variables()
        .iter()
        .map(|v| v.as_str().to_string())
        .collect();
    let mut rows = Vec::new();
    for sol in sols {
        let sol = sol.map_err(|e| format!("evaluation error: {e}"))?;
        rows.push(
            sol.iter()
                .map(|(v, t)| (v.as_str().to_string(), t.clone()))
                .collect(),
        );
    }
    Ok((vars, rows))
}

// ─── Entry runners ────────────────────────────────────────────────────────────

fn run_query_evaluation(
    g: &Graph,
    entry: &Entry,
    endpoints: &BTreeMap<String, Endpoint>,
    map: &BTreeMap<String, String>,
) -> Outcome {
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
    let load = |store: &TripleStore, iri: &str| -> Result<(), String> {
        let text = substitute(&read_text(iri)?, map);
        store
            .load_str_with_base(&text, RdfFormat::Turtle, iri, None)
            .map_err(|e| format!("load <{iri}>: {e}"))
    };

    // Every listener starts empty; the entry's serviceData fills its own.
    for ep in endpoints.values() {
        *ep.data.write().unwrap() = fresh_store();
    }
    for sd in objects(g, action, &nn(QT, "serviceData")) {
        let Some(sd) = as_node(sd.as_ref()) else {
            continue;
        };
        let (Some(ep), Some(data)) = (
            object(g, sd, &nn(QT, "endpoint")).and_then(|t| iri_of(t.as_ref())),
            object(g, sd, &nn(QT, "data")).and_then(|t| iri_of(t.as_ref())),
        ) else {
            return Outcome::Skip("qt:serviceData without endpoint or data".into());
        };
        let store = endpoints[&ep].data.read().unwrap().clone();
        if let Err(e) = load(&store, &data) {
            return Outcome::Skip(e);
        }
    }
    let local = fresh_store();
    for d in objects(g, action, &nn(QT, "data")) {
        if let Some(iri) = iri_of(d.as_ref()) {
            if let Err(e) = load(&local, &iri) {
                return Outcome::Skip(e);
            }
        }
    }

    let expected = match read_text(&result_iri).map(|t| substitute(&t, map)) {
        Ok(t) => t,
        Err(e) => return Outcome::Skip(e),
    };
    let (exp_vars, exp_rows) = match QueryResultsParser::from_format(QueryResultsFormat::Xml)
        .for_reader(expected.as_bytes())
    {
        Ok(ReaderQueryResultsParserOutput::Solutions(iter)) => {
            let vars: Vec<String> = iter
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            let mut rows = Vec::new();
            for sol in iter {
                match sol {
                    Ok(sol) => rows.push(
                        sol.iter()
                            .map(|(v, t)| (v.as_str().to_string(), t.clone()))
                            .collect::<Row>(),
                    ),
                    Err(e) => return Outcome::Skip(format!("<{result_iri}>: {e}")),
                }
            }
            (vars, rows)
        }
        Ok(_) => return Outcome::Skip("expected result is not a solution set".into()),
        Err(e) => return Outcome::Skip(format!("<{result_iri}>: {e}")),
    };

    let query = match read_text(&query_iri) {
        Ok(t) => format!("BASE <{query_iri}>\n{}", substitute(&t, map)),
        Err(e) => return Outcome::Skip(e),
    };
    let (vars, rows) = match local
        .query(&query)
        .map_err(|e| e.to_string())
        .and_then(solutions)
    {
        Ok(r) => r,
        Err(e) => return Outcome::Fail(e),
    };
    let (mut a, mut e) = (vars.clone(), exp_vars.clone());
    a.sort();
    e.sort();
    if a != e {
        return Outcome::Fail(format!(
            "projected variables differ: expected {exp_vars:?}, got {vars:?}"
        ));
    }
    if encode(&vars, &rows) == encode(&exp_vars, &exp_rows) {
        Outcome::Pass
    } else {
        Outcome::Fail(format!(
            "result sets differ\n  expected:\n    {}\n  actual:\n    {}",
            summary(&exp_rows),
            summary(&rows)
        ))
    }
}

fn run_positive_syntax(entry: &Entry) -> Outcome {
    let Some(iri) = iri_of(entry.action.as_ref()) else {
        return Outcome::Skip("action is not a file".into());
    };
    let text = match read_text(&iri) {
        Ok(t) => t,
        Err(e) => return Outcome::Skip(e),
    };
    match SparqlParser::new()
        .with_base_iri(iri.as_str())
        .map_err(|e| e.to_string())
        .and_then(|p| p.parse_query(&text).map_err(|e| e.to_string()))
    {
        Ok(_) => Outcome::Pass,
        Err(e) => Outcome::Fail(format!("must parse: {e}")),
    }
}

#[test]
fn w3c_sparql11_federation_suites() {
    let entries = all_entries();
    let endpoints: BTreeMap<String, Endpoint> = endpoint_iris(&entries)
        .into_iter()
        .map(|iri| (iri, endpoint()))
        .collect();
    let map: BTreeMap<String, String> = endpoints
        .iter()
        .map(|(iri, ep)| (iri.clone(), ep.url.clone()))
        .collect();
    std::env::set_var(
        "OTS_REMOTE_ALLOWLIST",
        map.values().cloned().collect::<Vec<_>>().join(","),
    );

    let mut passed = 0;
    let mut regressions = Vec::new();
    let mut fixed = Vec::new();
    let mut skipped = Vec::new();
    for (g, entry) in &entries {
        let outcome = match entry.kind.as_str() {
            "QueryEvaluationTest" => run_query_evaluation(g, entry, &endpoints, &map),
            "PositiveSyntaxTest11" => run_positive_syntax(entry),
            other => Outcome::Skip(format!("unhandled test type {other}")),
        };
        let known = KNOWN_FAILURES.iter().any(|(id, _)| *id == entry.id);
        match outcome {
            Outcome::Pass if known => fixed.push(entry.id.clone()),
            Outcome::Pass => passed += 1,
            Outcome::Fail(_) if known => {}
            Outcome::Fail(why) => regressions.push(format!("{}: {why}", entry.id)),
            Outcome::Skip(why) => skipped.push(format!("{}: {why}", entry.id)),
        }
    }
    for (id, _) in KNOWN_FAILURES {
        assert!(
            entries.iter().any(|(_, e)| e.id == *id),
            "KNOWN_FAILURES names {id}, which is not in the corpus"
        );
    }
    assert!(
        regressions.is_empty(),
        "entries that must pass failed:\n{}",
        regressions.join("\n")
    );
    assert!(
        fixed.is_empty(),
        "known failures now pass — remove them from KNOWN_FAILURES: {fixed:?}"
    );
    assert!(
        skipped.is_empty(),
        "runner-side skips:\n{}",
        skipped.join("\n")
    );
    assert!(passed >= PASS_FLOOR, "pass floor not met");
}
