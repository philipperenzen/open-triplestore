//! The OWL 2 datatype maps, for the profiles that reason over data values:
//! value spaces, value equality, value order, lexical validity and
//! datatype-restriction facets.
//!
//! Two maps are covered:
//!
//! - The OWL 2 EL and QL map (OWL 2 Profiles §2.2.1 and §3.2.1: the same
//!   nineteen datatypes, no facets), [`Dt::ALL`]. Their value spaces
//!   intersect either not at all or infinitely, so data reasoning reduces to
//!   three facts per value: which datatypes contain it, which datatypes are
//!   disjoint, and when two literals denote the same value.
//! - The OWL 2 RL map (OWL 2 Profiles §4.2: thirty-two datatypes), [`Dt::RL`]:
//!   the EL/QL map without `owl:real` and `owl:rational`, plus the
//!   integer-derived XSD types, `xsd:float`, `xsd:double`, `xsd:boolean` and
//!   `xsd:language`. [`Dt::ANY`] is the union of the two.
//!
//! RL also admits datatype restrictions (OWL 2 Structural Specification
//! §7.5): a base datatype narrowed by facets ([`Facet`], read with
//! [`facet_from`], decided with [`in_restriction`]). Bounds are decided with
//! [`compare`], the order of the value space; lengths, patterns and language
//! ranges on the value itself. Every decision is three-valued: `None` means
//! "cannot be told here", never a guess.
//!
//! Value spaces follow OWL 2 (Structural Specification §4): `xsd:float`,
//! `xsd:double` and the numbers (`owl:real` and below) are pairwise
//! disjoint, floating-point values are compared by identity (so `+0` and `−0`
//! are different values, and every NaN is the same one), and `xsd:boolean` is
//! disjoint from every other datatype.
//!
//! Storage note: oxigraph stores every integer-derived XSD type as
//! `xsd:integer` and `xsd:dateTimeStamp` as `xsd:dateTime`, so
//! `"-5"^^xsd:nonNegativeInteger` reaches the reasoner as the integer −5.
//! Membership is decided on the *value*, which gives the right answer for
//! every literal that was well-typed when written. A fork of oxigraph that
//! keeps the lexical forms and datatypes is in progress; once it lands the
//! derived types arrive here as written and [`value_of`] checks their ranges.
#![cfg_attr(not(all(feature = "owl2-ql", feature = "owl2-rl")), allow(dead_code))]

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;

use chrono::NaiveDateTime;
use oxigraph::model::Literal;

const XSD_NS: &str = "http://www.w3.org/2001/XMLSchema#";
const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const RDF_LANG_RANGE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langRange";

/// The datatypes of the OWL 2 EL, QL and RL datatype maps.
///
/// The first nineteen variants are the EL/QL map, in their original order
/// (the derived `Ord` keeps their relative order); the RL-only types follow.
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
    // ── RL map only ──
    NonPositiveInteger,
    PositiveInteger,
    NegativeInteger,
    Long,
    Int,
    Short,
    Byte,
    UnsignedLong,
    UnsignedInt,
    UnsignedShort,
    UnsignedByte,
    Float,
    Double,
    Language,
    Boolean,
}

/// The integer-derived types below `xsd:integer` that only the RL map has.
const RL_INTEGERS: [Dt; 11] = [
    Dt::NonPositiveInteger,
    Dt::PositiveInteger,
    Dt::NegativeInteger,
    Dt::Long,
    Dt::Int,
    Dt::Short,
    Dt::Byte,
    Dt::UnsignedLong,
    Dt::UnsignedInt,
    Dt::UnsignedShort,
    Dt::UnsignedByte,
];

impl Dt {
    /// The OWL 2 EL and QL datatype map (the same nineteen datatypes).
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

    /// The OWL 2 RL datatype map, in the order of OWL 2 Profiles §4.2.
    pub const RL: [Dt; 32] = [
        Dt::PlainLiteral,
        Dt::XmlLiteral,
        Dt::Literal,
        Dt::Decimal,
        Dt::Integer,
        Dt::NonNegativeInteger,
        Dt::NonPositiveInteger,
        Dt::PositiveInteger,
        Dt::NegativeInteger,
        Dt::Long,
        Dt::Int,
        Dt::Short,
        Dt::Byte,
        Dt::UnsignedLong,
        Dt::UnsignedInt,
        Dt::UnsignedShort,
        Dt::UnsignedByte,
        Dt::Float,
        Dt::Double,
        Dt::String,
        Dt::NormalizedString,
        Dt::Token,
        Dt::Language,
        Dt::Name,
        Dt::NcName,
        Dt::NmToken,
        Dt::Boolean,
        Dt::HexBinary,
        Dt::Base64Binary,
        Dt::AnyUri,
        Dt::DateTime,
        Dt::DateTimeStamp,
    ];

    /// Every datatype this module knows: the EL/QL map, then the RL-only types.
    pub const ANY: [Dt; 34] = [
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
        Dt::NonPositiveInteger,
        Dt::PositiveInteger,
        Dt::NegativeInteger,
        Dt::Long,
        Dt::Int,
        Dt::Short,
        Dt::Byte,
        Dt::UnsignedLong,
        Dt::UnsignedInt,
        Dt::UnsignedShort,
        Dt::UnsignedByte,
        Dt::Float,
        Dt::Double,
        Dt::Language,
        Dt::Boolean,
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
            Dt::NonPositiveInteger => "http://www.w3.org/2001/XMLSchema#nonPositiveInteger",
            Dt::PositiveInteger => "http://www.w3.org/2001/XMLSchema#positiveInteger",
            Dt::NegativeInteger => "http://www.w3.org/2001/XMLSchema#negativeInteger",
            Dt::Long => "http://www.w3.org/2001/XMLSchema#long",
            Dt::Int => "http://www.w3.org/2001/XMLSchema#int",
            Dt::Short => "http://www.w3.org/2001/XMLSchema#short",
            Dt::Byte => "http://www.w3.org/2001/XMLSchema#byte",
            Dt::UnsignedLong => "http://www.w3.org/2001/XMLSchema#unsignedLong",
            Dt::UnsignedInt => "http://www.w3.org/2001/XMLSchema#unsignedInt",
            Dt::UnsignedShort => "http://www.w3.org/2001/XMLSchema#unsignedShort",
            Dt::UnsignedByte => "http://www.w3.org/2001/XMLSchema#unsignedByte",
            Dt::Float => "http://www.w3.org/2001/XMLSchema#float",
            Dt::Double => "http://www.w3.org/2001/XMLSchema#double",
            Dt::Language => "http://www.w3.org/2001/XMLSchema#language",
            Dt::Boolean => "http://www.w3.org/2001/XMLSchema#boolean",
        }
    }

    /// A datatype of the EL/QL map ([`Dt::ALL`]) by IRI; `None` for every
    /// other IRI, the RL-only types included.
    pub fn from_iri(iri: &str) -> Option<Dt> {
        Self::find(&Dt::ALL, iri)
    }

    /// A datatype of the RL map ([`Dt::RL`]) by IRI.
    pub fn from_rl_iri(iri: &str) -> Option<Dt> {
        Self::find(&Dt::RL, iri)
    }

    /// Any datatype this module knows ([`Dt::ANY`]) by IRI.
    pub fn from_any_iri(iri: &str) -> Option<Dt> {
        Self::find(&Dt::ANY, iri)
    }

    fn find(map: &[Dt], iri: &str) -> Option<Dt> {
        // `rdf:langString` is the RDF 1.1 name for the language-tagged part
        // of `rdf:PlainLiteral`; treat it as the plain-literal datatype.
        if iri == RDF_LANG_STRING {
            return Some(Dt::PlainLiteral);
        }
        map.iter().copied().find(|d| d.iri() == iri)
    }

    /// The datatype this one is derived from in the XSD (and OWL 2)
    /// hierarchy. `xsd:float`, `xsd:double` and `xsd:boolean` sit directly
    /// below `rdfs:Literal`. Value-space containment can be wider than this
    /// chain (an `xsd:unsignedByte` is an `xsd:short`): see [`Dt::is_within`].
    pub fn parent(self) -> Option<Dt> {
        Some(match self {
            Dt::Literal => return None,
            Dt::PlainLiteral | Dt::XmlLiteral | Dt::Real => Dt::Literal,
            Dt::HexBinary | Dt::Base64Binary | Dt::AnyUri | Dt::DateTime => Dt::Literal,
            Dt::Float | Dt::Double | Dt::Boolean => Dt::Literal,
            Dt::Rational => Dt::Real,
            Dt::Decimal => Dt::Rational,
            Dt::Integer => Dt::Decimal,
            Dt::NonNegativeInteger | Dt::NonPositiveInteger | Dt::Long => Dt::Integer,
            Dt::NegativeInteger => Dt::NonPositiveInteger,
            Dt::PositiveInteger | Dt::UnsignedLong => Dt::NonNegativeInteger,
            Dt::Int => Dt::Long,
            Dt::Short => Dt::Int,
            Dt::Byte => Dt::Short,
            Dt::UnsignedInt => Dt::UnsignedLong,
            Dt::UnsignedShort => Dt::UnsignedInt,
            Dt::UnsignedByte => Dt::UnsignedShort,
            Dt::String => Dt::PlainLiteral,
            Dt::NormalizedString => Dt::String,
            Dt::Token => Dt::NormalizedString,
            Dt::Name | Dt::NmToken | Dt::Language => Dt::Token,
            Dt::NcName => Dt::Name,
            Dt::DateTimeStamp => Dt::DateTime,
        })
    }

    /// The datatype directly below `rdfs:Literal` above this one (itself
    /// for a root; `rdfs:Literal` for `rdfs:Literal`).
    pub fn root(self) -> Dt {
        let mut cur = self;
        while let Some(p) = cur.parent() {
            if p == Dt::Literal {
                return cur;
            }
            cur = p;
        }
        cur
    }

    /// The bounds of `xsd:integer` or a type derived from it (`None` on a
    /// side: unbounded); `None` for every other datatype.
    fn int_range(self) -> Option<(Option<i128>, Option<i128>)> {
        Some(match self {
            Dt::Integer => (None, None),
            Dt::NonNegativeInteger => (Some(0), None),
            Dt::NonPositiveInteger => (None, Some(0)),
            Dt::NegativeInteger => (None, Some(-1)),
            Dt::PositiveInteger => (Some(1), None),
            Dt::Long => (Some(i64::MIN.into()), Some(i64::MAX.into())),
            Dt::Int => (Some(i32::MIN.into()), Some(i32::MAX.into())),
            Dt::Short => (Some(i16::MIN.into()), Some(i16::MAX.into())),
            Dt::Byte => (Some(i8::MIN.into()), Some(i8::MAX.into())),
            Dt::UnsignedLong => (Some(0), Some(u64::MAX.into())),
            Dt::UnsignedInt => (Some(0), Some(u32::MAX.into())),
            Dt::UnsignedShort => (Some(0), Some(u16::MAX.into())),
            Dt::UnsignedByte => (Some(0), Some(u8::MAX.into())),
            _ => return None,
        })
    }

    /// Whether this datatype's value space is inside `other`'s. This is
    /// value-space containment, not derivation: integer types compare by
    /// their bounds (`xsd:unsignedByte` is inside `xsd:short`), and every
    /// `xsd:language` tag is an NCName (so also a Name and an NMTOKEN).
    pub fn is_within(self, other: Dt) -> bool {
        if let (Some((alo, ahi)), Some((blo, bhi))) = (self.int_range(), other.int_range()) {
            let lo_ok = match (alo, blo) {
                (_, None) => true,
                (None, Some(_)) => false,
                (Some(a), Some(b)) => a >= b,
            };
            let hi_ok = match (ahi, bhi) {
                (_, None) => true,
                (None, Some(_)) => false,
                (Some(a), Some(b)) => a <= b,
            };
            return lo_ok && hi_ok;
        }
        if self == Dt::Language && matches!(other, Dt::NcName | Dt::Name | Dt::NmToken) {
            return true;
        }
        let mut cur = Some(self);
        while let Some(c) = cur {
            if c == other {
                return true;
            }
            cur = c.parent();
        }
        false
    }

    /// Whether the value spaces of the two datatypes are disjoint.
    /// Datatypes under different roots always are. Inside one root they are
    /// only for integer types with non-overlapping bounds
    /// (`xsd:negativeInteger` and `xsd:nonNegativeInteger`, say; but
    /// `xsd:nonPositiveInteger` and `xsd:nonNegativeInteger` share 0): the
    /// other numbers contain every integer, and every string type shares
    /// values with every other (`"a"` is a string, a token, a language tag,
    /// a Name, an NCName and an NMTOKEN).
    pub fn disjoint(self, other: Dt) -> bool {
        if self == Dt::Literal || other == Dt::Literal {
            return false;
        }
        if self.root() != other.root() {
            return true;
        }
        match (self.int_range(), other.int_range()) {
            (Some((alo, ahi)), Some((blo, bhi))) => {
                ahi.zip(blo).is_some_and(|(h, l)| h < l) || bhi.zip(alo).is_some_and(|(h, l)| h < l)
            }
            _ => false,
        }
    }
}

/// A data value, compared by value: `"1"^^xsd:integer` and `"1.0"^^xsd:decimal`
/// are the same number, `"a"` and `"a"^^xsd:token` the same string.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Value {
    /// A number of `owl:real` and below, canonical: a decimal without
    /// superfluous zeros, or a reduced fraction `n/d` when it has no finite
    /// decimal form.
    Number(String),
    /// An `xsd:double`, as its IEEE 754 bits: compared by identity, so `+0`
    /// and `−0` differ; every NaN is normalized to one canonical NaN.
    Double(u64),
    /// An `xsd:float`, as for [`Value::Double`].
    Float(u32),
    /// An `xsd:boolean`.
    Boolean(bool),
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
    /// A literal of a datatype outside the maps here (or, as a
    /// [`value_key`], an ill-typed one), compared by its term.
    Other {
        lexical: String,
        datatype: String,
    },
}

/// The value of a literal, or `None` when its lexical form is not in the
/// lexical space of its datatype (an ill-typed literal: OWL 2 makes an
/// ontology with one inconsistent). A datatype outside [`Dt::ANY`] gives
/// [`Value::Other`].
pub(crate) fn value_of(lexical: &str, datatype: &str, lang: Option<&str>) -> Option<Value> {
    if let Some(lang) = lang {
        return Some(Value::LangStr(
            lexical.to_string(),
            lang.to_ascii_lowercase(),
        ));
    }
    let Some(dt) = Dt::from_any_iri(datatype) else {
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
        Dt::Integer
        | Dt::NonNegativeInteger
        | Dt::NonPositiveInteger
        | Dt::PositiveInteger
        | Dt::NegativeInteger
        | Dt::Long
        | Dt::Int
        | Dt::Short
        | Dt::Byte
        | Dt::UnsignedLong
        | Dt::UnsignedInt
        | Dt::UnsignedShort
        | Dt::UnsignedByte => {
            let n = decimal(lexical, false)?;
            if !int_in_range(&n, dt.int_range()?) {
                return None;
            }
            Value::Number(n)
        }
        Dt::Double => Value::Double(double_bits(lexical)?),
        Dt::Float => Value::Float(float_bits(lexical)?),
        Dt::Boolean => Value::Boolean(boolean(lexical)?),
        Dt::String => Value::Str(lexical.to_string()),
        Dt::NormalizedString => Value::Str(lexical.replace(['\t', '\n', '\r'], " ")),
        Dt::Token | Dt::Name | Dt::NcName | Dt::NmToken | Dt::Language => {
            let s = collapse(lexical);
            let ok = match dt {
                Dt::Token => true,
                Dt::NmToken => is_nmtoken(&s),
                Dt::Name => is_name(&s),
                Dt::NcName => is_name(&s) && !s.contains(':'),
                _ => is_language(&s),
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

/// Every datatype of [`Dt::ANY`] whose value space contains `v`.
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
                    out.extend(
                        RL_INTEGERS
                            .into_iter()
                            .filter(|d| d.int_range().is_some_and(|r| int_in_range(n, r))),
                    );
                }
            }
        }
        Value::Double(_) => out.push(Dt::Double),
        Value::Float(_) => out.push(Dt::Float),
        Value::Boolean(_) => out.push(Dt::Boolean),
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
                    if is_language(s) {
                        out.push(Dt::Language);
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

/// Whether value `v` is in datatype `dt`; `None` when that is unknown: `v`
/// is of a datatype outside the maps here. Decided for every datatype of
/// [`Dt::ANY`] otherwise. (A [`Value::Other`] key of an ill-typed
/// `xsd:double`, `xsd:float` or `xsd:boolean` literal is still known to be
/// outside every datatype but `rdfs:Literal`.)
pub(crate) fn in_value_space(v: &Value, dt: Dt) -> Option<bool> {
    if dt == Dt::Literal {
        return Some(true);
    }
    if let Value::Other { datatype, .. } = v {
        let disjoint = matches!(
            datatype.strip_prefix(XSD_NS),
            Some("double" | "float" | "boolean")
        );
        return disjoint.then_some(false);
    }
    Some(datatypes_of(v).contains(&dt))
}

/// The value of `lit`, or `None` when it is ill-typed.
pub(crate) fn literal_value(lit: &Literal) -> Option<Value> {
    value_of(lit.value(), lit.datatype().as_str(), lit.language())
}

/// What two literals share exactly when they denote the same data value
/// (value equality is key equality): `"1"^^xsd:integer` and
/// `"1.0"^^xsd:decimal` have one key. An ill-typed literal, or one of a
/// datatype outside the maps, is only the same as itself.
pub(crate) fn value_key(lit: &Literal) -> Value {
    literal_value(lit).unwrap_or_else(|| Value::Other {
        lexical: lit.value().to_string(),
        datatype: lit.datatype().as_str().to_string(),
    })
}

// ─── Order ──────────────────────────────────────────────────────────────────

/// The order of two values of one value space, as the bound facets use it;
/// `None` when they are not ordered with each other or that cannot be told
/// here.
///
/// - Numbers (`owl:real` and below) are ordered exactly; a fraction without a
///   finite decimal form only while the cross-multiplication fits a `u128`.
/// - `xsd:double` with `xsd:double` and `xsd:float` with `xsd:float` in IEEE
///   754 order: NaN is unordered (`None`), and `−0` and `+0` are `Equal`
///   (different values, but neither below the other).
/// - Date-times: two zoned or two unzoned ones by their instants; a zoned
///   with an unzoned one only when they are more than 14 hours apart (XSD
///   1.1 §D.2.2: otherwise the order is indeterminate).
/// - Everything else (strings, booleans, binaries, values of different
///   families) has no order.
pub(crate) fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => compare_numbers(x, y),
        (Value::Double(x), Value::Double(y)) => f64::from_bits(*x).partial_cmp(&f64::from_bits(*y)),
        (Value::Float(x), Value::Float(y)) => f32::from_bits(*x).partial_cmp(&f32::from_bits(*y)),
        (
            Value::DateTime {
                canonical: x,
                zoned: zx,
            },
            Value::DateTime {
                canonical: y,
                zoned: zy,
            },
        ) => {
            let (px, py) = (canonical_date_time(x)?, canonical_date_time(y)?);
            if zx == zy {
                return Some(px.cmp(&py));
            }
            // One unzoned value stands for any instant within ±14 hours.
            let slack = chrono::Duration::hours(14);
            let unzoned = if *zx { py } else { px };
            let lo = unzoned.checked_sub_signed(slack)?;
            let hi = unzoned.checked_add_signed(slack)?;
            let zoned = if *zx { px } else { py };
            let zoned_vs_unzoned = if zoned < lo {
                Ordering::Less
            } else if zoned > hi {
                Ordering::Greater
            } else {
                return None;
            };
            Some(if *zx {
                zoned_vs_unzoned
            } else {
                zoned_vs_unzoned.reverse()
            })
        }
        _ => None,
    }
}

/// A canonical date-time of [`Value::DateTime`] back as a `NaiveDateTime`
/// (UTC when it was zoned).
fn canonical_date_time(c: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(c.strip_suffix('Z').unwrap_or(c), "%Y-%m-%dT%H:%M:%S%.f").ok()
}

/// Compare two canonical numbers of [`Value::Number`].
fn compare_numbers(x: &str, y: &str) -> Option<Ordering> {
    if x.contains('/') || y.contains('/') {
        let (xneg, xn, xd) = as_fraction(x)?;
        let (yneg, yn, yd) = as_fraction(y)?;
        return Some(match (xneg, yneg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (neg, _) => {
                let o = xn.checked_mul(yd)?.cmp(&yn.checked_mul(xd)?);
                if neg {
                    o.reverse()
                } else {
                    o
                }
            }
        });
    }
    // Canonical decimals: zero is unsigned, no leading zeros in the integer
    // part (but a lone `0`), no trailing zeros in the fraction.
    let (xneg, xm) = split_sign(x);
    let (yneg, ym) = split_sign(y);
    Some(match (xneg, yneg) {
        (false, true) => Ordering::Greater,
        (true, false) => Ordering::Less,
        (false, false) => compare_magnitudes(xm, ym),
        (true, true) => compare_magnitudes(ym, xm),
    })
}

fn split_sign(n: &str) -> (bool, &str) {
    match n.strip_prefix('-') {
        Some(m) => (true, m),
        None => (false, n),
    }
}

/// Compare two unsigned canonical decimals digit by digit.
fn compare_magnitudes(a: &str, b: &str) -> Ordering {
    let (ai, af) = a.split_once('.').unwrap_or((a, ""));
    let (bi, bf) = b.split_once('.').unwrap_or((b, ""));
    ai.len()
        .cmp(&bi.len())
        .then_with(|| ai.cmp(bi))
        .then_with(|| af.cmp(bf))
}

/// A canonical number as `(negative, numerator, denominator)`, when both fit
/// a `u128`.
fn as_fraction(n: &str) -> Option<(bool, u128, u128)> {
    let (neg, m) = split_sign(n);
    if let Some((p, q)) = m.split_once('/') {
        return Some((neg, p.parse().ok()?, q.parse().ok()?));
    }
    let (i, f) = m.split_once('.').unwrap_or((m, ""));
    let den = 10u128.checked_pow(u32::try_from(f.len()).ok()?)?;
    let num = format!("{i}{f}").parse::<u128>().ok()?;
    Some((neg, num, den))
}

// ─── Facets ─────────────────────────────────────────────────────────────────

/// A constraining facet of a datatype restriction (OWL 2 Structural
/// Specification §7.5, with the facets of the datatype map §4).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Facet {
    MinInclusive(Value),
    MaxInclusive(Value),
    MinExclusive(Value),
    MaxExclusive(Value),
    Length(u64),
    MinLength(u64),
    MaxLength(u64),
    /// An XSD regular expression, matched against the whole string.
    Pattern(String),
    /// An RFC 4647 basic language range, for `rdf:PlainLiteral`.
    LangRange(String),
}

/// The facet `facet_iri` (`xsd:minInclusive` … `xsd:pattern`,
/// `rdf:langRange`) with the value `value`; `None` for another IRI, an
/// ill-typed value, a length that is no non-negative integer, or a pattern
/// or language range that is no plain string.
pub(crate) fn facet_from(facet_iri: &str, value: &Literal) -> Option<Facet> {
    let v = literal_value(value)?;
    if facet_iri == RDF_LANG_RANGE {
        return match v {
            Value::Str(s) => Some(Facet::LangRange(s)),
            _ => None,
        };
    }
    let length = |v: &Value| match v {
        Value::Number(n) if n.bytes().all(|b| b.is_ascii_digit()) => n.parse::<u64>().ok(),
        _ => None,
    };
    Some(match facet_iri.strip_prefix(XSD_NS)? {
        "minInclusive" => Facet::MinInclusive(v),
        "maxInclusive" => Facet::MaxInclusive(v),
        "minExclusive" => Facet::MinExclusive(v),
        "maxExclusive" => Facet::MaxExclusive(v),
        "length" => Facet::Length(length(&v)?),
        "minLength" => Facet::MinLength(length(&v)?),
        "maxLength" => Facet::MaxLength(length(&v)?),
        "pattern" => match v {
            Value::Str(s) => Facet::Pattern(s),
            _ => return None,
        },
        _ => return None,
    })
}

/// Whether `v` is in the restriction of `base` by `facets`: `Some(false)`
/// when it is outside `base` or fails a facet, `Some(true)` when it is in
/// `base` and meets every facet, `None` when neither can be told (a bound
/// [`compare`] cannot decide, a length or pattern on a value that has none
/// here, an XSD pattern this module cannot translate). A `false` anywhere
/// wins over an unknown.
pub(crate) fn in_restriction(v: &Value, base: Dt, facets: &[Facet]) -> Option<bool> {
    let mut known = match in_value_space(v, base) {
        Some(false) => return Some(false),
        Some(true) => true,
        None => false,
    };
    for f in facets {
        match facet_holds(v, base, f) {
            Some(false) => return Some(false),
            Some(true) => {}
            None => known = false,
        }
    }
    known.then_some(true)
}

fn facet_holds(v: &Value, base: Dt, f: &Facet) -> Option<bool> {
    match f {
        Facet::MinInclusive(b) => bound(v, b, |o| o != Ordering::Less),
        Facet::MaxInclusive(b) => bound(v, b, |o| o != Ordering::Greater),
        Facet::MinExclusive(b) => bound(v, b, |o| o == Ordering::Greater),
        Facet::MaxExclusive(b) => bound(v, b, |o| o == Ordering::Less),
        Facet::Length(n) => length_of(v).map(|l| l == *n),
        Facet::MinLength(n) => length_of(v).map(|l| l >= *n),
        Facet::MaxLength(n) => length_of(v).map(|l| l <= *n),
        Facet::Pattern(p) => match v {
            Value::Str(s) | Value::LangStr(s, _) | Value::Uri(s) => matches_pattern(p, s),
            _ => None,
        },
        Facet::LangRange(r) => match v {
            Value::LangStr(_, tag) => Some(lang_matches(tag, r)),
            Value::Other { .. } => None,
            // A plain-literal value without a tag matches no range.
            _ if base.is_within(Dt::PlainLiteral) => Some(false),
            _ => None,
        },
    }
}

/// A bound facet: `holds` on the order of `v` against the bound. A NaN is
/// in no bounded range (it is incomparable with every value), so a NaN on
/// either side of a floating-point bound is a decided `false`.
fn bound(v: &Value, b: &Value, holds: impl Fn(Ordering) -> bool) -> Option<bool> {
    let nan = match (v, b) {
        (Value::Double(x), Value::Double(y)) => {
            f64::from_bits(*x).is_nan() || f64::from_bits(*y).is_nan()
        }
        (Value::Float(x), Value::Float(y)) => {
            f32::from_bits(*x).is_nan() || f32::from_bits(*y).is_nan()
        }
        _ => false,
    };
    if nan {
        return Some(false);
    }
    compare(v, b).map(holds)
}

/// The length a length facet measures: characters of a string (the text of
/// a language-tagged one) or URI, octets of a binary.
fn length_of(v: &Value) -> Option<u64> {
    match v {
        Value::Str(s) | Value::LangStr(s, _) | Value::Uri(s) => {
            u64::try_from(s.chars().count()).ok()
        }
        Value::Hex(b) | Value::Base64(b) => u64::try_from(b.len()).ok(),
        _ => None,
    }
}

/// RFC 4647 §3.3.1 basic filtering: `range` is `*`, the tag, or a prefix of
/// the tag ending at a `-`; case-insensitively.
fn lang_matches(tag: &str, range: &str) -> bool {
    if range == "*" {
        return !tag.is_empty();
    }
    let (t, r) = (tag.to_ascii_lowercase(), range.to_ascii_lowercase());
    !r.is_empty()
        && (t == r || (t.len() > r.len() && t.starts_with(&r) && t.as_bytes()[r.len()] == b'-'))
}

/// Whether the whole of `s` matches the XSD regular expression `pattern`;
/// `None` when the pattern is invalid or uses a construct not translated to
/// the `regex` crate's syntax.
fn matches_pattern(pattern: &str, s: &str) -> Option<bool> {
    thread_local! {
        static CACHE: RefCell<HashMap<String, Option<regex::Regex>>> =
            RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.len() >= 256 && !cache.contains_key(pattern) {
            cache.clear();
        }
        let re = cache.entry(pattern.to_string()).or_insert_with(|| {
            xsd_regex(pattern).and_then(|t| regex::Regex::new(&format!("^(?:{t})$")).ok())
        });
        re.as_ref().map(|re| re.is_match(s))
    })
}

/// An XSD regular expression (XSD 1.1 Part 2, Appendix G) in the `regex`
/// crate's syntax, unanchored; `None` for what is not translated: the
/// name escapes `\i \I \c \C`, character-class subtraction, escapes XSD
/// does not define, and the crate's own extensions (`(?…)`, `&&`, `~~`,
/// `--`). XSD has no anchors (`^` and `$` are ordinary characters), its `.`
/// excludes `\r` as well as `\n`, and its `\s` and `\w` differ from the
/// crate's Unicode classes; those are rewritten.
fn xsd_regex(p: &str) -> Option<String> {
    let mut out = String::with_capacity(p.len() + 8);
    let mut chars = p.chars().peekable();
    let mut in_class = false;
    let mut class_start = false;
    while let Some(c) = chars.next() {
        let at_class_start = std::mem::take(&mut class_start);
        match c {
            '\\' => match chars.next()? {
                e @ ('n' | 'r' | 't' | '\\' | '|' | '.' | '-' | '^' | '?' | '*' | '+' | '{'
                | '}' | '(' | ')' | '[' | ']' | 'd' | 'D' | 'p' | 'P') => {
                    // `\p{..}`: the braces pass through below; a block
                    // escape (`\p{IsBasicLatin}`) fails to compile.
                    out.push('\\');
                    out.push(e);
                }
                's' => out.push_str("[ \\t\\n\\r]"),
                'S' => out.push_str("[^ \\t\\n\\r]"),
                'w' => out.push_str("[^\\p{P}\\p{Z}\\p{C}]"),
                'W' => out.push_str("[\\p{P}\\p{Z}\\p{C}]"),
                _ => return None,
            },
            '[' if in_class => return None, // only legal as subtraction `-[`
            '[' => {
                in_class = true;
                class_start = true;
                out.push('[');
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            '^' if in_class && at_class_start => out.push('^'),
            '-' if in_class && matches!(chars.peek(), Some(&('[' | '-'))) => return None,
            '&' | '~' if in_class => {
                out.push('\\');
                out.push(c);
            }
            '.' if !in_class => out.push_str("[^\\n\\r]"),
            '^' | '$' => {
                out.push('\\');
                out.push(c);
            }
            '(' if !in_class && chars.peek() == Some(&'?') => return None,
            _ => out.push(c),
        }
    }
    (!in_class).then_some(out)
}

// ─── Lexical forms ──────────────────────────────────────────────────────────

/// Whether the canonical integer `n` lies within `(min, max)`.
fn int_in_range(n: &str, (min, max): (Option<i128>, Option<i128>)) -> bool {
    match n.parse::<i128>() {
        Ok(x) => min.is_none_or(|m| x >= m) && max.is_none_or(|m| x <= m),
        // Beyond i128 only the unbounded side can still hold it.
        Err(_) => {
            let neg = n.starts_with('-');
            (neg && min.is_none()) || (!neg && max.is_none())
        }
    }
}

fn is_xsd_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
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

/// The `xsd:language` pattern `[a-zA-Z]{1,8}(-[a-zA-Z0-9]{1,8})*`.
fn is_language(s: &str) -> bool {
    s.split('-').enumerate().all(|(i, part)| {
        (1..=8).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit()))
    })
}

/// An `xsd:boolean` lexical form.
fn boolean(lexical: &str) -> Option<bool> {
    match lexical.trim_matches(is_xsd_space) {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// A floating-point lexical form, checked against the XSD 1.1 grammar of
/// `xsd:double` and `xsd:float` (they share it).
enum FloatLex<'a> {
    Inf {
        negative: bool,
    },
    NaN,
    /// A finite numeral, in a syntax Rust's float parser reads.
    Finite(&'a str),
}

fn float_lexical(lexical: &str) -> Option<FloatLex<'_>> {
    let s = lexical.trim_matches(is_xsd_space);
    match s {
        "INF" | "+INF" => return Some(FloatLex::Inf { negative: false }),
        "-INF" => return Some(FloatLex::Inf { negative: true }),
        "NaN" => return Some(FloatLex::NaN),
        _ => {}
    }
    let b = s.as_bytes();
    let digits_from = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        i
    };
    let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let int_end = digits_from(i);
    let mut mantissa_digits = int_end - i;
    i = int_end;
    if b.get(i) == Some(&b'.') {
        let frac_end = digits_from(i + 1);
        mantissa_digits += frac_end - (i + 1);
        i = frac_end;
    }
    if mantissa_digits == 0 {
        return None;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let exp_end = digits_from(i);
        if exp_end == i {
            return None;
        }
        i = exp_end;
    }
    (i == b.len()).then_some(FloatLex::Finite(s))
}

/// The bits of an `xsd:double` value (NaN canonical); too large a
/// magnitude rounds to an infinity, as XSD 1.1 maps it.
fn double_bits(lexical: &str) -> Option<u64> {
    let x: f64 = match float_lexical(lexical)? {
        FloatLex::Inf { negative: false } => f64::INFINITY,
        FloatLex::Inf { negative: true } => f64::NEG_INFINITY,
        FloatLex::NaN => f64::NAN,
        FloatLex::Finite(s) => s.parse().ok()?,
    };
    Some(if x.is_nan() {
        f64::NAN.to_bits()
    } else {
        x.to_bits()
    })
}

/// The bits of an `xsd:float` value, as [`double_bits`] (rounded once,
/// straight from the decimal numeral).
fn float_bits(lexical: &str) -> Option<u32> {
    let x: f32 = match float_lexical(lexical)? {
        FloatLex::Inf { negative: false } => f32::INFINITY,
        FloatLex::Inf { negative: true } => f32::NEG_INFINITY,
        FloatLex::NaN => f32::NAN,
        FloatLex::Finite(s) => s.parse().ok()?,
    };
    Some(if x.is_nan() {
        f32::NAN.to_bits()
    } else {
        x.to_bits()
    })
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

    fn val(lex: &str, dt: &str) -> Value {
        v(lex, dt).unwrap_or_else(|| panic!("{lex:?}^^xsd:{dt} is ill-typed"))
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

    fn lit(lex: &str, dt: &str) -> Literal {
        Literal::new_typed_literal(
            lex,
            oxigraph::model::NamedNode::new_unchecked(format!("{XSD}{dt}")),
        )
    }

    #[test]
    fn membership_is_decided_on_the_value() {
        let five = v("5", "integer").unwrap();
        let minus = v("-5", "integer").unwrap();
        assert_eq!(in_value_space(&five, Dt::NonNegativeInteger), Some(true));
        assert_eq!(in_value_space(&minus, Dt::NonNegativeInteger), Some(false));
        let half = v("1.5", "decimal").unwrap();
        assert_eq!(in_value_space(&half, Dt::Integer), Some(false));
        assert_eq!(
            in_value_space(&v("1.0", "decimal").unwrap(), Dt::Integer),
            Some(true)
        );
        assert_eq!(
            in_value_space(&v("a", "string").unwrap(), Dt::Real),
            Some(false)
        );
        // Outside the EL/QL map: known disjoint, or unknown.
        let dbl = v("1.0E0", "double").unwrap();
        assert_eq!(in_value_space(&dbl, Dt::Decimal), Some(false));
        assert_eq!(in_value_space(&dbl, Dt::Literal), Some(true));
        let date = v("2020-01-01", "date").unwrap();
        assert_eq!(in_value_space(&date, Dt::String), None);
    }

    #[test]
    fn derived_types_read_by_value() {
        assert_eq!(v("7", "byte"), v("7", "integer"));
        assert!(v("200", "byte").is_none());
        assert!(v("-1", "unsignedInt").is_none());
        assert!(v("0", "positiveInteger").is_none());
        assert!(v(
            "-99999999999999999999999999999999999999999",
            "negativeInteger"
        )
        .is_some());
        assert_eq!(v("en-GB", "language"), v("en-GB", "string"));
        assert!(v("english language", "language").is_none());
    }

    #[test]
    fn value_keys_cross_lexical_forms() {
        let same = |a: Literal, b: Literal| value_key(&a) == value_key(&b);
        assert!(same(lit("1", "integer"), lit("1.0", "decimal")));
        assert!(same(lit("abc", "string"), lit(" abc ", "token")));
        assert!(!same(lit("1", "integer"), lit("1", "string")));
        assert!(!same(lit("1", "integer"), lit("1.0E0", "double")));
        assert!(same(lit("x", "integer"), lit("x", "integer")));
        assert!(!same(lit("x", "integer"), lit("y", "integer")));
        // Floating-point and boolean literals now compare by value too.
        assert!(same(lit("1.0E0", "double"), lit("1", "double")));
        assert!(!same(lit("1", "double"), lit("1", "float")));
        assert!(same(lit("true", "boolean"), lit("1", "boolean")));
    }

    #[test]
    fn disjointness_follows_the_roots() {
        assert!(Dt::Integer.disjoint(Dt::String));
        assert!(Dt::DateTimeStamp.disjoint(Dt::AnyUri));
        assert!(!Dt::Integer.disjoint(Dt::Real));
        assert!(!Dt::NcName.disjoint(Dt::NmToken));
        assert!(!Dt::Literal.disjoint(Dt::HexBinary));
        assert!(Dt::NonNegativeInteger.is_within(Dt::Rational));
        assert!(!Dt::Decimal.is_within(Dt::Integer));
    }

    #[test]
    fn el_ql_answers_are_unchanged() {
        // On the EL/QL map, disjointness is still "different roots", and
        // containment still the derivation chain.
        for a in Dt::ALL {
            for b in Dt::ALL {
                let old = a != Dt::Literal && b != Dt::Literal && a.root() != b.root();
                assert_eq!(a.disjoint(b), old, "{a:?} {b:?}");
                let mut chain = false;
                let mut cur = Some(a);
                while let Some(c) = cur {
                    chain |= c == b;
                    cur = c.parent();
                }
                assert_eq!(a.is_within(b), chain, "{a:?} {b:?}");
            }
        }
    }

    #[test]
    fn hierarchy_reaches_the_roots() {
        for d in Dt::ANY {
            let mut cur = d;
            while let Some(p) = cur.parent() {
                cur = p;
            }
            assert_eq!(cur, Dt::Literal, "{d:?}");
        }
    }

    #[test]
    fn the_maps_by_iri() {
        assert_eq!(Dt::ALL.len(), 19);
        let count =
            |f: fn(&str) -> Option<Dt>| Dt::ANY.iter().filter(|d| f(d.iri()).is_some()).count();
        assert_eq!(count(Dt::from_iri), 19);
        assert_eq!(count(Dt::from_rl_iri), 32);
        assert_eq!(count(Dt::from_any_iri), 34);
        for d in Dt::ALL {
            assert_eq!(Dt::from_iri(d.iri()), Some(d));
        }
        for d in Dt::RL {
            assert_eq!(Dt::from_rl_iri(d.iri()), Some(d));
        }
        for d in Dt::ANY {
            assert_eq!(Dt::from_any_iri(d.iri()), Some(d));
        }
        assert_eq!(Dt::from_iri(Dt::Double.iri()), None);
        assert_eq!(Dt::from_rl_iri(Dt::Real.iri()), None);
        assert_eq!(Dt::from_rl_iri(Dt::Rational.iri()), None);
        // The RL map lists each datatype once.
        let mut rl = Dt::RL.to_vec();
        rl.sort();
        rl.dedup();
        assert_eq!(rl.len(), 32);
    }

    #[test]
    fn derived_integer_membership() {
        let has = |n: &str, d: Dt| datatypes_of(&val(n, "integer")).contains(&d);
        for d in [
            Dt::NonNegativeInteger,
            Dt::PositiveInteger,
            Dt::Long,
            Dt::Int,
            Dt::Short,
            Dt::Byte,
            Dt::UnsignedLong,
            Dt::UnsignedInt,
            Dt::UnsignedShort,
            Dt::UnsignedByte,
        ] {
            assert!(has("5", d), "5 {d:?}");
        }
        assert!(!has("5", Dt::NonPositiveInteger) && !has("5", Dt::NegativeInteger));
        for d in [
            Dt::NonPositiveInteger,
            Dt::NegativeInteger,
            Dt::Long,
            Dt::Int,
            Dt::Short,
            Dt::Byte,
        ] {
            assert!(has("-5", d), "-5 {d:?}");
        }
        for d in [
            Dt::NonNegativeInteger,
            Dt::PositiveInteger,
            Dt::UnsignedLong,
            Dt::UnsignedByte,
        ] {
            assert!(!has("-5", d), "-5 {d:?}");
        }
        assert!(!has("300", Dt::Byte) && !has("300", Dt::UnsignedByte));
        assert!(has("300", Dt::Short) && has("300", Dt::UnsignedShort));
        assert!(has("0", Dt::NonPositiveInteger) && has("0", Dt::NonNegativeInteger));
        assert!(!has("0", Dt::PositiveInteger) && !has("0", Dt::NegativeInteger));
        // 2^64: past unsignedLong and long, still a positive integer.
        assert!(has("18446744073709551616", Dt::PositiveInteger));
        assert!(!has("18446744073709551616", Dt::UnsignedLong));
        assert!(!has("18446744073709551616", Dt::Long));
        assert!(has("18446744073709551615", Dt::UnsignedLong));
        // Non-integers are in none of them.
        let half = datatypes_of(&val("0.5", "decimal"));
        assert!(RL_INTEGERS.iter().all(|d| !half.contains(d)));
        // Every lexical check of a derived type agrees with membership.
        assert!(v("128", "byte").is_none() && v("-128", "byte").is_some());
        assert!(v("65535", "unsignedShort").is_some() && v("65536", "unsignedShort").is_none());
        assert!(v("0", "nonPositiveInteger").is_some() && v("1", "nonPositiveInteger").is_none());
        for d in Dt::ANY {
            assert!(in_value_space(&val("5", "integer"), d).is_some(), "{d:?}");
        }
    }

    #[test]
    fn derived_integer_disjointness() {
        assert!(Dt::NegativeInteger.disjoint(Dt::NonNegativeInteger));
        assert!(Dt::NegativeInteger.disjoint(Dt::PositiveInteger));
        assert!(Dt::NegativeInteger.disjoint(Dt::UnsignedByte));
        assert!(Dt::PositiveInteger.disjoint(Dt::NonPositiveInteger));
        assert!(!Dt::NonPositiveInteger.disjoint(Dt::NonNegativeInteger));
        assert!(!Dt::NegativeInteger.disjoint(Dt::Byte));
        assert!(!Dt::Byte.disjoint(Dt::UnsignedLong));
        assert!(!Dt::PositiveInteger.disjoint(Dt::Decimal));
        assert!(Dt::Double.disjoint(Dt::Decimal));
        assert!(Dt::Float.disjoint(Dt::Double));
        assert!(Dt::Double.disjoint(Dt::Real));
        assert!(Dt::Boolean.disjoint(Dt::String));
        assert!(Dt::Boolean.disjoint(Dt::Integer));
        assert!(!Dt::Language.disjoint(Dt::NcName));
        assert!(!Dt::Float.disjoint(Dt::Literal));
        assert!(Dt::UnsignedByte.is_within(Dt::Short));
        assert!(!Dt::Byte.is_within(Dt::UnsignedByte));
        assert!(Dt::Int.is_within(Dt::Long) && !Dt::Long.is_within(Dt::Int));
        assert!(Dt::PositiveInteger.is_within(Dt::NonNegativeInteger));
        assert!(Dt::UnsignedLong.is_within(Dt::NonNegativeInteger));
        assert!(Dt::Byte.is_within(Dt::Decimal) && Dt::Byte.is_within(Dt::Real));
        assert!(Dt::Language.is_within(Dt::NcName) && Dt::Language.is_within(Dt::Token));
        assert!(!Dt::Language.is_within(Dt::Integer));
        // Against sample values: a shared value means not disjoint, and
        // containment never loses one.
        let samples: Vec<Value> = [
            ("0", "integer"),
            ("5", "integer"),
            ("-5", "integer"),
            ("300", "integer"),
            ("-300", "integer"),
            ("70000", "integer"),
            ("3000000000", "integer"),
            ("-3000000000", "integer"),
            ("18446744073709551615", "integer"),
            ("99999999999999999999999", "integer"),
            ("-99999999999999999999999", "integer"),
            ("0.5", "decimal"),
            ("1", "double"),
            ("1", "float"),
            ("true", "boolean"),
            ("a", "string"),
            ("en-GB", "string"),
            ("a b", "string"),
            ("a:b", "string"),
            ("2020-01-01T00:00:00Z", "dateTime"),
            ("2020-01-01T00:00:00", "dateTime"),
            ("0F", "hexBinary"),
            ("AA==", "base64Binary"),
            ("http://example.org/", "anyURI"),
        ]
        .iter()
        .map(|(l, d)| val(l, d))
        .collect();
        for a in Dt::ANY {
            assert!(!a.disjoint(a), "{a:?}");
            for b in Dt::ANY {
                assert_eq!(a.disjoint(b), b.disjoint(a), "{a:?} {b:?}");
                for s in &samples {
                    let dts = datatypes_of(s);
                    if dts.contains(&a) && dts.contains(&b) {
                        assert!(!a.disjoint(b), "{a:?} {b:?} share {s:?}");
                    }
                    if a.is_within(b) && dts.contains(&a) {
                        assert!(dts.contains(&b), "{a:?} within {b:?}, {s:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn floating_point_lexical_forms() {
        assert_eq!(v("1.5E0", "double"), Some(Value::Double(1.5f64.to_bits())));
        assert_eq!(v(" 15e-1 ", "double"), v("1.5", "double"));
        assert_eq!(v("1.", "double"), v("1", "double"));
        assert_eq!(v(".5", "double"), v("0.5", "double"));
        assert_eq!(v("+1.5E+0", "double"), v("1.5", "double"));
        assert_eq!(
            v("INF", "double"),
            Some(Value::Double(f64::INFINITY.to_bits()))
        );
        assert_eq!(v("+INF", "double"), v("INF", "double"));
        assert_eq!(
            v("-INF", "double"),
            Some(Value::Double(f64::NEG_INFINITY.to_bits()))
        );
        assert_eq!(v("1E400", "double"), v("INF", "double"));
        // One NaN; −0 and +0 are different values.
        assert_eq!(v("NaN", "double"), v("NaN", "double"));
        assert_ne!(v("-0", "double"), v("0", "double"));
        assert_eq!(v("0.0E0", "double"), v("0", "double"));
        assert_eq!(
            v("-0.0", "double"),
            Some(Value::Double((-0.0f64).to_bits()))
        );
        for bad in [
            ".", "", "e5", "1e", "1E+", "1.5 E0", "inf", "nan", "Infinity", "1,5", "--1", "0x10",
        ] {
            assert!(v(bad, "double").is_none(), "{bad:?}");
            assert!(v(bad, "float").is_none(), "{bad:?}");
        }
        assert_eq!(v("0.1", "float"), Some(Value::Float(0.1f32.to_bits())));
        assert_eq!(v("NaN", "float"), Some(Value::Float(f32::NAN.to_bits())));
        assert_ne!(v("-0", "float"), v("0", "float"));
        assert_eq!(v("1E39", "float"), v("INF", "float"));
        // float and double are disjoint, and both disjoint from the numbers.
        let f = val("1", "float");
        let d = val("1", "double");
        assert_ne!(f, d);
        assert_eq!(in_value_space(&f, Dt::Float), Some(true));
        assert_eq!(in_value_space(&f, Dt::Double), Some(false));
        assert_eq!(in_value_space(&d, Dt::Float), Some(false));
        assert_eq!(in_value_space(&d, Dt::Decimal), Some(false));
        assert_eq!(datatypes_of(&d), vec![Dt::Literal, Dt::Double]);
        assert_ne!(d, val("1", "integer"));
    }

    #[test]
    fn boolean_lexical_forms() {
        assert_eq!(v("true", "boolean"), Some(Value::Boolean(true)));
        assert_eq!(v("1", "boolean"), Some(Value::Boolean(true)));
        assert_eq!(v(" false\n", "boolean"), Some(Value::Boolean(false)));
        assert_eq!(v("0", "boolean"), Some(Value::Boolean(false)));
        for bad in ["TRUE", "yes", "", "01", "t"] {
            assert!(v(bad, "boolean").is_none(), "{bad:?}");
        }
        let t = val("true", "boolean");
        assert_eq!(datatypes_of(&t), vec![Dt::Literal, Dt::Boolean]);
        assert_eq!(in_value_space(&t, Dt::String), Some(false));
        assert_eq!(in_value_space(&t, Dt::Integer), Some(false));
    }

    #[test]
    fn language_membership() {
        let is_lang = |s: &str| datatypes_of(&Value::Str(s.into())).contains(&Dt::Language);
        assert!(is_lang("en"));
        assert!(is_lang("en-GB"));
        assert!(is_lang("x-1"));
        assert!(is_lang("zh-Hant-TW"));
        assert!(!is_lang("english language"));
        assert!(!is_lang("en_GB"));
        assert!(!is_lang("toolongtag"));
        assert!(!is_lang("1en"));
        assert!(!is_lang("en-123456789"));
        assert!(!is_lang(" en"));
        assert!(!is_lang(""));
        assert!(!is_lang("en-"));
        assert!(datatypes_of(&val(" en-GB ", "language")).contains(&Dt::Language));
        assert_eq!(
            in_value_space(&val("en", "string"), Dt::Language),
            Some(true)
        );
        // A language-tagged string is a plain literal, never an xsd:language.
        assert_eq!(
            in_value_space(&Value::LangStr("en".into(), "en".into()), Dt::Language),
            Some(false)
        );
    }

    #[test]
    fn order_of_values() {
        use Ordering::*;
        let n = |s: &str| Value::Number(s.into());
        let c = |a: &Value, b: &Value| compare(a, b);
        assert_eq!(c(&n("1"), &n("2")), Some(Less));
        assert_eq!(c(&n("-2"), &n("-1")), Some(Less));
        assert_eq!(c(&n("1.5"), &n("1.25")), Some(Greater));
        assert_eq!(c(&n("0"), &n("-0.5")), Some(Greater));
        assert_eq!(c(&n("10"), &n("9")), Some(Greater));
        assert_eq!(c(&n("-10"), &n("-9")), Some(Less));
        assert_eq!(c(&n("0.05"), &n("0.5")), Some(Less));
        assert_eq!(c(&n("2.5"), &n("2.5")), Some(Equal));
        assert_eq!(
            c(&n("123456789012345678901234567890123456789012"), &n("9")),
            Some(Greater)
        );
        assert_eq!(c(&n("1/3"), &n("0.5")), Some(Less));
        assert_eq!(c(&n("1/3"), &n("0.3")), Some(Greater));
        assert_eq!(c(&n("1/3"), &n("1/3")), Some(Equal));
        assert_eq!(c(&n("1/3"), &n("2/7")), Some(Greater));
        assert_eq!(c(&n("-1/3"), &n("1/3")), Some(Less));
        assert_eq!(c(&n("-1/3"), &n("-2/7")), Some(Less));
        let d = |s: &str| val(s, "double");
        assert_eq!(c(&d("1"), &d("2")), Some(Less));
        assert_eq!(c(&d("-INF"), &d("-1E300")), Some(Less));
        assert_eq!(c(&d("-0"), &d("0")), Some(Equal));
        assert_eq!(c(&d("NaN"), &d("1")), None);
        assert_eq!(c(&d("NaN"), &d("NaN")), None);
        assert_eq!(c(&val("1", "float"), &val("2", "float")), Some(Less));
        // Different value spaces are not ordered with each other.
        assert_eq!(c(&val("1", "float"), &d("2")), None);
        assert_eq!(c(&n("1"), &d("2")), None);
        assert_eq!(c(&val("a", "string"), &val("b", "string")), None);
        assert_eq!(c(&val("true", "boolean"), &val("false", "boolean")), None);
        let t = |s: &str| val(s, "dateTime");
        assert_eq!(
            c(&t("2020-01-01T00:00:00Z"), &t("2020-01-01T00:00:00.5Z")),
            Some(Less)
        );
        assert_eq!(
            c(&t("2020-01-01T02:00:00+01:00"), &t("2020-01-01T01:00:00Z")),
            Some(Equal)
        );
        assert_eq!(
            c(&t("2020-01-02T00:00:00"), &t("2020-01-01T23:00:00")),
            Some(Greater)
        );
        // Zoned against unzoned: only beyond ±14 hours.
        assert_eq!(
            c(&t("2020-01-01T00:00:00Z"), &t("2020-01-01T10:00:00")),
            None
        );
        assert_eq!(
            c(&t("2020-01-01T00:00:00Z"), &t("2020-01-01T15:00:00")),
            Some(Less)
        );
        assert_eq!(
            c(&t("2020-01-01T15:00:00"), &t("2020-01-01T00:00:00Z")),
            Some(Greater)
        );
    }

    fn rdf_lit(lex: &str) -> Literal {
        Literal::new_simple_literal(lex)
    }

    #[test]
    fn facets_from_their_iris() {
        let f = |name: &str, l: &Literal| facet_from(&format!("{XSD}{name}"), l);
        let five = Value::Number("5".into());
        assert_eq!(
            f("minInclusive", &lit("5", "integer")),
            Some(Facet::MinInclusive(five.clone()))
        );
        assert_eq!(
            f("maxInclusive", &lit("5.0", "decimal")),
            Some(Facet::MaxInclusive(five.clone()))
        );
        assert_eq!(
            f("minExclusive", &lit("5", "integer")),
            Some(Facet::MinExclusive(five.clone()))
        );
        assert_eq!(
            f("maxExclusive", &lit("5", "integer")),
            Some(Facet::MaxExclusive(five))
        );
        assert_eq!(f("length", &lit("3", "integer")), Some(Facet::Length(3)));
        assert_eq!(
            f("minLength", &lit("0", "nonNegativeInteger")),
            Some(Facet::MinLength(0))
        );
        assert_eq!(
            f("maxLength", &lit("8", "integer")),
            Some(Facet::MaxLength(8))
        );
        assert_eq!(f("length", &lit("-1", "integer")), None);
        assert_eq!(f("length", &lit("1.5", "decimal")), None);
        assert_eq!(f("length", &lit("3", "string")), None);
        assert_eq!(f("minInclusive", &lit("x", "integer")), None);
        assert_eq!(
            f("pattern", &rdf_lit("[a-z]+")),
            Some(Facet::Pattern("[a-z]+".into()))
        );
        assert_eq!(f("pattern", &lit("1", "integer")), None);
        assert_eq!(f("totalDigits", &lit("3", "integer")), None);
        assert_eq!(
            facet_from(RDF_LANG_RANGE, &rdf_lit("en")),
            Some(Facet::LangRange("en".into()))
        );
        assert_eq!(facet_from("http://example.org/facet", &rdf_lit("en")), None);
    }

    #[test]
    fn bound_facets() {
        let n = |s: &str| Value::Number(s.into());
        let r = |x: &Value, fs: &[Facet]| in_restriction(x, Dt::Integer, fs);
        let five = n("5");
        assert_eq!(
            r(
                &five,
                &[Facet::MinInclusive(n("1")), Facet::MaxInclusive(n("5"))]
            ),
            Some(true)
        );
        assert_eq!(
            r(
                &five,
                &[Facet::MinInclusive(n("1")), Facet::MaxExclusive(n("5"))]
            ),
            Some(false)
        );
        assert_eq!(r(&five, &[Facet::MinExclusive(n("4.5"))]), Some(true));
        assert_eq!(r(&five, &[Facet::MinExclusive(n("5"))]), Some(false));
        assert_eq!(r(&five, &[Facet::MaxInclusive(n("1/3"))]), Some(false));
        assert_eq!(r(&five, &[]), Some(true));
        // Outside the base: false, whatever the facets.
        assert_eq!(r(&n("5.5"), &[Facet::MinInclusive(n("1"))]), Some(false));
        assert_eq!(r(&val("a", "string"), &[]), Some(false));
        // Three-valued: an undecidable bound is unknown, a false one wins.
        let dbl = Value::Double(1.0f64.to_bits());
        assert_eq!(r(&five, &[Facet::MinInclusive(dbl.clone())]), None);
        assert_eq!(
            r(
                &five,
                &[
                    Facet::MinInclusive(dbl.clone()),
                    Facet::MaxInclusive(n("4"))
                ]
            ),
            Some(false)
        );
        assert_eq!(
            r(
                &five,
                &[Facet::MinInclusive(dbl), Facet::MaxInclusive(n("6"))]
            ),
            None
        );
        // Floating point: −0 meets a bound of +0; NaN meets no bound.
        let d = |s: &str| val(s, "double");
        let rd = |x: &Value, fs: &[Facet]| in_restriction(x, Dt::Double, fs);
        assert_eq!(rd(&d("-0"), &[Facet::MinInclusive(d("0"))]), Some(true));
        assert_eq!(rd(&d("-0"), &[Facet::MinExclusive(d("0"))]), Some(false));
        assert_eq!(
            rd(&d("NaN"), &[Facet::MinInclusive(d("-INF"))]),
            Some(false)
        );
        assert_eq!(rd(&d("1"), &[Facet::MaxInclusive(d("NaN"))]), Some(false));
        assert_eq!(
            rd(&d("INF"), &[Facet::MinInclusive(d("1E308"))]),
            Some(true)
        );
        // Date-times.
        let t = |s: &str| val(s, "dateTime");
        assert_eq!(
            in_restriction(
                &t("2020-06-01T00:00:00Z"),
                Dt::DateTimeStamp,
                &[Facet::MinInclusive(t("2020-01-01T00:00:00Z"))]
            ),
            Some(true)
        );
        // A value of a datatype outside the maps: unknown.
        let date = val("2020-01-01", "date");
        assert_eq!(in_restriction(&date, Dt::DateTime, &[]), None);
    }

    #[test]
    fn length_facets() {
        let s = Value::Str("héllo".into());
        assert_eq!(
            in_restriction(&s, Dt::String, &[Facet::Length(5)]),
            Some(true)
        );
        assert_eq!(
            in_restriction(&s, Dt::String, &[Facet::Length(6)]),
            Some(false)
        );
        assert_eq!(
            in_restriction(&s, Dt::String, &[Facet::MinLength(1), Facet::MaxLength(5)]),
            Some(true)
        );
        assert_eq!(
            in_restriction(&s, Dt::String, &[Facet::MaxLength(4)]),
            Some(false)
        );
        let hex = val("0FB8", "hexBinary");
        assert_eq!(
            in_restriction(&hex, Dt::HexBinary, &[Facet::Length(2)]),
            Some(true)
        );
        let b64 = val("AAEC", "base64Binary");
        assert_eq!(
            in_restriction(&b64, Dt::Base64Binary, &[Facet::Length(3)]),
            Some(true)
        );
        let uri = val("http://example.org/", "anyURI");
        assert_eq!(
            in_restriction(&uri, Dt::AnyUri, &[Facet::Length(19)]),
            Some(true)
        );
        let tagged = Value::LangStr("abc".into(), "en".into());
        assert_eq!(
            in_restriction(&tagged, Dt::PlainLiteral, &[Facet::Length(3)]),
            Some(true)
        );
        // A length on a number has no meaning here.
        assert_eq!(
            in_restriction(&Value::Number("5".into()), Dt::Integer, &[Facet::Length(1)]),
            None
        );
    }

    #[test]
    fn pattern_facets() {
        let p = |pat: &str, s: &str| {
            in_restriction(
                &Value::Str(s.into()),
                Dt::String,
                &[Facet::Pattern(pat.into())],
            )
        };
        assert_eq!(p("[a-z]+", "abc"), Some(true));
        assert_eq!(p("[a-z]+", "ab1"), Some(false));
        // Whole-string match: XSD patterns are implicitly anchored.
        assert_eq!(p("b", "abc"), Some(false));
        assert_eq!(p("a|b", "a"), Some(true));
        assert_eq!(p("a|b", "ab"), Some(false));
        assert_eq!(p("\\d{3}-\\d{4}", "555-1234"), Some(true));
        assert_eq!(p("[^0-9]*", "abc"), Some(true));
        assert_eq!(p("[^0-9]*", "a1"), Some(false));
        // `.` matches neither newline nor carriage return.
        assert_eq!(p("a.c", "abc"), Some(true));
        assert_eq!(p("a.c", "a\nc"), Some(false));
        assert_eq!(p("a.c", "a\rc"), Some(false));
        // `^` and `$` are ordinary characters in XSD.
        assert_eq!(p("a$", "a$"), Some(true));
        assert_eq!(p("a$", "a"), Some(false));
        assert_eq!(p("^a", "^a"), Some(true));
        // `\s` is the four XSD spaces only; `\w` excludes punctuation.
        assert_eq!(p("a\\sb", "a b"), Some(true));
        assert_eq!(p("a\\sb", "a\u{A0}b"), Some(false));
        assert_eq!(p("\\w+", "a+b"), Some(true));
        assert_eq!(p("\\w+", "a.b"), Some(false));
        assert_eq!(p("[\\s]", " "), Some(true));
        // Not translated: unknown, not false.
        assert_eq!(p("\\i\\c*", "abc"), None);
        assert_eq!(p("[a-z-[aeiou]]+", "bcd"), None);
        assert_eq!(p("(?i)abc", "ABC"), None);
        assert_eq!(p("\\bab", "ab"), None);
        assert_eq!(p("[a-z", "a"), None);
        assert_eq!(p("(a", "a"), None);
        // On a value without a string form here: unknown.
        assert_eq!(
            in_restriction(
                &Value::Number("5".into()),
                Dt::Integer,
                &[Facet::Pattern("5".into())]
            ),
            None
        );
        // A URI is matched as written (collapsed).
        assert_eq!(
            in_restriction(
                &val("http://example.org/a", "anyURI"),
                Dt::AnyUri,
                &[Facet::Pattern("http://example\\.org/.*".into())]
            ),
            Some(true)
        );
    }

    #[test]
    fn language_range_facets() {
        let tagged = |tag: &str| Value::LangStr("x".into(), tag.to_ascii_lowercase());
        let r = |x: &Value, range: &str| {
            in_restriction(x, Dt::PlainLiteral, &[Facet::LangRange(range.into())])
        };
        assert_eq!(r(&tagged("en-GB"), "en"), Some(true));
        assert_eq!(r(&tagged("en"), "EN"), Some(true));
        assert_eq!(r(&tagged("en-GB"), "en-gb"), Some(true));
        assert_eq!(r(&tagged("eng"), "en"), Some(false));
        assert_eq!(r(&tagged("en"), "en-GB"), Some(false));
        assert_eq!(r(&tagged("fr"), "*"), Some(true));
        // A plain literal without a tag matches no range.
        assert_eq!(r(&Value::Str("x".into()), "*"), Some(false));
        assert_eq!(
            in_restriction(
                &Value::Str("x".into()),
                Dt::String,
                &[Facet::LangRange("en".into())]
            ),
            Some(false)
        );
        // Combined with a length: both must hold.
        assert_eq!(
            in_restriction(
                &tagged("en"),
                Dt::PlainLiteral,
                &[Facet::LangRange("en".into()), Facet::MaxLength(0)]
            ),
            Some(false)
        );
    }
}
