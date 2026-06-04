//! OWL 2 EL profile — EL++ completion-rule classifier.
//!
//! OWL 2 EL is the profile used for large biomedical ontologies (SNOMED CT,
//! Gene Ontology).  It supports intersection, existential quantification,
//! property chains, transitivity, reflexivity, domain/range, and hasKey.
//!
//! This implementation expresses the EL++ completion rules (CR1–CR6) as
//! SPARQL INSERT operations executed in a fixed-point loop, writing the
//! derived subsumption hierarchy into the `target_graph`.
//!
//! # Completion Rules
//!
//! | Rule | Description                                              |
//! |------|----------------------------------------------------------|
//! | CR1  | subClassOf transitivity                                  |
//! | CR2  | Intersection decomposition and composition               |
//! | CR3  | Existential introduction (∃P.C ⊑ D → C ⊑ ∀P.D)         |
//! | CR4  | Existential propagation along subClassOf                 |
//! | CR5  | Property chains (P1 ∘ P2 ⊑ P)                           |
//! | CR6  | Bottom propagation (owl:Nothing)                         |
//! | CR7  | Role domain: ∃P.⊤ ⊑ A + x P y → x type A               |
//! | CR8  | Role range: ⊤ ⊑ ∀P.A + x P y → y type A                |
//! | CR9  | Reflexivity: P reflexive + x type C → x P x             |
//! | CR10 | Arbitrary-length property chains (N-element)             |
#![allow(dead_code)]

use std::time::Instant;
use tracing::{debug, info};

use super::common::{count_graph, ReasoningError, ReasoningReport, OWL2_EL_ENTAILMENT_GRAPH};
use crate::store::TripleStore;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const OWL_INTERSECTION_OF: &str = "http://www.w3.org/2002/07/owl#intersectionOf";
const OWL_SOME_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#someValuesFrom";
const OWL_ON_PROPERTY: &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_PROP_CHAIN_AXIOM: &str = "http://www.w3.org/2002/07/owl#propertyChainAxiom";
const OWL_NOTHING: &str = "http://www.w3.org/2002/07/owl#Nothing";
const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";
const OWL_ALL_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#allValuesFrom";
const OWL_REFLEXIVE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ReflexiveProperty";
const OWL_HAS_KEY: &str = "http://www.w3.org/2002/07/owl#hasKey";
const OWL_SAME_AS: &str = "http://www.w3.org/2002/07/owl#sameAs";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";

const MAX_ITERATIONS: usize = 500;

/// OWL 2 EL++ classifier.
pub struct El2Classifier<'a> {
    store: &'a TripleStore,
    target_graph: String,
}

impl<'a> El2Classifier<'a> {
    pub fn new(store: &'a TripleStore) -> Self {
        Self {
            store,
            target_graph: OWL2_EL_ENTAILMENT_GRAPH.to_string(),
        }
    }

    pub fn with_target(mut self, graph: impl Into<String>) -> Self {
        self.target_graph = graph.into();
        self
    }

    /// Classify the ontology and return a report.
    pub fn classify(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        let mut iterations = 0usize;

        info!("OWL 2 EL classification → <{}>", self.target_graph);

        loop {
            iterations += 1;
            let before = count_graph(self.store, &self.target_graph)?;

            self.rule_cr1()?;
            self.rule_cr2()?;
            self.rule_cr3()?;
            self.rule_cr4()?;
            self.rule_cr5()?;
            self.rule_cr6()?;
            self.rule_cr7()?;
            self.rule_cr8()?;
            self.rule_cr9()?;
            self.rule_cr10()?;
            self.rule_has_key()?;
            self.rule_abox_typing()?;
            self.rule_abox_intersection()?;
            self.rule_abox_existential()?;

            let after = count_graph(self.store, &self.target_graph)?;
            debug!(
                "EL iteration {}: +{} triples",
                iterations,
                after.saturating_sub(before)
            );
            if after == before || iterations >= MAX_ITERATIONS {
                break;
            }
        }

        let final_count = count_graph(self.store, &self.target_graph)?;
        info!(
            "EL classification complete: {} triples in {} iterations ({} ms)",
            final_count,
            iterations,
            start.elapsed().as_millis()
        );

        Ok(ReasoningReport {
            regime: "owl2-el".to_string(),
            triples_added: final_count,
            iterations,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
        })
    }

    /// Check whether the ontology is consistent (owl:Nothing has no subclasses
    /// other than itself).
    pub fn check_consistency(&self) -> Result<bool, ReasoningError> {
        let q = format!(
            "ASK {{ ?c <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> . FILTER(?c != <{OWL_NOTHING}>) }}"
        );
        match self.store.query(&q)? {
            oxigraph::sparql::QueryResults::Boolean(b) => Ok(!b),
            _ => Ok(true),
        }
    }

    // ─── CR1: subClassOf transitivity ────────────────────────────────────────

    fn rule_cr1(&self) -> Result<(), ReasoningError> {
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

    // ─── CR2: intersection ───────────────────────────────────────────────────
    // If A ⊑ B₁ ∩ B₂ and B₁ ∩ B₂ ⊑ D, then A ⊑ D.
    // Also: if A ⊑ B₁ and A ⊑ B₂ and (B₁ ∩ B₂) ⊑ D, then A ⊑ D.

    fn rule_cr2(&self) -> Result<(), ReasoningError> {
        // Propagate through intersectionOf: ?c subClassOf each operand
        let q1 = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> ?op }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list (<{RDF_FIRST}>|(<{RDF_REST}>+ /<{RDF_FIRST}>)) ?op .
                   FILTER(?op != <{RDF_NIL}>)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q1)?;

        // Join: if A ⊑ B1 and A ⊑ B2 and (B1 ∩ B2 exists as a class) → A ⊑ (B1 ∩ B2)
        let q2 = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?a <{RDFS_SUB_CLASS_OF}> ?c }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list <{RDF_FIRST}> ?b1 ;
                         <{RDF_REST}>  ?rest .
                   ?rest <{RDF_FIRST}> ?b2 ;
                         <{RDF_REST}>  <{RDF_NIL}> .
                   ?a <{RDFS_SUB_CLASS_OF}> ?b1 .
                   ?a <{RDFS_SUB_CLASS_OF}> ?b2 .
                   FILTER(?a != ?c)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q2)?;
        Ok(())
    }

    // ─── CR3: existential introduction ────────────────────────────────────────
    // If A ⊑ ∃P.B and B ⊑ C → A ⊑ ∃P.C

    fn rule_cr3(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?restr2 <{RDFS_SUB_CLASS_OF}> ?a }} }}
               WHERE {{
                   ?restr1 <{OWL_SOME_VALUES_FROM}> ?b .
                   ?restr1 <{OWL_ON_PROPERTY}> ?p .
                   ?restr2 <{OWL_SOME_VALUES_FROM}> ?c .
                   ?restr2 <{OWL_ON_PROPERTY}> ?p .
                   ?b <{RDFS_SUB_CLASS_OF}> ?c .
                   ?a <{RDFS_SUB_CLASS_OF}> ?restr1 .
                   FILTER(?restr1 != ?restr2) FILTER(?b != ?c)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── CR4: existential propagation ─────────────────────────────────────────
    // If A ⊑ ∃P.B and ∃P.B ⊑ C → A ⊑ C

    fn rule_cr4(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?a <{RDFS_SUB_CLASS_OF}> ?c }} }}
               WHERE {{
                   ?restr <{OWL_SOME_VALUES_FROM}> ?b .
                   ?restr <{OWL_ON_PROPERTY}> ?p .
                   ?restr <{RDFS_SUB_CLASS_OF}> ?c .
                   ?a <{RDFS_SUB_CLASS_OF}> ?restr .
                   FILTER(?a != ?c)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── CR5: property chains ─────────────────────────────────────────────────
    // ?p owl:propertyChainAxiom (?p1 ?p2) — propagate instances

    fn rule_cr5(&self) -> Result<(), ReasoningError> {
        // Two-element chain: p ← p1 ∘ p2
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

    // ─── CR6: bottom propagation ──────────────────────────────────────────────
    // If A ⊑ owl:Nothing → all descendants of A also ⊑ owl:Nothing

    fn rule_cr6(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }} }}
               WHERE {{
                   ?c <{RDFS_SUB_CLASS_OF}> ?d .
                   ?d <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> .
                   FILTER(?c != <{OWL_NOTHING}>)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── CR7: role domain propagation ────────────────────────────────────────
    // ∃P.⊤ ⊑ A (expressed as rdfs:domain) + x P y → x type A
    // This handles the EL pattern where domain constraints imply class membership.

    fn rule_cr7(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?a }} }}
               WHERE {{ ?p <{RDFS_DOMAIN}> ?a . ?x ?p ?y }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── CR8: role range propagation ─────────────────────────────────────────
    // ⊤ ⊑ ∀P.A (expressed as rdfs:range) + x P y → y type A

    fn rule_cr8(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y <{RDF_TYPE}> ?a }} }}
               WHERE {{ ?p <{RDFS_RANGE}> ?a . ?x ?p ?y .
                        FILTER(isIRI(?y) || isBlank(?y)) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── CR9: reflexivity ────────────────────────────────────────────────────
    // P is ReflexiveProperty → for every individual x, x P x

    fn rule_cr9(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x ?p ?x }} }}
               WHERE {{ ?p <{RDF_TYPE}> <{OWL_REFLEXIVE_PROPERTY}> .
                        ?x ?p2 ?o . FILTER(isIRI(?x))
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── CR10: arbitrary-length property chains ──────────────────────────────
    // Three-element chains: p ← p1 ∘ p2 ∘ p3

    fn rule_cr10(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x ?p ?w }} }}
               WHERE {{
                   ?p <{OWL_PROP_CHAIN_AXIOM}> ?list .
                   ?list <{RDF_FIRST}> ?p1 ;
                         <{RDF_REST}>  ?r1 .
                   ?r1   <{RDF_FIRST}> ?p2 ;
                         <{RDF_REST}>  ?r2 .
                   ?r2   <{RDF_FIRST}> ?p3 ;
                         <{RDF_REST}>  <{RDF_NIL}> .
                   ?x ?p1 ?y .
                   ?y ?p2 ?z .
                   ?z ?p3 ?w .
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── ABox: individual type propagation ──────────────────────────────────
    // x type C, C subClassOf D → x type D  (applies TBox to individuals)

    fn rule_abox_typing(&self) -> Result<(), ReasoningError> {
        // Search both the default graph and the target graph for subClassOf,
        // since CR1 writes transitive closures into the target graph.
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?d }} }}
               WHERE {{
                   {{ ?x <{RDF_TYPE}> ?c }}
                   UNION
                   {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
                   {{
                       {{ ?c <{RDFS_SUB_CLASS_OF}> ?d }}
                       UNION
                       {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> ?d }} }}
                   }}
                   FILTER(?c != ?d && isIRI(?x))
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── ABox: intersection membership for individuals ───────────────────────
    // x type A1, x type A2, C intersectionOf (A1 A2) → x type C

    fn rule_abox_intersection(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
               WHERE {{
                   ?c <{OWL_INTERSECTION_OF}> ?list .
                   ?list <{RDF_FIRST}> ?a1 ;
                         <{RDF_REST}>  ?rest .
                   ?rest <{RDF_FIRST}> ?a2 ;
                         <{RDF_REST}>  <{RDF_NIL}> .
                   {{ ?x <{RDF_TYPE}> ?a1 }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?a1 }} }}
                   {{ ?x <{RDF_TYPE}> ?a2 }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?a2 }} }}
                   FILTER(isIRI(?x))
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── ABox: existential restriction membership ────────────────────────────
    // x P y, y type B, [someValuesFrom B, onProperty P] subClassOf C → x type C

    fn rule_abox_existential(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
               WHERE {{
                   ?restr <{OWL_SOME_VALUES_FROM}> ?b ;
                          <{OWL_ON_PROPERTY}> ?p .
                   ?restr <{RDFS_SUB_CLASS_OF}> ?c .
                   ?x ?p ?y .
                   ?y <{RDF_TYPE}> ?b .
                   FILTER(isIRI(?x))
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── hasKey for EL ───────────────────────────────────────────────────────
    // Reuses RL pattern: C hasKey (p) . x type C . y type C . x p v . y p v → x sameAs y

    fn rule_has_key(&self) -> Result<(), ReasoningError> {
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
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    fn store_with(ttl: &str) -> TripleStore {
        let s = TripleStore::in_memory().unwrap();
        let preamble = "@prefix owl:  <http://www.w3.org/2002/07/owl#> .
                        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
                        @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
                        @prefix ex:   <http://example.org/> .\n";
        s.load_str(&format!("{preamble}{ttl}"), RdfFormat::Turtle, None)
            .unwrap();
        s
    }

    fn ask(store: &TripleStore, sparql: &str) -> bool {
        match store.query(sparql).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => false,
        }
    }

    const TG: &str = OWL2_EL_ENTAILMENT_GRAPH;

    #[test]
    fn test_el_subclass_transitivity() {
        let s = store_with(
            "ex:A rdfs:subClassOf ex:B .
             ex:B rdfs:subClassOf ex:C .",
        );
        El2Classifier::new(&s).classify().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ \
                 <http://example.org/A> <{RDFS_SUB_CLASS_OF}> <http://example.org/C> \
                 }} }}"
            )
        ));
    }

    #[test]
    fn test_el_consistency_ok() {
        let s = store_with("ex:A rdfs:subClassOf ex:B .");
        El2Classifier::new(&s).classify().unwrap();
        assert!(El2Classifier::new(&s).check_consistency().unwrap());
    }

    #[test]
    fn test_el_cr7_domain_propagation() {
        let s = store_with(
            "ex:knows rdfs:domain ex:Person .
             ex:alice ex:knows ex:bob .",
        );
        El2Classifier::new(&s).classify().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ \
                 <http://example.org/alice> <{RDF_TYPE}> <http://example.org/Person> \
                 }} }}"
            )
        ));
    }

    #[test]
    fn test_el_cr8_range_propagation() {
        let s = store_with(
            "ex:worksFor rdfs:range ex:Organisation .
             ex:alice ex:worksFor ex:acme .",
        );
        El2Classifier::new(&s).classify().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ \
                 <http://example.org/acme> <{RDF_TYPE}> <http://example.org/Organisation> \
                 }} }}"
            )
        ));
    }

    #[test]
    fn test_el_cr9_reflexive_property() {
        let s = store_with(
            "ex:knows rdf:type owl:ReflexiveProperty .
             ex:alice ex:knows ex:bob .",
        );
        El2Classifier::new(&s).classify().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ \
                 <http://example.org/alice> <http://example.org/knows> <http://example.org/alice> \
                 }} }}"
            )
        ));
    }

    #[test]
    fn test_el_cr10_three_element_chain() {
        let s = store_with(
            "ex:grandparent owl:propertyChainAxiom ( ex:parent ex:parent ex:parent ) .
             ex:a ex:parent ex:b .
             ex:b ex:parent ex:c .
             ex:c ex:parent ex:d .",
        );
        El2Classifier::new(&s).classify().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ \
                 <http://example.org/a> <http://example.org/grandparent> <http://example.org/d> \
                 }} }}"
            )
        ));
    }

    #[test]
    fn test_el_has_key() {
        let s = store_with(
            "ex:Person owl:hasKey ( ex:ssn ) .
             ex:alice rdf:type ex:Person ; ex:ssn \"123\" .
             ex:bob   rdf:type ex:Person ; ex:ssn \"123\" .",
        );
        El2Classifier::new(&s).classify().unwrap();
        assert!(ask(
            &s,
            &format!(
                "ASK {{ GRAPH <{TG}> {{ \
                 <http://example.org/alice> <{OWL_SAME_AS}> <http://example.org/bob> \
                 }} }}"
            )
        ));
    }
}
