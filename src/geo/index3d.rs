//! 3D spatial index — an R*-tree over per-geometry axis-aligned bounding boxes
//! (spec §3.5). The broad phase: a `distance3d`/`sf3dIntersects` query first asks
//! the tree for candidate IRIs whose AABBs overlap, then the exact `ots-geof:`
//! test runs only on those candidates (the standard PostGIS/CGAL two-phase
//! pattern). Mirrors the 2D [`super::spatial_index`] but keyed by `[f64; 3]`.
//!
//! Built and exercised by tests today; wired into the Graph-Store write path via
//! [`SpatialIndex3D`] (lazy AABB rebuild on dirty) and consumed by the OGC API /
//! 3D-Tiles broad phase ([`crate::tiles3d`]).

use rstar::{RTree, RTreeObject, AABB};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use tracing::info;

use super::datatypes::extract_wkt;
use super::geom3d::{parse_wkt3d, wkt_is_3d, Aabb3};

/// `geo:asWKT` — the geometry-serialisation predicate the index scans.
const GEO_AS_WKT: &str = "http://www.opengis.net/ont/geosparql#asWKT";

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
    #[allow(dead_code)] // exercised by tests / kept for API symmetry with the 2D index
    pub fn min(&self) -> [f64; 3] {
        self.envelope.lower()
    }
    #[allow(dead_code)] // exercised by tests / kept for API symmetry with the 2D index
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

    #[allow(dead_code)] // incremental insert path; the store wiring rebuilds in bulk
    pub fn insert(&mut self, entry: AabbEntry) {
        self.tree.insert(entry);
    }

    /// Candidate IRIs whose AABB intersects the query box (broad phase).
    pub fn candidates(&self, query: &Aabb3) -> Vec<String> {
        let env = AABB::from_corners(query.min, query.max);
        self.tree
            .locate_in_envelope_intersecting(env)
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

/// Thread-safe 3D R*-tree index over geometry bounding boxes — the volumetric
/// twin of [`super::spatial_index::SpatialIndex`].
///
/// It indexes **only** the geometries that are genuinely 3D (a `Z`/`ZM`-tagged
/// or natively-volumetric WKT, or a parsed AABB with a non-degenerate Z extent);
/// pure-2D footprints stay with the GEOS-backed 2D index and are skipped here, so
/// the broad phase never returns a flat geometry as a 3D candidate.
///
/// Like the 2D index it is `dirty`-flagged on every write to `geo:asWKT` triples
/// and rebuilt lazily on next access (see [`crate::store::TripleStore`]).
#[derive(Clone)]
pub struct SpatialIndex3D {
    tree: Arc<RwLock<Index3D>>,
    dirty: Arc<AtomicBool>,
}

impl Default for SpatialIndex3D {
    fn default() -> Self {
        Self::new()
    }
}

impl SpatialIndex3D {
    pub fn new() -> Self {
        Self {
            tree: Arc::new(RwLock::new(Index3D::new())),
            dirty: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Rebuild the entire 3D index from the store.
    ///
    /// Scans all triples with predicate `geo:asWKT`, strips the optional `<crs>`
    /// prefix, parses the WKT-Z body and computes its [`Aabb3`]. A geometry is
    /// indexed only when it is genuinely 3D — either the WKT names a 3D form
    /// ([`wkt_is_3d`]) or the bounding box has a non-degenerate Z extent. Pure-2D
    /// footprints are skipped (they belong to the 2D index). The 3D AABBs are then
    /// bulk-loaded into the R*-tree.
    pub fn rebuild(&self, store: &oxigraph::store::Store) {
        use oxigraph::model::{NamedNodeRef, NamedOrBlankNode, Term};

        let wkt_pred = NamedNodeRef::new_unchecked(GEO_AS_WKT);
        let mut entries: Vec<AabbEntry> = Vec::new();

        for quad in store.quads_for_pattern(None, Some(wkt_pred), None, None) {
            let quad = match quad {
                Ok(q) => q,
                Err(_) => continue,
            };

            // The geometry-node subject IRI is the index key (matches the 2D index).
            let subject_iri = match &quad.subject {
                NamedOrBlankNode::NamedNode(nn) => nn.as_str().to_string(),
                _ => continue,
            };

            // We only handle literal WKT here; strip any leading `<crs>` prefix.
            let value = match &quad.object {
                Term::Literal(lit) => lit.value().to_string(),
                _ => continue,
            };
            let body = extract_wkt(&value);

            let Some(geom) = parse_wkt3d(body) else {
                continue;
            };
            let Some(bbox) = geom.aabb() else {
                continue;
            };

            // Skip pure-2D geometry: index only when the WKT is volumetric/Z-tagged
            // or the AABB actually has vertical extent.
            let is_3d = wkt_is_3d(body) || bbox.min[2] != bbox.max[2];
            if !is_3d {
                continue;
            }

            entries.push(AabbEntry::new(subject_iri, &bbox));
        }

        let count = entries.len();
        *self.tree.write().unwrap() = Index3D::bulk_load(entries);
        self.dirty.store(false, Ordering::Relaxed);
        info!("3D R*-tree index rebuilt with {} entries", count);
    }

    /// Candidate geometry IRIs whose AABB intersects the query box (broad phase).
    pub fn query_intersecting(&self, query: &Aabb3) -> Vec<String> {
        self.tree.read().unwrap().candidates(query)
    }

    /// Number of indexed (3D) geometries.
    pub fn len(&self) -> usize {
        self.tree.read().unwrap().len()
    }

    /// Whether the index holds no geometries.
    pub fn is_empty(&self) -> bool {
        self.tree.read().unwrap().is_empty()
    }

    /// Mark the index as dirty (needs rebuild on next access).
    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// Whether the index needs rebuilding.
    pub fn is_dirty(&self) -> bool {
        self.dirty.load(Ordering::Relaxed)
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

    #[test]
    fn rebuild_indexes_only_3d_geometry() {
        use crate::store::TripleStore;
        use oxigraph::io::RdfFormat;

        // `g3d` is a closed (volumetric) PolyhedralSurface Z; `g2d` is a flat
        // footprint. Only the 3D one must enter the 3D index.
        let data = r#"
            @prefix geo: <http://www.opengis.net/ont/geosparql#> .
            @prefix ex:  <http://example.org/> .
            ex:g3d geo:asWKT "POLYHEDRALSURFACE Z (((0 0 0,0 1 0,1 1 0,1 0 0,0 0 0)),((0 0 0,1 0 0,1 0 2,0 0 2,0 0 0)),((1 0 0,1 1 0,1 1 2,1 0 2,1 0 0)),((1 1 0,0 1 0,0 1 2,1 1 2,1 1 0)),((0 1 0,0 0 0,0 0 2,0 1 2,0 1 0)),((0 0 2,0 1 2,1 1 2,1 0 2,0 0 2)))"^^geo:wktLiteral .
            ex:g2d geo:asWKT "POLYGON((10 10, 11 10, 11 11, 10 11, 10 10))"^^geo:wktLiteral .
        "#;
        let store = TripleStore::in_memory().unwrap();
        store.load_str(data, RdfFormat::Turtle, None).unwrap();

        let idx = SpatialIndex3D::new();
        idx.rebuild(store.store());

        // Exactly the 3D geometry is indexed; the flat footprint is skipped.
        assert_eq!(idx.len(), 1, "only the 3D geometry should be indexed");

        // A box overlapping the 3D solid returns it.
        let hits = idx.query_intersecting(&bbox([0.0, 0.0, 0.0], [0.5, 0.5, 0.5]));
        assert_eq!(hits, vec!["http://example.org/g3d".to_string()]);

        // A box only over the flat 2D footprint returns nothing (it isn't indexed).
        let none = idx.query_intersecting(&bbox([10.2, 10.2, -1.0], [10.8, 10.8, 1.0]));
        assert!(
            none.is_empty(),
            "2D footprint must not be a 3D candidate: {none:?}"
        );
    }

    #[test]
    fn dirty_flag_roundtrips() {
        let idx = SpatialIndex3D::new();
        assert!(!idx.is_dirty());
        assert!(idx.is_empty());
        idx.mark_dirty();
        assert!(idx.is_dirty());
    }
}
