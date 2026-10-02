//! RDF graph → OWL 2 structural model: the reverse of the OWL 2 RDF mapping
//! (OWL 2 Mapping to RDF Graphs, §3).
//!
//! Every triple is either consumed by the ontology header, a declaration, an
//! expression, an axiom or an annotation, or it is reported in
//! [`Mapped::unmapped`] — a triple that has no OWL 2 DL reading. A DL backend
//! that needs OWL syntax (Konclude) refuses input with unmapped triples rather
//! than reason over part of it.
//!
//! Two documented relaxations, both reported in [`Mapped::warnings`]:
//! * **Typing by use.** OWL 2 DL wants every entity declared. Like the OWL API,
//!   an undeclared IRI gets its kind from where it is used: a predicate with a
//!   literal object is a data property, any other predicate an object property,
//!   an `rdf:type` object a class; `rdfs:Class` reads as `owl:Class`. An IRI
//!   used both ways gets both kinds — punning the profile check then refuses.
//! * **Named restrictions.** `ex:C owl:onProperty …` (or `owl:intersectionOf`
//!   …) on an IRI is read as `EquivalentClasses(ex:C <expression>)`.
//!
//! SWRL rules (`swrl:` vocabulary) are not OWL 2 axioms; they are skipped with a
//! warning, so a DL run never claims to have applied them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use oxigraph::model::{NamedOrBlankNode, Term, Triple};

use super::owl_model::*;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDF_LIST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#List";
const RDF_PROPERTY: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property";
const RDFS_CLASS: &str = "http://www.w3.org/2000/01/rdf-schema#Class";
const RDFS_DATATYPE: &str = "http://www.w3.org/2000/01/rdf-schema#Datatype";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_SUB_PROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";
const RDFS_DOMAIN: &str = "http://www.w3.org/2000/01/rdf-schema#domain";
const RDFS_RANGE: &str = "http://www.w3.org/2000/01/rdf-schema#range";
const SWRL: &str = "http://www.w3.org/2003/11/swrl#";
const SWRLB: &str = "http://www.w3.org/2003/11/swrlb#";

fn owl(local: &str) -> String {
    format!("{OWL}{local}")
}

/// What the mapping produced.
#[derive(Debug, Clone, Default)]
pub struct Mapped {
    pub ontology: Ontology,
    /// Triples with no OWL 2 DL reading.
    pub unmapped: Vec<Triple>,
    /// Relaxations applied and things skipped (SWRL, imports, unused expressions).
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Node {
    Iri(String),
    Blank(String),
    Lit(Literal),
}

impl Node {
    fn iri(&self) -> Option<&str> {
        match self {
            Node::Iri(i) => Some(i),
            _ => None,
        }
    }
    fn is_blank(&self) -> bool {
        matches!(self, Node::Blank(_))
    }
}

fn subject_node(s: &NamedOrBlankNode) -> Node {
    match s {
        NamedOrBlankNode::NamedNode(n) => Node::Iri(n.as_str().to_string()),
        NamedOrBlankNode::BlankNode(b) => Node::Blank(b.as_str().to_string()),
    }
}

fn term_node(t: &Term) -> Option<Node> {
    Some(match t {
        Term::NamedNode(n) => Node::Iri(n.as_str().to_string()),
        Term::BlankNode(b) => Node::Blank(b.as_str().to_string()),
        Term::Literal(l) => Node::Lit(Literal {
            lexical: l.value().to_string(),
            datatype: l.datatype().as_str().to_string(),
            lang: l.language().map(str::to_string),
        }),
        #[allow(unreachable_patterns)]
        _ => return None,
    })
}

struct T {
    s: Node,
    p: String,
    o: Node,
}

/// Map `triples` to an OWL 2 ontology.
pub fn map_triples(triples: &[Triple]) -> Mapped {
    let mut m = Mapper::new(triples);
    m.run();
    m.finish()
}

struct Mapper<'a> {
    source: &'a [Triple],
    ts: Vec<Option<T>>,
    used: Vec<bool>,
    by_subject: HashMap<Node, Vec<usize>>,
    kinds: BTreeMap<String, BTreeSet<EntityKind>>,
    ce_cache: HashMap<String, Option<ClassExpr>>,
    dr_cache: HashMap<String, Option<DataRange>>,
    /// (s, p, o) of a reified triple → its annotations.
    reified: HashMap<(Node, String, Node), Vec<Annotation>>,
    axioms: Vec<Axiom>,
    warnings: BTreeSet<String>,
    ontology: Ontology,
    ontology_node: Option<Node>,
    /// Triples skipped as SWRL.
    skipped: HashSet<usize>,
}

impl<'a> Mapper<'a> {
    fn new(source: &'a [Triple]) -> Self {
        let mut ts = Vec::with_capacity(source.len());
        let mut by_subject: HashMap<Node, Vec<usize>> = HashMap::new();
        for (i, t) in source.iter().enumerate() {
            let o = term_node(&t.object);
            match o {
                Some(o) => {
                    let s = subject_node(&t.subject);
                    by_subject.entry(s.clone()).or_default().push(i);
                    ts.push(Some(T {
                        s,
                        p: t.predicate.as_str().to_string(),
                        o,
                    }));
                }
                None => ts.push(None),
            }
        }
        Mapper {
            source,
            used: vec![false; ts.len()],
            ts,
            by_subject,
            kinds: BTreeMap::new(),
            ce_cache: HashMap::new(),
            dr_cache: HashMap::new(),
            reified: HashMap::new(),
            axioms: Vec::new(),
            warnings: BTreeSet::new(),
            ontology: Ontology::default(),
            ontology_node: None,
            skipped: HashSet::new(),
        }
    }

    // ── triple access ────────────────────────────────────────────────────────

    fn get(&self, i: usize) -> Option<&T> {
        self.ts[i].as_ref()
    }

    /// Indices of `(subject, predicate, ?)` triples.
    fn find(&self, s: &Node, p: &str) -> Vec<usize> {
        self.by_subject
            .get(s)
            .map(|v| {
                v.iter()
                    .copied()
                    .filter(|&i| self.get(i).is_some_and(|t| t.p == p))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The single object of `(s, p, ?)`, marking the triple consumed.
    fn take_one(&mut self, s: &Node, p: &str) -> Option<Node> {
        let i = *self.find(s, p).first()?;
        self.used[i] = true;
        self.get(i).map(|t| t.o.clone())
    }

    fn peek_one(&self, s: &Node, p: &str) -> Option<Node> {
        let i = *self.find(s, p).first()?;
        self.get(i).map(|t| t.o.clone())
    }

    fn has_type(&self, s: &Node, ty: &str) -> bool {
        self.find(s, RDF_TYPE)
            .iter()
            .any(|&i| self.get(i).is_some_and(|t| t.o.iri() == Some(ty)))
    }

    fn consume_type(&mut self, s: &Node, ty: &str) {
        for i in self.find(s, RDF_TYPE) {
            if self.get(i).is_some_and(|t| t.o.iri() == Some(ty)) {
                self.used[i] = true;
            }
        }
    }

    /// An RDF list as its members, consuming the list's triples.
    fn list(&mut self, head: &Node) -> Option<Vec<Node>> {
        let mut out = Vec::new();
        let mut cur = head.clone();
        let mut seen = HashSet::new();
        let mut taken = Vec::new();
        loop {
            if cur.iri() == Some(RDF_NIL) {
                break;
            }
            if !cur.is_blank() || !seen.insert(cur.clone()) {
                return None;
            }
            let firsts = self.find(&cur, RDF_FIRST);
            let rests = self.find(&cur, RDF_REST);
            if firsts.len() != 1 || rests.len() != 1 {
                return None;
            }
            out.push(self.get(firsts[0])?.o.clone());
            let next = self.get(rests[0])?.o.clone();
            taken.push(firsts[0]);
            taken.push(rests[0]);
            for i in self.find(&cur, RDF_TYPE) {
                if self.get(i).is_some_and(|t| t.o.iri() == Some(RDF_LIST)) {
                    taken.push(i);
                }
            }
            cur = next;
        }
        for i in taken {
            self.used[i] = true;
        }
        Some(out)
    }

    // ── entity kinds ─────────────────────────────────────────────────────────

    fn add_kind(&mut self, iri: &str, k: EntityKind) {
        self.kinds.entry(iri.to_string()).or_default().insert(k);
    }

    fn kinds_of(&self, iri: &str) -> BTreeSet<EntityKind> {
        self.kinds.get(iri).cloned().unwrap_or_default()
    }

    fn is_datatype(&self, iri: &str) -> bool {
        DATATYPE_MAP.contains(&iri) || self.kinds_of(iri).contains(&EntityKind::Datatype)
    }

    fn is_annotation_property(&self, iri: &str) -> bool {
        BUILTIN_ANNOTATION_PROPERTIES.contains(&iri)
            || self.kinds_of(iri).contains(&EntityKind::AnnotationProperty)
    }

    fn is_data_property(&self, iri: &str) -> bool {
        iri == OWL_TOP_DATA_PROPERTY
            || iri == OWL_BOTTOM_DATA_PROPERTY
            || self.kinds_of(iri).contains(&EntityKind::DataProperty)
    }

    fn is_object_property(&self, iri: &str) -> bool {
        iri == OWL_TOP_OBJECT_PROPERTY
            || iri == OWL_BOTTOM_OBJECT_PROPERTY
            || self.kinds_of(iri).contains(&EntityKind::ObjectProperty)
    }

    // ── the run ──────────────────────────────────────────────────────────────

    fn run(&mut self) {
        self.skip_swrl();
        self.header();
        self.declarations();
        self.type_by_use();
        self.reifications();
        self.axiom_triples();
        self.ontology_annotations();
    }

    /// SWRL rules and everything reachable only from them.
    fn skip_swrl(&mut self) {
        let mut roots: Vec<Node> = Vec::new();
        for t in self.ts.iter().flatten() {
            let swrl_type = t.p == RDF_TYPE
                && t.o
                    .iri()
                    .is_some_and(|o| o.starts_with(SWRL) || o.starts_with(SWRLB));
            if swrl_type || t.p.starts_with(SWRL) {
                roots.push(t.s.clone());
            }
        }
        if roots.is_empty() {
            return;
        }
        let mut seen: HashSet<Node> = HashSet::new();
        let mut stack = roots;
        while let Some(n) = stack.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            for i in self.by_subject.get(&n).cloned().unwrap_or_default() {
                if let Some(t) = self.get(i) {
                    if t.o.is_blank() {
                        stack.push(t.o.clone());
                    }
                }
                if !self.used[i] {
                    self.used[i] = true;
                    self.skipped.insert(i);
                }
            }
        }
        // Variables (`?x a swrl:Variable`) are IRIs and caught above; any
        // `swrl:` triple whose subject is an IRI was a root.
        self.warnings
            .insert("SWRL rules are not OWL 2 axioms and were not passed to the DL backend".into());
    }

    fn header(&mut self) {
        let onts: Vec<Node> = self
            .ts
            .iter()
            .flatten()
            .filter(|t| t.p == RDF_TYPE && t.o.iri() == Some(&owl("Ontology")))
            .map(|t| t.s.clone())
            .collect();
        if onts.len() > 1 {
            self.warnings.insert(format!(
                "{} owl:Ontology headers in scope; their axioms are merged into one ontology",
                onts.len()
            ));
        }
        for (n, o) in onts.iter().enumerate() {
            self.consume_type(o, &owl("Ontology"));
            if n == 0 {
                self.ontology.iri = o.iri().map(str::to_string);
                if let Some(Node::Iri(v)) = self.take_one(o, &owl("versionIRI")) {
                    self.ontology.version_iri = Some(v);
                }
                self.ontology_node = Some(o.clone());
            }
            for i in self.find(o, &owl("imports")) {
                self.used[i] = true;
                if let Some(Node::Iri(target)) = self.get(i).map(|t| t.o.clone()) {
                    self.ontology.imports.push(target.clone());
                    self.warnings.insert(format!(
                        "owl:imports <{target}> is not followed: a run reasons over the graphs it was given"
                    ));
                }
            }
        }
    }

    fn declarations(&mut self) {
        let table: [(&str, EntityKind); 6] = [
            ("Class", EntityKind::Class),
            ("ObjectProperty", EntityKind::ObjectProperty),
            ("DatatypeProperty", EntityKind::DataProperty),
            ("AnnotationProperty", EntityKind::AnnotationProperty),
            ("NamedIndividual", EntityKind::NamedIndividual),
            ("", EntityKind::Datatype),
        ];
        let mut decls: Vec<(usize, String, EntityKind)> = Vec::new();
        for (i, t) in self.ts.iter().enumerate() {
            let Some(t) = t else { continue };
            if t.p != RDF_TYPE {
                continue;
            }
            let (Some(s), Some(o)) = (t.s.iri(), t.o.iri()) else {
                continue;
            };
            for (local, kind) in table {
                let ty = if kind == EntityKind::Datatype {
                    RDFS_DATATYPE.to_string()
                } else {
                    owl(local)
                };
                if o == ty {
                    decls.push((i, s.to_string(), kind));
                }
            }
            if o == RDFS_CLASS {
                decls.push((i, s.to_string(), EntityKind::Class));
                self.warnings
                    .insert("rdfs:Class read as owl:Class (typing by use)".into());
            }
        }
        for (i, iri, kind) in decls {
            self.used[i] = true;
            self.add_kind(&iri, kind);
            let ann = self.take_reified(i);
            self.axioms.push(Axiom {
                kind: AxiomKind::Declaration(kind, iri),
                annotations: ann,
            });
        }
        // Property characteristics that only an object property can have.
        let object_only: Vec<String> = self
            .ts
            .iter()
            .flatten()
            .filter(|t| t.p == RDF_TYPE)
            .filter(|t| {
                t.o.iri().is_some_and(|o| {
                    [
                        "InverseFunctionalProperty",
                        "ReflexiveProperty",
                        "IrreflexiveProperty",
                        "SymmetricProperty",
                        "AsymmetricProperty",
                        "TransitiveProperty",
                    ]
                    .iter()
                    .any(|l| o == owl(l))
                })
            })
            .filter_map(|t| t.s.iri().map(str::to_string))
            .collect();
        for s in object_only {
            if !self.is_object_property(&s) {
                self.add_kind(&s, EntityKind::ObjectProperty);
            }
        }
        // rdf:Property says nothing OWL can use: the kind comes from use.
        for i in 0..self.ts.len() {
            if self
                .get(i)
                .is_some_and(|t| t.p == RDF_TYPE && t.o.iri() == Some(RDF_PROPERTY))
            {
                self.used[i] = true;
                self.warnings.insert(
                    "rdf:Property declarations ignored; property kinds come from use".into(),
                );
            }
        }
    }

    /// Give undeclared IRIs a kind from where they are used.
    fn type_by_use(&mut self) {
        let mut add: Vec<(String, EntityKind)> = Vec::new();
        let structural: HashSet<String> = [
            RDF_TYPE,
            RDF_FIRST,
            RDF_REST,
            RDFS_SUB_CLASS_OF,
            RDFS_SUB_PROPERTY_OF,
            RDFS_DOMAIN,
            RDFS_RANGE,
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        for (i, t) in self.ts.iter().enumerate() {
            let Some(t) = t else { continue };
            // Skipped SWRL triples type nothing, and the ontology header's
            // own triples are annotations.
            if self.skipped.contains(&i) || Some(&t.s) == self.ontology_node.as_ref() {
                continue;
            }
            if structural.contains(&t.p) || is_reserved(&t.p) {
                // rdf:type C — C is a class (unless reserved vocabulary).
                if t.p == RDF_TYPE {
                    if let Some(c) = t.o.iri() {
                        if !is_reserved(c) || c == OWL_THING || c == OWL_NOTHING {
                            add.push((c.to_string(), EntityKind::Class));
                        }
                    }
                }
                continue;
            }
            if self.is_annotation_property(&t.p) {
                continue;
            }
            let explicit = self.kinds_of(&t.p);
            if explicit.contains(&EntityKind::ObjectProperty)
                || explicit.contains(&EntityKind::DataProperty)
            {
                // Declared: a use of the other kind is punning, reported by
                // the profile check from the assertion itself.
                continue;
            }
            match &t.o {
                Node::Lit(_) => add.push((t.p.clone(), EntityKind::DataProperty)),
                _ => add.push((t.p.clone(), EntityKind::ObjectProperty)),
            }
        }
        // Properties used only in schema position (restrictions, sub-property,
        // domain/range, chains) and never in an assertion.
        let mut prop_like: Vec<(String, Option<EntityKind>)> = Vec::new();
        for t in self.ts.iter().flatten() {
            if t.p == owl("onProperty") {
                if let Some(p) = t.o.iri() {
                    // A restriction with a data filler names a data property.
                    let data = ["onDataRange", "someValuesFrom", "allValuesFrom", "hasValue"]
                        .iter()
                        .any(|l| {
                            self.peek_one(&t.s, &owl(l)).is_some_and(|f| match &f {
                                Node::Lit(_) => true,
                                Node::Iri(i) => self.is_datatype(i),
                                Node::Blank(_) => self.has_type(&f, RDFS_DATATYPE),
                            })
                        });
                    prop_like.push((
                        p.to_string(),
                        Some(if data {
                            EntityKind::DataProperty
                        } else {
                            EntityKind::ObjectProperty
                        }),
                    ));
                }
            }
            if [
                RDFS_SUB_PROPERTY_OF.to_string(),
                owl("equivalentProperty"),
                owl("propertyDisjointWith"),
                RDFS_DOMAIN.to_string(),
                RDFS_RANGE.to_string(),
                owl("inverseOf"),
                owl("propertyChainAxiom"),
            ]
            .contains(&t.p)
            {
                if let Some(s) = t.s.iri() {
                    let kind = if t.p == RDFS_RANGE {
                        t.o.iri().map(|r| {
                            if self.is_datatype(r) {
                                EntityKind::DataProperty
                            } else {
                                EntityKind::ObjectProperty
                            }
                        })
                    } else if t.p == owl("inverseOf") || t.p == owl("propertyChainAxiom") {
                        Some(EntityKind::ObjectProperty)
                    } else {
                        None
                    };
                    prop_like.push((s.to_string(), kind));
                    if let Some(o) = t.o.iri() {
                        if t.p != RDFS_DOMAIN && t.p != RDFS_RANGE {
                            prop_like.push((o.to_string(), kind));
                        }
                    }
                }
            }
        }
        for (iri, k) in add {
            if self.kinds_of(&iri).is_empty() {
                self.warnings
                    .insert("undeclared entities were typed by use (see the OWL 2 DL docs)".into());
            }
            self.add_kind(&iri, k);
        }
        // Schema-only properties: settle after assertions gave their kinds.
        let mut changed = true;
        while changed {
            changed = false;
            for (iri, k) in &prop_like {
                if is_reserved(iri) || self.is_annotation_property(iri) {
                    continue;
                }
                let have = self.kinds_of(iri);
                if have.contains(&EntityKind::ObjectProperty)
                    || have.contains(&EntityKind::DataProperty)
                {
                    continue;
                }
                let kind = k.unwrap_or(EntityKind::ObjectProperty);
                self.add_kind(iri, kind);
                changed = true;
            }
        }
    }

    /// `owl:Axiom` / `owl:Annotation` reifications: remember the annotations
    /// of the triple they describe and consume the reification itself.
    fn reifications(&mut self) {
        let nodes: Vec<Node> = self
            .ts
            .iter()
            .flatten()
            .filter(|t| {
                t.p == RDF_TYPE
                    && (t.o.iri() == Some(&owl("Axiom")) || t.o.iri() == Some(&owl("Annotation")))
            })
            .map(|t| t.s.clone())
            .collect();
        // Nested annotations first (an owl:Annotation's source is a node that
        // is itself a reification), so they attach where they belong.
        for x in nodes.iter().rev() {
            let (Some(src), Some(Node::Iri(prop)), Some(tgt)) = (
                self.peek_one(x, &owl("annotatedSource")),
                self.peek_one(x, &owl("annotatedProperty")),
                self.peek_one(x, &owl("annotatedTarget")),
            ) else {
                continue;
            };
            self.consume_type(x, &owl("Axiom"));
            self.consume_type(x, &owl("Annotation"));
            for p in ["annotatedSource", "annotatedProperty", "annotatedTarget"] {
                self.take_one(x, &owl(p));
            }
            let anns = self.annotations_on(x);
            self.reified
                .entry((src, prop, tgt))
                .or_default()
                .extend(anns);
        }
    }

    /// Annotation triples `(x ap v)` on `x`, consumed, with nested annotations.
    fn annotations_on(&mut self, x: &Node) -> Vec<Annotation> {
        let mut out = Vec::new();
        for i in self.by_subject.get(x).cloned().unwrap_or_default() {
            if self.used[i] {
                continue;
            }
            let Some(t) = self.get(i) else { continue };
            if !self.is_annotation_property(&t.p) {
                continue;
            }
            let (p, o) = (t.p.clone(), t.o.clone());
            self.used[i] = true;
            let nested = self
                .reified
                .remove(&(x.clone(), p.clone(), o.clone()))
                .unwrap_or_default();
            out.push(Annotation {
                property: p,
                value: annotation_value(&o),
                annotations: nested,
            });
        }
        out
    }

    fn take_reified(&mut self, i: usize) -> Vec<Annotation> {
        let Some(t) = self.get(i) else {
            return Vec::new();
        };
        let key = (t.s.clone(), t.p.clone(), t.o.clone());
        self.reified.remove(&key).unwrap_or_default()
    }

    fn push(&mut self, i: usize, kind: AxiomKind) {
        let annotations = self.take_reified(i);
        self.axioms.push(Axiom { kind, annotations });
    }

    // ── expressions ──────────────────────────────────────────────────────────

    fn ope(&mut self, n: &Node) -> Option<ObjectProp> {
        match n {
            Node::Iri(p) => {
                if !self.is_object_property(p) && !self.is_data_property(p) {
                    self.add_kind(p, EntityKind::ObjectProperty);
                }
                Some(ObjectProp::Named(p.clone()))
            }
            Node::Blank(_) => {
                let inner = self.peek_one(n, &owl("inverseOf"))?;
                let Node::Iri(p) = inner else { return None };
                self.take_one(n, &owl("inverseOf"));
                Some(ObjectProp::Inverse(p))
            }
            Node::Lit(_) => None,
        }
    }

    fn individual(&self, n: &Node) -> Option<Individual> {
        match n {
            Node::Iri(i) => Some(Individual::Named(i.clone())),
            Node::Blank(b) => Some(Individual::Anonymous(b.clone())),
            Node::Lit(_) => None,
        }
    }

    fn literal_u32(n: &Node) -> Option<u32> {
        match n {
            Node::Lit(l) => l.lexical.trim().parse().ok(),
            _ => None,
        }
    }

    /// Whether the filler `n` reads as a data range.
    fn looks_like_data_range(&self, n: &Node) -> bool {
        match n {
            Node::Iri(i) => self.is_datatype(i),
            Node::Blank(_) => self.has_type(n, RDFS_DATATYPE),
            Node::Lit(_) => true,
        }
    }

    fn data_range(&mut self, n: &Node) -> Option<DataRange> {
        match n {
            Node::Iri(i) => {
                if !self.is_datatype(i) {
                    self.add_kind(i, EntityKind::Datatype);
                }
                Some(DataRange::Datatype(i.clone()))
            }
            Node::Lit(_) => None,
            Node::Blank(b) => {
                if let Some(c) = self.dr_cache.get(b) {
                    return c.clone();
                }
                let r = self.data_range_blank(n);
                self.dr_cache.insert(b.clone(), r.clone());
                r
            }
        }
    }

    fn data_range_blank(&mut self, n: &Node) -> Option<DataRange> {
        let r = if let Some(l) = self.peek_one(n, &owl("intersectionOf")) {
            let items = self.list(&l)?;
            self.take_one(n, &owl("intersectionOf"));
            DataRange::IntersectionOf(
                items
                    .iter()
                    .map(|x| self.data_range(x))
                    .collect::<Option<_>>()?,
            )
        } else if let Some(l) = self.peek_one(n, &owl("unionOf")) {
            let items = self.list(&l)?;
            self.take_one(n, &owl("unionOf"));
            DataRange::UnionOf(
                items
                    .iter()
                    .map(|x| self.data_range(x))
                    .collect::<Option<_>>()?,
            )
        } else if let Some(c) = self.peek_one(n, &owl("datatypeComplementOf")) {
            self.take_one(n, &owl("datatypeComplementOf"));
            DataRange::ComplementOf(Box::new(self.data_range(&c)?))
        } else if let Some(l) = self.peek_one(n, &owl("oneOf")) {
            let items = self.list(&l)?;
            self.take_one(n, &owl("oneOf"));
            DataRange::OneOf(
                items
                    .into_iter()
                    .map(|x| match x {
                        Node::Lit(l) => Some(l),
                        _ => None,
                    })
                    .collect::<Option<_>>()?,
            )
        } else if let Some(Node::Iri(dt)) = self.peek_one(n, &owl("onDatatype")) {
            let l = self.peek_one(n, &owl("withRestrictions"))?;
            let items = self.list(&l)?;
            let mut facets = Vec::new();
            for f in items {
                // Each facet node carries exactly one (facet value) triple.
                let idx: Vec<usize> = self
                    .by_subject
                    .get(&f)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|&i| !self.used[i])
                    .collect();
                if idx.len() != 1 {
                    return None;
                }
                let t = self.get(idx[0])?;
                let Node::Lit(v) = &t.o else { return None };
                facets.push((t.p.clone(), v.clone()));
                self.used[idx[0]] = true;
            }
            self.take_one(n, &owl("onDatatype"));
            self.take_one(n, &owl("withRestrictions"));
            if !self.is_datatype(&dt) {
                self.add_kind(&dt, EntityKind::Datatype);
            }
            DataRange::Restriction(dt, facets)
        } else {
            return None;
        };
        self.consume_type(n, RDFS_DATATYPE);
        Some(r)
    }

    fn class_expr(&mut self, n: &Node) -> Option<ClassExpr> {
        match n {
            Node::Iri(c) => {
                if !self.kinds_of(c).contains(&EntityKind::Class) && !is_reserved(c) {
                    self.add_kind(c, EntityKind::Class);
                }
                Some(ClassExpr::Class(c.clone()))
            }
            Node::Lit(_) => None,
            Node::Blank(b) => {
                if let Some(c) = self.ce_cache.get(b) {
                    return c.clone();
                }
                let r = self.class_expr_structure(n);
                self.ce_cache.insert(b.clone(), r.clone());
                r
            }
        }
    }

    /// The class expression a node's own triples describe (blank node, or an
    /// IRI read as a named restriction).
    fn class_expr_structure(&mut self, n: &Node) -> Option<ClassExpr> {
        let r = if let Some(l) = self.peek_one(n, &owl("intersectionOf")) {
            let items = self.list(&l)?;
            self.take_one(n, &owl("intersectionOf"));
            ClassExpr::IntersectionOf(
                items
                    .iter()
                    .map(|x| self.class_expr(x))
                    .collect::<Option<_>>()?,
            )
        } else if let Some(l) = self.peek_one(n, &owl("unionOf")) {
            let items = self.list(&l)?;
            self.take_one(n, &owl("unionOf"));
            ClassExpr::UnionOf(
                items
                    .iter()
                    .map(|x| self.class_expr(x))
                    .collect::<Option<_>>()?,
            )
        } else if let Some(c) = self.peek_one(n, &owl("complementOf")) {
            self.take_one(n, &owl("complementOf"));
            ClassExpr::ComplementOf(Box::new(self.class_expr(&c)?))
        } else if let Some(l) = self.peek_one(n, &owl("oneOf")) {
            let items = self.list(&l)?;
            self.take_one(n, &owl("oneOf"));
            ClassExpr::OneOf(
                items
                    .iter()
                    .map(|x| self.individual(x))
                    .collect::<Option<_>>()?,
            )
        } else if self.peek_one(n, &owl("onProperty")).is_some()
            || self.peek_one(n, &owl("onProperties")).is_some()
        {
            self.restriction(n)?
        } else {
            return None;
        };
        self.consume_type(n, &owl("Class"));
        if !self.has_type(n, &owl("Restriction"))
            && matches!(
                r,
                ClassExpr::SomeValuesFrom(..)
                    | ClassExpr::AllValuesFrom(..)
                    | ClassExpr::HasValue(..)
                    | ClassExpr::HasSelf(..)
                    | ClassExpr::MinCardinality(..)
                    | ClassExpr::MaxCardinality(..)
                    | ClassExpr::ExactCardinality(..)
                    | ClassExpr::DataSomeValuesFrom(..)
                    | ClassExpr::DataAllValuesFrom(..)
                    | ClassExpr::DataHasValue(..)
                    | ClassExpr::DataMinCardinality(..)
                    | ClassExpr::DataMaxCardinality(..)
                    | ClassExpr::DataExactCardinality(..)
            )
        {
            self.warnings
                .insert("a restriction without rdf:type owl:Restriction was read as one".into());
        }
        self.consume_type(n, &owl("Restriction"));
        Some(r)
    }

    fn restriction(&mut self, n: &Node) -> Option<ClassExpr> {
        // n-ary data restriction: owl:onProperties ( p1 … pn ).
        if let Some(l) = self.peek_one(n, &owl("onProperties")) {
            let props = self.list(&l)?;
            let props: Vec<String> = props
                .iter()
                .map(|p| p.iri().map(str::to_string))
                .collect::<Option<_>>()?;
            self.take_one(n, &owl("onProperties"));
            for p in &props {
                if !self.is_data_property(p) {
                    self.add_kind(p, EntityKind::DataProperty);
                }
            }
            if let Some(f) = self.peek_one(n, &owl("someValuesFrom")) {
                self.take_one(n, &owl("someValuesFrom"));
                return Some(ClassExpr::DataSomeValuesFrom(props, self.data_range(&f)?));
            }
            if let Some(f) = self.peek_one(n, &owl("allValuesFrom")) {
                self.take_one(n, &owl("allValuesFrom"));
                return Some(ClassExpr::DataAllValuesFrom(props, self.data_range(&f)?));
            }
            return None;
        }
        let pnode = self.peek_one(n, &owl("onProperty"))?;
        // Object or data? The property's kind decides; an unknown property
        // follows its filler.
        let data = match &pnode {
            Node::Iri(p) if self.is_data_property(p) && !self.is_object_property(p) => true,
            Node::Iri(p) if self.is_object_property(p) => false,
            Node::Iri(_) => ["someValuesFrom", "allValuesFrom", "hasValue", "onDataRange"]
                .iter()
                .any(|l| {
                    self.peek_one(n, &owl(l))
                        .is_some_and(|f| self.looks_like_data_range(&f))
                }),
            _ => false,
        };
        let card = |m: &mut Self, local: &str| -> Option<u32> {
            m.peek_one(n, &owl(local))
                .and_then(|v| Self::literal_u32(&v))
        };
        let r = if data {
            let p = pnode.iri()?.to_string();
            if let Some(f) = self.peek_one(n, &owl("someValuesFrom")) {
                self.take_one(n, &owl("someValuesFrom"));
                ClassExpr::DataSomeValuesFrom(vec![p], self.data_range(&f)?)
            } else if let Some(f) = self.peek_one(n, &owl("allValuesFrom")) {
                self.take_one(n, &owl("allValuesFrom"));
                ClassExpr::DataAllValuesFrom(vec![p], self.data_range(&f)?)
            } else if let Some(Node::Lit(v)) = self.peek_one(n, &owl("hasValue")) {
                self.take_one(n, &owl("hasValue"));
                ClassExpr::DataHasValue(p, v)
            } else {
                let filler = match self.peek_one(n, &owl("onDataRange")) {
                    Some(d) => {
                        self.take_one(n, &owl("onDataRange"));
                        Some(self.data_range(&d)?)
                    }
                    None => None,
                };
                let q = filler.is_some();
                let pick = |m: &mut Self, plain: &str, qual: &str| -> Option<u32> {
                    let local = if q { qual } else { plain };
                    let v = card(m, local)?;
                    m.take_one(n, &owl(local));
                    Some(v)
                };
                if let Some(c) = pick(self, "minCardinality", "minQualifiedCardinality") {
                    ClassExpr::DataMinCardinality(c, p, filler)
                } else if let Some(c) = pick(self, "maxCardinality", "maxQualifiedCardinality") {
                    ClassExpr::DataMaxCardinality(c, p, filler)
                } else {
                    let c = pick(self, "cardinality", "qualifiedCardinality")?;
                    ClassExpr::DataExactCardinality(c, p, filler)
                }
            }
        } else {
            let p = self.ope(&pnode)?;
            if let Some(f) = self.peek_one(n, &owl("someValuesFrom")) {
                self.take_one(n, &owl("someValuesFrom"));
                ClassExpr::SomeValuesFrom(p, Box::new(self.class_expr(&f)?))
            } else if let Some(f) = self.peek_one(n, &owl("allValuesFrom")) {
                self.take_one(n, &owl("allValuesFrom"));
                ClassExpr::AllValuesFrom(p, Box::new(self.class_expr(&f)?))
            } else if let Some(v) = self.peek_one(n, &owl("hasValue")) {
                self.take_one(n, &owl("hasValue"));
                ClassExpr::HasValue(p, self.individual(&v)?)
            } else if let Some(Node::Lit(v)) = self.peek_one(n, &owl("hasSelf")) {
                // Only `true` has a structural reading.
                if !(v.lexical == "true" || v.lexical == "1") {
                    return None;
                }
                self.take_one(n, &owl("hasSelf"));
                ClassExpr::HasSelf(p)
            } else {
                let filler = match self.peek_one(n, &owl("onClass")) {
                    Some(c) => {
                        self.take_one(n, &owl("onClass"));
                        Some(Box::new(self.class_expr(&c)?))
                    }
                    None => None,
                };
                let q = filler.is_some();
                let pick = |m: &mut Self, plain: &str, qual: &str| -> Option<u32> {
                    let local = if q { qual } else { plain };
                    let v = card(m, local)?;
                    m.take_one(n, &owl(local));
                    Some(v)
                };
                if let Some(c) = pick(self, "minCardinality", "minQualifiedCardinality") {
                    ClassExpr::MinCardinality(c, p, filler)
                } else if let Some(c) = pick(self, "maxCardinality", "maxQualifiedCardinality") {
                    ClassExpr::MaxCardinality(c, p, filler)
                } else {
                    let c = pick(self, "cardinality", "qualifiedCardinality")?;
                    ClassExpr::ExactCardinality(c, p, filler)
                }
            }
        };
        self.take_one(n, &owl("onProperty"));
        Some(r)
    }

    // ── axioms ───────────────────────────────────────────────────────────────

    fn axiom_triples(&mut self) {
        // n-ary and blank-node axioms first, so their member triples are
        // consumed before the generic pass sees them.
        self.nary_axioms();
        // Named (IRI) expressions: `ex:C owl:intersectionOf (…)`.
        self.named_expressions();
        for i in 0..self.ts.len() {
            if self.used[i] {
                continue;
            }
            let Some(t) = self.get(i) else { continue };
            let (s, p, o) = (t.s.clone(), t.p.clone(), t.o.clone());
            if self.axiom_from(i, &s, &p, &o) {
                self.used[i] = true;
            }
        }
    }

    fn named_expressions(&mut self) {
        let named: Vec<String> = self
            .ts
            .iter()
            .flatten()
            .filter(|t| {
                [
                    "intersectionOf",
                    "unionOf",
                    "complementOf",
                    "oneOf",
                    "onProperty",
                    "onProperties",
                ]
                .iter()
                .any(|l| t.p == owl(l))
            })
            .filter_map(|t| t.s.iri().map(str::to_string))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for c in named {
            let n = Node::Iri(c.clone());
            // A datatype with owl:oneOf etc. is a data range, read below.
            if self.is_datatype(&c) {
                continue;
            }
            if let Some(expr) = self.class_expr_structure(&n) {
                self.add_kind(&c, EntityKind::Class);
                self.warnings.insert(
                    "an IRI carrying a class expression was read as EquivalentClasses(IRI expression)"
                        .into(),
                );
                self.axioms
                    .push(Axiom::new(AxiomKind::EquivalentClasses(vec![
                        ClassExpr::Class(c),
                        expr,
                    ])));
            }
        }
    }

    fn nary_axioms(&mut self) {
        let typed: Vec<(Node, String)> = self
            .ts
            .iter()
            .flatten()
            .filter(|t| t.p == RDF_TYPE && t.s.is_blank())
            .filter_map(|t| t.o.iri().map(|o| (t.s.clone(), o.to_string())))
            .collect();
        for (x, ty) in typed {
            let kind = if ty == owl("AllDisjointClasses") {
                self.take_list_of(&x, "members").and_then(|ms| {
                    let ces: Option<Vec<ClassExpr>> =
                        ms.iter().map(|m| self.class_expr(m)).collect();
                    ces.map(AxiomKind::DisjointClasses)
                })
            } else if ty == owl("AllDisjointProperties") {
                self.take_list_of(&x, "members").and_then(|ms| {
                    let names: Vec<String> = ms
                        .iter()
                        .filter_map(|m| m.iri().map(str::to_string))
                        .collect();
                    if !names.is_empty() && names.iter().all(|p| self.is_data_property(p)) {
                        Some(AxiomKind::DisjointDataProperties(names))
                    } else {
                        let opes: Option<Vec<ObjectProp>> =
                            ms.iter().map(|m| self.ope(m)).collect();
                        opes.map(AxiomKind::DisjointObjectProperties)
                    }
                })
            } else if ty == owl("AllDifferent") {
                let ms = self
                    .take_list_of(&x, "members")
                    .or_else(|| self.take_list_of(&x, "distinctMembers"));
                ms.and_then(|ms| {
                    let inds: Option<Vec<Individual>> =
                        ms.iter().map(|m| self.individual(m)).collect();
                    inds.map(AxiomKind::DifferentIndividuals)
                })
            } else if ty == owl("NegativePropertyAssertion") {
                self.negative_assertion(&x)
            } else {
                continue;
            };
            if let Some(kind) = kind {
                self.consume_type(&x, &ty);
                let annotations = self.annotations_on(&x);
                self.axioms.push(Axiom { kind, annotations });
            }
        }
    }

    fn take_list_of(&mut self, x: &Node, local: &str) -> Option<Vec<Node>> {
        let l = self.peek_one(x, &owl(local))?;
        let items = self.list(&l)?;
        self.take_one(x, &owl(local));
        Some(items)
    }

    fn negative_assertion(&mut self, x: &Node) -> Option<AxiomKind> {
        let src = self.peek_one(x, &owl("sourceIndividual"))?;
        let prop = self.peek_one(x, &owl("assertionProperty"))?;
        let src = self.individual(&src)?;
        let kind = if let Some(t) = self.peek_one(x, &owl("targetIndividual")) {
            let tgt = self.individual(&t)?;
            let p = self.ope(&prop)?;
            self.take_one(x, &owl("targetIndividual"));
            AxiomKind::NegativeObjectPropertyAssertion(p, src, tgt)
        } else if let Some(Node::Lit(v)) = self.peek_one(x, &owl("targetValue")) {
            let p = prop.iri()?.to_string();
            if !self.is_data_property(&p) {
                self.add_kind(&p, EntityKind::DataProperty);
            }
            self.take_one(x, &owl("targetValue"));
            AxiomKind::NegativeDataPropertyAssertion(p, src, v)
        } else {
            return None;
        };
        self.take_one(x, &owl("sourceIndividual"));
        self.take_one(x, &owl("assertionProperty"));
        Some(kind)
    }

    /// The axiom triple `i` = (s p o) stands for. `false`: no reading.
    fn axiom_from(&mut self, i: usize, s: &Node, p: &str, o: &Node) -> bool {
        // The ontology header's triples are its annotations, collected last.
        if Some(s) == self.ontology_node.as_ref() {
            return false;
        }
        let kind = match p {
            RDF_TYPE => self.type_axiom(s, o),
            RDFS_SUB_CLASS_OF => self.pair(s, o, AxiomKind::SubClassOf),
            _ if p == owl("equivalentClass") => {
                if s.iri().is_some_and(|d| self.is_datatype(d)) {
                    let dt = s.iri().unwrap().to_string();
                    self.data_range(o)
                        .map(|r| AxiomKind::DatatypeDefinition(dt, r))
                } else {
                    self.pair(s, o, |a, b| AxiomKind::EquivalentClasses(vec![a, b]))
                }
            }
            _ if p == owl("disjointWith") => {
                self.pair(s, o, |a, b| AxiomKind::DisjointClasses(vec![a, b]))
            }
            _ if p == owl("disjointUnionOf") => {
                let (Some(c), Some(items)) = (s.iri().map(str::to_string), self.list(o)) else {
                    return false;
                };
                let ces: Option<Vec<ClassExpr>> =
                    items.iter().map(|x| self.class_expr(x)).collect();
                ces.map(|ces| AxiomKind::DisjointUnion(c, ces))
            }
            RDFS_SUB_PROPERTY_OF => self.property_pair(s, o, PropAxiom::Sub),
            _ if p == owl("equivalentProperty") => self.property_pair(s, o, PropAxiom::Equivalent),
            _ if p == owl("propertyDisjointWith") => self.property_pair(s, o, PropAxiom::Disjoint),
            _ if p == owl("propertyChainAxiom") => {
                let Some(sup) = self.ope(s) else { return false };
                let Some(items) = self.list(o) else {
                    return false;
                };
                let chain: Option<Vec<ObjectProp>> = items.iter().map(|x| self.ope(x)).collect();
                chain.map(|c| AxiomKind::SubObjectPropertyChain(c, sup))
            }
            RDFS_DOMAIN => self.domain_range(s, o, true),
            RDFS_RANGE => self.domain_range(s, o, false),
            _ if p == owl("inverseOf") => match (s, o) {
                (Node::Iri(a), Node::Iri(b)) => {
                    for x in [a, b] {
                        if !self.is_object_property(x) {
                            self.add_kind(x, EntityKind::ObjectProperty);
                        }
                    }
                    Some(AxiomKind::InverseObjectProperties(
                        ObjectProp::Named(a.clone()),
                        ObjectProp::Named(b.clone()),
                    ))
                }
                _ => None,
            },
            _ if p == owl("hasKey") => {
                let Some(ce) = self.class_expr(s) else {
                    return false;
                };
                let Some(items) = self.list(o) else {
                    return false;
                };
                let mut ops = Vec::new();
                let mut dps = Vec::new();
                for x in &items {
                    match x.iri() {
                        Some(pi) if self.is_data_property(pi) && !self.is_object_property(pi) => {
                            dps.push(pi.to_string())
                        }
                        _ => match self.ope(x) {
                            Some(op) => ops.push(op),
                            None => return false,
                        },
                    }
                }
                Some(AxiomKind::HasKey(ce, ops, dps))
            }
            _ if p == owl("sameAs") => match (self.individual(s), self.individual(o)) {
                (Some(a), Some(b)) => Some(AxiomKind::SameIndividual(vec![a, b])),
                _ => None,
            },
            _ if p == owl("differentFrom") => match (self.individual(s), self.individual(o)) {
                (Some(a), Some(b)) => Some(AxiomKind::DifferentIndividuals(vec![a, b])),
                _ => None,
            },
            _ if self.is_annotation_property(p) => {
                let subject = match s {
                    Node::Iri(x) => AnnotationSubject::Iri(x.clone()),
                    Node::Blank(b) => AnnotationSubject::Anonymous(b.clone()),
                    Node::Lit(_) => return false,
                };
                Some(AxiomKind::AnnotationAssertion(
                    p.to_string(),
                    subject,
                    annotation_value(o),
                ))
            }
            _ if is_reserved(p) => None,
            _ => {
                let Some(subj) = self.individual(s) else {
                    return false;
                };
                match o {
                    Node::Lit(l) => {
                        if !self.is_data_property(p) {
                            self.add_kind(p, EntityKind::DataProperty);
                        }
                        Some(AxiomKind::DataPropertyAssertion(
                            p.to_string(),
                            subj,
                            l.clone(),
                        ))
                    }
                    other => {
                        let Some(obj) = self.individual(other) else {
                            return false;
                        };
                        if !self.is_object_property(p) {
                            self.add_kind(p, EntityKind::ObjectProperty);
                        }
                        Some(AxiomKind::ObjectPropertyAssertion(
                            ObjectProp::Named(p.to_string()),
                            subj,
                            obj,
                        ))
                    }
                }
            }
        };
        match kind {
            Some(k) => {
                self.push(i, k);
                true
            }
            None => false,
        }
    }

    fn pair(
        &mut self,
        s: &Node,
        o: &Node,
        f: impl FnOnce(ClassExpr, ClassExpr) -> AxiomKind,
    ) -> Option<AxiomKind> {
        let a = self.class_expr(s)?;
        let b = self.class_expr(o)?;
        Some(f(a, b))
    }

    fn type_axiom(&mut self, s: &Node, o: &Node) -> Option<AxiomKind> {
        let ty = o.iri().map(str::to_string);
        if let Some(ty) = &ty {
            let characteristic = |local: &str| ty == &owl(local);
            if characteristic("FunctionalProperty") {
                let p = s.iri()?.to_string();
                return Some(
                    if self.is_data_property(&p) && !self.is_object_property(&p) {
                        AxiomKind::FunctionalDataProperty(p)
                    } else {
                        AxiomKind::FunctionalObjectProperty(self.ope(s)?)
                    },
                );
            }
            let object_char: [Characteristic; 6] = [
                (
                    "InverseFunctionalProperty",
                    AxiomKind::InverseFunctionalObjectProperty,
                ),
                ("ReflexiveProperty", AxiomKind::ReflexiveObjectProperty),
                ("IrreflexiveProperty", AxiomKind::IrreflexiveObjectProperty),
                ("SymmetricProperty", AxiomKind::SymmetricObjectProperty),
                ("AsymmetricProperty", AxiomKind::AsymmetricObjectProperty),
                ("TransitiveProperty", AxiomKind::TransitiveObjectProperty),
            ];
            for (local, make) in object_char {
                if characteristic(local) {
                    return Some(make(self.ope(s)?));
                }
            }
            // Other reserved types have no assertion reading (owl:Thing and
            // owl:Nothing are classes).
            if is_reserved(ty) && ty != OWL_THING && ty != OWL_NOTHING {
                return None;
            }
        }
        // A class assertion. The subject must be an individual — a blank
        // node that is itself an expression would have been consumed.
        let ind = self.individual(s)?;
        let ce = self.class_expr(o)?;
        if let Individual::Named(i) = &ind {
            self.add_kind(i, EntityKind::NamedIndividual);
        }
        Some(AxiomKind::ClassAssertion(ce, ind))
    }

    fn property_pair(&mut self, s: &Node, o: &Node, which: PropAxiom) -> Option<AxiomKind> {
        let (a, b) = (s.iri(), o.iri());
        let annotation = a.is_some_and(|x| self.is_annotation_property(x))
            && b.is_some_and(|x| self.is_annotation_property(x));
        if annotation {
            return match which {
                PropAxiom::Sub => Some(AxiomKind::SubAnnotationPropertyOf(
                    a?.to_string(),
                    b?.to_string(),
                )),
                _ => None,
            };
        }
        let data = a.is_some_and(|x| self.is_data_property(x) && !self.is_object_property(x))
            && b.is_some_and(|x| self.is_data_property(x) && !self.is_object_property(x));
        if data {
            let (a, b) = (a?.to_string(), b?.to_string());
            return Some(match which {
                PropAxiom::Sub => AxiomKind::SubDataPropertyOf(a, b),
                PropAxiom::Equivalent => AxiomKind::EquivalentDataProperties(vec![a, b]),
                PropAxiom::Disjoint => AxiomKind::DisjointDataProperties(vec![a, b]),
            });
        }
        let (x, y) = (self.ope(s)?, self.ope(o)?);
        Some(match which {
            PropAxiom::Sub => AxiomKind::SubObjectPropertyOf(x, y),
            PropAxiom::Equivalent => AxiomKind::EquivalentObjectProperties(vec![x, y]),
            PropAxiom::Disjoint => AxiomKind::DisjointObjectProperties(vec![x, y]),
        })
    }

    fn domain_range(&mut self, s: &Node, o: &Node, domain: bool) -> Option<AxiomKind> {
        if let Some(p) = s.iri() {
            if self.is_annotation_property(p) {
                let (p, c) = (p.to_string(), o.iri()?.to_string());
                return Some(if domain {
                    AxiomKind::AnnotationPropertyDomain(p, c)
                } else {
                    AxiomKind::AnnotationPropertyRange(p, c)
                });
            }
            if self.is_data_property(p) && !self.is_object_property(p) {
                let p = p.to_string();
                return Some(if domain {
                    AxiomKind::DataPropertyDomain(p, self.class_expr(o)?)
                } else {
                    AxiomKind::DataPropertyRange(p, self.data_range(o)?)
                });
            }
        }
        let p = self.ope(s)?;
        let c = self.class_expr(o)?;
        Some(if domain {
            AxiomKind::ObjectPropertyDomain(p, c)
        } else {
            AxiomKind::ObjectPropertyRange(p, c)
        })
    }

    /// The header's remaining triples are ontology annotations; an undeclared
    /// property there (`dcterms:title`, …) is an annotation property, as the
    /// OWL API reads it.
    fn ontology_annotations(&mut self) {
        let Some(o) = self.ontology_node.clone() else {
            return;
        };
        for i in self.by_subject.get(&o).cloned().unwrap_or_default() {
            if self.used[i] {
                continue;
            }
            let Some(t) = self.get(i) else { continue };
            let p = t.p.clone();
            if is_reserved(&p) && !BUILTIN_ANNOTATION_PROPERTIES.contains(&p.as_str()) {
                continue;
            }
            if self.is_object_property(&p) || self.is_data_property(&p) {
                continue;
            }
            if !self.is_annotation_property(&p) {
                self.add_kind(&p, EntityKind::AnnotationProperty);
            }
        }
        let anns = self.annotations_on(&o);
        self.ontology.annotations.extend(anns);
    }

    fn finish(mut self) -> Mapped {
        // Leftover expression and list triples whose node nothing referred to:
        // an unused expression is harmless, so it is a warning, not an error.
        let mut unmapped = Vec::new();
        let mut dangling = 0usize;
        let objects: HashSet<&Node> = self.ts.iter().flatten().map(|u| &u.o).collect();
        let referenced_nodes: HashSet<Node> = objects.into_iter().cloned().collect();
        for i in 0..self.ts.len() {
            if self.used[i] {
                continue;
            }
            let Some(t) = self.get(i) else {
                // RDF 1.2 triple terms have no OWL reading.
                unmapped.push(self.source[i].clone());
                continue;
            };
            let structural = t.s.is_blank()
                && (t.p == RDF_FIRST
                    || t.p == RDF_REST
                    || (t.p == RDF_TYPE
                        && [owl("Restriction"), owl("Class"), RDF_LIST.to_string()]
                            .iter()
                            .any(|x| t.o.iri() == Some(x)))
                    || ["onProperty", "someValuesFrom", "allValuesFrom", "onClass"]
                        .iter()
                        .any(|l| t.p == owl(l)));
            if structural && !referenced_nodes.contains(&t.s) {
                dangling += 1;
                continue;
            }
            unmapped.push(self.source[i].clone());
        }
        if dangling > 0 {
            self.warnings.insert(format!(
                "{dangling} triple(s) of class expressions or lists nothing refers to were ignored"
            ));
        }
        if !self.reified.is_empty() {
            self.warnings.insert(format!(
                "{} owl:Axiom annotation(s) describe a triple that is not in scope",
                self.reified.len()
            ));
        }
        // Declarations for everything typed by use, so the ontology is
        // self-describing (OWL 2 DL requires declarations).
        let declared: HashSet<(EntityKind, String)> = self
            .axioms
            .iter()
            .filter_map(|a| match &a.kind {
                AxiomKind::Declaration(k, i) => Some((*k, i.clone())),
                _ => None,
            })
            .collect();
        let mut implicit = Vec::new();
        for (iri, ks) in &self.kinds {
            for k in ks {
                if is_builtin(iri, *k) || declared.contains(&(*k, iri.clone())) {
                    continue;
                }
                implicit.push(Axiom::new(AxiomKind::Declaration(*k, iri.clone())));
            }
        }
        let mut axioms = implicit;
        axioms.append(&mut self.axioms);
        self.ontology.axioms = axioms;
        Mapped {
            ontology: self.ontology,
            unmapped,
            warnings: self.warnings.into_iter().collect(),
        }
    }
}

/// An object property characteristic: its `owl:` class name and axiom.
type Characteristic = (&'static str, fn(ObjectProp) -> AxiomKind);

#[derive(Clone, Copy)]
enum PropAxiom {
    Sub,
    Equivalent,
    Disjoint,
}

fn annotation_value(o: &Node) -> AnnotationValue {
    match o {
        Node::Iri(i) => AnnotationValue::Iri(i.clone()),
        Node::Blank(b) => AnnotationValue::Anonymous(b.clone()),
        Node::Lit(l) => AnnotationValue::Literal(l.clone()),
    }
}

/// Entities OWL 2 declares implicitly (§5.8.1).
pub fn is_builtin(iri: &str, kind: EntityKind) -> bool {
    match kind {
        EntityKind::Class => iri == OWL_THING || iri == OWL_NOTHING,
        EntityKind::ObjectProperty => {
            iri == OWL_TOP_OBJECT_PROPERTY || iri == OWL_BOTTOM_OBJECT_PROPERTY
        }
        EntityKind::DataProperty => iri == OWL_TOP_DATA_PROPERTY || iri == OWL_BOTTOM_DATA_PROPERTY,
        EntityKind::Datatype => DATATYPE_MAP.contains(&iri),
        EntityKind::AnnotationProperty => BUILTIN_ANNOTATION_PROPERTIES.contains(&iri),
        EntityKind::NamedIndividual => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::{RdfFormat, RdfParser};

    fn map(ttl: &str) -> Mapped {
        let triples: Vec<Triple> = RdfParser::from_format(RdfFormat::Turtle)
            .for_slice(ttl.as_bytes())
            .map(|q| q.unwrap().into())
            .collect();
        map_triples(&triples)
    }

    fn has(m: &Mapped, pred: impl Fn(&AxiomKind) -> bool) -> bool {
        m.ontology.axioms.iter().any(|a| pred(&a.kind))
    }

    const P: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
                     @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> . \
                     @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
                     @prefix xsd: <http://www.w3.org/2001/XMLSchema#> . \
                     @prefix ex: <http://example.org/> . ";

    #[test]
    fn restriction_subclass_maps() {
        let m = map(&format!(
            "{P} ex:A rdfs:subClassOf [ a owl:Restriction ; owl:onProperty ex:r ; owl:someValuesFrom ex:B ] ."
        ));
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        assert!(has(
            &m,
            |k| matches!(k, AxiomKind::SubClassOf(ClassExpr::Class(a),
            ClassExpr::SomeValuesFrom(ObjectProp::Named(r), b))
            if a == "http://example.org/A" && r == "http://example.org/r"
               && **b == ClassExpr::Class("http://example.org/B".into()))
        ));
    }

    #[test]
    fn data_and_object_assertions_split_by_object() {
        let m = map(&format!(
            "{P} ex:a ex:age 3 ; ex:knows ex:b ; rdfs:label \"a\" ."
        ));
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        assert!(has(
            &m,
            |k| matches!(k, AxiomKind::DataPropertyAssertion(p, ..) if p.ends_with("age"))
        ));
        assert!(has(&m, |k| matches!(
            k,
            AxiomKind::ObjectPropertyAssertion(..)
        )));
        assert!(has(&m, |k| matches!(k, AxiomKind::AnnotationAssertion(..))));
    }

    #[test]
    fn nary_disjointness_and_all_different() {
        let m = map(&format!(
            "{P} [] a owl:AllDisjointClasses ; owl:members ( ex:A ex:B ex:C ) .
                 [] a owl:AllDifferent ; owl:distinctMembers ( ex:a ex:b ) ."
        ));
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        assert!(has(
            &m,
            |k| matches!(k, AxiomKind::DisjointClasses(v) if v.len() == 3)
        ));
        assert!(has(
            &m,
            |k| matches!(k, AxiomKind::DifferentIndividuals(v) if v.len() == 2)
        ));
    }

    #[test]
    fn datatype_restriction_and_qualified_data_cardinality() {
        let m = map(&format!(
            "{P} ex:age a owl:DatatypeProperty .
                 ex:Adult owl:equivalentClass [ a owl:Restriction ; owl:onProperty ex:age ;
                    owl:someValuesFrom [ a rdfs:Datatype ; owl:onDatatype xsd:integer ;
                       owl:withRestrictions ( [ xsd:minInclusive 18 ] ) ] ] .
                 ex:Twins rdfs:subClassOf [ a owl:Restriction ; owl:onProperty ex:age ;
                    owl:qualifiedCardinality 2 ; owl:onDataRange xsd:integer ] ."
        ));
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        assert!(has(&m, |k| matches!(k, AxiomKind::EquivalentClasses(v)
            if matches!(&v[1], ClassExpr::DataSomeValuesFrom(_, DataRange::Restriction(_, f)) if f.len() == 1))));
        assert!(has(&m, |k| matches!(
            k,
            AxiomKind::SubClassOf(_, ClassExpr::DataExactCardinality(2, _, Some(_)))
        )));
    }

    #[test]
    fn negative_assertion_chain_inverse_and_key() {
        let m = map(&format!(
            "{P} [] a owl:NegativePropertyAssertion ; owl:sourceIndividual ex:a ;
                    owl:assertionProperty ex:r ; owl:targetIndividual ex:b .
                 ex:u owl:propertyChainAxiom ( ex:p [ owl:inverseOf ex:q ] ) .
                 ex:C owl:hasKey ( ex:k ex:d ) . ex:x ex:d 1 ."
        ));
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        assert!(has(&m, |k| matches!(
            k,
            AxiomKind::NegativeObjectPropertyAssertion(..)
        )));
        assert!(has(
            &m,
            |k| matches!(k, AxiomKind::SubObjectPropertyChain(c, _)
            if c.len() == 2 && matches!(c[1], ObjectProp::Inverse(_)))
        ));
        assert!(has(
            &m,
            |k| matches!(k, AxiomKind::HasKey(_, o, d) if o.len() == 1 && d.len() == 1)
        ));
    }

    #[test]
    fn axiom_annotations_attach_to_their_axiom() {
        let m = map(&format!(
            "{P} ex:A rdfs:subClassOf ex:B .
                 [] a owl:Axiom ; owl:annotatedSource ex:A ; owl:annotatedProperty rdfs:subClassOf ;
                    owl:annotatedTarget ex:B ; rdfs:comment \"why\" ."
        ));
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        assert!(m
            .ontology
            .axioms
            .iter()
            .any(|a| matches!(a.kind, AxiomKind::SubClassOf(..)) && a.annotations.len() == 1));
    }

    #[test]
    fn rdf_vocabulary_without_owl_reading_is_unmapped() {
        let m = map(&format!("{P} ex:a rdf:value ex:b . ex:s a rdf:Statement ."));
        assert_eq!(m.unmapped.len(), 2, "{:?}", m.ontology.axioms);
    }

    #[test]
    fn swrl_rules_are_skipped_with_a_warning() {
        let m = map(&format!(
            "{P} @prefix swrl: <http://www.w3.org/2003/11/swrl#> .
             [] a swrl:Imp ; swrl:body ( [ a swrl:ClassAtom ; swrl:classPredicate ex:A ; swrl:argument1 ex:x ] ) ;
                swrl:head () . ex:x a swrl:Variable ."
        ));
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        assert!(m.warnings.iter().any(|w| w.contains("SWRL")));
        // Nothing of the rule leaks into the signature handed to a reasoner.
        assert!(
            !m.ontology
                .axioms
                .iter()
                .any(|a| matches!(&a.kind, AxiomKind::Declaration(_, i) if i.starts_with(SWRL))),
            "{:?}",
            m.ontology.axioms
        );
    }

    #[test]
    fn named_restriction_is_read_as_equivalence() {
        let m = map(&format!(
            "{P} ex:SelfClass owl:onProperty ex:knows ; owl:hasSelf true . ex:a a ex:SelfClass ."
        ));
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        assert!(has(&m, |k| matches!(k, AxiomKind::EquivalentClasses(v)
            if matches!(v[1], ClassExpr::HasSelf(_)))));
    }
}
