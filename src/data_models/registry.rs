//! data-model and version metadata.
//!
//! All metadata is stored as RDF triples in the named graph
//! `<urn:system:data-model-registry>` inside Oxigraph.

use super::models::{DataModelRecord, DataModelVersion, SubGraphStatus, VersionStatus};
use crate::kind_detector::RegistryKind;
use crate::store::TripleStore;
use oxigraph::model::*;
use oxigraph::sparql::QueryResults;

// ─── Vocabulary constants ─────────────────────────────────────────────────────

pub const REGISTRY_GRAPH: &str = "urn:system:data-model-registry";
const VER: &str = "urn:system:vocab/";
const DCT: &str = "http://purl.org/dc/terms/";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const ADMS: &str = "http://www.w3.org/ns/adms#";
const PROV: &str = "http://www.w3.org/ns/prov#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

// ─── SPARQL helper ────────────────────────────────────────────────────────────

fn var_str(row: &[Option<Term>], idx: usize) -> Option<String> {
    row.get(idx)?.as_ref().map(|t| match t {
        Term::NamedNode(nn) => nn.as_str().to_string(),
        Term::Literal(lit) => lit.value().to_string(),
        Term::BlankNode(bn) => bn.as_str().to_string(),
        Term::Triple(_) => String::new(),
    })
}

// ─── Data Model CRUD ──────────────────────────────────────────────────────────

/// List all data model records from the registry.
pub fn list_data_models(store: &TripleStore) -> Vec<DataModelRecord> {
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        PREFIX owl: <{OWL}>
        SELECT ?id ?title ?ns ?latestPub ?latestDraft ?createdAt ?createdBy ?description ?isPublic ?ownerType ?ownerId ?kind WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            ?id a ver:DataModel ;
                dct:title ?title ;
                ver:namespace ?ns .
            OPTIONAL {{ ?id ver:latestPublished ?latestPub }}
            OPTIONAL {{ ?id ver:latestDraft ?latestDraft }}
            OPTIONAL {{ ?id dct:created ?createdAt }}
            OPTIONAL {{ ?id dct:creator ?createdBy }}
            OPTIONAL {{ ?id dct:description ?description }}
            OPTIONAL {{ ?id ver:isPublic ?isPublic }}
            OPTIONAL {{ ?id ver:ownerType ?ownerType }}
            OPTIONAL {{ ?id ver:ownerId ?ownerId }}
            OPTIONAL {{ ?id ver:kind ?kind }}
          }}
        }}
        "#
    );
    let mut records = Vec::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(&q) {
        for row in solutions.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            let id = match var_str(&vals, 0) {
                Some(v) => v,
                None => continue,
            };
            // Extract data_model_id from the IRI (last path segment after /data-model/)
            let data_model_id = id.rsplit('/').next().unwrap_or(&id).to_string();

            // Count versions
            let version_count = count_versions(store, &id);

            // Resolve latest published version label
            let latest_pub_iri = var_str(&vals, 3);
            let latest_published = latest_pub_iri
                .as_deref()
                .and_then(|iri| iri.rsplit('/').next().map(str::to_string));
            let latest_draft_iri = var_str(&vals, 4);
            let latest_draft = latest_draft_iri
                .as_deref()
                .and_then(|iri| iri.rsplit('/').next().map(str::to_string));
            let is_public = var_str(&vals, 8)
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);

            records.push(DataModelRecord {
                id: data_model_id,
                title: var_str(&vals, 1).unwrap_or_default(),
                namespace: var_str(&vals, 2).unwrap_or_default(),
                description: var_str(&vals, 7),
                is_public,
                owner_type: var_str(&vals, 9),
                owner_id: var_str(&vals, 10),
                latest_published,
                latest_draft,
                version_count,
                created_at: var_str(&vals, 5).unwrap_or_default(),
                created_by: var_str(&vals, 6),
                kind: var_str(&vals, 11)
                    .map(|s| RegistryKind::from_persisted(&s))
                    .unwrap_or_default(),
            });
        }
    }
    records
}

fn count_versions(store: &TripleStore, data_model_iri_str: &str) -> usize {
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        SELECT (COUNT(?v) AS ?cnt) WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            ?v ver:dataModel <{data_model_iri_str}> .
          }}
        }}
        "#
    );
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            if let Some(s) = var_str(&vals, 0) {
                return s.parse().unwrap_or(0);
            }
        }
    }
    0
}

/// Get a single data model record by id.
pub fn get_data_model(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
) -> Option<DataModelRecord> {
    let ont_iri = format!("{}/data-model/{}", base_url, data_model_id);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        SELECT ?title ?ns ?latestPub ?latestDraft ?createdAt ?createdBy ?description ?isPublic ?ownerType ?ownerId ?kind WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ont_iri}> a ver:DataModel ;
                dct:title ?title ;
                ver:namespace ?ns .
            OPTIONAL {{ <{ont_iri}> ver:latestPublished ?latestPub }}
            OPTIONAL {{ <{ont_iri}> ver:latestDraft ?latestDraft }}
            OPTIONAL {{ <{ont_iri}> dct:created ?createdAt }}
            OPTIONAL {{ <{ont_iri}> dct:creator ?createdBy }}
            OPTIONAL {{ <{ont_iri}> dct:description ?description }}
            OPTIONAL {{ <{ont_iri}> ver:isPublic ?isPublic }}
            OPTIONAL {{ <{ont_iri}> ver:ownerType ?ownerType }}
            OPTIONAL {{ <{ont_iri}> ver:ownerId ?ownerId }}
            OPTIONAL {{ <{ont_iri}> ver:kind ?kind }}
          }}
        }}
        "#
    );
    if let Ok(QueryResults::Solutions(solutions)) = store.query(&q) {
        if let Some(row) = solutions.flatten().next() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            let version_count = count_versions(store, &ont_iri);
            let latest_pub_iri = var_str(&vals, 2);
            let latest_published = latest_pub_iri
                .as_deref()
                .and_then(|iri| iri.rsplit('/').next().map(str::to_string));
            let latest_draft_iri = var_str(&vals, 3);
            let latest_draft = latest_draft_iri
                .as_deref()
                .and_then(|iri| iri.rsplit('/').next().map(str::to_string));
            let is_public = var_str(&vals, 7)
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false);
            return Some(DataModelRecord {
                id: data_model_id.to_string(),
                title: var_str(&vals, 0).unwrap_or_default(),
                namespace: var_str(&vals, 1).unwrap_or_default(),
                description: var_str(&vals, 6),
                is_public,
                owner_type: var_str(&vals, 8),
                owner_id: var_str(&vals, 9),
                latest_published,
                latest_draft,
                version_count,
                created_at: var_str(&vals, 4).unwrap_or_default(),
                created_by: var_str(&vals, 5),
                kind: var_str(&vals, 10)
                    .map(|s| RegistryKind::from_persisted(&s))
                    .unwrap_or_default(),
            });
        }
    }
    None
}

/// Insert a new data model record into the registry.
#[allow(clippy::too_many_arguments)]
pub fn insert_data_model(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    title: &str,
    namespace: &str,
    description: Option<&str>,
    is_public: bool,
    owner_type: Option<&str>,
    owner_id: Option<&str>,
    created_by: Option<&str>,
    created_at: &str,
) -> Result<(), crate::store::engine::StoreError> {
    let ont_iri = format!("{}/data-model/{}", base_url, data_model_id);
    let creator_triple = created_by
        .map(|u| format!("  dct:creator <{u}> ;\n"))
        .unwrap_or_default();
    let description_triple = description
        .filter(|d| !d.is_empty())
        .map(|d| {
            let escaped = d.replace('\\', "\\\\").replace('"', "\\\"");
            format!("  dct:description \"{escaped}\"@en ;\n")
        })
        .unwrap_or_default();
    let owner_triples = match (owner_type, owner_id) {
        (Some(ot), Some(oid)) if !ot.is_empty() && !oid.is_empty() => {
            let ot_e = ot.replace('"', "\\\"");
            let oid_e = oid.replace('"', "\\\"");
            format!("  ver:ownerType \"{ot_e}\" ;\n  ver:ownerId \"{oid_e}\" ;\n")
        }
        _ => String::new(),
    };
    let is_public_str = if is_public { "true" } else { "false" };
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        INSERT DATA {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ont_iri}> a ver:DataModel ;
              dct:title "{title}"@en ;
              ver:namespace "{namespace}" ;
              ver:isPublic "{is_public_str}" ;
              {owner_triples}{description_triple}
              {creator_triple}
              dct:created "{created_at}"^^<{XSD}dateTime> .
          }}
        }}
        "#
    );
    store.update(&q)
}

/// Upsert the logical `kind` (`data-model` | `vocabulary` | …) of a registry
/// entry. Called on every version upload so the type badge/filter reflects the
/// latest detected content.
pub fn set_data_model_kind(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    kind: RegistryKind,
) -> Result<(), crate::store::engine::StoreError> {
    let ont_iri = format!("{}/data-model/{}", base_url, data_model_id);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        DELETE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:kind ?old }} }}
        INSERT {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:kind "{kind}" }} }}
        WHERE  {{ GRAPH <{REGISTRY_GRAPH}> {{ OPTIONAL {{ <{ont_iri}> ver:kind ?old }} }} }}
        "#,
        kind = kind.as_str()
    );
    store.update(&q)
}

/// Update editable metadata fields on a data model.
#[allow(clippy::too_many_arguments)]
pub fn update_data_model(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    title: Option<&str>,
    namespace: Option<&str>,
    description: Option<&str>,
    is_public: Option<bool>,
    owner_type: Option<&str>,
    owner_id: Option<&str>,
) -> Result<(), crate::store::engine::StoreError> {
    let ont_iri = format!("{}/data-model/{}", base_url, data_model_id);

    if let Some(t) = title {
        let escaped = t.replace('\\', "\\\\").replace('"', "\\\"");
        let q = format!(
            r#"
            PREFIX dct: <{DCT}>
            DELETE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> dct:title ?old }} }}
            INSERT {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> dct:title "{escaped}"@en }} }}
            WHERE  {{ GRAPH <{REGISTRY_GRAPH}> {{ OPTIONAL {{ <{ont_iri}> dct:title ?old }} }} }}
            "#
        );
        store.update(&q)?;
    }

    if let Some(ns) = namespace {
        let escaped = ns.replace('\\', "\\\\").replace('"', "\\\"");
        let q = format!(
            r#"
            PREFIX ver: <{VER}>
            DELETE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:namespace ?old }} }}
            INSERT {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:namespace "{escaped}" }} }}
            WHERE  {{ GRAPH <{REGISTRY_GRAPH}> {{ OPTIONAL {{ <{ont_iri}> ver:namespace ?old }} }} }}
            "#
        );
        store.update(&q)?;
    }

    // description: allow clearing by passing empty string
    if let Some(desc) = description {
        let q_del = format!(
            r#"
            PREFIX dct: <{DCT}>
            DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> dct:description ?old }} }}
            "#
        );
        store.update(&q_del)?;
        if !desc.is_empty() {
            let escaped = desc.replace('\\', "\\\\").replace('"', "\\\"");
            let q_ins = format!(
                r#"
                PREFIX dct: <{DCT}>
                INSERT DATA {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> dct:description "{escaped}"@en }} }}
                "#
            );
            store.update(&q_ins)?;
        }
    }

    if let Some(pub_flag) = is_public {
        let val = if pub_flag { "true" } else { "false" };
        let q = format!(
            r#"
            PREFIX ver: <{VER}>
            DELETE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:isPublic ?old }} }}
            INSERT {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:isPublic "{val}" }} }}
            WHERE  {{ GRAPH <{REGISTRY_GRAPH}> {{ OPTIONAL {{ <{ont_iri}> ver:isPublic ?old }} }} }}
            "#
        );
        store.update(&q)?;
    }

    if let Some(ot) = owner_type {
        let escaped = ot.replace('"', "\\\"");
        let q = format!(
            r#"
            PREFIX ver: <{VER}>
            DELETE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:ownerType ?old }} }}
            INSERT {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:ownerType "{escaped}" }} }}
            WHERE  {{ GRAPH <{REGISTRY_GRAPH}> {{ OPTIONAL {{ <{ont_iri}> ver:ownerType ?old }} }} }}
            "#
        );
        store.update(&q)?;
    }

    if let Some(oid) = owner_id {
        let escaped = oid.replace('"', "\\\"");
        let q = format!(
            r#"
            PREFIX ver: <{VER}>
            DELETE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:ownerId ?old }} }}
            INSERT {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:ownerId "{escaped}" }} }}
            WHERE  {{ GRAPH <{REGISTRY_GRAPH}> {{ OPTIONAL {{ <{ont_iri}> ver:ownerId ?old }} }} }}
            "#
        );
        store.update(&q)?;
    }

    Ok(())
}

/// Update the notes on a version record.
pub fn update_version_notes(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    version: &str,
    notes: Option<&str>,
) -> Result<(), crate::store::engine::StoreError> {
    let ver_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, data_model_id, version
    );
    // Always delete existing notes first
    let q_del = format!(
        r#"
        PREFIX adms: <{ADMS}>
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> adms:versionNotes ?old }} }}
        "#
    );
    store.update(&q_del)?;
    if let Some(n) = notes.filter(|n| !n.is_empty()) {
        let escaped = n.replace('\\', "\\\\").replace('"', "\\\"");
        let q_ins = format!(
            r#"
            PREFIX adms: <{ADMS}>
            INSERT DATA {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> adms:versionNotes "{escaped}"@en }} }}
            "#
        );
        store.update(&q_ins)?;
    }
    Ok(())
}

/// Delete a data model record and all its version records from the registry.
/// Does NOT delete the actual named graph data — call `delete_version_graphs` for that.
pub fn delete_data_model(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
) -> Result<(), crate::store::engine::StoreError> {
    let ont_iri = format!("{}/data-model/{}", base_url, data_model_id);
    // Delete all version metadata records first
    let q1 = format!(
        r#"
        PREFIX ver: <{VER}>
        DELETE WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            ?v ver:dataModel <{ont_iri}> .
            ?v ?vp ?vo .
          }}
        }}
        "#
    );
    store.update(&q1)?;
    // Delete ontology record itself
    let q2 = format!(
        r#"
        DELETE WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ont_iri}> ?p ?o .
          }}
        }}
        "#
    );
    store.update(&q2)
}

// ─── Version CRUD ─────────────────────────────────────────────────────────────

/// List all versions for a data model, ordered newest first.
pub fn list_versions(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
) -> Vec<DataModelVersion> {
    let ont_iri = format!("{}/data-model/{}", base_url, data_model_id);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        PREFIX owl: <{OWL}>
        PREFIX adms: <{ADMS}>
        PREFIX prov: <{PROV}>
        SELECT ?v ?semver ?status ?graphIri ?createdAt ?createdBy ?derivedFrom ?notes ?branch WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            ?v ver:dataModel <{ont_iri}> ;
               owl:versionInfo ?semver ;
               ver:status ?status ;
               ver:graphIri ?graphIri .
            OPTIONAL {{ ?v dct:created ?createdAt }}
            OPTIONAL {{ ?v dct:creator ?createdBy }}
            OPTIONAL {{ ?v prov:wasDerivedFrom ?derivedFrom }}
            OPTIONAL {{ ?v adms:versionNotes ?notes }}
            OPTIONAL {{ ?v ver:branch ?branch }}
          }}
        }}
        ORDER BY DESC(?createdAt)
        "#
    );
    let mut records = Vec::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(&q) {
        for row in solutions.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            let ver_iri = match var_str(&vals, 0) {
                Some(v) => v,
                None => continue,
            };
            let semver = match var_str(&vals, 1) {
                Some(v) => v,
                None => continue,
            };
            let status_str = var_str(&vals, 2).unwrap_or_default();
            let status = VersionStatus::from_str(&status_str).unwrap_or(VersionStatus::Draft);
            let graph_iri = match var_str(&vals, 3) {
                Some(v) => v,
                None => continue,
            };
            let sub_graphs = get_sub_graphs(store, &ver_iri);
            let sub_graph_status = get_sub_graph_statuses(store, &ver_iri);
            let derived_from =
                var_str(&vals, 6).and_then(|iri| iri.rsplit('/').next().map(str::to_string));
            records.push(DataModelVersion {
                data_model_id: data_model_id.to_string(),
                version: semver,
                status,
                graph_iri,
                sub_graphs,
                created_at: var_str(&vals, 4).unwrap_or_default(),
                created_by: var_str(&vals, 5),
                derived_from,
                notes: var_str(&vals, 7),
                branch: var_str(&vals, 8),
                sub_graph_status,
            });
        }
    }
    records
}

fn get_sub_graphs(store: &TripleStore, ver_iri: &str) -> Vec<String> {
    // We store sub_graphs as individual ver:subGraph triples
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        SELECT ?g WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ver_iri}> ver:subGraph ?g .
          }}
        }}
        "#
    );
    let mut graphs = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            if let Some(g) = var_str(&vals, 0) {
                graphs.push(g);
            }
        }
    }
    graphs
}

/// Deterministic state-entry IRI for a (version, subgraph) pair.
fn sub_graph_state_iri(ver_iri: &str, sub_graph_iri: &str) -> String {
    let slug: String = sub_graph_iri
        .rsplit('/')
        .next()
        .unwrap_or("graph")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("{ver_iri}/subgraph-state/{slug}")
}

/// Read per-subgraph status overrides for a version (Phase 6).
pub fn get_sub_graph_statuses(store: &TripleStore, ver_iri: &str) -> Vec<SubGraphStatus> {
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        SELECT ?g ?status WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ver_iri}> ver:subGraphState ?entry .
            ?entry ver:subGraph ?g ;
                   ver:status ?status .
          }}
        }}
        "#
    );
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            if let (Some(g), Some(s)) = (var_str(&vals, 0), var_str(&vals, 1)) {
                if let Some(status) = VersionStatus::from_str(&s) {
                    out.push(SubGraphStatus {
                        graph_iri: g,
                        status,
                    });
                }
            }
        }
    }
    out
}

/// Set (or clear) the lifecycle status of a single subgraph within a version.
/// Passing `None` removes the override so the subgraph inherits the version status.
pub fn set_sub_graph_status(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    version: &str,
    sub_graph_iri: &str,
    new_status: Option<VersionStatus>,
) -> Result<(), crate::store::engine::StoreError> {
    let ver_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, data_model_id, version
    );
    let entry_iri = sub_graph_state_iri(&ver_iri, sub_graph_iri);
    // Always remove any prior entry first.
    let q_del = format!(
        r#"
        PREFIX ver: <{VER}>
        DELETE WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ver_iri}> ver:subGraphState <{entry_iri}> .
            <{entry_iri}> ?p ?o .
          }}
        }}
        "#
    );
    store.update(&q_del)?;
    if let Some(status) = new_status {
        let q_ins = format!(
            r#"
            PREFIX ver: <{VER}>
            INSERT DATA {{
              GRAPH <{REGISTRY_GRAPH}> {{
                <{ver_iri}> ver:subGraphState <{entry_iri}> .
                <{entry_iri}> a ver:SubGraphState ;
                  ver:subGraph <{sub_graph_iri}> ;
                  ver:status "{status}" .
              }}
            }}
            "#,
            status = status.as_str()
        );
        store.update(&q_ins)?;
    }
    Ok(())
}

/// Get a single version record.
pub fn get_version(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    version: &str,
) -> Option<DataModelVersion> {
    let _dm_iri = format!("{}/data-model/{}", base_url, data_model_id);
    let ver_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, data_model_id, version
    );
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        PREFIX owl: <{OWL}>
        PREFIX adms: <{ADMS}>
        PREFIX prov: <{PROV}>
        SELECT ?semver ?status ?graphIri ?createdAt ?createdBy ?derivedFrom ?notes ?branch WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ver_iri}> owl:versionInfo ?semver ;
               ver:status ?status ;
               ver:graphIri ?graphIri .
            OPTIONAL {{ <{ver_iri}> dct:created ?createdAt }}
            OPTIONAL {{ <{ver_iri}> dct:creator ?createdBy }}
            OPTIONAL {{ <{ver_iri}> prov:wasDerivedFrom ?derivedFrom }}
            OPTIONAL {{ <{ver_iri}> adms:versionNotes ?notes }}
            OPTIONAL {{ <{ver_iri}> ver:branch ?branch }}
          }}
        }}
        "#
    );
    if let Ok(QueryResults::Solutions(solutions)) = store.query(&q) {
        if let Some(row) = solutions.flatten().next() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            let semver = var_str(&vals, 0)?;
            let status_str = var_str(&vals, 1).unwrap_or_default();
            let status = VersionStatus::from_str(&status_str).unwrap_or(VersionStatus::Draft);
            let graph_iri = var_str(&vals, 2)?;
            let sub_graphs = get_sub_graphs(store, &ver_iri);
            let sub_graph_status = get_sub_graph_statuses(store, &ver_iri);
            let derived_from =
                var_str(&vals, 5).and_then(|iri| iri.rsplit('/').next().map(str::to_string));
            return Some(DataModelVersion {
                data_model_id: data_model_id.to_string(),
                version: semver,
                status,
                graph_iri,
                sub_graphs,
                created_at: var_str(&vals, 3).unwrap_or_default(),
                created_by: var_str(&vals, 4),
                derived_from,
                notes: var_str(&vals, 6),
                branch: var_str(&vals, 7),
                sub_graph_status,
            });
        }
    }
    None
}

/// Insert a new version record into the registry.
pub fn insert_version(
    store: &TripleStore,
    base_url: &str,
    record: &DataModelVersion,
) -> Result<(), crate::store::engine::StoreError> {
    let ont_iri = format!("{}/data-model/{}", base_url, &record.data_model_id);
    let ver_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, &record.data_model_id, &record.version
    );

    let creator_triple = record
        .created_by
        .as_deref()
        .map(|u| format!("    dct:creator <{}> ;\n", u))
        .unwrap_or_default();

    let derived_triple = record
        .derived_from
        .as_deref()
        .map(|v| {
            format!(
                "    prov:wasDerivedFrom <{}/data-model/{}/version/{}> ;\n",
                base_url, &record.data_model_id, v
            )
        })
        .unwrap_or_default();

    let notes_triple = record
        .notes
        .as_deref()
        .map(|n| {
            let escaped = n.replace('\\', "\\\\").replace('"', "\\\"");
            format!("    adms:versionNotes \"{escaped}\"@en ;\n")
        })
        .unwrap_or_default();

    let branch_triple = record
        .branch
        .as_deref()
        .map(|b| {
            let escaped = b.replace('\\', "\\\\").replace('"', "\\\"");
            format!("    ver:branch \"{escaped}\" ;\n")
        })
        .unwrap_or_default();

    // Build sub-graph triples
    let sub_graph_triples: String = record
        .sub_graphs
        .iter()
        .map(|g| format!("    ver:subGraph <{g}> ;\n"))
        .collect();

    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        PREFIX owl: <{OWL}>
        PREFIX adms: <{ADMS}>
        PREFIX prov: <{PROV}>
        INSERT DATA {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ver_iri}> a ver:DataModelVersion ;
              owl:versionInfo "{version}" ;
              ver:dataModel <{ont_iri}> ;
              ver:status "{status}" ;
              ver:graphIri <{graph_iri}> ;
              {sub_graph_triples}
              {creator_triple}
              {derived_triple}
              {notes_triple}
              {branch_triple}
              dct:created "{created_at}"^^<{XSD}dateTime> .
          }}
        }}
        "#,
        version = record.version,
        status = record.status.as_str(),
        graph_iri = record.graph_iri,
        created_at = record.created_at,
    );
    store.update(&q)?;

    // Also add ver:hasVersion link on the ontology
    let q2 = format!(
        r#"
        PREFIX ver: <{VER}>
        INSERT DATA {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ont_iri}> ver:hasVersion <{ver_iri}> .
          }}
        }}
        "#
    );
    store.update(&q2)
}

/// Update the status of a version in the registry.
pub fn update_version_status(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    version: &str,
    new_status: VersionStatus,
) -> Result<(), crate::store::engine::StoreError> {
    let ver_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, data_model_id, version
    );
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        DELETE {{
          GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:status ?old }}
        }}
        INSERT {{
          GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:status "{new}" }}
        }}
        WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:status ?old }}
        }}
        "#,
        new = new_status.as_str()
    );
    store.update(&q)
}

/// Update the ver:latestPublished pointer on a data model.
pub fn update_latest_published(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    version: &str,
) -> Result<(), crate::store::engine::StoreError> {
    let ont_iri = format!("{}/data-model/{}", base_url, data_model_id);
    let ver_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, data_model_id, version
    );
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        DELETE {{
          GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:latestPublished ?old }}
        }}
        INSERT {{
          GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:latestPublished <{ver_iri}> }}
        }}
        WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            OPTIONAL {{ <{ont_iri}> ver:latestPublished ?old }}
          }}
        }}
        "#
    );
    store.update(&q)
}

/// Update the ver:latestDraft pointer on a data model.
pub fn update_latest_draft(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    version: &str,
) -> Result<(), crate::store::engine::StoreError> {
    let ont_iri = format!("{}/data-model/{}", base_url, data_model_id);
    let ver_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, data_model_id, version
    );
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        DELETE {{
          GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:latestDraft ?old }}
        }}
        INSERT {{
          GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:latestDraft <{ver_iri}> }}
        }}
        WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            OPTIONAL {{ <{ont_iri}> ver:latestDraft ?old }}
          }}
        }}
        "#
    );
    store.update(&q)
}

/// Remove the ver:latestDraft pointer from a data model (when a draft is staged/published).
pub fn clear_latest_draft(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
) -> Result<(), crate::store::engine::StoreError> {
    let ont_iri = format!("{}/data-model/{}", base_url, data_model_id);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        DELETE WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{ <{ont_iri}> ver:latestDraft ?old }}
        }}
        "#
    );
    store.update(&q)
}

/// Check whether a version IRI already exists in the registry.
pub fn version_exists(
    store: &TripleStore,
    base_url: &str,
    data_model_id: &str,
    version: &str,
) -> bool {
    get_version(store, base_url, data_model_id, version).is_some()
}

/// Check whether a data model IRI already exists in the registry.
pub fn data_model_exists(store: &TripleStore, base_url: &str, data_model_id: &str) -> bool {
    get_data_model(store, base_url, data_model_id).is_some()
}
