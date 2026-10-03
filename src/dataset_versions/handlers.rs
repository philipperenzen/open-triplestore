//! Dataset versioning REST API.

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use oxigraph::io::RdfFormat;
use serde_json::json;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::Dataset;
use crate::data_models::diff::triple_delta;
use crate::server::error::AppError;
use crate::server::AppState;

use super::models::{
    CreateDatasetBranchRequest, CreateDatasetVersionRequest, DatasetBranchInfo, DatasetVersion,
    GcDatasetVersionsRequest, UpdateDatasetVersionRequest, VersionDataParams, VersionStatus,
};
use super::{cut, lock, materialise, registry, snapshot};

// ─── helpers ──────────────────────────────────────────────────────────────

fn load_dataset(state: &AppState, id: &str) -> Result<Dataset, AppError> {
    state
        .auth_db
        .get_dataset(id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Dataset '{id}' not found")))
}

fn require_read(state: &AppState, ds: &Dataset, uid: Option<&str>) -> Result<(), AppError> {
    if state
        .auth_db
        .can_access_dataset(uid, ds)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        Ok(())
    } else {
        Err(AppError::NotFound(format!("Dataset '{}' not found", ds.id)))
    }
}

/// The dataset's private *live source*-graph IRIs, as they bear on what `uid`
/// may see of a version snapshot.
///
/// A version snapshot copies every graph the dataset held at the time — private
/// ones included — into version-scoped IRIs that are absent from
/// `list_dataset_graph_entries`, so the plain private filter that guards `/sparql`
/// is a no-op on a version read. This maps a snapshot's *source* graph back to
/// the live `private` flag instead.
///
/// Returns `None` when the caller may write the dataset (writers and admins see
/// every graph). Otherwise `Some(set)` of the live graphs flagged private, which
/// the callers below use to drop the matching snapshot graphs. Fails closed: a
/// lookup error propagates as `500`, so nothing is served, rather than defaulting
/// to "nothing private".
fn private_source_filter(
    state: &AppState,
    ds: &Dataset,
    uid: Option<&str>,
) -> Result<Option<std::collections::HashSet<String>>, AppError> {
    let can_write = match uid {
        Some(u) => state
            .auth_db
            .can_write_dataset(u, ds)
            .map_err(|e| AppError::Internal(e.to_string()))?,
        None => false,
    };
    if can_write {
        return Ok(None);
    }
    let entries = state
        .auth_db
        .list_dataset_graph_entries(&ds.id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    Ok(Some(
        entries
            .into_iter()
            .filter(|e| e.private)
            .map(|e| e.graph_iri)
            .collect(),
    ))
}

/// The snapshot graphs of `record` that `uid` may read. Writers get them all;
/// non-writers lose every snapshot whose live source graph is private. When the
/// dataset has no private graph the full list is returned unchanged, so the
/// common case (and legacy versions with an empty `source_map`) is untouched;
/// once anything is private, a non-writer sees only snapshots with a non-private
/// source in the map (an unmapped snapshot is dropped, fail-closed).
fn readable_snapshot_graphs(
    state: &AppState,
    ds: &Dataset,
    uid: Option<&str>,
    record: &DatasetVersion,
) -> Result<Vec<String>, AppError> {
    match private_source_filter(state, ds, uid)? {
        None => Ok(record.snapshot_graphs.clone()),
        Some(private) if private.is_empty() => Ok(record.snapshot_graphs.clone()),
        Some(private) => Ok(record
            .source_map
            .iter()
            .filter(|m| !private.contains(&m.source_graph))
            .map(|m| m.snapshot_graph.clone())
            .collect()),
    }
}

fn require_write(state: &AppState, ds: &Dataset, uid: &str) -> Result<(), AppError> {
    if state
        .auth_db
        .can_write_dataset(uid, ds)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        Ok(())
    } else {
        // 403, not 401: the caller IS authenticated, they just lack the right.
        // A 401 here made clients treat "no write grant" as a session expiry.
        Err(AppError::Forbidden(
            "Write access to this dataset required".to_string(),
        ))
    }
}

fn validate_version(v: &str) -> Result<(), AppError> {
    crate::data_models::version_iri::validate_version(v).map_err(AppError::BadRequest)
}

// ─── list / get ─────────────────────────────────────────────────────────────

/// GET /api/datasets/:id/versions
pub async fn list_versions(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    require_read(&state, &ds, uid)?;
    let versions = registry::list_versions(&state.store, &state.base_url, &id);
    Ok(Json(versions))
}

/// GET /api/datasets/:id/versions/:ver
pub async fn get_version(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    require_read(&state, &ds, uid)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    Ok(Json(record))
}

/// GET /api/datasets/:id/versions/:ver/data
pub async fn get_version_data(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((id, ver)): Path<(String, String)>,
    Query(params): Query<VersionDataParams>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    require_read(&state, &ds, uid)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    // A delta's graphs are built on first read (never for an anonymous one).
    materialise::ensure_built(&state, &record, uid.is_none()).await?;

    // A version snapshot copies private graphs too, into IRIs the plain private
    // filter never sees. Restrict a non-writer to the snapshots whose live source
    // is not private, so a viewer (or anonymous caller on a public dataset) cannot
    // dump a private graph through a pinned version.
    let readable = readable_snapshot_graphs(&state, &ds, uid, &record)?;
    let graphs: Vec<String> = match params.graph.as_deref() {
        Some("all") | None => readable,
        Some(suffix) => readable
            .into_iter()
            .filter(|g| g.ends_with(suffix))
            .collect(),
    };

    let single = matches!(
        params.format.as_deref(),
        Some("turtle") | Some("ttl") | Some("ntriples") | Some("nt")
    ) && graphs.len() == 1;
    let fmt = if single {
        match params.format.as_deref() {
            Some("ntriples") | Some("nt") => RdfFormat::NTriples,
            _ => RdfFormat::Turtle,
        }
    } else {
        RdfFormat::TriG
    };
    let content_type = match fmt {
        RdfFormat::Turtle => "text/turtle",
        RdfFormat::NTriples => "application/n-triples",
        _ => "application/trig",
    };

    let mut out = Vec::new();
    for g in &graphs {
        let data = state
            .store
            .graph_store_get(Some(g), fmt)
            .map_err(AppError::from)?;
        out.extend_from_slice(&data);
    }

    use axum::http::HeaderValue;
    use axum::response::Response;
    let mut resp = Response::new(axum::body::Body::from(out));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    Ok(resp)
}

// ─── create draft snapshot ──────────────────────────────────────────────────

/// POST /api/datasets/:id/versions
///
/// A delta when one can be chained from a checkpoint, a checkpoint otherwise
/// (`cut::cut`); `"checkpoint": true` asks for a checkpoint. The response is
/// the record, plus `checkpoint_reason` when it is a checkpoint.
pub async fn create_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<CreateDatasetVersionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    validate_version(&body.version)?;
    let version_lock = lock::dataset(&id);
    let _held = version_lock.lock().await;
    if registry::version_exists(&state.store, &state.base_url, &id, &body.version) {
        return Err(AppError::BadRequest(format!(
            "Version '{}' already exists",
            body.version
        )));
    }

    // Source graphs: the requested subset, else every registered graph but
    // the statement-provenance graph (its annotations are about the data, not
    // part of it; delta note §6.3).
    let entries = state
        .auth_db
        .list_dataset_graph_entries(&id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let source_graphs: Vec<String> = if body.graphs.is_empty() {
        entries
            .iter()
            .filter(|e| !crate::provenance::is_statement_provenance_graph(&id, &e.graph_iri))
            .map(|e| e.graph_iri.clone())
            .collect()
    } else {
        body.graphs
            .iter()
            .filter(|g| entries.iter().any(|e| e.graph_iri == **g))
            .cloned()
            .collect()
    };
    if source_graphs.is_empty() {
        return Err(AppError::BadRequest(
            "Dataset has no graphs to snapshot".to_string(),
        ));
    }

    let req = cut::CutRequest {
        dataset_id: id.clone(),
        version: body.version.clone(),
        source_graphs: source_graphs.clone(),
        status: VersionStatus::Draft,
        created_by: Some(format!("{}/users/{}", state.base_url, user.user_id)),
        notes: body.notes.clone(),
        branch: body.branch.clone(),
        derived_from: None,
        force_checkpoint: body.checkpoint,
        shared: cut::shared_graphs(&state.auth_db, &id, &source_graphs),
        conforms_to_model: ds.conforms_to_model.clone().filter(|m| !m.is_empty()),
        conforms_to_version: ds.conforms_to_version.clone().filter(|v| !v.is_empty()),
    };
    let st = state.clone();
    let made = tokio::task::spawn_blocking(move || {
        let made = cut::cut(&st.store, &st.base_url, &req)?;
        super::after_cut(&st.store, &st.base_url, &made.record, &req.source_graphs);
        Ok::<_, String>(made)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(AppError::Internal)?;
    let record = made.record;
    // Re-test this dataset's saved queries against the new version (background).
    crate::saved_queries::testing::spawn_version_tests(&state, &id, &body.version);
    version_commit(
        &state,
        &user.user_id,
        &id,
        Some(&record.version),
        format!("Cut version {}", record.version),
    );
    let mut out = serde_json::to_value(&record).map_err(|e| AppError::Internal(e.to_string()))?;
    if let (Some(reason), Some(obj)) = (made.checkpoint_reason, out.as_object_mut()) {
        obj.insert("checkpoint_reason".into(), json!(reason));
    }
    Ok((StatusCode::CREATED, Json(out)))
}

/// PATCH /api/datasets/:id/versions/:ver — update notes.
pub async fn update_version_notes(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
    Json(body): Json<UpdateDatasetVersionRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    registry::update_version_notes(
        &state.store,
        &state.base_url,
        &id,
        &ver,
        body.notes.as_deref(),
    )
    .map_err(AppError::from)?;
    Ok(Json(json!({ "ok": true })))
}

// ─── lifecycle ────────────────────────────────────────────────────────────

/// POST /api/datasets/:id/versions/:ver/stage
pub async fn stage_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    if record.status != VersionStatus::Draft {
        return Err(AppError::BadRequest(
            "Only Draft versions can be staged".to_string(),
        ));
    }
    registry::update_version_status(
        &state.store,
        &state.base_url,
        &id,
        &ver,
        VersionStatus::Staged,
    )
    .map_err(AppError::from)?;
    if record.branch.is_none() {
        registry::clear_latest_draft(&state.store, &state.base_url, &id).map_err(AppError::from)?;
    }
    Ok(Json(json!({ "status": "staged", "version": ver })))
}

/// POST /api/datasets/:id/versions/:ver/publish
pub async fn publish_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let version_lock = lock::dataset(&id);
    let _held = version_lock.lock().await;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    if !matches!(record.status, VersionStatus::Staged | VersionStatus::Draft) {
        return Err(AppError::BadRequest(
            "Only Staged or Draft versions can be published".to_string(),
        ));
    }
    // A published version is physical: present in every backup and
    // independent of the change log's retention (delta note §3.2).
    promote(&state, &record).await?;
    // Deprecate the prior latest-published main version.
    let (latest_pub, _) = registry::get_pointers(&state.store, &state.base_url, &id);
    if let Some(old) = latest_pub {
        if old != ver {
            registry::update_version_status(
                &state.store,
                &state.base_url,
                &id,
                &old,
                VersionStatus::Deprecated,
            )
            .map_err(AppError::from)?;
        }
    }
    registry::update_version_status(
        &state.store,
        &state.base_url,
        &id,
        &ver,
        VersionStatus::Published,
    )
    .map_err(AppError::from)?;
    registry::update_latest_published(&state.store, &state.base_url, &id, &ver)
        .map_err(AppError::from)?;
    version_commit(
        &state,
        &user.user_id,
        &id,
        Some(&ver),
        format!("Published version {ver}"),
    );
    Ok(Json(json!({ "status": "published", "version": ver })))
}

/// Make a delta physical (delta note §3.2): build it, check what was built
/// against its content hash — graph by graph, the end-to-end check of its
/// chain (§3.5) — and make the built graphs its own snapshot graphs. A
/// mismatch marks the chain broken and refuses (409): publishing data that
/// is not what the version recorded would be worse than not publishing.
pub(crate) async fn promote(state: &AppState, record: &DatasetVersion) -> Result<(), AppError> {
    if !record.is_delta() {
        return Ok(());
    }
    materialise::ensure_built(state, record, false).await?;
    let st = state.clone();
    let rec = record.clone();
    tokio::task::spawn_blocking(move || -> Result<(), AppError> {
        let build_lock = lock::build(&rec.graph_iri);
        let _g = build_lock.blocking_lock();
        let mut quads = 0i64;
        for m in &rec.source_map {
            let (n, hash) = hash_graph(&st.store, &m.snapshot_graph, &m.source_graph)?;
            if let Some(expected) = m.content_hash.as_deref() {
                if hash.to_hex() != expected {
                    let _ = registry::set_chain_broken(
                        &st.store,
                        &st.base_url,
                        &rec.dataset_id,
                        &rec.version,
                    );
                    materialise::release(&st.store, &rec, false);
                    return Err(AppError::Conflict(json!({
                        "error": "the version rebuilt to content that does not match its content hash; its chain is marked broken",
                        "version": rec.version,
                        "graph": m.source_graph,
                    })));
                }
            }
            quads += n;
        }
        registry::promote_to_checkpoint(&st.store, &st.base_url, &rec, quads)
            .map_err(AppError::from)?;
        materialise::release(&st.store, &rec, true);
        Ok(())
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

/// The quads of `graph` and their set hash, counted under `hash_as`.
fn hash_graph(
    store: &crate::store::TripleStore,
    graph: &str,
    hash_as: &str,
) -> Result<(i64, crate::store::set_hash::SetHash), AppError> {
    let name = oxigraph::model::NamedNodeRef::new(graph)
        .map_err(|e| AppError::Internal(format!("<{graph}>: {e}")))?;
    let mut hash = crate::store::set_hash::SetHash::new();
    let mut n = 0i64;
    for q in store
        .store()
        .quads_for_pattern(
            None,
            None,
            None,
            Some(oxigraph::model::GraphNameRef::NamedNode(name)),
        )
        .flatten()
    {
        hash.add_quad(&q, Some(hash_as));
        n += 1;
    }
    Ok((n, hash))
}

/// POST /api/datasets/:id/versions/:ver/deprecate
pub async fn deprecate_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    if record.status == VersionStatus::Deprecated {
        return Err(AppError::BadRequest(
            "Version is already deprecated".to_string(),
        ));
    }
    registry::update_version_status(
        &state.store,
        &state.base_url,
        &id,
        &ver,
        VersionStatus::Deprecated,
    )
    .map_err(AppError::from)?;
    version_commit(
        &state,
        &user.user_id,
        &id,
        Some(&ver),
        format!("Deprecated version {ver}"),
    );
    Ok(Json(json!({ "status": "deprecated", "version": ver })))
}

/// POST /api/datasets/:id/versions/:ver/restore — copy the snapshot back onto
/// the dataset's live graphs.
pub async fn restore_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let version_lock = lock::dataset(&id);
    let _held = version_lock.lock().await;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    if record.source_map.is_empty() {
        return Err(AppError::BadRequest(
            "Version has no graph mapping to restore".to_string(),
        ));
    }
    // A restore overwrites (or, from an empty snapshot, deletes) each source
    // graph and registers it to the dataset again, so every one goes through
    // the gate a new target graph does. Since the snapshot, a graph may have
    // left the dataset: detached and taken up by someone else, a source run
    // graph a later promotion replaced, or a graph that became part of the
    // model registry. Such a graph is skipped — it is no longer one of the
    // dataset's live graphs — and reported; the rest is restored.
    let mut mappings = Vec::with_capacity(record.source_map.len());
    let mut claims = Vec::with_capacity(record.source_map.len());
    let mut skipped = Vec::new();
    for m in &record.source_map {
        let g = &m.source_graph;
        let registered = state
            .auth_db
            .dataset_has_graph(&id, g)
            .map_err(|e| AppError::Internal(e.to_string()))?;
        let claim = if registered {
            crate::auth::dataset_graph::refuse_model_registry_graph(
                &state.store,
                &state.base_url,
                &id,
                g,
            )
            .map(|_| crate::auth::dataset_graph::GraphClaim::Registered)
        } else {
            crate::auth::dataset_graph::gate_dataset_graph_target(
                &state.store,
                &state.auth_db,
                &state.base_url,
                &id,
                g,
                &user,
            )
        };
        match claim {
            Ok(claim) => {
                mappings.push(m.clone());
                claims.push((g.clone(), claim));
            }
            Err(reason) => skipped.push(json!({ "graph": g, "reason": reason })),
        }
    }
    let restored_graphs: Vec<String> = mappings.iter().map(|m| m.source_graph.clone()).collect();
    // Tier 1 needs only the change log; tier 2 copies the version's graphs,
    // so a delta must be built for it.
    let inverse = {
        let st = state.clone();
        let rec = record.clone();
        let gs = restored_graphs.clone();
        tokio::task::spawn_blocking(move || plan_inverse(&st.store, &rec, &gs))
            .await
            .map_err(|e| AppError::Internal(e.to_string()))?
    };
    if inverse.is_none() {
        materialise::ensure_built(&state, &record, false).await?;
    }
    let stamp = crate::commit_log::CommitStamp::new(&state.base_url);
    let ctx = crate::server::write_effects::context(
        &state,
        Some(&user),
        crate::commit_log::CommitKind::Dataset,
        &stamp,
    );
    let st = state.clone();
    let rec = record.clone();
    let maps = mappings.clone();
    let gs = restored_graphs.clone();
    let stamped = stamp.clone();
    let (restored, tier) =
        tokio::task::spawn_blocking(move || -> Result<(Vec<String>, u8), AppError> {
            let ldes = crate::ldes::capture::Capture::begin(&st, &gs);
            let (restored, tier) = {
                let _ctx = crate::store::changes::WriteContextGuard::set(ctx);
                match inverse.map(|inv| restore_by_inverse(&st.store, &maps, inv)) {
                    Some(Ok(true)) => (gs.clone(), 1),
                    _ => {
                        materialise::ensure_built_blocking(&st.store, &st.base_url, &rec)
                            .map_err(|e| AppError::Internal(e.to_string()))?;
                        (
                            snapshot::restore(&st.store, &maps).map_err(AppError::from)?,
                            2,
                        )
                    }
                }
            };
            let written = crate::server::write_effects::Written::read(&st, &stamped);
            ldes.finish(&st, &written.rows);
            written.entailment(&st, &gs);
            // The full-text index followed writes but never a restore, so
            // searches kept answering from the pre-restore content.
            if !written.sync_text_index(&st, &gs) {
                st.refresh_text_index_graphs(&restored);
            }
            Ok((restored, tier))
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;
    // Ensure restored graphs are registered to the dataset.
    for (g, claim) in &claims {
        if restored.contains(g) {
            let _ =
                crate::auth::dataset_graph::register_claimed_graph(&state.auth_db, &id, g, *claim);
        }
    }
    // Re-apply the version's validation-layer bindings (best-effort, tolerant of
    // shape graphs deleted since the snapshot).
    let studio = crate::shacl_studio::store::ShaclStudioStore::new(state.auth_db.pool());
    match crate::shacl_studio::bindings::restore_dataset_bindings(
        &state.store,
        &studio,
        &state.base_url,
        &id,
        &ver,
    ) {
        Ok(n) if n > 0 => tracing::info!("restored {n} validation binding(s) for {id} v{ver}"),
        Ok(_) => {}
        Err(e) => tracing::warn!("failed to restore validation bindings for {id} v{ver}: {e}"),
    }
    version_commit_stamped(
        &state,
        Some(&stamp),
        &user.user_id,
        &id,
        Some(&ver),
        format!("Restored version {ver}"),
    );
    Ok(Json(
        json!({ "restored": restored, "skipped": skipped, "version": ver, "tier": tier }),
    ))
}

/// Tier 1 of a restore (delta note §3.6): when the version is anchored in
/// the current epoch, every graph to restore has its content hash, the chain
/// from the version to now is intact for those graphs and its inverse fits
/// one transaction (`OTS_APPLY_DELTA_TX_MAX_QUADS`), that inverse —
/// `(to add, to remove)`. Applied in one transaction, it puts every graph
/// back at once: strictly better than the per-graph `MOVE` of tier 2, which
/// rebuilds each graph's count and leaves the version observable
/// half-restored between graphs.
fn plan_inverse(
    store: &crate::store::TripleStore,
    record: &DatasetVersion,
    graphs: &[String],
) -> Option<(Vec<oxigraph::model::Quad>, Vec<oxigraph::model::Quad>)> {
    let anchor = record.anchor.as_ref()?;
    let log = store.changes();
    if !log.enabled() || anchor.epoch != log.epoch() {
        return None;
    }
    let hashed = graphs.iter().all(|g| {
        record
            .source_map
            .iter()
            .any(|m| m.source_graph == *g && m.content_hash.is_some())
    });
    if !hashed {
        return None;
    }
    let head = log.head_seq();
    let chain = log.chain(graphs, &anchor.epoch, anchor.seq, head);
    if !chain.intact {
        return None;
    }
    let fold = crate::store::changes::fold_rows(&chain.rows);
    let (mut added, mut removed) = (Vec::new(), Vec::new());
    for g in graphs {
        if let Some(net) = fold.get(&Some(g.clone())) {
            let undo = net.clone().inverse();
            added.extend(undo.added);
            removed.extend(undo.removed);
        }
    }
    (added.len() + removed.len() <= crate::store::engine::apply_delta_tx_max_quads())
        .then_some((added, removed))
}

/// Apply a tier-1 inverse and check each restored graph against the content
/// hash the version recorded for it. `Ok(false)` when one does not match — a
/// write landed between the plan and the apply — and tier 2, which copies
/// the version's graphs over the live ones, must finish the job.
fn restore_by_inverse(
    store: &crate::store::TripleStore,
    mappings: &[super::models::GraphMapping],
    (added, removed): (Vec<oxigraph::model::Quad>, Vec<oxigraph::model::Quad>),
) -> Result<bool, AppError> {
    store
        .apply_delta(&added, &removed, true)
        .map_err(AppError::from)?;
    for m in mappings {
        let Some(expected) = m.content_hash.as_deref() else {
            return Ok(false);
        };
        let (_, hash) = hash_graph(store, &m.source_graph, &m.source_graph)?;
        if hash.to_hex() != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

// ─── branches ─────────────────────────────────────────────────────────────

/// GET /api/datasets/:id/branches
pub async fn list_branches(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    let uid = user.as_deref().map(|u| u.user_id.as_str());
    require_read(&state, &ds, uid)?;

    let versions = registry::list_versions(&state.store, &state.base_url, &id);
    let (latest_pub, _) = registry::get_pointers(&state.store, &state.base_url, &id);
    let base_record = latest_pub
        .as_deref()
        .and_then(|v| versions.iter().find(|x| x.version == v))
        .cloned();

    use std::collections::HashMap;
    let mut tips: HashMap<String, DatasetVersion> = HashMap::new();
    for v in &versions {
        let key = v.branch.clone().unwrap_or_else(|| "main".to_string());
        match tips.get(&key) {
            Some(existing) if existing.created_at >= v.created_at => {}
            _ => {
                tips.insert(key, v.clone());
            }
        }
    }

    let mut out: Vec<DatasetBranchInfo> = Vec::with_capacity(tips.len());
    for (branch, tip) in tips {
        let (ahead, behind) = match &base_record {
            Some(base) if base.version != tip.version => {
                version_delta_counts(&state, base, &tip, uid.is_none()).await?
            }
            _ => (0, 0),
        };
        out.push(DatasetBranchInfo {
            branch,
            tip_version: tip.version.clone(),
            status: tip.status.as_str().to_string(),
            base_version: tip.derived_from.clone(),
            owner: tip.created_by.clone(),
            created_at: tip.created_at.clone(),
            ahead,
            behind,
        });
    }
    out.sort_by(|a, b| a.branch.cmp(&b.branch));
    Ok(Json(out))
}

/// `(ahead, behind)` of `to` against `from`: from the change log when both
/// are anchored on one chain (delta note §3.5), else by comparing their
/// content — building a delta for an authenticated caller; for an anonymous
/// one an unbuilt delta counts as unknown, `(0, 0)`.
async fn version_delta_counts(
    state: &AppState,
    from: &DatasetVersion,
    to: &DatasetVersion,
    anonymous: bool,
) -> Result<(usize, usize), AppError> {
    let graphs = from.covered_graphs();
    if same_cover(from, to) {
        if let (Some(a), Some(b)) = (&from.anchor, &to.anchor) {
            if let Some(fold) = chain_delta(&state.store, a, b, &graphs) {
                return Ok(fold.values().fold((0, 0), |(ad, rm), n| {
                    (ad + n.added.len(), rm + n.removed.len())
                }));
            }
        }
    }
    for v in [from, to] {
        if v.is_delta() && !materialised(state, v) {
            if anonymous {
                return Ok((0, 0));
            }
            materialise::ensure_built(state, v, false).await?;
        }
    }
    Ok(triple_delta(
        &state.store,
        &from.snapshot_graphs,
        &to.snapshot_graphs,
    ))
}

fn materialised(state: &AppState, v: &DatasetVersion) -> bool {
    !v.is_delta()
        || state
            .store
            .changes()
            .materialisation(&v.graph_iri)
            .is_some()
}

/// Whether two versions cover the same source graphs.
fn same_cover(a: &DatasetVersion, b: &DatasetVersion) -> bool {
    let mut x = a.covered_graphs();
    let mut y = b.covered_graphs();
    x.sort();
    y.sort();
    x == y
}

/// The net change from the content at anchor `from` to the content at `to`
/// for `graphs`, folded from the change log: both in one epoch and the rows
/// between them intact for those graphs, else `None`. Either direction —
/// going back in sequence is the inverse of going forward.
fn chain_delta(
    store: &crate::store::TripleStore,
    from: &super::models::Anchor,
    to: &super::models::Anchor,
    graphs: &[String],
) -> Option<std::collections::BTreeMap<Option<String>, crate::store::changes::NetDelta>> {
    if from.epoch != to.epoch {
        return None;
    }
    let (lo, hi, backwards) = if from.seq <= to.seq {
        (from.seq, to.seq, false)
    } else {
        (to.seq, from.seq, true)
    };
    let chain = store.changes().chain(graphs, &from.epoch, lo, hi);
    if !chain.intact {
        return None;
    }
    let mut fold = crate::store::changes::fold_rows(&chain.rows);
    if backwards {
        for v in fold.values_mut() {
            *v = std::mem::take(v).inverse();
        }
    }
    Some(fold)
}

/// The live graphs' place in the change log, when capture records every
/// write (otherwise the live graphs may have moved without a row).
fn live_anchor(store: &crate::store::TripleStore) -> Option<super::models::Anchor> {
    let log = store.changes();
    log.enabled().then(|| super::models::Anchor {
        epoch: log.epoch().to_string(),
        seq: log.head_seq(),
    })
}

/// POST /api/datasets/:id/branches
///
/// A branch is the version it starts from under a new label. When that
/// version is anchored it is recorded as a delta on the same base — no data
/// is copied (delta note §3.3); otherwise its snapshots are copied as before.
pub async fn create_branch(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<CreateDatasetBranchRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ds = load_dataset(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let branch = body.branch.trim().to_string();
    if branch.is_empty() {
        return Err(AppError::BadRequest("branch name is required".to_string()));
    }
    let version_lock = lock::dataset(&id);
    let _held = version_lock.lock().await;

    let from = registry::get_version(&state.store, &state.base_url, &id, &body.from_version)
        .ok_or_else(|| AppError::NotFound(format!("Version '{}' not found", body.from_version)))?;

    let target_ver = body
        .target_version
        .clone()
        .unwrap_or_else(|| format!("{}-{}", body.from_version, branch));
    validate_version(&target_ver)?;
    if registry::version_exists(&state.store, &state.base_url, &id, &target_ver) {
        return Err(AppError::BadRequest(format!(
            "Version '{target_ver}' already exists"
        )));
    }

    let st = state.clone();
    let creator = Some(format!("{}/users/{}", state.base_url, user.user_id));
    let tv = target_ver.clone();
    let pin = registry::dataset_pin(&state.auth_db, &id);
    let record = tokio::task::spawn_blocking(move || {
        cut::branch(&st.store, &st.base_url, &from, &tv, &branch, creator, pin)
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
    .map_err(AppError::Internal)?;
    version_commit(
        &state,
        &user.user_id,
        &id,
        Some(&record.version),
        format!("Branched {} from {}", record.version, body.from_version),
    );
    Ok((StatusCode::CREATED, Json(record)))
}

/// Commit-trail entry for a version operation, scoped to the dataset's
/// registered graphs (so `GET …/commits` lists it) plus the version's snapshot
/// graphs when it has any. Best-effort.
fn version_commit(
    state: &AppState,
    user_id: &str,
    dataset_id: &str,
    version: Option<&str>,
    message: String,
) {
    version_commit_stamped(state, None, user_id, dataset_id, version, message)
}

/// As [`version_commit`], for an operation that wrote under `stamp` (a
/// restore): the commit points at its rows in the change log.
fn version_commit_stamped(
    state: &AppState,
    stamp: Option<&crate::commit_log::CommitStamp>,
    user_id: &str,
    dataset_id: &str,
    version: Option<&str>,
    message: String,
) {
    let mut affected = state
        .auth_db
        .list_dataset_graphs(dataset_id)
        .unwrap_or_default();
    if let Some(v) = version {
        if let Some(rec) = registry::get_version(&state.store, &state.base_url, dataset_id, v) {
            affected.extend(rec.snapshot_graphs.iter().cloned());
        }
    }
    affected.sort();
    affected.dedup();
    let stamp = stamp
        .cloned()
        .unwrap_or_else(|| crate::commit_log::CommitStamp::new(&state.base_url));
    crate::commit_log::record_stamped(
        &state.store,
        &state.base_url,
        &stamp,
        crate::commit_log::CommitKind::Dataset,
        message,
        Some(user_id),
        Some(format!(
            "{}/dataset/{}",
            state.base_url.trim_end_matches('/'),
            dataset_id
        )),
        affected,
        0,
        0,
        version.map(str::to_string),
    );
}

// ─── Retention: delete / diff / gc ────────────────────────────────────────────

/// Drop a version's snapshot graphs and its version-scoped validation graph,
/// then its registry triples. Returns the graphs dropped.
fn purge_version(
    state: &AppState,
    dataset_id: &str,
    record: &DatasetVersion,
) -> Result<Vec<String>, AppError> {
    let mut dropped: Vec<String> = record.snapshot_graphs.clone();
    dropped.push(crate::shacl_studio::bindings::version_validation_graph(
        &state.base_url,
        dataset_id,
        &record.version,
    ));
    let refs: Vec<&str> = dropped.iter().map(String::as_str).collect();
    state
        .store
        .bulk_delete_graphs(&refs)
        .map_err(AppError::from)?;
    registry::delete_version(&state.store, &state.base_url, dataset_id, &record.version)
        .map_err(AppError::from)?;
    // A delta's build (dropped above with its graphs) and the pin on its
    // range: forgotten, so retention may reclaim the rows.
    materialise::release(&state.store, record, true);
    Ok(dropped)
}

/// Every version of a dataset, purged: snapshot and built graphs, validation
/// graphs, registry records, pins. Dataset deletion calls it, and the admin
/// sweep for versions whose dataset is gone; before, a deleted dataset's
/// versions stayed behind — and a new dataset reusing its id saw them.
pub fn purge_all_for_dataset(state: &AppState, dataset_id: &str) -> Result<usize, AppError> {
    let versions = registry::list_versions(&state.store, &state.base_url, dataset_id);
    for v in &versions {
        purge_version(state, dataset_id, v)?;
    }
    // The dataset's own registry triples (latest pointers, hasVersion).
    let ds_iri = format!("{}/dataset/{dataset_id}", state.base_url);
    let _ = state.store.update(&format!(
        "DELETE WHERE {{ GRAPH <{}> {{ <{}> ?p ?o }} }}",
        registry::REGISTRY_GRAPH,
        crate::store::escape_sparql_iri(&ds_iri)
    ));
    Ok(versions.len())
}

fn dataset_or_404(state: &AppState, id: &str) -> Result<Dataset, AppError> {
    state
        .auth_db
        .get_dataset(id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Dataset '{id}' not found")))
}

/// `DELETE /api/datasets/:id/versions/:ver[?force=true]` — remove a version
/// and reclaim its snapshot graphs. Every replace-import used to leave a full
/// copy of each changed graph behind with no way to delete it, so N re-imports
/// retained N copies. A published version is refused (409) unless forced:
/// deprecate it first.
pub async fn delete_version(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((id, ver)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Result<impl IntoResponse, AppError> {
    let ds = dataset_or_404(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let version_lock = lock::dataset(&id);
    let _held = version_lock.lock().await;
    let record = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    // A checkpoint a delta chains from is that delta's data (delta note §3.7).
    if !record.is_delta() {
        let dependants = registry::versions_based_on(&state.store, &state.base_url, &id, &ver);
        if !dependants.is_empty() {
            return Err(AppError::Conflict(json!({
                "error": "delta versions chain from this checkpoint; delete or publish them first",
                "version": ver,
                "dependants": dependants,
            })));
        }
    }
    let force = q
        .get("force")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    if record.status == VersionStatus::Published && !force {
        return Err(AppError::Conflict(json!({
            "error": "version is published; deprecate it first, or pass ?force=true",
            "version": ver,
        })));
    }
    let dropped = purge_version(&state, &id, &record)?;
    version_commit(
        &state,
        &user.user_id,
        &id,
        None,
        format!("Deleted version {ver}"),
    );
    Ok(Json(json!({ "deleted": ver, "graphs_dropped": dropped })))
}

/// `GET /api/datasets/:id/versions/:ver/diff/:other` — the per-graph triple
/// delta from `ver` to `other`, where `other` is another version or `live`
/// (the dataset's current graphs). `added`/`removed` are counted from `ver`'s
/// point of view. There was no way to see what changed between two versions
/// short of exporting both.
#[derive(Debug, Default, serde::Deserialize)]
pub struct DiffParams {
    /// `rdf-patch` for the diff as an RDF Patch document (also via
    /// `Accept: application/rdf-patch`).
    pub format: Option<String>,
}

pub async fn diff_versions(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((id, ver, other)): Path<(String, String, String)>,
    axum::extract::Query(params): axum::extract::Query<DiffParams>,
    headers: axum::http::HeaderMap,
) -> Result<axum::response::Response, AppError> {
    let want_patch = params
        .format
        .as_deref()
        .is_some_and(|f| f.eq_ignore_ascii_case("rdf-patch"))
        || headers
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("rdf-patch"));
    let ds = dataset_or_404(&state, &id)?;
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    require_read(&state, &ds, uid)?;
    let from = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| AppError::NotFound(format!("Version '{ver}' not found")))?;
    // A version snapshot copies private graphs too, and the `live` side reads the
    // live source graphs directly. Hide every graph whose live source is private
    // from a non-writer (viewer, or anonymous on a public dataset), so the diff —
    // rdf-patch triples or the JSON add/remove counts — cannot expose one.
    let hidden = private_source_filter(&state, &ds, uid)?;
    let visible = |src: &str| hidden.as_ref().is_none_or(|p| !p.contains(src));
    let other_record = if other == "live" {
        None
    } else {
        Some(
            registry::get_version(&state.store, &state.base_url, &id, &other)
                .ok_or_else(|| AppError::NotFound(format!("Version '{other}' not found")))?,
        )
    };

    // From the change log when both sides sit on one chain: O(change), not
    // O(both sides), and nothing to build (delta note §3.5).
    let graphs: Vec<String> = from
        .covered_graphs()
        .into_iter()
        .filter(|g| visible(g))
        .collect();
    let to_anchor = match &other_record {
        None => live_anchor(&state.store),
        Some(t) if same_cover(&from, t) => t.anchor.clone(),
        Some(_) => None,
    };
    if let Some(fold) = from
        .anchor
        .as_ref()
        .zip(to_anchor.as_ref())
        .and_then(|(a, b)| chain_delta(&state.store, a, b, &graphs))
    {
        let dataset_iri = format!("{}/dataset/{}", state.base_url.trim_end_matches('/'), id);
        if want_patch {
            return Ok((
                StatusCode::OK,
                [(
                    axum::http::header::CONTENT_TYPE,
                    crate::rdf_patch::MEDIA_TYPE,
                )],
                chain_patch(&dataset_iri, &ver, &other, &graphs, &fold),
            )
                .into_response());
        }
        let (mut added, mut removed) = (0usize, 0usize);
        let per_graph: Vec<serde_json::Value> = graphs
            .iter()
            .map(|g| {
                let (a, r) = fold
                    .get(&Some(g.clone()))
                    .map(|n| (n.added.len(), n.removed.len()))
                    .unwrap_or((0, 0));
                added += a;
                removed += r;
                json!({ "source_graph": g, "added": a, "removed": r, "missing_on_other_side": false })
            })
            .collect();
        return Ok(Json(json!({
            "dataset_id": id,
            "from": ver,
            "to": other,
            "added": added,
            "removed": removed,
            "graphs": per_graph,
            "source": "chain",
        }))
        .into_response());
    }
    // Otherwise compare content: build the delta sides first.
    materialise::ensure_built(&state, &from, uid.is_none()).await?;
    if let Some(t) = &other_record {
        materialise::ensure_built(&state, t, uid.is_none()).await?;
    }
    // (source graph, graph holding the "to" side)
    let to_side: Vec<(String, String)> = if other == "live" {
        from.source_map
            .iter()
            .filter(|m| visible(&m.source_graph))
            .map(|m| (m.source_graph.clone(), m.source_graph.clone()))
            .collect()
    } else {
        let to = other_record
            .clone()
            .ok_or_else(|| AppError::NotFound(format!("Version '{other}' not found")))?;
        to.source_map
            .iter()
            .filter(|m| visible(&m.source_graph))
            .map(|m| (m.source_graph.clone(), m.snapshot_graph.clone()))
            .collect()
    };
    if want_patch {
        // (target = the live source graph, from = the version's snapshot, to = the other side)
        let mut mappings: Vec<(String, Option<String>, Option<String>)> = from
            .source_map
            .iter()
            .filter(|m| visible(&m.source_graph))
            .map(|m| {
                let to_graph = to_side
                    .iter()
                    .find(|(src, _)| *src == m.source_graph)
                    .map(|(_, g)| g.clone());
                (
                    m.source_graph.clone(),
                    Some(m.snapshot_graph.clone()),
                    to_graph,
                )
            })
            .collect();
        for (src, g) in &to_side {
            if !from.source_map.iter().any(|m| m.source_graph == *src) {
                mappings.push((src.clone(), None, Some(g.clone())));
            }
        }
        let dataset_iri = format!("{}/dataset/{}", state.base_url.trim_end_matches('/'), id);
        let text = crate::rdf_patch::generate(
            &state.store,
            &[
                ("dataset", dataset_iri.as_str()),
                ("from", ver.as_str()),
                ("to", other.as_str()),
            ],
            &mappings,
        );
        return Ok((
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                crate::rdf_patch::MEDIA_TYPE,
            )],
            text,
        )
            .into_response());
    }
    let delta =
        |a: &[String], b: &[String]| crate::data_models::diff::triple_delta(&state.store, a, b);
    let (mut added, mut removed) = (0usize, 0usize);
    let mut graphs = Vec::new();
    for m in &from.source_map {
        if !visible(&m.source_graph) {
            continue;
        }
        let to_graph = to_side
            .iter()
            .find(|(src, _)| *src == m.source_graph)
            .map(|(_, g)| g.clone());
        let (a, r) = match &to_graph {
            Some(g) => delta(
                std::slice::from_ref(&m.snapshot_graph),
                std::slice::from_ref(g),
            ),
            None => delta(std::slice::from_ref(&m.snapshot_graph), &[]),
        };
        added += a;
        removed += r;
        graphs.push(json!({
            "source_graph": m.source_graph,
            "added": a,
            "removed": r,
            "missing_on_other_side": to_graph.is_none(),
        }));
    }
    for (src, g) in &to_side {
        if !from.source_map.iter().any(|m| m.source_graph == *src) {
            let (a, r) = delta(&[], std::slice::from_ref(g));
            added += a;
            removed += r;
            graphs.push(json!({ "source_graph": src, "added": a, "removed": r, "missing_on_other_side": false, "only_on_other_side": true }));
        }
    }
    Ok(Json(json!({
        "dataset_id": id,
        "from": ver,
        "to": other,
        "added": added,
        "removed": removed,
        "graphs": graphs,
        "source": "materialised",
    }))
    .into_response())
}

/// A chain diff as an RDF Patch against the live source graphs: per graph,
/// the removed quads as `D` lines, then the added ones as `A`.
fn chain_patch(
    dataset_iri: &str,
    from: &str,
    to: &str,
    graphs: &[String],
    fold: &std::collections::BTreeMap<Option<String>, crate::store::changes::NetDelta>,
) -> String {
    use crate::rdf_patch::{Op, Patch, QuadText};
    let mut ops = Vec::new();
    for g in graphs {
        let Some(net) = fold.get(&Some(g.clone())) else {
            continue;
        };
        let mut removed: Vec<QuadText> = net.removed.iter().map(QuadText::from_quad).collect();
        let mut added: Vec<QuadText> = net.added.iter().map(QuadText::from_quad).collect();
        let key = |q: &QuadText| format!("{} {} {}", q.s, q.p, q.o);
        removed.sort_by_key(key);
        added.sort_by_key(key);
        ops.extend(removed.into_iter().map(Op::Delete));
        ops.extend(added.into_iter().map(Op::Add));
    }
    Patch {
        headers: vec![
            (
                "id".to_string(),
                format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            ),
            ("dataset".to_string(), dataset_iri.to_string()),
            ("from".to_string(), from.to_string()),
            ("to".to_string(), to.to_string()),
        ],
        prefixes: Vec::new(),
        ops,
        aborted: false,
    }
    .render()
}

/// `POST /api/datasets/:id/versions/gc` — retention. Keeps the newest `keep`
/// non-published versions and deletes the rest (snapshots included).
/// Published versions are never collected; deprecate and delete them
/// explicitly. Nor is a checkpoint a surviving delta chains from: deltas go
/// first, then the checkpoints nothing chains from any more (delta note
/// §3.7); a checkpoint kept for that reason is reported.
pub async fn gc_versions(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(body): Json<GcDatasetVersionsRequest>,
) -> Result<impl IntoResponse, AppError> {
    let ds = dataset_or_404(&state, &id)?;
    require_write(&state, &ds, &user.user_id)?;
    let version_lock = lock::dataset(&id);
    let _held = version_lock.lock().await;
    let mut candidates: Vec<DatasetVersion> =
        registry::list_versions(&state.store, &state.base_url, &id)
            .into_iter()
            .filter(|v| v.status != VersionStatus::Published)
            .collect();
    // Newest first; the version label breaks ties within one timestamp.
    candidates.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.version.cmp(&a.version))
    });
    let doomed: Vec<DatasetVersion> = candidates.into_iter().skip(body.keep).collect();
    let mut deleted = Vec::new();
    let mut kept_as_base = Vec::new();
    for v in doomed.iter().filter(|v| v.is_delta()) {
        purge_version(&state, &id, v)?;
        deleted.push(v.version.clone());
    }
    for v in doomed.iter().filter(|v| !v.is_delta()) {
        if !registry::versions_based_on(&state.store, &state.base_url, &id, &v.version).is_empty() {
            kept_as_base.push(v.version.clone());
            continue;
        }
        purge_version(&state, &id, v)?;
        deleted.push(v.version.clone());
    }
    if !deleted.is_empty() {
        version_commit(
            &state,
            &user.user_id,
            &id,
            None,
            format!(
                "Garbage-collected {} version(s): {}",
                deleted.len(),
                deleted.join(", ")
            ),
        );
    }
    Ok(Json(
        json!({ "kept": body.keep, "deleted": deleted, "kept_as_base": kept_as_base }),
    ))
}

/// `POST /api/admin/versions/sweep` — what deleted datasets and interrupted
/// operations left behind: every version of a dataset that no longer exists
/// is purged (graphs, registry, pins); version graphs no registry record
/// names are dropped; pins of versions the registry no longer holds are
/// released; builds past their time-to-live are dropped. Admin only.
pub async fn admin_sweep_orphans(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<impl IntoResponse, AppError> {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Admin role required".into()));
    }
    let st = state.clone();
    let report = tokio::task::spawn_blocking(move || sweep_orphans(&st))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;
    Ok(Json(report))
}

fn sweep_orphans(state: &AppState) -> Result<serde_json::Value, AppError> {
    use oxigraph::sparql::QueryResults;
    use std::collections::HashSet;
    let reg = registry::REGISTRY_GRAPH;
    let term = |t: &oxigraph::model::Term| match t {
        oxigraph::model::Term::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    };
    // 1. Versions of datasets that are gone.
    let prefix = format!("{}/dataset/", state.base_url);
    let mut datasets: HashSet<String> = HashSet::new();
    if let Ok(QueryResults::Solutions(sols)) = state.store.query(&format!(
        "PREFIX ver: <urn:system:vocab/> SELECT DISTINCT ?ds WHERE {{ GRAPH <{reg}> {{ ?v ver:dataset ?ds }} }}"
    )) {
        for row in sols.flatten() {
            if let Some(iri) = row.get("ds").and_then(term) {
                if let Some(id) = iri.strip_prefix(&prefix) {
                    datasets.insert(id.to_string());
                }
            }
        }
    }
    let mut purged_datasets = Vec::new();
    for id in datasets {
        let gone = state
            .auth_db
            .get_dataset(&id)
            .map_err(|e| AppError::Internal(e.to_string()))?
            .is_none();
        if gone {
            purge_all_for_dataset(state, &id)?;
            purged_datasets.push(id);
        }
    }
    // 2. Version graphs no record names.
    let mut named: HashSet<String> = HashSet::new();
    if let Ok(QueryResults::Solutions(sols)) = state.store.query(&format!(
        "PREFIX ver: <urn:system:vocab/>
         SELECT ?v ?g WHERE {{ GRAPH <{reg}> {{
            ?v a ver:DatasetVersion .
            OPTIONAL {{ {{ ?v ver:subGraph ?g }} UNION {{ ?v ver:graphMap ?m . ?m ver:snapshotGraph ?g }} }}
         }} }}"
    )) {
        for row in sols.flatten() {
            if let Some(v) = row.get("v").and_then(term) {
                named.insert(format!("{v}/validation"));
            }
            if let Some(g) = row.get("g").and_then(term) {
                named.insert(g);
            }
        }
    }
    let orphans: Vec<String> = state
        .store
        .named_graphs()
        .map_err(AppError::from)?
        .into_iter()
        .map(|g| g.as_str().to_string())
        .filter(|g| g.starts_with(&prefix))
        .filter(|g| crate::store::changes::is_version_snapshot_graph(g) && !named.contains(g))
        .collect();
    if !orphans.is_empty() {
        let refs: Vec<&str> = orphans.iter().map(String::as_str).collect();
        state
            .store
            .bulk_delete_graphs(&refs)
            .map_err(AppError::from)?;
    }
    // 3-4. Stale pins, expired builds.
    let pins_released = materialise::reconcile_pins(&state.store);
    let builds_dropped = materialise::sweep(&state.store);
    Ok(json!({
        "purged_datasets": purged_datasets,
        "orphan_graphs_dropped": orphans,
        "pins_released": pins_released,
        "builds_dropped": builds_dropped,
    }))
}
