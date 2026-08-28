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
        #[cfg(feature = "rdf-12")]
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
    // One record per model, whatever the registry graph holds. Every property
    // above except `a`, title and namespace is OPTIONAL, so a subject carrying
    // two values for any of them (a stray second `dct:created` from an
    // interrupted re-register, a description in two languages, …) makes this
    // SELECT fan out into one row per combination — and `data_model_id` is the
    // IRI's last path segment, so distinct IRIs can collide onto one id too.
    // Both hand the API duplicate ids. The registry page keys its list by id,
    // where a duplicate key is a hard render abort: the list never paints and
    // the page sits on its loading spinner forever, with only a console error
    // to show for it. Deduplicating here also skips the redundant
    // `count_versions` query each surplus row would otherwise run.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(&q) {
        for row in solutions.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            let id = match var_str(&vals, 0) {
                Some(v) => v,
                None => continue,
            };
            // Extract data_model_id from the IRI (last path segment after /data-model/)
            let data_model_id = id.rsplit('/').next().unwrap_or(&id).to_string();
            if !seen.insert(data_model_id.clone()) {
                tracing::debug!(
                    id = %data_model_id,
                    iri = %id,
                    "registry list: dropping a duplicate row for this model"
                );
                continue;
            }

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

/// One registry entry prepared for the Spark chat's platform context: the
/// model's human identity plus the named graph holding its current *published*
/// content, resolved in the same query. Distinct from [`DataModelRecord`] on
/// purpose — the prompt needs the graph IRI (which `list_data_models` discards)
/// and none of the per-model version counting that listing pays for.
pub struct ModelContextEntry {
    pub title: String,
    pub namespace: String,
    pub kind: RegistryKind,
    pub is_public: bool,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
    /// Named graph holding the latest published version's content, when one exists.
    pub graph_iri: Option<String>,
    /// That version's semver label ("2.1.0"), for prose.
    pub version: Option<String>,
    /// Named graph of the latest DRAFT version, when one exists — unreviewed
    /// content, surfaced so an assistant can offer it as an explicit choice
    /// rather than silently mixing it with published definitions.
    pub draft_graph_iri: Option<String>,
    /// The draft's semver label.
    pub draft_version: Option<String>,
}

/// List every registered model/vocabulary with the graph of its latest
/// published version, sorted by title so a prompt built from it is stable
/// across turns. Visibility is NOT applied here — callers filter with
/// `can_access_ontology`, exactly like the `/api/models` handler.
pub fn list_models_for_context(store: &TripleStore) -> Vec<ModelContextEntry> {
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        PREFIX owl: <{OWL}>
        SELECT ?m ?title ?ns ?kind ?isPublic ?ownerType ?ownerId ?graphIri ?semver ?draftIri ?draftVer WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            ?m a ver:DataModel ;
               dct:title ?title ;
               ver:namespace ?ns .
            OPTIONAL {{ ?m ver:kind ?kind }}
            OPTIONAL {{ ?m ver:isPublic ?isPublic }}
            OPTIONAL {{ ?m ver:ownerType ?ownerType }}
            OPTIONAL {{ ?m ver:ownerId ?ownerId }}
            OPTIONAL {{
              ?m ver:latestPublished ?v .
              ?v ver:graphIri ?graphIri .
              OPTIONAL {{ ?v owl:versionInfo ?semver }}
            }}
            OPTIONAL {{
              ?m ver:latestDraft ?d .
              ?d ver:graphIri ?draftIri .
              OPTIONAL {{ ?d owl:versionInfo ?draftVer }}
            }}
          }}
        }}
        "#
    );
    let mut out: Vec<ModelContextEntry> = Vec::new();
    // Same fanout guard as `list_data_models`: any doubled OPTIONAL value would
    // otherwise list the model once per combination.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(&q) {
        for row in solutions.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            let Some(m) = var_str(&vals, 0) else { continue };
            if !seen.insert(m) {
                continue;
            }
            out.push(ModelContextEntry {
                title: var_str(&vals, 1).unwrap_or_default(),
                namespace: var_str(&vals, 2).unwrap_or_default(),
                kind: var_str(&vals, 3)
                    .map(|s| RegistryKind::from_persisted(&s))
                    .unwrap_or_default(),
                is_public: var_str(&vals, 4)
                    .map(|v| v == "true" || v == "1")
                    .unwrap_or(false),
                owner_type: var_str(&vals, 5),
                owner_id: var_str(&vals, 6),
                graph_iri: var_str(&vals, 7),
                version: var_str(&vals, 8),
                draft_graph_iri: var_str(&vals, 9),
                draft_version: var_str(&vals, 10),
            });
        }
    }
    out.sort_by(|a, b| a.title.cmp(&b.title).then(a.namespace.cmp(&b.namespace)));
    out
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
    // Escape user-supplied literals before interpolating them into the SPARQL
    // string (mirrors the description/owner escaping above).
    let title_e = crate::store::escape_sparql_literal(title);
    let namespace_e = crate::store::escape_sparql_literal(namespace);
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        INSERT DATA {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ont_iri}> a ver:DataModel ;
              dct:title "{title_e}"@en ;
              ver:namespace "{namespace_e}" ;
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

    // Collect the version-record subjects for this model, then delete their triples
    // in bounded batches. A single `DELETE WHERE { ?v ver:dataModel <ont> . ?v ?vp ?vo }`
    // is unbounded: a model with thousands of versions produces one giant transaction
    // that can pin RocksDB under write pressure and stall unrelated writes. Batching by
    // a fixed number of subjects caps each transaction's size.
    let select_versions = format!(
        r#"
        PREFIX ver: <{VER}>
        SELECT ?v WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{ ?v ver:dataModel <{ont_iri}> }}
        }}
        "#
    );
    let mut version_iris: Vec<String> = Vec::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(&select_versions) {
        for row in solutions.flatten() {
            let vals: Vec<Option<Term>> = row.values().to_vec();
            if let Some(iri) = var_str(&vals, 0) {
                version_iris.push(iri);
            }
        }
    }

    // Subjects per delete transaction. Each version record is a handful of triples, so
    // this bounds a batch to a few thousand quads while keeping the query small.
    const DELETE_BATCH_SUBJECTS: usize = 256;
    for chunk in version_iris.chunks(DELETE_BATCH_SUBJECTS) {
        let values = chunk
            .iter()
            .map(|v| format!("<{v}>"))
            .collect::<Vec<_>>()
            .join(" ");
        let q = format!(
            r#"
            DELETE {{
              GRAPH <{REGISTRY_GRAPH}> {{ ?v ?vp ?vo }}
            }}
            WHERE {{
              VALUES ?v {{ {values} }}
              GRAPH <{REGISTRY_GRAPH}> {{ ?v ?vp ?vo }}
            }}
            "#
        );
        store.update(&q)?;
    }

    // Delete the model record itself — a single subject, inherently bounded.
    let q_record = format!(
        r#"
        DELETE WHERE {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{ont_iri}> ?p ?o .
          }}
        }}
        "#
    );
    store.update(&q_record)
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
    // The version becomes part of an IRI inside the SELECT below; an unsafe one
    // could close it and append graph patterns, so treat it as "no such version"
    // rather than querying with it.
    crate::data_models::version_iri::validate_version(version).ok()?;
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
    // The version lands in both an IRI and an `owl:versionInfo` literal below, and
    // this function is reached from several upload/seed/pipeline paths — validate
    // here so a caller that forgot to check cannot inject SPARQL.
    crate::data_models::version_iri::validate_version(&record.version)
        .map_err(crate::store::engine::StoreError::Parse)?;
    let ont_iri = format!("{}/data-model/{}", base_url, record.data_model_id);
    let ver_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, record.data_model_id, record.version
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
                base_url, record.data_model_id, v
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

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "http://localhost:7878";

    fn seed_one(store: &TripleStore, id: &str, created: &str) {
        insert_data_model(
            store,
            BASE,
            id,
            "Good Relations",
            "http://purl.org/goodrelations/v1#",
            None,
            true,
            None,
            None,
            None,
            created,
        )
        .unwrap();
    }

    /// A registry entry carrying a second value for one of the OPTIONAL
    /// properties must still list as exactly one model. Without the guard the
    /// SELECT fans out into a row per combination, the API hands the registry
    /// page two records with the same `id`, and its keyed list aborts mid-render
    /// — the symptom being a page stuck on "Loading models…" forever.
    #[test]
    fn a_second_created_date_does_not_duplicate_the_model() {
        let store = TripleStore::in_memory().unwrap();
        seed_one(&store, "gr", "2026-07-24T13:06:28Z");

        // Exactly what an interrupted re-register leaves behind: every other
        // triple is identical (RDF set semantics collapse them), only the fresh
        // timestamp survives alongside the original.
        store
            .update(&format!(
                r#"INSERT DATA {{ GRAPH <{REGISTRY_GRAPH}> {{
                     <{BASE}/data-model/gr> <{DCT}created> "2026-07-29T12:39:19Z"^^<{XSD}dateTime> .
                   }} }}"#
            ))
            .unwrap();

        let records = list_data_models(&store);
        assert_eq!(
            records.len(),
            1,
            "one registry entry must list once, got {:?}",
            records.iter().map(|r| &r.id).collect::<Vec<_>>()
        );
        assert_eq!(records[0].id, "gr");
    }

    /// The context listing resolves each model's latest PUBLISHED graph in one
    /// pass — that graph IRI is what lets the chat target the right graph for
    /// "what classes does model X define" instead of guessing an instance graph.
    #[test]
    fn context_listing_carries_the_published_graph() {
        let store = TripleStore::in_memory().unwrap();
        seed_one(&store, "gr", "2026-07-24T13:06:28Z");
        set_data_model_kind(&store, BASE, "gr", RegistryKind::Vocabulary).unwrap();

        // No published version yet: the entry lists with no graph.
        let entries = list_models_for_context(&store);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "Good Relations");
        assert!(entries[0].graph_iri.is_none());
        assert!(entries[0].is_public);

        let ver = DataModelVersion {
            data_model_id: "gr".into(),
            version: "1.2.0".into(),
            status: VersionStatus::Published,
            graph_iri: "http://purl.org/goodrelations/v1".into(),
            sub_graphs: Vec::new(),
            created_at: "2026-07-24T13:06:28Z".into(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: Vec::new(),
        };
        insert_version(&store, BASE, &ver).unwrap();
        update_latest_published(&store, BASE, "gr", "1.2.0").unwrap();

        let entries = list_models_for_context(&store);
        assert_eq!(entries.len(), 1, "still one entry per model");
        let e = &entries[0];
        assert_eq!(
            e.graph_iri.as_deref(),
            Some("http://purl.org/goodrelations/v1")
        );
        assert_eq!(e.version.as_deref(), Some("1.2.0"));
        assert_eq!(e.kind, RegistryKind::Vocabulary);
        assert_eq!(e.namespace, "http://purl.org/goodrelations/v1#");
        assert!(e.draft_graph_iri.is_none(), "no draft yet");

        // A newer draft rides along without displacing the published graph.
        let draft = DataModelVersion {
            data_model_id: "gr".into(),
            version: "1.3.0".into(),
            status: VersionStatus::Draft,
            graph_iri: "urn:draft:gr-1.3.0".into(),
            sub_graphs: Vec::new(),
            created_at: "2026-07-25T09:00:00Z".into(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: Vec::new(),
        };
        insert_version(&store, BASE, &draft).unwrap();
        update_latest_draft(&store, BASE, "gr", "1.3.0").unwrap();
        let e = &list_models_for_context(&store)[0];
        assert_eq!(
            e.graph_iri.as_deref(),
            Some("http://purl.org/goodrelations/v1"),
            "published stays"
        );
        assert_eq!(e.draft_graph_iri.as_deref(), Some("urn:draft:gr-1.3.0"));
        assert_eq!(e.draft_version.as_deref(), Some("1.3.0"));
    }

    /// The ordinary case keeps listing every distinct model.
    #[test]
    fn lists_each_distinct_model_once() {
        let store = TripleStore::in_memory().unwrap();
        seed_one(&store, "gr", "2026-07-24T13:06:28Z");
        seed_one(&store, "prov", "2026-07-24T13:06:29Z");

        let mut ids: Vec<String> = list_data_models(&store).into_iter().map(|r| r.id).collect();
        ids.sort();
        assert_eq!(ids, vec!["gr".to_string(), "prov".to_string()]);
    }
}
