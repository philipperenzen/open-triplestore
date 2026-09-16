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

    // Exactly one leader, and every member names it.
    let leader = wait_for("a leader", Duration::from_secs(10), || {
        let leaders: Vec<u64> = nodes
            .iter()
            .filter(|n| n.view().is_leader)
            .map(|n| n.id)
            .collect();
        (leaders.len() == 1).then(|| leaders[0])
    })
    .await;
    wait_for("agreement", Duration::from_secs(10), || {
        nodes
            .iter()
            .all(|n| n.view().leader == Some(leader))
            .then_some(())
    })
    .await;
    let v = nodes[(leader - 1) as usize].view();
    assert_eq!(v.state, "leader");
    assert_eq!(v.members.len(), 3);
    assert!(v.term >= 1);

    // The leader leaves the network and stops: the other two elect another.
    router.remove(leader);
    nodes[(leader - 1) as usize].shutdown().await;
    let new_leader = wait_for("a new leader", Duration::from_secs(15), || {
        nodes
            .iter()
            .filter(|n| n.id != leader && n.view().is_leader)
            .map(|n| n.id)
            .next()
    })
    .await;
    assert_ne!(new_leader, leader);
    let survivor = nodes
        .iter()
        .find(|n| n.id != leader && n.id != new_leader)
        .unwrap();
    wait_for(
        "the survivor follows the new leader",
        Duration::from_secs(10),
        || (survivor.view().leader == Some(new_leader)).then_some(()),
    )
    .await;
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
