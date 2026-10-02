//! Table 8 of the RL/RDF rules — the semantics of datatypes — over literals.
//!
//! `dt-type2` (`lt rdf:type dt`), `dt-eq` (`lt1 owl:sameAs lt2`) and
//! `dt-diff` (`lt1 owl:differentFrom lt2`) conclude triples with a literal
//! subject, which no RDF graph can hold. The rules that would read those
//! generalized triples are simulated here, in Rust, over the data values of
//! the literals in scope ([`super::super::datatypes`]):
//!
//! - **Typing.** Every literal has the datatypes of the RL map whose value
//!   space holds its value (`dt-type2`), plus the classes `prp-rng` and
//!   `cls-avf` give it, closed under `rdfs:subClassOf` / `owl:equivalentClass`
//!   (`cax-sco`, `cax-eqc1/2`) and `owl:intersectionOf` (`cls-int1`). Its
//!   RDF-representable consequence is `cls-svf1` on a data value:
//!   `?u ?p ?lt` with `?lt` of the `owl:someValuesFrom` class types `?u`.
//! - **Equality.** Equal-valued literals written differently
//!   (`"1"^^xsd:integer`, `"1.0"^^xsd:decimal`) are `owl:sameAs` (`dt-eq`), so
//!   `eq-rep-o` copies every triple with one as object to the others: the
//!   *variants*. With them in the target graph `cls-hv1/2`, `prp-key` and
//!   `prp-npa2` match by value through ordinary term joins.
//! - **Difference.** Literals with different values are `owl:differentFrom`
//!   (`dt-diff`). A rule that equates two literals — `prp-fp`, `cls-maxc2`,
//!   `cls-maxqc3/4` on a data property — then meets `eq-diff1`: two different
//!   values of a functional data property make the ontology inconsistent.
//! - **dt-not-type.** A literal typed (above) with a datatype whose value
//!   space does not hold its value, or ill-typed (no value at all), is an
//!   inconsistency; so is a literal of `owl:Nothing` (`cls-nothing2`) or of
//!   two disjoint or complementary classes (`cax-dw`, `cls-com`), and one
//!   counted by a `owl:maxQualifiedCardinality 0` (`cls-maxqc1`).
//!
//! Beyond the rule set, and sound under both OWL 2 semantics: a datatype
//! restriction (`owl:onDatatype` / `owl:withRestrictions`) is a class of
//! literals here too, holding exactly the values of its base datatype that
//! meet its facets. RL's syntax has no such restrictions, so the rules never
//! see one; a range or `someValuesFrom` naming one is honoured instead of
//! ignored.
//!
//! Not simulated: chains through property triples with a literal subject,
//! which arise only when a data property is used as an object property
//! (declared symmetric, transitive or the inverse of another), and
//! `owl:sameAs` between a literal and an IRI.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use oxigraph::model::{GraphName, Literal, NamedNode, NamedOrBlankNode, Quad, QuadRef, Term};

use super::*;
use crate::reasoning::datatypes::{self, Dt, Facet, Value};

const OWL_ON_DATATYPE: &str = "http://www.w3.org/2002/07/owl#onDatatype";
const OWL_WITH_RESTRICTIONS: &str = "http://www.w3.org/2002/07/owl#withRestrictions";
const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema#";
const NON_NEG_ONE: &str = "\"1\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger>";
const NON_NEG_ZERO: &str = "\"0\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger>";

/// What one scan of the literals in scope found.
#[derive(Default)]
pub(super) struct LiteralScan {
    /// The first ill-typed literal: its lexical form is no value of its
    /// datatype.
    ill_typed: Option<Literal>,
    /// Literals that are distinct terms but one value (two or more each).
    variants: Vec<Vec<Literal>>,
    /// One literal per distinct set of RL datatypes their values are in.
    signatures: HashMap<u64, Literal>,
}

/// The class axioms the literal typing closes over.
#[derive(Default)]
struct ClassAxioms {
    /// `c rdfs:subClassOf d` and both directions of `owl:equivalentClass`.
    sup: HashMap<Term, Vec<Term>>,
    /// `c owl:intersectionOf (m…)`.
    intersections: Vec<(Term, Vec<Term>)>,
    /// `d owl:onDatatype base ; owl:withRestrictions (…)`: the base datatype
    /// and the facets; `None` when the base or a facet cannot be read.
    restrictions: HashMap<Term, Option<(Dt, Vec<Facet>)>>,
    disjoint: Vec<(Term, Term)>,
    complements: Vec<(Term, Term)>,
}

fn hash_of<T: Hash>(t: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// Whether `lit` is ill-typed: of a datatype of the RL map and no value of
/// it, or of another XSD datatype whose lexical rules SHACL's `sh:datatype`
/// knows and fails.
fn ill_typed(lit: &Literal) -> bool {
    let dt = lit.datatype().as_str();
    if Dt::from_rl_iri(dt).is_some() {
        return datatypes::literal_value(lit).is_none();
    }
    dt.starts_with(XSD_NS) && !crate::shacl::constraints::xsd_lexical_valid(lit)
}

/// The value of `lit` when it is well-typed and of a datatype of the RL map
/// (or one this module otherwise models); `None` for an ill-typed literal or
/// one whose datatype is unknown here, which equals only itself.
fn known_value(lit: &Literal) -> Option<Value> {
    match datatypes::literal_value(lit)? {
        Value::Other { .. } => None,
        v => Some(v),
    }
}

/// The RL datatypes `v` is in, as a bit set over `Dt as u8`.
fn signature(v: &Value) -> u64 {
    datatypes::datatypes_of(v)
        .into_iter()
        .filter(|d| Dt::RL.contains(d))
        .fold(0u64, |m, d| m | (1u64 << (d as u8)))
}

fn rl_types(v: &Value) -> Vec<Term> {
    datatypes::datatypes_of(v)
        .into_iter()
        .filter(|d| Dt::RL.contains(d))
        .map(|d| NamedNode::new_unchecked(d.iri()).into())
        .collect()
}

fn named(iri: &str) -> Term {
    NamedNode::new_unchecked(iri).into()
}

impl ClassAxioms {
    /// The classes of a literal with value `v` that starts with `seed`
    /// (`prp-rng` / `cls-avf` types): its RL datatypes, the datatype
    /// restrictions it meets, then everything above under `rdfs:subClassOf`,
    /// `owl:equivalentClass` and `owl:intersectionOf`.
    fn closure(&self, v: &Value, seed: impl IntoIterator<Item = Term>) -> HashSet<Term> {
        let mut set: HashSet<Term> = HashSet::new();
        let mut todo: Vec<Term> = rl_types(v).into_iter().chain(seed).collect();
        for (d, r) in &self.restrictions {
            if let Some((base, facets)) = r {
                if datatypes::in_restriction(v, *base, facets) == Some(true) {
                    todo.push(d.clone());
                }
            }
        }
        loop {
            while let Some(c) = todo.pop() {
                if set.insert(c.clone()) {
                    if let Some(ds) = self.sup.get(&c) {
                        todo.extend(ds.iter().filter(|d| !set.contains(*d)).cloned());
                    }
                }
            }
            for (c, members) in &self.intersections {
                if !set.contains(c) && members.iter().all(|m| set.contains(m)) {
                    todo.push(c.clone());
                }
            }
            if todo.is_empty() {
                return set;
            }
        }
    }

    /// The inconsistency a literal with value `v` and classes `classes` makes,
    /// as `(rule, detail)`.
    fn clash(&self, lit: &Literal, v: &Value, classes: &HashSet<Term>) -> Option<(String, String)> {
        if classes.contains(&named(OWL_NOTHING)) {
            return Some((
                "cls-nothing2".into(),
                format!("the literal {lit} is an instance of owl:Nothing"),
            ));
        }
        for c in classes {
            let Term::NamedNode(n) = c else {
                if let Some(Some((base, facets))) = self.restrictions.get(c) {
                    if datatypes::in_restriction(v, *base, facets) == Some(false) {
                        return Some((
                            "dt-not-type".into(),
                            format!(
                                "{lit} is outside the datatype restriction {c} (base {}) its property's range or value restriction requires",
                                base.iri()
                            ),
                        ));
                    }
                }
                continue;
            };
            if let Some(dt) = Dt::from_rl_iri(n.as_str()) {
                if datatypes::in_value_space(v, dt) == Some(false) {
                    return Some((
                        "dt-not-type".into(),
                        format!("{lit} is typed {n} but its value is not in that datatype"),
                    ));
                }
            } else if let Some(Some((base, facets))) = self.restrictions.get(c) {
                if datatypes::in_restriction(v, *base, facets) == Some(false) {
                    return Some((
                        "dt-not-type".into(),
                        format!("{lit} is typed {n} but its value is outside that restriction"),
                    ));
                }
            }
        }
        for (a, b) in &self.disjoint {
            if classes.contains(a) && classes.contains(b) {
                return Some((
                    "cax-dw".into(),
                    format!("the literal {lit} is in the disjoint classes {a} and {b}"),
                ));
            }
        }
        for (a, b) in &self.complements {
            if classes.contains(a) && classes.contains(b) {
                return Some((
                    "cls-com".into(),
                    format!("the literal {lit} is in {a} and in its complement {b}"),
                ));
            }
        }
        None
    }
}

impl Owl2RLReasoner<'_> {
    /// The graphs a quad scan reads: the scope, or the unnamed default graph
    /// and the target graph.
    fn literal_graphs(&self) -> Vec<GraphName> {
        match self.scope() {
            Some(scope) => scope
                .into_iter()
                .filter_map(|g| NamedNode::new(g).ok().map(GraphName::NamedNode))
                .collect(),
            None => {
                let mut v = vec![GraphName::DefaultGraph];
                if let Ok(t) = NamedNode::new(self.target_graph.clone()) {
                    v.push(GraphName::NamedNode(t));
                }
                v
            }
        }
    }

    /// Every literal object in scope, once per quad.
    fn each_literal(
        &self,
        mut f: impl FnMut(&Literal),
    ) -> Result<(), ReasoningError> {
        for g in self.literal_graphs() {
            for quad in self
                .store
                .store()
                .quads_for_pattern(None, None, None, Some(g.as_ref()))
            {
                let quad = quad.map_err(|e| ReasoningError::Store(e.to_string()))?;
                if let Term::Literal(lit) = &quad.object {
                    f(lit);
                }
            }
        }
        Ok(())
    }

    /// One scan of the literals in scope: the ill-typed ones, the variants
    /// and the datatype signatures. The scan reads the quad index directly,
    /// not a DISTINCT query (which could take the sharded mirror path).
    pub(super) fn scan_literals(&self) -> Result<LiteralScan, ReasoningError> {
        let mut scan = LiteralScan::default();
        // Value hash → term hash of the first literal seen with it; a second,
        // different term with the same value hash marks a candidate.
        let mut first: HashMap<u64, u64> = HashMap::new();
        let mut candidates: HashSet<u64> = HashSet::new();
        self.each_literal(|lit| {
            if ill_typed(lit) {
                scan.ill_typed.get_or_insert_with(|| lit.clone());
                return;
            }
            let Some(v) = known_value(lit) else { return };
            scan.signatures
                .entry(signature(&v))
                .or_insert_with(|| lit.clone());
            let (vh, th) = (hash_of(&v), hash_of(lit));
            match first.get(&vh) {
                None => {
                    first.insert(vh, th);
                }
                Some(t) if *t != th => {
                    candidates.insert(vh);
                }
                Some(_) => {}
            }
        })?;
        drop(first);
        if !candidates.is_empty() {
            let mut groups: HashMap<Value, HashSet<Literal>> = HashMap::new();
            self.each_literal(|lit| {
                if ill_typed(lit) {
                    return;
                }
                if let Some(v) = known_value(lit) {
                    if candidates.contains(&hash_of(&v)) {
                        groups.entry(v).or_default().insert(lit.clone());
                    }
                }
            })?;
            scan.variants = groups
                .into_values()
                .filter(|g| g.len() > 1)
                .map(|g| {
                    let mut g: Vec<Literal> = g.into_iter().collect();
                    g.sort_by_key(|l| l.to_string());
                    g
                })
                .collect();
            scan.variants.sort_by_key(|g| g[0].to_string());
        }
        Ok(scan)
    }

    /// Rows of a SELECT in scope, each as the values of `vars` in order.
    fn select(&self, sparql: &str, vars: &[&str]) -> Result<Vec<Vec<Option<Term>>>, ReasoningError> {
        let mut out = Vec::new();
        if let oxigraph::sparql::QueryResults::Solutions(rows) = self.run_query(sparql)? {
            for row in rows {
                let row = row.map_err(|e| ReasoningError::Query(e.to_string()))?;
                out.push(vars.iter().map(|v| row.get(*v).cloned()).collect());
            }
        }
        Ok(out)
    }

    /// The class axioms in scope that type literals.
    fn class_axioms(&self) -> Result<ClassAxioms, ReasoningError> {
        let mut ax = ClassAxioms::default();
        let pairs = |p: &str| -> Result<Vec<(Term, Term)>, ReasoningError> {
            Ok(self
                .select(&format!("SELECT DISTINCT ?a ?b WHERE {{ ?a <{p}> ?b }}"), &["a", "b"])?
                .into_iter()
                .filter_map(|r| match (&r[0], &r[1]) {
                    (Some(a), Some(b)) => Some((a.clone(), b.clone())),
                    _ => None,
                })
                .collect())
        };
        for (a, b) in pairs(RDFS_SUB_CLASS_OF)? {
            ax.sup.entry(a).or_default().push(b);
        }
        for (a, b) in pairs(OWL_EQUIV_CLASS)? {
            ax.sup.entry(a.clone()).or_default().push(b.clone());
            ax.sup.entry(b).or_default().push(a);
        }
        ax.disjoint = pairs(OWL_DISJOINT_WITH)?;
        ax.complements = pairs(OWL_COMPLEMENT_OF)?;
        let mut inter: HashMap<Term, Vec<Term>> = HashMap::new();
        for r in self.select(
            &format!(
                "SELECT ?c ?m WHERE {{ ?c <{OWL_INTERSECTION_OF}> ?l . ?l <{RDF_REST}>*/<{RDF_FIRST}> ?m }}"
            ),
            &["c", "m"],
        )? {
            if let (Some(c), Some(m)) = (&r[0], &r[1]) {
                inter.entry(c.clone()).or_default().push(m.clone());
            }
        }
        ax.intersections = inter.into_iter().collect();
        // Datatype restrictions: the base, then each facet of the list.
        let mut restr: HashMap<Term, Option<(Dt, Vec<Facet>)>> = HashMap::new();
        for r in self.select(
            &format!(
                "SELECT ?d ?base ?f ?v WHERE {{ ?d <{OWL_ON_DATATYPE}> ?base . \
                 OPTIONAL {{ ?d <{OWL_WITH_RESTRICTIONS}> ?l . ?l <{RDF_REST}>*/<{RDF_FIRST}> ?r . ?r ?f ?v }} }}"
            ),
            &["d", "base", "f", "v"],
        )? {
            let (Some(d), Some(base)) = (&r[0], &r[1]) else { continue };
            let base = match base {
                Term::NamedNode(n) => Dt::from_rl_iri(n.as_str()),
                _ => None,
            };
            let entry = restr
                .entry(d.clone())
                .or_insert_with(|| base.map(|b| (b, Vec::new())));
            match (&r[2], &r[3]) {
                (Some(Term::NamedNode(f)), Some(Term::Literal(v))) => {
                    if let Some((_, facets)) = entry {
                        match datatypes::facet_from(f.as_str(), v) {
                            Some(facet) => {
                                if !facets.contains(&facet) {
                                    facets.push(facet)
                                }
                            }
                            // A facet that cannot be read leaves the
                            // restriction unknown, never wider.
                            None => *entry = None,
                        }
                    }
                }
                (None, None) => {}
                _ => *entry = None,
            }
        }
        ax.restrictions = restr;
        Ok(ax)
    }

    /// The `prp-rng` and `cls-avf` classes of literal objects, by value.
    fn literal_seeds(&self) -> Result<HashMap<Literal, Vec<Term>>, ReasoningError> {
        let mut seeds: HashMap<Literal, Vec<Term>> = HashMap::new();
        let queries = [
            format!(
                "SELECT DISTINCT ?lt ?c WHERE {{ ?p <{RDFS_RANGE}> ?c . ?x ?p ?lt . FILTER(isLiteral(?lt)) }}"
            ),
            format!(
                "SELECT DISTINCT ?lt ?c WHERE {{ ?r <{OWL_ALL_VALUES_FROM}> ?c . ?r <{OWL_ON_PROPERTY}> ?p . \
                 ?u <{RDF_TYPE}> ?r . ?u ?p ?lt . FILTER(isLiteral(?lt)) }}"
            ),
        ];
        for q in &queries {
            for r in self.select(q, &["lt", "c"])? {
                if let (Some(Term::Literal(lt)), Some(c)) = (&r[0], &r[1]) {
                    seeds.entry(lt.clone()).or_default().push(c.clone());
                }
            }
        }
        // Equal values share their classes (`dt-eq` with `eq-rep-s`).
        let mut by_value: HashMap<Value, Vec<Term>> = HashMap::new();
        for (lt, cs) in &seeds {
            if let Some(v) = known_value(lt) {
                by_value.entry(v).or_default().extend(cs.iter().cloned());
            }
        }
        for (lt, cs) in seeds.iter_mut() {
            if let Some(v) = known_value(lt) {
                if let Some(all) = by_value.get(&v) {
                    *cs = all.clone();
                }
            }
        }
        Ok(seeds)
    }

    /// Whether the triple is already in scope (asserted or derived).
    fn in_scope(&self, s: &NamedOrBlankNode, p: &NamedNode, o: &Term) -> bool {
        self.literal_graphs().iter().any(|g| {
            self.store
                .store()
                .contains(QuadRef::new(s, p, o, g.as_ref()))
                .unwrap_or(false)
        })
    }

    /// The Table 8 pass of one fixed-point round: writes the literal variants
    /// (`dt-eq` + `eq-rep-o`) and the `cls-svf1` types data values give their
    /// subjects. Returns how many triples it added.
    pub(super) fn literal_pass(&self) -> Result<usize, ReasoningError> {
        let scan = self.scan_literals()?;
        let target = NamedNode::new(self.target_graph.clone())
            .map_err(|e| ReasoningError::Store(e.to_string()))?;
        let mut new: HashSet<Quad> = HashSet::new();

        // dt-eq + eq-rep-o: every triple with one variant as object, with
        // each other variant.
        for group in &scan.variants {
            for lit in group {
                let obj: Term = lit.clone().into();
                for g in self.literal_graphs() {
                    for quad in self.store.store().quads_for_pattern(
                        None,
                        None,
                        Some(obj.as_ref()),
                        Some(g.as_ref()),
                    ) {
                        let quad = quad.map_err(|e| ReasoningError::Store(e.to_string()))?;
                        for other in group.iter().filter(|o| *o != lit) {
                            let o: Term = other.clone().into();
                            if !self.in_scope(&quad.subject, &quad.predicate, &o) {
                                new.insert(Quad::new(
                                    quad.subject.clone(),
                                    quad.predicate.clone(),
                                    o,
                                    target.clone(),
                                ));
                            }
                        }
                    }
                }
            }
        }

        // cls-svf1 with a data value: ?u ?p ?lt, ?lt of the filler class.
        let candidates = self.select(
            &format!(
                "SELECT DISTINCT ?u ?r ?y ?lt WHERE {{ ?r <{OWL_SOME_VALUES_FROM}> ?y . \
                 ?r <{OWL_ON_PROPERTY}> ?p . ?u ?p ?lt . FILTER(isLiteral(?lt)) }}"
            ),
            &["u", "r", "y", "lt"],
        )?;
        if !candidates.is_empty() {
            let ax = self.class_axioms()?;
            let seeds = self.literal_seeds()?;
            let mut memo: HashMap<Literal, HashSet<Term>> = HashMap::new();
            let rdf_type = NamedNode::new_unchecked(RDF_TYPE);
            for r in candidates {
                let (Some(u), Some(class), Some(y), Some(Term::Literal(lt))) =
                    (&r[0], &r[1], &r[2], &r[3])
                else {
                    continue;
                };
                let Some(v) = known_value(lt) else { continue };
                let classes = memo.entry(lt.clone()).or_insert_with(|| {
                    ax.closure(&v, seeds.get(lt).cloned().unwrap_or_default())
                });
                if !classes.contains(y) {
                    continue;
                }
                let subject = match u {
                    Term::NamedNode(n) => NamedOrBlankNode::from(n.clone()),
                    Term::BlankNode(b) => b.clone().into(),
                    _ => continue,
                };
                if !self.in_scope(&subject, &rdf_type, class) {
                    new.insert(Quad::new(subject, rdf_type.clone(), class.clone(), target.clone()));
                }
            }
        }

        let n = new.len();
        if n > 0 {
            self.store.insert_quads(new.into_iter().collect())?;
        }
        Ok(n)
    }

    /// The Table 8 inconsistencies (and the class-axiom clashes of literals):
    /// an ill-typed literal, a literal typed outside its datatype
    /// (`dt-not-type`), two different values equated by `prp-fp`,
    /// `cls-maxc2` or `cls-maxqc3/4` (`dt-diff` + `eq-diff1`), and a literal
    /// in `owl:Nothing`, two disjoint or complementary classes, or counted by
    /// a `owl:maxQualifiedCardinality 0`.
    pub(super) fn check_literals(&self) -> Result<(), ReasoningError> {
        let scan = self.scan_literals()?;
        if let Some(lit) = &scan.ill_typed {
            return Err(ReasoningError::inconsistency(
                "dt-not-type",
                format!("{lit} is not in the lexical space of its datatype"),
            ));
        }
        let ax = self.class_axioms()?;
        // Every literal has the classes of its datatypes.
        for lit in scan.signatures.values() {
            if let Some(v) = known_value(lit) {
                let classes = ax.closure(&v, Vec::new());
                if let Some((rule, detail)) = ax.clash(lit, &v, &classes) {
                    return Err(ReasoningError::inconsistency(&rule, detail));
                }
            }
        }
        // Literals typed by a range or a universal restriction.
        let seeds = self.literal_seeds()?;
        let mut memo: HashMap<Literal, HashSet<Term>> = HashMap::new();
        for (lit, cs) in &seeds {
            if let Some(v) = known_value(lit) {
                let classes = ax.closure(&v, cs.iter().cloned());
                if let Some((rule, detail)) = ax.clash(lit, &v, &classes) {
                    return Err(ReasoningError::inconsistency(&rule, detail));
                }
                memo.insert(lit.clone(), classes);
            }
        }
        let mut classes_of = |lit: &Literal| -> Option<HashSet<Term>> {
            let v = known_value(lit)?;
            Some(
                memo.entry(lit.clone())
                    .or_insert_with(|| ax.closure(&v, seeds.get(lit).cloned().unwrap_or_default()))
                    .clone(),
            )
        };

        // dt-diff against the rules that equate two literals.
        let pair_rules = [
            (
                "prp-fp",
                format!(
                    "SELECT DISTINCT ?a ?b WHERE {{ ?p <{RDF_TYPE}> <{OWL_FUNCTIONAL_PROP}> . \
                     ?x ?p ?a . ?x ?p ?b . FILTER(isLiteral(?a) && isLiteral(?b) && !sameTerm(?a, ?b)) }}"
                ),
            ),
            (
                "cls-maxc2",
                format!(
                    "SELECT DISTINCT ?a ?b WHERE {{ ?r <{OWL_MAX_CARDINALITY}> {NON_NEG_ONE} . \
                     ?r <{OWL_ON_PROPERTY}> ?p . ?u <{RDF_TYPE}> ?r . ?u ?p ?a . ?u ?p ?b . \
                     FILTER(isLiteral(?a) && isLiteral(?b) && !sameTerm(?a, ?b)) }}"
                ),
            ),
            (
                "cls-maxqc4",
                format!(
                    "SELECT DISTINCT ?a ?b WHERE {{ ?r <{OWL_MAX_QUAL_CARD}> {NON_NEG_ONE} . \
                     ?r <{OWL_ON_PROPERTY}> ?p . ?r <{OWL_ON_CLASS}> <{OWL_THING}> . \
                     ?u <{RDF_TYPE}> ?r . ?u ?p ?a . ?u ?p ?b . \
                     FILTER(isLiteral(?a) && isLiteral(?b) && !sameTerm(?a, ?b)) }}"
                ),
            ),
        ];
        for (rule, q) in &pair_rules {
            for r in self.select(q, &["a", "b"])? {
                if let (Some(Term::Literal(a)), Some(Term::Literal(b))) = (&r[0], &r[1]) {
                    if let (Some(va), Some(vb)) = (known_value(a), known_value(b)) {
                        if va != vb {
                            return Err(ReasoningError::inconsistency(
                                "dt-diff",
                                format!(
                                    "{rule} makes {a} and {b} the same, but they are different values"
                                ),
                            ));
                        }
                    }
                }
            }
        }
        // cls-maxqc3 counts only fillers of the qualifying class.
        for r in self.select(
            &format!(
                "SELECT DISTINCT ?c ?a ?b WHERE {{ ?r <{OWL_MAX_QUAL_CARD}> {NON_NEG_ONE} . \
                 ?r <{OWL_ON_PROPERTY}> ?p . ?r <{OWL_ON_CLASS}> ?c . \
                 ?u <{RDF_TYPE}> ?r . ?u ?p ?a . ?u ?p ?b . \
                 FILTER(isLiteral(?a) && isLiteral(?b) && !sameTerm(?a, ?b)) }}"
            ),
            &["c", "a", "b"],
        )? {
            let (Some(c), Some(Term::Literal(a)), Some(Term::Literal(b))) = (&r[0], &r[1], &r[2])
            else {
                continue;
            };
            let (Some(va), Some(vb)) = (known_value(a), known_value(b)) else { continue };
            if va == vb {
                continue;
            }
            let both = classes_of(a).is_some_and(|s| s.contains(c))
                && classes_of(b).is_some_and(|s| s.contains(c));
            if both {
                return Err(ReasoningError::inconsistency(
                    "dt-diff",
                    format!("cls-maxqc3 makes {a} and {b} the same, but they are different values"),
                ));
            }
        }
        // cls-maxqc1: a qualifying literal filler of a maximum of zero.
        for r in self.select(
            &format!(
                "SELECT DISTINCT ?c ?lt WHERE {{ ?r <{OWL_MAX_QUAL_CARD}> {NON_NEG_ZERO} . \
                 ?r <{OWL_ON_PROPERTY}> ?p . ?r <{OWL_ON_CLASS}> ?c . \
                 ?u <{RDF_TYPE}> ?r . ?u ?p ?lt . FILTER(isLiteral(?lt)) }}"
            ),
            &["c", "lt"],
        )? {
            if let (Some(c), Some(Term::Literal(lt))) = (&r[0], &r[1]) {
                if classes_of(lt).is_some_and(|s| s.contains(c)) {
                    return Err(ReasoningError::inconsistency(
                        "cls-maxqc1",
                        format!("{lt} is a qualifying value of a maximum qualified cardinality of 0"),
                    ));
                }
            }
        }
        Ok(())
    }
}
