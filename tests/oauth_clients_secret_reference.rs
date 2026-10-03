//! `OAUTH_CLIENTS_JSON` client secrets go through the secrets module like
//! every other credential: a reference (`file:/path`, `env:NAME`) resolves to
//! the stored secret, and a raw value is refused under `OTS_ENV=production`
//! with the module's `RawSecretRefused` message — the error the server stops
//! with at startup — before any client is written.
//!
//! Own test binary on purpose: `OTS_ENV` is process-wide, and the library's
//! own tests assume the development posture.

use open_triplestore::auth::db::AuthDb;
use open_triplestore::auth::oidc_provider::seed_clients_from_json;
use open_triplestore::auth::secret::decrypt_secret;

const JWT: &str = "a-strong-unique-signing-secret-of-64-chars-0123456789abcdef0123";
const CLIENT_SECRET: &str = "confidential-client-secret-0123456789";

fn seed(client_id: &str, secret: &str) -> String {
    serde_json::json!([{
        "client_id": client_id,
        "name": "Backend App",
        "public": false,
        "redirect_uris": ["https://app.example.org/callback"],
        "secret": secret,
    }])
    .to_string()
}

fn stored_secret(db: &AuthDb, client_id: &str) -> Option<String> {
    db.get_oauth_client(client_id)
        .unwrap()
        .and_then(|c| c.secret_enc)
        .map(|enc| decrypt_secret(&enc, JWT).unwrap())
}

/// The posture is process state, so the two postures are exercised in order
/// from one test, and the variable is cleared again whatever happens.
#[test]
fn a_reference_seeds_and_a_raw_secret_is_refused_in_production() {
    std::env::remove_var("OTS_ENV");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("client_secret");
    std::fs::write(&path, format!("{CLIENT_SECRET}\n")).unwrap();
    let reference = format!("file:{}", path.display());
    let db = AuthDb::in_memory().unwrap();

    // Development: a reference resolves to its value, and a raw value is
    // still accepted.
    seed_clients_from_json(&db, JWT, &seed("by-ref", &reference)).unwrap();
    assert_eq!(stored_secret(&db, "by-ref").as_deref(), Some(CLIENT_SECRET));
    seed_clients_from_json(&db, JWT, &seed("raw-dev", CLIENT_SECRET)).unwrap();
    assert_eq!(
        stored_secret(&db, "raw-dev").as_deref(),
        Some(CLIENT_SECRET)
    );

    // Production: the reference still seeds; a raw value is refused, and a
    // batch holding one writes nothing at all.
    std::env::set_var("OTS_ENV", "production");
    let mixed = serde_json::json!([
        {"client_id": "prod-ref", "name": "A", "public": false,
         "redirect_uris": ["https://a.example.org/cb"], "secret": reference},
        {"client_id": "prod-raw", "name": "B", "public": false,
         "redirect_uris": ["https://b.example.org/cb"], "secret": CLIENT_SECRET},
    ])
    .to_string();
    let outcome = (
        seed_clients_from_json(&db, JWT, &seed("prod-only-ref", &reference)),
        seed_clients_from_json(&db, JWT, &mixed),
    );
    std::env::remove_var("OTS_ENV");

    outcome.0.expect("a file: reference seeds in production");
    assert_eq!(
        stored_secret(&db, "prod-only-ref").as_deref(),
        Some(CLIENT_SECRET)
    );
    let err = outcome
        .1
        .expect_err("a raw client secret is refused")
        .to_string();
    assert!(
        err.contains("OAUTH_CLIENTS_JSON client 'prod-raw' secret")
            && err.contains("holds a raw secret value")
            && err.contains("OTS_ENV=production"),
        "the RawSecretRefused message, naming the client: {err}"
    );
    assert!(
        !err.contains(CLIENT_SECRET),
        "the value never appears: {err}"
    );
    assert!(
        db.get_oauth_client("prod-ref").unwrap().is_none(),
        "a refused batch seeds none of its clients"
    );
}

/// A reference that does not resolve stops the seed in every posture rather
/// than storing the reference text as if it were the secret.
#[test]
fn an_unresolvable_reference_is_an_error_not_a_secret() {
    let db = AuthDb::in_memory().unwrap();
    let err = seed_clients_from_json(
        &db,
        JWT,
        &seed("missing", "env:OTS_TEST_OAUTH_CLIENT_SECRET_UNSET_5d2a"),
    )
    .expect_err("an unset env: reference is refused");
    assert!(err.to_string().contains("could not be resolved"), "{err}");
    assert!(db.get_oauth_client("missing").unwrap().is_none());
}
