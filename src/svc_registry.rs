//! Best-effort self-registration with the service registry.
//!
//! Mirrors the Python `registry_client.py` registrar: POST `/register` once, then
//! heartbeat every `ttl/2` seconds so siblings can resolve "triplestore" instead of
//! hardcoding `http://localhost:7878`. Every call is fire-and-forget — the registry
//! being down never affects the triplestore (fail-soft).

use std::time::Duration;

use tracing::{debug, info};

/// The logical name this service advertises. Mirror of `service-registry/registry/names.py`.
const SERVICE_NAME: &str = "triplestore";
const TTL_SECONDS: u64 = 30;

/// Spawn a background task that registers `self_url` for `triplestore` and heartbeats.
///
/// `self_url` is what clients use to reach this store (the linked-data `base_url`).
/// `registry_url` defaults to `http://localhost:8500`; `token` is sent as a bearer when
/// non-empty (required only when the registry binds a non-loopback host).
pub fn spawn_registrar(self_url: String, registry_url: String, token: String) {
    let base = registry_url.trim_end_matches('/').to_string();
    info!("service-registry: self-registering as '{SERVICE_NAME}' -> {self_url} (registry {base})");
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        post(&client, &format!("{base}/register"), &token, &self_url).await;
        let mut ticker = tokio::time::interval(Duration::from_secs((TTL_SECONDS / 2).max(1)));
        ticker.tick().await; // the first tick fires immediately; we just registered, so skip it
        loop {
            ticker.tick().await;
            post(&client, &format!("{base}/heartbeat"), &token, &self_url).await;
        }
    });
}

async fn post(client: &reqwest::Client, url: &str, token: &str, self_url: &str) {
    let body = serde_json::json!({
        "name": SERVICE_NAME,
        "url": self_url,
        "ttl_seconds": TTL_SECONDS,
    });
    let mut req = client.post(url).json(&body).timeout(Duration::from_secs(2));
    if !token.is_empty() {
        req = req.bearer_auth(token);
    }
    if let Err(e) = req.send().await {
        debug!("service-registry: POST {url} failed (ignored): {e}");
    }
}
