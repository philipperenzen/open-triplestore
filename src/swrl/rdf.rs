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
//! As strict as the OWL/XML reader: an atom of an unknown type, a missing or
//! repeated required property, a malformed list, a class expression (a blank
//! node as `swrl:classPredicate`), a `swrl:DataRangeAtom` or an anonymous
//! individual refuses the rule set with a message naming the rule.
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

const SWRL: &str = "http://www.w3.org/2003/11/swrl#";
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
            Term::BlankNode(_) if local == "classPredicate" => Err(
                "ClassAtom over a class expression (a blank node) is not supported yet; \
                 only a named class is"
                    .to_string(),
            ),
            other => Err(format!(
                "swrl:{local} of {kind} must be an IRI, found {other}"
            )),
        }
    };
    Ok(match kind {
        "ClassAtom" => Atom::ClassAtom {
            class_iri: predicate("classPredicate")?,
            arg: arg("argument1")?,
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
        "DataRangeAtom" => {
            return Err(
                "DataRangeAtom is not supported yet; refusing the rule rather than running \
                 it without that condition"
                    .to_string(),
            )
        }
        other => return Err(format!("unknown SWRL atom type swrl:{other}")),
    })
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
                "DataRangeAtom",
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
