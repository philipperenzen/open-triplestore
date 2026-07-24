//! Networking helpers backing the opt-in `--port-fallback` / `PORT_FALLBACK`
//! flag (default off — see `server::run`'s listener-bind step).
//!
//! Upstream's default behavior is to refuse to start when the requested port
//! is already in use (see the `AddrInUse` branch in `server::run`), rather
//! than silently binding somewhere else. Some deployments — e.g. a CI matrix
//! or a fork that runs many instances on one host — prefer "just find a free
//! port and go". This module is that fallback, kept out of the default path.

use std::io;
use tokio::net::TcpListener;

/// Bind any free TCP port on `host` and return the bound listener directly.
///
/// Binding immediately (rather than probing a port number with
/// [`find_free_port`] and then binding it in a second step) avoids a TOCTOU
/// race where another process grabs the same port between the probe and the
/// real bind. Prefer this over `find_free_port` when you intend to actually
/// serve on the result.
pub async fn bind_free_port(host: &str) -> io::Result<TcpListener> {
    TcpListener::bind((host, 0)).await
}

/// Ask the OS for a free TCP port on `host` without holding it open.
///
/// Useful for logging or tests where only the port number is needed. Since
/// the listener is dropped immediately, another process could in principle
/// grab the same port before it is used — prefer [`bind_free_port`] for the
/// real server-startup path.
// Retained as a documented counterpart to `bind_free_port` (referenced from its
// docs); currently only exercised by tests, so allow it to be otherwise unused.
#[allow(dead_code)]
pub fn find_free_port(host: &str) -> io::Result<u16> {
    let listener = std::net::TcpListener::bind((host, 0))?;
    listener.local_addr().map(|a| a.port())
}

/// Rewrite `base_url`'s port to `new_port`, but ONLY when it currently
/// specifies `old_port` explicitly. A `base_url` that omits the port (e.g. it
/// is fronted by a reverse proxy on 443/80) is left untouched — this rewrite
/// exists purely to keep the advertised URL correct when `--port-fallback`
/// silently moved the bind off the port the operator asked for; it must never
/// invent a port on a URL that never had one.
///
/// Fails soft: an unparseable `base_url` (or one whose port already differs
/// from `old_port`) is returned unchanged rather than erroring, since the
/// caller (port-fallback logging) is best-effort by design.
pub fn rewrite_url_port(base_url: &str, old_port: u16, new_port: u16) -> String {
    let Ok(mut parsed) = url::Url::parse(base_url) else {
        return base_url.to_string();
    };
    if parsed.port() != Some(old_port) {
        return base_url.to_string();
    }
    if parsed.set_port(Some(new_port)).is_err() {
        return base_url.to_string();
    }
    parsed.to_string().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_free_port_returns_a_bindable_port() {
        let port = find_free_port("127.0.0.1").expect("OS grants a free port");
        assert_ne!(port, 0);
        // The port really is bindable (modulo an inherent, unavoidable TOCTOU
        // race against another process — see the fn's docs).
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_ok());
    }

    #[tokio::test]
    async fn bind_free_port_returns_a_listener_on_a_real_port() {
        let listener = bind_free_port("127.0.0.1").await.unwrap();
        assert_ne!(listener.local_addr().unwrap().port(), 0);
    }

    #[test]
    fn rewrite_url_port_replaces_matching_port() {
        assert_eq!(
            rewrite_url_port("http://localhost:7878", 7878, 51234),
            "http://localhost:51234"
        );
        assert_eq!(
            rewrite_url_port("https://example.org:7878/sub", 7878, 51234),
            "https://example.org:51234/sub"
        );
    }

    #[test]
    fn rewrite_url_port_leaves_non_matching_or_portless_urls_alone() {
        // Fronted by a reverse proxy on the default port — no explicit port.
        assert_eq!(
            rewrite_url_port("https://example.org", 7878, 51234),
            "https://example.org"
        );
        // Explicit port that doesn't match what we're told to replace.
        assert_eq!(
            rewrite_url_port("http://localhost:9000", 7878, 51234),
            "http://localhost:9000"
        );
        // Unparseable input is returned unchanged rather than erroring.
        assert_eq!(rewrite_url_port("not a url", 7878, 51234), "not a url");
    }
}
