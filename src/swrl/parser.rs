//! SWRL XML/OWL parser using quick-xml.
//!
//! Parses SWRL rules embedded in OWL ontologies (XML serialization).
//! Supports:
//! - ClassAtom
//! - ObjectPropertyAtom / DataPropertyAtom
//! - SameIndividualAtom / DifferentIndividualsAtom
//! - BuiltinAtom (math, string, comparison)
//! - Variables and individual references

use quick_xml::events::Event;
use quick_xml::Reader;
use tracing::{debug, warn};

use super::engine::{Atom, SwrlArg, SwrlRule};

/// Parse SWRL rules from an OWL/XML document.
pub fn parse_swrl(xml: &str) -> Result<Vec<SwrlRule>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut rules = Vec::new();
    let mut buf = Vec::new();
    let mut current_rule: Option<RuleBuilder> = None;
    let mut in_body = false;
    let mut in_head = false;
    let mut current_atom: Option<AtomBuilder> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let local_name = local_name(e.name().as_ref());
                let attrs = collect_attrs(&reader, e);

                match local_name.as_str() {
                    "Imp" | "DLSafeRule" | "Rule" => {
                        current_rule = Some(RuleBuilder::new());
                        if let Some(name) = attrs.get("rdf:about").or(attrs.get("IRI")) {
                            if let Some(ref mut r) = current_rule {
                                r.name = Some(name.clone());
                            }
                        }
                    }
                    "body" | "Body" => in_body = true,
                    "head" | "Head" => in_head = true,

                    // Atom types
                    "ClassAtom" => {
                        current_atom = Some(AtomBuilder::Class {
                            class_iri: None,
                            arg: None,
                        });
                    }
                    "ObjectPropertyAtom" | "DataPropertyAtom" => {
                        let is_data = local_name == "DataPropertyAtom";
                        current_atom = Some(AtomBuilder::Property {
                            property: None,
                            arg1: None,
                            arg2: None,
                            is_data,
                        });
                    }
                    "SameIndividualAtom" => {
                        current_atom = Some(AtomBuilder::SameIndividual {
                            arg1: None,
                            arg2: None,
                        });
                    }
                    "DifferentIndividualsAtom" => {
                        current_atom = Some(AtomBuilder::DifferentIndividuals {
                            arg1: None,
                            arg2: None,
                        });
                    }
                    "BuiltinAtom" => {
                        let builtin = attrs
                            .get("IRI")
                            .or(attrs.get("rdf:resource"))
                            .cloned()
                            .unwrap_or_default();
                        current_atom = Some(AtomBuilder::Builtin {
                            builtin,
                            args: Vec::new(),
                        });
                    }

                    // Atom components
                    "Class" | "classPredicate" => {
                        if let Some(iri) = attrs.get("IRI").or(attrs.get("rdf:resource")) {
                            if let Some(AtomBuilder::Class { ref mut class_iri, .. }) = current_atom
                            {
                                *class_iri = Some(iri.clone());
                            }
                        }
                    }
                    "ObjectProperty" | "DataProperty" | "propertyPredicate" => {
                        if let Some(iri) = attrs.get("IRI").or(attrs.get("rdf:resource")) {
                            if let Some(AtomBuilder::Property {
                                ref mut property, ..
                            }) = current_atom
                            {
                                *property = Some(iri.clone());
                            }
                        }
                    }
                    "Variable" => {
                        if let Some(iri) = attrs.get("IRI").or(attrs.get("rdf:about")) {
                            let var = SwrlArg::Variable(iri.clone());
                            set_next_arg(&mut current_atom, var);
                        }
                    }
                    "NamedIndividual" | "IndividualID" => {
                        if let Some(iri) = attrs.get("IRI").or(attrs.get("rdf:about")) {
                            let ind = SwrlArg::Individual(iri.clone());
                            set_next_arg(&mut current_atom, ind);
                        }
                    }
                    "Literal" => {
                        if let Some(dt) = attrs.get("datatypeIRI") {
                            // Value will come as text content
                            let val = SwrlArg::Literal {
                                value: String::new(),
                                datatype: Some(dt.clone()),
                            };
                            set_next_arg(&mut current_atom, val);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let local_name = local_name(e.name().as_ref());
                match local_name.as_str() {
                    "Imp" | "DLSafeRule" | "Rule" => {
                        if let Some(builder) = current_rule.take() {
                            match builder.build() {
                                Ok(rule) => {
                                    debug!("Parsed SWRL rule: {:?}", rule.name);
                                    rules.push(rule);
                                }
                                Err(e) => warn!("Skipping malformed SWRL rule: {}", e),
                            }
                        }
                    }
                    "body" | "Body" => {
                        in_body = false;
                    }
                    "head" | "Head" => {
                        in_head = false;
                    }
                    "ClassAtom" | "ObjectPropertyAtom" | "DataPropertyAtom"
                    | "SameIndividualAtom" | "DifferentIndividualsAtom" | "BuiltinAtom" => {
                        if let Some(atom_builder) = current_atom.take() {
                            if let Ok(atom) = atom_builder.build() {
                                if let Some(ref mut rule) = current_rule {
                                    if in_body {
                                        rule.body.push(atom);
                                    } else if in_head {
                                        rule.head.push(atom);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                // Handle literal text content
                if let Ok(text) = e.unescape() {
                    let text = text.to_string();
                    if !text.trim().is_empty() {
                        // Update the last literal arg with its value
                        if let Some(ref mut atom) = current_atom {
                            update_last_literal(atom, &text);
                        }
                    }
                }
            }
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok(rules)
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
        2 => Ok(Atom::ObjectPropertyAtom {
            property: predicate.to_string(),
            arg1: args[0].clone(),
            arg2: args[1].clone(),
        }),
        _ => Err(format!(
            "Unexpected number of arguments in atom: {}",
            input
        )),
    }
}

// ── Helper types ────────────────────────────────────────────────────────

enum AtomBuilder {
    Class {
        class_iri: Option<String>,
        arg: Option<SwrlArg>,
    },
    Property {
        property: Option<String>,
        arg1: Option<SwrlArg>,
        arg2: Option<SwrlArg>,
        is_data: bool,
    },
    SameIndividual {
        arg1: Option<SwrlArg>,
        arg2: Option<SwrlArg>,
    },
    DifferentIndividuals {
        arg1: Option<SwrlArg>,
        arg2: Option<SwrlArg>,
    },
    Builtin {
        builtin: String,
        args: Vec<SwrlArg>,
    },
}

impl AtomBuilder {
    fn build(self) -> Result<Atom, String> {
        match self {
            AtomBuilder::Class { class_iri, arg } => Ok(Atom::ClassAtom {
                class_iri: class_iri.ok_or("ClassAtom missing class IRI")?,
                arg: arg.ok_or("ClassAtom missing argument")?,
            }),
            AtomBuilder::Property {
                property,
                arg1,
                arg2,
                is_data,
            } => {
                let prop = property.ok_or("PropertyAtom missing property IRI")?;
                let a1 = arg1.ok_or("PropertyAtom missing first argument")?;
                let a2 = arg2.ok_or("PropertyAtom missing second argument")?;
                if is_data {
                    Ok(Atom::DataPropertyAtom {
                        property: prop,
                        arg1: a1,
                        arg2: a2,
                    })
                } else {
                    Ok(Atom::ObjectPropertyAtom {
                        property: prop,
                        arg1: a1,
                        arg2: a2,
                    })
                }
            }
            AtomBuilder::SameIndividual { arg1, arg2 } => Ok(Atom::SameIndividualAtom {
                arg1: arg1.ok_or("SameIndividualAtom missing first argument")?,
                arg2: arg2.ok_or("SameIndividualAtom missing second argument")?,
            }),
            AtomBuilder::DifferentIndividuals { arg1, arg2 } => {
                Ok(Atom::DifferentIndividualsAtom {
                    arg1: arg1.ok_or("DifferentIndividualsAtom missing first argument")?,
                    arg2: arg2.ok_or("DifferentIndividualsAtom missing second argument")?,
                })
            }
            AtomBuilder::Builtin { builtin, args } => Ok(Atom::BuiltinAtom { builtin, args }),
        }
    }
}

fn set_next_arg(atom: &mut Option<AtomBuilder>, arg: SwrlArg) {
    match atom {
        Some(AtomBuilder::Class {
            arg: ref mut class_arg,
            ..
        }) if class_arg.is_none() => *class_arg = Some(arg),
        Some(AtomBuilder::Property {
            ref mut arg1,
            ref mut arg2,
            ..
        }) => {
            if arg1.is_none() {
                *arg1 = Some(arg);
            } else if arg2.is_none() {
                *arg2 = Some(arg);
            }
        }
        Some(AtomBuilder::SameIndividual {
            ref mut arg1,
            ref mut arg2,
            ..
        })
        | Some(AtomBuilder::DifferentIndividuals {
            ref mut arg1,
            ref mut arg2,
            ..
        }) => {
            if arg1.is_none() {
                *arg1 = Some(arg);
            } else if arg2.is_none() {
                *arg2 = Some(arg);
            }
        }
        Some(AtomBuilder::Builtin { ref mut args, .. }) => {
            args.push(arg);
        }
        _ => {}
    }
}

fn update_last_literal(atom: &mut AtomBuilder, text: &str) {
    let update = |arg: &mut Option<SwrlArg>| {
        if let Some(SwrlArg::Literal { ref mut value, .. }) = arg {
            if value.is_empty() {
                *value = text.to_string();
            }
        }
    };
    match atom {
        AtomBuilder::Class { ref mut arg, .. } => update(arg),
        AtomBuilder::Property { ref mut arg2, .. } => update(arg2),
        AtomBuilder::Builtin { ref mut args, .. } => {
            if let Some(last) = args.last_mut() {
                if let SwrlArg::Literal { ref mut value, .. } = last {
                    if value.is_empty() {
                        *value = text.to_string();
                    }
                }
            }
        }
        _ => {}
    }
}

fn local_name(bytes: &[u8]) -> String {
    let full = String::from_utf8_lossy(bytes);
    full.rsplit_once(':')
        .map(|(_, local)| local.to_string())
        .unwrap_or_else(|| full.to_string())
}

fn collect_attrs(
    reader: &Reader<&[u8]>,
    e: &quick_xml::events::BytesStart,
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for attr in e.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        if let Ok(val) = attr.decode_and_unescape_value(reader.decoder()) {
            map.insert(key, val.to_string());
        }
    }
    map
}

struct RuleBuilder {
    name: Option<String>,
    body: Vec<Atom>,
    head: Vec<Atom>,
}

impl RuleBuilder {
    fn new() -> Self {
        RuleBuilder {
            name: None,
            body: Vec::new(),
            head: Vec::new(),
        }
    }

    fn build(self) -> Result<SwrlRule, String> {
        if self.body.is_empty() && self.head.is_empty() {
            return Err("Rule has no body or head".to_string());
        }
        Ok(SwrlRule {
            name: self.name,
            body: self.body,
            head: self.head,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_rules() {
        let input = r#"
# If ?x is a Person and ?x knows ?y, then ?y is a Person
Person(?x) ^ knows(?x, ?y) -> Person(?y)

# If ?x is a Parent and ?x hasChild ?y, then ?y hasParent ?x
Parent(?x) ^ hasChild(?x, ?y) -> hasParent(?y, ?x)
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
        let atom = parse_single_atom("Person(?x)").unwrap();
        assert!(matches!(atom, Atom::ClassAtom { .. }));

        let atom = parse_single_atom("knows(?x, ?y)").unwrap();
        assert!(matches!(atom, Atom::ObjectPropertyAtom { .. }));
    }
}
