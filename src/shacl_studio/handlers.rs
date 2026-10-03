//! HTTP handlers for SHACL Studio: shape-graph Library CRUD + revisions, pipeline
//! CRUD + run + history, model-context / derive tooling, and the form platform
//! manifest. Error convention matches the existing SHACL handlers:
//! `Result<_, (StatusCode, String)>`.

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use bytes::Bytes;
use serde::Deserialize;
use uuid::Uuid;

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{OwnerType, Visibility};
use crate::server::AppState;

use super::access::*;
use super::models::*;
use super::read_scope::ReadScope;
use super::store::ShaclStudioStore;

type ApiErr = (StatusCode, String);

const EMPTY_SHAPES: &str = "# SHACL shapes\nPREFIX sh: <http://www.w3.org/ns/shacl#>\nPREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\nPREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nPREFIX xsd: <http://www.w3.org/2001/XMLSchema#>\n";

fn studio(state: &AppState) -> ShaclStudioStore {
    ShaclStudioStore::new(state.auth_db.pool())
}
fn org_ids(state: &AppState, uid: &str) -> Vec<String> {
    state.auth_db.get_user_org_ids(uid).unwrap_or_default()
}
fn e500<E: ToString>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
pub(crate) fn parse_visibility(s: &Option<String>) -> Visibility {
    s.as_deref()
        .and_then(Visibility::from_str)
        .unwrap_or(Visibility::Private)
}

/// Resolve the owner for a new artifact from the request, defaulting to the
/// current user. Organisation ownership requires membership.
pub(crate) fn resolve_owner(
    state: &AppState,
    user: &AuthenticatedUser,
    owner_type: &Option<String>,
    owner_id: &Option<String>,
) -> Result<(OwnerType, String), ApiErr> {
    match owner_type.as_deref().and_then(OwnerType::from_str) {
        Some(OwnerType::Organisation) | Some(OwnerType::Group) => {
            let oid = owner_id
                .clone()
                .ok_or((StatusCode::BAD_REQUEST, "owner_id required".into()))?;
            if !user.is_admin() && !org_ids(state, &user.user_id).iter().any(|o| o == &oid) {
                return Err((
                    StatusCode::FORBIDDEN,
                    "Not a member of the owning organisation".into(),
                ));
            }
            Ok((OwnerType::Organisation, oid))
        }
        _ => Ok((OwnerType::User, user.user_id.clone())),
    }
}

// ─── Graphs of registered model versions ─────────────────────────────────────
//
// A shape graph can be a graph of a registered model version: a seed bundle
// binds a model's graph in place (the nen2660-imbor bundle binds CROW's IMBOR
// Kern), and `register_shape_graph` adopts any readable graph. The Studio
// applies the registry's rules to such a graph, as the SPARQL and Graph Store
// paths do (`crate::data_models::write_guard`): a version whose licence allows
// no altered copies is never written, copied into an editable graph, or served
// altered; any other version with a licence record is marked possibly
// modified before a write.

/// A Studio write into `graph_iri` (a save, a restore, an import, the clear on
/// delete): refused (403) when the graph holds a registered model version
/// whose licence allows no altered copies. For any other version with a
/// licence record, the record stops calling the content unchanged before the
/// write runs, so a write cut short never leaves a false "unchanged".
fn guard_registry_graph_write(state: &AppState, graph_iri: &str) -> Result<(), ApiErr> {
    let v = crate::data_models::write_guard::check(&state.store, &state.base_url, graph_iri)
        .map_err(|m| (StatusCode::FORBIDDEN, m))?;
    if let Some(v) = v {
        crate::data_models::write_guard::mark(&state.store, std::slice::from_ref(&v))
            .map_err(e500)?;
        state.mark_vocab_registry_dirty();
    }
    Ok(())
}

/// Whether `user` may change the content of `graph_iri`, the graph of a
/// Library entry they manage, through the Studio. Managing the entry is not
/// enough on its own: an entry is also made for a graph that already exists
/// (`register_shape_graph`, a dataset's shapes graph, a seed bundle's
/// binding), and its owner is whoever made it. So the authority follows the
/// graph, as it does for a Graph Store write:
///
/// * an admin;
/// * a graph the Studio minted for its entry (`urn:shapes:…`): the entry is
///   its only ACL;
/// * a model-registry graph: whoever may write the registry entry holding it
///   (`can_write_ontology`); nobody else;
/// * a graph a dataset holds — in its namespace or registered to it — for
///   whoever may write that dataset (org members editing their dataset's
///   shapes graph, as `PUT /api/datasets/:id/shapes` allows them);
/// * any other graph: a graph-ACL write grant.
///
/// Evaluated at every write, so an entry made before this rule, or a grant
/// since revoked, gives no more than the rule does.
fn may_write_shape_graph_content(
    state: &AppState,
    user: &AuthenticatedUser,
    graph_iri: &str,
) -> Result<bool, ApiErr> {
    use crate::auth::dataset_graph;
    use crate::data_models::registry;
    if user.is_admin() || owns_backing_graph(graph_iri) {
        return Ok(true);
    }
    if dataset_graph::graph_held_by_model_registry(&state.store, &state.base_url, graph_iri) {
        let Some(id) = registry::data_model_holding_graph(&state.store, &state.base_url, graph_iri)
        else {
            return Ok(false);
        };
        let Some(entry) = registry::get_data_model(&state.store, &state.base_url, &id) else {
            return Ok(false);
        };
        return state
            .auth_db
            .can_write_ontology(
                &user.user_id,
                entry.owner_type.as_deref(),
                entry.owner_id.as_deref(),
            )
            .map_err(e500);
    }
    for id in dataset_graph::datasets_holding_graph(&state.auth_db, &state.base_url, graph_iri) {
        if let Some(ds) = state.auth_db.get_dataset(&id).map_err(e500)? {
            if state
                .auth_db
                .can_write_dataset(&user.user_id, &ds)
                .map_err(e500)?
            {
                return Ok(true);
            }
        }
    }
    Ok(dataset_graph::may_write_graph_directly(
        &state.auth_db,
        user,
        graph_iri,
    ))
}

/// A Studio write into `graph_iri` by `user` (a save, a restore, an import,
/// the clear on delete): refused (403) unless
/// [`may_write_shape_graph_content`] allows it, then the registry's licence
/// rule ([`guard_registry_graph_write`]).
fn guard_shape_graph_write(
    state: &AppState,
    user: &AuthenticatedUser,
    graph_iri: &str,
) -> Result<(), ApiErr> {
    if !may_write_shape_graph_content(state, user, graph_iri)? {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "You may not change graph <{graph_iri}>: managing its Library entry is not \
                 enough. Its content can be changed by an admin, by whoever may write the \
                 dataset or registry entry that holds it, or with write access to the graph."
            ),
        ));
    }
    guard_registry_graph_write(state, graph_iri)
}

/// Copying shapes out of `graph_iri` into an editable graph (a clone, an
/// import): refused (403) when the graph holds a registered model version
/// whose licence allows no altered copies, as the registry refuses a draft or
/// a branch of it: the editable copy would be one. Binding that shape graph to
/// a dataset only reads it and stays allowed.
fn refuse_no_derivatives_copy(state: &AppState, graph_iri: &str) -> Result<(), ApiErr> {
    match crate::data_models::registry::attributed_version_of_graph(
        &state.store,
        &state.base_url,
        graph_iri,
    ) {
        Some(v) if v.attribution.no_derivatives => Err((
            StatusCode::FORBIDDEN,
            format!(
                "Copying shapes out of graph <{graph_iri}> is refused: it holds version '{}' of \
                 registry entry '{}', {}, whose licence allows no altered copies, and an editable \
                 copy would be one. Bind that shape graph to your dataset instead: validation \
                 only reads it.",
                v.version,
                v.data_model_id,
                crate::data_models::vocab_files::source_phrase(&v.attribution.file)
            ),
        )),
        _ => Ok(()),
    }
}

/// Whether the Studio may serve `graph_iri`'s content to `user`. For a graph
/// of a registered version in an entry whose licence allows no altered
/// copies, the registry's rule applies (`data_models::handlers::
/// ensure_servable`): only a checked, unchanged copy goes to everyone; any
/// other state only to users who may write the entry. With `snapshot`, the
/// content is a stored revision, which no check vouches for, so it goes only
/// to those users whatever the record says.
fn ensure_studio_servable(
    state: &AppState,
    user: &AuthenticatedUser,
    graph_iri: &str,
    snapshot: bool,
) -> Result<(), ApiErr> {
    use crate::data_models::registry;
    let Some(v) = registry::attributed_version_of_graph(&state.store, &state.base_url, graph_iri)
    else {
        return Ok(());
    };
    let nd = registry::no_derivatives_attribution(&state.store, &state.base_url, &v.data_model_id);
    let Some(nd) = nd else {
        return Ok(());
    };
    let withheld = || {
        (
            StatusCode::FORBIDDEN,
            format!(
                "The content of graph <{graph_iri}> is withheld: it holds version '{}' of registry \
                 entry '{}', {}, whose licence allows no altered copies, and {}.",
                v.version,
                v.data_model_id,
                crate::data_models::vocab_files::source_phrase(&nd.file),
                if snapshot {
                    "no check vouches for a stored revision of it"
                } else {
                    "this copy is not a checked, unchanged copy of it"
                }
            ),
        )
    };
    let parent = registry::get_data_model(&state.store, &state.base_url, &v.data_model_id)
        .ok_or_else(withheld)?;
    if snapshot {
        let may_write = state
            .auth_db
            .can_write_ontology(
                &user.user_id,
                parent.owner_type.as_deref(),
                parent.owner_id.as_deref(),
            )
            .map_err(e500)?;
        return if may_write { Ok(()) } else { Err(withheld()) };
    }
    crate::data_models::handlers::ensure_servable(
        state,
        &v.data_model_id,
        &parent,
        &v.version,
        Some(user),
    )
    .map_err(|e| match e {
        crate::server::error::AppError::Forbidden(_) => withheld(),
        other => e500(other.message()),
    })
}

// ─── Shape graphs ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateShapeGraphBody {
    pub name: String,
    pub description: Option<String>,
    pub visibility: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
    pub turtle: Option<String>,
    pub source: Option<String>,
}

pub async fn create_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<CreateShapeGraphBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let (owner_type, owner_id) = resolve_owner(&state, &user, &body.owner_type, &body.owner_id)?;
    let source = body
        .source
        .as_deref()
        .map(ShapeSource::from_str_or_manual)
        .unwrap_or(ShapeSource::Manual);
    let turtle = body.turtle.unwrap_or_else(|| EMPTY_SHAPES.to_string());
    let set = create_shape_graph_from_turtle(
        &state,
        &user,
        &body.name,
        body.description.as_deref(),
        owner_type,
        &owner_id,
        parse_visibility(&body.visibility),
        &body.tags,
        source,
        &turtle,
        "Created",
    )?;
    Ok((StatusCode::CREATED, Json(set)))
}

/// Create a shape graph from Turtle: the registry row, the store graph, the
/// first revision and its commit. Shared by the create endpoint and the
/// specification importers.
#[allow(clippy::too_many_arguments)]
pub(crate) fn create_shape_graph_from_turtle(
    state: &AppState,
    user: &AuthenticatedUser,
    name: &str,
    description: Option<&str>,
    owner_type: OwnerType,
    owner_id: &str,
    visibility: Visibility,
    tags: &[String],
    source: ShapeSource,
    turtle: &str,
    note: &str,
) -> Result<ShapeGraph, ApiErr> {
    let st = studio(state);
    let graph_iri = format!("urn:shapes:{}", Uuid::new_v4());
    let set = st
        .create_shape_graph(
            name,
            description,
            owner_type,
            owner_id,
            visibility,
            &graph_iri,
            tags,
            source,
            Some(&user.user_id),
        )
        .map_err(e500)?;
    state
        .store
        .graph_store_put(Some(&graph_iri), turtle, oxigraph::io::RdfFormat::Turtle)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let (targets, count) = super::run::analyze_shapes_graph(&state.store, &graph_iri);
    let version = st
        .save_shape_graph_revision(
            &set.id,
            turtle,
            &targets,
            count,
            Some(note),
            Some(&user.user_id),
        )
        .map_err(e500)?;
    record_shape_graph_commit(
        state,
        &set.id,
        &graph_iri,
        version,
        Some(note),
        &user.user_id,
    );
    st.get_shape_graph(&set.id).map_err(e500)?.ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "shape graph missing right after creation".to_string(),
        )
    })
}

pub async fn list_shape_graphs(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiErr> {
    let st = studio(&state);
    let orgs = org_ids(&state, &user.user_id);
    let withheld = withheld_graphs(&state, &user)?;
    let sets: Vec<ShapeGraph> = st
        .list_shape_graphs()
        .map_err(e500)?
        .into_iter()
        .filter(|s| {
            can_access_set(s, Some(&user.user_id), &orgs) && !withheld.contains(&s.graph_iri)
        })
        .collect();
    Ok(Json(sets))
}

/// The graphs some dataset holds as private that `user` may not read
/// ([`crate::auth::acl::withheld_private_graphs`]). The Library adopts a
/// dataset's shapes graph in place, and an entry's visibility does not decide
/// who reads a graph the Studio did not mint: no entry of one of these is
/// served to `user`, whatever its visibility.
fn withheld_graphs(
    state: &AppState,
    user: &AuthenticatedUser,
) -> Result<std::collections::HashSet<String>, ApiErr> {
    crate::auth::acl::withheld_private_graphs(&state.auth_db, Some(user)).map_err(e500)
}

async fn load_set_checked(
    state: &AppState,
    user: &AuthenticatedUser,
    id: &str,
    need_manage: bool,
) -> Result<ShapeGraph, ApiErr> {
    let st = studio(state);
    let set = st
        .get_shape_graph(id)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Shape graph not found".into()))?;
    let orgs = org_ids(state, &user.user_id);
    let ok = if need_manage {
        can_manage_set(&set, Some(&user.user_id), &orgs, user.is_admin())
    } else {
        can_access_set(&set, Some(&user.user_id), &orgs)
    };
    // Reading or managing an entry is working with its graph: an entry of a
    // private dataset graph is only for who may read that graph.
    if !ok
        || crate::auth::acl::private_graph_withheld(&state.auth_db, Some(user), &set.graph_iri)
            .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".into()));
    }
    Ok(set)
}

pub async fn get_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, false).await?;
    Ok(Json(set))
}

#[derive(Deserialize)]
pub struct UpdateShapeGraphBody {
    pub name: String,
    pub description: Option<String>,
    pub visibility: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

pub async fn update_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateShapeGraphBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, true).await?;
    // The entry's visibility decides who may read its graph through the
    // Studio (`GET …/turtle`). For a graph the Studio did not mint, widening
    // it is a decision for whoever may change that graph.
    let visibility = parse_visibility(&body.visibility);
    if visibility != set.visibility
        && !owns_backing_graph(&set.graph_iri)
        && !may_write_shape_graph_content(&state, &user, &set.graph_iri)?
    {
        return Err((
            StatusCode::FORBIDDEN,
            format!(
                "You may not change who can read graph <{}> through the Library: it is not a \
                 graph the Studio made, and you may not change it.",
                set.graph_iri
            ),
        ));
    }
    studio(&state)
        .update_shape_graph_meta(
            &id,
            &body.name,
            body.description.as_deref(),
            visibility,
            &body.tags,
        )
        .map_err(e500)?;
    let set = studio(&state).get_shape_graph(&id).map_err(e500)?;
    Ok(Json(set))
}

pub async fn delete_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, true).await?;
    // Only a graph the Studio created (`urn:shapes:…`) is the shape graph's
    // own, and is cleared with it (best-effort). A graph the Library adopted
    // in place (a seed bundle's binding, `register_shape_graph`, a dataset's
    // shapes graph) belongs to someone else, perhaps to a registry model:
    // only the Library's rows go, never its data.
    if owns_backing_graph(&set.graph_iri) {
        guard_shape_graph_write(&state, &user, &set.graph_iri)?;
        let _ = state
            .store
            .update(&format!("CLEAR SILENT GRAPH <{}>", set.graph_iri));
    }
    studio(&state).delete_shape_graph(&id).map_err(e500)?;
    Ok(StatusCode::NO_CONTENT)
}

/// Whether a shape graph's backing graph is one the Studio minted for it
/// (create, clone), rather than an existing graph adopted in place.
fn owns_backing_graph(graph_iri: &str) -> bool {
    graph_iri.starts_with("urn:shapes:")
}

pub async fn get_shape_graph_turtle(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiErr> {
    let set = load_set_checked(&state, &user, &id, false).await?;
    ensure_studio_servable(&state, &user, &set.graph_iri, false)?;
    let want_shaclc = q.get("format").map(|v| v == "shaclc").unwrap_or(false)
        || headers
            .get(ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(|a| a.contains("text/shaclc"))
            .unwrap_or(false);
    if want_shaclc {
        let shaclc = crate::shaclc::serialize(&state.store, &set.graph_iri).map_err(e500)?;
        return Ok((StatusCode::OK, [(CONTENT_TYPE, "text/shaclc")], shaclc).into_response());
    }
    // Prefixed: this is the document a human reads in the source view, and the
    // one the visual builder scans for the prefixes it offers. Serialised
    // without a header it was a wall of full <http://…> IRIs.
    let data = state
        .store
        .dump_prefixed(
            oxigraph::io::RdfFormat::Turtle,
            Some(&set.graph_iri),
            |ns| state.prefix_registry.declaration_for(ns),
        )
        .map_err(e500)?;
    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], data).into_response())
}

/// Revision notes reach the commit log's RDF, so a caller-supplied one is
/// bounded and stripped of control characters before it gets there.
const MAX_REVISION_NOTE: usize = 200;

/// Normalise a caller-supplied revision message, or `None` if nothing is left.
fn revision_note(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_REVISION_NOTE)
        .collect();
    let trimmed = cleaned.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub async fn put_shape_graph_turtle(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, true).await?;
    let raw = String::from_utf8(body.to_vec())
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid UTF-8".into()))?;
    let ct = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");
    let turtle = if ct.contains("shaclc") {
        crate::shaclc::parse(&raw)
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("SHACLC parse error: {e}")))?
    } else {
        raw
    };
    // Every history entry used to read "Edited": the note was hard-coded here.
    let note = q.get("message").and_then(|m| revision_note(m));
    let version = write_shapes_revision(
        &state,
        &set.graph_iri,
        &id,
        &turtle,
        Some(note.as_deref().unwrap_or("Edited")),
        &user,
    )?;
    Ok(Json(serde_json::json!({ "version": version })))
}

/// Write Turtle to a shape graph's graph, recompute facets, and snapshot it.
/// The caller has checked that `by` manages the entry; whether they may
/// change its graph is checked here ([`guard_shape_graph_write`]).
fn write_shapes_revision(
    state: &AppState,
    graph_iri: &str,
    set_id: &str,
    turtle: &str,
    note: Option<&str>,
    user: &AuthenticatedUser,
) -> Result<i64, ApiErr> {
    guard_shape_graph_write(state, user, graph_iri)?;
    let by = user.user_id.as_str();
    state
        .store
        .graph_store_put(Some(graph_iri), turtle, oxigraph::io::RdfFormat::Turtle)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let (targets, count) = super::run::analyze_shapes_graph(&state.store, graph_iri);
    let version = studio(state)
        .save_shape_graph_revision(set_id, turtle, &targets, count, note, Some(by))
        .map_err(e500)?;
    record_shape_graph_commit(state, set_id, graph_iri, version, note, by);
    Ok(version)
}

/// IRI a shape graph is addressed by in the commit trail.
fn shape_graph_subject_iri(base_url: &str, set_id: &str) -> String {
    format!(
        "{}/shacl/shape-graphs/{}",
        base_url.trim_end_matches('/'),
        set_id
    )
}

/// Record a shape-graph change in the shared commit trail (kind = Shapes). The
/// backing shapes graph is the affected graph; the prior version is the parent.
/// Best-effort: a failed commit must never abort the change that persisted.
fn record_shape_graph_commit(
    state: &AppState,
    set_id: &str,
    graph_iri: &str,
    version: i64,
    note: Option<&str>,
    by: &str,
) {
    let msg = match note.map(str::trim) {
        Some(m) if !m.is_empty() => m.to_string(),
        _ => format!("Revision {version}"),
    };
    let mut rec = crate::commit_log::CommitRecord::new(crate::commit_log::CommitKind::Shapes, msg);
    rec.actor_iri = Some(format!("{}/users/{}", state.base_url, by));
    rec.subject_iri = Some(shape_graph_subject_iri(&state.base_url, set_id));
    rec.version = Some(version.to_string());
    rec.revision = Some(version.to_string());
    if version > 1 {
        rec.parent_revision = Some((version - 1).to_string());
    }
    rec.affected_graphs = vec![graph_iri.to_string()];
    if let Err(e) = crate::commit_log::insert_commit(&state.store, &state.base_url, &rec) {
        tracing::warn!("failed to record shape-graph commit for {set_id} v{version}: {e}");
    }
}

pub async fn list_shape_graph_revisions(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    load_set_checked(&state, &user, &id, false).await?;
    let revs = studio(&state)
        .list_shape_graph_revisions(&id)
        .map_err(e500)?;
    Ok(Json(revs))
}

pub async fn get_shape_graph_revision(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((id, rev)): Path<(String, i64)>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, false).await?;
    ensure_studio_servable(&state, &user, &set.graph_iri, true)?;
    let rev = studio(&state)
        .get_shape_graph_revision(&id, rev)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Revision not found".into()))?;
    Ok(Json(rev))
}

pub async fn restore_shape_graph_revision(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((id, rev)): Path<(String, i64)>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, true).await?;
    let snapshot = studio(&state)
        .get_shape_graph_revision(&id, rev)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Revision not found".into()))?;
    let version = write_shapes_revision(
        &state,
        &set.graph_iri,
        &id,
        &snapshot.turtle,
        Some(&format!("Restored revision {rev}")),
        &user,
    )?;
    Ok(Json(serde_json::json!({ "version": version })))
}

#[derive(Deserialize)]
pub struct CloneShapeGraphBody {
    pub name: Option<String>,
}

pub async fn clone_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CloneShapeGraphBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let src = load_set_checked(&state, &user, &id, false).await?;
    refuse_no_derivatives_copy(&state, &src.graph_iri)?;
    let st = studio(&state);
    let turtle = state
        .store
        .graph_store_get(Some(&src.graph_iri), oxigraph::io::RdfFormat::Turtle)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_else(|| EMPTY_SHAPES.to_string());
    let graph_iri = format!("urn:shapes:{}", Uuid::new_v4());
    let name = body.name.unwrap_or_else(|| format!("{} (copy)", src.name));
    let set = st
        .create_shape_graph(
            &name,
            src.description.as_deref(),
            OwnerType::User,
            &user.user_id,
            Visibility::Private,
            &graph_iri,
            &src.tags,
            ShapeSource::Manual,
            Some(&user.user_id),
        )
        .map_err(e500)?;
    write_shapes_revision(
        &state,
        &graph_iri,
        &set.id,
        &turtle,
        Some(&format!("Cloned from {}", src.id)),
        &user,
    )?;
    let set = st.get_shape_graph(&set.id).map_err(e500)?;
    Ok((StatusCode::CREATED, Json(set)))
}

// ─── Meta-validation (SHACL-of-SHACL) ────────────────────────────────────────

/// Validate a shape graph's Turtle *as data* against the built-in SHACL-SHACL
/// meta-shapes. Reuses the shared validation plumbing — no pipeline row, no
/// persisted run. Read access to the set is sufficient.
pub async fn validate_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, false).await?;
    let store = state.store.clone();
    let data_graph = set.graph_iri.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        super::run::run_validation(
            &store,
            &[super::seed::SHACL_SHACL_GRAPH.to_string()],
            &[data_graph],
            SeverityThreshold::Violation,
            false,
        )
    })
    .await
    .map_err(e500)?
    .map_err(e500)?;

    Ok(Json(serde_json::json!({
        "shape_graph_id": id,
        "conforms": outcome.report.conforms,
        "passes": outcome.passes,
        "results_count": outcome.report.results_count,
        "violation_count": outcome.violation_count,
        "warning_count": outcome.warning_count,
        "info_count": outcome.info_count,
        "report": outcome.report,
    })))
}

// ─── Lifecycle & history ─────────────────────────────────────────────────────

/// Transition a shape graph's lifecycle status. Gating is independent of status
/// (a Draft set still gates) — these endpoints only express publication intent
/// to consumers (form platform, downstream pipelines). Mirrors the dataset-version
/// lifecycle, minus branches/latest-published pointers (shape graphs have neither).
async fn transition_shape_graph(
    state: &AppState,
    user: &AuthenticatedUser,
    id: &str,
    to: VersionStatus,
) -> Result<Response, ApiErr> {
    let set = load_set_checked(state, user, id, true).await?;
    let allowed = match to {
        VersionStatus::Staged => matches!(set.status, VersionStatus::Draft),
        VersionStatus::Published => {
            matches!(set.status, VersionStatus::Draft | VersionStatus::Staged)
        }
        VersionStatus::Deprecated => !matches!(set.status, VersionStatus::Deprecated),
        VersionStatus::Draft => false,
    };
    if !allowed {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Cannot transition shape graph from {} to {}",
                set.status.as_str(),
                to.as_str()
            ),
        ));
    }
    studio(state).set_shape_graph_status(id, to).map_err(e500)?;
    Ok(Json(serde_json::json!({ "status": to.as_str() })).into_response())
}

pub async fn stage_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    transition_shape_graph(&state, &user, &id, VersionStatus::Staged).await
}

pub async fn publish_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    transition_shape_graph(&state, &user, &id, VersionStatus::Published).await
}

pub async fn deprecate_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    transition_shape_graph(&state, &user, &id, VersionStatus::Deprecated).await
}

/// GET /api/shacl/shape-graphs/:id/commits — the shape graph's slice of the shared
/// commit trail (kind = Shapes), newest first, with actor names resolved.
pub async fn list_shape_graph_commits(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<crate::commit_log::CommitsParams>,
) -> Result<impl IntoResponse, ApiErr> {
    load_set_checked(&state, &user, &id, false).await?;
    let subject = shape_graph_subject_iri(&state.base_url, &id);
    let scope = crate::commit_log::CommitScope::Subject(subject);
    let mut commits = crate::commit_log::list_commits(&state.store, &scope, &params.to_query());
    crate::commit_log::resolve_actors(state.auth_db.as_ref(), &mut commits);
    Ok(Json(commits))
}

// ─── Bindings (the validation layer) ─────────────────────────────────────────

#[derive(Deserialize)]
pub struct BindingBody {
    pub target: ValidationTarget,
    /// The shape graph that validates the target.
    pub shape_graph_id: String,
}

#[derive(Deserialize)]
pub struct BindingQuery {
    pub target_kind: Option<String>,
    pub target_id: Option<String>,
    /// Reverse lookup: which targets does this shape graph validate?
    pub shape_graph_id: Option<String>,
}

/// Resolve a target to its validation-layer IRI, checking the caller may bind
/// to it. Dataset → write on the dataset; Graph → graph-level write ACL (same
/// gate as a Graph Store write); ShapeGraph → manage on the shape graph.
async fn resolve_target_for_write(
    state: &AppState,
    user: &AuthenticatedUser,
    target: &ValidationTarget,
) -> Result<String, ApiErr> {
    match target.kind {
        TargetKind::Dataset => {
            let ds = state
                .auth_db
                .get_dataset(&target.id)
                .map_err(e500)?
                .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
            if !state
                .auth_db
                .can_write_dataset(&user.user_id, &ds)
                .map_err(e500)?
            {
                return Err((
                    StatusCode::FORBIDDEN,
                    "Write access denied for dataset".into(),
                ));
            }
            Ok(super::bindings::dataset_target_iri(
                &state.base_url,
                &target.id,
            ))
        }
        TargetKind::Graph => {
            if !crate::auth::acl::check_graph_permission(
                Some(user),
                &target.id,
                "write",
                &state.auth_db,
            ) {
                return Err((
                    StatusCode::FORBIDDEN,
                    format!("Write access denied for graph <{}>", target.id),
                ));
            }
            Ok(target.id.clone())
        }
        TargetKind::ShapeGraph => {
            let set = load_set_checked(state, user, &target.id, true).await?;
            Ok(set.graph_iri)
        }
    }
}

/// Read-side target resolution: Dataset/ShapeGraph require access; a graph's
/// bindings are low-sensitivity metadata, so any authenticated caller may read.
async fn resolve_target_for_read(
    state: &AppState,
    user: &AuthenticatedUser,
    target: &ValidationTarget,
) -> Result<String, ApiErr> {
    match target.kind {
        TargetKind::Dataset => {
            let ds = state
                .auth_db
                .get_dataset(&target.id)
                .map_err(e500)?
                .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
            if !state
                .auth_db
                .can_access_dataset(Some(&user.user_id), &ds)
                .map_err(e500)?
            {
                return Err((StatusCode::FORBIDDEN, "Access denied".into()));
            }
            Ok(super::bindings::dataset_target_iri(
                &state.base_url,
                &target.id,
            ))
        }
        TargetKind::Graph => Ok(target.id.clone()),
        TargetKind::ShapeGraph => {
            let set = load_set_checked(state, user, &target.id, false).await?;
            Ok(set.graph_iri)
        }
    }
}

/// Record a binding change in the shape graph's commit history (the binding lives
/// in the validation-layer graph). Best-effort.
fn record_binding_commit(
    state: &AppState,
    set: &ShapeGraph,
    target_iri: &str,
    added: bool,
    by: &str,
) {
    let verb = if added { "Bound to" } else { "Unbound from" };
    let mut rec = crate::commit_log::CommitRecord::new(
        crate::commit_log::CommitKind::Shapes,
        format!("{verb} {target_iri}"),
    );
    rec.actor_iri = Some(format!("{}/users/{}", state.base_url, by));
    rec.subject_iri = Some(shape_graph_subject_iri(&state.base_url, &set.id));
    rec.affected_graphs = vec![super::bindings::VALIDATION_GRAPH.to_string()];
    if let Err(e) = crate::commit_log::insert_commit(&state.store, &state.base_url, &rec) {
        tracing::warn!("failed to record binding commit for set {}: {e}", set.id);
    }
}

/// POST /api/shacl/bindings — bind a shape graph to a target (dataset/graph/shape
/// set). Idempotent. The caller must be able to write the target and *manage* the
/// shape graph (a binding makes it an enforcing validator).
pub async fn create_binding(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<BindingBody>,
) -> Result<impl IntoResponse, ApiErr> {
    // Binding makes this shape graph an enforcing validator of the target, so the
    // caller must be able to *manage* the shape graph (not merely read it).
    let set = load_set_checked(&state, &user, &body.shape_graph_id, true).await?;
    let target_iri = resolve_target_for_write(&state, &user, &body.target).await?;
    super::bindings::add_binding(&state.store, &target_iri, &set.graph_iri).map_err(e500)?;
    record_binding_commit(&state, &set, &target_iri, true, &user.user_id);
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "target": target_iri,
            "shape_graph_id": set.id,
            "shape_graph_graph": set.graph_iri,
        })),
    ))
}

/// DELETE /api/shacl/bindings — remove a binding. Same access rules as create.
pub async fn delete_binding(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<BindingBody>,
) -> Result<impl IntoResponse, ApiErr> {
    // Same authority as create_binding: managing the enforcing validator.
    let set = load_set_checked(&state, &user, &body.shape_graph_id, true).await?;
    let target_iri = resolve_target_for_write(&state, &user, &body.target).await?;
    super::bindings::remove_binding(&state.store, &target_iri, &set.graph_iri).map_err(e500)?;
    record_binding_commit(&state, &set, &target_iri, false, &user.user_id);
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/shacl/bindings — list bindings. With `?target_kind=&target_id=` it
/// returns the shape graphs bound to that target; with `?shape_graph_id=` it returns
/// the target IRIs that shape graph validates (reverse, for impact display).
pub async fn list_bindings(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Query(q): Query<BindingQuery>,
) -> Result<impl IntoResponse, ApiErr> {
    if let Some(set_id) = q.shape_graph_id {
        let set = load_set_checked(&state, &user, &set_id, false).await?;
        let targets = super::bindings::targets_for_shape_graph(&state.store, &set.graph_iri);
        return Ok(Json(
            serde_json::json!({ "shape_graph_id": set.id, "targets": targets }),
        ));
    }
    let (Some(kind), Some(id)) = (q.target_kind, q.target_id) else {
        return Err((
            StatusCode::BAD_REQUEST,
            "Provide either shape_graph_id or target_kind+target_id".into(),
        ));
    };
    let target = ValidationTarget {
        kind: TargetKind::from_str_or_dataset(&kind),
        id,
    };
    let target_iri = resolve_target_for_read(&state, &user, &target).await?;
    let st = studio(&state);
    let orgs = org_ids(&state, &user.user_id);
    let withheld = withheld_graphs(&state, &user)?;
    // Resolve bound shape-graph graphs back to records the caller may access.
    let sets: Vec<ShapeGraph> = super::bindings::bindings_for_target(&state.store, &target_iri)
        .into_iter()
        .filter(|giri| !withheld.contains(giri))
        .filter_map(|giri| st.get_shape_graph_by_iri(&giri).ok().flatten())
        .filter(|s| can_access_set(s, Some(&user.user_id), &orgs))
        .collect();
    Ok(Json(
        serde_json::json!({ "target": target_iri, "shape_graphs": sets }),
    ))
}

/// GET /api/datasets/:id/effective-shapes — the shape graphs that effectively
/// apply to a dataset (its own bindings ∪ each contained graph's bindings).
pub async fn dataset_effective_shapes(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
    if !state
        .auth_db
        .can_access_dataset(Some(&user.user_id), &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".into()));
    }
    // Bound shapes come with the dataset, but an entry names its graph and
    // carries its target classes: not one of a private graph the caller may
    // not read.
    let withheld = withheld_graphs(&state, &user)?;
    let sets: Vec<ShapeGraph> = super::bindings::effective_shape_graphs_for_dataset(
        &state.store,
        &state.auth_db,
        &studio(&state),
        &state.base_url,
        &ds,
    )
    .into_iter()
    .filter(|s| !withheld.contains(&s.graph_iri))
    .collect();
    Ok(Json(sets))
}

// ─── Shapes catalog & compose ────────────────────────────────────────────────

/// GET /api/shacl/shapes — the **graph-first** shapes catalog. Real stores hold
/// tens of thousands of shapes (e.g. an large information model), so without a
/// `?graph=` parameter this returns a cheap *summary* of the graphs that contain
/// shapes (`{ "graphs": [...] }`, each with node/property counts + registration);
/// with `?graph=<iri>` it returns that one graph's shapes (`{ "graph", "shapes" }`).
/// Either way, a graph appears only to a caller who may read it: a graph
/// registered in the Library to whoever the Library shows its entry to, any
/// other graph by the rule a `/sparql` query is scoped to (dataset visibility
/// plus graph-ACL read grants; admins read every graph). A shape's IRI, label,
/// target classes and path are data from its graph.
pub async fn list_shapes_catalog(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiErr> {
    let st = studio(&state);
    let orgs = org_ids(&state, &user.user_id);
    let reader = ReadScope::for_user(&state.auth_db, &user).map_err(e500)?;
    let mut reg_all: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut reg_access: HashMap<String, (String, String)> = HashMap::new();
    for s in st.list_shape_graphs().map_err(e500)? {
        reg_all.insert(s.graph_iri.clone());
        if can_access_set(&s, Some(&user.user_id), &orgs) && !reader.withholds(&s.graph_iri) {
            reg_access.insert(s.graph_iri.clone(), (s.id.clone(), s.name.clone()));
        }
    }
    let visible = |g: &str| {
        if reg_all.contains(g) {
            reg_access.contains_key(g)
        } else {
            reader.may_read_graph(g)
        }
    };

    // Drill-down: one graph's shapes.
    if let Some(graph) = params.get("graph") {
        if !visible(graph) {
            return Err((StatusCode::FORBIDDEN, "Access denied for that graph".into()));
        }
        let reg = reg_access.get(graph);
        let shapes: Vec<serde_json::Value> = super::catalog::catalog_shapes(&state.store, graph)
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "graph": s.graph, "shape": s.shape, "kind": s.kind, "label": s.label,
                    "target_classes": s.target_classes, "path": s.path,
                    "registered": reg.is_some(),
                    "shape_graph_id": reg.map(|(id, _)| id.clone()),
                    "shape_graph_name": reg.map(|(_, n)| n.clone()),
                })
            })
            .collect();
        return Ok(Json(
            serde_json::json!({ "graph": graph, "shapes": shapes }),
        ));
    }

    // Default: the cheap graph summary.
    let graphs: Vec<serde_json::Value> = super::catalog::catalog_graph_summary(&state.store)
        .into_iter()
        .filter(|g| visible(&g.graph))
        .map(|g| {
            let reg = reg_access.get(&g.graph);
            serde_json::json!({
                "graph": g.graph,
                "node_count": g.node_count,
                "property_count": g.property_count,
                "total": g.node_count + g.property_count,
                "registered": reg.is_some(),
                "shape_graph_id": reg.map(|(id, _)| id.clone()),
                "shape_graph_name": reg.map(|(_, n)| n.clone()),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "graphs": graphs })))
}

#[derive(Deserialize)]
pub struct ImportShapeRef {
    pub source_graph: String,
    pub shape: String,
}

#[derive(Deserialize)]
pub struct ImportShapesBody {
    pub shapes: Vec<ImportShapeRef>,
    pub note: Option<String>,
}

/// POST /api/shacl/shape-graphs/:id/import-shapes — copy existing shapes (each
/// with its full closure) into this shape graph. The picked shapes become part
/// of the graph (copy semantics). Records a new revision + Shapes commit.
pub async fn import_shapes(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ImportShapesBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let set = load_set_checked(&state, &user, &id, true).await?;
    if body.shapes.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "No shapes to import".into()));
    }
    // Copying a shape reads its source graph's triples into the destination, so
    // the caller must have read on every source graph — mirror the read-ACL gate
    // in resolve_target_for_read / register_shape_graph. Manage on the
    // destination (above) is not sufficient to pull from an arbitrary graph.
    for r in &body.shapes {
        if !crate::auth::acl::check_graph_permission(
            Some(&user),
            &r.source_graph,
            "read",
            &state.auth_db,
        ) {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Read access denied for graph <{}>", r.source_graph),
            ));
        }
    }
    let sources: std::collections::BTreeSet<&str> = body
        .shapes
        .iter()
        .map(|r| r.source_graph.as_str())
        .collect();
    for g in sources {
        refuse_no_derivatives_copy(&state, g)?;
    }
    guard_shape_graph_write(&state, &user, &set.graph_iri)?;
    let refs: Vec<(String, String)> = body
        .shapes
        .iter()
        .map(|r| (r.source_graph.clone(), r.shape.clone()))
        .collect();
    let copied =
        super::catalog::import_shapes_into(&state.store, &set.graph_iri, &refs).map_err(e500)?;

    // Snapshot the post-import Turtle as a new revision (+ commit).
    let turtle = state
        .store
        .graph_store_get(Some(&set.graph_iri), oxigraph::io::RdfFormat::Turtle)
        .map_err(e500)?;
    let turtle = String::from_utf8(turtle).map_err(|_| e500("graph is not valid UTF-8"))?;
    let note = body
        .note
        .unwrap_or_else(|| format!("Imported {copied} shape(s)"));
    let (targets, count) = super::run::analyze_shapes_graph(&state.store, &set.graph_iri);
    let version = studio(&state)
        .save_shape_graph_revision(
            &set.id,
            &turtle,
            &targets,
            count,
            Some(&note),
            Some(&user.user_id),
        )
        .map_err(e500)?;
    record_shape_graph_commit(
        &state,
        &set.id,
        &set.graph_iri,
        version,
        Some(&note),
        &user.user_id,
    );
    let updated = studio(&state).get_shape_graph(&set.id).map_err(e500)?;
    Ok(Json(
        serde_json::json!({ "imported": copied, "version": version, "shape_graph": updated }),
    ))
}

#[derive(Deserialize)]
pub struct RegisterShapeGraphBody {
    pub graph_iri: String,
    pub name: String,
    pub description: Option<String>,
    pub visibility: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
}

/// POST /api/shacl/register-shape-graph — adopt an existing named graph that
/// already holds SHACL as a first-class shape graph, *in place* (no copy): the
/// record points at the graph. Powers "this existing shapes graph should be in
/// the Library too". Idempotent: re-registering returns the existing record
/// to a caller who may see it.
///
/// The caller becomes the entry's owner, and an owner edits the entry's graph
/// in place, so registering needs the right to change that graph
/// ([`may_write_shape_graph_content`]), not only to read it; the Studio checks
/// that right again at every write. A graph named like one the Studio mints
/// (`urn:shapes:…`) is left to admins: the Studio clears such a graph when
/// its entry is deleted, so it must be one the Studio made.
pub async fn register_shape_graph(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<RegisterShapeGraphBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let st = studio(&state);
    if let Some(existing) = st.get_shape_graph_by_iri(&body.graph_iri).map_err(e500)? {
        let orgs = org_ids(&state, &user.user_id);
        let withheld =
            crate::auth::acl::private_graph_withheld(&state.auth_db, Some(&user), &body.graph_iri)
                .map_err(e500)?;
        if !user.is_admin() && (!can_access_set(&existing, Some(&user.user_id), &orgs) || withheld)
        {
            return Err((StatusCode::FORBIDDEN, "Access denied".into()));
        }
        return Ok((StatusCode::OK, Json(existing)));
    }
    let denied = || {
        (
            StatusCode::FORBIDDEN,
            format!(
                "Graph <{}> is not writable by you, so it cannot become your Library shape graph: \
                 its owner edits it in place. Ask for write access to it, or import its shapes \
                 into a shape graph of your own.",
                body.graph_iri
            ),
        )
    };
    if oxigraph::model::NamedNode::new(&body.graph_iri).is_err() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("<{}> is not a valid graph IRI", body.graph_iri),
        ));
    }
    if !user.is_admin() && owns_backing_graph(&body.graph_iri) {
        return Err(denied());
    }
    if !may_write_shape_graph_content(&state, &user, &body.graph_iri)? {
        return Err(denied());
    }
    let (targets, count) = super::run::analyze_shapes_graph(&state.store, &body.graph_iri);
    if count == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Graph contains no SHACL shapes".into(),
        ));
    }
    let (owner_type, owner_id) = resolve_owner(&state, &user, &body.owner_type, &body.owner_id)?;
    let set = st
        .create_shape_graph(
            &body.name,
            body.description.as_deref(),
            owner_type,
            &owner_id,
            parse_visibility(&body.visibility),
            &body.graph_iri,
            &body.tags,
            ShapeSource::Imported,
            Some(&user.user_id),
        )
        .map_err(e500)?;
    // Seed revision 1 from the graph's current Turtle (adopt in place — no PUT).
    let turtle = state
        .store
        .graph_store_get(Some(&body.graph_iri), oxigraph::io::RdfFormat::Turtle)
        .map_err(e500)?;
    let turtle = String::from_utf8(turtle).map_err(|_| e500("graph is not valid UTF-8"))?;
    let version = st
        .save_shape_graph_revision(
            &set.id,
            &turtle,
            &targets,
            count,
            Some("Registered existing graph"),
            Some(&user.user_id),
        )
        .map_err(e500)?;
    record_shape_graph_commit(
        &state,
        &set.id,
        &body.graph_iri,
        version,
        Some("Registered existing graph"),
        &user.user_id,
    );
    let set = st.get_shape_graph(&set.id).map_err(e500)?.ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "shape graph vanished after create".into(),
    ))?;
    Ok((StatusCode::CREATED, Json(set)))
}

// ─── Pipelines ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PipelineBody {
    pub name: String,
    pub description: Option<String>,
    pub visibility: Option<String>,
    pub owner_type: Option<String>,
    pub owner_id: Option<String>,
    #[serde(default)]
    pub targets: Vec<ValidationTarget>,
    #[serde(default)]
    pub dataset_ids: Vec<String>,
    #[serde(default)]
    pub graph_iris: Vec<String>,
    #[serde(default)]
    pub target_classes: Vec<String>,
    #[serde(default)]
    pub shape_graph_ids: Vec<String>,
    pub severity_threshold: Option<String>,
    #[serde(default)]
    pub run_inference: bool,
    pub max_results: Option<i64>,
    #[serde(default)]
    pub trigger_on_write: bool,
    pub schedule_cron: Option<String>,
    #[serde(default)]
    pub gate_writes: bool,
    pub retention: Option<i64>,
    #[serde(default)]
    pub inferred_target: Option<WriteTarget>,
    #[serde(default)]
    pub inferred_target_graph: Option<String>,
    #[serde(default)]
    pub results_target: Option<ResultsTarget>,
    #[serde(default)]
    pub results_target_graph: Option<String>,
}

fn validate_cron(cron: &Option<String>) -> Result<(), ApiErr> {
    if let Some(c) = cron.as_deref().filter(|s| !s.is_empty()) {
        if c.split_whitespace().count() != 5 {
            return Err((
                StatusCode::BAD_REQUEST,
                "schedule_cron must be a 5-field cron expression".into(),
            ));
        }
    }
    Ok(())
}

/// Authorize a pipeline's *write* surface against the acting user before it is stored/updated.
///
/// Every graph the pipeline would WRITE must be writable by the caller (admins bypass this ACL),
/// using the same authority as the SPARQL/Graph-Store write path (`check_graph_permission(write)`):
/// - an explicit (caller-supplied) inferred/results target graph, and
/// - every data graph in scope when `run_inference` is set — SHACL-AF inference
///   materialises triples *in place* into those graphs, so it is a write to them.
///
/// None of them may be a graph of a registered model version whose licence allows no altered
/// copies (`data_models::write_guard::check`), for admins too.
///
/// The pipeline's own auto-namespaced `urn:system:*:{id}` report/inference graphs are server-owned
/// and exempt. `exec::owner_can_write` re-checks the same authority at run time (covering the
/// scheduler's otherwise-ambient authority and any later change to the owner's grants).
fn authorize_pipeline_targets(
    state: &AppState,
    user: &AuthenticatedUser,
    pipeline: &ValidationPipeline,
) -> Result<(), ApiErr> {
    use crate::auth::acl::check_graph_permission;

    let mut write_targets: Vec<String> = Vec::new();
    if pipeline.run_inference {
        if let Some(g) = pipeline.inferred_target_graph.as_deref() {
            if !g.trim().is_empty() {
                write_targets.push(g.to_string());
            }
        }
    }
    if matches!(pipeline.results_target, ResultsTarget::NewGraph) {
        if let Some(g) = pipeline.results_target_graph.as_deref() {
            if !g.trim().is_empty() {
                write_targets.push(g.to_string());
            }
        }
    }
    // In-place inference writes into the data graphs themselves.
    if pipeline.run_inference {
        let st = studio(state);
        write_targets.extend(super::exec::resolve_data_graphs(
            &state.auth_db,
            &st,
            pipeline,
        ));
    }
    write_targets.sort();
    write_targets.dedup();
    for g in &write_targets {
        // Admins pass the graph ACL.
        if !user.is_admin() && !check_graph_permission(Some(user), g, "write", &state.auth_db) {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Write access denied for graph <{g}>"),
            ));
        }
        // The model registry's guard, for admins too: a graph of a version
        // whose licence allows no altered copies is never written, in place
        // or as a target. `exec` applies it again at every run.
        crate::data_models::write_guard::check(&state.store, &state.base_url, g).map_err(|m| {
            (
                StatusCode::FORBIDDEN,
                format!("This pipeline would write: {m}"),
            )
        })?;
    }
    Ok(())
}

/// Authorize a pipeline's *read* surface against `user`: every dataset, data
/// graph and shape graph in its scope ([`super::read_scope::pipeline_unreadable`]).
///
/// A run's report carries its data graphs' focus nodes and values to whoever
/// runs the pipeline or opens the run. So this is checked when a pipeline is
/// created or updated (its author decides what it reads), when it runs (the
/// caller receives the report) and when a stored run's report is opened, each
/// time afresh: a grant since revoked, or a pipeline stored before this check,
/// gives no more than the caller may read. The scheduler applies the same
/// check to the pipeline's creator.
fn authorize_pipeline_reads(
    state: &AppState,
    user: &AuthenticatedUser,
    pipeline: &ValidationPipeline,
) -> Result<(), ApiErr> {
    let reader = ReadScope::for_user(&state.auth_db, user).map_err(e500)?;
    let unreadable = super::read_scope::pipeline_unreadable(
        &state.store,
        &state.auth_db,
        &studio(state),
        &state.base_url,
        pipeline,
        &reader,
    )
    .map_err(e500)?;
    match unreadable {
        None => Ok(()),
        Some(what) => Err((
            StatusCode::FORBIDDEN,
            format!(
                "Read access denied for {what}: a pipeline's report carries the data it \
                 validates, so everything in its scope must be readable by you"
            ),
        )),
    }
}

/// Authorize a pipeline's *write gate* against `user`, when it has one.
///
/// A pipeline with `gate_writes` refuses (422) every write its shapes reject
/// to the graphs it covers, whoever makes it — the graphs' owners and editors
/// included. So setting a gate needs what a validation-layer binding, which
/// gates writes the same way, needs ([`resolve_target_for_write`]): write
/// access to every dataset and graph the gate covers
/// ([`super::gate::gated_scope`]). Admins pass. Whoever may name a dataset in
/// a pipeline — every signed-in user, for a public one — must not be able to
/// block its editors' writes with shapes that reject everything.
///
/// A dataset that does not exist is refused (404), as a binding to one is:
/// a later dataset with that id would otherwise inherit the gate. The gate
/// checks its creator's authority again at every write
/// (`gate::creator_may_gate`), so a grant revoked since, or a pipeline stored
/// before this check, gates nothing.
fn authorize_pipeline_gate(
    state: &AppState,
    user: &AuthenticatedUser,
    pipeline: &ValidationPipeline,
) -> Result<(), ApiErr> {
    const WHY: &str = "a pipeline that gates writes refuses them for everyone who writes \
                       what it covers, so gating needs the right to write it";
    if !pipeline.gate_writes {
        return Ok(());
    }
    let scope = super::gate::gated_scope(pipeline);
    for id in &scope.datasets {
        let ds = state
            .auth_db
            .get_dataset(id)
            .map_err(e500)?
            .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Dataset '{id}' not found")))?;
        if !user.is_admin()
            && !state
                .auth_db
                .can_write_dataset(&user.user_id, &ds)
                .map_err(e500)?
        {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Write access denied for dataset '{id}': {WHY}"),
            ));
        }
    }
    for g in &scope.graphs {
        if !crate::auth::acl::check_graph_permission(Some(user), g, "write", &state.auth_db) {
            return Err((
                StatusCode::FORBIDDEN,
                format!("Write access denied for graph <{g}>: {WHY}"),
            ));
        }
    }
    Ok(())
}

pub async fn create_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<PipelineBody>,
) -> Result<impl IntoResponse, ApiErr> {
    validate_cron(&body.schedule_cron)?;
    let (owner_type, owner_id) = resolve_owner(&state, &user, &body.owner_type, &body.owner_id)?;
    let now = chrono::Utc::now().to_rfc3339();
    let schedule = body.schedule_cron.filter(|s| !s.is_empty());
    let pipeline = ValidationPipeline {
        id: Uuid::new_v4().to_string(),
        name: body.name,
        description: body.description,
        owner_type,
        owner_id,
        visibility: parse_visibility(&body.visibility),
        targets: body.targets,
        dataset_ids: body.dataset_ids,
        graph_iris: body.graph_iris,
        target_classes: body.target_classes,
        shape_graph_ids: body.shape_graph_ids,
        severity_threshold: body
            .severity_threshold
            .as_deref()
            .map(SeverityThreshold::from_str_or_default)
            .unwrap_or(SeverityThreshold::Violation),
        run_inference: body.run_inference,
        max_results: body.max_results,
        trigger_on_write: body.trigger_on_write,
        schedule_cron: schedule,
        gate_writes: body.gate_writes,
        retention: body.retention.unwrap_or(50).clamp(1, 500),
        inferred_target: body.inferred_target.unwrap_or_default(),
        inferred_target_graph: body
            .inferred_target_graph
            .clone()
            .filter(|s| !s.trim().is_empty()),
        results_target: body.results_target.unwrap_or_default(),
        results_target_graph: body
            .results_target_graph
            .clone()
            .filter(|s| !s.trim().is_empty()),
        last_run_at: None,
        last_conforms: None,
        created_by: Some(user.user_id.clone()),
        created_at: now.clone(),
        updated_at: now,
    };
    authorize_pipeline_reads(&state, &user, &pipeline)?;
    authorize_pipeline_targets(&state, &user, &pipeline)?;
    authorize_pipeline_gate(&state, &user, &pipeline)?;
    studio(&state).insert_pipeline(&pipeline).map_err(e500)?;
    Ok((StatusCode::CREATED, Json(pipeline)))
}

pub async fn list_pipelines(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiErr> {
    let orgs = org_ids(&state, &user.user_id);
    let pipelines: Vec<ValidationPipeline> = studio(&state)
        .list_pipelines()
        .map_err(e500)?
        .into_iter()
        .filter(|p| can_access_pipeline(p, Some(&user.user_id), &orgs))
        .collect();
    Ok(Json(pipelines))
}

async fn load_pipeline_checked(
    state: &AppState,
    user: &AuthenticatedUser,
    id: &str,
    need_manage: bool,
) -> Result<ValidationPipeline, ApiErr> {
    let p = studio(state)
        .get_pipeline(id)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Pipeline not found".into()))?;
    let orgs = org_ids(state, &user.user_id);
    let ok = if need_manage {
        can_manage_pipeline(&p, Some(&user.user_id), &orgs, user.is_admin())
    } else {
        can_access_pipeline(&p, Some(&user.user_id), &orgs)
    };
    if !ok {
        return Err((StatusCode::FORBIDDEN, "Access denied".into()));
    }
    Ok(p)
}

pub async fn get_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let p = load_pipeline_checked(&state, &user, &id, false).await?;
    Ok(Json(p))
}

pub async fn update_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PipelineBody>,
) -> Result<impl IntoResponse, ApiErr> {
    validate_cron(&body.schedule_cron)?;
    let existing = load_pipeline_checked(&state, &user, &id, true).await?;
    let updated = ValidationPipeline {
        name: body.name,
        description: body.description,
        visibility: parse_visibility(&body.visibility),
        targets: body.targets,
        dataset_ids: body.dataset_ids,
        graph_iris: body.graph_iris,
        target_classes: body.target_classes,
        shape_graph_ids: body.shape_graph_ids,
        severity_threshold: body
            .severity_threshold
            .as_deref()
            .map(SeverityThreshold::from_str_or_default)
            .unwrap_or(existing.severity_threshold),
        run_inference: body.run_inference,
        max_results: body.max_results,
        trigger_on_write: body.trigger_on_write,
        schedule_cron: body.schedule_cron.filter(|s| !s.is_empty()),
        gate_writes: body.gate_writes,
        retention: body.retention.unwrap_or(existing.retention).clamp(1, 500),
        inferred_target: body.inferred_target.unwrap_or(existing.inferred_target),
        inferred_target_graph: body
            .inferred_target_graph
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| existing.inferred_target_graph.clone()),
        results_target: body.results_target.unwrap_or(existing.results_target),
        results_target_graph: body
            .results_target_graph
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| existing.results_target_graph.clone()),
        ..existing
    };
    authorize_pipeline_reads(&state, &user, &updated)?;
    authorize_pipeline_targets(&state, &user, &updated)?;
    authorize_pipeline_gate(&state, &user, &updated)?;
    studio(&state).update_pipeline(&updated).map_err(e500)?;
    Ok(Json(updated))
}

pub async fn delete_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    load_pipeline_checked(&state, &user, &id, true).await?;
    studio(&state).delete_pipeline(&id).map_err(e500)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn run_pipeline(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiErr> {
    let pipeline = load_pipeline_checked(&state, &user, &id, false).await?;
    // Seeing a shared pipeline is not reading its data: the report goes to
    // the caller, so the caller must be able to read its scope.
    authorize_pipeline_reads(&state, &user, &pipeline)?;
    let store = state.store.clone();
    let auth_db = state.auth_db.clone();
    let base_url = state.base_url.to_string();
    let actor = user.user_id.clone();
    // A test run validates but records nothing — no run row, no last-run update.
    let test = q
        .get("test")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let dataset_ids = pipeline.dataset_ids.clone();
    // Validation is blocking — run it off the async runtime.
    let run = tokio::task::spawn_blocking(move || {
        let st = ShaclStudioStore::new(auth_db.pool());
        if test {
            super::exec::execute_pipeline_dry(
                &store,
                &auth_db,
                &st,
                &base_url,
                &pipeline,
                Some(&actor),
            )
        } else {
            super::exec::execute_pipeline(
                &store,
                &auth_db,
                &st,
                &base_url,
                &pipeline,
                "manual",
                Some(&actor),
            )
        }
    })
    .await
    .map_err(e500)?
    .map_err(e500)?;

    // Best-effort private usage telemetry. Only real (non-test) runs count.
    if !test {
        for ds in &dataset_ids {
            let _ = state
                .auth_db
                .record_dataset_usage(ds, Some(&user.user_id), "pipeline");
        }
    }
    Ok(Json(run))
}

pub async fn list_pipeline_runs(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiErr> {
    load_pipeline_checked(&state, &user, &id, false).await?;
    let limit = q
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50);
    let runs = studio(&state)
        .list_pipeline_runs(&id, limit)
        .map_err(e500)?;
    Ok(Json(runs))
}

pub async fn get_pipeline_run(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path((id, run_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiErr> {
    let pipeline = load_pipeline_checked(&state, &user, &id, false).await?;
    // The full report is data from the pipeline's scope (run summaries, which
    // carry only counts, stay listed to everyone who sees the pipeline).
    authorize_pipeline_reads(&state, &user, &pipeline)?;
    let run = studio(&state)
        .get_pipeline_run(&run_id)
        .map_err(e500)?
        .filter(|r| r.pipeline_id == id)
        .ok_or((StatusCode::NOT_FOUND, "Run not found".into()))?;
    Ok(Json(run))
}

#[derive(Deserialize)]
pub struct LatestRunsBody {
    pub pipeline_ids: Vec<String>,
}

pub async fn list_latest_pipeline_runs(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<LatestRunsBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let st = studio(&state);
    let orgs = org_ids(&state, &user.user_id);
    // Only include pipelines the caller may access.
    let mut allowed = Vec::new();
    for id in &body.pipeline_ids {
        if let Ok(Some(p)) = st.get_pipeline(id) {
            if can_access_pipeline(&p, Some(&user.user_id), &orgs) {
                allowed.push(id.clone());
            }
        }
    }
    let runs = st.list_latest_runs(&allowed).map_err(e500)?;
    Ok(Json(runs))
}

// ─── Introspection / tooling ─────────────────────────────────────────────────

/// Resolve a `?dataset=` (with access check) or `?graphs=a,b` selector to graph IRIs.
async fn resolve_scope(
    state: &AppState,
    user: &AuthenticatedUser,
    q: &HashMap<String, String>,
) -> Result<Vec<String>, ApiErr> {
    if let Some(ds_id) = q.get("dataset") {
        let ds = state
            .auth_db
            .get_dataset(ds_id)
            .map_err(e500)?
            .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
        if !state
            .auth_db
            .can_access_dataset(Some(&user.user_id), &ds)
            .map_err(e500)?
        {
            return Err((StatusCode::FORBIDDEN, "Access denied".into()));
        }
        return state.auth_db.list_dataset_graphs(ds_id).map_err(e500);
    }
    if let Some(graphs) = q.get("graphs") {
        let graphs: Vec<String> = graphs
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        // The `?dataset=` branch above gates on can_access_dataset; caller-named
        // graphs must be gated too, or this leaks model structure from graphs the
        // caller can't read. Require read on each (same gate as a Graph Store read).
        for g in &graphs {
            if !crate::auth::acl::check_graph_permission(Some(user), g, "read", &state.auth_db) {
                return Err((
                    StatusCode::FORBIDDEN,
                    format!("Read access denied for graph <{g}>"),
                ));
            }
        }
        return Ok(graphs);
    }
    // No scope named. This used to return an empty list, which `values_clause`
    // turned into "no GRAPH wrapper at all" — so introspection ran over the
    // union graph and any authenticated caller could enumerate every tenant's
    // classes, properties and datatypes. Default to exactly what the caller can
    // read over /sparql instead.
    let mut graphs: Vec<String> = crate::server::routes::accessible_read_graphs(state, Some(user))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.message()))?
        .into_iter()
        .collect();
    graphs.sort();
    Ok(graphs)
}

pub async fn model_context(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<impl IntoResponse, ApiErr> {
    let graphs = resolve_scope(&state, &user, &q).await?;
    let store = state.store.clone();
    let ctx =
        tokio::task::spawn_blocking(move || super::introspect::model_context(&store, &graphs))
            .await
            .map_err(e500)?;
    Ok(Json(ctx))
}

#[derive(Deserialize)]
pub struct DeriveBody {
    pub dataset_id: Option<String>,
    #[serde(default)]
    pub graphs: Vec<String>,
    #[serde(default)]
    pub target_classes: Vec<String>,
}

pub async fn derive_shapes(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Json(body): Json<DeriveBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let graphs = if let Some(ds_id) = &body.dataset_id {
        let ds = state
            .auth_db
            .get_dataset(ds_id)
            .map_err(e500)?
            .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
        if !state
            .auth_db
            .can_access_dataset(Some(&user.user_id), &ds)
            .map_err(e500)?
        {
            return Err((StatusCode::FORBIDDEN, "Access denied".into()));
        }
        state.auth_db.list_dataset_graphs(ds_id).map_err(e500)?
    } else {
        // The dataset branch above gates on can_access_dataset; caller-named graphs
        // must be gated too, or derivation reads structure/values from graphs the
        // caller can't read. Require read on each (same gate as a Graph Store read).
        for g in &body.graphs {
            if !crate::auth::acl::check_graph_permission(Some(&user), g, "read", &state.auth_db) {
                return Err((
                    StatusCode::FORBIDDEN,
                    format!("Read access denied for graph <{g}>"),
                ));
            }
        }
        if body.graphs.is_empty() {
            // Neither a dataset nor any graph named: fall back to the caller's
            // readable set rather than the whole store (see `resolve_scope`).
            let mut graphs: Vec<String> =
                crate::server::routes::accessible_read_graphs(&state, Some(&user))
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.message()))?
                    .into_iter()
                    .collect();
            graphs.sort();
            graphs
        } else {
            body.graphs.clone()
        }
    };
    let targets = body.target_classes.clone();
    let store = state.store.clone();
    let (turtle, stats) = tokio::task::spawn_blocking(move || {
        super::introspect::derive_shapes(&store, &graphs, &targets)
    })
    .await
    .map_err(e500)?;
    Ok(Json(
        serde_json::json!({ "turtle": turtle, "stats": stats }),
    ))
}

// ─── form-manifest (optional auth — public datasets load anonymously) ────

pub async fn form_manifest(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|u| u.user_id.as_str());
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(e500)?
        .ok_or((StatusCode::NOT_FOUND, "Dataset not found".into()))?;
    if !state
        .auth_db
        .can_access_dataset(uid, &dataset)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".into()));
    }
    let withheld = crate::auth::acl::withheld_private_graphs(
        &state.auth_db,
        user.as_ref().map(|Extension(u)| u),
    )
    .map_err(e500)?;
    let manifest = super::manifest::build_manifest(
        &state.store,
        &state.auth_db,
        &state.base_url,
        &studio(&state),
        &dataset,
        &withheld,
    );
    Ok(Json(manifest))
}
