//! OWL 2 QL profile — query rewriting via the PerfectRef algorithm.
//!
//! OWL 2 QL is based on DL-Lite and enables LOGSPACE query answering through
//! *query rewriting* rather than materialisation.  Instead of storing entailed
//! triples, the SPARQL query is rewritten at the AST level to include `UNION`
//! branches that account for TBox axioms:
//!
//! - `rdfs:subClassOf` / `owl:equivalentClass`
//! - `rdfs:subPropertyOf` / `owl:equivalentProperty`
//! - `owl:inverseOf`
//! - `owl:someValuesFrom` existential restrictions
//!
//! The implementation uses the `spargebra` crate for AST-level query parsing
//! and manipulation, avoiding the fragile string-level approach.
//!
//! # Usage
//! ```rust,ignore
//! let rewriter = QLQueryRewriter::new(&store);
//! let rewritten = rewriter.rewrite_query(sparql_str)?;
//! // execute `rewritten` against the store
//! ```

use spargebra::algebra::GraphPattern;
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern};
use spargebra::Query;

use super::common::{ReasoningError, ReasoningReport, OWL2_QL_ENTAILMENT_GRAPH};
use crate::store::TripleStore;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use tracing::debug;

// ─── Namespace constants ──────────────────────────────────────────────────────

const RDF_TYPE:             &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUB_CLASS_OF:    &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_SUB_PROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";
const OWL_EQUIV_CLASS:      &str = "http://www.w3.org/2002/07/owl#equivalentClass";
const OWL_EQUIV_PROP:       &str = "http://www.w3.org/2002/07/owl#equivalentProperty";
const OWL_INVERSE_OF:       &str = "http://www.w3.org/2002/07/owl#inverseOf";
const OWL_SOME_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#someValuesFrom";
const OWL_ON_PROPERTY:      &str = "http://www.w3.org/2002/07/owl#onProperty";

// ─── TBox snapshot with transitive closure ───────────────────────────────────

/// Lightweight in-memory TBox with full transitive closure.
#[derive(Default)]
struct TBox {
    /// Subclasses: class IRI → set of all subclass IRIs (incl. equivalents).
    /// Used for query rewriting: "?x type C" expands to include all subclasses.
    subclasses: HashMap<String, HashSet<String>>,
    /// Subproperties: property IRI → set of all subproperty IRIs (incl. equivalents).
    /// Used for query rewriting: "?x P ?y" expands to include all subproperties.
    subproperties: HashMap<String, HashSet<String>>,
    /// Inverse pairs: property IRI → set of inverse property IRIs
    inverses: HashMap<String, HashSet<String>>,
    /// Existential domain: property IRI → set of class IRIs (∃P.⊤ ⊑ C)
    existential_domain: HashMap<String, HashSet<String>>,
}

impl TBox {
    /// Compute transitive closure of a relation stored as Vec<(sub, sup)> pairs.
    fn transitive_closure(pairs: Vec<(String, String)>) -> HashMap<String, HashSet<String>> {
        let mut map: HashMap<String, HashSet<String>> = HashMap::new();
        for (sub, sup) in &pairs {
            map.entry(sub.clone()).or_default().insert(sup.clone());
        }
        // Fixed-point iteration
        let mut changed = true;
        while changed {
            changed = false;
            let keys: Vec<String> = map.keys().cloned().collect();
            for k in keys {
                let supers: Vec<String> = map[&k].iter().cloned().collect();
                for sup in supers {
                    if let Some(grand) = map.get(&sup).cloned() {
                        let entry = map.entry(k.clone()).or_default();
                        for g in grand {
                            if entry.insert(g) {
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
        map
    }
}

// ─── Rewriter ────────────────────────────────────────────────────────────────

/// OWL 2 QL query rewriter.
pub struct QLQueryRewriter<'a> {
    store: &'a TripleStore,
    pub target_graph: String,
}

impl<'a> QLQueryRewriter<'a> {
    pub fn new(store: &'a TripleStore) -> Self {
        Self {
            store,
            target_graph: OWL2_QL_ENTAILMENT_GRAPH.to_string(),
        }
    }

    /// Rewrite a SPARQL SELECT/ASK/CONSTRUCT query using the PerfectRef algorithm.
    ///
    /// The rewritten query is semantically equivalent to the original under the
    /// OWL 2 QL entailment regime: every answer to the original query over the
    /// entailed dataset is also an answer to the rewritten query over the
    /// asserted dataset alone.
    pub fn rewrite_query(&self, sparql: &str) -> Result<String, ReasoningError> {
        let tbox = self.load_tbox()?;
        let query = Query::parse(sparql, None)
            .map_err(|e| ReasoningError::Query(format!("SPARQL parse error: {e}")))?;

        let rewritten = match query {
            Query::Select {
                dataset,
                pattern,
                base_iri,
            } => {
                let new_pattern = self.rewrite_pattern(pattern, &tbox);
                Query::Select {
                    dataset,
                    pattern: new_pattern,
                    base_iri,
                }
            }
            Query::Ask {
                dataset,
                pattern,
                base_iri,
            } => {
                let new_pattern = self.rewrite_pattern(pattern, &tbox);
                Query::Ask {
                    dataset,
                    pattern: new_pattern,
                    base_iri,
                }
            }
            Query::Construct {
                template,
                dataset,
                pattern,
                base_iri,
            } => {
                let new_pattern = self.rewrite_pattern(pattern, &tbox);
                Query::Construct {
                    template,
                    dataset,
                    pattern: new_pattern,
                    base_iri,
                }
            }
            other => other,
        };

        let result = rewritten.to_string();
        debug!("OWL-QL rewritten query ({} chars)", result.len());
        Ok(result)
    }

    /// Materialize TBox closure triples into the target graph for inspection.
    pub fn materialize_tbox(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        let tbox = self.load_tbox()?;

        let mut count = 0usize;
        // subclasses map: sup → {subs}; write sub rdfs:subClassOf sup
        for (sup, subs) in &tbox.subclasses {
            for sub in subs {
                if sub != sup {
                    let q = format!(
                        "INSERT {{ GRAPH <{}> {{ <{sub}> <{RDFS_SUB_CLASS_OF}> <{sup}> }} }} WHERE {{}}",
                        self.target_graph
                    );
                    self.store.update(&q)?;
                    count += 1;
                }
            }
        }
        for (sup, subs) in &tbox.subproperties {
            for sub in subs {
                if sub != sup {
                    let q = format!(
                        "INSERT {{ GRAPH <{}> {{ <{sub}> <{RDFS_SUB_PROPERTY_OF}> <{sup}> }} }} WHERE {{}}",
                        self.target_graph
                    );
                    self.store.update(&q)?;
                    count += 1;
                }
            }
        }

        Ok(ReasoningReport {
            regime: "owl2-ql".to_string(),
            triples_added: count,
            iterations: 1,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
        })
    }

    // ─── Pattern rewriting ────────────────────────────────────────────────────

    fn rewrite_pattern(&self, pattern: GraphPattern, tbox: &TBox) -> GraphPattern {
        match pattern {
            GraphPattern::Bgp { patterns } => self.rewrite_bgp(patterns, tbox),
            GraphPattern::Join { left, right } => GraphPattern::Join {
                left: Box::new(self.rewrite_pattern(*left, tbox)),
                right: Box::new(self.rewrite_pattern(*right, tbox)),
            },
            GraphPattern::LeftJoin {
                left,
                right,
                expression,
            } => GraphPattern::LeftJoin {
                left: Box::new(self.rewrite_pattern(*left, tbox)),
                right: Box::new(self.rewrite_pattern(*right, tbox)),
                expression,
            },
            GraphPattern::Filter { expr, inner } => GraphPattern::Filter {
                expr,
                inner: Box::new(self.rewrite_pattern(*inner, tbox)),
            },
            GraphPattern::Union { left, right } => GraphPattern::Union {
                left: Box::new(self.rewrite_pattern(*left, tbox)),
                right: Box::new(self.rewrite_pattern(*right, tbox)),
            },
            GraphPattern::Graph { name, inner } => GraphPattern::Graph {
                name,
                inner: Box::new(self.rewrite_pattern(*inner, tbox)),
            },
            GraphPattern::Extend {
                inner,
                variable,
                expression,
            } => GraphPattern::Extend {
                inner: Box::new(self.rewrite_pattern(*inner, tbox)),
                variable,
                expression,
            },
            GraphPattern::Minus { left, right } => GraphPattern::Minus {
                left: Box::new(self.rewrite_pattern(*left, tbox)),
                right: Box::new(self.rewrite_pattern(*right, tbox)),
            },
            GraphPattern::OrderBy { inner, expression } => GraphPattern::OrderBy {
                inner: Box::new(self.rewrite_pattern(*inner, tbox)),
                expression,
            },
            GraphPattern::Project { inner, variables } => GraphPattern::Project {
                inner: Box::new(self.rewrite_pattern(*inner, tbox)),
                variables,
            },
            GraphPattern::Distinct { inner } => GraphPattern::Distinct {
                inner: Box::new(self.rewrite_pattern(*inner, tbox)),
            },
            GraphPattern::Reduced { inner } => GraphPattern::Reduced {
                inner: Box::new(self.rewrite_pattern(*inner, tbox)),
            },
            GraphPattern::Slice {
                inner,
                start,
                length,
            } => GraphPattern::Slice {
                inner: Box::new(self.rewrite_pattern(*inner, tbox)),
                start,
                length,
            },
            GraphPattern::Group {
                inner,
                variables,
                aggregates,
            } => GraphPattern::Group {
                inner: Box::new(self.rewrite_pattern(*inner, tbox)),
                variables,
                aggregates,
            },
            other => other,
        }
    }

    /// Rewrite a BGP: for each triple pattern, collect all rewritings and
    /// combine them via UNION. Patterns with no rewritings are kept as-is.
    fn rewrite_bgp(&self, patterns: Vec<TriplePattern>, tbox: &TBox) -> GraphPattern {
        // Each triple produces one or more alternative BGP patterns (Union).
        // We then join all the alternatives together.
        let mut alternatives_per_triple: Vec<Vec<TriplePattern>> = Vec::new();

        for tp in &patterns {
            let rewrites = self.rewrite_triple(tp, tbox);
            alternatives_per_triple.push(rewrites);
        }

        // Build the combined pattern as a Join of Unions.
        // Start with the first triple's alternatives, then join with each subsequent.
        let mut result: Option<GraphPattern> = None;

        for alts in alternatives_per_triple {
            let union = Self::alternatives_to_union(alts);
            result = Some(match result {
                None => union,
                Some(prev) => GraphPattern::Join {
                    left: Box::new(prev),
                    right: Box::new(union),
                },
            });
        }

        result.unwrap_or(GraphPattern::Bgp { patterns: vec![] })
    }

    /// Turn a list of alternative triple patterns into a union tree.
    fn alternatives_to_union(alts: Vec<TriplePattern>) -> GraphPattern {
        let mut iter = alts.into_iter().map(|tp| GraphPattern::Bgp {
            patterns: vec![tp],
        });
        let first = iter.next().unwrap_or(GraphPattern::Bgp { patterns: vec![] });
        iter.fold(first, |acc, next| GraphPattern::Union {
            left: Box::new(acc),
            right: Box::new(next),
        })
    }

    /// Produce all rewritings of a single triple pattern under the TBox.
    fn rewrite_triple(&self, tp: &TriplePattern, tbox: &TBox) -> Vec<TriplePattern> {
        let mut results: Vec<TriplePattern> = vec![tp.clone()];

        match &tp.predicate {
            // ?s rdf:type <C> → also ?s rdf:type <C'> for each C' ⊑* C (subclass of C)
            // because any individual of type C' satisfies the query for C.
            NamedNodePattern::NamedNode(pred) if pred.as_str() == RDF_TYPE => {
                if let TermPattern::NamedNode(class) = &tp.object {
                    let class_iri = class.as_str();
                    // Add alternatives for each subclass of the queried class
                    if let Some(subs) = tbox.subclasses.get(class_iri) {
                        for sub in subs {
                            if sub != class_iri {
                                if let Ok(sub_node) = oxrdf::NamedNode::new(sub) {
                                    results.push(TriplePattern {
                                        subject: tp.subject.clone(),
                                        predicate: tp.predicate.clone(),
                                        object: TermPattern::NamedNode(sub_node),
                                    });
                                }
                            }
                        }
                    }
                    // Add alternatives for existential domain: ∃P.⊤ ⊑ C → ?s ?P ?any
                    for (prop_iri, domains) in &tbox.existential_domain {
                        if domains.contains(class_iri) {
                            if let Ok(prop) = oxrdf::NamedNode::new(prop_iri) {
                                let fresh_var = spargebra::term::Variable::new("_ql_any")
                                    .unwrap_or_else(|_| spargebra::term::Variable::new("ql_any").unwrap());
                                results.push(TriplePattern {
                                    subject: tp.subject.clone(),
                                    predicate: NamedNodePattern::NamedNode(prop),
                                    object: TermPattern::Variable(fresh_var),
                                });
                            }
                        }
                    }
                }
            }
            // ?s <P> ?o → also ?s <P'> ?o for each subproperty P' ⊑* P, plus inverses
            // because any triple with P' satisfies the query for P.
            NamedNodePattern::NamedNode(pred) => {
                let pred_iri = pred.as_str().to_string();
                // Subproperty rewritings
                if let Some(subs) = tbox.subproperties.get(&pred_iri) {
                    for sub in subs {
                        if sub != &pred_iri {
                            if let Ok(sub_node) = oxrdf::NamedNode::new(sub) {
                                results.push(TriplePattern {
                                    subject: tp.subject.clone(),
                                    predicate: NamedNodePattern::NamedNode(sub_node),
                                    object: tp.object.clone(),
                                });
                            }
                        }
                    }
                }
                // Inverse rewritings: ?s <P> ?o → ?o <Q> ?s  (where Q inverseOf P)
                if let Some(invs) = tbox.inverses.get(&pred_iri) {
                    for inv in invs {
                        if let Ok(inv_node) = oxrdf::NamedNode::new(inv) {
                            results.push(TriplePattern {
                                subject: tp.object.clone(),
                                predicate: NamedNodePattern::NamedNode(inv_node),
                                object: tp.subject.clone(),
                            });
                        }
                    }
                }
            }
            // Variable predicate — no static rewriting possible
            NamedNodePattern::Variable(_) => {}
        }

        // Deduplicate (naive but correct)
        let mut seen = HashSet::new();
        results.retain(|tp: &TriplePattern| seen.insert(tp.to_string()));
        results
    }

    // ─── TBox loading ─────────────────────────────────────────────────────────

    fn load_tbox(&self) -> Result<TBox, ReasoningError> {
        let mut tbox = TBox::default();

        // Collect raw subClassOf pairs (sub → super), including equivalentClass both ways.
        // For query rewriting we need the inverse: super → {subs} (subclasses map).
        let mut subclass_pairs = self.query_pairs(RDFS_SUB_CLASS_OF)?;
        let equiv_class = self.query_pairs(OWL_EQUIV_CLASS)?;
        for (a, b) in equiv_class {
            subclass_pairs.push((a.clone(), b.clone()));
            subclass_pairs.push((b, a));
        }
        // Invert: (sub, sup) → (sup, sub) so transitive_closure gives super → {subs}
        let inverted_class: Vec<(String, String)> = subclass_pairs
            .iter()
            .map(|(sub, sup)| (sup.clone(), sub.clone()))
            .collect();
        tbox.subclasses = TBox::transitive_closure(inverted_class);

        // Collect raw subPropertyOf pairs, similarly inverted.
        let mut subprop_pairs = self.query_pairs(RDFS_SUB_PROPERTY_OF)?;
        let equiv_prop = self.query_pairs(OWL_EQUIV_PROP)?;
        for (a, b) in equiv_prop {
            subprop_pairs.push((a.clone(), b.clone()));
            subprop_pairs.push((b, a));
        }
        let inverted_prop: Vec<(String, String)> = subprop_pairs
            .iter()
            .map(|(sub, sup)| (sup.clone(), sub.clone()))
            .collect();
        tbox.subproperties = TBox::transitive_closure(inverted_prop);

        // Inverse properties (symmetric: if P inverseOf Q then Q inverseOf P)
        let inv_pairs = self.query_pairs(OWL_INVERSE_OF)?;
        for (p, q) in &inv_pairs {
            tbox.inverses.entry(p.clone()).or_default().insert(q.clone());
            tbox.inverses.entry(q.clone()).or_default().insert(p.clone());
        }

        // Existential domain: ∃P.⊤ ⊑ C, collected from two axiom patterns:
        //
        // 1. rdfs:domain — "P rdfs:domain C" is equivalent to ∃P.⊤ ⊑ C in OWL 2 QL.
        let domain_pairs = self.query_pairs("http://www.w3.org/2000/01/rdf-schema#domain")?;
        for (prop, cls) in domain_pairs {
            tbox.existential_domain.entry(prop).or_default().insert(cls);
        }

        // 2. OWL existential restrictions — "?restr owl:someValuesFrom ?filler .
        //    ?restr owl:onProperty ?prop . ?class rdfs:subClassOf ?restr"
        //    means the class is in the existential domain of the property.
        let q = format!(
            "SELECT ?prop ?class WHERE {{ \
               ?restr <{OWL_SOME_VALUES_FROM}> ?filler . \
               ?restr <{OWL_ON_PROPERTY}> ?prop . \
               ?class <{RDFS_SUB_CLASS_OF}> ?restr . \
               FILTER(isIRI(?prop) && isIRI(?class)) \
             }}"
        );
        match self.store.query(&q)? {
            oxigraph::sparql::QueryResults::Solutions(sols) => {
                for sol in sols.flatten() {
                    let prop = sol.get("prop").and_then(|v| {
                        if let oxigraph::model::Term::NamedNode(nn) = v {
                            Some(nn.as_str().to_string())
                        } else {
                            None
                        }
                    });
                    let cls = sol.get("class").and_then(|v| {
                        if let oxigraph::model::Term::NamedNode(nn) = v {
                            Some(nn.as_str().to_string())
                        } else {
                            None
                        }
                    });
                    if let (Some(p), Some(c)) = (prop, cls) {
                        tbox.existential_domain.entry(p).or_default().insert(c);
                    }
                }
            }
            _ => {}
        }

        Ok(tbox)
    }

    fn query_pairs(&self, predicate: &str) -> Result<Vec<(String, String)>, ReasoningError> {
        let q = format!(
            "SELECT ?s ?o WHERE {{ ?s <{predicate}> ?o . FILTER(isIRI(?s) && isIRI(?o)) }}"
        );
        let mut pairs = Vec::new();
        match self.store.query(&q)? {
            oxigraph::sparql::QueryResults::Solutions(sols) => {
                for sol in sols.flatten() {
                    let s = sol.get("s").and_then(|v| {
                        if let oxigraph::model::Term::NamedNode(nn) = v {
                            Some(nn.as_str().to_string())
                        } else {
                            None
                        }
                    });
                    let o = sol.get("o").and_then(|v| {
                        if let oxigraph::model::Term::NamedNode(nn) = v {
                            Some(nn.as_str().to_string())
                        } else {
                            None
                        }
                    });
                    if let (Some(s), Some(o)) = (s, o) {
                        if s != o {
                            pairs.push((s, o));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(pairs)
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

    fn ask_rewritten(store: &TripleStore, sparql: &str) -> bool {
        let rw = QLQueryRewriter::new(store);
        let rewritten = rw.rewrite_query(sparql).unwrap();
        match store.query(&rewritten).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => false,
        }
    }

    #[test]
    fn test_ql_rewriter_loads() {
        let s = store_with("ex:Prof rdfs:subClassOf ex:Staff .");
        let rw = QLQueryRewriter::new(&s);
        rw.materialize_tbox().unwrap();
    }

    #[test]
    fn test_ql_subclass_rewriting() {
        // Store asserts alice is a Prof; TBox says Prof ⊑ Staff
        // Query asks if alice is Staff → should succeed via rewriting
        let s = store_with(
            "ex:Prof rdfs:subClassOf ex:Staff .
             ex:alice rdf:type ex:Prof .",
        );
        assert!(ask_rewritten(
            &s,
            "ASK { <http://example.org/alice> \
             <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://example.org/Staff> }"
        ));
    }

    #[test]
    fn test_ql_equivalent_class_rewriting() {
        let s = store_with(
            "ex:Faculty owl:equivalentClass ex:AcademicStaff .
             ex:alice rdf:type ex:Faculty .",
        );
        assert!(ask_rewritten(
            &s,
            "ASK { <http://example.org/alice> \
             <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://example.org/AcademicStaff> }"
        ));
    }

    #[test]
    fn test_ql_inverse_rewriting() {
        let s = store_with(
            "ex:teaches owl:inverseOf ex:taughtBy .
             ex:bob ex:teaches ex:cs101 .",
        );
        assert!(ask_rewritten(
            &s,
            "ASK { <http://example.org/cs101> \
             <http://example.org/taughtBy> \
             <http://example.org/bob> }"
        ));
    }

    #[test]
    fn test_ql_subproperty_rewriting() {
        let s = store_with(
            "ex:fatherOf rdfs:subPropertyOf ex:parentOf .
             ex:bob ex:fatherOf ex:alice .",
        );
        assert!(ask_rewritten(
            &s,
            "ASK { <http://example.org/bob> \
             <http://example.org/parentOf> \
             <http://example.org/alice> }"
        ));
    }

    #[test]
    fn test_ql_transitive_subclass_rewriting() {
        // Three-level hierarchy: PhD ⊑ Student ⊑ Person
        // Query asks if alice (PhD) is a Person → needs transitive closure
        let s = store_with(
            "ex:PhD rdfs:subClassOf ex:Student .
             ex:Student rdfs:subClassOf ex:Person .
             ex:alice rdf:type ex:PhD .",
        );
        assert!(ask_rewritten(
            &s,
            "ASK { <http://example.org/alice> \
             <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://example.org/Person> }"
        ));
    }

    #[test]
    fn test_ql_materialize_tbox() {
        let s = store_with(
            "ex:Prof rdfs:subClassOf ex:Staff .
             ex:Staff rdfs:subClassOf ex:Employee .",
        );
        let rw = QLQueryRewriter::new(&s);
        let report = rw.materialize_tbox().unwrap();
        // Should have Prof→Staff, Prof→Employee, Staff→Employee
        assert!(report.triples_added >= 2);
    }
}
