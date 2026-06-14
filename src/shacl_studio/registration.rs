//! Auto-registration of dataset shapes graphs into the SHACL Studio Library.
//!
//! Uploaded SHACL used to be detected at import (`dataset_graphs.graph_role =
//! 'shapes'`) but was never adopted into the Library (`shape_sets`) nor bound
//! in the validation layer, so it stayed invisible to the Studio, to
//! `GET /api/datasets/:id/effective-shapes`, and to pipelines.
//! [`auto_register_dataset_shapes_graph`] closes that gap and is the single
//! adoption path shared by bulk import, the dataset role/shacl setters,
//! `PUT /api/datasets/:id/shapes`, dataset-validation self-healing and the
//! boot backfill in [`super::migrate`].

use crate::auth::models::{Dataset, GraphKind};
use crate::server::AppState;

use super::bindings;
use super::models::{ShapeGraph, ShapeSource};
use super::store::ShaclStudioStore;

/// Last meaningful segment of a graph IRI (after `/`, `#` or `:`), used to
/// disambiguate auto-generated names when a dataset has several shapes graphs.
fn iri_tail(iri: &str) -> &str {
    iri.trim_end_matches(['/', '#'])
        .rsplit(['/', '#', ':'])
        .find(|s| !s.is_empty())
        .unwrap_or(iri)
}

/// Idempotently adopt `graph_iri` (a graph holding SHACL shapes that belongs to
/// `dataset`) as a Library [`ShapeGraph`] *in place* (no copy) and bind it to
/// the dataset in the validation layer.
///
/// * `Ok(Some(set))` — registered (or already registered); binding ensured.
/// * `Ok(None)` — the graph holds no SHACL shapes; nothing was created.
pub fn auto_register_dataset_shapes_graph(
    state: &AppState,
    dataset: &Dataset,
    graph_iri: &str,
    actor_user_id: Option<&str>,
) -> anyhow::Result<Option<ShapeGraph>> {
    let studio = ShaclStudioStore::new(state.auth_db.pool());
    let target_iri = bindings::dataset_target_iri(&state.base_url, &dataset.id);

    // Already in the Library → only ensure the dataset binding exists
    // (INSERT DATA is idempotent).
    if let Some(existing) = studio.get_shape_graph_by_iri(graph_iri)? {
        bindings::add_binding(&state.store, &target_iri, graph_iri)?;
        return Ok(Some(existing));
    }

    let (targets, count) = super::run::analyze_shapes_graph(&state.store, graph_iri);
    if count == 0 {
        return Ok(None);
    }

    // "{dataset} shapes", suffixed with the IRI tail when the dataset has
    // several shapes-role graphs so the Library names stay distinguishable.
    let shapes_role_graphs = state
        .auth_db
        .list_dataset_graph_entries(&dataset.id)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.graph_role == Some(GraphKind::Shapes))
        .count();
    let name = if shapes_role_graphs > 1 {
        format!("{} shapes ({})", dataset.name, iri_tail(graph_iri))
    } else {
        format!("{} shapes", dataset.name)
    };

    let set = studio.create_shape_graph(
        &name,
        Some(&format!(
            "Auto-registered from an import into dataset '{}' (graph <{graph_iri}>).",
            dataset.name
        )),
        dataset.owner_type,
        &dataset.owner_id,
        dataset.visibility,
        graph_iri,
        &["imported".to_string(), format!("dataset:{}", dataset.id)],
        ShapeSource::Imported,
        actor_user_id,
    )?;

    // Seed revision 1 from the graph's current Turtle (adopt in place — no PUT).
    let turtle = state
        .store
        .graph_store_get(Some(graph_iri), oxigraph::io::RdfFormat::Turtle)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .unwrap_or_default();
    let version = studio.save_shape_graph_revision(
        &set.id,
        &turtle,
        &targets,
        count,
        Some("Auto-registered dataset shapes graph"),
        actor_user_id,
    )?;

    // Best-effort commit-trail entry (mirrors the Studio handlers' recipe).
    let mut rec = crate::commit_log::CommitRecord::new(
        crate::commit_log::CommitKind::Shapes,
        "Auto-registered dataset shapes graph",
    );
    rec.actor_iri = actor_user_id.map(|u| format!("{}/users/{u}", state.base_url));
    rec.subject_iri = Some(format!(
        "{}/shacl/shape-graphs/{}",
        state.base_url.trim_end_matches('/'),
        set.id
    ));
    rec.version = Some(version.to_string());
    rec.revision = Some(version.to_string());
    rec.affected_graphs = vec![graph_iri.to_string()];
    if let Err(e) = crate::commit_log::insert_commit(&state.store, &state.base_url, &rec) {
        tracing::warn!(
            "shacl_studio: failed to record auto-registration commit for {}: {e}",
            set.id
        );
    }

    bindings::add_binding(&state.store, &target_iri, graph_iri)?;

    tracing::info!(
        dataset = %dataset.id,
        graph = %graph_iri,
        shape_graph = %set.id,
        "shacl_studio: auto-registered dataset shapes graph"
    );

    Ok(Some(studio.get_shape_graph(&set.id)?.unwrap_or(set)))
}
