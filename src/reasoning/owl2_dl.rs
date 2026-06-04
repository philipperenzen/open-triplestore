//! OWL 2 DL profile — native DL extension layer + external reasoner bridge.
//!
//! OWL 2 DL (SROIQ(D)) is N2EXPTIME-complete and requires a tableau algorithm
//! with blocked-node merging that cannot be fully expressed as SPARQL INSERT
//! rules.  This module provides two complementary approaches:
//!
//! 1. **[`Owl2DLReasoner`]** — a native in-process reasoner that:
//!    - First runs all ~80 OWL 2 RL forward-chaining rules.
//!    - Then applies additional DL-specific SPARQL INSERT rules for axioms
//!      expressible without a tableau: `owl:hasSelf`, `owl:disjointUnionOf`,
//!      `owl:NegativePropertyAssertion`, `owl:hasKey`, and cardinality
//!      annotations.
//!    - Detects inconsistencies raised by both RL rules and DL-specific checks.
//!
//! 2. **[`ExternalReasonerBridge`]** — delegates to any `ExternalReasoner`
//!    (HermiT, Pellet, ELK, Konclude, …) after running the native DL rules.
//!
//! 3. **[`NativeTableauStub`]** — placeholder that satisfies the
//!    `ExternalReasoner` trait without a real tableau; the bridge now succeeds
//!    by returning native DL results rather than `NotSupported`.
//!
//! # Known limitations
//! - `owl:hasKey` only handles key lists of 1 or 2 properties.  Longer lists
//!   require an external tableau reasoner.
//! - `owl:minCardinality` / `owl:cardinality` insert annotation triples
//!   (`urn:dl:minCardinality`, `urn:dl:exactCardinality`) to record the
//!   constraint obligation; existential witnesses cannot be generated from
//!   SPARQL INSERT alone.
//!
//! # Connecting a real reasoner
//!
//! ```rust,ignore
//! struct HermitBridge { process: std::process::Child }
//! impl ExternalReasoner for HermitBridge { … }
//!
//! let bridge = ExternalReasonerBridge::new(Box::new(HermitBridge::start()?));
//! let report = bridge.materialize(&store, &[], "urn:entailment:owl2-dl")?;
//! ```

use std::time::Instant;
use tracing::{debug, info};

use super::common::{count_graph, ReasoningError, ReasoningReport, OWL2_DL_ENTAILMENT_GRAPH};
use crate::store::TripleStore;

// ─── Namespace constants ──────────────────────────────────────────────────────

const RDF_TYPE:                    &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST:                   &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST:                    &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL:                     &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDFS_SUB_CLASS_OF:           &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL_SAME_AS:                 &str = "http://www.w3.org/2002/07/owl#sameAs";
const OWL_DISJOINT_WITH:           &str = "http://www.w3.org/2002/07/owl#disjointWith";
const OWL_ON_PROPERTY:             &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_HAS_SELF:                &str = "http://www.w3.org/2002/07/owl#hasSelf";
const OWL_DISJOINT_UNION_OF:       &str = "http://www.w3.org/2002/07/owl#disjointUnionOf";
const OWL_NEGATIVE_PROP_ASSERTION: &str = "http://www.w3.org/2002/07/owl#NegativePropertyAssertion";
const OWL_SOURCE_INDIVIDUAL:       &str = "http://www.w3.org/2002/07/owl#sourceIndividual";
const OWL_ASSERTION_PROPERTY:      &str = "http://www.w3.org/2002/07/owl#assertionProperty";
const OWL_TARGET_INDIVIDUAL:       &str = "http://www.w3.org/2002/07/owl#targetIndividual";
const OWL_TARGET_VALUE:            &str = "http://www.w3.org/2002/07/owl#targetValue";
const OWL_HAS_KEY:                 &str = "http://www.w3.org/2002/07/owl#hasKey";
const OWL_MIN_CARDINALITY:         &str = "http://www.w3.org/2002/07/owl#minCardinality";
const OWL_CARDINALITY:             &str = "http://www.w3.org/2002/07/owl#cardinality";
const OWL_MIN_QUAL_CARD:           &str = "http://www.w3.org/2002/07/owl#minQualifiedCardinality";
const OWL_QUAL_CARD:               &str = "http://www.w3.org/2002/07/owl#qualifiedCardinality";
const OWL_ON_CLASS:                &str = "http://www.w3.org/2002/07/owl#onClass";
const XSD_BOOLEAN:                 &str = "http://www.w3.org/2001/XMLSchema#boolean";

/// Annotation IRI written to the entailment graph to record a minCardinality obligation.
pub const DL_MIN_CARDINALITY:           &str = "urn:dl:minCardinality";
/// Annotation IRI for an exactCardinality obligation.
pub const DL_EXACT_CARDINALITY:         &str = "urn:dl:exactCardinality";
/// Annotation IRI for a minQualifiedCardinality obligation.
pub const DL_MIN_QUAL_CARDINALITY:      &str = "urn:dl:minQualifiedCardinality";
/// Annotation IRI for an exactQualifiedCardinality obligation.
pub const DL_EXACT_QUAL_CARDINALITY:    &str = "urn:dl:exactQualifiedCardinality";

const MAX_ITERATIONS: usize = 500;

// ─── Native DL Reasoner ───────────────────────────────────────────────────────

/// OWL 2 DL native reasoner.
///
/// Runs all OWL 2 RL rules first, then applies the additional DL-specific
/// axiom rules that can be expressed as SPARQL INSERT operations.
pub struct Owl2DLReasoner<'a> {
    store: &'a TripleStore,
    target_graph: String,
    /// If `true`, inconsistency rules raise `ReasoningError::Inconsistency`.
    pub detect_inconsistency: bool,
}

impl<'a> Owl2DLReasoner<'a> {
    pub fn new(store: &'a TripleStore) -> Self {
        Self {
            store,
            target_graph: OWL2_DL_ENTAILMENT_GRAPH.to_string(),
            detect_inconsistency: true,
        }
    }

    pub fn with_target(mut self, graph: impl Into<String>) -> Self {
        self.target_graph = graph.into();
        self
    }

    /// Materialize all OWL 2 DL inferences into the target graph.
    ///
    /// Step 1: Run all OWL 2 RL rules (via [`super::owl2_rl::Owl2RLReasoner`]).
    /// Step 2: Fixed-point loop over DL-specific extension rules.
    /// Step 3: Consistency check.
    pub fn materialize(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        info!("OWL 2 DL materialization → <{}>", self.target_graph);

        // ── Step 1: RL rules ──────────────────────────────────────────────────
        let rl_report = super::owl2_rl::Owl2RLReasoner::new(self.store)
            .with_target(self.target_graph.clone())
            .materialize()?;

        debug!(
            "OWL 2 DL: RL phase added {} triples in {} iterations",
            rl_report.triples_added, rl_report.iterations
        );

        // ── Step 2: DL extension rules ────────────────────────────────────────
        let mut dl_iterations = 0usize;

        loop {
            dl_iterations += 1;
            let before = count_graph(self.store, &self.target_graph)?;

            self.rule_dl_has_self()?;
            self.rule_dl_disjoint_union_subclass()?;
            self.rule_dl_disjoint_union_pairwise()?;
            self.rule_dl_has_key_one()?;
            self.rule_dl_has_key_two()?;
            self.rule_dl_min_cardinality()?;
            self.rule_dl_cardinality()?;
            self.rule_dl_min_qualified_cardinality()?;
            self.rule_dl_qualified_cardinality()?;
            // TG-aware cax-sco: propagates types using schema triples that RL
            // deposited in TG (e.g. transitively-closed subClassOf chains).
            self.rule_dl_cax_sco_tg()?;

            let after = count_graph(self.store, &self.target_graph)?;
            if after == before || dl_iterations >= MAX_ITERATIONS {
                break;
            }
        }

        // ── Step 3: Consistency check ─────────────────────────────────────────
        if self.detect_inconsistency {
            self.check_consistency()?;
        }

        let total_triples = count_graph(self.store, &self.target_graph)?;
        Ok(ReasoningReport {
            regime: "owl2-dl".to_string(),
            triples_added: total_triples,
            iterations: rl_report.iterations + dl_iterations,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
        })
    }

    /// Check for inconsistencies not covered by RL rules.
    ///
    /// Detects violated `owl:NegativePropertyAssertion` axioms (both object and
    /// data property variants).
    pub fn check_consistency(&self) -> Result<(), ReasoningError> {
        self.rule_dl_negative_object_assertion()?;
        self.rule_dl_negative_data_assertion()?;
        Ok(())
    }

    // ── DL-specific rules ──────────────────────────────────────────────────────

    /// `dl-has-self`: For each class C with `owl:hasSelf true` on property p,
    /// every individual x of type C satisfies `x p x`.
    ///
    /// Because SPARQL INSERT requires a concrete predicate IRI, this rule first
    /// SELECTs the (class, property) pairs, then issues one UPDATE per pair.
    fn rule_dl_has_self(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;

        // Find all (class, property) pairs with owl:hasSelf true
        let select_q = format!(
            "SELECT DISTINCT ?c ?p WHERE {{ \
               ?c <{OWL_ON_PROPERTY}> ?p . \
               ?c <{OWL_HAS_SELF}> \"true\"^^<{XSD_BOOLEAN}> . \
             }}"
        );

        let pairs = match self.store.query(&select_q)? {
            oxigraph::sparql::QueryResults::Solutions(sols) => sols
                .flatten()
                .filter_map(|s| {
                    let c = match s.get("c")? {
                        oxigraph::model::Term::NamedNode(n) => n.as_str().to_string(),
                        _ => return None,
                    };
                    let p = match s.get("p")? {
                        oxigraph::model::Term::NamedNode(n) => n.as_str().to_string(),
                        _ => return None,
                    };
                    Some((c, p))
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        };

        for (c, p) in pairs {
            // Check both the default graph and TG so that RL-derived types are visible.
            let q = format!(
                "INSERT {{ GRAPH <{tg}> {{ ?x <{p}> ?x }} }} \
                 WHERE {{ {{ ?x <{RDF_TYPE}> <{c}> }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> <{c}> }} }} \
                          FILTER(isIRI(?x)) }}"
            );
            self.store.update(&q)?;
        }
        Ok(())
    }

    /// `dl-disjoint-union-subclass`: Each member of a `owl:disjointUnionOf`
    /// list is a subclass of the union class.
    fn rule_dl_disjoint_union_subclass(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?ci <{RDFS_SUB_CLASS_OF}> ?c }} }} \
             WHERE {{ \
               ?c <{OWL_DISJOINT_UNION_OF}> ?list . \
               ?list (<{RDF_FIRST}>|(<{RDF_REST}>+/<{RDF_FIRST}>)) ?ci . \
               FILTER(?ci != <{RDF_NIL}>) \
               FILTER(isIRI(?ci)) \
             }}"
        );
        self.store.update(&q).map_err(Into::into)
    }

    /// `dl-disjoint-union-pairwise`: Members of a `owl:disjointUnionOf` list
    /// are pairwise disjoint.
    fn rule_dl_disjoint_union_pairwise(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?ci <{OWL_DISJOINT_WITH}> ?cj }} }} \
             WHERE {{ \
               ?c <{OWL_DISJOINT_UNION_OF}> ?list . \
               ?list (<{RDF_FIRST}>|(<{RDF_REST}>+/<{RDF_FIRST}>)) ?ci . \
               ?list (<{RDF_FIRST}>|(<{RDF_REST}>+/<{RDF_FIRST}>)) ?cj . \
               FILTER(?ci != ?cj) \
               FILTER(?ci != <{RDF_NIL}>) \
               FILTER(?cj != <{RDF_NIL}>) \
               FILTER(isIRI(?ci)) \
               FILTER(isIRI(?cj)) \
             }}"
        );
        self.store.update(&q).map_err(Into::into)
    }

    /// `dl-negative-object-assertion` (consistency check): Raises
    /// `ReasoningError::Inconsistency` when a `owl:NegativePropertyAssertion`
    /// is violated by an asserted object-property triple.
    fn rule_dl_negative_object_assertion(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ \
               ?npa <{RDF_TYPE}> <{OWL_NEGATIVE_PROP_ASSERTION}> . \
               ?npa <{OWL_SOURCE_INDIVIDUAL}> ?s . \
               ?npa <{OWL_ASSERTION_PROPERTY}> ?p . \
               ?npa <{OWL_TARGET_INDIVIDUAL}> ?o . \
               ?s ?p ?o . \
             }}"
        );
        match self.store.query(&q)? {
            oxigraph::sparql::QueryResults::Boolean(true) => Err(ReasoningError::Inconsistency(
                "NegativeObjectPropertyAssertion violated: an asserted triple contradicts a \
                 declared owl:NegativePropertyAssertion"
                    .to_string(),
            )),
            _ => Ok(()),
        }
    }

    /// `dl-negative-data-assertion` (consistency check): Same as
    /// `rule_dl_negative_object_assertion` but for data property assertions.
    fn rule_dl_negative_data_assertion(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ \
               ?npa <{RDF_TYPE}> <{OWL_NEGATIVE_PROP_ASSERTION}> . \
               ?npa <{OWL_SOURCE_INDIVIDUAL}> ?s . \
               ?npa <{OWL_ASSERTION_PROPERTY}> ?p . \
               ?npa <{OWL_TARGET_VALUE}> ?v . \
               ?s ?p ?v . \
             }}"
        );
        match self.store.query(&q)? {
            oxigraph::sparql::QueryResults::Boolean(true) => Err(ReasoningError::Inconsistency(
                "NegativeDataPropertyAssertion violated: an asserted triple contradicts a \
                 declared owl:NegativePropertyAssertion"
                    .to_string(),
            )),
            _ => Ok(()),
        }
    }

    /// `dl-has-key` (1-key): If class C has a key list of exactly one property
    /// p, two individuals of type C with the same p-value are `owl:sameAs`.
    fn rule_dl_has_key_one(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x <{OWL_SAME_AS}> ?y }} }} \
             WHERE {{ \
               ?c <{OWL_HAS_KEY}> ?keylist . \
               ?keylist <{RDF_FIRST}> ?p . \
               ?keylist <{RDF_REST}> <{RDF_NIL}> . \
               ?x <{RDF_TYPE}> ?c . \
               ?y <{RDF_TYPE}> ?c . \
               ?x ?p ?v . \
               ?y ?p ?v . \
               FILTER(?x != ?y) \
               FILTER(isIRI(?x)) \
               FILTER(isIRI(?y)) \
             }}"
        );
        self.store.update(&q).map_err(Into::into)
    }

    /// `dl-has-key` (2-key): Same as `dl-has-key-one` but for key lists of
    /// exactly two properties.  Lists longer than 2 require a tableau reasoner.
    fn rule_dl_has_key_two(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x <{OWL_SAME_AS}> ?y }} }} \
             WHERE {{ \
               ?c <{OWL_HAS_KEY}> ?keylist . \
               ?keylist <{RDF_FIRST}> ?p1 . \
               ?keylist <{RDF_REST}> ?rest . \
               ?rest <{RDF_FIRST}> ?p2 . \
               ?rest <{RDF_REST}> <{RDF_NIL}> . \
               ?x <{RDF_TYPE}> ?c . \
               ?y <{RDF_TYPE}> ?c . \
               ?x ?p1 ?v1 . \
               ?y ?p1 ?v1 . \
               ?x ?p2 ?v2 . \
               ?y ?p2 ?v2 . \
               FILTER(?x != ?y) \
               FILTER(isIRI(?x)) \
               FILTER(isIRI(?y)) \
             }}"
        );
        self.store.update(&q).map_err(Into::into)
    }

    /// `dl-min-cardinality`: Records a minCardinality obligation in the
    /// entailment graph.  Existential witnesses cannot be generated from SPARQL
    /// alone; connect an external tableau reasoner for full ABox completion.
    fn rule_dl_min_cardinality(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x <{DL_MIN_CARDINALITY}> ?n }} }} \
             WHERE {{ \
               ?c <{OWL_MIN_CARDINALITY}> ?n . \
               ?c <{OWL_ON_PROPERTY}> ?p . \
               ?x <{RDF_TYPE}> ?c . \
               FILTER(isIRI(?x)) \
             }}"
        );
        self.store.update(&q).map_err(Into::into)
    }

    /// `dl-cardinality`: Records an exactCardinality obligation.  The
    /// `owl:maxCardinality` side is already handled by RL rules `cls-maxc1`/
    /// `cls-maxc2`.
    fn rule_dl_cardinality(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x <{DL_EXACT_CARDINALITY}> ?n }} }} \
             WHERE {{ \
               ?c <{OWL_CARDINALITY}> ?n . \
               ?c <{OWL_ON_PROPERTY}> ?p . \
               ?x <{RDF_TYPE}> ?c . \
               FILTER(isIRI(?x)) \
             }}"
        );
        self.store.update(&q).map_err(Into::into)
    }

    /// `dl-min-qualified-cardinality`: Records a minQualifiedCardinality
    /// obligation (with `owl:onClass` filler).
    fn rule_dl_min_qualified_cardinality(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x <{DL_MIN_QUAL_CARDINALITY}> ?n }} }} \
             WHERE {{ \
               ?c <{OWL_MIN_QUAL_CARD}> ?n . \
               ?c <{OWL_ON_PROPERTY}> ?p . \
               ?c <{OWL_ON_CLASS}> ?filler . \
               ?x <{RDF_TYPE}> ?c . \
               FILTER(isIRI(?x)) \
             }}"
        );
        self.store.update(&q).map_err(Into::into)
    }

    /// `dl-qualified-cardinality`: Records an exactQualifiedCardinality
    /// obligation.
    fn rule_dl_qualified_cardinality(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x <{DL_EXACT_QUAL_CARDINALITY}> ?n }} }} \
             WHERE {{ \
               ?c <{OWL_QUAL_CARD}> ?n . \
               ?c <{OWL_ON_PROPERTY}> ?p . \
               ?c <{OWL_ON_CLASS}> ?filler . \
               ?x <{RDF_TYPE}> ?c . \
               FILTER(isIRI(?x)) \
             }}"
        );
        self.store.update(&q).map_err(Into::into)
    }

    /// TG-aware `cax-sco`: Like the RL rule, but also reads `rdfs:subClassOf` and
    /// `rdf:type` triples from TG so that RL-derived schema/type facts are visible.
    ///
    /// This handles cases where:
    /// - `scm-sco` deposited a transitive subClassOf chain into TG, which `cax-sco`
    ///   (reading only the default graph) cannot see.
    /// - DL `disjointUnionOf` derived a subClassOf into TG that RL cannot chain from.
    fn rule_dl_cax_sco_tg(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c2 }} }} \
             WHERE {{ \
               {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} UNION {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }} \
               {{ ?x <{RDF_TYPE}> ?c1 }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c1 }} }} \
               FILTER(?c1 != ?c2) \
             }}"
        );
        self.store.update(&q).map_err(Into::into)
    }
}

// ─── ExternalReasoner trait ───────────────────────────────────────────────────

/// Contract for an OWL 2 DL reasoner.
pub trait ExternalReasoner: Send + Sync {
    /// Human-readable name (e.g. `"hermit"`, `"pellet"`).
    fn name(&self) -> &'static str;

    /// Load the ontology serialized as Turtle and classify it.
    /// Returns the derived subsumption hierarchy as Turtle.
    fn classify(&self, ontology_turtle: &str) -> Result<String, ReasoningError>;

    /// Check whether the ontology is consistent.
    fn check_consistency(&self, ontology_turtle: &str) -> Result<bool, ReasoningError>;

    /// Compute all inferences and return them as Turtle.
    fn get_inferences(&self, ontology_turtle: &str) -> Result<String, ReasoningError>;
}

// ─── NativeTableauStub ────────────────────────────────────────────────────────

/// Placeholder that satisfies `ExternalReasoner` without a real tableau.
///
/// When used with [`ExternalReasonerBridge`], the bridge now runs the native
/// DL rules before calling the stub, so `materialize()` **succeeds** (with
/// partial native results) rather than returning `NotSupported`.
///
/// Replace with a real implementation of `ExternalReasoner` to activate full
/// OWL 2 DL tableau reasoning (HermiT, Pellet, ELK, Konclude, …).
pub struct NativeTableauStub;

impl ExternalReasoner for NativeTableauStub {
    fn name(&self) -> &'static str {
        "native-dl-stub"
    }

    fn classify(&self, _: &str) -> Result<String, ReasoningError> {
        Err(ReasoningError::NotSupported(
            "OWL 2 DL native tableau is not yet implemented. \
             Plug in an ExternalReasoner (HermiT, Pellet, ELK, …)."
                .to_string(),
        ))
    }

    fn check_consistency(&self, _: &str) -> Result<bool, ReasoningError> {
        Err(ReasoningError::NotSupported(
            "OWL 2 DL consistency check requires an ExternalReasoner.".to_string(),
        ))
    }

    fn get_inferences(&self, _: &str) -> Result<String, ReasoningError> {
        Err(ReasoningError::NotSupported(
            "OWL 2 DL inference requires an ExternalReasoner.".to_string(),
        ))
    }
}

// ─── ExternalReasonerBridge ───────────────────────────────────────────────────

/// Bridges an [`ExternalReasoner`] with the local [`TripleStore`].
///
/// The bridge first runs the native OWL 2 DL rules (via [`Owl2DLReasoner`]),
/// then — if the inner reasoner is not the `"native-dl-stub"` — additionally
/// calls the external reasoner to load further inferences.
pub struct ExternalReasonerBridge {
    reasoner: Box<dyn ExternalReasoner>,
}

impl ExternalReasonerBridge {
    pub fn new(reasoner: Box<dyn ExternalReasoner>) -> Self {
        Self { reasoner }
    }

    /// Run native DL rules first, then optionally delegate to the external
    /// reasoner and load the resulting inferences into `target_graph`.
    pub fn materialize(
        &self,
        store: &TripleStore,
        source_graphs: &[String],
        target_graph: &str,
    ) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();

        // ── Step 1: Run the native DL reasoner ───────────────────────────────
        let native_report = Owl2DLReasoner::new(store)
            .with_target(target_graph)
            .materialize()?;

        // ── Step 2: Optionally call the external reasoner ────────────────────
        if self.reasoner.name() != "native-dl-stub" {
            let ontology_ttl = if source_graphs.is_empty() {
                store
                    .dump(oxigraph::io::RdfFormat::Turtle, None)
                    .map_err(|e| ReasoningError::Store(e.to_string()))
                    .and_then(|bytes| {
                        String::from_utf8(bytes)
                            .map_err(|e| ReasoningError::Store(e.to_string()))
                    })?
            } else {
                let mut combined = String::new();
                for g in source_graphs {
                    let bytes = store
                        .dump(oxigraph::io::RdfFormat::Turtle, Some(g))
                        .map_err(|e| ReasoningError::Store(e.to_string()))?;
                    combined.push_str(
                        &String::from_utf8(bytes)
                            .map_err(|e| ReasoningError::Store(e.to_string()))?,
                    );
                    combined.push('\n');
                }
                combined
            };

            let inferred_ttl = self.reasoner.get_inferences(&ontology_ttl)?;
            store
                .load_str(&inferred_ttl, oxigraph::io::RdfFormat::Turtle, Some(target_graph))
                .map_err(|e| ReasoningError::Store(e.to_string()))?;
        }

        let count = count_graph(store, target_graph)?;

        Ok(ReasoningReport {
            regime: format!("owl2-dl({})", self.reasoner.name()),
            triples_added: count,
            iterations: native_report.iterations + 1,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: target_graph.to_string(),
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(ttl: &str) -> TripleStore {
        let store = crate::store::TripleStore::in_memory().unwrap();
        store.load_str(ttl, oxigraph::io::RdfFormat::Turtle, None).unwrap();
        store
    }

    fn ask(store: &TripleStore, sparql: &str) -> bool {
        match store.query(sparql).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => panic!("expected ASK result"),
        }
    }

    #[test]
    fn test_stub_classify_returns_not_supported() {
        let stub = NativeTableauStub;
        assert!(matches!(
            stub.classify(""),
            Err(ReasoningError::NotSupported(_))
        ));
    }

    #[test]
    fn test_stub_check_consistency_returns_not_supported() {
        let stub = NativeTableauStub;
        assert!(matches!(
            stub.check_consistency(""),
            Err(ReasoningError::NotSupported(_))
        ));
    }

    #[test]
    fn test_bridge_with_stub_now_succeeds() {
        let store = crate::store::TripleStore::in_memory().unwrap();
        let bridge = ExternalReasonerBridge::new(Box::new(NativeTableauStub));
        // Bridge should succeed (native DL rules run; stub is skipped)
        assert!(bridge
            .materialize(&store, &[], "urn:entailment:owl2-dl")
            .is_ok());
    }

    #[test]
    fn test_dl_empty_store_ok() {
        let store = crate::store::TripleStore::in_memory().unwrap();
        let result = Owl2DLReasoner::new(&store).materialize();
        assert!(result.is_ok());
    }

    #[test]
    fn test_dl_report_regime_name() {
        let store = crate::store::TripleStore::in_memory().unwrap();
        let report = Owl2DLReasoner::new(&store).materialize().unwrap();
        assert_eq!(report.regime, "owl2-dl");
    }

    #[test]
    fn test_dl_has_self_inserts_reflexive_triple() {
        let store = store_with(r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
            ex:SelfClass owl:onProperty ex:knows ;
                         owl:hasSelf "true"^^xsd:boolean .
            ex:alice a ex:SelfClass .
        "#);
        Owl2DLReasoner::new(&store)
            .with_target("urn:entailment:owl2-dl")
            .materialize()
            .unwrap();
        assert!(ask(&store,
            "ASK { GRAPH <urn:entailment:owl2-dl> { <http://example.org/alice> \
             <http://example.org/knows> <http://example.org/alice> } }"));
    }

    #[test]
    fn test_dl_has_self_no_false_positive() {
        let store = store_with(r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
            ex:SelfClass owl:onProperty ex:knows ;
                         owl:hasSelf "true"^^xsd:boolean .
            ex:bob a ex:OtherClass .
        "#);
        Owl2DLReasoner::new(&store)
            .with_target("urn:entailment:owl2-dl")
            .materialize()
            .unwrap();
        assert!(!ask(&store,
            "ASK { GRAPH <urn:entailment:owl2-dl> { <http://example.org/bob> \
             <http://example.org/knows> <http://example.org/bob> } }"));
    }

    #[test]
    fn test_dl_negative_object_assertion_ok() {
        let store = store_with(r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            _:npa a owl:NegativePropertyAssertion ;
                  owl:sourceIndividual ex:alice ;
                  owl:assertionProperty ex:hates ;
                  owl:targetIndividual ex:bob .
            # The triple ex:alice ex:hates ex:bob does NOT exist — no violation
        "#);
        let result = Owl2DLReasoner::new(&store).materialize();
        assert!(result.is_ok());
    }

    #[test]
    fn test_dl_negative_object_assertion_violated() {
        let store = store_with(r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            _:npa a owl:NegativePropertyAssertion ;
                  owl:sourceIndividual ex:alice ;
                  owl:assertionProperty ex:hates ;
                  owl:targetIndividual ex:bob .
            ex:alice ex:hates ex:bob .
        "#);
        let result = Owl2DLReasoner::new(&store).materialize();
        assert!(matches!(result, Err(ReasoningError::Inconsistency(_))));
    }

    #[test]
    fn test_dl_negative_data_assertion_violated() {
        let store = store_with(r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            _:npa a owl:NegativePropertyAssertion ;
                  owl:sourceIndividual ex:alice ;
                  owl:assertionProperty ex:age ;
                  owl:targetValue 30 .
            ex:alice ex:age 30 .
        "#);
        let result = Owl2DLReasoner::new(&store).materialize();
        assert!(matches!(result, Err(ReasoningError::Inconsistency(_))));
    }
}
