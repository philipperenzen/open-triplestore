//! Copy a dataset's live named graphs into version-scoped snapshot graphs, and
//! restore a snapshot back onto the live graphs.

use oxigraph::model::*;
use std::collections::HashMap;

use crate::store::engine::StoreError;
use crate::store::TripleStore;
use super::models::GraphMapping;

fn slugify_last_segment(iri: &str) -> String {
    let last = iri.trim_end_matches('/').rsplit('/').next().unwrap_or("graph");
    let slug: String = last.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() { "graph".to_string() } else { slug }
}

fn version_iri(base_url: &str, dataset_id: &str, version: &str) -> String {
    format!("{base_url}/dataset/{dataset_id}/version/{version}")
}

/// Ensure snapshot suffixes are unique even when two source graphs slugify to
/// the same value (disambiguate with an index suffix).
fn unique_suffix(used: &mut HashMap<String, usize>, base: &str) -> String {
    let n = used.entry(base.to_string()).or_insert(0);
    let out = if *n == 0 { base.to_string() } else { format!("{base}-{n}") };
    *n += 1;
    out
}

/// Snapshot the given live source graphs into version-scoped graphs.
/// Returns the snapshot→source mappings (snapshot graph IRIs included).
pub fn snapshot_graphs(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
    source_graphs: &[String],
) -> Result<Vec<GraphMapping>, StoreError> {
    let ver_iri = version_iri(base_url, dataset_id, version);
    let mut used: HashMap<String, usize> = HashMap::new();

    let mut mappings: Vec<GraphMapping> = Vec::new();
    for src in source_graphs {
        let suffix = unique_suffix(&mut used, &slugify_last_segment(src));
        let snap = format!("{ver_iri}/{suffix}");
        mappings.push(GraphMapping { snapshot_graph: snap, source_graph: src.clone() });
    }

    // Clear snapshot targets first (idempotent on retry).
    let snap_iris: Vec<String> = mappings.iter().map(|m| m.snapshot_graph.clone()).collect();
    let snap_refs: Vec<&str> = snap_iris.iter().map(|s| s.as_str()).collect();
    store.bulk_delete_graphs(&snap_refs)?;

    // Copy quads from each source graph into its snapshot graph.
    let mut all_quads: Vec<Quad> = Vec::new();
    for m in &mappings {
        let target = GraphName::NamedNode(
            NamedNode::new(&m.snapshot_graph)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?,
        );
        let src_g = GraphNameRef::NamedNode(
            NamedNodeRef::new(&m.source_graph)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?,
        );
        for q in store.quads_for_graph(src_g)? {
            all_quads.push(Quad::new(q.subject, q.predicate, q.object, target.clone()));
        }
    }
    store.bulk_insert_quads(all_quads, &snap_iris)?;
    Ok(mappings)
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
    let mut used: HashMap<String, usize> = HashMap::new();

    let mut new_map: Vec<GraphMapping> = Vec::new();
    for m in source_map {
        let suffix = unique_suffix(&mut used, &slugify_last_segment(&m.snapshot_graph));
        let snap = format!("{ver_iri}/{suffix}");
        new_map.push(GraphMapping { snapshot_graph: snap, source_graph: m.source_graph.clone() });
    }

    let new_iris: Vec<String> = new_map.iter().map(|m| m.snapshot_graph.clone()).collect();
    let new_refs: Vec<&str> = new_iris.iter().map(|s| s.as_str()).collect();
    store.bulk_delete_graphs(&new_refs)?;

    let mut all_quads: Vec<Quad> = Vec::new();
    for (old, new) in source_map.iter().zip(new_map.iter()) {
        let target = GraphName::NamedNode(
            NamedNode::new(&new.snapshot_graph)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?,
        );
        let src_g = GraphNameRef::NamedNode(
            NamedNodeRef::new(&old.snapshot_graph)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?,
        );
        for q in store.quads_for_graph(src_g)? {
            all_quads.push(Quad::new(q.subject, q.predicate, q.object, target.clone()));
        }
    }
    store.bulk_insert_quads(all_quads, &new_iris)?;
    Ok(new_map)
}

/// Restore a version's snapshots back onto the dataset's live source graphs.
/// Each source graph is cleared then re-populated from its snapshot.
pub fn restore(
    store: &TripleStore,
    source_map: &[GraphMapping],
) -> Result<Vec<String>, StoreError> {
    let src_iris: Vec<String> = source_map.iter().map(|m| m.source_graph.clone()).collect();
    let src_refs: Vec<&str> = src_iris.iter().map(|s| s.as_str()).collect();
    store.bulk_delete_graphs(&src_refs)?;

    let mut all_quads: Vec<Quad> = Vec::new();
    for m in source_map {
        let target = GraphName::NamedNode(
            NamedNode::new(&m.source_graph)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?,
        );
        let snap_g = GraphNameRef::NamedNode(
            NamedNodeRef::new(&m.snapshot_graph)
                .map_err(|e| StoreError::Parse(format!("Invalid IRI: {e}")))?,
        );
        for q in store.quads_for_graph(snap_g)? {
            all_quads.push(Quad::new(q.subject, q.predicate, q.object, target.clone()));
        }
    }
    store.bulk_insert_quads(all_quads, &src_iris)?;
    Ok(src_iris)
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
        assert!(map[0].snapshot_graph.starts_with("http://x/dataset/ds1/version/1.0.0/"));
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
