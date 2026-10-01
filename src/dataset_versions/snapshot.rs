//! Copy a dataset's live named graphs into version-scoped snapshot graphs, and
//! restore a snapshot back onto the live graphs.

use oxigraph::model::*;
use std::collections::HashMap;

use super::models::GraphMapping;
use crate::store::engine::StoreError;
use crate::store::TripleStore;

fn slugify_last_segment(iri: &str) -> String {
    let last = iri
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("graph");
    let slug: String = last
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "graph".to_string()
    } else {
        slug
    }
}

fn version_iri(base_url: &str, dataset_id: &str, version: &str) -> String {
    format!("{base_url}/dataset/{dataset_id}/version/{version}")
}

/// Suffixes a version's own graphs use beside its snapshots: a source graph
/// slugifying to one of these is disambiguated, or its snapshot would be the
/// version's validation graph (`shacl_studio::bindings::version_validation_graph`)
/// and the bindings snapshot would wipe it.
const RESERVED_SUFFIXES: &[&str] = &["validation"];

/// A fresh suffix tracker with the reserved suffixes already taken.
fn suffix_tracker() -> HashMap<String, usize> {
    RESERVED_SUFFIXES
        .iter()
        .map(|s| (s.to_string(), 1))
        .collect()
}

/// Ensure snapshot suffixes are unique even when two source graphs slugify to
/// the same value (disambiguate with an index suffix).
fn unique_suffix(used: &mut HashMap<String, usize>, base: &str) -> String {
    let n = used.entry(base.to_string()).or_insert(0);
    let out = if *n == 0 {
        base.to_string()
    } else {
        format!("{base}-{n}")
    };
    *n += 1;
    out
}

/// Snapshot the given live source graphs into version-scoped graphs.
/// Returns the snapshot→source mappings (snapshot graph IRIs included).
/// Copy every quad of `from` into `to`, streaming in batches, so a large graph
/// never has to be held in memory twice. Snapshot, clone and restore used to
/// collect ALL quads of ALL graphs into one `Vec` before inserting — an OOM
/// risk exactly on the datasets versioning is meant for. Returns the count.
fn copy_graph(store: &TripleStore, from: &str, to: &str) -> Result<usize, StoreError> {
    const BATCH: usize = 50_000;
    let target = GraphName::NamedNode(
        NamedNode::new(to).map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?,
    );
    let src =
        NamedNodeRef::new(from).map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?;
    let affected = vec![to.to_string()];
    let mut batch: Vec<Quad> = Vec::with_capacity(BATCH);
    let mut copied = 0usize;
    for q in store
        .store()
        .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(src)))
    {
        let q = q?;
        batch.push(Quad::new(q.subject, q.predicate, q.object, target.clone()));
        if batch.len() >= BATCH {
            copied += batch.len();
            store.bulk_insert_quads(std::mem::take(&mut batch), &affected)?;
            batch.reserve(BATCH);
        }
    }
    if !batch.is_empty() {
        copied += batch.len();
        store.bulk_insert_quads(batch, &affected)?;
    }
    Ok(copied)
}

/// The snapshot graph each source graph of a version maps to:
/// `{base}/dataset/{id}/version/{ver}/{slug}`. Deterministic — the same for a
/// checkpoint that copies into them now and a delta that builds them later
/// (delta note §3.1).
pub fn mapping(
    base_url: &str,
    dataset_id: &str,
    version: &str,
    source_graphs: &[String],
) -> Vec<GraphMapping> {
    let ver_iri = version_iri(base_url, dataset_id, version);
    let mut used = suffix_tracker();
    source_graphs
        .iter()
        .map(|src| {
            let suffix = unique_suffix(&mut used, &slugify_last_segment(src));
            GraphMapping::new(format!("{ver_iri}/{suffix}"), src.clone())
        })
        .collect()
}

pub fn snapshot_graphs(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
    source_graphs: &[String],
) -> Result<Vec<GraphMapping>, StoreError> {
    let mut mappings = mapping(base_url, dataset_id, version, source_graphs);
    copy_into(store, &mut mappings)?;
    Ok(mappings)
}

/// Copy each mapping's source graph into its snapshot graph (cleared first,
/// idempotent on retry), recording per graph the set hash of the content —
/// under the source graph — and its quad count.
pub fn copy_into(store: &TripleStore, mappings: &mut [GraphMapping]) -> Result<(), StoreError> {
    let snap_iris: Vec<String> = mappings.iter().map(|m| m.snapshot_graph.clone()).collect();
    let snap_refs: Vec<&str> = snap_iris.iter().map(|s| s.as_str()).collect();
    store.bulk_delete_graphs(&snap_refs)?;
    for m in mappings.iter_mut() {
        let (n, hash) =
            copy_graph_hashed(store, &m.source_graph, &m.snapshot_graph, &m.source_graph)?;
        m.quad_count = Some(n as i64);
        m.content_hash = Some(hash.to_hex());
    }
    Ok(())
}

/// As [`copy_graph`], hashing every copied quad under `hash_as` on the way.
pub fn copy_graph_hashed(
    store: &TripleStore,
    from: &str,
    to: &str,
    hash_as: &str,
) -> Result<(usize, crate::store::set_hash::SetHash), StoreError> {
    const BATCH: usize = 50_000;
    let target = GraphName::NamedNode(
        NamedNode::new(to).map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?,
    );
    let src =
        NamedNodeRef::new(from).map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?;
    let affected = vec![to.to_string()];
    let mut hash = crate::store::set_hash::SetHash::new();
    let mut batch: Vec<Quad> = Vec::with_capacity(BATCH);
    let mut copied = 0usize;
    for q in store
        .store()
        .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(src)))
    {
        let q = q?;
        hash.add_quad(&q, Some(hash_as));
        batch.push(Quad::new(q.subject, q.predicate, q.object, target.clone()));
        if batch.len() >= BATCH {
            copied += batch.len();
            store.bulk_insert_quads(std::mem::take(&mut batch), &affected)?;
            batch.reserve(BATCH);
        }
    }
    if !batch.is_empty() {
        copied += batch.len();
        store.bulk_insert_quads(batch, &affected)?;
    }
    Ok((copied, hash))
}

/// Clone an existing version's snapshot graphs into a new version (used for
/// branching). The new snapshots keep the original `source_graph` so a restore
/// targets the same live graphs.
pub fn clone_version(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    source_map: &[GraphMapping],
    target_version: &str,
) -> Result<Vec<GraphMapping>, StoreError> {
    let ver_iri = version_iri(base_url, dataset_id, target_version);
    let mut used = suffix_tracker();

    let mut new_map: Vec<GraphMapping> = Vec::new();
    for m in source_map {
        let suffix = unique_suffix(&mut used, &slugify_last_segment(&m.snapshot_graph));
        let snap = format!("{ver_iri}/{suffix}");
        // The same content as the version it clones: its hash and count too.
        let mut cloned = GraphMapping::new(snap, m.source_graph.clone());
        cloned.content_hash = m.content_hash.clone();
        cloned.quad_count = m.quad_count;
        new_map.push(cloned);
    }

    let new_iris: Vec<String> = new_map.iter().map(|m| m.snapshot_graph.clone()).collect();
    let new_refs: Vec<&str> = new_iris.iter().map(|s| s.as_str()).collect();
    store.bulk_delete_graphs(&new_refs)?;

    for (old, new) in source_map.iter().zip(new_map.iter()) {
        copy_graph(store, &old.snapshot_graph, &new.snapshot_graph)?;
    }
    Ok(new_map)
}

/// Restore a version's snapshots back onto the dataset's live source graphs.
///
/// Each snapshot is first copied (streaming) into a private staging graph and
/// then swapped into place with one `MOVE` — a single transaction — so readers
/// never see the live graph empty or half-written. It used to be cleared
/// BEFORE the snapshot was copied back, which left a window in which the
/// dataset's graph was simply gone.
pub fn restore(
    store: &TripleStore,
    source_map: &[GraphMapping],
) -> Result<Vec<String>, StoreError> {
    let mut restored = Vec::with_capacity(source_map.len());
    for m in source_map {
        let staging = format!("urn:ots:restore:{}", uuid::Uuid::new_v4());
        let copied = copy_graph(store, &m.snapshot_graph, &staging)?;
        if copied == 0 {
            // `MOVE SILENT` of a graph that does not exist is a no-op, and would
            // leave the live graph untouched: an empty snapshot means "empty".
            store.bulk_delete_graphs(&[m.source_graph.as_str()])?;
        } else {
            store.update(&format!(
                "MOVE SILENT GRAPH <{staging}> TO GRAPH <{}>",
                m.source_graph
            ))?;
        }
        restored.push(m.source_graph.clone());
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;

    fn put(store: &TripleStore, g: &str, s: &str, p: &str, o: &str) {
        let q = Quad::new(
            NamedNode::new(s).unwrap(),
            NamedNode::new(p).unwrap(),
            NamedNode::new(o).unwrap(),
            GraphName::NamedNode(NamedNode::new(g).unwrap()),
        );
        store.store_quad(q).unwrap();
    }

    fn count(store: &TripleStore, g: &str) -> usize {
        store.count_graph(Some(g)).unwrap()
    }

    #[test]
    fn snapshot_then_restore_round_trips() {
        let store = TripleStore::in_memory().unwrap();
        let base = "http://x";
        let live = "http://x/g/people";
        put(&store, live, "http://x/a", "http://x/p", "http://x/b");
        put(&store, live, "http://x/c", "http://x/p", "http://x/d");

        // Snapshot the live graph into version 1.0.0.
        let map = snapshot_graphs(&store, base, "ds1", "1.0.0", &[live.to_string()]).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map[0].source_graph, live);
        assert!(map[0]
            .snapshot_graph
            .starts_with("http://x/dataset/ds1/version/1.0.0/"));
        assert_eq!(count(&store, &map[0].snapshot_graph), 2);

        // Mutate the live graph, then restore the snapshot back over it.
        store.bulk_delete_graphs(&[live]).unwrap();
        assert_eq!(count(&store, live), 0);
        let restored = restore(&store, &map).unwrap();
        assert_eq!(restored, vec![live.to_string()]);
        assert_eq!(count(&store, live), 2);
    }

    #[test]
    fn clone_version_copies_into_new_graphs() {
        let store = TripleStore::in_memory().unwrap();
        let base = "http://x";
        let live = "http://x/g/data";
        put(&store, live, "http://x/a", "http://x/p", "http://x/b");

        let v1 = snapshot_graphs(&store, base, "ds1", "1.0.0", &[live.to_string()]).unwrap();
        let branch = clone_version(&store, base, "ds1", &v1, "1.0.0-feature").unwrap();
        assert_eq!(branch.len(), 1);
        // Branch keeps the same source target for restore.
        assert_eq!(branch[0].source_graph, live);
        assert!(branch[0].snapshot_graph.contains("/version/1.0.0-feature/"));
        assert_eq!(count(&store, &branch[0].snapshot_graph), 1);
    }
}
