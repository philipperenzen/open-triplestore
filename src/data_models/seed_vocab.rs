//! Seed the standard RDF vocabularies into the model/vocabulary registry as
//! **public, system-owned** entries, so OWL, RDF, RDFS, SKOS, DCAT, PROV, … are
//! browsable and queryable under `/api/models` out of the box (each with a
//! published `1.0.0` version holding the actual RDF).
//!
//! The TTL sources are the same canonical files the web UI uses for term lookup
//! (`frontend/public/vocab/*.ttl`), embedded at compile time so the seed needs no
//! network or filesystem at runtime.
//!
//! Idempotent: skips any entry whose id already exists, so it never clobbers a
//! user-created model and never duplicates on restart. Best-effort — a failure on
//! one vocabulary is logged and skipped. Opt out with `SEED_STANDARD_VOCABS=false`.

use crate::data_models::models::{DataModelVersion, VersionStatus};
use crate::data_models::{registry, upload};
use crate::server::AppState;

const VERSION: &str = "1.0.0";

struct StdVocab {
    /// Registry id (also the IRI slug under `/data-model/{id}`).
    id: &'static str,
    title: &'static str,
    namespace: &'static str,
    ttl: &'static str,
}

/// The bundled standard vocabularies. `include_str!` paths are relative to this
/// file (`src/data_models/`), so they reach the shared web-UI vocab assets. The
/// Docker builder stage copies `frontend/public/vocab/` for this reason.
const VOCABS: &[StdVocab] = &[
    StdVocab {
        id: "rdf",
        title: "RDF 1.1",
        namespace: "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
        ttl: include_str!("../../frontend/public/vocab/rdf.ttl"),
    },
    StdVocab {
        id: "rdfs",
        title: "RDF Schema 1.1",
        namespace: "http://www.w3.org/2000/01/rdf-schema#",
        ttl: include_str!("../../frontend/public/vocab/rdfs.ttl"),
    },
    StdVocab {
        id: "owl",
        title: "OWL 2",
        namespace: "http://www.w3.org/2002/07/owl#",
        ttl: include_str!("../../frontend/public/vocab/owl.ttl"),
    },
    StdVocab {
        id: "xsd",
        title: "XML Schema Datatypes",
        namespace: "http://www.w3.org/2001/XMLSchema#",
        ttl: include_str!("../../frontend/public/vocab/xsd.ttl"),
    },
    StdVocab {
        id: "skos",
        title: "SKOS",
        namespace: "http://www.w3.org/2004/02/skos/core#",
        ttl: include_str!("../../frontend/public/vocab/skos.ttl"),
    },
    StdVocab {
        id: "dcterms",
        title: "DCMI Metadata Terms",
        namespace: "http://purl.org/dc/terms/",
        ttl: include_str!("../../frontend/public/vocab/dcterms.ttl"),
    },
    StdVocab {
        id: "dcat",
        title: "DCAT 2",
        namespace: "http://www.w3.org/ns/dcat#",
        ttl: include_str!("../../frontend/public/vocab/dcat.ttl"),
    },
    StdVocab {
        id: "prov",
        title: "PROV-O",
        namespace: "http://www.w3.org/ns/prov#",
        ttl: include_str!("../../frontend/public/vocab/prov.ttl"),
    },
    StdVocab {
        id: "foaf",
        title: "FOAF",
        namespace: "http://xmlns.com/foaf/0.1/",
        ttl: include_str!("../../frontend/public/vocab/foaf.ttl"),
    },
    StdVocab {
        id: "org",
        title: "Organization Ontology",
        namespace: "http://www.w3.org/ns/org#",
        ttl: include_str!("../../frontend/public/vocab/org.ttl"),
    },
    StdVocab {
        id: "qb",
        title: "RDF Data Cube",
        namespace: "http://purl.org/linked-data/cube#",
        ttl: include_str!("../../frontend/public/vocab/qb.ttl"),
    },
    StdVocab {
        id: "schema",
        title: "Schema.org",
        namespace: "https://schema.org/",
        ttl: include_str!("../../frontend/public/vocab/schema.ttl"),
    },
    StdVocab {
        id: "shacl",
        title: "SHACL",
        namespace: "http://www.w3.org/ns/shacl#",
        ttl: include_str!("../../frontend/public/vocab/shacl.ttl"),
    },
    StdVocab {
        id: "time",
        title: "OWL-Time",
        namespace: "http://www.w3.org/2006/time#",
        ttl: include_str!("../../frontend/public/vocab/time.ttl"),
    },
    StdVocab {
        id: "vann",
        title: "VANN",
        namespace: "http://purl.org/vocab/vann/",
        ttl: include_str!("../../frontend/public/vocab/vann.ttl"),
    },
    StdVocab {
        id: "void",
        title: "VoID",
        namespace: "http://rdfs.org/ns/void#",
        ttl: include_str!("../../frontend/public/vocab/void.ttl"),
    },
    StdVocab {
        id: "geosparql",
        title: "GeoSPARQL",
        namespace: "http://www.opengis.net/ont/geosparql#",
        ttl: include_str!("../../frontend/public/vocab/geosparql.ttl"),
    },
    StdVocab {
        id: "ots",
        title: "Open Triplestore Vocabulary",
        namespace: "https://opentriplestore.org/ns#",
        ttl: include_str!("../../frontend/public/vocab/ots.ttl"),
    },
];

/// Seed every standard vocabulary that isn't already in the registry. Returns the
/// number of newly-seeded entries (0 once everything is already present, which is
/// the idempotent steady state).
pub fn seed_standard_vocabularies(state: &AppState) -> usize {
    let disabled = std::env::var("SEED_STANDARD_VOCABS")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off"
            )
        })
        .unwrap_or(false);
    if disabled {
        return 0;
    }
    let mut seeded = 0usize;
    for v in VOCABS {
        match seed_one(state, v) {
            Ok(true) => seeded += 1,
            Ok(false) => {}
            Err(e) => tracing::warn!("vocabulary seed '{}' skipped: {e}", v.id),
        }
    }
    if seeded > 0 {
        tracing::info!("Seeded {seeded} standard vocabularies into the model registry");
    }
    seeded
}

/// Returns `Ok(true)` if a new entry was created, `Ok(false)` if it already
/// existed (idempotent skip).
fn seed_one(state: &AppState, v: &StdVocab) -> anyhow::Result<bool> {
    if registry::data_model_exists(&state.store, &state.base_url, v.id) {
        return Ok(false);
    }

    // Parse the TTL once and reuse the quads for both kind detection and loading —
    // `parse_and_load` would otherwise reparse the same bytes a second time.
    let quads = upload::parse_rdf(v.ttl.as_bytes(), "text/turtle", "vocab.ttl")
        .map_err(|e| anyhow::anyhow!("parse: {e}"))?;
    let detected = crate::kind_detector::detect(&quads);
    let now = chrono::Utc::now().to_rfc3339();

    registry::insert_data_model(
        &state.store,
        &state.base_url,
        v.id,
        v.title,
        v.namespace,
        Some("Bundled standard vocabulary, seeded as a public reference."),
        true, // public
        None, // system-owned
        None,
        None,
        &now,
    )?;

    // Load the already-parsed RDF into a published 1.0.0 version (merged into one
    // graph). Reuses the quads parsed above instead of reparsing the TTL.
    let result = upload::load_parsed(
        &state.store,
        &state.base_url,
        v.id,
        Some(VERSION),
        quads,
        true,
    )
    .map_err(|e| anyhow::anyhow!("load: {e}"))?;

    let graph_iri = format!(
        "{}/data-model/{}/version/{}",
        state.base_url, v.id, result.version
    );
    let record = DataModelVersion {
        data_model_id: v.id.to_string(),
        version: result.version.clone(),
        status: VersionStatus::Published,
        graph_iri,
        sub_graphs: result.sub_graphs,
        created_at: now,
        created_by: None,
        derived_from: None,
        notes: None,
        branch: None,
        sub_graph_status: Vec::new(),
    };
    registry::insert_version(&state.store, &state.base_url, &record)?;
    if let Some(kind) = detected.primary {
        registry::set_data_model_kind(&state.store, &state.base_url, v.id, kind)?;
    }
    registry::update_latest_published(&state.store, &state.base_url, v.id, &result.version)?;
    Ok(true)
}
