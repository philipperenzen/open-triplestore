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
//! **Evaluation** (SHACL-AF §5). A function's body is an `sh:select` or
//! `sh:ask` query, parsed once when the function is registered, with its
//! `sh:prefixes` prologue (following `owl:imports`, as for constraints and
//! rules). A call binds each argument to its parameter's variable as an RDF
//! **term** (`substitute_variable`) — never by pasting text, which also
//! rewrote `$xy` when binding `$x`. The body then reads the data of the query
//! that called it: the run's own source (its snapshot, its RAM copy or the live
//! memory store) confined to the run's data graphs ([`DataScope`]), so a body
//! can no more name another graph than the constraint calling it can. Nested
//! calls are bounded ([`MAX_CALL_DEPTH`]). Outside a shapes run — a designated
//! function called from `/sparql` — there is no run to read, and a body sees
//! an empty dataset.

use crate::store::TripleStore;
use opengraph::spargebra::algebra::GraphPattern;
use opengraph::spargebra::Query as SpargebraQuery;
use oxigraph::model::{GraphNameRef, NamedNodeRef, NamedOrBlankNode, TermRef};
use oxigraph::sparql::{
    PreparedSparqlQuery, QueryResults, QuerySolutionIter, QueryTripleIter, SparqlEvaluator,
    Variable,
};
use oxigraph::store::{Store, Transaction};
use oxrdf::{NamedNode, Term};
use std::cell::{Cell, RefCell};
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
            let handler = match handler {
                Ok(h) => h,
                Err(e) => {
                    warn!("{e}; skipped");
                    continue;
                }
            };
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
    let declared = functions_in_graph(store, shapes_graph)
        .into_iter()
        .map(|(iri, handler)| handler.map(|h| (iri, h)))
        .collect::<Result<Vec<_>, String>>()?;
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

/// Every named `sh:SPARQLFunction` declared in `graph`, each with its handler
/// ready to register via `SparqlEvaluator::with_custom_function`, or the reason
/// its declaration cannot be used (a body that does not parse, a `SELECT` with
/// more than one result variable …). Empty (cheap) when the graph declares
/// none or is not a valid graph name.
pub(crate) fn functions_in_graph(
    store: &TripleStore,
    graph: &str,
) -> Vec<(NamedNode, Result<FnHandler, String>)> {
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
            _ => continue, // only named functions are callable (§5.4)
        };
        let handler = FunctionBody::load(store, iri.as_str(), graph)
            .map(|body| body.into_handler())
            .map_err(|e| format!("sh:SPARQLFunction <{}> in <{graph}>: {e}", iri.as_str()));
        out.push((iri, handler));
    }
    out
}

/// Per parameter, in call order: whether the `sh:SPARQLFunction` `iri`
/// declared in `graph` may be called without it (`sh:optional true`). Empty
/// when `graph` declares no usable function `iri` — a built-in, say — whose
/// arguments are then all mandatory.
pub(crate) fn parameter_optionality(store: &TripleStore, graph: &str, iri: &str) -> Vec<bool> {
    FunctionBody::load(store, iri, graph)
        .map(|body| body.params.iter().map(|p| p.optional).collect())
        .unwrap_or_default()
}

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";

/// How deep function calls may nest — a body calling a function whose body
/// calls a function … — before a call is answered unbound. A function that
/// calls itself would otherwise recurse until the thread's stack overflows.
pub(crate) const MAX_CALL_DEPTH: usize = 16;

/// One parameter of a function: the body's variable (the local name of its
/// `sh:path`) and whether a call may leave it out (`sh:optional true`).
#[derive(Clone, Debug)]
struct Parameter {
    var: Variable,
    optional: bool,
}

/// A parsed `sh:SPARQLFunction` body.
#[derive(Clone, Debug)]
struct FunctionBody {
    /// The query, with every parameter variable visible at its top level so it
    /// can be bound as a term (spareval binds only top-level variables).
    query: SpargebraQuery,
    /// The `SELECT`'s single result variable; `None` for an `sh:ask` body.
    result: Option<Variable>,
    /// In call order (§5.2).
    params: Vec<Parameter>,
}

impl FunctionBody {
    /// Read and parse the function `iri` declared in `graph`.
    fn load(store: &TripleStore, iri: &str, graph: &str) -> Result<Self, String> {
        let objects = |subject: &str, p: &str| {
            store.objects_for_subject_in_graph(subject, &format!("{SH}{p}"), Some(graph))
        };
        let (text, is_ask) = match (objects(iri, "select").first(), objects(iri, "ask").first()) {
            (Some(select), None) => (lexical(select), false),
            (None, Some(ask)) => (lexical(ask), true),
            (Some(_), Some(_)) => return Err("has both sh:select and sh:ask".to_string()),
            (None, None) => return Err("has neither sh:select nor sh:ask".to_string()),
        };

        let mut params: Vec<(Option<f64>, String, bool)> = Vec::new();
        for p in objects(iri, "parameter") {
            let node = lexical(&p);
            let Some(path) = objects(&node, "path").first().map(lexical) else {
                return Err("a sh:parameter has no sh:path".to_string());
            };
            let order = objects(&node, "order")
                .first()
                .and_then(|o| lexical(o).trim().parse::<f64>().ok());
            let optional = objects(&node, "optional")
                .first()
                .is_some_and(|o| lexical(o) == "true");
            params.push((order, local_name(&path), optional));
        }
        // §5.2: by sh:order (0 when unspecified) if any parameter has one,
        // otherwise by the local names of the paths.
        if params.iter().any(|(o, _, _)| o.is_some()) {
            params.sort_by(|a, b| a.0.unwrap_or(0.0).total_cmp(&b.0.unwrap_or(0.0)));
        } else {
            params.sort_by(|a, b| a.1.cmp(&b.1));
        }
        let params = params
            .into_iter()
            .map(|(_, name, optional)| {
                Variable::new(&name)
                    .map(|var| Parameter { var, optional })
                    .map_err(|e| format!("parameter `{name}` is not a SPARQL variable name: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let prologue = super::engine::sparql_prefixes(store, graph, iri);
        let query = crate::sparql::parser()
            .parse_query(&format!("{prologue}{text}"))
            .map_err(|e| format!("body does not parse: {e}"))?;
        let vars: Vec<Variable> = params.iter().map(|p| p.var.clone()).collect();
        let (query, result) = match (query, is_ask) {
            (
                SpargebraQuery::Ask {
                    dataset,
                    mut pattern,
                    base_iri,
                },
                true,
            ) => {
                // The parser projects an ASK pattern onto no variable at all:
                // add the parameters, so their bindings reach the pattern.
                project_onto(&mut pattern, &vars);
                let query = SpargebraQuery::Ask {
                    dataset,
                    pattern,
                    base_iri,
                };
                (query, None)
            }
            (
                SpargebraQuery::Select {
                    dataset,
                    mut pattern,
                    base_iri,
                },
                false,
            ) => {
                let result = project_parameters(&mut pattern, &vars)?;
                let query = SpargebraQuery::Select {
                    dataset,
                    pattern,
                    base_iri,
                };
                (query, Some(result))
            }
            (_, true) => return Err("sh:ask must be an ASK query".to_string()),
            (_, false) => return Err("sh:select must be a SELECT query".to_string()),
        };
        Ok(Self {
            query,
            result,
            params,
        })
    }

    fn into_handler(self) -> FnHandler {
        Arc::new(move |args: &[Term]| self.call(args))
    }

    /// One call (§5.4): `None` — an error, unbound in a `BIND` — when a
    /// mandatory argument is missing, there are too many, the body fails, or
    /// the `SELECT`'s result variable is unbound or has no solution.
    fn call(&self, args: &[Term]) -> Option<Term> {
        if args.len() > self.params.len() || self.params[args.len()..].iter().any(|p| !p.optional) {
            return None;
        }
        let _depth = CallDepth::enter()?;
        current_scope(|scope| match scope {
            Some(scope) => self.evaluate(scope, args),
            None => {
                // Not called from a shapes run: no data to read.
                let store = empty_store()?;
                let evaluator = store.query_options();
                let scope = DataScope {
                    source: ScopeSource::Store(store.store()),
                    data_graphs: &[],
                    evaluator: &evaluator,
                };
                self.evaluate(&scope, args)
            }
        })
    }

    fn evaluate(&self, scope: &DataScope<'_>, args: &[Term]) -> Option<Term> {
        // Unoptimized: the optimizer does not know the arguments are bound,
        // so it treats them as unbound — it drops `BIND ($x AS ?r)` and turns
        // `?v = $x` into `sameTerm`. A body is a small query; correctness
        // costs little here.
        let mut prepared = without_optimizer(scope.evaluator.clone()).for_query(self.query.clone());
        // Always confined: a body reads the run's data graphs (the default
        // graph when the run names none) whatever FROM / GRAPH it says.
        crate::store::engine::confine_dataset(prepared.dataset_mut(), scope.data_graphs).ok()?;
        for (param, arg) in self.params.iter().zip(args) {
            prepared = prepared.substitute_variable(param.var.clone(), arg.clone());
        }
        match scope.execute(prepared).ok()? {
            QueryResults::Boolean(b) => Some(Term::Literal(oxrdf::Literal::new_typed_literal(
                if b { "true" } else { "false" },
                NamedNode::new_unchecked(XSD_BOOLEAN),
            ))),
            QueryResults::Solutions(mut rows) => {
                let var = self.result.as_ref()?;
                rows.next()?.ok()?.get(var).cloned()
            }
            QueryResults::Graph(_) => None,
        }
    }
}

/// Add `vars` to the outermost projection of `pattern` (under any solution
/// modifiers), or wrap the pattern in one: spareval binds a variable as a term
/// only when it is a top-level variable of the query, and passes the binding
/// into the pattern through the projection.
pub(crate) fn project_onto(pattern: &mut GraphPattern, vars: &[Variable]) {
    let mut p = pattern;
    loop {
        match p {
            GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. }
            | GraphPattern::OrderBy { inner, .. } => p = inner,
            GraphPattern::Project { variables, .. } => {
                for v in vars {
                    if !variables.contains(v) {
                        variables.push(v.clone());
                    }
                }
                return;
            }
            other => {
                if !vars.is_empty() {
                    let inner = std::mem::replace(
                        other,
                        GraphPattern::Bgp {
                            patterns: Vec::new(),
                        },
                    );
                    *other = GraphPattern::Project {
                        inner: Box::new(inner),
                        variables: vars.to_vec(),
                    };
                }
                return;
            }
        }
    }
}

/// Make every parameter variable a top-level variable of a function's
/// `SELECT` by adding it to the projection — the bindings then flow into the
/// pattern — and return the body's single result variable (§5.4).
fn project_parameters(pattern: &mut GraphPattern, params: &[Variable]) -> Result<Variable, String> {
    let mut p = pattern;
    loop {
        match p {
            GraphPattern::Distinct { inner }
            | GraphPattern::Reduced { inner }
            | GraphPattern::Slice { inner, .. }
            | GraphPattern::OrderBy { inner, .. } => p = inner,
            GraphPattern::Project { variables, .. } => {
                let [result] = variables.as_slice() else {
                    return Err(format!(
                        "sh:select must have exactly one result variable, not {}",
                        variables.len()
                    ));
                };
                let result = result.clone();
                if params.contains(&result) {
                    return Err(format!("the result variable ?{result} is a parameter"));
                }
                for v in params {
                    if !variables.contains(v) {
                        variables.push(v.clone());
                    }
                }
                return Ok(result);
            }
            _ => return Err("sh:select has no projection".to_string()),
        }
    }
}

/// `evaluator` with the query optimizer off, for a query whose variables are
/// bound with `substitute_variable`: the optimizer types those variables as
/// never bound and rewrites the query accordingly (see
/// [`FunctionBody::evaluate`]). `without_optimizations` is hidden from
/// oxigraph's documentation but public and stable in 0.5.
pub(crate) fn without_optimizer(evaluator: SparqlEvaluator) -> SparqlEvaluator {
    evaluator.without_optimizations()
}

/// Where a function body called during a query reads from: the data source,
/// data graphs and evaluator of the query that called it.
#[derive(Clone, Copy)]
pub(crate) enum ScopeSource<'a> {
    /// A RocksDB snapshot: one shapes run's consistent view.
    Transaction(&'a Transaction<'a>),
    /// A store: the accelerator's RAM copy or the live memory store.
    Store(&'a Store),
}

/// The data a query evaluated for a shapes run reads, which a function body
/// it calls reads too. A query run through [`DataScope::execute`] makes this
/// scope the current one on the evaluating thread for as long as it evaluates
/// — including every later step of its lazily evaluated results — so a
/// function handler, which spareval calls with nothing but its arguments,
/// finds it with [`current_scope`].
#[derive(Clone, Copy)]
pub(crate) struct DataScope<'a> {
    pub(crate) source: ScopeSource<'a>,
    /// The run's data graphs; empty means the default graph.
    pub(crate) data_graphs: &'a [String],
    /// The run's evaluator, so a body can call the run's other functions.
    pub(crate) evaluator: &'a SparqlEvaluator,
}

impl<'a> DataScope<'a> {
    /// Execute `prepared` on the scope's source, with this scope current
    /// during its evaluation. The caller has set the query's dataset.
    pub(crate) fn execute(
        self,
        prepared: PreparedSparqlQuery,
    ) -> Result<QueryResults<'a>, oxigraph::sparql::QueryEvaluationError> {
        let results = in_scope(&self, || match self.source {
            ScopeSource::Transaction(tx) => prepared.on_transaction(tx).execute(),
            ScopeSource::Store(store) => prepared.on_store(store).execute(),
        })?;
        Ok(match results {
            QueryResults::Solutions(rows) => {
                let variables: Arc<[Variable]> = rows.variables().into();
                QueryResults::Solutions(QuerySolutionIter::new(
                    variables,
                    Scoped {
                        scope: self,
                        inner: rows,
                    },
                ))
            }
            QueryResults::Graph(triples) => QueryResults::Graph(QueryTripleIter::new(Scoped {
                scope: self,
                inner: triples,
            })),
            boolean => boolean,
        })
    }
}

/// A lazily evaluated result whose every step runs with `scope` current.
struct Scoped<'a, I> {
    scope: DataScope<'a>,
    inner: I,
}

impl<I: Iterator> Iterator for Scoped<'_, I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<I::Item> {
        let scope = self.scope;
        let inner = &mut self.inner;
        in_scope(&scope, || inner.next())
    }
}

thread_local! {
    /// The scopes of the queries evaluating on this thread, innermost last.
    /// Each entry points at a [`DataScope`] that [`in_scope`] borrows for
    /// exactly as long as the entry is on the stack.
    static SCOPES: RefCell<Vec<*const ()>> = const { RefCell::new(Vec::new()) };
    /// Function calls in progress on this thread (see [`MAX_CALL_DEPTH`]).
    static CALL_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Run `f` with `scope` as this thread's current scope.
fn in_scope<R>(scope: &DataScope<'_>, f: impl FnOnce() -> R) -> R {
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            SCOPES.with(|s| {
                s.borrow_mut().pop();
            });
        }
    }
    SCOPES.with(|s| {
        s.borrow_mut()
            .push(scope as *const DataScope<'_> as *const ())
    });
    let _pop = Pop; // pops on every exit, unwinding included
    f()
}

/// Call `f` with this thread's current scope, if a query evaluating on it set
/// one. `f` cannot keep the reference: its lifetime ends with the call.
fn current_scope<R>(f: impl for<'s> FnOnce(Option<&'s DataScope<'s>>) -> R) -> R {
    match SCOPES.with(|s| s.borrow().last().copied()) {
        // SAFETY: the pointer was pushed by `in_scope` from a live
        // `&DataScope`, and is popped before that borrow ends (the pop runs in
        // a drop guard, so unwinding pops it too). An entry is on this
        // thread's stack only while the `in_scope` call that pushed it is
        // running *on this thread*, and we are on this thread inside it: the
        // spareval evaluation calling this function handler runs
        // synchronously within that call. The higher-ranked lifetime keeps
        // the reference from escaping `f`.
        Some(ptr) => f(Some(unsafe { &*(ptr as *const DataScope<'_>) })),
        None => f(None),
    }
}

/// A guard counting one function call in progress on this thread; `None`
/// when [`MAX_CALL_DEPTH`] calls already are.
struct CallDepth;

impl CallDepth {
    fn enter() -> Option<Self> {
        CALL_DEPTH.with(|d| {
            if d.get() >= MAX_CALL_DEPTH {
                warn!(
                    "sh:SPARQLFunction calls nested more than {MAX_CALL_DEPTH} deep; the call is \
                     unbound"
                );
                None
            } else {
                d.set(d.get() + 1);
                Some(CallDepth)
            }
        })
    }
}

impl Drop for CallDepth {
    fn drop(&mut self) {
        CALL_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// An empty store for the bodies of functions called outside a shapes run.
/// Nothing ever writes to it, and `TripleStore::query_options` takes `&self`,
/// so one shared instance serves every thread.
fn empty_store() -> Option<&'static TripleStore> {
    static EMPTY: OnceLock<Option<TripleStore>> = OnceLock::new();
    EMPTY.get_or_init(|| TripleStore::in_memory().ok()).as_ref()
}

fn lexical(t: &Term) -> String {
    match t {
        Term::NamedNode(nn) => nn.as_str().to_string(),
        Term::Literal(l) => l.value().to_string(),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        #[cfg(feature = "rdf-12")]
        Term::Triple(t) => t.to_string(),
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

    /// Called outside a shapes run, a handler evaluates on the shared empty
    /// store; repeated invocations (one per binding row in real queries) must
    /// keep returning the right value without rebuilding a store each time.
    #[test]
    fn handler_evaluates_repeatedly_on_shared_store() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(FN_TTL, RdfFormat::Turtle, Some("urn:fns"))
            .unwrap();
        let fns = functions_in_graph(&store, "urn:fns");
        assert_eq!(fns.len(), 1, "ex:double should be discovered");
        let (iri, handler) = &fns[0];
        let handler = handler.as_ref().expect("the body parses");
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
