//! OWL 2 class expressions and data ranges inside SWRL atoms (SWRL §2: a
//! `ClassAtom` may hold any OWL description, a `DataRangeAtom` any data
//! range).
//!
//! The readers build these trees; the engine then
//! - evaluates a data range natively over a bound value
//!   ([`super::datarange`]), and
//! - replaces a class-expression atom by a class atom over an auxiliary named
//!   class `urn:ots:swrl:aux:<hash>`, adding
//!   `aux owl:equivalentClass <expression>` (the expression in the OWL 2 RDF
//!   mapping) to what the regime reasons over, so the regime materialises who
//!   belongs to it ([`AuxClass`]).
//!
//! The hash is taken over the expression's functional-syntax rendering, so the
//! same expression always gets the same auxiliary class.

use std::fmt;

use oxigraph::model::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};
use sha2::{Digest, Sha256};

const OWL: &str = "http://www.w3.org/2002/07/owl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The prefix of auxiliary class IRIs.
pub const AUX_PREFIX: &str = "urn:ots:swrl:aux:";

/// An object property expression: a property or its inverse.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PropExpr {
    Named(String),
    Inverse(String),
}

/// An OWL 2 class expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ClassExpr {
    Named(String),
    IntersectionOf(Vec<ClassExpr>),
    UnionOf(Vec<ClassExpr>),
    ComplementOf(Box<ClassExpr>),
    OneOf(Vec<String>),
    SomeValuesFrom(PropExpr, Box<ClassExpr>),
    AllValuesFrom(PropExpr, Box<ClassExpr>),
    HasValue(PropExpr, String),
    HasSelf(PropExpr),
    /// `ObjectMin/Max/ExactCardinality(n p [C])`.
    Cardinality {
        kind: CardKind,
        n: u64,
        prop: PropExpr,
        filler: Option<Box<ClassExpr>>,
    },
    DataSomeValuesFrom(String, DataRange),
    DataAllValuesFrom(String, DataRange),
    DataHasValue(String, Literal),
    DataCardinality {
        kind: CardKind,
        n: u64,
        prop: String,
        range: Option<DataRange>,
    },
}

/// Which cardinality restriction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardKind {
    Min,
    Max,
    Exact,
}

impl CardKind {
    fn name(self) -> &'static str {
        match self {
            CardKind::Min => "Min",
            CardKind::Max => "Max",
            CardKind::Exact => "Exact",
        }
    }

    fn rdf(self, qualified: bool) -> &'static str {
        match (self, qualified) {
            (CardKind::Min, false) => "minCardinality",
            (CardKind::Max, false) => "maxCardinality",
            (CardKind::Exact, false) => "cardinality",
            (CardKind::Min, true) => "minQualifiedCardinality",
            (CardKind::Max, true) => "maxQualifiedCardinality",
            (CardKind::Exact, true) => "qualifiedCardinality",
        }
    }
}

/// An OWL 2 data range.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DataRange {
    Datatype(String),
    IntersectionOf(Vec<DataRange>),
    UnionOf(Vec<DataRange>),
    ComplementOf(Box<DataRange>),
    OneOf(Vec<Literal>),
    /// `DatatypeRestriction(dt facet value …)`: facet IRIs with their values.
    Restriction(String, Vec<(String, Literal)>),
}

fn nn(iri: &str) -> NamedNode {
    NamedNode::new_unchecked(iri)
}

fn owl(local: &str) -> NamedNode {
    nn(&format!("{OWL}{local}"))
}

fn rdf(local: &str) -> NamedNode {
    nn(&format!("{RDF}{local}"))
}

/// Checks every IRI in an expression, so nothing unrepresentable reaches the
/// regime's input.
fn check_iri(iri: &str, what: &str) -> Result<(), String> {
    NamedNode::new(iri)
        .map(|_| ())
        .map_err(|e| format!("Invalid {what} IRI '{iri}' in a class expression: {e}"))
}

impl PropExpr {
    fn validate(&self) -> Result<(), String> {
        match self {
            PropExpr::Named(p) | PropExpr::Inverse(p) => check_iri(p, "object property"),
        }
    }

    fn to_rdf(&self, out: &mut Vec<Triple>) -> NamedOrBlankNode {
        match self {
            PropExpr::Named(p) => nn(p).into(),
            PropExpr::Inverse(p) => {
                let b = BlankNode::default();
                out.push(Triple::new(b.clone(), owl("inverseOf"), nn(p)));
                b.into()
            }
        }
    }
}

impl ClassExpr {
    /// Every IRI in the expression is a valid IRI.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            ClassExpr::Named(c) => check_iri(c, "class"),
            ClassExpr::IntersectionOf(xs) | ClassExpr::UnionOf(xs) => {
                if xs.is_empty() {
                    return Err("an empty class intersection or union".to_string());
                }
                xs.iter().try_for_each(ClassExpr::validate)
            }
            ClassExpr::ComplementOf(x) => x.validate(),
            ClassExpr::OneOf(is) => {
                if is.is_empty() {
                    return Err("an empty ObjectOneOf".to_string());
                }
                is.iter().try_for_each(|i| check_iri(i, "individual"))
            }
            ClassExpr::SomeValuesFrom(p, c) | ClassExpr::AllValuesFrom(p, c) => {
                p.validate()?;
                c.validate()
            }
            ClassExpr::HasValue(p, i) => {
                p.validate()?;
                check_iri(i, "individual")
            }
            ClassExpr::HasSelf(p) => p.validate(),
            ClassExpr::Cardinality { prop, filler, .. } => {
                prop.validate()?;
                filler.as_ref().map_or(Ok(()), |f| f.validate())
            }
            ClassExpr::DataSomeValuesFrom(p, r) | ClassExpr::DataAllValuesFrom(p, r) => {
                check_iri(p, "data property")?;
                r.validate()
            }
            ClassExpr::DataHasValue(p, _) => check_iri(p, "data property"),
            ClassExpr::DataCardinality { prop, range, .. } => {
                check_iri(prop, "data property")?;
                range.as_ref().map_or(Ok(()), |r| r.validate())
            }
        }
    }

    /// The expression in the OWL 2 RDF mapping: its node (the class IRI for a
    /// named class, else a fresh blank node) and the triples describing it.
    pub fn to_rdf(&self, out: &mut Vec<Triple>) -> NamedOrBlankNode {
        let fresh = |out: &mut Vec<Triple>, ty: &str| {
            let b = BlankNode::default();
            out.push(Triple::new(b.clone(), rdf("type"), owl(ty)));
            b
        };
        match self {
            ClassExpr::Named(c) => nn(c).into(),
            ClassExpr::IntersectionOf(xs) | ClassExpr::UnionOf(xs) => {
                let b = fresh(out, "Class");
                let items: Vec<Term> = xs.iter().map(|x| x.to_rdf(out).into()).collect();
                let list = rdf_list(&items, out);
                let p = if matches!(self, ClassExpr::IntersectionOf(_)) {
                    "intersectionOf"
                } else {
                    "unionOf"
                };
                out.push(Triple::new(b.clone(), owl(p), list));
                b.into()
            }
            ClassExpr::ComplementOf(x) => {
                let b = fresh(out, "Class");
                let inner = x.to_rdf(out);
                out.push(Triple::new(b.clone(), owl("complementOf"), inner));
                b.into()
            }
            ClassExpr::OneOf(is) => {
                let b = fresh(out, "Class");
                let items: Vec<Term> = is.iter().map(|i| nn(i).into()).collect();
                let list = rdf_list(&items, out);
                out.push(Triple::new(b.clone(), owl("oneOf"), list));
                b.into()
            }
            ClassExpr::SomeValuesFrom(p, c) | ClassExpr::AllValuesFrom(p, c) => {
                let b = fresh(out, "Restriction");
                let prop = p.to_rdf(out);
                let filler = c.to_rdf(out);
                let k = if matches!(self, ClassExpr::SomeValuesFrom(..)) {
                    "someValuesFrom"
                } else {
                    "allValuesFrom"
                };
                out.push(Triple::new(b.clone(), owl("onProperty"), prop));
                out.push(Triple::new(b.clone(), owl(k), filler));
                b.into()
            }
            ClassExpr::HasValue(p, i) => {
                let b = fresh(out, "Restriction");
                let prop = p.to_rdf(out);
                out.push(Triple::new(b.clone(), owl("onProperty"), prop));
                out.push(Triple::new(b.clone(), owl("hasValue"), nn(i)));
                b.into()
            }
            ClassExpr::HasSelf(p) => {
                let b = fresh(out, "Restriction");
                let prop = p.to_rdf(out);
                out.push(Triple::new(b.clone(), owl("onProperty"), prop));
                out.push(Triple::new(
                    b.clone(),
                    owl("hasSelf"),
                    Literal::new_typed_literal("true", nn(&format!("{XSD}boolean"))),
                ));
                b.into()
            }
            ClassExpr::Cardinality {
                kind,
                n,
                prop,
                filler,
            } => {
                let b = fresh(out, "Restriction");
                let p = prop.to_rdf(out);
                out.push(Triple::new(b.clone(), owl("onProperty"), p));
                out.push(Triple::new(
                    b.clone(),
                    owl(kind.rdf(filler.is_some())),
                    non_negative(*n),
                ));
                if let Some(f) = filler {
                    let f = f.to_rdf(out);
                    out.push(Triple::new(b.clone(), owl("onClass"), f));
                }
                b.into()
            }
            ClassExpr::DataSomeValuesFrom(p, r) | ClassExpr::DataAllValuesFrom(p, r) => {
                let b = fresh(out, "Restriction");
                let range = r.to_rdf(out);
                let k = if matches!(self, ClassExpr::DataSomeValuesFrom(..)) {
                    "someValuesFrom"
                } else {
                    "allValuesFrom"
                };
                out.push(Triple::new(b.clone(), owl("onProperty"), nn(p)));
                out.push(Triple::new(b.clone(), owl(k), range));
                b.into()
            }
            ClassExpr::DataHasValue(p, v) => {
                let b = fresh(out, "Restriction");
                out.push(Triple::new(b.clone(), owl("onProperty"), nn(p)));
                out.push(Triple::new(b.clone(), owl("hasValue"), v.clone()));
                b.into()
            }
            ClassExpr::DataCardinality {
                kind,
                n,
                prop,
                range,
            } => {
                let b = fresh(out, "Restriction");
                out.push(Triple::new(b.clone(), owl("onProperty"), nn(prop)));
                out.push(Triple::new(
                    b.clone(),
                    owl(kind.rdf(range.is_some())),
                    non_negative(*n),
                ));
                if let Some(r) = range {
                    let r = r.to_rdf(out);
                    out.push(Triple::new(b.clone(), owl("onDataRange"), r));
                }
                b.into()
            }
        }
    }
}

impl DataRange {
    /// Every IRI in the range is a valid IRI, and every list is non-empty.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            DataRange::Datatype(d) => check_iri(d, "datatype"),
            DataRange::IntersectionOf(xs) | DataRange::UnionOf(xs) => {
                if xs.is_empty() {
                    return Err("an empty data intersection or union".to_string());
                }
                xs.iter().try_for_each(DataRange::validate)
            }
            DataRange::ComplementOf(x) => x.validate(),
            DataRange::OneOf(vs) => {
                if vs.is_empty() {
                    return Err("an empty DataOneOf".to_string());
                }
                Ok(())
            }
            DataRange::Restriction(d, facets) => {
                check_iri(d, "datatype")?;
                facets.iter().try_for_each(|(f, _)| check_iri(f, "facet"))
            }
        }
    }

    /// The range in the OWL 2 RDF mapping.
    pub fn to_rdf(&self, out: &mut Vec<Triple>) -> NamedOrBlankNode {
        let fresh = |out: &mut Vec<Triple>| {
            let b = BlankNode::default();
            out.push(Triple::new(
                b.clone(),
                rdf("type"),
                nn(&format!("{RDFS}Datatype")),
            ));
            b
        };
        match self {
            DataRange::Datatype(d) => nn(d).into(),
            DataRange::IntersectionOf(xs) | DataRange::UnionOf(xs) => {
                let b = fresh(out);
                let items: Vec<Term> = xs.iter().map(|x| x.to_rdf(out).into()).collect();
                let list = rdf_list(&items, out);
                let p = if matches!(self, DataRange::IntersectionOf(_)) {
                    "intersectionOf"
                } else {
                    "unionOf"
                };
                out.push(Triple::new(b.clone(), owl(p), list));
                b.into()
            }
            DataRange::ComplementOf(x) => {
                let b = fresh(out);
                let inner = x.to_rdf(out);
                out.push(Triple::new(b.clone(), owl("datatypeComplementOf"), inner));
                b.into()
            }
            DataRange::OneOf(vs) => {
                let b = fresh(out);
                let items: Vec<Term> = vs.iter().map(|v| v.clone().into()).collect();
                let list = rdf_list(&items, out);
                out.push(Triple::new(b.clone(), owl("oneOf"), list));
                b.into()
            }
            DataRange::Restriction(d, facets) => {
                let b = fresh(out);
                out.push(Triple::new(b.clone(), owl("onDatatype"), nn(d)));
                let items: Vec<Term> = facets
                    .iter()
                    .map(|(f, v)| {
                        let r = BlankNode::default();
                        out.push(Triple::new(r.clone(), nn(f), v.clone()));
                        r.into()
                    })
                    .collect();
                let list = rdf_list(&items, out);
                out.push(Triple::new(b.clone(), owl("withRestrictions"), list));
                b.into()
            }
        }
    }
}

fn non_negative(n: u64) -> Literal {
    Literal::new_typed_literal(n.to_string(), nn(&format!("{XSD}nonNegativeInteger")))
}

/// An RDF list of `items` (blank-node cells), returning its head.
fn rdf_list(items: &[Term], out: &mut Vec<Triple>) -> Term {
    let mut rest: Term = rdf("nil").into();
    for item in items.iter().rev() {
        let cell = BlankNode::default();
        out.push(Triple::new(cell.clone(), rdf("first"), item.clone()));
        out.push(Triple::new(cell.clone(), rdf("rest"), rest));
        rest = cell.into();
    }
    rest
}

fn join<T: fmt::Display>(xs: &[T]) -> String {
    xs.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

impl fmt::Display for PropExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PropExpr::Named(p) => write!(f, "<{p}>"),
            PropExpr::Inverse(p) => write!(f, "ObjectInverseOf(<{p}>)"),
        }
    }
}

/// OWL 2 functional syntax.
impl fmt::Display for ClassExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClassExpr::Named(c) => write!(f, "<{c}>"),
            ClassExpr::IntersectionOf(xs) => write!(f, "ObjectIntersectionOf({})", join(xs)),
            ClassExpr::UnionOf(xs) => write!(f, "ObjectUnionOf({})", join(xs)),
            ClassExpr::ComplementOf(x) => write!(f, "ObjectComplementOf({x})"),
            ClassExpr::OneOf(is) => write!(
                f,
                "ObjectOneOf({})",
                is.iter()
                    .map(|i| format!("<{i}>"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            ClassExpr::SomeValuesFrom(p, c) => write!(f, "ObjectSomeValuesFrom({p} {c})"),
            ClassExpr::AllValuesFrom(p, c) => write!(f, "ObjectAllValuesFrom({p} {c})"),
            ClassExpr::HasValue(p, i) => write!(f, "ObjectHasValue({p} <{i}>)"),
            ClassExpr::HasSelf(p) => write!(f, "ObjectHasSelf({p})"),
            ClassExpr::Cardinality {
                kind,
                n,
                prop,
                filler,
            } => match filler {
                Some(c) => write!(f, "Object{}Cardinality({n} {prop} {c})", kind.name()),
                None => write!(f, "Object{}Cardinality({n} {prop})", kind.name()),
            },
            ClassExpr::DataSomeValuesFrom(p, r) => write!(f, "DataSomeValuesFrom(<{p}> {r})"),
            ClassExpr::DataAllValuesFrom(p, r) => write!(f, "DataAllValuesFrom(<{p}> {r})"),
            ClassExpr::DataHasValue(p, v) => write!(f, "DataHasValue(<{p}> {v})"),
            ClassExpr::DataCardinality {
                kind,
                n,
                prop,
                range,
            } => match range {
                Some(r) => write!(f, "Data{}Cardinality({n} <{prop}> {r})", kind.name()),
                None => write!(f, "Data{}Cardinality({n} <{prop}>)", kind.name()),
            },
        }
    }
}

/// OWL 2 functional syntax.
impl fmt::Display for DataRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataRange::Datatype(d) => write!(f, "<{d}>"),
            DataRange::IntersectionOf(xs) => write!(f, "DataIntersectionOf({})", join(xs)),
            DataRange::UnionOf(xs) => write!(f, "DataUnionOf({})", join(xs)),
            DataRange::ComplementOf(x) => write!(f, "DataComplementOf({x})"),
            DataRange::OneOf(vs) => write!(f, "DataOneOf({})", join(vs)),
            DataRange::Restriction(d, facets) => write!(
                f,
                "DatatypeRestriction(<{d}> {})",
                facets
                    .iter()
                    .map(|(k, v)| format!("<{k}> {v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        }
    }
}

/// A class expression standing in a rule, named by an auxiliary class.
#[derive(Debug, Clone)]
pub struct AuxClass {
    /// `urn:ots:swrl:aux:<hash>`.
    pub iri: String,
    pub expr: ClassExpr,
}

impl AuxClass {
    pub fn new(expr: ClassExpr) -> Self {
        let digest = Sha256::digest(expr.to_string().as_bytes());
        AuxClass {
            iri: format!("{AUX_PREFIX}{}", hex::encode(&digest[..16])),
            expr,
        }
    }

    /// `aux owl:equivalentClass <expression>` plus the expression's triples.
    pub fn axioms(&self) -> Vec<Triple> {
        let mut out = Vec::new();
        let node = self.expr.to_rdf(&mut out);
        out.push(Triple::new(nn(&self.iri), owl("equivalentClass"), node));
        out.push(Triple::new(nn(&self.iri), rdf("type"), owl("Class")));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aux_classes_are_deterministic_and_mapped_to_rdf() {
        let e = ClassExpr::SomeValuesFrom(
            PropExpr::Inverse("http://ex/p".into()),
            Box::new(ClassExpr::Named("http://ex/B".into())),
        );
        let a = AuxClass::new(e.clone());
        assert_eq!(a.iri, AuxClass::new(e).iri);
        assert!(a.iri.starts_with(AUX_PREFIX));
        assert_eq!(
            a.expr.to_string(),
            "ObjectSomeValuesFrom(ObjectInverseOf(<http://ex/p>) <http://ex/B>)"
        );
        let axioms = a.axioms();
        let has = |p: &str| axioms.iter().any(|t| t.predicate.as_str() == p);
        assert!(has(&format!("{OWL}equivalentClass")));
        assert!(has(&format!("{OWL}someValuesFrom")));
        assert!(has(&format!("{OWL}inverseOf")));
        let other = AuxClass::new(ClassExpr::ComplementOf(Box::new(ClassExpr::Named(
            "http://ex/B".into(),
        ))));
        assert_ne!(other.iri, a.iri);
    }
}
