//! Change capture for LDES publishing.
//!
//! A write to a graph that belongs to a stream-enabled dataset is bracketed:
//! [`before`] indexes the graph's entities (IRI subject → hash of its direct
//! triples) and [`after`] re-indexes and diffs. Every entity whose hash
//! changed or appeared becomes a member carrying its current description
//! (direct triples plus blank-node closure); every entity that vanished
//! becomes a tombstone. Graphs of datasets without a stream are never indexed,
//! so writes elsewhere cost one indexed SQLite lookup.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use oxigraph::model::{GraphNameRef, NamedNodeRef, NamedOrBlankNode, Quad, Term};

use crate::server::AppState;
use crate::store::TripleStore;

/// entity IRI → order-independent hash of its `(predicate, object)` pairs
pub type SubjectIndex = HashMap<String, u64>;

/// What [`before`] captured: one index per tracked `(graph, dataset)`.
#[derive(Debug, Default)]
pub struct Before {
    pub graphs: Vec<(String, String, SubjectIndex)>,
}

fn hash_pair(p: &str, o: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    o.hash(&mut h);
    h.finish()
}

/// Index the IRI-subject entities of `graph`.
pub fn subject_index(store: &TripleStore, graph: &str) -> SubjectIndex {
    let mut idx: SubjectIndex = HashMap::new();
    let Ok(g) = NamedNodeRef::new(graph) else {
        return idx;
    };
    for q in store
        .store()
        .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(g)))
        .flatten()
    {
        if let NamedOrBlankNode::NamedNode(s) = &q.subject {
            let h = hash_pair(q.predicate.as_str(), &q.object.to_string());
            let e = idx.entry(s.as_str().to_string()).or_insert(0);
            *e = e.wrapping_add(h).rotate_left(1);
        }
    }
    idx
}

/// The entity's description as N-Triples: its direct triples plus the
/// closure of blank-node objects.
pub fn describe_entity(store: &TripleStore, graph: &str, entity: &str) -> String {
    let Ok(g) = NamedNodeRef::new(graph) else {
        return String::new();
    };
    let Ok(root) = NamedNodeRef::new(entity) else {
        return String::new();
    };
    let mut out = String::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: Vec<NamedOrBlankNode> = vec![NamedOrBlankNode::NamedNode(root.into_owned())];
    while let Some(s) = queue.pop() {
        for q in store
            .store()
            .quads_for_pattern(
                Some(s.as_ref()),
                None,
                None,
                Some(GraphNameRef::NamedNode(g)),
            )
            .flatten()
        {
            let Quad {
                subject,
                predicate,
                object,
                ..
            } = q;
            if let Term::BlankNode(b) = &object {
                if seen.insert(b.as_str().to_string()) {
                    queue.push(NamedOrBlankNode::BlankNode(b.clone()));
                }
            }
            out.push_str(&format!("{subject} {predicate} {object} .\n"));
        }
    }
    out
}

/// Capture the entity index of every tracked graph among `graphs`.
pub fn before(state: &AppState, graphs: &[String]) -> Before {
    let tracked = match crate::ldes::store::tracked(&state.auth_db, graphs) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("ldes: tracked-graph lookup failed: {e}");
            return Before::default();
        }
    };
    Before {
        graphs: tracked
            .into_iter()
            .map(|(g, ds)| {
                let idx = subject_index(&state.store, &g);
                (g, ds, idx)
            })
            .collect(),
    }
}

/// Re-index the graphs captured by [`before`] and append a member per change.
/// Returns the number of members written. Best-effort: failures are logged.
pub fn after(state: &AppState, before: Before) -> usize {
    let mut written = 0;
    let now = chrono::Utc::now().to_rfc3339();
    for (graph, dataset, old) in before.graphs {
        let new = subject_index(&state.store, &graph);
        for (entity, h) in &new {
            if old.get(entity) == Some(h) {
                continue;
            }
            let nt = describe_entity(&state.store, &graph, entity);
            match crate::ldes::store::insert_member(
                &state.auth_db,
                &dataset,
                entity,
                &graph,
                &now,
                false,
                &nt,
            ) {
                Ok(_) => written += 1,
                Err(e) => tracing::warn!("ldes: member insert failed for <{entity}>: {e}"),
            }
        }
        for entity in old.keys() {
            if new.contains_key(entity) {
                continue;
            }
            match crate::ldes::store::insert_member(
                &state.auth_db,
                &dataset,
                entity,
                &graph,
                &now,
                true,
                "",
            ) {
                Ok(_) => written += 1,
                Err(e) => tracing::warn!("ldes: tombstone insert failed for <{entity}>: {e}"),
            }
        }
    }
    written
}

/// Publish every entity of `graphs` as a member (initial publish when a
/// stream is enabled, and after a bulk import). Only applies when the dataset
/// has an enabled stream.
pub fn publish_all(state: &AppState, dataset_id: &str, graphs: &[String]) -> usize {
    match crate::ldes::store::stream(&state.auth_db, dataset_id) {
        Ok(Some(cfg)) if cfg.enabled => {}
        _ => return 0,
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut written = 0;
    for graph in graphs {
        let idx = subject_index(&state.store, graph);
        let mut entities: Vec<&String> = idx.keys().collect();
        entities.sort();
        for entity in entities {
            let nt = describe_entity(&state.store, graph, entity);
            match crate::ldes::store::insert_member(
                &state.auth_db,
                dataset_id,
                entity,
                graph,
                &now,
                false,
                &nt,
            ) {
                Ok(_) => written += 1,
                Err(e) => tracing::warn!("ldes: member insert failed for <{entity}>: {e}"),
            }
        }
    }
    written
}
