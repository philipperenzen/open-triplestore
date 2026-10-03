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
//! prefixes vary. A class description instead of a named `owlx:Class`, a
//! `dataRangeAtom`, an unknown element, or a DTD entity (`&swrlb;`) refuses
//! the document with a message naming it.

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::engine::{Atom, SwrlArg, SwrlRule};
use super::names::Names;
use super::parser::{collect_attrs, local_name, resolve_general_ref};

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
            _ => return None,
        })
    }
}

struct AtomState {
    kind: Kind,
    element: String,
    predicate: Option<String>,
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
                            args: Vec::new(),
                        })
                    }
                    None if name == "dataRangeAtom" => {
                        return Err(
                            "dataRangeAtom is not supported yet; refusing the document rather \
                             than running its rule without that condition"
                                .to_string(),
                        )
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
                if atom.kind == Kind::Class && atom.predicate.is_none() {
                    if name != "Class" {
                        return Err(format!(
                            "classAtom over the class description <{name}> is not supported \
                             yet; only a named owlx:Class is"
                        ));
                    }
                    atom.predicate = Some(named("name", &name)?);
                    Frame::Leaf(name)
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
            None | Some(Frame::Outside) => Ok(()),
            Some(_) if text.trim().is_empty() => Ok(()),
            Some(_) => Err(format!("Unexpected text '{}' inside a rule", text.trim())),
        }
    }
}

fn build(atom: AtomState) -> Result<Atom, String> {
    let wanted = match atom.kind {
        Kind::Class => 1,
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
                    r#"<swrlx:classAtom><owlx:IntersectionOf/><ruleml:var>x</ruleml:var></swrlx:classAtom>"#,
                ),
                "class description",
            ),
            (wrap(r#"<swrlx:dataRangeAtom/>"#), "dataRangeAtom"),
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
