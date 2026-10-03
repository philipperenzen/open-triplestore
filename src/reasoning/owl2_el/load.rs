//! Reading an EL ontology out of the quad index: RDF graphs → normalized
//! [`Axioms`] (OWL 2 Mapping to RDF Graphs, reverse direction, for the
//! constructs of the EL profile).
//!
//! Every class expression gets a concept id of its own and is *defined* by
//! its structure (`N ≡ ∃r.F`, `N ≡ A₁ ⊓ … ⊓ Aₙ`), so a restriction written
//! twice as two blank nodes is two names for one class, and the rules find
//! that out. A construct outside the profile (a union, a universal, a
//! cardinality, an inverse property, …) is kept as an opaque name and
//! counted in [`Loaded::ignored`]: treating an expression as an unknown
//! class loses consequences but never adds a wrong one.

use std::collections::{BTreeMap, HashMap};

use oxigraph::model::{GraphNameRef, NamedNodeRef, Term};

use super::datatypes::{datatypes_of, value_of, Dt, Value};
use super::saturate::{Axioms, Cid, FxMap, FxSet, Rid, BOTTOM, TOP};
use crate::reasoning::common::ReasoningError;
use crate::store::TripleStore;

pub(crate) const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub(crate) const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
pub(crate) const OWL: &str = "http://www.w3.org/2002/07/owl#";
pub(crate) const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// A term id in the [`Loaded::terms`] table.
pub(crate) type Tid = u32;

/// What a concept id stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A name the loader or the saturation made up.
    Fresh,
    /// A class IRI.
    Class(Tid),
    /// A blank-node class expression (or an opaque blank-node class).
    Expr(Tid),
    /// The nominal of an individual (IRI or blank node).
    Individual(Tid),
    /// The nominal of a data value; the term is the first literal read
    /// with that value.
    Literal(Tid),
    /// A datatype or a data range.
    Datatype,
}

/// The vocabulary the loader interprets, interned up front.
#[derive(Clone, Copy)]
pub(crate) struct Vocab {
    pub rdf_type: Tid,
    pub rdf_first: Tid,
    pub rdf_rest: Tid,
    pub rdf_nil: Tid,
    pub sub_class_of: Tid,
    pub sub_property_of: Tid,
    pub domain: Tid,
    pub range: Tid,
    pub equivalent_class: Tid,
    pub equivalent_property: Tid,
    pub disjoint_with: Tid,
    pub same_as: Tid,
    pub thing: Tid,
    pub nothing: Tid,
}

/// The ontology as the saturation sees it, plus what is needed to turn the
/// result back into RDF.
pub(crate) struct Loaded {
    pub ax: Axioms,
    pub terms: Vec<Term>,
    term_ids: HashMap<Term, Tid>,
    pub vocab: Vocab,
    /// What each concept id stands for; ids past the end are fresh.
    kinds: Vec<Kind>,
    /// The term of each named role (`None` for a fresh one).
    pub role_terms: Vec<Option<Tid>>,
    /// Class IRIs, to classify (`owl:Thing`/`owl:Nothing` excluded).
    pub classes: Vec<Cid>,
    /// Blank-node class expressions with an EL definition: these may appear
    /// as the object of a derived `rdfs:subClassOf`.
    pub defined_exprs: FxSet<Cid>,
    /// Individuals (IRIs and blank nodes).
    pub individuals: Vec<Cid>,
    /// Asserted property edges between nominals.
    pub edges: Vec<(Cid, Rid, Cid)>,
    /// Every literal term read for each value nominal.
    pub literal_terms: FxMap<Cid, Vec<Tid>>,
    /// Literals whose lexical form is not in their datatype's lexical space.
    pub ill_typed: FxSet<Cid>,
    /// Concepts made for negative property assertions (for the message).
    pub negative_assertions: FxSet<Cid>,
    /// `owl:hasKey`: the class and its key properties.
    pub keys: Vec<(Cid, Vec<Rid>)>,
    /// Every premise triple, as term ids, so derived triples already stated
    /// are not written again.
    pub premises: FxSet<(Tid, Tid, Tid)>,
    /// Constructs outside the profile (or not supported): count and one
    /// example subject, by construct name.
    pub ignored: BTreeMap<&'static str, (usize, String)>,
}

impl Loaded {
    pub fn kind(&self, c: Cid) -> Kind {
        self.kinds.get(c as usize).copied().unwrap_or(Kind::Fresh)
    }

    pub fn term(&self, t: Tid) -> &Term {
        &self.terms[t as usize]
    }

    /// Intern a vocabulary IRI (for output predicates/objects).
    pub fn iri(&mut self, iri: &str) -> Tid {
        intern(
            &mut self.terms,
            &mut self.term_ids,
            Term::NamedNode(oxigraph::model::NamedNode::new_unchecked(iri)),
        )
    }
}

fn intern(terms: &mut Vec<Term>, ids: &mut HashMap<Term, Tid>, t: Term) -> Tid {
    if let Some(&id) = ids.get(&t) {
        return id;
    }
    let id = terms.len() as Tid;
    terms.push(t.clone());
    ids.insert(t, id);
    id
}

/// Read every quad of `graphs` (`None` = the unnamed default graph) and
/// build the normalized ontology.
pub(crate) fn load(
    store: &TripleStore,
    graphs: &[Option<String>],
) -> Result<Loaded, ReasoningError> {
    let mut terms: Vec<Term> = Vec::new();
    let mut term_ids: HashMap<Term, Tid> = HashMap::new();
    let mut premises: FxSet<(Tid, Tid, Tid)> = FxSet::default();
    let mut triples: Vec<(Tid, Tid, Tid)> = Vec::new();
    for g in graphs {
        let graph_ref = match g {
            Some(g) => match NamedNodeRef::new(g) {
                Ok(nn) => GraphNameRef::NamedNode(nn),
                Err(_) => continue,
            },
            None => GraphNameRef::DefaultGraph,
        };
        for quad in store
            .store()
            .quads_for_pattern(None, None, None, Some(graph_ref))
        {
            let quad = quad.map_err(|e| ReasoningError::Store(e.to_string()))?;
            if matches!(quad.object, Term::Triple(_)) {
                continue;
            }
            let s = intern(&mut terms, &mut term_ids, quad.subject.into());
            let p = intern(&mut terms, &mut term_ids, quad.predicate.into());
            let o = intern(&mut terms, &mut term_ids, quad.object);
            if premises.insert((s, p, o)) {
                triples.push((s, p, o));
            }
        }
    }
    let mut b = Builder::new(terms, term_ids, premises);
    b.read(&triples);
    Ok(b.finish())
}

struct Builder {
    terms: Vec<Term>,
    term_ids: HashMap<Term, Tid>,
    premises: FxSet<(Tid, Tid, Tid)>,
    ax: Axioms,
    kinds: Vec<Kind>,
    /// Vocabulary-predicate triples by subject, for parsing expressions/lists.
    by_subject: FxMap<Tid, Vec<(Tid, Tid)>>,
    concept_of: FxMap<Tid, Cid>,
    individual_of: FxMap<Tid, Cid>,
    literal_of: FxMap<Tid, Cid>,
    role_of: FxMap<Tid, Rid>,
    role_terms: Vec<Option<Tid>>,
    classes: Vec<Cid>,
    defined_exprs: FxSet<Cid>,
    individuals: Vec<Cid>,
    edges: Vec<(Cid, Rid, Cid)>,
    keys: Vec<(Cid, Vec<Rid>)>,
    literal_terms: FxMap<Cid, Vec<Tid>>,
    literal_values: HashMap<Value, Cid>,
    ill_typed: FxSet<Cid>,
    negative_assertions: FxSet<Cid>,
    /// The EL datatypes' concepts, made on first use.
    dt_atoms: Option<HashMap<Dt, Cid>>,
    /// Data ranges read so far (datatype IRIs and blank nodes).
    data_range_of: FxMap<Tid, Option<Cid>>,
    /// Whether two individuals can turn out to be the same (sameAs, keys,
    /// object nominals): then every edge matters, since it is also an edge
    /// of the individuals equal to its ends.
    equality: bool,
    object_props: FxSet<Tid>,
    data_props: FxSet<Tid>,
    annotation_props: FxSet<Tid>,
    datatypes: FxSet<Tid>,
    ignored: BTreeMap<&'static str, (usize, String)>,
    v: Vocab,
    /// Interned IRIs of the rest of the vocabulary, by local name.
    owl: HashMap<&'static str, Tid>,
}

/// OWL terms the loader looks up by name.
const OWL_TERMS: &[&str] = &[
    "Class",
    "Restriction",
    "ObjectProperty",
    "DatatypeProperty",
    "AnnotationProperty",
    "NamedIndividual",
    "Ontology",
    "TransitiveProperty",
    "ReflexiveProperty",
    "FunctionalProperty",
    "InverseFunctionalProperty",
    "SymmetricProperty",
    "AsymmetricProperty",
    "IrreflexiveProperty",
    "AllDisjointClasses",
    "AllDisjointProperties",
    "AllDifferent",
    "NegativePropertyAssertion",
    "intersectionOf",
    "unionOf",
    "complementOf",
    "oneOf",
    "onProperty",
    "onProperties",
    "someValuesFrom",
    "allValuesFrom",
    "hasValue",
    "hasSelf",
    "cardinality",
    "minCardinality",
    "maxCardinality",
    "qualifiedCardinality",
    "minQualifiedCardinality",
    "maxQualifiedCardinality",
    "members",
    "distinctMembers",
    "propertyChainAxiom",
    "hasKey",
    "inverseOf",
    "propertyDisjointWith",
    "disjointUnionOf",
    "differentFrom",
    "sameAs",
    "datatypeComplementOf",
    "onDatatype",
    "withRestrictions",
    "topObjectProperty",
    "bottomObjectProperty",
    "topDataProperty",
    "bottomDataProperty",
    "sourceIndividual",
    "assertionProperty",
    "targetIndividual",
    "targetValue",
];

/// The datatype IRIs outside `xsd:` that name data ranges.
const OTHER_DATATYPES: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#Literal",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral",
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON",
    "http://www.w3.org/2002/07/owl#real",
    "http://www.w3.org/2002/07/owl#rational",
];

impl Builder {
    fn new(
        mut terms: Vec<Term>,
        mut term_ids: HashMap<Term, Tid>,
        premises: FxSet<(Tid, Tid, Tid)>,
    ) -> Self {
        let mut iri = |s: String| {
            intern(
                &mut terms,
                &mut term_ids,
                Term::NamedNode(oxigraph::model::NamedNode::new_unchecked(s)),
            )
        };
        let v = Vocab {
            rdf_type: iri(format!("{RDF}type")),
            rdf_first: iri(format!("{RDF}first")),
            rdf_rest: iri(format!("{RDF}rest")),
            rdf_nil: iri(format!("{RDF}nil")),
            sub_class_of: iri(format!("{RDFS}subClassOf")),
            sub_property_of: iri(format!("{RDFS}subPropertyOf")),
            domain: iri(format!("{RDFS}domain")),
            range: iri(format!("{RDFS}range")),
            equivalent_class: iri(format!("{OWL}equivalentClass")),
            equivalent_property: iri(format!("{OWL}equivalentProperty")),
            disjoint_with: iri(format!("{OWL}disjointWith")),
            same_as: iri(format!("{OWL}sameAs")),
            thing: iri(format!("{OWL}Thing")),
            nothing: iri(format!("{OWL}Nothing")),
        };
        let mut owl = HashMap::new();
        for name in OWL_TERMS {
            owl.insert(*name, iri(format!("{OWL}{name}")));
        }
        let mut datatypes = FxSet::default();
        for dt in OTHER_DATATYPES {
            datatypes.insert(iri(dt.to_string()));
        }
        Builder {
            terms,
            term_ids,
            premises,
            ax: Axioms::new(),
            kinds: vec![Kind::Fresh, Kind::Fresh],
            by_subject: FxMap::default(),
            concept_of: FxMap::default(),
            individual_of: FxMap::default(),
            literal_of: FxMap::default(),
            role_of: FxMap::default(),
            role_terms: Vec::new(),
            classes: Vec::new(),
            defined_exprs: FxSet::default(),
            individuals: Vec::new(),
            edges: Vec::new(),
            keys: Vec::new(),
            literal_terms: FxMap::default(),
            literal_values: HashMap::new(),
            ill_typed: FxSet::default(),
            negative_assertions: FxSet::default(),
            dt_atoms: None,
            data_range_of: FxMap::default(),
            equality: false,
            object_props: FxSet::default(),
            data_props: FxSet::default(),
            annotation_props: FxSet::default(),
            datatypes,
            ignored: BTreeMap::new(),
            v,
            owl,
        }
    }

    fn o(&self, name: &str) -> Tid {
        self.owl[name]
    }

    fn iri_str(&self, t: Tid) -> Option<&str> {
        match &self.terms[t as usize] {
            Term::NamedNode(n) => Some(n.as_str()),
            _ => None,
        }
    }

    fn is_literal(&self, t: Tid) -> bool {
        matches!(self.terms[t as usize], Term::Literal(_))
    }

    /// A term of the RDF, RDFS, OWL or XSD vocabulary (an XSD predicate is a
    /// facet of a datatype restriction).
    fn is_vocab(&self, t: Tid) -> bool {
        self.iri_str(t).is_some_and(|s| {
            s.starts_with(RDF) || s.starts_with(RDFS) || s.starts_with(OWL) || s.starts_with(XSD)
        })
    }

    fn is_datatype(&self, t: Tid) -> bool {
        self.datatypes.contains(&t) || self.iri_str(t).is_some_and(|s| s.starts_with(XSD))
    }

    fn ignore(&mut self, construct: &'static str, subject: Tid) {
        let example = self.terms[subject as usize].to_string();
        let e = self.ignored.entry(construct).or_insert((0, example));
        e.0 += 1;
    }

    fn objects(&self, s: Tid, p: Tid) -> impl Iterator<Item = Tid> + '_ {
        self.by_subject
            .get(&s)
            .into_iter()
            .flatten()
            .filter(move |(pp, _)| *pp == p)
            .map(|(_, o)| *o)
    }

    fn object(&self, s: Tid, p: Tid) -> Option<Tid> {
        self.objects(s, p).next()
    }

    fn has(&self, s: Tid, p: Tid) -> bool {
        self.object(s, p).is_some()
    }

    /// The members of an RDF list; `None` when it is malformed or cyclic.
    fn list(&self, head: Tid) -> Option<Vec<Tid>> {
        let mut out = Vec::new();
        let mut seen = FxSet::default();
        let mut cell = head;
        while cell != self.v.rdf_nil {
            if !seen.insert(cell) {
                return None;
            }
            out.push(self.object(cell, self.v.rdf_first)?);
            cell = self.object(cell, self.v.rdf_rest)?;
        }
        Some(out)
    }

    fn new_concept(&mut self, kind: Kind) -> Cid {
        let c = self.ax.fresh_concept();
        if self.kinds.len() <= c as usize {
            self.kinds.resize(c as usize + 1, Kind::Fresh);
        }
        self.kinds[c as usize] = kind;
        c
    }

    fn role(&mut self, t: Tid) -> Rid {
        if let Some(&r) = self.role_of.get(&t) {
            return r;
        }
        let r = self.ax.fresh_role();
        self.role_of.insert(t, r);
        if self.role_terms.len() <= r as usize {
            self.role_terms.resize(r as usize + 1, None);
        }
        self.role_terms[r as usize] = Some(t);
        r
    }

    fn individual(&mut self, t: Tid) -> Cid {
        if let Some(&c) = self.individual_of.get(&t) {
            return c;
        }
        let c = self.new_concept(Kind::Individual(t));
        self.ax.nominal.insert(c);
        self.individual_of.insert(t, c);
        self.individuals.push(c);
        c
    }

    /// The nominal of a literal's *value*: literals with equal values (`"1"`
    /// and `"01"` as integers, a string and the same token) share one.
    fn literal(&mut self, t: Tid) -> Cid {
        if let Some(&c) = self.literal_of.get(&t) {
            return c;
        }
        let Term::Literal(lit) = &self.terms[t as usize] else {
            unreachable!("literal() is only called on literals")
        };
        let parsed = value_of(lit.value(), lit.datatype().as_str(), lit.language());
        let value = parsed.clone().unwrap_or_else(|| Value::Other {
            lexical: lit.value().to_string(),
            datatype: lit.datatype().as_str().to_string(),
        });
        let c = match self.literal_values.get(&value) {
            Some(&c) => c,
            None => {
                let c = self.new_concept(Kind::Literal(t));
                self.ax.nominal.insert(c);
                self.ax.literal.insert(c);
                self.literal_values.insert(value.clone(), c);
                // Only the EL map's datatypes: `datatypes_of` also names the
                // RL-only ones (`xsd:int`, `xsd:double`, …).
                for dt in datatypes_of(&value)
                    .into_iter()
                    .filter(|d| Dt::ALL.contains(d))
                {
                    let d = self.dt_atom(dt);
                    self.ax.add_sub(c, d);
                }
                if parsed.is_none() {
                    // OWL 2: an ill-typed literal makes the ontology inconsistent.
                    self.ill_typed.insert(c);
                    self.ax.add_sub(c, BOTTOM);
                }
                c
            }
        };
        self.literal_of.insert(t, c);
        self.literal_terms.entry(c).or_default().push(t);
        c
    }

    /// The concept of an EL datatype. The first call makes all nineteen,
    /// with their hierarchy and the disjointness of the top-level value spaces.
    fn dt_atom(&mut self, dt: Dt) -> Cid {
        if self.dt_atoms.is_none() {
            let mut atoms = HashMap::new();
            for d in Dt::ALL {
                let c = self.new_concept(Kind::Datatype);
                self.ax.datatype.push(c);
                atoms.insert(d, c);
            }
            for d in Dt::ALL {
                if let Some(p) = d.parent() {
                    self.ax.add_sub(atoms[&d], atoms[&p]);
                }
            }
            for (i, a) in Dt::ROOTS.iter().enumerate() {
                for b in &Dt::ROOTS[i + 1..] {
                    self.ax.add_conj(atoms[a], atoms[b], BOTTOM);
                }
            }
            self.dt_atoms = Some(atoms);
        }
        self.dt_atoms.as_ref().expect("made above")[&dt]
    }

    /// The concept of a data range in the EL profile: an EL datatype, a
    /// declared datatype, an intersection of data ranges or a `oneOf` of one
    /// literal. `None` (and a report) for anything else.
    fn data_range(&mut self, t: Tid) -> Option<Cid> {
        if let Some(&c) = self.data_range_of.get(&t) {
            return c;
        }
        // Memoize a placeholder first: a cyclic definition ends here.
        self.data_range_of.insert(t, None);
        let c = self.read_data_range(t);
        self.data_range_of.insert(t, c);
        c
    }

    fn read_data_range(&mut self, t: Tid) -> Option<Cid> {
        if let Some(iri) = self.iri_str(t) {
            if let Some(dt) = Dt::from_iri(iri) {
                return Some(self.dt_atom(dt));
            }
            if self.datatypes.contains(&t) && !iri.starts_with(XSD) {
                // A datatype of the ontology's own (its definition, if any,
                // is an owl:equivalentClass read with the axioms).
                return Some(self.new_concept(Kind::Datatype));
            }
            self.ignore("datatype outside the EL profile", t);
            return None;
        }
        if let Some(list) = self.object(t, self.o("intersectionOf")) {
            let members = self.list(list)?;
            let mut cs = Vec::new();
            for m in members {
                cs.push(self.data_range(m)?);
            }
            let c = self.new_concept(Kind::Datatype);
            for &m in &cs {
                self.ax.add_sub(c, m);
            }
            self.ax.add_conj_n(&cs, c);
            return Some(c);
        }
        if let Some(list) = self.object(t, self.o("oneOf")) {
            return match self.list(list).as_deref() {
                Some([v]) if self.is_literal(*v) => Some(self.literal(*v)),
                _ => {
                    self.ignore("DataOneOf with more than one literal", t);
                    None
                }
            };
        }
        if self.has(t, self.o("onDatatype")) {
            self.ignore("DatatypeRestriction", t);
        } else if self.has(t, self.o("datatypeComplementOf")) {
            self.ignore("DataComplementOf", t);
        } else if self.has(t, self.o("unionOf")) {
            self.ignore("DataUnionOf", t);
        } else {
            self.ignore("unknown data range", t);
        }
        None
    }

    /// The concept id of a class expression node, defining it from its
    /// structure the first time it is seen.
    fn concept(&mut self, t: Tid) -> Cid {
        if t == self.v.thing {
            return TOP;
        }
        if t == self.v.nothing {
            return BOTTOM;
        }
        if let Some(&c) = self.concept_of.get(&t) {
            return c;
        }
        let c = match self.terms[t as usize] {
            Term::NamedNode(_) => {
                let c = self.new_concept(Kind::Class(t));
                self.classes.push(c);
                c
            }
            _ => self.new_concept(Kind::Expr(t)),
        };
        // Memoize before the definition is read: lists and restrictions can
        // refer back to the node.
        self.concept_of.insert(t, c);
        if self.define(t, c) && matches!(self.terms[t as usize], Term::BlankNode(_)) {
            self.defined_exprs.insert(c);
        }
        c
    }

    /// Read the definition of class expression `t` into axioms for `c`.
    /// Returns whether `t` has an EL definition.
    fn define(&mut self, t: Tid, c: Cid) -> bool {
        if let Some(list) = self.object(t, self.o("intersectionOf")) {
            let Some(members) = self.list(list) else {
                self.ignore("malformed intersectionOf list", t);
                return false;
            };
            let members: Vec<Cid> = members.into_iter().map(|m| self.concept(m)).collect();
            for &m in &members {
                self.ax.add_sub(c, m);
            }
            self.ax.add_conj_n(&members, c);
            return true;
        }
        if let Some(p) = self.object(t, self.o("onProperty")) {
            if !matches!(self.terms[p as usize], Term::NamedNode(_)) {
                self.ignore("ObjectInverseOf", t);
                return false;
            }
            if let Some(f) = self.object(t, self.o("someValuesFrom")) {
                let r = self.role(p);
                let filler =
                    if self.data_props.contains(&p) || self.is_datatype(f) || self.is_data_range(f)
                    {
                        self.data_range(f)
                    } else {
                        Some(self.concept(f))
                    };
                let Some(f) = filler else {
                    self.ignore(
                        "DataSomeValuesFrom over a data range outside the EL profile",
                        t,
                    );
                    return false;
                };
                self.ax.add_exists_rhs(c, r, f);
                self.ax.add_exists_lhs(r, f, c);
                return true;
            }
            if let Some(v) = self.object(t, self.o("hasValue")) {
                // ∃p.{v}: ObjectHasValue or DataHasValue.
                let r = self.role(p);
                let f = if self.is_literal(v) {
                    self.literal(v)
                } else {
                    self.equality = true;
                    self.individual(v)
                };
                self.ax.add_exists_rhs(c, r, f);
                self.ax.add_exists_lhs(r, f, c);
                return true;
            }
            if self.has(t, self.o("hasSelf")) {
                let r = self.role(p);
                self.ax.self_rhs.entry(c).or_default().push(r);
                self.ax.self_lhs.entry(r).or_default().push(c);
                return true;
            }
            if self.has(t, self.o("allValuesFrom")) {
                self.ignore("ObjectAllValuesFrom", t);
                return false;
            }
            for card in [
                "cardinality",
                "minCardinality",
                "maxCardinality",
                "qualifiedCardinality",
                "minQualifiedCardinality",
                "maxQualifiedCardinality",
            ] {
                if self.has(t, self.o(card)) {
                    self.ignore("cardinality restriction", t);
                    return false;
                }
            }
            return false;
        }
        if self.has(t, self.o("onProperties")) {
            self.ignore("n-ary data restriction", t);
            return false;
        }
        if let Some(list) = self.object(t, self.o("oneOf")) {
            return match self.list(list).as_deref() {
                Some([a]) if !self.is_literal(*a) => {
                    self.equality = true;
                    let n = self.individual(*a);
                    self.ax.add_sub(c, n);
                    self.ax.add_sub(n, c);
                    true
                }
                _ => {
                    self.ignore("ObjectOneOf with more than one individual", t);
                    false
                }
            };
        }
        if self.has(t, self.o("unionOf")) {
            self.ignore("ObjectUnionOf", t);
            return false;
        }
        if self.has(t, self.o("complementOf")) {
            self.ignore("ObjectComplementOf", t);
            return false;
        }
        false
    }

    /// A blank node that is a data range (`datatypeComplementOf`, a
    /// faceted `onDatatype`, a `oneOf` of literals or an intersection of
    /// datatypes).
    fn is_data_range(&self, t: Tid) -> bool {
        if self.has(t, self.o("datatypeComplementOf")) || self.has(t, self.o("onDatatype")) {
            return true;
        }
        let first_member = |p: &str| {
            self.object(t, self.o(p))
                .and_then(|l| self.list(l))
                .and_then(|m| m.first().copied())
        };
        if let Some(m) = first_member("oneOf") {
            return self.is_literal(m);
        }
        if let Some(m) = first_member("intersectionOf") {
            return self.is_datatype(m) || self.is_data_range(m);
        }
        false
    }

    fn read(&mut self, triples: &[(Tid, Tid, Tid)]) {
        let rdf_type = self.v.rdf_type;
        // Index the vocabulary triples by subject; note the property kinds.
        for &(s, p, o) in triples {
            if self.is_vocab(p) {
                self.by_subject.entry(s).or_default().push((p, o));
            } else if self.is_literal(o) {
                self.data_props.insert(p);
            }
        }
        for &(s, p, o) in triples {
            if p != rdf_type {
                continue;
            }
            if o == self.o("ObjectProperty") {
                self.object_props.insert(s);
            } else if o == self.o("DatatypeProperty") {
                self.data_props.insert(s);
            } else if o == self.o("AnnotationProperty") {
                self.annotation_props.insert(s);
            } else if self
                .iri_str(o)
                .is_some_and(|i| i == "http://www.w3.org/2000/01/rdf-schema#Datatype")
            {
                self.datatypes.insert(s);
            }
        }
        for &(s, p, o) in triples {
            if p == self.v.range && (self.is_datatype(o) || self.is_data_range(o)) {
                self.data_props.insert(s);
            }
        }
        // TBox and declarations.
        for &(s, p, o) in triples {
            if self.is_vocab(p) {
                self.read_vocab(s, p, o);
            }
        }
        // ABox. An edge over a property no axiom mentions cannot lead to a
        // consequence, so it is not loaded (its ends still are) — unless two
        // individuals can turn out equal, which copies edges between them.
        let relevant = self.relevant_roles();
        let equality = self.equality;
        for &(s, p, o) in triples {
            if self.is_vocab(p) || self.annotation_props.contains(&p) {
                continue;
            }
            if self.is_literal(s) {
                continue;
            }
            let x = self.individual(s);
            let r = if equality {
                Some(self.role(p))
            } else {
                self.role_of.get(&p).copied()
            };
            let relevant = |r: &Rid| equality || relevant.contains(r);
            if self.is_literal(o) {
                if let Some(r) = r.filter(relevant) {
                    let y = self.literal(o);
                    self.edges.push((x, r, y));
                }
            } else {
                let y = self.individual(o);
                if let Some(r) = r.filter(relevant) {
                    self.edges.push((x, r, y));
                }
            }
        }
    }

    fn relevant_roles(&self) -> FxSet<Rid> {
        let mut out: FxSet<Rid> = FxSet::default();
        for &(r, s) in &self.ax.role_sub {
            out.insert(r);
            out.insert(s);
        }
        for &(r, s, t) in &self.ax.chains {
            out.extend([r, s, t]);
        }
        out.extend(self.ax.reflexive.iter().copied());
        out.extend(self.ax.functional.iter().copied());
        out.extend(self.ax.self_lhs.keys().copied());
        for rs in self.ax.self_rhs.values() {
            out.extend(rs.iter().copied());
        }
        out.extend(self.ax.ranges.iter().map(|(r, _)| *r));
        for entries in self.ax.ex_rhs.values().chain(self.ax.ex_lhs.values()) {
            out.extend(entries.iter().map(|(r, _)| *r));
        }
        for (_, props) in &self.keys {
            out.extend(props.iter().copied());
        }
        out
    }

    fn named(&self, t: Tid) -> bool {
        matches!(self.terms[t as usize], Term::NamedNode(_))
    }

    fn read_vocab(&mut self, s: Tid, p: Tid, o: Tid) {
        let v = self.v;
        if p == v.rdf_type {
            self.read_type(s, o);
        } else if p == v.sub_class_of {
            let (a, b) = (self.concept(s), self.concept(o));
            self.ax.add_sub(a, b);
        } else if p == v.equivalent_class && self.datatypes.contains(&s) {
            // A datatype definition: the datatype is the data range.
            if let (Some(a), Some(b)) = (self.data_range(s), self.data_range(o)) {
                self.ax.add_sub(a, b);
                self.ax.add_sub(b, a);
            }
        } else if p == v.equivalent_class {
            let (a, b) = (self.concept(s), self.concept(o));
            self.ax.add_sub(a, b);
            self.ax.add_sub(b, a);
        } else if p == v.disjoint_with {
            let (a, b) = (self.concept(s), self.concept(o));
            self.ax.add_conj(a, b, BOTTOM);
        } else if p == v.sub_property_of || p == v.equivalent_property {
            if !(self.named(s) && self.named(o)) {
                self.ignore("ObjectInverseOf", s);
                return;
            }
            let (r, q) = (self.role(s), self.role(o));
            self.ax.role_sub.push((r, q));
            if p == self.v.equivalent_property {
                self.ax.role_sub.push((q, r));
            }
        } else if p == v.domain {
            if !self.named(s) {
                self.ignore("ObjectInverseOf", s);
                return;
            }
            let r = self.role(s);
            let c = self.concept(o);
            self.ax.add_exists_lhs(r, TOP, c);
        } else if p == v.range {
            if !self.named(s) {
                self.ignore("ObjectInverseOf", s);
                return;
            }
            let r = self.role(s);
            let c = if self.data_props.contains(&s) {
                match self.data_range(o) {
                    Some(c) => c,
                    None => return,
                }
            } else {
                self.concept(o)
            };
            self.ax.ranges.push((r, c));
        } else if p == self.o("propertyChainAxiom") {
            let chain = self.list(o).unwrap_or_default();
            if chain.is_empty() || !self.named(s) || chain.iter().any(|t| !self.named(*t)) {
                self.ignore("ObjectInverseOf or malformed chain", s);
                return;
            }
            let t = self.role(s);
            let chain: Vec<Rid> = chain.into_iter().map(|c| self.role(c)).collect();
            self.ax.add_chain(&chain, t);
        } else if p == self.o("hasKey") {
            let props = self.list(o).unwrap_or_default();
            if props.is_empty() || props.iter().any(|t| !self.named(*t)) {
                self.ignore("ObjectInverseOf or malformed key", s);
                return;
            }
            let c = self.concept(s);
            let props: Vec<Rid> = props.into_iter().map(|t| self.role(t)).collect();
            self.keys.push((c, props));
            self.equality = true;
        } else if p == self.o("members") {
            let members = self.list(o).unwrap_or_default();
            let types: Vec<Tid> = self.objects(s, v.rdf_type).collect();
            if types.contains(&self.o("AllDisjointClasses")) {
                let cs: Vec<Cid> = members.into_iter().map(|m| self.concept(m)).collect();
                for (i, &a) in cs.iter().enumerate() {
                    for &b in &cs[i + 1..] {
                        self.ax.add_conj(a, b, BOTTOM);
                    }
                }
            } else if types.contains(&self.o("AllDisjointProperties")) {
                self.ignore("DisjointObjectProperties", s);
            } else if types.contains(&self.o("AllDifferent")) {
                self.all_different(&members);
            }
        } else if p == self.o("distinctMembers") {
            let members = self.list(o).unwrap_or_default();
            self.all_different(&members);
        } else if p == self.v.same_as {
            if self.is_literal(s) || self.is_literal(o) {
                return;
            }
            self.equality = true;
            let (a, b) = (self.individual(s), self.individual(o));
            self.ax.add_sub(a, b);
            self.ax.add_sub(b, a);
        } else if p == self.o("differentFrom") {
            self.all_different(&[s, o]);
        } else if p == self.o("inverseOf") {
            self.ignore("InverseObjectProperties", s);
        } else if p == self.o("propertyDisjointWith") {
            self.ignore("DisjointObjectProperties", s);
        } else if p == self.o("disjointUnionOf") {
            self.ignore("DisjointUnion", s);
        } else if [
            "intersectionOf",
            "unionOf",
            "complementOf",
            "oneOf",
            "someValuesFrom",
            "hasValue",
            "hasSelf",
            "allValuesFrom",
        ]
        .iter()
        .any(|n| p == self.o(n))
        {
            // A class (named or not) defined by its structure: read the
            // definition even when the node is used nowhere else.
            if !self.is_data_range(s) && !self.datatypes.contains(&s) {
                self.concept(s);
            }
        }
    }

    /// `DifferentIndividuals`: pairwise `{a} ⊓ {b} ⊑ ⊥`.
    fn all_different(&mut self, members: &[Tid]) {
        let named: Vec<Tid> = members
            .iter()
            .copied()
            .filter(|m| !self.is_literal(*m))
            .collect();
        let cs: Vec<Cid> = named.into_iter().map(|m| self.individual(m)).collect();
        for (i, &a) in cs.iter().enumerate() {
            for &b in &cs[i + 1..] {
                if a != b {
                    self.ax.add_conj(a, b, BOTTOM);
                }
            }
        }
    }

    /// `NegativeObjectPropertyAssertion` / `NegativeDataPropertyAssertion`:
    /// `{a} ⊓ ∃p.{b} ⊑ ⊥`.
    fn negative_assertion(&mut self, n: Tid) {
        let src = self.object(n, self.o("sourceIndividual"));
        let prop = self.object(n, self.o("assertionProperty"));
        let target = self
            .object(n, self.o("targetIndividual"))
            .or_else(|| self.object(n, self.o("targetValue")));
        let (Some(src), Some(prop), Some(target)) = (src, prop, target) else {
            self.ignore("malformed NegativePropertyAssertion", n);
            return;
        };
        if !self.named(prop) || self.is_literal(src) {
            self.ignore("malformed NegativePropertyAssertion", n);
            return;
        }
        let a = self.individual(src);
        let r = self.role(prop);
        let b = if self.is_literal(target) {
            self.literal(target)
        } else {
            self.individual(target)
        };
        let e = self.new_concept(Kind::Fresh);
        self.negative_assertions.insert(e);
        self.ax.add_exists_lhs(r, b, e);
        self.ax.add_conj(a, e, BOTTOM);
    }

    fn read_type(&mut self, s: Tid, o: Tid) {
        if o == self.o("TransitiveProperty") {
            if self.named(s) {
                let r = self.role(s);
                self.ax.chains.push((r, r, r));
            } else {
                self.ignore("ObjectInverseOf", s);
            }
        } else if o == self.o("ReflexiveProperty") {
            if self.named(s) {
                let r = self.role(s);
                self.ax.reflexive.push(r);
            } else {
                self.ignore("ObjectInverseOf", s);
            }
        } else if o == self.o("FunctionalProperty") {
            if self.data_props.contains(&s) && self.named(s) {
                let r = self.role(s);
                self.ax.functional.push(r);
            } else {
                self.ignore("FunctionalObjectProperty", s);
            }
        } else if o == self.o("InverseFunctionalProperty") {
            self.ignore("InverseFunctionalObjectProperty", s);
        } else if o == self.o("SymmetricProperty") {
            self.ignore("SymmetricObjectProperty", s);
        } else if o == self.o("AsymmetricProperty") {
            self.ignore("AsymmetricObjectProperty", s);
        } else if o == self.o("IrreflexiveProperty") {
            self.ignore("IrreflexiveObjectProperty", s);
        } else if o == self.o("NegativePropertyAssertion") {
            self.negative_assertion(s);
        } else if o == self.o("NamedIndividual") {
            if !self.is_literal(s) {
                self.individual(s);
            }
        } else if o == self.v.thing || o == self.v.nothing || !self.is_vocab(o) {
            // A class assertion.
            if self.is_literal(s) {
                return;
            }
            let x = self.individual(s);
            let c = self.concept(o);
            self.ax.add_sub(x, c);
        } else if (o == self.o("Class")
            || self
                .iri_str(o)
                .is_some_and(|i| i == "http://www.w3.org/2000/01/rdf-schema#Class"))
            && self.named(s)
        {
            // A declared class is classified even when no axiom mentions it.
            self.concept(s);
        }
    }

    fn finish(self) -> Loaded {
        Loaded {
            ax: self.ax,
            terms: self.terms,
            term_ids: self.term_ids,
            vocab: self.v,
            kinds: self.kinds,
            role_terms: self.role_terms,
            classes: self.classes,
            defined_exprs: self.defined_exprs,
            individuals: self.individuals,
            edges: self.edges,
            literal_terms: self.literal_terms,
            ill_typed: self.ill_typed,
            negative_assertions: self.negative_assertions,
            keys: self.keys,
            premises: self.premises,
            ignored: self.ignored,
        }
    }
}
