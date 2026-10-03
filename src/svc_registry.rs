//! Best-effort self-registration with the service registry.
//!
//! Mirrors the Python `registry_client.py` registrar: POST `/register` once, then
//! heartbeat every `ttl/2` seconds so siblings can resolve "triplestore" instead of
//! hardcoding `http://localhost:7878`. Every call is fire-and-forget — the registry
//! being down never affects the triplestore (fail-soft).

use std::time::Duration;

use tracing::{debug, info, warn};

/// The logical name this service advertises in the service registry.
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
        let url = format!("{base}/register");
        let mut last = post(&client, &url, &token, &self_url).await;
        report(&url, None, &last);
        let mut ticker = tokio::time::interval(Duration::from_secs((TTL_SECONDS / 2).max(1)));
        ticker.tick().await; // the first tick fires immediately; we just registered, so skip it
        let url = format!("{base}/heartbeat");
        loop {
            ticker.tick().await;
            let now = post(&client, &url, &token, &self_url).await;
            report(&url, Some(&last), &now);
            last = now;
        }
    });
}

/// What one registry call came back with.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    /// A 2xx answer.
    Accepted,
    /// The registry answered with a non-2xx status (a 401 for a missing or
    /// wrong token, a 500, …): the store is *not* registered.
    Rejected(u16),
    /// No answer at all (registry down, timeout). Expected wherever no
    /// registry runs, so it stays a debug line.
    Unreachable(String),
}

async fn post(client: &reqwest::Client, url: &str, token: &str, self_url: &str) -> Outcome {
    let body = serde_json::json!({
        "name": SERVICE_NAME,
        "url": self_url,
        "ttl_seconds": TTL_SECONDS,
    });
    let mut req = client.post(url).json(&body).timeout(Duration::from_secs(2));
    if !token.is_empty() {
        req = req.bearer_auth(token);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => Outcome::Accepted,
        Ok(resp) => Outcome::Rejected(resp.status().as_u16()),
        Err(e) => Outcome::Unreachable(e.to_string()),
    }
}

/// Whether a rejection deserves a warning: the first one, and each change of
/// status after that — not one line per heartbeat while it persists.
fn warn_on(prev: Option<&Outcome>, now: &Outcome) -> bool {
    matches!(now, Outcome::Rejected(_)) && prev != Some(now)
}

fn report(url: &str, prev: Option<&Outcome>, now: &Outcome) {
    match now {
        Outcome::Accepted => {
            if matches!(prev, Some(Outcome::Rejected(_))) {
                info!("service-registry: POST {url} accepted again");
            }
        }
        Outcome::Rejected(status) if warn_on(prev, now) => {
            let hint = if *status == 401 || *status == 403 {
                " — check LD_REGISTRY_TOKEN"
            } else {
                ""
            };
            warn!("service-registry: POST {url} answered HTTP {status}; this store is not registered{hint}");
        }
        Outcome::Rejected(status) => {
            debug!("service-registry: POST {url} answered HTTP {status} (still)");
        }
        Outcome::Unreachable(e) => {
            debug!("service-registry: POST {url} failed (ignored): {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rejection_warns_once_per_status() {
        let r401 = Outcome::Rejected(401);
        let r500 = Outcome::Rejected(500);
        assert!(warn_on(None, &r401), "the first rejection warns");
        assert!(!warn_on(Some(&r401), &r401), "a persisting one does not");
        assert!(warn_on(Some(&r401), &r500), "a new status warns again");
        assert!(warn_on(Some(&Outcome::Accepted), &r401));
        assert!(warn_on(Some(&Outcome::Unreachable("x".into())), &r401));
        assert!(!warn_on(None, &Outcome::Accepted));
        assert!(!warn_on(None, &Outcome::Unreachable("refused".into())));
    }

    #[tokio::test]
    async fn a_non_2xx_answer_is_a_rejection_not_success() {
        use axum::{http::StatusCode, routing::post as route_post, Router};
        let app = Router::new()
            .route(
                "/register",
                route_post(|| async { StatusCode::UNAUTHORIZED }),
            )
            .route("/heartbeat", route_post(|| async { StatusCode::OK }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::new();
        let base = format!("http://{addr}");
        assert_eq!(
            post(&client, &format!("{base}/register"), "", "http://s").await,
            Outcome::Rejected(401)
        );
        assert_eq!(
            post(&client, &format!("{base}/heartbeat"), "", "http://s").await,
            Outcome::Accepted
        );
    }
}
