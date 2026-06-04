//! Symmetric encryption for sensitive credentials stored in the database
//! (OAuth client secrets, etc.).
//!
//! Uses AES-256-GCM with a 96-bit random nonce.  The encryption key is derived
//! from the JWT secret via HKDF-SHA256 so no extra key material needs to be
//! configured.  The stored blob is `nonce (12 bytes) || ciphertext` encoded as
//! standard base64.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    AeadCore, Aes256Gcm, Key,
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
    *Key::<Aes256Gcm>::from_slice(&okm)
}

/// Encrypt `plaintext` and return a base64-encoded `nonce || ciphertext` blob.
pub fn encrypt_secret(plaintext: &str, jwt_secret: &str) -> anyhow::Result<String> {
    let key = derive_key(jwt_secret);
    let cipher = Aes256Gcm::new(&key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
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
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);
    let key = derive_key(jwt_secret);
    let cipher = Aes256Gcm::new(&key);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("AES-GCM decrypt error: {e}"))?;

    String::from_utf8(plaintext).map_err(|e| anyhow::anyhow!("UTF-8 decode error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
