//! Native evaluation of SWRL `DataRangeAtom`s: whether a value belongs to an
//! OWL 2 data range.
//!
//! - A **datatype** holds the literals in its value space, read by value: an
//!   `xsd:byte` 5 is in `xsd:integer`, `xsd:decimal`, `owl:rational` and
//!   `owl:real`; `"5.0"^^xsd:decimal` is in `xsd:integer`. `xsd:float` and
//!   `xsd:double` are disjoint from the decimals and from each other, as in
//!   the OWL 2 datatype map. `rdfs:Literal` holds every literal; an
//!   ill-typed literal is in no datatype but `rdfs:Literal`.
//! - **Facets** (`DatatypeRestriction`): `minInclusive`, `maxInclusive`,
//!   `minExclusive`, `maxExclusive` (numbers, dates, times, durations),
//!   `length`, `minLength`, `maxLength` (characters; octets for binary types),
//!   `pattern` (an XSD regular expression, anchored), `totalDigits`,
//!   `fractionDigits` and `rdf:langRange`.
//! - **`DataOneOf`** holds the listed values (compared by value),
//!   **union / intersection** combine, and **`DataComplementOf`** holds every
//!   literal outside its range. An individual is in no data range.

use std::cmp::Ordering;

use oxigraph::model::{Literal, Term};

use super::builtins::xpath_regex;
use super::expr::DataRange;
use super::values::{
    compare_terms, integer_bounds, terms_equal, Num, Value, RDF_LANG_STRING, RDF_PLAIN_LITERAL,
    STRING_TYPES, XSD,
};

const RDFS_LITERAL: &str = "http://www.w3.org/2000/01/rdf-schema#Literal";
const RDF_XML_LITERAL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral";
const OWL_REAL: &str = "http://www.w3.org/2002/07/owl#real";
const OWL_RATIONAL: &str = "http://www.w3.org/2002/07/owl#rational";
const RDF_LANG_RANGE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langRange";

/// Whether the datatype is one this evaluator knows: a range over an unknown
/// datatype is refused when the rule is compiled, not silently false.
pub(crate) fn known_datatype(iri: &str) -> bool {
    if matches!(
        iri,
        RDFS_LITERAL
            | RDF_XML_LITERAL
            | OWL_REAL
            | OWL_RATIONAL
            | RDF_LANG_STRING
            | RDF_PLAIN_LITERAL
    ) {
        return true;
    }
    let Some(local) = iri.strip_prefix(XSD) else {
        return false;
    };
    integer_bounds(local).is_some()
        || STRING_TYPES.contains(&local)
        || matches!(
            local,
            "decimal"
                | "float"
                | "double"
                | "boolean"
                | "dateTime"
                | "dateTimeStamp"
                | "date"
                | "time"
                | "duration"
                | "yearMonthDuration"
                | "dayTimeDuration"
                | "anyURI"
                | "hexBinary"
                | "base64Binary"
                | "gYear"
                | "gYearMonth"
                | "gMonth"
                | "gMonthDay"
                | "gDay"
        )
}

/// Check that every datatype, facet and pattern in `range` is understood.
pub(crate) fn check_range(range: &DataRange) -> Result<(), String> {
    match range {
        DataRange::Datatype(d) => {
            if known_datatype(d) {
                Ok(())
            } else {
                Err(format!(
                    "data range over the unknown datatype <{d}>: refusing the rule rather than \
                     treating every value as outside it"
                ))
            }
        }
        DataRange::IntersectionOf(xs) | DataRange::UnionOf(xs) => {
            xs.iter().try_for_each(check_range)
        }
        DataRange::ComplementOf(x) => check_range(x),
        DataRange::OneOf(_) => Ok(()),
        DataRange::Restriction(d, facets) => {
            check_range(&DataRange::Datatype(d.clone()))?;
            for (f, v) in facets {
                let local = f
                    .strip_prefix(XSD)
                    .or_else(|| (f == RDF_LANG_RANGE).then_some("langRange"));
                match local {
                    Some(
                        "minInclusive" | "maxInclusive" | "minExclusive" | "maxExclusive"
                        | "length" | "minLength" | "maxLength" | "totalDigits" | "fractionDigits"
                        | "langRange",
                    ) => {}
                    Some("pattern") => {
                        xpath_regex(v.value(), "")
                            .map_err(|e| format!("facet xsd:pattern \"{}\": {e}", v.value()))?;
                    }
                    _ => return Err(format!("unsupported facet <{f}> in a DatatypeRestriction")),
                }
            }
            Ok(())
        }
    }
}

/// Whether `value` belongs to `range`.
pub(crate) fn holds(range: &DataRange, value: &Term) -> bool {
    let Term::Literal(lit) = value else {
        return false;
    };
    match range {
        DataRange::Datatype(d) => in_datatype(d, lit),
        DataRange::IntersectionOf(xs) => xs.iter().all(|x| holds(x, value)),
        DataRange::UnionOf(xs) => xs.iter().any(|x| holds(x, value)),
        DataRange::ComplementOf(x) => !holds(x, value),
        DataRange::OneOf(vs) => vs
            .iter()
            .any(|v| terms_equal(&Term::Literal(v.clone()), value)),
        DataRange::Restriction(d, facets) => {
            in_datatype(d, lit) && facets.iter().all(|(f, v)| facet_holds(f, v, lit))
        }
    }
}

/// The values a range lists, when it is a finite enumeration (`DataOneOf`,
/// or a union of them): what an unbound variable may be bound to.
pub(crate) fn enumerate(range: &DataRange) -> Option<Vec<Literal>> {
    match range {
        DataRange::OneOf(vs) => Some(vs.clone()),
        DataRange::UnionOf(xs) => {
            let mut out: Vec<Literal> = Vec::new();
            for x in xs {
                for v in enumerate(x)? {
                    if !out.contains(&v) {
                        out.push(v);
                    }
                }
            }
            Some(out)
        }
        DataRange::IntersectionOf(xs) => {
            // Finite when any member is: keep the listed values all hold.
            let (finite, rest): (Vec<_>, Vec<_>) = xs.iter().partition(|x| enumerate(x).is_some());
            let first = finite.first()?;
            let candidates = enumerate(first)?;
            Some(
                candidates
                    .into_iter()
                    .filter(|v| {
                        let t = Term::Literal(v.clone());
                        finite.iter().skip(1).all(|x| holds(x, &t))
                            && rest.iter().all(|x| holds(x, &t))
                    })
                    .collect(),
            )
        }
        DataRange::Restriction(..) | DataRange::Datatype(_) | DataRange::ComplementOf(_) => None,
    }
}

fn in_datatype(dt: &str, lit: &Literal) -> bool {
    if dt == RDFS_LITERAL {
        return true;
    }
    let lit_dt = lit.datatype().as_str();
    if dt == RDF_LANG_STRING {
        return lit.language().is_some();
    }
    if dt == RDF_PLAIN_LITERAL {
        return lit.language().is_some() || lit_dt == format!("{XSD}string");
    }
    if dt == RDF_XML_LITERAL {
        return lit_dt == RDF_XML_LITERAL;
    }
    let value = Value::of_literal(lit);
    if dt == OWL_REAL || dt == OWL_RATIONAL {
        return matches!(value, Value::Num(Num::I(_) | Num::D(_)));
    }
    let Some(local) = dt.strip_prefix(XSD) else {
        return false;
    };
    if let Some((lo, hi)) = integer_bounds(local) {
        let Value::Num(n @ (Num::I(_) | Num::D(_))) = value else {
            return false;
        };
        let Some(i) = n.as_i64() else {
            // Integral but beyond i64 only fits the unbounded types.
            return matches!(n, Num::D(_))
                && is_integral_decimal(lit.value())
                && lo.is_none_or(|lo| lo <= 0)
                && hi.is_none();
        };
        let i = i as i128;
        return lo.is_none_or(|lo| i >= lo) && hi.is_none_or(|hi| i <= hi);
    }
    let lex = lit.value();
    match local {
        "decimal" => matches!(value, Value::Num(Num::I(_) | Num::D(_))),
        "float" => matches!(value, Value::Num(Num::F(_))),
        "double" => matches!(value, Value::Num(Num::Db(_))),
        "boolean" => matches!(value, Value::Bool(_)),
        "dateTime" => matches!(value, Value::DateTime(_)),
        "dateTimeStamp" => matches!(value, Value::DateTime(d) if d.timezone_offset().is_some()),
        "date" => matches!(value, Value::Date(_)),
        "time" => matches!(value, Value::Time(_)),
        "duration" => matches!(value, Value::Duration(_)),
        "yearMonthDuration" => {
            matches!(value, Value::Duration(d) if d.days() == 0 && d.hours() == 0
                && d.minutes() == 0 && d.seconds() == Default::default())
        }
        "dayTimeDuration" => {
            matches!(value, Value::Duration(d) if d.years() == 0 && d.months() == 0)
        }
        "anyURI" => matches!(value, Value::AnyUri(_)) || lit_dt == format!("{XSD}anyURI"),
        "string" => lit.language().is_none() && matches!(value, Value::Str(..)),
        "normalizedString" => {
            lit.language().is_none()
                && matches!(value, Value::Str(..))
                && !lex.contains(['\n', '\r', '\t'])
        }
        "token" => lit.language().is_none() && matches!(value, Value::Str(..)) && is_token(lex),
        "language" => {
            lit.language().is_none()
                && is_token(lex)
                && regex::Regex::new(r"^[a-zA-Z]{1,8}(-[a-zA-Z0-9]{1,8})*$")
                    .is_ok_and(|r| r.is_match(lex))
        }
        "NMTOKEN" => {
            lit.language().is_none()
                && !lex.is_empty()
                && lex.chars().all(|c| is_name_char(c) || c == ':')
        }
        "Name" => lit.language().is_none() && is_name(lex, true),
        "NCName" => lit.language().is_none() && is_name(lex, false),
        "hexBinary" => {
            lit_dt == format!("{XSD}hexBinary")
                && lex.len().is_multiple_of(2)
                && lex.bytes().all(|b| b.is_ascii_hexdigit())
        }
        "base64Binary" => {
            lit_dt == format!("{XSD}base64Binary")
                && lex
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/= ".contains(&b))
        }
        "gYear" | "gYearMonth" | "gMonth" | "gMonthDay" | "gDay" => {
            lit_dt == dt && g_valid(local, lex.trim())
        }
        _ => false,
    }
}

fn g_valid(local: &str, lex: &str) -> bool {
    use std::str::FromStr;
    match local {
        "gYear" => oxsdatatypes::GYear::from_str(lex).is_ok(),
        "gYearMonth" => oxsdatatypes::GYearMonth::from_str(lex).is_ok(),
        "gMonth" => oxsdatatypes::GMonth::from_str(lex).is_ok(),
        "gMonthDay" => oxsdatatypes::GMonthDay::from_str(lex).is_ok(),
        _ => oxsdatatypes::GDay::from_str(lex).is_ok(),
    }
}

fn is_integral_decimal(lex: &str) -> bool {
    let t = lex.trim();
    match t.split_once('.') {
        None => true,
        Some((_, frac)) => frac.bytes().all(|b| b == b'0'),
    }
}

fn is_token(s: &str) -> bool {
    !s.contains(['\n', '\r', '\t']) && !s.starts_with(' ') && !s.ends_with(' ') && !s.contains("  ")
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '\u{B7}')
}

fn is_name(s: &str, colon: bool) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let start_ok = first.is_alphabetic() || first == '_' || (colon && first == ':');
    start_ok && chars.all(|c| is_name_char(c) || (colon && c == ':'))
}

/// The length a length facet measures: characters, or octets for binary.
fn facet_length(lit: &Literal) -> usize {
    let dt = lit.datatype().as_str();
    let lex = lit.value();
    if dt == format!("{XSD}hexBinary") {
        lex.len() / 2
    } else if dt == format!("{XSD}base64Binary") {
        let data = lex.bytes().filter(|b| !b" ".contains(b)).count();
        let pad = lex.bytes().rev().take_while(|b| *b == b'=').count();
        (data / 4) * 3 - pad.min(2)
    } else {
        lex.chars().count()
    }
}

fn facet_holds(facet: &str, bound: &Literal, lit: &Literal) -> bool {
    let local = facet
        .strip_prefix(XSD)
        .or_else(|| (facet == RDF_LANG_RANGE).then_some("langRange"))
        .unwrap_or("");
    let value = Term::Literal(lit.clone());
    let limit = Term::Literal(bound.clone());
    let as_count = || Value::num(&limit).and_then(Num::as_i64);
    match local {
        "minInclusive" => matches!(
            compare_terms(&value, &limit),
            Some(Ordering::Greater | Ordering::Equal)
        ),
        "maxInclusive" => matches!(
            compare_terms(&value, &limit),
            Some(Ordering::Less | Ordering::Equal)
        ),
        "minExclusive" => compare_terms(&value, &limit) == Some(Ordering::Greater),
        "maxExclusive" => compare_terms(&value, &limit) == Some(Ordering::Less),
        "length" => as_count().is_some_and(|n| facet_length(lit) as i64 == n),
        "minLength" => as_count().is_some_and(|n| facet_length(lit) as i64 >= n),
        "maxLength" => as_count().is_some_and(|n| facet_length(lit) as i64 <= n),
        "pattern" => xpath_regex(&format!("^(?:{})$", bound.value()), "")
            .is_ok_and(|r| r.is_match(lit.value())),
        "totalDigits" | "fractionDigits" => {
            let Some(n) = as_count() else { return false };
            let lex = lit.value().trim().trim_start_matches(['+', '-']);
            let (int, frac) = lex.split_once('.').unwrap_or((lex, ""));
            let int = int.trim_start_matches('0');
            let frac = frac.trim_end_matches('0');
            if local == "totalDigits" {
                ((int.len() + frac.len()).max(1) as i64) <= n
            } else {
                (frac.len() as i64) <= n
            }
        }
        "langRange" => {
            let Some(lang) = lit.language() else {
                return false;
            };
            let range = bound.value().to_ascii_lowercase();
            let lang = lang.to_ascii_lowercase();
            range == "*" || lang == range || lang.starts_with(&format!("{range}-"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::model::NamedNode;

    fn lit(v: &str, dt: &str) -> Term {
        Literal::new_typed_literal(v, NamedNode::new_unchecked(format!("{XSD}{dt}"))).into()
    }

    fn dt(local: &str) -> DataRange {
        DataRange::Datatype(format!("{XSD}{local}"))
    }

    #[test]
    fn datatypes_hold_by_value() {
        assert!(holds(&dt("integer"), &lit("5", "byte")));
        assert!(holds(&dt("decimal"), &lit("5", "integer")));
        assert!(holds(&dt("integer"), &lit("5.0", "decimal")));
        assert!(!holds(&dt("integer"), &lit("5.5", "decimal")));
        assert!(!holds(&dt("nonNegativeInteger"), &lit("-1", "integer")));
        assert!(!holds(&dt("double"), &lit("1", "integer")));
        assert!(!holds(&dt("integer"), &lit("x", "integer")));
        assert!(holds(
            &DataRange::Datatype(RDFS_LITERAL.into()),
            &lit("x", "integer")
        ));
        assert!(!holds(
            &dt("string"),
            &Term::NamedNode(NamedNode::new_unchecked("http://ex/a"))
        ));
    }

    #[test]
    fn facets_one_of_and_complement() {
        let adult = DataRange::Restriction(
            format!("{XSD}integer"),
            vec![(
                format!("{XSD}minInclusive"),
                Literal::new_typed_literal("18", NamedNode::new_unchecked(format!("{XSD}integer"))),
            )],
        );
        assert!(holds(&adult, &lit("18", "integer")));
        assert!(!holds(&adult, &lit("17", "integer")));
        let code = DataRange::Restriction(
            format!("{XSD}string"),
            vec![(
                format!("{XSD}pattern"),
                Literal::new_simple_literal("[A-Z]{2}[0-9]+"),
            )],
        );
        assert!(holds(&code, &lit("NL12", "string")));
        assert!(!holds(&code, &lit("xNL12", "string")));
        let one_of = DataRange::OneOf(vec![Literal::new_typed_literal(
            "1",
            NamedNode::new_unchecked(format!("{XSD}integer")),
        )]);
        assert!(holds(&one_of, &lit("1.0", "decimal")));
        assert!(holds(
            &DataRange::ComplementOf(Box::new(one_of.clone())),
            &lit("2", "integer")
        ));
        assert_eq!(enumerate(&one_of).unwrap().len(), 1);
        assert!(enumerate(&adult).is_none());
        assert!(check_range(&DataRange::Datatype("http://ex/myType".into())).is_err());
    }
}
