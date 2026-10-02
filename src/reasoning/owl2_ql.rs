//! OWL 2 QL (DL-Lite_R): the TBox closure, ground materialisation,
//! consistency, and existential query rewriting.
//!
//! The TBox is read as inclusions between DL-Lite *basic concepts* (a named
//! class `A`, or `∃R`: the subjects of a role `R`, so `∃P⁻` is the objects
//! of `P`) and between roles, plus negative inclusions:
//!
//! | RDF | DL-Lite |
//! |---|---|
//! | `rdfs:subClassOf`, `owl:equivalentClass` | `B ⊑ C` for every QL sub/superclass expression |
//! | `rdfs:domain` / `rdfs:range` | `∃P ⊑ C` / `∃P⁻ ⊑ C` (a datatype range on a data property is a value check) |
//! | `[owl:onProperty R; owl:someValuesFrom owl:Thing]` | `∃R` on either side |
//! | `[owl:onProperty R; owl:someValuesFrom A]` on the right | `B ⊑ ∃R.A`, read with a fresh role `F`: `F ⊑ R`, `∃F⁻ ⊑ A`, `B ⊑ ∃F` |
//! | `owl:intersectionOf` on the right | one inclusion per conjunct |
//! | `owl:complementOf`, `owl:disjointWith`, `owl:AllDisjointClasses`, `owl:Nothing` | `B ⊑ ¬C` |
//! | `rdfs:subPropertyOf`, `owl:equivalentProperty`, `owl:inverseOf`, `owl:SymmetricProperty` | `R ⊑ S` (both polarities) |
//! | `owl:propertyDisjointWith`, `owl:AllDisjointProperties`, `owl:AsymmetricProperty` | `R ⊑ ¬S` |
//! | `owl:ReflexiveProperty`, `owl:IrreflexiveProperty` | every element has (has no) a `P` loop |
//!
//! Everything else (`owl:TransitiveProperty`, functional properties, keys,
//! chains, `owl:sameAs`, unions, cardinalities, …) is outside the profile; it
//! is not used and is reported in [`ReasoningReport::ignored_axioms`].
//!
//! [`QLQueryRewriter::materialize`] closes the hierarchies and the negative
//! inclusions (unsatisfiability propagates through roles and fresh roles),
//! writes every entailed *ground* atom over the individuals in the data
//! (class memberships, property assertions, the named-class and
//! named-property closure), and checks consistency: a negative inclusion, an
//! irreflexive or asymmetric property, `a owl:differentFrom a`, or a
//! data-property value outside its declared datatype makes the run fail with
//! [`ReasoningError::Inconsistency`]; what was derived stays in the target.
//!
//! Ground atoms cannot express an existential: `Parent ⊑ ∃hasChild` says that
//! every parent has *some* child, not which. A query reaches those anonymous
//! elements through blank nodes, which SPARQL treats as existential
//! variables. [`rewrite_existentials`] rewrites the parts of a query that
//! contain blank nodes into conditions on named individuals (a tree-witness
//! rewriting into the anonymous part of the canonical model); variables only
//! bind to names, which the materialised graph answers directly. That is the
//! `?entailment=owl2-ql` path of `/sparql`.
//!
//! [`QLQueryRewriter::rewrite_query`] is the standalone rewriting: it also
//! expands every ground atom through the hierarchies, so it answers over the
//! asserted data alone, without a materialised graph.
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

use oxigraph::model::{
    GraphName, Literal, NamedNode, NamedNodeRef, NamedOrBlankNode, NamedOrBlankNodeRef, Quad, Term,
    TermRef,
};
use spargebra::algebra::{Expression, GraphPattern};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern, Variable};
use spargebra::Query;

use super::common::{
    IgnoredAxiom, ReasoningError, ReasoningReport, IGNORED_SAMPLE, OWL2_QL_ENTAILMENT_GRAPH,
};
use crate::store::TripleStore;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tracing::debug;

// ─── Namespace constants ──────────────────────────────────────────────────────

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS_NS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL_NS: &str = "http://www.w3.org/2002/07/owl#";
const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema#";

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDF_PLAIN_LITERAL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral";
const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const RDF_XML_LITERAL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_SUB_PROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const RDFS_LITERAL: &str = "http://www.w3.org/2000/01/rdf-schema#Literal";
const RDFS_DATATYPE: &str = "http://www.w3.org/2000/01/rdf-schema#Datatype";
const OWL_THING: &str = "http://www.w3.org/2002/07/owl#Thing";
const OWL_NOTHING: &str = "http://www.w3.org/2002/07/owl#Nothing";
const OWL_EQUIV_CLASS: &str = "http://www.w3.org/2002/07/owl#equivalentClass";
const OWL_EQUIV_PROP: &str = "http://www.w3.org/2002/07/owl#equivalentProperty";
const OWL_INVERSE_OF: &str = "http://www.w3.org/2002/07/owl#inverseOf";
const OWL_SOME_VALUES_FROM: &str = "http://www.w3.org/2002/07/owl#someValuesFrom";
const OWL_ON_PROPERTY: &str = "http://www.w3.org/2002/07/owl#onProperty";
const OWL_INTERSECTION_OF: &str = "http://www.w3.org/2002/07/owl#intersectionOf";
const OWL_COMPLEMENT_OF: &str = "http://www.w3.org/2002/07/owl#complementOf";
const OWL_DISJOINT_WITH: &str = "http://www.w3.org/2002/07/owl#disjointWith";
const OWL_ALL_DISJOINT_CLASSES: &str = "http://www.w3.org/2002/07/owl#AllDisjointClasses";
const OWL_ALL_DISJOINT_PROPERTIES: &str = "http://www.w3.org/2002/07/owl#AllDisjointProperties";
const OWL_MEMBERS: &str = "http://www.w3.org/2002/07/owl#members";
const OWL_PROPERTY_DISJOINT_WITH: &str = "http://www.w3.org/2002/07/owl#propertyDisjointWith";
const OWL_DATATYPE_PROPERTY: &str = "http://www.w3.org/2002/07/owl#DatatypeProperty";
const OWL_SYMMETRIC: &str = "http://www.w3.org/2002/07/owl#SymmetricProperty";
const OWL_ASYMMETRIC: &str = "http://www.w3.org/2002/07/owl#AsymmetricProperty";
const OWL_REFLEXIVE: &str = "http://www.w3.org/2002/07/owl#ReflexiveProperty";
const OWL_IRREFLEXIVE: &str = "http://www.w3.org/2002/07/owl#IrreflexiveProperty";
const OWL_DIFFERENT_FROM: &str = "http://www.w3.org/2002/07/owl#differentFrom";
const OWL_REAL: &str = "http://www.w3.org/2002/07/owl#real";
const OWL_RATIONAL: &str = "http://www.w3.org/2002/07/owl#rational";

/// Axiom-level vocabulary outside OWL 2 QL: `(type or predicate, is a type)`.
/// Class expressions outside the profile are reported through the axiom
/// that uses them.
const NON_PROFILE: &[(&str, bool)] = &[
    ("http://www.w3.org/2002/07/owl#TransitiveProperty", true),
    ("http://www.w3.org/2002/07/owl#FunctionalProperty", true),
    (
        "http://www.w3.org/2002/07/owl#InverseFunctionalProperty",
        true,
    ),
    (
        "http://www.w3.org/2002/07/owl#NegativePropertyAssertion",
        true,
    ),
    ("http://www.w3.org/2002/07/owl#sameAs", false),
    ("http://www.w3.org/2002/07/owl#hasKey", false),
    ("http://www.w3.org/2002/07/owl#propertyChainAxiom", false),
    ("http://www.w3.org/2002/07/owl#disjointUnionOf", false),
];

/// Prefix of the fresh roles that stand for qualified existentials. Not an
/// IRI (no scheme), so it can never name a property in the store.
const FRESH: &str = "_:ql-fresh-";

/// The most blank nodes one connected part of a query may have and still be
/// rewritten (every subset of them may be anonymous). Larger parts are left
/// as written: still sound, answered over named individuals only.
const MAX_BNODES: usize = 8;
/// Search steps one group of anonymous blank nodes may take.
const SEARCH_BUDGET: usize = 50_000;
/// Union branches one connected part of a query may produce.
const MAX_BRANCHES: usize = 512;

// ─── TBox: basic concepts and roles ──────────────────────────────────────────

/// A DL-Lite role: a property, or its inverse.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

    fn is_fresh(&self) -> bool {
        self.iri.starts_with(FRESH)
    }
}

/// A DL-Lite basic concept: a named class, or `∃R` (the subjects of a role,
/// so `∃P⁻` is the objects of `P`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Basic {
    Class(String),
    Exists(Role),
}

/// The axioms read from the TBox, before closure.
#[derive(Default)]
struct Axioms {
    concept_incl: Vec<(Basic, Basic)>,
    role_incl: Vec<(Role, Role)>,
    concept_ni: Vec<(Basic, Basic)>,
    role_ni: Vec<(Role, Role)>,
    reflexive: Vec<String>,
    irreflexive: Vec<String>,
    data_props: HashSet<String>,
    datatypes: HashSet<String>,
    data_ranges: Vec<(String, String)>,
    fresh: usize,
    ignored: Ignored,
}

/// Axioms outside the profile: a count and the first few.
#[derive(Default, Clone)]
struct Ignored {
    seen: HashSet<(String, String)>,
    sample: Vec<IgnoredAxiom>,
}

impl Ignored {
    fn add(&mut self, axiom: &str, subject: &str, reason: &str) {
        if self.seen.insert((axiom.to_string(), subject.to_string()))
            && self.sample.len() < IGNORED_SAMPLE
        {
            self.sample.push(IgnoredAxiom {
                axiom: axiom.to_string(),
                subject: subject.to_string(),
                reason: reason.to_string(),
            });
        }
    }

    fn count(&self) -> usize {
        self.seen.len()
    }
}

/// The closed TBox, every basic concept and role interned to an index.
pub struct QlTBox {
    basics: Vec<Basic>,
    basic_ix: HashMap<Basic, usize>,
    roles: Vec<Role>,
    role_ix: HashMap<Role, usize>,
    /// Role → its inverse.
    inv: Vec<usize>,
    /// Role → the basic concept `∃role`.
    exists: Vec<usize>,
    /// Basic concept → the role it is `∃` of, if any.
    exists_of: Vec<Option<usize>>,
    /// Role → every role it is included in, itself first-class (sorted).
    role_sup: Vec<Vec<usize>>,
    /// Role → every role included in it (sorted).
    role_sub: Vec<Vec<usize>>,
    /// Basic concept → every basic concept it is included in (sorted).
    sup: Vec<Vec<usize>>,
    /// Basic concept → every basic concept included in it (sorted).
    sub: Vec<Vec<usize>>,
    /// Declared negative inclusions between basic concepts, both ways.
    ni: Vec<Vec<usize>>,
    /// Declared negative inclusions between roles, both ways and both
    /// polarities.
    role_ni: Vec<Vec<usize>>,
    /// Basic concepts / roles that are empty in every model.
    unsat: Vec<bool>,
    role_unsat: Vec<bool>,
    /// Roles every element has a loop of (through a reflexive sub-role).
    loops: Vec<bool>,
    /// Positive irreflexive roles.
    irreflexive: Vec<usize>,
    /// Positive reflexive roles.
    reflexive: Vec<usize>,
    /// Basic concepts every element belongs to (sorted, upward closed).
    top: Vec<usize>,
    /// Roles whose values are literals (data properties).
    data_roles: Vec<bool>,
    /// Positive data role → the datatypes its values must belong to.
    data_ranges: HashMap<usize, Vec<String>>,
    /// An inconsistency that holds whatever the data (the domain of an
    /// interpretation is never empty).
    clash: Option<(String, String)>,
    ignored: Ignored,
}

impl QlTBox {
    fn empty() -> Self {
        Self {
            basics: Vec::new(),
            basic_ix: HashMap::new(),
            roles: Vec::new(),
            role_ix: HashMap::new(),
            inv: Vec::new(),
            exists: Vec::new(),
            exists_of: Vec::new(),
            role_sup: Vec::new(),
            role_sub: Vec::new(),
            sup: Vec::new(),
            sub: Vec::new(),
            ni: Vec::new(),
            role_ni: Vec::new(),
            unsat: Vec::new(),
            role_unsat: Vec::new(),
            loops: Vec::new(),
            irreflexive: Vec::new(),
            reflexive: Vec::new(),
            top: Vec::new(),
            data_roles: Vec::new(),
            data_ranges: HashMap::new(),
            clash: None,
            ignored: Ignored::default(),
        }
    }

    fn push_basic(&mut self, b: Basic) -> usize {
        let i = self.basics.len();
        self.basics.push(b.clone());
        self.basic_ix.insert(b, i);
        i
    }

    /// Intern `r` and its inverse (and `∃` of both); returns `r`'s index.
    fn role_id(&mut self, r: &Role) -> usize {
        if let Some(&i) = self.role_ix.get(r) {
            return i;
        }
        let i = self.roles.len();
        self.roles.push(r.clone());
        self.roles.push(r.inv());
        self.role_ix.insert(r.clone(), i);
        self.role_ix.insert(r.inv(), i + 1);
        self.inv.push(i + 1);
        self.inv.push(i);
        let a = self.push_basic(Basic::Exists(r.clone()));
        let b = self.push_basic(Basic::Exists(r.inv()));
        self.exists.push(a);
        self.exists.push(b);
        i
    }

    fn basic_id(&mut self, b: &Basic) -> usize {
        if let Some(&i) = self.basic_ix.get(b) {
            return i;
        }
        match b {
            Basic::Exists(r) => {
                let r = self.role_id(r);
                self.exists[r]
            }
            Basic::Class(_) => self.push_basic(b.clone()),
        }
    }

    /// Close the axioms: hierarchies, negative inclusions, unsatisfiability.
    fn build(ax: Axioms) -> Self {
        let mut t = Self::empty();
        let ci: Vec<(usize, usize)> = ax
            .concept_incl
            .iter()
            .map(|(a, b)| (t.basic_id(a), t.basic_id(b)))
            .collect();
        let ri: Vec<(usize, usize)> = ax
            .role_incl
            .iter()
            .map(|(a, b)| (t.role_id(a), t.role_id(b)))
            .collect();
        let cn: Vec<(usize, usize)> = ax
            .concept_ni
            .iter()
            .map(|(a, b)| (t.basic_id(a), t.basic_id(b)))
            .collect();
        let rn: Vec<(usize, usize)> = ax
            .role_ni
            .iter()
            .map(|(a, b)| (t.role_id(a), t.role_id(b)))
            .collect();
        let reflexive: Vec<usize> = ax
            .reflexive
            .iter()
            .map(|p| t.role_id(&Role::named(p.as_str())))
            .collect();
        let irreflexive: Vec<usize> = ax
            .irreflexive
            .iter()
            .map(|p| t.role_id(&Role::named(p.as_str())))
            .collect();
        for (p, dt) in &ax.data_ranges {
            let r = t.role_id(&Role::named(p.as_str()));
            t.data_ranges.entry(r).or_default().push(dt.clone());
        }

        let nb = t.basics.len();
        let nr = t.roles.len();

        // Role hierarchy: every inclusion in both polarities.
        let mut redges: Vec<Vec<usize>> = vec![Vec::new(); nr];
        for &(a, b) in &ri {
            if a != b {
                redges[a].push(b);
                redges[t.inv[a]].push(t.inv[b]);
            }
        }
        t.role_sup = (0..nr).map(|r| reach(r, &redges)).collect();
        t.role_sub = invert(&t.role_sup);

        // Concept hierarchy: declared inclusions, and ∃R ⊑ ∃S for R ⊑ S.
        let mut cedges: Vec<Vec<usize>> = vec![Vec::new(); nb];
        for &(a, b) in &ci {
            if a != b {
                cedges[a].push(b);
            }
        }
        for (r, outs) in redges.iter().enumerate() {
            for &s in outs {
                cedges[t.exists[r]].push(t.exists[s]);
            }
        }
        t.sup = (0..nb).map(|b| reach(b, &cedges)).collect();
        t.sub = invert(&t.sup);
        t.exists_of = vec![None; nb];
        for (r, &e) in t.exists.iter().enumerate() {
            t.exists_of[e] = Some(r);
        }

        t.ni = vec![Vec::new(); nb];
        for &(a, b) in &cn {
            t.ni[a].push(b);
            t.ni[b].push(a);
        }
        t.role_ni = vec![Vec::new(); nr];
        for &(a, b) in &rn {
            let (ia, ib) = (t.inv[a], t.inv[b]);
            t.role_ni[a].push(b);
            t.role_ni[b].push(a);
            t.role_ni[ia].push(ib);
            t.role_ni[ib].push(ia);
        }

        t.data_roles = (0..nr)
            .map(|r| ax.data_props.contains(&t.roles[r].iri))
            .collect();

        // Reflexivity: every element has a loop of every super-role of a
        // reflexive role, and so is in ∃R and ∃R⁻ of each.
        t.loops = vec![false; nr];
        let mut top: BTreeSet<usize> = BTreeSet::new();
        for &q in &reflexive {
            for &r in &t.role_sup[q] {
                t.loops[r] = true;
                t.loops[t.inv[r]] = true;
            }
            for e in [t.exists[q], t.exists[t.inv[q]]] {
                top.extend(t.sup[e].iter().copied());
            }
        }
        t.top = top.into_iter().collect();
        t.reflexive = reflexive;
        t.irreflexive = irreflexive;

        // Unsatisfiability, to a fixed point: a basic concept under a
        // negative inclusion pair or an empty concept is empty; a role is
        // empty with its ∃ (either side), its inverse, or a super-role.
        t.unsat = vec![false; nb];
        t.role_unsat = vec![false; nr];
        loop {
            let mut changed = false;
            for b in 0..nb {
                if !t.unsat[b]
                    && (t.sup[b].iter().any(|&s| t.unsat[s])
                        || clash_in(&t.sup[b], &t.ni).is_some())
                {
                    t.unsat[b] = true;
                    changed = true;
                }
            }
            for r in 0..nr {
                if !t.role_unsat[r]
                    && (t.role_sup[r].iter().any(|&s| t.role_unsat[s])
                        || clash_in(&t.role_sup[r], &t.role_ni).is_some()
                        || t.unsat[t.exists[r]]
                        || t.unsat[t.exists[t.inv[r]]])
                {
                    t.role_unsat[r] = true;
                    changed = true;
                }
            }
            for r in 0..nr {
                if t.role_unsat[r] {
                    for x in [t.exists[r], t.exists[t.inv[r]]] {
                        if !t.unsat[x] {
                            t.unsat[x] = true;
                            changed = true;
                        }
                    }
                    let i = t.inv[r];
                    if !t.role_unsat[i] {
                        t.role_unsat[i] = true;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // What holds of every element, data or not.
        if let Some(&b) = t.top.iter().find(|&&b| t.unsat[b]) {
            t.clash = Some((
                "ql-cls-nothing".into(),
                format!(
                    "every individual is in {}, which is unsatisfiable",
                    t.show(b)
                ),
            ));
        } else if let Some((a, b)) = clash_in(&t.top, &t.ni) {
            t.clash = Some((
                "ql-cls-disjoint".into(),
                format!(
                    "every individual is in both {} and {}, declared disjoint",
                    t.show(a),
                    t.show(b)
                ),
            ));
        } else if let Some(&p) = t.irreflexive.iter().find(|&&p| t.loops[p]) {
            t.clash = Some((
                "ql-prp-irp".into(),
                format!(
                    "<{}> is irreflexive but includes a reflexive property",
                    t.roles[p].iri
                ),
            ));
        } else if let Some(r) = (0..nr)
            .find(|&r| t.loops[r] && (t.role_unsat[r] || t.role_ni[r].iter().any(|&s| t.loops[s])))
        {
            t.clash = Some((
                "ql-prp-disjoint".into(),
                format!(
                    "the loops every individual has of {} are declared disjoint",
                    t.show_role(r)
                ),
            ));
        }
        t.ignored = ax.ignored;
        t
    }

    fn show_role(&self, r: usize) -> String {
        let role = &self.roles[r];
        if role.is_fresh() {
            "a qualified existential".into()
        } else if role.inverse {
            format!("inverse(<{}>)", role.iri)
        } else {
            format!("<{}>", role.iri)
        }
    }

    fn show(&self, b: usize) -> String {
        match &self.basics[b] {
            Basic::Class(c) => format!("<{c}>"),
            Basic::Exists(_) => match self.exists_of[b] {
                Some(r) => format!("∃{}", self.show_role(r)),
                None => "?".into(),
            },
        }
    }

    fn class_id(&self, iri: &str) -> Option<usize> {
        self.basic_ix.get(&Basic::Class(iri.to_string())).copied()
    }

    fn property_id(&self, iri: &str) -> Option<usize> {
        self.role_ix.get(&Role::named(iri)).copied()
    }

    /// Whether an anonymous element created as an `r`-successor is in basic
    /// concept `b`: it is in `∃r⁻`, everything above it, and what every
    /// element is in. A data value is in nothing.
    fn node_has(&self, r: usize, b: usize) -> bool {
        !self.data_roles[r]
            && (self.sup[self.exists[self.inv[r]]].binary_search(&b).is_ok()
                || self.top.binary_search(&b).is_ok())
    }

    /// Roles an `r`-successor has successors of.
    fn children(&self, r: usize) -> Vec<usize> {
        (0..self.roles.len())
            .filter(|&s| self.node_has(r, self.exists[s]))
            .collect()
    }

    /// Whether every model has an anonymous `r`-successor somewhere,
    /// whatever the data: reachable from what every element is in.
    fn always_exists(&self, r: usize) -> bool {
        let mut seen: HashSet<usize> = HashSet::new();
        let mut queue: Vec<usize> = self.top.iter().filter_map(|&b| self.exists_of[b]).collect();
        while let Some(s) = queue.pop() {
            if s == r {
                return true;
            }
            if seen.insert(s) {
                queue.extend(self.children(s));
            }
        }
        false
    }

    /// Roles whose successors can, through successors of successors, lead
    /// to an `r`-successor (`r` included).
    fn ancestors(&self, r: usize) -> Vec<usize> {
        let n = self.roles.len();
        let mut parents: Vec<Vec<usize>> = vec![Vec::new(); n];
        for s in 0..n {
            for c in self.children(s) {
                parents[c].push(s);
            }
        }
        let mut seen: Vec<bool> = vec![false; n];
        let mut out = vec![r];
        seen[r] = true;
        let mut i = 0;
        while i < out.len() {
            for &p in &parents[out[i]] {
                if !seen[p] {
                    seen[p] = true;
                    out.push(p);
                }
            }
            i += 1;
        }
        out
    }
}

/// Everything reachable from `start` over `edges`, `start` included, sorted.
fn reach(start: usize, edges: &[Vec<usize>]) -> Vec<usize> {
    let mut seen: HashSet<usize> = HashSet::from([start]);
    let mut out = vec![start];
    let mut i = 0;
    while i < out.len() {
        for &n in &edges[out[i]] {
            if seen.insert(n) {
                out.push(n);
            }
        }
        i += 1;
    }
    out.sort_unstable();
    out
}

fn invert(sup: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut sub: Vec<Vec<usize>> = vec![Vec::new(); sup.len()];
    for (a, sups) in sup.iter().enumerate() {
        for &b in sups {
            sub[b].push(a);
        }
    }
    for s in &mut sub {
        s.sort_unstable();
    }
    sub
}

/// A pair of members of the sorted `set` declared disjoint in `ni`.
fn clash_in(set: &[usize], ni: &[Vec<usize>]) -> Option<(usize, usize)> {
    for &a in set {
        for &b in &ni[a] {
            if set.binary_search(&b).is_ok() {
                return Some((a, b));
            }
        }
    }
    None
}

// ─── Reading the TBox ────────────────────────────────────────────────────────

/// The graphs a run reads, over the store's quad index.
struct Src<'a> {
    store: &'a oxigraph::store::Store,
    graphs: Vec<GraphName>,
}

impl<'a> Src<'a> {
    fn each(
        &self,
        s: Option<NamedOrBlankNodeRef<'_>>,
        p: Option<NamedNodeRef<'_>>,
        o: Option<TermRef<'_>>,
        mut f: impl FnMut(Quad),
    ) -> Result<(), ReasoningError> {
        for g in &self.graphs {
            for q in self.store.quads_for_pattern(s, p, o, Some(g.as_ref())) {
                f(q.map_err(|e| ReasoningError::Store(e.to_string()))?);
            }
        }
        Ok(())
    }

    fn pairs(&self, p: &str) -> Result<Vec<(NamedOrBlankNode, Term)>, ReasoningError> {
        let mut out = Vec::new();
        self.each(None, Some(NamedNodeRef::new_unchecked(p)), None, |q| {
            out.push((q.subject, q.object))
        })?;
        out.sort_by_key(|(s, o)| (s.to_string(), o.to_string()));
        out.dedup();
        Ok(out)
    }

    fn objects(&self, s: &Term, p: &str) -> Result<Vec<Term>, ReasoningError> {
        let mut out = Vec::new();
        if let Some(s) = node(s) {
            self.each(
                Some(s.as_ref()),
                Some(NamedNodeRef::new_unchecked(p)),
                None,
                |q| out.push(q.object),
            )?;
        }
        out.dedup();
        Ok(out)
    }

    fn object(&self, s: &Term, p: &str) -> Result<Option<Term>, ReasoningError> {
        Ok(self.objects(s, p)?.into_iter().next())
    }

    fn subjects_of_type(&self, class: &str) -> Result<Vec<NamedOrBlankNode>, ReasoningError> {
        let mut out = Vec::new();
        self.each(
            None,
            Some(NamedNodeRef::new_unchecked(RDF_TYPE)),
            Some(NamedNodeRef::new_unchecked(class).into()),
            |q| out.push(q.subject),
        )?;
        out.sort_by_key(|s| s.to_string());
        out.dedup();
        Ok(out)
    }

    fn has(&self, s: &Term, p: &str, o: &Term) -> Result<bool, ReasoningError> {
        let Some(s) = node(s) else {
            return Ok(false);
        };
        let p = NamedNodeRef::new_unchecked(p);
        for g in &self.graphs {
            if self
                .store
                .quads_for_pattern(
                    Some(s.as_ref()),
                    Some(p),
                    Some(o.as_ref()),
                    Some(g.as_ref()),
                )
                .next()
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The members of an RDF list, or `None` when it is malformed.
    fn list(&self, head: &Term) -> Result<Option<Vec<Term>>, ReasoningError> {
        let mut out = Vec::new();
        let mut cell = head.clone();
        for _ in 0..10_000 {
            if matches!(&cell, Term::NamedNode(n) if n.as_str() == RDF_NIL) {
                return Ok(Some(out));
            }
            let (Some(first), Some(rest)) = (
                self.object(&cell, RDF_FIRST)?,
                self.object(&cell, RDF_REST)?,
            ) else {
                return Ok(None);
            };
            out.push(first);
            cell = rest;
        }
        Ok(None)
    }
}

fn node(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(NamedOrBlankNode::NamedNode(n.clone())),
        Term::BlankNode(b) => Some(NamedOrBlankNode::BlankNode(b.clone())),
        _ => None,
    }
}

fn label(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        other => other.to_string(),
    }
}

/// A QL subclass expression.
enum SubExpr {
    Basic(Basic),
    /// `owl:Nothing`: an inclusion from it holds trivially.
    Bottom,
    Unsupported,
}

/// One conjunct of a QL superclass expression.
enum SupAtom {
    Pos(Basic),
    Neg(Basic),
    Qualified(Role, String),
    Bottom,
}

struct Loader<'s, 'a> {
    src: &'s Src<'a>,
    ax: Axioms,
}

impl Loader<'_, '_> {
    fn is_datatype(&self, iri: &str) -> bool {
        iri.starts_with(XSD_NS)
            || matches!(
                iri,
                RDFS_LITERAL
                    | RDF_PLAIN_LITERAL
                    | RDF_LANG_STRING
                    | RDF_XML_LITERAL
                    | OWL_REAL
                    | OWL_RATIONAL
            )
            || iri == "http://www.w3.org/1999/02/22-rdf-syntax-ns#HTML"
            || iri == "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON"
            || self.ax.datatypes.contains(iri)
    }

    fn ignore(&mut self, axiom: &str, subject: &Term, reason: &str) {
        self.ax.ignored.add(axiom, &label(subject), reason);
    }

    fn fresh_role(&mut self) -> Role {
        self.ax.fresh += 1;
        Role::named(format!("{FRESH}{}", self.ax.fresh))
    }

    /// A property expression: `P`, or `[owl:inverseOf P]`.
    fn prop_expr(&self, t: &Term) -> Result<Option<Role>, ReasoningError> {
        Ok(match t {
            Term::NamedNode(n) => Some(Role::named(n.as_str())),
            Term::BlankNode(_) => match self.src.object(t, OWL_INVERSE_OF)? {
                Some(Term::NamedNode(p)) => Some(Role::named(p.as_str()).inv()),
                _ => None,
            },
            _ => None,
        })
    }

    fn sub_expr(&mut self, t: &Term) -> Result<SubExpr, ReasoningError> {
        Ok(match t {
            Term::NamedNode(n) if n.as_str() == OWL_NOTHING => SubExpr::Bottom,
            Term::NamedNode(n) if n.as_str() == OWL_THING => SubExpr::Unsupported,
            Term::NamedNode(n) => SubExpr::Basic(Basic::Class(n.as_str().to_string())),
            Term::BlankNode(_) => {
                let (Some(on), Some(filler)) = (
                    self.src.object(t, OWL_ON_PROPERTY)?,
                    self.src.object(t, OWL_SOME_VALUES_FROM)?,
                ) else {
                    return Ok(SubExpr::Unsupported);
                };
                let Some(role) = self.prop_expr(&on)? else {
                    return Ok(SubExpr::Unsupported);
                };
                match &filler {
                    Term::NamedNode(f) if f.as_str() == OWL_THING => {
                        SubExpr::Basic(Basic::Exists(role))
                    }
                    Term::NamedNode(f) if f.as_str() == RDFS_LITERAL && !role.inverse => {
                        self.ax.data_props.insert(role.iri.clone());
                        SubExpr::Basic(Basic::Exists(role))
                    }
                    _ => SubExpr::Unsupported,
                }
            }
            _ => SubExpr::Unsupported,
        })
    }

    /// Read a superclass expression into `out`; `false` when it is outside
    /// the profile.
    fn sup_expr(
        &mut self,
        t: &Term,
        out: &mut Vec<SupAtom>,
        depth: usize,
    ) -> Result<bool, ReasoningError> {
        if depth > 16 {
            return Ok(false);
        }
        match t {
            Term::NamedNode(n) if n.as_str() == OWL_THING => Ok(true),
            Term::NamedNode(n) if n.as_str() == OWL_NOTHING => {
                out.push(SupAtom::Bottom);
                Ok(true)
            }
            Term::NamedNode(n) => {
                out.push(SupAtom::Pos(Basic::Class(n.as_str().to_string())));
                Ok(true)
            }
            Term::BlankNode(_) => {
                if let Some(list) = self.src.object(t, OWL_INTERSECTION_OF)? {
                    let Some(members) = self.src.list(&list)? else {
                        return Ok(false);
                    };
                    for m in &members {
                        if !self.sup_expr(m, out, depth + 1)? {
                            return Ok(false);
                        }
                    }
                    return Ok(true);
                }
                if let Some(c) = self.src.object(t, OWL_COMPLEMENT_OF)? {
                    return Ok(match self.sub_expr(&c)? {
                        SubExpr::Basic(b) => {
                            out.push(SupAtom::Neg(b));
                            true
                        }
                        SubExpr::Bottom => true,
                        SubExpr::Unsupported => false,
                    });
                }
                let (Some(on), Some(filler)) = (
                    self.src.object(t, OWL_ON_PROPERTY)?,
                    self.src.object(t, OWL_SOME_VALUES_FROM)?,
                ) else {
                    return Ok(false);
                };
                let Some(role) = self.prop_expr(&on)? else {
                    return Ok(false);
                };
                match &filler {
                    Term::NamedNode(f) if f.as_str() == OWL_THING => {
                        out.push(SupAtom::Pos(Basic::Exists(role)));
                    }
                    Term::NamedNode(f) if f.as_str() == OWL_NOTHING => out.push(SupAtom::Bottom),
                    Term::NamedNode(f) if self.is_datatype(f.as_str()) && !role.inverse => {
                        // C ⊑ ∃U.D: C has some U value. That the value is in
                        // D adds nothing to the ground closure.
                        self.ax.data_props.insert(role.iri.clone());
                        out.push(SupAtom::Pos(Basic::Exists(role)));
                    }
                    Term::NamedNode(f) => {
                        out.push(SupAtom::Qualified(role, f.as_str().to_string()));
                    }
                    _ => return Ok(false),
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn add_sub(&mut self, sub: Basic, sups: Vec<SupAtom>) {
        for a in sups {
            match a {
                SupAtom::Pos(b) => self.ax.concept_incl.push((sub.clone(), b)),
                SupAtom::Neg(b) => self.ax.concept_ni.push((sub.clone(), b)),
                SupAtom::Bottom => self.ax.concept_ni.push((sub.clone(), sub.clone())),
                SupAtom::Qualified(r, class) => {
                    // B ⊑ ∃R.A: F ⊑ R, ∃F⁻ ⊑ A, B ⊑ ∃F for a fresh F.
                    let f = self.fresh_role();
                    self.ax.role_incl.push((f.clone(), r));
                    self.ax
                        .concept_incl
                        .push((Basic::Exists(f.inv()), Basic::Class(class)));
                    self.ax.concept_incl.push((sub.clone(), Basic::Exists(f)));
                }
            }
        }
    }

    /// `sub ⊑ sup`; `true` when it is in the profile (and was added).
    fn inclusion(&mut self, sub: &Term, sup: &Term) -> Result<bool, ReasoningError> {
        match self.sub_expr(sub)? {
            SubExpr::Bottom => Ok(true),
            SubExpr::Unsupported => Ok(false),
            SubExpr::Basic(b) => {
                let mut sups = Vec::new();
                if self.sup_expr(sup, &mut sups, 0)? {
                    self.add_sub(b, sups);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
        }
    }

    fn disjoint(&mut self, a: &Term, b: &Term) -> Result<bool, ReasoningError> {
        Ok(match (self.sub_expr(a)?, self.sub_expr(b)?) {
            (SubExpr::Unsupported, _) | (_, SubExpr::Unsupported) => false,
            (SubExpr::Basic(x), SubExpr::Basic(y)) => {
                self.ax.concept_ni.push((x, y));
                true
            }
            _ => true,
        })
    }

    fn load(mut self) -> Result<Axioms, ReasoningError> {
        let src = self.src;
        for d in src.subjects_of_type(RDFS_DATATYPE)? {
            if let NamedOrBlankNode::NamedNode(n) = d {
                self.ax.datatypes.insert(n.as_str().to_string());
            }
        }
        for p in src.subjects_of_type(OWL_DATATYPE_PROPERTY)? {
            if let NamedOrBlankNode::NamedNode(n) = p {
                self.ax.data_props.insert(n.as_str().to_string());
            }
        }
        let ranges = src.pairs(RDFS_RANGE)?;
        for (p, c) in &ranges {
            if let (NamedOrBlankNode::NamedNode(p), Term::NamedNode(c)) = (p, c) {
                if self.is_datatype(c.as_str()) {
                    self.ax.data_props.insert(p.as_str().to_string());
                }
            }
        }

        for (s, o) in src.pairs(RDFS_SUB_CLASS_OF)? {
            let s = Term::from(s);
            if !self.inclusion(&s, &o)? {
                self.ignore("rdfs:subClassOf", &s, "a class expression outside OWL 2 QL");
            }
        }
        for (s, o) in src.pairs(OWL_EQUIV_CLASS)? {
            let s = Term::from(s);
            let forward = self.inclusion(&s, &o)?;
            let backward = self.inclusion(&o, &s)?;
            if !forward && !backward {
                self.ignore(
                    "owl:equivalentClass",
                    &s,
                    "a class expression outside OWL 2 QL",
                );
            } else if !forward || !backward {
                self.ignore(
                    "owl:equivalentClass",
                    &s,
                    "only one direction is in OWL 2 QL; the other is not used",
                );
            }
        }
        for (s, o) in src.pairs(OWL_DISJOINT_WITH)? {
            let s = Term::from(s);
            if !self.disjoint(&s, &o)? {
                self.ignore(
                    "owl:disjointWith",
                    &s,
                    "a class expression outside OWL 2 QL",
                );
            }
        }
        for a in src.subjects_of_type(OWL_ALL_DISJOINT_CLASSES)? {
            let a = Term::from(a);
            let members = match src.object(&a, OWL_MEMBERS)? {
                Some(l) => src.list(&l)?,
                None => None,
            };
            let Some(members) = members else {
                self.ignore("owl:AllDisjointClasses", &a, "no member list");
                continue;
            };
            for (i, x) in members.iter().enumerate() {
                for y in &members[i + 1..] {
                    if !self.disjoint(x, y)? {
                        self.ignore(
                            "owl:AllDisjointClasses",
                            &a,
                            "a class expression outside OWL 2 QL",
                        );
                    }
                }
            }
        }

        let roles =
            |l: &Self, a: &Term, b: &Term| -> Result<Option<(Role, Role)>, ReasoningError> {
                Ok(match (l.prop_expr(a)?, l.prop_expr(b)?) {
                    (Some(x), Some(y)) => Some((x, y)),
                    _ => None,
                })
            };
        for (s, o) in src.pairs(RDFS_SUB_PROPERTY_OF)? {
            let s = Term::from(s);
            match roles(&self, &s, &o)? {
                Some((x, y)) => self.ax.role_incl.push((x, y)),
                None => self.ignore(
                    "rdfs:subPropertyOf",
                    &s,
                    "a property expression outside OWL 2 QL",
                ),
            }
        }
        for (s, o) in src.pairs(OWL_EQUIV_PROP)? {
            let s = Term::from(s);
            match roles(&self, &s, &o)? {
                Some((x, y)) => {
                    self.ax.role_incl.push((x.clone(), y.clone()));
                    self.ax.role_incl.push((y, x));
                }
                None => self.ignore(
                    "owl:equivalentProperty",
                    &s,
                    "a property expression outside OWL 2 QL",
                ),
            }
        }
        for (s, o) in src.pairs(OWL_INVERSE_OF)? {
            // `[owl:inverseOf P]` is a property expression, not an axiom.
            let NamedOrBlankNode::NamedNode(_) = &s else {
                continue;
            };
            let s = Term::from(s);
            match roles(&self, &s, &o)? {
                Some((x, y)) => {
                    self.ax.role_incl.push((x.clone(), y.inv()));
                    self.ax.role_incl.push((y.inv(), x));
                }
                None => self.ignore(
                    "owl:inverseOf",
                    &s,
                    "a property expression outside OWL 2 QL",
                ),
            }
        }
        for (s, o) in src.pairs(OWL_PROPERTY_DISJOINT_WITH)? {
            let s = Term::from(s);
            match roles(&self, &s, &o)? {
                Some((x, y)) => self.ax.role_ni.push((x, y)),
                None => self.ignore(
                    "owl:propertyDisjointWith",
                    &s,
                    "a property expression outside OWL 2 QL",
                ),
            }
        }
        for a in src.subjects_of_type(OWL_ALL_DISJOINT_PROPERTIES)? {
            let a = Term::from(a);
            let members = match src.object(&a, OWL_MEMBERS)? {
                Some(l) => src.list(&l)?,
                None => None,
            };
            let Some(members) = members else {
                self.ignore("owl:AllDisjointProperties", &a, "no member list");
                continue;
            };
            for (i, x) in members.iter().enumerate() {
                for y in &members[i + 1..] {
                    match roles(&self, x, y)? {
                        Some((x, y)) => self.ax.role_ni.push((x, y)),
                        None => self.ignore(
                            "owl:AllDisjointProperties",
                            &a,
                            "a property expression outside OWL 2 QL",
                        ),
                    }
                }
            }
        }

        for (p, c) in src.pairs(RDFS_DOMAIN)? {
            let p = Term::from(p);
            let mut sups = Vec::new();
            match self.prop_expr(&p)? {
                Some(r) if self.sup_expr(&c, &mut sups, 0)? => self.add_sub(Basic::Exists(r), sups),
                _ => self.ignore("rdfs:domain", &p, "a class expression outside OWL 2 QL"),
            }
        }
        for (p, c) in ranges {
            let p = Term::from(p);
            let Some(r) = self.prop_expr(&p)? else {
                self.ignore("rdfs:range", &p, "a property expression outside OWL 2 QL");
                continue;
            };
            if let Term::NamedNode(c) = &c {
                if self.is_datatype(c.as_str()) {
                    if c.as_str() != RDFS_LITERAL && !r.inverse {
                        self.ax.data_ranges.push((r.iri, c.as_str().to_string()));
                    }
                    continue;
                }
            }
            if self.ax.data_props.contains(&r.iri) {
                self.ignore(
                    "rdfs:range",
                    &p,
                    "a data range outside OWL 2 QL (only datatypes are checked)",
                );
                continue;
            }
            let mut sups = Vec::new();
            if self.sup_expr(&c, &mut sups, 0)? {
                self.add_sub(Basic::Exists(r.inv()), sups);
            } else {
                self.ignore("rdfs:range", &p, "a class expression outside OWL 2 QL");
            }
        }

        for p in src.subjects_of_type(OWL_SYMMETRIC)? {
            if let NamedOrBlankNode::NamedNode(p) = p {
                let r = Role::named(p.as_str());
                self.ax.role_incl.push((r.clone(), r.inv()));
            }
        }
        for p in src.subjects_of_type(OWL_ASYMMETRIC)? {
            if let NamedOrBlankNode::NamedNode(p) = p {
                let r = Role::named(p.as_str());
                self.ax.role_ni.push((r.clone(), r.inv()));
            }
        }
        for p in src.subjects_of_type(OWL_REFLEXIVE)? {
            if let NamedOrBlankNode::NamedNode(p) = p {
                self.ax.reflexive.push(p.as_str().to_string());
            }
        }
        for p in src.subjects_of_type(OWL_IRREFLEXIVE)? {
            if let NamedOrBlankNode::NamedNode(p) = p {
                self.ax.irreflexive.push(p.as_str().to_string());
            }
        }

        for &(iri, is_type) in NON_PROFILE {
            let name = format!("owl:{}", iri.trim_start_matches(OWL_NS));
            let subjects: Vec<NamedOrBlankNode> = if is_type {
                src.subjects_of_type(iri)?
            } else {
                src.pairs(iri)?.into_iter().map(|(s, _)| s).collect()
            };
            for s in subjects {
                self.ignore(&name, &Term::from(s), "outside OWL 2 QL");
            }
        }
        Ok(self.ax)
    }
}

impl QlTBox {
    fn load(src: &Src<'_>) -> Result<Self, ReasoningError> {
        let ax = Loader {
            src,
            ax: Axioms::default(),
        }
        .load()?;
        Ok(Self::build(ax))
    }
}

// ─── Datatypes (until the OWL 2 datatype map lands) ─────────────────────────

const XSD_DECIMAL: &str = "http://www.w3.org/2001/XMLSchema#decimal";
const XSD_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#integer";
const XSD_NON_NEGATIVE_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#nonNegativeInteger";
const XSD_NON_POSITIVE_INTEGER: &str = "http://www.w3.org/2001/XMLSchema#nonPositiveInteger";
const XSD_LONG: &str = "http://www.w3.org/2001/XMLSchema#long";
const XSD_INT: &str = "http://www.w3.org/2001/XMLSchema#int";
const XSD_SHORT: &str = "http://www.w3.org/2001/XMLSchema#short";
const XSD_UNSIGNED_LONG: &str = "http://www.w3.org/2001/XMLSchema#unsignedLong";
const XSD_UNSIGNED_INT: &str = "http://www.w3.org/2001/XMLSchema#unsignedInt";
const XSD_UNSIGNED_SHORT: &str = "http://www.w3.org/2001/XMLSchema#unsignedShort";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const XSD_NORMALIZED_STRING: &str = "http://www.w3.org/2001/XMLSchema#normalizedString";
const XSD_TOKEN: &str = "http://www.w3.org/2001/XMLSchema#token";
const XSD_NAME: &str = "http://www.w3.org/2001/XMLSchema#Name";
const XSD_DATE_TIME: &str = "http://www.w3.org/2001/XMLSchema#dateTime";

/// The datatype directly above `dt` (XSD, `owl:real`/`rational`,
/// `rdf:PlainLiteral`) and the primitive value space it belongs to. Value
/// spaces of different families are disjoint; `None` for a datatype outside
/// the table.
fn datatype_parent(dt: &str) -> Option<(Option<&'static str>, &'static str)> {
    const NUM: &str = "numeric";
    const PLAIN: &str = "plain";
    if let Some(local) = dt.strip_prefix(XSD_NS) {
        return Some(match local {
            "decimal" => (Some(OWL_RATIONAL), NUM),
            "integer" => (Some(XSD_DECIMAL), NUM),
            "nonNegativeInteger" | "nonPositiveInteger" | "long" => (Some(XSD_INTEGER), NUM),
            "positiveInteger" | "unsignedLong" => (Some(XSD_NON_NEGATIVE_INTEGER), NUM),
            "negativeInteger" => (Some(XSD_NON_POSITIVE_INTEGER), NUM),
            "int" => (Some(XSD_LONG), NUM),
            "short" => (Some(XSD_INT), NUM),
            "byte" => (Some(XSD_SHORT), NUM),
            "unsignedInt" => (Some(XSD_UNSIGNED_LONG), NUM),
            "unsignedShort" => (Some(XSD_UNSIGNED_INT), NUM),
            "unsignedByte" => (Some(XSD_UNSIGNED_SHORT), NUM),
            "string" => (Some(RDF_PLAIN_LITERAL), PLAIN),
            "normalizedString" => (Some(XSD_STRING), PLAIN),
            "token" => (Some(XSD_NORMALIZED_STRING), PLAIN),
            "language" | "NMTOKEN" | "Name" => (Some(XSD_TOKEN), PLAIN),
            "NCName" => (Some(XSD_NAME), PLAIN),
            "dateTime" => (None, "dateTime"),
            "dateTimeStamp" => (Some(XSD_DATE_TIME), "dateTime"),
            "boolean" => (None, "boolean"),
            "double" => (None, "double"),
            "float" => (None, "float"),
            "hexBinary" => (None, "hexBinary"),
            "base64Binary" => (None, "base64Binary"),
            "anyURI" => (None, "anyURI"),
            _ => return None,
        });
    }
    Some(match dt {
        OWL_REAL => (None, NUM),
        OWL_RATIONAL => (Some(OWL_REAL), NUM),
        RDF_PLAIN_LITERAL => (None, PLAIN),
        RDF_LANG_STRING => (Some(RDF_PLAIN_LITERAL), PLAIN),
        RDF_XML_LITERAL => (None, "XMLLiteral"),
        _ => return None,
    })
}

/// Whether `lit` is in datatype `range`: `Some(false)` only when their value
/// spaces are disjoint, `None` when telling needs the literal's value (an
/// `xsd:integer` against `xsd:nonNegativeInteger`) or a datatype is unknown.
fn literal_in_range(lit: &Literal, range: &str) -> Option<bool> {
    if range == RDFS_LITERAL {
        return Some(true);
    }
    let dt = lit.datatype().as_str();
    let (_, range_family) = datatype_parent(range)?;
    let (_, dt_family) = datatype_parent(dt)?;
    if dt_family != range_family {
        return Some(false);
    }
    let mut cur = Some(dt.to_string());
    while let Some(c) = cur {
        if c == range {
            return Some(true);
        }
        cur = datatype_parent(&c).and_then(|(p, _)| p.map(str::to_string));
    }
    // A plain string is never a language-tagged one, nor the reverse.
    if range == RDF_LANG_STRING || dt == RDF_LANG_STRING {
        return Some(false);
    }
    None
}

// ─── The TBox cache for query-time rewriting ────────────────────────────────

type CacheKey = (u64, u64, Option<Vec<String>>);

fn tbox_cache() -> &'static Mutex<HashMap<CacheKey, Arc<QlTBox>>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<QlTBox>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The closed TBox of `scope` (the unnamed default graph when `None`), cached
/// per store and write generation.
fn cached_tbox(
    store: &TripleStore,
    scope: Option<Vec<String>>,
) -> Result<Arc<QlTBox>, ReasoningError> {
    let key = (store.instance_id(), store.write_generation(), scope);
    if let Ok(cache) = tbox_cache().lock() {
        if let Some(t) = cache.get(&key) {
            return Ok(t.clone());
        }
    }
    let src = Src {
        store: store.store(),
        graphs: graph_names(key.2.as_deref())?,
    };
    let tbox = Arc::new(QlTBox::load(&src)?);
    if let Ok(mut cache) = tbox_cache().lock() {
        // A write moves the generation on; entries for older ones are dead.
        cache.retain(|k, _| k.0 != key.0 || k.1 == key.1);
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(key, tbox.clone());
    }
    Ok(tbox)
}

fn graph_names(scope: Option<&[String]>) -> Result<Vec<GraphName>, ReasoningError> {
    match scope {
        None => Ok(vec![GraphName::DefaultGraph]),
        Some(graphs) => graphs
            .iter()
            .map(|g| {
                NamedNode::new(g.as_str())
                    .map(GraphName::NamedNode)
                    .map_err(|e| ReasoningError::Query(format!("graph <{g}>: {e}")))
            })
            .collect(),
    }
}

/// Rewrite the blank nodes of a query for OWL 2 QL entailment over a
/// materialised graph: `Ok(None)` when it has none (the query runs as is).
///
/// This is the `?entailment=owl2-ql` path of `/sparql`. The TBox is read
/// from the graphs the query reads (its `FROM` clauses, else the unnamed
/// default graph) and cached until the next write. Only basic graph
/// patterns with blank nodes change: each connected part with blank nodes
/// becomes the union of its matches over named individuals and over the
/// anonymous elements the TBox's existentials imply, with one solution per
/// binding of its variables.
pub fn rewrite_existentials(
    store: &TripleStore,
    sparql: &str,
) -> Result<Option<String>, ReasoningError> {
    if !sparql.contains("_:") && !sparql.contains('[') {
        return Ok(None);
    }
    // A query that does not parse fails on its own, with the store's error.
    let Ok(query) = crate::sparql::parser().parse_query(sparql) else {
        return Ok(None);
    };
    let dataset = match &query {
        Query::Select { dataset, .. }
        | Query::Ask { dataset, .. }
        | Query::Construct { dataset, .. }
        | Query::Describe { dataset, .. } => dataset.clone(),
    };
    let scope = dataset
        .map(|d| {
            let mut g: Vec<String> = d.default.iter().map(|n| n.as_str().to_string()).collect();
            g.sort();
            g.dedup();
            g
        })
        .filter(|g| !g.is_empty());
    let tbox = cached_tbox(store, scope)?;
    let mut rw = Rewriter::new(&tbox, Fresh::for_query(sparql), Mode::Existential);
    let rewritten = rw.query(query);
    Ok(rw.changed.then(|| rewritten.to_string()))
}

// ─── Materialisation and the standalone rewriter ────────────────────────────

fn inconsistency(rule: &str, detail: String) -> ReasoningError {
    ReasoningError::inconsistency(rule, detail)
}

/// OWL 2 QL reasoner: materialisation, consistency and query rewriting.
pub struct QLQueryRewriter<'a> {
    store: &'a TripleStore,
    pub target_graph: String,
    /// When set, the TBox and data are read from ONLY these graphs. Without
    /// it, from the unnamed default graph.
    sources: Option<Vec<String>>,
}

impl<'a> QLQueryRewriter<'a> {
    pub fn new(store: &'a TripleStore) -> Self {
        Self {
            store,
            target_graph: OWL2_QL_ENTAILMENT_GRAPH.to_string(),
            sources: None,
        }
    }

    /// Read the TBox and data from `sources` only. Without a scope they are
    /// read from the unnamed default graph. `POST /api/reasoning/materialize`
    /// sets this from `source_graphs` or the dataset's conformance layer, and
    /// `POST /api/reasoning/rewrite` from the graphs the caller may read.
    ///
    /// Unlike the rule-based reasoners, the target graph is not added: the
    /// closure is computed in memory and never reads its own output.
    pub fn with_sources(mut self, sources: Vec<String>) -> Self {
        self.sources = Some(sources);
        self
    }

    pub fn with_target(mut self, graph: impl Into<String>) -> Self {
        self.target_graph = graph.into();
        self
    }

    fn src(&self) -> Result<Src<'a>, ReasoningError> {
        Ok(Src {
            store: self.store.store(),
            graphs: graph_names(self.sources.as_deref())?,
        })
    }

    fn target(&self) -> Result<NamedNode, ReasoningError> {
        NamedNode::new(self.target_graph.as_str())
            .map_err(|e| ReasoningError::Query(format!("target <{}>: {e}", self.target_graph)))
    }

    /// Rewrite a SPARQL SELECT/ASK/CONSTRUCT query over the TBox, so that it
    /// answers under OWL 2 QL entailment over the asserted data alone: every
    /// atom is expanded through the class and property hierarchies, and the
    /// parts with blank nodes are rewritten existentially (see
    /// [`rewrite_existentials`]). Duplicate solutions are possible where an
    /// individual is in a class for more than one reason.
    pub fn rewrite_query(&self, sparql: &str) -> Result<String, ReasoningError> {
        let tbox = QlTBox::load(&self.src()?)?;
        let query = crate::sparql::parser()
            .parse_query(sparql)
            .map_err(|e| ReasoningError::Query(format!("SPARQL parse error: {e}")))?;
        let mut rw = Rewriter::new(&tbox, Fresh::for_query(sparql), Mode::Full);
        let result = rw.query(query).to_string();
        debug!("OWL-QL rewritten query ({} chars)", result.len());
        Ok(result)
    }

    /// Write the named-class and named-property closure (and
    /// `A rdfs:subClassOf owl:Nothing` for every unsatisfiable class) into
    /// the target graph.
    // Library API: the server materialises with `materialize`, so the
    // binary, which compiles this module too, never calls it.
    #[allow(dead_code)]
    pub fn materialize_tbox(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        let src = self.src()?;
        let tbox = QlTBox::load(&src)?;
        let target = self.target()?;
        let mut out: HashSet<Quad> = HashSet::new();
        tbox_closure(&tbox, &target, &mut out);
        let added = self.write(out)?;
        Ok(self.report(&tbox, added, start))
    }

    /// Materialise OWL 2 QL into the target graph: the TBox closure and every
    /// entailed ground atom over the individuals in the data, then check
    /// consistency. An inconsistency is an error; what was derived stays in
    /// the target graph.
    pub fn materialize(&self) -> Result<ReasoningReport, ReasoningError> {
        let start = Instant::now();
        let src = self.src()?;
        let tbox = QlTBox::load(&src)?;
        let target = self.target()?;
        let mut out: HashSet<Quad> = HashSet::new();
        tbox_closure(&tbox, &target, &mut out);
        let clash = ground(&tbox, &src, &target, &mut out)?;
        let added = self.write(out)?;
        if let Some((rule, detail)) = tbox.clash.clone().or(clash) {
            return Err(inconsistency(&rule, detail));
        }
        Ok(self.report(&tbox, added, start))
    }

    /// Insert what is not in the target graph already; the number inserted.
    fn write(&self, out: HashSet<Quad>) -> Result<usize, ReasoningError> {
        let new: Vec<Quad> = out
            .into_iter()
            .filter(|q| !self.store.store().contains(q).unwrap_or(false))
            .collect();
        let n = new.len();
        if n > 0 {
            self.store.insert_quads(new)?;
        }
        Ok(n)
    }

    fn report(&self, tbox: &QlTBox, added: usize, start: Instant) -> ReasoningReport {
        ReasoningReport {
            regime: "owl2-ql".to_string(),
            triples_added: added,
            iterations: 1,
            elapsed_ms: start.elapsed().as_millis() as u64,
            target_graph: self.target_graph.clone(),
            ignored_axioms: tbox.ignored.count(),
            ignored_sample: tbox.ignored.sample.clone(),
        }
    }
}

/// `A ⊑ B` between named classes and `P ⊑ Q` between named properties, for
/// every pair the closure entails; `A ⊑ owl:Nothing` for empty classes.
fn tbox_closure(tbox: &QlTBox, target: &NamedNode, out: &mut HashSet<Quad>) {
    let sub_class_of = NamedNode::new_unchecked(RDFS_SUB_CLASS_OF);
    let sub_property_of = NamedNode::new_unchecked(RDFS_SUB_PROPERTY_OF);
    for (b, basic) in tbox.basics.iter().enumerate() {
        let Basic::Class(a) = basic else { continue };
        if a == OWL_THING || a == OWL_NOTHING {
            continue;
        }
        let a = NamedNode::new_unchecked(a.as_str());
        if tbox.unsat[b] {
            out.insert(Quad::new(
                a.clone(),
                sub_class_of.clone(),
                NamedNode::new_unchecked(OWL_NOTHING),
                target.clone(),
            ));
        }
        for &s in &tbox.sup[b] {
            if let Basic::Class(c) = &tbox.basics[s] {
                if s != b && c != OWL_THING && c != a.as_str() {
                    out.insert(Quad::new(
                        a.clone(),
                        sub_class_of.clone(),
                        NamedNode::new_unchecked(c.as_str()),
                        target.clone(),
                    ));
                }
            }
        }
    }
    for (r, role) in tbox.roles.iter().enumerate() {
        if role.inverse || role.is_fresh() {
            continue;
        }
        for &q in &tbox.role_sup[r] {
            let sup = &tbox.roles[q];
            if !sup.inverse && !sup.is_fresh() && sup.iri != role.iri {
                out.insert(Quad::new(
                    NamedNode::new_unchecked(role.iri.as_str()),
                    sub_property_of.clone(),
                    NamedNode::new_unchecked(sup.iri.as_str()),
                    target.clone(),
                ));
            }
        }
    }
}

/// Keep the first inconsistency found.
fn note(clash: &mut Option<(String, String)>, rule: &str, detail: String) {
    if clash.is_none() {
        *clash = Some((rule.to_string(), detail));
    }
}

fn reserved(iri: &str) -> bool {
    iri.starts_with(RDF_NS) || iri.starts_with(RDFS_NS) || iri.starts_with(OWL_NS)
}

/// Every entailed ground atom over the individuals of `src` that is not
/// asserted there, into `out`; the first inconsistency found, if any.
fn ground(
    tbox: &QlTBox,
    src: &Src<'_>,
    target: &NamedNode,
    out: &mut HashSet<Quad>,
) -> Result<Option<(String, String)>, ReasoningError> {
    let rdf_type = NamedNode::new_unchecked(RDF_TYPE);
    let mut clash: Option<(String, String)> = None;
    // Individual → the basic concepts it is asserted in.
    let mut members: HashMap<Term, Vec<usize>> = HashMap::new();
    // (subject, object) → the positive roles asserted between them, for
    // the properties whose closure says more than the assertion.
    let mut pairs: HashMap<(Term, Term), Vec<usize>> = HashMap::new();

    for (b, basic) in tbox.basics.iter().enumerate() {
        let Basic::Class(c) = basic else { continue };
        if c == OWL_THING {
            continue;
        }
        src.each(
            None,
            Some(rdf_type.as_ref()),
            Some(NamedNodeRef::new_unchecked(c.as_str()).into()),
            |q| members.entry(q.subject.into()).or_default().push(b),
        )?;
    }

    for (r, role) in tbox.roles.iter().enumerate() {
        if role.inverse || role.is_fresh() {
            continue;
        }
        let sups = &tbox.role_sup[r];
        let need_pairs = sups.len() > 1
            || tbox.role_unsat[r]
            || sups
                .iter()
                .any(|&s| !tbox.role_ni[s].is_empty() || tbox.irreflexive.contains(&s));
        let ranges: Vec<&String> = sups
            .iter()
            .filter_map(|s| tbox.data_ranges.get(s))
            .flatten()
            .collect();
        let pred = NamedNode::new_unchecked(role.iri.as_str());
        let mut literal_sups: Vec<(Term, NamedNode, Term)> = Vec::new();
        src.each(None, Some(pred.as_ref()), None, |q| {
            let s: Term = q.subject.into();
            members.entry(s.clone()).or_default().push(tbox.exists[r]);
            match q.object {
                Term::Literal(l) => {
                    for range in &ranges {
                        if literal_in_range(&l, range) == Some(false) {
                            note(
                                &mut clash,
                                "ql-dt-range",
                                format!(
                                    "{} is not a value of <{range}>, the range of <{}>",
                                    l, role.iri
                                ),
                            );
                        }
                    }
                    for &q2 in sups {
                        let sup = &tbox.roles[q2];
                        if q2 != r && !sup.inverse && !sup.is_fresh() {
                            literal_sups.push((
                                s.clone(),
                                NamedNode::new_unchecked(sup.iri.as_str()),
                                Term::Literal(l.clone()),
                            ));
                        }
                    }
                    if need_pairs {
                        pairs.entry((s, Term::Literal(l))).or_default().push(r);
                    }
                }
                o @ (Term::NamedNode(_) | Term::BlankNode(_)) => {
                    members
                        .entry(o.clone())
                        .or_default()
                        .push(tbox.exists[tbox.inv[r]]);
                    if need_pairs {
                        pairs.entry((s, o)).or_default().push(r);
                    }
                }
                #[allow(unreachable_patterns)]
                _ => {}
            }
        })?;
        for (s, p, o) in literal_sups {
            if !src.has(&s, p.as_str(), &o)? {
                if let Some(s) = node(&s) {
                    out.insert(Quad::new(s, p, o, target.clone()));
                }
            }
        }
    }

    // Reflexive properties: every individual has the loop.
    let mut individuals: HashSet<Term> = HashSet::new();
    if !tbox.reflexive.is_empty() {
        src.each(None, None, None, |q| {
            let p = q.predicate.as_str();
            if p == RDF_TYPE {
                if let Term::NamedNode(c) = &q.object {
                    if !reserved(c.as_str()) {
                        individuals.insert(q.subject.into());
                    }
                }
            } else if !reserved(p) {
                if !matches!(q.object, Term::Literal(_)) {
                    individuals.insert(q.object);
                }
                individuals.insert(q.subject.into());
            }
        })?;
        for x in &individuals {
            members.entry(x.clone()).or_default();
        }
    }

    // Each individual: everything above what it is asserted in.
    let mut stamp: Vec<usize> = vec![usize::MAX; tbox.basics.len()];
    for (i, (x, asserted)) in members.iter().enumerate() {
        let mut set: Vec<usize> = Vec::new();
        for &a in asserted.iter().chain(tbox.top.iter()) {
            for &s in &tbox.sup[a] {
                if stamp[s] != i {
                    stamp[s] = i;
                    set.push(s);
                }
            }
        }
        for &b in &set {
            if tbox.unsat[b] {
                note(
                    &mut clash,
                    "ql-cls-nothing",
                    format!("{} is in {}, which is unsatisfiable", x, tbox.show(b)),
                );
            }
            for &c in &tbox.ni[b] {
                if stamp[c] == i {
                    note(
                        &mut clash,
                        "ql-cls-disjoint",
                        format!(
                            "{} is in both {} and {}, declared disjoint",
                            x,
                            tbox.show(b),
                            tbox.show(c)
                        ),
                    );
                }
            }
        }
        let Some(subject) = node(x) else { continue };
        for &b in &set {
            if let Basic::Class(c) = &tbox.basics[b] {
                if c != OWL_THING && !asserted.contains(&b) {
                    out.insert(Quad::new(
                        subject.clone(),
                        rdf_type.clone(),
                        NamedNode::new_unchecked(c.as_str()),
                        target.clone(),
                    ));
                }
            }
        }
    }

    // Each pair: every role above what is asserted (or, for a loop, a
    // reflexive property), read forwards.
    let mut holds: HashMap<(Term, Term), Vec<usize>> = HashMap::new();
    let loops = individuals
        .iter()
        .map(|x| ((x.clone(), x.clone()), &tbox.reflexive));
    for ((s, o), rs) in pairs.iter().map(|(k, v)| (k.clone(), v)).chain(loops) {
        for &r in rs {
            for &q in &tbox.role_sup[r] {
                if tbox.roles[q].inverse {
                    holds
                        .entry((o.clone(), s.clone()))
                        .or_default()
                        .push(tbox.inv[q]);
                } else {
                    holds.entry((s.clone(), o.clone())).or_default().push(q);
                }
            }
        }
    }
    for v in holds.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    for ((x, y), qs) in &holds {
        for &q in qs {
            if tbox.role_unsat[q] {
                note(
                    &mut clash,
                    "ql-prp-nothing",
                    format!("{x} {} {y}, an unsatisfiable property", tbox.show_role(q)),
                );
            }
            if x == y && tbox.irreflexive.contains(&q) {
                note(
                    &mut clash,
                    "ql-prp-irp",
                    format!("{x} {} {x}, an irreflexive property", tbox.show_role(q)),
                );
            }
            for &p in &tbox.role_ni[q] {
                let (pp, key) = if tbox.roles[p].inverse {
                    (tbox.inv[p], (y.clone(), x.clone()))
                } else {
                    (p, (x.clone(), y.clone()))
                };
                if holds
                    .get(&key)
                    .is_some_and(|v| v.binary_search(&pp).is_ok())
                {
                    note(
                        &mut clash,
                        "ql-prp-disjoint",
                        format!(
                            "{x} {} {y} and {}, declared disjoint",
                            tbox.show_role(q),
                            tbox.show_role(p)
                        ),
                    );
                }
            }
        }
        let Some(subject) = node(x) else { continue };
        let asserted = pairs.get(&(x.clone(), y.clone()));
        for &q in qs {
            let role = &tbox.roles[q];
            if role.is_fresh() || asserted.is_some_and(|v| v.contains(&q)) {
                continue;
            }
            if !src.has(x, &role.iri, y)? {
                out.insert(Quad::new(
                    subject.clone(),
                    NamedNode::new_unchecked(role.iri.as_str()),
                    y.clone(),
                    target.clone(),
                ));
            }
        }
    }

    src.each(
        None,
        Some(NamedNodeRef::new_unchecked(OWL_DIFFERENT_FROM)),
        None,
        |q| {
            let s: Term = q.subject.into();
            if s == q.object {
                note(
                    &mut clash,
                    "ql-different-from",
                    format!("{s} owl:differentFrom itself"),
                );
            }
        },
    )?;
    Ok(clash)
}

// ─── Query rewriting ────────────────────────────────────────────────────────

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
        TermPattern::Variable(self.variable())
    }

    fn variable(&mut self) -> Variable {
        self.next += 1;
        Variable::new(format!("{}{}", self.prefix, self.next))
            .expect("a fresh variable name is valid")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Expand every atom through the hierarchies (no materialised graph).
    Full,
    /// Only rewrite blank nodes (the ground closure is materialised).
    Existential,
}

/// One end of an atom inside a group of anonymous blank nodes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    /// The named element the group hangs from.
    Root,
    /// A blank node of the group (index into the group).
    Node(usize),
}

enum GroupAtom {
    Class(usize, String),
    Role(End, String, End),
}

/// One step of a placement order: a blank node, and the atom linking it to
/// one placed before it (the atom, the placed end, whether the atom reads
/// from that end to this node) — `None` for the first node.
type Step = (usize, Option<(usize, End, bool)>);

/// What a group of anonymous blank nodes needs of the named individuals.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Cond {
    /// The root is in `∃R` for each role.
    Rooted(BTreeSet<usize>),
    /// Somewhere, an `R`-successor exists.
    Anywhere(usize),
}

struct Rewriter<'t> {
    tbox: &'t QlTBox,
    fresh: Fresh,
    mode: Mode,
    /// Whether anything was rewritten.
    changed: bool,
}

fn type_pattern(subject: TermPattern, class: &str) -> TriplePattern {
    TriplePattern {
        subject,
        predicate: NamedNodePattern::NamedNode(oxrdf::NamedNode::new_unchecked(RDF_TYPE)),
        object: TermPattern::NamedNode(oxrdf::NamedNode::new_unchecked(class)),
    }
}

fn bgp(patterns: Vec<TriplePattern>) -> GraphPattern {
    GraphPattern::Bgp { patterns }
}

fn join(parts: Vec<GraphPattern>) -> GraphPattern {
    parts
        .into_iter()
        .reduce(|a, b| GraphPattern::Join {
            left: Box::new(a),
            right: Box::new(b),
        })
        .unwrap_or(bgp(vec![]))
}

/// `None` for no alternatives (nothing matches).
fn union(parts: Vec<GraphPattern>) -> Option<GraphPattern> {
    parts.into_iter().reduce(|a, b| GraphPattern::Union {
        left: Box::new(a),
        right: Box::new(b),
    })
}

fn bnode_of(t: &TermPattern) -> Option<&oxrdf::BlankNode> {
    match t {
        TermPattern::BlankNode(b) => Some(b),
        _ => None,
    }
}

impl<'t> Rewriter<'t> {
    fn new(tbox: &'t QlTBox, fresh: Fresh, mode: Mode) -> Self {
        Self {
            tbox,
            fresh,
            mode,
            changed: false,
        }
    }

    fn query(&mut self, query: Query) -> Query {
        match query {
            Query::Select {
                dataset,
                pattern,
                base_iri,
            } => Query::Select {
                dataset,
                pattern: self.pattern(pattern),
                base_iri,
            },
            Query::Ask {
                dataset,
                pattern,
                base_iri,
            } => Query::Ask {
                dataset,
                pattern: self.pattern(pattern),
                base_iri,
            },
            Query::Construct {
                template,
                dataset,
                pattern,
                base_iri,
            } => Query::Construct {
                template,
                dataset,
                pattern: self.pattern(pattern),
                base_iri,
            },
            other => other,
        }
    }

    fn boxed(&mut self, p: GraphPattern) -> Box<GraphPattern> {
        Box::new(self.pattern(p))
    }

    fn pattern(&mut self, pattern: GraphPattern) -> GraphPattern {
        match pattern {
            GraphPattern::Bgp { patterns } => self.bgp(patterns),
            GraphPattern::Join { left, right } => GraphPattern::Join {
                left: self.boxed(*left),
                right: self.boxed(*right),
            },
            GraphPattern::LeftJoin {
                left,
                right,
                expression,
            } => GraphPattern::LeftJoin {
                left: self.boxed(*left),
                right: self.boxed(*right),
                expression,
            },
            GraphPattern::Lateral { left, right } => GraphPattern::Lateral {
                left: self.boxed(*left),
                right: self.boxed(*right),
            },
            GraphPattern::Filter { expr, inner } => GraphPattern::Filter {
                expr,
                inner: self.boxed(*inner),
            },
            GraphPattern::Union { left, right } => GraphPattern::Union {
                left: self.boxed(*left),
                right: self.boxed(*right),
            },
            GraphPattern::Graph { name, inner } => GraphPattern::Graph {
                name,
                inner: self.boxed(*inner),
            },
            GraphPattern::Extend {
                inner,
                variable,
                expression,
            } => GraphPattern::Extend {
                inner: self.boxed(*inner),
                variable,
                expression,
            },
            GraphPattern::Minus { left, right } => GraphPattern::Minus {
                left: self.boxed(*left),
                right: self.boxed(*right),
            },
            GraphPattern::OrderBy { inner, expression } => GraphPattern::OrderBy {
                inner: self.boxed(*inner),
                expression,
            },
            GraphPattern::Project { inner, variables } => GraphPattern::Project {
                inner: self.boxed(*inner),
                variables,
            },
            GraphPattern::Distinct { inner } => GraphPattern::Distinct {
                inner: self.boxed(*inner),
            },
            GraphPattern::Reduced { inner } => GraphPattern::Reduced {
                inner: self.boxed(*inner),
            },
            GraphPattern::Slice {
                inner,
                start,
                length,
            } => GraphPattern::Slice {
                inner: self.boxed(*inner),
                start,
                length,
            },
            GraphPattern::Group {
                inner,
                variables,
                aggregates,
            } => GraphPattern::Group {
                inner: self.boxed(*inner),
                variables,
                aggregates,
            },
            other => other,
        }
    }

    fn bgp(&mut self, patterns: Vec<TriplePattern>) -> GraphPattern {
        let (with, without): (Vec<_>, Vec<_>) = patterns
            .into_iter()
            .partition(|tp| bnode_of(&tp.subject).is_some() || bnode_of(&tp.object).is_some());
        if with.is_empty() {
            return self.ground(without);
        }
        let mut parts = Vec::new();
        if !without.is_empty() {
            parts.push(self.ground(without));
        }
        for component in components(with) {
            parts.push(self.component(component));
        }
        join(parts)
    }

    /// Atoms that bind only to names: as written over a materialised graph,
    /// else each expanded through the hierarchies.
    fn ground(&mut self, patterns: Vec<TriplePattern>) -> GraphPattern {
        if self.mode == Mode::Existential {
            return bgp(patterns);
        }
        let parts: Vec<GraphPattern> = patterns
            .iter()
            .map(|tp| {
                let alts: Vec<GraphPattern> = self
                    .expand(tp)
                    .into_iter()
                    .map(|tp| bgp(vec![tp]))
                    .collect();
                union(alts).unwrap_or(bgp(vec![]))
            })
            .collect();
        join(parts)
    }

    /// Every rewriting of one ground atom under the TBox (itself first).
    fn expand(&mut self, tp: &TriplePattern) -> Vec<TriplePattern> {
        let mut results: Vec<TriplePattern> = vec![tp.clone()];
        let tbox = self.tbox;
        match &tp.predicate {
            NamedNodePattern::NamedNode(pred) if pred.as_str() == RDF_TYPE => {
                if let TermPattern::NamedNode(class) = &tp.object {
                    if let Some(c) = tbox.class_id(class.as_str()) {
                        for &b in &tbox.sub[c] {
                            if b != c {
                                if let Some(tp) = self.membership(&tp.subject, b) {
                                    results.push(tp);
                                }
                            }
                        }
                    }
                }
            }
            NamedNodePattern::NamedNode(pred) => {
                if let Some(p) = tbox.property_id(pred.as_str()) {
                    for &r in &tbox.role_sub[p] {
                        if r != p && !tbox.roles[r].is_fresh() {
                            results.push(role_pattern(&tp.subject, &tbox.roles[r], &tp.object));
                        }
                    }
                }
            }
            NamedNodePattern::Variable(_) => {}
        }
        let mut seen = HashSet::new();
        results.retain(|tp| seen.insert(tp.clone()));
        results
    }

    /// The atom for `subject` being in basic concept `b`, if it has one over
    /// named data (`∃` of a fresh role has none).
    fn membership(&mut self, subject: &TermPattern, b: usize) -> Option<TriplePattern> {
        match &self.tbox.basics[b] {
            Basic::Class(c) if c == OWL_THING => None,
            Basic::Class(c) => Some(type_pattern(subject.clone(), c)),
            Basic::Exists(r) if r.is_fresh() => None,
            Basic::Exists(r) => {
                let f = self.fresh.var();
                Some(role_pattern(subject, r, &f))
            }
        }
    }

    /// `subject ∈ ∃R`: the union of every basic concept under `∃R`.
    fn in_exists(&mut self, subject: &TermPattern, r: usize) -> Option<GraphPattern> {
        let subs = self.tbox.sub[self.tbox.exists[r]].clone();
        let alts: Vec<GraphPattern> = subs
            .into_iter()
            .filter_map(|b| self.membership(subject, b))
            .map(|tp| bgp(vec![tp]))
            .collect();
        union(alts)
    }

    /// One connected part of a BGP joined through blank nodes: the union of
    /// its matches for every choice of which blank nodes are anonymous,
    /// one solution per binding of its variables.
    fn component(&mut self, atoms: Vec<TriplePattern>) -> GraphPattern {
        let mut bnodes: Vec<oxrdf::BlankNode> = Vec::new();
        for tp in &atoms {
            for t in [&tp.subject, &tp.object] {
                if let Some(b) = bnode_of(t) {
                    if !bnodes.contains(b) {
                        bnodes.push(b.clone());
                    }
                }
            }
        }
        let supported = bnodes.len() <= MAX_BNODES
            && atoms.iter().all(|tp| match &tp.predicate {
                NamedNodePattern::Variable(_) => false,
                NamedNodePattern::NamedNode(p) => {
                    let simple = |t: &TermPattern| {
                        matches!(
                            t,
                            TermPattern::NamedNode(_)
                                | TermPattern::BlankNode(_)
                                | TermPattern::Variable(_)
                                | TermPattern::Literal(_)
                        )
                    };
                    simple(&tp.subject)
                        && if p.as_str() == RDF_TYPE {
                            matches!(tp.object, TermPattern::NamedNode(_))
                        } else {
                            simple(&tp.object)
                        }
                }
            });
        if !supported {
            return bgp(atoms);
        }
        self.changed = true;
        let mut vars: Vec<Variable> = Vec::new();
        for tp in &atoms {
            for t in [&tp.subject, &tp.object] {
                if let TermPattern::Variable(v) = t {
                    if !vars.contains(v) {
                        vars.push(v.clone());
                    }
                }
            }
        }
        // Named blank nodes become variables the projection drops.
        let names: HashMap<oxrdf::BlankNode, TermPattern> = bnodes
            .iter()
            .map(|b| (b.clone(), self.fresh.var()))
            .collect();

        let mut branches: Vec<GraphPattern> = Vec::new();
        for mask in 0u32..(1u32 << bnodes.len()) {
            match self.branches(&atoms, &bnodes, mask, &names) {
                Some(mut b) => branches.append(&mut b),
                // Too hard to rewrite: answer over names only (sound).
                None => {
                    branches.clear();
                    let renamed = atoms.iter().map(|tp| rename(tp, &names)).collect();
                    branches.push(self.ground(renamed));
                    break;
                }
            }
            if branches.len() > MAX_BRANCHES {
                branches.clear();
                let renamed = atoms.iter().map(|tp| rename(tp, &names)).collect();
                branches.push(self.ground(renamed));
                break;
            }
        }
        let inner = union(branches).unwrap_or_else(|| {
            // Nothing can match: an empty VALUES.
            GraphPattern::Values {
                variables: vec![],
                bindings: vec![],
            }
        });
        if vars.is_empty() {
            // Nothing to bind: whether it matches at all.
            return GraphPattern::Filter {
                expr: Expression::Exists(Box::new(inner)),
                inner: Box::new(bgp(vec![])),
            };
        }
        GraphPattern::Distinct {
            inner: Box::new(GraphPattern::Project {
                inner: Box::new(inner),
                variables: vars,
            }),
        }
    }

    /// The branches for one choice of anonymous blank nodes (`mask`); `None`
    /// when the search gave up.
    fn branches(
        &mut self,
        atoms: &[TriplePattern],
        bnodes: &[oxrdf::BlankNode],
        mask: u32,
        names: &HashMap<oxrdf::BlankNode, TermPattern>,
    ) -> Option<Vec<GraphPattern>> {
        let anon_ix = |t: &TermPattern| -> Option<usize> {
            let b = bnode_of(t)?;
            let i = bnodes.iter().position(|x| x == b)?;
            (mask & (1 << i) != 0).then_some(i)
        };
        let n = bnodes.len();
        // Groups: anonymous blank nodes joined by an atom.
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut [usize], i: usize) -> usize {
            let mut i = i;
            while p[i] != i {
                p[i] = p[p[i]];
                i = p[i];
            }
            i
        }
        for tp in atoms {
            if let (Some(a), Some(b)) = (anon_ix(&tp.subject), anon_ix(&tp.object)) {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                parent[ra] = rb;
            }
        }
        let mut groups: Vec<Vec<usize>> = Vec::new();
        let mut group_of: HashMap<usize, usize> = HashMap::new();
        for i in 0..n {
            if mask & (1 << i) == 0 {
                continue;
            }
            let r = find(&mut parent, i);
            let g = *group_of.entry(r).or_insert_with(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
            groups[g].push(i);
        }

        // The named terms each group touches, unified into one root.
        let mut named_atoms: Vec<TriplePattern> = Vec::new();
        let mut group_atoms: Vec<Vec<&TriplePattern>> = vec![Vec::new(); groups.len()];
        let mut adjacent: Vec<Vec<TermPattern>> = vec![Vec::new(); groups.len()];
        for tp in atoms {
            let s = anon_ix(&tp.subject);
            let is_type =
                matches!(&tp.predicate, NamedNodePattern::NamedNode(p) if p.as_str() == RDF_TYPE);
            let o = if is_type { None } else { anon_ix(&tp.object) };
            let Some(i) = s.or(o) else {
                named_atoms.push(tp.clone());
                continue;
            };
            let g = group_of[&find(&mut parent, i)];
            group_atoms[g].push(tp);
            if s.is_none() {
                adjacent[g].push(rename_term(&tp.subject, names));
            }
            if o.is_none() && !is_type {
                adjacent[g].push(rename_term(&tp.object, names));
            }
        }

        // Unify each group's adjacent terms: one root per group, a constant
        // if any (two different constants, or a literal, cannot be).
        let mut subst: HashMap<TermPattern, TermPattern> = HashMap::new();
        let mut roots: Vec<Option<TermPattern>> = Vec::new();
        {
            // Union-find over terms, across groups.
            let mut terms: Vec<TermPattern> = Vec::new();
            let mut tparent: Vec<usize> = Vec::new();
            let ix =
                |t: &TermPattern, terms: &mut Vec<TermPattern>, tp: &mut Vec<usize>| match terms
                    .iter()
                    .position(|x| x == t)
                {
                    Some(i) => i,
                    None => {
                        terms.push(t.clone());
                        tp.push(tp.len());
                        tp.len() - 1
                    }
                };
            for adj in &adjacent {
                let mut first: Option<usize> = None;
                for t in adj {
                    let i = ix(t, &mut terms, &mut tparent);
                    if let Some(f) = first {
                        let (a, b) = (find(&mut tparent, f), find(&mut tparent, i));
                        tparent[a] = b;
                    } else {
                        first = Some(i);
                    }
                }
            }
            let mut rep: HashMap<usize, TermPattern> = HashMap::new();
            for (i, t) in terms.iter().enumerate() {
                let r = find(&mut tparent, i);
                let better = match (rep.get(&r), t) {
                    (_, TermPattern::Literal(_)) => return Some(vec![]),
                    (None, _) => true,
                    (Some(TermPattern::NamedNode(a)), TermPattern::NamedNode(b)) if a != b => {
                        return Some(vec![])
                    }
                    (Some(TermPattern::NamedNode(_)), _) => false,
                    (Some(_), TermPattern::NamedNode(_)) => true,
                    (Some(_), _) => false,
                };
                if better {
                    rep.insert(r, t.clone());
                }
            }
            for (i, t) in terms.iter().enumerate() {
                let r = find(&mut tparent, i);
                if rep[&r] != *t {
                    subst.insert(t.clone(), rep[&r].clone());
                }
            }
            for adj in &adjacent {
                roots.push(adj.first().map(|t| {
                    let i = terms.iter().position(|x| x == t).expect("interned");
                    rep[&find(&mut tparent, i)].clone()
                }));
            }
        }

        // Each group's conditions.
        let mut conds: Vec<Vec<Cond>> = Vec::new();
        for (g, nodes) in groups.iter().enumerate() {
            let local = |i: usize| nodes.iter().position(|&x| x == i).expect("in group");
            let mut gatoms = Vec::new();
            for tp in &group_atoms[g] {
                let NamedNodePattern::NamedNode(p) = &tp.predicate else {
                    return None;
                };
                let end = |t: &TermPattern| match anon_ix(t) {
                    Some(i) => End::Node(local(i)),
                    None => End::Root,
                };
                if p.as_str() == RDF_TYPE {
                    let (Some(i), TermPattern::NamedNode(c)) = (anon_ix(&tp.subject), &tp.object)
                    else {
                        return None;
                    };
                    gatoms.push(GroupAtom::Class(local(i), c.as_str().to_string()));
                } else {
                    gatoms.push(GroupAtom::Role(
                        end(&tp.subject),
                        p.as_str().to_string(),
                        end(&tp.object),
                    ));
                }
            }
            let c = self.solve(nodes.len(), &gatoms, roots[g].is_some())?;
            if c.is_empty() {
                return Some(vec![]);
            }
            conds.push(c);
        }

        // The named part, with the unification applied.
        let apply = |t: &TermPattern| {
            let t = rename_term(t, names);
            subst.get(&t).cloned().unwrap_or(t)
        };
        let named: Vec<TriplePattern> = named_atoms
            .iter()
            .map(|tp| TriplePattern {
                subject: apply(&tp.subject),
                predicate: tp.predicate.clone(),
                object: apply(&tp.object),
            })
            .collect();
        let mut binds: Vec<(Variable, TermPattern)> = Vec::new();
        for (from, to) in &subst {
            if let TermPattern::Variable(v) = from {
                if !names.values().any(|n| n == from) {
                    binds.push((v.clone(), to.clone()));
                }
            }
        }
        binds.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));

        // One branch per combination of the groups' conditions.
        let mut combos: Vec<Vec<&Cond>> = vec![vec![]];
        for c in &conds {
            let mut next = Vec::new();
            for combo in &combos {
                for alt in c {
                    let mut x = combo.clone();
                    x.push(alt);
                    next.push(x);
                }
            }
            combos = next;
            if combos.len() > MAX_BRANCHES {
                return None;
            }
        }
        let mut out = Vec::new();
        'combo: for combo in combos {
            let mut parts = vec![self.ground(named.clone())];
            for (g, cond) in combo.iter().enumerate() {
                match cond {
                    Cond::Rooted(roles) => {
                        let root = roots[g].clone().expect("rooted group has a root");
                        for &r in roles {
                            match self.in_exists(&root, r) {
                                Some(p) => parts.push(p),
                                None => continue 'combo,
                            }
                        }
                    }
                    Cond::Anywhere(r) => {
                        if self.tbox.always_exists(*r) {
                            continue;
                        }
                        let x = self.fresh.var();
                        let alts: Vec<GraphPattern> = self
                            .tbox
                            .ancestors(*r)
                            .into_iter()
                            .filter_map(|a| self.in_exists(&x, a))
                            .collect();
                        match union(alts) {
                            Some(p) => parts.push(p),
                            None => continue 'combo,
                        }
                    }
                }
            }
            let mut branch = join(parts);
            // Variables unified with the root: bound after it, never before.
            for (v, t) in &binds {
                let expression = match t {
                    TermPattern::Variable(x) => Expression::Variable(x.clone()),
                    TermPattern::NamedNode(nn) => Expression::NamedNode(nn.clone()),
                    _ => continue,
                };
                branch = GraphPattern::Extend {
                    inner: Box::new(branch),
                    variable: v.clone(),
                    expression,
                };
            }
            out.push(branch);
        }
        Some(out)
    }

    /// The conditions under which a group of `n` anonymous blank nodes with
    /// these atoms maps into the anonymous part of the canonical model:
    /// below the root (`rooted`), or anywhere. `None` when the search ran
    /// out of budget.
    fn solve(&self, n: usize, atoms: &[GroupAtom], rooted: bool) -> Option<Vec<Cond>> {
        // A placement order: each node after the first is linked by an atom
        // to one placed before it (or to the root).
        let links = |first: Option<usize>| -> Vec<Step> {
            let mut order: Vec<Step> = Vec::new();
            let mut placed = vec![false; n];
            if let Some(f) = first {
                order.push((f, None));
                placed[f] = true;
            }
            loop {
                let mut progress = false;
                for (ai, a) in atoms.iter().enumerate() {
                    let GroupAtom::Role(u, _, v) = a else {
                        continue;
                    };
                    let is_placed = |e: &End, placed: &[bool]| match e {
                        End::Root => rooted,
                        End::Node(i) => placed[*i],
                    };
                    for (from, to, forward) in [(u, v, true), (v, u, false)] {
                        if let End::Node(t) = to {
                            if !placed[*t] && is_placed(from, &placed) {
                                placed[*t] = true;
                                order.push((*t, Some((ai, *from, forward))));
                                progress = true;
                            }
                        }
                    }
                }
                if !progress {
                    break;
                }
            }
            order
        };

        let mut found: BTreeSet<Cond> = BTreeSet::new();
        let mut budget = SEARCH_BUDGET;
        if rooted {
            let order = links(None);
            if order.len() != n {
                return Some(vec![]);
            }
            let mut placed: Vec<Option<Vec<usize>>> = vec![None; n];
            self.search(
                0,
                &order,
                atoms,
                &mut placed,
                rooted,
                &mut found,
                &mut budget,
            );
        } else {
            for first in 0..n {
                let order = links(Some(first));
                if order.len() != n {
                    return Some(vec![]);
                }
                for r0 in 0..self.tbox.roles.len() {
                    if self.tbox.role_unsat[r0] {
                        continue;
                    }
                    let mut placed: Vec<Option<Vec<usize>>> = vec![None; n];
                    placed[first] = Some(vec![r0]);
                    self.search(
                        1,
                        &order,
                        atoms,
                        &mut placed,
                        rooted,
                        &mut found,
                        &mut budget,
                    );
                }
            }
        }
        if budget == 0 {
            return None;
        }
        // A stronger condition than another adds nothing to the union.
        let all: Vec<Cond> = found.into_iter().collect();
        let keep = |c: &Cond| {
            !all.iter().any(|d| match (c, d) {
                (Cond::Rooted(a), Cond::Rooted(b)) => b != a && b.is_subset(a),
                _ => false,
            })
        };
        Some(all.iter().filter(|c| keep(c)).cloned().collect())
    }

    #[allow(clippy::too_many_arguments)]
    fn search(
        &self,
        k: usize,
        order: &[Step],
        atoms: &[GroupAtom],
        placed: &mut [Option<Vec<usize>>],
        rooted: bool,
        found: &mut BTreeSet<Cond>,
        budget: &mut usize,
    ) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let t = self.tbox;
        if k == order.len() {
            if atoms.iter().all(|a| self.holds(a, placed)) {
                found.insert(if rooted {
                    Cond::Rooted(placed.iter().flatten().map(|p| p[0]).collect())
                } else {
                    let top = placed
                        .iter()
                        .flatten()
                        .min_by_key(|p| p.len())
                        .expect("placed");
                    Cond::Anywhere(top[0])
                });
            }
            return;
        }
        let (node, link) = order[k];
        let Some((ai, other, forward)) = link else {
            return;
        };
        let GroupAtom::Role(_, p, _) = &atoms[ai] else {
            return;
        };
        let Some(p) = t.property_id(p) else {
            return;
        };
        let base: Vec<usize> = match other {
            End::Root => vec![],
            End::Node(j) => match &placed[j] {
                Some(path) => path.clone(),
                None => return,
            },
        };
        // The role from `other` to `node`.
        let want = if forward { p } else { t.inv[p] };
        let min_depth = if rooted {
            1
        } else {
            placed.iter().flatten().map(Vec::len).min().unwrap_or(1)
        };
        let mut candidates: Vec<Vec<usize>> = Vec::new();
        if base.len() < order.len() + 1 {
            for &s in &t.role_sub[want] {
                if t.role_unsat[s] {
                    continue;
                }
                let creatable = match base.last() {
                    None => rooted,
                    Some(&last) => t.node_has(last, t.exists[s]),
                };
                if creatable {
                    let mut path = base.clone();
                    path.push(s);
                    candidates.push(path);
                }
            }
        }
        if base.len() > min_depth {
            let last = *base.last().expect("non-empty");
            if t.role_sup[last].binary_search(&t.inv[want]).is_ok() {
                candidates.push(base[..base.len() - 1].to_vec());
            }
        }
        if !base.is_empty() && t.loops[p] {
            candidates.push(base.clone());
        }
        for c in candidates {
            placed[node] = Some(c);
            self.search(k + 1, order, atoms, placed, rooted, found, budget);
            placed[node] = None;
        }
    }

    /// Whether an atom holds of the placement.
    fn holds(&self, atom: &GroupAtom, placed: &[Option<Vec<usize>>]) -> bool {
        let t = self.tbox;
        let path = |e: &End| -> Option<Vec<usize>> {
            match e {
                End::Root => Some(vec![]),
                End::Node(i) => placed[*i].clone(),
            }
        };
        match atom {
            GroupAtom::Class(i, c) => {
                if c == OWL_THING {
                    return placed[*i]
                        .as_ref()
                        .is_some_and(|p| !t.data_roles[p[p.len() - 1]]);
                }
                let (Some(p), Some(b)) = (&placed[*i], t.class_id(c)) else {
                    return false;
                };
                t.node_has(p[p.len() - 1], b)
            }
            GroupAtom::Role(u, p, v) => {
                let (Some(pu), Some(pv), Some(p)) = (path(u), path(v), t.property_id(p)) else {
                    return false;
                };
                (pv.len() == pu.len() + 1
                    && pv.starts_with(&pu)
                    && t.role_sup[pv[pv.len() - 1]].binary_search(&p).is_ok())
                    || (pu.len() == pv.len() + 1
                        && pu.starts_with(&pv)
                        && t.role_sup[pu[pu.len() - 1]]
                            .binary_search(&t.inv[p])
                            .is_ok())
                    || (pu == pv && !pu.is_empty() && t.loops[p])
            }
        }
    }
}

/// `subject R object`: `subject P object`, or `object P subject` for `P⁻`.
fn role_pattern(subject: &TermPattern, role: &Role, object: &TermPattern) -> TriplePattern {
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

fn rename_term(t: &TermPattern, names: &HashMap<oxrdf::BlankNode, TermPattern>) -> TermPattern {
    match bnode_of(t).and_then(|b| names.get(b)) {
        Some(v) => v.clone(),
        None => t.clone(),
    }
}

fn rename(tp: &TriplePattern, names: &HashMap<oxrdf::BlankNode, TermPattern>) -> TriplePattern {
    TriplePattern {
        subject: rename_term(&tp.subject, names),
        predicate: tp.predicate.clone(),
        object: rename_term(&tp.object, names),
    }
}

/// Split atoms with blank nodes into parts joined through shared blank nodes.
fn components(atoms: Vec<TriplePattern>) -> Vec<Vec<TriplePattern>> {
    let mut parts: Vec<(HashSet<oxrdf::BlankNode>, Vec<TriplePattern>)> = Vec::new();
    for tp in atoms {
        let bs: HashSet<oxrdf::BlankNode> = [&tp.subject, &tp.object]
            .into_iter()
            .filter_map(bnode_of)
            .cloned()
            .collect();
        let mut merged: (HashSet<oxrdf::BlankNode>, Vec<TriplePattern>) = (bs.clone(), vec![tp]);
        let mut rest = Vec::new();
        for part in parts {
            if part.0.iter().any(|b| bs.contains(b)) {
                merged.0.extend(part.0);
                merged.1.extend(part.1);
            } else {
                rest.push(part);
            }
        }
        rest.push(merged);
        parts = rest;
    }
    parts.into_iter().map(|(_, atoms)| atoms).collect()
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
