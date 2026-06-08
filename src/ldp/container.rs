//! LDP Container management — `ldp:contains` CRUD, ETag, member listing,
//! Direct/Indirect Containers, and Non-RDF Source binary storage.
#![allow(dead_code)]

use crate::store::TripleStore;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use sha2::{Digest, Sha256};

// ─── LDP vocabulary IRIs ──────────────────────────────────────────────────────

pub const LDP_CONTAINS: &str = "http://www.w3.org/ns/ldp#contains";
pub const LDP_RESOURCE: &str = "http://www.w3.org/ns/ldp#Resource";
pub const LDP_RDF_SOURCE: &str = "http://www.w3.org/ns/ldp#RDFSource";
pub const LDP_BASIC_CONTAINER: &str = "http://www.w3.org/ns/ldp#BasicContainer";
pub const LDP_DIRECT_CONTAINER: &str = "http://www.w3.org/ns/ldp#DirectContainer";
pub const LDP_INDIRECT_CONTAINER: &str = "http://www.w3.org/ns/ldp#IndirectContainer";
pub const LDP_NON_RDF_SOURCE: &str = "http://www.w3.org/ns/ldp#NonRDFSource";
pub const LDP_MEMBERSHIP_RESOURCE: &str = "http://www.w3.org/ns/ldp#membershipResource";
pub const LDP_HAS_MEMBER_RELATION: &str = "http://www.w3.org/ns/ldp#hasMemberRelation";
pub const LDP_INSERTED_CONTENT_REL: &str = "http://www.w3.org/ns/ldp#insertedContentRelation";
pub const LDP_MEMBER: &str = "http://www.w3.org/ns/ldp#member";
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

// Internal IRIs for Non-RDF Source binary storage
const BINARY_CONTENT_IRI: &str = "urn:ldp:binaryContent";
const CONTENT_TYPE_IRI: &str = "urn:ldp:contentType";
const XSD_BASE64_BINARY: &str = "http://www.w3.org/2001/XMLSchema#base64Binary";

// ─── ContainerType ────────────────────────────────────────────────────────────

/// The LDP type of a resource.
#[derive(Debug, Clone, PartialEq)]
pub enum ContainerType {
    /// `ldp:DirectContainer`
    Direct,
    /// `ldp:IndirectContainer`
    Indirect,
    /// `ldp:BasicContainer`
    Basic,
    /// `ldp:NonRDFSource`
    NonRdfSource,
    /// `ldp:RDFSource` (non-container)
    RdfSource,
    /// No LDP type found
    Unknown,
}

/// Determine the most-specific LDP type for a resource IRI.
pub fn get_container_type(store: &TripleStore, iri: &str) -> ContainerType {
    let check = |type_iri: &str| -> bool {
        let q = format!("ASK {{ <{iri}> <{RDF_TYPE}> <{type_iri}> }}");
        matches!(
            store.query(&q),
            Ok(oxigraph::sparql::QueryResults::Boolean(true))
        )
    };

    if check(LDP_DIRECT_CONTAINER) {
        ContainerType::Direct
    } else if check(LDP_INDIRECT_CONTAINER) {
        ContainerType::Indirect
    } else if check(LDP_BASIC_CONTAINER) {
        ContainerType::Basic
    } else if check(LDP_NON_RDF_SOURCE) {
        ContainerType::NonRdfSource
    } else if check(LDP_RDF_SOURCE) {
        ContainerType::RdfSource
    } else {
        ContainerType::Unknown
    }
}

// ─── Membership info ──────────────────────────────────────────────────────────

/// Membership triple metadata for Direct and Indirect Containers.
#[derive(Debug, Clone)]
pub struct MembershipInfo {
    /// IRI of the resource that holds the membership triples.
    pub membership_resource: String,
    /// Predicate used in membership triples.
    pub has_member_relation: String,
    /// For Indirect Containers: the property on the new member whose value
    /// becomes the membership triple object.
    pub inserted_content_relation: Option<String>,
}

/// Retrieve membership triple metadata for a Direct or Indirect Container.
/// Returns `None` for Basic Containers and plain RDF Sources.
pub fn get_membership_info(
    store: &TripleStore,
    container_iri: &str,
) -> Result<Option<MembershipInfo>, String> {
    let q = format!(
        "SELECT ?mr ?hmr ?icr WHERE {{ \
           <{container_iri}> <{LDP_MEMBERSHIP_RESOURCE}> ?mr . \
           <{container_iri}> <{LDP_HAS_MEMBER_RELATION}> ?hmr . \
           OPTIONAL {{ <{container_iri}> <{LDP_INSERTED_CONTENT_REL}> ?icr }} \
         }}"
    );
    match store.query(&q).map_err(|e| e.to_string())? {
        oxigraph::sparql::QueryResults::Solutions(sols) => {
            for sol in sols.flatten() {
                let mr = match sol.get("mr") {
                    Some(oxigraph::model::Term::NamedNode(n)) => n.as_str().to_string(),
                    _ => continue,
                };
                let hmr = match sol.get("hmr") {
                    Some(oxigraph::model::Term::NamedNode(n)) => n.as_str().to_string(),
                    _ => continue,
                };
                let icr = match sol.get("icr") {
                    Some(oxigraph::model::Term::NamedNode(n)) => Some(n.as_str().to_string()),
                    _ => None,
                };
                return Ok(Some(MembershipInfo {
                    membership_resource: mr,
                    has_member_relation: hmr,
                    inserted_content_relation: icr,
                }));
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

// ─── ETag ──────────────────────────────────────────────────────────────────────

/// Compute a short ETag from the serialized content.
pub fn compute_etag(content: &[u8]) -> String {
    let hash = Sha256::digest(content);
    format!("\"{:.16x}\"", hash)
}

// ─── Container creation ────────────────────────────────────────────────────────

/// Ensure the container IRI is typed as an LDP Basic Container.
pub fn ensure_container(store: &TripleStore, container_iri: &str) -> Result<(), String> {
    let q = format!(
        "INSERT DATA {{ \
           <{container_iri}> <{RDF_TYPE}> <{LDP_BASIC_CONTAINER}> . \
           <{container_iri}> <{RDF_TYPE}> <{LDP_RDF_SOURCE}> . \
           <{container_iri}> <{RDF_TYPE}> <{LDP_RESOURCE}> . \
         }}"
    );
    store.update(&q).map_err(|e| e.to_string())
}

/// Ensure the IRI is typed as an LDP Direct Container with membership triples.
pub fn ensure_direct_container(
    store: &TripleStore,
    container_iri: &str,
    membership_resource: &str,
    has_member_relation: &str,
    inserted_content_relation: Option<&str>,
) -> Result<(), String> {
    let icr_triple = inserted_content_relation
        .map(|icr| format!("<{container_iri}> <{LDP_INSERTED_CONTENT_REL}> <{icr}> . "))
        .unwrap_or_default();

    let q = format!(
        "INSERT DATA {{ \
           <{container_iri}> <{RDF_TYPE}> <{LDP_DIRECT_CONTAINER}> . \
           <{container_iri}> <{RDF_TYPE}> <{LDP_RDF_SOURCE}> . \
           <{container_iri}> <{RDF_TYPE}> <{LDP_RESOURCE}> . \
           <{container_iri}> <{LDP_MEMBERSHIP_RESOURCE}> <{membership_resource}> . \
           <{container_iri}> <{LDP_HAS_MEMBER_RELATION}> <{has_member_relation}> . \
           {icr_triple}\
         }}"
    );
    store.update(&q).map_err(|e| e.to_string())
}

/// Ensure the IRI is typed as an LDP Indirect Container with membership triples.
pub fn ensure_indirect_container(
    store: &TripleStore,
    container_iri: &str,
    membership_resource: &str,
    has_member_relation: &str,
    inserted_content_relation: &str,
) -> Result<(), String> {
    let q = format!(
        "INSERT DATA {{ \
           <{container_iri}> <{RDF_TYPE}> <{LDP_INDIRECT_CONTAINER}> . \
           <{container_iri}> <{RDF_TYPE}> <{LDP_RDF_SOURCE}> . \
           <{container_iri}> <{RDF_TYPE}> <{LDP_RESOURCE}> . \
           <{container_iri}> <{LDP_MEMBERSHIP_RESOURCE}> <{membership_resource}> . \
           <{container_iri}> <{LDP_HAS_MEMBER_RELATION}> <{has_member_relation}> . \
           <{container_iri}> <{LDP_INSERTED_CONTENT_REL}> <{inserted_content_relation}> . \
         }}"
    );
    store.update(&q).map_err(|e| e.to_string())
}

// ─── Containment (ldp:contains) ───────────────────────────────────────────────

/// Add an `ldp:contains` triple from the container to the new member.
pub fn add_member(
    store: &TripleStore,
    container_iri: &str,
    member_iri: &str,
) -> Result<(), String> {
    let q = format!("INSERT DATA {{ <{container_iri}> <{LDP_CONTAINS}> <{member_iri}> }}");
    store.update(&q).map_err(|e| e.to_string())
}

/// Remove the `ldp:contains` triple pointing to the deleted member, plus all
/// triples that have the member as subject (its description graph).
pub fn remove_member(
    store: &TripleStore,
    container_iri: &str,
    member_iri: &str,
) -> Result<(), String> {
    let q1 = format!("DELETE DATA {{ <{container_iri}> <{LDP_CONTAINS}> <{member_iri}> }}");
    store.update(&q1).map_err(|e| e.to_string())?;

    let q2 = format!("DELETE WHERE {{ <{member_iri}> ?p ?o }}");
    store.update(&q2).map_err(|e| e.to_string())?;

    Ok(())
}

/// List all direct `ldp:contains` members of a container (sorted, paginated).
pub fn list_members(
    store: &TripleStore,
    container_iri: &str,
    offset: usize,
    limit: usize,
) -> Result<Vec<String>, String> {
    let q = format!(
        "SELECT ?member WHERE {{ <{container_iri}> <{LDP_CONTAINS}> ?member }} \
         ORDER BY ?member OFFSET {offset} LIMIT {limit}"
    );
    let mut members = Vec::new();
    if let oxigraph::sparql::QueryResults::Solutions(sols) =
        store.query(&q).map_err(|e| e.to_string())?
    {
        for sol in sols.flatten() {
            if let Some(oxigraph::model::Term::NamedNode(nn)) = sol.get("member") {
                members.push(nn.as_str().to_string());
            }
        }
    }
    Ok(members)
}

/// Count all direct `ldp:contains` members of a container.
pub fn count_members(store: &TripleStore, container_iri: &str) -> Result<usize, String> {
    let q = format!(
        "SELECT (COUNT(?member) AS ?n) WHERE {{ <{container_iri}> <{LDP_CONTAINS}> ?member }}"
    );
    if let oxigraph::sparql::QueryResults::Solutions(sols) =
        store.query(&q).map_err(|e| e.to_string())?
    {
        for sol in sols.flatten() {
            if let Some(oxigraph::model::Term::Literal(lit)) = sol.get("n") {
                if let Ok(n) = lit.value().parse::<usize>() {
                    return Ok(n);
                }
            }
        }
    }
    Ok(0)
}

// ─── Direct / Indirect Container membership triples ───────────────────────────

/// Add a membership triple `(membershipResource, hasMemberRelation, memberIri)`
/// for Direct or Indirect Containers.
pub fn add_direct_membership_triple(
    store: &TripleStore,
    membership_resource: &str,
    has_member_relation: &str,
    member_iri: &str,
) -> Result<(), String> {
    let q =
        format!("INSERT DATA {{ <{membership_resource}> <{has_member_relation}> <{member_iri}> }}");
    store.update(&q).map_err(|e| e.to_string())
}

/// Remove a membership triple when a member is deleted from a Direct or Indirect Container.
pub fn remove_direct_membership_triple(
    store: &TripleStore,
    membership_resource: &str,
    has_member_relation: &str,
    member_iri: &str,
) -> Result<(), String> {
    let q =
        format!("DELETE DATA {{ <{membership_resource}> <{has_member_relation}> <{member_iri}> }}");
    store.update(&q).map_err(|e| e.to_string())
}

/// List all membership triples `(subject, predicate, object)` from a Direct or
/// Indirect Container (via its `ldp:membershipResource` and `ldp:hasMemberRelation`).
pub fn list_membership_triples(
    store: &TripleStore,
    container_iri: &str,
) -> Result<Vec<(String, String, String)>, String> {
    let info = get_membership_info(store, container_iri)?;
    let info = match info {
        Some(i) => i,
        None => return Ok(vec![]),
    };

    let mr = &info.membership_resource;
    let hmr = &info.has_member_relation;

    let q = format!("SELECT ?obj WHERE {{ <{mr}> <{hmr}> ?obj }}");
    let mut triples = Vec::new();
    if let oxigraph::sparql::QueryResults::Solutions(sols) =
        store.query(&q).map_err(|e| e.to_string())?
    {
        for sol in sols.flatten() {
            if let Some(oxigraph::model::Term::NamedNode(nn)) = sol.get("obj") {
                triples.push((mr.clone(), hmr.clone(), nn.as_str().to_string()));
            }
        }
    }
    Ok(triples)
}

// ─── Non-RDF Source (binary) ──────────────────────────────────────────────────

/// Store a Non-RDF Source as base64-encoded content in the triple store.
///
/// Writes three triples:
/// - `<iri> <urn:ldp:binaryContent> "base64"^^xsd:base64Binary`
/// - `<iri> <urn:ldp:contentType> "mime-type-string"`
/// - `<iri> rdf:type ldp:NonRDFSource, ldp:Resource`
pub fn store_binary_resource(
    store: &TripleStore,
    iri: &str,
    content_type: &str,
    data: &[u8],
) -> Result<(), String> {
    let encoded = BASE64.encode(data);
    // Escape the content_type string for SPARQL
    let ct_escaped = content_type.replace('\\', "\\\\").replace('"', "\\\"");
    let q = format!(
        "INSERT DATA {{ \
           <{iri}> <{BINARY_CONTENT_IRI}> \"{encoded}\"^^<{XSD_BASE64_BINARY}> . \
           <{iri}> <{CONTENT_TYPE_IRI}> \"{ct_escaped}\" . \
           <{iri}> <{RDF_TYPE}> <{LDP_NON_RDF_SOURCE}> . \
           <{iri}> <{RDF_TYPE}> <{LDP_RESOURCE}> . \
         }}"
    );
    store.update(&q).map_err(|e| e.to_string())
}

/// Retrieve a Non-RDF Source. Returns `(content_type, raw_bytes)` or `None`.
pub fn get_binary_resource(
    store: &TripleStore,
    iri: &str,
) -> Result<Option<(String, Vec<u8>)>, String> {
    let q = format!(
        "SELECT ?data ?ct WHERE {{ \
           <{iri}> <{BINARY_CONTENT_IRI}> ?data . \
           <{iri}> <{CONTENT_TYPE_IRI}> ?ct . \
         }}"
    );
    match store.query(&q).map_err(|e| e.to_string())? {
        oxigraph::sparql::QueryResults::Solutions(sols) => {
            for sol in sols.flatten() {
                let encoded = match sol.get("data") {
                    Some(oxigraph::model::Term::Literal(lit)) => lit.value().to_string(),
                    _ => continue,
                };
                let ct = match sol.get("ct") {
                    Some(oxigraph::model::Term::Literal(lit)) => lit.value().to_string(),
                    _ => continue,
                };
                let bytes = BASE64
                    .decode(encoded.as_bytes())
                    .map_err(|e| e.to_string())?;
                return Ok(Some((ct, bytes)));
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// Check whether an IRI is a Non-RDF Source.
pub fn is_non_rdf_source(store: &TripleStore, iri: &str) -> bool {
    let q = format!("ASK {{ <{iri}> <{RDF_TYPE}> <{LDP_NON_RDF_SOURCE}> }}");
    matches!(
        store.query(&q),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    )
}

// ─── Resource existence and description ───────────────────────────────────────

/// Check whether an IRI exists as an LDP Resource in the store.
pub fn resource_exists(store: &TripleStore, iri: &str) -> bool {
    // A resource exists if it has outgoing triples OR is referenced by the container via ldp:contains.
    let q = format!("ASK {{ {{ <{iri}> ?p ?o }} UNION {{ ?s ?p <{iri}> }} }}");
    match store.query(&q) {
        Ok(oxigraph::sparql::QueryResults::Boolean(b)) => b,
        _ => false,
    }
}

/// Dump all triples about `iri` as N-Triples bytes.
pub fn describe_resource(store: &TripleStore, iri: &str) -> Result<Vec<u8>, String> {
    let q = format!("CONSTRUCT {{ <{iri}> ?p ?o }} WHERE {{ <{iri}> ?p ?o }}");
    match store.query(&q).map_err(|e| e.to_string())? {
        oxigraph::sparql::QueryResults::Graph(triples) => {
            let mut buf = Vec::new();
            for t in triples.flatten() {
                buf.extend_from_slice(
                    format!("{} {} {} .\n", t.subject, t.predicate, t.object).as_bytes(),
                );
            }
            Ok(buf)
        }
        _ => Ok(Vec::new()),
    }
}

/// Load Turtle data about a resource into the default graph.
///
/// Relative IRIs resolve against `iri` so an idiomatic `<>` subject (LDP's way of
/// referring to the resource being written) attaches to the resource itself.
pub fn load_resource_turtle(store: &TripleStore, iri: &str, turtle: &str) -> Result<(), String> {
    store
        .load_str_with_base(turtle, oxigraph::io::RdfFormat::Turtle, iri, None)
        .map_err(|e| e.to_string())
}

/// Load JSON-LD data about a resource into the default graph.
///
/// Relative IRIs resolve against `iri` (see [`load_resource_turtle`]).
pub fn load_resource_jsonld(store: &TripleStore, iri: &str, jsonld: &str) -> Result<(), String> {
    store
        .load_str_with_base(
            jsonld,
            oxigraph::io::RdfFormat::JsonLd {
                profile: Default::default(),
            },
            iri,
            None,
        )
        .map_err(|e| e.to_string())
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;

    fn fresh() -> TripleStore {
        TripleStore::in_memory().unwrap()
    }

    // ── ETag ─────────────────────────────────────────────────────────────────

    #[test]
    fn test_etag_deterministic() {
        let e1 = compute_etag(b"hello");
        let e2 = compute_etag(b"hello");
        assert_eq!(e1, e2);
        assert!(e1.starts_with('"'));
        assert!(e1.ends_with('"'));
    }

    #[test]
    fn test_etag_different_for_different_content() {
        assert_ne!(compute_etag(b"a"), compute_etag(b"b"));
    }

    // ── Basic container ───────────────────────────────────────────────────────

    #[test]
    fn test_container_lifecycle() {
        let store = fresh();
        ensure_container(&store, "http://ex.org/c1/").unwrap();
        add_member(&store, "http://ex.org/c1/", "http://ex.org/c1/item1").unwrap();
        add_member(&store, "http://ex.org/c1/", "http://ex.org/c1/item2").unwrap();
        let members = list_members(&store, "http://ex.org/c1/", 0, 100).unwrap();
        assert_eq!(members.len(), 2);
        remove_member(&store, "http://ex.org/c1/", "http://ex.org/c1/item1").unwrap();
        let members = list_members(&store, "http://ex.org/c1/", 0, 100).unwrap();
        assert_eq!(members.len(), 1);
    }

    #[test]
    fn test_get_container_type_basic() {
        let store = fresh();
        ensure_container(&store, "http://ex.org/bc/").unwrap();
        assert_eq!(
            get_container_type(&store, "http://ex.org/bc/"),
            ContainerType::Basic
        );
    }

    #[test]
    fn test_count_members_empty() {
        let store = fresh();
        ensure_container(&store, "http://ex.org/empty/").unwrap();
        assert_eq!(count_members(&store, "http://ex.org/empty/").unwrap(), 0);
    }

    #[test]
    fn test_list_members_pagination() {
        let store = fresh();
        ensure_container(&store, "http://ex.org/page/").unwrap();
        for i in 1..=5 {
            add_member(
                &store,
                "http://ex.org/page/",
                &format!("http://ex.org/page/item{i}"),
            )
            .unwrap();
        }
        let page0 = list_members(&store, "http://ex.org/page/", 0, 2).unwrap();
        assert_eq!(page0.len(), 2);
        let page1 = list_members(&store, "http://ex.org/page/", 2, 2).unwrap();
        assert_eq!(page1.len(), 2);
        let page2 = list_members(&store, "http://ex.org/page/", 4, 2).unwrap();
        assert_eq!(page2.len(), 1);
    }

    // ── Direct container ──────────────────────────────────────────────────────

    #[test]
    fn test_get_container_type_direct() {
        let store = fresh();
        ensure_direct_container(
            &store,
            "http://ex.org/dc/",
            "http://ex.org/collection",
            "http://www.w3.org/ns/ldp#member",
            None,
        )
        .unwrap();
        assert_eq!(
            get_container_type(&store, "http://ex.org/dc/"),
            ContainerType::Direct
        );
    }

    #[test]
    fn test_get_membership_info_direct() {
        let store = fresh();
        ensure_direct_container(
            &store,
            "http://ex.org/dc/",
            "http://ex.org/res",
            "http://ex.org/hasMember",
            None,
        )
        .unwrap();
        let info = get_membership_info(&store, "http://ex.org/dc/")
            .unwrap()
            .unwrap();
        assert_eq!(info.membership_resource, "http://ex.org/res");
        assert_eq!(info.has_member_relation, "http://ex.org/hasMember");
        assert!(info.inserted_content_relation.is_none());
    }

    #[test]
    fn test_get_membership_info_basic_returns_none() {
        let store = fresh();
        ensure_container(&store, "http://ex.org/bc/").unwrap();
        let info = get_membership_info(&store, "http://ex.org/bc/").unwrap();
        assert!(info.is_none());
    }

    #[test]
    fn test_add_remove_direct_membership_triple() {
        let store = fresh();
        add_direct_membership_triple(
            &store,
            "http://ex.org/res",
            "http://ex.org/hasMember",
            "http://ex.org/item1",
        )
        .unwrap();
        let q = "ASK { <http://ex.org/res> <http://ex.org/hasMember> <http://ex.org/item1> }";
        assert!(matches!(
            store.query(q).unwrap(),
            oxigraph::sparql::QueryResults::Boolean(true)
        ));
        remove_direct_membership_triple(
            &store,
            "http://ex.org/res",
            "http://ex.org/hasMember",
            "http://ex.org/item1",
        )
        .unwrap();
        assert!(matches!(
            store.query(q).unwrap(),
            oxigraph::sparql::QueryResults::Boolean(false)
        ));
    }

    // ── Indirect container ────────────────────────────────────────────────────

    #[test]
    fn test_get_container_type_indirect() {
        let store = fresh();
        ensure_indirect_container(
            &store,
            "http://ex.org/ic/",
            "http://ex.org/res",
            "http://www.w3.org/ns/ldp#member",
            "http://ex.org/via",
        )
        .unwrap();
        assert_eq!(
            get_container_type(&store, "http://ex.org/ic/"),
            ContainerType::Indirect
        );
    }

    // ── Non-RDF Source ────────────────────────────────────────────────────────

    #[test]
    fn test_get_container_type_non_rdf() {
        let store = fresh();
        store_binary_resource(&store, "http://ex.org/img", "image/png", b"\x89PNG").unwrap();
        assert_eq!(
            get_container_type(&store, "http://ex.org/img"),
            ContainerType::NonRdfSource
        );
    }

    #[test]
    fn test_store_binary_resource_round_trip() {
        let store = fresh();
        let data = b"\x89PNG\r\nHello binary world";
        store_binary_resource(&store, "http://ex.org/img", "image/png", data).unwrap();
        let (ct, retrieved) = get_binary_resource(&store, "http://ex.org/img")
            .unwrap()
            .unwrap();
        assert_eq!(ct, "image/png");
        assert_eq!(retrieved, data);
    }

    #[test]
    fn test_binary_content_type_preserved() {
        let store = fresh();
        store_binary_resource(&store, "http://ex.org/vid", "video/mp4", b"fakedata").unwrap();
        let (ct, _) = get_binary_resource(&store, "http://ex.org/vid")
            .unwrap()
            .unwrap();
        assert_eq!(ct, "video/mp4");
    }

    #[test]
    fn test_get_binary_resource_missing_returns_none() {
        let store = fresh();
        let result = get_binary_resource(&store, "http://ex.org/missing").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_is_non_rdf_source() {
        let store = fresh();
        assert!(!is_non_rdf_source(&store, "http://ex.org/x"));
        store_binary_resource(&store, "http://ex.org/x", "text/plain", b"hi").unwrap();
        assert!(is_non_rdf_source(&store, "http://ex.org/x"));
    }
}
