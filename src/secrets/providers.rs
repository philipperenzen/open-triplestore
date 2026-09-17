//! Resolution back-ends for [`SecretRef`](super::SecretRef): the process
//! environment, a file, and HashiCorp Vault (KV v1 and v2).

use super::{Secret, SecretError, SecretRef};

pub const VAULT_ADDR_ENV: &str = "VAULT_ADDR";
pub const VAULT_TOKEN_ENV: &str = "VAULT_TOKEN";
pub const VAULT_TOKEN_FILE_ENV: &str = "VAULT_TOKEN_FILE";
pub const VAULT_NAMESPACE_ENV: &str = "VAULT_NAMESPACE";
pub const VAULT_CACERT_ENV: &str = "VAULT_CACERT";

pub(super) fn resolve(reference: &SecretRef) -> Result<Secret, SecretError> {
    let unresolvable = |reason: String| SecretError::Unresolvable {
        reference: reference.to_string(),
        reason,
    };
    match reference {
        SecretRef::Env { name } => match std::env::var(name) {
            Ok(v) if !v.is_empty() => Ok(Secret::new(v)),
            Ok(_) => Err(unresolvable(format!(
                "environment variable {name} is empty"
            ))),
            Err(_) => Err(unresolvable(format!(
                "environment variable {name} is not set"
            ))),
        },
        SecretRef::File { path } => match std::fs::read_to_string(path) {
            Ok(contents) => {
                let v = contents.trim_end_matches(['\n', '\r']);
                if v.is_empty() {
                    Err(unresolvable("the file is empty".to_string()))
                } else {
                    Ok(Secret::new(v))
                }
            }
            // The OS error text names only the path (already in the
            // reference), never file contents.
            Err(e) => Err(unresolvable(format!("cannot read the file: {}", e.kind()))),
        },
        SecretRef::Vault {
            mount,
            path,
            key,
            kv_version,
        } => vault::read(mount, path, key, *kv_version)
            .map(Secret::new)
            .map_err(unresolvable),
    }
}

mod vault {
    use super::*;
    use std::sync::OnceLock;

    fn client() -> Result<&'static reqwest::Client, String> {
        static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
        CLIENT
            .get_or_init(|| {
                let mut b = reqwest::Client::builder()
                    .user_agent(concat!("open-triplestore/", env!("CARGO_PKG_VERSION")))
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(crate::remote::timeout());
                if let Ok(ca) = std::env::var(VAULT_CACERT_ENV) {
                    let pem = std::fs::read(&ca).map_err(|e| {
                        format!("{VAULT_CACERT_ENV}: cannot read {ca}: {}", e.kind())
                    })?;
                    let cert = reqwest::Certificate::from_pem(&pem)
                        .map_err(|e| format!("{VAULT_CACERT_ENV}: not a PEM certificate: {e}"))?;
                    b = b.add_root_certificate(cert);
                }
                b.build().map_err(|e| format!("vault client: {e}"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    fn token() -> Result<String, String> {
        if let Ok(file) = std::env::var(VAULT_TOKEN_FILE_ENV) {
            let t = std::fs::read_to_string(&file).map_err(|e| {
                format!(
                    "{VAULT_TOKEN_FILE_ENV}: cannot read the token file: {}",
                    e.kind()
                )
            })?;
            let t = t.trim().to_string();
            if t.is_empty() {
                return Err(format!("{VAULT_TOKEN_FILE_ENV}: the token file is empty"));
            }
            return Ok(t);
        }
        match std::env::var(VAULT_TOKEN_ENV) {
            Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
            _ => Err(format!(
                "no Vault token: set {VAULT_TOKEN_FILE_ENV} (Vault Agent sink) or {VAULT_TOKEN_ENV}"
            )),
        }
    }

    pub(super) fn read(
        mount: &str,
        path: &str,
        key: &str,
        kv_version: u8,
    ) -> Result<String, String> {
        let addr = std::env::var(VAULT_ADDR_ENV)
            .ok()
            .map(|a| a.trim().trim_end_matches('/').to_string())
            .filter(|a| !a.is_empty())
            .ok_or_else(|| format!("{VAULT_ADDR_ENV} is not set"))?;
        let token = token()?;
        let namespace = std::env::var(VAULT_NAMESPACE_ENV)
            .ok()
            .filter(|n| !n.trim().is_empty());
        let url = if kv_version == 2 {
            format!("{addr}/v1/{mount}/data/{path}")
        } else {
            format!("{addr}/v1/{mount}/{path}")
        };
        let client = client()?;
        let key = key.to_string();
        crate::remote::blocking(async move {
            let mut req = client.get(&url).header("X-Vault-Token", token);
            if let Some(ns) = namespace {
                req = req.header("X-Vault-Namespace", ns);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| format!("Vault request failed: {}", e.without_url()))?;
            let status = resp.status();
            if !status.is_success() {
                return Err(format!("Vault answered {status}"));
            }
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|_| "Vault answered with a body that is not JSON".to_string())?;
            let data = if kv_version == 2 {
                &body["data"]["data"]
            } else {
                &body["data"]
            };
            match data.get(&key) {
                Some(serde_json::Value::String(s)) if !s.is_empty() => Ok(s.clone()),
                Some(serde_json::Value::Null) | None => {
                    Err(format!("the secret has no field named '{key}'"))
                }
                Some(other) => Ok(other.to_string()),
            }
        })
    }
}
