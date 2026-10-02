//! OWL 2 EL profile — EL++ completion-rule classifier.
//!
//! OWL 2 EL is the profile used for large biomedical ontologies (SNOMED CT,
//! Gene Ontology). This module covers part of it: intersections, existential
//! restrictions, equivalent classes, the property hierarchy, transitive and
//! reflexive properties, property chains of two or three properties,
//! domain/range, disjoint classes and hasKey.
//!
//! The completion rules are SPARQL INSERT operations executed in a
//! fixed-point loop, writing what they derive into the `target_graph`. The
//! rule names are this module's own; they do not follow the CR numbering of
//! the EL++ literature.
//!
//! # Completion Rules
//!
//! | Rule  | Description                                                  |
//! |-------|--------------------------------------------------------------|
//! | EQC   | `A ≡ B` → `A ⊑ B`, `B ⊑ A`                                   |
//! | CR1   | subClassOf transitivity                                      |
//! | CR2   | Intersection decomposition and (two-operand) composition     |
//! | ROLE  | `p ≡ q` → `p ⊑ q`, `q ⊑ p`; subPropertyOf transitivity       |
//! | CR4   | `A ⊑ ∃r.B`, `B ⊑ C` (or `C = ⊤`), `r ⊑ s` → `A ⊑ ∃s.C`       |
//! | CR5   | Property chains (P1 ∘ P2 ⊑ P)                                |
//! | CR6   | Bottom: along subClassOf, through `∃r.⊥`, and from disjoint superclasses |
//! | CR7   | Role domain: `P rdfs:domain A` + `x P y` → `x type A`        |
//! | CR8   | Role range: `P rdfs:range A` + `x P y` → `y type A`          |
//! | CR9   | Reflexivity: P reflexive → `x P x`                           |
//! | CR10  | Three-element property chains                                |
//! | ABox  | Typing, intersection and existential membership; sub-property and transitive property assertions; n-ary hasKey |
//!
//! There is deliberately no rule that turns `A ⊑ ∃p.B` and `B ⊑ C` into a
//! subsumption *into* `A`: the old CR3 wrote `∃p.C ⊑ A`, which does not
//! follow, and with the ABox existential rule it typed any `x p y, y a C`
//! as an `A`.
//!
//! # Consistency
//!
//! An EL ontology is inconsistent when an individual is an instance of
//! `owl:Nothing` (directly, through its classes, or by being typed with two
//! disjoint classes) or `owl:Thing ⊑ owl:Nothing`. An unsatisfiable *class*
//! (`C ⊑ owl:Nothing`) without instances is not an inconsistency;
//! [`El2Classifier::unsatisfiable_classes`] lists those.
//! [`El2Classifier::classify`] checks consistency after the fixed point.
#![allow(dead_code)]

use std::time::Instant;
use tracing::{debug, info};

use super::common::{count_graph, ReasoningError, ReasoningReport, OWL2_EL_ENTAILMENT_GRAPH};
use crate::store::TripleStore;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_SUB_PROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";
const OWL_EQUIVALENT_CLASS: &str = "http://www.w3.org/2002/07/owl#equivalentClass";
const OWL_EQUIVALENT_PROPERTY: &str = "http://www.w3.org/2002/07/owl#equivalentProperty";
const OWL_TRANSITIVE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#TransitiveProperty";
const OWL_DISJOINT_WITH: &str = "http://www.w3.org/2002/07/owl#disjointWith";
const OWL_INTERSECTION_OF: &str = "http://www.w3.org/2002/07/owl#intersectionOf";
const OWL_SOME_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#someValuesFrom";
const OWL_ON_PROPERTY: &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_PROP_CHAIN_AXIOM: &str = "http://www.w3.org/2002/07/owl#propertyChainAxiom";
const OWL_NOTHING: &str = "http://www.w3.org/2002/07/owl#Nothing";
const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";
const OWL_ALL_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#allValuesFrom";
const OWL_REFLEXIVE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#ReflexiveProperty";
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
    /// When set, the rules read ONLY these graphs (plus the target graph).
    /// Without it they read the unnamed default graph, as they always did.
    sources: Option<Vec<String>>,
    /// Check consistency after the fixed point and fail with
    /// [`ReasoningError::Inconsistency`] when the ontology is inconsistent.
    pub detect_inconsistency: bool,
}

impl<'a> El2Classifier<'a> {
    /// Restrict the rules to `sources` (plus the target graph). Without a
    /// scope the rules read the unnamed default graph only, so a dataset's
    /// named graphs — and the model version it conforms to — were invisible to
    /// materialisation; this is what `POST /api/reasoning/materialize` sets
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
            None => self.store.update(sparql),
        }
    }

    fn run_query(
        &self,
        sparql: &str,
    ) -> Result<oxigraph::sparql::QueryResults<'static>, crate::store::engine::StoreError> {
        match self.scope() {
            Some(scope) => self.store.query_scoped(sparql, &scope),
            None => self.store.query(sparql),
        }
    }

    pub fn new(store: &'a TripleStore) -> Self {
        Self {
            store,
            target_graph: OWL2_EL_ENTAILMENT_GRAPH.to_string(),
            sources: None,
            detect_inconsistency: true,
        }
    }

    pub fn with_target(mut self, graph: impl Into<String>) -> Self {
        self.target_graph = graph.into();
        self
    }

    /// Classify the ontology and return a report.
    ///
    /// Fails with [`ReasoningError::Inconsistency`] when the result is
    /// inconsistent (see [`check_consistency`](Self::check_consistency)) and
    /// `detect_inconsistency` is set; what was derived stays in the target
    /// graph.
    pub fn classify(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        let mut iterations = 0usize;
        // Report the delta this run produced, not the graph's final size.
        let initial = count_graph(self.store, &self.target_graph)?;

        info!("OWL 2 EL classification → <{}>", self.target_graph);

        loop {
            iterations += 1;
            let before = count_graph(self.store, &self.target_graph)?;

            // TBox
            self.rule_equivalent_class()?;
            self.rule_cr1()?;
            self.rule_cr2()?;
            self.rule_role_hierarchy()?;
            self.rule_cr4()?;
            self.rule_cr6()?;
            // ABox
            self.rule_abox_subproperty()?;
            self.rule_abox_transitive()?;
            self.rule_cr5()?;
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

        if self.detect_inconsistency {
            if let Some(reason) = self.inconsistency()? {
                return Err(ReasoningError::Inconsistency(reason));
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
            triples_added: final_count.saturating_sub(initial),
            iterations,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
            ..Default::default()
        })
    }

    /// Whether the classified ontology is consistent: no individual is an
    /// instance of `owl:Nothing` or of two disjoint classes, and
    /// `owl:Thing ⊑ owl:Nothing` does not hold. Run it after
    /// [`classify`](Self::classify) (with `detect_inconsistency` off), since
    /// it reads what the rules derived.
    ///
    /// An unsatisfiable class (`C ⊑ owl:Nothing`) is *not* an inconsistency
    /// on its own — see [`unsatisfiable_classes`](Self::unsatisfiable_classes).
    pub fn check_consistency(&self) -> Result<bool, ReasoningError> {
        Ok(self.inconsistency()?.is_none())
    }

    /// Why the ontology is inconsistent, or `None` when it is consistent.
    fn inconsistency(&self) -> Result<Option<String>, ReasoningError> {
        let tg = &self.target_graph;
        let in_nothing = format!(
            r#"SELECT ?x WHERE {{
                   {{ ?x <{RDF_TYPE}> <{OWL_NOTHING}> }}
                   UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> <{OWL_NOTHING}> }} }}
               }} LIMIT 1"#
        );
        if let Some(x) = self.first_binding(&in_nothing, "x")? {
            return Ok(Some(format!("{x} is an instance of owl:Nothing")));
        }
        let top_bottom = format!(
            r#"ASK {{
                   {{ <{OWL_THING}> <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }}
                   UNION {{ GRAPH <{tg}> {{ <{OWL_THING}> <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }} }}
               }}"#
        );
        if let oxigraph::sparql::QueryResults::Boolean(true) = self.run_query(&top_bottom)? {
            return Ok(Some("owl:Thing is a subclass of owl:Nothing".to_string()));
        }
        let disjoint = format!(
            r#"SELECT ?x WHERE {{
                   {{ ?a <{OWL_DISJOINT_WITH}> ?b }}
                   {{ ?x <{RDF_TYPE}> ?a }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?a }} }}
                   {{ ?x <{RDF_TYPE}> ?b }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?b }} }}
               }} LIMIT 1"#
        );
        if let Some(x) = self.first_binding(&disjoint, "x")? {
            return Ok(Some(format!("{x} is an instance of two disjoint classes")));
        }
        Ok(None)
    }

    /// The named classes other than `owl:Nothing` that are subclasses of
    /// `owl:Nothing` after classification, sorted.
    pub fn unsatisfiable_classes(&self) -> Result<Vec<String>, ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            r#"SELECT DISTINCT ?c WHERE {{
                   {{ ?c <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }}
                   UNION {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }} }}
                   FILTER(isIRI(?c) && ?c != <{OWL_NOTHING}>)
               }}"#
        );
        let mut out = Vec::new();
        if let oxigraph::sparql::QueryResults::Solutions(sols) = self.run_query(&q)? {
            for sol in sols {
                let sol = sol.map_err(|e| ReasoningError::Query(e.to_string()))?;
                if let Some(oxigraph::model::Term::NamedNode(c)) = sol.get("c") {
                    out.push(c.as_str().to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }

    fn first_binding(&self, sparql: &str, var: &str) -> Result<Option<String>, ReasoningError> {
        if let oxigraph::sparql::QueryResults::Solutions(mut sols) = self.run_query(sparql)? {
            if let Some(sol) = sols.next() {
                let sol = sol.map_err(|e| ReasoningError::Query(e.to_string()))?;
                return Ok(sol.get(var).map(|t| t.to_string()));
            }
        }
        Ok(None)
    }

    // ─── EQC: equivalent classes ─────────────────────────────────────────────
    // A ≡ B → A ⊑ B and B ⊑ A. Either side may be a class expression (an
    // intersection or a restriction), which is how EL definitions are written.

    fn rule_equivalent_class(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?a <{RDFS_SUB_CLASS_OF}> ?b . ?b <{RDFS_SUB_CLASS_OF}> ?a }} }}
               WHERE  {{ ?a <{OWL_EQUIVALENT_CLASS}> ?b . FILTER(?a != ?b) }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
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
        self.run_update(&q)?;
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
        self.run_update(&q1)?;

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
        self.run_update(&q2)?;
        Ok(())
    }

    // ─── ROLE: the property hierarchy ────────────────────────────────────────
    // p ≡ q → p ⊑ q and q ⊑ p; p ⊑ q, q ⊑ r → p ⊑ r.

    fn rule_role_hierarchy(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?q . ?q <{RDFS_SUB_PROPERTY_OF}> ?p }} }}
               WHERE  {{ ?p <{OWL_EQUIVALENT_PROPERTY}> ?q . FILTER(?p != ?q) }} ;
               INSERT {{ GRAPH <{tg}> {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?r }} }}
               WHERE  {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?q .
                         ?q <{RDFS_SUB_PROPERTY_OF}> ?r .
                         FILTER(?p != ?r) }}"#
        );
        self.run_update(&q)?;
        Ok(())
    }

    // ─── CR4: existential subsumption, structurally ──────────────────────────
    // A ⊑ ∃r.B, B ⊑ C (or B = C, or C = ⊤), r ⊑ s (or r = s) → A ⊑ ∃s.C,
    // for every restriction ∃s.C in scope. CR1 then carries A to whatever
    // ∃s.C is a subclass of (∃s.C ⊑ D gives A ⊑ D), and CR2 to intersections
    // that have ∃s.C as an operand. Two restrictions on the same property
    // with the same filler are the same class: each subsumes the other.

    fn rule_cr4(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?a <{RDFS_SUB_CLASS_OF}> ?r2 }} }}
               WHERE {{
                   ?a  <{RDFS_SUB_CLASS_OF}> ?r1 .
                   ?r1 <{OWL_ON_PROPERTY}> ?p ;
                       <{OWL_SOME_VALUES_FROM}> ?b .
                   {{ ?r2 <{OWL_SOME_VALUES_FROM}> ?b }}
                   UNION {{ ?b <{RDFS_SUB_CLASS_OF}> ?c . ?r2 <{OWL_SOME_VALUES_FROM}> ?c }}
                   UNION {{ ?r2 <{OWL_SOME_VALUES_FROM}> <{OWL_THING}> }}
                   ?r2 <{OWL_ON_PROPERTY}> ?s .
                   FILTER(?s = ?p || EXISTS {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?s }})
                   FILTER(?r1 != ?r2 && ?a != ?r2)
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
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
        self.run_update(&q)?;
        Ok(())
    }

    // ─── CR6: bottom propagation ──────────────────────────────────────────────
    // A ⊑ D, D ⊑ ⊥ → A ⊑ ⊥; A ⊑ ∃r.B, B ⊑ ⊥ → A ⊑ ⊥ (∃r.⊥ is empty);
    // A ⊑ B, A ⊑ C, B disjointWith C → A ⊑ ⊥ (and A ⊑ C, A disjointWith C).

    fn rule_cr6(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?c <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }} }}
               WHERE {{
                   ?c <{RDFS_SUB_CLASS_OF}> ?d .
                   ?d <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> .
                   FILTER(?c != <{OWL_NOTHING}>)
               }} ;
               INSERT {{ GRAPH <{tg}> {{ ?a <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }} }}
               WHERE {{
                   ?a <{RDFS_SUB_CLASS_OF}> ?r .
                   ?r <{OWL_ON_PROPERTY}> ?p ;
                      <{OWL_SOME_VALUES_FROM}> ?b .
                   {{ ?b <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }}
                   UNION {{ BIND(<{OWL_NOTHING}> AS ?b) }}
               }} ;
               INSERT {{ GRAPH <{tg}> {{ ?a <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }} }}
               WHERE {{
                   {{ ?b <{OWL_DISJOINT_WITH}> ?c }} UNION {{ ?c <{OWL_DISJOINT_WITH}> ?b }}
                   ?a <{RDFS_SUB_CLASS_OF}> ?b .
                   ?a <{RDFS_SUB_CLASS_OF}> ?c .
                   FILTER(?a != <{OWL_NOTHING}>)
               }} ;
               INSERT {{ GRAPH <{tg}> {{ ?a <{RDFS_SUB_CLASS_OF}> <{OWL_NOTHING}> }} }}
               WHERE {{
                   {{ ?a <{OWL_DISJOINT_WITH}> ?c }} UNION {{ ?c <{OWL_DISJOINT_WITH}> ?a }}
                   ?a <{RDFS_SUB_CLASS_OF}> ?c .
                   FILTER(?a != <{OWL_NOTHING}>)
               }}"#
        );
        self.run_update(&q)?;
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
        self.run_update(&q)?;
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
        self.run_update(&q)?;
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
        self.run_update(&q)?;
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
        self.run_update(&q)?;
        Ok(())
    }

    // ─── ABox: property hierarchy and transitivity ───────────────────────────
    // x p y, p ⊑ q → x q y;  x p y, y p z, p transitive → x p z

    fn rule_abox_subproperty(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x ?q ?y }} }}
               WHERE {{ ?p <{RDFS_SUB_PROPERTY_OF}> ?q . ?x ?p ?y .
                        FILTER(isIRI(?q) && ?p != ?q) }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    fn rule_abox_transitive(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x ?p ?z }} }}
               WHERE {{ ?p <{RDF_TYPE}> <{OWL_TRANSITIVE_PROPERTY}> .
                        ?x ?p ?y . ?y ?p ?z . FILTER(isIRI(?p)) }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
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
        self.run_update(&q)?;
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
        self.run_update(&q)?;
        Ok(())
    }

    // ─── ABox: existential restriction membership ────────────────────────────
    // x P y, y type B (or B = ⊤), R = [someValuesFrom B, onProperty P]
    // → x type R. ABox typing then carries x to every superclass of R, and the
    // intersection rule to an intersection that has R as an operand.

    fn rule_abox_existential(&self) -> Result<(), ReasoningError> {
        let q = format!(
            r#"INSERT {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> ?restr }} }}
               WHERE {{
                   ?restr <{OWL_SOME_VALUES_FROM}> ?b ;
                          <{OWL_ON_PROPERTY}> ?p .
                   ?x ?p ?y .
                   {{ ?y <{RDF_TYPE}> ?b }}
                   UNION {{ GRAPH <{tg}> {{ ?y <{RDF_TYPE}> ?b }} }}
                   UNION {{ BIND(<{OWL_THING}> AS ?b) }}
                   FILTER(isIRI(?x))
               }}"#,
            tg = self.target_graph
        );
        self.run_update(&q)?;
        Ok(())
    }

    // ─── hasKey for EL ───────────────────────────────────────────────────────
    // C hasKey (p1 … pn) . x type C . y type C . x pi vi . y pi vi for every
    // key property → x sameAs y. The keys are read in Rust (the list walk is
    // shared with the RL `prp-key` rule), then one INSERT per key.

    fn rule_has_key(&self) -> Result<(), ReasoningError> {
        let tg = &self.target_graph;
        for (class, props) in super::common::has_keys(self.store, self.scope().as_deref())? {
            let mut patterns = String::new();
            for (i, p) in props.iter().enumerate() {
                patterns.push_str(&format!("?x <{p}> ?v{i} . ?y <{p}> ?v{i} . "));
            }
            let q = format!(
                r#"INSERT {{ GRAPH <{tg}> {{ ?x <{OWL_SAME_AS}> ?y }} }}
                   WHERE {{
                       {{ ?x <{RDF_TYPE}> <{class}> }} UNION {{ GRAPH <{tg}> {{ ?x <{RDF_TYPE}> <{class}> }} }}
                       {{ ?y <{RDF_TYPE}> <{class}> }} UNION {{ GRAPH <{tg}> {{ ?y <{RDF_TYPE}> <{class}> }} }}
                       {patterns}
                       FILTER(?x != ?y) FILTER(isIRI(?x)) FILTER(isIRI(?y))
                   }}"#
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
