//! SWRL rule parsers: OWL/XML `DLSafeRule` and an ad-hoc text form. The other
//! syntaxes have their own modules: [`super::rdf`] (SWRL RDF syntax, any
//! serialisation), [`super::functional`] (OWL 2 functional-syntax
//! `DLSafeRule`), [`super::swrlapi`] (the SWRLAPI human-readable syntax) and
//! [`super::ruleml`] (the SWRL §4 RuleML XML syntax).
//!
//! The OWL/XML reader ([`parse_swrl`]) is a whitelist state machine. Inside a
//! `DLSafeRule` every element must be one it understands, in the place the
//! OWL 2 XML serialization (as written by the OWL API and Protégé) puts it, or
//! the whole document is refused with an error naming the element. A reader
//! that skips what it does not know drops atoms, and a rule missing one of its
//! body atoms fires on bindings its author excluded.
//!
//! Understood inside `Body` and `Head`:
//! - `ClassAtom` over a named `Class`
//! - `ObjectPropertyAtom` over an `ObjectProperty` or `ObjectInverseOf` (the
//!   arguments are swapped, so `ObjectInverseOf(p)(?x, ?y)` is `p(?y, ?x)`)
//! - `DataPropertyAtom`
//! - `SameIndividualAtom` / `DifferentIndividualsAtom`
//! - `BuiltInAtom` (the OWL API spelling) and `BuiltinAtom`
//!
//! Arguments are `Variable`, `NamedIndividual` and `Literal` (typed,
//! language-tagged or plain), each checked against its position: an
//! individual where a data value belongs, or a literal where an individual
//! belongs, is refused. IRIs are given as `IRI` (relative ones resolve against
//! `xml:base`) or `abbreviatedIRI` (expanded with the document's `Prefix`
//! declarations; `rdf:`, `rdfs:`, `xsd:`, `owl:`, `swrl:` and `swrlb:` are
//! predeclared). Refused with a message until later work supports them:
//! class-expression atoms (a `ClassAtom` over anything but a named class),
//! `DataRangeAtom` and anonymous individuals. Elements outside a `DLSafeRule`
//! (the rest of an ontology) are ignored, and so is a rule's `Annotation`;
//! an `Imp` element is pointed at the reader for its syntax.
//!
//! The ad-hoc text form ([`parse_swrl_text`], `A(?x) ^ B(?x,?y) -> C(?y)`) is a
//! convenience shorthand, not a second serialization of the same model. Two
//! limits follow from [`parse_single_atom`]: every predicate is taken verbatim
//! as an IRI — there are no prefix declarations — so it must be an absolute
//! IRI (`http://ex/Person(?x)`; bare names are rejected); and every
//! two-argument atom becomes a property atom, so `swrlb:` builtins cannot be
//! expressed in the text form at all. Use another syntax for any rule with a
//! builtin. Nor can the text form say whether a property is an object or a
//! data property: a two-argument atom whose second argument is a variable is
//! an untyped [`Atom::PropertyAtom`].
//!
//! Both forms refuse a class or property predicate that is not an IRI at parse
//! time ([`validate_predicate_iri`]): the predicate ends up inside the generated
//! SPARQL Update, and text is not a place for unchecked input.

use std::collections::HashMap;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use tracing::debug;

use super::engine::{validate_predicate_iri, Atom, SwrlArg, SwrlRule};
use super::names::Names;

/// `rdf:PlainLiteral`: OWL 2's plain literal, lexical form `text@lang`.
const RDF_PLAIN_LITERAL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral";

/// Parse SWRL rules from an OWL/XML document.
///
/// Strict: any element inside a `DLSafeRule` the reader does not understand,
/// in the place it occurs, refuses the whole document (see the module docs).
pub fn parse_swrl(xml: &str) -> Result<Vec<SwrlRule>, String> {
    let mut reader = Reader::from_str(xml);
    // Not trimmed: whitespace inside a `<Literal>` is part of its value.
    reader.config_mut().trim_text(false);

    let mut parser = OwlXmlRuleParser::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) => parser.open(e)?,
            Ok(Event::Empty(ref e)) => {
                parser.open(e)?;
                parser.close()?;
            }
            Ok(Event::End(_)) => parser.close()?,
            Ok(Event::Text(ref e)) => parser.text(&e.xml10_content())?,
            Ok(Event::CData(ref e)) => parser.text(&e.xml10_content())?,
            Ok(Event::GeneralRef(ref e)) => parser.text(&resolve_general_ref(e)?)?,
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
        buf.clear();
    }
    Ok(parser.rules)
}

/// What an argument position accepts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ArgSort {
    /// An individual: `Variable` or `NamedIndividual`.
    Individual,
    /// A data value: `Variable` or `Literal`.
    Data,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AtomKind {
    Class,
    ObjectProperty,
    DataProperty,
    SameIndividual,
    DifferentIndividuals,
    Builtin,
}

impl AtomKind {
    fn from_element(name: &str) -> Option<Self> {
        Some(match name {
            "ClassAtom" => AtomKind::Class,
            "ObjectPropertyAtom" => AtomKind::ObjectProperty,
            "DataPropertyAtom" => AtomKind::DataProperty,
            "SameIndividualAtom" => AtomKind::SameIndividual,
            "DifferentIndividualsAtom" => AtomKind::DifferentIndividuals,
            // The OWL API and Protégé write `BuiltInAtom`; `BuiltinAtom` is
            // accepted as well, as earlier releases of this reader read only it.
            "BuiltInAtom" | "BuiltinAtom" => AtomKind::Builtin,
            _ => return None,
        })
    }

    /// The sort of argument `index` (0-based), or `None` past the last one.
    fn arg_sort(self, index: usize) -> Option<ArgSort> {
        use ArgSort::*;
        let sorts: &[ArgSort] = match self {
            AtomKind::Class => &[Individual],
            AtomKind::ObjectProperty
            | AtomKind::SameIndividual
            | AtomKind::DifferentIndividuals => &[Individual, Individual],
            AtomKind::DataProperty => &[Individual, Data],
            // Built-in arguments are data values, as many as the built-in takes.
            AtomKind::Builtin => return Some(Data),
        };
        sorts.get(index).copied()
    }

    /// Whether the atom starts with a predicate element (class or property).
    fn has_predicate_element(self) -> bool {
        matches!(
            self,
            AtomKind::Class | AtomKind::ObjectProperty | AtomKind::DataProperty
        )
    }
}

struct AtomBuilder {
    kind: AtomKind,
    element: String,
    /// Class or property IRI (from a child element), or the built-in IRI.
    predicate: Option<String>,
    /// `ObjectInverseOf(p)`: the arguments are swapped when the atom is built.
    inverse: bool,
    args: Vec<SwrlArg>,
}

impl AtomBuilder {
    fn build(self) -> Result<Atom, String> {
        let el = &self.element;
        if self.kind == AtomKind::Builtin {
            if self.args.is_empty() {
                return Err(format!("{el} needs at least one argument"));
            }
            return Ok(Atom::BuiltinAtom {
                builtin: self
                    .predicate
                    .ok_or_else(|| format!("{el} missing IRI attribute"))?,
                args: self.args,
            });
        }
        let wanted = if self.kind == AtomKind::Class { 1 } else { 2 };
        if self.args.len() != wanted {
            return Err(format!(
                "{el} needs {wanted} argument(s), found {}",
                self.args.len()
            ));
        }
        let mut args = self.args.into_iter();
        let (a, b) = (args.next(), args.next());
        let a = a.expect("arity checked above");
        Ok(match self.kind {
            AtomKind::Class => {
                let class_iri = self.predicate.ok_or("ClassAtom missing class IRI")?;
                validate_predicate_iri(&class_iri, "class")?;
                Atom::ClassAtom { class_iri, arg: a }
            }
            AtomKind::ObjectProperty | AtomKind::DataProperty => {
                let property = self
                    .predicate
                    .ok_or_else(|| format!("{el} missing property IRI"))?;
                validate_predicate_iri(&property, "property")?;
                let b = b.expect("arity checked above");
                // ObjectInverseOf(p)(x, y) holds exactly when p(y, x) does.
                let (arg1, arg2) = if self.inverse { (b, a) } else { (a, b) };
                if self.kind == AtomKind::DataProperty {
                    Atom::DataPropertyAtom {
                        property,
                        arg1,
                        arg2,
                    }
                } else {
                    Atom::ObjectPropertyAtom {
                        property,
                        arg1,
                        arg2,
                    }
                }
            }
            AtomKind::SameIndividual => Atom::SameIndividualAtom {
                arg1: a,
                arg2: b.expect("arity checked above"),
            },
            AtomKind::DifferentIndividuals => Atom::DifferentIndividualsAtom {
                arg1: a,
                arg2: b.expect("arity checked above"),
            },
            AtomKind::Builtin => unreachable!("handled above"),
        })
    }
}

#[derive(Default)]
struct RuleBuilder {
    name: Option<String>,
    body: Option<Vec<Atom>>,
    head: Option<Vec<Atom>>,
}

impl RuleBuilder {
    fn build(self) -> Result<SwrlRule, String> {
        let body = self.body.ok_or("DLSafeRule has no Body")?;
        let head = self.head.ok_or("DLSafeRule has no Head")?;
        if head.is_empty() {
            return Err("DLSafeRule has an empty Head".to_string());
        }
        Ok(SwrlRule {
            name: self.name,
            body,
            head,
        })
    }
}

/// One open element. The stack of these is the parser's state.
enum Frame {
    /// An element outside any rule: the rest of the ontology.
    Outside,
    /// A subtree inside a rule that carries no rule content (`Annotation`).
    Skip,
    Rule(RuleBuilder),
    Atoms {
        head: bool,
    },
    Atom(AtomBuilder),
    InverseOf,
    Literal {
        value: String,
        datatype: Option<String>,
        language: Option<String>,
    },
    /// An element that may hold nothing (`Class`, `Variable`, …).
    Leaf(String),
}

#[derive(Default)]
struct OwlXmlRuleParser {
    stack: Vec<Frame>,
    rules: Vec<SwrlRule>,
    /// `Prefix` declarations and `xml:base`, for `abbreviatedIRI` and
    /// relative `IRI` attributes.
    names: Names,
}

impl OwlXmlRuleParser {
    fn open(&mut self, e: &BytesStart) -> Result<(), String> {
        let name = local_name(e.name().as_ref());
        let attrs = collect_attrs(e)?;
        if let Some(base) = attrs.get("xml:base") {
            self.names.set_base(base)?;
        }
        let names = &self.names;
        let frame = match self.stack.last_mut() {
            None | Some(Frame::Outside) => match name.as_str() {
                "DLSafeRule" => Frame::Rule(RuleBuilder {
                    name: attrs.get("IRI").map(|i| names.resolve(i)),
                    ..RuleBuilder::default()
                }),
                "Prefix" => {
                    let (Some(prefix), Some(iri)) = (attrs.get("name"), attrs.get("IRI")) else {
                        return Err("<Prefix> needs name and IRI attributes".to_string());
                    };
                    let iri = names.resolve(iri);
                    self.names.declare(prefix, &iri);
                    Frame::Outside
                }
                "Imp" => {
                    return Err(
                        "<Imp> is the SWRL RDF/XML syntax: send it with format \"rdf\" and \
                         rdf_format \"rdfxml\"; format \"xml\" reads OWL/XML DLSafeRule elements"
                            .to_string(),
                    )
                }
                "imp" => {
                    return Err(
                        "<imp> is the SWRL RuleML XML syntax: send it with format \"ruleml\"; \
                         format \"xml\" reads OWL/XML DLSafeRule elements"
                            .to_string(),
                    )
                }
                _ => Frame::Outside,
            },
            Some(Frame::Skip) => Frame::Skip,
            Some(Frame::Rule(rule)) => match name.as_str() {
                "Annotation" => Frame::Skip,
                "Body" | "Head" => {
                    let head = name == "Head";
                    let slot = if head { &mut rule.head } else { &mut rule.body };
                    if slot.is_some() {
                        return Err(format!("DLSafeRule has more than one <{name}>"));
                    }
                    *slot = Some(Vec::new());
                    Frame::Atoms { head }
                }
                _ => return Err(format!("Unexpected element <{name}> in DLSafeRule")),
            },
            Some(Frame::Atoms { head }) => {
                let place = if *head { "Head" } else { "Body" };
                match AtomKind::from_element(&name) {
                    Some(kind) => {
                        let predicate = if kind == AtomKind::Builtin {
                            Some(iri_attr(names, &attrs, &name)?)
                        } else {
                            None
                        };
                        Frame::Atom(AtomBuilder {
                            kind,
                            element: name,
                            predicate,
                            inverse: false,
                            args: Vec::new(),
                        })
                    }
                    None if name == "DataRangeAtom" => {
                        return Err(
                            "DataRangeAtom is not supported yet; refusing the document rather \
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
            Some(Frame::Atom(atom)) => open_in_atom(names, atom, &name, &attrs)?,
            Some(Frame::InverseOf) => {
                let Some(Frame::Atom(atom)) = self.stack.iter_mut().rev().nth(1) else {
                    unreachable!("ObjectInverseOf is only opened inside an atom");
                };
                match name.as_str() {
                    "ObjectProperty" if atom.predicate.is_none() => {
                        atom.predicate = Some(iri_attr(names, &attrs, &name)?);
                        Frame::Leaf(name)
                    }
                    _ => {
                        return Err(format!(
                            "ObjectInverseOf must hold exactly one ObjectProperty, found <{name}>"
                        ))
                    }
                }
            }
            Some(Frame::Literal { .. }) => {
                return Err(format!("Unexpected element <{name}> inside <Literal>"))
            }
            Some(Frame::Leaf(parent)) => {
                return Err(format!("Unexpected element <{name}> inside <{parent}>"))
            }
        };
        self.stack.push(frame);
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        match self.stack.pop() {
            Some(Frame::Rule(builder)) => {
                let rule = builder.build()?;
                debug!("Parsed SWRL rule: {:?}", rule.name);
                self.rules.push(rule);
            }
            Some(Frame::Atom(builder)) => {
                let element = builder.element.clone();
                let atom = builder
                    .build()
                    .map_err(|e| format!("Malformed SWRL {element}: {e}"))?;
                let head = match self.stack.last() {
                    Some(Frame::Atoms { head }) => *head,
                    _ => unreachable!("atoms are only opened inside Body or Head"),
                };
                let Some(Frame::Rule(rule)) = self.stack.iter_mut().rev().nth(1) else {
                    unreachable!("Body and Head are only opened inside a rule");
                };
                let slot = if head { &mut rule.head } else { &mut rule.body };
                slot.get_or_insert_with(Vec::new).push(atom);
            }
            Some(Frame::InverseOf) => {
                let Some(Frame::Atom(atom)) = self.stack.last() else {
                    unreachable!("ObjectInverseOf is only opened inside an atom");
                };
                if atom.predicate.is_none() {
                    return Err("ObjectInverseOf must hold exactly one ObjectProperty".to_string());
                }
            }
            Some(Frame::Literal {
                value,
                datatype,
                language,
            }) => {
                let Some(Frame::Atom(atom)) = self.stack.last_mut() else {
                    unreachable!("a Literal is only opened inside an atom");
                };
                atom.args.push(literal_arg(value, datatype, language));
            }
            Some(_) => {}
            None => return Err("XML parse error: unbalanced end tag".to_string()),
        }
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), String> {
        match self.stack.last_mut() {
            Some(Frame::Literal { value, .. }) => {
                value.push_str(text);
                Ok(())
            }
            None | Some(Frame::Outside) | Some(Frame::Skip) => Ok(()),
            Some(_) if text.trim().is_empty() => Ok(()),
            Some(_) => Err(format!(
                "Unexpected text '{}' inside a SWRL rule",
                text.trim()
            )),
        }
    }
}

/// The child elements an atom may hold: its predicate element first (class or
/// property), then its arguments, each checked against the sort its position
/// takes.
fn open_in_atom(
    names: &Names,
    atom: &mut AtomBuilder,
    name: &str,
    attrs: &HashMap<String, String>,
) -> Result<Frame, String> {
    let el = atom.element.clone();
    if atom.kind.has_predicate_element() && atom.predicate.is_none() {
        let expected = match atom.kind {
            AtomKind::Class => "Class",
            AtomKind::ObjectProperty => "ObjectProperty",
            _ => "DataProperty",
        };
        if name == expected {
            atom.predicate = Some(iri_attr(names, attrs, name)?);
            return Ok(Frame::Leaf(name.to_string()));
        }
        if atom.kind == AtomKind::ObjectProperty && name == "ObjectInverseOf" {
            atom.inverse = true;
            return Ok(Frame::InverseOf);
        }
        if atom.kind == AtomKind::Class && (name.starts_with("Object") || name.starts_with("Data"))
        {
            return Err(format!(
                "ClassAtom over the class expression <{name}> is not supported yet; only a \
                 named <Class> is. Refusing the document rather than reading the expression \
                 as a different condition"
            ));
        }
        return Err(format!("{el} must start with <{expected}>, found <{name}>"));
    }

    let index = atom.args.len();
    let sort = atom
        .kind
        .arg_sort(index)
        .ok_or_else(|| format!("{el} takes {index} argument(s); <{name}> would be one more"))?;
    let position = format!("argument {} of {el}", index + 1);
    match name {
        "Variable" => {
            // A variable is only an identity within its rule, so a prefixed
            // name whose prefix is not declared still names one.
            let var = match (attrs.get("IRI"), attrs.get("abbreviatedIRI")) {
                (Some(iri), _) if !iri.is_empty() => names.resolve(iri),
                (_, Some(short)) if !short.is_empty() => {
                    names.expand(short).unwrap_or_else(|_| short.clone())
                }
                _ => return Err("Variable without an IRI".to_string()),
            };
            atom.args.push(SwrlArg::Variable(var));
            Ok(Frame::Leaf(name.to_string()))
        }
        "NamedIndividual" if sort == ArgSort::Individual => {
            atom.args
                .push(SwrlArg::Individual(iri_attr(names, attrs, name)?));
            Ok(Frame::Leaf(name.to_string()))
        }
        "Literal" if sort == ArgSort::Data => Ok(Frame::Literal {
            value: String::new(),
            datatype: attrs.get("datatypeIRI").map(|d| names.resolve(d)),
            language: attrs.get("xml:lang").cloned(),
        }),
        "NamedIndividual" => Err(format!(
            "{position} takes a data value (Variable or Literal), found <NamedIndividual>"
        )),
        "Literal" => Err(format!(
            "{position} takes an individual (Variable or NamedIndividual), found <Literal>"
        )),
        "AnonymousIndividual" => Err(format!(
            "{position} is an AnonymousIndividual, which rules do not support"
        )),
        _ => Err(format!("Unexpected element <{name}> as {position}")),
    }
}

/// The IRI of a class, property, individual or built-in element: its `IRI`
/// attribute (relative IRIs resolve against `xml:base`) or its
/// `abbreviatedIRI` (expanded with the document's `Prefix` declarations).
fn iri_attr(
    names: &Names,
    attrs: &HashMap<String, String>,
    element: &str,
) -> Result<String, String> {
    if let Some(iri) = attrs.get("IRI") {
        return Ok(names.resolve(iri));
    }
    if let Some(short) = attrs.get("abbreviatedIRI") {
        return names
            .expand(short)
            .map_err(|e| format!("<{element} abbreviatedIRI=\"{short}\">: {e}"));
    }
    Err(format!("<{element}> without an IRI attribute"))
}

/// An OWL/XML `Literal`: typed, language-tagged, or plain. An
/// `rdf:PlainLiteral` carries its language after the last `@` of its text.
fn literal_arg(value: String, datatype: Option<String>, language: Option<String>) -> SwrlArg {
    match datatype.as_deref() {
        Some(RDF_PLAIN_LITERAL) => {
            let (text, lang) = value.rsplit_once('@').unwrap_or((&value, ""));
            SwrlArg::Literal {
                value: text.to_string(),
                datatype: None,
                language: (!lang.is_empty()).then(|| lang.to_string()),
            }
        }
        _ if language.is_some() => SwrlArg::Literal {
            value,
            datatype: None,
            language,
        },
        _ => SwrlArg::Literal {
            value,
            datatype,
            language: None,
        },
    }
}

/// Also parse from a simpler text-based rule format (non-XML):
/// `ClassName(?x) ^ propertyName(?x, ?y) -> ClassName2(?y)`
pub fn parse_swrl_text(input: &str) -> Result<Vec<SwrlRule>, String> {
    let mut rules = Vec::new();

    for (i, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.splitn(2, "->").collect();
        if parts.len() != 2 {
            return Err(format!("Line {}: expected 'body -> head' format", i + 1));
        }

        let body_atoms = parse_atom_list(parts[0].trim())?;
        let head_atoms = parse_atom_list(parts[1].trim())?;

        rules.push(SwrlRule {
            name: Some(format!("rule_{}", i + 1)),
            body: body_atoms,
            head: head_atoms,
        });
    }

    Ok(rules)
}

/// Parse a list of atoms separated by `^` (conjunction).
fn parse_atom_list(input: &str) -> Result<Vec<Atom>, String> {
    let mut atoms = Vec::new();
    for part in input.split('^') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        atoms.push(parse_single_atom(part)?);
    }
    Ok(atoms)
}

/// Parse a single atom: `Pred(?x, ?y)` or `Class(?x)`
fn parse_single_atom(input: &str) -> Result<Atom, String> {
    let paren_start = input
        .find('(')
        .ok_or_else(|| format!("Expected '(' in atom: {}", input))?;
    let paren_end = input
        .rfind(')')
        .ok_or_else(|| format!("Expected ')' in atom: {}", input))?;

    let predicate = input[..paren_start].trim();
    validate_predicate_iri(predicate, "predicate")?;
    let args_str = &input[paren_start + 1..paren_end];
    let args: Vec<SwrlArg> = args_str
        .split(',')
        .map(|a| {
            let a = a.trim();
            if a.starts_with('?') {
                SwrlArg::Variable(a.to_string())
            } else if a.starts_with('"') {
                SwrlArg::Literal {
                    value: a.trim_matches('"').to_string(),
                    datatype: None,
                    language: None,
                }
            } else {
                SwrlArg::Individual(a.to_string())
            }
        })
        .collect();

    match args.len() {
        1 => Ok(Atom::ClassAtom {
            class_iri: predicate.to_string(),
            arg: args[0].clone(),
        }),
        // The text form has no property declarations. A constant second
        // argument says which kind of property this is; a variable does not,
        // so the atom stays untyped and matches either kind of value.
        2 => {
            let property = predicate.to_string();
            let (arg1, arg2) = (args[0].clone(), args[1].clone());
            Ok(match arg2 {
                SwrlArg::Literal { .. } => Atom::DataPropertyAtom {
                    property,
                    arg1,
                    arg2,
                },
                SwrlArg::Individual(_) => Atom::ObjectPropertyAtom {
                    property,
                    arg1,
                    arg2,
                },
                SwrlArg::Variable(_) => Atom::PropertyAtom {
                    property,
                    arg1,
                    arg2,
                },
            })
        }
        _ => Err(format!("Unexpected number of arguments in atom: {}", input)),
    }
}

pub(super) fn local_name(name: &str) -> String {
    name.rsplit_once(':')
        .map(|(_, local)| local.to_string())
        .unwrap_or_else(|| name.to_string())
}

/// The text of a character or predefined entity reference (`&#38;`,
/// `&amp;`). Entities a DTD declares are not expanded: the document is
/// refused naming the entity.
pub(super) fn resolve_general_ref(e: &quick_xml::events::BytesRef) -> Result<String, String> {
    match e.resolve_char_ref() {
        Ok(Some(c)) => Ok(c.to_string()),
        Ok(None) => {
            let name = e.xml10_content();
            quick_xml::escape::resolve_predefined_entity(&name)
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "XML parse error: unknown entity &{name}; (DTD entities are not \
                         expanded; write the full IRI)"
                    )
                })
        }
        Err(err) => Err(format!("XML parse error: {err}")),
    }
}

/// The element's attributes. A malformed attribute fails the document rather
/// than vanishing: an `IRI` that silently went missing would change the rule.
pub(super) fn collect_attrs(e: &BytesStart) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|err| format!("XML parse error: {err}"))?;
        let key = attr.key.as_ref().to_string();
        let val = attr
            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
            .map_err(|err| format!("XML parse error in attribute {key}: {err}"))?;
        map.insert(key, val.to_string());
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_rules() {
        let input = r#"
# If ?x is a Person and ?x knows ?y, then ?y is a Person
http://ex/Person(?x) ^ http://ex/knows(?x, ?y) -> http://ex/Person(?y)

# If ?x is a Parent and ?x hasChild ?y, then ?y hasParent ?x
http://ex/Parent(?x) ^ http://ex/hasChild(?x, ?y) -> http://ex/hasParent(?y, ?x)
"#;
        let rules = parse_swrl_text(input).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].body.len(), 2);
        assert_eq!(rules[0].head.len(), 1);
    }

    #[test]
    fn test_parse_swrl_xml_basic() {
        let xml = r#"<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#">
    <DLSafeRule>
        <Body>
            <ClassAtom>
                <Class IRI="http://example.org/Person"/>
                <Variable IRI="urn:swrl:var#x"/>
            </ClassAtom>
        </Body>
        <Head>
            <ClassAtom>
                <Class IRI="http://example.org/Agent"/>
                <Variable IRI="urn:swrl:var#x"/>
            </ClassAtom>
        </Head>
    </DLSafeRule>
</Ontology>"#;
        let rules = parse_swrl(xml).unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].body.len(), 1);
        assert_eq!(rules[0].head.len(), 1);
    }

    #[test]
    fn test_parse_single_atom() {
        let atom = parse_single_atom("http://ex/Person(?x)").unwrap();
        assert!(matches!(atom, Atom::ClassAtom { .. }));

        let atom = parse_single_atom("http://ex/knows(?x, ?y)").unwrap();
        assert!(matches!(atom, Atom::PropertyAtom { .. }));
        let atom = parse_single_atom("http://ex/knows(?x, http://ex/b)").unwrap();
        assert!(matches!(atom, Atom::ObjectPropertyAtom { .. }));
        let atom = parse_single_atom("http://ex/name(?x, \"Bo\")").unwrap();
        assert!(matches!(atom, Atom::DataPropertyAtom { .. }));
    }

    /// Predicates are validated where they are read. A bare name is a relative
    /// IRI and is refused; so is anything carrying SPARQL syntax.
    #[test]
    fn text_form_rejects_predicates_that_are_not_iris() {
        let err = parse_single_atom("Person(?x)").unwrap_err();
        assert!(err.contains("Person"), "{err}");
        let err = parse_swrl_text(
            "http://ex/Person> } ; INSERT DATA { GRAPH <urn:probe> { <urn:s> <urn:p> <urn:o> } } ; INSERT { } WHERE { ?z a <http://ex/Q(?x) -> http://ex/T(?x)",
        )
        .unwrap_err();
        assert!(err.contains("Invalid SWRL predicate IRI"), "{err}");
    }

    /// The OWL/XML form: a `Class`/`ObjectProperty` IRI attribute that is not an
    /// IRI fails the document instead of being dropped from the rule.
    #[test]
    fn xml_form_rejects_predicates_that_are_not_iris() {
        let xml = r#"<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#">
    <DLSafeRule>
        <Body>
            <ObjectPropertyAtom>
                <ObjectProperty IRI="http://ex/p> ; DROP ALL ; INSERT { } WHERE { ?z &lt;http://ex/q"/>
                <Variable IRI="urn:swrl:var#x"/>
                <Variable IRI="urn:swrl:var#y"/>
            </ObjectPropertyAtom>
        </Body>
        <Head>
            <ClassAtom>
                <Class IRI="http://ex/Agent"/>
                <Variable IRI="urn:swrl:var#x"/>
            </ClassAtom>
        </Head>
    </DLSafeRule>
</Ontology>"#;
        let err = parse_swrl(xml).unwrap_err();
        assert!(err.contains("Invalid SWRL property IRI"), "{err}");
    }

    /// What the OWL API writes around a rule — the ontology's other axioms, a
    /// rule annotation, an XML-escaped literal — is read or skipped, and every
    /// literal form keeps its value, datatype or language.
    #[test]
    fn xml_form_reads_owl_api_output() {
        let xml = r#"<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://ex/o">
    <Declaration><Class IRI="http://ex/Person"/></Declaration>
    <DLSafeRule>
        <Annotation>
            <AnnotationProperty abbreviatedIRI="rdfs:label"/>
            <Literal>adults</Literal>
        </Annotation>
        <Body>
            <DataPropertyAtom>
                <DataProperty IRI="http://ex/name"/>
                <Variable IRI="urn:swrl#x"/>
                <Literal xml:lang="en"> A &amp; B </Literal>
            </DataPropertyAtom>
            <DataPropertyAtom>
                <DataProperty IRI="http://ex/code"/>
                <Variable IRI="urn:swrl#x"/>
                <Literal datatypeIRI="http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral">x@nl</Literal>
            </DataPropertyAtom>
            <DataPropertyAtom>
                <DataProperty IRI="http://ex/tag"/>
                <Variable IRI="urn:swrl#x"/>
                <Literal/>
            </DataPropertyAtom>
        </Body>
        <Head>
            <ClassAtom>
                <Class IRI="http://ex/Agent"/>
                <Variable IRI="urn:swrl#x"/>
            </ClassAtom>
        </Head>
    </DLSafeRule>
</Ontology>"#;
        let rules = parse_swrl(xml).unwrap();
        assert_eq!(rules.len(), 1);
        let values: Vec<_> = rules[0]
            .body
            .iter()
            .map(|a| match a {
                Atom::DataPropertyAtom {
                    arg2:
                        SwrlArg::Literal {
                            value,
                            datatype,
                            language,
                        },
                    ..
                } => (value.clone(), datatype.clone(), language.clone()),
                other => panic!("unexpected atom {other:?}"),
            })
            .collect();
        assert_eq!(
            values,
            vec![
                (" A & B ".to_string(), None, Some("en".to_string())),
                ("x".to_string(), None, Some("nl".to_string())),
                (String::new(), None, None),
            ]
        );
    }

    /// Strictness at the edges: an extra argument, a missing Head, the RDF/XML
    /// syntax, a prefixed class name and text where none belongs all refuse
    /// the document.
    #[test]
    fn xml_form_refuses_what_it_cannot_read() {
        let rule = |body: &str, head: &str| {
            format!(
                r#"<Ontology xmlns="http://www.w3.org/2002/07/owl#"><DLSafeRule>{body}{head}</DLSafeRule></Ontology>"#
            )
        };
        let head = r#"<Head><ClassAtom><Class IRI="http://ex/B"/><Variable IRI="urn:v#x"/></ClassAtom></Head>"#;
        let cases = [
            (
                rule(
                    r#"<Body><ClassAtom><Class IRI="http://ex/A"/><Variable IRI="urn:v#x"/><Variable IRI="urn:v#y"/></ClassAtom></Body>"#,
                    head,
                ),
                "one more",
            ),
            (
                rule(r#"<Body/>"#, ""),
                "no Head",
            ),
            (
                rule(
                    r#"<Body><ClassAtom><Class abbreviatedIRI="ex:A"/><Variable IRI="urn:v#x"/></ClassAtom></Body>"#,
                    head,
                ),
                "abbreviatedIRI",
            ),
            (
                rule(
                    r#"<Body><ClassAtom>stray<Class IRI="http://ex/A"/><Variable IRI="urn:v#x"/></ClassAtom></Body>"#,
                    head,
                ),
                "Unexpected text",
            ),
            (
                r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:swrl="http://www.w3.org/2003/11/swrl#"><swrl:Imp/></rdf:RDF>"#.to_string(),
                "RDF/XML",
            ),
        ];
        for (xml, wanted) in cases {
            let err = parse_swrl(&xml).expect_err(&xml);
            assert!(err.contains(wanted), "expected '{wanted}' in: {err}");
        }
    }
}
