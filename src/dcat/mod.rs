//! DCAT 3 / DCAT-AP / DCAT-AP-NL catalogue generation with VoID statistics and PROV-O provenance.

pub mod catalog;
pub mod vocabulary;

pub use catalog::generate_catalog_bytes;
// Library surface; the binary's handlers only call `generate_catalog_bytes`.
#[allow(unused_imports)]
pub use catalog::{CatalogOptions, Profile};
