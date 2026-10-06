//! Entailment regimes for SHACL validation: `sh:entailment` (SHACL §1.5,
//! SHACL-AF §8.3).
//!
//! A shapes graph may declare `?x sh:entailment E`. SHACL §1.5 makes every
//! such `E` either a regime the processor validates under or a failure:
//! "the processor MUST signal a failure" for a regime it does not support.
//! This module reads the declarations ([`declared_regimes`]) and runs a
//! validation under them ([`validate_entailed`]).
//!
//! Supported:
//!
//! * **`sh:Rules`** (SHACL-AF §8.3): the shapes graph's SHACL rules run
//!   before the validation, with the engine `/infer` uses
//!   ([`super::engine::infer_into`]: `sh:order`, `sh:condition`,
//!   `sh:deactivated`, rules re-run to a fixed point), and the validation sees
//!   the data plus what they inferred.
//! * **RDFS** (`http://www.w3.org/ns/entailment/RDFS`, with the
//!   `rdfs-entailment` feature): the RDFS materialiser (rdfs1–13) computes the
//!   data's RDFS entailments. With both regimes declared, rules and RDFS run
//!   in turn until neither adds a triple.
//!
//! Everything else fails the run with an error naming the regime.
//!
//! **Where the inferences live.** SHACL-AF §8.4 lets the validated data graph
//! be the original data plus a dedicated inferences graph. Here both live in
//! a run-local in-memory store: the run copies its data graphs and its shapes
//! graph into it, materialises into an inferences graph there, validates the
//! data graphs plus that graph, and drops the store. Nothing is written to the
//! caller's store, so the regime needs no write authority, works the same on
//! the scratch store a write gate stages a write in, and cannot leak an
//! inference into another reader's view. Rules read the run's data graphs and
//! the inferences graph only (`construct_confined`, the run's `DataScope`) —
//! never the copied shapes graph, unless it is one of the data graphs.
//!
//! The cost is the copy: a run under a regime holds its data graphs in memory
//! once more. Runs of shapes graphs that declare no regime are unaffected.

use super::report::ValidationReport;
use crate::store::TripleStore;
use oxigraph::model::{GraphName, GraphNameRef, NamedNode, NamedNodeRef, Quad, Term};
use oxigraph::store::{QuadIter, Transaction};
use tracing::{info, warn};

const SH_ENTAILMENT: &str = "http://www.w3.org/ns/shacl#entailment";
/// SHACL-AF §8.3: "execute the rules of the shapes graph before validation".
pub(crate) const SH_RULES: &str = "http://www.w3.org/ns/shacl#Rules";
/// The RDFS entailment regime (SPARQL 1.1 Entailment Regimes, RDF 1.1 Semantics).
pub(crate) const RDFS: &str = "http://www.w3.org/ns/entailment/RDFS";

/// The run's data source in its metrics and telemetry: a run-local copy with
/// the entailment's inferences beside it.
pub(crate) const SOURCE_KIND: &str = "entailed";

/// Names inside the run-local store. Nothing outside it ever sees them; a data
/// graph of the same name only moves them aside ([`fresh_name`]).
const INFERENCES_GRAPH: &str = "urn:x-ots:shacl-run:inferences";
const DEFAULT_GRAPH_STAND_IN: &str = "urn:x-ots:shacl-run:default-graph";

/// Quads copied into the run-local store per insert.
const COPY_CHUNK: usize = 50_000;

/// Rounds of "RDFS, then rules" when both regimes are declared. Each of them
/// runs to its own fixed point within a round; the bound only stops a pair
/// that keeps feeding each other (a rule minting a fresh blank node per run).
const MAX_ROUNDS: usize = 16;

/// The regimes a shapes graph declares, all of them supported.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Regimes {
    pub rules: bool,
    pub rdfs: bool,
}

impl Regimes {
    pub(crate) fn is_empty(&self) -> bool {
        !self.rules && !self.rdfs
    }
}

/// The `sh:entailment` values in `shapes_graph`. `Err` — naming every
/// unsupported value — when one of them is a regime this build does not
/// validate under (SHACL §1.5): the shapes graph then fails, it is never
/// validated as though the declaration were not there.
pub(crate) fn declared_regimes(store: &TripleStore, shapes_graph: &str) -> Result<Regimes, String> {
    let Ok(graph) = NamedNodeRef::new(shapes_graph) else {
        // Not an IRI: it holds no triples, and loading its shapes fails.
        return Ok(Regimes::default());
    };
    let predicate = NamedNodeRef::new_unchecked(SH_ENTAILMENT);
    let mut regimes = Regimes::default();
    let mut unsupported: Vec<String> = Vec::new();
    for quad in store
        .store()
        .quads_for_pattern(None, Some(predicate), None, Some(graph.into()))
    {
        let quad = quad.map_err(|e| format!("reading sh:entailment of <{shapes_graph}>: {e}"))?;
        match &quad.object {
            Term::NamedNode(n) if n.as_str() == SH_RULES => regimes.rules = true,
            Term::NamedNode(n) if n.as_str() == RDFS && cfg!(feature = "rdfs-entailment") => {
                regimes.rdfs = true
            }
            other => unsupported.push(other.to_string()),
        }
    }
    if unsupported.is_empty() {
        return Ok(regimes);
    }
    unsupported.sort();
    unsupported.dedup();
    let rdfs = if cfg!(feature = "rdfs-entailment") {
        format!(" and <{RDFS}>")
    } else {
        format!(" (<{RDFS}> needs a build with the rdfs-entailment feature)")
    };
    Err(format!(
        "shapes graph <{shapes_graph}> declares sh:entailment {}: an entailment regime this \
         processor does not support, so it cannot validate with this shapes graph \
         (SHACL §1.5). Supported: <{SH_RULES}>{rdfs}",
        unsupported.join(", ")
    ))
}

/// Validate `data_graphs` of `store` against `shapes_graph` under `regimes`
/// (see the module docs). `store` is only read.
pub(crate) fn validate_entailed(
    store: &TripleStore,
    shapes_graph: &str,
    data_graphs: &[String],
    regimes: &Regimes,
) -> Result<ValidationReport, String> {
    let scratch = TripleStore::in_memory()
        .map_err(|e| format!("entailment: cannot create the run's store: {e}"))?;
    // The functions the same run sees in `store`, so it computes the same.
    scratch.inherit_registered_functions(store);

    let mut taken: Vec<&str> = data_graphs.iter().map(String::as_str).collect();
    taken.push(shapes_graph);
    let inferences = fresh_name(INFERENCES_GRAPH, &taken);

    // One read snapshot for every copy, as a run's `DataView` takes one: a
    // write landing mid-copy must not leave the run with two instants.
    let snapshot = if store.is_persistent() {
        store.store().start_transaction().ok()
    } else {
        None
    };
    let source = Source {
        store,
        snapshot: snapshot.as_ref(),
    };

    // Each data graph under its own name. A run that names none validates the
    // unnamed default graph, which is copied under a stand-in name so that the
    // inferences graph can be read beside it. A data graph that is not an IRI
    // holds nothing and is skipped, as the run's view skips it.
    let mut run_graphs: Vec<String> = Vec::new();
    if data_graphs.is_empty() {
        let stand_in = fresh_name(DEFAULT_GRAPH_STAND_IN, &taken);
        source.copy(GraphNameRef::DefaultGraph, &scratch, &stand_in)?;
        run_graphs.push(stand_in);
    } else {
        for g in data_graphs {
            let Ok(name) = NamedNodeRef::new(g) else {
                continue;
            };
            if !run_graphs.contains(g) {
                source.copy(name.into(), &scratch, g)?;
                run_graphs.push(g.clone());
            }
        }
    }
    // The shapes graph, for the shapes and rules to load from.
    if !run_graphs.iter().any(|g| g == shapes_graph) {
        if let Ok(name) = NamedNodeRef::new(shapes_graph) {
            source.copy(name.into(), &scratch, shapes_graph)?;
        }
    }
    drop(snapshot);

    let inferred = entail(&scratch, shapes_graph, &run_graphs, &inferences, regimes)?;
    info!(
        "SHACL entailment (rules: {}, rdfs: {}): {inferred} triples inferred for the run of <{shapes_graph}>",
        regimes.rules, regimes.rdfs
    );

    let mut graphs = run_graphs;
    graphs.push(inferences);
    let (mut report, _) = super::engine::validate_graphs(&scratch, shapes_graph, &graphs)?;
    if let Some(metrics) = report.metrics.as_mut() {
        // The inferences graph is the run's, not one of the caller's graphs.
        metrics.graphs = metrics.graphs.saturating_sub(1);
    }
    Ok(report)
}

/// Run the regimes over `run_graphs` of the run-local store, materialising
/// into `inferences`. Returns the size of the inferences graph.
fn entail(
    scratch: &TripleStore,
    shapes_graph: &str,
    run_graphs: &[String],
    inferences: &str,
    regimes: &Regimes,
) -> Result<usize, String> {
    let count = || {
        scratch
            .count_graph(Some(inferences))
            .map_err(|e| format!("entailment: {e}"))
    };
    // Rules read what earlier rules and RDFS inferred.
    let mut scope = run_graphs.to_vec();
    scope.push(inferences.to_string());
    let mut rounds = 0;
    loop {
        rounds += 1;
        let before = count()?;
        if regimes.rdfs {
            rdfs(scratch, run_graphs, inferences)?;
        }
        if regimes.rules {
            super::engine::infer_into(scratch, shapes_graph, &scope, Some(inferences))
                .map_err(|e| format!("sh:entailment sh:Rules: {e}"))?;
        }
        let after = count()?;
        // A regime on its own reaches its fixed point in one pass; two feed
        // each other until a round adds nothing.
        if !(regimes.rules && regimes.rdfs) || after == before {
            return Ok(after);
        }
        if rounds >= MAX_ROUNDS {
            warn!(
                "SHACL entailment for <{shapes_graph}>: rules and RDFS still inferring after \
                 {MAX_ROUNDS} rounds; validating what was inferred so far"
            );
            return Ok(after);
        }
    }
}

#[cfg(feature = "rdfs-entailment")]
fn rdfs(scratch: &TripleStore, sources: &[String], target: &str) -> Result<(), String> {
    crate::reasoning::rdfs::RdfsMaterializer::with_target(scratch, target)
        .with_sources(sources.to_vec())
        .materialize()
        .map(|_| ())
        .map_err(|e| format!("sh:entailment RDFS: {e}"))
}

#[cfg(not(feature = "rdfs-entailment"))]
fn rdfs(_scratch: &TripleStore, _sources: &[String], _target: &str) -> Result<(), String> {
    // `declared_regimes` refuses the regime in such a build.
    Err(format!(
        "sh:entailment <{RDFS}> needs a build with the rdfs-entailment feature"
    ))
}

/// `base`, or `base-1`, `base-2` … — the first not in `taken`.
fn fresh_name(base: &str, taken: &[&str]) -> String {
    let mut name = base.to_string();
    let mut n = 0;
    while taken.contains(&name.as_str()) {
        n += 1;
        name = format!("{base}-{n}");
    }
    name
}

/// Where the run's copies are read from: one RocksDB snapshot on a persistent
/// store, else the live store (the memory backend's transaction would hold its
/// exclusive write lock for the whole copy).
struct Source<'a, 'b> {
    store: &'a TripleStore,
    snapshot: Option<&'a Transaction<'b>>,
}

impl Source<'_, '_> {
    fn quads(&self, graph: GraphNameRef<'_>) -> QuadIter<'_> {
        match self.snapshot {
            Some(tx) => tx.quads_for_pattern(None, None, None, Some(graph)),
            None => self
                .store
                .store()
                .quads_for_pattern(None, None, None, Some(graph)),
        }
    }

    /// Copy `graph` into `target` of `to`, quad by quad: blank nodes keep
    /// their identity, so a report's focus nodes are the caller's.
    fn copy(&self, graph: GraphNameRef<'_>, to: &TripleStore, target: &str) -> Result<(), String> {
        let target = GraphName::NamedNode(
            NamedNode::new(target).map_err(|e| format!("entailment: graph <{target}>: {e}"))?,
        );
        let mut chunk: Vec<Quad> = Vec::new();
        for quad in self.quads(graph) {
            let quad = quad.map_err(|e| format!("entailment: reading {graph}: {e}"))?;
            chunk.push(Quad::new(
                quad.subject,
                quad.predicate,
                quad.object,
                target.clone(),
            ));
            if chunk.len() == COPY_CHUNK {
                to.insert_quads(std::mem::take(&mut chunk))
                    .map_err(|e| format!("entailment: copying {graph}: {e}"))?;
            }
        }
        if !chunk.is_empty() {
            to.insert_quads(chunk)
                .map_err(|e| format!("entailment: copying {graph}: {e}"))?;
        }
        Ok(())
    }
}
