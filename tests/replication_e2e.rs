//! Replication end to end: a real leader — its router, its authentication,
//! its per-IP rate limiter — on a local listener, and a follower reaching it
//! through the HTTP client the follower thread uses. This is the path the
//! two-container example (`docker-compose.replication.yml`) takes, and the
//! path the in-process tests in `replication.rs` cannot see: the limiter
//! answering a bootstrap of more graphs than its burst with `429`, and a
//! hot follower's long-poll returning the moment a row lands.
//!
//! No network beyond the loopback listener.

mod common;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use common::*;
use open_triplestore::auth::jwt::{generate_api_token, hash_token};
use open_triplestore::auth::models::ApiScope;
use open_triplestore::server::AppState;
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
use open_triplestore::store::replication::{HttpLeader, Mode, ReplicationConfig, Role, Scope};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphNameRef, NamedNodeRef};
use serde_json::Value;
use tower::ServiceExt as _;

/// More graphs than the Graph Store limiter's burst of 40, so a bootstrap
/// has to get through `429`s to finish.
const GRAPHS: usize = 44;

fn graph_iri(n: usize) -> String {
    format!("https://example.org/e2e/g{n}")
}

fn ttl(n: usize) -> String {
    format!("<https://example.org/e2e/s{n}> <https://example.org/e2e/p> <https://example.org/e2e/o{n}> .")
}

/// The graph's quads as sorted N-Triples lines, so two stores compare.
fn graph(store: &TripleStore, g: &str) -> BTreeSet<String> {
    store
        .quads_for_graph(GraphNameRef::NamedNode(NamedNodeRef::new(g).unwrap()))
        .unwrap()
        .into_iter()
        .map(|q| format!("{} {} {}", q.subject, q.predicate, q.object))
        .collect()
}

/// An admin API token minted on the leader for user `adm` — what the compose
/// example puts in `OTS_REPLICATION_TOKEN` — rather than a session JWT: the
/// API-token path is the one that stamps `last_used_at` on every request,
/// which is what moved the identity version under the follower's own polling.
fn mint_api_token(state: &AppState) -> String {
    let raw = generate_api_token();
    state
        .auth_db
        .create_api_token(
            "repl-tok",
            "adm",
            "replica",
            &hash_token(&raw),
            &format!("{}...", &raw[..11]),
            &[ApiScope::Read, ApiScope::Admin],
            None,
        )
        .unwrap();
    raw
}

/// Counts the `429`s the leader hands out, so the test can say the limiter
/// was really in the way.
async fn count_429(State(c): State<Arc<AtomicUsize>>, req: Request, next: Next) -> Response {
    let resp = next.run(req).await;
    if resp.status() == StatusCode::TOO_MANY_REQUESTS {
        c.fetch_add(1, Ordering::SeqCst);
    }
    resp
}

/// The leader's router on a loopback listener; its base URL.
fn serve(app: axum::Router) -> String {
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

async fn call(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<(&str, String)>,
) -> (StatusCode, Value, String) {
    let mut b = axum::http::Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let body = match body {
        Some((ct, s)) => {
            b = b.header(header::CONTENT_TYPE, ct);
            Body::from(s)
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let st = resp.status();
    let txt = body_text(resp.into_body()).await;
    let json = serde_json::from_str(&txt).unwrap_or(Value::Null);
    (st, json, txt)
}

#[test]
fn a_follower_bootstraps_through_the_leaders_rate_limiter_and_tails_it_live() {
    // ── The leader: 44 graphs, capture on, served with its limiter ────────
    let store = TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
        .with_replication(ReplicationConfig::leader());
    let (leader_state, token) = admin_state_with_store(store);
    for n in 0..GRAPHS {
        leader_state
            .store
            .graph_store_put(Some(&graph_iri(n)), &ttl(n), RdfFormat::Turtle)
            .unwrap();
    }
    let throttled = Arc::new(AtomicUsize::new(0));
    let leader_app = test_app(leader_state.clone()).layer(axum::middleware::from_fn_with_state(
        throttled.clone(),
        count_429,
    ));
    let base = serve(leader_app.clone());

    // ── The follower: hot, every graph, the leader's token ───────────────
    let api_token = mint_api_token(&leader_state);
    let mut config = ReplicationConfig::follower(&base, Mode::Hot, Scope::All);
    config.token = Some(api_token.clone());
    config.node_id = "e2e-follower".to_string();
    let (follower_state, _) =
        admin_state_with_store(TripleStore::in_memory().unwrap().with_replication(config));
    let follower = &follower_state.store;
    let follower_app = test_app(follower_state.clone());
    let http = HttpLeader::new(&base, Some(&api_token));

    // ── 1. Bootstrap, through the limiter ────────────────────────────────
    let t = Instant::now();
    let p = follower.replicate_once(&http).expect("bootstrap over HTTP");
    let bootstrap_took = t.elapsed();
    assert!(p.bootstrapped, "{p:?}");
    assert_eq!(p.resyncs, 1, "one bootstrap, not a restart per 429: {p:?}");
    assert!(
        p.refetched_graphs >= GRAPHS,
        "every graph fetched whole: {p:?}"
    );
    // The server reads only these two spellings as "off".
    let limiter_off = matches!(
        std::env::var("RATE_LIMIT_DISABLED").as_deref(),
        Ok("1") | Ok("true")
    );
    if !limiter_off {
        assert!(
            throttled.load(Ordering::SeqCst) > 0,
            "44 graph fetches must trip a limiter with a burst of 40; the test \
             would not be testing the wait otherwise"
        );
    }
    for n in [0, GRAPHS / 2, GRAPHS - 1] {
        let g = graph_iri(n);
        assert_eq!(graph(follower, &g), graph(&leader_state.store, &g), "{g}");
    }
    let st = follower.replication().status();
    assert_eq!(st.role, Role::Follower);
    assert_eq!(st.lag_rows, Some(0), "{st:?}");
    assert!(st.healthy, "{st:?}");
    assert_eq!(
        st.epoch.as_deref(),
        Some(leader_state.store.changes().epoch())
    );

    // ── 2. A write on the leader over HTTP lands during the long-poll ─────
    // The follower asks first and is held; the write comes a second later;
    // the follower returns with the row long before its hold would expire.
    let writer_app = leader_app.clone();
    let writer_token = token.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let (st, _, txt) = call(
                &writer_app,
                Method::POST,
                "/sparql",
                Some(&writer_token),
                Some((
                    "application/sparql-update",
                    format!(
                        "INSERT DATA {{ GRAPH <{}> {{ <https://example.org/e2e/live> <https://example.org/e2e/p> \"hello\" }} }}",
                        graph_iri(0)
                    ),
                )),
            )
            .await;
            assert!(st.is_success(), "leader update: {st} {txt}");
        });
    });
    let t = Instant::now();
    let p = follower.replicate_once(&http).expect("tail over HTTP");
    let tail_took = t.elapsed();
    writer.join().unwrap();
    assert_eq!(p.applied_rows, 1, "the row landed during the hold: {p:?}");
    assert!(
        tail_took < Duration::from_secs(15),
        "a hot follower returns when the row lands, not when its hold expires: {tail_took:?}"
    );
    assert!(
        graph(follower, &graph_iri(0))
            .iter()
            .any(|l| l.contains("\"hello\"")),
        "the write is on the follower"
    );

    // ── 3. What each side says ───────────────────────────────────────────
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // The follower, publicly: role, lag, health.
        let (st, s, _) = call(
            &follower_app,
            Method::GET,
            "/api/replication/status",
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(s["role"], "follower");
        assert_eq!(s["mode"], "hot");
        assert_eq!(s["lag_rows"], 0);
        assert_eq!(s["healthy"], true);
        assert_eq!(s["read_only"], true);
        assert_eq!(s["leader_url"], base);
        assert_eq!(s["resyncs"], 1);

        // The follower refuses a write and says where writes go.
        let (st, _, txt) = call(
            &follower_app,
            Method::POST,
            "/sparql",
            Some(&token),
            Some((
                "application/sparql-update",
                "INSERT DATA { <urn:e2e:s> <urn:e2e:p> \"no\" }".to_string(),
            )),
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{txt}");
        assert!(
            txt.contains("read-only replica") && txt.contains(&base),
            "names the leader: {txt}"
        );

        // The leader holds the follower's cursor at the newest row.
        let (st, c, _) = call(
            &leader_app,
            Method::GET,
            "/api/admin/changes/status",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(c["enabled"], true);
        let cursors = c["cursors"].as_array().unwrap();
        let mine = cursors
            .iter()
            .find(|c| c["name"] == "e2e-follower")
            .expect("the follower bookmarked itself on the leader");
        assert_eq!(mine["seq"], c["newest_seq"], "at the newest row: {c}");
    });

    // The bootstrap figure includes the first page request's long-poll hold
    // (nothing landed after the bootstrap, so it ran the full 25 s).
    eprintln!(
        "bootstrap of {GRAPHS} graphs through the limiter, plus one held long-poll: {bootstrap_took:?} ({} × 429); live row: {tail_took:?}",
        throttled.load(Ordering::SeqCst)
    );
}

/// The identity database comes across the same way, over HTTP: a SQLite
/// snapshot fetched when the leader's version moved, applied in place. This
/// is what makes the token the follower authenticates with — minted on the
/// leader — valid on the follower too, which the compose walkthrough relies on.
#[test]
fn the_identity_database_comes_across_over_http() {
    use open_triplestore::auth::db::AuthDb;
    use open_triplestore::auth::models::SystemRole;
    use open_triplestore::store::replication::replicate_identity_once;

    // The leader's identity database is a file, as in a deployment: only a
    // file has a `data_version` for the manifest to carry, so only a file
    // lets the follower skip a snapshot that has not changed.
    let store = TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
        .with_replication(ReplicationConfig::leader());
    let dir = tempfile::tempdir().unwrap();
    let mut leader_state = test_state_with_store(store);
    leader_state.auth_db = Arc::new(AuthDb::open(&dir.path().join("auth.db")).unwrap());
    leader_state
        .auth_db
        .create_user(
            "adm",
            "admin",
            "admin@test.com",
            "hash",
            SystemRole::SuperAdmin,
        )
        .unwrap();
    let api_token = mint_api_token(&leader_state);
    leader_state
        .auth_db
        .create_user("u1", "one", "one@test.com", "hash", SystemRole::User)
        .unwrap();
    let base = serve(test_app(leader_state.clone()));
    let http = HttpLeader::new(&base, Some(&api_token));

    let follower_auth = AuthDb::in_memory().unwrap();
    assert!(follower_auth.get_user_by_id("u1").unwrap().is_none());
    assert!(
        replicate_identity_once(&follower_auth, &http).expect("snapshot over HTTP"),
        "the first round applies the leader's database"
    );
    assert_eq!(
        follower_auth
            .get_user_by_id("u1")
            .unwrap()
            .map(|u| u.username),
        Some("one".to_string())
    );
    // The admin who minted the follower's token is on the follower now too:
    // the token is valid on both nodes.
    assert!(follower_auth.get_user_by_id("adm").unwrap().is_some());

    // Nothing changed on the leader: the manifest says so and nothing is
    // fetched — the follower's own token use, which stamps `last_used_at` at
    // most once a minute, did not move the version between the two rounds.
    assert!(!replicate_identity_once(&follower_auth, &http).unwrap());

    // A change on the leader moves its version; the next round applies it.
    leader_state
        .auth_db
        .create_user("u2", "two", "two@test.com", "hash", SystemRole::User)
        .unwrap();
    assert!(replicate_identity_once(&follower_auth, &http).unwrap());
    assert!(follower_auth.get_user_by_id("u2").unwrap().is_some());

    // Without a token the snapshot is not served — an anonymous client is
    // told so rather than handed the identity database.
    let anon = HttpLeader::new(&base, None);
    let err = replicate_identity_once(&follower_auth, &anon).expect_err("401");
    assert!(err.contains("401"), "{err}");
}
