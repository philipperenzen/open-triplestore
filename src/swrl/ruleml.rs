//! The SWRL XML concrete syntax of the SWRL submission §4: RuleML `imp`
//! rules with `swrlx:` atoms.
//!
//! ```xml
//! <ruleml:imp>
//!   <ruleml:_rlab ruleml:href="#uncle"/>
//!   <ruleml:_body>
//!     <swrlx:individualPropertyAtom swrlx:property="http://ex/hasParent">
//!       <ruleml:var>x1</ruleml:var> <ruleml:var>x2</ruleml:var>
//!     </swrlx:individualPropertyAtom>
//!   </ruleml:_body>
//!   <ruleml:_head> … </ruleml:_head>
//! </ruleml:imp>
//! ```
//!
//! Atoms: `classAtom` (an `owlx:Class` then one argument),
//! `individualPropertyAtom` and `datavaluedPropertyAtom` (`swrlx:property`),
//! `sameIndividualAtom`, `differentIndividualsAtom`, and `builtinAtom`
//! (`swrlx:builtin`). Arguments: `ruleml:var`, `owlx:Individual owlx:name`,
//! `owlx:DataValue owlx:datatype`. Names resolve against `xml:base` when they
//! are relative. Elements are matched by local name, as the namespace
//! prefixes vary.
//!
//! A `classAtom` may hold any class description of the OWL XML presentation
//! syntax (`owlx:IntersectionOf`, `owlx:UnionOf`, `owlx:ComplementOf`,
//! `owlx:OneOf`, `owlx:ObjectRestriction` and `owlx:DataRestriction` with
//! `someValuesFrom`, `allValuesFrom`, `hasValue`, `cardinality`,
//! `minCardinality`, `maxCardinality`), and a `datarangeAtom` an
//! `owlx:Datatype` or an `owlx:OneOf` of data values. An unknown element, or a
//! DTD entity (`&swrlb;`), refuses the document with a message naming it.

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::engine::{constant_term, Atom, SwrlArg, SwrlRule};
use super::expr::{CardKind, ClassExpr, DataRange, PropExpr};
use super::names::Names;
use super::parser::{collect_attrs, local_name, resolve_general_ref, XmlNode};

/// Parse SWRL §4 RuleML XML rules.
pub fn parse_swrl_ruleml(xml: &str) -> Result<Vec<SwrlRule>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut p = RuleMlParser::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) => p.open(e)?,
            Ok(Event::Empty(ref e)) => {
                p.open(e)?;
                p.close()?;
            }
            Ok(Event::End(_)) => p.close()?,
            Ok(Event::Text(ref e)) => p.text(&e.xml10_content())?,
            Ok(Event::CData(ref e)) => p.text(&e.xml10_content())?,
            Ok(Event::GeneralRef(ref e)) => p.text(&resolve_general_ref(e)?)?,
            Err(e) => return Err(format!("XML parse error: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(p.rules)
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Class,
    IndividualProperty,
    DatavaluedProperty,
    SameIndividual,
    DifferentIndividuals,
    Builtin,
    DataRange,
}

impl Kind {
    fn from_element(name: &str) -> Option<Kind> {
        Some(match name {
            "classAtom" => Kind::Class,
            "individualPropertyAtom" => Kind::IndividualProperty,
            "datavaluedPropertyAtom" => Kind::DatavaluedProperty,
            "sameIndividualAtom" => Kind::SameIndividual,
            "differentIndividualsAtom" => Kind::DifferentIndividuals,
            "builtinAtom" => Kind::Builtin,
            "datarangeAtom" | "dataRangeAtom" => Kind::DataRange,
            _ => return None,
        })
    }
}

struct AtomState {
    kind: Kind,
    element: String,
    predicate: Option<String>,
    /// A class description other than a named class.
    expr: Option<ClassExpr>,
    /// A `datarangeAtom`'s range.
    range: Option<DataRange>,
    args: Vec<SwrlArg>,
}

enum Frame {
    Outside,
    Rule {
        name: Option<String>,
        body: Option<Vec<Atom>>,
        head: Option<Vec<Atom>>,
    },
    Atoms {
        head: bool,
    },
    Atom(AtomState),
    /// `ruleml:var`, collecting its name.
    Var(String),
    /// `owlx:DataValue`, collecting its lexical form.
    DataValue {
        value: String,
        datatype: Option<String>,
    },
    Leaf(String),
    /// A class description or data range, read whole.
    Tree(XmlNode),
}

#[derive(Default)]
struct RuleMlParser {
    stack: Vec<Frame>,
    rules: Vec<SwrlRule>,
    names: Names,
}

/// An attribute by its local name (`swrlx:property`, `owlx:name`, …).
fn attr<'a>(attrs: &'a HashMap<String, String>, local: &str) -> Option<&'a String> {
    attrs
        .iter()
        .find(|(k, _)| local_name(k) == local)
        .map(|(_, v)| v)
}

impl RuleMlParser {
    fn open(&mut self, e: &BytesStart) -> Result<(), String> {
        let name = local_name(e.name().as_ref());
        let attrs = collect_attrs(e)?;
        if let Some(base) = attrs.get("xml:base") {
            self.names.set_base(base)?;
        }
        let names = &self.names;
        let named = |what: &str, element: &str| -> Result<String, String> {
            attr(&attrs, what)
                .map(|v| names.resolve(v))
                .ok_or_else(|| format!("<{element}> needs a {what} attribute"))
        };
        let frame = match self.stack.last_mut() {
            None | Some(Frame::Outside) => match name.as_str() {
                "imp" => Frame::Rule {
                    name: None,
                    body: None,
                    head: None,
                },
                "Imp" => {
                    return Err("<Imp> is the SWRL RDF/XML syntax: send it with format \
                                \"rdf\" and rdf_format \"rdfxml\""
                        .to_string())
                }
                "DLSafeRule" => {
                    return Err("<DLSafeRule> is OWL/XML: send it with format \"xml\"".to_string())
                }
                _ => Frame::Outside,
            },
            Some(Frame::Rule {
                name: rule_name,
                body,
                head,
            }) => match name.as_str() {
                "_rlab" => {
                    *rule_name = attr(&attrs, "href").map(|h| names.resolve(h));
                    Frame::Leaf(name)
                }
                "_body" | "_head" => {
                    let is_head = name == "_head";
                    let slot = if is_head { head } else { body };
                    if slot.is_some() {
                        return Err(format!("ruleml:imp has more than one <{name}>"));
                    }
                    *slot = Some(Vec::new());
                    Frame::Atoms { head: is_head }
                }
                _ => return Err(format!("Unexpected element <{name}> in ruleml:imp")),
            },
            Some(Frame::Atoms { head }) => {
                let place = if *head { "_head" } else { "_body" };
                match Kind::from_element(&name) {
                    Some(kind) => {
                        let predicate = match kind {
                            Kind::IndividualProperty | Kind::DatavaluedProperty => {
                                Some(named("property", &name)?)
                            }
                            Kind::Builtin => Some(named("builtin", &name)?),
                            _ => None,
                        };
                        Frame::Atom(AtomState {
                            kind,
                            element: name,
                            predicate,
                            expr: None,
                            range: None,
                            args: Vec::new(),
                        })
                    }
                    None => {
                        return Err(format!(
                            "Unknown SWRL atom <{name}> in {place}; refusing the document \
                             rather than running its rule without that atom"
                        ))
                    }
                }
            }
            Some(Frame::Atom(atom)) => {
                let slot_open = match atom.kind {
                    Kind::Class => atom.predicate.is_none() && atom.expr.is_none(),
                    Kind::DataRange => atom.range.is_none(),
                    _ => false,
                };
                if slot_open && atom.args.is_empty() && name == "Class" {
                    atom.predicate = Some(named("name", &name)?);
                    Frame::Leaf(name)
                } else if slot_open && atom.args.is_empty() {
                    Frame::Tree(XmlNode {
                        name: name.clone(),
                        attrs: attrs.clone(),
                        ..XmlNode::default()
                    })
                } else {
                    match name.as_str() {
                        "var" => Frame::Var(String::new()),
                        "Individual" => {
                            atom.args.push(SwrlArg::Individual(named("name", &name)?));
                            Frame::Leaf(name)
                        }
                        "DataValue" => Frame::DataValue {
                            value: String::new(),
                            datatype: attr(&attrs, "datatype").map(|d| names.resolve(d)),
                        },
                        _ => {
                            return Err(format!(
                                "Unexpected element <{name}> as an argument of <{}>",
                                atom.element
                            ))
                        }
                    }
                }
            }
            Some(Frame::Leaf(parent)) => {
                return Err(format!("Unexpected element <{name}> inside <{parent}>"))
            }
            Some(Frame::Var(_)) | Some(Frame::DataValue { .. }) => {
                return Err(format!("Unexpected element <{name}> inside a value"))
            }
            Some(Frame::Tree(_)) => Frame::Tree(XmlNode {
                name: name.clone(),
                attrs: attrs.clone(),
                ..XmlNode::default()
            }),
        };
        self.stack.push(frame);
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        match self.stack.pop() {
            Some(Frame::Rule { name, body, head }) => {
                let head = head.ok_or("ruleml:imp has no _head")?;
                if head.is_empty() {
                    return Err("ruleml:imp has an empty _head".to_string());
                }
                self.rules.push(SwrlRule {
                    name,
                    body: body.unwrap_or_default(),
                    head,
                });
            }
            Some(Frame::Atom(atom)) => {
                let element = atom.element.clone();
                let built = build(atom).map_err(|e| format!("Malformed <{element}>: {e}"))?;
                let is_head = matches!(self.stack.last(), Some(Frame::Atoms { head: true }));
                let Some(Frame::Rule { body, head, .. }) = self.stack.iter_mut().rev().nth(1)
                else {
                    unreachable!("atoms are only opened inside _body or _head of a rule");
                };
                let slot = if is_head { head } else { body };
                slot.get_or_insert_with(Vec::new).push(built);
            }
            Some(Frame::Var(name)) => {
                let name = name.trim();
                if name.is_empty() {
                    return Err("ruleml:var without a name".to_string());
                }
                self.push_arg(SwrlArg::Variable(format!("?{name}")))?;
            }
            Some(Frame::Tree(node)) => match self.stack.last_mut() {
                Some(Frame::Tree(parent)) => parent.children.push(node),
                Some(Frame::Atom(atom)) => {
                    let names = &self.names;
                    if atom.kind == Kind::DataRange {
                        atom.range = Some(owlx_range(names, &node)?);
                    } else {
                        atom.expr = Some(owlx_class(names, &node)?);
                    }
                }
                _ => unreachable!("a description is only opened inside an atom"),
            },
            Some(Frame::DataValue { value, datatype }) => {
                self.push_arg(SwrlArg::Literal {
                    value,
                    datatype,
                    language: None,
                })?;
            }
            Some(_) => {}
            None => return Err("XML parse error: unbalanced end tag".to_string()),
        }
        Ok(())
    }

    fn push_arg(&mut self, arg: SwrlArg) -> Result<(), String> {
        match self.stack.last_mut() {
            Some(Frame::Atom(atom)) => {
                atom.args.push(arg);
                Ok(())
            }
            _ => Err("an argument outside an atom".to_string()),
        }
    }

    fn text(&mut self, text: &str) -> Result<(), String> {
        match self.stack.last_mut() {
            Some(Frame::Var(name)) => {
                name.push_str(text);
                Ok(())
            }
            Some(Frame::DataValue { value, .. }) => {
                value.push_str(text);
                Ok(())
            }
            Some(Frame::Tree(node)) => {
                node.text.push_str(text);
                Ok(())
            }
            None | Some(Frame::Outside) => Ok(()),
            Some(_) if text.trim().is_empty() => Ok(()),
            Some(_) => Err(format!("Unexpected text '{}' inside a rule", text.trim())),
        }
    }
}

fn build(atom: AtomState) -> Result<Atom, String> {
    let wanted = match atom.kind {
        Kind::Class | Kind::DataRange => 1,
        Kind::Builtin => atom.args.len().max(1),
        _ => 2,
    };
    if atom.args.len() != wanted {
        return Err(format!(
            "needs {wanted} argument(s), found {}",
            atom.args.len()
        ));
    }
    let mut args = atom.args.into_iter();
    let mut next = move || args.next();
    Ok(match atom.kind {
        Kind::Class if atom.expr.is_some() => Atom::ClassExpressionAtom {
            expr: atom.expr.expect("checked"),
            arg: next().expect("arity checked"),
        },
        Kind::DataRange => Atom::DataRangeAtom {
            range: atom.range.ok_or("missing its data range")?,
            arg: next().expect("arity checked"),
        },
        Kind::Class => Atom::ClassAtom {
            class_iri: atom.predicate.ok_or("missing owlx:Class")?,
            arg: next().expect("arity checked"),
        },
        Kind::IndividualProperty => Atom::ObjectPropertyAtom {
            property: atom.predicate.expect("read at open"),
            arg1: next().expect("arity checked"),
            arg2: next().expect("arity checked"),
        },
        Kind::DatavaluedProperty => Atom::DataPropertyAtom {
            property: atom.predicate.expect("read at open"),
            arg1: next().expect("arity checked"),
            arg2: next().expect("arity checked"),
        },
        Kind::SameIndividual => Atom::SameIndividualAtom {
            arg1: next().expect("arity checked"),
            arg2: next().expect("arity checked"),
        },
        Kind::DifferentIndividuals => Atom::DifferentIndividualsAtom {
            arg1: next().expect("arity checked"),
            arg2: next().expect("arity checked"),
        },
        Kind::Builtin => {
            let mut all = Vec::new();
            while let Some(a) = next() {
                all.push(a);
            }
            Atom::BuiltinAtom {
                builtin: atom.predicate.expect("read at open"),
                args: all,
            }
        }
    })
}

/// An attribute of a tree node, by local name, resolved as a name.
fn node_name(names: &Names, node: &XmlNode, what: &str) -> Result<String, String> {
    attr(&node.attrs, what)
        .map(|v| names.resolve(v))
        .ok_or_else(|| format!("<{}> needs a {what} attribute", node.name))
}

fn data_value(names: &Names, node: &XmlNode) -> Result<oxigraph::model::Literal, String> {
    if node.name != "DataValue" {
        return Err(format!("expected <owlx:DataValue>, found <{}>", node.name));
    }
    let arg = SwrlArg::Literal {
        value: node.text.clone(),
        datatype: attr(&node.attrs, "datatype").map(|d| names.resolve(d)),
        language: None,
    };
    match constant_term(&arg)? {
        oxigraph::model::Term::Literal(l) => Ok(l),
        _ => unreachable!("a literal argument is a literal"),
    }
}

fn only_child(node: &XmlNode) -> Result<&XmlNode, String> {
    match node.children.as_slice() {
        [one] => Ok(one),
        other => Err(format!(
            "<{}> takes one element, found {}",
            node.name,
            other.len()
        )),
    }
}

fn restriction_count(node: &XmlNode) -> Result<u64, String> {
    attr(&node.attrs, "value")
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| format!("<{}> needs a non-negative owlx:value", node.name))
}

/// An OWL XML presentation syntax class description.
fn owlx_class(names: &Names, node: &XmlNode) -> Result<ClassExpr, String> {
    let all = |n: &XmlNode| -> Result<Vec<ClassExpr>, String> {
        n.children.iter().map(|c| owlx_class(names, c)).collect()
    };
    Ok(match node.name.as_str() {
        "Class" => ClassExpr::Named(node_name(names, node, "name")?),
        "IntersectionOf" => ClassExpr::IntersectionOf(all(node)?),
        "UnionOf" => ClassExpr::UnionOf(all(node)?),
        "ComplementOf" => ClassExpr::ComplementOf(Box::new(owlx_class(names, only_child(node)?)?)),
        "OneOf" => ClassExpr::OneOf(
            node.children
                .iter()
                .map(|c| {
                    if c.name == "Individual" {
                        node_name(names, c, "name")
                    } else {
                        Err(format!("expected <owlx:Individual>, found <{}>", c.name))
                    }
                })
                .collect::<Result<_, _>>()?,
        ),
        "ObjectRestriction" => {
            let p = node_name(names, node, "property")?;
            let r = only_child(node)?;
            match r.name.as_str() {
                "someValuesFrom" => ClassExpr::SomeValuesFrom(
                    PropExpr::Named(p),
                    Box::new(owlx_class(names, only_child(r)?)?),
                ),
                "allValuesFrom" => ClassExpr::AllValuesFrom(
                    PropExpr::Named(p),
                    Box::new(owlx_class(names, only_child(r)?)?),
                ),
                "hasValue" => {
                    let i = only_child(r)?;
                    ClassExpr::HasValue(PropExpr::Named(p), node_name(names, i, "name")?)
                }
                k @ ("cardinality" | "minCardinality" | "maxCardinality") => {
                    ClassExpr::Cardinality {
                        kind: card(k),
                        n: restriction_count(r)?,
                        prop: PropExpr::Named(p),
                        filler: None,
                    }
                }
                other => {
                    return Err(format!(
                        "Unknown restriction <{other}> in an ObjectRestriction"
                    ))
                }
            }
        }
        "DataRestriction" => {
            let p = node_name(names, node, "property")?;
            let r = only_child(node)?;
            match r.name.as_str() {
                "someValuesFrom" => {
                    ClassExpr::DataSomeValuesFrom(p, owlx_range(names, only_child(r)?)?)
                }
                "allValuesFrom" => {
                    ClassExpr::DataAllValuesFrom(p, owlx_range(names, only_child(r)?)?)
                }
                "hasValue" => ClassExpr::DataHasValue(p, data_value(names, only_child(r)?)?),
                k @ ("cardinality" | "minCardinality" | "maxCardinality") => {
                    ClassExpr::DataCardinality {
                        kind: card(k),
                        n: restriction_count(r)?,
                        prop: p,
                        range: None,
                    }
                }
                other => {
                    return Err(format!(
                        "Unknown restriction <{other}> in a DataRestriction"
                    ))
                }
            }
        }
        other => {
            return Err(format!(
                "Unknown class description <{other}> in a classAtom; refusing the document \
                 rather than reading it as a different condition"
            ))
        }
    })
}

fn card(k: &str) -> CardKind {
    match k {
        "minCardinality" => CardKind::Min,
        "maxCardinality" => CardKind::Max,
        _ => CardKind::Exact,
    }
}

/// An `owlx:Datatype` or an `owlx:OneOf` of data values.
fn owlx_range(names: &Names, node: &XmlNode) -> Result<DataRange, String> {
    Ok(match node.name.as_str() {
        "Datatype" => DataRange::Datatype(node_name(names, node, "name")?),
        "OneOf" => DataRange::OneOf(
            node.children
                .iter()
                .map(|c| data_value(names, c))
                .collect::<Result<_, _>>()?,
        ),
        other => {
            return Err(format!(
                "Unknown data range <{other}> in a datarangeAtom; refusing the document"
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r##"<?xml version="1.0"?>
<swrlx:Ontology xmlns:swrlx="http://www.w3.org/2003/11/swrlx#"
                xmlns:owlx="http://www.w3.org/2003/05/owl-xml"
                xmlns:ruleml="http://www.w3.org/2003/11/ruleml"
                xml:base="http://ex/family">
  <ruleml:imp>
    <ruleml:_rlab ruleml:href="#uncle"/>
    <ruleml:_body>
      <swrlx:individualPropertyAtom swrlx:property="#hasParent">
        <ruleml:var>x1</ruleml:var>
        <ruleml:var>x2</ruleml:var>
      </swrlx:individualPropertyAtom>
      <swrlx:individualPropertyAtom swrlx:property="#hasBrother">
        <ruleml:var>x2</ruleml:var>
        <ruleml:var>x3</ruleml:var>
      </swrlx:individualPropertyAtom>
      <swrlx:datavaluedPropertyAtom swrlx:property="#age">
        <ruleml:var>x1</ruleml:var>
        <owlx:DataValue owlx:datatype="http://www.w3.org/2001/XMLSchema#int">5</owlx:DataValue>
      </swrlx:datavaluedPropertyAtom>
    </ruleml:_body>
    <ruleml:_head>
      <swrlx:individualPropertyAtom swrlx:property="#hasUncle">
        <ruleml:var>x1</ruleml:var>
        <ruleml:var>x3</ruleml:var>
      </swrlx:individualPropertyAtom>
    </ruleml:_head>
  </ruleml:imp>
</swrlx:Ontology>"##;

    #[test]
    fn reads_the_submission_example() {
        let rules = parse_swrl_ruleml(DOC).unwrap();
        assert_eq!(rules.len(), 1);
        let r = &rules[0];
        assert_eq!(r.name.as_deref(), Some("http://ex/family#uncle"));
        assert_eq!(r.body.len(), 3);
        assert!(
            matches!(&r.body[0], Atom::ObjectPropertyAtom { property, .. }
            if property == "http://ex/family#hasParent")
        );
        assert!(matches!(&r.body[2], Atom::DataPropertyAtom {
            arg2: SwrlArg::Literal { value, .. }, ..
        } if value == "5"));
    }

    #[test]
    fn reads_class_descriptions_and_data_ranges() {
        let doc = r#"<r xmlns:swrlx="s" xmlns:ruleml="r" xmlns:owlx="o"><ruleml:imp><ruleml:_body>
            <swrlx:classAtom>
              <owlx:ObjectRestriction owlx:property="http://ex/knows">
                <owlx:someValuesFrom><owlx:Class owlx:name="http://ex/Person"/></owlx:someValuesFrom>
              </owlx:ObjectRestriction>
              <ruleml:var>x</ruleml:var>
            </swrlx:classAtom>
            <swrlx:datavaluedPropertyAtom swrlx:property="http://ex/age">
              <ruleml:var>x</ruleml:var><ruleml:var>a</ruleml:var>
            </swrlx:datavaluedPropertyAtom>
            <swrlx:datarangeAtom>
              <owlx:Datatype owlx:name="http://www.w3.org/2001/XMLSchema#integer"/>
              <ruleml:var>a</ruleml:var>
            </swrlx:datarangeAtom>
          </ruleml:_body>
          <ruleml:_head><swrlx:classAtom><owlx:Class owlx:name="http://ex/B"/><ruleml:var>x</ruleml:var></swrlx:classAtom></ruleml:_head></ruleml:imp></r>"#;
        let rules = parse_swrl_ruleml(doc).unwrap();
        let body = &rules[0].body;
        assert!(matches!(
            &body[0],
            Atom::ClassExpressionAtom {
                expr: ClassExpr::SomeValuesFrom(..),
                ..
            }
        ));
        assert!(matches!(
            &body[2],
            Atom::DataRangeAtom {
                range: DataRange::Datatype(_),
                ..
            }
        ));
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let wrap = |body: &str| {
            format!(
                r#"<r xmlns:swrlx="s" xmlns:ruleml="r" xmlns:owlx="o"><ruleml:imp><ruleml:_body>{body}</ruleml:_body>
                <ruleml:_head><swrlx:classAtom><owlx:Class owlx:name="http://ex/B"/><ruleml:var>x</ruleml:var></swrlx:classAtom></ruleml:_head></ruleml:imp></r>"#
            )
        };
        let cases = [
            (
                wrap(
                    r#"<swrlx:classAtom><owlx:FancyOf/><ruleml:var>x</ruleml:var></swrlx:classAtom>"#,
                ),
                "class description",
            ),
            (wrap(r#"<swrlx:datarangeAtom/>"#), "needs 1 argument"),
            (wrap(r#"<swrlx:fancyAtom/>"#), "Unknown SWRL atom"),
            (
                wrap(
                    r#"<swrlx:builtinAtom swrlx:builtin="&swrlb;equal"><ruleml:var>x</ruleml:var></swrlx:builtinAtom>"#,
                ),
                "XML parse error",
            ),
            (
                wrap(
                    r#"<swrlx:classAtom><owlx:Class owlx:name="http://ex/A"/><ruleml:var>x</ruleml:var><ruleml:var>y</ruleml:var></swrlx:classAtom>"#,
                ),
                "needs 1 argument",
            ),
        ];
        for (doc, wanted) in cases {
            let err = parse_swrl_ruleml(&doc).unwrap_err();
            assert!(err.contains(wanted), "expected '{wanted}' in: {err}");
        }
    }
}
