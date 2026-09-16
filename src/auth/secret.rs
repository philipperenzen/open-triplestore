//! Storage for sensitive credentials held in the identity database (OAuth /
//! OIDC client secrets).
//!
//! Two forms share the column:
//!
//! * a **secret reference** ([`crate::secrets::SecretRef`]) — `env:NAME`,
//!   `file:/path`, `vault:<mount>/data/<path>#<key>` — stored verbatim and
//!   resolved at the moment of use. This is the form a production deployment
//!   uses: the store holds a pointer, rotation happens in the secret store,
//!   and nothing here ever persists the value.
//! * a **legacy encrypted blob** — AES-256-GCM with a 96-bit random nonce,
//!   the key derived from the JWT secret via HKDF-SHA256, encoded as
//!   `base64(nonce || ciphertext)`. Kept so deployments that predate secret
//!   references keep working; accepted with a deprecation warning outside the
//!   production posture and refused inside it.
//!
//! The two are told apart by syntax: a reference carries a `:` after a known
//! scheme, which base64 never produces.

use aes_gcm::{
    aead::{Aead, Generate, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hkdf::Hkdf;
use sha2::Sha256;

const HKDF_INFO: &[u8] = b"open-triplestore oauth-secrets v1";

/// Derive a 32-byte AES key from the JWT secret.
fn derive_key(jwt_secret: &str) -> Key<Aes256Gcm> {
    let hk = Hkdf::<Sha256>::new(None, jwt_secret.as_bytes());
    let mut okm = [0u8; 32];
    hk.expand(HKDF_INFO, &mut okm)
        .expect("HKDF expand failed (output too long)");
    Key::<Aes256Gcm>::try_from(okm.as_slice()).expect("HKDF OKM is exactly 32 bytes")
}

/// Encrypt `plaintext` and return a base64-encoded `nonce || ciphertext` blob.
pub fn encrypt_secret(plaintext: &str, jwt_secret: &str) -> anyhow::Result<String> {
    let key = derive_key(jwt_secret);
    let cipher = Aes256Gcm::new(&key);
    let nonce = Nonce::generate();
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("AES-GCM encrypt error: {e}"))?;

    let mut blob = nonce.to_vec();
    blob.extend_from_slice(&ciphertext);
    Ok(B64.encode(&blob))
}

/// Decrypt a blob produced by [`encrypt_secret`].
pub fn decrypt_secret(encoded: &str, jwt_secret: &str) -> anyhow::Result<String> {
    let blob = B64
        .decode(encoded)
        .map_err(|e| anyhow::anyhow!("base64 decode error: {e}"))?;

    if blob.len() < 12 {
        anyhow::bail!("encrypted blob too short");
    }

    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let nonce =
        Nonce::try_from(nonce_bytes).map_err(|_| anyhow::anyhow!("invalid nonce length"))?;
    let key = derive_key(jwt_secret);
    let cipher = Aes256Gcm::new(&key);
    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("AES-GCM decrypt error: {e}"))?;

    String::from_utf8(plaintext).map_err(|e| anyhow::anyhow!("UTF-8 decode error: {e}"))
}

/// Turn a caller-supplied secret into the value to store.
///
/// A reference is validated (well-formed **and** resolvable) and stored as
/// itself. A raw value is refused in the production posture and, elsewhere,
/// encrypted with a deprecation warning naming `setting`.
pub fn store_configured_secret(
    setting: &str,
    value: &str,
    jwt_secret: &str,
) -> anyhow::Result<String> {
    match crate::secrets::configured(setting, value)? {
        crate::secrets::ConfiguredSecret::Reference(r) => {
            crate::secrets::validate(&r)?;
            Ok(r.to_string())
        }
        crate::secrets::ConfiguredSecret::Raw(raw) => encrypt_secret(raw.expose(), jwt_secret),
    }
}

/// Read back whatever [`store_configured_secret`] wrote.
pub fn read_stored_secret(stored: &str, jwt_secret: &str) -> anyhow::Result<String> {
    if crate::secrets::looks_like_ref(stored) {
        let reference = crate::secrets::SecretRef::parse(stored)?;
        return Ok(crate::secrets::resolve(&reference)?.expose().to_string());
    }
    decrypt_secret(stored, jwt_secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_is_stored_as_itself_and_resolved_on_read() {
        std::env::set_var("OTS_AUTH_SECRET_TEST", "client-secret-value");
        let stored =
            store_configured_secret("client_secret", "env:OTS_AUTH_SECRET_TEST", "jwt").unwrap();
        assert_eq!(stored, "env:OTS_AUTH_SECRET_TEST", "the pointer is what lands in the database");
        assert_eq!(
            read_stored_secret(&stored, "jwt").unwrap(),
            "client-secret-value"
        );
        // An unresolvable pointer is refused at write time, not at login time.
        assert!(store_configured_secret("client_secret", "env:OTS_AUTH_SECRET_MISSING", "jwt").is_err());
    }

    #[test]
    fn a_legacy_blob_still_reads_back() {
        // Outside the production posture a raw value is encrypted as before.
        let stored = store_configured_secret("client_secret", "plaintext-secret", "jwt").unwrap();
        assert_ne!(stored, "plaintext-secret");
        assert!(!crate::secrets::looks_like_ref(&stored));
        assert_eq!(read_stored_secret(&stored, "jwt").unwrap(), "plaintext-secret");
    }

    #[test]
    fn round_trip() {
        let secret = "super-secret-client-secret-value";
        let jwt_key = "my-jwt-secret-key";
        let enc = encrypt_secret(secret, jwt_key).unwrap();
        let dec = decrypt_secret(&enc, jwt_key).unwrap();
        assert_eq!(dec, secret);
    }

    #[test]
    fn wrong_key_fails() {
        let enc = encrypt_secret("value", "key1").unwrap();
        assert!(decrypt_secret(&enc, "key2").is_err());
    }

    #[test]
    fn each_encryption_unique() {
        let enc1 = encrypt_secret("same", "key").unwrap();
        let enc2 = encrypt_secret("same", "key").unwrap();
        // Different nonces → different ciphertext
        assert_ne!(enc1, enc2);
    }
}
