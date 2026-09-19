//! The follower's HTTP client against a leader that rate-limits it.
//!
//! A leader answers `429 Too Many Requests` with a `Retry-After` from its
//! per-IP limiter, to a follower like to any other client. A follower that
//! took that as a failed catch-up restarted the whole catch-up on its next
//! tick — its bootstrap, every graph in scope again from the first — which
//! is exactly what kept the limiter drained: the two-container example
//! (`docker-compose.replication.yml`) never bootstrapped, `resyncs` climbing
//! past a hundred with nothing applied. The client now waits the
//! `Retry-After` out — a whole second when the header says `0`, because the
//! leader truncates to seconds and a `0` means "under a second" — and
//! sends the *same* request again, bounded.
//!
//! The "leader" here is a local listener that throttles the first N answers
//! of a route and then behaves.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use open_triplestore::store::replication::{
    retry_after_wait, HttpLeader, LeaderSource, RETRY_AFTER_ATTEMPTS, RETRY_AFTER_MAX,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Answers `429` with `Retry-After: {retry_after}` to the first `throttle`
/// requests, then `then` with a body.
#[derive(Clone)]
struct Leader {
    hits: Arc<AtomicUsize>,
    throttle: usize,
    retry_after: &'static str,
    then: StatusCode,
}

impl Leader {
    fn answer(&self, body: &'static str) -> (StatusCode, HeaderMap, &'static str) {
        let n = self.hits.fetch_add(1, Ordering::SeqCst);
        if n < self.throttle {
            let mut h = HeaderMap::new();
            h.insert("Retry-After", self.retry_after.parse().unwrap());
            return (StatusCode::TOO_MANY_REQUESTS, h, "Rate limit reached");
        }
        (self.then, HeaderMap::new(), body)
    }
}

const MANIFEST: &str =
    r#"{"epoch":"e1","newest_seq":7,"capture_enabled":true,"graphs":[null],"datasets":[]}"#;

async fn manifest(State(l): State<Leader>) -> impl IntoResponse {
    l.answer(MANIFEST)
}

async fn identity(State(l): State<Leader>) -> impl IntoResponse {
    l.answer("sqlite-bytes")
}

/// The leader on a local listener; its base URL.
fn serve(leader: Leader) -> String {
    let app = Router::new()
        .route("/api/replication/manifest", get(manifest))
        .route("/api/replication/identity", get(identity))
        .with_state(leader);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

fn leader(
    throttle: usize,
    retry_after: &'static str,
    then: StatusCode,
) -> (Arc<AtomicUsize>, String) {
    let hits = Arc::new(AtomicUsize::new(0));
    let base = serve(Leader {
        hits: hits.clone(),
        throttle,
        retry_after,
        then,
    });
    (hits, base)
}

#[test]
fn a_429_is_waited_out_and_the_same_request_sent_again() {
    let (hits, base) = leader(2, "1", StatusCode::OK);
    let t = Instant::now();
    let m = HttpLeader::new(&base, Some("tok"))
        .manifest()
        .expect("the third answer is the manifest");
    assert_eq!(m.epoch, "e1");
    assert_eq!(m.newest_seq, 7);
    assert_eq!(
        hits.load(Ordering::SeqCst),
        3,
        "two throttled answers, then the manifest — the same request, not a restart"
    );
    assert!(
        t.elapsed() >= Duration::from_secs(2),
        "it waited the two Retry-After seconds out, not {:?}",
        t.elapsed()
    );
}

#[test]
fn the_bytes_route_waits_too_and_a_zero_means_a_second() {
    // The identity snapshot goes through the other request path. The
    // leader's limiter truncates its `Retry-After` to whole seconds, so a
    // `0` is "under a second", and the client waits a whole one rather
    // than sending the same request again at once.
    let (hits, base) = leader(1, "0", StatusCode::OK);
    let t = Instant::now();
    let bytes = HttpLeader::new(&base, Some("tok"))
        .identity_snapshot()
        .expect("the second answer is the snapshot");
    assert_eq!(bytes, b"sqlite-bytes");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert!(
        t.elapsed() >= Duration::from_secs(1),
        "a zero is a second, not now: {:?}",
        t.elapsed()
    );
}

#[test]
fn a_leader_that_never_stops_throttling_is_given_up_on() {
    // Eight one-second waits (a `0` is read as a second), then an error.
    let (hits, base) = leader(usize::MAX, "0", StatusCode::OK);
    let t = Instant::now();
    let err = HttpLeader::new(&base, Some("tok"))
        .manifest()
        .expect_err("a permanent 429 is an error, not a hang");
    assert!(err.contains("HTTP 429"), "{err}");
    assert!(
        t.elapsed() >= Duration::from_secs(RETRY_AFTER_ATTEMPTS as u64),
        "every wait was honoured: {:?}",
        t.elapsed()
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        RETRY_AFTER_ATTEMPTS + 1,
        "the first answer plus one retry per allowed wait"
    );
}

#[test]
fn a_missing_retry_after_still_waits_and_retries() {
    // No header: a second's wait is assumed rather than an immediate retry.
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    let app = Router::new().route(
        "/api/replication/manifest",
        get(move || {
            let h = h.clone();
            async move {
                if h.fetch_add(1, Ordering::SeqCst) == 0 {
                    (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response()
                } else {
                    (StatusCode::OK, MANIFEST).into_response()
                }
            }
        }),
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, app).await.unwrap();
        });
    });
    let base = format!("http://{}", rx.recv().unwrap());
    let t = Instant::now();
    let m = HttpLeader::new(&base, None)
        .manifest()
        .expect("second answer");
    assert_eq!(m.epoch, "e1");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert!(t.elapsed() >= Duration::from_secs(1), "{:?}", t.elapsed());
}

/// The header's value is what is waited, not a flat second: a `3` waits
/// three, and the reading is pinned as a function so the cap is too.
#[test]
fn the_retry_after_value_is_honoured_and_capped() {
    let (hits, base) = leader(1, "3", StatusCode::OK);
    let t = Instant::now();
    HttpLeader::new(&base, Some("tok"))
        .manifest()
        .expect("the second answer is the manifest");
    assert_eq!(hits.load(Ordering::SeqCst), 2);
    assert!(
        t.elapsed() >= Duration::from_secs(3),
        "a 3 is three seconds, not one: {:?}",
        t.elapsed()
    );

    assert_eq!(retry_after_wait(None), Duration::from_secs(1));
    assert_eq!(retry_after_wait(Some("0")), Duration::from_secs(1));
    assert_eq!(retry_after_wait(Some(" 3 ")), Duration::from_secs(3));
    assert_eq!(retry_after_wait(Some("soon")), Duration::from_secs(1));
    assert_eq!(retry_after_wait(Some("999")), RETRY_AFTER_MAX);
}

#[test]
fn any_other_failure_is_returned_at_once() {
    let (hits, base) = leader(0, "1", StatusCode::INTERNAL_SERVER_ERROR);
    let err = HttpLeader::new(&base, Some("tok"))
        .manifest()
        .expect_err("a 500 is not something to wait out");
    assert!(err.contains("HTTP 500"), "{err}");
    assert_eq!(hits.load(Ordering::SeqCst), 1, "no retry");
}
