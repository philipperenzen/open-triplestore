//! Internal vocabulary search & recommendation service (LOV replacement).
//!
//! Public LOV (lov.linkeddata.es) is frequently unreachable and prefix.cc's
//! availability is equally shaky, so this module brings both in-house:
//!
//! * [`catalog`] — the embedded LOV vocabulary catalog (~900 vocabularies:
//!   metadata, tags, versions, VOAF reuse metrics) overlaid with this
//!   instance's public model/vocabulary registry.  Always available.
//! * [`corpus`] — term extraction from the full LOV N-Quads corpus (bundled
//!   in the Docker image / downloaded once, sha256-pinned) and from platform
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
        let public: Vec<_> = records.into_iter().filter(|r| r.is_public).collect();
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

/// Boot task: locate (or download) the LOV corpus, then build/reopen the LOV
/// term index in the background.  Also does the initial platform refresh so
/// the service is warm right after seeding.
pub async fn boot_vocab_search(state: AppState, data_dir: std::path::PathBuf) {
    // Corpus discovery/download (async, network only when configured).
    let corpus_path = corpus::ensure_corpus(&data_dir).await;
    if let Ok(mut guard) = state.vocab_corpus.write() {
        guard.clone_from(&corpus_path);
    }

    // Initial refresh of everything derived from the registry — runs on every
    // build (feature on or off, engine present or not).
    {
        let state_bg = state.clone();
        let refreshed = tokio::task::spawn_blocking(move || {
            state_bg
                .vocab_registry_dirty
                .store(false, std::sync::atomic::Ordering::Relaxed);
            refresh_platform_state(&state_bg);
        })
        .await;
        if refreshed.is_err() {
            tracing::warn!("vocab-search: initial platform refresh panicked");
            state
                .vocab_registry_dirty
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
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
                return;
            };
            let sha = match index::file_sha256(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("vocab-search: cannot hash corpus: {e}");
                    return;
                }
            };
            let dir = engine.lov_index_dir(&sha);
            if index::VocabSearchEngine::lov_stats_marker(&dir).exists() {
                match engine.reopen_lov_index(&dir) {
                    Ok(()) => {
                        tracing::info!("vocab-search: reopened LOV term index at {:?}", dir);
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
                        "vocab-search: extracted {} terms from {} vocabularies ({} instances dropped) in {:?}",
                        stats.terms,
                        stats.vocabularies,
                        stats.instances_dropped,
                        started.elapsed()
                    );
                    if let Err(e) = engine.set_lov_index_from_docs(&dir, &docs, stats) {
                        tracing::warn!("vocab-search: LOV index build failed: {e}");
                    } else {
                        tracing::info!(
                            "vocab-search: LOV term index ready in {:?}",
                            started.elapsed()
                        );
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
