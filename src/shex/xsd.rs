//! XML Schema datatypes as ShEx 2.1 needs them (§5.4.3, §5.4.5): lexical
//! validity of the SPARQL operand datatypes and the integer types derived
//! from `xsd:decimal`, and numeric values with exact decimal arithmetic (no
//! `f64` rounding of `xsd:decimal` / `xsd:integer`, whose facets compare
//! exactly).

use std::cmp::Ordering;
use std::sync::OnceLock;

use regex::Regex;

use super::ast::XSD;

/// An exact decimal: sign, integer digits without leading zeros, fraction
/// digits without trailing zeros. Zero is `{ neg: false, int: "", frac: "" }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Dec {
    neg: bool,
    int: String,
    frac: String,
}

impl Dec {
    /// Parse an `xsd:decimal` or `xsd:integer` lexical form (Turtle INTEGER /
    /// DECIMAL included). `None` when it is not one.
    pub fn parse(s: &str) -> Option<Dec> {
        let (neg, body) = match s.as_bytes().first()? {
            b'-' => (true, &s[1..]),
            b'+' => (false, &s[1..]),
            _ => (false, s),
        };
        let (int, frac) = match body.split_once('.') {
            Some((i, f)) => (i, f),
            None => (body, ""),
        };
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let int = int.trim_start_matches('0').to_string();
        let frac = frac.trim_end_matches('0').to_string();
        let zero = int.is_empty() && frac.is_empty();
        Some(Dec {
            neg: neg && !zero,
            int,
            frac,
        })
    }

    pub fn is_integer(&self) -> bool {
        self.frac.is_empty()
    }

    /// Digits in the canonical form (`totalDigits`): at least one.
    pub fn total_digits(&self) -> u64 {
        ((self.int.len() + self.frac.len()) as u64).max(1)
    }

    /// Fraction digits in the canonical form, trailing zeros ignored.
    pub fn fraction_digits(&self) -> u64 {
        self.frac.len() as u64
    }

    pub fn to_f64(&self) -> f64 {
        let s = format!(
            "{}{}.{}",
            if self.neg { "-" } else { "" },
            if self.int.is_empty() { "0" } else { &self.int },
            if self.frac.is_empty() {
                "0"
            } else {
                &self.frac
            }
        );
        s.parse().unwrap_or(f64::NAN)
    }

    fn cmp_magnitude(&self, other: &Dec) -> Ordering {
        self.int
            .len()
            .cmp(&other.int.len())
            .then_with(|| self.int.cmp(&other.int))
            .then_with(|| {
                let n = self.frac.len().max(other.frac.len());
                let a = format!("{:0<n$}", self.frac);
                let b = format!("{:0<n$}", other.frac);
                a.cmp(&b)
            })
    }
}

impl Ord for Dec {
    fn cmp(&self, other: &Dec) -> Ordering {
        match (self.neg, other.neg) {
            (false, true) => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, false) => self.cmp_magnitude(other),
            (true, true) => other.cmp_magnitude(self),
        }
    }
}

impl PartialOrd for Dec {
    fn partial_cmp(&self, other: &Dec) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A numeric value: exact for the `xsd:decimal` family, IEEE for
/// `xsd:float` / `xsd:double`.
#[derive(Debug, Clone)]
pub enum Numeric {
    Decimal(Dec),
    Double(f64),
}

impl Numeric {
    /// Compare after XPath numeric type promotion: decimal against decimal is
    /// exact; anything involving a float or double compares as doubles.
    /// `None` when either side is NaN.
    pub fn partial_cmp_value(&self, other: &Numeric) -> Option<Ordering> {
        match (self, other) {
            (Numeric::Decimal(a), Numeric::Decimal(b)) => Some(a.cmp(b)),
            _ => self.as_f64().partial_cmp(&other.as_f64()),
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            Numeric::Decimal(d) => d.to_f64(),
            Numeric::Double(f) => *f,
        }
    }

    /// Parse a ShExC numeric literal token (INTEGER, DECIMAL or DOUBLE).
    pub fn parse_token(s: &str) -> Option<Numeric> {
        if s.contains(['e', 'E']) {
            parse_double(s).map(Numeric::Double)
        } else {
            Dec::parse(s).map(Numeric::Decimal)
        }
    }
}

impl std::fmt::Display for Numeric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Numeric::Decimal(d) => {
                let sign = if d.neg { "-" } else { "" };
                let int = if d.int.is_empty() { "0" } else { &d.int };
                if d.frac.is_empty() {
                    write!(f, "{sign}{int}")
                } else {
                    write!(f, "{sign}{int}.{}", d.frac)
                }
            }
            Numeric::Double(x) => write!(f, "{x:e}"),
        }
    }
}

/// Equality by numeric value: two representations of one facet (`1.0` in
/// ShExC, `1` in ShExJ) are the same facet.
impl PartialEq for Numeric {
    fn eq(&self, other: &Numeric) -> bool {
        self.partial_cmp_value(other) == Some(Ordering::Equal)
    }
}

fn parse_double(s: &str) -> Option<f64> {
    match s {
        // XSD 1.0 / XPath casting: "+INF" is not a lexical form.
        "INF" => return Some(f64::INFINITY),
        "-INF" => return Some(f64::NEG_INFINITY),
        "NaN" => return Some(f64::NAN),
        _ => {}
    }
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([Ee][+-]?[0-9]+)?$").unwrap()
    });
    if !re.is_match(s) {
        return None;
    }
    s.parse().ok()
}

/// The integer types derived from `xsd:decimal`, with their value ranges
/// (`None` = unbounded on that side).
fn integer_range(local: &str) -> Option<(Option<&'static str>, Option<&'static str>)> {
    Some(match local {
        "integer" => (None, None),
        "nonPositiveInteger" => (None, Some("0")),
        "negativeInteger" => (None, Some("-1")),
        "long" => (Some("-9223372036854775808"), Some("9223372036854775807")),
        "int" => (Some("-2147483648"), Some("2147483647")),
        "short" => (Some("-32768"), Some("32767")),
        "byte" => (Some("-128"), Some("127")),
        "nonNegativeInteger" => (Some("0"), None),
        "unsignedLong" => (Some("0"), Some("18446744073709551615")),
        "unsignedInt" => (Some("0"), Some("4294967295")),
        "unsignedShort" => (Some("0"), Some("65535")),
        "unsignedByte" => (Some("0"), Some("255")),
        "positiveInteger" => (Some("1"), None),
        _ => return None,
    })
}

fn is_integer_lexical(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `datatype` is one of the numeric types (SPARQL operand types and
/// the types derived from them).
pub fn is_numeric_datatype(datatype: &str) -> bool {
    datatype
        .strip_prefix(XSD)
        .is_some_and(|l| matches!(l, "decimal" | "float" | "double") || integer_range(l).is_some())
}

/// Whether `datatype` is `xsd:decimal` or derived from it (the types
/// `totalDigits` / `fractionDigits` apply to).
pub fn is_decimal_derived(datatype: &str) -> bool {
    datatype
        .strip_prefix(XSD)
        .is_some_and(|l| l == "decimal" || integer_range(l).is_some())
}

/// The numeric value of a literal, when its datatype is numeric and its
/// lexical form valid.
pub fn numeric_value(lexical: &str, datatype: &str) -> Option<Numeric> {
    let local = datatype.strip_prefix(XSD)?;
    match local {
        "float" | "double" => parse_double(lexical).map(Numeric::Double),
        "decimal" => Dec::parse(lexical).map(Numeric::Decimal),
        _ => {
            integer_range(local)?;
            if !lexical_valid(lexical, datatype) {
                return None;
            }
            Dec::parse(lexical).map(Numeric::Decimal)
        }
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).unwrap())
}

const TZ: &str = r"(Z|[+-]((0[0-9]|1[0-3]):[0-5][0-9]|14:00))?";

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

fn valid_ymd(s: &str) -> bool {
    // `-?YYYY+-MM-DD` prefix, already shape-checked by the caller's regex.
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s),
    };
    let mut parts = body.splitn(3, '-');
    let (Some(y), Some(m), Some(d)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let d = &d[..2.min(d.len())];
    let (Ok(y), Ok(m), Ok(d)) = (y.parse::<i64>(), m.parse::<u32>(), d.parse::<u32>()) else {
        return false;
    };
    // XSD 1.1: year 0 is 1 BCE; leap years follow the proleptic calendar.
    let y = if neg { -y } else { y };
    d >= 1 && d <= days_in_month(y, m)
}

/// Whether `lexical` is in the lexical space of `datatype`. Datatypes the
/// engine does not know (custom ones, most non-XSD ones) are accepted:
/// ShEx tests their lexical form only for the SPARQL operand types (§5.4.3),
/// and this checks a few more XSD types the same way.
pub fn lexical_valid(lexical: &str, datatype: &str) -> bool {
    let Some(local) = datatype.strip_prefix(XSD) else {
        return true;
    };
    static DATE_TIME: OnceLock<Regex> = OnceLock::new();
    static DATE: OnceLock<Regex> = OnceLock::new();
    static TIME: OnceLock<Regex> = OnceLock::new();
    static LANGUAGE: OnceLock<Regex> = OnceLock::new();
    if let Some((lo, hi)) = integer_range(local) {
        if !is_integer_lexical(lexical) {
            return false;
        }
        let Some(v) = Dec::parse(lexical) else {
            return false;
        };
        let above = lo.is_none_or(|lo| v >= Dec::parse(lo).unwrap());
        let below = hi.is_none_or(|hi| v <= Dec::parse(hi).unwrap());
        return above && below;
    }
    match local {
        "decimal" => !lexical.contains(['e', 'E']) && Dec::parse(lexical).is_some(),
        "float" | "double" => parse_double(lexical).is_some(),
        "boolean" => matches!(lexical, "true" | "false" | "1" | "0"),
        "dateTime" | "dateTimeStamp" => {
            let r = re(
                &DATE_TIME,
                &format!(
                    r"^-?([1-9][0-9]{{3,}}|0[0-9]{{3}})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T(([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](\.[0-9]+)?|24:00:00(\.0+)?){TZ}$"
                ),
            );
            r.is_match(lexical)
                && valid_ymd(lexical)
                && (local != "dateTimeStamp" || lexical.ends_with('Z') || {
                    let t = &lexical[lexical.len().saturating_sub(6)..];
                    t.starts_with(['+', '-']) && t.as_bytes().get(3) == Some(&b':')
                })
        }
        "date" => {
            let r = re(
                &DATE,
                &format!(
                    r"^-?([1-9][0-9]{{3,}}|0[0-9]{{3}})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01]){TZ}$"
                ),
            );
            r.is_match(lexical) && valid_ymd(lexical)
        }
        "time" => re(
            &TIME,
            &format!(
                r"^(([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](\.[0-9]+)?|24:00:00(\.0+)?){TZ}$"
            ),
        )
        .is_match(lexical),
        "language" => re(&LANGUAGE, r"^[a-zA-Z]{1,8}(-[a-zA-Z0-9]{1,8})*$").is_match(lexical),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> Dec {
        Dec::parse(s).unwrap()
    }

    #[test]
    fn decimals_compare_exactly() {
        assert!(d("0.1") < d("0.10000000000000000001"));
        assert_eq!(d("1.0"), d("01"));
        assert!(d("-2") < d("-1.5"));
        assert_eq!(d("-0"), d("0.000"));
        assert!(d("123456789012345678901234567890") > d("123456789012345678901234567889.9"));
    }

    #[test]
    fn digit_counts_use_the_canonical_form() {
        assert_eq!(d("0012.300").total_digits(), 3);
        assert_eq!(d("0012.300").fraction_digits(), 1);
        assert_eq!(d("5").fraction_digits(), 0);
    }

    #[test]
    fn lexical_spaces() {
        let x = |l: &str| format!("{XSD}{l}");
        assert!(lexical_valid("127", &x("byte")));
        assert!(!lexical_valid("128", &x("byte")));
        assert!(!lexical_valid("1.0", &x("integer")));
        assert!(lexical_valid("1.", &x("decimal")));
        assert!(!lexical_valid("1e3", &x("decimal")));
        assert!(lexical_valid("-INF", &x("double")));
        assert!(!lexical_valid("inf", &x("double")));
        assert!(!lexical_valid("abc", &x("integer")));
        assert!(lexical_valid("2016-02-29T00:00:00Z", &x("dateTime")));
        assert!(!lexical_valid("2015-02-29T00:00:00Z", &x("dateTime")));
        assert!(!lexical_valid("2016-07", &x("date")));
        assert!(lexical_valid("anything", "http://example.org/dt"));
    }

    #[test]
    fn numeric_promotion() {
        let a = Numeric::Decimal(d("1"));
        let b = Numeric::Double(1.0);
        assert_eq!(a, b);
        assert_eq!(
            Numeric::Double(f64::NAN).partial_cmp_value(&a),
            None,
            "NaN compares with nothing"
        );
    }
}
