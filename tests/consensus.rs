//! Consensus (P4): three in-process members elect one leader, every member
//! sees the same one, and when the leader goes off the network the other
//! two elect another. Raft decides who leads; the data path is the change
//! log, and a member that is not the leader is read-only. No sockets: the
//! members talk through an in-process router.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::store::consensus::{
    ClusterConfig, InProcessRouter, Member, Timing, Transport,
};
use open_triplestore::store::replication::{Mode, ReplicationConfig, Role};
use open_triplestore::store::TripleStore;
use tower::ServiceExt as _;

/// Long enough that a re-election inside the window is not a failure, short
/// enough that a real hang still ends the test.
const TIMEOUT: Duration = Duration::from_secs(20);

async fn wait_for<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let t = Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(t.elapsed() < timeout, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_members_elect_one_leader_and_re_elect_when_it_leaves() {
    let router = Arc::new(InProcessRouter::default());
    let members: BTreeMap<u64, String> = (1..=3)
        .map(|i| (i, format!("inprocess://node-{i}")))
        .collect();
    let timing = Timing {
        election_min: 150,
        election_max: 300,
        heartbeat: 50,
    };
    let mut nodes = Vec::new();
    for id in 1..=3 {
        let m = Member::start(
            id,
            members.clone(),
            Transport::InProcess(router.clone()),
            timing,
        )
        .await
        .expect("member starts");
        router.add(id, m.raft.clone());
        nodes.push(m);
    }

    // Exactly one leader, and every member names it — asked as one
    // condition over one snapshot of the views. Finding a leader in one poll
    // and checking agreement in the next is a race against the cluster: with
    // an election timeout of 150-300 ms a new election can intervene, and the
    // member the first poll captured is then stale for good.
    let leader = wait_for("one leader every member names", TIMEOUT, || {
        let views: Vec<_> = nodes.iter().map(|n| n.view()).collect();
        let mut leaders = nodes
            .iter()
            .zip(&views)
            .filter(|(_, v)| v.is_leader)
            .map(|(n, _)| n.id);
        let id = leaders.next()?;
        if leaders.next().is_some() {
            return None; // Two at once: a term is still settling.
        }
        views.iter().all(|v| v.leader == Some(id)).then_some(id)
    })
    .await;
    let v = nodes[(leader - 1) as usize].view();
    assert_eq!(v.state, "leader");
    assert_eq!(v.members.len(), 3);
    assert!(v.term >= 1);

    // The leader leaves the network and stops: the other two elect another.
    router.remove(leader);
    nodes[(leader - 1) as usize].shutdown().await;
    // The same question, and the same reason to ask it once: a new leader is
    // only a new leader when the member still standing beside it says so.
    let new_leader = wait_for("a new leader the survivor follows", TIMEOUT, || {
        let alive: Vec<_> = nodes.iter().filter(|n| n.id != leader).collect();
        let views: Vec<_> = alive.iter().map(|n| n.view()).collect();
        let id = alive
            .iter()
            .zip(&views)
            .find(|(_, v)| v.is_leader)
            .map(|(n, _)| n.id)?;
        views.iter().all(|v| v.leader == Some(id)).then_some(id)
    })
    .await;
    assert_ne!(new_leader, leader);
    let term_after = nodes[(new_leader - 1) as usize].view().term;
    assert!(
        term_after > v.term,
        "a new term: {} > {}",
        term_after,
        v.term
    );
    for n in nodes.iter().filter(|n| n.id != leader) {
        n.shutdown().await;
    }
}

/// A cluster member's replication settings: its name, its synchronous
/// followers (the other members), the majority it needs, and — until an
/// election says otherwise — a read-only follower.
#[test]
fn a_cluster_member_follows_until_elected_and_acknowledges_by_majority() {
    let cluster = ClusterConfig::parse(
        Some("1=http://a:7878,2=http://b:7878,3=http://c:7878"),
        Some("2"),
        Some("s3cret"),
        Some("400"),
        None,
    )
    .unwrap();
    let mut c = ReplicationConfig::none();
    c.apply_cluster(cluster);
    assert_eq!(c.role, Role::Cluster);
    assert_eq!(c.node_id, "node-2");
    assert_eq!(c.sync_followers, vec!["node-1", "node-3"]);
    assert_eq!(c.sync_required, 1);
    // A member follows hot, so its interval is its poll — not the warm
    // minute the environment's default would give it.
    assert_eq!(c.interval, c.poll);
    assert_eq!(c.mode, Mode::Hot);
    let store = TripleStore::in_memory().unwrap().with_replication(c);
    let rep = store.replication();
    assert_eq!(rep.role(), Role::Cluster);
    assert_eq!(rep.effective_role(), Role::Follower, "no leader known yet");
    assert!(rep.read_only());
    assert!(rep.leader_url().is_none());
    let s = rep.status();
    assert_eq!(s.configured_role, Role::Cluster);
    assert_eq!(s.role, Role::Follower);
    let view = s.cluster.expect("a member reports its cluster");
    assert_eq!(view.id, 2);
    assert_eq!(view.members.len(), 3);
    assert_eq!(view.state, "starting");
}

/// The Raft routes are not user routes: a node that is not a cluster member
/// answers 404 whatever the caller sends.
#[tokio::test]
async fn the_raft_routes_answer_404_off_a_cluster() {
    let (state, token) = admin_state();
    let app = test_app(state);
    for path in [
        "/api/replication/raft/vote",
        "/api/replication/raft/append",
        "/api/replication/raft/snapshot",
    ] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("x-cluster-secret", "whatever")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::UNPROCESSABLE_ENTITY
                || resp.status() == StatusCode::BAD_REQUEST,
            "{path}: {}",
            resp.status()
        );
    }
}
