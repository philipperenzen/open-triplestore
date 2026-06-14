//! 3D Tiles 1.1 generation (spec §6) — turn stored geometry into a streamable
//! tileset whose features carry their IRI.
//!
//! Two routes per dataset:
//!
//! * `GET /api/datasets/:id/3dtiles/tileset.json` — a 3D Tiles **1.1** tileset
//!   (`asset.version "1.1"`) with a single root tile (`refine: "ADD"`) whose
//!   `boundingVolume.region` is computed from the dataset's features and whose
//!   `content.uri` points at the GLB below. Single-tile for v1: no implicit
//!   tiling yet (see TODOs).
//! * `GET /api/datasets/:id/3dtiles/content.glb` — a binary glTF (GLB) holding the
//!   dataset's building meshes with the **EXT_mesh_features** + **EXT_structural_metadata**
//!   extensions. The structural-metadata property table has one STRING column
//!   `iri`, row *i* = the IRI of feature *i*: this is the binding key that lets a
//!   Cesium pick resolve a pixel → featureId → `iri` → SPARQL.
//!
//! ## Coordinate frames
//!
//! 3D Tiles / glTF positions are emitted directly in **ECEF metres** (EPSG:4978,
//! geocentric) and the tile transform is left **identity** — the simplest correct
//! choice. Stored geometry is reprojected to WGS84 `(lon, lat)` (RD New via
//! [`crate::geo::crs`]), the stored Z is treated as ellipsoidal height (an
//! approximation: 3D BAG heights are NAP/orthometric, off by the geoid undulation
//! — a few tens of metres in NL, acceptable for visualisation), and
//! `(lon, lat, h)` is mapped to ECEF by the standard WGS84 ellipsoidal formula
//! [`wgs84_to_ecef`].
//!
//! TODO: implicit tiling (quadtree/octree subdivision) for large datasets; Draco
//! mesh compression; per-feature batching beyond one GLB; orthometric→ellipsoidal
//! height correction via a geoid model.

pub mod glb;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};

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
    state
        .auth_db
        .list_dataset_graphs(dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Collect every geometry-bearing feature's raw stored WKT (keeping Z and the
/// optional `<crs>` prefix), reproject XY to WGS84 and triangulate, returning the
/// geographic features plus the accumulated bounding region.
fn collect_features(
    store: &crate::store::TripleStore,
    data_graphs: &[String],
) -> (Vec<GeoFeature>, RegionAccum) {
    let from: String = data_graphs
        .iter()
        .map(|g| format!("FROM <{g}> "))
        .collect::<Vec<_>>()
        .join("");
    // Raw geo:asWKT per feature — we need the unflattened Z, so we go to the store
    // directly rather than through the (2D-reprojected) viewer feed.
    let query = format!(
        "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
         SELECT ?el ?wkt {from} \
         WHERE {{ ?el geo:hasGeometry/geo:asWKT ?wkt }} ORDER BY ?el"
    );

    let mut region = RegionAccum::new();
    let mut features: Vec<GeoFeature> = Vec::new();

    let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) else {
        return (features, region);
    };
    for sol in solutions.flatten() {
        let Some(el) = sol.get("el") else { continue };
        let iri = match el {
            oxigraph::model::Term::NamedNode(n) => n.as_str().to_string(),
            oxigraph::model::Term::BlankNode(b) => format!("_:{}", b.as_str()),
            other => other.to_string(),
        };
        let Some(wkt_term) = sol.get("wkt") else { continue };
        let wkt_literal = match wkt_term {
            oxigraph::model::Term::Literal(l) => l.value().to_string(),
            _ => continue,
        };
        if let Some(tri) = triangulate_feature(&wkt_literal, &mut region) {
            if !tri.is_empty() {
                features.push(GeoFeature { iri, tri_lonlath: tri });
            }
        }
    }
    (features, region)
}

/// Reproject a stored WKT literal to WGS84 `(lon, lat, h)` and triangulate it into
/// a flat `[lon, lat, h, …]` triple list, feeding the bounding-region accumulator.
/// Returns `None` for an unsupported CRS or an unmeshable geometry.
fn triangulate_feature(wkt_literal: &str, region: &mut RegionAccum) -> Option<Vec<f64>> {
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

    let mut out: Vec<f64> = Vec::with_capacity(tris.len() * 9);
    for [a, b, c] in &tris {
        for &(x, y, z) in &[*a, *b, *c] {
            // XY → WGS84 lon/lat; the stored Z passes through as height (metres).
            let (lon, lat) = transform_xy(source, Crs::Wgs84, x, y).unwrap_or((x, y));
            if !lon.is_finite() || !lat.is_finite() {
                return None;
            }
            region.add(lon, lat, z);
            out.push(lon);
            out.push(lat);
            out.push(z);
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
                    .map(|[a, b, c]| {
                        [
                            (a.x, a.y, a.z),
                            (b.x, b.y, b.z),
                            (c.x, c.y, c.z),
                        ]
                    })
                    .collect();
            }
        }
    }
    // 2D fallback: fan-triangulate the first polygon ring at z=0.
    footprint_triangles(body)
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

/// GET /api/datasets/:dataset_id/3dtiles/tileset.json — 3D Tiles 1.1 tileset.
async fn tileset_json(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let data_graphs = authorize(&state, &user, &dataset_id)?;
    let (_features, region) = collect_features(&state.store, &data_graphs);

    let region_arr = region.region();
    // Geometric error: a coarse heuristic from the region's diagonal extent so the
    // single tile is shown at a reasonable distance. Single-tile means no LOD yet.
    let geometric_error = {
        let dx = (region_arr[2] - region_arr[0]).abs();
        let dy = (region_arr[3] - region_arr[1]).abs();
        let span_m = ((dx * dx + dy * dy).sqrt()) * WGS84_A; // angle (rad) → metres
        (span_m).max(1.0)
    };

    let content_uri = format!("/api/datasets/{dataset_id}/3dtiles/content.glb");

    let tileset = serde_json::json!({
        // gltfUpAxis Z: our GLB POSITION accessors are already absolute ECEF
        // (Z-up geocentric, EPSG:4978) with an identity tile transform, so Cesium
        // must NOT apply the default glTF Y-up→Z-up rotation.
        "asset": { "version": "1.1", "tilesetVersion": "1.0", "gltfUpAxis": "Z" },
        "geometricError": geometric_error,
        "root": {
            "boundingVolume": { "region": region_arr },
            "geometricError": geometric_error,
            "refine": "ADD",
            // Identity transform: GLB positions are already in ECEF metres.
            "transform": [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0
            ],
            "content": { "uri": content_uri }
        }
    });

    Ok(Json(tileset))
}

/// GET /api/datasets/:dataset_id/3dtiles/content.glb — binary glTF content.
async fn content_glb(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let data_graphs = authorize(&state, &user, &dataset_id)?;
    let (features, _region) = collect_features(&state.store, &data_graphs);

    // Project each feature's geographic triangles into ECEF metres and hand them to
    // the GLB encoder. The encoder assigns each feature its index as the GPU
    // feature id and stores its IRI in the property table's `iri` column — the
    // binding key for the Cesium pick round-trip.
    let glb_features: Vec<GlbFeature> = features
        .iter()
        .map(|f| {
            let mut positions: Vec<f32> = Vec::with_capacity(f.tri_lonlath.len());
            for triple in f.tri_lonlath.chunks_exact(3) {
                let ecef = wgs84_to_ecef(triple[0], triple[1], triple[2]);
                positions.push(ecef[0] as f32);
                positions.push(ecef[1] as f32);
                positions.push(ecef[2] as f32);
            }
            GlbFeature {
                iri: f.iri.clone(),
                positions,
                indices: Vec::new(), // non-indexed triangle soup
            }
        })
        .collect();

    let bytes = encode_glb(&glb_features);

    axum::http::Response::builder()
        .status(StatusCode::OK)
        // Cesium fetches GLB tile content as model/gltf-binary.
        .header(axum::http::header::CONTENT_TYPE, "model/gltf-binary")
        .header("vary", "Accept")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

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
        let (features, region) = collect_features(&store, &[]);
        assert_eq!(features.len(), 1, "one meshable feature: {features:?}");
        assert_eq!(features[0].iri, "http://example.org/b");
        assert!(!features[0].tri_lonlath.is_empty(), "has triangles");
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
        let (features, _region) = collect_features(&store, &[]);
        assert!(features.is_empty(), "UTM metres must not be meshed: {features:?}");
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
        let (features, _region) = collect_features(&store, &[]);
        assert_eq!(features.len(), 2);

        let glb_features: Vec<GlbFeature> = features
            .iter()
            .map(|f| {
                let mut positions: Vec<f32> = Vec::new();
                for triple in f.tri_lonlath.chunks_exact(3) {
                    let ecef = wgs84_to_ecef(triple[0], triple[1], triple[2]);
                    positions.push(ecef[0] as f32);
                    positions.push(ecef[1] as f32);
                    positions.push(ecef[2] as f32);
                }
                GlbFeature { iri: f.iri.clone(), positions, indices: Vec::new() }
            })
            .collect();
        let glb = encode_glb(&glb_features);
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
}
