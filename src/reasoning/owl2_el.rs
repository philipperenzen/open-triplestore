//! OWL 2 EL profile — a native EL++ classifier.
//!
//! OWL 2 EL is the profile of large biomedical ontologies (SNOMED CT, the
//! Gene Ontology). This module reads the ontology straight out of the quad
//! index, normalizes it (`load.rs`), saturates it with the EL++ completion
//! rules (`saturate.rs`) and writes what follows into the target graph:
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
//! - the **property-assertion closure** between individuals (and to data
//!   values): through sub-properties, chains of any length, transitivity,
//!   reflexivity and self restrictions, and the edges `owl:hasValue` implies;
//! - **equality** — `owl:sameAs` between individuals that keys, nominals or
//!   asserted `owl:sameAs` make the same.
//!
//! Only triples not already in a premise graph are written, through
//! [`TripleStore::insert_quads`] so the graph index and change capture see
//! them. The rules read the source graphs (or, unscoped, the unnamed default
//! graph) plus the target graph itself.
//!
//! # Supported constructs (OWL 2 Profiles §2.2)
//!
//! Class expressions: class IRIs, `owl:Thing`, `owl:Nothing`,
//! `owl:intersectionOf` of any arity, `owl:someValuesFrom`,
//! `owl:hasValue` and `owl:hasSelf` over object properties, `owl:oneOf` of one
//! individual; `owl:someValuesFrom` and `owl:hasValue` over data properties.
//! Data ranges: the nineteen EL datatypes (`datatypes.rs`), datatypes declared
//! in the ontology (and their definitions), intersections, `owl:oneOf` of one
//! literal. Axioms: `rdfs:subClassOf`, `owl:equivalentClass`,
//! `owl:disjointWith`, `owl:AllDisjointClasses`, `rdfs:subPropertyOf`,
//! `owl:equivalentProperty`, `owl:propertyChainAxiom`, `owl:TransitiveProperty`,
//! `owl:ReflexiveProperty`, `rdfs:domain`, `rdfs:range` (object and data),
//! `owl:FunctionalProperty` on data properties, `owl:hasKey`, `owl:sameAs`,
//! `owl:differentFrom`, `owl:AllDifferent`, `owl:NegativePropertyAssertion`,
//! class and property assertions. Literals compare by value.
//!
//! What the loader cannot use — constructs outside the profile, such as
//! unions, universals, cardinalities, inverse properties or functional object
//! properties — is counted per construct in [`ReasoningReport::ignored`].
//! Leaving an axiom out never adds a wrong consequence, it only loses some.
//!
//! # Consistency
//!
//! An EL ontology is inconsistent when `owl:Thing ⊑ owl:Nothing`, an
//! individual is an instance of `owl:Nothing` (directly, through its classes
//! and successors, by being typed with two disjoint classes, by being the
//! same as an individual it is different from, or by having a property value
//! a negative property assertion denies), or a literal is ill-typed or
//! outside a data range it must be in (a functional data property with two
//! values, say). An
//! unsatisfiable *class* without instances is not an inconsistency;
//! [`El2Classifier::unsatisfiable_classes`] lists those.
//! [`El2Classifier::classify`] fails with [`ReasoningError::Inconsistency`]
//! after writing what it derived.
// The binary re-declares the library's modules; the inspection methods
// below are used by the library and its tests only.
#![allow(dead_code)]

mod datatypes;
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
    rule: &'static str,
    detail: String,
}

/// A saturated ontology.
struct Saturated {
    loaded: Loaded,
    sat: Saturation,
    /// Rounds of saturation (keys can merge individuals, which needs another).
    rounds: usize,
    /// Subsumers of the classes that needed a hypothetical instance (see
    /// [`Saturation::hypothetical_subsumers`]); the rest read `sat` directly.
    hypothetical: FxMap<Cid, FxSet<Cid>>,
}

impl Saturated {
    /// The subsumers of class `c`.
    fn subs(&self, c: Cid) -> Option<&FxSet<Cid>> {
        self.hypothetical
            .get(&c)
            .or_else(|| self.sat.context(c).map(|cx| &cx.subs))
    }
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
    /// scope the rules read the unnamed default graph and the target graph, so
    /// a dataset's named graphs — and the model version it conforms to — are
    /// invisible to materialisation; this is what `POST /api/reasoning/materialize` sets
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
        // Keys: individuals that agree on a key are the same, which can make
        // more individuals agree. Each round merges at least one pair.
        let mut rounds = 1;
        loop {
            let same = key_matches(&loaded, &sat);
            if same.is_empty() {
                break;
            }
            for (x, y) in same {
                sat.add_sub(x, y);
                sat.add_sub(y, x);
            }
            sat.run();
            rounds += 1;
        }
        // Classes whose subsumers may rest on two reachable contexts being
        // the same nominal get a hypothetical instance (consistent ontologies
        // only: in an inconsistent one every subsumption holds).
        let mut hypothetical: FxMap<Cid, FxSet<Cid>> = FxMap::default();
        if inconsistency(&loaded, &sat).is_none() && sat.has_unreached_nominal_members() {
            for &c in &loaded.classes {
                if sat.needs_hypothesis(c) {
                    hypothetical.insert(c, sat.hypothetical_subsumers(c));
                }
            }
        }
        Ok(Saturated {
            loaded,
            sat,
            rounds,
            hypothetical,
        })
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
        let mut s = self.saturate()?;
        let derived = derived_triples(&mut s);
        let Saturated {
            loaded,
            sat,
            rounds,
            ..
        } = s;
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
                return Err(ReasoningError::inconsistency(clash.rule, clash.detail));
            }
        }
        Ok(ReasoningReport {
            regime: "owl2-el".to_string(),
            triples_added: added,
            iterations: rounds,
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
            ..Default::default()
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
            .filter(|&&c| s.subs(c).is_some_and(|subs| subs.contains(&BOTTOM)))
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

/// The first reason the saturated ontology is inconsistent, if any. The rule
/// ids follow the OWL 2 RL names for the same clash where there is one. The
/// checks go from the most specific cause to the least: an ill-typed
/// literal, two values of a functional property, a value outside a range,
/// then an individual's classes.
fn inconsistency(loaded: &Loaded, sat: &Saturation) -> Option<Clash> {
    if sat.subsumes(TOP, BOTTOM) {
        return Some(Clash {
            rule: "el-top-bottom",
            detail: "owl:Thing is a subclass of owl:Nothing".to_string(),
        });
    }
    let name = |c: Cid| match loaded.kind(c) {
        Kind::Individual(t) | Kind::Literal(t) | Kind::Class(t) => loaded.term(t).to_string(),
        _ => "a class expression".to_string(),
    };
    for &v in &loaded.ill_typed {
        if sat.subsumes(v, BOTTOM) {
            return Some(Clash {
                rule: "dt-not-type",
                detail: format!("{} is ill-typed", name(v)),
            });
        }
    }
    let clashing: Vec<(Cid, &saturate::Context)> = loaded
        .individuals
        .iter()
        .filter_map(|&x| sat.context(x).map(|cx| (x, cx)))
        .filter(|(_, cx)| cx.subs.contains(&BOTTOM))
        .collect();
    for &(x, cx) in &clashing {
        for &r in &sat.axioms().functional {
            let values: FxSet<Cid> = cx
                .succ
                .get(&r)
                .into_iter()
                .flatten()
                .filter_map(|&y| sat.context(y))
                .flat_map(|ycx| ycx.noms.iter().copied())
                .collect();
            if values.len() > 1 {
                let p = loaded.role_terms[r as usize]
                    .map(|t| loaded.term(t).to_string())
                    .unwrap_or_default();
                return Some(Clash {
                    rule: "prp-fp",
                    detail: format!("{} has two values for the functional property {p}", name(x)),
                });
            }
        }
    }
    for &v in loaded.literal_terms.keys() {
        if sat.subsumes(v, BOTTOM) {
            return Some(Clash {
                rule: "dt-not-type",
                detail: format!(
                    "{} is outside a data range it is required to be in",
                    name(v)
                ),
            });
        }
    }
    let conj = &sat.axioms().conj;
    let (x, cx) = *clashing.first()?;
    let pair = cx.subs.iter().find_map(|&a| {
        conj.get(&a).and_then(|entries| {
            entries
                .iter()
                .find(|&&(a2, b)| b == BOTTOM && cx.subs.contains(&a2))
                .map(|&(a2, _)| (a, a2))
        })
    });
    let n = name(x);
    Some(match pair {
        Some((a, b)) if sat.is_nominal(a) && sat.is_nominal(b) => Clash {
            rule: "eq-diff1",
            detail: format!(
                "{} and {} are the same individual and different ones",
                name(a),
                name(b)
            ),
        },
        Some((a, b))
            if loaded.negative_assertions.contains(&a)
                || loaded.negative_assertions.contains(&b) =>
        {
            Clash {
                rule: "prp-npa1",
                detail: format!("{n} has a property value a negative property assertion denies"),
            }
        }
        Some(_) => Clash {
            rule: "cax-dw",
            detail: format!("{n} is an instance of two disjoint classes"),
        },
        None => Clash {
            rule: "cls-nothing2",
            detail: format!("{n} is an instance of owl:Nothing"),
        },
    })
}

/// Every entailed triple of the output vocabulary not already a premise.
fn derived_triples(s: &mut Saturated) -> Vec<(Tid, Tid, Tid)> {
    let v = s.loaded.vocab;
    let equivalent_class = s.loaded.iri(&format!("{OWL}equivalentClass"));
    let sub_property_of = s.loaded.iri(&format!("{RDFS}subPropertyOf"));
    let equivalent_property = s.loaded.iri(&format!("{OWL}equivalentProperty"));
    let s = &*s;
    let (loaded, sat) = (&s.loaded, &s.sat);
    let mut out: Vec<(Tid, Tid, Tid)> = Vec::new();
    let mut seen: FxSet<(Tid, Tid, Tid)> = FxSet::default();
    let mut emit = |t: (Tid, Tid, Tid)| {
        if !loaded.premises.contains(&t) && seen.insert(t) {
            out.push(t);
        }
    };

    // Classification.
    for &c in &loaded.classes {
        let (Kind::Class(ct), Some(subs)) = (loaded.kind(c), s.subs(c)) else {
            continue;
        };
        let unsat = subs.contains(&BOTTOM);
        for &a in subs {
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
                    if !unsat && s.subs(a).is_some_and(|sa| sa.contains(&c)) {
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
            // An edge to any context that is a nominal — an individual, a
            // value, or a `hasValue` filler — is an edge to that nominal.
            let mut targets: FxSet<Cid> = FxSet::default();
            for &y in ys {
                if let Some(ycx) = sat.context(y) {
                    targets.extend(ycx.noms.iter().copied());
                }
            }
            for n in targets {
                match loaded.kind(n) {
                    Kind::Individual(yt) => emit((xt, *rt, yt)),
                    Kind::Literal(yt) => {
                        // Stated already with an equal value in another form?
                        let terms = loaded.literal_terms.get(&n);
                        if !terms.is_some_and(|ts| {
                            ts.iter().any(|t| loaded.premises.contains(&(xt, *rt, *t)))
                        }) {
                            emit((xt, *rt, yt));
                        }
                    }
                    _ => {}
                }
            }
        }
        // Equality: other individuals among x's nominals.
        for &n in &ctx.noms {
            if let (true, Kind::Individual(yt)) = (n != x, loaded.kind(n)) {
                emit((xt, v.same_as, yt));
            }
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
            // The values of a key property: the nominals of its successors
            // (an individual, a data value, or a filler known to be one).
            let values: Vec<FxSet<Cid>> = props
                .iter()
                .map(|p| {
                    ctx.succ
                        .get(p)
                        .into_iter()
                        .flatten()
                        .filter_map(|&y| sat.context(y))
                        .flat_map(|ycx| ycx.noms.iter().copied())
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
                    let (x, y) = (members[i].0, members[j].0);
                    if sat.subsumes(x, y) {
                        continue; // already the same individual
                    }
                    let (a, b) = (&members[i].1, &members[j].1);
                    if a.iter().zip(b).all(|(va, vb)| !va.is_disjoint(vb)) {
                        out.push((x, y));
                    }
                }
            }
        }
    }
    out
}
