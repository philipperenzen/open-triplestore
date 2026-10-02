//! The OWL 2 EL datatype map (OWL 2 Profiles §2.2.1): nineteen datatypes,
//! no facets, value spaces and value equality.
//!
//! EL keeps only datatypes whose value spaces intersect either not at all or
//! infinitely, so data reasoning reduces to three facts per value: which EL
//! datatypes contain it, which datatypes are disjoint, and when two literals
//! denote the same value. This module answers those; the EL engine turns the
//! datatypes into concepts and each distinct value into a nominal.
//!
//! This is the EL subset of the OWL 2 datatype map. A shared module for the
//! RL and EL maps (with the RL types, facets and exact float semantics) is
//! planned; this one should fold into it.
//!
//! Storage note: oxigraph stores every integer-derived XSD type as
//! `xsd:integer` and `xsd:dateTimeStamp` as `xsd:dateTime`, so
//! `"-5"^^xsd:nonNegativeInteger` reaches the reasoner as the integer −5.
//! Membership is decided on the *value*, which gives the right answer for
//! every literal that was well-typed when written.

use chrono::NaiveDateTime;

/// The datatypes of the OWL 2 EL datatype map.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Dt {
    Literal,
    PlainLiteral,
    XmlLiteral,
    Real,
    Rational,
    Decimal,
    Integer,
    NonNegativeInteger,
    String,
    NormalizedString,
    Token,
    Name,
    NcName,
    NmToken,
    HexBinary,
    Base64Binary,
    AnyUri,
    DateTime,
    DateTimeStamp,
}

impl Dt {
    pub const ALL: [Dt; 19] = [
        Dt::Literal,
        Dt::PlainLiteral,
        Dt::XmlLiteral,
        Dt::Real,
        Dt::Rational,
        Dt::Decimal,
        Dt::Integer,
        Dt::NonNegativeInteger,
        Dt::String,
        Dt::NormalizedString,
        Dt::Token,
        Dt::Name,
        Dt::NcName,
        Dt::NmToken,
        Dt::HexBinary,
        Dt::Base64Binary,
        Dt::AnyUri,
        Dt::DateTime,
        Dt::DateTimeStamp,
    ];

    pub fn iri(self) -> &'static str {
        match self {
            Dt::Literal => "http://www.w3.org/2000/01/rdf-schema#Literal",
            Dt::PlainLiteral => "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral",
            Dt::XmlLiteral => "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral",
            Dt::Real => "http://www.w3.org/2002/07/owl#real",
            Dt::Rational => "http://www.w3.org/2002/07/owl#rational",
            Dt::Decimal => "http://www.w3.org/2001/XMLSchema#decimal",
            Dt::Integer => "http://www.w3.org/2001/XMLSchema#integer",
            Dt::NonNegativeInteger => "http://www.w3.org/2001/XMLSchema#nonNegativeInteger",
            Dt::String => "http://www.w3.org/2001/XMLSchema#string",
            Dt::NormalizedString => "http://www.w3.org/2001/XMLSchema#normalizedString",
            Dt::Token => "http://www.w3.org/2001/XMLSchema#token",
            Dt::Name => "http://www.w3.org/2001/XMLSchema#Name",
            Dt::NcName => "http://www.w3.org/2001/XMLSchema#NCName",
            Dt::NmToken => "http://www.w3.org/2001/XMLSchema#NMTOKEN",
            Dt::HexBinary => "http://www.w3.org/2001/XMLSchema#hexBinary",
            Dt::Base64Binary => "http://www.w3.org/2001/XMLSchema#base64Binary",
            Dt::AnyUri => "http://www.w3.org/2001/XMLSchema#anyURI",
            Dt::DateTime => "http://www.w3.org/2001/XMLSchema#dateTime",
            Dt::DateTimeStamp => "http://www.w3.org/2001/XMLSchema#dateTimeStamp",
        }
    }

    pub fn from_iri(iri: &str) -> Option<Dt> {
        // `rdf:langString` is the RDF 1.1 name for the language-tagged part
        // of `rdf:PlainLiteral`; treat it as the plain-literal datatype.
        if iri == "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString" {
            return Some(Dt::PlainLiteral);
        }
        Dt::ALL.into_iter().find(|d| d.iri() == iri)
    }

    /// The datatype whose value space directly contains this one's.
    pub fn parent(self) -> Option<Dt> {
        Some(match self {
            Dt::Literal => return None,
            Dt::PlainLiteral | Dt::XmlLiteral | Dt::Real => Dt::Literal,
            Dt::HexBinary | Dt::Base64Binary | Dt::AnyUri | Dt::DateTime => Dt::Literal,
            Dt::Rational => Dt::Real,
            Dt::Decimal => Dt::Rational,
            Dt::Integer => Dt::Decimal,
            Dt::NonNegativeInteger => Dt::Integer,
            Dt::String => Dt::PlainLiteral,
            Dt::NormalizedString => Dt::String,
            Dt::Token => Dt::NormalizedString,
            Dt::Name | Dt::NmToken => Dt::Token,
            Dt::NcName => Dt::Name,
            Dt::DateTimeStamp => Dt::DateTime,
        })
    }

    /// The datatypes directly below `rdfs:Literal`: their value spaces are
    /// pairwise disjoint (OWL 2 Structural Specification §4: binary data,
    /// IRIs and strings are separate value spaces).
    pub const ROOTS: [Dt; 7] = [
        Dt::PlainLiteral,
        Dt::XmlLiteral,
        Dt::Real,
        Dt::HexBinary,
        Dt::Base64Binary,
        Dt::AnyUri,
        Dt::DateTime,
    ];
}

/// A data value, compared by value: `"1"^^xsd:integer` and `"1.0"^^xsd:decimal`
/// are the same number, `"a"` and `"a"^^xsd:token` the same string.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Value {
    /// A number, canonical: a decimal without superfluous zeros, or a
    /// reduced fraction `n/d` when it has no finite decimal form.
    Number(String),
    /// A string without a language tag.
    Str(String),
    /// A string with a (lower-cased) language tag.
    LangStr(String, String),
    /// A date-time: with a time zone normalized to UTC, without one kept
    /// local (the two never compare equal).
    DateTime {
        canonical: String,
        zoned: bool,
    },
    Hex(Vec<u8>),
    Base64(Vec<u8>),
    Uri(String),
    Xml(String),
    /// A literal of a datatype outside the EL map, compared by its term.
    Other {
        lexical: String,
        datatype: String,
    },
}

/// The value of a literal, or `None` when its lexical form is not in the
/// lexical space of its EL datatype (an ill-typed literal: OWL 2 makes an
/// ontology with one inconsistent).
pub(crate) fn value_of(lexical: &str, datatype: &str, lang: Option<&str>) -> Option<Value> {
    if let Some(lang) = lang {
        return Some(Value::LangStr(
            lexical.to_string(),
            lang.to_ascii_lowercase(),
        ));
    }
    let Some(dt) = Dt::from_iri(datatype) else {
        return Some(Value::Other {
            lexical: lexical.to_string(),
            datatype: datatype.to_string(),
        });
    };
    let v = match dt {
        Dt::Literal | Dt::Real => return None, // no lexical forms
        Dt::PlainLiteral => {
            // `text@lang` (empty lang: a plain string).
            let at = lexical.rfind('@')?;
            let (text, lang) = (&lexical[..at], &lexical[at + 1..]);
            if lang.is_empty() {
                Value::Str(text.to_string())
            } else {
                Value::LangStr(text.to_string(), lang.to_ascii_lowercase())
            }
        }
        Dt::XmlLiteral => Value::Xml(lexical.to_string()),
        Dt::Rational => Value::Number(rational(lexical)?),
        Dt::Decimal => Value::Number(decimal(lexical, true)?),
        Dt::Integer | Dt::NonNegativeInteger => {
            let n = decimal(lexical, false)?;
            if dt == Dt::NonNegativeInteger && n.starts_with('-') {
                return None;
            }
            Value::Number(n)
        }
        Dt::String => Value::Str(lexical.to_string()),
        Dt::NormalizedString => Value::Str(lexical.replace(['\t', '\n', '\r'], " ")),
        Dt::Token | Dt::Name | Dt::NcName | Dt::NmToken => {
            let s = collapse(lexical);
            let ok = match dt {
                Dt::Token => true,
                Dt::NmToken => is_nmtoken(&s),
                Dt::Name => is_name(&s),
                _ => is_name(&s) && !s.contains(':'),
            };
            if !ok {
                return None;
            }
            Value::Str(s)
        }
        Dt::HexBinary => Value::Hex(hex::decode(lexical.trim()).ok()?),
        Dt::Base64Binary => {
            use base64::Engine;
            let s: String = lexical.chars().filter(|c| !c.is_whitespace()).collect();
            Value::Base64(base64::engine::general_purpose::STANDARD.decode(s).ok()?)
        }
        Dt::AnyUri => Value::Uri(collapse(lexical)),
        Dt::DateTime | Dt::DateTimeStamp => {
            let v = date_time(lexical)?;
            if dt == Dt::DateTimeStamp && !matches!(v, Value::DateTime { zoned: true, .. }) {
                return None;
            }
            v
        }
    };
    Some(v)
}

/// Every EL datatype whose value space contains `v`.
pub(crate) fn datatypes_of(v: &Value) -> Vec<Dt> {
    let mut out = vec![Dt::Literal];
    match v {
        Value::Number(n) => {
            out.extend([Dt::Real, Dt::Rational]);
            if !n.contains('/') {
                out.push(Dt::Decimal);
                if !n.contains('.') {
                    out.push(Dt::Integer);
                    if !n.starts_with('-') {
                        out.push(Dt::NonNegativeInteger);
                    }
                }
            }
        }
        Value::Str(s) => {
            out.extend([Dt::PlainLiteral, Dt::String]);
            if !s.contains(['\t', '\n', '\r']) {
                out.push(Dt::NormalizedString);
                if collapse(s) == *s {
                    out.push(Dt::Token);
                    if is_nmtoken(s) {
                        out.push(Dt::NmToken);
                    }
                    if is_name(s) {
                        out.push(Dt::Name);
                        if !s.contains(':') {
                            out.push(Dt::NcName);
                        }
                    }
                }
            }
        }
        Value::LangStr(..) => out.push(Dt::PlainLiteral),
        Value::DateTime { zoned, .. } => {
            out.push(Dt::DateTime);
            if *zoned {
                out.push(Dt::DateTimeStamp);
            }
        }
        Value::Hex(_) => out.push(Dt::HexBinary),
        Value::Base64(_) => out.push(Dt::Base64Binary),
        Value::Uri(_) => out.push(Dt::AnyUri),
        Value::Xml(_) => out.push(Dt::XmlLiteral),
        Value::Other { .. } => {}
    }
    out
}

/// XSD whitespace `collapse`.
fn collapse(s: &str) -> String {
    s.split([' ', '\t', '\n', '\r'])
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == ':'
}

fn is_name_char(c: char) -> bool {
    is_name_start(c) || c.is_numeric() || matches!(c, '-' | '.' | '\u{B7}')
}

fn is_nmtoken(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_name_char)
}

fn is_name(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(is_name_start) && cs.all(is_name_char)
}

/// Canonical form of an `xsd:decimal` (or, without `allow_point`, an
/// `xsd:integer`) lexical form.
fn decimal(lexical: &str, allow_point: bool) -> Option<String> {
    let s = lexical.trim();
    let (neg, body) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) if allow_point => (i, f),
        Some(_) => return None,
        None => (body, ""),
    };
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    let mut out = String::new();
    if neg && !(int.is_empty() && frac.is_empty()) {
        out.push('-');
    }
    out.push_str(if int.is_empty() { "0" } else { int });
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    Some(out)
}

/// Canonical form of an `owl:rational` lexical form `n/d`: the decimal when
/// the reduced fraction has one, else the reduced `n/d`.
fn rational(lexical: &str) -> Option<String> {
    let (n, d) = lexical.trim().split_once('/')?;
    let n = decimal(n, false)?;
    let d = decimal(d, false)?;
    if d.starts_with('-') || d == "0" {
        return None;
    }
    let neg = n.starts_with('-');
    let (Ok(mut num), Ok(mut den)) = (n.trim_start_matches('-').parse::<u128>(), d.parse::<u128>())
    else {
        // Too large to reduce here: equal values may then compare unequal,
        // which loses an entailment but adds none.
        return Some(format!("{n}/{d}"));
    };
    let g = gcd(num, den);
    if g > 1 {
        num /= g;
        den /= g;
    }
    let sign = if neg && num != 0 { "-" } else { "" };
    if den == 1 {
        return Some(format!("{sign}{num}"));
    }
    // A finite decimal iff the denominator has no prime factor but 2 and 5.
    let (mut twos, mut fives, mut rest) = (0u32, 0u32, den);
    while rest % 2 == 0 {
        rest /= 2;
        twos += 1;
    }
    while rest % 5 == 0 {
        rest /= 5;
        fives += 1;
    }
    if rest == 1 {
        let k = twos.max(fives);
        let scale = 10u128.checked_pow(k)?;
        let scaled = num.checked_mul(scale / den)?;
        let digits = format!("{scaled:0>width$}", width = k as usize + 1);
        let (i, f) = digits.split_at(digits.len() - k as usize);
        return decimal(&format!("{sign}{i}.{f}"), true);
    }
    Some(format!("{sign}{num}/{den}"))
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// An `xsd:dateTime`: zoned values are normalized to UTC.
fn date_time(lexical: &str) -> Option<Value> {
    let s = lexical.trim();
    // Split off the zone: `Z`, or `±hh:mm` after the time part.
    let (local, offset_min) = if let Some(rest) = s.strip_suffix('Z') {
        (rest, Some(0i64))
    } else if s.len() > 6
        && matches!(&s.as_bytes()[s.len() - 6], b'+' | b'-')
        && s.as_bytes()[s.len() - 3] == b':'
        && s[..s.len() - 6].contains('T')
    {
        let (l, z) = s.split_at(s.len() - 6);
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let h: i64 = z[1..3].parse().ok()?;
        let m: i64 = z[4..6].parse().ok()?;
        if h > 14 || m > 59 || (h == 14 && m != 0) {
            return None;
        }
        (l, Some(sign * (h * 60 + m)))
    } else {
        (s, None)
    };
    // 24:00:00 is the first instant of the next day.
    let (local, next_day) = match local.split_once("T24:00:00") {
        Some((date, frac)) if frac.trim_start_matches('.').bytes().all(|b| b == b'0') => {
            (format!("{date}T00:00:00"), true)
        }
        _ => (local.to_string(), false),
    };
    let mut dt = NaiveDateTime::parse_from_str(&local, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
    // chrono accepts a single-digit field; XSD does not.
    let (date, time) = local.split_once('T')?;
    let date_ok = date.rsplitn(3, '-').take(2).all(|p| p.len() == 2);
    if !date_ok || time.len() < 8 || &time[2..3] != ":" || &time[5..6] != ":" {
        return None;
    }
    if next_day {
        dt += chrono::Duration::days(1);
    }
    if let Some(off) = offset_min {
        dt -= chrono::Duration::minutes(off);
    }
    let mut canonical = dt.format("%Y-%m-%dT%H:%M:%S%.f").to_string();
    if canonical.contains('.') {
        canonical = canonical
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string();
    }
    if offset_min.is_some() {
        canonical.push('Z');
    }
    Some(Value::DateTime {
        canonical,
        zoned: offset_min.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

    fn v(lex: &str, dt: &str) -> Option<Value> {
        value_of(lex, &format!("{XSD}{dt}"), None)
    }

    #[test]
    fn numbers_compare_by_value() {
        assert_eq!(v("01", "integer"), v("1.0", "decimal"));
        assert_eq!(v("-0", "integer"), v("0", "integer"));
        assert_eq!(v("+2.50", "decimal"), v("2.5", "decimal"));
        assert_eq!(
            value_of("6/4", "http://www.w3.org/2002/07/owl#rational", None),
            v("1.5", "decimal")
        );
        assert_eq!(
            value_of("2/6", "http://www.w3.org/2002/07/owl#rational", None),
            Some(Value::Number("1/3".into()))
        );
        assert!(v("abc", "integer").is_none());
        assert!(v("1.5", "integer").is_none());
        assert!(v("-1", "nonNegativeInteger").is_none());
    }

    #[test]
    fn number_membership() {
        let dts = datatypes_of(&v("5", "decimal").unwrap());
        assert!(dts.contains(&Dt::NonNegativeInteger));
        let dts = datatypes_of(&v("-5.5", "decimal").unwrap());
        assert!(dts.contains(&Dt::Decimal) && !dts.contains(&Dt::Integer));
    }

    #[test]
    fn strings_and_their_subtypes() {
        assert_eq!(v("abc", "string"), v(" abc ", "token"));
        let dts = datatypes_of(&v("abc", "string").unwrap());
        for d in [Dt::Token, Dt::Name, Dt::NcName, Dt::NmToken] {
            assert!(dts.contains(&d), "{d:?}");
        }
        let dts = datatypes_of(&v("a b", "string").unwrap());
        assert!(dts.contains(&Dt::Token) && !dts.contains(&Dt::Name));
        assert!(v("1abc", "NCName").is_none());
        assert!(v("a:b", "NCName").is_none());
        assert!(v("a:b", "Name").is_some());
    }

    #[test]
    fn date_times_normalize_their_zone() {
        assert_eq!(
            v("2020-01-01T01:00:00+01:00", "dateTime"),
            v("2020-01-01T00:00:00Z", "dateTime")
        );
        assert_ne!(
            v("2020-01-01T00:00:00", "dateTime"),
            v("2020-01-01T00:00:00Z", "dateTime")
        );
        assert_eq!(
            v("2019-12-31T24:00:00Z", "dateTime"),
            v("2020-01-01T00:00:00.000Z", "dateTime")
        );
        assert!(v("2020-01-01T00:00:00", "dateTimeStamp").is_none());
        assert!(v("2020-1-01T00:00:00", "dateTime").is_none());
    }

    #[test]
    fn hierarchy_reaches_the_roots() {
        for d in Dt::ALL {
            let mut cur = d;
            while let Some(p) = cur.parent() {
                cur = p;
            }
            assert_eq!(cur, Dt::Literal, "{d:?}");
        }
    }
}
