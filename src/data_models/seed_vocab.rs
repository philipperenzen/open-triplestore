//! Seed the standard RDF vocabularies into the model/vocabulary registry as
//! **public, system-owned** entries, so OWL, RDF, RDFS, SKOS, DCAT, PROV, … are
//! browsable and queryable under `/api/models` out of the box — each published
//! under its **real** version (OWL 2.0, DCAT 3.0, PROV 2013-04-30, …) rather than
//! a synthetic placeholder, so the registry shows the actual version of every
//! standard it ships.
//!
//! The TTL sources are the same canonical files the web UI uses for term lookup
//! (`frontend/public/vocab/*.ttl`), embedded at compile time so the seed needs no
//! network or filesystem at runtime.
//!
//! Idempotent: skips any entry whose id already exists, so it never clobbers a
//! user-created model and never duplicates on restart. Best-effort — a failure on
//! one vocabulary is logged and skipped. Opt out with `SEED_STANDARD_VOCABS=false`.
//!
//! Installs seeded before real versioning shipped have these vocabularies pinned
//! at the old synthetic `1.0.0`; [`migrate_synthetic_versions`] re-seeds those
//! (system-owned only) at the correct version on the next boot.

use crate::data_models::models::{DataModelVersion, VersionStatus};
use crate::data_models::{registry, upload};
use crate::server::AppState;

/// The synthetic version every bundled vocabulary used to be seeded under, before
/// each got its real version. Used only by the one-time upgrade migration.
const LEGACY_SYNTHETIC_VERSION: &str = "1.0.0";

struct StdVocab {
    /// Registry id (also the IRI slug under `/data-model/{id}`).
    id: &'static str,
    title: &'static str,
    namespace: &'static str,
    /// The vocabulary's real published version, used as the registry version
    /// label and the `/version/{version}` graph-IRI segment (so it must be
    /// IRI-safe — no spaces, `/` or `#`). Taken from the file's own
    /// `owl:versionInfo`/`owl:versionIRI` where it declares one cleanly, else the
    /// canonical W3C/published version of that standard.
    version: &'static str,
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
        version: "1.1", // W3C Recommendation, RDF 1.1 (2014-02-25)
        ttl: include_str!("../../frontend/public/vocab/rdf.ttl"),
    },
    StdVocab {
        id: "rdfs",
        title: "RDF Schema 1.1",
        namespace: "http://www.w3.org/2000/01/rdf-schema#",
        version: "1.1", // W3C Recommendation, RDF Schema 1.1 (2014-02-25)
        ttl: include_str!("../../frontend/public/vocab/rdfs.ttl"),
    },
    StdVocab {
        id: "owl",
        title: "OWL 2",
        namespace: "http://www.w3.org/2002/07/owl#",
        version: "2.0", // OWL 2 (the file's own owl:versionInfo is a CVS $Date$ stamp)
        ttl: include_str!("../../frontend/public/vocab/owl.ttl"),
    },
    StdVocab {
        id: "xsd",
        title: "XML Schema Datatypes 1.1",
        namespace: "http://www.w3.org/2001/XMLSchema#",
        version: "1.1", // W3C Recommendation, XSD 1.1 (2012-04-05)
        ttl: include_str!("../../frontend/public/vocab/xsd.ttl"),
    },
    StdVocab {
        id: "skos",
        title: "SKOS",
        namespace: "http://www.w3.org/2004/02/skos/core#",
        version: "2009-08-18", // SKOS Reference, W3C Recommendation date
        ttl: include_str!("../../frontend/public/vocab/skos.ttl"),
    },
    StdVocab {
        id: "dcterms",
        title: "DCMI Metadata Terms",
        namespace: "http://purl.org/dc/terms/",
        version: "2020-01-20", // DCMI Metadata Terms, latest revision
        ttl: include_str!("../../frontend/public/vocab/dcterms.ttl"),
    },
    StdVocab {
        id: "dcat",
        title: "DCAT 3",
        namespace: "http://www.w3.org/ns/dcat#",
        version: "3.0", // the bundled file declares owl:versionIRI <…/dcat3>
        ttl: include_str!("../../frontend/public/vocab/dcat.ttl"),
    },
    StdVocab {
        id: "prov",
        title: "PROV-O",
        namespace: "http://www.w3.org/ns/prov#",
        version: "2013-04-30", // file owl:versionInfo "Recommendation version 2013-04-30"
        ttl: include_str!("../../frontend/public/vocab/prov.ttl"),
    },
    StdVocab {
        id: "foaf",
        title: "FOAF 0.99",
        namespace: "http://xmlns.com/foaf/0.1/",
        version: "0.99", // FOAF 0.99 "Paddington Edition", the last published version
        ttl: include_str!("../../frontend/public/vocab/foaf.ttl"),
    },
    StdVocab {
        id: "org",
        title: "Organization Ontology",
        namespace: "http://www.w3.org/ns/org#",
        version: "0.8", // file owl:versionInfo "0.8"
        ttl: include_str!("../../frontend/public/vocab/org.ttl"),
    },
    StdVocab {
        id: "qb",
        title: "RDF Data Cube",
        namespace: "http://purl.org/linked-data/cube#",
        version: "0.2", // file owl:versionInfo "0.2"
        ttl: include_str!("../../frontend/public/vocab/qb.ttl"),
    },
    StdVocab {
        id: "schema",
        title: "Schema.org",
        namespace: "https://schema.org/",
        // The bundled schema.org subset carries no version marker; this is its
        // nominal release label — adjust if a specific release was imported.
        version: "29.0",
        ttl: include_str!("../../frontend/public/vocab/schema.ttl"),
    },
    StdVocab {
        id: "shacl",
        title: "SHACL",
        namespace: "http://www.w3.org/ns/shacl#",
        version: "2017-07-20", // file header: "Version from 2017-07-20" (W3C Rec date)
        ttl: include_str!("../../frontend/public/vocab/shacl.ttl"),
    },
    StdVocab {
        id: "time",
        title: "OWL-Time",
        namespace: "http://www.w3.org/2006/time#",
        version: "2016", // file owl:versionIRI <http://www.w3.org/2006/time#2016>
        ttl: include_str!("../../frontend/public/vocab/time.ttl"),
    },
    StdVocab {
        id: "vann",
        title: "VANN",
        namespace: "http://purl.org/vocab/vann/",
        version: "1.1", // VANN 1.1
        ttl: include_str!("../../frontend/public/vocab/vann.ttl"),
    },
    StdVocab {
        id: "void",
        title: "VoID",
        namespace: "http://rdfs.org/ns/void#",
        version: "2011-03-06", // file owl:versionInfo "2011-03-06"
        ttl: include_str!("../../frontend/public/vocab/void.ttl"),
    },
    StdVocab {
        id: "geosparql",
        title: "GeoSPARQL",
        namespace: "http://www.opengis.net/ont/geosparql#",
        version: "1.1", // file owl:versionIRI :1.1 (OGC GeoSPARQL 1.1)
        ttl: include_str!("../../frontend/public/vocab/geosparql.ttl"),
    },
    StdVocab {
        id: "ots",
        title: "Open Triplestore Vocabulary",
        namespace: "https://opentriplestore.org/ns#",
        version: "1.0", // file owl:versionInfo "1.0"
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
    // Upgrade installs seeded under the old synthetic 1.0.0 before the loop runs,
    // so the loop recreates them at their real version.
    migrate_synthetic_versions(state);
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

/// One-time upgrade for installs seeded before real versioning: a bundled
/// vocabulary whose *only* version is the old synthetic `1.0.0` — and which is
/// still system-owned, i.e. untouched by an admin — is dropped (registry records
/// + its named graphs) so the seed loop recreates it under its real version.
///
/// Strictly scoped: it never touches a vocabulary an admin has claimed
/// (`owner_id`/`created_by` set) or versioned beyond the single synthetic
/// entry, nor any user-created model. A vocabulary already at its real version
/// has `len() == 1 && version == real`, so this no-ops on every later boot.
/// Returns the number upgraded.
fn migrate_synthetic_versions(state: &AppState) -> usize {
    let mut upgraded = 0usize;
    for v in VOCABS {
        if v.version == LEGACY_SYNTHETIC_VERSION {
            continue; // would be indistinguishable from a current entry
        }
        let Some(record) = registry::get_data_model(&state.store, &state.base_url, v.id) else {
            continue; // not seeded yet — the loop creates it correctly
        };
        // Only adopt system-owned entries (seeded with no owner / creator). An
        // admin who claimed or re-versioned the vocab keeps their copy untouched.
        let system_owned = record.owner_id.is_none() && record.created_by.is_none();
        let versions = registry::list_versions(&state.store, &state.base_url, v.id);
        let only_synthetic = versions.len() == 1 && versions[0].version == LEGACY_SYNTHETIC_VERSION;
        if !(system_owned && only_synthetic) {
            continue;
        }
        // Drop the old version's named graphs, then its registry records.
        let mut graphs: Vec<String> = Vec::new();
        for ver in &versions {
            graphs.push(ver.graph_iri.clone());
            graphs.extend(ver.sub_graphs.iter().cloned());
        }
        let refs: Vec<&str> = graphs.iter().map(|s| s.as_str()).collect();
        if let Err(e) = state.store.bulk_delete_graphs(&refs) {
            tracing::warn!("vocab upgrade '{}': graph cleanup failed: {e}", v.id);
            continue;
        }
        if let Err(e) = registry::delete_data_model(&state.store, &state.base_url, v.id) {
            tracing::warn!("vocab upgrade '{}': registry cleanup failed: {e}", v.id);
            continue;
        }
        upgraded += 1;
    }
    if upgraded > 0 {
        tracing::info!(
            "Upgraded {upgraded} bundled vocabularies from the synthetic 1.0.0 to their real version"
        );
    }
    upgraded
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

    // Load the already-parsed RDF into a published version stamped with the
    // vocabulary's real version (merged into one graph). Reuses the quads parsed
    // above instead of reparsing the TTL.
    let result = upload::load_parsed(
        &state.store,
        &state.base_url,
        v.id,
        Some(v.version),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::AppState;
    use crate::store::TripleStore;

    /// Every bundled vocabulary is published under its real version — never the
    /// old synthetic `1.0.0` — and the stale DCAT title is corrected.
    #[test]
    fn seeds_every_vocabulary_at_its_real_version() {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());
        let seeded = seed_standard_vocabularies(&state);
        assert_eq!(seeded, VOCABS.len(), "every bundled vocabulary seeded");

        for v in VOCABS {
            let rec = registry::get_data_model(&state.store, &state.base_url, v.id)
                .unwrap_or_else(|| panic!("vocabulary '{}' should be seeded", v.id));
            assert_eq!(
                rec.latest_published.as_deref(),
                Some(v.version),
                "vocabulary '{}' published at its curated real version",
                v.id
            );
            assert_ne!(
                rec.latest_published.as_deref(),
                Some(LEGACY_SYNTHETIC_VERSION),
                "vocabulary '{}' must not use the synthetic placeholder",
                v.id
            );
        }

        // The headline cases the request named, plus the corrected title.
        let owl = registry::get_data_model(&state.store, &state.base_url, "owl").unwrap();
        assert_eq!(owl.latest_published.as_deref(), Some("2.0"));
        let dcat = registry::get_data_model(&state.store, &state.base_url, "dcat").unwrap();
        assert_eq!(dcat.latest_published.as_deref(), Some("3.0"));
        assert_eq!(dcat.title, "DCAT 3", "stale \"DCAT 2\" title corrected");

        // Idempotent: a second pass seeds nothing new.
        assert_eq!(seed_standard_vocabularies(&state), 0);
    }

    /// An install seeded under the old synthetic `1.0.0` (system-owned) is upgraded
    /// in place to the vocabulary's real version on the next seed.
    #[test]
    fn upgrades_a_system_vocab_pinned_at_the_synthetic_version() {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());

        // Recreate an "old install": seed `owl` at the synthetic 1.0.0, system-owned.
        let owl = VOCABS.iter().find(|v| v.id == "owl").unwrap();
        let quads = upload::parse_rdf(owl.ttl.as_bytes(), "text/turtle", "owl.ttl").unwrap();
        registry::insert_data_model(
            &state.store,
            &state.base_url,
            owl.id,
            owl.title,
            owl.namespace,
            None,
            true,
            None,
            None,
            None,
            "2020-01-01T00:00:00+00:00",
        )
        .unwrap();
        let result = upload::load_parsed(
            &state.store,
            &state.base_url,
            owl.id,
            Some(LEGACY_SYNTHETIC_VERSION),
            quads,
            true,
        )
        .unwrap();
        let graph_iri = format!(
            "{}/data-model/{}/version/{}",
            state.base_url, owl.id, result.version
        );
        registry::insert_version(
            &state.store,
            &state.base_url,
            &DataModelVersion {
                data_model_id: owl.id.to_string(),
                version: result.version.clone(),
                status: VersionStatus::Published,
                graph_iri,
                sub_graphs: result.sub_graphs,
                created_at: "2020-01-01T00:00:00+00:00".to_string(),
                created_by: None,
                derived_from: None,
                notes: None,
                branch: None,
                sub_graph_status: Vec::new(),
            },
        )
        .unwrap();
        registry::update_latest_published(
            &state.store,
            &state.base_url,
            owl.id,
            LEGACY_SYNTHETIC_VERSION,
        )
        .unwrap();
        assert_eq!(
            registry::get_data_model(&state.store, &state.base_url, "owl")
                .unwrap()
                .latest_published
                .as_deref(),
            Some(LEGACY_SYNTHETIC_VERSION),
            "precondition: pinned at the synthetic version"
        );

        // Seeding migrates it to the real version and drops the synthetic one.
        seed_standard_vocabularies(&state);
        let owl_rec = registry::get_data_model(&state.store, &state.base_url, "owl").unwrap();
        assert_eq!(
            owl_rec.latest_published.as_deref(),
            Some("2.0"),
            "upgraded to its real version"
        );
        assert!(
            !registry::version_exists(
                &state.store,
                &state.base_url,
                "owl",
                LEGACY_SYNTHETIC_VERSION
            ),
            "synthetic 1.0.0 version removed"
        );
    }
}
