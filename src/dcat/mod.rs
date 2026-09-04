//! DCAT 3 / DCAT-AP / DCAT-AP-NL catalogue generation with VoID statistics and PROV-O provenance.

pub mod catalog;
pub mod vocabulary;

pub use catalog::generate_catalog_bytes;
// Turtle conveniences kept for library consumers and the conformance tests;
// the HTTP handlers serialise per negotiated format via `generate_catalog_bytes`.
#[allow(unused_imports)]
pub use catalog::{generate_dcat_catalog, generate_org_dcat_catalog, CatalogOptions, Profile};
