//! The SWRLAPI human-readable rule syntax, as Protégé's SWRL tab shows it:
//!
//! ```text
//! ex:Person(?p) ^ ex:hasAge(?p, ?age) ^ swrlb:greaterThan(?age, 17) -> ex:Adult(?p)
//! ```
//!
//! Names are prefixed (`ex:Person`), full `<IRI>`s, or bare (`Person`) when a
//! default prefix is declared. Prefixes come from the request first, then the
//! server's prefix registry; `rdf:`, `rdfs:`, `xsd:`, `owl:`, `swrl:` and
//! `swrlb:` are always known. A predicate in the `swrlb:` namespace (or
//! another SWRLAPI built-in library) is a built-in; `sameAs` and
//! `differentFrom` are the identity atoms; a datatype with one argument
//! (`xsd:integer(?v)`, also `rdfs:Literal`, `rdf:PlainLiteral`,
//! `rdf:langString`, `owl:real`, `owl:rational`) is a data range atom;
//! otherwise one argument makes a class atom and two a property atom. Without an ontology the syntax cannot
//! say whether a property is an object or a data property, so a property atom
//! whose second argument is a variable stays untyped
//! ([`Atom::PropertyAtom`]); a literal second argument makes it a data
//! property atom, an individual an object property atom.
//!
//! Arguments: `?x` variables, names and `<IRI>`s for individuals, quoted
//! strings (`"v"`, `"v"^^xsd:date`, `"v"@en`), numbers (`17` integer, `2.5`
//! decimal, `1e3` double) and `true`/`false`. Rules follow one another; a
//! rule ends where its head's last atom is not followed by `^`.

use super::engine::{Atom, SwrlArg, SwrlRule};
use super::expr::DataRange;
use super::lexer::{Tok, Tokens};
use super::names::Names;

const OWL_SAME_AS: &str = "http://www.w3.org/2002/07/owl#sameAs";
const OWL_DIFFERENT_FROM: &str = "http://www.w3.org/2002/07/owl#differentFrom";
const SWRLB: &str = "http://www.w3.org/2003/11/swrlb#";
/// SWRLAPI's own built-in libraries (`swrlm:`, `temporal:`, …). They parse
/// as built-ins, so the engine refuses them by name instead of reading them
/// as classes or properties.
const SWRLAPI_BUILTINS: &str = "http://swrl.stanford.edu/ontologies/built-ins/";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// Parse SWRLAPI rules, resolving names with `names`.
pub(crate) fn parse_swrlapi(text: &str, names: &Names) -> Result<Vec<SwrlRule>, String> {
    let mut toks = Tokens::new(text)?;
    let mut rules = Vec::new();
    while toks.peek().is_some() {
        let body = if toks.peek() == Some(&Tok::Arrow) {
            Vec::new()
        } else {
            atoms(&mut toks, names)?
        };
        toks.expect(&Tok::Arrow)?;
        let head = atoms(&mut toks, names)?;
        rules.push(SwrlRule {
            name: Some(format!("rule_{}", rules.len() + 1)),
            body,
            head,
        });
    }
    Ok(rules)
}

fn atoms(toks: &mut Tokens, names: &Names) -> Result<Vec<Atom>, String> {
    let mut out = vec![atom(toks, names)?];
    while toks.peek() == Some(&Tok::Caret) {
        toks.next();
        out.push(atom(toks, names)?);
    }
    Ok(out)
}

/// A name or `<IRI>` as a full IRI. A bare name uses the default prefix.
fn resolve(toks: &Tokens, names: &Names, tok: &Tok) -> Result<String, String> {
    match tok {
        Tok::Iri(i) => Ok(names.resolve(i)),
        Tok::Name(n) if n.contains(':') => names.expand(n).map_err(|e| toks.error(e)),
        Tok::Name(n) => names.expand(&format!(":{n}")).map_err(|_| {
            toks.error(format!(
                "bare name '{n}': declare a default prefix (\"prefixes\": {{\"\": \"http://…\"}}) \
                 or write prefix:{n}"
            ))
        }),
        other => Err(toks.error(format!("expected a name, found {other}"))),
    }
}

fn atom(toks: &mut Tokens, names: &Names) -> Result<Atom, String> {
    let pred_tok = toks
        .next()
        .ok_or_else(|| toks.error("expected an atom, found the end of the input"))?;
    let predicate = match &pred_tok {
        Tok::Name(n) if n == "sameAs" => OWL_SAME_AS.to_string(),
        Tok::Name(n) if n == "differentFrom" => OWL_DIFFERENT_FROM.to_string(),
        Tok::Name(_) | Tok::Iri(_) => resolve(toks, names, &pred_tok)?,
        other => return Err(toks.error(format!("expected an atom, found {other}"))),
    };
    toks.expect(&Tok::LParen)?;
    let mut args = vec![arg(toks, names)?];
    while toks.peek() == Some(&Tok::Comma) {
        toks.next();
        args.push(arg(toks, names)?);
    }
    toks.expect(&Tok::RParen)?;

    if predicate.starts_with(SWRLB) || predicate.starts_with(SWRLAPI_BUILTINS) {
        return Ok(Atom::BuiltinAtom {
            builtin: predicate,
            args,
        });
    }
    let is_same = predicate == OWL_SAME_AS;
    let is_different = predicate == OWL_DIFFERENT_FROM;
    let mut args = args.into_iter();
    let (a, b, rest) = (args.next(), args.next(), args.next());
    Ok(match (a, b, rest) {
        (Some(arg1), Some(arg2), None) if is_same => Atom::SameIndividualAtom { arg1, arg2 },
        (Some(arg1), Some(arg2), None) if is_different => {
            Atom::DifferentIndividualsAtom { arg1, arg2 }
        }
        _ if is_same || is_different => {
            return Err(toks.error(format!("{pred_tok} takes two arguments")))
        }
        (Some(arg), None, None) if is_datatype(&predicate) => Atom::DataRangeAtom {
            range: DataRange::Datatype(predicate),
            arg,
        },
        (Some(arg), None, None) => Atom::ClassAtom {
            class_iri: predicate,
            arg,
        },
        (Some(arg1), Some(arg2), None) => match arg2 {
            SwrlArg::Literal { .. } => Atom::DataPropertyAtom {
                property: predicate,
                arg1,
                arg2,
            },
            SwrlArg::Individual(_) => Atom::ObjectPropertyAtom {
                property: predicate,
                arg1,
                arg2,
            },
            SwrlArg::Variable(_) => Atom::PropertyAtom {
                property: predicate,
                arg1,
                arg2,
            },
        },
        _ => {
            return Err(toks.error(format!(
                "{pred_tok} has more than two arguments, so it must be a built-in, but \
                 <{predicate}> is not in a built-in namespace"
            )))
        }
    })
}

/// Whether `iri` names a datatype: a unary atom over it is a data range atom.
fn is_datatype(iri: &str) -> bool {
    iri.starts_with(XSD)
        || matches!(
            iri,
            "http://www.w3.org/2000/01/rdf-schema#Literal"
                | "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral"
                | "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"
                | "http://www.w3.org/2002/07/owl#real"
                | "http://www.w3.org/2002/07/owl#rational"
        )
}

fn arg(toks: &mut Tokens, names: &Names) -> Result<SwrlArg, String> {
    let tok = toks
        .next()
        .ok_or_else(|| toks.error("expected an argument, found the end of the input"))?;
    let typed = |value: String, dt: &str| SwrlArg::Literal {
        value,
        datatype: Some(format!("{XSD}{dt}")),
        language: None,
    };
    Ok(match tok {
        Tok::Var(v) => SwrlArg::Variable(v),
        Tok::Str(value) => match toks.peek().cloned() {
            Some(Tok::DoubleCaret) => {
                toks.next();
                let dt_tok = toks
                    .next()
                    .ok_or_else(|| toks.error("expected a datatype after ^^"))?;
                SwrlArg::Literal {
                    value,
                    datatype: Some(resolve(toks, names, &dt_tok)?),
                    language: None,
                }
            }
            Some(Tok::Lang(lang)) => {
                toks.next();
                SwrlArg::Literal {
                    value,
                    datatype: None,
                    language: Some(lang),
                }
            }
            _ => SwrlArg::Literal {
                value,
                datatype: None,
                language: None,
            },
        },
        Tok::Number(n) => {
            let dt = if n.contains(['e', 'E']) {
                "double"
            } else if n.contains('.') {
                "decimal"
            } else {
                "integer"
            };
            typed(n, dt)
        }
        Tok::Name(n) if n == "true" || n == "false" => typed(n, "boolean"),
        Tok::Name(_) | Tok::Iri(_) => SwrlArg::Individual(resolve(toks, names, &tok)?),
        Tok::BlankNode(b) => {
            return Err(toks.error(format!(
                "{b} is an anonymous individual, which rules do not support"
            )))
        }
        other => return Err(toks.error(format!("expected an argument, found {other}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Names {
        let mut n = Names::default();
        n.declare("ex", "http://ex/");
        n
    }

    #[test]
    fn reads_rules_with_builtins_literals_and_identity() {
        let text = r#"ex:Person(?p) ^ ex:age(?p, ?a) ^ swrlb:greaterThan(?a, 17)
                        ^ ex:nick(?p, "A, ^ B"@en) -> ex:Adult(?p)
                      ex:knows(?x, ?y) ^ differentFrom(?x, ?y) -> ex:Acquainted(?x)
                      -> ex:Agent(ex:alice)"#;
        let rules = parse_swrlapi(text, &names()).unwrap();
        assert_eq!(rules.len(), 3);
        let r = &rules[0];
        assert_eq!(r.body.len(), 4);
        assert!(matches!(&r.body[1], Atom::PropertyAtom { .. }));
        assert!(matches!(&r.body[2], Atom::BuiltinAtom { args, .. }
            if matches!(&args[1], SwrlArg::Literal { datatype: Some(d), .. }
                if d.ends_with("#integer"))));
        assert!(matches!(&r.body[3], Atom::DataPropertyAtom {
            arg2: SwrlArg::Literal { value, language: Some(l), .. }, ..
        } if value == "A, ^ B" && l == "en"));
        assert!(matches!(
            &rules[1].body[1],
            Atom::DifferentIndividualsAtom { .. }
        ));
        assert!(rules[2].body.is_empty());
        let rules =
            parse_swrlapi("ex:age(?p, ?a) ^ xsd:integer(?a) -> ex:Aged(?p)", &names()).unwrap();
        assert!(
            matches!(&rules[0].body[1], Atom::DataRangeAtom { range: DataRange::Datatype(d), .. }
            if d.ends_with("#integer"))
        );
    }

    #[test]
    fn names_need_known_prefixes() {
        let err = parse_swrlapi("Person(?p) -> ex:Agent(?p)", &names()).unwrap_err();
        assert!(err.contains("bare name 'Person'"), "{err}");
        let err = parse_swrlapi("foaf:Person(?p) -> ex:Agent(?p)", &names()).unwrap_err();
        assert!(err.contains("Unknown prefix 'foaf:'"), "{err}");
        let mut n = names();
        n.declare("", "http://ex/");
        let rules = parse_swrlapi("Person(?p) -> Agent(?p)", &n).unwrap();
        assert!(
            matches!(&rules[0].head[0], Atom::ClassAtom { class_iri, .. }
            if class_iri == "http://ex/Agent")
        );
        let err = parse_swrlapi("ex:p(?a, ?b, ?c) -> ex:T(?a)", &names()).unwrap_err();
        assert!(err.contains("built-in"), "{err}");
    }
}
