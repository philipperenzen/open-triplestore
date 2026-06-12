//! Input validation for account fields (email, username, password).
//!
//! The email check is a pragmatic RFC 5321/5322 subset: dot-atom local part,
//! dotted hostname domain with a TLD label. Quoted local parts, address
//! literals (`user@[127.0.0.1]`) and dotless domains (`user@localhost`) are
//! deliberately rejected — accounts need a deliverable, real-world address.

/// Maximum total length of an address (RFC 5321 path limit minus brackets).
const EMAIL_MAX_LEN: usize = 254;
/// Maximum length of the local part (RFC 5321).
const EMAIL_LOCAL_MAX_LEN: usize = 64;

/// Validate an email address. Returns a human-readable reason on failure.
pub fn validate_email(email: &str) -> Result<(), &'static str> {
    let email = email.trim();
    if email.is_empty() {
        return Err("Email is required");
    }
    if email.len() > EMAIL_MAX_LEN {
        return Err("Email is too long");
    }
    // Split on the LAST '@' — the domain may not contain one, and we don't
    // support quoted local parts where '@' could legally appear.
    let Some(at) = email.rfind('@') else {
        return Err("Email must contain an @");
    };
    let (local, domain) = (&email[..at], &email[at + 1..]);

    if local.is_empty() || local.len() > EMAIL_LOCAL_MAX_LEN {
        return Err("Email has an invalid local part");
    }
    // Dot-atom: atext characters separated by single dots.
    if local.starts_with('.') || local.ends_with('.') || local.contains("..") {
        return Err("Email has an invalid local part");
    }
    for c in local.chars() {
        let ok = c.is_ascii_alphanumeric() || "!#$%&'*+/=?^_`{|}~-.".contains(c);
        if !ok {
            return Err("Email has an invalid local part");
        }
    }

    validate_email_domain(domain)
}

/// Validate the domain half: dotted labels, letter-digit-hyphen, with a TLD.
fn validate_email_domain(domain: &str) -> Result<(), &'static str> {
    if domain.is_empty() || domain.len() > 253 {
        return Err("Email has an invalid domain");
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        // user@localhost etc. — not deliverable on the open internet.
        return Err("Email domain must include a dot (e.g. example.org)");
    }
    for label in &labels {
        if label.is_empty() || label.len() > 63 {
            return Err("Email has an invalid domain");
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err("Email has an invalid domain");
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err("Email has an invalid domain");
        }
    }
    // The TLD label must not be all-numeric (no IP-address lookalikes).
    let tld = labels.last().unwrap();
    if tld.len() < 2 || tld.chars().all(|c| c.is_ascii_digit()) {
        return Err("Email has an invalid domain");
    }
    Ok(())
}

/// Validate a username: 3–50 chars, ASCII letters/digits plus `._-`, starting
/// with a letter or digit. (Existing accounts predating this rule are
/// grandfathered — the check runs only on registration and rename.)
pub fn validate_username(username: &str) -> Result<(), &'static str> {
    if username.len() < 3 || username.len() > 50 {
        return Err("Username must be 3-50 characters");
    }
    let mut chars = username.chars();
    let first = chars.next().unwrap();
    if !first.is_ascii_alphanumeric() {
        return Err("Username must start with a letter or digit");
    }
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err("Username may only contain letters, digits, '.', '_' and '-'");
    }
    Ok(())
}

/// Validate a password: length window only (Argon2id + lockout do the heavy
/// lifting; arbitrary composition rules reduce real-world entropy).
pub fn validate_password(password: &str) -> Result<(), &'static str> {
    if password.len() < 8 || password.len() > 1024 {
        return Err("Password must be between 8 and 1024 characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_addresses() {
        for ok in [
            "alice@example.org",
            "a.b-c_d+tag@sub.example.co.uk",
            "x@y.zw",
            "UPPER.case@EXAMPLE.ORG",
            "o'brien@example.ie",
            "user@xn--p1ai.example",
            "1234567890@example.com",
            "u@e-x.org",
        ] {
            assert!(validate_email(ok).is_ok(), "{ok} should be accepted");
        }
    }

    #[test]
    fn rejects_faulty_addresses() {
        for bad in [
            "",
            "plainaddress",
            "missing-domain@",
            "@missing-local.org",
            "two@@ats.org",
            "user@localhost",      // dotless domain
            "user@example",        // dotless domain
            "user@.example.org",   // empty label
            "user@example..org",   // empty label
            "user@example.org.",   // trailing dot → empty label
            "user@-bad.org",       // label starts with hyphen
            "user@bad-.org",       // label ends with hyphen
            "user@exa mple.org",   // space in domain
            "us er@example.org",   // space in local
            ".user@example.org",   // leading dot
            "user.@example.org",   // trailing dot
            "us..er@example.org",  // double dot
            "user@example.123",    // all-numeric TLD
            "user@example.o",      // 1-char TLD
            "user@127.0.0.1",      // IP-style domain
            "user@exam\nple.org",  // embedded control char (leading/trailing are trimmed)
            "<script>@example.org",
        ] {
            assert!(validate_email(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn rejects_overlong_addresses() {
        let long_local = format!("{}@example.org", "a".repeat(65));
        assert!(validate_email(&long_local).is_err());
        let long_total = format!("{}@example.org", "a".repeat(250));
        assert!(validate_email(&long_total).is_err());
        // At the limits → fine.
        let max_local = format!("{}@example.org", "a".repeat(64));
        assert!(validate_email(&max_local).is_ok());
    }

    #[test]
    fn username_rules() {
        for ok in ["abc", "Alice", "a-b_c.d", "u123", "0start"] {
            assert!(validate_username(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in [
            "ab",                  // too short
            &"x".repeat(51),       // too long
            "-lead",               // bad first char
            ".lead",               // bad first char
            "has space",           // space
            "tab\there",           // control
            "emoji😀",             // non-ASCII
            "semi;colon",          // punctuation
            "slash/name",          // path-ish
        ] {
            assert!(validate_username(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn password_rules() {
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password("1234567").is_err());
        assert!(validate_password(&"x".repeat(1024)).is_ok());
        assert!(validate_password(&"x".repeat(1025)).is_err());
    }
}
