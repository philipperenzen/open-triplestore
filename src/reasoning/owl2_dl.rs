//! OWL 2 DL — the native backend: OWL 2 RL plus the DL-syntax rules, run to
//! one joint fixed point.
//!
//! OWL 2 DL (SROIQ(D)) needs a tableau (or hypertableau) reasoner; this
//! module is not one. [`Owl2DLReasoner`] is sound but incomplete: it applies
//! every OWL 2 RL/RDF rule ([`super::owl2_rl`]) together with rules for DL
//! syntax that forward chaining can still honour —
//!
//! - `owl:hasSelf` in both directions (`x : ∃p.Self` ⇒ `x p x`, and back);
//! - `owl:ReflexiveProperty` (`x p x` for every individual in scope, so
//!   `prp-irp` catches a property that is also irreflexive);
//! - `owl:disjointUnionOf` (members are subclasses and pairwise disjoint);
//!
//! — interleaved until neither adds anything, so a DL consequence feeds the RL
//! rules and the other way round. Keys of any length and negative property
//! assertions are RL rules (`prp-key`, `prp-npa1/2`).
//!
//! Minimum and exact cardinalities cannot be satisfied by forward chaining
//! (that needs existential witnesses). Their obligations are recorded as
//! `urn:dl:*` triples in a diagnostics graph beside the target
//! ([`super::dl_backend::diagnostics_graph`]), which `?entailment=` never
//! folds into a query.
//!
//! Complete OWL 2 DL reasoning goes through a [`super::dl_backend::DlBackend`]
//! (Konclude or the reasoner sidecar), selected with `OTS_DL_BACKEND`.

use std::time::Instant;
use tracing::{debug, info};

use super::common::{count_graph, ReasoningError, ReasoningReport, OWL2_DL_ENTAILMENT_GRAPH};
use crate::store::TripleStore;

// ─── Namespace constants ──────────────────────────────────────────────────────

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL_DISJOINT_WITH: &str = "http://www.w3.org/2002/07/owl#disjointWith";
const OWL_ON_PROPERTY: &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_HAS_SELF: &str = "http://www.w3.org/2002/07/owl#hasSelf";
const OWL_DISJOINT_UNION_OF: &str = "http://www.w3.org/2002/07/owl#disjointUnionOf";
const OWL_REFLEXIVE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ReflexiveProperty";
const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";
const OWL_NAMED_INDIVIDUAL: &str = "http://www.w3.org/2002/07/owl#NamedIndividual";
const OWL_MIN_CARDINALITY: &str = "http://www.w3.org/2002/07/owl#minCardinality";
const OWL_CARDINALITY: &str = "http://www.w3.org/2002/07/owl#cardinality";
const OWL_MIN_QUAL_CARD: &str = "http://www.w3.org/2002/07/owl#minQualifiedCardinality";
const OWL_QUAL_CARD: &str = "http://www.w3.org/2002/07/owl#qualifiedCardinality";
const OWL_ON_CLASS: &str = "http://www.w3.org/2002/07/owl#onClass";

/// Diagnostics IRI recording a minCardinality obligation.
pub const DL_MIN_CARDINALITY: &str = "urn:dl:minCardinality";
/// Diagnostics IRI for an exactCardinality obligation.
pub const DL_EXACT_CARDINALITY: &str = "urn:dl:exactCardinality";
/// Diagnostics IRI for a minQualifiedCardinality obligation.
pub const DL_MIN_QUAL_CARDINALITY: &str = "urn:dl:minQualifiedCardinality";
/// Diagnostics IRI for an exactQualifiedCardinality obligation.
pub const DL_EXACT_QUAL_CARDINALITY: &str = "urn:dl:exactQualifiedCardinality";

/// Outer rounds (one RL fixed point + one DL pass each) before the run fails
/// with [`ReasoningError::NotConverged`].
const MAX_ITERATIONS: usize = 500;

// ─── Native DL Reasoner ───────────────────────────────────────────────────────

/// OWL 2 DL native reasoner: RL and DL-syntax rules to a joint fixed point.
pub struct Owl2DLReasoner<'a> {
    store: &'a TripleStore,
    target_graph: String,
    /// Where cardinality obligations go; default `<target>:diagnostics`.
    diagnostics_graph: Option<String>,
    /// When set, the rules read ONLY these graphs (plus the target graph).
    /// Without it they read the unnamed default graph plus the target graph
    /// (`TripleStore::update_over`), so rules see their own consequences.
    sources: Option<Vec<String>>,
    /// If `true`, inconsistency rules raise `ReasoningError::Inconsistency`.
    pub detect_inconsistency: bool,
    /// Identity policy handed to the RL rules (see `Owl2RLReasoner`).
    identity: super::identity::IdentityPolicy,
}

impl<'a> Owl2DLReasoner<'a> {
    /// Restrict the rules to `sources` (plus the target graph). Without a
    /// scope the rules read the unnamed default graph and the target graph, so
    /// a dataset's named graphs — and the model version it conforms to — are
    /// invisible to materialisation; this is what `POST /api/reasoning/materialize` sets
    /// from `source_graphs` or the dataset's conformance layer.
    pub fn with_sources(mut self, sources: Vec<String>) -> Self {
        self.sources = Some(sources);
        self
    }

    fn scope(&self) -> Option<Vec<String>> {
        self.sources.as_ref().map(|s| {
            let mut g = s.clone();
            if !g.contains(&self.target_graph) {
                g.push(self.target_graph.clone());
            }
            g
        })
    }

    fn run_update(&self, sparql: &str) -> Result<(), crate::store::engine::StoreError> {
        match self.scope() {
            Some(scope) => self.store.update_scoped(sparql, &scope),
            None => self
                .store
                .update_over(sparql, std::slice::from_ref(&self.target_graph)),
        }
    }

    pub fn new(store: &'a TripleStore) -> Self {
        Self {
            store,
            target_graph: OWL2_DL_ENTAILMENT_GRAPH.to_string(),
            diagnostics_graph: None,
            sources: None,
            detect_inconsistency: true,
            identity: super::identity::IdentityPolicy::Full,
        }
    }

    pub fn with_target(mut self, graph: impl Into<String>) -> Self {
        self.target_graph = graph.into();
        self
    }

    /// Where the `urn:dl:*` cardinality obligations go (default
    /// `<target>:diagnostics`).
    pub fn with_diagnostics(mut self, graph: impl Into<String>) -> Self {
        self.diagnostics_graph = Some(graph.into());
        self
    }

    fn diagnostics(&self) -> String {
        self.diagnostics_graph
            .clone()
            .unwrap_or_else(|| super::dl_backend::diagnostics_graph(&self.target_graph))
    }

    /// Identity policy for the RL rules (`sameas-off` skips the equality rules).
    pub fn with_identity_policy(mut self, policy: super::identity::IdentityPolicy) -> Self {
        self.identity = policy;
        self
    }

    fn rl(&self) -> super::owl2_rl::Owl2RLReasoner<'a> {
        let mut rl = super::owl2_rl::Owl2RLReasoner::new(self.store)
            .with_target(self.target_graph.clone())
            .with_identity_policy(self.identity);
        rl.detect_inconsistency = self.detect_inconsistency;
        match &self.sources {
            Some(s) => rl.with_sources(s.clone()),
            None => rl,
        }
    }

    /// Materialize the joint RL + DL closure into the target graph.
    ///
    /// Each round runs the RL rules to their own fixed point (with their
    /// consistency checks), then one pass of the DL rules; the run ends when a
    /// DL pass adds nothing the RL rules have not already seen.
    pub fn materialize(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        info!("OWL 2 DL materialization → <{}>", self.target_graph);
        // Report the delta this run produced, not the graph's final size.
        let initial = count_graph(self.store, &self.target_graph)?;
        let rl = self.rl();
        let mut iterations = 0usize;
        let mut rounds = 0usize;
        loop {
            rounds += 1;
            iterations += rl.materialize()?.iterations;
            let mid = count_graph(self.store, &self.target_graph)?;
            self.rule_dl_has_self()?;
            self.rule_dl_has_self_converse()?;
            self.rule_dl_reflexive()?;
            self.rule_dl_disjoint_union_subclass()?;
            self.rule_dl_disjoint_union_pairwise()?;
            iterations += 1;
            let after = count_graph(self.store, &self.target_graph)?;
            debug!("OWL 2 DL round {rounds}: DL rules added {}", after - mid);
            if after == mid {
                break;
            }
            if rounds >= MAX_ITERATIONS {
                return Err(ReasoningError::NotConverged {
                    regime: "owl2-dl".to_string(),
                    iterations,
                });
            }
        }
        self.record_cardinality_obligations()?;

        let total_triples = count_graph(self.store, &self.target_graph)?;
        Ok(ReasoningReport {
            regime: "owl2-dl".to_string(),
            triples_added: total_triples.saturating_sub(initial),
            iterations,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
            ..Default::default()
        })
    }

    // ── DL-specific rules ──────────────────────────────────────────────────────

    /// `dl-has-self`: `?c owl:hasSelf true ; owl:onProperty ?p . ?x a ?c` ⇒
    /// `?x ?p ?x`.
    fn rule_dl_has_self(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x ?p ?x }} }} \
             WHERE {{ ?c <{OWL_HAS_SELF}> ?self ; <{OWL_ON_PROPERTY}> ?p . \
                      FILTER(?self = true) \
                      ?x <{RDF_TYPE}> ?c . FILTER(isIRI(?x)) FILTER(isIRI(?p)) }}"
        );
        self.run_update(&q).map_err(Into::into)
    }

    /// `dl-has-self-converse`: `?c owl:hasSelf true ; owl:onProperty ?p .
    /// ?x ?p ?x` ⇒ `?x a ?c`.
    fn rule_dl_has_self_converse(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }} \
             WHERE {{ ?c <{OWL_HAS_SELF}> ?self ; <{OWL_ON_PROPERTY}> ?p . \
                      FILTER(?self = true) \
                      ?x ?p ?x . FILTER(isIRI(?x)) }}"
        );
        self.run_update(&q).map_err(Into::into)
    }

    /// `dl-reflexive`: `?p a owl:ReflexiveProperty` ⇒ `?x ?p ?x` for every
    /// individual in scope — anything typed with a class outside the reserved
    /// vocabulary (or `owl:Thing` / `owl:NamedIndividual`), and both ends of
    /// a `?p` assertion. `prp-irp` then catches reflexive + irreflexive.
    fn rule_dl_reflexive(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            "INSERT {{ GRAPH <{tg}> {{ ?x ?p ?x }} }} \
             WHERE {{ ?p <{RDF_TYPE}> <{OWL_REFLEXIVE_PROPERTY}> . FILTER(isIRI(?p)) \
               {{ ?x <{RDF_TYPE}> ?c . \
                  FILTER(?c IN (<{OWL_THING}>, <{OWL_NAMED_INDIVIDUAL}>) || \
                         !(STRSTARTS(STR(?c), \"http://www.w3.org/2002/07/owl#\") || \
                           STRSTARTS(STR(?c), \"http://www.w3.org/2000/01/rdf-schema#\") || \
                           STRSTARTS(STR(?c), \"http://www.w3.org/1999/02/22-rdf-syntax-ns#\") || \
                           STRSTARTS(STR(?c), \"http://www.w3.org/2001/XMLSchema#\"))) }} \
               UNION {{ ?x ?p ?y }} UNION {{ ?y ?p ?x }} \
               FILTER(isIRI(?x)) }}"
        );
        self.run_update(&q).map_err(Into::into)
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
        self.run_update(&q).map_err(Into::into)
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
        self.run_update(&q).map_err(Into::into)
    }

    /// Minimum and exact cardinality obligations the rules cannot satisfy
    /// (no existential witnesses), recorded in the diagnostics graph:
    /// `?x urn:dl:minCardinality ?n` for `?x a [ owl:minCardinality ?n ]`, and
    /// likewise for exact and qualified cardinalities.
    fn record_cardinality_obligations(&self) -> Result<(), ReasoningError> {
        let dg = self.diagnostics();
        for (card, mark, qualified) in [
            (OWL_MIN_CARDINALITY, DL_MIN_CARDINALITY, false),
            (OWL_CARDINALITY, DL_EXACT_CARDINALITY, false),
            (OWL_MIN_QUAL_CARD, DL_MIN_QUAL_CARDINALITY, true),
            (OWL_QUAL_CARD, DL_EXACT_QUAL_CARDINALITY, true),
        ] {
            let on_class = if qualified {
                format!("?c <{OWL_ON_CLASS}> ?filler . ")
            } else {
                String::new()
            };
            let q = format!(
                "INSERT {{ GRAPH <{dg}> {{ ?x <{mark}> ?n }} }} \
                 WHERE {{ ?c <{card}> ?n ; <{OWL_ON_PROPERTY}> ?p . {on_class}\
                          ?x <{RDF_TYPE}> ?c . FILTER(isIRI(?x)) }}"
            );
            self.run_update(&q)?;
        }
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(ttl: &str) -> TripleStore {
        let store = crate::store::TripleStore::in_memory().unwrap();
        store
            .load_str(ttl, oxigraph::io::RdfFormat::Turtle, None)
            .unwrap();
        store
    }

    fn ask(store: &TripleStore, sparql: &str) -> bool {
        match store.query(sparql).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => panic!("expected ASK result"),
        }
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
        let store = store_with(
            r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
            ex:SelfClass owl:onProperty ex:knows ;
                         owl:hasSelf "true"^^xsd:boolean .
            ex:alice a ex:SelfClass .
        "#,
        );
        Owl2DLReasoner::new(&store)
            .with_target("urn:entailment:owl2-dl")
            .materialize()
            .unwrap();
        assert!(ask(
            &store,
            "ASK { GRAPH <urn:entailment:owl2-dl> { <http://example.org/alice> \
             <http://example.org/knows> <http://example.org/alice> } }"
        ));
    }

    #[test]
    fn test_dl_has_self_no_false_positive() {
        let store = store_with(
            r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
            ex:SelfClass owl:onProperty ex:knows ;
                         owl:hasSelf "true"^^xsd:boolean .
            ex:bob a ex:OtherClass .
        "#,
        );
        Owl2DLReasoner::new(&store)
            .with_target("urn:entailment:owl2-dl")
            .materialize()
            .unwrap();
        assert!(!ask(
            &store,
            "ASK { GRAPH <urn:entailment:owl2-dl> { <http://example.org/bob> \
             <http://example.org/knows> <http://example.org/bob> } }"
        ));
    }

    #[test]
    fn test_dl_negative_object_assertion_ok() {
        let store = store_with(
            r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            _:npa a owl:NegativePropertyAssertion ;
                  owl:sourceIndividual ex:alice ;
                  owl:assertionProperty ex:hates ;
                  owl:targetIndividual ex:bob .
            # The triple ex:alice ex:hates ex:bob does NOT exist — no violation
        "#,
        );
        let result = Owl2DLReasoner::new(&store).materialize();
        assert!(result.is_ok());
    }

    #[test]
    fn test_dl_negative_object_assertion_violated() {
        let store = store_with(
            r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            _:npa a owl:NegativePropertyAssertion ;
                  owl:sourceIndividual ex:alice ;
                  owl:assertionProperty ex:hates ;
                  owl:targetIndividual ex:bob .
            ex:alice ex:hates ex:bob .
        "#,
        );
        let result = Owl2DLReasoner::new(&store).materialize();
        assert!(matches!(result, Err(ReasoningError::Inconsistency { .. })));
    }

    #[test]
    fn test_dl_negative_data_assertion_violated() {
        let store = store_with(
            r#"
            @prefix owl: <http://www.w3.org/2002/07/owl#> .
            @prefix ex:  <http://example.org/> .
            _:npa a owl:NegativePropertyAssertion ;
                  owl:sourceIndividual ex:alice ;
                  owl:assertionProperty ex:age ;
                  owl:targetValue 30 .
            ex:alice ex:age 30 .
        "#,
        );
        let result = Owl2DLReasoner::new(&store).materialize();
        assert!(matches!(result, Err(ReasoningError::Inconsistency { .. })));
    }
}
