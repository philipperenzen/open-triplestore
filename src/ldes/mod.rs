//! Linked Data Event Streams: publish any dataset as a TREE-fragmented stream
//! of immutable version objects, and sync a remote stream into a dataset.
//!
//! * [`store`]   — the SQLite side: per-dataset stream config, the append-only
//!   member log, and the client's sync bookmarks.
//! * [`capture`] — change capture at write time: an entity index of each
//!   tracked graph before and after a write yields one member per changed
//!   entity (or a tombstone), without any per-triple bookkeeping.
//! * [`publish`] — the HTTP surface: the `ldes:EventStream` and its nodes.
//! * [`client`]  — the sync client: follow `tree:view` / `tree:relation`,
//!   keep the newest version per entity, materialise into a graph.
//!
//! Domain-neutral by construction: members are whatever IRI-subject entities a
//! graph holds. See docs/ldes.md.

pub mod capture;
pub mod client;
pub mod publish;
pub mod store;

pub const LDES: &str = "https://w3id.org/ldes#";
pub const TREE: &str = "https://w3id.org/tree#";
pub const DCT: &str = "http://purl.org/dc/terms/";
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
pub const OTS: &str = "https://opentriplestore.org/ns#";
/// The type of a member that records an entity's disappearance.
pub const TOMBSTONE: &str = "https://opentriplestore.org/ns#Tombstone";

/// `{base}/api/datasets/{id}/ldes` — the stream, node and member IRIs are
/// the URLs that serve them, so a client can follow every link it is given.
pub fn stream_iri(base_url: &str, dataset_id: &str) -> String {
    format!(
        "{}/api/datasets/{dataset_id}/ldes",
        base_url.trim_end_matches('/')
    )
}
pub fn node_iri(base_url: &str, dataset_id: &str, page: u64) -> String {
    format!("{}/nodes/{page}", stream_iri(base_url, dataset_id))
}
pub fn member_iri(base_url: &str, dataset_id: &str, member_id: i64) -> String {
    format!("{}/members/{member_id}", stream_iri(base_url, dataset_id))
}
