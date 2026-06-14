//! One-time, idempotent migration that wraps each dataset's existing
//! `shapes_graph_iri` in a Library **ShapeGraph** so legacy shapes appear in the
//! consolidated Studio. Runs at startup with access to both the auth db and the
//! triplestore. It is purely additive — the legacy `shacl_on_write` gate keeps
//! working untouched; new pipeline gating is opt-in on top.

use crate::auth::db::AuthDb;
use crate::store::TripleStore;

use super::models::ShapeSource;
use super::store::ShaclStudioStore;

/// Returns the number of shape graphs newly created.
pub fn migrate_legacy(
    store: &TripleStore,
    auth_db: &AuthDb,
    base_url: &str,
) -> anyhow::Result<usize> {
    let studio = ShaclStudioStore::new(auth_db.pool());
    let datasets = auth_db.list_datasets()?;
    let mut created = 0usize;

    for d in datasets {
        let Some(iri) = d.shapes_graph_iri.clone().filter(|s| !s.is_empty()) else {
            continue;
        };

        // Wrap the legacy shapes graph in a Library ShapeGraph (idempotent: skip if
        // a set already points at this graph).
        if studio.get_shape_graph_by_iri(&iri)?.is_none() {
            let (targets, count) = super::run::analyze_shapes_graph(store, &iri);
            let set = studio.create_shape_graph(
                &format!("{} shapes", d.name),
                Some("Imported from the dataset's existing shapes graph."),
                d.owner_type,
                &d.owner_id,
                d.visibility,
                &iri,
                &[],
                ShapeSource::Imported,
                None,
            )?;

            let ttl = store
                .graph_store_get(Some(&iri), oxigraph::io::RdfFormat::Turtle)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .unwrap_or_default();
            studio.save_shape_graph_revision(
                &set.id,
                &ttl,
                &targets,
                count,
                Some("Initial import"),
                None,
            )?;
            created += 1;
        }

        // Record the dataset→shape-graph link in the validation layer so the legacy
        // attachment shows up in the new model. INSERT DATA is idempotent, so this
        // is safe to run even when the set was wrapped on a prior boot.
        let target_iri = super::bindings::dataset_target_iri(base_url, &d.id);
        if let Err(e) = super::bindings::add_binding(store, &target_iri, &iri) {
            tracing::warn!(
                "shacl_studio: failed to write legacy binding for dataset {}: {e}",
                d.id
            );
        }
    }

    if created > 0 {
        tracing::info!("shacl_studio: imported {created} legacy shapes graph(s) into the Library");
    }
    Ok(created)
}

/// Boot-time self-healing sweep for existing deployments: every dataset with a
/// `shapes_graph_iri` and every dataset graph carrying the `shapes` role is
/// adopted into the Library and bound to its dataset (both idempotent — see
/// [`super::registration::auto_register_dataset_shapes_graph`]). Failures are
/// logged per graph and never abort boot. Returns the number of graphs swept.
pub fn backfill_dataset_shapes(state: &crate::server::AppState) -> usize {
    use crate::auth::models::GraphKind;

    let datasets = match state.auth_db.list_datasets() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("shacl_studio: shapes backfill could not list datasets: {e}");
            return 0;
        }
    };

    let mut swept = 0usize;
    for d in datasets {
        let mut graph_iris: Vec<String> = Vec::new();
        if let Some(iri) = d.shapes_graph_iri.clone().filter(|s| !s.is_empty()) {
            graph_iris.push(iri);
        }
        if let Ok(entries) = state.auth_db.list_dataset_graph_entries(&d.id) {
            for e in entries {
                if e.graph_role == Some(GraphKind::Shapes) && !graph_iris.contains(&e.graph_iri) {
                    graph_iris.push(e.graph_iri);
                }
            }
        }
        for iri in graph_iris {
            match super::registration::auto_register_dataset_shapes_graph(state, &d, &iri, None) {
                Ok(_) => swept += 1,
                Err(e) => tracing::warn!(
                    "shacl_studio: shapes backfill failed for dataset {} graph <{iri}>: {e}",
                    d.id
                ),
            }
        }
    }
    if swept > 0 {
        tracing::info!("shacl_studio: shapes backfill swept {swept} dataset shapes graph(s)");
    }
    swept
}
