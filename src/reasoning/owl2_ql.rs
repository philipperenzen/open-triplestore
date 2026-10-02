//! OWL 2 QL profile — query rewriting over the DL-Lite_R concept and role
//! hierarchies.
//!
//! OWL 2 QL is based on DL-Lite and enables LOGSPACE query answering through
//! *query rewriting* rather than materialisation.  Instead of storing entailed
//! triples, the SPARQL query is rewritten at the AST level to include `UNION`
//! branches that account for TBox axioms:
//!
//! - `rdfs:subClassOf` / `owl:equivalentClass` between named classes
//! - `rdfs:subPropertyOf` / `owl:equivalentProperty` / `owl:inverseOf`,
//!   composed (a sub-property of an inverse is a sub-property of the inverse)
//! - `rdfs:domain`, `rdfs:range`, and unqualified existentials on the left
//!   (`[owl:onProperty P; owl:someValuesFrom owl:Thing] rdfs:subClassOf C`),
//!   through both hierarchies
//!
//! This is the part of PerfectRef that rewrites one atom at a time. It does
//! not use existentials on the right (`C ⊑ ∃P.D`): those only answer an atom
//! whose other end is an unbound, unshared variable, and a one-atom rewriter
//! cannot tell. In particular `C ⊑ ∃P.D` does **not** make `∃P ⊑ C` — a
//! subject of `P` is not thereby a `C`.
//!
//! The implementation uses the `spargebra` crate for AST-level query parsing
//! and manipulation, avoiding the fragile string-level approach.
//!
//! # Usage
//! ```no_run
//! # use open_triplestore::reasoning::owl2_ql::QLQueryRewriter;
//! # use open_triplestore::store::TripleStore;
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let store = TripleStore::in_memory()?;
//! # let sparql_str = "SELECT ?s WHERE { ?s a <http://example.org/C> }";
//! let rewriter = QLQueryRewriter::new(&store);
//! let rewritten = rewriter.rewrite_query(sparql_str)?;
//! // execute `rewritten` against the store
//! # let _ = rewritten;
//! # Ok(())
//! # }
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

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_SUB_PROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";
const OWL_EQUIV_CLASS: &str = "http://www.w3.org/2002/07/owl#equivalentClass";
const OWL_EQUIV_PROP: &str = "http://www.w3.org/2002/07/owl#equivalentProperty";
const OWL_INVERSE_OF: &str = "http://www.w3.org/2002/07/owl#inverseOf";
const OWL_SOME_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#someValuesFrom";
const OWL_ON_PROPERTY: &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";

// ─── TBox: basic concepts and roles ──────────────────────────────────────────

/// A DL-Lite role: a property, or its inverse.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Role {
    iri: String,
    inverse: bool,
}

impl Role {
    fn named(iri: impl Into<String>) -> Self {
        Self {
            iri: iri.into(),
            inverse: false,
        }
    }

    fn inv(&self) -> Self {
        Self {
            iri: self.iri.clone(),
            inverse: !self.inverse,
        }
    }
}

/// A DL-Lite basic concept: a named class, or `∃R` (the subjects of a role,
/// so `∃P⁻` is the objects of `P`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Basic {
    Class(String),
    Exists(Role),
}

/// The TBox as direct inclusions between basic concepts and between roles,
/// indexed by the super side. Queries walk them downwards: everything under
/// what an atom asks for answers it.
#[derive(Default)]
struct TBox {
    /// Named class `C` → the basic concepts declared directly under it
    /// (subclasses, `∃P` from domains, `∃P⁻` from ranges).
    concept_subs: HashMap<String, HashSet<Basic>>,
    /// Role → the roles declared directly under it. Every inclusion is stored
    /// in both polarities (`R ⊑ S` also as `R⁻ ⊑ S⁻`).
    role_subs: HashMap<Role, HashSet<Role>>,
}

impl TBox {
    fn add_concept(&mut self, sub: Basic, sup: &str) {
        if sub != Basic::Class(sup.to_string()) {
            self.concept_subs
                .entry(sup.to_string())
                .or_default()
                .insert(sub);
        }
    }

    fn add_role(&mut self, sub: Role, sup: Role) {
        if sub != sup {
            self.role_subs
                .entry(sup.inv())
                .or_default()
                .insert(sub.inv());
            self.role_subs.entry(sup).or_default().insert(sub);
        }
    }

    /// `role` and every role under it, through sub-properties, equivalences
    /// and inverses in any combination.
    fn sub_roles(&self, role: &Role) -> Vec<Role> {
        let mut seen: HashSet<Role> = HashSet::from([role.clone()]);
        let mut out = vec![role.clone()];
        let mut i = 0;
        while i < out.len() {
            if let Some(subs) = self.role_subs.get(&out[i]) {
                for s in subs {
                    if seen.insert(s.clone()) {
                        out.push(s.clone());
                    }
                }
            }
            i += 1;
        }
        out
    }

    /// `Class(class)` and every basic concept under it: subclasses, and the
    /// subjects (or objects) of every property whose domain (or range) is
    /// one of them, through the role hierarchy.
    fn sub_concepts(&self, class: &str) -> Vec<Basic> {
        let first = Basic::Class(class.to_string());
        let mut seen: HashSet<Basic> = HashSet::from([first.clone()]);
        let mut out = vec![first];
        let mut i = 0;
        while i < out.len() {
            let next: Vec<Basic> = match &out[i] {
                Basic::Class(c) => self
                    .concept_subs
                    .get(c)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default(),
                Basic::Exists(r) => self.sub_roles(r).into_iter().map(Basic::Exists).collect(),
            };
            for b in next {
                if seen.insert(b.clone()) {
                    out.push(b);
                }
            }
            i += 1;
        }
        out
    }

    /// Every named class mentioned on the super side of an inclusion.
    fn classes(&self) -> Vec<String> {
        let mut out: Vec<String> = self.concept_subs.keys().cloned().collect();
        out.sort();
        out
    }

    /// Every property mentioned in a role inclusion.
    fn properties(&self) -> Vec<String> {
        let mut out: Vec<String> = self.role_subs.keys().map(|r| r.iri.clone()).collect();
        out.sort();
        out.dedup();
        out
    }
}

/// Fresh variables for the rewritten atoms: one per atom, so two existential
/// atoms are never joined by accident, named so they cannot collide with a
/// variable of the query.
struct Fresh {
    prefix: String,
    next: usize,
}

impl Fresh {
    fn for_query(sparql: &str) -> Self {
        let mut prefix = "_ql".to_string();
        while sparql.contains(prefix.as_str()) {
            prefix.push('_');
        }
        Self { prefix, next: 0 }
    }

    fn var(&mut self) -> TermPattern {
        self.next += 1;
        TermPattern::Variable(
            spargebra::term::Variable::new(format!("{}{}", self.prefix, self.next))
                .expect("a fresh variable name is valid"),
        )
    }
}

// ─── Rewriter ────────────────────────────────────────────────────────────────

/// OWL 2 QL query rewriter.
pub struct QLQueryRewriter<'a> {
    store: &'a TripleStore,
    pub target_graph: String,
    /// When set, the TBox is read from ONLY these graphs. Without it, from
    /// the unnamed default graph.
    sources: Option<Vec<String>>,
}

impl<'a> QLQueryRewriter<'a> {
    /// Read the TBox from `sources` only. Without a scope it is read from the
    /// unnamed default graph. `POST /api/reasoning/materialize` sets this from
    /// `source_graphs` or the dataset's conformance layer, and
    /// `POST /api/reasoning/rewrite` from the graphs the caller may read.
    ///
    /// Unlike the materialising reasoners, the target graph is not added: the
    /// rewriter computes its closure in memory and never reads its own output.
    pub fn with_sources(mut self, sources: Vec<String>) -> Self {
        self.sources = Some(sources);
        self
    }

    fn scope(&self) -> Option<Vec<String>> {
        self.sources.clone()
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
            target_graph: OWL2_QL_ENTAILMENT_GRAPH.to_string(),
            sources: None,
        }
    }

    /// Rewrite a SPARQL SELECT/ASK/CONSTRUCT query over the TBox.
    ///
    /// Every answer of the rewritten query over the asserted data is an
    /// answer of the original under OWL 2 QL entailment (the rewriting is
    /// sound). It is not complete: atoms that only an existential on the
    /// right of an axiom would answer are not rewritten (see the module
    /// documentation).
    pub fn rewrite_query(&self, sparql: &str) -> Result<String, ReasoningError> {
        let tbox = self.load_tbox()?;
        let query = crate::sparql::parser()
            .parse_query(sparql)
            .map_err(|e| ReasoningError::Query(format!("SPARQL parse error: {e}")))?;
        let fresh = &mut Fresh::for_query(sparql);

        let rewritten = match query {
            Query::Select {
                dataset,
                pattern,
                base_iri,
            } => {
                let new_pattern = self.rewrite_pattern(pattern, &tbox, fresh);
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
                let new_pattern = self.rewrite_pattern(pattern, &tbox, fresh);
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
                let new_pattern = self.rewrite_pattern(pattern, &tbox, fresh);
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

        // The named-class and named-property closure: `sub ⊑ sup` for every
        // pair the hierarchies (inverses included) entail.
        let mut triples = String::new();
        let mut count = 0usize;
        for sup in tbox.classes() {
            for sub in tbox.sub_concepts(&sup) {
                if let Basic::Class(sub) = sub {
                    if sub != sup {
                        triples.push_str(&format!("<{sub}> <{RDFS_SUB_CLASS_OF}> <{sup}> .\n"));
                        count += 1;
                    }
                }
            }
        }
        for sup in tbox.properties() {
            for sub in tbox.sub_roles(&Role::named(sup.clone())) {
                if !sub.inverse && sub.iri != sup {
                    triples.push_str(&format!(
                        "<{}> <{RDFS_SUB_PROPERTY_OF}> <{sup}> .\n",
                        sub.iri
                    ));
                    count += 1;
                }
            }
        }
        if count > 0 {
            self.store.update(&format!(
                "INSERT DATA {{ GRAPH <{}> {{ {triples} }} }}",
                self.target_graph
            ))?;
        }

        Ok(ReasoningReport {
            regime: "owl2-ql".to_string(),
            triples_added: count,
            iterations: 1,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
            ignored: Vec::new(),
        })
    }

    // ─── Pattern rewriting ────────────────────────────────────────────────────

    fn rewrite_pattern(
        &self,
        pattern: GraphPattern,
        tbox: &TBox,
        fresh: &mut Fresh,
    ) -> GraphPattern {
        match pattern {
            GraphPattern::Bgp { patterns } => self.rewrite_bgp(patterns, tbox, fresh),
            GraphPattern::Join { left, right } => GraphPattern::Join {
                left: Box::new(self.rewrite_pattern(*left, tbox, fresh)),
                right: Box::new(self.rewrite_pattern(*right, tbox, fresh)),
            },
            GraphPattern::LeftJoin {
                left,
                right,
                expression,
            } => GraphPattern::LeftJoin {
                left: Box::new(self.rewrite_pattern(*left, tbox, fresh)),
                right: Box::new(self.rewrite_pattern(*right, tbox, fresh)),
                expression,
            },
            GraphPattern::Filter { expr, inner } => GraphPattern::Filter {
                expr,
                inner: Box::new(self.rewrite_pattern(*inner, tbox, fresh)),
            },
            GraphPattern::Union { left, right } => GraphPattern::Union {
                left: Box::new(self.rewrite_pattern(*left, tbox, fresh)),
                right: Box::new(self.rewrite_pattern(*right, tbox, fresh)),
            },
            GraphPattern::Graph { name, inner } => GraphPattern::Graph {
                name,
                inner: Box::new(self.rewrite_pattern(*inner, tbox, fresh)),
            },
            GraphPattern::Extend {
                inner,
                variable,
                expression,
            } => GraphPattern::Extend {
                inner: Box::new(self.rewrite_pattern(*inner, tbox, fresh)),
                variable,
                expression,
            },
            GraphPattern::Minus { left, right } => GraphPattern::Minus {
                left: Box::new(self.rewrite_pattern(*left, tbox, fresh)),
                right: Box::new(self.rewrite_pattern(*right, tbox, fresh)),
            },
            GraphPattern::OrderBy { inner, expression } => GraphPattern::OrderBy {
                inner: Box::new(self.rewrite_pattern(*inner, tbox, fresh)),
                expression,
            },
            GraphPattern::Project { inner, variables } => GraphPattern::Project {
                inner: Box::new(self.rewrite_pattern(*inner, tbox, fresh)),
                variables,
            },
            GraphPattern::Distinct { inner } => GraphPattern::Distinct {
                inner: Box::new(self.rewrite_pattern(*inner, tbox, fresh)),
            },
            GraphPattern::Reduced { inner } => GraphPattern::Reduced {
                inner: Box::new(self.rewrite_pattern(*inner, tbox, fresh)),
            },
            GraphPattern::Slice {
                inner,
                start,
                length,
            } => GraphPattern::Slice {
                inner: Box::new(self.rewrite_pattern(*inner, tbox, fresh)),
                start,
                length,
            },
            GraphPattern::Group {
                inner,
                variables,
                aggregates,
            } => GraphPattern::Group {
                inner: Box::new(self.rewrite_pattern(*inner, tbox, fresh)),
                variables,
                aggregates,
            },
            other => other,
        }
    }

    /// Rewrite a BGP: for each triple pattern, collect all rewritings and
    /// combine them via UNION. Patterns with no rewritings are kept as-is.
    fn rewrite_bgp(
        &self,
        patterns: Vec<TriplePattern>,
        tbox: &TBox,
        fresh: &mut Fresh,
    ) -> GraphPattern {
        // Each triple produces one or more alternative BGP patterns (Union).
        // We then join all the alternatives together.
        let mut result: Option<GraphPattern> = None;
        for tp in &patterns {
            let union = Self::alternatives_to_union(self.rewrite_triple(tp, tbox, fresh));
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
        let mut iter = alts
            .into_iter()
            .map(|tp| GraphPattern::Bgp { patterns: vec![tp] });
        let first = iter
            .next()
            .unwrap_or(GraphPattern::Bgp { patterns: vec![] });
        iter.fold(first, |acc, next| GraphPattern::Union {
            left: Box::new(acc),
            right: Box::new(next),
        })
    }

    /// The atom for `subject` being in `basic`: `?s a A`, `?s P ?fresh`, or
    /// `?fresh P ?s` for `∃P⁻`.
    fn concept_atom(subject: &TermPattern, basic: &Basic, fresh: &mut Fresh) -> TriplePattern {
        match basic {
            Basic::Class(c) => TriplePattern {
                subject: subject.clone(),
                predicate: NamedNodePattern::NamedNode(oxrdf::NamedNode::new_unchecked(RDF_TYPE)),
                object: TermPattern::NamedNode(oxrdf::NamedNode::new_unchecked(c)),
            },
            Basic::Exists(r) => Self::role_atom(subject, r, &fresh.var()),
        }
    }

    /// The atom for `subject R object`: `subject P object`, or
    /// `object P subject` when `R` is `P⁻`.
    fn role_atom(subject: &TermPattern, role: &Role, object: &TermPattern) -> TriplePattern {
        let (s, o) = if role.inverse {
            (object, subject)
        } else {
            (subject, object)
        };
        TriplePattern {
            subject: s.clone(),
            predicate: NamedNodePattern::NamedNode(oxrdf::NamedNode::new_unchecked(&role.iri)),
            object: o.clone(),
        }
    }

    /// Produce all rewritings of a single triple pattern under the TBox.
    fn rewrite_triple(
        &self,
        tp: &TriplePattern,
        tbox: &TBox,
        fresh: &mut Fresh,
    ) -> Vec<TriplePattern> {
        let mut results: Vec<TriplePattern> = vec![tp.clone()];

        match &tp.predicate {
            // ?s rdf:type <C> → ?s answers it if it is in any basic concept
            // under C: a subclass, the domain side of a property whose domain
            // is under C, the range side of one whose range is, each through
            // the role hierarchy.
            NamedNodePattern::NamedNode(pred) if pred.as_str() == RDF_TYPE => {
                if let TermPattern::NamedNode(class) = &tp.object {
                    for basic in tbox.sub_concepts(class.as_str()).iter().skip(1) {
                        results.push(Self::concept_atom(&tp.subject, basic, fresh));
                    }
                }
            }
            // ?s <P> ?o → ?s <Q> ?o for every Q ⊑ P, and ?o <Q> ?s for every
            // Q ⊑ P⁻ (an inverse, or a sub-property of one).
            NamedNodePattern::NamedNode(pred) => {
                for role in tbox.sub_roles(&Role::named(pred.as_str())).iter().skip(1) {
                    results.push(Self::role_atom(&tp.subject, role, &tp.object));
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

        // Class inclusions between named classes, equivalence both ways.
        for (sub, sup) in self.query_pairs(RDFS_SUB_CLASS_OF)? {
            tbox.add_concept(Basic::Class(sub), &sup);
        }
        for (a, b) in self.query_pairs(OWL_EQUIV_CLASS)? {
            tbox.add_concept(Basic::Class(a.clone()), &b);
            tbox.add_concept(Basic::Class(b), &a);
        }

        // Role inclusions. `P inverseOf Q` is `P ≡ Q⁻`.
        for (sub, sup) in self.query_pairs(RDFS_SUB_PROPERTY_OF)? {
            tbox.add_role(Role::named(sub), Role::named(sup));
        }
        for (a, b) in self.query_pairs(OWL_EQUIV_PROP)? {
            tbox.add_role(Role::named(a.clone()), Role::named(b.clone()));
            tbox.add_role(Role::named(b), Role::named(a));
        }
        for (p, q) in self.query_pairs(OWL_INVERSE_OF)? {
            tbox.add_role(Role::named(p.clone()), Role::named(q.clone()).inv());
            tbox.add_role(Role::named(q).inv(), Role::named(p));
        }

        // Domains and ranges: `P rdfs:domain C` is ∃P ⊑ C, `P rdfs:range C`
        // is ∃P⁻ ⊑ C.
        for (prop, cls) in self.query_pairs(RDFS_DOMAIN)? {
            tbox.add_concept(Basic::Exists(Role::named(prop)), &cls);
        }
        for (prop, cls) in self.query_pairs(RDFS_RANGE)? {
            tbox.add_concept(Basic::Exists(Role::named(prop).inv()), &cls);
        }

        // An unqualified existential on the left says the same as a domain
        // (or, on `[owl:inverseOf P]`, a range):
        // `[owl:onProperty P; owl:someValuesFrom owl:Thing] rdfs:subClassOf C`
        // is ∃P ⊑ C, and an equivalence includes it. A restriction on the
        // *right* (`C ⊑ ∃P.D`) says nothing of the kind and is not read.
        let q = format!(
            "SELECT ?prop ?inv ?class WHERE {{ \
               ?restr <{OWL_SOME_VALUES_FROM}> <{OWL_THING}> . \
               {{ ?restr <{RDFS_SUB_CLASS_OF}> ?class }} \
               UNION {{ ?restr <{OWL_EQUIV_CLASS}> ?class }} \
               UNION {{ ?class <{OWL_EQUIV_CLASS}> ?restr }} \
               {{ ?restr <{OWL_ON_PROPERTY}> ?prop }} \
               UNION {{ ?restr <{OWL_ON_PROPERTY}> ?on . ?on <{OWL_INVERSE_OF}> ?prop . \
                        FILTER(isBlank(?on)) BIND(true AS ?inv) }} \
               FILTER(isIRI(?prop) && isIRI(?class)) \
             }}"
        );
        if let oxigraph::sparql::QueryResults::Solutions(sols) = self.run_query(&q)? {
            for sol in sols.flatten() {
                let iri = |v: &str| match sol.get(v) {
                    Some(oxigraph::model::Term::NamedNode(nn)) => Some(nn.as_str().to_string()),
                    _ => None,
                };
                if let (Some(p), Some(c)) = (iri("prop"), iri("class")) {
                    let role = Role::named(p);
                    let role = if sol.get("inv").is_some() {
                        role.inv()
                    } else {
                        role
                    };
                    tbox.add_concept(Basic::Exists(role), &c);
                }
            }
        }

        Ok(tbox)
    }

    fn query_pairs(&self, predicate: &str) -> Result<Vec<(String, String)>, ReasoningError> {
        let q = format!(
            "SELECT ?s ?o WHERE {{ ?s <{predicate}> ?o . FILTER(isIRI(?s) && isIRI(?o)) }}"
        );
        let mut pairs = Vec::new();
        if let oxigraph::sparql::QueryResults::Solutions(sols) = self.run_query(&q)? {
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
