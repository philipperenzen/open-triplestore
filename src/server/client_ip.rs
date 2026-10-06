//! The requesting client's IP address, derived the same way everywhere.
//!
//! The rate limiter, the audit log and the LLM guard's anonymous budget all
//! need to know who is calling. `X-Forwarded-For` and `X-Real-IP` are plain
//! request headers that any client can set, so they are believed only when
//! the TCP peer is one of the reverse proxies listed in `TRUSTED_PROXY_CIDRS`.
//! Otherwise the TCP peer address is the client.

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::extract::{ConnectInfo, FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{Extensions, HeaderMap};
use ipnet::IpNet;

/// The reverse proxies whose forwarded-for headers are believed
/// (`TRUSTED_PROXY_CIDRS`). Empty means no proxy: the TCP peer is the client.
#[derive(Clone, Debug, Default)]
pub struct TrustedProxies(Arc<Vec<IpNet>>);

impl TrustedProxies {
    pub fn new(cidrs: Vec<IpNet>) -> Self {
        Self(Arc::new(cidrs))
    }

    pub fn contains(&self, ip: &IpAddr) -> bool {
        self.0.iter().any(|cidr| cidr.contains(ip))
    }
}

/// The TCP peer address, present when the server runs with
/// `into_make_service_with_connect_info` (production always does).
pub fn peer_ip(extensions: &Extensions) -> Option<IpAddr> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip())
}

/// The client IP for a request that arrived from `peer`.
///
/// Forwarded headers count only when `peer` is a trusted proxy. The
/// `X-Forwarded-For` chain is then walked right to left, skipping trusted
/// hops; the first untrusted address is the client. The left-most entry is
/// whatever the client wrote, so trusting it would let a caller behind the
/// proxy pose as any address. `X-Real-IP` is the fallback, then the peer.
/// `None` only when there is no peer address at all.
pub fn resolve(
    headers: &HeaderMap,
    peer: Option<IpAddr>,
    trusted: &TrustedProxies,
) -> Option<IpAddr> {
    let peer_is_trusted = peer.is_some_and(|ip| trusted.contains(&ip));
    if peer_is_trusted {
        if let Some(xff) = headers.get("x-forwarded-for").and_then(|h| h.to_str().ok()) {
            for entry in xff.rsplit(',') {
                if let Ok(ip) = entry.trim().parse::<IpAddr>() {
                    if !trusted.contains(&ip) {
                        return Some(ip);
                    }
                }
            }
        }
        if let Some(ip) = headers
            .get("x-real-ip")
            .and_then(|h| h.to_str().ok())
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
        {
            return Some(ip);
        }
    }
    peer
}

/// Extractor for the requesting client's IP (see [`resolve`]). Never rejects:
/// a request without a TCP peer address (an in-process test router) yields
/// `ClientIp(None)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClientIp(pub Option<IpAddr>);

impl ClientIp {
    pub fn from_parts(
        headers: &HeaderMap,
        extensions: &Extensions,
        trusted: &TrustedProxies,
    ) -> Self {
        ClientIp(resolve(headers, peer_ip(extensions), trusted))
    }

    /// The address as the audit log and the LLM log store it.
    pub fn as_string(&self) -> Option<String> {
        self.0.map(|ip| ip.to_string())
    }
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for ClientIp
where
    TrustedProxies: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let trusted = TrustedProxies::from_ref(state);
        Ok(Self::from_parts(
            &parts.headers,
            &parts.extensions,
            &trusted,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted(cidrs: &[&str]) -> TrustedProxies {
        TrustedProxies::new(cidrs.iter().map(|c| c.parse().unwrap()).collect())
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, v.parse().unwrap());
        }
        h
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn forged_header_from_untrusted_peer_is_ignored() {
        let h = headers(&[("x-forwarded-for", "1.2.3.4"), ("x-real-ip", "5.6.7.8")]);
        let got = resolve(&h, Some(ip("203.0.113.9")), &trusted(&["10.0.0.0/8"]));
        assert_eq!(got, Some(ip("203.0.113.9")));
    }

    #[test]
    fn direct_connection_without_proxies_uses_peer() {
        let h = headers(&[("x-forwarded-for", "1.2.3.4")]);
        assert_eq!(
            resolve(&h, Some(ip("198.51.100.7")), &TrustedProxies::default()),
            Some(ip("198.51.100.7"))
        );
    }

    #[test]
    fn trusted_proxy_chain_is_walked_right_to_left() {
        // client forged "1.2.3.4"; the edge proxy appended the real client,
        // then an inner proxy (also trusted) appended the edge's address.
        let h = headers(&[("x-forwarded-for", "1.2.3.4, 198.51.100.7, 10.0.0.2")]);
        let got = resolve(&h, Some(ip("10.0.0.1")), &trusted(&["10.0.0.0/8"]));
        assert_eq!(got, Some(ip("198.51.100.7")));
    }

    #[test]
    fn trusted_proxy_falls_back_to_real_ip_then_peer() {
        let t = trusted(&["10.0.0.0/8"]);
        let h = headers(&[("x-real-ip", " 198.51.100.8 ")]);
        assert_eq!(
            resolve(&h, Some(ip("10.0.0.1")), &t),
            Some(ip("198.51.100.8"))
        );
        let h = headers(&[("x-forwarded-for", "garbage"), ("x-real-ip", "nope")]);
        assert_eq!(resolve(&h, Some(ip("10.0.0.1")), &t), Some(ip("10.0.0.1")));
    }

    #[test]
    fn no_peer_means_unknown_even_with_headers() {
        let h = headers(&[("x-forwarded-for", "1.2.3.4")]);
        assert_eq!(resolve(&h, None, &trusted(&["0.0.0.0/0"])), None);
    }
}
