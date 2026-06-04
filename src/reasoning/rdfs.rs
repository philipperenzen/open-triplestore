//! RDFS entailment via forward-chaining materialization.
//!
//! Implements the complete RDFS entailment rule set (rdfs1–rdfs13) as SPARQL
//! INSERT operations executed in a fixed-point loop.  All inferred triples are
//! written to the `target_graph` named graph, leaving asserted data untouched.
//!
//! # Rules
//!
//! | Rule   | Antecedent                                    | Consequent                          |
//! |--------|-----------------------------------------------|-------------------------------------|
//! | rdfs1  | ?x ?p ?lit . FILTER(isLiteral(?lit))          | ?lit rdf:type rdfs:Literal          |
//! | rdfs2  | ?p rdfs:domain ?C . ?x ?p ?y                 | ?x rdf:type ?C                     |
//! | rdfs3  | ?p rdfs:range  ?C . ?x ?p ?y                 | ?y rdf:type ?C                     |
//! | rdfs4a | ?x ?p ?y                                      | ?x rdf:type rdfs:Resource           |
//! | rdfs4b | ?x ?p ?y . FILTER(!isLiteral(?y))             | ?y rdf:type rdfs:Resource           |
//! | rdfs5  | ?p rdfs:subPropertyOf ?q . ?q … ?r            | ?p rdfs:subPropertyOf ?r            |
//! | rdfs6  | ?p rdf:type rdf:Property                      | ?p rdfs:subPropertyOf ?p            |
//! | rdfs7  | ?p rdfs:subPropertyOf ?q . ?x ?p ?y           | ?x ?q ?y                           |
//! | rdfs8  | ?C rdf:type rdfs:Class                        | ?C rdfs:subClassOf rdfs:Resource    |
//! | rdfs9  | ?C rdfs:subClassOf ?D . ?x rdf:type ?C        | ?x rdf:type ?D                     |
//! | rdfs10 | ?C rdf:type rdfs:Class                        | ?C rdfs:subClassOf ?C               |
//! | rdfs11 | ?C rdfs:subClassOf ?D . ?D … ?E               | ?C rdfs:subClassOf ?E               |
//! | rdfs12 | ?p rdf:type rdfs:ContainerMembershipProperty  | ?p rdfs:subPropertyOf rdfs:member   |
//! | rdfs13 | ?D rdf:type rdfs:Datatype                     | ?D rdfs:subClassOf rdfs:Literal     |

use std::time::Instant;
use tracing::{debug, info};

use super::common::{count_graph, ReasoningError, ReasoningReport, RDFS_ENTAILMENT_GRAPH};
use crate::store::TripleStore;

// ─── Namespace constants ──────────────────────────────────────────────────────

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_PROPERTY: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const RDFS_SUB_PROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_RESOURCE: &str = "http://www.w3.org/2000/01/rdf-schema#Resource";
const RDFS_LITERAL: &str = "http://www.w3.org/2000/01/rdf-schema#Literal";
const RDFS_CLASS: &str = "http://www.w3.org/2000/01/rdf-schema#Class";
const RDFS_DATATYPE: &str = "http://www.w3.org/2000/01/rdf-schema#Datatype";
const RDFS_MEMBER: &str = "http://www.w3.org/2000/01/rdf-schema#member";
const RDFS_CONTAINER_MEMBERSHIP_PROPERTY: &str =
    "http://www.w3.org/2000/01/rdf-schema#ContainerMembershipProperty";

/// Maximum fixed-point iterations (safety valve).
const MAX_ITERATIONS: usize = 500;

// ─── Materializer ─────────────────────────────────────────────────────────────

/// Forward-chaining RDFS materializer.
///
/// Writes inferred triples into `target_graph`.  On the next call, previously
/// entailed triples are already present and will not be double-counted.
pub struct RdfsMaterializer<'a> {
    store: &'a TripleStore,
    target_graph: String,
}

impl<'a> RdfsMaterializer<'a> {
    /// Create a materializer targeting the standard RDFS entailment graph
    /// (`urn:entailment:rdfs`).
    pub fn new(store: &'a TripleStore) -> Self {
        Self::with_target(store, RDFS_ENTAILMENT_GRAPH)
    }

    /// Create a materializer targeting a custom named graph.
    pub fn with_target(store: &'a TripleStore, target_graph: impl Into<String>) -> Self {
        Self {
            store,
            target_graph: target_graph.into(),
        }
    }

    /// Run all RDFS rules to fixed point and return a summary.
    pub fn materialize(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        let mut iterations = 0usize;

        info!("RDFS materialization → <{}>", self.target_graph);

        loop {
            iterations += 1;
            let before = count_graph(self.store, &self.target_graph)?;

            // Core inference rules (produce chained inferences)
            self.apply_rdfs2()?;
            self.apply_rdfs3()?;
            self.apply_rdfs5()?;
            self.apply_rdfs7()?;
            self.apply_rdfs9()?;
            self.apply_rdfs11()?;
            self.apply_rdfs12()?;
            self.apply_rdfs13()?;

            let after = count_graph(self.store, &self.target_graph)?;
            let added_this_round = after.saturating_sub(before);

            debug!(
                "RDFS iteration {}: +{} triples",
                iterations, added_this_round
            );

            if added_this_round == 0 || iterations >= MAX_ITERATIONS {
                break;
            }
        }

        // Axiomatic rules: run once after fixed-point convergence.
        // These generate many triples but do not produce chained inferences
        // with the core rules above.
        self.apply_rdfs1()?;
        self.apply_rdfs4a()?;
        self.apply_rdfs4b()?;
        self.apply_rdfs6()?;
        self.apply_rdfs8()?;
        self.apply_rdfs10()?;

        let final_count = count_graph(self.store, &self.target_graph)?;

        info!(
            "RDFS materialization complete: {} triples in {} iterations ({} ms)",
            final_count,
            iterations,
            start.elapsed().as_millis()
        );

        Ok(ReasoningReport {
            regime: "rdfs".to_string(),
            triples_added: final_count,
            iterations,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
        })
    }

    // ─── Rule rdfs2: domain ───────────────────────────────────────────────────

    fn apply_rdfs2(&self) -> Result<(), ReasoningError> {
        // Also look in target graph for property triples inferred by rdfs7
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

    // ─── Rule rdfs3: range ────────────────────────────────────────────────────

    fn apply_rdfs3(&self) -> Result<(), ReasoningError> {
        // Also look in target graph for property triples inferred by rdfs7
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?y <{RDF_TYPE}> ?c }} }}
               WHERE  {{
                   ?p <{RDFS_RANGE}> ?c .
                   {{ ?x ?p ?y }} UNION {{ GRAPH <{tg}> {{ ?x ?p ?y }} }}
                   FILTER(isIRI(?y) || isBlank(?y))
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs5: subPropertyOf transitivity ───────────────────────────────

    fn apply_rdfs5(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?r }} }}
               WHERE  {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?q .
                         ?q <{RDFS_SUB_PROPERTY_OF}> ?r .
                         FILTER(?p != ?r) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs7: subPropertyOf inheritance ────────────────────────────────

    fn apply_rdfs7(&self) -> Result<(), ReasoningError> {
        // Also look in target graph for subPropertyOf from rdfs5 and property triples from rdfs7
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x ?q ?y }} }}
               WHERE  {{
                   {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?q }} UNION {{ GRAPH <{tg}> {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?q }} }}
                   {{ ?x ?p ?y }} UNION {{ GRAPH <{tg}> {{ ?x ?p ?y }} }}
                   FILTER(?p != ?q)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs9: subClassOf instance propagation ──────────────────────────

    fn apply_rdfs9(&self) -> Result<(), ReasoningError> {
        // Also look in target graph for types inferred in previous iterations
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?d }} }}
               WHERE  {{
                   {{ ?c <{RDFS_SUB_CLASS_OF}> ?d }}
                   UNION {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> ?d }} }}
                   {{ ?x <{RDF_TYPE}> ?c }}
                   UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?c }} }}
                   FILTER(?c != ?d)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs11: subClassOf transitivity ─────────────────────────────────

    fn apply_rdfs11(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> ?e }} }}
               WHERE  {{
                   {{ ?c <{RDFS_SUB_CLASS_OF}> ?d }}
                   UNION {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> ?d }} }}
                   {{ ?d <{RDFS_SUB_CLASS_OF}> ?e }}
                   UNION {{ GRAPH <{tg}> {{ ?d <{RDFS_SUB_CLASS_OF}> ?e }} }}
                   FILTER(?c != ?e)
               }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs12: ContainerMembershipProperty → subPropertyOf rdfs:member ─

    fn apply_rdfs12(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p <{RDFS_SUB_PROPERTY_OF}> <{RDFS_MEMBER}> }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{RDFS_CONTAINER_MEMBERSHIP_PROPERTY}> }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs13: rdfs:Datatype → subClassOf rdfs:Literal ────────────────

    fn apply_rdfs13(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?d <{RDFS_SUB_CLASS_OF}> <{RDFS_LITERAL}> }} }}
               WHERE  {{ ?d <{RDF_TYPE}> <{RDFS_DATATYPE}> }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Axiomatic rules (run once after fixed-point convergence)
    // ═══════════════════════════════════════════════════════════════════════════

    // ─── Rule rdfs1: datatype → rdfs:Literal ───────────────────────────────
    // For every typed literal, its datatype IRI is a subclass of rdfs:Literal.
    // (Literals cannot be subjects in RDF, so the W3C RDFS rule is expressed
    //  via the datatype rather than the literal value itself.)

    fn apply_rdfs1(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?dt <{RDFS_SUB_CLASS_OF}> <{RDFS_LITERAL}> }} }}
               WHERE  {{ ?s ?p ?lit . FILTER(isLiteral(?lit))
                         BIND(DATATYPE(?lit) AS ?dt) FILTER(BOUND(?dt)) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs4a: every subject is a rdfs:Resource ───────────────────────

    fn apply_rdfs4a(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?s <{RDF_TYPE}> <{RDFS_RESOURCE}> }} }}
               WHERE  {{ ?s ?p ?o . FILTER(isIRI(?s) || isBlank(?s)) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs4b: every IRI/bnode object is a rdfs:Resource ──────────────

    fn apply_rdfs4b(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?o <{RDF_TYPE}> <{RDFS_RESOURCE}> }} }}
               WHERE  {{ ?s ?p ?o . FILTER(isIRI(?o) || isBlank(?o)) }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs6: every property is subPropertyOf itself ──────────────────

    fn apply_rdfs6(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?p }} }}
               WHERE  {{ ?p <{RDF_TYPE}> <{RDF_PROPERTY}> }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs8: every class is subClassOf rdfs:Resource ─────────────────

    fn apply_rdfs8(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> <{RDFS_RESOURCE}> }} }}
               WHERE  {{ ?c <{RDF_TYPE}> <{RDFS_CLASS}> }}"#,
            tg = self.target_graph
        );
        self.store.update(&q)?;
        Ok(())
    }

    // ─── Rule rdfs10: every class is subClassOf itself ───────────────────────

    fn apply_rdfs10(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> ?c }} }}
               WHERE  {{ ?c <{RDF_TYPE}> <{RDFS_CLASS}> }}"#,
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
        s.load_str(ttl, RdfFormat::Turtle, None).unwrap();
        s
    }

    fn ask(store: &TripleStore, sparql: &str) -> bool {
        match store.query(sparql).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => false,
        }
    }

    #[test]
    fn test_rdfs_subclass_type_propagation() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             @prefix ex:   <http://example.org/> .
             ex:Student rdfs:subClassOf ex:Person .
             ex:alice   rdf:type        ex:Student .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/alice> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://example.org/Person> } }"
        ));
    }

    #[test]
    fn test_rdfs_subclass_transitivity() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix ex:   <http://example.org/> .
             ex:PhD     rdfs:subClassOf ex:Student .
             ex:Student rdfs:subClassOf ex:Person .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/PhD> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://example.org/Person> } }"
        ));
    }

    #[test]
    fn test_rdfs_domain() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             @prefix ex:   <http://example.org/> .
             ex:teaches rdfs:domain ex:Professor .
             ex:bob     ex:teaches  ex:cs101 .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/bob> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://example.org/Professor> } }"
        ));
    }

    #[test]
    fn test_rdfs_range() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             @prefix ex:   <http://example.org/> .
             ex:teaches rdfs:range ex:Course .
             ex:bob     ex:teaches ex:cs101 .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/cs101> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://example.org/Course> } }"
        ));
    }

    #[test]
    fn test_rdfs_subproperty() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix ex:   <http://example.org/> .
             ex:fatherOf rdfs:subPropertyOf ex:parentOf .
             ex:bob      ex:fatherOf        ex:alice .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/bob> \
               <http://example.org/parentOf> \
               <http://example.org/alice> } }"
        ));
    }

    #[test]
    fn test_rdfs1_datatype_subclass_literal() {
        let s = store_with(
            "@prefix ex:  <http://example.org/> .
             @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
             ex:bob ex:age \"42\"^^xsd:integer .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        // The datatype xsd:integer should be inferred as subClassOf rdfs:Literal
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://www.w3.org/2001/XMLSchema#integer> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://www.w3.org/2000/01/rdf-schema#Literal> } }"
        ));
    }

    #[test]
    fn test_rdfs4a_subject_resource() {
        let s = store_with(
            "@prefix ex: <http://example.org/> .
             ex:bob ex:knows ex:alice .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/bob> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://www.w3.org/2000/01/rdf-schema#Resource> } }"
        ));
    }

    #[test]
    fn test_rdfs4b_object_resource() {
        let s = store_with(
            "@prefix ex: <http://example.org/> .
             ex:bob ex:knows ex:alice .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/alice> \
               <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
               <http://www.w3.org/2000/01/rdf-schema#Resource> } }"
        ));
    }

    #[test]
    fn test_rdfs6_property_self_subproperty() {
        let s = store_with(
            "@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             @prefix ex:   <http://example.org/> .
             ex:knows rdf:type rdf:Property .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/knows> \
               <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> \
               <http://example.org/knows> } }"
        ));
    }

    #[test]
    fn test_rdfs8_class_subclass_resource() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix ex:   <http://example.org/> .
             ex:Person rdfs:subClassOf ex:Agent .
             ex:Person a rdfs:Class .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/Person> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://www.w3.org/2000/01/rdf-schema#Resource> } }"
        ));
    }

    #[test]
    fn test_rdfs10_class_self_subclass() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix ex:   <http://example.org/> .
             ex:Person a rdfs:Class .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://example.org/Person> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://example.org/Person> } }"
        ));
    }

    #[test]
    fn test_rdfs12_container_membership_property() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
             rdf:_1 a rdfs:ContainerMembershipProperty .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://www.w3.org/1999/02/22-rdf-syntax-ns#_1> \
               <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> \
               <http://www.w3.org/2000/01/rdf-schema#member> } }"
        ));
    }

    #[test]
    fn test_rdfs13_datatype_subclass_literal() {
        let s = store_with(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
             @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
             xsd:integer a rdfs:Datatype .",
        );
        RdfsMaterializer::new(&s).materialize().unwrap();
        assert!(ask(
            &s,
            "ASK { GRAPH <urn:entailment:rdfs> \
             { <http://www.w3.org/2001/XMLSchema#integer> \
               <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
               <http://www.w3.org/2000/01/rdf-schema#Literal> } }"
        ));
    }
}
