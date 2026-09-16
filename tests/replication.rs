//! Replication on the change log (P4): a follower tails a leader's
//! `/api/admin/changes` with a cursor, applies `full` rows as deltas, fetches
//! the graph for `counts` / `unknown` rows, resynchronises every graph in
//! scope on a store-scoped row or an epoch change, refuses writes, and says
//! where it stands at `/api/replication/status`. Temperatures (cold / warm /
//! hot) only change how often the follower asks; the scope (all graphs, some
//! graphs, some datasets) changes what it applies. These tests drive the
//! follower in-process against an in-process leader — no network.

mod common;

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::store::changes::{DEFAULT_MAX_PAYLOAD, DEFAULT_MAX_SCAN};
use open_triplestore::store::engine::StoreError;
use open_triplestore::store::replication::{
    DatasetGraphs, InProcessLeader, Mode, ReplicationConfig, Role, Scope,
};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::model::{GraphNameRef, NamedNodeRef};
use serde_json::Value;
use tower::ServiceExt as _;

const G1: &str = "https://example.org/rep/g1";
const G2: &str = "https://example.org/rep/g2";

fn leader() -> TripleStore {
    TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
        .with_replication(ReplicationConfig::leader())
}

fn follower(scope: Scope) -> TripleStore {
    TripleStore::in_memory()
        .unwrap()
        .with_replication(ReplicationConfig::follower("inprocess", Mode::Hot, scope))
}

fn ttl(ns: &[u32]) -> String {
    ns.iter()
        .map(|n| format!("<https://example.org/rep/s{n}> <https://example.org/rep/p> <https://example.org/rep/o{n}> ."))
        .collect::<Vec<_>>()
        .join("\n")
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

#[test]
fn a_follower_bootstraps_from_the_manifest_and_then_tails_the_log() {
    let l = leader();
    l.graph_store_put(Some(G1), &ttl(&[1, 2]), RdfFormat::Turtle)
        .unwrap();
    l.graph_store_put(Some(G2), &ttl(&[3]), RdfFormat::Turtle)
        .unwrap();
    let src = InProcessLeader::new(l.clone());
    let f = follower(Scope::All);

    let p = f.replicate_once(&src).unwrap();
    assert!(p.bootstrapped, "{p:?}");
    assert_eq!(graph(&f, G1), graph(&l, G1));
    assert_eq!(graph(&f, G2), graph(&l, G2));
    assert_eq!(
        f.graph_count_cached(Some(G1)),
        Some(2),
        "the count index follows"
    );

    // Two ground updates on the leader: two full rows, applied as deltas.
    l.update(&format!(
        "INSERT DATA {{ GRAPH <{G1}> {{ <https://example.org/rep/s9> <https://example.org/rep/p> <https://example.org/rep/o9> }} }}"
    ))
    .unwrap();
    l.update(&format!(
        "DELETE DATA {{ GRAPH <{G1}> {{ <https://example.org/rep/s1> <https://example.org/rep/p> <https://example.org/rep/o1> }} }}"
    ))
    .unwrap();
    let p = f.replicate_once(&src).unwrap();
    assert!(!p.bootstrapped);
    assert_eq!(p.applied_rows, 2, "{p:?}");
    assert_eq!(p.refetched_graphs, 0, "deltas need no fetch: {p:?}");
    assert_eq!(graph(&f, G1), graph(&l, G1));
    assert_eq!(f.graph_count_cached(Some(G1)), Some(2));
    assert!(p.caught_up);

    // The follower's position, on both sides: its bookmark and the cursor it
    // set on the leader (which pins the leader's retention).
    let st = f.replication().status();
    assert_eq!(st.role, Role::Follower);
    assert_eq!(st.applied_seq, l.changes().last_seq());
    assert_eq!(st.epoch.as_deref(), Some(l.changes().epoch()));
    let cursor = l
        .changes()
        .cursor(&st.node_id)
        .expect("the follower bookmarked itself on the leader");
    assert_eq!(cursor.seq, l.changes().last_seq());
    assert_eq!(st.lag_rows, Some(0));
    assert!(st.healthy, "{st:?}");
}

#[test]
fn counts_and_unknown_rows_make_the_follower_fetch_the_graph() {
    // Caps of one quad: every scanned update is an unknown row.
    let l = TripleStore::in_memory().unwrap().with_change_capture(1, 1);
    l.graph_store_put(Some(G1), &ttl(&[1, 2, 3]), RdfFormat::Turtle)
        .unwrap();
    let src = InProcessLeader::new(l.clone());
    let f = follower(Scope::All);
    f.replicate_once(&src).unwrap();
    assert_eq!(graph(&f, G1), graph(&l, G1));

    l.update(&format!(
        "INSERT {{ GRAPH <{G1}> {{ ?s <https://example.org/rep/q> ?o }} }} WHERE {{ GRAPH <{G1}> {{ ?s <https://example.org/rep/p> ?o }} }}"
    ))
    .unwrap();
    let p = f.replicate_once(&src).unwrap();
    assert!(p.refetched_graphs >= 1, "{p:?}");
    assert_eq!(graph(&f, G1), graph(&l, G1));
    assert_eq!(graph(&f, G1).len(), 6);
}

#[test]
fn a_store_scoped_row_resynchronises_every_graph_in_scope() {
    let l = leader();
    l.graph_store_put(Some(G1), &ttl(&[1]), RdfFormat::Turtle)
        .unwrap();
    l.graph_store_put(Some(G2), &ttl(&[2]), RdfFormat::Turtle)
        .unwrap();
    let src = InProcessLeader::new(l.clone());
    let f = follower(Scope::All);
    f.replicate_once(&src).unwrap();

    // `GRAPH ?g` cannot be bounded: one store-scoped unknown row. Between the
    // two rows the leader also drops a graph.
    l.update("INSERT { GRAPH ?g { ?s <https://example.org/rep/x> \"x\" } } WHERE { GRAPH ?g { ?s <https://example.org/rep/p> ?o } }")
        .unwrap();
    l.graph_store_delete(Some(G2)).unwrap();
    let p = f.replicate_once(&src).unwrap();
    assert!(p.resyncs >= 1, "{p:?}");
    assert_eq!(graph(&f, G1), graph(&l, G1));
    assert_eq!(graph(&f, G1).len(), 2);
    assert!(
        graph(&f, G2).is_empty(),
        "the dropped graph is gone here too"
    );
}

#[test]
fn scope_some_replicates_only_the_named_graphs() {
    let l = leader();
    l.graph_store_put(Some(G1), &ttl(&[1]), RdfFormat::Turtle)
        .unwrap();
    l.graph_store_put(Some(G2), &ttl(&[2]), RdfFormat::Turtle)
        .unwrap();
    let src = InProcessLeader::new(l.clone());
    let f = follower(Scope::Graphs(vec![Some(G1.to_string())]));
    f.replicate_once(&src).unwrap();
    assert_eq!(graph(&f, G1), graph(&l, G1));
    assert!(graph(&f, G2).is_empty(), "out of scope");

    l.update(&format!(
        "INSERT DATA {{ GRAPH <{G2}> {{ <https://example.org/rep/s7> <https://example.org/rep/p> <https://example.org/rep/o7> }} }}"
    ))
    .unwrap();
    let p = f.replicate_once(&src).unwrap();
    assert_eq!(
        p.applied_rows, 0,
        "a row outside the scope is skipped: {p:?}"
    );
    assert!(graph(&f, G2).is_empty());
    // The cursor still advances past skipped rows.
    assert_eq!(f.replication().status().applied_seq, l.changes().last_seq());
}

#[test]
fn scope_datasets_resolves_to_the_leaders_dataset_graphs() {
    let l = leader();
    l.graph_store_put(Some(G1), &ttl(&[1]), RdfFormat::Turtle)
        .unwrap();
    l.graph_store_put(Some(G2), &ttl(&[2]), RdfFormat::Turtle)
        .unwrap();
    let src = InProcessLeader::new(l.clone()).with_datasets(vec![
        DatasetGraphs {
            id: "ds-a".into(),
            graphs: vec![G1.to_string()],
        },
        DatasetGraphs {
            id: "ds-b".into(),
            graphs: vec![G2.to_string()],
        },
    ]);
    let f = follower(Scope::Datasets(vec!["ds-b".to_string()]));
    f.replicate_once(&src).unwrap();
    assert!(graph(&f, G1).is_empty());
    assert_eq!(graph(&f, G2), graph(&l, G2));
}

#[test]
fn an_epoch_change_forces_a_full_resynchronisation() {
    let l1 = leader();
    l1.graph_store_put(Some(G1), &ttl(&[1, 2]), RdfFormat::Turtle)
        .unwrap();
    let f = follower(Scope::All);
    f.replicate_once(&InProcessLeader::new(l1.clone())).unwrap();
    assert_eq!(graph(&f, G1).len(), 2);

    // A new leader (a promoted follower, a restore): another epoch, other data.
    let l2 = leader();
    l2.graph_store_put(Some(G1), &ttl(&[5]), RdfFormat::Turtle)
        .unwrap();
    l2.graph_store_put(Some(G2), &ttl(&[6]), RdfFormat::Turtle)
        .unwrap();
    let p = f.replicate_once(&InProcessLeader::new(l2.clone())).unwrap();
    assert_eq!(p.resyncs, 1, "{p:?}");
    assert_eq!(graph(&f, G1), graph(&l2, G1));
    assert_eq!(graph(&f, G2), graph(&l2, G2));
    let st = f.replication().status();
    assert_eq!(st.epoch.as_deref(), Some(l2.changes().epoch()));
    assert_eq!(st.applied_seq, l2.changes().last_seq());
}

#[test]
fn a_follower_is_read_only_except_for_what_it_replicates() {
    let f = follower(Scope::All);
    let err = f
        .update("INSERT DATA { <https://example.org/rep/s> <https://example.org/rep/p> <https://example.org/rep/o> }")
        .unwrap_err();
    assert!(matches!(err, StoreError::ReadOnly(_)), "{err}");
    assert!(f
        .graph_store_put(Some(G1), &ttl(&[1]), RdfFormat::Turtle)
        .is_err());
    // A leader is not read-only, nor is an unconfigured store.
    assert!(!leader().replication().read_only());
    assert!(!TripleStore::in_memory().unwrap().replication().read_only());
}

#[test]
fn temperatures_only_change_how_often_the_follower_asks() {
    let cold = ReplicationConfig::follower("http://leader", Mode::Cold, Scope::All);
    let warm = ReplicationConfig::follower("http://leader", Mode::Warm, Scope::All);
    let hot = ReplicationConfig::follower("http://leader", Mode::Hot, Scope::All);
    assert_eq!(cold.interval.as_secs(), 3600);
    assert_eq!(warm.interval.as_secs(), 60);
    assert_eq!(hot.interval, hot.poll);
    assert!(hot.poll.as_millis() <= 1000);
    // The environment's spellings, including "medium" for warm.
    let parsed = ReplicationConfig::parse(
        "follower",
        "medium",
        Some("https://example.org/rep/g1, default"),
        None,
        Some("http://leader:7878/"),
        Some("tok"),
        Some("replica-1"),
        Some("250"),
        None,
    );
    assert_eq!(parsed.role, Role::Follower);
    assert_eq!(parsed.mode, Mode::Warm);
    assert_eq!(
        parsed.scope,
        Scope::Graphs(vec![Some("https://example.org/rep/g1".to_string()), None])
    );
    assert_eq!(parsed.leader_url.as_deref(), Some("http://leader:7878"));
    assert_eq!(parsed.poll.as_millis(), 250);
    assert_eq!(parsed.node_id, "replica-1");
    let ds = ReplicationConfig::parse(
        "follower",
        "cold",
        None,
        Some("ds-a,ds-b"),
        None,
        None,
        None,
        None,
        Some("120"),
    );
    assert_eq!(
        ds.scope,
        Scope::Datasets(vec!["ds-a".to_string(), "ds-b".to_string()])
    );
    assert_eq!(ds.interval.as_secs(), 120, "an explicit interval wins");
}

// ─── The HTTP surface ───────────────────────────────────────────────────────

async fn call(
    app: &axum::Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<(&str, String)>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
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
    (st, serde_json::from_str(&txt).unwrap_or(Value::String(txt)))
}

#[tokio::test]
async fn the_leader_publishes_a_manifest_and_the_follower_its_status() {
    // The leader: a dataset with one graph, capture on.
    let (state, token) = admin_state_with_store(leader());
    state
        .store
        .graph_store_put(Some(G1), &ttl(&[1]), RdfFormat::Turtle)
        .unwrap();
    let ds = state
        .auth_db
        .create_dataset(
            "rep-ds",
            "Rep",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph(&ds.id, G1).unwrap();
    let app = test_app(state.clone());

    let (st, _) = call(&app, Method::GET, "/api/replication/manifest", None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, m) = call(
        &app,
        Method::GET,
        "/api/replication/manifest",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{m}");
    assert_eq!(m["capture_enabled"], true);
    assert_eq!(m["epoch"], state.store.changes().epoch());
    assert_eq!(m["newest_seq"], state.store.changes().last_seq());
    assert!(
        m["graphs"].as_array().unwrap().iter().any(|g| g == G1),
        "{m}"
    );
    let dsm = m["datasets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == ds.id)
        .expect("the dataset is listed");
    assert_eq!(dsm["graphs"][0], G1);
    let (st, s) = call(&app, Method::GET, "/api/replication/status", None, None).await;
    assert_eq!(st, StatusCode::OK, "{s}");
    assert_eq!(s["role"], "leader");
    assert_eq!(s["read_only"], false);

    // The follower: status is public; writes answer 503.
    let f = follower(Scope::All);
    f.replicate_once(&InProcessLeader::new(state.store.clone()))
        .unwrap();
    let (fstate, ftoken) = admin_state_with_store(f);
    let fapp = test_app(fstate.clone());
    let (st, s) = call(&fapp, Method::GET, "/api/replication/status", None, None).await;
    assert_eq!(st, StatusCode::OK, "{s}");
    assert_eq!(s["role"], "follower");
    assert_eq!(s["mode"], "hot");
    assert_eq!(s["read_only"], true);
    assert_eq!(s["applied_seq"], state.store.changes().last_seq());
    assert_eq!(s["healthy"], true, "{s}");
    let (st, body) = call(
        &fapp,
        Method::POST,
        "/sparql",
        Some(&ftoken),
        Some((
            "application/sparql-update",
            format!("INSERT DATA {{ GRAPH <{G1}> {{ <https://example.org/rep/s9> <https://example.org/rep/p> <https://example.org/rep/o9> }} }}"),
        )),
    )
    .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    let (st, body) = call(
        &fapp,
        Method::PUT,
        &format!("/store?graph={}", url_encode(G1)),
        Some(&ftoken),
        Some(("text/turtle", ttl(&[1]))),
    )
    .await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    // Reads are what a replica is for.
    let (st, _) = call(
        &fapp,
        Method::GET,
        &format!(
            "/sparql?query={}",
            url_encode("SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }")
        ),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

// ─── Synchronous replication ────────────────────────────────────────────────

/// A leader with a synchronous follower waits for that follower's cursor to
/// cover its write. Nobody applies: the write returns after the timeout,
/// degraded, and says so. A follower that keeps up: the write returns as
/// soon as the cursor moves, the flag clears, the leader is synchronous
/// again.
#[test]
fn a_synchronous_leader_waits_for_its_follower_and_degrades_visibly_without_one() {
    let l = TripleStore::in_memory()
        .unwrap()
        .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
        .with_replication(ReplicationConfig::leader().with_sync(&["f1"], 1, 300));
    assert_eq!(
        l.replication().ack_state(),
        Some("sync"),
        "nothing degraded yet"
    );

    // No follower has ever acknowledged: the write waits the timeout out.
    let t = std::time::Instant::now();
    l.graph_store_put(Some(G1), &ttl(&[1]), RdfFormat::Turtle)
        .unwrap();
    let waited = t.elapsed();
    assert!(
        waited >= std::time::Duration::from_millis(300),
        "{waited:?}"
    );
    let s = l.replication().status().sync.expect("configured");
    assert!(s.degraded_since.is_some(), "{s:?}");
    assert_eq!(s.degraded_waits, 1);
    assert_eq!(l.replication().ack_state(), Some("degraded"));

    // While degraded and no follower has caught up, a write does not wait.
    let t = std::time::Instant::now();
    l.graph_store_put(Some(G2), &ttl(&[2]), RdfFormat::Turtle)
        .unwrap();
    assert!(
        t.elapsed() < std::time::Duration::from_millis(150),
        "{:?}",
        t.elapsed()
    );

    // A follower named f1 catches up in the background, every 20 ms.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let handle = {
        let l = l.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let f = TripleStore::in_memory().unwrap().with_replication(
                ReplicationConfig::follower("inprocess", Mode::Hot, Scope::All).with_node_id("f1"),
            );
            let src = InProcessLeader::new(l);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = f.replicate_once(&src);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            f
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(150));
    // The follower is current: this write is acknowledged well inside the
    // timeout, and the leader is synchronous again.
    let t = std::time::Instant::now();
    l.update(&format!(
        "INSERT DATA {{ GRAPH <{G1}> {{ <https://example.org/rep/s9> <https://example.org/rep/p> <https://example.org/rep/o9> }} }}"
    ))
    .unwrap();
    let waited = t.elapsed();
    assert!(waited < std::time::Duration::from_millis(300), "{waited:?}");
    let s = l.replication().status().sync.expect("configured");
    assert!(s.degraded_since.is_none(), "{s:?}");
    assert_eq!(s.acked, vec!["f1".to_string()]);
    assert_eq!(s.last_confirmed_seq, l.changes().last_seq());
    assert_eq!(l.replication().ack_state(), Some("sync"));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let f = handle.join().unwrap();
    assert_eq!(graph(&f, G1), graph(&l, G1));
}

#[test]
fn the_synchronous_settings_have_defaults_and_bounds() {
    let (f, r, t) = ReplicationConfig::parse_sync(Some("a, b ,c"), Some("all"), Some("10"));
    assert_eq!(f, vec!["a", "b", "c"]);
    assert_eq!(r, 3, "`all` means every named follower");
    assert_eq!(t.as_millis(), 50, "the timeout floor");
    let (_, r, t) = ReplicationConfig::parse_sync(Some("a"), Some("5"), None);
    assert_eq!(r, 1, "never more than there are");
    assert_eq!(t.as_millis(), 2000, "the default timeout");
    let (f, r, _) = ReplicationConfig::parse_sync(None, None, None);
    assert!(f.is_empty());
    assert_eq!(r, 1);
    assert!(!ReplicationConfig::leader()
        .with_sync(&[], 1, 2000)
        .sync_followers
        .iter()
        .any(|_| true));
}

/// The long-poll: a request with `wait_ms` and no new row holds until a row
/// lands, then answers at once; with no write it answers empty after the
/// wait. A write's response carries the acknowledgement header.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_change_endpoint_long_polls_and_writes_carry_the_ack_header() {
    let (state, token) = admin_state_with_store(
        TripleStore::in_memory()
            .unwrap()
            .with_change_capture(DEFAULT_MAX_SCAN, DEFAULT_MAX_PAYLOAD)
            .with_replication(ReplicationConfig::leader().with_sync(&["f1"], 1, 100)),
    );
    let app = test_app(state.clone());

    // Nothing lands: the wait runs out, the page is empty.
    let t = std::time::Instant::now();
    let (st, v) = call(
        &app,
        Method::GET,
        "/api/admin/changes?after=0&wait_ms=200",
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert!(v["rows"].as_array().unwrap().is_empty(), "{v}");
    assert!(
        t.elapsed() >= std::time::Duration::from_millis(200),
        "{:?}",
        t.elapsed()
    );

    // A write lands 100 ms into a 5 s wait: the request answers with it.
    let writer = {
        let store = state.store.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            tokio::task::spawn_blocking(move || {
                store
                    .graph_store_put(Some(G1), &ttl(&[1]), RdfFormat::Turtle)
                    .unwrap()
            })
            .await
            .unwrap();
        })
    };
    let t = std::time::Instant::now();
    let (st, v) = call(
        &app,
        Method::GET,
        "/api/admin/changes?after=0&wait_ms=5000",
        Some(&token),
        None,
    )
    .await;
    writer.await.unwrap();
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["rows"].as_array().unwrap().len(), 1, "{v}");
    assert!(
        t.elapsed() < std::time::Duration::from_millis(2000),
        "{:?}",
        t.elapsed()
    );

    // The write route says what the synchronous followers said: nobody
    // acknowledged, so this leader is degraded.
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::PUT)
                .uri(format!("/store?graph={}", url_encode(G2)))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "text/turtle")
                .body(Body::from(ttl(&[2])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        resp.headers()
            .get("x-replication-ack")
            .and_then(|v| v.to_str().ok()),
        Some("degraded")
    );
    let (_, s) = call(&app, Method::GET, "/api/replication/status", None, None).await;
    assert_eq!(s["sync"]["required"], 1, "{s}");
    assert!(s["sync"]["degraded_since"].is_string(), "{s}");
}
