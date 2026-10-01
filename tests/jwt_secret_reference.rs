//! `JWT_SECRET` goes through the secrets module like every other credential:
//! a reference (`file:/path`, `env:NAME`) resolves to the signing secret, and
//! a raw value is refused under `OTS_ENV=production` with the module's
//! `RawSecretRefused` message — the error the server stops with at startup
//! (`main` bails with exactly this helper's error).
//!
//! Own test binary on purpose: `OTS_ENV` is process-wide, and the library's
//! own tests assume the development posture.

use open_triplestore::auth::jwt::configured_jwt_secret;

const SECRET: &str = "a-strong-unique-signing-secret-of-64-chars-0123456789abcdef0123";

/// The posture is process state, so the two postures are exercised in order
/// from one test, and the variable is cleared again whatever happens.
#[test]
fn a_reference_starts_and_a_raw_value_is_refused_in_production() {
    std::env::remove_var("OTS_ENV");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("jwt_secret");
    std::fs::write(&path, format!("{SECRET}\n")).unwrap();
    let reference = format!("file:{}", path.display());

    // Development: a reference resolves, and a raw value is still accepted.
    assert_eq!(configured_jwt_secret(&reference).unwrap(), SECRET);
    assert_eq!(configured_jwt_secret(SECRET).unwrap(), SECRET);
    std::env::set_var("OTS_JWT_TEST_SECRET_9c1e", SECRET);
    assert_eq!(
        configured_jwt_secret("env:OTS_JWT_TEST_SECRET_9c1e").unwrap(),
        SECRET
    );

    // Production: the reference still starts; a raw value is refused with the
    // module's message, which names the setting and never the value.
    std::env::set_var("OTS_ENV", "production");
    let outcome = (
        configured_jwt_secret(&reference),
        configured_jwt_secret(SECRET),
    );
    std::env::remove_var("OTS_ENV");
    std::env::remove_var("OTS_JWT_TEST_SECRET_9c1e");

    assert_eq!(outcome.0.unwrap(), SECRET, "a file: reference starts");
    let err = outcome
        .1
        .expect_err("a raw JWT_SECRET is refused")
        .to_string();
    assert!(
        err.starts_with("JWT_SECRET: ") && err.contains("holds a raw secret value"),
        "the RawSecretRefused message, prefixed with the setting: {err}"
    );
    assert!(
        err.contains("OTS_ENV=production") && err.contains("env:NAME"),
        "the message says what to do instead: {err}"
    );
    assert!(!err.contains(SECRET), "the value never appears: {err}");
}
