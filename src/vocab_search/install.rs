//! Offline vocabulary install — copy a vocabulary from the bundled LOV
//! corpus into the model/vocabulary registry as a first-class entry.
//!
//! Mirrors the standard-vocabulary seeder (`seed_vocab::seed_one`): registry
//! header + one published version whose named graph holds the vocabulary's
//! triples, kind auto-detected.  No network is involved — the triples come
//! from the local `lov.nq.gz`.

use serde::Serialize;

use crate::data_models::models::{DataModelVersion, VersionStatus};
use crate::data_models::{registry, upload};
use crate::kind_detector;
use crate::store::TripleStore;

use super::catalog::VocabCatalog;
use super::corpus;

#[derive(Debug, Serialize)]
pub struct InstallOutcome {
    pub model_id: String,
    pub version: String,
    pub triples: usize,
    pub kind: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("Unknown vocabulary {0:?}")]
    UnknownVocab(String),
    #[error("Vocabulary {0:?} is not present in the bundled corpus")]
    NotInCorpus(String),
    #[error("The LOV corpus file is not available on this instance")]
    CorpusUnavailable,
    #[error("A registry entry for {0:?} already exists")]
    AlreadyInstalled(String),
    #[error("{0}")]
    Internal(String),
}

/// Derive an IRI-safe, slug-like version label from LOV version metadata.
fn version_label(name: Option<&str>, issued: Option<&str>) -> String {
    let candidate = name
        .map(|n| n.trim().trim_start_matches('v').to_string())
        .filter(|n| {
            !n.is_empty()
                && n.chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
        })
        .or_else(|| issued.map(str::to_string))
        .unwrap_or_else(|| "lov".to_string());
    candidate
}

/// Install `prefix` from the corpus at `corpus_path` into the registry.
pub fn install_lov_vocab(
    store: &TripleStore,
    base_url: &str,
    catalog: &VocabCatalog,
    corpus_path: Option<&std::path::Path>,
    prefix: &str,
    installed_by: Option<&str>,
) -> Result<InstallOutcome, InstallError> {
    // Installs run behind the admin gate: visibility checks don't apply.
    let see_all = |_: &super::catalog::PlatformVocab| true;
    let vocab = catalog
        .lov_by_prefix(prefix)
        .or_else(|| {
            catalog.info(prefix, &see_all).and_then(|e| {
                // Allow lookup by URI/namespace too.
                let p = e.prefix.clone();
                catalog.lov_by_prefix(&p)
            })
        })
        .ok_or_else(|| InstallError::UnknownVocab(prefix.to_string()))?
        .clone();
    if vocab.graph_quads == 0 {
        return Err(InstallError::NotInCorpus(prefix.to_string()));
    }
    let corpus_path = corpus_path.ok_or(InstallError::CorpusUnavailable)?;

    // Registry id: the LOV prefix, lowercased (registry ids are slugs).
    let model_id = vocab.prefix.to_lowercase();
    if registry::data_model_exists(store, base_url, &model_id) {
        return Err(InstallError::AlreadyInstalled(model_id));
    }
    // A different id already claiming the namespace also counts as installed.
    if let Some(existing) = catalog.info(&vocab.nsp, &see_all) {
        if existing.source == "platform" {
            return Err(InstallError::AlreadyInstalled(
                existing.model_id.unwrap_or(model_id),
            ));
        }
    }

    let quads = corpus::extract_vocab_quads(corpus_path, &vocab.uri)
        .map_err(|e| InstallError::Internal(format!("corpus read failed: {e}")))?;
    if quads.is_empty() {
        return Err(InstallError::NotInCorpus(prefix.to_string()));
    }
    let triples = quads.len();

    let latest = vocab.versions.first();
    let version = version_label(
        latest.and_then(|v| v.name.as_deref()),
        latest.and_then(|v| v.issued.as_deref()),
    );

    let title = vocab
        .titles
        .first()
        .map(|t| t.value.clone())
        .unwrap_or_else(|| vocab.prefix.clone());
    let description = vocab.descriptions.first().map(|d| d.value.clone());
    let now = chrono::Utc::now().to_rfc3339();

    registry::insert_data_model(
        store,
        base_url,
        &model_id,
        &title,
        &vocab.nsp,
        description.as_deref(),
        true, // public reference vocabulary
        None,
        None,
        installed_by,
        &now,
    )
    .map_err(|e| InstallError::Internal(format!("registry insert failed: {e}")))?;

    // Any failure past this point rolls the registry entry (and loaded
    // graphs) back — a half-installed entry would otherwise block reinstall
    // forever via the data_model_exists guard above.
    let rollback = |graphs: &[String], reason: String| -> InstallError {
        let refs: Vec<&str> = graphs.iter().map(String::as_str).collect();
        if let Err(e) = store.bulk_delete_graphs(&refs) {
            tracing::warn!("install rollback: graph cleanup failed for {model_id}: {e}");
        }
        if let Err(e) = registry::delete_data_model(store, base_url, &model_id) {
            tracing::warn!("install rollback: registry cleanup failed for {model_id}: {e}");
        }
        InstallError::Internal(reason)
    };

    let kind = kind_detector::detect(&quads).primary;

    let result = match upload::load_parsed(store, base_url, &model_id, Some(&version), quads, true)
    {
        Ok(r) => r,
        Err(e) => return Err(rollback(&[], format!("graph load failed: {e}"))),
    };

    let graph_iri = format!(
        "{}/data-model/{}/version/{}",
        base_url, model_id, result.version
    );
    let mut loaded_graphs = vec![graph_iri.clone()];
    loaded_graphs.extend(result.sub_graphs.iter().cloned());
    let record = DataModelVersion {
        data_model_id: model_id.clone(),
        version: result.version.clone(),
        status: VersionStatus::Published,
        graph_iri,
        sub_graphs: result.sub_graphs,
        created_at: latest
            .and_then(|v| v.issued.as_deref())
            .map(|d| format!("{d}T00:00:00Z"))
            .unwrap_or_else(|| now.clone()),
        created_by: installed_by.map(str::to_string),
        derived_from: None,
        notes: Some(format!(
            "Installed from the bundled LOV corpus (snapshot {}, CC BY 4.0).",
            catalog.source().snapshot_date
        )),
        branch: None,
        sub_graph_status: Vec::new(),
    };
    if let Err(e) = registry::insert_version(store, base_url, &record) {
        return Err(rollback(
            &loaded_graphs,
            format!("version insert failed: {e}"),
        ));
    }

    if let Some(k) = kind {
        if let Err(e) = registry::set_data_model_kind(store, base_url, &model_id, k) {
            return Err(rollback(&loaded_graphs, format!("kind update failed: {e}")));
        }
    }
    if let Err(e) = registry::update_latest_published(store, base_url, &model_id, &result.version) {
        return Err(rollback(
            &loaded_graphs,
            format!("latest update failed: {e}"),
        ));
    }

    Ok(InstallOutcome {
        model_id,
        version: result.version,
        triples,
        kind: kind
            .map(|k| format!("{k:?}").to_lowercase())
            .unwrap_or_else(|| "unknown".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_from_corpus_fixture() {
        use std::io::Write;
        let catalog = VocabCatalog::bundled();
        let store = TripleStore::in_memory().unwrap();
        let base = "http://localhost:7878";

        // Minimal corpus: two quads in the real GoodRelations graph.
        let nq = b"<http://purl.org/goodrelations/v1#Offering> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> <http://purl.org/goodrelations/v1> .\n<http://purl.org/goodrelations/v1#Offering> <http://www.w3.org/2000/01/rdf-schema#label> \"Offering\"@en <http://purl.org/goodrelations/v1> .\n";
        let dir = std::env::temp_dir().join("ots-install-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mini.nq.gz");
        let mut enc = flate2::write::GzEncoder::new(
            std::fs::File::create(&path).unwrap(),
            Default::default(),
        );
        enc.write_all(nq).unwrap();
        enc.finish().unwrap();

        // Creator passed as an IRI, matching the registry convention.
        let outcome = install_lov_vocab(
            &store,
            base,
            &catalog,
            Some(&path),
            "gr",
            Some("http://localhost:7878/users/admin-1"),
        )
        .expect("install succeeds");
        assert_eq!(outcome.model_id, "gr");
        assert_eq!(outcome.triples, 2);
        assert!(registry::data_model_exists(&store, base, "gr"));
        let versions = registry::list_versions(&store, base, "gr");
        assert_eq!(versions.len(), 1);

        // Re-install is a conflict, not a duplicate.
        let again = install_lov_vocab(&store, base, &catalog, Some(&path), "gr", None);
        assert!(matches!(again, Err(InstallError::AlreadyInstalled(_))));
    }

    #[test]
    fn version_label_sanitizes() {
        assert_eq!(version_label(Some("v2.3"), None), "2.3");
        assert_eq!(version_label(Some("v2015-07-22"), None), "2015-07-22");
        // "W3C Recommendation" contains spaces → falls back to the issued date.
        assert_eq!(
            version_label(Some("W3C Recommendation"), Some("2016-11-12")),
            "2016-11-12"
        );
        assert_eq!(version_label(None, None), "lov");
    }
}
