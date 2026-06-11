//! HTTP handlers for `POST /api/import/bulk` and `POST /api/import/analyze`.

use axum::extract::{Extension, Multipart, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::bulk::{
    parse_and_load_bulk_gated, BulkError, BulkResult, GraphChange, InputFile, WriteGate,
};
use crate::auth::dataset_graph;
use crate::auth::middleware::AuthenticatedUser;
use crate::data_models::upload::{format_from_filename, format_from_media_type, parse_quads};
use crate::dataset_versions::models::VersionStatus;
use crate::kind_detector;
use crate::server::error::AppError;
use crate::server::AppState;

/// Optional metadata sidecar parsed from the `meta` multipart field.
///
/// All fields are optional. `targets` maps a filename to the target graph IRI
/// for that file (used for triple formats; quad formats preserve their own
/// graph names unless `merge` is true). `auto_split_files` lists filenames
/// that should be split by detected role into sub-graphs.
#[derive(Debug, Default, Deserialize)]
struct BulkMeta {
    #[serde(default)]
    dataset_id: Option<String>,
    #[serde(default)]
    replace: bool,
    /// Force every quad into the file's `target_graph`, even for quad formats.
    #[serde(default)]
    merge: bool,
    /// Default target graph IRI for any file without an entry in `targets`.
    #[serde(default)]
    default_target_graph: Option<String>,
    /// Per-filename target graph overrides.
    #[serde(default)]
    targets: std::collections::HashMap<String, String>,
    /// Per-filename map of embedded graph IRI → replacement target IRI, applied
    /// to quad-format files so embedded graph names can be re-homed under the
    /// dataset namespace at write time (instead of a post-import MOVE). Outer key
    /// is the filename; inner map is `{ embedded_iri: new_target_iri }`. Only
    /// consulted for quad formats with `merge=false`.
    #[serde(default)]
    graph_remap: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
    /// Filenames for which auto_split should be applied (triples split by role
    /// into `{target_graph}/model`, `{target_graph}/shapes`, etc.)
    #[serde(default)]
    auto_split_files: std::collections::HashSet<String>,
    /// Filenames whose target graphs should be replaced (PUT) rather than merged
    /// (POST). A global `replace=true` forces replace for every file.
    #[serde(default)]
    replace_files: std::collections::HashSet<String>,
    /// Version bump applied when a replace changes data: `major | minor | patch`
    /// (defaults to `patch`). The new semver is derived from the dataset's
    /// highest existing version.
    #[serde(default)]
    version_bump: Option<String>,
    /// Per-filename explicit graph role override (`instances | model | vocabulary
    /// | shapes | entailment`). When set, the file's target graph(s) are tagged
    /// with this role instead of auto-detecting from the loaded triples.
    #[serde(default)]
    graph_roles: std::collections::HashMap<String, String>,
}

/// What versioning did during a replace import, surfaced to the client so the
/// wizard can report "published v1.3.0" or "identical — saved as draft".
#[derive(Debug, Default, Clone, Serialize)]
pub struct VersionOutcome {
    /// Bump level that was requested (`major | minor | patch`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bump: Option<String>,
    /// Registered graphs whose contents changed and were archived.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed_graphs: Vec<String>,
    /// Registered graphs whose upload was identical to current data.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub identical_graphs: Vec<String>,
    /// Semver of the published version that archived the previous data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
    /// Semver of the draft created when the upload was identical.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_version: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct BulkResponse {
    #[serde(flatten)]
    pub result: BulkResult,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dataset_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_outcome: Option<VersionOutcome>,
}

/// `POST /api/import/bulk` (multipart/form-data)
///
/// Form fields:
/// - `file` (repeatable): each file part is one RDF document.
/// - `meta`: optional JSON blob (see `BulkMeta`).
/// - Or any of the meta fields directly as form fields:
///   `dataset_id`, `replace`, `merge`, `default_target_graph`, plus
///   `target_<filename>` entries for per-file targets.
pub async fn bulk_import(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    let mut meta = BulkMeta::default();
    // Collect files first; we resolve target graphs after meta is fully read.
    let mut raw_files: Vec<(String, String, Vec<u8>)> = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                // Cap the number of files per import. The body is already bounded
                // (200 MB), but tens of thousands of tiny `file` parts would still
                // amplify per-file bookkeeping and graph-index churn into a DoS.
                const MAX_BULK_IMPORT_FILES: usize = 2000;
                if raw_files.len() >= MAX_BULK_IMPORT_FILES {
                    return Err(AppError::BadRequest(format!(
                        "Too many files in one import (maximum {MAX_BULK_IMPORT_FILES})"
                    )));
                }
                let filename = field
                    .file_name()
                    .map(sanitize_filename)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("upload-{}.bin", raw_files.len()));
                let content_type = field
                    .content_type()
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| infer_mime(&filename));
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Failed to read file: {e}")))?;
                raw_files.push((filename, content_type, bytes.to_vec()));
            }
            "meta" => {
                let txt = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("meta read error: {e}")))?;
                if !txt.trim().is_empty() {
                    meta = serde_json::from_str(&txt)
                        .map_err(|e| AppError::BadRequest(format!("Invalid meta JSON: {e}")))?;
                }
            }
            "dataset_id" => {
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() {
                    meta.dataset_id = Some(v.trim().to_string());
                }
            }
            "replace" => {
                let v = field.text().await.unwrap_or_default();
                meta.replace = matches!(v.trim(), "true" | "1");
            }
            "merge" => {
                let v = field.text().await.unwrap_or_default();
                meta.merge = matches!(v.trim(), "true" | "1");
            }
            "version_bump" => {
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() {
                    meta.version_bump = Some(v.trim().to_string());
                }
            }
            "default_target_graph" => {
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() {
                    meta.default_target_graph = Some(v.trim().to_string());
                }
            }
            other if other.starts_with("target_") => {
                let key = other.trim_start_matches("target_").to_string();
                let v = field.text().await.unwrap_or_default();
                if !v.trim().is_empty() {
                    meta.targets.insert(key, v.trim().to_string());
                }
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }

    if raw_files.is_empty() {
        return Err(AppError::BadRequest("No files provided".to_string()));
    }

    // Authorization: bulk import lets the caller write triples directly into
    // named graphs and (optionally) attach those graphs to a dataset. Without
    // a per-dataset gate, any publisher could overwrite any private dataset's
    // graphs.
    //   - If `dataset_id` is set: caller must have write access to that dataset.
    //   - If `dataset_id` is unset: only platform admins may bulk-load into
    //     arbitrary unmanaged graphs (otherwise any user could clobber data
    //     belonging to someone else's dataset by guessing graph IRIs).
    match meta.dataset_id.as_deref() {
        Some(ds_id) => {
            let dataset = state
                .auth_db
                .get_dataset(ds_id)
                .map_err(|e| AppError::Internal(e.to_string()))?
                .ok_or_else(|| AppError::NotFound(format!("Dataset '{ds_id}' not found")))?;
            if !state
                .auth_db
                .can_write_dataset(&user.user_id, &dataset)
                .map_err(|e| AppError::Internal(e.to_string()))?
            {
                return Err(AppError::Unauthorized(
                    "Write access to this dataset required".to_string(),
                ));
            }
        }
        None => {
            if !user.is_admin() {
                return Err(AppError::Unauthorized(
                    "Bulk import without a dataset_id is restricted to admins".to_string(),
                ));
            }
        }
    }

    // Per-graph write boundary (mirrors the Graph Store Protocol path). Passing
    // `can_write_dataset` above only proves the caller may write *into the
    // dataset*; it does NOT constrain which graph IRIs they name as targets.
    // Without this gate a caller with write access to dataset A could set the
    // target graph to one owned by another tenant's dataset B (or a
    // `urn:system:*` graph) and, with `replace=true`, overwrite or wipe it.
    //
    // A dataset-scoped import may therefore only write a graph that is either
    // (a) already registered to the dataset, or (b) under the dataset's own
    // canonical IRI namespace `{base}/dataset/{id}/...`. Admins, and the
    // admin-only no-`dataset_id` branch above, are unrestricted. The check runs
    // against the fully-resolved graph set inside `parse_and_load_bulk` (see
    // below), so it also covers graph names embedded in quad-format files — and,
    // because `graph_remap` is applied during parsing *before* the set is
    // resolved, it checks the re-homed targets (a remap pointing at a foreign
    // graph is therefore still rejected on its final destination).
    let authz_is_admin = user.is_admin();
    let authz_dataset_id = meta.dataset_id.clone();
    let authz_db = state.auth_db.clone();
    let authz_registered: std::collections::HashSet<String> = match authz_dataset_id.as_deref() {
        Some(ds_id) if !authz_is_admin => authz_db
            .list_dataset_graphs(ds_id)
            .map_err(|e| AppError::Internal(e.to_string()))?
            .into_iter()
            .collect(),
        _ => std::collections::HashSet::new(),
    };
    let authz_namespace = authz_dataset_id
        .as_deref()
        .map(|ds_id| format!("{}/", dataset_graph::dataset_iri(&state.base_url, ds_id)));
    let authorize = move |graphs: &[String]| -> Result<(), String> {
        // Admins and unmanaged (admin-only) imports may target any graph.
        if authz_is_admin || authz_dataset_id.is_none() {
            return Ok(());
        }
        let ds_id = authz_dataset_id.as_deref().unwrap_or_default();
        let namespace = authz_namespace.as_deref().unwrap_or_default();
        for g in graphs {
            // In scope when the graph is registered to THIS dataset or lives under
            // its IRI namespace…
            let in_dataset =
                authz_registered.contains(g) || (!namespace.is_empty() && g.starts_with(namespace));
            // …and is not ALSO claimed by another dataset. The second clause closes
            // a register-then-overwrite bypass: `POST /api/datasets/{id}/graphs`
            // only checks dataset-write, so a caller could otherwise attach another
            // tenant's graph IRI to their own dataset and then name it here. On a
            // lookup error we fail closed (treat the graph as foreign).
            let owned_by_other = authz_db
                .graph_has_other_dataset_refs(g, ds_id)
                .unwrap_or(true);
            if owned_by_other || !in_dataset {
                return Err(format!(
                    "Target graph <{g}> is outside dataset '{ds_id}'. Bulk import may only write \
                     graphs that belong solely to this dataset — registered to it or under its IRI \
                     namespace <{namespace}>."
                ));
            }
        }
        Ok(())
    };

    // Where DEFAULT-graph (and blank-node-graph) triples in a quad file go when the
    // file names no target. A non-admin dataset-scoped import routes them into the
    // dataset's own namespaced default graph so they fall under the authorize gate
    // above (and stay out of the shared global default graph); admins and unmanaged
    // (no-`dataset_id`) imports keep the legacy global-default behavior.
    let unnamed_graph_target: Option<String> = match meta.dataset_id.as_deref() {
        Some(ds_id) if !authz_is_admin => Some(dataset_graph::dataset_default_graph_iri(
            &state.base_url,
            ds_id,
        )),
        _ => None,
    };

    let inputs: Vec<InputFile> = raw_files
        .into_iter()
        .map(|(filename, content_type, bytes)| {
            let target_graph = meta
                .targets
                .get(&filename)
                .cloned()
                .or_else(|| meta.default_target_graph.clone());
            let auto_split = meta.auto_split_files.contains(&filename);
            let replace = meta.replace || meta.replace_files.contains(&filename);
            let graph_remap = meta.graph_remap.get(&filename).cloned().unwrap_or_default();
            InputFile {
                filename,
                content_type,
                bytes,
                target_graph,
                merge_into_target: meta.merge,
                unnamed_graph_target: unnamed_graph_target.clone(),
                auto_split,
                replace,
                graph_remap,
            }
        })
        .collect();

    let store = state.store.clone();

    // Normalise the requested version bump up front.
    let bump = match meta.version_bump.as_deref() {
        Some("major") => "major",
        Some("minor") => "minor",
        _ => "patch",
    }
    .to_string();

    // When replacing graphs in a managed dataset we either cut a new published
    // version (data changed) or record a draft (the upload was identical),
    // archiving the previous contents *before* they are cleared so nothing is
    // lost. Only graphs registered to the dataset that currently hold data take
    // part; for unmanaged (admin) imports there is nothing to version.
    let outcome = std::sync::Arc::new(std::sync::Mutex::new(VersionOutcome::default()));
    let archive_store = state.store.clone();
    let archive_db = state.auth_db.clone();
    let archive_base = state.base_url.clone();
    let archive_ds = meta.dataset_id.clone();
    let archive_creator = format!("{}/users/{}", state.base_url, user.user_id);
    let archive_bump = bump.clone();
    let outcome_w = outcome.clone();
    let before_replace = move |changes: &[GraphChange]| -> Result<(), String> {
        let Some(ds_id) = archive_ds.as_deref() else {
            return Ok(());
        };
        let registered = archive_db.list_dataset_graphs(ds_id).unwrap_or_default();

        // Partition registered, data-bearing replace targets into changed vs
        // identical. Brand-new graphs (no current data) have nothing to archive
        // and are not versioned.
        let mut changed: Vec<String> = Vec::new();
        let mut identical: Vec<String> = Vec::new();
        for c in changes {
            if !registered.contains(&c.graph) {
                continue;
            }
            if archive_store.count_graph(Some(&c.graph)).unwrap_or(0) == 0 {
                continue;
            }
            if c.changed {
                changed.push(c.graph.clone());
            } else {
                identical.push(c.graph.clone());
            }
        }

        let mut out = outcome_w.lock().unwrap();
        out.bump = Some(archive_bump.clone());
        out.identical_graphs = identical.clone();

        if !changed.is_empty() {
            let existing = crate::dataset_versions::registry::list_versions(
                &archive_store,
                &archive_base,
                ds_id,
            );
            let version = crate::dataset_versions::next_semver(&existing, &archive_bump);
            crate::dataset_versions::snapshot_as_version(
                &archive_store,
                &archive_base,
                ds_id,
                &version,
                &changed,
                VersionStatus::Published,
                Some(&archive_creator),
                Some(&format!(
                    "Archived previous data before replace via import ({archive_bump} bump)"
                )),
            )?;
            out.changed_graphs = changed;
            out.new_version = Some(version);
        } else if !identical.is_empty() {
            let existing = crate::dataset_versions::registry::list_versions(
                &archive_store,
                &archive_base,
                ds_id,
            );
            let version = crate::dataset_versions::next_semver(&existing, &archive_bump);
            crate::dataset_versions::snapshot_as_version(
                &archive_store,
                &archive_base,
                ds_id,
                &version,
                &identical,
                VersionStatus::Draft,
                Some(&archive_creator),
                Some("Upload identical to current data — saved as draft"),
            )?;
            out.draft_version = Some(version);
        }
        Ok(())
    };

    // SHACL write-gating: bulk import honours the same gates as the GSP write
    // path (gate_writes pipelines, validation-layer bindings, and the owning
    // dataset's shacl_on_write). The `applies` pre-check is metadata-only, so
    // ungated imports (the common case, e.g. large IFC loads) skip validation
    // and quad buffering entirely.
    let gate_store = state.store.clone();
    let gate_db = state.auth_db.clone();
    let gate_base = state.base_url.clone();

    let result = tokio::task::spawn_blocking(move || {
        let studio = crate::shacl_studio::store::ShaclStudioStore::new(gate_db.pool());
        let gate = WriteGate {
            applies: Box::new(|g| {
                crate::shacl_studio::gate::import_gates_apply(
                    &gate_store,
                    &gate_db,
                    &studio,
                    &gate_base,
                    g,
                )
            }),
            check: Box::new(|g, quads| {
                crate::shacl_studio::gate::check_import_gates(
                    &gate_store,
                    &gate_db,
                    &studio,
                    &gate_base,
                    g,
                    quads,
                )
                .map_err(|r| crate::shacl_studio::gate::summarize_report(&r, 5))
            }),
        };
        parse_and_load_bulk_gated(&store, inputs, authorize, before_replace, Some(&gate))
    })
    .await
    .map_err(|e| AppError::Internal(format!("Bulk import task failed: {e}")))?
    .map_err(|e| match e {
        // A target graph outside the dataset's write scope ⇒ 403, not 400.
        BulkError::Forbidden(m) => AppError::Forbidden(m),
        BulkError::Failed(m) => AppError::BadRequest(m),
    })?;

    // Best-effort: register newly-touched graphs against the dataset and
    // auto-detect + store graph_role for each.
    if let Some(ds_id) = meta.dataset_id.as_deref() {
        let dataset_record = state.auth_db.get_dataset(ds_id).ok().flatten();
        for file_result in &result.file_results {
            if file_result.status != "ok" {
                continue;
            }
            // Explicit role chosen by the user for this file, if any.
            let explicit_role = meta
                .graph_roles
                .get(&file_result.filename)
                .and_then(|r| crate::auth::models::GraphKind::from_str(r));
            for iri in &file_result.graph_iris {
                if let Err(e) = state.auth_db.add_dataset_graph(ds_id, iri) {
                    tracing::warn!(dataset = %ds_id, graph = %iri, error = %e, "failed to register graph in dataset");
                    continue;
                }
                let role = if let Some(role) = explicit_role {
                    // User picked a role: apply it (overrides any prior/auto value).
                    let _ = state.auth_db.set_dataset_graph_role(ds_id, iri, Some(role));
                    Some(role)
                } else {
                    // Keep a previously-stored role; otherwise auto-detect from
                    // the stored quads.
                    let existing = state
                        .auth_db
                        .list_dataset_graph_entries(ds_id)
                        .ok()
                        .and_then(|entries| {
                            entries
                                .iter()
                                .find(|e| e.graph_iri == *iri)
                                .and_then(|e| e.graph_role)
                        });
                    match existing {
                        Some(r) => Some(r),
                        None => detect_and_store_graph_role(&state, ds_id, iri),
                    }
                };
                // Uploaded SHACL: adopt the graph into the SHACL Studio Library
                // and bind it to the dataset in the validation layer, so the
                // shapes are immediately visible in the Studio and effective for
                // validation. Best-effort — never fails the import.
                if role == Some(crate::auth::models::GraphKind::Shapes) {
                    if let Some(ds) = dataset_record.as_ref() {
                        if let Err(e) =
                            crate::shacl_studio::registration::auto_register_dataset_shapes_graph(
                                &state,
                                ds,
                                iri,
                                Some(&user.user_id),
                            )
                        {
                            tracing::warn!(dataset = %ds_id, graph = %iri, error = %e, "failed to auto-register imported shapes graph in SHACL Studio");
                        }
                    }
                }
            }
        }

        // Rewrite the DCAT metadata named graph so it reflects the newly-registered
        // graphs (void:subset + ots:graphRole triples).
        if let Ok(Some(ds)) = state.auth_db.get_dataset(ds_id) {
            let entries = state
                .auth_db
                .list_dataset_graph_entries(ds_id)
                .unwrap_or_default();
            dataset_graph::write_dataset_metadata_graph(
                &state.store,
                &state.base_url,
                &ds,
                &entries,
            );
        }
    }

    // Surface the versioning outcome only when it actually did something.
    let version_outcome = {
        let o = outcome.lock().unwrap().clone();
        if o.new_version.is_some() || o.draft_version.is_some() {
            Some(o)
        } else {
            None
        }
    };

    Ok((
        StatusCode::OK,
        Json(BulkResponse {
            result,
            dataset_id: meta.dataset_id,
            version_outcome,
        }),
    ))
}

/// Run `kind_detector::detect` on a graph's quads and store the inferred role.
/// Returns the role that was stored (`None` when detection was inconclusive or
/// the graph could not be read).
fn detect_and_store_graph_role(
    state: &AppState,
    dataset_id: &str,
    graph_iri: &str,
) -> Option<crate::auth::models::GraphKind> {
    use oxigraph::model::GraphNameRef;
    let graph_name = oxigraph::model::NamedNode::new(graph_iri).ok()?;
    let quads = state
        .store
        .quads_for_graph(GraphNameRef::NamedNode(graph_name.as_ref()))
        .ok()?;
    let detected = kind_detector::detect(&quads);
    let role = detected.to_graph_role()?;
    let _ = state
        .auth_db
        .set_dataset_graph_role(dataset_id, graph_iri, Some(role));
    Some(role)
}

// ─── Per-split result ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct RoleSplit {
    pub role: String,
    pub triple_count: usize,
    pub suggested_suffix: String,
}

#[derive(Debug, Serialize)]
pub struct AnalyzeResponse {
    pub total_triples: usize,
    pub splits: Vec<RoleSplit>,
    pub is_mixed: bool,
}

/// `POST /api/import/analyze` — scan an RDF file and return role-split suggestions.
///
/// Accepts a single `file` field in a multipart upload. Does **not** import
/// any data; analysis only.
pub async fn analyze_import(mut multipart: Multipart) -> Result<impl IntoResponse, AppError> {
    let mut filename = String::new();
    let mut content_type = String::new();
    let mut bytes: Option<Vec<u8>> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        if field.name().unwrap_or("") == "file" {
            filename = field
                .file_name()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "upload.bin".to_string());
            content_type = field
                .content_type()
                .map(|c| c.to_string())
                .unwrap_or_else(|| infer_mime(&filename));
            let b = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("Failed to read file: {e}")))?;
            bytes = Some(b.to_vec());
            break;
        } else {
            let _ = field.bytes().await;
        }
    }

    let bytes = bytes.ok_or_else(|| AppError::BadRequest("No file field provided".to_string()))?;

    let format = format_from_media_type(&content_type)
        .or_else(|| format_from_filename(&filename))
        .ok_or_else(|| {
            AppError::BadRequest(format!(
                "Cannot detect RDF format from content-type '{}' or filename '{}'",
                content_type, filename
            ))
        })?;

    let quads = tokio::task::spawn_blocking(move || parse_quads(&bytes, format))
        .await
        .map_err(|e| AppError::Internal(format!("Parse task failed: {e}")))?
        .map_err(AppError::BadRequest)?;

    let total_triples = quads.len();

    // Count triples per detected role, using the same subject-tree
    // classification as the actual auto-split (`do_split`) so the preview and
    // the import agree on where each quad would land.
    let roles = kind_detector::classify_quad_roles(&quads);
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for role in &roles {
        *counts.entry(role.as_str().to_string()).or_default() += 1;
    }

    let role_order = ["model", "vocabulary", "shapes", "entailment", "instances"];
    let mut splits: Vec<RoleSplit> = role_order
        .iter()
        .filter_map(|&role_key| {
            let count = *counts.get(role_key)?;
            if count == 0 {
                return None;
            }
            let suffix = format!("/{}", role_key);
            Some(RoleSplit {
                role: role_key.to_string(),
                triple_count: count,
                suggested_suffix: suffix,
            })
        })
        .collect();

    // Include any remaining roles not in the ordered list.
    for (role_key, count) in &counts {
        if count > &0 && !role_order.contains(&role_key.as_str()) {
            splits.push(RoleSplit {
                role: role_key.clone(),
                triple_count: *count,
                suggested_suffix: format!("/{}", role_key),
            });
        }
    }

    let is_mixed = splits.len() > 1;

    Ok(Json(AnalyzeResponse {
        total_triples,
        splits,
        is_mixed,
    }))
}

/// Reduce a client-supplied multipart filename to a safe basename.
///
/// Strips any directory components (both `/` and `\`) so a crafted upload name
/// like `../../etc/passwd` cannot influence target-graph minting, metadata keys,
/// or any path that might later be derived from the name. Control characters are
/// removed and `.`/`..` collapse to empty (the caller substitutes a default).
fn sanitize_filename(raw: impl AsRef<str>) -> String {
    let base = raw.as_ref().rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base == "." || base == ".." {
        return String::new();
    }
    base.chars().filter(|c| !c.is_control()).collect()
}

fn infer_mime(filename: &str) -> String {
    match filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "ttl" | "turtle" => "text/turtle".to_string(),
        "nt" => "application/n-triples".to_string(),
        "nq" => "application/n-quads".to_string(),
        "trig" => "application/trig".to_string(),
        "rdf" | "xml" | "owl" => "application/rdf+xml".to_string(),
        "jsonld" | "json" => "application/ld+json".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}
