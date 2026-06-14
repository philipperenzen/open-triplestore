//! GeoSPARQL namespace constants and IRI definitions.
//!
//! Implements vocabularies from:
//! - OGC GeoSPARQL 1.1 (22-047r1)
//! - Simple Features (OGC 06-103r4)
//! - Egenhofer relations
//! - RCC8 relations

// ─── Datatypes ───

/// WKT literal datatype IRI
pub const WKT_LITERAL: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";

/// GML literal datatype IRI
pub const GML_LITERAL: &str = "http://www.opengis.net/ont/geosparql#gmlLiteral";

// ─── 3D / volumetric datatypes (additive; spec §3.3) ───
// Canonical datatype IRIs the platform defines and advertises; emitted by the
// CityJSON converter (which inlines the same string) and consumed by external
// clients, so they are allow(dead_code) for internal-usage analysis.

/// A CityJSON geometry object embedded as a JSON literal (loss-free 3D BAG).
#[allow(dead_code)]
pub const CITYJSON_LITERAL: &str = "https://open-triplestore.org/def/cityjsonGeometryLiteral";
/// A base64 glTF/GLB fragment or a URI to one (render-ready).
#[allow(dead_code)]
pub const GLTF_LITERAL: &str = "https://open-triplestore.org/def/gltfGeometryLiteral";

// ─── ots-geof: 3D function IRIs (additive; spec §3.4) ───
// Namespace https://open-triplestore.org/def/function/geo3d/ — never collides
// with geof:, so GeoSPARQL 1.1 stays conformant.

pub const OTS3D_DISTANCE3D: &str = "https://open-triplestore.org/def/function/geo3d/distance3d";
pub const OTS3D_VOLUME: &str = "https://open-triplestore.org/def/function/geo3d/volume";
pub const OTS3D_AREA3D: &str = "https://open-triplestore.org/def/function/geo3d/area3d";
pub const OTS3D_ZMIN: &str = "https://open-triplestore.org/def/function/geo3d/zMin";
pub const OTS3D_ZMAX: &str = "https://open-triplestore.org/def/function/geo3d/zMax";
pub const OTS3D_HEIGHT: &str = "https://open-triplestore.org/def/function/geo3d/height";
pub const OTS3D_BBOX3D: &str = "https://open-triplestore.org/def/function/geo3d/boundingBox3d";
pub const OTS3D_CENTROID3D: &str = "https://open-triplestore.org/def/function/geo3d/centroid3d";
pub const OTS3D_FOOTPRINT2D: &str = "https://open-triplestore.org/def/function/geo3d/footprint2d";
pub const OTS3D_EXTRUDE: &str = "https://open-triplestore.org/def/function/geo3d/extrude";
pub const OTS3D_SF_INTERSECTS: &str =
    "https://open-triplestore.org/def/function/geo3d/sf3dIntersects";
pub const OTS3D_SF_DISJOINT: &str = "https://open-triplestore.org/def/function/geo3d/sf3dDisjoint";

// ─── Simple Features topological function IRIs ───

pub const SF_CONTAINS: &str = "http://www.opengis.net/def/function/geosparql/sfContains";
pub const SF_CROSSES: &str = "http://www.opengis.net/def/function/geosparql/sfCrosses";
pub const SF_DISJOINT: &str = "http://www.opengis.net/def/function/geosparql/sfDisjoint";
pub const SF_EQUALS: &str = "http://www.opengis.net/def/function/geosparql/sfEquals";
pub const SF_INTERSECTS: &str = "http://www.opengis.net/def/function/geosparql/sfIntersects";
pub const SF_OVERLAPS: &str = "http://www.opengis.net/def/function/geosparql/sfOverlaps";
pub const SF_TOUCHES: &str = "http://www.opengis.net/def/function/geosparql/sfTouches";
pub const SF_WITHIN: &str = "http://www.opengis.net/def/function/geosparql/sfWithin";

// ─── Egenhofer topological function IRIs ───

pub const EH_CONTAINS: &str = "http://www.opengis.net/def/function/geosparql/ehContains";
pub const EH_COVERED_BY: &str = "http://www.opengis.net/def/function/geosparql/ehCoveredBy";
pub const EH_COVERS: &str = "http://www.opengis.net/def/function/geosparql/ehCovers";
pub const EH_DISJOINT: &str = "http://www.opengis.net/def/function/geosparql/ehDisjoint";
pub const EH_EQUALS: &str = "http://www.opengis.net/def/function/geosparql/ehEquals";
pub const EH_INSIDE: &str = "http://www.opengis.net/def/function/geosparql/ehInside";
pub const EH_MEET: &str = "http://www.opengis.net/def/function/geosparql/ehMeet";
pub const EH_OVERLAP: &str = "http://www.opengis.net/def/function/geosparql/ehOverlap";

// ─── RCC8 topological function IRIs ───

pub const RCC8_DC: &str = "http://www.opengis.net/def/function/geosparql/rcc8dc";
pub const RCC8_EC: &str = "http://www.opengis.net/def/function/geosparql/rcc8ec";
pub const RCC8_PO: &str = "http://www.opengis.net/def/function/geosparql/rcc8po";
pub const RCC8_TPPI: &str = "http://www.opengis.net/def/function/geosparql/rcc8tppi";
pub const RCC8_TPP: &str = "http://www.opengis.net/def/function/geosparql/rcc8tpp";
pub const RCC8_NTPP: &str = "http://www.opengis.net/def/function/geosparql/rcc8ntpp";
pub const RCC8_NTPPI: &str = "http://www.opengis.net/def/function/geosparql/rcc8ntppi";
pub const RCC8_EQ: &str = "http://www.opengis.net/def/function/geosparql/rcc8eq";

// ─── Non-topological (constructive) function IRIs ───

pub const BOUNDARY: &str = "http://www.opengis.net/def/function/geosparql/boundary";
pub const BUFFER: &str = "http://www.opengis.net/def/function/geosparql/buffer";
pub const CONVEX_HULL: &str = "http://www.opengis.net/def/function/geosparql/convexHull";
pub const DIFFERENCE: &str = "http://www.opengis.net/def/function/geosparql/difference";
pub const ENVELOPE: &str = "http://www.opengis.net/def/function/geosparql/envelope";
pub const INTERSECTION: &str = "http://www.opengis.net/def/function/geosparql/intersection";
pub const SYM_DIFFERENCE: &str = "http://www.opengis.net/def/function/geosparql/symDifference";
pub const UNION: &str = "http://www.opengis.net/def/function/geosparql/union";

// ─── Scalar function IRIs ───

pub const DISTANCE: &str = "http://www.opengis.net/def/function/geosparql/distance";
pub const GET_SRID: &str = "http://www.opengis.net/def/function/geosparql/getSRID";
pub const AREA: &str = "http://www.opengis.net/def/function/geosparql/area";
pub const RELATE: &str = "http://www.opengis.net/def/function/geosparql/relate";
pub const TRANSFORM: &str = "http://www.opengis.net/def/function/geosparql/transform";

// ─── Spatial Measure Units ───

pub const METRE: &str = "http://www.opengis.net/def/uom/OGC/1.0/metre";
pub const DEGREE: &str = "http://www.opengis.net/def/uom/OGC/1.0/degree";
pub const RADIAN: &str = "http://www.opengis.net/def/uom/OGC/1.0/radian";
pub const UNITY: &str = "http://www.opengis.net/def/uom/OGC/1.0/unity";
pub const KILOMETRE: &str = "http://www.opengis.net/def/uom/OGC/1.0/kilometre";
pub const CENTIMETRE: &str = "http://www.opengis.net/def/uom/OGC/1.0/centimetre";
pub const MILLIMETRE: &str = "http://www.opengis.net/def/uom/OGC/1.0/millimetre";

// ─── Default CRS ───

pub const CRS84: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";
