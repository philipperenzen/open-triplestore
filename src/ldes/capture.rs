//! Change capture for LDES publishing.
//!
//! A write to a graph that belongs to a stream-enabled dataset is bracketed:
//! [`before`] indexes the graph's entities (IRI subject → hash of the
//! description a member would carry: direct triples plus blank-node closure)
//! and [`after`] re-indexes and diffs. Every entity whose hash
//! changed or appeared becomes a member carrying its current description
//! (direct triples plus blank-node closure); every entity that vanished
//! becomes a tombstone. Graphs of datasets without a stream are never indexed,
//! so writes elsewhere cost one indexed SQLite lookup.
//!
//! Graphs marked private are never published: [`before`] does not track them
//! and [`publish_all`] skips them, so their entities never become members of
//! a stream that every viewer of the dataset can read. Members published
//! before a graph was marked private are not retracted.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use oxigraph::model::{GraphNameRef, NamedNodeRef, NamedOrBlankNode, Quad, Term};

use crate::server::AppState;
use crate::store::TripleStore;

/// entity IRI → order-independent hash of its description
pub type SubjectIndex = HashMap<String, u64>;

/// What [`before`] captured: one index per tracked `(graph, dataset)`.
#[derive(Debug, Default)]
pub struct Before {
    pub graphs: Vec<(String, String, SubjectIndex)>,
}

fn hash_of<T: Hash + ?Sized>(v: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

fn hash_pair(p: &str, o: u64) -> u64 {
    hash_of(&(p, o))
}

/// Hashes the blank nodes of one graph by content, so an edit anywhere in an
/// entity's blank-node closure changes the entity's hash — and re-writing
/// the same structure under fresh blank-node labels does not.
struct ClosureHasher<'a> {
    store: &'a TripleStore,
    graph: NamedNodeRef<'a>,
    memo: HashMap<String, u64>,
    open: HashSet<String>,
}

impl ClosureHasher<'_> {
    fn term(&mut self, t: &Term) -> u64 {
        match t {
            Term::BlankNode(b) => self.blank(b.as_str()),
            other => hash_of(&other.to_string()),
        }
    }

    /// The pairs of `label` summed (wrapping), so the result is independent
    /// of quad order. A cycle back into a node being hashed contributes a
    /// constant.
    fn blank(&mut self, label: &str) -> u64 {
        if let Some(h) = self.memo.get(label) {
            return *h;
        }
        if !self.open.insert(label.to_string()) {
            return hash_of("ots:blank-node-cycle");
        }
        let node = oxigraph::model::BlankNode::new_unchecked(label);
        let quads: Vec<Quad> = self
            .store
            .store()
            .quads_for_pattern(
                Some(node.as_ref().into()),
                None,
                None,
                Some(GraphNameRef::NamedNode(self.graph)),
            )
            .flatten()
            .collect();
        let mut h = hash_of("ots:blank-node");
        for q in &quads {
            let o = self.term(&q.object);
            h = h.wrapping_add(hash_pair(q.predicate.as_str(), o));
        }
        self.open.remove(label);
        self.memo.insert(label.to_string(), h);
        h
    }
}

/// Index the IRI-subject entities of `graph`: each entity's hash covers its
/// direct triples and, through [`ClosureHasher`], the blank nodes they reach
/// — the same description [`describe_entity`] publishes.
pub fn subject_index(store: &TripleStore, graph: &str) -> SubjectIndex {
    let mut idx: SubjectIndex = HashMap::new();
    let Ok(g) = NamedNodeRef::new(graph) else {
        return idx;
    };
    let mut hasher = ClosureHasher {
        store,
        graph: g,
        memo: HashMap::new(),
        open: HashSet::new(),
    };
    for q in store
        .store()
        .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(g)))
        .flatten()
    {
        if let NamedOrBlankNode::NamedNode(s) = &q.subject {
            let h = hash_pair(q.predicate.as_str(), hasher.term(&q.object));
            let e = idx.entry(s.as_str().to_string()).or_insert(0);
            *e = e.wrapping_add(h);
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

/// After members were appended to `dataset_id`: seal the pages that filled
/// and apply the retention policy, if one is declared (debounced unless
/// `force`). Returns the number of members the policy removed. Best-effort.
pub fn settle(state: &AppState, dataset_id: &str, force: bool) -> usize {
    let page_size = match crate::ldes::store::stream(&state.auth_db, dataset_id) {
        Ok(Some(cfg)) => cfg.page_size,
        Ok(None) => return 0,
        Err(e) => {
            tracing::warn!("ldes: stream lookup failed for {dataset_id}: {e}");
            return 0;
        }
    };
    if let Err(e) = crate::ldes::store::seal_full_pages(&state.auth_db, dataset_id, page_size) {
        tracing::warn!("ldes: sealing pages failed for {dataset_id}: {e}");
    }
    crate::ldes::store::sweep(&state.auth_db, dataset_id, force)
}

/// Re-index the graphs captured by [`before`] and append a member per change.
/// Returns the number of members written. Best-effort: failures are logged.
pub fn after(state: &AppState, before: Before) -> usize {
    let mut written = 0;
    let now = chrono::Utc::now().to_rfc3339();
    let mut touched: HashSet<String> = HashSet::new();
    for (graph, dataset, old) in before.graphs {
        touched.insert(dataset.clone());
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
    if written > 0 {
        for dataset in &touched {
            settle(state, dataset, false);
        }
    }
    written
}

/// How a write handler captures LDES members: from the change log's rows
/// when the log records the write — no graph is scanned (delta note §2.6) —
/// else by indexing the tracked graphs before and after ([`before`] /
/// [`after`]).
pub enum Capture {
    /// The log records this write; `graphs` are the tracked ones among the
    /// write's targets, with their dataset.
    Rows(Vec<(String, String)>),
    Scan(Before),
}

impl Capture {
    /// Start capturing a write to `graphs`. Off the async runtime: the scan
    /// fallback reads every tracked graph.
    pub fn begin(state: &AppState, graphs: &[String]) -> Capture {
        if state.store.changes().enabled() {
            match crate::ldes::store::tracked(&state.auth_db, graphs) {
                Ok(t) => Capture::Rows(t),
                Err(e) => {
                    tracing::warn!("ldes: tracked-graph lookup failed: {e}");
                    Capture::Rows(Vec::new())
                }
            }
        } else {
            Capture::Scan(before(state, graphs))
        }
    }

    /// Publish what the write changed. `rows` are the write's own rows
    /// (`ChangeLog::rows_for_commit`); empty when the log did not record it
    /// after all, in which case every tracked graph is republished whole —
    /// correct, if not cheap. Returns the members written.
    pub fn finish(self, state: &AppState, rows: &[crate::store::changes::ChangeRow]) -> usize {
        match self {
            Capture::Scan(b) => after(state, b),
            Capture::Rows(tracked) => after_rows(state, &tracked, rows),
        }
    }
}

/// `OTS_LDES_BNODE_DEPTH` (default 8): how far up a blank-node tree the
/// owner of a changed blank-node subject is looked for.
fn bnode_depth() -> usize {
    std::env::var("OTS_LDES_BNODE_DEPTH")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(8)
}

/// `OTS_LDES_BNODE_SUBJECTS_MAX` (default 10 000): above this many
/// blank-node subjects in one graph's rows the graph is republished whole
/// instead of walking each one up to its owner.
fn bnode_subjects_max() -> usize {
    std::env::var("OTS_LDES_BNODE_SUBJECTS_MAX")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(10_000)
}

/// The IRI entity a blank node hangs from in `graph`: up the tree through
/// the store as it is now, and through the quads the write removed (a link
/// the write cut is still the owner it changed). `None` for an orphan.
fn bnode_owner(
    store: &TripleStore,
    graph: NamedNodeRef<'_>,
    bnode: &oxigraph::model::BlankNode,
    removed: &[Quad],
    depth: usize,
) -> Option<String> {
    let mut current = bnode.clone();
    for _ in 0..depth {
        let parent: Option<NamedOrBlankNode> = store
            .store()
            .quads_for_pattern(
                None,
                None,
                Some(oxigraph::model::TermRef::BlankNode(current.as_ref())),
                Some(GraphNameRef::NamedNode(graph)),
            )
            .flatten()
            .map(|q| q.subject)
            .next()
            .or_else(|| {
                removed
                    .iter()
                    .find(|q| matches!(&q.object, Term::BlankNode(b) if *b == current))
                    .map(|q| q.subject.clone())
            });
        match parent? {
            NamedOrBlankNode::NamedNode(n) => return Some(n.as_str().to_string()),
            NamedOrBlankNode::BlankNode(b) => current = b,
        }
    }
    None
}

/// Members from a write's rows: every IRI subject a row added or removed a
/// quad of, plus the owner of every changed blank-node subtree, becomes a
/// member carrying its current description — or a tombstone when nothing of
/// it is left in the graph. A graph whose rows do not carry their quads (a
/// change over the payload cap, or one the log could not bound) is
/// republished whole, as is one with more blank-node subjects than
/// `OTS_LDES_BNODE_SUBJECTS_MAX`.
pub fn after_rows(
    state: &AppState,
    tracked: &[(String, String)],
    rows: &[crate::store::changes::ChangeRow],
) -> usize {
    use crate::store::changes::{decode_text, Extent};
    if tracked.is_empty() {
        return 0;
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut written = 0;
    let mut touched: HashSet<String> = HashSet::new();
    let store_scoped = rows.iter().any(|r| r.scope == "store");
    for (graph, dataset) in tracked {
        let mine: Vec<&crate::store::changes::ChangeRow> = rows
            .iter()
            .filter(|r| r.graph_iri.as_deref() == Some(graph.as_str()))
            .collect();
        let whole =
            store_scoped || mine.is_empty() || mine.iter().any(|r| r.extent != Extent::Full);
        let Ok(g) = NamedNodeRef::new(graph) else {
            continue;
        };
        let mut entities: HashSet<String> = HashSet::new();
        let mut blank_subjects = 0usize;
        let mut republish = whole;
        if !republish {
            let removed: Vec<Quad> = mine
                .iter()
                .flat_map(|r| decode_text(r.removed.as_deref()))
                .collect();
            let added: Vec<Quad> = mine
                .iter()
                .flat_map(|r| decode_text(r.added.as_deref()))
                .collect();
            let max = bnode_subjects_max();
            let depth = bnode_depth();
            for q in added.iter().chain(removed.iter()) {
                match &q.subject {
                    NamedOrBlankNode::NamedNode(s) => {
                        entities.insert(s.as_str().to_string());
                    }
                    NamedOrBlankNode::BlankNode(b) => {
                        blank_subjects += 1;
                        if blank_subjects > max {
                            republish = true;
                            break;
                        }
                        if let Some(owner) = bnode_owner(&state.store, g, b, &removed, depth) {
                            entities.insert(owner);
                        }
                    }
                }
            }
        }
        if republish {
            written += publish_all(state, dataset, std::slice::from_ref(graph));
            continue;
        }
        let mut entities: Vec<String> = entities.into_iter().collect();
        entities.sort();
        for entity in entities {
            let Ok(e) = NamedNodeRef::new(&entity) else {
                continue;
            };
            let present = state
                .store
                .store()
                .quads_for_pattern(
                    Some(oxigraph::model::NamedOrBlankNodeRef::NamedNode(e)),
                    None,
                    None,
                    Some(GraphNameRef::NamedNode(g)),
                )
                .next()
                .is_some();
            let (deleted, nt) = if present {
                (false, describe_entity(&state.store, graph, &entity))
            } else {
                (true, String::new())
            };
            match crate::ldes::store::insert_member(
                &state.auth_db,
                dataset,
                &entity,
                graph,
                &now,
                deleted,
                &nt,
            ) {
                Ok(_) => {
                    written += 1;
                    touched.insert(dataset.clone());
                }
                Err(e) => tracing::warn!("ldes: member insert failed for <{entity}>: {e}"),
            }
        }
    }
    for dataset in &touched {
        settle(state, dataset, false);
    }
    written
}

/// Publish every entity of `graphs` as a member (initial publish when a
/// stream is enabled, and after a bulk import). Only applies when the dataset
/// has an enabled stream; graphs the dataset marks private are skipped.
pub fn publish_all(state: &AppState, dataset_id: &str, graphs: &[String]) -> usize {
    match crate::ldes::store::stream(&state.auth_db, dataset_id) {
        Ok(Some(cfg)) if cfg.enabled => {}
        _ => return 0,
    }
    let private: HashSet<String> = match state.auth_db.list_dataset_graph_entries(dataset_id) {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| e.private)
            .map(|e| e.graph_iri)
            .collect(),
        Err(e) => {
            tracing::warn!("ldes: graph registry lookup failed; publishing nothing: {e}");
            return 0;
        }
    };
    let now = chrono::Utc::now().to_rfc3339();
    let mut written = 0;
    for graph in graphs
        .iter()
        .filter(|g| !private.contains(*g))
        .filter(|g| !crate::provenance::is_any_statement_provenance_graph(g))
    {
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
    if written > 0 {
        settle(state, dataset_id, false);
    }
    written
}
