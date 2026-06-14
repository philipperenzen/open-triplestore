//! 3D spatial index — an R*-tree over per-geometry axis-aligned bounding boxes
//! (spec §3.5). The broad phase: a `distance3d`/`sf3dIntersects` query first asks
//! the tree for candidate IRIs whose AABBs overlap, then the exact `ots-geof:`
//! test runs only on those candidates (the standard PostGIS/CGAL two-phase
//! pattern). Mirrors the 2D [`super::spatial_index`] but keyed by `[f64; 3]`.
//!
//! Built and exercised by tests today; wiring it into the Graph-Store write path
//! (transactional AABB upserts on commit) and the OGC API/3D-Tiles broad phase
//! is the P1.5 task, so the public API is `allow(dead_code)` until then.
#![allow(dead_code)]

use rstar::{RTree, RTreeObject, AABB};

use super::geom3d::Aabb3;

/// One indexed geometry: an IRI and its 3D bounding box.
#[derive(Clone, Debug)]
pub struct AabbEntry {
    pub iri: String,
    envelope: AABB<[f64; 3]>,
}

impl AabbEntry {
    pub fn new(iri: impl Into<String>, bbox: &Aabb3) -> Self {
        AabbEntry {
            iri: iri.into(),
            envelope: AABB::from_corners(bbox.min, bbox.max),
        }
    }
    pub fn min(&self) -> [f64; 3] {
        self.envelope.lower()
    }
    pub fn max(&self) -> [f64; 3] {
        self.envelope.upper()
    }
}

impl RTreeObject for AabbEntry {
    type Envelope = AABB<[f64; 3]>;
    fn envelope(&self) -> Self::Envelope {
        self.envelope
    }
}

/// An R*-tree over 3D AABBs.
pub struct Index3D {
    tree: RTree<AabbEntry>,
}

impl Default for Index3D {
    fn default() -> Self {
        Self::new()
    }
}

impl Index3D {
    pub fn new() -> Self {
        Index3D { tree: RTree::new() }
    }

    /// Build the tree in one shot (cheaper, better-balanced than repeated insert).
    pub fn bulk_load(entries: Vec<AabbEntry>) -> Self {
        Index3D {
            tree: RTree::bulk_load(entries),
        }
    }

    pub fn insert(&mut self, entry: AabbEntry) {
        self.tree.insert(entry);
    }

    /// Candidate IRIs whose AABB intersects the query box (broad phase).
    pub fn candidates(&self, query: &Aabb3) -> Vec<String> {
        let env = AABB::from_corners(query.min, query.max);
        self.tree
            .locate_in_envelope_intersecting(&env)
            .map(|e| e.iri.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tree.size()
    }
    pub fn is_empty(&self) -> bool {
        self.tree.size() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bbox(min: [f64; 3], max: [f64; 3]) -> Aabb3 {
        Aabb3 { min, max }
    }

    #[test]
    fn two_phase_candidates() {
        let idx = Index3D::bulk_load(vec![
            AabbEntry::new("a", &bbox([0.0, 0.0, 0.0], [5.0, 5.0, 5.0])),
            AabbEntry::new("b", &bbox([10.0, 10.0, 10.0], [20.0, 20.0, 20.0])),
            AabbEntry::new("c", &bbox([3.0, 3.0, 3.0], [8.0, 8.0, 8.0])),
        ]);
        let hits = idx.candidates(&bbox([1.0, 1.0, 1.0], [6.0, 6.0, 6.0]));
        assert!(hits.contains(&"a".to_string()));
        assert!(hits.contains(&"c".to_string()));
        assert!(!hits.contains(&"b".to_string()));
    }

    #[test]
    fn z_separation_excludes() {
        // Same XY, disjoint Z — the 2D index would match, the 3D index must not.
        let idx = Index3D::bulk_load(vec![
            AabbEntry::new("ground", &bbox([0.0, 0.0, 0.0], [10.0, 10.0, 3.0])),
            AabbEntry::new("roof", &bbox([0.0, 0.0, 50.0], [10.0, 10.0, 53.0])),
        ]);
        let hits = idx.candidates(&bbox([1.0, 1.0, 0.0], [2.0, 2.0, 2.0]));
        assert_eq!(hits, vec!["ground".to_string()]);
    }
}
