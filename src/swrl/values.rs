//! Typed values for the SWRL built-ins and data ranges, on top of
//! `oxsdatatypes` (the XPath/XSD value types Oxigraph itself uses).
//!
//! A literal is read into a [`Value`] by its datatype; an ill-typed literal
//! (`"x"^^xsd:integer`) reads as [`Value::Other`] and so satisfies no numeric,
//! string or temporal built-in. Numbers follow XPath type promotion
//! (integer → decimal → float → double), and every operation is checked: an
//! overflow makes the built-in false rather than wrapping.

use std::cmp::Ordering;
use std::str::FromStr;

use oxigraph::model::{Literal, NamedNode, Term};
use oxsdatatypes::{
    Boolean, Date, DateTime, DayTimeDuration, Decimal, Double, Duration, Float, Integer, Time,
    YearMonthDuration,
};

pub(crate) const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
pub(crate) const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
pub(crate) const RDF_PLAIN_LITERAL: &str =
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral";

pub(crate) fn xsd(local: &str) -> NamedNode {
    NamedNode::new_unchecked(format!("{XSD}{local}"))
}

/// A number, at its place in the XPath promotion order.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Num {
    I(Integer),
    D(Decimal),
    F(Float),
    Db(Double),
}

impl Num {
    fn rank(self) -> u8 {
        match self {
            Num::I(_) => 0,
            Num::D(_) => 1,
            Num::F(_) => 2,
            Num::Db(_) => 3,
        }
    }

    /// This number at rank `r` (never lower than its own).
    fn at(self, r: u8) -> Num {
        match (self, r) {
            (n, r) if n.rank() >= r => n,
            (Num::I(i), 1) => Num::D(Decimal::from(i)),
            (Num::I(i), 2) => Num::F(Float::from(i)),
            (Num::I(i), _) => Num::Db(Double::from(i)),
            (Num::D(d), 2) => Num::F(Float::from(d)),
            (Num::D(d), _) => Num::Db(Double::from(d)),
            (Num::F(f), _) => Num::Db(Double::from(f)),
            (n, _) => n,
        }
    }

    fn pair(a: Num, b: Num) -> (Num, Num) {
        let r = a.rank().max(b.rank());
        (a.at(r), b.at(r))
    }

    pub(crate) fn to_f64(self) -> f64 {
        match self {
            Num::I(i) => f64::from(Double::from(i)),
            Num::D(d) => f64::from(Double::from(d)),
            Num::F(f) => f64::from(f),
            Num::Db(d) => f64::from(d),
        }
    }

    pub(crate) fn from_i64(i: i64) -> Num {
        Num::I(Integer::from(i))
    }

    /// The value as an `i64`, when it is integral.
    pub(crate) fn as_i64(self) -> Option<i64> {
        match self {
            Num::I(i) => Some(i64::from(i)),
            Num::D(d) => {
                let i = Integer::try_from(d).ok()?;
                (Decimal::from(i) == d).then(|| i64::from(i))
            }
            Num::F(_) | Num::Db(_) => {
                let f = self.to_f64();
                (f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15).then_some(f as i64)
            }
        }
    }

    pub(crate) fn add(a: Num, b: Num) -> Option<Num> {
        Some(match Num::pair(a, b) {
            (Num::I(x), Num::I(y)) => Num::I(x.checked_add(y)?),
            (Num::D(x), Num::D(y)) => Num::D(x.checked_add(y)?),
            (Num::F(x), Num::F(y)) => Num::F(x + y),
            (Num::Db(x), Num::Db(y)) => Num::Db(x + y),
            _ => return None,
        })
    }

    pub(crate) fn sub(a: Num, b: Num) -> Option<Num> {
        Some(match Num::pair(a, b) {
            (Num::I(x), Num::I(y)) => Num::I(x.checked_sub(y)?),
            (Num::D(x), Num::D(y)) => Num::D(x.checked_sub(y)?),
            (Num::F(x), Num::F(y)) => Num::F(x - y),
            (Num::Db(x), Num::Db(y)) => Num::Db(x - y),
            _ => return None,
        })
    }

    pub(crate) fn mul(a: Num, b: Num) -> Option<Num> {
        Some(match Num::pair(a, b) {
            (Num::I(x), Num::I(y)) => Num::I(x.checked_mul(y)?),
            (Num::D(x), Num::D(y)) => Num::D(x.checked_mul(y)?),
            (Num::F(x), Num::F(y)) => Num::F(x * y),
            (Num::Db(x), Num::Db(y)) => Num::Db(x * y),
            _ => return None,
        })
    }

    /// `op:numeric-divide`: integer / integer is a decimal; dividing an
    /// integer or decimal by zero is an error (false).
    pub(crate) fn div(a: Num, b: Num) -> Option<Num> {
        let r = a.rank().max(b.rank()).max(1);
        Some(match (a.at(r), b.at(r)) {
            (Num::D(x), Num::D(y)) => Num::D(x.checked_div(y)?),
            (Num::F(x), Num::F(y)) => Num::F(x / y),
            (Num::Db(x), Num::Db(y)) => Num::Db(x / y),
            _ => return None,
        })
    }

    /// `op:numeric-integer-divide`: the quotient truncated toward zero.
    pub(crate) fn idiv(a: Num, b: Num) -> Option<Num> {
        match Num::pair(a, b) {
            (Num::I(x), Num::I(y)) => Some(Num::I(x.checked_div(y)?)),
            (x, y) => {
                let q = Num::div(x, y)?.to_f64();
                if !q.is_finite() {
                    return None;
                }
                Some(Num::from_i64(q.trunc() as i64))
            }
        }
    }

    /// `op:numeric-mod`: the remainder takes the sign of the dividend.
    pub(crate) fn rem(a: Num, b: Num) -> Option<Num> {
        Some(match Num::pair(a, b) {
            (Num::I(x), Num::I(y)) => Num::I(x.checked_rem(y)?),
            (Num::D(x), Num::D(y)) => Num::D(x.checked_rem(y)?),
            (Num::F(x), Num::F(y)) => Num::F(Float::from(f32::from(x) % f32::from(y))),
            (Num::Db(x), Num::Db(y)) => Num::Db(Double::from(f64::from(x) % f64::from(y))),
            _ => return None,
        })
    }

    pub(crate) fn neg(self) -> Option<Num> {
        Some(match self {
            Num::I(x) => Num::I(x.checked_neg()?),
            Num::D(x) => Num::D(x.checked_neg()?),
            Num::F(x) => Num::F(-x),
            Num::Db(x) => Num::Db(-x),
        })
    }

    pub(crate) fn abs(self) -> Option<Num> {
        Some(match self {
            Num::I(x) => Num::I(x.checked_abs()?),
            Num::D(x) => Num::D(x.checked_abs()?),
            Num::F(x) => Num::F(x.abs()),
            Num::Db(x) => Num::Db(x.abs()),
        })
    }

    pub(crate) fn ceil(self) -> Option<Num> {
        Some(match self {
            Num::I(x) => Num::I(x),
            Num::D(x) => Num::D(x.checked_ceil()?),
            Num::F(x) => Num::F(x.ceil()),
            Num::Db(x) => Num::Db(x.ceil()),
        })
    }

    pub(crate) fn floor(self) -> Option<Num> {
        Some(match self {
            Num::I(x) => Num::I(x),
            Num::D(x) => Num::D(x.checked_floor()?),
            Num::F(x) => Num::F(x.floor()),
            Num::Db(x) => Num::Db(x.floor()),
        })
    }

    /// `fn:round`: halves round toward positive infinity.
    pub(crate) fn round(self) -> Option<Num> {
        Some(match self {
            Num::I(x) => Num::I(x),
            Num::D(x) => Num::D(x.checked_round()?),
            Num::F(x) => Num::F(x.round()),
            Num::Db(x) => Num::Db(x.round()),
        })
    }

    /// `fn:round-half-to-even` at `precision` decimal digits.
    pub(crate) fn round_half_even(self, precision: i64) -> Option<Num> {
        if !(-30..=30).contains(&precision) {
            return None;
        }
        match self {
            Num::F(_) | Num::Db(_) => {
                let f = self.to_f64();
                let scale = 10f64.powi(precision as i32);
                let s = f * scale;
                let fl = s.floor();
                let diff = s - fl;
                // Up past the half; at exactly the half, up only to an even.
                let up = diff > 0.5 || (diff == 0.5 && fl % 2.0 != 0.0);
                let r = if up { fl + 1.0 } else { fl };
                let out = r / scale;
                Some(match self {
                    Num::F(_) => Num::F(Float::from(out as f32)),
                    _ => Num::Db(Double::from(out)),
                })
            }
            Num::I(_) | Num::D(_) => {
                let x = match self.at(1) {
                    Num::D(d) => d,
                    _ => return None,
                };
                let ten = Decimal::from(10);
                let mut factor = Decimal::from(1);
                for _ in 0..precision.unsigned_abs() {
                    factor = factor.checked_mul(ten)?;
                }
                let scaled = if precision >= 0 {
                    x.checked_mul(factor)?
                } else {
                    x.checked_div(factor)?
                };
                let fl = scaled.checked_floor()?;
                let diff = scaled.checked_sub(fl)?;
                let half = Decimal::from_str("0.5").ok()?;
                let one = Decimal::from(1);
                let even = fl.checked_rem(Decimal::from(2))? == Decimal::from(0);
                let r = match diff.cmp(&half) {
                    Ordering::Greater => fl.checked_add(one)?,
                    Ordering::Less => fl,
                    Ordering::Equal if even => fl,
                    Ordering::Equal => fl.checked_add(one)?,
                };
                let out = if precision >= 0 {
                    r.checked_div(factor)?
                } else {
                    r.checked_mul(factor)?
                };
                Some(match self {
                    Num::I(_) => Num::I(Integer::try_from(out).ok()?),
                    _ => Num::D(out),
                })
            }
        }
    }

    pub(crate) fn cmp(a: Num, b: Num) -> Option<Ordering> {
        match Num::pair(a, b) {
            (Num::I(x), Num::I(y)) => Some(x.cmp(&y)),
            (Num::D(x), Num::D(y)) => Some(x.cmp(&y)),
            (Num::F(x), Num::F(y)) => x.partial_cmp(&y),
            (Num::Db(x), Num::Db(y)) => x.partial_cmp(&y),
            _ => None,
        }
    }

    pub(crate) fn to_literal(self) -> Literal {
        match self {
            Num::I(i) => Literal::new_typed_literal(i.to_string(), xsd("integer")),
            Num::D(d) => Literal::new_typed_literal(d.to_string(), xsd("decimal")),
            Num::F(f) => Literal::new_typed_literal(f.to_string(), xsd("float")),
            Num::Db(d) => Literal::new_typed_literal(d.to_string(), xsd("double")),
        }
    }
}

/// A literal's value, read by its datatype.
#[derive(Debug, Clone)]
pub(crate) enum Value {
    Num(Num),
    /// A string, with its language tag if it has one.
    Str(String, Option<String>),
    Bool(bool),
    DateTime(DateTime),
    Date(Date),
    Time(Time),
    Duration(Duration),
    AnyUri(String),
    /// A literal of another datatype, or an ill-typed one.
    Other,
}

/// The integer datatypes and their value bounds (`None` = unbounded).
pub(crate) fn integer_bounds(local: &str) -> Option<(Option<i128>, Option<i128>)> {
    Some(match local {
        "integer" => (None, None),
        "nonPositiveInteger" => (None, Some(0)),
        "negativeInteger" => (None, Some(-1)),
        "long" => (Some(i64::MIN as i128), Some(i64::MAX as i128)),
        "int" => (Some(i32::MIN as i128), Some(i32::MAX as i128)),
        "short" => (Some(i16::MIN as i128), Some(i16::MAX as i128)),
        "byte" => (Some(i8::MIN as i128), Some(i8::MAX as i128)),
        "nonNegativeInteger" => (Some(0), None),
        "unsignedLong" => (Some(0), Some(u64::MAX as i128)),
        "unsignedInt" => (Some(0), Some(u32::MAX as i128)),
        "unsignedShort" => (Some(0), Some(u16::MAX as i128)),
        "unsignedByte" => (Some(0), Some(u8::MAX as i128)),
        "positiveInteger" => (Some(1), None),
        _ => return None,
    })
}

/// The XSD string datatypes (`xsd:string` and those derived from it).
pub(crate) const STRING_TYPES: &[&str] = &[
    "string",
    "normalizedString",
    "token",
    "language",
    "Name",
    "NCName",
    "NMTOKEN",
];

fn parse_integer_lexical(lex: &str) -> Option<i128> {
    let t = lex.trim();
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse::<i128>().ok()
}

impl Value {
    pub(crate) fn of_literal(l: &Literal) -> Value {
        if let Some(lang) = l.language() {
            return Value::Str(l.value().to_string(), Some(lang.to_string()));
        }
        let dt = l.datatype().as_str();
        let lex = l.value();
        let Some(local) = dt.strip_prefix(XSD) else {
            return Value::Other;
        };
        if let Some((lo, hi)) = integer_bounds(local) {
            let Some(v) = parse_integer_lexical(lex) else {
                return Value::Other;
            };
            if lo.is_some_and(|lo| v < lo) || hi.is_some_and(|hi| v > hi) {
                return Value::Other;
            }
            return match i64::try_from(v) {
                Ok(i) => Value::Num(Num::from_i64(i)),
                Err(_) => Decimal::try_from(v)
                    .map(|d| Value::Num(Num::D(d)))
                    .unwrap_or(Value::Other),
            };
        }
        let parsed = match local {
            "decimal" => Decimal::from_str(lex.trim())
                .ok()
                .map(|d| Value::Num(Num::D(d))),
            "float" => Float::from_str(lex.trim())
                .ok()
                .map(|f| Value::Num(Num::F(f))),
            "double" => Double::from_str(lex.trim())
                .ok()
                .map(|d| Value::Num(Num::Db(d))),
            "boolean" => Boolean::from_str(lex.trim())
                .ok()
                .map(|b| Value::Bool(bool::from(b))),
            "dateTime" => DateTime::from_str(lex.trim()).ok().map(Value::DateTime),
            "dateTimeStamp" => DateTime::from_str(lex.trim())
                .ok()
                .filter(|d| d.timezone_offset().is_some())
                .map(Value::DateTime),
            "date" => Date::from_str(lex.trim()).ok().map(Value::Date),
            "time" => Time::from_str(lex.trim()).ok().map(Value::Time),
            "duration" => Duration::from_str(lex.trim()).ok().map(Value::Duration),
            "yearMonthDuration" => YearMonthDuration::from_str(lex.trim())
                .ok()
                .map(|d| Value::Duration(d.into())),
            "dayTimeDuration" => DayTimeDuration::from_str(lex.trim())
                .ok()
                .map(|d| Value::Duration(d.into())),
            "anyURI" => Some(Value::AnyUri(lex.trim().to_string())),
            s if STRING_TYPES.contains(&s) => Some(Value::Str(lex.to_string(), None)),
            _ => None,
        };
        parsed.unwrap_or(Value::Other)
    }

    pub(crate) fn of_term(t: &Term) -> Value {
        match t {
            Term::Literal(l) => Value::of_literal(l),
            _ => Value::Other,
        }
    }

    /// The value of `t` as a number.
    pub(crate) fn num(t: &Term) -> Option<Num> {
        match Value::of_term(t) {
            Value::Num(n) => Some(n),
            _ => None,
        }
    }

    /// The value of `t` as a string (a plain, `xsd:string`-family,
    /// language-tagged or `xsd:anyURI` literal).
    pub(crate) fn string(t: &Term) -> Option<String> {
        match Value::of_term(t) {
            Value::Str(s, _) | Value::AnyUri(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn boolean(t: &Term) -> Option<bool> {
        match Value::of_term(t) {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }
}

/// Order two values of comparable kinds; `None` when they are not comparable.
pub(crate) fn compare_values(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => Num::cmp(*x, *y),
        (Value::Str(x, lx), Value::Str(y, ly)) if lx == ly => Some(x.cmp(y)),
        (Value::Str(x, None), Value::AnyUri(y))
        | (Value::AnyUri(x), Value::Str(y, None))
        | (Value::AnyUri(x), Value::AnyUri(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::DateTime(x), Value::DateTime(y)) => x.partial_cmp(y),
        (Value::Date(x), Value::Date(y)) => x.partial_cmp(y),
        (Value::Time(x), Value::Time(y)) => x.partial_cmp(y),
        (Value::Duration(x), Value::Duration(y)) => x.partial_cmp(y),
        _ => None,
    }
}

/// Order two terms by value (`None`: not comparable).
pub(crate) fn compare_terms(a: &Term, b: &Term) -> Option<Ordering> {
    compare_values(&Value::of_term(a), &Value::of_term(b))
}

/// `swrlb:equal`: literals of comparable kinds are equal by value (`1` =
/// `1.0`); anything else is equal when it is the same RDF term.
pub(crate) fn terms_equal(a: &Term, b: &Term) -> bool {
    if a == b {
        return true;
    }
    match (a, b) {
        (Term::Literal(_), Term::Literal(_)) => {
            compare_terms(a, b).is_some_and(|o| o == Ordering::Equal)
        }
        _ => false,
    }
}

pub(crate) fn string_literal(s: impl Into<String>) -> Term {
    Literal::new_typed_literal(s.into(), xsd("string")).into()
}

pub(crate) fn bool_literal(b: bool) -> Term {
    Literal::new_typed_literal(if b { "true" } else { "false" }, xsd("boolean")).into()
}

pub(crate) fn int_literal(i: i64) -> Term {
    Num::from_i64(i).to_literal().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(v: &str, dt: &str) -> Term {
        Literal::new_typed_literal(v, xsd(dt)).into()
    }

    #[test]
    fn numbers_promote_and_compare_by_value() {
        assert!(terms_equal(&lit("1", "integer"), &lit("1.0", "decimal")));
        assert!(terms_equal(&lit("1", "byte"), &lit("1", "integer")));
        assert!(!terms_equal(&lit("1", "integer"), &lit("2", "integer")));
        assert_eq!(
            compare_terms(&lit("2", "integer"), &lit("1.5", "double")),
            Some(Ordering::Greater)
        );
        // An ill-typed or out-of-range literal has no numeric value.
        assert!(Value::num(&lit("x", "integer")).is_none());
        assert!(Value::num(&lit("300", "byte")).is_none());
        let half = Num::D(Decimal::from_str("2.5").unwrap());
        assert_eq!(half.round_half_even(0).unwrap().to_literal().value(), "2");
        let x = Num::D(Decimal::from_str("3.14159").unwrap());
        assert_eq!(x.round_half_even(2).unwrap().to_literal().value(), "3.14");
        assert_eq!(
            Num::div(Num::from_i64(1), Num::from_i64(2))
                .unwrap()
                .to_literal()
                .value(),
            "0.5"
        );
        assert!(Num::div(Num::from_i64(1), Num::from_i64(0)).is_none());
    }
}
