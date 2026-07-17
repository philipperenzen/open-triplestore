//! IFC file import: store the source file as a downloadable dataset asset and
//! transform it into linked data (BOT layer + optional full ifcOWL lift).
//! Shared by the bulk-import endpoint (multipart `.ifc` parts) and the demo
//! seeder.

use axum::body::Bytes;

use crate::ifc::{convert, ConvertOptions, IfcStats};
use crate::server::AppState;

/// What an IFC import produced.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IfcImportOutcome {
    /// Stored asset id (None when no object store is configured).
    pub asset_id: Option<String>,
    /// Public download URL of the stored IFC.
    pub asset_url: Option<String>,
    /// Graph holding the BOT topology + properties.
    pub bot_graph: String,
    /// Graph holding the full ifcOWL-style lift (when requested).
    pub ifcowl_graph: Option<String>,
    pub stats: IfcStats,
}

/// Import one IFC file into `dataset_id`: persist the bytes as an asset (so
/// users can download the original), convert to RDF, load the graphs, and
/// register them on the dataset. Heavy work (parse + store load) runs on the
/// blocking pool.
#[allow(clippy::too_many_arguments)]
pub async fn import_ifc_bytes(
    state: &AppState,
    dataset_id: &str,
    user_id: &str,
    file_name: &str,
    bytes: Vec<u8>,
    target_graph: Option<String>,
    public_asset: bool,
    include_ifcowl: bool,
    // FOG file URL fallback when no object store is configured (e.g. the
    // original public source of a seeded file) — keeps the 3D viewer working
    // without S3.
    fallback_file_url: Option<String>,
    // Map anchor override — wins over the file's own IfcSite georeference
    // (exporters often leave a default location like the RD origin in there).
    anchor_wkt: Option<String>,
) -> Result<IfcImportOutcome, String> {
    let base = state.base_url.trim_end_matches('/').to_string();

    // 1. Keep the original file as a dataset asset — the "download IFC" source.
    // The asset's `uploaded_by` has a FK to users, so a system-owned import (the
    // first-boot demo seed, which passes an empty owner_id) skips the asset and
    // falls back to the source URL for the FOG file reference instead.
    let (asset_id, asset_url) = if state.object_store.is_configured() && !user_id.is_empty() {
        let asset_id = uuid::Uuid::new_v4().to_string();
        let file_name_clean = crate::assets::sanitize_filename(file_name);
        let s3_key = format!("datasets/{dataset_id}/{asset_id}/{file_name_clean}");
        let declared = "application/x-step";
        let kind = crate::assets::classify(declared, &file_name_clean, &bytes);
        let meta = crate::assets::extract_for(kind, &bytes, declared, &file_name_clean);
        let size = bytes.len() as i64;
        state
            .object_store
            .upload(&s3_key, Bytes::from(bytes.clone()), declared)
            .await
            .map_err(|e| format!("asset upload failed: {e}"))?;
        let asset = state
            .auth_db
            .create_asset(
                &asset_id,
                dataset_id,
                &file_name_clean,
                declared,
                &s3_key,
                size,
                user_id,
                public_asset,
            )
            .map_err(|e| format!("asset record failed: {e}"))?;
        if let Err(e) =
            crate::server::routes::insert_asset_triples(state, &asset, dataset_id, kind, &meta)
        {
            tracing::warn!("ifc import: asset metadata insert failed: {e}");
        }
        let assets_graph = crate::server::routes::assets_graph_iri(&state.base_url, dataset_id);
        let _ = state.auth_db.add_dataset_graph(dataset_id, &assets_graph);
        let url = format!("{base}/api/datasets/{dataset_id}/assets/{asset_id}/download");
        (Some(asset_id), Some(url))
    } else {
        (None, fallback_file_url)
    };

    // 2. Convert + load on the blocking pool (CPU + store writes).
    let bot_graph = target_graph
        .filter(|g| !g.trim().is_empty())
        .unwrap_or_else(|| format!("{base}/dataset/{dataset_id}/building"));
    let ifcowl_graph = include_ifcowl.then(|| format!("{bot_graph}/ifcowl"));
    let inst_base = if bot_graph.ends_with('/') || bot_graph.ends_with('#') {
        bot_graph.clone()
    } else {
        format!("{bot_graph}/")
    };

    let store = state.store.clone();
    let opts = ConvertOptions {
        inst_base,
        ifc_file_url: asset_url.clone(),
        anchor_wkt, // None ⇒ emit() falls back to the file's own IfcSite georeference
        include_ifcowl,
    };
    let bot_graph_c = bot_graph.clone();
    let ifcowl_graph_c = ifcowl_graph.clone();
    let stats = tokio::task::spawn_blocking(move || -> Result<IfcStats, String> {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        drop(bytes);
        use oxigraph::io::RdfFormat;
        // Both sinks may fail; RefCell lets them share the error slot while the
        // converter holds them as two simultaneous &mut closures.
        let err: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let mut bot_first = true;
        let mut owl_first = true;
        let stats = {
            let mut bot_sink = |chunk: &str| {
                if err.borrow().is_some() {
                    return;
                }
                let r = if bot_first {
                    bot_first = false;
                    store.graph_store_put(Some(&bot_graph_c), chunk, RdfFormat::NTriples)
                } else {
                    store.graph_store_post(Some(&bot_graph_c), chunk, RdfFormat::NTriples)
                };
                if let Err(e) = r {
                    *err.borrow_mut() = Some(format!("loading BOT graph failed: {e}"));
                }
            };
            let mut owl_sink = |chunk: &str| {
                if err.borrow().is_some() {
                    return;
                }
                let Some(g) = ifcowl_graph_c.as_deref() else {
                    return;
                };
                let r = if owl_first {
                    owl_first = false;
                    store.graph_store_put(Some(g), chunk, RdfFormat::NTriples)
                } else {
                    store.graph_store_post(Some(g), chunk, RdfFormat::NTriples)
                };
                if let Err(e) = r {
                    *err.borrow_mut() = Some(format!("loading ifcOWL graph failed: {e}"));
                }
            };
            convert(&text, &opts, &mut bot_sink, &mut owl_sink)?
        };
        match err.into_inner() {
            Some(e) => Err(e),
            None => Ok(stats),
        }
    })
    .await
    .map_err(|e| format!("ifc conversion task failed: {e}"))??;

    // 3. Register the graphs on the dataset so scoping/visibility apply.
    let _ = state.auth_db.add_dataset_graph(dataset_id, &bot_graph);
    let _ = super::handlers::detect_and_store_graph_role(state, dataset_id, &bot_graph);
    if let Some(g) = &ifcowl_graph {
        let _ = state.auth_db.add_dataset_graph(dataset_id, g);
    }
    state.auth_db.invalidate_accessible_graphs_cache();
    #[cfg(feature = "text-search")]
    state.mark_text_dirty();

    Ok(IfcImportOutcome {
        asset_id,
        asset_url,
        bot_graph,
        ifcowl_graph,
        stats,
    })
}

/// Does this multipart part look like an IFC STEP file?
pub fn is_ifc_file(filename: &str, content_type: &str) -> bool {
    let f = filename.to_ascii_lowercase();
    let ct = content_type.to_ascii_lowercase();
    f.ends_with(".ifc")
        || ct.contains("application/x-step")
        || ct.contains("model/ifc")
        || ct.contains("application/ifc")
}
