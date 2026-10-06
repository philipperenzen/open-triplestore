//! The one parser every setting goes through.

use super::{Kind, Spec};

/// A boolean: `1`, `true`, `yes`, `on` or `0`, `false`, `no`, `off`, in any
/// case. Anything else is an error, never a default.
pub fn bool(raw: &str) -> Result<bool, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!(
            "`{other}` is not a boolean (use true/false, 1/0, yes/no or on/off)"
        )),
    }
}

/// A comma-separated list: entries trimmed, empty ones dropped.
pub fn list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// A byte count: digits, optionally followed by `KiB`, `MiB` or `GiB` (any
/// case, with or without a space).
pub fn bytes(raw: &str) -> Result<u64, String> {
    let t = raw.trim();
    let split = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
    let (digits, suffix) = t.split_at(split);
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("`{t}` is not a byte count (e.g. 1048576 or 64MiB)"))?;
    let unit: u64 = match suffix.trim().to_ascii_lowercase().as_str() {
        "" => 1,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        other => {
            return Err(format!(
                "`{other}` is not a size unit (use KiB, MiB or GiB, or a plain byte count)"
            ))
        }
    };
    n.checked_mul(unit)
        .ok_or_else(|| format!("`{t}` is too large"))
}

/// A number setting's value: an integer in range, or a byte count.
pub fn number(spec: &Spec, raw: &str) -> Result<u64, String> {
    match spec.kind {
        Kind::Int { min, max } => {
            let t = raw.trim();
            let n: u64 = t
                .parse()
                .map_err(|_| format!("`{t}` is not a whole number"))?;
            if n < min || n > max {
                return Err(if max == u64::MAX {
                    format!("{n} is below the minimum of {min}")
                } else {
                    format!("{n} is outside {min}..={max}")
                });
            }
            Ok(n)
        }
        Kind::Bytes { min } => {
            let n = bytes(raw)?;
            if n < min {
                return Err(format!("{n} bytes is below the minimum of {min}"));
            }
            Ok(n)
        }
        _ => Err(format!("{} is not a number setting", spec.name)),
    }
}

/// An absolute `http` or `https` URL.
pub fn http_url(raw: &str) -> Result<(), String> {
    let u = url::Url::parse(raw.trim()).map_err(|e| format!("`{raw}` is not a URL: {e}"))?;
    if !matches!(u.scheme(), "http" | "https") || u.host_str().is_none() {
        return Err(format!("`{raw}` is not an absolute http(s) URL"));
    }
    Ok(())
}

/// Validate `raw` against `spec` and return it normalised: trimmed, a
/// boolean as `true`/`false`, an enum value in its canonical spelling, a
/// number as plain digits.
pub fn validate(spec: &Spec, raw: &str) -> Result<String, String> {
    let t = raw.trim();
    match spec.kind {
        Kind::Bool => bool(t).map(|b| b.to_string()),
        Kind::Int { .. } | Kind::Bytes { .. } => number(spec, t).map(|n| n.to_string()),
        Kind::Enum(choices) => choices
            .iter()
            .find(|c| c.eq_ignore_ascii_case(t))
            .map(|c| c.to_string())
            .ok_or_else(|| format!("`{t}` is not one of {}", choices.join(", "))),
        Kind::List => Ok(list(t).join(",")),
        Kind::Url => http_url(t).map(|()| t.to_string()),
        Kind::Text | Kind::Path => Ok(t.to_string()),
        Kind::Secret => {
            if crate::secrets::looks_like_ref(t) {
                crate::secrets::SecretRef::parse(t)
                    .map(|_| t.to_string())
                    .map_err(|e| e.to_string())
            } else {
                Ok(t.to_string())
            }
        }
        Kind::Custom { check, .. } => check(t).map(|()| t.to_string()),
    }
}
