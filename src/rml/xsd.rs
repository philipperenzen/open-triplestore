//! Lexical-space checks for the XSD datatypes R2RML can validate.
//!
//! A literal whose datatype overrides the natural one and whose lexical form
//! is not in that datatype's lexical space is *ill-typed*, and generating it
//! is a data error (R2RML §10.3, §4.3). Only datatypes this module knows are
//! checked; any other datatype IRI is not validatable and passes.

use std::str::FromStr;

use oxsdatatypes::{
    Date, DateTime, DayTimeDuration, Duration, GDay, GMonth, GMonthDay, GYear, GYearMonth, Time,
    YearMonthDuration,
};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// Why `lexical` is not a `datatype` value, or `None` when it is (or when
/// the datatype is not one this module validates).
pub fn ill_typed(lexical: &str, datatype: &str) -> Option<&'static str> {
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
    fn datatypes_it_cannot_validate_pass() {
        assert_eq!(ill_typed("anything", &x("string")), None);
        assert_eq!(ill_typed("anything", "http://example.org/dt"), None);
    }
}
