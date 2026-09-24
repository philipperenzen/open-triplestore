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

/// Graphs never adopted into the Library on a dataset's behalf: a graph the
/// Studio mints for an entry of its own (`urn:shapes:`; without an entry, the
/// entry went and took its content with it), a system graph, and a
/// model-registry graph (bind a model's shapes instead). An adopted entry is
/// owned by the dataset's owner, whose managers the Studio then lets edit a
/// `urn:shapes:` graph outright.
pub(crate) fn never_adopted(
    store: &crate::store::TripleStore,
    base_url: &str,
    graph_iri: &str,
) -> bool {
    graph_iri.starts_with("urn:shapes:")
        || graph_iri.starts_with("urn:system:")
        || crate::auth::dataset_graph::graph_held_by_model_registry(store, base_url, graph_iri)
}

/// Whether `dataset` may have `graph_iri` adopted into the Library on its
/// behalf (see [`auto_register_dataset_shapes_graph`]).
fn dataset_may_adopt(state: &AppState, dataset: &Dataset, graph_iri: &str) -> bool {
    use crate::auth::dataset_graph;
    if never_adopted(&state.store, &state.base_url, graph_iri) {
        return false;
    }
    dataset.shapes_graph_iri.as_deref() == Some(graph_iri)
        || dataset_graph::dataset_holds_graph(
            &state.auth_db,
            &state.base_url,
            &dataset.id,
            graph_iri,
        )
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

    // A new entry is owned by the dataset's owner, and its managers may edit
    // it through the Studio as far as `handlers::may_write_shape_graph_content`
    // lets them. Only a graph the dataset actually uses as its shapes is
    // adopted: one it holds (namespace, registered) or links as its shapes
    // graph. Never a graph the Studio mints for an entry of its own
    // (`urn:shapes:`, whose entry went with it), a system graph or a
    // model-registry graph (bind a model's shapes instead).
    if !dataset_may_adopt(state, dataset, graph_iri) {
        tracing::info!(
            dataset = %dataset.id,
            graph = %graph_iri,
            "shacl_studio: not adopting a graph the dataset does not hold into the Library"
        );
        return Ok(None);
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
