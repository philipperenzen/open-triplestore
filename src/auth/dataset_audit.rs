//! The built-in **dataset-structure** SHACL model and a startup audit that keeps
//! every dataset's metadata graph conforming to it.
//!
//! A dataset's metadata graph (`urn:system:metadata:dataset:{id}`, written by
//! [`super::dataset_graph`]) is a `dcat:Dataset` description. The model asserts
//! the *governance contract*: a dataset MUST have a title, a stable identifier
//! and a declared visibility (enforced — `sh:Violation`), and SHOULD contain at
//! least one named graph, each carrying a valid `ots:graphRole` (advisory —
//! `sh:Warning`, so creating an empty dataset or adding a not-yet-classified
//! graph is never blocked).
//!
//! Enforcement happens at the create/update API via
//! [`super::dataset_graph::write_dataset_metadata_graph_checked`]. The startup
//! [`audit_dataset_metadata`] **repairs** legacy datasets by regenerating their
//! metadata from the current record (which now emits identifier + visibility);
//! anything still non-conforming is flagged in `urn:system:audit` — never
//! deleted.

use oxigraph::io::RdfFormat;

use crate::shacl::report::ValidationReport;
use crate::store::TripleStore;

use super::dataset_graph::{
    build_dataset_metadata_ttl, dataset_metadata_graph_iri, write_dataset_metadata_graph,
};
use super::db::AuthDb;

/// System graph holding the built-in dataset-structure shapes.
pub const DATASET_STRUCTURE_GRAPH: &str = "urn:system:shapes:dataset-structure";
/// System graph recording datasets that could not be made to conform.
pub const AUDIT_GRAPH: &str = "urn:system:audit";

const SYSTEM_OWNER: &str = "system";

/// The dataset-structure SHACL model. `dct:title`/`dct:identifier`/`ots:visibility`
/// are enforced; `void:subset` presence and per-graph `ots:graphRole` are advisory.
pub const DATASET_STRUCTURE_TTL: &str = r#"@prefix sh:   <http://www.w3.org/ns/shacl#> .
@prefix dcat: <http://www.w3.org/ns/dcat#> .
@prefix dct:  <http://purl.org/dc/terms/> .
@prefix void: <http://rdfs.org/ns/void#> .
@prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
@prefix otso: <https://opentriplestore.org/ontology/> .

# Named property shapes (the engine executes `sh:property <iri>`, not inline
# blank-node property shapes — see src/shacl/engine.rs).
otso:DatasetShape a sh:NodeShape ;
    sh:targetClass dcat:Dataset ;
    sh:property otso:DatasetTitle ;
    sh:property otso:DatasetIdentifier ;
    sh:property otso:DatasetVisibility .

otso:DatasetTitle a sh:PropertyShape ;
    sh:path dct:title ; sh:minCount 1 ;
    sh:message "A dataset must have a title." .
otso:DatasetIdentifier a sh:PropertyShape ;
    sh:path dct:identifier ; sh:minCount 1 ;
    sh:message "A dataset must have a stable identifier." .
otso:DatasetVisibility a sh:PropertyShape ;
    sh:path otso:visibility ; sh:minCount 1 ;
    sh:message "A dataset must declare its visibility." .
"#;

/// Validate a dataset-metadata Turtle string against the dataset-structure model
/// in an isolated in-memory store. Returns `None` when validation can't run
/// (shapes not seeded yet, or an engine error) so callers never block on
/// infrastructure issues — only on a real, non-conforming report.
pub fn validate_metadata(main_store: &TripleStore, metadata_ttl: &str) -> Option<ValidationReport> {
    let shapes_bytes = main_store
        .graph_store_get(Some(DATASET_STRUCTURE_GRAPH), RdfFormat::Turtle)
        .ok()?;
    let shapes_ttl = String::from_utf8_lossy(&shapes_bytes).into_owned();
    if shapes_ttl.trim().is_empty() {
        return None; // shapes not seeded yet
    }
    let temp = TripleStore::in_memory().ok()?;
    temp.graph_store_put(
        Some(DATASET_STRUCTURE_GRAPH),
        &shapes_ttl,
        RdfFormat::Turtle,
    )
    .ok()?;
    let data_graph = "urn:system:validate:dataset-metadata";
    temp.graph_store_put(Some(data_graph), metadata_ttl, RdfFormat::Turtle)
        .ok()?;
    crate::shacl::validate(&temp, DATASET_STRUCTURE_GRAPH, &[data_graph.to_string()]).ok()
}

/// Idempotently seed the dataset-structure shapes graph + its Library entry.
/// Mirrors `shacl_studio::seed::seed_shacl_shacl`. Returns the shape-graph id.
pub fn seed_dataset_structure_shapes(
    store: &TripleStore,
    auth_db: &AuthDb,
) -> anyhow::Result<String> {
    use crate::auth::models::{OwnerType, Visibility};
    use crate::shacl_studio::models::ShapeSource;
    use crate::shacl_studio::store::ShaclStudioStore;

    store
        .graph_store_put(
            Some(DATASET_STRUCTURE_GRAPH),
            DATASET_STRUCTURE_TTL,
            RdfFormat::Turtle,
        )
        .map_err(|e| anyhow::anyhow!("seed dataset-structure graph: {e}"))?;

    let studio = ShaclStudioStore::new(auth_db.pool());
    if let Some(existing) = studio.get_shape_graph_by_iri(DATASET_STRUCTURE_GRAPH)? {
        return Ok(existing.id);
    }
    let set = studio.create_shape_graph(
        "Dataset structure (governance)",
        Some("Built-in SHACL model for dataset metadata: required title/identifier/visibility and per-graph roles."),
        OwnerType::User,
        SYSTEM_OWNER,
        Visibility::Public,
        DATASET_STRUCTURE_GRAPH,
        &["governance".to_string(), "builtin".to_string()],
        ShapeSource::Imported,
        None,
    )?;
    let (targets, count) =
        crate::shacl_studio::run::analyze_shapes_graph(store, DATASET_STRUCTURE_GRAPH);
    studio.save_shape_graph_revision(
        &set.id,
        DATASET_STRUCTURE_TTL,
        &targets,
        count,
        Some("Built-in"),
        None,
    )?;
    tracing::info!("auth: seeded built-in dataset-structure shape graph");
    Ok(set.id)
}

/// Audit every dataset's metadata graph against the dataset-structure model and
/// repair non-conformers by regenerating their metadata from the current record.
/// Returns `(repaired, still_failing)`. Idempotent; never deletes data.
pub fn audit_dataset_metadata(
    store: &TripleStore,
    auth_db: &AuthDb,
    base_url: &str,
) -> anyhow::Result<(usize, usize)> {
    let datasets = auth_db.list_datasets()?;
    let mut repaired = 0usize;
    let mut failing = 0usize;

    for ds in datasets {
        let meta_iri = dataset_metadata_graph_iri(&ds.id);
        // Validate what's currently stored (regenerate if absent).
        let stored_bytes = store
            .graph_store_get(Some(&meta_iri), RdfFormat::Turtle)
            .unwrap_or_default();
        let stored = String::from_utf8_lossy(&stored_bytes).into_owned();
        let conforming_now = if stored.trim().is_empty() {
            false
        } else {
            validate_metadata(store, &stored)
                .map(|r| r.conforms)
                .unwrap_or(true)
        };
        if conforming_now {
            continue;
        }

        // Repair: regenerate metadata from the current record (now emits the
        // required identifier + visibility) and re-validate.
        let entries = auth_db
            .list_dataset_graph_entries(&ds.id)
            .unwrap_or_default();
        write_dataset_metadata_graph(store, base_url, &ds, &entries);
        let rebuilt = build_dataset_metadata_ttl(base_url, &ds, &entries);
        let still_bad = validate_metadata(store, &rebuilt)
            .map(|r| !r.conforms)
            .unwrap_or(false);
        if still_bad {
            mark_nonconforming(store, base_url, &ds.id);
            failing += 1;
            tracing::warn!(
                "dataset {} metadata still non-conforming after repair",
                ds.id
            );
        } else {
            repaired += 1;
        }
    }

    if repaired > 0 || failing > 0 {
        tracing::info!(
            "dataset metadata audit: {repaired} repaired, {failing} still non-conforming"
        );
    }
    Ok((repaired, failing))
}

/// Flag a dataset as non-conforming in the audit graph (best-effort).
fn mark_nonconforming(store: &TripleStore, base_url: &str, dataset_id: &str) {
    let ds_iri = format!("{base_url}/datasets/{dataset_id}");
    let q = format!(
        "INSERT DATA {{ GRAPH <{AUDIT_GRAPH}> {{ <{ds_iri}> <https://opentriplestore.org/ontology/auditStatus> \"nonconforming\" }} }}"
    );
    let _ = store.update(&q);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_shapes_are_valid_and_enforce() {
        let store = TripleStore::in_memory().unwrap();
        let auth = AuthDb::in_memory().unwrap();
        seed_dataset_structure_shapes(&store, &auth).unwrap();

        // A complete dataset metadata description conforms.
        let good = r#"@prefix dcat: <http://www.w3.org/ns/dcat#> .
@prefix dct: <http://purl.org/dc/terms/> .
@prefix void: <http://rdfs.org/ns/void#> .
@prefix otso: <https://opentriplestore.org/ontology/> .
<urn:ds:1> a dcat:Dataset ; dct:title "T" ; dct:identifier "1" ; otso:visibility "public" ;
    void:subset <urn:g:1> .
<urn:g:1> otso:graphRole otso:Instances ."#;
        let r = validate_metadata(&store, good).expect("validation runs");
        assert!(
            r.conforms,
            "complete metadata must conform: {:?}",
            r.results
        );

        // Missing identifier + visibility → a violation (the enforced core).
        let bad = r#"@prefix dcat: <http://www.w3.org/ns/dcat#> .
@prefix dct: <http://purl.org/dc/terms/> .
<urn:ds:2> a dcat:Dataset ; dct:title "T" ."#;
        let r = validate_metadata(&store, bad).expect("validation runs");
        assert!(
            !r.conforms,
            "metadata missing identifier/visibility must not conform"
        );
    }
}
