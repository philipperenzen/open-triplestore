//! The SWRL RDF syntax (SWRL submission §5): rules stored as triples.
//!
//! A rule is a `swrl:Imp` with `swrl:body` and `swrl:head`, each an RDF list
//! of atoms; an atom is typed `swrl:ClassAtom` (`swrl:classPredicate`,
//! `swrl:argument1`), `swrl:IndividualPropertyAtom` or
//! `swrl:DatavaluedPropertyAtom` (`swrl:propertyPredicate`, `swrl:argument1`,
//! `swrl:argument2`), `swrl:SameIndividualAtom` or
//! `swrl:DifferentIndividualsAtom` (`swrl:argument1`, `swrl:argument2`), or
//! `swrl:BuiltinAtom` (`swrl:builtin`, `swrl:arguments` as an RDF list). An
//! argument typed `swrl:Variable` is a variable; any other IRI is an
//! individual, a literal is a data value.
//!
//! A `swrl:classPredicate` may be a class expression (a blank node, read by
//! the OWL 2 RDF mapping: `owl:intersectionOf`, `owl:unionOf`,
//! `owl:complementOf`, `owl:oneOf`, and `owl:Restriction`s with
//! `owl:someValuesFrom`, `owl:allValuesFrom`, `owl:hasValue`, `owl:hasSelf`
//! and the (qualified) cardinalities), and a `swrl:DataRangeAtom` holds its
//! range in `swrl:dataRange` (a datatype, `owl:oneOf`, `owl:intersectionOf`,
//! `owl:unionOf`, `owl:datatypeComplementOf` or `owl:onDatatype` with
//! `owl:withRestrictions`).
//!
//! As strict as the OWL/XML reader: an atom of an unknown type, a missing or
//! repeated required property, a malformed list or expression, or an
//! anonymous individual refuses the rule set with a message naming the rule.
//!
//! Rules are read from a parsed document ([`parse_swrl_rdf`]) or straight
//! from graphs in the store ([`rules_in_graphs`]), fetching triples by
//! subject so a large model graph that also holds rules is never loaded whole.

use std::collections::HashSet;

use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{
    Graph, GraphNameRef, NamedNode, NamedNodeRef, NamedOrBlankNode, Term, TermRef,
};

use super::engine::{Atom, SwrlArg, SwrlRule};
use super::expr::{CardKind, ClassExpr, DataRange, PropExpr};

const SWRL: &str = "http://www.w3.org/2003/11/swrl#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const RDFS_DATATYPE: &str = "http://www.w3.org/2000/01/rdf-schema#Datatype";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

fn swrl(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{SWRL}{local}"))
}

fn iri(s: &str) -> NamedNodeRef<'_> {
    NamedNodeRef::new_unchecked(s)
}

/// Where the triples come from.
trait RuleSource {
    /// Every `swrl:Imp`, in a stable order.
    fn imps(&self) -> Result<Vec<NamedOrBlankNode>, String>;
    /// The objects of `(subject, predicate, ?o)`, deduplicated.
    fn objects(
        &self,
        subject: &NamedOrBlankNode,
        predicate: NamedNodeRef<'_>,
    ) -> Result<Vec<Term>, String>;
}

struct GraphSource<'a>(&'a Graph);

impl RuleSource for GraphSource<'_> {
    fn imps(&self) -> Result<Vec<NamedOrBlankNode>, String> {
        let imp = swrl("Imp");
        let mut out: Vec<NamedOrBlankNode> = self
            .0
            .subjects_for_predicate_object(iri(RDF_TYPE), imp.as_ref())
            .map(|s| s.into_owned())
            .collect();
        out.sort_by_key(|s| s.to_string());
        Ok(out)
    }

    fn objects(
        &self,
        subject: &NamedOrBlankNode,
        predicate: NamedNodeRef<'_>,
    ) -> Result<Vec<Term>, String> {
        Ok(self
            .0
            .objects_for_subject_predicate(subject.as_ref(), predicate)
            .map(|o| o.into_owned())
            .collect())
    }
}

struct StoreSource<'a> {
    store: &'a oxigraph::store::Store,
    graphs: Vec<NamedNode>,
}

impl RuleSource for StoreSource<'_> {
    fn imps(&self) -> Result<Vec<NamedOrBlankNode>, String> {
        let imp = swrl("Imp");
        let mut out = Vec::new();
        for g in &self.graphs {
            for q in self.store.quads_for_pattern(
                None,
                Some(iri(RDF_TYPE)),
                Some(TermRef::NamedNode(imp.as_ref())),
                Some(GraphNameRef::NamedNode(g.as_ref())),
            ) {
                let q = q.map_err(|e| format!("reading rules from <{g}>: {e}"))?;
                if !out.contains(&q.subject) {
                    out.push(q.subject);
                }
            }
        }
        out.sort_by_key(|s| s.to_string());
        Ok(out)
    }

    fn objects(
        &self,
        subject: &NamedOrBlankNode,
        predicate: NamedNodeRef<'_>,
    ) -> Result<Vec<Term>, String> {
        let mut out = Vec::new();
        for g in &self.graphs {
            for q in self.store.quads_for_pattern(
                Some(subject.as_ref()),
                Some(predicate),
                None,
                Some(GraphNameRef::NamedNode(g.as_ref())),
            ) {
                let q = q.map_err(|e| format!("reading rules from <{g}>: {e}"))?;
                if !out.contains(&q.object) {
                    out.push(q.object);
                }
            }
        }
        Ok(out)
    }
}

/// Parse a document in the SWRL RDF syntax. Graph names in a quad format are
/// ignored: every triple counts.
pub fn parse_swrl_rdf(
    text: &str,
    format: RdfFormat,
    base_iri: Option<&str>,
) -> Result<Vec<SwrlRule>, String> {
    let mut parser = RdfParser::from_format(format);
    if let Some(base) = base_iri {
        parser = parser
            .with_base_iri(base)
            .map_err(|e| format!("Invalid base IRI '{base}': {e}"))?;
    }
    let mut graph = Graph::new();
    for quad in parser.for_slice(text.as_bytes()) {
        let quad = quad.map_err(|e| format!("RDF parse error: {e}"))?;
        graph.insert(quad.as_ref());
    }
    read_rules(&GraphSource(&graph))
}

/// The `swrl:Imp` rules stored in `graphs` of `store`, read together (a
/// variable may be declared in another of the graphs than the rule).
pub fn rules_in_graphs(
    store: &oxigraph::store::Store,
    graphs: &[String],
) -> Result<Vec<SwrlRule>, String> {
    let graphs = graphs
        .iter()
        .map(|g| NamedNode::new(g.as_str()).map_err(|e| format!("Invalid graph IRI '{g}': {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    read_rules(&StoreSource { store, graphs })
}

/// Whether any of `graphs` holds a `swrl:Imp`. Cheap: one index probe per
/// graph.
pub fn graphs_hold_rules(store: &oxigraph::store::Store, graphs: &[String]) -> bool {
    let imp = swrl("Imp");
    graphs.iter().any(|g| {
        NamedNode::new(g.as_str()).is_ok_and(|g| {
            store
                .quads_for_pattern(
                    None,
                    Some(iri(RDF_TYPE)),
                    Some(TermRef::NamedNode(imp.as_ref())),
                    Some(GraphNameRef::NamedNode(g.as_ref())),
                )
                .next()
                .is_some()
        })
    })
}

fn read_rules(src: &dyn RuleSource) -> Result<Vec<SwrlRule>, String> {
    let mut rules = Vec::new();
    let mut errors = Vec::new();
    for imp in src.imps()? {
        match read_rule(src, &imp) {
            Ok(rule) => rules.push(rule),
            Err(e) => errors.push(format!("rule {imp}: {e}")),
        }
    }
    if !errors.is_empty() {
        return Err(format!("SWRL RDF rules refused: {}", errors.join("; ")));
    }
    Ok(rules)
}

fn read_rule(src: &dyn RuleSource, imp: &NamedOrBlankNode) -> Result<SwrlRule, String> {
    let body = match at_most_one(src, imp, "body")? {
        Some(list) => read_list(src, &list)?,
        None => Vec::new(),
    };
    let head = match at_most_one(src, imp, "head")? {
        Some(list) => read_list(src, &list)?,
        None => Vec::new(),
    };
    if head.is_empty() {
        return Err("swrl:Imp has no swrl:head atoms".to_string());
    }
    Ok(SwrlRule {
        name: match imp {
            NamedOrBlankNode::NamedNode(n) => Some(n.as_str().to_string()),
            _ => None,
        },
        body: body
            .iter()
            .map(|a| read_atom(src, a))
            .collect::<Result<_, _>>()?,
        head: head
            .iter()
            .map(|a| read_atom(src, a))
            .collect::<Result<_, _>>()?,
    })
}

fn at_most_one(
    src: &dyn RuleSource,
    subject: &NamedOrBlankNode,
    local: &str,
) -> Result<Option<Term>, String> {
    let mut values = src.objects(subject, swrl(local).as_ref())?;
    match values.len() {
        0 => Ok(None),
        1 => Ok(values.pop()),
        n => Err(format!("{n} values of swrl:{local}, expected one")),
    }
}

fn exactly_one(
    src: &dyn RuleSource,
    subject: &NamedOrBlankNode,
    local: &str,
    atom: &str,
) -> Result<Term, String> {
    at_most_one(src, subject, local)?.ok_or_else(|| format!("{atom} has no swrl:{local}"))
}

fn as_resource(term: &Term) -> Option<NamedOrBlankNode> {
    match term {
        Term::NamedNode(n) => Some(n.clone().into()),
        Term::BlankNode(b) => Some(b.clone().into()),
        _ => None,
    }
}

/// The members of an RDF list, refusing a cycle or a cell without exactly
/// one `rdf:first` and one `rdf:rest`.
fn read_list(src: &dyn RuleSource, head: &Term) -> Result<Vec<Term>, String> {
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut cell = head.clone();
    loop {
        if matches!(&cell, Term::NamedNode(n) if n.as_str() == RDF_NIL) {
            return Ok(items);
        }
        let node = as_resource(&cell).ok_or_else(|| format!("{cell} is not an RDF list"))?;
        if !seen.insert(node.clone()) {
            return Err("cyclic RDF list".to_string());
        }
        let mut first = src.objects(&node, iri(RDF_FIRST))?;
        let mut rest = src.objects(&node, iri(RDF_REST))?;
        if first.len() != 1 || rest.len() != 1 {
            return Err(format!(
                "malformed RDF list cell {node}: needs one rdf:first and one rdf:rest"
            ));
        }
        items.push(first.pop().expect("length checked"));
        cell = rest.pop().expect("length checked");
    }
}

fn read_atom(src: &dyn RuleSource, term: &Term) -> Result<Atom, String> {
    let node = as_resource(term).ok_or_else(|| format!("{term} is not an atom"))?;
    let types: Vec<String> = src
        .objects(&node, iri(RDF_TYPE))?
        .into_iter()
        .filter_map(|t| match t {
            Term::NamedNode(n) if n.as_str().starts_with(SWRL) => {
                Some(n.as_str()[SWRL.len()..].to_string())
            }
            _ => None,
        })
        .filter(|t| t != "Atom")
        .collect();
    let kind = match types.as_slice() {
        [one] => one.as_str(),
        [] => return Err(format!("atom {node} has no swrl: atom type")),
        _ => return Err(format!("atom {node} has several swrl: types: {types:?}")),
    };
    let arg = |local: &str| -> Result<SwrlArg, String> {
        read_arg(src, &exactly_one(src, &node, local, kind)?)
    };
    let predicate = |local: &str| -> Result<String, String> {
        match exactly_one(src, &node, local, kind)? {
            Term::NamedNode(n) => Ok(n.as_str().to_string()),
            other => Err(format!(
                "swrl:{local} of {kind} must be an IRI, found {other}"
            )),
        }
    };
    Ok(match kind {
        "ClassAtom" => match read_class(src, &exactly_one(src, &node, "classPredicate", kind)?, 0)?
        {
            ClassExpr::Named(class_iri) => Atom::ClassAtom {
                class_iri,
                arg: arg("argument1")?,
            },
            expr => Atom::ClassExpressionAtom {
                expr,
                arg: arg("argument1")?,
            },
        },
        "IndividualPropertyAtom" => Atom::ObjectPropertyAtom {
            property: predicate("propertyPredicate")?,
            arg1: arg("argument1")?,
            arg2: arg("argument2")?,
        },
        "DatavaluedPropertyAtom" => Atom::DataPropertyAtom {
            property: predicate("propertyPredicate")?,
            arg1: arg("argument1")?,
            arg2: arg("argument2")?,
        },
        "SameIndividualAtom" => Atom::SameIndividualAtom {
            arg1: arg("argument1")?,
            arg2: arg("argument2")?,
        },
        "DifferentIndividualsAtom" => Atom::DifferentIndividualsAtom {
            arg1: arg("argument1")?,
            arg2: arg("argument2")?,
        },
        "BuiltinAtom" => {
            let list = exactly_one(src, &node, "arguments", kind)?;
            let args = read_list(src, &list)?
                .iter()
                .map(|a| read_arg(src, a))
                .collect::<Result<Vec<_>, _>>()?;
            if args.is_empty() {
                return Err("BuiltinAtom has no arguments".to_string());
            }
            Atom::BuiltinAtom {
                builtin: predicate("builtin")?,
                args,
            }
        }
        "DataRangeAtom" => Atom::DataRangeAtom {
            range: read_range(src, &exactly_one(src, &node, "dataRange", kind)?, 0)?,
            arg: arg("argument1")?,
        },
        other => return Err(format!("unknown SWRL atom type swrl:{other}")),
    })
}

fn owl(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{OWL}{local}"))
}

/// How deep an expression may nest (a cycle in the blank nodes would recurse
/// forever).
const MAX_DEPTH: usize = 64;

fn one_owl(
    src: &dyn RuleSource,
    node: &NamedOrBlankNode,
    local: &str,
) -> Result<Option<Term>, String> {
    let mut v = src.objects(node, owl(local).as_ref())?;
    match v.len() {
        0 => Ok(None),
        1 => Ok(v.pop()),
        n => Err(format!("{n} values of owl:{local} on {node}, expected one")),
    }
}

fn has_type(src: &dyn RuleSource, node: &NamedOrBlankNode, ty: &str) -> Result<bool, String> {
    Ok(src
        .objects(node, iri(RDF_TYPE))?
        .iter()
        .any(|t| matches!(t, Term::NamedNode(n) if n.as_str() == ty)))
}

fn cardinality(t: &Term) -> Result<u64, String> {
    match t {
        Term::Literal(l) => l
            .value()
            .trim()
            .parse()
            .map_err(|_| format!("cardinality {t} is not a non-negative integer")),
        other => Err(format!("cardinality {other} is not a literal")),
    }
}

fn read_prop(src: &dyn RuleSource, t: &Term) -> Result<PropExpr, String> {
    match t {
        Term::NamedNode(n) => Ok(PropExpr::Named(n.as_str().to_string())),
        Term::BlankNode(b) => match one_owl(src, &b.clone().into(), "inverseOf")? {
            Some(Term::NamedNode(p)) => Ok(PropExpr::Inverse(p.as_str().to_string())),
            _ => Err(format!(
                "property expression {t} is not owl:inverseOf a property"
            )),
        },
        other => Err(format!("{other} is not a property")),
    }
}

/// Whether `t` reads as a data range rather than a class: a datatype IRI or
/// a node typed `rdfs:Datatype`.
fn is_data_range(src: &dyn RuleSource, t: &Term) -> Result<bool, String> {
    Ok(match t {
        Term::NamedNode(n) => {
            let s = n.as_str();
            s.starts_with(XSD)
                || s == "http://www.w3.org/2000/01/rdf-schema#Literal"
                || s == "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral"
                || s == "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"
                || s == "http://www.w3.org/2002/07/owl#real"
                || s == "http://www.w3.org/2002/07/owl#rational"
                || has_type(src, &n.clone().into(), RDFS_DATATYPE)?
        }
        Term::BlankNode(b) => has_type(src, &b.clone().into(), RDFS_DATATYPE)?,
        _ => false,
    })
}

/// A class expression in the OWL 2 RDF mapping.
fn read_class(src: &dyn RuleSource, t: &Term, depth: usize) -> Result<ClassExpr, String> {
    if depth > MAX_DEPTH {
        return Err("class expression nested too deeply (or cyclic)".to_string());
    }
    let node = match t {
        Term::NamedNode(n) => return Ok(ClassExpr::Named(n.as_str().to_string())),
        Term::BlankNode(b) => NamedOrBlankNode::from(b.clone()),
        other => return Err(format!("{other} is not a class expression")),
    };
    let classes = |list: Term| -> Result<Vec<ClassExpr>, String> {
        read_list(src, &list)?
            .iter()
            .map(|c| read_class(src, c, depth + 1))
            .collect()
    };
    if let Some(l) = one_owl(src, &node, "intersectionOf")? {
        return Ok(ClassExpr::IntersectionOf(classes(l)?));
    }
    if let Some(l) = one_owl(src, &node, "unionOf")? {
        return Ok(ClassExpr::UnionOf(classes(l)?));
    }
    if let Some(c) = one_owl(src, &node, "complementOf")? {
        return Ok(ClassExpr::ComplementOf(Box::new(read_class(
            src,
            &c,
            depth + 1,
        )?)));
    }
    if let Some(l) = one_owl(src, &node, "oneOf")? {
        let items = read_list(src, &l)?
            .into_iter()
            .map(|i| match i {
                Term::NamedNode(n) => Ok(n.as_str().to_string()),
                other => Err(format!(
                    "owl:oneOf member {other} is not a named individual"
                )),
            })
            .collect::<Result<_, _>>()?;
        return Ok(ClassExpr::OneOf(items));
    }
    let Some(on) = one_owl(src, &node, "onProperty")? else {
        return Err(format!(
            "class expression {node} is not an owl:intersectionOf, unionOf, complementOf, \
             oneOf or owl:Restriction"
        ));
    };
    let data_prop = match &on {
        Term::NamedNode(n) => has_type(src, &n.clone().into(), &format!("{OWL}DatatypeProperty"))?,
        _ => false,
    };
    let prop_iri = || match &on {
        Term::NamedNode(n) => Ok(n.as_str().to_string()),
        other => Err(format!("data property {other} is not an IRI")),
    };
    for (local, some) in [("someValuesFrom", true), ("allValuesFrom", false)] {
        if let Some(f) = one_owl(src, &node, local)? {
            return Ok(if is_data_range(src, &f)? {
                let r = read_range(src, &f, depth + 1)?;
                if some {
                    ClassExpr::DataSomeValuesFrom(prop_iri()?, r)
                } else {
                    ClassExpr::DataAllValuesFrom(prop_iri()?, r)
                }
            } else {
                let p = read_prop(src, &on)?;
                let c = Box::new(read_class(src, &f, depth + 1)?);
                if some {
                    ClassExpr::SomeValuesFrom(p, c)
                } else {
                    ClassExpr::AllValuesFrom(p, c)
                }
            });
        }
    }
    if let Some(v) = one_owl(src, &node, "hasValue")? {
        return Ok(match v {
            Term::Literal(l) => ClassExpr::DataHasValue(prop_iri()?, l),
            Term::NamedNode(i) => ClassExpr::HasValue(read_prop(src, &on)?, i.as_str().to_string()),
            other => return Err(format!("owl:hasValue {other} is an anonymous individual")),
        });
    }
    if one_owl(src, &node, "hasSelf")?.is_some() {
        return Ok(ClassExpr::HasSelf(read_prop(src, &on)?));
    }
    for (local, kind, qualified) in [
        ("minCardinality", CardKind::Min, false),
        ("maxCardinality", CardKind::Max, false),
        ("cardinality", CardKind::Exact, false),
        ("minQualifiedCardinality", CardKind::Min, true),
        ("maxQualifiedCardinality", CardKind::Max, true),
        ("qualifiedCardinality", CardKind::Exact, true),
    ] {
        let Some(n) = one_owl(src, &node, local)? else {
            continue;
        };
        let n = cardinality(&n)?;
        let on_class = one_owl(src, &node, "onClass")?;
        let on_range = one_owl(src, &node, "onDataRange")?;
        if qualified && on_class.is_none() && on_range.is_none() {
            return Err(format!(
                "owl:{local} without owl:onClass or owl:onDataRange"
            ));
        }
        if on_range.is_some() || (on_class.is_none() && data_prop) {
            return Ok(ClassExpr::DataCardinality {
                kind,
                n,
                prop: prop_iri()?,
                range: on_range
                    .map(|r| read_range(src, &r, depth + 1))
                    .transpose()?,
            });
        }
        return Ok(ClassExpr::Cardinality {
            kind,
            n,
            prop: read_prop(src, &on)?,
            filler: on_class
                .map(|c| read_class(src, &c, depth + 1).map(Box::new))
                .transpose()?,
        });
    }
    Err(format!(
        "owl:Restriction {node} has no restriction this reader knows"
    ))
}

/// A data range in the OWL 2 RDF mapping.
fn read_range(src: &dyn RuleSource, t: &Term, depth: usize) -> Result<DataRange, String> {
    if depth > MAX_DEPTH {
        return Err("data range nested too deeply (or cyclic)".to_string());
    }
    let node = match t {
        Term::NamedNode(n) => return Ok(DataRange::Datatype(n.as_str().to_string())),
        Term::BlankNode(b) => NamedOrBlankNode::from(b.clone()),
        other => return Err(format!("{other} is not a data range")),
    };
    let ranges = |list: Term| -> Result<Vec<DataRange>, String> {
        read_list(src, &list)?
            .iter()
            .map(|r| read_range(src, r, depth + 1))
            .collect()
    };
    if let Some(l) = one_owl(src, &node, "intersectionOf")? {
        return Ok(DataRange::IntersectionOf(ranges(l)?));
    }
    if let Some(l) = one_owl(src, &node, "unionOf")? {
        return Ok(DataRange::UnionOf(ranges(l)?));
    }
    if let Some(c) = one_owl(src, &node, "datatypeComplementOf")? {
        return Ok(DataRange::ComplementOf(Box::new(read_range(
            src,
            &c,
            depth + 1,
        )?)));
    }
    if let Some(l) = one_owl(src, &node, "oneOf")? {
        let items = read_list(src, &l)?
            .into_iter()
            .map(|i| match i {
                Term::Literal(l) => Ok(l),
                other => Err(format!(
                    "owl:oneOf member {other} of a data range is not a literal"
                )),
            })
            .collect::<Result<_, _>>()?;
        return Ok(DataRange::OneOf(items));
    }
    if let Some(Term::NamedNode(dt)) = one_owl(src, &node, "onDatatype")? {
        let list = one_owl(src, &node, "withRestrictions")?
            .ok_or("owl:onDatatype without owl:withRestrictions")?;
        let mut facets = Vec::new();
        for f in read_list(src, &list)? {
            let f = as_resource(&f).ok_or("a facet restriction is not a node")?;
            // The one property of the restriction node is the facet.
            let mut found = None;
            for facet in [
                "minInclusive",
                "maxInclusive",
                "minExclusive",
                "maxExclusive",
                "length",
                "minLength",
                "maxLength",
                "pattern",
                "totalDigits",
                "fractionDigits",
            ] {
                let p = NamedNode::new_unchecked(format!("{XSD}{facet}"));
                if let Some(Term::Literal(v)) = src.objects(&f, p.as_ref())?.pop() {
                    found = Some((p.as_str().to_string(), v));
                }
            }
            let lr =
                NamedNode::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#langRange");
            if let Some(Term::Literal(v)) = src.objects(&f, lr.as_ref())?.pop() {
                found = Some((lr.as_str().to_string(), v));
            }
            facets
                .push(found.ok_or_else(|| format!("facet restriction {f} names no known facet"))?);
        }
        return Ok(DataRange::Restriction(dt.as_str().to_string(), facets));
    }
    Err(format!("data range {node} is not one this reader knows"))
}

fn read_arg(src: &dyn RuleSource, term: &Term) -> Result<SwrlArg, String> {
    let is_variable = |node: &NamedOrBlankNode| -> Result<bool, String> {
        let var = Term::NamedNode(swrl("Variable"));
        Ok(src.objects(node, iri(RDF_TYPE))?.contains(&var))
    };
    match term {
        Term::NamedNode(n) => {
            let node: NamedOrBlankNode = n.clone().into();
            Ok(if is_variable(&node)? {
                SwrlArg::Variable(n.as_str().to_string())
            } else {
                SwrlArg::Individual(n.as_str().to_string())
            })
        }
        Term::BlankNode(b) => {
            let node: NamedOrBlankNode = b.clone().into();
            if is_variable(&node)? {
                Ok(SwrlArg::Variable(format!("_:{}", b.as_str())))
            } else {
                Err(format!(
                    "argument {term} is a blank node that is not a swrl:Variable; anonymous \
                     individuals are not supported in rules"
                ))
            }
        }
        Term::Literal(l) => Ok(match l.language() {
            Some(lang) => SwrlArg::Literal {
                value: l.value().to_string(),
                datatype: None,
                language: Some(lang.to_string()),
            },
            None => SwrlArg::Literal {
                value: l.value().to_string(),
                datatype: Some(l.datatype().as_str().to_string()).filter(|d| d != RDF_LANG_STRING),
                language: None,
            },
        }),
        #[allow(unreachable_patterns)]
        other => Err(format!(
            "argument {other} is not an IRI, blank node or literal"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: &str = r#"
@prefix ex:   <http://ex/> .
@prefix swrl: <http://www.w3.org/2003/11/swrl#> .
@prefix swrlb: <http://www.w3.org/2003/11/swrlb#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:x a swrl:Variable . ex:y a swrl:Variable . ex:a a swrl:Variable .
ex:Adult a swrl:Imp ;
  swrl:body (
    [ a swrl:ClassAtom ; swrl:classPredicate ex:Person ; swrl:argument1 ex:x ]
    [ a swrl:DatavaluedPropertyAtom ; swrl:propertyPredicate ex:age ;
      swrl:argument1 ex:x ; swrl:argument2 ex:a ]
    [ a swrl:BuiltinAtom ; swrl:builtin swrlb:greaterThan ;
      swrl:arguments ( ex:a "17"^^xsd:integer ) ]
    [ a swrl:IndividualPropertyAtom ; swrl:propertyPredicate ex:knows ;
      swrl:argument1 ex:x ; swrl:argument2 ex:bob ]
  ) ;
  swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:Adult ; swrl:argument1 ex:x ] ) .
"#;

    #[test]
    fn reads_every_atom_kind() {
        let rules = parse_swrl_rdf(RULE, RdfFormat::Turtle, None).unwrap();
        assert_eq!(rules.len(), 1);
        let r = &rules[0];
        assert_eq!(r.name.as_deref(), Some("http://ex/Adult"));
        assert_eq!(r.body.len(), 4);
        assert!(
            matches!(&r.body[0], Atom::ClassAtom { arg: SwrlArg::Variable(v), .. } if v == "http://ex/x")
        );
        assert!(matches!(&r.body[1], Atom::DataPropertyAtom { .. }));
        assert!(
            matches!(&r.body[2], Atom::BuiltinAtom { args, .. } if args.len() == 2
                && matches!(&args[1], SwrlArg::Literal { value, .. } if value == "17"))
        );
        assert!(
            matches!(&r.body[3], Atom::ObjectPropertyAtom { arg2: SwrlArg::Individual(i), .. } if i == "http://ex/bob")
        );
    }

    #[test]
    fn reads_class_expressions_and_data_ranges() {
        let doc = r#"
@prefix ex: <http://ex/> . @prefix swrl: <http://www.w3.org/2003/11/swrl#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
ex:x a swrl:Variable . ex:a a swrl:Variable .
ex:r a swrl:Imp ;
  swrl:body (
    [ a swrl:ClassAtom ; swrl:argument1 ex:x ;
      swrl:classPredicate [ a owl:Restriction ; owl:onProperty [ owl:inverseOf ex:p ] ;
                            owl:someValuesFrom ex:B ] ]
    [ a swrl:DatavaluedPropertyAtom ; swrl:propertyPredicate ex:age ;
      swrl:argument1 ex:x ; swrl:argument2 ex:a ]
    [ a swrl:DataRangeAtom ; swrl:argument1 ex:a ;
      swrl:dataRange [ a rdfs:Datatype ; owl:onDatatype xsd:integer ;
                       owl:withRestrictions ( [ xsd:minInclusive 18 ] ) ] ]
  ) ;
  swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:Adult ; swrl:argument1 ex:x ] ) .
"#;
        let rules = parse_swrl_rdf(doc, RdfFormat::Turtle, None).unwrap();
        let body = &rules[0].body;
        assert!(matches!(&body[0], Atom::ClassExpressionAtom {
            expr: ClassExpr::SomeValuesFrom(PropExpr::Inverse(p), _), .. } if p == "http://ex/p"));
        assert!(
            matches!(&body[2], Atom::DataRangeAtom { range: DataRange::Restriction(dt, f), .. }
            if dt.ends_with("#integer") && f.len() == 1)
        );
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let cases = [
            (
                "ex:r a swrl:Imp ; swrl:body ( [ a swrl:FancyAtom ] ) ; \
                 swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:B ; swrl:argument1 ex:x ] ) .",
                "unknown SWRL atom type",
            ),
            (
                "ex:r a swrl:Imp ; swrl:body ( [ a swrl:DataRangeAtom ] ) ; \
                 swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:B ; swrl:argument1 ex:x ] ) .",
                "has no swrl:dataRange",
            ),
            (
                "ex:r a swrl:Imp ; swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate \
                 [ a <http://www.w3.org/2002/07/owl#Restriction> ] ; swrl:argument1 ex:x ] ) .",
                "class expression",
            ),
            (
                "ex:r a swrl:Imp ; swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:B ] ) .",
                "has no swrl:argument1",
            ),
            (
                "ex:r a swrl:Imp ; swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:B ; \
                 swrl:argument1 [ ] ] ) .",
                "anonymous individuals",
            ),
            ("ex:r a swrl:Imp .", "no swrl:head"),
        ];
        for (ttl, wanted) in cases {
            let doc = format!(
                "@prefix ex: <http://ex/> . @prefix swrl: <http://www.w3.org/2003/11/swrl#> . \
                 ex:x a swrl:Variable . {ttl}"
            );
            let err = parse_swrl_rdf(&doc, RdfFormat::Turtle, None).unwrap_err();
            assert!(err.contains(wanted), "expected '{wanted}' in: {err}");
        }
    }
}
