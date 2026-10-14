//! Rules out: the SWRL RDF syntax (§5), for rules stored with a dataset, and
//! OWL 2 functional syntax, for showing a stored rule exactly.
//!
//! [`rule_to_rdf`] writes a rule as a `swrl:Imp` the strict reader in
//! [`super::rdf`] reads back to the same rule: atoms, lists and class
//! expressions are blank nodes, and every variable is an IRI under the rule's
//! own IRI (`<rule>#x`), so removing a rule never touches another rule's
//! variables. [`rule_to_functional`] renders a `DLSafeRule(…)` that the
//! functional-syntax reader parses back to the same rule.
//!
//! The text and SWRLAPI syntaxes cannot say whether a property atom means an
//! object or a data property; the RDF syntax must. [`settle_property_atoms`]
//! decides it from the rule itself (a literal, a class atom, a data range, …)
//! and refuses the rule when nothing in it decides.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use oxigraph::model::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};

use super::engine::{Atom, SwrlArg, SwrlRule};

const SWRL: &str = "http://www.w3.org/2003/11/swrl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

fn swrl(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{SWRL}{local}"))
}

fn rdf(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{RDF}{local}"))
}

fn named(iri: &str, what: &str) -> Result<NamedNode, String> {
    NamedNode::new(iri).map_err(|e| format!("Invalid {what} IRI '{iri}': {e}"))
}

/// Built-ins whose arguments may be individuals: the list built-ins (RDF
/// list nodes) and the identity comparisons (which compare IRIs too).
const LIST_OR_IDENTITY_BUILTINS: &[&str] = &[
    "equal",
    "notEqual",
    "listConcat",
    "listIntersection",
    "listSubtraction",
    "member",
    "length",
    "first",
    "rest",
    "sublist",
    "empty",
];

/// What a variable stands for, by the positions it occurs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Individual,
    Data,
}

fn note(sorts: &mut HashMap<String, Sort>, arg: &SwrlArg, sort: Sort) {
    if let SwrlArg::Variable(v) = arg {
        sorts.entry(v.clone()).or_insert(sort);
    }
}

/// Turn every property atom of unknown kind (the text and SWRLAPI syntaxes)
/// into an object or a data property atom. The second argument decides: a
/// literal is data, an individual is an object; a variable is whatever its
/// other occurrences in the rule make it (an argument of a class atom, an
/// object property atom or a same/different-individuals atom is an
/// individual; the value of a data property atom or a data range atom is
/// data; failing those, an argument of a built-in other than the list and
/// identity built-ins is data). A variable nothing in the rule pins down refuses the rule,
/// naming the property, since the RDF syntax has no way to leave it open.
pub fn settle_property_atoms(rules: &mut [SwrlRule]) -> Result<(), String> {
    for (i, rule) in rules.iter_mut().enumerate() {
        let mut sorts: HashMap<String, Sort> = HashMap::new();
        for atom in rule.body.iter().chain(&rule.head) {
            match atom {
                Atom::ClassAtom { arg, .. } | Atom::ClassExpressionAtom { arg, .. } => {
                    note(&mut sorts, arg, Sort::Individual)
                }
                Atom::ObjectPropertyAtom { arg1, arg2, .. }
                | Atom::SameIndividualAtom { arg1, arg2 }
                | Atom::DifferentIndividualsAtom { arg1, arg2 } => {
                    note(&mut sorts, arg1, Sort::Individual);
                    note(&mut sorts, arg2, Sort::Individual);
                }
                Atom::DataPropertyAtom { arg1, arg2, .. } => {
                    note(&mut sorts, arg1, Sort::Individual);
                    note(&mut sorts, arg2, Sort::Data);
                }
                Atom::PropertyAtom { arg1, .. } => note(&mut sorts, arg1, Sort::Individual),
                Atom::DataRangeAtom { arg, .. } => note(&mut sorts, arg, Sort::Data),
                Atom::BuiltinAtom { .. } => {}
            }
        }
        // A built-in's arguments are data values, except for the list
        // built-ins (RDF list nodes) and `equal` / `notEqual` (which also
        // compare IRIs): a weaker hint, used only when nothing above decides.
        for atom in rule.body.iter().chain(&rule.head) {
            let Atom::BuiltinAtom { builtin, args } = atom else {
                continue;
            };
            let local = builtin.rsplit(['#', '/']).next().unwrap_or("");
            if LIST_OR_IDENTITY_BUILTINS.contains(&local) {
                continue;
            }
            for a in args {
                note(&mut sorts, a, Sort::Data);
            }
        }
        let name = rule
            .name
            .clone()
            .unwrap_or_else(|| format!("rule {}", i + 1));
        for atom in rule.body.iter_mut().chain(rule.head.iter_mut()) {
            let Atom::PropertyAtom {
                property,
                arg1,
                arg2,
            } = atom
            else {
                continue;
            };
            let sort = match &*arg2 {
                SwrlArg::Literal { .. } => Sort::Data,
                SwrlArg::Individual(_) => Sort::Individual,
                SwrlArg::Variable(v) => *sorts.get(v).ok_or_else(|| {
                    format!(
                        "{name}: cannot tell whether <{property}> is an object or a data \
                         property ({v} occurs nowhere else that says). Write the rule in a \
                         syntax that says it (OWL/XML, functional or RDF), or add an atom \
                         that pins {v} down"
                    )
                })?,
            };
            let (property, arg1, arg2) = (property.clone(), arg1.clone(), arg2.clone());
            *atom = match sort {
                Sort::Individual => Atom::ObjectPropertyAtom {
                    property,
                    arg1,
                    arg2,
                },
                Sort::Data => Atom::DataPropertyAtom {
                    property,
                    arg1,
                    arg2,
                },
            };
        }
    }
    Ok(())
}

/// The IRIs a rule's variables get under `imp`: `<imp>#<local name>`, the
/// local name taken from `?x` or the variable's own IRI, kept unique.
fn variable_iris(rule: &SwrlRule, imp: &str) -> Result<HashMap<String, NamedNode>, String> {
    let mut out: HashMap<String, NamedNode> = HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    for atom in rule.body.iter().chain(&rule.head) {
        for arg in atom_args(atom) {
            let SwrlArg::Variable(v) = arg else { continue };
            if out.contains_key(v) {
                continue;
            }
            let raw = v.trim_start_matches('?');
            let local = raw
                .rsplit(['#', '/', ':'])
                .find(|s| !s.is_empty())
                .unwrap_or("v");
            let mut base: String = local
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            if base.is_empty() {
                base = "v".to_string();
            }
            let mut name = base.clone();
            let mut n = 2;
            while !used.insert(name.clone()) {
                name = format!("{base}{n}");
                n += 1;
            }
            out.insert(v.clone(), named(&format!("{imp}#{name}"), "variable")?);
        }
    }
    Ok(out)
}

fn atom_args(atom: &Atom) -> Vec<&SwrlArg> {
    match atom {
        Atom::ClassAtom { arg, .. }
        | Atom::ClassExpressionAtom { arg, .. }
        | Atom::DataRangeAtom { arg, .. } => vec![arg],
        Atom::ObjectPropertyAtom { arg1, arg2, .. }
        | Atom::DataPropertyAtom { arg1, arg2, .. }
        | Atom::PropertyAtom { arg1, arg2, .. }
        | Atom::SameIndividualAtom { arg1, arg2 }
        | Atom::DifferentIndividualsAtom { arg1, arg2 } => vec![arg1, arg2],
        Atom::BuiltinAtom { args, .. } => args.iter().collect(),
    }
}

fn literal(value: &str, datatype: Option<&str>, language: Option<&str>) -> Result<Literal, String> {
    Ok(match (language, datatype) {
        (Some(lang), _) => Literal::new_language_tagged_literal(value, lang)
            .map_err(|e| format!("Invalid language tag '{lang}': {e}"))?,
        (None, Some(dt)) => Literal::new_typed_literal(value, named(dt, "datatype")?),
        (None, None) => Literal::new_simple_literal(value),
    })
}

fn rdf_list(items: Vec<Term>, out: &mut Vec<Triple>) -> Term {
    let mut rest: Term = rdf("nil").into();
    for item in items.into_iter().rev() {
        let cell = BlankNode::default();
        out.push(Triple::new(cell.clone(), rdf("first"), item));
        out.push(Triple::new(cell.clone(), rdf("rest"), rest));
        rest = cell.into();
    }
    rest
}

/// `rule` in the SWRL RDF syntax, as the `swrl:Imp` named `imp`. Its
/// variables are typed `swrl:Variable` under `imp`. A property atom of
/// unknown kind is refused: settle it first ([`settle_property_atoms`]).
pub fn rule_to_rdf(rule: &SwrlRule, imp: &NamedNode) -> Result<Vec<Triple>, String> {
    let vars = variable_iris(rule, imp.as_str())?;
    let mut out = Vec::new();
    out.push(Triple::new(imp.clone(), rdf("type"), swrl("Imp")));
    for v in vars.values() {
        out.push(Triple::new(v.clone(), rdf("type"), swrl("Variable")));
    }
    let arg = |a: &SwrlArg| -> Result<Term, String> {
        Ok(match a {
            SwrlArg::Variable(v) => vars[v].clone().into(),
            SwrlArg::Individual(i) => named(i, "individual")?.into(),
            SwrlArg::Literal {
                value,
                datatype,
                language,
            } => literal(value, datatype.as_deref(), language.as_deref())?.into(),
        })
    };
    let atoms = |list: &[Atom], out: &mut Vec<Triple>| -> Result<Term, String> {
        let mut nodes = Vec::new();
        for atom in list {
            let node = BlankNode::default();
            let ty = |t: &str| Triple::new(node.clone(), rdf("type"), swrl(t));
            match atom {
                Atom::ClassAtom { class_iri, arg: a } => {
                    out.push(ty("ClassAtom"));
                    out.push(Triple::new(
                        node.clone(),
                        swrl("classPredicate"),
                        named(class_iri, "class")?,
                    ));
                    out.push(Triple::new(node.clone(), swrl("argument1"), arg(a)?));
                }
                Atom::ClassExpressionAtom { expr, arg: a } => {
                    expr.validate()?;
                    out.push(ty("ClassAtom"));
                    let c: NamedOrBlankNode = expr.to_rdf(out);
                    out.push(Triple::new(node.clone(), swrl("classPredicate"), c));
                    out.push(Triple::new(node.clone(), swrl("argument1"), arg(a)?));
                }
                Atom::ObjectPropertyAtom {
                    property,
                    arg1,
                    arg2,
                }
                | Atom::DataPropertyAtom {
                    property,
                    arg1,
                    arg2,
                } => {
                    out.push(ty(if matches!(atom, Atom::ObjectPropertyAtom { .. }) {
                        "IndividualPropertyAtom"
                    } else {
                        "DatavaluedPropertyAtom"
                    }));
                    out.push(Triple::new(
                        node.clone(),
                        swrl("propertyPredicate"),
                        named(property, "property")?,
                    ));
                    out.push(Triple::new(node.clone(), swrl("argument1"), arg(arg1)?));
                    out.push(Triple::new(node.clone(), swrl("argument2"), arg(arg2)?));
                }
                Atom::PropertyAtom { property, .. } => {
                    return Err(format!(
                        "<{property}>: a property atom of unknown kind has no RDF form"
                    ))
                }
                Atom::SameIndividualAtom { arg1, arg2 }
                | Atom::DifferentIndividualsAtom { arg1, arg2 } => {
                    out.push(ty(if matches!(atom, Atom::SameIndividualAtom { .. }) {
                        "SameIndividualAtom"
                    } else {
                        "DifferentIndividualsAtom"
                    }));
                    out.push(Triple::new(node.clone(), swrl("argument1"), arg(arg1)?));
                    out.push(Triple::new(node.clone(), swrl("argument2"), arg(arg2)?));
                }
                Atom::DataRangeAtom { range, arg: a } => {
                    range.validate()?;
                    out.push(ty("DataRangeAtom"));
                    let r: NamedOrBlankNode = range.to_rdf(out);
                    out.push(Triple::new(node.clone(), swrl("dataRange"), r));
                    out.push(Triple::new(node.clone(), swrl("argument1"), arg(a)?));
                }
                Atom::BuiltinAtom { builtin, args } => {
                    out.push(ty("BuiltinAtom"));
                    out.push(Triple::new(
                        node.clone(),
                        swrl("builtin"),
                        named(builtin, "built-in")?,
                    ));
                    let items = args.iter().map(&arg).collect::<Result<Vec<_>, _>>()?;
                    let list = rdf_list(items, out);
                    out.push(Triple::new(node.clone(), swrl("arguments"), list));
                }
            }
            nodes.push(node.into());
        }
        Ok(rdf_list(nodes, out))
    };
    let body = atoms(&rule.body, &mut out)?;
    let head = atoms(&rule.head, &mut out)?;
    out.push(Triple::new(imp.clone(), swrl("body"), body));
    out.push(Triple::new(imp.clone(), swrl("head"), head));
    Ok(out)
}

fn functional_arg(a: &SwrlArg) -> String {
    match a {
        SwrlArg::Variable(v) => {
            if v.contains(':') {
                format!("Variable(<{v}>)")
            } else {
                // `?x` from the text and SWRLAPI syntaxes: any IRI names it.
                format!("Variable(<urn:swrl:var#{}>)", v.trim_start_matches('?'))
            }
        }
        SwrlArg::Individual(i) => format!("<{i}>"),
        SwrlArg::Literal {
            value,
            datatype,
            language,
        } => match literal(value, datatype.as_deref(), language.as_deref()) {
            Ok(l) => l.to_string(),
            Err(_) => Literal::new_simple_literal(value).to_string(),
        },
    }
}

/// `rule` as an OWL 2 functional-syntax `DLSafeRule(…)`, full IRIs only. A
/// property atom of unknown kind is written as an object property atom.
pub fn rule_to_functional(rule: &SwrlRule) -> String {
    let atoms = |list: &[Atom]| -> String {
        list.iter()
            .map(|atom| match atom {
                Atom::ClassAtom { class_iri, arg } => {
                    format!("ClassAtom(<{class_iri}> {})", functional_arg(arg))
                }
                Atom::ClassExpressionAtom { expr, arg } => {
                    format!("ClassAtom({expr} {})", functional_arg(arg))
                }
                Atom::ObjectPropertyAtom {
                    property,
                    arg1,
                    arg2,
                }
                | Atom::PropertyAtom {
                    property,
                    arg1,
                    arg2,
                } => format!(
                    "ObjectPropertyAtom(<{property}> {} {})",
                    functional_arg(arg1),
                    functional_arg(arg2)
                ),
                Atom::DataPropertyAtom {
                    property,
                    arg1,
                    arg2,
                } => format!(
                    "DataPropertyAtom(<{property}> {} {})",
                    functional_arg(arg1),
                    functional_arg(arg2)
                ),
                Atom::SameIndividualAtom { arg1, arg2 } => format!(
                    "SameIndividualAtom({} {})",
                    functional_arg(arg1),
                    functional_arg(arg2)
                ),
                Atom::DifferentIndividualsAtom { arg1, arg2 } => format!(
                    "DifferentIndividualsAtom({} {})",
                    functional_arg(arg1),
                    functional_arg(arg2)
                ),
                Atom::DataRangeAtom { range, arg } => {
                    format!("DataRangeAtom({range} {})", functional_arg(arg))
                }
                Atom::BuiltinAtom { builtin, args } => {
                    let mut s = format!("BuiltInAtom(<{builtin}>");
                    for a in args {
                        let _ = write!(s, " {}", functional_arg(a));
                    }
                    s.push(')');
                    s
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "DLSafeRule(Body({}) Head({}))",
        atoms(&rule.body),
        atoms(&rule.head)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swrl::{parse_rules, rdf::parse_swrl_rdf, ParseOptions};
    use oxigraph::io::{RdfFormat, RdfSerializer};

    fn turtle(triples: &[Triple]) -> String {
        let mut w = RdfSerializer::from_format(RdfFormat::NTriples).for_writer(Vec::new());
        for t in triples {
            w.serialize_triple(t).unwrap();
        }
        String::from_utf8(w.finish().unwrap()).unwrap()
    }

    fn round_trip(format: &str, text: &str) -> (Vec<SwrlRule>, Vec<SwrlRule>) {
        let mut rules = parse_rules(format, text, &ParseOptions::default()).unwrap();
        settle_property_atoms(&mut rules).unwrap();
        let mut all = Vec::new();
        for (i, r) in rules.iter().enumerate() {
            let imp =
                NamedNode::new(format!("urn:uuid:00000000-0000-0000-0000-00000000000{i}")).unwrap();
            all.extend(rule_to_rdf(r, &imp).unwrap());
        }
        let back = parse_swrl_rdf(&turtle(&all), RdfFormat::NTriples, None).unwrap();
        (rules, back)
    }

    #[test]
    fn functional_rules_survive_rdf_and_functional_round_trips() {
        let text = "Prefix(ex:=<http://ex/>) \
            DLSafeRule(Body(ClassAtom(ex:Person Variable(ex:p)) \
              DataPropertyAtom(ex:age Variable(ex:p) Variable(ex:a)) \
              BuiltInAtom(<http://www.w3.org/2003/11/swrlb#greaterThanOrEqual> Variable(ex:a) \"18\"^^<http://www.w3.org/2001/XMLSchema#integer>) \
              ClassAtom(ObjectSomeValuesFrom(ex:knows ex:Person) Variable(ex:p)) \
              DataRangeAtom(DataOneOf(\"a\" \"b\") Variable(ex:a))) \
              Head(ClassAtom(ex:Adult Variable(ex:p)) ObjectPropertyAtom(ex:likes Variable(ex:p) ex:cake)))";
        let (rules, back) = round_trip("functional", text);
        assert_eq!(back.len(), 1);
        let f1 = rule_to_functional(&rules[0]);
        let f2 = rule_to_functional(&back[0]);
        // Variable names differ (the stored ones live under the rule's IRI);
        // the rest is the same rule.
        let strip = |s: &str| {
            let stored = "urn:uuid:00000000-0000-0000-0000-000000000000#";
            s.replace("Variable(<http://ex/p>)", "Variable(p)")
                .replace("Variable(<http://ex/a>)", "Variable(a)")
                .replace(&format!("Variable(<{stored}p>)"), "Variable(p)")
                .replace(&format!("Variable(<{stored}a>)"), "Variable(a)")
        };
        assert_eq!(strip(&f1), strip(&f2));
        // The functional rendering reads back.
        let again = parse_rules("functional", &f2, &ParseOptions::default()).unwrap();
        assert_eq!(rule_to_functional(&again[0]), f2);
    }

    #[test]
    fn text_property_atoms_are_settled_by_the_rule() {
        let (rules, back) = round_trip(
            "text",
            "http://ex/A(?x) ^ http://ex/p(?x, ?y) ^ http://ex/B(?y) ^ http://ex/q(?x, \"v\") -> http://ex/C(?x)",
        );
        assert!(matches!(rules[0].body[1], Atom::ObjectPropertyAtom { .. }));
        assert!(matches!(rules[0].body[3], Atom::DataPropertyAtom { .. }));
        assert!(matches!(back[0].body[1], Atom::ObjectPropertyAtom { .. }));
        assert!(matches!(back[0].body[3], Atom::DataPropertyAtom { .. }));
    }

    #[test]
    fn an_undecidable_property_atom_is_refused_by_name() {
        let mut rules = parse_rules(
            "text",
            "http://ex/p(?x, ?y) -> http://ex/q(?x, ?y)",
            &ParseOptions::default(),
        )
        .unwrap();
        let e = settle_property_atoms(&mut rules).unwrap_err();
        assert!(e.contains("<http://ex/p>"), "{e}");
    }
}
