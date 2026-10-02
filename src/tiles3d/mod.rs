//! 3D Tiles 1.1 generation (spec §6) — turn stored geometry into a single-tile
//! tileset whose features carry their IRI.
//!
//! Two routes per dataset:
//!
//! * `GET /api/datasets/:id/3dtiles/tileset.json` — a 3D Tiles **1.1** tileset
//!   (`asset.version "1.1"`) with a single root tile (`refine: "ADD"`) whose
//!   `boundingVolume.region` is computed from the features' bounding boxes and
//!   whose `content.uri` points at the GLB below. Single-tile for v1: no
//!   implicit tiling yet (see TODOs).
//! * `GET /api/datasets/:id/3dtiles/content.glb` — a binary glTF (GLB) holding the
//!   dataset's building meshes with the **EXT_mesh_features** + **EXT_structural_metadata**
//!   extensions. The structural-metadata property table has one STRING column
//!   `iri`, row *i* = the IRI of feature *i*: this is the binding key that lets a
//!   Cesium pick resolve a pixel → featureId → `iri` → SPARQL.
//!
//! ## Coordinate frames
//!
//! 3D Tiles / glTF positions are **ECEF metres** (EPSG:4978, geocentric) and the
//! tile transform is **identity**. The GLB's mesh node is translated to a local
//! origin at the centre of the content, and the POSITION accessor holds f32
//! offsets from it: absolute ECEF in f32 would snap every vertex to a 0.25–0.5 m
//! grid at Dutch magnitudes. Stored geometry is reprojected to WGS84 `(lon, lat)` (RD New via
//! [`crate::geo::crs`]), each feature is GROUNDED (its lowest vertex shifted to
//! h = 0: stored heights are orthometric — NAP for 3DBAG, base at ~10 m around
//! Nijmegen — and would float every solid a storey above the bare-ellipsoid
//! terrain the viewer renders), and `(lon, lat, h)` is mapped to ECEF by the
//! standard WGS84 ellipsoidal formula [`wgs84_to_ecef`].
//!
//! ## Size, cost and caching
//!
//! Until tiling exists the whole dataset is one GLB, so the content is capped at
//! [`max_features`] features (first in IRI order). A capped tileset says so in
//! `asset.extras.truncated` and the GLB in an `X-Tiles3d-Truncated:
//! served/total` header. Both bodies are built off the async runtime
//! (`spawn_blocking`) and cached per (store, write generation, dataset, readable
//! graphs, cap), so repeat requests between writes cost a lookup. The routes are
//! mounted behind the per-IP rate limiter.
//!
//! ## Credits
//!
//! Both routes are public for public datasets, so licensed content must carry
//! its attribution with it. A dataset holding 3DBAG-derived geometry (CC BY 4.0)
//! gets the licensor's credit in the GLB's glTF `asset.copyright` — the field
//! Cesium and other clients display — and, because 3D Tiles 1.1 has no copyright
//! member, as structured `asset.extras.credits` in the tileset for the app's own
//! viewer to link. See [`dataset_credits`].
//!
//! TODO: implicit tiling (quadtree/octree subdivision) for large datasets, which
//! would lift the feature cap; Draco mesh compression; per-feature batching
//! beyond one GLB; true
//! orthometric→ellipsoidal height correction via a geoid + terrain model instead
//! of per-feature grounding.

pub mod glb;

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::{Extension, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::auth::middleware::AuthenticatedUser;
use crate::geo::crs::{transform_xy, Crs};
use crate::geo::datatypes::{extract_crs, extract_wkt};
use crate::server::AppState;

use glb::{encode_glb, GlbFeature};

/// WGS84 ellipsoid (GRS80/WGS84) — semi-major axis and flattening.
const WGS84_A: f64 = 6_378_137.0;
const WGS84_F: f64 = 1.0 / 298.257_223_563;

/// Convert geodetic WGS84 `(lon, lat in degrees, height in metres)` to ECEF
/// `(X, Y, Z)` metres (EPSG:4978), the geocentric frame 3D Tiles/glTF use.
///
/// `N = a / sqrt(1 - e² sin²φ)`,
/// `X = (N + h) cosφ cosλ`, `Y = (N + h) cosφ sinλ`, `Z = (N(1 - e²) + h) sinφ`.
pub fn wgs84_to_ecef(lon_deg: f64, lat_deg: f64, h: f64) -> [f64; 3] {
    let e2 = WGS84_F * (2.0 - WGS84_F); // first eccentricity squared
    let lon = lon_deg.to_radians();
    let lat = lat_deg.to_radians();
    let (sin_lat, cos_lat) = (lat.sin(), lat.cos());
    let n = WGS84_A / (1.0 - e2 * sin_lat * sin_lat).sqrt();
    let x = (n + h) * cos_lat * lon.cos();
    let y = (n + h) * cos_lat * lon.sin();
    let z = (n * (1.0 - e2) + h) * sin_lat;
    [x, y, z]
}

/// An attribution the served tiles must carry for licensed content.
#[derive(Debug, PartialEq)]
struct DataCredit {
    /// The credit line, worded exactly as the rights holder requires.
    text: &'static str,
    /// The rights holder's copyright page, which they ask digital media to link.
    url: &'static str,
    license: &'static str,
    license_url: &'static str,
}

/// 3DBAG, CC BY 4.0 (https://docs.3dbag.nl/en/copyright/): fixed credit
/// wording plus a link to that page; its 3D Tiles docs add "The Copyright
/// notice is required."
const THREEDBAG_CREDIT: DataCredit = DataCredit {
    text: "© 3DBAG by tudelft3d and 3DGI",
    url: "https://docs.3dbag.nl/en/copyright/",
    license: "CC BY 4.0",
    license_url: "https://creativecommons.org/licenses/by/4.0/",
};

impl DataCredit {
    /// Plain-text notice for glTF `asset.copyright`. The tiles are always an
    /// adaptation (triangulated, reprojected, grounded), so CC BY 4.0
    /// §3(a)(1)(B) wants "modified" said. No ';' — Cesium splits on it.
    fn notice(&self) -> String {
        format!(
            "{} ({}), {} ({}), modified",
            self.text, self.url, self.license, self.license_url
        )
    }
}

/// Credits owed for the dataset's content. 3DBAG-derived graphs are recognised
/// by their provenance: the seeded lift stamps `prov:wasDerivedFrom` on every
/// geometry node and `dct:source` on the building layer, both pointing into
/// 3dbag.nl — the same "3dbag" match the frontend applies to file links.
fn dataset_credits(
    store: &crate::store::TripleStore,
    data_graphs: &[String],
) -> Vec<&'static DataCredit> {
    let from: String = data_graphs.iter().map(|g| format!("FROM <{g}> ")).collect();
    let query = format!(
        "PREFIX prov: <http://www.w3.org/ns/prov#>\n\
         PREFIX dct: <http://purl.org/dc/terms/>\n\
         ASK {from}WHERE {{ ?s prov:wasDerivedFrom|dct:source ?src \
         FILTER(isIRI(?src) && CONTAINS(LCASE(STR(?src)), \"3dbag\")) }}"
    );
    match store.query(&query) {
        Ok(oxigraph::sparql::QueryResults::Boolean(true)) => vec![&THREEDBAG_CREDIT],
        _ => Vec::new(),
    }
}

/// The tileset `asset`. 3D Tiles 1.1 defines no copyright member, so credits
/// ride in `extras` (the app's Cesium viewer turns them into linked on-screen
/// credits); the GLB content carries the same notice in glTF's own field.
fn tileset_asset(credits: &[&DataCredit], truncated: Option<Truncation>) -> serde_json::Value {
    // gltfUpAxis Z: our GLB is ECEF (Z-up geocentric, EPSG:4978) — node
    // translation plus POSITION offsets — with an identity tile transform, so
    // Cesium must NOT apply the default glTF Y-up→Z-up rotation.
    let mut asset =
        serde_json::json!({ "version": "1.1", "tilesetVersion": "1.0", "gltfUpAxis": "Z" });
    if let Some(t) = truncated {
        asset["extras"]["truncated"] = serde_json::json!({
            "served": t.served,
            "total": t.total,
            "maxFeatures": t.cap,
        });
    }
    if !credits.is_empty() {
        let list: Vec<serde_json::Value> = credits
            .iter()
            .map(|c| {
                serde_json::json!({
                    "text": c.text,
                    "url": c.url,
                    "license": c.license,
                    "licenseUrl": c.license_url,
                    "modified": true,
                })
            })
            .collect();
        asset["extras"]["credits"] = serde_json::Value::Array(list);
    }
    asset
}

/// Set glTF `asset.copyright` ("a copyright message suitable for display to
/// credit the content creator", glTF 2.0) on an encoded GLB by rewriting its
/// JSON chunk; the BIN chunk is carried over byte for byte. Anything that is
/// not a well-formed GLB comes back unchanged.
fn with_gltf_copyright(glb: Vec<u8>, copyright: &str) -> Vec<u8> {
    const JSON_CHUNK: [u8; 4] = *b"JSON";
    if glb.len() < 20 || glb[16..20] != JSON_CHUNK {
        return glb;
    }
    let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
    let Some(json_bytes) = glb.get(20..20 + json_len) else {
        return glb;
    };
    let Ok(mut gltf) = serde_json::from_slice::<serde_json::Value>(json_bytes) else {
        return glb;
    };
    gltf["asset"]["copyright"] = copyright.into();
    let Ok(mut json) = serde_json::to_vec(&gltf) else {
        return glb;
    };
    // Chunks stay 4-byte aligned; the JSON chunk pads with spaces.
    json.resize(json.len().next_multiple_of(4), b' ');
    let rest = &glb[20 + json_len..];
    let total = 20 + json.len() + rest.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&glb[..8]); // magic + container version
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&JSON_CHUNK);
    out.extend_from_slice(&json);
    out.extend_from_slice(rest);
    out
}

/// The 3D Tiles routes (anonymous-capable for public datasets via the
/// `optional_auth` layer applied where this router is merged).
pub fn tiles3d_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/datasets/:dataset_id/3dtiles/tileset.json",
            get(tileset_json),
        )
        .route(
            "/api/datasets/:dataset_id/3dtiles/content.glb",
            get(content_glb),
        )
}

/// A reprojected feature ready for ECEF meshing: its IRI and the geographic
/// `(lon, lat, h)` vertices of its triangles (flat triples).
#[derive(Debug)]
struct GeoFeature {
    iri: String,
    /// Flat `[lon, lat, h, lon, lat, h, …]` triples, in WGS84 degrees + metres.
    tri_lonlath: Vec<f64>,
}

/// Geographic extent accumulator for the tileset bounding region (radians + m).
#[derive(Default)]
struct RegionAccum {
    west: f64,
    south: f64,
    east: f64,
    north: f64,
    min_h: f64,
    max_h: f64,
    any: bool,
}

impl RegionAccum {
    fn new() -> Self {
        RegionAccum {
            west: f64::INFINITY,
            south: f64::INFINITY,
            east: f64::NEG_INFINITY,
            north: f64::NEG_INFINITY,
            min_h: f64::INFINITY,
            max_h: f64::NEG_INFINITY,
            any: false,
        }
    }
    fn add(&mut self, lon_deg: f64, lat_deg: f64, h: f64) {
        self.any = true;
        self.west = self.west.min(lon_deg);
        self.east = self.east.max(lon_deg);
        self.south = self.south.min(lat_deg);
        self.north = self.north.max(lat_deg);
        self.min_h = self.min_h.min(h);
        self.max_h = self.max_h.max(h);
    }
    /// 3D Tiles `boundingVolume.region`: `[west, south, east, north, minH, maxH]`
    /// with the four angles in **radians**. Falls back to a small box if empty.
    fn region(&self) -> [f64; 6] {
        if !self.any {
            return [0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        }
        [
            self.west.to_radians(),
            self.south.to_radians(),
            self.east.to_radians(),
            self.north.to_radians(),
            self.min_h,
            self.max_h,
        ]
    }
}

/// Shared auth + graph resolution for both routes. Mirrors `routes::viewer_feed`:
/// anonymous access works for public datasets.
fn authorize(
    state: &AppState,
    user: &Option<Extension<AuthenticatedUser>>,
    dataset_id: &str,
) -> Result<Vec<String>, (StatusCode, String)> {
    let dataset = state
        .auth_db
        .get_dataset(dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }
    // Only the graphs this caller may READ: a viewer (or an anonymous caller on
    // a public dataset) must not get 3D-Tiles built from private graphs.
    state
        .auth_db
        .list_readable_dataset_graphs(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Run `f(iri, wkt_literal)` over every geometry-bearing feature's raw stored
/// WKT (keeping Z and the optional `<crs>` prefix), in feature-IRI order — the
/// order the feature cap keeps.
///
/// When `query_bbox` is given (a 3D AABB in the *stored* coordinate frame), the
/// engine's 3D R*-tree is asked first for the candidate geometry IRIs whose AABB
/// overlaps the box — the broad phase (spec §3.5). Only those geometries are then
/// visited (the narrow phase). When no bbox is given, or the 3D index is empty
/// (no volumetric data / `geometry3d` disabled), this falls back to the full
/// scan, so behaviour is unchanged for the existing whole-dataset routes.
fn for_each_geometry(
    store: &crate::store::TripleStore,
    data_graphs: &[String],
    query_bbox: Option<&Aabb3Frame>,
    mut f: impl FnMut(String, &str),
) {
    // ── Broad phase ──────────────────────────────────────────────────────────
    // Pre-filter to candidate geometry-node IRIs via the 3D index when a query
    // box is supplied and the index actually holds volumetric geometry. The set
    // is matched against `?g` (the `geo:hasGeometry` node carrying `geo:asWKT`).
    let candidate_filter = broad_phase_filter(store, query_bbox);

    let from: String = data_graphs
        .iter()
        .map(|g| format!("FROM <{g}> "))
        .collect::<Vec<_>>()
        .join("");
    // Raw geo:asWKT per feature — we need the unflattened Z, so we go to the store
    // directly rather than through the (2D-reprojected) viewer feed. `?g` is bound
    // so a VALUES-based broad-phase candidate set can constrain the geometry node,
    // and sorts a feature's geometries so a cap cuts in the same place every time.
    let query = format!(
        "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
         SELECT ?el ?wkt {from} \
         WHERE {{ ?el geo:hasGeometry ?g . ?g geo:asWKT ?wkt {candidate_filter} }} ORDER BY ?el ?g"
    );

    let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) else {
        return;
    };
    for sol in solutions.flatten() {
        let Some(el) = sol.get("el") else { continue };
        let iri = match el {
            oxigraph::model::Term::NamedNode(n) => n.as_str().to_string(),
            oxigraph::model::Term::BlankNode(b) => format!("_:{}", b.as_str()),
            other => other.to_string(),
        };
        let Some(oxigraph::model::Term::Literal(wkt)) = sol.get("wkt") else {
            continue;
        };
        f(iri, wkt.value());
    }
}

/// The features one GLB carries, and how many meshable features the dataset
/// has in all (`total > features.len()` when the cap cut it).
#[derive(Debug)]
struct Collected {
    features: Vec<GeoFeature>,
    total: usize,
}

/// Reproject and triangulate the first `cap` meshable features; past the cap,
/// only count the rest (a bounding-box check, no triangles kept).
fn collect_features(
    store: &crate::store::TripleStore,
    data_graphs: &[String],
    query_bbox: Option<&Aabb3Frame>,
    cap: usize,
) -> Collected {
    let mut features: Vec<GeoFeature> = Vec::new();
    let mut total = 0;
    for_each_geometry(store, data_graphs, query_bbox, |iri, wkt| {
        if features.len() < cap {
            if let Some(tri) = triangulate_feature(wkt) {
                features.push(GeoFeature {
                    iri,
                    tri_lonlath: tri,
                });
                total += 1;
            }
        } else if feature_extent(wkt).is_some() {
            total += 1;
        }
    });
    Collected { features, total }
}

/// The tileset's view of the same features: the bounding region of the first
/// `cap` meshable ones, built from per-feature bounding boxes instead of
/// triangles, plus the served/total counts.
struct Extent {
    region: RegionAccum,
    served: usize,
    total: usize,
}

fn collect_extent(store: &crate::store::TripleStore, data_graphs: &[String], cap: usize) -> Extent {
    let mut region = RegionAccum::new();
    let (mut served, mut total) = (0, 0);
    for_each_geometry(store, data_graphs, None, |_iri, wkt| {
        let Some([west, south, east, north, height]) = feature_extent(wkt) else {
            return;
        };
        total += 1;
        if served < cap {
            served += 1;
            region.add(west, south, 0.0);
            region.add(east, north, height);
        }
    });
    Extent {
        region,
        served,
        total,
    }
}

/// A query bounding box in the stored coordinate frame. Aliased to the engine's
/// [`crate::geo::geom3d::Aabb3`] when the `geometry3d` feature is on; otherwise a
/// stub (no 3D index exists to query, so the broad phase is a no-op).
#[cfg(feature = "geometry3d")]
type Aabb3Frame = crate::geo::geom3d::Aabb3;
#[cfg(not(feature = "geometry3d"))]
type Aabb3Frame = ();

/// Build a SPARQL graph-pattern fragment constraining `?g` to the broad-phase
/// candidate geometry nodes, or an empty string when no pre-filter applies.
///
/// Returns `""` (full scan) when: no query box is given; the `geometry3d` feature
/// is off; or the 3D index is empty. A `VALUES ?g { … }` block is emitted only
/// when the index returns a candidate set — keeping the whole-dataset routes on
/// their existing, unfiltered path.
#[cfg(feature = "geometry3d")]
fn broad_phase_filter(
    store: &crate::store::TripleStore,
    query_bbox: Option<&Aabb3Frame>,
) -> String {
    let Some(bbox) = query_bbox else {
        return String::new();
    };
    let index = store.spatial_index_3d();
    if index.is_empty() {
        return String::new(); // no volumetric data indexed → fall back to full scan
    }
    let candidates = index.query_intersecting(bbox);
    // Bind the candidate geometry nodes; an empty set yields `VALUES ?g {}` which
    // correctly selects nothing (the box overlaps no indexed 3D geometry).
    let values: String = candidates.iter().map(|iri| format!("<{iri}> ")).collect();
    format!("VALUES ?g {{ {values}}}")
}

/// No 3D index without the feature → always the full scan.
#[cfg(not(feature = "geometry3d"))]
fn broad_phase_filter(
    _store: &crate::store::TripleStore,
    _query_bbox: Option<&Aabb3Frame>,
) -> String {
    String::new()
}

/// Reproject a stored WKT literal to WGS84 `(lon, lat, h)` and triangulate it into
/// a flat `[lon, lat, h, …]` triple list. Returns `None` for an unsupported CRS
/// or an unmeshable geometry.
fn triangulate_feature(wkt_literal: &str) -> Option<Vec<f64>> {
    // Resolve the source CRS from the optional `<crs>` prefix (default WGS84/CRS84).
    let source = match extract_crs(wkt_literal) {
        Some(uri) => Crs::from_uri(uri)?, // explicit but unsupported → skip
        None => Crs::Wgs84,
    };
    let body = extract_wkt(wkt_literal);

    let tris = wkt_triangles(body);
    if tris.is_empty() {
        return None;
    }

    // Ground the feature: stored heights are orthometric (NAP for 3DBAG — a
    // building base sits at ~10 m around Nijmegen), but the viewer renders on
    // the bare WGS84 ellipsoid, so absolute heights float every solid a storey
    // above the ground. Shifting each feature so its lowest vertex touches
    // h = 0 keeps building shapes exact and looks right on ellipsoid terrain;
    // only the sub-metre ground slope BETWEEN features is flattened.
    let min_z = tris
        .iter()
        .flatten()
        .map(|&(_, _, z)| z)
        .fold(f64::INFINITY, f64::min);
    let min_z = if min_z.is_finite() { min_z } else { 0.0 };

    let mut out: Vec<f64> = Vec::with_capacity(tris.len() * 9);
    for [a, b, c] in &tris {
        for &(x, y, z) in &[*a, *b, *c] {
            // XY → WGS84 lon/lat; Z becomes height above the feature's base.
            let (lon, lat) = transform_xy(source, Crs::Wgs84, x, y).unwrap_or((x, y));
            if !lon.is_finite() || !lat.is_finite() {
                return None;
            }
            let h = z - min_z;
            out.push(lon);
            out.push(lat);
            out.push(h);
        }
    }
    Some(out)
}

/// Triangulate a WKT geometry body into local `(x, y, z)` triangle triples,
/// preserving the source XY axes (reprojection happens by the caller).
///
/// With the `geometry3d` feature this uses the full WKT-Z parser + fan
/// triangulation; without it (or when that yields nothing) it falls back to a
/// 2D-footprint triangulation that promotes the footprint to z=0, so a
/// `geometry3d`-less build still produces a (flat) tileset.
fn wkt_triangles(body: &str) -> Vec<[(f64, f64, f64); 3]> {
    #[cfg(feature = "geometry3d")]
    {
        if let Some(g) = crate::geo::geom3d::parse_wkt3d(body) {
            let tris = g.triangles();
            if !tris.is_empty() {
                return tris
                    .into_iter()
                    .map(|[a, b, c]| [(a.x, a.y, a.z), (b.x, b.y, b.z), (c.x, c.y, c.z)])
                    .collect();
            }
        }
    }
    // 2D fallback: fan-triangulate the first polygon ring at z=0.
    footprint_triangles(body)
}

/// Degrees added around a reprojected bounding box: the image of a projected
/// box is slightly curved, so its sampled corners and edge midpoints can fall a
/// hair inside the true extent. 1e-7° is about a centimetre.
const REPROJECTED_PAD_DEG: f64 = 1e-7;

/// A feature's grounded WGS84 bounding box, `[west, south, east, north, height]`
/// in degrees and metres, from the same vertices [`triangulate_feature`] meshes
/// but without building triangles. `None` exactly when that returns `None`
/// (unsupported CRS, nothing meshable, non-finite coordinates).
fn feature_extent(wkt_literal: &str) -> Option<[f64; 5]> {
    let source = match extract_crs(wkt_literal) {
        Some(uri) => Crs::from_uri(uri)?,
        None => Crs::Wgs84,
    };
    let [min, max] = surface_bounds(extract_wkt(wkt_literal))?;
    let (mut west, mut south) = (f64::INFINITY, f64::INFINITY);
    let (mut east, mut north) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mid = |a: f64, b: f64| a + (b - a) / 2.0;
    for x in [min[0], mid(min[0], max[0]), max[0]] {
        for y in [min[1], mid(min[1], max[1]), max[1]] {
            let (lon, lat) = transform_xy(source, Crs::Wgs84, x, y).unwrap_or((x, y));
            if !lon.is_finite() || !lat.is_finite() {
                return None;
            }
            west = west.min(lon);
            east = east.max(lon);
            south = south.min(lat);
            north = north.max(lat);
        }
    }
    if source != Crs::Wgs84 {
        west -= REPROJECTED_PAD_DEG;
        south -= REPROJECTED_PAD_DEG;
        east += REPROJECTED_PAD_DEG;
        north += REPROJECTED_PAD_DEG;
    }
    Some([west, south, east, north, max[2] - min[2]])
}

/// The `[min, max]` corners of the vertices [`wkt_triangles`] would mesh, in the
/// stored frame. Mirrors its choice: the 3D parser's faces first, else the 2D
/// footprint fallback.
fn surface_bounds(body: &str) -> Option<[[f64; 3]; 2]> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut add = |p: [f64; 3]| {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    };
    let mut any = false;
    #[cfg(feature = "geometry3d")]
    {
        if let Some(g) = crate::geo::geom3d::parse_wkt3d(body) {
            surface_coords(&g, &mut |c| {
                any = true;
                add([c.x, c.y, c.z]);
            });
        }
    }
    if !any {
        for &(x, y, z) in footprint_triangles(body).iter().flatten() {
            any = true;
            add([x, y, z]);
        }
    }
    any.then_some([min, max])
}

/// Visit the vertices `Geometry3D::triangles` fans: every triangle corner, and
/// each face's exterior ring once it has the three vertices a fan needs.
#[cfg(feature = "geometry3d")]
fn surface_coords(
    g: &crate::geo::geom3d::Geometry3D,
    f: &mut impl FnMut(crate::geo::geom3d::Coord3),
) {
    use crate::geo::geom3d::Geometry3D;
    let mut face = |ring: &[crate::geo::geom3d::Coord3]| {
        if ring.len() >= 3 {
            ring.iter().for_each(|c| f(*c));
        }
    };
    match g {
        Geometry3D::Triangle(t) => face(t),
        Geometry3D::Tin(ts) => ts.iter().for_each(|t| face(t)),
        Geometry3D::Polygon(p) => face(&p.exterior),
        Geometry3D::MultiPolygon(ps) | Geometry3D::PolyhedralSurface(ps) => {
            ps.iter().for_each(|p| face(&p.exterior))
        }
        Geometry3D::GeometryCollection(gs) => gs.iter().for_each(|g| surface_coords(g, f)),
        _ => {}
    }
}

/// Fan-triangulate a 2D `POLYGON`/`MULTIPOLYGON` footprint at z=0. A deliberately
/// tiny parser for the fallback path (the 3D parser owns the real work). Returns
/// empty for points/lines or unparseable input.
fn footprint_triangles(body: &str) -> Vec<[(f64, f64, f64); 3]> {
    let upper = body.trim_start().to_ascii_uppercase();
    if !upper.starts_with("POLYGON") && !upper.starts_with("MULTIPOLYGON") {
        return Vec::new();
    }
    // Extract the first parenthesised coordinate ring: the substring between the
    // first run of '(' and its matching ')' at the coordinate level.
    let mut rings: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut current: Vec<(f64, f64)> = Vec::new();
    let mut num = String::new();
    let mut coords: Vec<f64> = Vec::new();
    let flush_num = |num: &mut String, coords: &mut Vec<f64>| {
        if !num.is_empty() {
            if let Ok(v) = num.parse::<f64>() {
                coords.push(v);
            }
            num.clear();
        }
    };
    for ch in body.chars() {
        match ch {
            '0'..='9' | '.' | '-' | '+' | 'e' | 'E' => num.push(ch),
            ' ' | '\t' | '\n' | '\r' => {
                flush_num(&mut num, &mut coords);
                if coords.len() >= 2 {
                    current.push((coords[0], coords[1]));
                    coords.clear();
                }
            }
            ',' => {
                flush_num(&mut num, &mut coords);
                if coords.len() >= 2 {
                    current.push((coords[0], coords[1]));
                }
                coords.clear();
            }
            '(' => {
                current.clear();
                coords.clear();
                num.clear();
            }
            ')' => {
                flush_num(&mut num, &mut coords);
                if coords.len() >= 2 {
                    current.push((coords[0], coords[1]));
                }
                coords.clear();
                if current.len() >= 3 {
                    rings.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    for ring in &rings {
        // Drop a trailing closing-duplicate vertex.
        let n = if ring.len() > 3 && ring.first() == ring.last() {
            ring.len() - 1
        } else {
            ring.len()
        };
        for k in 1..n.saturating_sub(1) {
            out.push([
                (ring[0].0, ring[0].1, 0.0),
                (ring[k].0, ring[k].1, 0.0),
                (ring[k + 1].0, ring[k + 1].1, 0.0),
            ]);
        }
    }
    out
}

/// Default for [`max_features`]. One GLB of 10 000 LoD2 buildings is in the
/// order of 100–150 MB, about what a browser loads as a single tile.
pub const DEFAULT_MAX_FEATURES: usize = 10_000;

/// How many features one tileset carries: `TILES3D_MAX_FEATURES`, or
/// [`DEFAULT_MAX_FEATURES`] when unset or not a positive integer. The cap
/// stands in for tiling: past it the content is truncated (and says so), not
/// refused.
pub fn max_features() -> usize {
    std::env::var("TILES3D_MAX_FEATURES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_FEATURES)
}

/// A cut the cap made: `served` of `total` meshable features, cap `cap`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Truncation {
    served: usize,
    total: usize,
    cap: usize,
}

impl Truncation {
    fn of(served: usize, total: usize, cap: usize) -> Option<Self> {
        (total > served).then_some(Truncation { served, total, cap })
    }
}

/// Build the tileset JSON body. The region comes from bounding boxes; no
/// triangles are built.
fn build_tileset(
    store: &crate::store::TripleStore,
    dataset_id: &str,
    data_graphs: &[String],
    cap: usize,
) -> Built {
    // Whole-dataset tileset: no query bbox, so the broad phase is skipped and the
    // full scan runs (a future `?bbox=` param plugs straight in here).
    let extent = collect_extent(store, data_graphs, cap);
    let truncated = Truncation::of(extent.served, extent.total, cap);

    let region_arr = extent.region.region();
    // Geometric error: a coarse heuristic from the region's diagonal extent so the
    // single tile is shown at a reasonable distance. Single-tile means no LOD yet.
    let geometric_error = {
        let dx = (region_arr[2] - region_arr[0]).abs();
        let dy = (region_arr[3] - region_arr[1]).abs();
        let span_m = ((dx * dx + dy * dy).sqrt()) * WGS84_A; // angle (rad) → metres
        (span_m).max(1.0)
    };

    let content_uri = format!("/api/datasets/{dataset_id}/3dtiles/content.glb");
    let credits = dataset_credits(store, data_graphs);

    let tileset = serde_json::json!({
        "asset": tileset_asset(&credits, truncated),
        "geometricError": geometric_error,
        "root": {
            "boundingVolume": { "region": region_arr },
            "geometricError": geometric_error,
            "refine": "ADD",
            // Identity transform: the GLB node carries its own ECEF origin.
            "transform": [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0
            ],
            "content": { "uri": content_uri }
        }
    });

    Built {
        body: serde_json::to_vec(&tileset).unwrap_or_default().into(),
        truncated,
    }
}

/// Project each feature's geographic triangles into ECEF metres relative to a
/// local origin — the ECEF point under the centre of the features' lon/lat/h
/// box — and return the GLB features plus that origin (`None` when empty).
fn to_glb_features(features: &[GeoFeature]) -> (Vec<GlbFeature>, Option<[f64; 3]>) {
    if features.is_empty() {
        return (Vec::new(), None);
    }
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for f in features {
        for t in f.tri_lonlath.as_chunks::<3>().0 {
            for i in 0..3 {
                min[i] = min[i].min(t[i]);
                max[i] = max[i].max(t[i]);
            }
        }
    }
    let mid = |i: usize| min[i] + (max[i] - min[i]) / 2.0;
    let origin = wgs84_to_ecef(mid(0), mid(1), mid(2));
    let glb_features = features
        .iter()
        .map(|f| {
            let mut positions: Vec<f32> = Vec::with_capacity(f.tri_lonlath.len());
            for t in f.tri_lonlath.as_chunks::<3>().0 {
                let ecef = wgs84_to_ecef(t[0], t[1], t[2]);
                for i in 0..3 {
                    positions.push((ecef[i] - origin[i]) as f32);
                }
            }
            GlbFeature {
                iri: f.iri.clone(),
                positions,
                indices: Vec::new(), // non-indexed triangle soup
            }
        })
        .collect();
    (glb_features, Some(origin))
}

/// Build the GLB body: the first `cap` features, meshed around a local origin.
fn build_glb(store: &crate::store::TripleStore, data_graphs: &[String], cap: usize) -> Built {
    // Whole-dataset content: no query bbox → broad phase skipped, full scan runs.
    let collected = collect_features(store, data_graphs, None, cap);
    let truncated = Truncation::of(collected.features.len(), collected.total, cap);

    // The encoder assigns each feature its index as the GPU feature id and
    // stores its IRI in the property table's `iri` column — the binding key for
    // the Cesium pick round-trip.
    let (glb_features, origin) = to_glb_features(&collected.features);
    let mut bytes = encode_glb(&glb_features, origin);
    let credits = dataset_credits(store, data_graphs);
    if !credits.is_empty() {
        let notice: Vec<String> = credits.iter().map(|c| c.notice()).collect();
        // "; " separates credits: Cesium splits asset.copyright on ';'.
        bytes = with_gltf_copyright(bytes, &notice.join("; "));
    }
    Built {
        body: bytes.into(),
        truncated,
    }
}

/// A built response body and the cut the cap made, if any.
#[derive(Debug)]
struct Built {
    body: bytes::Bytes,
    truncated: Option<Truncation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Tileset,
    Glb,
}

/// What a built body depends on. The store's write generation moves on every
/// write, so a cached body never outlives the data it was built from; the
/// readable graphs carry the caller's read scope, so a viewer and an owner of
/// the same dataset never share an entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    store: u64,
    generation: u64,
    dataset: String,
    graphs: Vec<String>,
    cap: usize,
    kind: Kind,
}

impl CacheKey {
    /// Snapshot the generation BEFORE building, so a write landing mid-build
    /// leaves the result under the old generation instead of the new one.
    fn new(
        store: &crate::store::TripleStore,
        kind: Kind,
        dataset: &str,
        graphs: &[String],
        cap: usize,
    ) -> Self {
        let mut graphs = graphs.to_vec();
        graphs.sort();
        CacheKey {
            store: store.instance_id(),
            generation: store.write_generation(),
            dataset: dataset.to_string(),
            graphs,
            cap,
            kind,
        }
    }
}

/// Bytes of built bodies the cache may hold; a body larger than this is served
/// but not kept.
const CACHE_BUDGET_BYTES: usize = 256 * 1024 * 1024;
/// Entries the cache may hold, whatever their size.
const CACHE_MAX_ENTRIES: usize = 64;

struct TileCache {
    entries: lru::LruCache<CacheKey, Arc<Built>>,
    bytes: usize,
}

impl TileCache {
    fn get(&mut self, key: &CacheKey) -> Option<Arc<Built>> {
        self.entries.get(key).cloned()
    }

    fn put(&mut self, key: CacheKey, built: Arc<Built>) {
        let size = built.body.len();
        if size > CACHE_BUDGET_BYTES {
            return;
        }
        // A write moved this store on: everything built before it is dead.
        let stale: Vec<CacheKey> = self
            .entries
            .iter()
            .map(|(k, _)| k)
            .filter(|k| k.store == key.store && k.generation < key.generation)
            .cloned()
            .collect();
        for k in stale {
            if let Some(old) = self.entries.pop(&k) {
                self.bytes -= old.body.len();
            }
        }
        while self.bytes + size > CACHE_BUDGET_BYTES {
            match self.entries.pop_lru() {
                Some((_, old)) => self.bytes -= old.body.len(),
                None => break,
            }
        }
        if let Some((_, old)) = self.entries.push(key, built) {
            self.bytes -= old.body.len();
        }
        self.bytes += size;
    }
}

fn tile_cache() -> &'static Mutex<TileCache> {
    static CACHE: OnceLock<Mutex<TileCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(TileCache {
            entries: lru::LruCache::new(
                NonZeroUsize::new(CACHE_MAX_ENTRIES).expect("non-zero cache size"),
            ),
            bytes: 0,
        })
    })
}

/// The cached body for `key`, or build it on the blocking pool (the scan,
/// reprojection and meshing are CPU-bound and would otherwise hold an async
/// worker — and `/livez` with it) and cache it.
async fn cached_build(
    key: CacheKey,
    build: impl FnOnce() -> Built + Send + 'static,
) -> Result<Arc<Built>, (StatusCode, String)> {
    if let Some(hit) = tile_cache().lock().ok().and_then(|mut c| c.get(&key)) {
        return Ok(hit);
    }
    let built = Arc::new(
        tokio::task::spawn_blocking(build)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?,
    );
    if let Some(t) = built.truncated {
        tracing::warn!(
            dataset = %key.dataset,
            served = t.served,
            total = t.total,
            "3D Tiles {:?} capped at TILES3D_MAX_FEATURES={}",
            key.kind,
            t.cap
        );
    }
    if let Ok(mut c) = tile_cache().lock() {
        c.put(key, built.clone());
    }
    Ok(built)
}

/// GET /api/datasets/:dataset_id/3dtiles/tileset.json — 3D Tiles 1.1 tileset.
async fn tileset_json(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let data_graphs = authorize(&state, &user, &dataset_id)?;
    let cap = max_features();
    let key = CacheKey::new(&state.store, Kind::Tileset, &dataset_id, &data_graphs, cap);
    let store = state.store.clone();
    let built = cached_build(key, move || {
        build_tileset(&store, &dataset_id, &data_graphs, cap)
    })
    .await?;
    Ok((
        [(header::CONTENT_TYPE, "application/json")],
        built.body.clone(),
    )
        .into_response())
}

/// GET /api/datasets/:dataset_id/3dtiles/content.glb — binary glTF content.
async fn content_glb(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let data_graphs = authorize(&state, &user, &dataset_id)?;
    let cap = max_features();
    let key = CacheKey::new(&state.store, Kind::Glb, &dataset_id, &data_graphs, cap);
    let store = state.store.clone();
    let built = cached_build(key, move || build_glb(&store, &data_graphs, cap)).await?;

    let mut resp = axum::http::Response::builder()
        .status(StatusCode::OK)
        // Cesium fetches GLB tile content as model/gltf-binary.
        .header(header::CONTENT_TYPE, "model/gltf-binary")
        .header("vary", "Accept");
    if let Some(t) = built.truncated {
        resp = resp.header("x-tiles3d-truncated", format!("{}/{}", t.served, t.total));
    }
    resp.body(axum::body::Body::from(built.body.clone()))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    /// Every feature, uncapped.
    fn all_features(store: &TripleStore, query_bbox: Option<&Aabb3Frame>) -> Vec<GeoFeature> {
        collect_features(store, &[], query_bbox, usize::MAX).features
    }

    /// The region the old triangle pass accumulated: every meshed vertex.
    fn triangle_region(features: &[GeoFeature]) -> [f64; 6] {
        let mut r = RegionAccum::new();
        for f in features {
            for t in f.tri_lonlath.as_chunks::<3>().0 {
                r.add(t[0], t[1], t[2]);
            }
        }
        r.region()
    }

    /// The glTF JSON chunk of a GLB.
    fn gltf_json(glb: &[u8]) -> serde_json::Value {
        let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
        serde_json::from_slice(&glb[20..20 + json_len]).unwrap()
    }

    /// The f32 POSITION triples of a GLB.
    fn glb_positions(glb: &[u8]) -> Vec<[f32; 3]> {
        let gltf = gltf_json(glb);
        let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
        let bin = &glb[20 + json_len + 8..];
        let view = &gltf["bufferViews"][0];
        let off = view["byteOffset"].as_u64().unwrap() as usize;
        let len = view["byteLength"].as_u64().unwrap() as usize;
        bin[off..off + len]
            .chunks_exact(12)
            .map(|c| {
                let f = |i: usize| f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]);
                [f(0), f(4), f(8)]
            })
            .collect()
    }

    #[test]
    fn ecef_origin_and_poles() {
        // (0,0,0): on the equator at the prime meridian → X = a, Y = Z = 0.
        let p = wgs84_to_ecef(0.0, 0.0, 0.0);
        assert!((p[0] - WGS84_A).abs() < 1e-3, "X {}", p[0]);
        assert!(p[1].abs() < 1e-6 && p[2].abs() < 1e-6);

        // North pole: X = Y = 0, Z = b = a(1-f).
        let np = wgs84_to_ecef(0.0, 90.0, 0.0);
        let b = WGS84_A * (1.0 - WGS84_F);
        assert!(np[0].abs() < 1e-3 && np[1].abs() < 1e-3);
        assert!((np[2] - b).abs() < 1e-3, "Z {}", np[2]);

        // A point near Nijmegen should land ~6.36e6 m from the geocentre.
        let nij = wgs84_to_ecef(5.86, 51.85, 0.0);
        let r = (nij[0] * nij[0] + nij[1] * nij[1] + nij[2] * nij[2]).sqrt();
        assert!((r - 6.365e6).abs() < 5e3, "geocentric radius {r}");
    }

    #[test]
    fn footprint_triangulates_polygon() {
        // A unit square footprint → two triangles (4 distinct corners, closed ring).
        let tris = footprint_triangles("POLYGON((0 0, 1 0, 1 1, 0 1, 0 0))");
        assert_eq!(tris.len(), 2, "{tris:?}");
        // All at z = 0.
        for t in &tris {
            for v in t {
                assert_eq!(v.2, 0.0);
            }
        }
    }

    #[test]
    fn footprint_ignores_points() {
        assert!(footprint_triangles("POINT(4 52)").is_empty());
    }

    #[test]
    fn collect_features_reprojects_rd_polygon() {
        // An RD-prefixed footprint near Nijmegen should yield a meshable feature
        // whose region lands in NL lon/lat.
        let data = r#"
            @prefix geo: <http://www.opengis.net/ont/geosparql#> .
            @prefix ex:  <http://example.org/> .
            ex:b geo:hasGeometry [ geo:asWKT
                "<http://www.opengis.net/def/crs/EPSG/0/28992> POLYGON((187320 428330, 187610 428330, 187610 428690, 187320 428690, 187320 428330))"^^geo:wktLiteral ] .
        "#;
        let store = TripleStore::in_memory().unwrap();
        store.load_str(data, RdfFormat::Turtle, None).unwrap();
        let features = all_features(&store, None);
        assert_eq!(features.len(), 1, "one meshable feature: {features:?}");
        assert_eq!(features[0].iri, "http://example.org/b");
        assert!(!features[0].tri_lonlath.is_empty(), "has triangles");
        let region = collect_extent(&store, &[], usize::MAX).region;
        assert!(region.any, "region populated");
        // Region corners near Nijmegen (~5.86 lon, ~51.85 lat).
        assert!((region.west - 5.86).abs() < 0.1, "west {}", region.west);
        assert!((region.south - 51.85).abs() < 0.1, "south {}", region.south);
    }

    #[test]
    fn unsupported_crs_feature_is_skipped() {
        let data = r#"
            @prefix geo: <http://www.opengis.net/ont/geosparql#> .
            @prefix ex:  <http://example.org/> .
            ex:b geo:hasGeometry [ geo:asWKT
                "<http://www.opengis.net/def/crs/EPSG/0/25832> POLYGON((687000 5338000, 687010 5338000, 687010 5338010, 687000 5338000))"^^geo:wktLiteral ] .
        "#;
        let store = TripleStore::in_memory().unwrap();
        store.load_str(data, RdfFormat::Turtle, None).unwrap();
        let features = all_features(&store, None);
        assert!(
            features.is_empty(),
            "UTM metres must not be meshed: {features:?}"
        );
    }

    #[test]
    fn glb_for_dataset_carries_iris() {
        let data = r#"
            @prefix geo: <http://www.opengis.net/ont/geosparql#> .
            @prefix ex:  <http://example.org/> .
            ex:a geo:hasGeometry [ geo:asWKT "POLYGON((0 0, 0.001 0, 0.001 0.001, 0 0.001, 0 0))"^^geo:wktLiteral ] .
            ex:b geo:hasGeometry [ geo:asWKT "POLYGON((1 1, 1.001 1, 1.001 1.001, 1 1.001, 1 1))"^^geo:wktLiteral ] .
        "#;
        let store = TripleStore::in_memory().unwrap();
        store.load_str(data, RdfFormat::Turtle, None).unwrap();
        let features = all_features(&store, None);
        assert_eq!(features.len(), 2);

        let glb = build_glb(&store, &[], DEFAULT_MAX_FEATURES).body;
        // Valid container with the structural-metadata table.
        let magic = u32::from_le_bytes([glb[0], glb[1], glb[2], glb[3]]);
        assert_eq!(magic, 0x4654_6C67);
        let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
        let json_str = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        assert!(json_str.contains("EXT_structural_metadata"));
        // The IRI strings live in the property-table values buffer (binary chunk),
        // per the glTF EXT_structural_metadata STRING encoding — not the JSON.
        let blob = String::from_utf8_lossy(&glb);
        assert!(blob.contains("example.org/a") && blob.contains("example.org/b"));
    }

    #[test]
    fn credits_follow_3dbag_provenance_per_graph() {
        // The seeded lift's shape: 3DBAG geometry nodes in their own graph,
        // derived from the 3DBAG copyright page; another graph without it.
        let bag = r#"
            @prefix geo:  <http://www.opengis.net/ont/geosparql#> .
            @prefix prov: <http://www.w3.org/ns/prov#> .
            @prefix ex:   <http://example.org/> .
            ex:b geo:hasGeometry ex:g .
            ex:g prov:wasDerivedFrom <https://docs.3dbag.nl/en/copyright/> .
        "#;
        let other = r#"
            <http://example.org/h> <http://www.w3.org/ns/prov#wasDerivedFrom> <https://ex.org/src.city.json> .
        "#;
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(bag, RdfFormat::Turtle, Some("http://example.org/bag"))
            .unwrap();
        store
            .load_str(other, RdfFormat::Turtle, Some("http://example.org/other"))
            .unwrap();

        let with_bag = [
            "http://example.org/other".to_string(),
            "http://example.org/bag".to_string(),
        ];
        assert_eq!(dataset_credits(&store, &with_bag), vec![&THREEDBAG_CREDIT]);
        assert!(
            dataset_credits(&store, &["http://example.org/other".to_string()]).is_empty(),
            "a dataset without 3DBAG content owes no 3DBAG credit"
        );

        // The tileset carries the credit as structured extras; 3D Tiles' own
        // asset members are unchanged, and a credit-free asset has no extras.
        let asset = tileset_asset(&dataset_credits(&store, &with_bag), None);
        assert_eq!(asset["version"], "1.1");
        assert_eq!(asset["gltfUpAxis"], "Z");
        let c = &asset["extras"]["credits"][0];
        assert_eq!(c["text"], "© 3DBAG by tudelft3d and 3DGI");
        assert_eq!(c["url"], "https://docs.3dbag.nl/en/copyright/");
        assert_eq!(c["license"], "CC BY 4.0");
        assert_eq!(
            c["licenseUrl"],
            "https://creativecommons.org/licenses/by/4.0/"
        );
        assert_eq!(c["modified"], true);
        assert!(tileset_asset(&[], None).get("extras").is_none());
    }

    #[test]
    fn gltf_copyright_is_stamped_into_the_json_chunk() {
        let chunk_len = |b: &[u8]| u32::from_le_bytes([b[12], b[13], b[14], b[15]]) as usize;
        let glb = encode_glb(
            &[GlbFeature {
                iri: "http://example.org/a".to_string(),
                positions: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                indices: Vec::new(),
            }],
            None,
        );
        let notice = THREEDBAG_CREDIT.notice();
        assert!(
            !notice.contains(';'),
            "Cesium would split the credit on ';'"
        );

        let out = with_gltf_copyright(glb.clone(), &notice);
        let json_len = chunk_len(&out);
        assert_eq!(json_len % 4, 0, "JSON chunk stays 4-byte aligned");
        assert_eq!(
            u32::from_le_bytes([out[8], out[9], out[10], out[11]]) as usize,
            out.len(),
            "header length matches the rewritten container"
        );
        let gltf: serde_json::Value = serde_json::from_slice(&out[20..20 + json_len]).unwrap();
        assert_eq!(
            gltf["asset"]["copyright"],
            "© 3DBAG by tudelft3d and 3DGI (https://docs.3dbag.nl/en/copyright/), \
             CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/), modified"
        );
        assert_eq!(gltf["asset"]["version"], "2.0");
        assert!(gltf["extensions"]["EXT_structural_metadata"].is_object());
        assert_eq!(
            &out[20 + json_len..],
            &glb[20 + chunk_len(&glb)..],
            "the BIN chunk is carried over byte for byte"
        );

        // Not a GLB: returned untouched rather than mangled.
        assert_eq!(
            with_gltf_copyright(b"not a glb".to_vec(), &notice),
            b"not a glb"
        );
    }

    /// End-to-end over the REAL bundled 3DBAG sample (the one the seed lifts): a
    /// volumetric-only CityJSON conversion must mesh the whole block, and the GLB
    /// must carry one property-table row + a colour per feature. This pins the
    /// "empty tileset" regression — the demo's headline 3D-Tiles content — against
    /// real data, where the prior tests only covered 2-feature toy inputs.
    #[cfg(feature = "geometry3d")]
    #[test]
    fn tileset_from_bundled_3dbag_has_full_block() {
        use crate::imports::cityjson::{convert_cityjson, CityJsonOptions};
        let data = include_str!("../../frontend/public/samples/schependomlaan-3dbag.city.json");
        let doc: serde_json::Value = serde_json::from_str(data).unwrap();
        let opts = CityJsonOptions {
            inst_base: "http://localhost/dataset/viewer-3d-demo/".to_string(),
            // The seed's provenance (CITYJSON_ZONES), which the credit keys on.
            source_url: Some("https://docs.3dbag.nl/en/copyright/".to_string()),
            generated_at: None,
            volumetric_only: true, // drop the 78 flat LoD0 footprints, keep the solids
        };
        let (nt, stats) = convert_cityjson(&doc, &opts).unwrap();
        assert!(
            stats.geometries >= 70,
            "the LoD2.2 solids are converted: {} geometries",
            stats.geometries
        );

        let store = TripleStore::in_memory().unwrap();
        store.load_str(&nt, RdfFormat::NTriples, None).unwrap();
        let features = all_features(&store, None);
        let region = collect_extent(&store, &[], usize::MAX).region;
        assert!(
            features.len() >= 70,
            "the whole block meshes into the tileset, not ~4 boxes: {} features",
            features.len()
        );
        assert!(region.any, "bounding region populated");
        assert!(
            features
                .iter()
                .all(|f| !f.iri.is_empty() && !f.tri_lonlath.is_empty()),
            "every feature carries its IRI and triangles"
        );
        assert_eq!(
            dataset_credits(&store, &[]),
            vec![&THREEDBAG_CREDIT],
            "the lifted block owes the 3DBAG credit"
        );

        let glb = build_glb(&store, &[], DEFAULT_MAX_FEATURES).body;
        let magic = u32::from_le_bytes([glb[0], glb[1], glb[2], glb[3]]);
        assert_eq!(magic, 0x4654_6C67, "valid GLB container");
        let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
        let json_str = std::str::from_utf8(&glb[20..20 + json_len]).unwrap();
        assert!(json_str.contains("EXT_structural_metadata"));
        assert!(
            json_str.contains("COLOR_0"),
            "buildings carry per-feature colour"
        );
        let count = serde_json::from_str::<serde_json::Value>(json_str.trim_end()).unwrap()
            ["extensions"]["EXT_structural_metadata"]["propertyTables"][0]["count"]
            .as_u64()
            .unwrap() as usize;
        assert_eq!(count, features.len(), "one IRI row per meshed feature");
    }

    #[cfg(feature = "geometry3d")]
    #[test]
    fn broad_phase_bbox_prefilters_features() {
        use crate::geo::geom3d::Aabb3;

        // Two 3D buildings, far apart in XY (WGS84 degrees) and with vertical
        // extent so both enter the 3D index. The geometry nodes are *named* (the
        // index — like the 2D one — keys on a NamedNode geometry-node IRI).
        let data = r#"
            @prefix geo: <http://www.opengis.net/ont/geosparql#> .
            @prefix ex:  <http://example.org/> .
            ex:a  geo:hasGeometry ex:ga .
            ex:ga geo:asWKT "POLYHEDRALSURFACE Z (((0 0 0,0 0.001 0,0.001 0.001 0,0.001 0 0,0 0 0)),((0 0 0,0 0 5,0 0.001 5,0 0.001 0,0 0 0)),((0 0 5,0.001 0 5,0.001 0.001 5,0 0.001 5,0 0 5)))"^^geo:wktLiteral .
            ex:b  geo:hasGeometry ex:gb .
            ex:gb geo:asWKT "POLYHEDRALSURFACE Z (((10 10 0,10 10.001 0,10.001 10.001 0,10.001 10 0,10 10 0)),((10 10 0,10 10 5,10 10.001 5,10 10.001 0,10 10 0)),((10 10 5,10.001 10 5,10.001 10.001 5,10 10.001 5,10 10 5)))"^^geo:wktLiteral .
        "#;
        let store = TripleStore::in_memory().unwrap();
        store.load_str(data, RdfFormat::Turtle, None).unwrap();

        // No bbox → full scan returns both.
        let all = all_features(&store, None);
        assert_eq!(all.len(), 2, "full scan: {all:?}");

        // A box around building `a` only → the broad phase returns just `a`.
        let around_a = Aabb3 {
            min: [-0.5, -0.5, -1.0],
            max: [0.5, 0.5, 6.0],
        };
        let just_a = all_features(&store, Some(&around_a));
        assert_eq!(just_a.len(), 1, "broad phase narrowed to one: {just_a:?}");
        assert_eq!(just_a[0].iri, "http://example.org/a");

        // A box over empty space → the broad phase returns nothing.
        let empty_region = Aabb3 {
            min: [100.0, 100.0, 0.0],
            max: [101.0, 101.0, 1.0],
        };
        let none = all_features(&store, Some(&empty_region));
        assert!(none.is_empty(), "no candidates over empty space: {none:?}");
    }

    /// Three square buildings and one point, all WGS84 near Nijmegen.
    const FOUR_FEATURES: &str = r#"
        @prefix geo: <http://www.opengis.net/ont/geosparql#> .
        @prefix ex:  <http://example.org/> .
        ex:a geo:hasGeometry [ geo:asWKT "POLYGON((5.86 51.84, 5.861 51.84, 5.861 51.841, 5.86 51.841, 5.86 51.84))"^^geo:wktLiteral ] .
        ex:b geo:hasGeometry [ geo:asWKT "POLYGON((5.87 51.85, 5.871 51.85, 5.871 51.851, 5.87 51.851, 5.87 51.85))"^^geo:wktLiteral ] .
        ex:c geo:hasGeometry [ geo:asWKT "POLYGON((5.88 51.86, 5.881 51.86, 5.881 51.861, 5.88 51.861, 5.88 51.86))"^^geo:wktLiteral ] .
        ex:d geo:hasGeometry [ geo:asWKT "POINT(5.89 51.87)"^^geo:wktLiteral ] .
    "#;

    #[test]
    fn extent_region_equals_the_triangle_region_for_wgs84() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(FOUR_FEATURES, RdfFormat::Turtle, None)
            .unwrap();
        let features = all_features(&store, None);
        assert_eq!(features.len(), 3, "the point is not meshed");
        let extent = collect_extent(&store, &[], usize::MAX);
        assert_eq!((extent.served, extent.total), (3, 3));
        assert_eq!(
            extent.region.region(),
            triangle_region(&features),
            "bounding boxes give the same region as the triangles they bound"
        );
    }

    #[cfg(feature = "geometry3d")]
    #[test]
    fn extent_region_covers_reprojected_solids() {
        // The bundled 3DBAG block is RD New with real heights: the bbox region
        // must contain every meshed vertex, and stay close to it.
        use crate::imports::cityjson::{convert_cityjson, CityJsonOptions};
        let data = include_str!("../../frontend/public/samples/schependomlaan-3dbag.city.json");
        let doc: serde_json::Value = serde_json::from_str(data).unwrap();
        let opts = CityJsonOptions {
            inst_base: "http://localhost/dataset/viewer-3d-demo/".to_string(),
            source_url: None,
            generated_at: None,
            volumetric_only: true,
        };
        let (nt, _) = convert_cityjson(&doc, &opts).unwrap();
        let store = TripleStore::in_memory().unwrap();
        store.load_str(&nt, RdfFormat::NTriples, None).unwrap();

        let tri = triangle_region(&all_features(&store, None));
        let bbox = collect_extent(&store, &[], usize::MAX).region.region();
        // The RD grid is rotated against the meridians, so a building's
        // reprojected box is a little wider than the building: tens of
        // centimetres here. Looser than the vertices is still a valid volume.
        let slack = 1e-5_f64.to_radians(); // ~1 m
        assert!(
            bbox[0] <= tri[0] && bbox[1] <= tri[1],
            "{bbox:?} vs {tri:?}"
        );
        assert!(
            bbox[2] >= tri[2] && bbox[3] >= tri[3],
            "{bbox:?} vs {tri:?}"
        );
        for i in 0..4 {
            assert!(
                (bbox[i] - tri[i]).abs() < slack,
                "side {i}: {bbox:?} vs {tri:?}"
            );
        }
        assert_eq!(bbox[4], tri[4], "grounded: lowest point at h = 0");
        assert_eq!(bbox[5], tri[5], "tallest building sets the top");
    }

    #[test]
    fn cap_truncates_in_iri_order_and_says_so() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(FOUR_FEATURES, RdfFormat::Turtle, None)
            .unwrap();

        let capped = collect_features(&store, &[], None, 2);
        let iris: Vec<&str> = capped.features.iter().map(|f| f.iri.as_str()).collect();
        assert_eq!(iris, ["http://example.org/a", "http://example.org/b"]);
        assert_eq!(capped.total, 3, "the point never counts as content");

        let extent = collect_extent(&store, &[], 2);
        assert_eq!((extent.served, extent.total), (2, 3));
        assert_eq!(
            extent.region.region(),
            triangle_region(&capped.features),
            "the region bounds what the GLB carries"
        );

        let tileset: serde_json::Value =
            serde_json::from_slice(&build_tileset(&store, "ds", &[], 2).body).unwrap();
        assert_eq!(
            tileset["asset"]["extras"]["truncated"],
            serde_json::json!({ "served": 2, "total": 3, "maxFeatures": 2 })
        );
        let glb = build_glb(&store, &[], 2);
        assert_eq!(
            glb.truncated,
            Some(Truncation {
                served: 2,
                total: 3,
                cap: 2
            })
        );
        assert_eq!(
            gltf_json(&glb.body)["extensions"]["EXT_structural_metadata"]["propertyTables"][0]
                ["count"],
            2
        );

        // Under the cap nothing is flagged.
        assert!(build_glb(&store, &[], 3).truncated.is_none());
        let full: serde_json::Value =
            serde_json::from_slice(&build_tileset(&store, "ds", &[], 3).body).unwrap();
        assert!(full["asset"].get("extras").is_none(), "{full}");
    }

    #[test]
    fn glb_positions_keep_millimetres_at_ecef_magnitudes() {
        // A 1.23 m × 0.37 m outbuilding near Nijmegen: at |ECEF| ≈ 6.4e6 m an f32
        // steps in 0.5 m, so absolute coordinates would flatten it to a sliver.
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                r#"
                @prefix geo: <http://www.opengis.net/ont/geosparql#> .
                <http://example.org/shed> geo:hasGeometry [ geo:asWKT
                    "<http://www.opengis.net/def/crs/EPSG/0/28992> POLYGON((187320 428330, 187321.23 428330, 187321.23 428330.37, 187320 428330.37, 187320 428330))"^^geo:wktLiteral ] .
                "#,
                RdfFormat::Turtle,
                None,
            )
            .unwrap();
        let features = all_features(&store, None);
        let glb = build_glb(&store, &[], DEFAULT_MAX_FEATURES).body;
        let origin: Vec<f64> = gltf_json(&glb)["nodes"][0]["translation"]
            .as_array()
            .expect("the mesh node carries the local origin")
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();

        let exact: Vec<[f64; 3]> = features[0]
            .tri_lonlath
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| wgs84_to_ecef(t[0], t[1], t[2]))
            .collect();
        let served = glb_positions(&glb);
        assert_eq!(served.len(), exact.len());
        let (mut worst, mut worst_absolute) = (0.0_f64, 0.0_f64);
        for (p, e) in served.iter().zip(&exact) {
            for i in 0..3 {
                worst = worst.max((origin[i] + p[i] as f64 - e[i]).abs());
                worst_absolute = worst_absolute.max(((e[i] as f32) as f64 - e[i]).abs());
            }
        }
        assert!(worst < 1e-3, "served vertices are {worst} m off");
        assert!(
            worst_absolute > 0.05,
            "absolute f32 ECEF really is coarse here ({worst_absolute} m), so this test bites"
        );
    }

    #[test]
    fn cache_drops_older_generations_and_keeps_scopes_apart() {
        let key = |generation: u64, graphs: &[&str]| CacheKey {
            store: 1,
            generation,
            dataset: "ds".to_string(),
            graphs: graphs.iter().map(|g| g.to_string()).collect(),
            cap: 10,
            kind: Kind::Glb,
        };
        let built = |n: usize| {
            Arc::new(Built {
                body: vec![0u8; n].into(),
                truncated: None,
            })
        };
        let mut cache = TileCache {
            entries: lru::LruCache::new(NonZeroUsize::new(4).unwrap()),
            bytes: 0,
        };
        cache.put(key(1, &["g:pub"]), built(10));
        cache.put(key(1, &["g:pub", "g:priv"]), built(20));
        assert_eq!(cache.get(&key(1, &["g:pub"])).unwrap().body.len(), 10);
        assert_eq!(
            cache.get(&key(1, &["g:pub", "g:priv"])).unwrap().body.len(),
            20,
            "a wider read scope is its own entry"
        );
        assert!(cache.get(&key(2, &["g:pub"])).is_none(), "a write misses");

        cache.put(key(2, &["g:pub"]), built(5));
        assert!(
            cache.get(&key(1, &["g:pub", "g:priv"])).is_none(),
            "entries from before the write are dropped"
        );
        assert_eq!(cache.bytes, 5, "byte accounting follows the evictions");

        // Over the entry limit the least recently used goes.
        for n in 0..5 {
            cache.put(key(2, &[&format!("g:{n}")]), built(1));
        }
        assert_eq!(cache.entries.len(), 4);
        assert_eq!(cache.bytes, 4);
    }
}
