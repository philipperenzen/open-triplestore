//! Building a delta version's graphs on demand (delta note §3.3).
//!
//! A delta's snapshot graphs — the same IRIs a checkpoint has — hold nothing
//! until a reader needs them. [`ensure_built`] fronts every read: a
//! checkpoint is returned as it is; a delta is built once, by whichever
//! direction is cheaper:
//!
//! * **forward** — copy the base checkpoint's graphs, apply the net of the
//!   delta's range of the change log;
//! * **backward** — copy the live graphs and apply the inverse of everything
//!   since the delta (only in the current epoch, with an intact chain to now;
//!   a write that races the copy discards it).
//!
//! A build writes graphs nobody reads until its materialisation is recorded,
//! so it may commit in several transactions; it runs under one write
//! bracket, so the store's caches are invalidated once. Reads touch the
//! build; a build unused for `OTS_MATERIALISATION_TTL_SECS` (3600) is
//! dropped by [`sweep`], all expired ones in one `bulk_delete_graphs`.
//!
//! **A cold read is a store write**, and costs like one: it invalidates the
//! query cache and the accelerator for the whole store. So an anonymous
//! caller never triggers one (503 with `Retry-After`; the versions anonymous
//! readers are entitled to — Published ones — are always physical), a build
//! takes a permit of the expensive-operation pool and answers 503 when it
//! outlasts the write timeout (it continues in the background), and one
//! larger than `OTS_MATERIALISATION_MAX_QUADS` (2 000 000) is refused with
//! 507.

use std::collections::BTreeMap;

use oxigraph::model::{GraphName, NamedNode, Quad};

use super::models::DatasetVersion;
use super::{cut, lock, registry, snapshot};
use crate::server::error::AppError;
use crate::server::AppState;
use crate::store::changes::{self, Materialisation, NetDelta};
use crate::store::set_hash::SetHash;
use crate::store::{QuadOp, TripleStore};

pub const DEFAULT_MAX_QUADS: i64 = 2_000_000;
pub const DEFAULT_TTL_SECS: u64 = 3600;

fn max_quads() -> i64 {
    std::env::var("OTS_MATERIALISATION_MAX_QUADS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|n: &i64| *n > 0)
        .unwrap_or(DEFAULT_MAX_QUADS)
}

fn ttl_secs() -> u64 {
    std::env::var("OTS_MATERIALISATION_TTL_SECS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_TTL_SECS)
}

/// Every live build.
pub fn list(store: &TripleStore) -> Vec<Materialisation> {
    store.changes().materialisations(None)
}

/// Why a build did not happen.
#[derive(Debug)]
pub enum BuildError {
    /// It would hold more quads than `OTS_MATERIALISATION_MAX_QUADS`.
    Budget {
        needed: i64,
        max: i64,
    },
    /// The data cannot be rebuilt: the chain is broken, or the base is gone.
    Broken(String),
    Store(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Budget { needed, max } => write!(
                f,
                "building it would take {needed} quads, over the {max} OTS_MATERIALISATION_MAX_QUADS allows"
            ),
            BuildError::Broken(r) => write!(f, "its data cannot be rebuilt: {r}"),
            BuildError::Store(e) => write!(f, "{e}"),
        }
    }
}

fn store_err(e: impl std::fmt::Display) -> BuildError {
    BuildError::Store(e.to_string())
}

/// `quad`, moved into `graph`.
fn retarget(quad: &Quad, graph: &GraphName) -> Quad {
    Quad::new(
        quad.subject.clone(),
        quad.predicate.clone(),
        quad.object.clone(),
        graph.clone(),
    )
}

fn target_of(iri: &str) -> Result<GraphName, BuildError> {
    NamedNode::new(iri)
        .map(GraphName::NamedNode)
        .map_err(store_err)
}

/// Apply one graph's net delta into its build target.
fn apply_into(store: &TripleStore, target: &str, net: &NetDelta) -> Result<(), BuildError> {
    if net.is_empty() {
        return Ok(());
    }
    let g = target_of(target)?;
    let added: Vec<Quad> = net.added.iter().map(|q| retarget(q, &g)).collect();
    let removed: Vec<Quad> = net.removed.iter().map(|q| retarget(q, &g)).collect();
    // Chunked: the target is unobservable until the build is recorded.
    store
        .apply_quad_ops_chunked(&QuadOp::from_delta(added, removed))
        .map(|_| ())
        .map_err(store_err)
}

/// Build `record`'s graphs (blocking). The caller holds the version's build
/// lock. Returns the quads built.
pub fn build(
    store: &TripleStore,
    base_url: &str,
    record: &DatasetVersion,
) -> Result<i64, BuildError> {
    let Some(range) = record.patch_range.clone() else {
        return Err(BuildError::Broken(
            "it records no range of the change log".into(),
        ));
    };
    let Some(base_label) = record.base_version.as_deref() else {
        return Err(BuildError::Broken("it names no base".into()));
    };
    let base = registry::get_version(store, base_url, &record.dataset_id, base_label)
        .ok_or_else(|| BuildError::Broken(format!("its base {base_label} is gone")))?;
    if base.is_delta() {
        return Err(BuildError::Broken(format!(
            "its base {base_label} is not physical"
        )));
    }
    let max = max_quads();
    let needed = record.quad_count.unwrap_or(0);
    if needed > max {
        return Err(BuildError::Budget { needed, max });
    }
    let log = store.changes();
    let covered = record.covered_graphs();
    let forward = log.chain(&covered, &range.epoch, range.from_seq, range.to_seq);
    let current = log.epoch() == range.epoch;
    let head = log.head_seq();
    let backward = current.then(|| log.chain(&covered, &range.epoch, range.to_seq, head));
    let base_cost: i64 = base
        .source_map
        .iter()
        .filter(|m| covered.contains(&m.source_graph))
        .filter_map(|m| m.quad_count)
        .sum::<i64>()
        + forward.change_count();
    let live_cost: Option<i64> = backward.as_ref().filter(|c| c.intact).map(|c| {
        covered
            .iter()
            .map(|g| store.graph_count_cached(Some(g)).unwrap_or(0) as i64)
            .sum::<i64>()
            + c.change_count()
    });
    let _w = store.begin_write().map_err(store_err)?;
    // The cheaper end first; either end that holds will do, so a base whose
    // snapshot was tampered with does not cost a version the live graphs and
    // their chain can still rebuild, nor the reverse.
    let built = match (forward.intact, live_cost) {
        (true, Some(live)) if live < base_cost => {
            build_backward(store, record, backward.as_ref(), head)
                .or_else(|_| build_forward(store, record, &base, &forward))
        }
        (true, Some(_)) => build_forward(store, record, &base, &forward)
            .or_else(|_| build_backward(store, record, backward.as_ref(), head)),
        (true, None) => build_forward(store, record, &base, &forward),
        (false, Some(_)) => build_backward(store, record, backward.as_ref(), head),
        (false, None) => Err(BuildError::Broken(
            forward
                .reason
                .clone()
                .unwrap_or_else(|| "the chain is broken".into()),
        )),
    }?;
    drop(_w);
    let now = chrono::Utc::now().to_rfc3339();
    log.put_materialisation(&Materialisation {
        version_iri: record.graph_iri.clone(),
        dataset_id: record.dataset_id.clone(),
        graphs: record.snapshot_graphs.clone(),
        quads: built,
        built_at: now.clone(),
        last_used: now,
    })
    .map_err(store_err)?;
    Ok(built)
}

fn build_forward(
    store: &TripleStore,
    record: &DatasetVersion,
    base: &DatasetVersion,
    chain: &changes::Chain,
) -> Result<i64, BuildError> {
    let fold: BTreeMap<Option<String>, NetDelta> = changes::fold_rows(&chain.rows);
    let targets: Vec<&str> = record
        .source_map
        .iter()
        .map(|m| m.snapshot_graph.as_str())
        .collect();
    store.bulk_delete_graphs(&targets).map_err(store_err)?;
    let mut total = 0i64;
    for m in &record.source_map {
        let base_m = base
            .source_map
            .iter()
            .find(|b| b.source_graph == m.source_graph)
            .ok_or_else(|| {
                BuildError::Broken(format!("the base does not cover <{}>", m.source_graph))
            })?;
        let (_, copied) = snapshot::copy_graph_hashed(
            store,
            &base_m.snapshot_graph,
            &m.snapshot_graph,
            &m.source_graph,
        )
        .map_err(store_err)?;
        // The base's snapshot is immutable by convention only: a copy that
        // no longer hashes to what the base recorded is not the base.
        if let Some(expected) = base_m.content_hash.as_deref() {
            if copied.to_hex() != expected {
                return Err(BuildError::Broken(format!(
                    "base {}'s copy of <{}> no longer matches its content hash",
                    base.version, m.source_graph
                )));
            }
        }
        if let Some(net) = fold.get(&Some(m.source_graph.clone())) {
            apply_into(store, &m.snapshot_graph, net)?;
        }
        total += store
            .graph_count_cached(Some(&m.snapshot_graph))
            .unwrap_or(0) as i64;
    }
    Ok(total)
}

fn build_backward(
    store: &TripleStore,
    record: &DatasetVersion,
    chain: Option<&changes::Chain>,
    head: i64,
) -> Result<i64, BuildError> {
    let chain = chain.ok_or_else(|| BuildError::Broken("not in the current epoch".into()))?;
    let undo = changes::fold_rows(&chain.rows);
    let targets: Vec<&str> = record
        .source_map
        .iter()
        .map(|m| m.snapshot_graph.as_str())
        .collect();
    store.bulk_delete_graphs(&targets).map_err(store_err)?;
    let covered = record.covered_graphs();
    let mut total = 0i64;
    for m in &record.source_map {
        let (_, live) =
            snapshot::copy_graph_hashed(store, &m.source_graph, &m.snapshot_graph, &m.source_graph)
                .map_err(store_err)?;
        let net = undo.get(&Some(m.source_graph.clone()));
        // The live graph must be the version plus what the chain says came
        // after it; anything else is a write the chain does not describe.
        if let Some(expected) = m.content_hash.as_deref().and_then(SetHash::from_hex) {
            let mut expected = expected;
            if let Some(net) = net {
                for q in &net.added {
                    expected.add_quad(q, Some(&m.source_graph));
                }
                for q in &net.removed {
                    expected.remove_quad(q, Some(&m.source_graph));
                }
            }
            if expected != live {
                let _ = store.bulk_delete_graphs(&targets);
                return Err(BuildError::Broken(format!(
                    "the live <{}> is not this version plus its chain",
                    m.source_graph
                )));
            }
        }
        if let Some(net) = net {
            apply_into(store, &m.snapshot_graph, &net.clone().inverse())?;
        }
        total += store
            .graph_count_cached(Some(&m.snapshot_graph))
            .unwrap_or(0) as i64;
    }
    // The live copy must not have raced a write.
    let now = store.changes().head_seq();
    if store.changes().touched(&covered, head, now) {
        let _ = store.bulk_delete_graphs(&targets);
        return Err(BuildError::Broken(
            "a write raced the copy of the live graphs".into(),
        ));
    }
    Ok(total)
}

/// Make `record`'s graphs readable. Nothing to do for a checkpoint; a delta
/// already built is touched; otherwise it is built — never for an anonymous
/// caller (503, `Retry-After`).
pub async fn ensure_built(
    state: &AppState,
    record: &DatasetVersion,
    anonymous: bool,
) -> Result<(), AppError> {
    if !record.is_delta() {
        return Ok(());
    }
    if record.chain_broken {
        return Err(AppError::Gone(format!(
            "version {}'s data cannot be rebuilt: its chain is broken",
            record.version
        )));
    }
    let log = state.store.changes();
    let build_lock = lock::build(&record.graph_iri);
    {
        let _g = build_lock.lock().await;
        if log.materialisation(&record.graph_iri).is_some() {
            log.touch_materialisation(&record.graph_iri);
            return Ok(());
        }
    }
    if anonymous {
        return Err(AppError::RetryLater {
            retry_after_secs: 30,
            message: format!(
                "version {} is built on demand: sign in to build it, or retry once it has been",
                record.version
            ),
        });
    }
    let guard = build_lock.lock_owned().await;
    if log.materialisation(&record.graph_iri).is_some() {
        log.touch_materialisation(&record.graph_iri);
        return Ok(());
    }
    let permit = state
        .expensive_semaphore
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| AppError::ServiceUnavailable("Server overloaded".into()))?;
    let st = state.clone();
    let rec = record.clone();
    // The guard and the permit go with the build: a build that outlasts the
    // wait below keeps both until it is done, so no second build starts.
    let task = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _permit = permit;
        build(&st.store, &st.base_url, &rec)
    });
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(state.write_timeout_secs),
        task,
    )
    .await;
    match outcome {
        Err(_) => Err(AppError::ServiceUnavailable(format!(
            "building version {} is taking longer than the write timeout; it continues, retry shortly",
            record.version
        ))),
        Ok(Err(e)) => Err(AppError::Internal(e.to_string())),
        Ok(Ok(Ok(_))) => Ok(()),
        Ok(Ok(Err(BuildError::Budget { needed, max }))) => Err(AppError::InsufficientStorage(
            format!(
                "version {} would take {needed} quads to build, over the {max} the materialisation budget allows",
                record.version
            ),
        )),
        Ok(Ok(Err(BuildError::Broken(reason)))) => {
            let _ = registry::set_chain_broken(
                &state.store,
                &state.base_url,
                &record.dataset_id,
                &record.version,
            );
            Err(AppError::Gone(format!(
                "version {}'s data cannot be rebuilt: {reason}",
                record.version
            )))
        }
        Ok(Ok(Err(BuildError::Store(e)))) => Err(AppError::Internal(e)),
    }
}

/// As [`ensure_built`], on a blocking thread (a background job): no
/// admission control — the caller is already off the request path.
pub fn ensure_built_blocking(
    store: &TripleStore,
    base_url: &str,
    record: &DatasetVersion,
) -> Result<(), BuildError> {
    if !record.is_delta() {
        return Ok(());
    }
    if record.chain_broken {
        return Err(BuildError::Broken("its chain is broken".into()));
    }
    let build_lock = lock::build(&record.graph_iri);
    let _g = build_lock.blocking_lock();
    let log = store.changes();
    if log.materialisation(&record.graph_iri).is_some() {
        log.touch_materialisation(&record.graph_iri);
        return Ok(());
    }
    build(store, base_url, record).map(|_| ())
}

/// Drop every build unused for the time-to-live, in one store write, and
/// forget it. Skips a build whose lock is held (it is being built or read).
/// Returns the builds dropped.
pub fn sweep(store: &TripleStore) -> usize {
    let log = store.changes();
    if !log.readable() {
        return 0;
    }
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(ttl_secs() as i64)).to_rfc3339();
    let expired = log.materialisations(Some(&cutoff));
    if expired.is_empty() {
        return 0;
    }
    let mut locks = Vec::new();
    let mut victims: Vec<Materialisation> = Vec::new();
    for m in expired {
        let l = lock::build(&m.version_iri);
        if let Ok(g) = l.clone().try_lock_owned() {
            // Re-read under the lock: a reader may have touched it since.
            if log
                .materialisation(&m.version_iri)
                .is_some_and(|fresh| fresh.last_used < cutoff)
            {
                locks.push(g);
                victims.push(m);
            }
        }
    }
    if victims.is_empty() {
        return 0;
    }
    let graphs: Vec<&str> = victims
        .iter()
        .flat_map(|m| m.graphs.iter().map(String::as_str))
        .collect();
    if let Err(e) = store.bulk_delete_graphs(&graphs) {
        tracing::warn!("materialisation sweep failed: {e}");
        return 0;
    }
    for m in &victims {
        log.delete_materialisation(&m.version_iri);
    }
    drop(locks);
    victims.len()
}

/// Drop the pins of delta versions the registry no longer holds (a version
/// deleted by a path that did not unpin it, a registry replaced by a backup
/// restore): a stale pin would hold back retention forever.
pub fn reconcile_pins(store: &TripleStore) -> usize {
    let log = store.changes();
    let mut dropped = 0;
    for pin in log.pins() {
        let Some(iri) = pin.name.strip_prefix("version:") else {
            continue;
        };
        let ask = format!(
            "ASK {{ GRAPH <{}> {{ <{}> ?p ?o }} }}",
            registry::REGISTRY_GRAPH,
            crate::store::escape_sparql_iri(iri)
        );
        let exists = matches!(
            store.query(&ask),
            Ok(oxigraph::sparql::QueryResults::Boolean(true))
        );
        if !exists && log.delete_pin(&pin.name) {
            dropped += 1;
        }
    }
    dropped
}

/// Forget a version's build and its pin, and drop its built graphs (if any)
/// — a purge, or a promotion that keeps the graphs (`keep_graphs`).
pub fn release(store: &TripleStore, record: &DatasetVersion, keep_graphs: bool) {
    let log = store.changes();
    if !keep_graphs {
        if let Some(m) = log.materialisation(&record.graph_iri) {
            let graphs: Vec<&str> = m.graphs.iter().map(String::as_str).collect();
            let _ = store.bulk_delete_graphs(&graphs);
        }
    }
    log.delete_materialisation(&record.graph_iri);
    log.delete_pin(&cut::pin_name(&record.graph_iri));
}
