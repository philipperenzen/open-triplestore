//! A test-only reference evaluator for the OWL 2 RL/RDF rules (OWL 2
//! Profiles §4.3, Tables 4–9), over *generalized* triples: a literal may be
//! a subject, so `dt-type2`, `dt-eq` and `dt-diff` are applied as written.
//!
//! It is deliberately naive (every rule re-joins the whole graph each round)
//! and shares no code with the engine. `eq-ref` is left out, as the engine
//! leaves it out by default (decision D2).
//!
//! Literal values are known only for the small alphabet the differential
//! generator uses (integers, decimals, plain strings, booleans); any other
//! literal equals only itself and has no datatype from the map.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use oxigraph::model::{Literal, NamedNode, Term};

pub type T = (Term, Term, Term);

pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
pub const OWL: &str = "http://www.w3.org/2002/07/owl#";
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

pub fn iri(s: &str) -> Term {
    NamedNode::new_unchecked(s).into()
}
pub fn rdf(l: &str) -> Term {
    iri(&format!("{RDF}{l}"))
}
pub fn rdfs(l: &str) -> Term {
    iri(&format!("{RDFS}{l}"))
}
pub fn owl(l: &str) -> Term {
    iri(&format!("{OWL}{l}"))
}
pub fn xsd(l: &str) -> Term {
    iri(&format!("{XSD}{l}"))
}

/// The OWL 2 RL datatype map, by local name: `rdf:` two, `rdfs:Literal`, and
/// the XSD types.
pub fn rl_datatypes() -> Vec<Term> {
    let x = "decimal integer nonNegativeInteger nonPositiveInteger positiveInteger \
             negativeInteger long int short byte unsignedLong unsignedInt unsignedShort \
             unsignedByte float double string normalizedString token language Name NCName \
             NMTOKEN boolean hexBinary base64Binary anyURI dateTime dateTimeStamp";
    let mut v = vec![rdf("PlainLiteral"), rdf("XMLLiteral"), rdfs("Literal")];
    v.extend(x.split_whitespace().map(xsd));
    v
}

/// A literal's value as a comparable key, for the generator's alphabet.
pub fn value_key(l: &Literal) -> Option<String> {
    let dt = l.datatype().as_str().strip_prefix(XSD)?;
    match dt {
        "integer" | "decimal" => l.value().parse::<f64>().ok().map(|f| format!("num:{f}")),
        "string" => Some(format!("str:{}", l.value())),
        "boolean" => Some(format!("bool:{}", l.value())),
        _ => None,
    }
}

/// The RL datatypes whose value space holds the literal's value
/// (`dt-type2`), for the generator's alphabet.
pub fn types_of(l: &Literal) -> Vec<Term> {
    let Some(key) = value_key(l) else {
        return Vec::new();
    };
    let mut v = vec![rdfs("Literal")];
    if let Some(num) = key.strip_prefix("num:") {
        let f: f64 = num.parse().unwrap();
        v.push(xsd("decimal"));
        if f.fract() == 0.0 {
            // (datatype, min, max) of the integer types
            let bounds: [(&str, f64, f64); 12] = [
                ("integer", f64::MIN, f64::MAX),
                ("nonNegativeInteger", 0.0, f64::MAX),
                ("nonPositiveInteger", f64::MIN, 0.0),
                ("positiveInteger", 1.0, f64::MAX),
                ("negativeInteger", f64::MIN, -1.0),
                ("long", -9.2e18, 9.2e18),
                ("int", -2147483648.0, 2147483647.0),
                ("short", -32768.0, 32767.0),
                ("byte", -128.0, 127.0),
                ("unsignedLong", 0.0, 1.8e19),
                ("unsignedInt", 0.0, 4294967295.0),
                ("unsignedShort", 0.0, 65535.0),
            ];
            for (d, lo, hi) in bounds {
                if f >= lo && f <= hi {
                    v.push(xsd(d));
                }
            }
            if (0.0..=255.0).contains(&f) {
                v.push(xsd("unsignedByte"));
            }
        }
    } else if let Some(s) = key.strip_prefix("str:") {
        v.push(xsd("string"));
        v.push(rdf("PlainLiteral"));
        // The alphabet's strings are single NCName-like words with no
        // leading letter-only subtag, so not xsd:language.
        assert!(
            s.starts_with(|c: char| c.is_ascii_alphabetic())
                && s.chars().all(|c| c.is_ascii_alphanumeric())
                && s.chars().any(|c| c.is_ascii_digit())
        );
        for d in ["normalizedString", "token", "Name", "NCName", "NMTOKEN"] {
            v.push(xsd(d));
        }
    } else {
        v.push(xsd("boolean"));
    }
    v
}

/// A generalized graph with a predicate index, rebuilt each round.
pub struct G {
    pub set: HashSet<T>,
    by_p: HashMap<Term, Vec<(Term, Term)>>,
}

impl G {
    pub fn new(set: HashSet<T>) -> Self {
        let mut by_p: HashMap<Term, Vec<(Term, Term)>> = HashMap::new();
        for (s, p, o) in &set {
            by_p.entry(p.clone())
                .or_default()
                .push((s.clone(), o.clone()));
        }
        G { set, by_p }
    }
    pub fn pairs(&self, p: &Term) -> &[(Term, Term)] {
        self.by_p.get(p).map(|v| v.as_slice()).unwrap_or(&[])
    }
    pub fn all(&self) -> impl Iterator<Item = &T> {
        self.set.iter()
    }
    pub fn has(&self, s: &Term, p: &Term, o: &Term) -> bool {
        self.set.contains(&(s.clone(), p.clone(), o.clone()))
    }
    pub fn objs(&self, s: &Term, p: &Term) -> Vec<Term> {
        self.pairs(p)
            .iter()
            .filter(|(a, _)| a == s)
            .map(|(_, o)| o.clone())
            .collect()
    }
    pub fn subs(&self, p: &Term, o: &Term) -> Vec<Term> {
        self.pairs(p)
            .iter()
            .filter(|(_, b)| b == o)
            .map(|(s, _)| s.clone())
            .collect()
    }
    pub fn one(&self, s: &Term, p: &Term) -> Option<Term> {
        self.objs(s, p).into_iter().next()
    }
    /// The members of the RDF list at `head` (the first `rdf:first` /
    /// `rdf:rest` of each node; the generator builds no branching lists).
    pub fn list(&self, head: &Term) -> Option<Vec<Term>> {
        let (first, rest, nil) = (rdf("first"), rdf("rest"), rdf("nil"));
        let mut out = Vec::new();
        let mut node = head.clone();
        while node != nil {
            out.push(self.one(&node, &first)?);
            node = self.one(&node, &rest)?;
            if out.len() > 64 {
                return None;
            }
        }
        Some(out)
    }
    /// `(x, members)` for each `x p list`.
    pub fn lists(&self, p: &Term) -> Vec<(Term, Vec<Term>)> {
        self.pairs(p)
            .iter()
            .filter_map(|(x, l)| self.list(l).map(|m| (x.clone(), m)))
            .collect()
    }
    /// The individuals typed `c`.
    pub fn instances(&self, c: &Term) -> Vec<Term> {
        self.subs(&rdf("type"), c)
    }
    pub fn typed(&self, x: &Term, c: &Term) -> bool {
        self.has(x, &rdf("type"), c)
    }
}

fn t(s: &Term, p: &Term, o: &Term) -> T {
    (s.clone(), p.clone(), o.clone())
}

fn lit(x: &Term) -> Option<&Literal> {
    match x {
        Term::Literal(l) => Some(l),
        _ => None,
    }
}

/// Table 4 (equality), without eq-ref.
fn table4(g: &G, out: &mut Vec<T>) {
    let same = owl("sameAs");
    let pairs = g.pairs(&same);
    for (x, y) in pairs {
        out.push(t(y, &same, x)); // eq-sym
        for z in g.objs(y, &same) {
            out.push(t(x, &same, &z)); // eq-trans
        }
    }
    for (s, p, o) in g.all() {
        for s2 in g.objs(s, &same) {
            out.push(t(&s2, p, o)); // eq-rep-s
        }
        for p2 in g.objs(p, &same) {
            out.push(t(s, &p2, o)); // eq-rep-p
        }
        for o2 in g.objs(o, &same) {
            out.push(t(s, p, &o2)); // eq-rep-o
        }
    }
}

/// Table 5 (properties), the rules that conclude triples.
fn table5(g: &G, out: &mut Vec<T>) {
    let (ty, same) = (rdf("type"), owl("sameAs"));
    // prp-ap
    for p in ["label", "comment", "seeAlso", "isDefinedBy"] {
        out.push(t(&rdfs(p), &ty, &owl("AnnotationProperty")));
    }
    for p in [
        "deprecated",
        "versionInfo",
        "priorVersion",
        "backwardCompatibleWith",
        "incompatibleWith",
    ] {
        out.push(t(&owl(p), &ty, &owl("AnnotationProperty")));
    }
    for (p, c) in g.pairs(&rdfs("domain")) {
        for (x, _) in g.pairs(p) {
            out.push(t(x, &ty, c)); // prp-dom
        }
    }
    for (p, c) in g.pairs(&rdfs("range")) {
        for (_, y) in g.pairs(p) {
            out.push(t(y, &ty, c)); // prp-rng
        }
    }
    for p in g.instances(&owl("FunctionalProperty")) {
        for (x, y1) in g.pairs(&p) {
            for y2 in g.objs(x, &p) {
                out.push(t(y1, &same, &y2)); // prp-fp
            }
        }
    }
    for p in g.instances(&owl("InverseFunctionalProperty")) {
        for (x1, y) in g.pairs(&p) {
            for x2 in g.subs(&p, y) {
                out.push(t(x1, &same, &x2)); // prp-ifp
            }
        }
    }
    for p in g.instances(&owl("SymmetricProperty")) {
        for (x, y) in g.pairs(&p) {
            out.push(t(y, &p, x)); // prp-symp
        }
    }
    for p in g.instances(&owl("TransitiveProperty")) {
        for (x, y) in g.pairs(&p) {
            for z in g.objs(y, &p) {
                out.push(t(x, &p, &z)); // prp-trp
            }
        }
    }
    for (p1, p2) in g.pairs(&rdfs("subPropertyOf")) {
        for (x, y) in g.pairs(p1) {
            out.push(t(x, p2, y)); // prp-spo1
        }
    }
    for (p, chain) in g.lists(&owl("propertyChainAxiom")) {
        // prp-spo2: walk the chain from every start.
        let mut ends: Vec<(Term, Term)> = g.pairs(&chain[0]).to_vec();
        for link in &chain[1..] {
            ends = ends
                .iter()
                .flat_map(|(u, v)| g.objs(v, link).into_iter().map(move |w| (u.clone(), w)))
                .collect();
        }
        for (u, w) in ends {
            out.push(t(&u, &p, &w));
        }
    }
    for (p1, p2) in g.pairs(&owl("equivalentProperty")) {
        for (x, y) in g.pairs(p1) {
            out.push(t(x, p2, y)); // prp-eqp1
        }
        for (x, y) in g.pairs(p2) {
            out.push(t(x, p1, y)); // prp-eqp2
        }
    }
    for (p1, p2) in g.pairs(&owl("inverseOf")) {
        for (x, y) in g.pairs(p1) {
            out.push(t(y, p2, x)); // prp-inv1
        }
        for (x, y) in g.pairs(p2) {
            out.push(t(y, p1, x)); // prp-inv2
        }
    }
    for (c, keys) in g.lists(&owl("hasKey")) {
        let xs = g.instances(&c);
        for x in &xs {
            for y in &xs {
                let all = keys
                    .iter()
                    .all(|p| g.objs(x, p).iter().any(|z| g.has(y, p, z)));
                if all {
                    out.push(t(x, &same, y)); // prp-key
                }
            }
        }
    }
}

/// Whether a cardinality term is the number `n`.
fn is_n(x: &Term, n: &str) -> bool {
    lit(x).is_some_and(|l| l.value() == n)
}

/// The restrictions `(x, p)` with `x onProperty p`, and `x pred value`.
fn restrictions(g: &G, pred: &str) -> Vec<(Term, Term, Term)> {
    let mut v = Vec::new();
    for (x, val) in g.pairs(&owl(pred)) {
        for p in g.objs(x, &owl("onProperty")) {
            v.push((x.clone(), p, val.clone()));
        }
    }
    v
}

/// Tables 6 and 7 (classes and class axioms), the rules that conclude
/// triples.
fn table67(g: &G, out: &mut Vec<T>) {
    let (ty, same, thing) = (rdf("type"), owl("sameAs"), owl("Thing"));
    out.push(t(&thing, &ty, &owl("Class"))); // cls-thing
    out.push(t(&owl("Nothing"), &ty, &owl("Class"))); // cls-nothing1
    for (c, ms) in g.lists(&owl("intersectionOf")) {
        for y in g.instances(&ms[0]) {
            if ms.iter().all(|m| g.typed(&y, m)) {
                out.push(t(&y, &ty, &c)); // cls-int1
            }
        }
        for y in g.instances(&c) {
            for m in &ms {
                out.push(t(&y, &ty, m)); // cls-int2
            }
        }
    }
    for (c, ms) in g.lists(&owl("unionOf")) {
        for m in &ms {
            for y in g.instances(m) {
                out.push(t(&y, &ty, &c)); // cls-uni
            }
        }
    }
    for (x, p, y) in restrictions(g, "someValuesFrom") {
        for (u, v) in g.pairs(&p) {
            if y == thing || g.typed(v, &y) {
                out.push(t(u, &ty, &x)); // cls-svf1, cls-svf2
            }
        }
    }
    for (x, p, y) in restrictions(g, "allValuesFrom") {
        for u in g.instances(&x) {
            for v in g.objs(&u, &p) {
                out.push(t(&v, &ty, &y)); // cls-avf
            }
        }
    }
    for (x, p, y) in restrictions(g, "hasValue") {
        for u in g.instances(&x) {
            out.push(t(&u, &p, &y)); // cls-hv1
        }
        for u in g.subs(&p, &y) {
            out.push(t(&u, &ty, &x)); // cls-hv2
        }
    }
    for (x, p, n) in restrictions(g, "maxCardinality") {
        if is_n(&n, "1") {
            for u in g.instances(&x) {
                let ys = g.objs(&u, &p);
                for a in &ys {
                    for b in &ys {
                        out.push(t(a, &same, b)); // cls-maxc2
                    }
                }
            }
        }
    }
    for (x, p, n) in restrictions(g, "maxQualifiedCardinality") {
        if !is_n(&n, "1") {
            continue;
        }
        for c in g.objs(&x, &owl("onClass")) {
            for u in g.instances(&x) {
                let ys: Vec<Term> = g
                    .objs(&u, &p)
                    .into_iter()
                    .filter(|y| c == thing || g.typed(y, &c))
                    .collect();
                for a in &ys {
                    for b in &ys {
                        out.push(t(a, &same, b)); // cls-maxqc3, cls-maxqc4
                    }
                }
            }
        }
    }
    for (c, ys) in g.lists(&owl("oneOf")) {
        for y in ys {
            out.push(t(&y, &ty, &c)); // cls-oo
        }
    }
    for (c1, c2) in g.pairs(&rdfs("subClassOf")) {
        for x in g.instances(c1) {
            out.push(t(&x, &ty, c2)); // cax-sco
        }
    }
    for (c1, c2) in g.pairs(&owl("equivalentClass")) {
        for x in g.instances(c1) {
            out.push(t(&x, &ty, c2)); // cax-eqc1
        }
        for x in g.instances(c2) {
            out.push(t(&x, &ty, c1)); // cax-eqc2
        }
    }
}

/// Every literal in the graph, subject or object.
fn literals(g: &G) -> Vec<Literal> {
    let mut seen: HashSet<Literal> = HashSet::new();
    for (s, _, o) in g.all() {
        for x in [s, o] {
            if let Some(l) = lit(x) {
                seen.insert(l.clone());
            }
        }
    }
    let mut v: Vec<Literal> = seen.into_iter().collect();
    v.sort_by_key(|l| l.to_string());
    v
}

/// Table 8 (datatypes), the rules that conclude triples.
fn table8(g: &G, out: &mut Vec<T>) {
    let (ty, same, diff) = (rdf("type"), owl("sameAs"), owl("differentFrom"));
    for d in rl_datatypes() {
        out.push(t(&d, &ty, &rdfs("Datatype"))); // dt-type1
    }
    let lits = literals(g);
    for a in &lits {
        let ta: Term = a.clone().into();
        for d in types_of(a) {
            out.push(t(&ta, &ty, &d)); // dt-type2
        }
        let Some(ka) = value_key(a) else { continue };
        for b in &lits {
            let Some(kb) = value_key(b) else { continue };
            let tb: Term = b.clone().into();
            let p = if ka == kb { &same } else { &diff };
            out.push(t(&ta, p, &tb)); // dt-eq, dt-diff
        }
    }
}

/// Table 9 (schema).
fn table9(g: &G, out: &mut Vec<T>) {
    let (ty, sco, eqc) = (rdf("type"), rdfs("subClassOf"), owl("equivalentClass"));
    let (spo, eqp) = (rdfs("subPropertyOf"), owl("equivalentProperty"));
    let (dom, rng) = (rdfs("domain"), rdfs("range"));
    for c in g.instances(&owl("Class")) {
        out.push(t(&c, &sco, &c)); // scm-cls
        out.push(t(&c, &eqc, &c));
        out.push(t(&c, &sco, &owl("Thing")));
        out.push(t(&owl("Nothing"), &sco, &c));
    }
    for (c1, c2) in g.pairs(&sco) {
        for c3 in g.objs(c2, &sco) {
            out.push(t(c1, &sco, &c3)); // scm-sco
        }
        if g.has(c2, &sco, c1) {
            out.push(t(c1, &eqc, c2)); // scm-eqc2
        }
    }
    for (c1, c2) in g.pairs(&eqc) {
        out.push(t(c1, &sco, c2)); // scm-eqc1
        out.push(t(c2, &sco, c1));
    }
    for kind in ["ObjectProperty", "DatatypeProperty"] {
        for p in g.instances(&owl(kind)) {
            out.push(t(&p, &spo, &p)); // scm-op, scm-dp
            out.push(t(&p, &eqp, &p));
        }
    }
    for (p1, p2) in g.pairs(&spo) {
        for p3 in g.objs(p2, &spo) {
            out.push(t(p1, &spo, &p3)); // scm-spo
        }
        if g.has(p2, &spo, p1) {
            out.push(t(p1, &eqp, p2)); // scm-eqp2
        }
        for c in g.objs(p2, &dom) {
            out.push(t(p1, &dom, &c)); // scm-dom2
        }
        for c in g.objs(p2, &rng) {
            out.push(t(p1, &rng, &c)); // scm-rng2
        }
    }
    for (p1, p2) in g.pairs(&eqp) {
        out.push(t(p1, &spo, p2)); // scm-eqp1
        out.push(t(p2, &spo, p1));
    }
    for (p, c1) in g.pairs(&dom) {
        for c2 in g.objs(c1, &sco) {
            out.push(t(p, &dom, &c2)); // scm-dom1
        }
    }
    for (p, c1) in g.pairs(&rng) {
        for c2 in g.objs(c1, &sco) {
            out.push(t(p, &rng, &c2)); // scm-rng1
        }
    }
    let hv = restrictions(g, "hasValue");
    let svf = restrictions(g, "someValuesFrom");
    let avf = restrictions(g, "allValuesFrom");
    for (c1, p1, i1) in &hv {
        for (c2, p2, i2) in &hv {
            if i1 == i2 && g.has(p1, &spo, p2) {
                out.push(t(c1, &sco, c2)); // scm-hv
            }
        }
    }
    for (c1, p1, y1) in &svf {
        for (c2, p2, y2) in &svf {
            if p1 == p2 && g.has(y1, &sco, y2) {
                out.push(t(c1, &sco, c2)); // scm-svf1
            }
            if y1 == y2 && g.has(p1, &spo, p2) {
                out.push(t(c1, &sco, c2)); // scm-svf2
            }
        }
    }
    for (c1, p1, y1) in &avf {
        for (c2, p2, y2) in &avf {
            if p1 == p2 && g.has(y1, &sco, y2) {
                out.push(t(c1, &sco, c2)); // scm-avf1
            }
            if y1 == y2 && g.has(p1, &spo, p2) {
                out.push(t(c2, &sco, c1)); // scm-avf2
            }
        }
    }
    for (c, ms) in g.lists(&owl("intersectionOf")) {
        for m in ms {
            out.push(t(&c, &sco, &m)); // scm-int
        }
    }
    for (c, ms) in g.lists(&owl("unionOf")) {
        for m in ms {
            out.push(t(&m, &sco, &c)); // scm-uni
        }
    }
    let _ = ty;
}

/// The first rule that derives `false` on the closure `g`, if any.
pub fn clash(g: &G) -> Option<&'static str> {
    let (ty, same, diff) = (rdf("type"), owl("sameAs"), owl("differentFrom"));
    let thing = owl("Thing");
    let some_pair = |ys: &[Term]| -> bool {
        ys.iter()
            .enumerate()
            .any(|(i, a)| ys.iter().skip(i + 1).any(|b| g.has(a, &same, b)))
    };
    if g.pairs(&same).iter().any(|(x, y)| g.has(x, &diff, y)) {
        return Some("eq-diff1");
    }
    for x in g.instances(&owl("AllDifferent")) {
        for (pred, rule) in [("members", "eq-diff2"), ("distinctMembers", "eq-diff3")] {
            for l in g.objs(&x, &owl(pred)) {
                if g.list(&l).is_some_and(|ys| some_pair(&ys)) {
                    return Some(rule);
                }
            }
        }
    }
    for p in g.instances(&owl("IrreflexiveProperty")) {
        if g.pairs(&p).iter().any(|(x, y)| x == y) {
            return Some("prp-irp");
        }
    }
    for p in g.instances(&owl("AsymmetricProperty")) {
        if g.pairs(&p).iter().any(|(x, y)| g.has(y, &p, x)) {
            return Some("prp-asyp");
        }
    }
    for (p1, p2) in g.pairs(&owl("propertyDisjointWith")) {
        if g.pairs(p1).iter().any(|(x, y)| g.has(x, p2, y)) {
            return Some("prp-pdw");
        }
    }
    for x in g.instances(&owl("AllDisjointProperties")) {
        for l in g.objs(&x, &owl("members")) {
            let ps = g.list(&l).unwrap_or_default();
            for (i, a) in ps.iter().enumerate() {
                for b in ps.iter().skip(i + 1) {
                    if g.pairs(a).iter().any(|(u, y)| g.has(u, b, y)) {
                        return Some("prp-adp");
                    }
                }
            }
        }
    }
    for (x, i1) in g.pairs(&owl("sourceIndividual")) {
        for p in g.objs(x, &owl("assertionProperty")) {
            for (pred, rule) in [
                ("targetIndividual", "prp-npa1"),
                ("targetValue", "prp-npa2"),
            ] {
                if g.objs(x, &owl(pred)).iter().any(|i2| g.has(i1, &p, i2)) {
                    return Some(rule);
                }
            }
        }
    }
    if !g.instances(&owl("Nothing")).is_empty() {
        return Some("cls-nothing2");
    }
    for (pred, rule) in [("complementOf", "cls-com"), ("disjointWith", "cax-dw")] {
        for (c1, c2) in g.pairs(&owl(pred)) {
            if g.instances(c1).iter().any(|x| g.typed(x, c2)) {
                return Some(rule);
            }
        }
    }
    for (x, p, n) in restrictions(g, "maxCardinality") {
        if is_n(&n, "0") && g.instances(&x).iter().any(|u| !g.objs(u, &p).is_empty()) {
            return Some("cls-maxc1");
        }
    }
    for (x, p, n) in restrictions(g, "maxQualifiedCardinality") {
        if !is_n(&n, "0") {
            continue;
        }
        for c in g.objs(&x, &owl("onClass")) {
            let hit = g
                .instances(&x)
                .iter()
                .any(|u| g.objs(u, &p).iter().any(|y| c == thing || g.typed(y, &c)));
            if hit {
                return Some(if c == thing {
                    "cls-maxqc2"
                } else {
                    "cls-maxqc1"
                });
            }
        }
    }
    for x in g.instances(&owl("AllDisjointClasses")) {
        for l in g.objs(&x, &owl("members")) {
            let cs = g.list(&l).unwrap_or_default();
            for (i, a) in cs.iter().enumerate() {
                if cs
                    .iter()
                    .skip(i + 1)
                    .any(|b| g.instances(a).iter().any(|z| g.typed(z, b)))
                {
                    return Some("cax-adc");
                }
            }
        }
    }
    let map = rl_datatypes();
    for l in literals(g) {
        if value_key(&l).is_none() {
            continue;
        }
        let lt: Term = l.clone().into();
        let types = types_of(&l);
        if g.objs(&lt, &ty)
            .iter()
            .any(|d| map.contains(d) && !types.contains(d))
        {
            return Some("dt-not-type");
        }
    }
    None
}

/// The closure of `input` under the rules, or the rule that derives `false`.
pub fn closure(input: HashSet<T>) -> Result<G, &'static str> {
    let mut g = G::new(input);
    for _ in 0..500 {
        let mut out = Vec::new();
        table4(&g, &mut out);
        table5(&g, &mut out);
        table67(&g, &mut out);
        table8(&g, &mut out);
        table9(&g, &mut out);
        let before = g.set.len();
        let mut set = std::mem::take(&mut g.set);
        set.extend(out);
        let grew = set.len() > before;
        g = G::new(set);
        if !grew {
            return match clash(&g) {
                Some(rule) => Err(rule),
                None => Ok(g),
            };
        }
    }
    panic!("the reference closure did not converge");
}
