//! The one place outbound HTTP clients are built, and the process-wide rustls
//! crypto provider.
//!
//! reqwest 0.13 offers rustls only with the aws-lc-rs crypto provider (its
//! `rustls` feature) or with none (`rustls-no-provider`). The rest of the tree
//! (lettre, the AWS SDK, the SQL connectors, openidconnect) runs rustls over
//! ring, so reqwest is built with `rustls-no-provider` and this module installs
//! ring as the process-wide provider before any client is built. A reqwest
//! client built without a provider panics ("No rustls crypto provider is
//! configured"), which is why `clippy.toml` disallows `reqwest::Client::new` /
//! `builder` outside this module.
//!
//! The provider matters beyond reqwest: the SQL Server plugin's tiberius links
//! aws-lc-rs, and with two providers compiled in and none installed, every
//! `rustls::ClientConfig::builder()` (the mysql crate's, for one) panics. `main`
//! therefore installs ring before anything else, and the SQL plugins do so in
//! `connect()`.
//!
//! Server certificates are checked by `rustls-platform-verifier`, i.e. against
//! the operating system's trust store (the `ca-certificates` bundle in the
//! Docker image), so operator-installed CAs are honoured.

use std::sync::Once;

/// Install `ring` as the process-wide rustls crypto provider (idempotent).
pub fn install_crypto_provider() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // Err means a provider is already installed; any provider will do then.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// A [`reqwest::ClientBuilder`] that is safe to build: use instead of
/// `reqwest::Client::builder()`.
#[allow(clippy::disallowed_methods)] // the one sanctioned call site
pub fn builder() -> reqwest::ClientBuilder {
    install_crypto_provider();
    reqwest::Client::builder()
}

/// A default [`reqwest::Client`]: use instead of `reqwest::Client::new()`.
///
/// # Panics
///
/// Like `reqwest::Client::new()`, if the TLS backend cannot be initialised.
pub fn client() -> reqwest::Client {
    builder().build().expect("building the default HTTP client")
}

#[cfg(test)]
mod tests {
    #[test]
    fn clients_build_without_a_preinstalled_provider() {
        let _ = super::client();
        let _ = super::builder()
            .timeout(std::time::Duration::from_secs(1))
            .build()
            .unwrap();
    }
}
