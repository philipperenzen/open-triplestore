//! RFC 6238 TOTP (time-based one-time passwords) for two-factor login.
//!
//! Standard authenticator-app parameters: HMAC-SHA1, 6 digits, 30-second
//! periods. The shared secret is 20 random bytes, presented to the user as
//! base32 (RFC 4648, no padding) and stored AES-GCM-encrypted via
//! [`super::secret`].

use hmac::{Hmac, Mac};
use sha1::Sha1;

/// Time-step length in seconds (RFC 6238 default).
pub const PERIOD_SECS: u64 = 30;
/// Number of code digits.
const DIGITS: u32 = 6;
/// Accepted clock skew, in steps, on either side of "now".
const SKEW_STEPS: i64 = 1;

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Generate a fresh 160-bit shared secret, base32-encoded (no padding).
pub fn generate_secret() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 20];
    rand::thread_rng().fill_bytes(&mut bytes);
    base32_encode(&bytes)
}

/// Encode bytes as RFC 4648 base32 without padding.
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    for chunk in data.chunks(5) {
        let mut buf = [0u8; 5];
        buf[..chunk.len()].copy_from_slice(chunk);
        let bits = u64::from(buf[0]) << 32
            | u64::from(buf[1]) << 24
            | u64::from(buf[2]) << 16
            | u64::from(buf[3]) << 8
            | u64::from(buf[4]);
        let n_chars = match chunk.len() {
            1 => 2,
            2 => 4,
            3 => 5,
            4 => 7,
            _ => 8,
        };
        for i in 0..n_chars {
            let idx = ((bits >> (35 - 5 * i)) & 0x1f) as usize;
            out.push(BASE32_ALPHABET[idx] as char);
        }
    }
    out
}

/// Decode RFC 4648 base32 (case-insensitive, padding optional).
pub fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut bits: u64 = 0;
    let mut n_bits = 0u32;
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    for c in s.trim_end_matches('=').chars() {
        let v = match c.to_ascii_uppercase() {
            'A'..='Z' => c.to_ascii_uppercase() as u64 - 'A' as u64,
            '2'..='7' => c as u64 - '2' as u64 + 26,
            _ => return None,
        };
        bits = (bits << 5) | v;
        n_bits += 5;
        if n_bits >= 8 {
            n_bits -= 8;
            out.push(((bits >> n_bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Compute the TOTP code for a base32 secret at a given Unix time step.
pub fn code_at_step(secret_b32: &str, step: u64) -> Option<String> {
    let key = base32_decode(secret_b32)?;
    let mut mac = Hmac::<Sha1>::new_from_slice(&key).ok()?;
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let bin = (u32::from(digest[offset]) & 0x7f) << 24
        | u32::from(digest[offset + 1]) << 16
        | u32::from(digest[offset + 2]) << 8
        | u32::from(digest[offset + 3]);
    let code = bin % 10u32.pow(DIGITS);
    Some(format!("{code:06}"))
}

/// Current time step (Unix time / period).
pub fn current_step() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now / PERIOD_SECS
}

/// Verify a submitted code against the secret, allowing ±[`SKEW_STEPS`] of
/// clock drift. Returns the matching step so callers can persist it and
/// reject replays (a TOTP code must never authenticate twice).
///
/// `min_step` is the last successfully-used step + 1 (0 when unknown).
pub fn verify_code(secret_b32: &str, submitted: &str, min_step: u64) -> Option<u64> {
    let submitted = submitted.trim().replace(' ', "");
    if submitted.len() != DIGITS as usize || !submitted.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let now = current_step() as i64;
    for delta in -SKEW_STEPS..=SKEW_STEPS {
        let step = now + delta;
        if step < 0 || (step as u64) < min_step {
            continue;
        }
        if let Some(expected) = code_at_step(secret_b32, step as u64) {
            // Constant-time comparison: a timing oracle on code bytes would
            // let an attacker recover a valid code digit by digit.
            if constant_time_eq(expected.as_bytes(), submitted.as_bytes()) {
                return Some(step as u64);
            }
        }
    }
    None
}

pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Build the otpauth:// provisioning URI consumed by authenticator apps.
pub fn otpauth_url(secret_b32: &str, username: &str, issuer: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    let issuer_enc = utf8_percent_encode(issuer, NON_ALPHANUMERIC).to_string();
    let user_enc = utf8_percent_encode(username, NON_ALPHANUMERIC).to_string();
    format!(
        "otpauth://totp/{issuer_enc}:{user_enc}?secret={secret_b32}&issuer={issuer_enc}&algorithm=SHA1&digits={DIGITS}&period={PERIOD_SECS}"
    )
}

/// Generate `n` single-use recovery codes (`xxxx-xxxx-xxxx`, base32 charset).
pub fn generate_recovery_codes(n: usize) -> Vec<String> {
    use rand::RngCore;
    (0..n)
        .map(|_| {
            let mut bytes = [0u8; 8];
            rand::thread_rng().fill_bytes(&mut bytes);
            let s = base32_encode(&bytes).to_lowercase();
            format!("{}-{}-{}", &s[0..4], &s[4..8], &s[8..12])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6238 Appendix B test vectors (SHA-1, 8 digits truncated to 6 here
    /// we verify the full HOTP binary via known SHA1 vectors instead).
    /// Secret "12345678901234567890" = base32 GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ.
    #[test]
    fn rfc6238_sha1_vectors() {
        let secret = base32_encode(b"12345678901234567890");
        assert_eq!(secret, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        // (time, expected 8-digit) from RFC 6238; compare the last 6 digits.
        for (t, expected8) in [
            (59u64, "94287082"),
            (1111111109, "07081804"),
            (1111111111, "14050471"),
            (1234567890, "89005924"),
            (2000000000, "69279037"),
            (20000000000, "65353130"),
        ] {
            let step = t / PERIOD_SECS;
            let got = code_at_step(&secret, step).unwrap();
            assert_eq!(got, &expected8[2..], "at t={t}");
        }
    }

    #[test]
    fn base32_round_trip() {
        for data in [
            &b"12345678901234567890"[..],
            b"",
            b"a",
            b"ab",
            b"abc",
            b"abcd",
            b"abcde",
        ] {
            let enc = base32_encode(data);
            assert_eq!(base32_decode(&enc).unwrap(), data, "{enc}");
        }
        // Case-insensitive + padding tolerated.
        assert_eq!(base32_decode("mfrgg===").unwrap(), b"abc");
    }

    #[test]
    fn verify_accepts_adjacent_steps_and_blocks_replay() {
        let secret = generate_secret();
        let now = current_step();
        let code = code_at_step(&secret, now).unwrap();
        // Accepted at the current step…
        let used = verify_code(&secret, &code, 0).unwrap();
        assert_eq!(used, now);
        // …and rejected when the same step was already consumed.
        assert!(verify_code(&secret, &code, used + 1).is_none());
        // Previous step still accepted within skew. Guard against the wall
        // clock ticking into the next period mid-test (would shift the window).
        let prev = code_at_step(&secret, now - 1).unwrap();
        if prev != code && current_step() == now {
            assert_eq!(verify_code(&secret, &prev, 0), Some(now - 1));
        }
    }

    #[test]
    fn verify_rejects_garbage() {
        let secret = generate_secret();
        for bad in ["", "12345", "1234567", "abcdef", "12 34 5"] {
            assert!(verify_code(&secret, bad, 0).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn recovery_codes_are_unique_and_formatted() {
        let codes = generate_recovery_codes(10);
        assert_eq!(codes.len(), 10);
        let set: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(set.len(), 10);
        for c in &codes {
            assert_eq!(c.len(), 14);
            assert!(c
                .chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'));
        }
    }

    #[test]
    fn otpauth_url_is_escaped() {
        let url = otpauth_url("SECRET234", "alice b", "Open Triplestore");
        assert!(url.starts_with("otpauth://totp/Open%20Triplestore:alice%20b?secret=SECRET234"));
        assert!(url.contains("issuer=Open%20Triplestore"));
        assert!(url.contains("digits=6"));
        assert!(url.contains("period=30"));
    }
}
