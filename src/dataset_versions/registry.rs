//! Dataset version metadata, stored as RDF triples in the named graph
//! `<urn:system:dataset-version-registry>` inside Oxigraph.

use oxigraph::model::*;
use oxigraph::sparql::QueryResults;

use super::models::{DatasetVersion, GraphMapping, VersionStatus};
use crate::store::TripleStore;

pub const REGISTRY_GRAPH: &str = "urn:system:dataset-version-registry";
const VER: &str = "urn:system:vocab/";
const DCT: &str = "http://purl.org/dc/terms/";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const ADMS: &str = "http://www.w3.org/ns/adms#";
const PROV: &str = "http://www.w3.org/ns/prov#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn var_str(row: &[Option<Term>], idx: usize) -> Option<String> {
    row.get(idx)?.as_ref().map(|t| match t {
        Term::NamedNode(nn) => nn.as_str().to_string(),
        Term::Literal(lit) => lit.value().to_string(),
        Term::BlankNode(bn) => bn.as_str().to_string(),
        Term::Triple(_) => String::new(),
    })
}

fn dataset_iri(base_url: &str, dataset_id: &str) -> String {
    format!("{base_url}/dataset/{dataset_id}")
}

fn version_iri(base_url: &str, dataset_id: &str, version: &str) -> String {
    format!("{base_url}/dataset/{dataset_id}/version/{version}")
}

// ─── Version listing ──────────────────────────────────────────────────────

/// List all versions for a dataset, newest first.
pub fn list_versions(store: &TripleStore, base_url: &str, dataset_id: &str) -> Vec<DatasetVersion> {
    let ds_iri = dataset_iri(base_url, dataset_id);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        PREFIX owl: <{OWL}>
        PREFIX adms: <{ADMS}>
        PREFIX prov: <{PROV}>
        SELECT ?v ?semver ?status ?graphIri ?createdAt ?createdBy ?derivedFrom ?notes ?branch WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            ?v ver:dataset <{ds_iri}> ;
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
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            let ver_iri = match var_str(&vals, 0) {
                Some(v) => v,
                None => continue,
            };
            let semver = match var_str(&vals, 1) {
                Some(v) => v,
                None => continue,
            };
            let status = VersionStatus::from_str(&var_str(&vals, 2).unwrap_or_default())
                .unwrap_or(VersionStatus::Draft);
            let graph_iri = match var_str(&vals, 3) {
                Some(v) => v,
                None => continue,
            };
            let snapshot_graphs = get_snapshot_graphs(store, &ver_iri);
            let source_map = get_graph_map(store, &ver_iri);
            let derived_from =
                var_str(&vals, 6).and_then(|i| i.rsplit('/').next().map(str::to_string));
            out.push(DatasetVersion {
                dataset_id: dataset_id.to_string(),
                version: semver,
                status,
                graph_iri,
                snapshot_graphs,
                source_map,
                created_at: var_str(&vals, 4).unwrap_or_default(),
                created_by: var_str(&vals, 5),
                derived_from,
                notes: var_str(&vals, 7),
                branch: var_str(&vals, 8),
            });
        }
    }
    out
}

/// Get a single version record.
pub fn get_version(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
) -> Option<DatasetVersion> {
    let ver_iri = version_iri(base_url, dataset_id, version);
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
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        if let Some(row) = sols.flatten().next() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            let semver = var_str(&vals, 0)?;
            let status = VersionStatus::from_str(&var_str(&vals, 1).unwrap_or_default())
                .unwrap_or(VersionStatus::Draft);
            let graph_iri = var_str(&vals, 2)?;
            let snapshot_graphs = get_snapshot_graphs(store, &ver_iri);
            let source_map = get_graph_map(store, &ver_iri);
            let derived_from =
                var_str(&vals, 5).and_then(|i| i.rsplit('/').next().map(str::to_string));
            return Some(DatasetVersion {
                dataset_id: dataset_id.to_string(),
                version: semver,
                status,
                graph_iri,
                snapshot_graphs,
                source_map,
                created_at: var_str(&vals, 3).unwrap_or_default(),
                created_by: var_str(&vals, 4),
                derived_from,
                notes: var_str(&vals, 6),
                branch: var_str(&vals, 7),
            });
        }
    }
    None
}

fn get_snapshot_graphs(store: &TripleStore, ver_iri: &str) -> Vec<String> {
    let q = format!(
        r#"PREFIX ver: <{VER}>
        SELECT ?g WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:subGraph ?g }} }}"#
    );
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            if let Some(g) = var_str(row.values(), 0) {
                out.push(g);
            }
        }
    }
    out
}

fn get_graph_map(store: &TripleStore, ver_iri: &str) -> Vec<GraphMapping> {
    let q = format!(
        r#"PREFIX ver: <{VER}>
        SELECT ?snap ?src WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ver_iri}> ver:graphMap ?m .
            ?m ver:snapshotGraph ?snap ; ver:sourceGraph ?src .
          }}
        }}"#
    );
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            let vals = row.values().to_vec();
            if let (Some(snap), Some(src)) = (var_str(&vals, 0), var_str(&vals, 1)) {
                out.push(GraphMapping {
                    snapshot_graph: snap,
                    source_graph: src,
                });
            }
        }
    }
    out
}

// ─── Insert / update ──────────────────────────────────────────────────────

/// Insert a new dataset version record (with snapshot graphs + source mapping).
pub fn insert_version(
    store: &TripleStore,
    base_url: &str,
    record: &DatasetVersion,
) -> Result<(), crate::store::engine::StoreError> {
    let ds_iri = dataset_iri(base_url, &record.dataset_id);
    let ver_iri = version_iri(base_url, &record.dataset_id, &record.version);

    let creator = record
        .created_by
        .as_deref()
        .map(|u| format!("    dct:creator <{u}> ;\n"))
        .unwrap_or_default();
    let derived = record
        .derived_from
        .as_deref()
        .map(|v| {
            format!(
                "    prov:wasDerivedFrom <{}> ;\n",
                version_iri(base_url, &record.dataset_id, v)
            )
        })
        .unwrap_or_default();
    let notes = record
        .notes
        .as_deref()
        .map(|n| {
            let e = n.replace('\\', "\\\\").replace('"', "\\\"");
            format!("    adms:versionNotes \"{e}\"@en ;\n")
        })
        .unwrap_or_default();
    let branch = record
        .branch
        .as_deref()
        .map(|b| {
            let e = b.replace('\\', "\\\\").replace('"', "\\\"");
            format!("    ver:branch \"{e}\" ;\n")
        })
        .unwrap_or_default();
    let sub_graphs: String = record
        .snapshot_graphs
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
            <{ver_iri}> a ver:DatasetVersion ;
              owl:versionInfo "{version}" ;
              ver:dataset <{ds_iri}> ;
              ver:status "{status}" ;
              ver:graphIri <{ver_iri}> ;
              {sub_graphs}{creator}{derived}{notes}{branch}
              dct:created "{created_at}"^^<{XSD}dateTime> .
            <{ds_iri}> ver:hasVersion <{ver_iri}> .
          }}
        }}
        "#,
        version = record.version,
        status = record.status.as_str(),
        created_at = record.created_at,
    );
    store.update(&q)?;

    // graph-map entries (snapshot → source)
    for (i, m) in record.source_map.iter().enumerate() {
        let entry = format!("{ver_iri}/graph-map/{i}");
        let qm = format!(
            r#"
            PREFIX ver: <{VER}>
            INSERT DATA {{
              GRAPH <{REGISTRY_GRAPH}> {{
                <{ver_iri}> ver:graphMap <{entry}> .
                <{entry}> ver:snapshotGraph <{snap}> ;
                  ver:sourceGraph <{src}> .
              }}
            }}
            "#,
            snap = m.snapshot_graph,
            src = m.source_graph,
        );
        store.update(&qm)?;
    }
    Ok(())
}

pub fn update_version_status(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
    new_status: VersionStatus,
) -> Result<(), crate::store::engine::StoreError> {
    let ver_iri = version_iri(base_url, dataset_id, version);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        DELETE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:status ?old }} }}
        INSERT {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:status "{new}" }} }}
        WHERE  {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:status ?old }} }}
        "#,
        new = new_status.as_str()
    );
    store.update(&q)
}

pub fn update_latest_published(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
) -> Result<(), crate::store::engine::StoreError> {
    set_pointer(store, base_url, dataset_id, version, "latestPublished")
}

pub fn update_latest_draft(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
) -> Result<(), crate::store::engine::StoreError> {
    set_pointer(store, base_url, dataset_id, version, "latestDraft")
}

fn set_pointer(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
    pred: &str,
) -> Result<(), crate::store::engine::StoreError> {
    let ds_iri = dataset_iri(base_url, dataset_id);
    let ver_iri = version_iri(base_url, dataset_id, version);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        DELETE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ds_iri}> ver:{pred} ?old }} }}
        INSERT {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ds_iri}> ver:{pred} <{ver_iri}> }} }}
        WHERE  {{ GRAPH <{REGISTRY_GRAPH}> {{ OPTIONAL {{ <{ds_iri}> ver:{pred} ?old }} }} }}
        "#
    );
    store.update(&q)
}

pub fn clear_latest_draft(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
) -> Result<(), crate::store::engine::StoreError> {
    let ds_iri = dataset_iri(base_url, dataset_id);
    let q = format!(
        r#"PREFIX ver: <{VER}>
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ds_iri}> ver:latestDraft ?o }} }}"#
    );
    store.update(&q)
}

pub fn update_version_notes(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
    notes: Option<&str>,
) -> Result<(), crate::store::engine::StoreError> {
    let ver_iri = version_iri(base_url, dataset_id, version);
    let q_del = format!(
        r#"PREFIX adms: <{ADMS}>
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> adms:versionNotes ?o }} }}"#
    );
    store.update(&q_del)?;
    if let Some(n) = notes.filter(|n| !n.is_empty()) {
        let e = n.replace('\\', "\\\\").replace('"', "\\\"");
        let q_ins = format!(
            r#"PREFIX adms: <{ADMS}>
            INSERT DATA {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> adms:versionNotes "{e}"@en }} }}"#
        );
        store.update(&q_ins)?;
    }
    Ok(())
}

/// Read the dataset's `latestPublished` / `latestDraft` version labels.
pub fn get_pointers(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
) -> (Option<String>, Option<String>) {
    let ds_iri = dataset_iri(base_url, dataset_id);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        SELECT ?pub ?draft WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            OPTIONAL {{ <{ds_iri}> ver:latestPublished ?pub }}
            OPTIONAL {{ <{ds_iri}> ver:latestDraft ?draft }}
          }}
        }}
        "#
    );
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        if let Some(row) = sols.flatten().next() {
            let vals = row.values().to_vec();
            let to_label =
                |s: Option<String>| s.and_then(|i| i.rsplit('/').next().map(str::to_string));
            return (to_label(var_str(&vals, 0)), to_label(var_str(&vals, 1)));
        }
    }
    (None, None)
}

pub fn version_exists(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
) -> bool {
    get_version(store, base_url, dataset_id, version).is_some()
}
