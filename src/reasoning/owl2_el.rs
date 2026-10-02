//! OWL 2 EL profile — a native EL++ classifier.
//!
//! OWL 2 EL is the profile of large biomedical ontologies (SNOMED CT, the
//! Gene Ontology). This module reads the ontology straight out of the quad
//! index, normalizes it ([`load`]), saturates it with the EL++ completion
//! rules ([`saturate`]) and writes what follows into the target graph:
//!
//! - **classification** — `rdfs:subClassOf` from every class IRI to each
//!   class IRI and EL class expression that subsumes it, `owl:equivalentClass`
//!   between equivalent class IRIs, `C rdfs:subClassOf owl:Nothing` for an
//!   unsatisfiable class, and `owl:Thing rdfs:subClassOf C` when `C` is
//!   equivalent to `owl:Thing`;
//! - the **property hierarchy** — `rdfs:subPropertyOf` (and
//!   `owl:equivalentProperty`) closed under inclusion;
//! - **realization** — `rdf:type` from every individual to every class IRI
//!   it belongs to (`owl:Nothing` when it belongs to none consistently);
//! - the **property-assertion closure** between individuals (and literals):
//!   through sub-properties, chains of any length, transitivity and
//!   reflexivity;
//! - `owl:sameAs` between individuals that agree on an `owl:hasKey`.
//!
//! Only triples not already in a premise graph are written, through
//! [`TripleStore::insert_quads`] so the graph index and change capture see
//! them. The rules read the source graphs (or, unscoped, the unnamed default
//! graph) plus the target graph itself.
//!
//! # Supported constructs
//!
//! Class expressions: class IRIs, `owl:Thing`, `owl:Nothing`,
//! `owl:intersectionOf` (any arity), `owl:someValuesFrom` over an object
//! property. Axioms: `rdfs:subClassOf`, `owl:equivalentClass`,
//! `owl:disjointWith`, `owl:AllDisjointClasses`, `rdfs:subPropertyOf`,
//! `owl:equivalentProperty`, `owl:propertyChainAxiom`,
//! `owl:TransitiveProperty`, `owl:ReflexiveProperty`, `rdfs:domain`,
//! `rdfs:range` (object properties), `owl:hasKey`, class and property
//! assertions. What the loader cannot use is counted per construct in
//! [`ReasoningReport::ignored`]; leaving an axiom out never adds a wrong
//! consequence, it only loses some.
//!
//! # Consistency
//!
//! An EL ontology is inconsistent when `owl:Thing ⊑ owl:Nothing` or an
//! individual is an instance of `owl:Nothing` (directly, through its
//! classes and successors, or by being typed with two disjoint classes). An
//! unsatisfiable *class* without instances is not an inconsistency;
//! [`El2Classifier::unsatisfiable_classes`] lists those.
//! [`El2Classifier::classify`] fails with [`ReasoningError::Inconsistency`]
//! after writing what it derived.

mod load;
mod saturate;

use std::time::Instant;

use oxigraph::model::{GraphName, NamedNode, NamedOrBlankNode, Quad, Term};
use tracing::info;

use super::common::{IgnoredAxioms, ReasoningError, ReasoningReport, OWL2_EL_ENTAILMENT_GRAPH};
use crate::store::TripleStore;
use load::{Kind, Loaded, Tid, OWL, RDFS};
use saturate::{Cid, FxMap, FxSet, Saturation, BOTTOM, TOP};

/// OWL 2 EL++ classifier.
pub struct El2Classifier<'a> {
    store: &'a TripleStore,
    target_graph: String,
    /// When set, the rules read ONLY these graphs (plus the target graph).
    /// Without it they read the unnamed default graph (plus the target).
    sources: Option<Vec<String>>,
    /// Check consistency after saturation and fail with
    /// [`ReasoningError::Inconsistency`] when the ontology is inconsistent.
    pub detect_inconsistency: bool,
}

/// Why an ontology is inconsistent: the rule id and a sentence.
struct Clash {
    #[allow(dead_code)] // the rule id goes into `Inconsistency { rule, .. }` once that lands
    rule: &'static str,
    detail: String,
}

/// A saturated ontology.
struct Saturated {
    loaded: Loaded,
    sat: Saturation,
}

impl<'a> El2Classifier<'a> {
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

    /// Restrict the rules to `sources` (plus the target graph). Without a
    /// scope the rules read the unnamed default graph only, so a dataset's
    /// named graphs — and the model version it conforms to — were invisible to
    /// materialisation; this is what `POST /api/reasoning/materialize` sets
    /// from `source_graphs` or the dataset's conformance layer.
    pub fn with_sources(mut self, sources: Vec<String>) -> Self {
        self.sources = Some(sources);
        self
    }

    /// The graphs the rules read: the sources (or the default graph) and the
    /// target graph.
    fn premise_graphs(&self) -> Vec<Option<String>> {
        let mut graphs: Vec<Option<String>> = match &self.sources {
            Some(s) => s.iter().cloned().map(Some).collect(),
            None => vec![None],
        };
        let target = Some(self.target_graph.clone());
        if !graphs.contains(&target) {
            graphs.push(target);
        }
        graphs
    }

    fn saturate(&self) -> Result<Saturated, ReasoningError> {
        let mut loaded = load::load(self.store, &self.premise_graphs())?;
        let mut sat = Saturation::new(std::mem::take(&mut loaded.ax));
        sat.init(TOP);
        for &c in &loaded.classes {
            sat.init(c);
        }
        for &x in &loaded.individuals {
            sat.init(x);
        }
        for &(x, r, y) in &loaded.edges {
            sat.add_link(x, r, y);
        }
        sat.run();
        Ok(Saturated { loaded, sat })
    }

    /// Classify the ontology, write the consequences into the target graph
    /// and return a report.
    ///
    /// Fails with [`ReasoningError::Inconsistency`] when the ontology is
    /// inconsistent and `detect_inconsistency` is set; what was derived stays
    /// in the target graph.
    pub fn classify(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        info!("OWL 2 EL classification → <{}>", self.target_graph);
        let Saturated { mut loaded, sat } = self.saturate()?;
        let derived = derived_triples(&mut loaded, &sat);
        let graph = GraphName::NamedNode(
            NamedNode::new(self.target_graph.as_str())
                .map_err(|e| ReasoningError::Store(format!("target graph: {e}")))?,
        );
        let quads: Vec<Quad> = derived
            .into_iter()
            .filter_map(|(s, p, o)| {
                let subject = match loaded.term(s) {
                    Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n.clone()),
                    Term::BlankNode(b) => NamedOrBlankNode::BlankNode(b.clone()),
                    _ => return None,
                };
                let Term::NamedNode(predicate) = loaded.term(p) else {
                    return None;
                };
                Some(Quad::new(
                    subject,
                    predicate.clone(),
                    loaded.term(o).clone(),
                    graph.clone(),
                ))
            })
            .collect();
        let added = quads.len();
        if !quads.is_empty() {
            self.store.insert_quads(quads)?;
        }
        info!(
            "EL classification complete: +{} triples, {} contexts, {} rule steps ({} ms)",
            added,
            sat.contexts().count(),
            sat.steps,
            start.elapsed().as_millis()
        );
        if self.detect_inconsistency {
            if let Some(clash) = inconsistency(&loaded, &sat) {
                return Err(ReasoningError::Inconsistency(clash.detail));
            }
        }
        Ok(ReasoningReport {
            regime: "owl2-el".to_string(),
            triples_added: added,
            iterations: 1,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
            ignored: loaded
                .ignored
                .iter()
                .map(|(construct, (count, example))| IgnoredAxioms {
                    construct: construct.to_string(),
                    count: *count,
                    example: example.clone(),
                })
                .collect(),
        })
    }

    /// Whether the ontology is consistent: no individual is an instance of
    /// `owl:Nothing` or of two disjoint classes, and `owl:Thing ⊑ owl:Nothing`
    /// does not hold. Saturates afresh; writes nothing.
    ///
    /// An unsatisfiable class (`C ⊑ owl:Nothing`) is *not* an inconsistency
    /// on its own — see [`unsatisfiable_classes`](Self::unsatisfiable_classes).
    pub fn check_consistency(&self) -> Result<bool, ReasoningError> {
        let s = self.saturate()?;
        Ok(inconsistency(&s.loaded, &s.sat).is_none())
    }

    /// The class IRIs other than `owl:Nothing` that are subclasses of
    /// `owl:Nothing`, sorted. Saturates afresh; writes nothing.
    pub fn unsatisfiable_classes(&self) -> Result<Vec<String>, ReasoningError> {
        let s = self.saturate()?;
        let mut out: Vec<String> = s
            .loaded
            .classes
            .iter()
            .filter(|&&c| s.sat.subsumes(c, BOTTOM))
            .filter_map(|&c| match s.loaded.kind(c) {
                Kind::Class(t) => match s.loaded.term(t) {
                    Term::NamedNode(n) => Some(n.as_str().to_string()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }
}

/// The first reason the saturated ontology is inconsistent, if any.
fn inconsistency(loaded: &Loaded, sat: &Saturation) -> Option<Clash> {
    if sat.subsumes(TOP, BOTTOM) {
        return Some(Clash {
            rule: "el-top-bottom",
            detail: "owl:Thing is a subclass of owl:Nothing".to_string(),
        });
    }
    for &x in &loaded.individuals {
        let Some(ctx) = sat.context(x) else { continue };
        if !ctx.subs.contains(&BOTTOM) {
            continue;
        }
        let name = match loaded.kind(x) {
            Kind::Individual(t) => loaded.term(t).to_string(),
            _ => "an individual".to_string(),
        };
        let conj = &sat.axioms().conj;
        let disjoint = ctx.subs.iter().any(|a| {
            conj.get(a).is_some_and(|entries| {
                entries
                    .iter()
                    .any(|&(a2, b)| b == BOTTOM && ctx.subs.contains(&a2))
            })
        });
        return Some(if disjoint {
            Clash {
                rule: "cax-dw",
                detail: format!("{name} is an instance of two disjoint classes"),
            }
        } else {
            Clash {
                rule: "cls-nothing2",
                detail: format!("{name} is an instance of owl:Nothing"),
            }
        });
    }
    None
}

/// Every entailed triple of the output vocabulary not already a premise.
fn derived_triples(loaded: &mut Loaded, sat: &Saturation) -> Vec<(Tid, Tid, Tid)> {
    let v = loaded.vocab;
    let equivalent_class = loaded.iri(&format!("{OWL}equivalentClass"));
    let sub_property_of = loaded.iri(&format!("{RDFS}subPropertyOf"));
    let equivalent_property = loaded.iri(&format!("{OWL}equivalentProperty"));
    let loaded = &*loaded;
    let mut out: Vec<(Tid, Tid, Tid)> = Vec::new();
    let mut seen: FxSet<(Tid, Tid, Tid)> = FxSet::default();
    let mut emit = |t: (Tid, Tid, Tid)| {
        if !loaded.premises.contains(&t) && seen.insert(t) {
            out.push(t);
        }
    };

    // Classification.
    for &c in &loaded.classes {
        let (Kind::Class(ct), Some(ctx)) = (loaded.kind(c), sat.context(c)) else {
            continue;
        };
        let unsat = ctx.subs.contains(&BOTTOM);
        for &a in &ctx.subs {
            if a == c || a == TOP {
                continue;
            }
            if a == BOTTOM {
                emit((ct, v.sub_class_of, v.nothing));
                continue;
            }
            match loaded.kind(a) {
                Kind::Class(at) => {
                    emit((ct, v.sub_class_of, at));
                    if !unsat && sat.subsumes(a, c) {
                        emit((ct, equivalent_class, at));
                    }
                }
                Kind::Expr(at) if loaded.defined_exprs.contains(&a) => {
                    emit((ct, v.sub_class_of, at));
                }
                _ => {}
            }
        }
    }
    if let Some(ctx) = sat.context(TOP) {
        for &a in &ctx.subs {
            match loaded.kind(a) {
                Kind::Class(at) => emit((v.thing, v.sub_class_of, at)),
                _ if a == BOTTOM => emit((v.thing, v.sub_class_of, v.nothing)),
                _ => {}
            }
        }
    }

    // The property hierarchy.
    for (r, rt) in loaded.role_terms.iter().enumerate() {
        let Some(rt) = *rt else { continue };
        for &s in sat.sup_star(r as u32) {
            if s as usize == r {
                continue;
            }
            let Some(Some(st)) = loaded.role_terms.get(s as usize) else {
                continue;
            };
            emit((rt, sub_property_of, *st));
            if sat.sup_star(s).contains(&(r as u32)) {
                emit((rt, equivalent_property, *st));
            }
        }
    }

    // Realization and the property-assertion closure.
    for &x in &loaded.individuals {
        let (Kind::Individual(xt), Some(ctx)) = (loaded.kind(x), sat.context(x)) else {
            continue;
        };
        for &a in &ctx.subs {
            match loaded.kind(a) {
                Kind::Class(at) => emit((xt, v.rdf_type, at)),
                _ if a == BOTTOM => emit((xt, v.rdf_type, v.nothing)),
                _ => {}
            }
        }
        for (&r, ys) in &ctx.succ {
            let Some(Some(rt)) = loaded.role_terms.get(r as usize) else {
                continue;
            };
            for &y in ys {
                if let Kind::Individual(yt) | Kind::Literal(yt) = loaded.kind(y) {
                    emit((xt, *rt, yt));
                }
            }
        }
    }

    // Keys.
    for (x, y) in key_matches(loaded, sat) {
        if let (Kind::Individual(xt), Kind::Individual(yt)) = (loaded.kind(x), loaded.kind(y)) {
            emit((xt, v.same_as, yt));
            emit((yt, v.same_as, xt));
        }
    }
    out
}

/// Pairs of distinct named individuals that are both instances of a key's
/// class and share a value for every key property (OWL 2 keys are
/// DL-safe: they apply to named individuals only).
fn key_matches(loaded: &Loaded, sat: &Saturation) -> Vec<(Cid, Cid)> {
    let mut out: Vec<(Cid, Cid)> = Vec::new();
    for (class, props) in &loaded.keys {
        // Each member's values, per key property.
        let mut members: Vec<(Cid, Vec<FxSet<Cid>>)> = Vec::new();
        for &x in &loaded.individuals {
            let Kind::Individual(t) = loaded.kind(x) else {
                continue;
            };
            if !matches!(loaded.term(t), Term::NamedNode(_)) || !sat.subsumes(x, *class) {
                continue;
            }
            let ctx = sat.context(x).expect("individual contexts exist");
            let values: Vec<FxSet<Cid>> = props
                .iter()
                .map(|p| {
                    ctx.succ
                        .get(p)
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(|y| sat.is_nominal(*y))
                        .collect()
                })
                .collect();
            if values.iter().all(|v| !v.is_empty()) {
                members.push((x, values));
            }
        }
        // Bucket by the first property's values, then check the rest.
        let mut buckets: FxMap<Cid, Vec<usize>> = FxMap::default();
        for (i, (_, values)) in members.iter().enumerate() {
            for &v in &values[0] {
                buckets.entry(v).or_default().push(i);
            }
        }
        let mut pairs: FxSet<(usize, usize)> = FxSet::default();
        for idx in buckets.values() {
            for (k, &i) in idx.iter().enumerate() {
                for &j in &idx[k + 1..] {
                    let (i, j) = (i.min(j), i.max(j));
                    if i == j || !pairs.insert((i, j)) {
                        continue;
                    }
                    let (a, b) = (&members[i].1, &members[j].1);
                    if a.iter().zip(b).all(|(va, vb)| !va.is_disjoint(vb)) {
                        out.push((members[i].0, members[j].0));
                    }
                }
            }
        }
    }
    out
}
