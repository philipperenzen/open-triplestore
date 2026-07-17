pub mod crs;
pub mod datatypes;
pub mod functions;
pub mod gml;
pub mod spatial_index;
pub mod viewer_feed;
pub mod vocabulary;

// Additive 3D / volumetric layer (spec §3) — namespaced under `ots-geof:` so the
// GeoSPARQL 1.1 surface stays byte-for-byte conformant. Gated by the
// `geometry3d` feature (enabled in `full`).
#[cfg(feature = "geometry3d")]
pub mod functions3d;
#[cfg(feature = "geometry3d")]
pub mod geom3d;
#[cfg(feature = "geometry3d")]
pub mod index3d;
