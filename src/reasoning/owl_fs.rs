//! OWL 2 functional-style syntax writer (Structural Specification §3–§10,
//! grammar in its Appendix). Full IRIs throughout, so no prefix handling.

use std::collections::HashMap;
use std::fmt::Write as _;

use super::owl_model::*;

/// The empty data range. `DataComplementOf(rdfs:Literal)` says the same, but
/// Konclude v0.7.0 reads it as non-empty; disjoint value spaces it gets right.
const EMPTY_DATA_RANGE: &str = "DataIntersectionOf(<http://www.w3.org/2001/XMLSchema#integer> <http://www.w3.org/2001/XMLSchema#string>)";

/// Writes an [`Ontology`]; anonymous individuals get stable `_:bN` labels.
#[derive(Default)]
pub struct FsWriter {
    labels: HashMap<String, String>,
}

impl FsWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// The ontology document, with `extra` axioms appended (entailment and
    /// satisfiability checks add the negated conclusion this way).
    pub fn ontology(&mut self, ont: &Ontology, extra: &[Axiom]) -> String {
        let mut out = String::new();
        out.push_str("Ontology(");
        if let Some(iri) = &ont.iri {
            let _ = write!(out, "{}", iri_ref(iri));
            if let Some(v) = &ont.version_iri {
                let _ = write!(out, " {}", iri_ref(v));
            }
        }
        out.push('\n');
        // `owl:imports` is deliberately not written as Import(…): a reasoner
        // would fetch it, and a run reasons over exactly the graphs it was
        // given (the mapping warns about every import it dropped).
        for a in &ont.annotations {
            let _ = writeln!(out, "{}", self.annotation(a));
        }
        for a in ont.axioms.iter().chain(extra) {
            let _ = writeln!(out, "{}", self.axiom(a));
        }
        out.push_str(")\n");
        out
    }

    pub fn axiom(&mut self, a: &Axiom) -> String {
        let anns: String = a
            .annotations
            .iter()
            .map(|x| format!("{} ", self.annotation(x)))
            .collect();
        use AxiomKind::*;
        let (name, body) = match &a.kind {
            Declaration(k, iri) => ("Declaration", format!("{}({})", k.fs_name(), iri_ref(iri))),
            SubClassOf(x, y) => ("SubClassOf", format!("{} {}", self.ce(x), self.ce(y))),
            EquivalentClasses(v) => ("EquivalentClasses", self.ces(v)),
            DisjointClasses(v) => ("DisjointClasses", self.ces(v)),
            DisjointUnion(c, v) => ("DisjointUnion", format!("{} {}", iri_ref(c), self.ces(v))),
            SubObjectPropertyOf(x, y) => ("SubObjectPropertyOf", format!("{} {}", ope(x), ope(y))),
            SubObjectPropertyChain(c, sup) => (
                "SubObjectPropertyOf",
                format!(
                    "ObjectPropertyChain({}) {}",
                    c.iter().map(ope).collect::<Vec<_>>().join(" "),
                    ope(sup)
                ),
            ),
            EquivalentObjectProperties(v) => ("EquivalentObjectProperties", opes(v)),
            DisjointObjectProperties(v) => ("DisjointObjectProperties", opes(v)),
            InverseObjectProperties(x, y) => {
                ("InverseObjectProperties", format!("{} {}", ope(x), ope(y)))
            }
            ObjectPropertyDomain(p, c) => {
                ("ObjectPropertyDomain", format!("{} {}", ope(p), self.ce(c)))
            }
            ObjectPropertyRange(p, c) => {
                ("ObjectPropertyRange", format!("{} {}", ope(p), self.ce(c)))
            }
            FunctionalObjectProperty(p) => ("FunctionalObjectProperty", ope(p)),
            InverseFunctionalObjectProperty(p) => ("InverseFunctionalObjectProperty", ope(p)),
            ReflexiveObjectProperty(p) => ("ReflexiveObjectProperty", ope(p)),
            IrreflexiveObjectProperty(p) => ("IrreflexiveObjectProperty", ope(p)),
            SymmetricObjectProperty(p) => ("SymmetricObjectProperty", ope(p)),
            AsymmetricObjectProperty(p) => ("AsymmetricObjectProperty", ope(p)),
            TransitiveObjectProperty(p) => ("TransitiveObjectProperty", ope(p)),
            SubDataPropertyOf(x, y) => (
                "SubDataPropertyOf",
                format!("{} {}", iri_ref(x), iri_ref(y)),
            ),
            EquivalentDataProperties(v) => ("EquivalentDataProperties", iris(v)),
            DisjointDataProperties(v) => ("DisjointDataProperties", iris(v)),
            DataPropertyDomain(p, c) => (
                "DataPropertyDomain",
                format!("{} {}", iri_ref(p), self.ce(c)),
            ),
            DataPropertyRange(p, r) => ("DataPropertyRange", format!("{} {}", iri_ref(p), dr(r))),
            FunctionalDataProperty(p) => ("FunctionalDataProperty", iri_ref(p)),
            DatatypeDefinition(d, r) => ("DatatypeDefinition", format!("{} {}", iri_ref(d), dr(r))),
            HasKey(c, os, ds) => (
                "HasKey",
                format!(
                    "{} ({}) ({})",
                    self.ce(c),
                    os.iter().map(ope).collect::<Vec<_>>().join(" "),
                    iris(ds)
                ),
            ),
            SameIndividual(v) => ("SameIndividual", self.inds(v)),
            DifferentIndividuals(v) => ("DifferentIndividuals", self.inds(v)),
            ClassAssertion(c, i) => ("ClassAssertion", format!("{} {}", self.ce(c), self.ind(i))),
            ObjectPropertyAssertion(p, x, y) => (
                "ObjectPropertyAssertion",
                format!("{} {} {}", ope(p), self.ind(x), self.ind(y)),
            ),
            NegativeObjectPropertyAssertion(p, x, y) => (
                "NegativeObjectPropertyAssertion",
                format!("{} {} {}", ope(p), self.ind(x), self.ind(y)),
            ),
            DataPropertyAssertion(p, x, l) => (
                "DataPropertyAssertion",
                format!("{} {} {}", iri_ref(p), self.ind(x), literal(l)),
            ),
            NegativeDataPropertyAssertion(p, x, l) => (
                "NegativeDataPropertyAssertion",
                format!("{} {} {}", iri_ref(p), self.ind(x), literal(l)),
            ),
            AnnotationAssertion(p, s, v) => {
                let subject = match s {
                    AnnotationSubject::Iri(i) => iri_ref(i),
                    AnnotationSubject::Anonymous(b) => self.label(b),
                };
                (
                    "AnnotationAssertion",
                    format!("{} {} {}", iri_ref(p), subject, self.value(v)),
                )
            }
            SubAnnotationPropertyOf(x, y) => (
                "SubAnnotationPropertyOf",
                format!("{} {}", iri_ref(x), iri_ref(y)),
            ),
            AnnotationPropertyDomain(p, c) => (
                "AnnotationPropertyDomain",
                format!("{} {}", iri_ref(p), iri_ref(c)),
            ),
            AnnotationPropertyRange(p, c) => (
                "AnnotationPropertyRange",
                format!("{} {}", iri_ref(p), iri_ref(c)),
            ),
        };
        format!("{name}({anns}{body})")
    }

    fn annotation(&mut self, a: &Annotation) -> String {
        let nested: String = a
            .annotations
            .iter()
            .map(|x| format!("{} ", self.annotation(x)))
            .collect();
        format!(
            "Annotation({nested}{} {})",
            iri_ref(&a.property),
            self.value(&a.value)
        )
    }

    fn value(&mut self, v: &AnnotationValue) -> String {
        match v {
            AnnotationValue::Iri(i) => iri_ref(i),
            AnnotationValue::Anonymous(b) => self.label(b),
            AnnotationValue::Literal(l) => literal(l),
        }
    }

    fn label(&mut self, blank: &str) -> String {
        let n = self.labels.len();
        self.labels
            .entry(blank.to_string())
            .or_insert_with(|| format!("_:b{n}"))
            .clone()
    }

    fn ind(&mut self, i: &Individual) -> String {
        match i {
            Individual::Named(n) => iri_ref(n),
            Individual::Anonymous(b) => self.label(b),
        }
    }

    fn inds(&mut self, v: &[Individual]) -> String {
        v.iter().map(|i| self.ind(i)).collect::<Vec<_>>().join(" ")
    }

    fn ces(&mut self, v: &[ClassExpr]) -> String {
        v.iter().map(|c| self.ce(c)).collect::<Vec<_>>().join(" ")
    }

    pub fn ce(&mut self, c: &ClassExpr) -> String {
        use ClassExpr::*;
        let card =
            |w: &mut Self, name: &str, n: &u32, p: &ObjectProp, f: &Option<Box<ClassExpr>>| match f
            {
                Some(f) => format!("{name}({n} {} {})", ope(p), w.ce(f)),
                None => format!("{name}({n} {})", ope(p)),
            };
        let dcard = |name: &str, n: &u32, p: &str, f: &Option<DataRange>| match f {
            Some(f) => format!("{name}({n} {} {})", iri_ref(p), dr(f)),
            None => format!("{name}({n} {})", iri_ref(p)),
        };
        match c {
            Class(i) => iri_ref(i),
            // The grammar wants two or more operands (OneOf: one or more), but
            // an RDF list can be shorter: write what it means instead.
            IntersectionOf(v) | UnionOf(v) if v.len() == 1 => self.ce(&v[0]),
            IntersectionOf(v) if v.is_empty() => iri_ref(OWL_THING),
            UnionOf(v) if v.is_empty() => iri_ref(OWL_NOTHING),
            OneOf(v) if v.is_empty() => iri_ref(OWL_NOTHING),
            IntersectionOf(v) => format!("ObjectIntersectionOf({})", self.ces(v)),
            UnionOf(v) => format!("ObjectUnionOf({})", self.ces(v)),
            ComplementOf(x) => format!("ObjectComplementOf({})", self.ce(x)),
            OneOf(v) => format!("ObjectOneOf({})", self.inds(v)),
            SomeValuesFrom(p, x) => format!("ObjectSomeValuesFrom({} {})", ope(p), self.ce(x)),
            AllValuesFrom(p, x) => format!("ObjectAllValuesFrom({} {})", ope(p), self.ce(x)),
            HasValue(p, i) => format!("ObjectHasValue({} {})", ope(p), self.ind(i)),
            HasSelf(p) => format!("ObjectHasSelf({})", ope(p)),
            MinCardinality(n, p, f) => card(self, "ObjectMinCardinality", n, p, f),
            MaxCardinality(n, p, f) => card(self, "ObjectMaxCardinality", n, p, f),
            ExactCardinality(n, p, f) => card(self, "ObjectExactCardinality", n, p, f),
            DataSomeValuesFrom(ps, r) => format!("DataSomeValuesFrom({} {})", iris(ps), dr(r)),
            DataAllValuesFrom(ps, r) => format!("DataAllValuesFrom({} {})", iris(ps), dr(r)),
            DataHasValue(p, l) => format!("DataHasValue({} {})", iri_ref(p), literal(l)),
            DataMinCardinality(n, p, f) => dcard("DataMinCardinality", n, p, f),
            DataMaxCardinality(n, p, f) => dcard("DataMaxCardinality", n, p, f),
            DataExactCardinality(n, p, f) => dcard("DataExactCardinality", n, p, f),
        }
    }
}

pub fn iri_ref(iri: &str) -> String {
    format!("<{iri}>")
}

fn iris(v: &[String]) -> String {
    v.iter().map(|i| iri_ref(i)).collect::<Vec<_>>().join(" ")
}

pub fn ope(p: &ObjectProp) -> String {
    match p {
        ObjectProp::Named(n) => iri_ref(n),
        ObjectProp::Inverse(n) => format!("ObjectInverseOf({})", iri_ref(n)),
    }
}

fn opes(v: &[ObjectProp]) -> String {
    v.iter().map(ope).collect::<Vec<_>>().join(" ")
}

pub fn dr(r: &DataRange) -> String {
    match r {
        DataRange::Datatype(d) => iri_ref(d),
        // As in `ce`: one operand is the operand; no operands is everything or
        // nothing.
        DataRange::IntersectionOf(v) | DataRange::UnionOf(v) if v.len() == 1 => dr(&v[0]),
        DataRange::IntersectionOf(v) if v.is_empty() => iri_ref(RDFS_LITERAL),
        DataRange::UnionOf(v) if v.is_empty() => EMPTY_DATA_RANGE.into(),
        DataRange::OneOf(ls) if ls.is_empty() => EMPTY_DATA_RANGE.into(),
        DataRange::IntersectionOf(v) => {
            format!(
                "DataIntersectionOf({})",
                v.iter().map(dr).collect::<Vec<_>>().join(" ")
            )
        }
        DataRange::UnionOf(v) => format!(
            "DataUnionOf({})",
            v.iter().map(dr).collect::<Vec<_>>().join(" ")
        ),
        DataRange::ComplementOf(x) => format!("DataComplementOf({})", dr(x)),
        DataRange::OneOf(ls) => format!(
            "DataOneOf({})",
            ls.iter().map(literal).collect::<Vec<_>>().join(" ")
        ),
        DataRange::Restriction(d, fs) => format!(
            "DatatypeRestriction({} {})",
            iri_ref(d),
            fs.iter()
                .map(|(f, v)| format!("{} {}", iri_ref(f), literal(v)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn literal(l: &Literal) -> String {
    match &l.lang {
        Some(lang) => format!("\"{}\"@{lang}", escape(&l.lexical)),
        None => format!("\"{}\"^^{}", escape(&l.lexical), iri_ref(&l.datatype)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_nested_expressions_and_annotations() {
        let ont = Ontology {
            iri: Some("http://example.org/o".into()),
            axioms: vec![
                Axiom {
                    kind: AxiomKind::SubClassOf(
                        ClassExpr::Class("http://example.org/A".into()),
                        ClassExpr::SomeValuesFrom(
                            ObjectProp::Inverse("http://example.org/r".into()),
                            Box::new(ClassExpr::Class("http://example.org/B".into())),
                        ),
                    ),
                    annotations: vec![Annotation {
                        property: "http://www.w3.org/2000/01/rdf-schema#comment".into(),
                        value: AnnotationValue::Literal(Literal {
                            lexical: "say \"hi\"".into(),
                            datatype: "http://www.w3.org/2001/XMLSchema#string".into(),
                            lang: None,
                        }),
                        annotations: vec![],
                    }],
                },
                Axiom::new(AxiomKind::ClassAssertion(
                    ClassExpr::Class("http://example.org/A".into()),
                    Individual::Anonymous("x1".into()),
                )),
            ],
            ..Default::default()
        };
        let text = FsWriter::new().ontology(&ont, &[]);
        assert!(text.contains(
            "SubClassOf(Annotation(<http://www.w3.org/2000/01/rdf-schema#comment> \"say \\\"hi\\\"\"^^<http://www.w3.org/2001/XMLSchema#string>) <http://example.org/A> ObjectSomeValuesFrom(ObjectInverseOf(<http://example.org/r>) <http://example.org/B>))"
        ), "{text}");
        assert!(
            text.contains("ClassAssertion(<http://example.org/A> _:b0)"),
            "{text}"
        );
    }

    #[test]
    fn short_operand_lists_stay_in_the_grammar() {
        // ObjectIntersectionOf/UnionOf and their Data* forms need two or more
        // operands, the OneOf forms one or more; RDF lists can be shorter.
        let a = || ClassExpr::Class("http://example.org/A".into());
        let int = || DataRange::Datatype("http://www.w3.org/2001/XMLSchema#integer".into());
        let mut w = FsWriter::new();
        assert_eq!(
            w.ce(&ClassExpr::IntersectionOf(vec![a()])),
            "<http://example.org/A>"
        );
        assert_eq!(
            w.ce(&ClassExpr::UnionOf(vec![a()])),
            "<http://example.org/A>"
        );
        assert_eq!(
            w.ce(&ClassExpr::IntersectionOf(vec![])),
            "<http://www.w3.org/2002/07/owl#Thing>"
        );
        assert_eq!(
            w.ce(&ClassExpr::UnionOf(vec![])),
            "<http://www.w3.org/2002/07/owl#Nothing>"
        );
        assert_eq!(
            w.ce(&ClassExpr::OneOf(vec![])),
            "<http://www.w3.org/2002/07/owl#Nothing>"
        );
        // Nested: the operand itself is still written in full.
        assert_eq!(
            w.ce(&ClassExpr::UnionOf(vec![ClassExpr::IntersectionOf(vec![
                a(),
                ClassExpr::Class("http://example.org/B".into()),
            ])])),
            "ObjectIntersectionOf(<http://example.org/A> <http://example.org/B>)"
        );

        let int_iri = "<http://www.w3.org/2001/XMLSchema#integer>";
        assert_eq!(dr(&DataRange::IntersectionOf(vec![int()])), int_iri);
        assert_eq!(dr(&DataRange::UnionOf(vec![int()])), int_iri);
        let literal = "<http://www.w3.org/2000/01/rdf-schema#Literal>";
        assert_eq!(dr(&DataRange::IntersectionOf(vec![])), literal);
        assert_eq!(dr(&DataRange::UnionOf(vec![])), EMPTY_DATA_RANGE);
        assert_eq!(dr(&DataRange::OneOf(vec![])), EMPTY_DATA_RANGE);
    }

    #[test]
    fn one_element_rdf_list_writes_the_operand() {
        // WebOnt-I5.26-002 shape: `owl:intersectionOf ( ex:B )`.
        let ttl = "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
                   @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
                   @prefix ex: <http://example.org/> . \
                   ex:A rdfs:subClassOf [ a owl:Class ; owl:intersectionOf ( ex:B ) ] . \
                   ex:C rdfs:subClassOf [ a owl:Class ; owl:unionOf ( ex:B ) ] .";
        let triples: Vec<oxigraph::model::Triple> =
            oxigraph::io::RdfParser::from_format(oxigraph::io::RdfFormat::Turtle)
                .for_slice(ttl.as_bytes())
                .map(|q| q.unwrap().into())
                .collect();
        let m = crate::reasoning::owl_mapping::map_triples(&triples);
        assert!(m.unmapped.is_empty(), "{:?}", m.unmapped);
        let text = FsWriter::new().ontology(&m.ontology, &[]);
        assert!(
            text.contains("SubClassOf(<http://example.org/A> <http://example.org/B>)"),
            "{text}"
        );
        assert!(
            text.contains("SubClassOf(<http://example.org/C> <http://example.org/B>)"),
            "{text}"
        );
        assert!(!text.contains("IntersectionOf"), "{text}");
        assert!(!text.contains("UnionOf"), "{text}");
    }
}
