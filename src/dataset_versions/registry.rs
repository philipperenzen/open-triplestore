//! Dataset version metadata, stored as RDF triples in the named graph
//! `<urn:system:dataset-version-registry>` inside Oxigraph.

use oxigraph::model::*;
use oxigraph::sparql::QueryResults;

use super::models::{Anchor, DatasetVersion, GraphMapping, PatchRange, VersionKind, VersionStatus};
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
        #[cfg(feature = "rdf-12")]
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
        SELECT ?v ?semver ?status ?graphIri ?createdAt ?createdBy ?derivedFrom ?notes ?branch {DELTA_VARS} WHERE {{
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
            {delta_optionals}
          }}
        }}
        ORDER BY DESC(?createdAt)
        "#,
        delta_optionals = delta_optionals("?v"),
    );
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            let ver_iri = match var_str(&vals, 0) {
                Some(v) => v,
                None => continue,
            };
            // One record per version, whatever duplicate triples a racing
            // pair of writers left behind.
            if !seen.insert(ver_iri.clone()) {
                continue;
            }
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
            let mut record = DatasetVersion::checkpoint(
                dataset_id,
                &semver,
                status,
                graph_iri,
                source_map,
                var_str(&vals, 5),
                var_str(&vals, 7),
                var_str(&vals, 8),
            );
            record.created_at = var_str(&vals, 4).unwrap_or_default();
            record.derived_from = derived_from;
            apply_delta_fields(store, &mut record, &vals[9..], &ver_iri, snapshot_graphs);
            out.push(record);
        }
    }
    out
}

/// The SELECT variables of [`delta_optionals`], in order.
const DELTA_VARS: &str = "?base ?pEpoch ?pFrom ?pTo ?hash ?qcount ?ccount ?broken";

/// OPTIONAL clauses reading a version's delta-versioning fields (delta note
/// §3.1), for the version bound to `v`.
fn delta_optionals(v: &str) -> String {
    format!(
        "OPTIONAL {{ {v} ver:baseVersion ?base }}
            OPTIONAL {{ {v} ver:patchEpoch ?pEpoch }}
            OPTIONAL {{ {v} ver:patchFromSeq ?pFrom }}
            OPTIONAL {{ {v} ver:patchToSeq ?pTo }}
            OPTIONAL {{ {v} ver:contentHash ?hash }}
            OPTIONAL {{ {v} ver:quadCount ?qcount }}
            OPTIONAL {{ {v} ver:changeCount ?ccount }}
            OPTIONAL {{ {v} ver:chainBroken ?broken }}"
    )
}

/// Fill a record's kind and delta fields from the [`DELTA_VARS`] values.
/// `ver:subGraph` present means physical: a checkpoint, its graphs returned
/// as they are. No `ver:subGraph` but a base means a delta: its graphs are
/// its graph map's targets, built on demand (delta note §3.1).
fn apply_delta_fields(
    store: &TripleStore,
    record: &mut DatasetVersion,
    vals: &[Option<Term>],
    ver_iri: &str,
    snapshot_graphs: Vec<String>,
) {
    let int = |i: usize| var_str(vals, i).and_then(|s| s.parse::<i64>().ok());
    let base = var_str(vals, 0).and_then(|i| i.rsplit('/').next().map(str::to_string));
    let epoch = var_str(vals, 1);
    let (from, to) = (int(2), int(3));
    record.content_hash = var_str(vals, 4);
    record.quad_count = int(5);
    record.change_count = int(6);
    record.chain_broken = var_str(vals, 7).is_some_and(|b| b == "true" || b == "1");
    if snapshot_graphs.is_empty() && base.is_some() {
        record.kind = VersionKind::Delta;
        record.base_version = base;
        record.snapshot_graphs = record
            .source_map
            .iter()
            .map(|m| m.snapshot_graph.clone())
            .collect();
        if let (Some(epoch), Some(from), Some(to)) = (epoch.clone(), from, to) {
            record.patch_range = Some(PatchRange {
                epoch,
                from_seq: from,
                to_seq: to,
            });
        }
        record.materialised = store.changes().materialisation(ver_iri).is_some();
    } else {
        record.kind = VersionKind::Checkpoint;
        record.snapshot_graphs = snapshot_graphs;
    }
    if let (Some(epoch), Some(seq)) = (epoch, to) {
        record.anchor = Some(Anchor { epoch, seq });
    }
}

/// Get a single version record.
pub fn get_version(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
) -> Option<DatasetVersion> {
    // Same reasoning as the data-model registry: the version is interpolated into
    // an IRI in the SELECT below, so an unsafe one reads as "not found".
    crate::data_models::version_iri::validate_version(version).ok()?;
    let ver_iri = version_iri(base_url, dataset_id, version);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        PREFIX owl: <{OWL}>
        PREFIX adms: <{ADMS}>
        PREFIX prov: <{PROV}>
        SELECT ?semver ?status ?graphIri ?createdAt ?createdBy ?derivedFrom ?notes ?branch {DELTA_VARS} WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ver_iri}> owl:versionInfo ?semver ;
               ver:status ?status ;
               ver:graphIri ?graphIri .
            OPTIONAL {{ <{ver_iri}> dct:created ?createdAt }}
            OPTIONAL {{ <{ver_iri}> dct:creator ?createdBy }}
            OPTIONAL {{ <{ver_iri}> prov:wasDerivedFrom ?derivedFrom }}
            OPTIONAL {{ <{ver_iri}> adms:versionNotes ?notes }}
            OPTIONAL {{ <{ver_iri}> ver:branch ?branch }}
            {delta_optionals}
          }}
        }}
        "#,
        delta_optionals = delta_optionals(&format!("<{ver_iri}>")),
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
            let mut record = DatasetVersion::checkpoint(
                dataset_id,
                &semver,
                status,
                graph_iri,
                source_map,
                var_str(&vals, 4),
                var_str(&vals, 6),
                var_str(&vals, 7),
            );
            record.created_at = var_str(&vals, 3).unwrap_or_default();
            record.derived_from = derived_from;
            apply_delta_fields(store, &mut record, &vals[8..], &ver_iri, snapshot_graphs);
            return Some(record);
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
        SELECT ?snap ?src ?hash ?quads WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ver_iri}> ver:graphMap ?m .
            ?m ver:snapshotGraph ?snap ; ver:sourceGraph ?src .
            OPTIONAL {{ ?m ver:graphHash ?hash }}
            OPTIONAL {{ ?m ver:graphQuads ?quads }}
          }}
        }}"#
    );
    let mut out: Vec<GraphMapping> = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            let vals = row.values().to_vec();
            if let (Some(snap), Some(src)) = (var_str(&vals, 0), var_str(&vals, 1)) {
                if out.iter().any(|m| m.snapshot_graph == snap) {
                    continue;
                }
                let mut m = GraphMapping::new(snap, src);
                m.content_hash = var_str(&vals, 2);
                m.quad_count = var_str(&vals, 3).and_then(|n| n.parse().ok());
                out.push(m);
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
    // Reached from the version/branch/pipeline paths as well as the HTTP handlers;
    // the version goes into an IRI and an `owl:versionInfo` literal below, so it is
    // validated here too rather than trusting each caller to have checked.
    crate::data_models::version_iri::validate_version(&record.version)
        .map_err(crate::store::engine::StoreError::Parse)?;
    if let Some(derived) = record.derived_from.as_deref() {
        crate::data_models::version_iri::validate_version(derived)
            .map_err(crate::store::engine::StoreError::Parse)?;
    }
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
    // A delta has no `ver:subGraph`: that predicate means "physical", and
    // `purge_version` drops every graph it lists — pointing a delta at
    // anything would let collecting it delete what it names (delta note §3.1).
    let sub_graphs: String = if record.is_delta() {
        String::new()
    } else {
        record
            .snapshot_graphs
            .iter()
            .map(|g| format!("    ver:subGraph <{g}> ;\n"))
            .collect()
    };
    let delta_fields = delta_field_lines(base_url, record);

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
              {sub_graphs}{creator}{derived}{notes}{branch}{delta_fields}
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
        let mut extra = String::new();
        if let Some(h) = m.content_hash.as_deref() {
            if h.bytes().all(|b| b.is_ascii_hexdigit()) {
                extra.push_str(&format!(" ;\n                  ver:graphHash \"{h}\""));
            }
        }
        if let Some(n) = m.quad_count {
            extra.push_str(&format!(
                " ;\n                  ver:graphQuads \"{n}\"^^<{XSD}integer>"
            ));
        }
        let qm = format!(
            r#"
            PREFIX ver: <{VER}>
            INSERT DATA {{
              GRAPH <{REGISTRY_GRAPH}> {{
                <{ver_iri}> ver:graphMap <{entry}> .
                <{entry}> ver:snapshotGraph <{snap}> ;
                  ver:sourceGraph <{src}>{extra} .
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

/// The registry lines for a record's delta-versioning fields.
fn delta_field_lines(base_url: &str, record: &DatasetVersion) -> String {
    let mut out = String::new();
    if let Some(base) = record.base_version.as_deref() {
        out.push_str(&format!(
            "    ver:baseVersion <{}> ;\n",
            version_iri(base_url, &record.dataset_id, base)
        ));
    }
    if let Some(r) = &record.patch_range {
        out.push_str(&format!(
            "    ver:patchFromSeq \"{}\"^^<{XSD}integer> ;\n",
            r.from_seq
        ));
    }
    if let Some(a) = &record.anchor {
        let e = a.epoch.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str(&format!("    ver:patchEpoch \"{e}\" ;\n"));
        out.push_str(&format!(
            "    ver:patchToSeq \"{}\"^^<{XSD}integer> ;\n",
            a.seq
        ));
    }
    if let Some(h) = record.content_hash.as_deref() {
        if h.bytes().all(|b| b.is_ascii_hexdigit()) {
            out.push_str(&format!("    ver:contentHash \"{h}\" ;\n"));
        }
    }
    if let Some(n) = record.quad_count {
        out.push_str(&format!("    ver:quadCount \"{n}\"^^<{XSD}integer> ;\n"));
    }
    if let Some(n) = record.change_count {
        out.push_str(&format!("    ver:changeCount \"{n}\"^^<{XSD}integer> ;\n"));
    }
    if record.chain_broken {
        out.push_str(&format!("    ver:chainBroken \"true\"^^<{XSD}boolean> ;\n"));
    }
    out
}

/// Mark a version's chain broken: its data cannot be rebuilt (a row of its
/// range was lost, or the rebuild did not reproduce its content hash).
pub fn set_chain_broken(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
) -> Result<(), crate::store::engine::StoreError> {
    let ver_iri = version_iri(base_url, dataset_id, version);
    store.update(&format!(
        r#"PREFIX ver: <{VER}>
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:chainBroken ?o }} }} ;
        INSERT DATA {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:chainBroken "true"^^<{XSD}boolean> }} }}"#
    ))
}

/// Turn a materialised delta into a checkpoint: its graph map's targets —
/// already built — become its `ver:subGraph`s, and it stops naming a base
/// and a range. Its anchor and content hash stay (delta note §3.2: a
/// published version is physical).
pub fn promote_to_checkpoint(
    store: &TripleStore,
    base_url: &str,
    record: &DatasetVersion,
    quad_count: i64,
) -> Result<(), crate::store::engine::StoreError> {
    let ver_iri = version_iri(base_url, &record.dataset_id, &record.version);
    let subs: String = record
        .source_map
        .iter()
        .map(|m| format!("<{ver_iri}> ver:subGraph <{}> .\n", m.snapshot_graph))
        .collect();
    store.update(&format!(
        r#"PREFIX ver: <{VER}>
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:baseVersion ?b }} }} ;
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:patchFromSeq ?f }} }} ;
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:changeCount ?c }} }} ;
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:quadCount ?q }} }} ;
        INSERT DATA {{ GRAPH <{REGISTRY_GRAPH}> {{
          {subs}<{ver_iri}> ver:quadCount "{quad_count}"^^<{XSD}integer> .
        }} }}"#
    ))
}

/// Versions of the dataset that name `base` as their `ver:baseVersion`: a
/// checkpoint any of them chains from is never collected (delta note §3.7).
pub fn versions_based_on(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    base: &str,
) -> Vec<String> {
    let base_iri = version_iri(base_url, dataset_id, base);
    let q = format!(
        r#"PREFIX ver: <{VER}>
        PREFIX owl: <{OWL}>
        SELECT DISTINCT ?semver WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{
            ?v ver:baseVersion <{base_iri}> ; owl:versionInfo ?semver .
        }} }}"#
    );
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            if let Some(v) = var_str(row.values(), 0) {
                out.push(v);
            }
        }
    }
    out
}

/// Remove a version's registry triples: its graph map, its own description,
/// and every pointer the dataset holds to it (`ver:hasVersion`,
/// `ver:latestPublished`, `ver:latestDraft`). The snapshot graphs themselves
/// are dropped by the caller (the store's bulk path), which also decides the
/// policy — published versions are refused unless forced.
pub fn delete_version(
    store: &TripleStore,
    base_url: &str,
    dataset_id: &str,
    version: &str,
) -> Result<(), crate::store::engine::StoreError> {
    crate::data_models::version_iri::validate_version(version).map_err(|e| {
        crate::store::engine::StoreError::Parse(format!("version '{version}': {e}"))
    })?;
    let ver_iri = version_iri(base_url, dataset_id, version);
    let ds_iri = dataset_iri(base_url, dataset_id);
    let q = format!(
        r#"PREFIX ver: <{VER}>
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ver:graphMap ?e . ?e ?p ?o }} }} ;
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ver_iri}> ?p ?o }} }} ;
        DELETE WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ <{ds_iri}> ?p <{ver_iri}> }} }}"#
    );
    store.update(&q)
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
