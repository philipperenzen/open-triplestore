//! DCAT 3 / DCAT-AP / DCAT-AP-NL catalogue generation with VoID statistics and PROV-O provenance.

pub mod authority;
pub mod catalog;
pub mod graph;
pub mod vocabulary;

pub use catalog::generate_catalog_bytes;
// Library surface; the binary's handlers only call `generate_catalog_bytes`.
#[allow(unused_imports)]
pub use catalog::{
    build_catalog, build_catalog_report, generate_dcat_catalog, generate_org_dcat_catalog,
    CatalogOptions, CatalogReport, Profile,
};
