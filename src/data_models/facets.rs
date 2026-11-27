//! What a registry version holds: counts of its classes, properties, concepts,
//! individuals, shapes and rules.
//!
//! A registry entry's `kind` says what an upload was guessed to be. Most real
//! content is several things at once (an ontology declares classes and
//! properties, a thesaurus a scheme and its concepts, a quality profile
//! individuals of a vocabulary class), so the facets say what is there instead.
//! They are the detector's own tallies ([`crate::kind_detector::Evidence`]),
//! counted once per version and stored on its registry record (`ver:facets`, a
//! JSON value like the attribution record), so a listing reads them without
//! scanning a graph.
//!
//! A stored record carries the number of triples it was counted over. It is
//! served only while the version's content graphs still hold that many: a
//! version edited since is counted again the next time it is opened, or at
//! the next start. A change that keeps the triple count the same is not
//! noticed until then.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::models::DataModelVersion;
use super::registry;
use crate::kind_detector::{self, Evidence};
use crate::server::AppState;
use crate::store::engine::StoreError;
use crate::store::TripleStore;

/// Content above this many triples is not scanned: its facets are reported as
/// `capped`, with no counts. The scan holds the content in memory.
pub const MAX_TRIPLES: usize = 2_000_000;

/// A version opened through the API is counted inline only up to this many
/// triples; a larger one is left to the start-up pass, so a request never waits
/// on a long scan.
pub const INLINE_MAX_TRIPLES: usize = 250_000;

/// The content facets of one registry version.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ContentFacets {
    /// Named classes (`owl:Class`, `rdfs:Class`).
    pub classes: usize,
    /// Declared properties (OWL object, datatype and annotation properties,
    /// `rdf:Property`).
    pub properties: usize,
    /// Resources typed `skos:Concept`.
    pub concepts: usize,
    /// Resources typed `skos:ConceptScheme`.
    pub concept_schemes: usize,
    /// Subjects typed with a class that is not itself a schema construct:
    /// the individuals (instance data) the version holds.
    pub individuals: usize,
    /// SHACL node and property shapes.
    pub shapes: usize,
    /// SWRL and SPIN entailment rules.
    pub rules: usize,
    /// Triples in the version's content graphs when these were counted.
    pub triples: usize,
    /// The content was too large to scan: no counts are given.
    #[serde(default)]
    pub capped: bool,
}

impl ContentFacets {
    /// The facets of a scan's evidence, over content of `triples` triples.
    pub fn from_evidence(ev: &Evidence, triples: usize) -> Self {
        ContentFacets {
            classes: ev.owl_classes + ev.rdfs_classes,
            properties: ev.owl_properties,
            concepts: ev.skos_concepts,
            concept_schemes: ev.skos_concept_schemes,
            individuals: ev.instance_subjects,
            shapes: ev.shacl_shapes,
            rules: ev.entailment_rules,
            triples,
            capped: false,
        }
    }
}

/// The triples held by `graphs` right now.
pub fn content_triples(store: &TripleStore, graphs: &[String]) -> usize {
    graphs
        .iter()
        .map(|g| {
            store
                .graph_count_cached(Some(g))
                .or_else(|| store.count_graph(Some(g)).ok())
                .unwrap_or(0)
        })
        .sum()
}

/// Count the facets of `graphs` by scanning them, or, above `max_triples`,
/// report them as capped.
pub fn compute(
    store: &TripleStore,
    graphs: &[String],
    max_triples: usize,
) -> Result<ContentFacets, StoreError> {
    use oxigraph::model::{GraphNameRef, NamedNodeRef};
    let triples = content_triples(store, graphs);
    if triples > max_triples {
        return Ok(ContentFacets {
            triples,
            capped: true,
            ..ContentFacets::default()
        });
    }
    let mut quads = Vec::with_capacity(triples);
    for g in graphs {
        let node = NamedNodeRef::new(g)
            .map_err(|e| StoreError::Parse(format!("invalid graph IRI {g}: {e}")))?;
        quads.extend(store.quads_for_graph(GraphNameRef::NamedNode(node))?);
    }
    let detected = kind_detector::detect(&quads);
    Ok(ContentFacets::from_evidence(&detected.evidence, triples))
}

/// The version's stored facets, when they still describe its content.
pub fn stored_if_fresh(
    store: &TripleStore,
    base_url: &str,
    version: &DataModelVersion,
) -> Option<ContentFacets> {
    let stored = registry::get_version_facets(
        store,
        &registry::version_record_iri(base_url, &version.data_model_id, &version.version),
    )?;
    (stored.triples == content_triples(store, &version.content_graphs())).then_some(stored)
}

/// Count `version`'s facets and store them. `max_triples` bounds the scan, see
/// [`compute`]. Best effort: a registry that cannot be written to still gives
/// the counts back.
pub fn refresh(
    store: &TripleStore,
    base_url: &str,
    version: &DataModelVersion,
    max_triples: usize,
) -> Option<ContentFacets> {
    let facets = match compute(store, &version.content_graphs(), max_triples) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(model = %version.data_model_id, version = %version.version, error = %e, "facets: the version could not be counted");
            return None;
        }
    };
    let iri = registry::version_record_iri(base_url, &version.data_model_id, &version.version);
    if let Err(e) = registry::set_version_facets(store, &iri, &facets) {
        tracing::warn!(model = %version.data_model_id, version = %version.version, error = %e, "facets: the counts could not be stored");
    }
    Some(facets)
}

/// The stored facets of `version`, counted first when none are stored or they
/// no longer fit its content and the content is small enough to scan inline
/// ([`INLINE_MAX_TRIPLES`]). Blocking: the scan runs on the calling thread.
pub fn ensure_inline(
    store: &TripleStore,
    base_url: &str,
    version: &DataModelVersion,
) -> Option<ContentFacets> {
    stored_if_fresh(store, base_url, version)
        .or_else(|| refresh(store, base_url, version, INLINE_MAX_TRIPLES))
        .filter(|f| !f.capped)
}

/// Count every version whose facets are missing or out of date. Returns how
/// many were counted. Run once at start, after the seeders, in the
/// background: a version seeded or uploaded before facets existed gets its
/// counts here.
pub fn backfill(state: &AppState) -> usize {
    let (store, base) = (&state.store, state.base_url.as_str());
    let mut counted = 0;
    for entry in registry::list_data_models(store) {
        for version in registry::list_versions(store, base, &entry.id) {
            if stored_if_fresh(store, base, &version).is_some() {
                continue;
            }
            if refresh(store, base, &version, MAX_TRIPLES).is_some() {
                counted += 1;
            }
        }
    }
    if counted > 0 {
        tracing::info!("facets: counted the content of {counted} registry version(s)");
    }
    counted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facets_are_the_detectors_tallies() {
        let ev = Evidence {
            owl_classes: 3,
            rdfs_classes: 2,
            owl_properties: 7,
            skos_concept_schemes: 1,
            skos_concepts: 40,
            instance_subjects: 5,
            shacl_shapes: 4,
            entailment_rules: 1,
            ..Evidence::default()
        };
        let f = ContentFacets::from_evidence(&ev, 123);
        assert_eq!(
            (
                f.classes,
                f.properties,
                f.concepts,
                f.concept_schemes,
                f.individuals,
                f.shapes,
                f.rules,
                f.triples,
                f.capped
            ),
            (5, 7, 40, 1, 5, 4, 1, 123, false)
        );
    }

    #[test]
    fn a_stored_record_without_the_cap_flag_reads_as_not_capped() {
        let f: ContentFacets = serde_json::from_str(
            r#"{"classes":1,"properties":0,"concepts":0,"concept_schemes":0,"individuals":0,"shapes":0,"rules":0,"triples":9}"#,
        )
        .unwrap();
        assert!(!f.capped);
        assert_eq!(f.classes, 1);
    }
}
