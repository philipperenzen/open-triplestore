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
