//! One consistent read of a set of graphs, for a caller that copies them out
//! of the store (the repair layer's sandbox, `docs/notes/repair-layer-design.md`
//! §5.1 and §5.3).
//!
//! `quads_for_graph` reads the live store once per graph, outside any
//! transaction, so a write landing between two graphs is half-seen. This
//! opens one source for the whole set, the same three the SHACL view picks
//! (`src/shacl/view.rs`):
//!
//! * the query accelerator's clean in-memory copy of the store, when one is
//!   published — a consistent snapshot by construction;
//! * otherwise, on a persistent store, one RocksDB readable transaction for
//!   every graph — a single snapshot, no lock, writers never blocked;
//! * otherwise (the in-memory backend, whose transaction would take the
//!   exclusive write lock) the live store per graph. That read is not a
//!   snapshot; the caller compares [`TripleStore::write_generation`] before
//!   and after and says so when it moved ([`GraphsSnapshot::source`] tells
//!   it which case it is in).
//!
//! Lives in its own file and uses only `TripleStore`'s public surface, so the
//! engine module is untouched.

use std::time::Instant;

use oxigraph::model::{GraphName, GraphNameRef, Quad};

use super::{StoreError, TripleStore};

/// Where a [`GraphsSnapshot`] was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotSource {
    /// The accelerator's published copy of the whole store.
    Mirror,
    /// One RocksDB transaction for every graph.
    Transaction,
    /// The live in-memory store, graph by graph: consistent only if the
    /// write generation did not move while it was read.
    Live,
}

impl SnapshotSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SnapshotSource::Mirror => "mirror",
            SnapshotSource::Transaction => "snapshot",
            SnapshotSource::Live => "live",
        }
    }
}

/// The quads of a set of graphs, read through one source.
#[derive(Debug, Default)]
pub struct GraphsSnapshot {
    pub quads: Vec<Quad>,
    pub source: Option<SnapshotSource>,
}

/// Why a snapshot read stopped short.
#[derive(Debug)]
pub enum SnapshotError {
    Store(StoreError),
    /// The deadline passed between two graphs; the copy was abandoned.
    Deadline {
        graphs_read: usize,
    },
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotError::Store(e) => write!(f, "{e}"),
            SnapshotError::Deadline { graphs_read } => write!(
                f,
                "the time budget ran out after {graphs_read} graph(s) were read"
            ),
        }
    }
}

impl TripleStore {
    /// Every quad of `graphs`, read through one consistent source (see the
    /// module docs). `deadline` is checked between graphs: past it the read
    /// stops and the copy so far is dropped, so a request that has run out
    /// of time does not keep reading.
    pub fn quads_for_graphs_snapshot(
        &self,
        graphs: &[GraphName],
        deadline: Option<Instant>,
    ) -> Result<GraphsSnapshot, SnapshotError> {
        let past = |read: usize| -> Result<(), SnapshotError> {
            match deadline {
                Some(d) if Instant::now() >= d => {
                    Err(SnapshotError::Deadline { graphs_read: read })
                }
                _ => Ok(()),
            }
        };
        let mut quads: Vec<Quad> = Vec::new();
        if let Some(mirror) = self.mirror_full_copy() {
            for (i, g) in graphs.iter().enumerate() {
                past(i)?;
                for q in mirror.quads_for_pattern(None, None, None, Some(graph_ref(g))) {
                    quads.push(q.map_err(|e| SnapshotError::Store(e.into()))?);
                }
            }
            return Ok(GraphsSnapshot {
                quads,
                source: Some(SnapshotSource::Mirror),
            });
        }
        if self.is_persistent() {
            if let Ok(tx) = self.store().start_transaction() {
                for (i, g) in graphs.iter().enumerate() {
                    past(i)?;
                    for q in tx.quads_for_pattern(None, None, None, Some(graph_ref(g))) {
                        quads.push(q.map_err(|e| SnapshotError::Store(e.into()))?);
                    }
                }
                return Ok(GraphsSnapshot {
                    quads,
                    source: Some(SnapshotSource::Transaction),
                });
            }
        }
        for (i, g) in graphs.iter().enumerate() {
            past(i)?;
            for q in self
                .store()
                .quads_for_pattern(None, None, None, Some(graph_ref(g)))
            {
                quads.push(q.map_err(|e| SnapshotError::Store(e.into()))?);
            }
        }
        Ok(GraphsSnapshot {
            quads,
            source: Some(SnapshotSource::Live),
        })
    }
}

fn graph_ref(g: &GraphName) -> GraphNameRef<'_> {
    g.as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::RdfFormat;
    use oxigraph::model::NamedNode;

    fn named(iri: &str) -> GraphName {
        GraphName::NamedNode(NamedNode::new(iri).unwrap())
    }

    #[test]
    fn reads_every_named_graph_of_the_set_and_nothing_else() {
        let store = TripleStore::in_memory().unwrap();
        for (g, n) in [("urn:g1", 3), ("urn:g2", 2), ("urn:other", 4)] {
            let nt: String = (0..n)
                .map(|i| format!("<urn:s{i}> <urn:p> <urn:o{i}> .\n"))
                .collect();
            store.load_str(&nt, RdfFormat::NTriples, Some(g)).unwrap();
        }
        let snap = store
            .quads_for_graphs_snapshot(&[named("urn:g1"), named("urn:g2")], None)
            .unwrap();
        assert_eq!(snap.quads.len(), 5);
        assert_eq!(snap.source, Some(SnapshotSource::Live));
        assert!(snap
            .quads
            .iter()
            .all(|q| q.graph_name != named("urn:other")));
    }

    #[test]
    fn a_passed_deadline_abandons_the_read() {
        let store = TripleStore::in_memory().unwrap();
        let past = Instant::now() - std::time::Duration::from_secs(1);
        match store.quads_for_graphs_snapshot(&[named("urn:g1")], Some(past)) {
            Err(SnapshotError::Deadline { graphs_read: 0 }) => {}
            other => panic!("expected the deadline, got {other:?}"),
        }
    }

    #[test]
    fn a_persistent_store_is_read_through_one_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let store = TripleStore::open(dir.path())
            .unwrap()
            .with_parallel_query(false, 1, 0);
        store
            .load_str(
                "<urn:s> <urn:p> <urn:o> .",
                RdfFormat::NTriples,
                Some("urn:g"),
            )
            .unwrap();
        let snap = store
            .quads_for_graphs_snapshot(&[named("urn:g")], None)
            .unwrap();
        assert_eq!(snap.quads.len(), 1);
        assert_eq!(snap.source, Some(SnapshotSource::Transaction));
    }
}
