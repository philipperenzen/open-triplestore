//! Internal vocabulary search & recommendation service (LOV replacement).
//!
//! Public LOV (lov.linkeddata.es) is frequently unreachable and prefix.cc's
//! availability is equally shaky, so this module brings both in-house:
//!
//! * [`catalog`] — the embedded LOV vocabulary catalog (~900 vocabularies:
//!   metadata, tags, versions, VOAF reuse metrics) overlaid with this
//!   instance's public model/vocabulary registry.  Always available.
//! * [`corpus`] — term extraction from the LOV N-Quads corpus (the Docker
//!   image bundles the vocabularies this project may redistribute; the full
//!   dump can be downloaded once, sha256-pinned, or mounted, but only its
//!   redistributable vocabularies are term-indexed) and from platform
//!   registry version graphs.
//! * [`index`] (feature `vocab-search`) — the Tantivy term search engine
//!   with LOV's ranking formula plus an instance-local usage signal.
//! * [`recommend`] (feature `vocab-search`) — CLARIAH-style vocabulary
//!   recommender with combiSQORE homogenization.
//! * [`install`] — offline "install this vocabulary" into the registry.
//! * [`routes`] — the `/api/vocab/*` HTTP surface (LOV API v2 envelopes).
//!
//! The prefix half of the service lives in [`crate::prefixes`].
//!
//! # Freshness model
//!
//! Registry-derived state (catalog overlay, platform prefixes, platform term
//! index, local usage metrics) is rebuilt when `AppState::
//! mark_vocab_registry_dirty` has been called — checked at the top of every
//! vocab route, rebuilt off the request path via `spawn_blocking`.  The LOV
//! index is built once per corpus snapshot in a boot background task and
//! reopened instantly on warm boots.

pub mod catalog;
pub mod corpus;
pub mod install;
pub mod local_metrics;
pub mod routes;

#[cfg(feature = "vocab-search")]
pub mod index;
#[cfg(feature = "vocab-search")]
pub mod recommend;

use crate::server::AppState;
use crate::store::replication::Role;
use crate::store::TripleStore;

/// How often a replica looks for registry changes that arrived by
/// replication, and a Raft member that leads checks earlier LOV installs.
const REPLICA_WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Bring the LOV installs of earlier releases in line
/// ([`install::migrate_legacy_installs`]), when this node's store may be
/// written here (not a replication follower, not a Raft member that does not
/// lead: those get the leader's result by replication).  After a change the
/// registry-derived state is marked stale.  Whether anything changed.
/// Blocking.
fn check_earlier_lov_installs(state: &AppState) -> bool {
    if state.store.replication().read_only() {
        return false;
    }
    let auth = &state.auth_db;
    // Only admins could install: a creator this instance knows as a user who
    // is not an admin did not.
    let creator_is_non_admin =
        |user_id: &str| matches!(auth.get_user_by_id(user_id), Ok(Some(u)) if !u.role.is_admin());
    let done = install::migrate_legacy_installs(
        &state.store,
        &state.base_url,
        &state.vocab_catalog,
        &creator_is_non_admin,
    );
    if done.is_empty() {
        return false;
    }
    let private = done.iter().filter(|d| d.made_private).count();
    let recorded = done.iter().filter(|d| d.recorded_licence).count();
    tracing::info!(
        "vocab-search: LOV installs of earlier releases: {private} made private, {recorded} \
         licence record(s) written; their graphs and notes are left as they are"
    );
    if private > 0 {
        state.auth_db.invalidate_accessible_graphs_cache();
    }
    state.mark_vocab_registry_dirty();
    true
}

/// A digest of what the registry-derived vocab state is built from: each
/// registry entry with its visibility, namespace, title and latest published
/// version, and that version's licence record (whether its content is a
/// checked, unchanged copy decides whether a no-derivatives entry is indexed;
/// see `servable_to_everyone`).  `None` when the registry cannot be read.
fn registry_fingerprint(store: &TripleStore) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    // The attribution OPTIONAL sits inside the latestPublished one: an entry
    // without a latest version must not join with every attributed record.
    let q = format!(
        "SELECT ?m ?public ?ns ?title ?latest ?a WHERE {{ GRAPH <{}> {{ \
           ?m a <urn:system:vocab/DataModel> . \
           OPTIONAL {{ ?m <urn:system:vocab/isPublic> ?public }} \
           OPTIONAL {{ ?m <urn:system:vocab/namespace> ?ns }} \
           OPTIONAL {{ ?m <http://purl.org/dc/terms/title> ?title }} \
           OPTIONAL {{ ?m <urn:system:vocab/latestPublished> ?latest . \
                      OPTIONAL {{ ?latest <urn:system:vocab/attribution> ?a }} }} \
         }} }}",
        crate::data_models::registry::REGISTRY_GRAPH
    );
    let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&q) else {
        return None;
    };
    let mut rows = Vec::new();
    for solution in solutions {
        let solution = solution.ok()?;
        let row: Vec<String> = solution
            .iter()
            .map(|(var, term)| format!("{var}={term}"))
            .collect();
        rows.push(row.join(" "));
    }
    rows.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    rows.hash(&mut h);
    Some(h.finish())
}

/// On a replica (a follower, or a member of a Raft cluster), every
/// [`REPLICA_WATCH_INTERVAL`]:
///
/// * a cluster member that leads now checks earlier LOV installs, so the
///   check is done even when no member led at its boot (a rolling upgrade);
/// * a node whose store is read-only compares [`registry_fingerprint`] with
///   the last one and, when the registry changed by replication (the leader
///   made an entry private, say), marks the registry-derived state stale, so
///   the next vocab request rebuilds it.  A leader's own writes mark it
///   through the API already.
fn spawn_replica_watch(state: AppState, mut last: Option<u64>) {
    tokio::spawn(async move {
        // Read-only at the last look: a change seen since may have arrived by
        // replication even when this member leads now.
        let mut was_read_only = state.store.replication().read_only();
        let mut tick = tokio::time::interval(REPLICA_WATCH_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await; // immediate: the boot has just done both
        loop {
            tick.tick().await;
            let s = state.clone();
            let checked = tokio::task::spawn_blocking(move || {
                let replication = s.store.replication();
                if replication.role() == Role::Cluster {
                    check_earlier_lov_installs(&s);
                }
                (replication.read_only(), registry_fingerprint(&s.store))
            })
            .await;
            let Ok((read_only, fingerprint)) = checked else {
                tracing::warn!("vocab-search: the replica registry watch panicked");
                continue;
            };
            if let (Some(before), Some(now)) = (last, fingerprint) {
                if before != now && (read_only || was_read_only) {
                    state.mark_vocab_registry_dirty();
                }
            }
            if fingerprint.is_some() {
                last = fingerprint;
            }
            was_read_only = read_only;
        }
    });
}

/// Rebuild everything derived from the model/vocabulary registry.
///
/// Blocking (SPARQL over the registry + optional index build) — call from
/// `spawn_blocking` or a boot task.
pub fn refresh_platform_state(state: &AppState) {
    let records = crate::data_models::registry::list_data_models(&state.store);
    state.vocab_catalog.set_platform_records(&records);
    state
        .prefix_registry
        .set_platform_prefixes(state.vocab_catalog.platform_prefix_pairs());

    #[cfg(feature = "vocab-search")]
    if let Some(engine) = &state.vocab_engine {
        let public = servable_to_everyone(&state.store, &state.base_url, records);
        let (docs, stats) = corpus::extract_platform_terms(
            &state.store,
            &state.base_url,
            &public,
            &state.vocab_catalog,
        );
        engine.set_platform_docs(&docs, stats);
        // Local-usage ranking feeds anonymous search: only publicly
        // accessible graphs may contribute.
        let public_graphs = state
            .auth_db
            .get_accessible_graph_iris_cached(None)
            .map(|a| a.0.clone())
            .unwrap_or_default();
        engine.set_local_usage(local_metrics::compute_local_usage(
            &state.store,
            &state.base_url,
            &public_graphs,
        ));
    }
}

/// The registry entries whose term text the platform index may serve to
/// anyone: the public ones, less those holding content whose licence allows
/// no altered copies (IMBOR, a no-derivatives LOV install or seed-bundle
/// model) whose latest published version is not a checked, unchanged copy of
/// it. That is the rule `data_models::handlers::ensure_servable` applies to an
/// anonymous caller, so search and autocomplete never serve labels, comments
/// or definitions the version downloads withhold: an altered copy, a legacy
/// LOV install an admin made public again, a version an earlier release let
/// someone make in such an entry.
#[cfg(feature = "vocab-search")]
fn servable_to_everyone(
    store: &TripleStore,
    base_url: &str,
    records: Vec<crate::data_models::models::DataModelRecord>,
) -> Vec<crate::data_models::models::DataModelRecord> {
    use crate::data_models::registry;
    let latest = registry::latest_published_attributions(store);
    records
        .into_iter()
        .filter(|r| r.is_public)
        .filter(|r| {
            registry::no_derivatives_attribution(store, base_url, &r.id).is_none()
                || latest
                    .get(&r.id)
                    .is_some_and(|a| a.no_derivatives && a.unchanged)
        })
        .collect()
}

/// Boot task: locate (or download) the LOV corpus, then build/reopen the LOV
/// term index in the background.  Also does the initial platform refresh so
/// the service is warm right after seeding.
pub async fn boot_vocab_search(state: AppState, data_dir: std::path::PathBuf) {
    // Corpus discovery/download (async, network only when configured).
    let corpus_path = corpus::ensure_corpus(&data_dir).await;
    if let Ok(mut guard) = state.vocab_corpus.write() {
        guard.clone_from(&corpus_path);
    }

    // Which vocabularies the corpus holds decides what is installable: the
    // image's corpus carries only the redistributable ones, a full dump all
    // (those that may not be redistributed install privately and stay out of
    // the term index).
    // Scanned alongside the rest of boot; until it lands nothing shows as
    // installable.
    if let Some(path) = corpus_path.clone() {
        let catalog = state.vocab_catalog.clone();
        tokio::task::spawn_blocking(move || match corpus::corpus_graphs(&path) {
            Ok(graphs) => {
                tracing::info!("vocab-search: corpus holds {} graphs", graphs.len());
                catalog.set_corpus_graphs(Some(graphs));
            }
            Err(e) => tracing::warn!("vocab-search: cannot scan corpus graphs: {e}"),
        });
    }

    // LOV installs made by earlier releases: public whatever their licence,
    // an owl:versionInfo triple added to their graph, LOV's CC BY 4.0 noted
    // as their licence.  Checked at every boot (one query once nothing is
    // left to do), before the registry-derived state below is first built,
    // so an entry made private here never reaches the platform term index.
    // Needs no corpus.  Only where this node may write.
    let migrated = {
        let state_bg = state.clone();
        match tokio::task::spawn_blocking(move || check_earlier_lov_installs(&state_bg)).await {
            Ok(changed) => changed,
            Err(_) => {
                tracing::warn!("vocab-search: the check of earlier LOV installs panicked");
                false
            }
        }
    };

    // A replica's registry changes arrive by replication, which marks nothing
    // stale here: watched below, from the registry as the first refresh reads
    // it.
    let replica = matches!(
        state.store.replication().role(),
        Role::Follower | Role::Cluster
    );

    // Initial refresh of everything derived from the registry — runs on every
    // build (feature on or off, engine present or not).
    let mut baseline = None;
    {
        let state_bg = state.clone();
        let refreshed = tokio::task::spawn_blocking(move || {
            state_bg
                .vocab_registry_dirty
                .store(false, std::sync::atomic::Ordering::Relaxed);
            let fingerprint = replica
                .then(|| registry_fingerprint(&state_bg.store))
                .flatten();
            refresh_platform_state(&state_bg);
            fingerprint
        })
        .await;
        match refreshed {
            Ok(fingerprint) => baseline = fingerprint,
            Err(_) => {
                tracing::warn!("vocab-search: initial platform refresh panicked");
                state
                    .vocab_registry_dirty
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
    // A request may have started a refresh from the registry as it was before
    // the check above, and finish after the refresh above: rebuild once more.
    if migrated {
        state.mark_vocab_registry_dirty();
    }
    if replica {
        spawn_replica_watch(state.clone(), baseline);
    }

    #[cfg(feature = "vocab-search")]
    if let Some(engine) = state.vocab_engine.clone() {
        engine.set_corpus_available(corpus_path.is_some());
        let state_bg = state.clone();
        let build = tokio::task::spawn_blocking(move || {
            // LOV index: reopen when this snapshot finished indexing before
            // (completion marker present), otherwise extract + build.
            let Some(path) = corpus_path else {
                tracing::info!(
                    "vocab-search: no LOV corpus — term search covers platform vocabularies only"
                );
                // Indexes of an earlier schema version are never opened again.
                if let Err(e) = engine.remove_stale_lov_indexes(None) {
                    tracing::warn!("vocab-search: cannot remove superseded LOV indexes: {e}");
                }
                return;
            };
            let sha = match index::file_sha256(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("vocab-search: cannot hash corpus: {e}");
                    return;
                }
            };
            let dir = engine.lov_index_dir(&sha, &state_bg.vocab_catalog.redistributable_digest());
            // Indexes of another corpus, catalogue policy or schema version
            // are never opened again; an earlier one may hold vocabularies
            // this build may not serve.  Removed once this one is in use.
            let remove_stale = || match engine.remove_stale_lov_indexes(Some(&dir)) {
                Ok(0) => {}
                Ok(n) => tracing::info!("vocab-search: removed {n} superseded LOV term index(es)"),
                Err(e) => tracing::warn!("vocab-search: cannot remove superseded LOV indexes: {e}"),
            };
            if index::VocabSearchEngine::lov_stats_marker(&dir).exists() {
                match engine.reopen_lov_index(&dir) {
                    Ok(()) => {
                        tracing::info!("vocab-search: reopened LOV term index at {:?}", dir);
                        remove_stale();
                        return;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "vocab-search: reopen failed ({e}); rebuilding index at {:?}",
                            dir
                        );
                    }
                }
            }
            // No marker (or unusable index): rebuild from scratch.
            let _ = std::fs::remove_dir_all(&dir);
            let started = std::time::Instant::now();
            match corpus::extract_lov_terms(&path, &state_bg.vocab_catalog) {
                Ok((docs, stats)) => {
                    tracing::info!(
                        "vocab-search: extracted {} terms from {} vocabularies ({} instances dropped; \
                         {} vocabularies in the corpus not indexed because they may not be \
                         redistributed) in {:?}",
                        stats.terms,
                        stats.vocabularies,
                        stats.instances_dropped,
                        stats.not_redistributable,
                        started.elapsed()
                    );
                    if let Err(e) = engine.set_lov_index_from_docs(&dir, &docs, stats) {
                        tracing::warn!("vocab-search: LOV index build failed: {e}");
                    } else {
                        tracing::info!(
                            "vocab-search: LOV term index ready in {:?}",
                            started.elapsed()
                        );
                        remove_stale();
                    }
                }
                Err(e) => tracing::warn!("vocab-search: corpus extraction failed: {e}"),
            }
        });
        if build.await.is_err() {
            tracing::warn!("vocab-search: boot build task panicked");
        }
    }
}

#[cfg(all(test, feature = "vocab-search"))]
mod tests {
    use super::*;
    use crate::data_models::models::{ContentAttribution, DataModelVersion, VersionStatus};
    use crate::data_models::{registry, seed_vocab, vocab_files};

    fn state() -> AppState {
        AppState::test_default_with_store(TripleStore::in_memory().unwrap())
    }

    /// The registry models whose terms the platform index gets, as
    /// `refresh_platform_state` extracts them.
    fn indexed_models(state: &AppState) -> std::collections::BTreeSet<String> {
        let records = registry::list_data_models(&state.store);
        let public = servable_to_everyone(&state.store, &state.base_url, records);
        let (docs, _) = corpus::extract_platform_terms(
            &state.store,
            &state.base_url,
            &public,
            &state.vocab_catalog,
        );
        docs.into_iter().map(|d| d.model_id).collect()
    }

    fn mark(state: &AppState, id: &str, version: &str) {
        registry::mark_possibly_modified(
            &state.store,
            &registry::version_record_iri(&state.base_url, id, version),
            vocab_files::written_stored_copy,
        )
        .unwrap();
    }

    /// IMBOR's terms are served by search only while its latest version is a
    /// checked, unchanged copy; once a re-check finds it altered they are
    /// not, as its downloads are withheld. A replica sees the flip in the
    /// registry fingerprint. A CC BY copy marked the same way stays indexed.
    #[test]
    fn no_derivatives_terms_are_indexed_only_while_the_copy_is_unchanged() {
        let state = state();
        seed_vocab::seed_standard_vocabularies(&state);
        let models = indexed_models(&state);
        assert!(models.contains("imbor"), "{models:?}");
        assert!(models.contains("bot"), "{models:?}");

        let before = registry_fingerprint(&state.store);
        mark(&state, "imbor", "2025");
        assert_ne!(registry_fingerprint(&state.store), before);
        let models = indexed_models(&state);
        assert!(!models.contains("imbor"), "{models:?}");
        assert!(models.contains("bot"));

        mark(&state, "bot", "0.3.2");
        assert!(indexed_models(&state).contains("bot"));
    }

    /// A LOV install of an earlier release whose licence allows no altered
    /// copies was made private with a record calling it not unchanged; an
    /// admin makes the entry public again. Its terms stay out of the index.
    #[test]
    fn a_withheld_no_derivatives_entry_made_public_again_is_not_indexed() {
        let state = state();
        let base = state.base_url.to_string();
        let now = "2026-01-01T00:00:00Z";
        registry::insert_data_model(
            &state.store,
            &base,
            "pna",
            "PNA",
            "http://example.org/pna#",
            None,
            true,
            None,
            None,
            None,
            now,
        )
        .unwrap();
        let graph = registry::version_record_iri(&base, "pna", "1.0");
        state
            .store
            .graph_store_put(
                Some(&graph),
                "<http://example.org/pna#Thing> a <http://www.w3.org/2002/07/owl#Class> ; \
                 <http://www.w3.org/2000/01/rdf-schema#label> \"Thing\" .",
                oxigraph::io::RdfFormat::Turtle,
            )
            .unwrap();
        registry::insert_version(
            &state.store,
            &base,
            &DataModelVersion {
                data_model_id: "pna".into(),
                version: "1.0".into(),
                status: VersionStatus::Published,
                graph_iri: graph.clone(),
                sub_graphs: vec![],
                created_at: now.into(),
                created_by: None,
                derived_from: None,
                notes: None,
                branch: None,
                sub_graph_status: vec![],
            },
        )
        .unwrap();
        registry::update_latest_published(&state.store, &base, "pna", "1.0").unwrap();
        let record = |unchanged| ContentAttribution {
            file: "LOV corpus graph".into(),
            licenses: vec![],
            copyright: vec![],
            notice: None,
            status: None,
            source_url: "https://lov.linkeddata.es/".into(),
            specification_url: None,
            changes: None,
            stored_copy: "An earlier release's copy.".into(),
            unchanged,
            remarks: None,
            no_derivatives: true,
            header: None,
            notice_url: "https://example.org/notice".into(),
        };
        registry::set_attribution(&state.store, &graph, Some(&record(false))).unwrap();
        assert!(!indexed_models(&state).contains("pna"));

        // The same entry holding a checked, unchanged copy is indexed.
        registry::set_attribution(&state.store, &graph, Some(&record(true))).unwrap();
        assert!(indexed_models(&state).contains("pna"));
    }

    /// Through the engine: a refresh after the re-check drops the altered
    /// no-derivatives vocabulary from the platform index.
    #[test]
    fn a_refresh_drops_an_altered_no_derivatives_vocabulary_from_search() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state();
        state.vocab_engine = Some(std::sync::Arc::new(index::VocabSearchEngine::new(
            dir.path().to_path_buf(),
        )));
        seed_vocab::seed_standard_vocabularies(&state);
        refresh_platform_state(&state);
        let engine = state.vocab_engine.clone().unwrap();
        let before = engine.status().platform_vocabularies;
        mark(&state, "imbor", "2025");
        refresh_platform_state(&state);
        assert_eq!(engine.status().platform_vocabularies, before - 1);
    }
}
