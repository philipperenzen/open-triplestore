//! DCAT 2 catalog generation with VoID statistics and PROV-O provenance.

pub mod catalog;
pub mod vocabulary;

pub use catalog::generate_dcat_catalog;
pub use catalog::generate_org_dcat_catalog;
