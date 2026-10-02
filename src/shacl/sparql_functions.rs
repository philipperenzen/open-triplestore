//! SHACL-AF `sh:SPARQLFunction` registration (SHACL Advanced Features §5).
//!
//! A `sh:SPARQLFunction` defines a user function whose body is a SPARQL `SELECT`
//! projecting a single variable (`sh:ask` bodies are not supported yet). Each is
//! registered as an Oxigraph custom function so it is callable from SHACL-SPARQL
//! constraints and rules — e.g. the reference example's
//! `ex:distanceMetres(geomA, geomB)` wrapping `geof:distance`.
//!
//! **Scope.** Oxigraph consults custom functions before its own `xsd:` casts,
//! and a later registration replaces an earlier one, so a function registered
//! for everyone could redefine `xsd:integer(…)` or `geof:sfWithin` for every
//! other tenant's queries, write gates and pipelines. Therefore:
//!
//! * a function belongs to the runs of the shapes graph that declares it
//!   (`TripleStore::query_options_for_shapes`); no other run sees it;
//! * plain queries — `/sparql`, SPARQL Update, the reasoners — see only the
//!   functions in the graphs an admin designates with
//!   `OTS_SPARQL_FUNCTION_GRAPHS` ([`Registry`]). A designated graph must be
//!   [`DESIGNATED_GRAPH`] or lie under it: `urn:system:` graphs are never a
//!   dataset's to hold, so only an admin can write one;
//! * no function may take an IRI in a namespace the server or the standards
//!   own, or one the server registers itself ([`reserved_reason`]). A shapes
//!   graph that tries fails its run; a designated graph's definition is skipped
//!   with a warning, so one bad definition cannot break every query.
//!
//! **Evaluation model.** Definitions are discovered through the raw quad index
//! (never via `store.query`, which would recurse through `query_options`). The body
//! is evaluated against a *shared empty in-memory store* with the parameters
//! textually bound, which (a) avoids re-entrantly querying the store under
//! evaluation and (b) fully supports "expression" functions whose `WHERE` clause is empty. A
//! function whose body actually queries data is not supported in this form and
//! returns unbound — a documented limitation; express such logic as a
//! SHACL-SPARQL constraint instead.

use crate::store::TripleStore;
use oxigraph::model::{GraphNameRef, NamedNodeRef, NamedOrBlankNode, Term as OxTerm, TermRef};
use oxrdf::{NamedNode, Term};
use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use tracing::warn;

pub type FnHandler = Arc<dyn Fn(&[Term]) -> Option<Term> + Send + Sync>;

/// A set of registered functions, shared between the evaluators built from it.
pub type Functions = Arc<Vec<(NamedNode, FnHandler)>>;

/// The graph, or graph prefix, an admin may designate with
/// `OTS_SPARQL_FUNCTION_GRAPHS`.
pub const DESIGNATED_GRAPH: &str = "urn:system:functions";

/// Whether `iri` may be designated: [`DESIGNATED_GRAPH`] itself or a graph
/// under `urn:system:functions:`.
pub fn is_designatable_graph(iri: &str) -> bool {
    iri == DESIGNATED_GRAPH
        || iri
            .strip_prefix(DESIGNATED_GRAPH)
            .and_then(|rest| rest.strip_prefix(':'))
            .is_some_and(|name| !name.is_empty())
}

/// Namespaces no `sh:SPARQLFunction` may define a function in: the XSD casts
/// and the vocabularies whose functions are the standards' or the server's.
const RESERVED_NAMESPACES: &[&str] = &[
    "http://www.w3.org/2001/XMLSchema#",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
    "http://www.w3.org/2000/01/rdf-schema#",
    "http://www.w3.org/2002/07/owl#",
    "http://www.w3.org/ns/shacl#",
    "http://www.w3.org/ns/sparql#",
    "http://www.w3.org/2005/xpath-functions#",
    "http://www.w3.org/2005/xpath-functions/math#",
    "http://www.opengis.net/def/function/geosparql/",
    "https://open-triplestore.org/def/function/",
];

/// Why a `sh:SPARQLFunction` may not take `iri`, or `None` when it may.
/// `builtins` are the IRIs the server registers itself.
pub fn reserved_reason(iri: &str, builtins: &HashSet<String>) -> Option<&'static str> {
    if builtins.contains(iri) {
        return Some("the server defines a function with this IRI");
    }
    if RESERVED_NAMESPACES.iter().any(|ns| iri.starts_with(ns)) {
        return Some("its namespace is reserved for XSD casts and standard or built-in functions");
    }
    None
}

/// The functions every query of a store sees: those of the graphs an admin
/// designated (`OTS_SPARQL_FUNCTION_GRAPHS`), or a fixed set handed down from
/// another store (a write gate's scratch store inherits the live store's).
#[derive(Default)]
pub(crate) struct Registry {
    graphs: Vec<String>,
    inherited: Option<Functions>,
    /// Discovered from `graphs`, keyed by the write generation it was read at.
    cached: Option<(u64, Functions)>,
}

impl Registry {
    /// The designation in `OTS_SPARQL_FUNCTION_GRAPHS` (comma-separated),
    /// read once per process. An entry that may not be designated is ignored
    /// with a warning.
    pub(crate) fn from_env() -> Self {
        static FROM_ENV: OnceLock<Vec<String>> = OnceLock::new();
        let graphs = FROM_ENV.get_or_init(|| {
            let raw = std::env::var("OTS_SPARQL_FUNCTION_GRAPHS").unwrap_or_default();
            raw.split(',')
                .map(str::trim)
                .filter(|g| !g.is_empty())
                .filter(|g| {
                    let ok = is_designatable_graph(g);
                    if !ok {
                        warn!(
                            "OTS_SPARQL_FUNCTION_GRAPHS: <{g}> ignored: a function graph must \
                             be <{DESIGNATED_GRAPH}> or start with {DESIGNATED_GRAPH}:, which \
                             only an admin can write"
                        );
                    }
                    ok
                })
                .map(str::to_string)
                .collect()
        });
        Self {
            graphs: graphs.clone(),
            ..Self::default()
        }
    }

    /// Whether `graph` is one of the designated function graphs.
    pub(crate) fn designates(&self, graph: &str) -> bool {
        self.graphs.iter().any(|g| g == graph)
    }

    pub(crate) fn set_graphs(&mut self, graphs: Vec<String>) {
        self.graphs = graphs;
        self.cached = None;
    }

    pub(crate) fn inherit(&mut self, functions: Functions) {
        self.inherited = Some(functions);
        self.cached = None;
    }

    /// The registered set if it is known without reading the store: the
    /// inherited set, else the one cached at `generation`. Otherwise the
    /// graphs to discover it from.
    pub(crate) fn lookup(&self, generation: u64) -> Result<Functions, Vec<String>> {
        if let Some(f) = &self.inherited {
            return Ok(f.clone());
        }
        match &self.cached {
            Some((g, f)) if *g == generation => Ok(f.clone()),
            _ => Err(self.graphs.clone()),
        }
    }

    pub(crate) fn store_discovered(&mut self, generation: u64, functions: Functions) {
        if self.inherited.is_none() {
            self.cached = Some((generation, functions));
        }
    }
}

/// The functions declared in the admin-designated `graphs`, in order. A
/// definition at a reserved IRI ([`reserved_reason`]), or one an earlier graph
/// already defined, is skipped with a warning.
pub(crate) fn designated_functions(
    store: &TripleStore,
    graphs: &[String],
    builtins: &HashSet<String>,
) -> Vec<(NamedNode, FnHandler)> {
    let mut out: Vec<(NamedNode, FnHandler)> = Vec::new();
    for graph in graphs {
        for (iri, handler) in functions_in_graph(store, graph) {
            if let Some(why) = reserved_reason(iri.as_str(), builtins) {
                warn!(
                    "sh:SPARQLFunction <{}> in <{graph}> skipped: {why}",
                    iri.as_str()
                );
            } else if out.iter().any(|(seen, _)| *seen == iri) {
                warn!(
                    "sh:SPARQLFunction <{}> in <{graph}> skipped: an earlier function graph \
                     defines it",
                    iri.as_str()
                );
            } else {
                out.push((iri, handler));
            }
        }
    }
    out
}

/// The functions a run of `shapes_graph` adds to `registered` (the functions
/// every query sees). `Err` names the first definition that would redefine a
/// reserved or registered function: the run must fail, not quietly compute
/// with something other than what its author wrote.
pub(crate) fn shapes_graph_functions(
    store: &TripleStore,
    shapes_graph: &str,
    registered: &[(NamedNode, FnHandler)],
    builtins: &HashSet<String>,
) -> Result<Vec<(NamedNode, FnHandler)>, String> {
    let declared = functions_in_graph(store, shapes_graph);
    for (iri, _) in &declared {
        let why = reserved_reason(iri.as_str(), builtins).or_else(|| {
            registered
                .iter()
                .any(|(r, _)| r == iri)
                .then_some("an admin-designated function graph defines a function with this IRI")
        });
        if let Some(why) = why {
            return Err(format!(
                "sh:SPARQLFunction <{}> in shapes graph <{shapes_graph}> cannot be registered: \
                 {why}",
                iri.as_str()
            ));
        }
    }
    Ok(declared)
}

/// Every named `sh:SPARQLFunction` declared in `graph`, ready to register via
/// `SparqlEvaluator::with_custom_function`. Empty (cheap) when the graph
/// declares none or is not a valid graph name.
pub(crate) fn functions_in_graph(store: &TripleStore, graph: &str) -> Vec<(NamedNode, FnHandler)> {
    let Ok(graph_node) = NamedNodeRef::new(graph) else {
        return Vec::new();
    };
    let ty = NamedNodeRef::new_unchecked(RDF_TYPE);
    let class_iri = format!("{SH}SPARQLFunction");
    let class = NamedNodeRef::new_unchecked(&class_iri);

    let mut out = Vec::new();
    for quad in store.store().quads_for_pattern(
        None,
        Some(ty),
        Some(TermRef::NamedNode(class)),
        Some(GraphNameRef::NamedNode(graph_node)),
    ) {
        let Ok(quad) = quad else { continue };
        let iri = match quad.subject {
            NamedOrBlankNode::NamedNode(nn) => nn,
            _ => continue, // only named functions are callable
        };
        if let Some(handler) = build_handler(store, iri.as_str(), Some(graph)) {
            out.push((iri, handler));
        }
    }
    out
}

/// Process-wide store for evaluating function bodies. Building a `TripleStore`
/// (store allocation + `ParallelMirror`/`QueryCache` env reads) per *invocation*
/// — i.e. per binding row — was pure overhead: the store stays empty (bodies are
/// "expression" queries with the parameters textually substituted), nothing ever
/// writes to it, and `TripleStore::query` takes `&self`, so one shared instance
/// is safe to evaluate against from any thread.
fn eval_store() -> Option<&'static TripleStore> {
    static EVAL_STORE: OnceLock<Option<TripleStore>> = OnceLock::new();
    EVAL_STORE
        .get_or_init(|| TripleStore::in_memory().ok())
        .as_ref()
}

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

fn build_handler(store: &TripleStore, iri: &str, graph: Option<&str>) -> Option<FnHandler> {
    let body = obj1(store, iri, &format!("{SH}select"), graph)?;
    let prologue = prefixes(store, iri, graph);
    // Ordered parameter variable names, derived from each sh:parameter's sh:path
    // local name (the body references them as `$localName`).
    let mut params: Vec<(f64, String)> = Vec::new();
    for p in objs(store, iri, &format!("{SH}parameter"), graph) {
        let Some(path) = obj1(store, &p, &format!("{SH}path"), graph) else {
            continue;
        };
        let order = obj1(store, &p, &format!("{SH}order"), graph)
            .and_then(|o| o.parse::<f64>().ok())
            .unwrap_or(0.0);
        params.push((order, local_name(&path)));
    }
    params.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let var_names: Vec<String> = params.into_iter().map(|(_, n)| n).collect();

    let full_body = format!("{prologue}{body}");
    let handler: FnHandler = Arc::new(move |args: &[Term]| {
        let mut q = full_body.clone();
        for (i, vn) in var_names.iter().enumerate() {
            let arg = args.get(i)?;
            // oxrdf Term Display is valid SPARQL term syntax (e.g. "…"^^<dt>, <iri>).
            q = q.replace(&format!("${vn}"), &arg.to_string());
        }
        // Evaluate against the shared empty store so we never re-enter the
        // caller's query (and never pay a per-row store construction).
        let tmp = eval_store()?;
        match tmp.query(&q) {
            Ok(oxigraph::sparql::QueryResults::Solutions(sols)) => {
                for sol in sols.flatten() {
                    if let Some((_, term)) = sol.iter().next() {
                        return Some(term.clone());
                    }
                }
                None
            }
            Ok(oxigraph::sparql::QueryResults::Boolean(b)) => {
                Some(Term::Literal(oxrdf::Literal::new_typed_literal(
                    if b { "true" } else { "false" },
                    NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#boolean"),
                )))
            }
            _ => None,
        }
    });
    Some(handler)
}

fn obj1(store: &TripleStore, subj: &str, pred: &str, graph: Option<&str>) -> Option<String> {
    store
        .objects_for_subject_in_graph(subj, pred, graph)
        .into_iter()
        .next()
        .map(|t| lexical(&t))
}

fn objs(store: &TripleStore, subj: &str, pred: &str, graph: Option<&str>) -> Vec<String> {
    store
        .objects_for_subject_in_graph(subj, pred, graph)
        .iter()
        .map(lexical)
        .collect()
}

/// Build the SPARQL `PREFIX` prologue from the function node's `sh:prefixes`.
fn prefixes(store: &TripleStore, iri: &str, graph: Option<&str>) -> String {
    let mut out = String::new();
    for owner in objs(store, iri, &format!("{SH}prefixes"), graph) {
        for decl in objs(store, &owner, &format!("{SH}declare"), graph) {
            let p = obj1(store, &decl, &format!("{SH}prefix"), graph);
            let ns = obj1(store, &decl, &format!("{SH}namespace"), graph);
            if let (Some(p), Some(ns)) = (p, ns) {
                out.push_str(&format!("PREFIX {p}: <{ns}>\n"));
            }
        }
    }
    out
}

fn lexical(t: &OxTerm) -> String {
    match t {
        OxTerm::NamedNode(nn) => nn.as_str().to_string(),
        OxTerm::Literal(l) => l.value().to_string(),
        OxTerm::BlankNode(b) => format!("_:{}", b.as_str()),
        #[cfg(feature = "rdf-12")]
        OxTerm::Triple(t) => t.to_string(),
    }
}

/// Local name of an IRI: the part after the last `#` or `/`.
fn local_name(iri: &str) -> String {
    iri.rsplit(['#', '/']).next().unwrap_or(iri).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::RdfFormat;

    const FN_TTL: &str = r#"
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        @prefix ex: <http://example.org/> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:double a sh:SPARQLFunction ;
            sh:parameter [ sh:path ex:x ; sh:order 0 ] ;
            sh:returnType xsd:integer ;
            sh:select "SELECT ($x * 2 AS ?result) WHERE {}" .
    "#;

    /// Handlers evaluate on the shared process-wide store; repeated invocations
    /// (one per binding row in real queries) must keep returning the right value
    /// without rebuilding a store each time.
    #[test]
    fn handler_evaluates_repeatedly_on_shared_store() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(FN_TTL, RdfFormat::Turtle, Some("urn:fns"))
            .unwrap();
        let fns = functions_in_graph(&store, "urn:fns");
        assert_eq!(fns.len(), 1, "ex:double should be discovered");
        let (iri, handler) = &fns[0];
        assert_eq!(iri.as_str(), "http://example.org/double");
        for _ in 0..3 {
            let arg = Term::Literal(oxrdf::Literal::new_typed_literal(
                "21",
                NamedNode::new_unchecked("http://www.w3.org/2001/XMLSchema#integer"),
            ));
            match handler(&[arg]).expect("function should return a value") {
                Term::Literal(l) => assert_eq!(l.value(), "42"),
                other => panic!("expected a literal, got {other}"),
            }
        }
    }

    /// Discovery reads the one graph it is asked about, never the store.
    #[test]
    fn functions_are_discovered_per_graph() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(FN_TTL, RdfFormat::Turtle, Some("urn:other"))
            .unwrap();
        store.load_str(FN_TTL, RdfFormat::Turtle, None).unwrap();
        assert!(functions_in_graph(&store, "urn:fns").is_empty());
        assert_eq!(functions_in_graph(&store, "urn:other").len(), 1);
    }

    #[test]
    fn only_system_function_graphs_can_be_designated() {
        for ok in ["urn:system:functions", "urn:system:functions:geo"] {
            assert!(is_designatable_graph(ok), "{ok}");
        }
        for bad in [
            "urn:system:functions:",
            "urn:system:functionsx",
            "urn:system:shapes",
            "http://example.org/functions",
        ] {
            assert!(!is_designatable_graph(bad), "{bad}");
        }
    }

    #[test]
    fn reserved_iris_cover_casts_standards_and_builtins() {
        let builtins: HashSet<String> = ["http://example.org/registered".to_string()].into();
        for reserved in [
            "http://www.w3.org/2001/XMLSchema#integer",
            "http://www.opengis.net/def/function/geosparql/sfWithin",
            "http://www.w3.org/ns/sparql#adjust",
            "https://open-triplestore.org/def/function/geo3d/volume",
            "http://example.org/registered",
        ] {
            assert!(reserved_reason(reserved, &builtins).is_some(), "{reserved}");
        }
        assert!(reserved_reason("http://example.org/double", &builtins).is_none());
    }
}
