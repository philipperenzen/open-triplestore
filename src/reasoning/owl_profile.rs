//! OWL 2 DL profile check: the typing constraints (Structural Specification
//! §5.8.1) and the global restrictions (§11) on an ontology built by
//! [`super::owl_mapping`].
//!
//! What is checked:
//! * typing — no IRI is two kinds of property, or both a class and a datatype;
//!   no reserved-vocabulary IRI is declared or used as a class;
//! * `owl:topDataProperty` only as a super data property;
//! * datatypes — each one used is `rdfs:Literal`, in the OWL 2 datatype map, or
//!   defined by exactly one acyclic `DatatypeDefinition`; facets are OWL 2 facets;
//! * simple object properties in cardinality and self restrictions and in
//!   functional, inverse-functional, irreflexive, asymmetric and disjointness
//!   axioms (§11.1);
//! * a regular property hierarchy (§11.2);
//! * the restrictions on anonymous individuals (§11.2).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use super::common::ProfileViolation;
use super::owl_mapping::is_builtin;
use super::owl_model::*;

/// Report at most this many violations; the last entry says how many more.
const MAX_REPORTED: usize = 100;

/// Every OWL 2 DL violation found in `ont`. Empty: the ontology is in OWL 2 DL.
pub fn check(ont: &Ontology) -> Vec<ProfileViolation> {
    let mut v = Checker::default();
    v.typing(ont);
    v.top_data_property(ont);
    v.datatypes(ont);
    v.simple_roles(ont);
    v.regularity(ont);
    v.anonymous_individuals(ont);
    let mut out = v.out;
    if out.len() > MAX_REPORTED {
        let more = out.len() - MAX_REPORTED;
        out.truncate(MAX_REPORTED);
        out.push(ProfileViolation::new(
            "more-violations",
            format!("{more} further violation(s) not listed"),
        ));
    }
    out
}

#[derive(Default)]
struct Checker {
    out: Vec<ProfileViolation>,
    seen: HashSet<(String, String)>,
}

impl Checker {
    fn push(&mut self, rule: &str, detail: String) {
        if self.seen.insert((rule.to_string(), detail.clone())) {
            self.out.push(ProfileViolation::new(rule, detail));
        }
    }

    // ── typing constraints ───────────────────────────────────────────────────

    fn typing(&mut self, ont: &Ontology) {
        let mut kinds: BTreeMap<&str, BTreeSet<EntityKind>> = BTreeMap::new();
        for a in &ont.axioms {
            if let AxiomKind::Declaration(k, iri) = &a.kind {
                kinds.entry(iri).or_default().insert(*k);
                if is_reserved(iri) && !is_builtin(iri, *k) {
                    self.push(
                        "reserved-vocabulary",
                        format!(
                            "<{iri}> is reserved vocabulary and cannot be declared as a {}",
                            k.fs_name()
                        ),
                    );
                }
            }
        }
        use EntityKind::*;
        for (iri, ks) in &kinds {
            let props: Vec<&str> = [ObjectProperty, DataProperty, AnnotationProperty]
                .iter()
                .filter(|k| ks.contains(k))
                .map(|k| k.fs_name())
                .collect();
            if props.len() > 1 {
                self.push(
                    "property-punning",
                    format!("<{iri}> is used as {}", props.join(" and ")),
                );
            }
            if ks.contains(&Class) && ks.contains(&Datatype) {
                self.push(
                    "class-datatype-punning",
                    format!("<{iri}> is used as both a class and a datatype"),
                );
            }
        }
        let mut classes = BTreeSet::new();
        for a in &ont.axioms {
            visit_classes(&a.kind, &mut |c| {
                classes.insert(c.to_string());
            });
        }
        for c in classes {
            if is_reserved(&c) && c != OWL_THING && c != OWL_NOTHING {
                self.push(
                    "reserved-vocabulary",
                    format!("<{c}> is reserved vocabulary and cannot be used as a class"),
                );
            }
        }
    }

    fn top_data_property(&mut self, ont: &Ontology) {
        for a in &ont.axioms {
            let bad = match &a.kind {
                AxiomKind::SubDataPropertyOf(sub, _) => sub == OWL_TOP_DATA_PROPERTY,
                AxiomKind::Declaration(..) => false,
                other => data_properties_of(other).contains(&OWL_TOP_DATA_PROPERTY),
            };
            if bad {
                self.push(
                    "top-data-property",
                    "owl:topDataProperty may only be a super data property".into(),
                );
            }
        }
    }

    // ── datatypes ────────────────────────────────────────────────────────────

    fn datatypes(&mut self, ont: &Ontology) {
        let mut defs: HashMap<&str, Vec<&DataRange>> = HashMap::new();
        for a in &ont.axioms {
            if let AxiomKind::DatatypeDefinition(dt, dr) = &a.kind {
                defs.entry(dt).or_default().push(dr);
            }
        }
        let mut used: BTreeSet<String> = BTreeSet::new();
        let mut facets: BTreeSet<String> = BTreeSet::new();
        for a in &ont.axioms {
            visit_data(
                &a.kind,
                &mut |dt| {
                    used.insert(dt.to_string());
                },
                &mut |f| {
                    facets.insert(f.to_string());
                },
            );
        }
        for dt in &used {
            let builtin = DATATYPE_MAP.contains(&dt.as_str());
            match defs.get(dt.as_str()).map(Vec::len).unwrap_or(0) {
                0 if !builtin => self.push(
                    "unknown-datatype",
                    format!(
                        "<{dt}> is not in the OWL 2 datatype map and has no DatatypeDefinition"
                    ),
                ),
                n if n > 0 && builtin => self.push(
                    "datatype-redefined",
                    format!("<{dt}> is in the OWL 2 datatype map and cannot be redefined"),
                ),
                n if n > 1 => self.push(
                    "datatype-redefined",
                    format!("<{dt}> has {n} DatatypeDefinition axioms"),
                ),
                _ => {}
            }
        }
        for f in facets {
            if !FACETS.contains(&f.as_str()) {
                self.push("unknown-facet", format!("<{f}> is not an OWL 2 facet"));
            }
        }
        // Acyclic definitions: DFS over "defined in terms of".
        let deps: HashMap<&str, BTreeSet<String>> = defs
            .iter()
            .map(|(dt, drs)| {
                let mut d = BTreeSet::new();
                for dr in drs {
                    datatypes_in(dr, &mut |x| {
                        d.insert(x.to_string());
                    });
                }
                (*dt, d)
            })
            .collect();
        for dt in deps.keys() {
            let mut stack: Vec<String> = deps[dt].iter().cloned().collect();
            let mut seen = HashSet::new();
            while let Some(x) = stack.pop() {
                if x == *dt {
                    self.push(
                        "cyclic-datatype-definition",
                        format!("<{dt}> is defined in terms of itself"),
                    );
                    break;
                }
                if seen.insert(x.clone()) {
                    if let Some(next) = deps.get(x.as_str()) {
                        stack.extend(next.iter().cloned());
                    }
                }
            }
        }
    }

    // ── simple roles ─────────────────────────────────────────────────────────

    fn simple_roles(&mut self, ont: &Ontology) {
        let h = Hierarchy::new(ont);
        let non_simple = h.non_simple();
        let mut need: Vec<(ObjectProp, &'static str)> = Vec::new();
        for a in &ont.axioms {
            match &a.kind {
                AxiomKind::FunctionalObjectProperty(p) => {
                    need.push((p.clone(), "FunctionalObjectProperty"))
                }
                AxiomKind::InverseFunctionalObjectProperty(p) => {
                    need.push((p.clone(), "InverseFunctionalObjectProperty"))
                }
                AxiomKind::IrreflexiveObjectProperty(p) => {
                    need.push((p.clone(), "IrreflexiveObjectProperty"))
                }
                AxiomKind::AsymmetricObjectProperty(p) => {
                    need.push((p.clone(), "AsymmetricObjectProperty"))
                }
                AxiomKind::DisjointObjectProperties(ps) => {
                    for p in ps {
                        need.push((p.clone(), "DisjointObjectProperties"));
                    }
                }
                _ => {}
            }
            visit_exprs(&a.kind, &mut |ce| match ce {
                ClassExpr::MinCardinality(_, p, _)
                | ClassExpr::MaxCardinality(_, p, _)
                | ClassExpr::ExactCardinality(_, p, _) => {
                    need.push((p.clone(), "an object cardinality restriction"))
                }
                ClassExpr::HasSelf(p) => need.push((p.clone(), "ObjectHasSelf")),
                _ => {}
            });
        }
        for (p, place) in need {
            if non_simple.contains(&p) {
                self.push(
                    "non-simple-property",
                    format!(
                        "<{}> is not simple (it has a transitive or chain-defined sub-property) and cannot be used in {place}",
                        p.name()
                    ),
                );
            }
        }
    }

    // ── property hierarchy (regularity) ─────────────────────────────────────

    fn regularity(&mut self, ont: &Ontology) {
        let h = Hierarchy::new(ont);
        // Required strict pairs (smaller, larger), by property name: the
        // order treats OPE and INV(OPE) alike on the left, and both sides
        // collapse to names here.
        let mut less: BTreeSet<(String, String)> = BTreeSet::new();
        for a in &ont.axioms {
            let AxiomKind::SubObjectPropertyChain(chain, sup) = &a.kind else {
                continue;
            };
            let n = chain.len();
            if n < 2 || sup.name() == OWL_TOP_OBJECT_PROPERTY {
                continue;
            }
            if n == 2 && chain[0] == *sup && chain[1] == *sup {
                continue;
            }
            let range: Vec<&ObjectProp> = if chain[0] == *sup {
                chain[1..].iter().collect()
            } else if chain[n - 1] == *sup {
                chain[..n - 1].iter().collect()
            } else {
                chain.iter().collect()
            };
            for s in range {
                less.insert((s.name().to_string(), sup.name().to_string()));
            }
        }
        // Transitive closure of `less`.
        let mut succ: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (a, b) in &less {
            succ.entry(a.clone()).or_default().insert(b.clone());
        }
        for start in succ.keys().cloned().collect::<Vec<_>>() {
            let mut seen = BTreeSet::new();
            let mut q: VecDeque<String> = succ[&start].iter().cloned().collect();
            while let Some(x) = q.pop_front() {
                if !seen.insert(x.clone()) {
                    continue;
                }
                if x == start {
                    self.push(
                        "irregular-property-hierarchy",
                        format!("<{start}> is defined through a property chain in terms of itself"),
                    );
                    break;
                }
                // The hierarchy may not contradict the order: x > start, so
                // x →* start (either direction) is not allowed.
                let xs = ObjectProp::Named(x.clone());
                if h.reaches(&xs, &ObjectProp::Named(start.clone()))
                    || h.reaches(&xs, &ObjectProp::Inverse(start.clone()))
                {
                    self.push(
                        "irregular-property-hierarchy",
                        format!(
                            "<{x}> is defined by a chain over <{start}> but is also its sub-property"
                        ),
                    );
                }
                if let Some(n) = succ.get(&x) {
                    q.extend(n.iter().cloned());
                }
            }
        }
    }

    // ── anonymous individuals ────────────────────────────────────────────────

    fn anonymous_individuals(&mut self, ont: &Ontology) {
        let anon = |i: &Individual| matches!(i, Individual::Anonymous(_));
        let mut edges: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut named_links: HashMap<String, usize> = HashMap::new();
        let mut nodes: BTreeSet<String> = BTreeSet::new();
        for a in &ont.axioms {
            match &a.kind {
                AxiomKind::SameIndividual(is) | AxiomKind::DifferentIndividuals(is)
                    if is.iter().any(anon) =>
                {
                    self.push(
                        "anonymous-individual",
                        "an anonymous individual cannot occur in SameIndividual or DifferentIndividuals"
                            .into(),
                    )
                }
                AxiomKind::NegativeObjectPropertyAssertion(_, x, y) if anon(x) || anon(y) => self.push(
                    "anonymous-individual",
                    "an anonymous individual cannot occur in a negative property assertion".into(),
                ),
                AxiomKind::NegativeDataPropertyAssertion(_, x, _) if anon(x) => self.push(
                    "anonymous-individual",
                    "an anonymous individual cannot occur in a negative property assertion".into(),
                ),
                AxiomKind::ObjectPropertyAssertion(_, x, y) => match (x, y) {
                    (Individual::Anonymous(a), Individual::Anonymous(b)) => {
                        nodes.insert(a.clone());
                        nodes.insert(b.clone());
                        let key = if a <= b {
                            (a.clone(), b.clone())
                        } else {
                            (b.clone(), a.clone())
                        };
                        *edges.entry(key).or_default() += 1;
                    }
                    (Individual::Anonymous(a), Individual::Named(_))
                    | (Individual::Named(_), Individual::Anonymous(a)) => {
                        nodes.insert(a.clone());
                        *named_links.entry(a.clone()).or_default() += 1;
                    }
                    _ => {}
                },
                _ => {}
            }
            visit_exprs(&a.kind, &mut |ce| {
                let bad = match ce {
                    ClassExpr::OneOf(is) => is.iter().any(anon),
                    ClassExpr::HasValue(_, i) => anon(i),
                    _ => false,
                };
                if bad {
                    self.push(
                        "anonymous-individual",
                        "an anonymous individual cannot occur in ObjectOneOf or ObjectHasValue"
                            .into(),
                    );
                }
            });
        }
        // Forest: union-find; an edge inside one tree closes a cycle.
        let mut parent: HashMap<String, String> =
            nodes.iter().map(|n| (n.clone(), n.clone())).collect();
        fn find(p: &mut HashMap<String, String>, x: &str) -> String {
            let mut r = x.to_string();
            while p[&r] != r {
                r = p[&r].clone();
            }
            p.insert(x.to_string(), r.clone());
            r
        }
        for ((a, b), count) in &edges {
            if *count > 1 {
                self.push(
                    "anonymous-individual",
                    "two anonymous individuals are linked by more than one property assertion"
                        .into(),
                );
            }
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra == rb {
                self.push(
                    "anonymous-individual",
                    "property assertions between anonymous individuals form a cycle".into(),
                );
            } else {
                parent.insert(ra, rb);
            }
        }
        // Each tree needs a member with at most one link to a named individual.
        let mut trees: HashMap<String, bool> = HashMap::new();
        for n in &nodes {
            let root = find(&mut parent, n);
            let ok = named_links.get(n).copied().unwrap_or(0) <= 1;
            let e = trees.entry(root).or_insert(false);
            *e = *e || ok;
        }
        if trees.values().any(|ok| !ok) {
            self.push(
                "anonymous-individual",
                "a tree of anonymous individuals has no member with at most one link to a named individual"
                    .into(),
            );
        }
    }
}

/// The object property hierarchy relation → of §11.1.
struct Hierarchy {
    up: HashMap<ObjectProp, BTreeSet<ObjectProp>>,
    composite: BTreeSet<ObjectProp>,
}

impl Hierarchy {
    fn new(ont: &Ontology) -> Self {
        let mut h = Hierarchy {
            up: HashMap::new(),
            composite: BTreeSet::new(),
        };
        for p in [OWL_TOP_OBJECT_PROPERTY, OWL_BOTTOM_OBJECT_PROPERTY] {
            h.composite.insert(ObjectProp::Named(p.into()));
            h.composite.insert(ObjectProp::Inverse(p.into()));
        }
        for a in &ont.axioms {
            match &a.kind {
                AxiomKind::SubObjectPropertyOf(x, y) => h.edge(x, y),
                AxiomKind::EquivalentObjectProperties(ps) => {
                    for x in ps {
                        for y in ps {
                            if x != y {
                                h.edge(x, y);
                            }
                        }
                    }
                }
                AxiomKind::InverseObjectProperties(x, y) => {
                    h.edge(x, &y.inverse());
                    h.edge(&y.inverse(), x);
                }
                AxiomKind::SymmetricObjectProperty(x) => h.edge(x, &x.inverse()),
                AxiomKind::TransitiveObjectProperty(x) => {
                    h.composite.insert(x.clone());
                    h.composite.insert(x.inverse());
                }
                AxiomKind::SubObjectPropertyChain(c, sup) if c.len() > 1 => {
                    h.composite.insert(sup.clone());
                    h.composite.insert(sup.inverse());
                }
                _ => {}
            }
        }
        h
    }

    fn edge(&mut self, x: &ObjectProp, y: &ObjectProp) {
        self.up.entry(x.clone()).or_default().insert(y.clone());
        self.up.entry(x.inverse()).or_default().insert(y.inverse());
    }

    fn reaches(&self, from: &ObjectProp, to: &ObjectProp) -> bool {
        let mut seen = HashSet::new();
        let mut q = VecDeque::from([from.clone()]);
        while let Some(x) = q.pop_front() {
            if &x == to {
                return true;
            }
            if seen.insert(x.clone()) {
                if let Some(n) = self.up.get(&x) {
                    q.extend(n.iter().cloned());
                }
            }
        }
        false
    }

    /// Every OPE with a composite OPE' →* it.
    fn non_simple(&self) -> HashSet<ObjectProp> {
        let mut out = HashSet::new();
        let mut q: VecDeque<ObjectProp> = self.composite.iter().cloned().collect();
        while let Some(x) = q.pop_front() {
            if out.insert(x.clone()) {
                if let Some(n) = self.up.get(&x) {
                    q.extend(n.iter().cloned());
                }
            }
        }
        out
    }
}

// ── visitors ─────────────────────────────────────────────────────────────────

fn class_exprs_of(k: &AxiomKind) -> Vec<&ClassExpr> {
    use AxiomKind::*;
    match k {
        SubClassOf(a, b) => vec![a, b],
        EquivalentClasses(v) | DisjointClasses(v) | DisjointUnion(_, v) => v.iter().collect(),
        ObjectPropertyDomain(_, c)
        | ObjectPropertyRange(_, c)
        | DataPropertyDomain(_, c)
        | HasKey(c, ..)
        | ClassAssertion(c, _) => vec![c],
        _ => vec![],
    }
}

/// Every class expression in `k`, nested ones included.
fn visit_exprs<'a>(k: &'a AxiomKind, f: &mut dyn FnMut(&'a ClassExpr)) {
    fn walk<'a>(c: &'a ClassExpr, f: &mut dyn FnMut(&'a ClassExpr)) {
        f(c);
        match c {
            ClassExpr::IntersectionOf(v) | ClassExpr::UnionOf(v) => {
                v.iter().for_each(|x| walk(x, f))
            }
            ClassExpr::ComplementOf(x)
            | ClassExpr::SomeValuesFrom(_, x)
            | ClassExpr::AllValuesFrom(_, x) => walk(x, f),
            ClassExpr::MinCardinality(_, _, Some(x))
            | ClassExpr::MaxCardinality(_, _, Some(x))
            | ClassExpr::ExactCardinality(_, _, Some(x)) => walk(x, f),
            _ => {}
        }
    }
    for c in class_exprs_of(k) {
        walk(c, f);
    }
}

fn visit_classes(k: &AxiomKind, f: &mut dyn FnMut(&str)) {
    if let AxiomKind::DisjointUnion(c, _) = k {
        f(c);
    }
    visit_exprs(k, &mut |ce| {
        if let ClassExpr::Class(c) = ce {
            f(c);
        }
    });
}

fn data_ranges_of(k: &AxiomKind) -> Vec<&DataRange> {
    let mut out = Vec::new();
    if let AxiomKind::DataPropertyRange(_, r) | AxiomKind::DatatypeDefinition(_, r) = k {
        out.push(r);
    }
    visit_exprs(k, &mut |ce| match ce {
        ClassExpr::DataSomeValuesFrom(_, r) | ClassExpr::DataAllValuesFrom(_, r) => out.push(r),
        ClassExpr::DataMinCardinality(_, _, Some(r))
        | ClassExpr::DataMaxCardinality(_, _, Some(r))
        | ClassExpr::DataExactCardinality(_, _, Some(r)) => out.push(r),
        _ => {}
    });
    out
}

fn datatypes_in(r: &DataRange, f: &mut dyn FnMut(&str)) {
    match r {
        DataRange::Datatype(d) => f(d),
        DataRange::IntersectionOf(v) | DataRange::UnionOf(v) => {
            v.iter().for_each(|x| datatypes_in(x, f))
        }
        DataRange::ComplementOf(x) => datatypes_in(x, f),
        DataRange::OneOf(ls) => ls.iter().for_each(|l| f(&l.datatype)),
        DataRange::Restriction(d, fs) => {
            f(d);
            fs.iter().for_each(|(_, l)| f(&l.datatype));
        }
    }
}

/// Datatypes (`dt`) and facets (`facet`) used anywhere in `k`, literals'
/// datatypes included.
fn visit_data(k: &AxiomKind, dt: &mut dyn FnMut(&str), facet: &mut dyn FnMut(&str)) {
    for r in data_ranges_of(k) {
        datatypes_in(r, dt);
        fn facets(r: &DataRange, f: &mut dyn FnMut(&str)) {
            match r {
                DataRange::Restriction(_, fs) => fs.iter().for_each(|(x, _)| f(x)),
                DataRange::IntersectionOf(v) | DataRange::UnionOf(v) => {
                    v.iter().for_each(|x| facets(x, f))
                }
                DataRange::ComplementOf(x) => facets(x, f),
                _ => {}
            }
        }
        facets(r, facet);
    }
    let lit = |l: &Literal, dt: &mut dyn FnMut(&str)| {
        // A language-tagged literal is an rdf:PlainLiteral value.
        if l.lang.is_none() {
            dt(&l.datatype)
        }
    };
    match k {
        AxiomKind::Declaration(EntityKind::Datatype, d) => dt(d),
        AxiomKind::DataPropertyAssertion(_, _, l)
        | AxiomKind::NegativeDataPropertyAssertion(_, _, l) => lit(l, dt),
        _ => {}
    }
    visit_exprs(k, &mut |ce| {
        if let ClassExpr::DataHasValue(_, l) = ce {
            lit(l, dt)
        }
    });
}

fn data_properties_of(k: &AxiomKind) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    match k {
        AxiomKind::SubDataPropertyOf(a, b) => {
            out.push(a);
            out.push(b);
        }
        AxiomKind::EquivalentDataProperties(v) | AxiomKind::DisjointDataProperties(v) => {
            out.extend(v.iter().map(String::as_str))
        }
        AxiomKind::DataPropertyDomain(p, _)
        | AxiomKind::DataPropertyRange(p, _)
        | AxiomKind::FunctionalDataProperty(p)
        | AxiomKind::DataPropertyAssertion(p, ..)
        | AxiomKind::NegativeDataPropertyAssertion(p, ..) => out.push(p),
        AxiomKind::HasKey(_, _, ds) => out.extend(ds.iter().map(String::as_str)),
        _ => {}
    }
    visit_exprs(k, &mut |ce| match ce {
        ClassExpr::DataSomeValuesFrom(ps, _) | ClassExpr::DataAllValuesFrom(ps, _) => {
            out.extend(ps.iter().map(String::as_str))
        }
        ClassExpr::DataHasValue(p, _)
        | ClassExpr::DataMinCardinality(_, p, _)
        | ClassExpr::DataMaxCardinality(_, p, _)
        | ClassExpr::DataExactCardinality(_, p, _) => out.push(p),
        _ => {}
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reasoning::owl_mapping::map_triples;
    use oxigraph::io::{RdfFormat, RdfParser};

    fn violations(ttl: &str) -> Vec<ProfileViolation> {
        let triples: Vec<oxigraph::model::Triple> = RdfParser::from_format(RdfFormat::Turtle)
            .for_slice(ttl.as_bytes())
            .map(|q| q.unwrap().into())
            .collect();
        check(&map_triples(&triples).ontology)
    }

    const P: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
                     @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
                     @prefix xsd: <http://www.w3.org/2001/XMLSchema#> . \
                     @prefix ex: <http://example.org/> . ";

    #[test]
    fn plain_ontology_is_in_profile() {
        let v = violations(&format!(
            "{P} ex:A rdfs:subClassOf ex:B . ex:a a ex:A ; ex:r ex:b ; ex:age 3 ."
        ));
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn transitive_sub_property_in_cardinality_is_not_simple() {
        let v = violations(&format!(
            "{P} ex:part a owl:TransitiveProperty ; rdfs:subPropertyOf ex:hasPart .
                 ex:W rdfs:subClassOf [ a owl:Restriction ; owl:onProperty ex:hasPart ;
                   owl:maxCardinality 4 ] ."
        ));
        assert!(v.iter().any(|x| x.rule == "non-simple-property"), "{v:?}");
    }

    #[test]
    fn cyclic_chains_are_irregular_and_self_chains_are_not() {
        let v = violations(&format!(
            "{P} ex:hasUncle owl:propertyChainAxiom ( ex:hasFather ex:hasBrother ) .
                 ex:hasBrother owl:propertyChainAxiom ( ex:hasChild ex:hasUncle ) ."
        ));
        assert!(
            v.iter().any(|x| x.rule == "irregular-property-hierarchy"),
            "{v:?}"
        );
        let v = violations(&format!(
            "{P} ex:hasChild owl:propertyChainAxiom ( ex:hasChild ex:hasSibling ) ."
        ));
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn object_and_data_punning_is_reported() {
        let v = violations(&format!("{P} ex:a ex:p ex:b . ex:c ex:p 4 ."));
        assert!(v.iter().any(|x| x.rule == "property-punning"), "{v:?}");
    }

    #[test]
    fn datatypes_outside_the_map_are_reported() {
        let v = violations(&format!("{P} ex:a ex:born \"2020-01-01\"^^xsd:date ."));
        assert!(v.iter().any(|x| x.rule == "unknown-datatype"), "{v:?}");
    }

    #[test]
    fn anonymous_individual_cycles_are_reported() {
        let v = violations(&format!("{P} _:x ex:r _:y . _:y ex:r _:z . _:z ex:r _:x ."));
        assert!(v.iter().any(|x| x.rule == "anonymous-individual"), "{v:?}");
    }
}
