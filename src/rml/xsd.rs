//! Lexical-space checks for the XSD datatypes, and `rdf:JSON`, that R2RML can
//! validate.
//!
//! A literal whose datatype overrides the natural one and whose lexical form
//! is not in that datatype's lexical space is *ill-typed*, and generating it
//! is a data error (R2RML §10.3, §4.3). Only datatypes this module knows are
//! checked; any other datatype IRI is not validatable and passes.

use std::str::FromStr;

use ots_plugin_api::sources::ValueKind;
use oxsdatatypes::{
    Date, DateTime, DayTimeDuration, Duration, GDay, GMonth, GMonthDay, GYear, GYearMonth, Time,
    TimezoneOffset, YearMonthDuration,
};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// Why `lexical` is not a `datatype` value, or `None` when it is (or when
/// the datatype is not one this module validates).
pub fn ill_typed(lexical: &str, datatype: &str) -> Option<&'static str> {
    // rdf:JSON: a JSON text (JSON-LD 1.1 §10.2), which the store also refuses
    // on every other write path (`store::literals`).
    if datatype == crate::store::literals::RDF_JSON {
        return crate::store::literals::json_lexical_error(lexical)
            .map(|_| "not JSON, which rdf:JSON requires");
    }
    let local = datatype.strip_prefix(XSD)?;
    let ok = match local {
        "boolean" => matches!(lexical, "true" | "false" | "1" | "0"),
        "decimal" => is_decimal(lexical),
        "integer" => is_integer(lexical),
        "long" => in_range(lexical, i64::MIN as i128, i64::MAX as i128),
        "int" => in_range(lexical, i32::MIN as i128, i32::MAX as i128),
        "short" => in_range(lexical, i16::MIN as i128, i16::MAX as i128),
        "byte" => in_range(lexical, i8::MIN as i128, i8::MAX as i128),
        "unsignedLong" => in_range(lexical, 0, u64::MAX as i128),
        "unsignedInt" => in_range(lexical, 0, u32::MAX as i128),
        "unsignedShort" => in_range(lexical, 0, u16::MAX as i128),
        "unsignedByte" => in_range(lexical, 0, u8::MAX as i128),
        "nonNegativeInteger" => sign_ok(lexical, |n| n >= 0, |neg, zero| !neg || zero),
        "positiveInteger" => sign_ok(lexical, |n| n > 0, |neg, zero| !neg && !zero),
        "nonPositiveInteger" => sign_ok(lexical, |n| n <= 0, |neg, zero| neg || zero),
        "negativeInteger" => sign_ok(lexical, |n| n < 0, |neg, zero| neg && !zero),
        "double" | "float" => is_float(lexical),
        "date" => Date::from_str(lexical).is_ok(),
        "time" => Time::from_str(lexical).is_ok(),
        "dateTime" => DateTime::from_str(lexical).is_ok(),
        "dateTimeStamp" => DateTime::from_str(lexical).is_ok_and(|d| d.timezone().is_some()),
        "duration" => Duration::from_str(lexical).is_ok(),
        "dayTimeDuration" => DayTimeDuration::from_str(lexical).is_ok(),
        "yearMonthDuration" => YearMonthDuration::from_str(lexical).is_ok(),
        "gYear" => GYear::from_str(lexical).is_ok(),
        "gYearMonth" => GYearMonth::from_str(lexical).is_ok(),
        "gMonth" => GMonth::from_str(lexical).is_ok(),
        "gDay" => GDay::from_str(lexical).is_ok(),
        "gMonthDay" => GMonthDay::from_str(lexical).is_ok(),
        "hexBinary" => {
            lexical.len().is_multiple_of(2) && lexical.bytes().all(|b| b.is_ascii_hexdigit())
        }
        _ => return None,
    };
    (!ok).then_some("not in the lexical space of the datatype")
}

/// The canonical lexical representation of a source value of `kind` in its
/// natural datatype (R2RML §10.2: the natural RDF lexical form of an SQL
/// value is "the canonical lexical representation" of its XSD datatype, in
/// XML Schema Part 2, which R2RML cites): `xsd:integer` without a sign or
/// leading zeros, `xsd:decimal` with exactly one digit either side of the
/// point at the least (`1.0`, `-0.5`), `xsd:double` as a mantissa with one
/// digit before the point and an exponent (`3.0E1`, `1.65E0`), `xsd:boolean`
/// as `true` / `false`, and `xsd:dateTime` / `xsd:time` with no trailing
/// zeros in the seconds and a timezone, when there is one, shifted to `Z`.
/// `None` when the value is not in the datatype's lexical space, or the kind
/// has no canonical form to compute; the value then stays as the source
/// wrote it, and a datatype that does not accept it is caught as ill-typed.
pub fn canonical(lexical: &str, kind: ValueKind) -> Option<String> {
    match kind {
        ValueKind::Integer => canonical_integer(lexical),
        ValueKind::Decimal => canonical_decimal(lexical),
        ValueKind::Float => canonical_double(lexical),
        ValueKind::Boolean => match lexical {
            "true" | "1" => Some("true".into()),
            "false" | "0" => Some("false".into()),
            _ => None,
        },
        ValueKind::DateTime => {
            let dt = DateTime::from_str(lexical).ok()?;
            let dt = match dt.timezone_offset() {
                Some(_) => dt.adjust(Some(TimezoneOffset::UTC))?,
                None => dt,
            };
            Some(dt.to_string())
        }
        ValueKind::Time => {
            let t = Time::from_str(lexical).ok()?;
            let t = match t.timezone_offset() {
                Some(_) => t.adjust(Some(TimezoneOffset::UTC))?,
                None => t,
            };
            Some(t.to_string())
        }
        ValueKind::Binary => (lexical.len().is_multiple_of(2)
            && lexical.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| lexical.to_ascii_uppercase()),
        _ => None,
    }
}

/// `+007` → `7`, `-0` → `0`.
fn canonical_integer(s: &str) -> Option<String> {
    if !is_integer(s) {
        return None;
    }
    let negative = s.starts_with('-');
    let digits = s.trim_start_matches(['+', '-']).trim_start_matches('0');
    Some(match (digits.is_empty(), negative) {
        (true, _) => "0".to_string(),
        (false, true) => format!("-{digits}"),
        (false, false) => digits.to_string(),
    })
}

/// `+01.50` → `1.5`, `2` → `2.0`, `-.0` → `0.0`.
fn canonical_decimal(s: &str) -> Option<String> {
    if !is_decimal(s) {
        return None;
    }
    let negative = s.starts_with('-');
    let body = s.trim_start_matches(['+', '-']);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    if int.is_empty() && frac.is_empty() {
        return Some("0.0".into());
    }
    Some(format!(
        "{}{}.{}",
        if negative { "-" } else { "" },
        if int.is_empty() { "0" } else { int },
        if frac.is_empty() { "0" } else { frac }
    ))
}

/// `30` → `3.0E1`, `1.65` → `1.65E0`, `0` → `0.0E0`: the shortest mantissa
/// that reads back as the same double. `INF`, `-INF` and `NaN` as they are.
fn canonical_double(s: &str) -> Option<String> {
    match s {
        "INF" | "+INF" => return Some("INF".into()),
        "-INF" | "NaN" => return Some(s.into()),
        _ if !is_float(s) => return None,
        _ => {}
    }
    let x: f64 = s.parse().ok()?;
    if !x.is_finite() {
        // Out of range for a double: no canonical form to give.
        return None;
    }
    let exp = format!("{x:e}");
    let (mantissa, exponent) = exp.split_once('e')?;
    Some(if mantissa.contains('.') {
        format!("{mantissa}E{exponent}")
    } else {
        format!("{mantissa}.0E{exponent}")
    })
}

/// `[+-]?[0-9]+`. Unbounded, as `xsd:integer` is.
fn is_integer(s: &str) -> bool {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn in_range(s: &str, min: i128, max: i128) -> bool {
    is_integer(s) && s.parse::<i128>().is_ok_and(|n| (min..=max).contains(&n))
}

/// A sign-constrained integer type. `small` decides values that fit an
/// `i128`; `huge(negative, zero)` decides the rest by their sign alone.
fn sign_ok(s: &str, small: impl Fn(i128) -> bool, huge: impl Fn(bool, bool) -> bool) -> bool {
    if !is_integer(s) {
        return false;
    }
    match s.parse::<i128>() {
        Ok(n) => small(n),
        Err(_) => {
            let negative = s.starts_with('-');
            let zero = s.trim_start_matches(['+', '-']).bytes().all(|b| b == b'0');
            huge(negative, zero)
        }
    }
}

/// `[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)`
fn is_decimal(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    let digits = |d: &str| d.bytes().all(|b| b.is_ascii_digit());
    match frac {
        None => !int.is_empty() && digits(int),
        Some(f) => (!int.is_empty() || !f.is_empty()) && digits(int) && digits(f),
    }
}

/// A decimal with an optional exponent, or `INF`, `+INF`, `-INF`, `NaN`.
fn is_float(s: &str) -> bool {
    if matches!(s, "INF" | "+INF" | "-INF" | "NaN") {
        return true;
    }
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    is_decimal(mantissa) && exponent.is_none_or(is_integer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(local: &str) -> String {
        format!("{XSD}{local}")
    }

    #[test]
    fn well_typed_values_pass() {
        for (v, t) in [
            ("true", "boolean"),
            ("0", "boolean"),
            ("-12", "integer"),
            ("123456789012345678901234567890", "integer"),
            ("+1.50", "decimal"),
            (".5", "decimal"),
            ("1.", "decimal"),
            ("1e10", "double"),
            ("-INF", "float"),
            ("2026-10-02", "date"),
            ("2026-10-02T10:00:00Z", "dateTime"),
            ("2026-10-02T10:00:00+02:00", "dateTimeStamp"),
            ("12:30:00", "time"),
            ("P1Y2M", "duration"),
            ("2026", "gYear"),
            ("0FB7", "hexBinary"),
            ("127", "byte"),
            ("0", "nonNegativeInteger"),
            ("-1", "negativeInteger"),
            (
                "99999999999999999999999999999999999999999",
                "positiveInteger",
            ),
        ] {
            assert_eq!(ill_typed(v, &x(t)), None, "{v} as xsd:{t}");
        }
    }

    #[test]
    fn ill_typed_values_are_caught() {
        for (v, t) in [
            ("yes", "boolean"),
            ("1.5", "integer"),
            (" 1", "integer"),
            ("", "integer"),
            ("abc", "decimal"),
            (".", "decimal"),
            ("1e", "double"),
            ("2026-13-01", "date"),
            ("2026-10-02T10:00:00", "dateTimeStamp"),
            ("128", "byte"),
            ("-1", "unsignedInt"),
            ("0", "positiveInteger"),
            (
                "-99999999999999999999999999999999999999999",
                "nonNegativeInteger",
            ),
            ("ABC", "hexBinary"),
        ] {
            assert!(ill_typed(v, &x(t)).is_some(), "{v:?} as xsd:{t}");
        }
    }

    #[test]
    fn rdf_json_must_be_json() {
        let json = crate::store::literals::RDF_JSON;
        assert_eq!(ill_typed("{\"a\": [1, 2]}", json), None);
        assert_eq!(ill_typed("\"text\"", json), None);
        assert!(ill_typed("bareword", json).is_some());
        assert!(ill_typed("[{]", json).is_some());
    }

    #[test]
    fn natural_values_take_their_canonical_form() {
        for (v, kind, want) in [
            ("30", ValueKind::Float, Some("3.0E1")),
            ("30.0", ValueKind::Float, Some("3.0E1")),
            ("1.65", ValueKind::Float, Some("1.65E0")),
            ("80.25", ValueKind::Float, Some("8.025E1")),
            ("2.0000000000000000e+000", ValueKind::Float, Some("2.0E0")),
            ("0.001", ValueKind::Float, Some("1.0E-3")),
            ("0", ValueKind::Float, Some("0.0E0")),
            ("-1.5e300", ValueKind::Float, Some("-1.5E300")),
            ("INF", ValueKind::Float, Some("INF")),
            ("NaN", ValueKind::Float, Some("NaN")),
            ("1e999", ValueKind::Float, None),
            ("abc", ValueKind::Float, None),
            ("+007", ValueKind::Integer, Some("7")),
            ("-0", ValueKind::Integer, Some("0")),
            ("-12", ValueKind::Integer, Some("-12")),
            ("1.5", ValueKind::Integer, None),
            ("+01.50", ValueKind::Decimal, Some("1.5")),
            ("2", ValueKind::Decimal, Some("2.0")),
            ("-.50", ValueKind::Decimal, Some("-0.5")),
            ("-0.00", ValueKind::Decimal, Some("0.0")),
            ("1", ValueKind::Boolean, Some("true")),
            ("false", ValueKind::Boolean, Some("false")),
            (
                "2009-10-10T12:12:22",
                ValueKind::DateTime,
                Some("2009-10-10T12:12:22"),
            ),
            (
                "2009-10-10T12:12:22.500",
                ValueKind::DateTime,
                Some("2009-10-10T12:12:22.5"),
            ),
            (
                "2009-10-10T12:12:22+02:00",
                ValueKind::DateTime,
                Some("2009-10-10T10:12:22Z"),
            ),
            (
                "2009-10-10T12:12:22+00:00",
                ValueKind::DateTime,
                Some("2009-10-10T12:12:22Z"),
            ),
            ("12:30:00.100Z", ValueKind::Time, Some("12:30:00.1Z")),
            ("0fb7", ValueKind::Binary, Some("0FB7")),
            ("x", ValueKind::Text, None),
        ] {
            assert_eq!(canonical(v, kind).as_deref(), want, "{v} as {kind:?}");
        }
    }

    #[test]
    fn datatypes_it_cannot_validate_pass() {
        assert_eq!(ill_typed("anything", &x("string")), None);
        assert_eq!(ill_typed("anything", "http://example.org/dt"), None);
    }
}
