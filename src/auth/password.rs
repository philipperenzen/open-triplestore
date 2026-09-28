use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

/// Hash a password with Argon2id.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    // Since argon2 0.6 the hasher draws the salt itself: 16 random bytes from
    // the OS RNG (the crate's default `getrandom` feature) — the same salt
    // length `SaltString::generate(&mut OsRng)` produced under 0.5.
    let hash = Argon2::default()
        .hash_password(password.as_bytes())
        .map_err(|e| anyhow::anyhow!("Password hashing failed: {}", e))?;
    Ok(hash.to_string())
}

/// Verify a password against its Argon2id hash.
pub fn verify_password(password: &str, hash: &str) -> anyhow::Result<bool> {
    let parsed_hash =
        PasswordHash::new(hash).map_err(|e| anyhow::anyhow!("Invalid password hash: {}", e))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_and_verify() {
        let hash = hash_password("secret123").unwrap();
        assert!(verify_password("secret123", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    /// PHC strings minted by the code path above as it was before the argon2
    /// 0.6 upgrade (argon2 0.5.3 / password-hash 0.5: `Argon2::default()` with
    /// a `SaltString::generate(&mut OsRng)` salt) — the exact shape of every
    /// `users.password_hash` written before it. The parameters travel in the
    /// string, so these must keep verifying under any later argon2 release, or
    /// every existing account is locked out.
    const HASHES_FROM_ARGON2_0_5: &[(&str, &str)] = &[
        (
            "correct horse battery staple",
            "$argon2id$v=19$m=19456,t=2,p=1$ONNupglNTmqHzmpwPSgvoQ$cQAE2ih2ViY0++pqRYmTFGhN8ZdEDzrjApOlVwkrmoI",
        ),
        (
            "Wachtwoord-ë€ 2026!",
            "$argon2id$v=19$m=19456,t=2,p=1$/3mPoeUI/X5VrAiLC8o8fA$Eiz/xr51e0xDLjZGrCPhf3OwCPImMbNAyd8VbNMlNfA",
        ),
    ];

    #[test]
    fn hashes_stored_before_the_argon2_0_6_upgrade_still_verify() {
        for (password, hash) in HASHES_FROM_ARGON2_0_5 {
            assert!(
                verify_password(password, hash).unwrap(),
                "a stored pre-upgrade hash no longer verifies: {hash}"
            );
            assert!(!verify_password("wrong", hash).unwrap());
            assert!(!verify_password(&format!("{password} "), hash).unwrap());
            assert!(!verify_password("", hash).unwrap());
        }
    }

    #[test]
    fn new_hashes_keep_the_stored_format() {
        // Same algorithm, version and cost parameters as the rows already in
        // the database, and a 16-byte (22-char B64) salt, so a hash written
        // after the upgrade is indistinguishable from one written before it.
        let hash = hash_password("secret123").unwrap();
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "unexpected PHC prefix: {hash}"
        );
        assert_eq!(hash.split('$').nth(4).map(str::len), Some(22), "{hash}");
        assert_ne!(
            hash_password("secret123").unwrap(),
            hash,
            "salt must be random"
        );
    }

    #[test]
    fn non_phc_values_never_verify() {
        // SSO-only accounts store `oauth:<provider>:<subject>` in password_hash.
        for stored in ["oauth:google:12345", "", "$argon2id$v=19$m=19456,t=2,p=1$"] {
            assert!(
                !verify_password("oauth:google:12345", stored).unwrap_or(false),
                "{stored:?} must not verify"
            );
        }
    }
}
