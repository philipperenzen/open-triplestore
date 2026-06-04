//! OWL 2 RL profile — forward-chaining materialization.
//!
//! Implements the complete rule set from W3C OWL 2 Profiles, Tables 4–9
//! (approximately 80 rules), expressed as SPARQL INSERT operations executed
//! in a fixed-point loop.
//!
//! Inconsistency-detection rules raise `ReasoningError::Inconsistency` rather
//! than inserting triples.
//!
//! # Usage
//! ```rust,ignore
//! let report = Owl2RLReasoner::new(&store)
//!     .with_target("urn:entailment:owl2-rl")
//!     .materialize()?;
//! ```
#![allow(dead_code)]

use std::time::Instant;
use tracing::{debug, info};

use super::common::{count_graph, ReasoningError, ReasoningReport, OWL2_RL_ENTAILMENT_GRAPH};
use crate::store::TripleStore;

// ─── Namespace constants ──────────────────────────────────────────────────────

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_SUB_PROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";
const OWL_SAME_AS: &str = "http://www.w3.org/2002/07/owl#sameAs";
const OWL_DIFFERENT_FROM: &str = "http://www.w3.org/2002/07/owl#differentFrom";
const OWL_EQUIV_CLASS: &str = "http://www.w3.org/2002/07/owl#equivalentClass";
const OWL_EQUIV_PROP: &str = "http://www.w3.org/2002/07/owl#equivalentProperty";
const OWL_INVERSE_OF: &str = "http://www.w3.org/2002/07/owl#inverseOf";
const OWL_DISJOINT_WITH: &str = "http://www.w3.org/2002/07/owl#disjointWith";
const OWL_FUNCTIONAL_PROP: &str = "http://www.w3.org/2002/07/owl#FunctionalProperty";
const OWL_INV_FUNCTIONAL: &str = "http://www.w3.org/2002/07/owl#InverseFunctionalProperty";
const OWL_SYMMETRIC_PROP: &str = "http://www.w3.org/2002/07/owl#SymmetricProperty";
const OWL_ASYMMETRIC_PROP: &str = "http://www.w3.org/2002/07/owl#AsymmetricProperty";
const OWL_IRREFLEXIVE_PROP: &str = "http://www.w3.org/2002/07/owl#IrreflexiveProperty";
const OWL_TRANSITIVE_PROP: &str = "http://www.w3.org/2002/07/owl#TransitiveProperty";
const OWL_NOTHING: &str = "http://www.w3.org/2002/07/owl#Nothing";
const OWL_HAS_VALUE: &str = "http://www.w3.org/2002/07/owl#hasValue";
const OWL_ON_PROPERTY: &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_SOME_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#someValuesFrom";
const OWL_ALL_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#allValuesFrom";
const OWL_MAX_CARDINALITY: &str = "http://www.w3.org/2002/07/owl#maxCardinality";
const OWL_MAX_QUAL_CARD: &str = "http://www.w3.org/2002/07/owl#maxQualifiedCardinality";
const OWL_INTERSECTION_OF: &str = "http://www.w3.org/2002/07/owl#intersectionOf";
const OWL_UNION_OF: &str = "http://www.w3.org/2002/07/owl#unionOf";
const OWL_ONE_OF: &str = "http://www.w3.org/2002/07/owl#oneOf";
const OWL_PROP_CHAIN_AXIOM: &str = "http://www.w3.org/2002/07/owl#propertyChainAxiom";
const OWL_MEMBERS: &str = "http://www.w3.org/2002/07/owl#members";
const OWL_ALL_DISJOINT: &str = "http://www.w3.org/2002/07/owl#AllDisjointClasses";
const OWL_COMPLEMENT_OF: &str = "http://www.w3.org/2002/07/owl#complementOf";
const OWL_HAS_KEY: &str = "http://www.w3.org/2002/07/owl#hasKey";
const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";
const OWL_CLASS: &str = "http://www.w3.org/2002/07/owl#Class";
const OWL_OBJECT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ObjectProperty";
const OWL_DATATYPE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#DatatypeProperty";
const OWL_ANNOTATION_PROPERTY: &str = "http://www.w3.org/2002/07/owl#AnnotationProperty";
const OWL_ON_CLASS: &str = "http://www.w3.org/2002/07/owl#onClass";
const OWL_NEGATIVE_PROP_ASSERTION: &str = "http://www.w3.org/2002/07/owl#NegativePropertyAssertion";
const OWL_SOURCE_INDIVIDUAL: &str = "http://www.w3.org/2002/07/owl#sourceIndividual";
const OWL_ASSERTION_PROPERTY: &str = "http://www.w3.org/2002/07/owl#assertionProperty";
const OWL_TARGET_INDIVIDUAL: &str = "http://www.w3.org/2002/07/owl#targetIndividual";
const OWL_TARGET_VALUE: &str = "http://www.w3.org/2002/07/owl#targetValue";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

const MAX_ITERATIONS: usize = 500;

// ─── Reasoner ─────────────────────────────────────────────────────────────────

/// OWL 2 RL forward-chaining reasoner.
pub struct Owl2RLReasoner<'a> {
    store: &'a TripleStore,
    target_graph: String,
    /// If `true`, inconsistency rules raise `ReasoningError::Inconsistency`.
    pub detect_inconsistency: bool,
}

impl<'a> Owl2RLReasoner<'a> {
    pub fn new(store: &'a TripleStore) -> Self {
        Self {
            store,
            target_graph: OWL2_RL_ENTAILMENT_GRAPH.to_string(),
            detect_inconsistency: true,
        }
    }

    pub fn with_target(mut self, graph: impl Into<String>) -> Self {
        self.target_graph = graph.into();
        self
    }

    /// Run all RL rules to fixed point, then check consistency.
    pub fn materialize(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        let mut iterations = 0usize;

        info!("OWL 2 RL materialization → <{}>", self.target_graph);

        loop {
            iterations += 1;
            let before = count_graph(self.store, &self.target_graph)?;

            // Table 9 — Schema (must run first to populate scm triples)
            self.rule_scm_cls()?;
            self.rule_scm_sco()?;
            self.rule_scm_spo()?;
            self.rule_scm_eqc1()?;
            self.rule_scm_eqc2()?;
            self.rule_scm_eqp1()?;
            self.rule_scm_eqp2()?;
            self.rule_scm_dom1()?;
            self.rule_scm_dom2()?;
            self.rule_scm_rng1()?;
            self.rule_scm_rng2()?;
            self.rule_scm_hv()?;
            self.rule_scm_svf1()?;
            self.rule_scm_svf2()?;
            self.rule_scm_avf1()?;
            self.rule_scm_avf2()?;
            self.rule_scm_int()?;
            self.rule_scm_uni()?;

            // Table 7 — Class axioms
            self.rule_cax_sco()?;
            self.rule_cax_eqc1()?;
            self.rule_cax_eqc2()?;
            self.rule_cax_adc()?;

            // Table 5 — Property axioms
            self.rule_prp_dom()?;
            self.rule_prp_rng()?;
            self.rule_prp_symp()?;
            self.rule_prp_trp()?;
            self.rule_prp_spo1()?;
            self.rule_prp_spo2()?;
            self.rule_prp_inv1()?;
            self.rule_prp_inv2()?;
            self.rule_prp_fp()?;
            self.rule_prp_ifp()?;
            self.rule_prp_hv1()?;
            self.rule_prp_hv2()?;
            self.rule_prp_key()?;

            // Table 6 — Classes
            self.rule_cls_int1()?;
            self.rule_cls_int2()?;
            self.rule_cls_uni()?;
            self.rule_cls_svf1()?;
            self.rule_cls_svf2()?;
            self.rule_cls_avf()?;
            self.rule_cls_hv1()?;
            self.rule_cls_hv2()?;
            self.rule_cls_maxc2()?;
            self.rule_cls_maxqc1()?;
            self.rule_cls_maxqc2()?;
            self.rule_cls_maxqc3()?;
            self.rule_cls_maxqc4()?;
            self.rule_cls_oo()?;

            // Table 4 — Equality
            self.rule_eq_sym()?;
            self.rule_eq_trans()?;
            self.rule_eq_rep_s()?;
            self.rule_eq_rep_p()?;
            self.rule_eq_rep_o()?;

            let after = count_graph(self.store, &self.target_graph)?;
            let added = after.saturating_sub(before);
            debug!("OWL 2 RL iteration {}: +{} triples", iterations, added);
            if added == 0 || iterations >= MAX_ITERATIONS {
                break;
            }
        }

        // Consistency checks (after fixed point)
        if self.detect_inconsistency {
            self.check_consistency()?;
        }

        let final_count = count_graph(self.store, &self.target_graph)?;
        info!(
            "OWL 2 RL materialization complete: {} triples in {} iterations ({} ms)",
            final_count,
            iterations,
            start.elapsed().as_millis()
        );

        Ok(ReasoningReport {
            regime: "owl2-rl".to_string(),
            triples_added: final_count,
            iterations,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
        })
    }

    /// Run inconsistency checks.  Returns `Err(Inconsistency)` if any are triggered.
    pub fn check_consistency(&self) -> Result<(), ReasoningError> {
        self.rule_eq_diff1()?;
        self.rule_prp_irp()?;
        self.rule_prp_asyp()?;
        self.rule_prp_npa1()?;
        self.rule_prp_npa2()?;
        self.rule_cls_nothing2()?;
        self.rule_cls_maxc1()?;
        self.rule_cls_com()?;
        self.rule_cax_dw()?;
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 4 — Semantics of Equality
    // ═══════════════════════════════════════════════════════════════════════════

    /// eq-sym: ?x owl:sameAs ?y → ?y owl:sameAs ?x
    fn rule_eq_sym(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y <{OWL_SAME_AS}> ?x }} }}
               WHERE  {{ ?x <{OWL_SAME_AS}> ?y . FILTER(?x != ?y) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// eq-trans: ?x owl:sameAs ?y . ?y owl:sameAs ?z → ?x owl:sameAs ?z
    fn rule_eq_trans(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{OWL_SAME_AS}> ?z }} }}
               WHERE  {{ ?x <{OWL_SAME_AS}> ?y . ?y <{OWL_SAME_AS}> ?z . FILTER(?x != ?z) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// eq-rep-s: ?s owl:sameAs ?s' . ?s ?p ?o → ?s' ?p ?o
    fn rule_eq_rep_s(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?sp ?p ?o }} }}
               WHERE  {{ ?s <{OWL_SAME_AS}> ?sp . ?s ?p ?o .
                         FILTER(?s != ?sp) FILTER(?p != <{OWL_SAME_AS}>) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// eq-rep-p: ?p owl:sameAs ?p' . ?s ?p ?o → ?s ?p' ?o
    fn rule_eq_rep_p(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?s ?pp ?o }} }}
               WHERE  {{ ?p <{OWL_SAME_AS}> ?pp . ?s ?p ?o .
                         FILTER(?p != ?pp) FILTER(isIRI(?pp)) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// eq-rep-o: ?o owl:sameAs ?o' . ?s ?p ?o → ?s ?p ?o'
    fn rule_eq_rep_o(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?s ?p ?op }} }}
               WHERE  {{ ?o <{OWL_SAME_AS}> ?op . ?s ?p ?o . FILTER(?o != ?op) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// eq-diff1: ?x owl:sameAs ?y . ?x owl:differentFrom ?y → INCONSISTENCY
    fn rule_eq_diff1(&self) -> Result<(), ReasoningError> {
        let q = format!("ASK {{ ?x <{OWL_SAME_AS}> ?y . ?x <{OWL_DIFFERENT_FROM}> ?y }}");
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "owl:sameAs and owl:differentFrom on the same pair".to_string(),
            ));
        }
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 5 — Semantics of Property Axioms
    // ═══════════════════════════════════════════════════════════════════════════

    /// prp-dom: ?p rdfs:domain ?c . ?x ?p ?y → ?x rdf:type ?c
    fn rule_prp_dom(&self) -> Result<(), ReasoningError> {
        // Also look in the target graph for property triples (e.g. inferred via prp-spo1)
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
               WHERE  {{
                   ?p <{RDFS_DOMAIN}> ?c .
                   {{ ?x ?p ?y }} UNION {{ GRAPH <{tg}> {{ ?x ?p ?y }} }}
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-rng: ?p rdfs:range ?c . ?x ?p ?y → ?y rdf:type ?c
    fn rule_prp_rng(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y <{RDF_TYPE}> ?c }} }}
               WHERE  {{ ?p <{RDFS_RANGE}> ?c . ?x ?p ?y . FILTER(isIRI(?y) || isBlank(?y)) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-fp: ?p FunctionalProperty . ?x ?p ?y1 . ?x ?p ?y2 → ?y1 owl:sameAs ?y2
    fn rule_prp_fp(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y1 <{OWL_SAME_AS}> ?y2 }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{OWL_FUNCTIONAL_PROP}> .
                         ?x ?p ?y1 . ?x ?p ?y2 . FILTER(?y1 != ?y2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-ifp: ?p InverseFunctionalProperty . ?y p x1 . ?y p x2 → x1 owl:sameAs x2
    fn rule_prp_ifp(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x1 <{OWL_SAME_AS}> ?x2 }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{OWL_INV_FUNCTIONAL}> .
                         ?x1 ?p ?y . ?x2 ?p ?y . FILTER(?x1 != ?x2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-irp: ?p IrreflexiveProperty . ?x ?p ?x → INCONSISTENCY
    fn rule_prp_irp(&self) -> Result<(), ReasoningError> {
        let q = format!("ASK {{ ?p <{RDF_TYPE}> <{OWL_IRREFLEXIVE_PROP}> . ?x ?p ?x }}");
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "IrreflexiveProperty has reflexive triple".to_string(),
            ));
        }
        Ok(())
    }

    /// prp-symp: ?p SymmetricProperty . ?x ?p ?y → ?y ?p ?x
    fn rule_prp_symp(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y ?p ?x }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{OWL_SYMMETRIC_PROP}> . ?x ?p ?y }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-asyp: ?p AsymmetricProperty . ?x ?p ?y . ?y ?p ?x → INCONSISTENCY
    fn rule_prp_asyp(&self) -> Result<(), ReasoningError> {
        let q = format!("ASK {{ ?p <{RDF_TYPE}> <{OWL_ASYMMETRIC_PROP}> . ?x ?p ?y . ?y ?p ?x }}");
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "AsymmetricProperty violation".to_string(),
            ));
        }
        Ok(())
    }

    /// prp-trp: ?p TransitiveProperty . ?x ?p ?y . ?y ?p ?z → ?x ?p ?z
    fn rule_prp_trp(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x ?p ?z }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{OWL_TRANSITIVE_PROP}> .
                         ?x ?p ?y . ?y ?p ?z . FILTER(?x != ?z) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-spo1: ?p1 rdfs:subPropertyOf ?p2 . ?x ?p1 ?y → ?x ?p2 ?y
    fn rule_prp_spo1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x ?p2 ?y }} }}
               WHERE  {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 . ?x ?p1 ?y . FILTER(?p1 != ?p2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-inv1: ?p1 owl:inverseOf ?p2 . ?x ?p1 ?y → ?y ?p2 ?x
    fn rule_prp_inv1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y ?p2 ?x }} }}
               WHERE  {{ ?p1 <{OWL_INVERSE_OF}> ?p2 . ?x ?p1 ?y }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-inv2: ?p1 owl:inverseOf ?p2 . ?x ?p2 ?y → ?y ?p1 ?x
    fn rule_prp_inv2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y ?p1 ?x }} }}
               WHERE  {{ ?p1 <{OWL_INVERSE_OF}> ?p2 . ?x ?p2 ?y }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-spo2: property chain axiom propagation
    /// ?p owl:propertyChainAxiom (?p1 ?p2) . ?x ?p1 ?y . ?y ?p2 ?z → ?x ?p ?z
    fn rule_prp_spo2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x ?p ?z }} }}
               WHERE {{
                   ?p <{OWL_PROP_CHAIN_AXIOM}> ?list .
                   ?list <{RDF_FIRST}> ?p1 ;
                         <{RDF_REST}>  ?rest .
                   ?rest <{RDF_FIRST}> ?p2 ;
                         <{RDF_REST}>  <{RDF_NIL}> .
                   ?x ?p1 ?y .
                   ?y ?p2 ?z .
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-key: hasKey with single property
    /// ?c owl:hasKey (?p) . ?x type ?c . ?y type ?c . ?x ?p ?v . ?y ?p ?v → ?x sameAs ?y
    fn rule_prp_key(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{OWL_SAME_AS}> ?y }} }}
               WHERE {{
                   ?c <{OWL_HAS_KEY}> ?list .
                   ?list <{RDF_FIRST}> ?p ;
                         <{RDF_REST}>  <{RDF_NIL}> .
                   ?x <{RDF_TYPE}> ?c .
                   ?y <{RDF_TYPE}> ?c .
                   ?x ?p ?v .
                   ?y ?p ?v .
                   FILTER(?x != ?y) FILTER(isIRI(?x)) FILTER(isIRI(?y))
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-npa1: NegativePropertyAssertion (object) → INCONSISTENCY
    fn rule_prp_npa1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ \
               ?npa <{RDF_TYPE}> <{OWL_NEGATIVE_PROP_ASSERTION}> . \
               ?npa <{OWL_SOURCE_INDIVIDUAL}> ?s . \
               ?npa <{OWL_ASSERTION_PROPERTY}> ?p . \
               ?npa <{OWL_TARGET_INDIVIDUAL}> ?o . \
               ?s ?p ?o . \
             }}"
        );
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "NegativeObjectPropertyAssertion violated".to_string(),
            ));
        }
        Ok(())
    }

    /// prp-npa2: NegativePropertyAssertion (data) → INCONSISTENCY
    fn rule_prp_npa2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ \
               ?npa <{RDF_TYPE}> <{OWL_NEGATIVE_PROP_ASSERTION}> . \
               ?npa <{OWL_SOURCE_INDIVIDUAL}> ?s . \
               ?npa <{OWL_ASSERTION_PROPERTY}> ?p . \
               ?npa <{OWL_TARGET_VALUE}> ?v . \
               ?s ?p ?v . \
             }}"
        );
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "NegativeDataPropertyAssertion violated".to_string(),
            ));
        }
        Ok(())
    }

    /// prp-hv1: ?x owl:hasValue ?y . ?x owl:onProperty ?p . ?u rdf:type ?x → ?u ?p ?y
    fn rule_prp_hv1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?u ?p ?y }} }}
               WHERE  {{ ?x <{OWL_HAS_VALUE}> ?y . ?x <{OWL_ON_PROPERTY}> ?p .
                         ?u <{RDF_TYPE}> ?x }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// prp-hv2: ?x owl:hasValue ?y . ?x owl:onProperty ?p . ?u ?p ?y → ?u rdf:type ?x
    fn rule_prp_hv2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?u <{RDF_TYPE}> ?x }} }}
               WHERE  {{ ?x <{OWL_HAS_VALUE}> ?y . ?x <{OWL_ON_PROPERTY}> ?p .
                         ?u ?p ?y }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 6 — Semantics of Classes
    // ═══════════════════════════════════════════════════════════════════════════

    /// cls-nothing2: ?x rdf:type owl:Nothing → INCONSISTENCY
    fn rule_cls_nothing2(&self) -> Result<(), ReasoningError> {
        let q = format!("ASK {{ ?x <{RDF_TYPE}> <{OWL_NOTHING}> }}");
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "An individual is an instance of owl:Nothing".to_string(),
            ));
        }
        Ok(())
    }

    /// cls-int1: ?c owl:intersectionOf list(c1..cn) . ?x type ci for all i → ?x type ?c
    fn rule_cls_int1(&self) -> Result<(), ReasoningError> {
        // For two-class intersections (most common case in RL).
        // Full n-ary intersections require recursive list traversal which is
        // complex in SPARQL; we handle the two-element case here.
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list <{RDF_FIRST}> ?c1 ;
                         <{RDF_REST}>  ?rest .
                   ?rest <{RDF_FIRST}> ?c2 ;
                         <{RDF_REST}>  <{RDF_NIL}> .
                   ?x <{RDF_TYPE}> ?c1 .
                   ?x <{RDF_TYPE}> ?c2 .
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-int2: ?c owl:intersectionOf list(c1..cn) . ?x type ?c → ?x type ci for each i
    fn rule_cls_int2(&self) -> Result<(), ReasoningError> {
        // Head element
        let q1 = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c1 }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list <{RDF_FIRST}> ?c1 .
                   ?x <{RDF_TYPE}> ?c .
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q1)?;
        // Rest element(s)
        let q2 = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?cm }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list <{RDF_REST}>+ ?rest .
                   ?rest <{RDF_FIRST}> ?cm .
                   FILTER(?cm != <{RDF_NIL}>) .
                   ?x <{RDF_TYPE}> ?c .
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q2)?;
        Ok(())
    }

    /// cls-uni: ?c owl:unionOf list(c1..cn) . ?x type ci → ?x type ?c
    fn rule_cls_uni(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
               WHERE {{
                   ?c <{OWL_UNION_OF}> ?list .
                   ?list (<{RDF_FIRST}>|(<{RDF_REST}>+ /<{RDF_FIRST}>)) ?ci .
                   FILTER(?ci != <{RDF_NIL}>) .
                   ?x <{RDF_TYPE}> ?ci .
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-svf1: ?x owl:someValuesFrom ?y . ?x owl:onProperty ?p .
    ///           ?u ?p ?v . ?v rdf:type ?y → ?u rdf:type ?x
    fn rule_cls_svf1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?u <{RDF_TYPE}> ?x }} }}
               WHERE  {{ ?x <{OWL_SOME_VALUES_FROM}> ?y . ?x <{OWL_ON_PROPERTY}> ?p .
                         ?u ?p ?v . ?v <{RDF_TYPE}> ?y }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-svf2: ?x owl:someValuesFrom owl:Thing . ?x owl:onProperty ?p . ?u ?p ?v → ?u rdf:type ?x
    fn rule_cls_svf2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?u <{RDF_TYPE}> ?x }} }}
               WHERE  {{ ?x <{OWL_SOME_VALUES_FROM}> <http://www.w3.org/2002/07/owl#Thing> .
                         ?x <{OWL_ON_PROPERTY}> ?p . ?u ?p ?v }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-avf: ?x owl:allValuesFrom ?y . ?x owl:onProperty ?p .
    ///          ?u rdf:type ?x . ?u ?p ?v → ?v rdf:type ?y
    fn rule_cls_avf(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?v <{RDF_TYPE}> ?y }} }}
               WHERE  {{ ?x <{OWL_ALL_VALUES_FROM}> ?y . ?x <{OWL_ON_PROPERTY}> ?p .
                         ?u <{RDF_TYPE}> ?x . ?u ?p ?v .
                         FILTER(isIRI(?v) || isBlank(?v)) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-hv1: hasValue + onProperty + type(x) → triple
    fn rule_cls_hv1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?u ?p ?y }} }}
               WHERE  {{ ?x <{OWL_HAS_VALUE}> ?y . ?x <{OWL_ON_PROPERTY}> ?p .
                         ?u <{RDF_TYPE}> ?x }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-hv2: hasValue + onProperty + triple → type
    fn rule_cls_hv2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?u <{RDF_TYPE}> ?x }} }}
               WHERE  {{ ?x <{OWL_HAS_VALUE}> ?y . ?x <{OWL_ON_PROPERTY}> ?p .
                         ?u ?p ?y }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-maxc1: maxCardinality "0"^^xsd:nonNegativeInteger . type ?x . ?u ?p ?y → INCONSISTENCY
    fn rule_cls_maxc1(&self) -> Result<(), ReasoningError> {
        // Handle both xsd:nonNegativeInteger (OWL standard) and xsd:integer (common Turtle)
        let q = format!(
            r#"ASK {{
                {{ ?x <{OWL_MAX_CARDINALITY}> "0"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> }}
                UNION
                {{ ?x <{OWL_MAX_CARDINALITY}> "0"^^<http://www.w3.org/2001/XMLSchema#integer> }}
                ?x <{OWL_ON_PROPERTY}> ?p .
                {{ ?u <{RDF_TYPE}> ?x }} UNION {{ GRAPH <{tg}> {{ ?u <{RDF_TYPE}> ?x }} }}
                ?u ?p ?y
            }}"#,
            tg = self.target_graph
        );
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "owl:maxCardinality 0 violated".to_string(),
            ));
        }
        Ok(())
    }

    /// cls-maxc2: maxCardinality "1" . type ?x . ?u ?p ?y1 . ?u ?p ?y2 → y1 owl:sameAs y2
    fn rule_cls_maxc2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y1 <{OWL_SAME_AS}> ?y2 }} }}
               WHERE {{
                   ?x <{OWL_MAX_CARDINALITY}> "1"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                   ?x <{OWL_ON_PROPERTY}> ?p .
                   ?u <{RDF_TYPE}> ?x . ?u ?p ?y1 . ?u ?p ?y2 .
                   FILTER(?y1 != ?y2)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-maxqc1: maxQualifiedCardinality 0 + onClass + type + qualified property → INCONSISTENCY
    fn rule_cls_maxqc1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"ASK {{
                ?x <{OWL_MAX_QUAL_CARD}> "0"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                ?x <{OWL_ON_PROPERTY}> ?p .
                ?x <{OWL_ON_CLASS}> ?c .
                ?u <{RDF_TYPE}> ?x . ?u ?p ?y .
                ?y <{RDF_TYPE}> ?c .
            }}"#
        );
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "owl:maxQualifiedCardinality 0 violated".to_string(),
            ));
        }
        Ok(())
    }

    /// cls-maxqc2: maxQualifiedCardinality 0 + onClass owl:Thing → INCONSISTENCY (same as maxc1)
    fn rule_cls_maxqc2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"ASK {{
                ?x <{OWL_MAX_QUAL_CARD}> "0"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                ?x <{OWL_ON_PROPERTY}> ?p .
                ?x <{OWL_ON_CLASS}> <{OWL_THING}> .
                ?u <{RDF_TYPE}> ?x . ?u ?p ?y .
            }}"#
        );
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "owl:maxQualifiedCardinality 0 (Thing) violated".to_string(),
            ));
        }
        Ok(())
    }

    /// cls-maxqc3: maxQualifiedCardinality 1 + onClass + two qualified fillers → sameAs
    fn rule_cls_maxqc3(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y1 <{OWL_SAME_AS}> ?y2 }} }}
               WHERE {{
                   ?x <{OWL_MAX_QUAL_CARD}> "1"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                   ?x <{OWL_ON_PROPERTY}> ?p .
                   ?x <{OWL_ON_CLASS}> ?c .
                   ?u <{RDF_TYPE}> ?x .
                   ?u ?p ?y1 . ?y1 <{RDF_TYPE}> ?c .
                   ?u ?p ?y2 . ?y2 <{RDF_TYPE}> ?c .
                   FILTER(?y1 != ?y2)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-maxqc4: maxQualifiedCardinality 1 + onClass owl:Thing + two fillers → sameAs
    fn rule_cls_maxqc4(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y1 <{OWL_SAME_AS}> ?y2 }} }}
               WHERE {{
                   ?x <{OWL_MAX_QUAL_CARD}> "1"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> .
                   ?x <{OWL_ON_PROPERTY}> ?p .
                   ?x <{OWL_ON_CLASS}> <{OWL_THING}> .
                   ?u <{RDF_TYPE}> ?x .
                   ?u ?p ?y1 .
                   ?u ?p ?y2 .
                   FILTER(?y1 != ?y2)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cls-com: ?c1 owl:complementOf ?c2 . ?x type ?c1 . ?x type ?c2 → INCONSISTENCY
    fn rule_cls_com(&self) -> Result<(), ReasoningError> {
        let q = format!(
            "ASK {{ ?c1 <{OWL_COMPLEMENT_OF}> ?c2 . ?x <{RDF_TYPE}> ?c1 . ?x <{RDF_TYPE}> ?c2 }}"
        );
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "owl:complementOf violated: individual is member of both classes".to_string(),
            ));
        }
        Ok(())
    }

    /// cls-oo: ?c owl:oneOf list(x1..xn) → xi rdf:type ?c for each xi
    fn rule_cls_oo(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?xi <{RDF_TYPE}> ?c }} }}
               WHERE {{
                   ?c <{OWL_ONE_OF}> ?list .
                   ?list (<{RDF_FIRST}>|(<{RDF_REST}>+ /<{RDF_FIRST}>)) ?xi .
                   FILTER(?xi != <{RDF_NIL}>)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 7 — Semantics of Class Axioms
    // ═══════════════════════════════════════════════════════════════════════════

    /// cax-sco: ?c1 rdfs:subClassOf ?c2 . ?x type ?c1 → ?x type ?c2
    fn rule_cax_sco(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c2 }} }}
               WHERE  {{
                   ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 .
                   {{ ?x <{RDF_TYPE}> ?c1 }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c1 }} }}
                   FILTER(?c1 != ?c2)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cax-eqc1: ?c1 owl:equivalentClass ?c2 . ?x type ?c1 → ?x type ?c2
    fn rule_cax_eqc1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_EQUIV_CLASS}> ?c2 . ?x <{RDF_TYPE}> ?c1 }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cax-eqc2: ?c1 owl:equivalentClass ?c2 . ?x type ?c2 → ?x type ?c1
    fn rule_cax_eqc2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c1 }} }}
               WHERE  {{ ?c1 <{OWL_EQUIV_CLASS}> ?c2 . ?x <{RDF_TYPE}> ?c2 }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cax-adc: AllDisjointClasses → pairwise owl:disjointWith
    fn rule_cax_adc(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?ci <{OWL_DISJOINT_WITH}> ?cj }} }}
               WHERE {{
                   ?adc <{RDF_TYPE}> <{OWL_ALL_DISJOINT}> .
                   ?adc <{OWL_MEMBERS}> ?list .
                   ?list (<{RDF_FIRST}>|(<{RDF_REST}>+/<{RDF_FIRST}>)) ?ci .
                   ?list (<{RDF_FIRST}>|(<{RDF_REST}>+/<{RDF_FIRST}>)) ?cj .
                   FILTER(?ci != ?cj) FILTER(isIRI(?ci)) FILTER(isIRI(?cj))
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// cax-dw: ?c1 owl:disjointWith ?c2 . ?x type ?c1 . ?x type ?c2 → INCONSISTENCY
    fn rule_cax_dw(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        // Check both default graph and entailment graph for disjointWith and type triples
        let q = format!(
            "ASK {{ \
               {{ ?c1 <{OWL_DISJOINT_WITH}> ?c2 }} UNION {{ GRAPH <{tg}> {{ ?c1 <{OWL_DISJOINT_WITH}> ?c2 }} }} \
               {{ ?x <{RDF_TYPE}> ?c1 }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c1 }} }} \
               {{ ?x <{RDF_TYPE}> ?c2 }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c2 }} }} \
             }}"
        );
        if self.ask(&q)? {
            return Err(ReasoningError::Inconsistency(
                "owl:disjointWith violated".to_string(),
            ));
        }
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Table 9 — Semantics of the Schema Vocabulary
    // ═══════════════════════════════════════════════════════════════════════════

    /// scm-sco: ?c1 rdfs:subClassOf ?c2 . ?c2 rdfs:subClassOf ?c3 → ?c1 rdfs:subClassOf ?c3
    fn rule_scm_sco(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c3 }} }}
               WHERE  {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 .
                         ?c2 <{RDFS_SUB_CLASS_OF}> ?c3 .
                         FILTER(?c1 != ?c3) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-eqc1: ?c1 owl:equivalentClass ?c2 → ?c1 rdfs:subClassOf ?c2 . ?c2 rdfs:subClassOf ?c1
    fn rule_scm_eqc1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 . ?c2 <{RDFS_SUB_CLASS_OF}> ?c1 }} }}
               WHERE  {{ ?c1 <{OWL_EQUIV_CLASS}> ?c2 }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-eqc2: ?c1 rdfs:subClassOf ?c2 . ?c2 rdfs:subClassOf ?c1 → ?c1 owl:equivalentClass ?c2
    fn rule_scm_eqc2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{OWL_EQUIV_CLASS}> ?c2 }} }}
               WHERE  {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 .
                         ?c2 <{RDFS_SUB_CLASS_OF}> ?c1 .
                         FILTER(?c1 != ?c2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-spo: ?p1 rdfs:subPropertyOf ?p2 . ?p2 rdfs:subPropertyOf ?p3 → ?p1 rdfs:subPropertyOf ?p3
    fn rule_scm_spo(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p3 }} }}
               WHERE  {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 .
                         ?p2 <{RDFS_SUB_PROPERTY_OF}> ?p3 .
                         FILTER(?p1 != ?p3) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-eqp1: ?p1 owl:equivalentProperty ?p2 → ?p1 rdfs:subPropertyOf ?p2 . ?p2 rdfs:subPropertyOf ?p1
    fn rule_scm_eqp1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 . ?p2 <{RDFS_SUB_PROPERTY_OF}> ?p1 }} }}
               WHERE  {{ ?p1 <{OWL_EQUIV_PROP}> ?p2 }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-eqp2: ?p1 subPropertyOf ?p2 . ?p2 subPropertyOf ?p1 → equivalentProperty
    fn rule_scm_eqp2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{OWL_EQUIV_PROP}> ?p2 }} }}
               WHERE  {{ ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 .
                         ?p2 <{RDFS_SUB_PROPERTY_OF}> ?p1 .
                         FILTER(?p1 != ?p2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-dom1: ?p rdfs:domain ?c1 . ?c1 rdfs:subClassOf ?c2 → ?p rdfs:domain ?c2
    fn rule_scm_dom1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p <{RDFS_DOMAIN}> ?c2 }} }}
               WHERE  {{ ?p <{RDFS_DOMAIN}> ?c1 . ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-dom2: ?p2 rdfs:domain ?c . ?p1 rdfs:subPropertyOf ?p2 → ?p1 rdfs:domain ?c
    fn rule_scm_dom2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{RDFS_DOMAIN}> ?c }} }}
               WHERE  {{ ?p2 <{RDFS_DOMAIN}> ?c . ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-rng1: ?p rdfs:range ?c1 . ?c1 rdfs:subClassOf ?c2 → ?p rdfs:range ?c2
    fn rule_scm_rng1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p <{RDFS_RANGE}> ?c2 }} }}
               WHERE  {{ ?p <{RDFS_RANGE}> ?c1 . ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-rng2: ?p2 rdfs:range ?c . ?p1 rdfs:subPropertyOf ?p2 → ?p1 rdfs:range ?c
    fn rule_scm_rng2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p1 <{RDFS_RANGE}> ?c }} }}
               WHERE  {{ ?p2 <{RDFS_RANGE}> ?c . ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-hv: c1 hasValue i / onProperty p1, c2 hasValue i / onProperty p2 .
    ///         p1 subPropertyOf p2 → c1 subClassOf c2
    fn rule_scm_hv(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_HAS_VALUE}> ?i . ?c1 <{OWL_ON_PROPERTY}> ?p1 .
                         ?c2 <{OWL_HAS_VALUE}> ?i . ?c2 <{OWL_ON_PROPERTY}> ?p2 .
                         ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 . FILTER(?c1 != ?c2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-svf1: c1 someValuesFrom y1 / onProperty p, c2 someValuesFrom y2 / onProperty p .
    ///           y1 subClassOf y2 → c1 subClassOf c2
    fn rule_scm_svf1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_SOME_VALUES_FROM}> ?y1 . ?c1 <{OWL_ON_PROPERTY}> ?p .
                         ?c2 <{OWL_SOME_VALUES_FROM}> ?y2 . ?c2 <{OWL_ON_PROPERTY}> ?p .
                         ?y1 <{RDFS_SUB_CLASS_OF}> ?y2 . FILTER(?c1 != ?c2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-svf2: c1 someValuesFrom y / onProperty p1, c2 someValuesFrom y / onProperty p2 .
    ///           p1 subPropertyOf p2 → c1 subClassOf c2
    fn rule_scm_svf2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_SOME_VALUES_FROM}> ?y . ?c1 <{OWL_ON_PROPERTY}> ?p1 .
                         ?c2 <{OWL_SOME_VALUES_FROM}> ?y . ?c2 <{OWL_ON_PROPERTY}> ?p2 .
                         ?p1 <{RDFS_SUB_PROPERTY_OF}> ?p2 . FILTER(?c1 != ?c2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-avf1: c1 allValuesFrom y1, c2 allValuesFrom y2, same onProperty .
    ///           y1 subClassOf y2 → c1 subClassOf c2
    fn rule_scm_avf1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_ALL_VALUES_FROM}> ?y1 . ?c1 <{OWL_ON_PROPERTY}> ?p .
                         ?c2 <{OWL_ALL_VALUES_FROM}> ?y2 . ?c2 <{OWL_ON_PROPERTY}> ?p .
                         ?y1 <{RDFS_SUB_CLASS_OF}> ?y2 . FILTER(?c1 != ?c2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-avf2: c1 allValuesFrom y, c2 allValuesFrom y .
    ///           p2 subPropertyOf p1 → c1 subClassOf c2
    fn rule_scm_avf2(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c1 <{RDFS_SUB_CLASS_OF}> ?c2 }} }}
               WHERE  {{ ?c1 <{OWL_ALL_VALUES_FROM}> ?y . ?c1 <{OWL_ON_PROPERTY}> ?p1 .
                         ?c2 <{OWL_ALL_VALUES_FROM}> ?y . ?c2 <{OWL_ON_PROPERTY}> ?p2 .
                         ?p2 <{RDFS_SUB_PROPERTY_OF}> ?p1 . FILTER(?c1 != ?c2) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-cls: every owl:Class is subClassOf owl:Thing and subClassOf itself
    fn rule_scm_cls(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{
                   ?c <{RDFS_SUB_CLASS_OF}> <{OWL_THING}> .
                   ?c <{RDFS_SUB_CLASS_OF}> ?c .
               }} }}
               WHERE {{ ?c <{RDF_TYPE}> <{OWL_CLASS}> }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-int: intersection members are superclasses of the intersection
    fn rule_scm_int(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> ?ci }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list (<{RDF_FIRST}>|(<{RDF_REST}>+/<{RDF_FIRST}>)) ?ci .
                   FILTER(?ci != <{RDF_NIL}>) FILTER(isIRI(?ci))
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    /// scm-uni: each union member is a subclass of the union
    fn rule_scm_uni(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?ci <{RDFS_SUB_CLASS_OF}> ?c }} }}
               WHERE {{
                   ?c <{OWL_UNION_OF}> ?list .
                   ?list (<{RDF_FIRST}>|(<{RDF_REST}>+/<{RDF_FIRST}>)) ?ci .
                   FILTER(?ci != <{RDF_NIL}>) FILTER(isIRI(?ci))
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Helper ───────────────────────────────────────────────────────────────

    fn ask(&self, sparql: &str) -> Result<bool, ReasoningError> {
        match self.store.query(sparql)? {
            oxigraph::sparql::QueryResults::Boolean(b) => Ok(b),
            _ => Ok(false),
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    fn store_with(ttl: &str) -> TripleStore {
        let s = TripleStore::in_memory().unwrap();
        let prefixes = "@prefix owl:  <http://www.w3.org/2002/07/owl#> .
                        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
                        @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
                        @prefix ex:   <http://example.org/> .\n";
        s.load_str(&format!("{}{}", prefixes, ttl), RdfFormat::Turtle, None)
            .unwrap();
        s
    }

    fn ask(store: &TripleStore, sparql: &str) -> bool {
        match store.query(sparql).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => false,
        }
    }

    const TG: &str = OWL2_RL_ENTAILMENT_GRAPH;

    #[test]
    fn test_rl_same_as_sym() {
        let s = store_with("ex:a owl:sameAs ex:b .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!("ASK {{ GRAPH <{TG}> {{ <http://example.org/b> <{OWL_SAME_AS}> <http://example.org/a> }} }}")
        ));
    }

    #[test]
    fn test_rl_same_as_trans() {
        let s = store_with("ex:a owl:sameAs ex:b . ex:b owl:sameAs ex:c .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!("ASK {{ GRAPH <{TG}> {{ <http://example.org/a> <{OWL_SAME_AS}> <http://example.org/c> }} }}")
        ));
    }

    #[test]
    fn test_rl_transitive() {
        let s = store_with(
            "ex:anc rdf:type owl:TransitiveProperty .
             ex:a   ex:anc ex:b .
             ex:b   ex:anc ex:c .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/a> \
                 <http://example.org/anc> <http://example.org/c> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_symmetric() {
        let s = store_with(
            "ex:friend rdf:type owl:SymmetricProperty .
             ex:a ex:friend ex:b .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/b> \
                 <http://example.org/friend> <http://example.org/a> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_inverse() {
        let s = store_with("ex:parent owl:inverseOf ex:childOf . ex:a ex:parent ex:b .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/b> \
                 <http://example.org/childOf> <http://example.org/a> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_disjoint_inconsistency() {
        let s = store_with(
            "ex:Cat owl:disjointWith ex:Dog .
             ex:x   rdf:type ex:Cat .
             ex:x   rdf:type ex:Dog .",
        );
        assert!(Owl2RLReasoner::new(&s).materialize().is_err());
    }

    #[test]
    fn test_rl_property_chain() {
        let s = store_with(
            "ex:uncle owl:propertyChainAxiom ( ex:parent ex:brother ) .
             ex:alice ex:parent ex:bob .
             ex:bob   ex:brother ex:charlie .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/alice> \
                 <http://example.org/uncle> <http://example.org/charlie> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_has_key() {
        let s = store_with(
            "ex:Person owl:hasKey ( ex:ssn ) .
             ex:a rdf:type ex:Person ; ex:ssn \"123\" .
             ex:b rdf:type ex:Person ; ex:ssn \"123\" .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/a> \
                 <{OWL_SAME_AS}> <http://example.org/b> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_max_cardinality_1_same_as() {
        let s = store_with(
            "ex:R owl:maxCardinality \"1\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger> ;
                  owl:onProperty ex:spouse .
             ex:alice rdf:type ex:R ; ex:spouse ex:bob ; ex:spouse ex:robert .",
        );
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/bob> \
                 <{OWL_SAME_AS}> <http://example.org/robert> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_complement_of_inconsistency() {
        let s = store_with(
            "ex:Alive owl:complementOf ex:Dead .
             ex:x rdf:type ex:Alive .
             ex:x rdf:type ex:Dead .",
        );
        assert!(Owl2RLReasoner::new(&s).materialize().is_err());
    }

    #[test]
    fn test_rl_all_disjoint_classes() {
        let s = store_with(
            "_:adc rdf:type owl:AllDisjointClasses ;
                   owl:members ( ex:Cat ex:Dog ex:Fish ) .
             ex:x rdf:type ex:Cat .
             ex:x rdf:type ex:Dog .",
        );
        // Should produce pairwise disjointWith, then detect inconsistency
        assert!(Owl2RLReasoner::new(&s).materialize().is_err());
    }

    #[test]
    fn test_rl_scm_cls_subclass_thing() {
        let s = store_with("ex:Person rdf:type owl:Class .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/Person> \
                 <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
                 <http://www.w3.org/2002/07/owl#Thing> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_scm_int_intersection_superclass() {
        let s = store_with("ex:AB owl:intersectionOf ( ex:A ex:B ) .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        // AB ⊑ A and AB ⊑ B
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/AB> \
                 <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
                 <http://example.org/A> }} }}"
            )
        ));
    }

    #[test]
    fn test_rl_scm_uni_union_subclass() {
        let s = store_with("ex:AorB owl:unionOf ( ex:A ex:B ) .");
        Owl2RLReasoner::new(&s).materialize().unwrap();
        // A ⊑ AorB
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ <http://example.org/A> \
                 <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
                 <http://example.org/AorB> }} }}"
            )
        ));
    }
}
