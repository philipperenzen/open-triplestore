//! SKOS-aware inferencing: the `skos` dataset entailment regime.
//!
//! The SKOS Reference formalises most of its data model in the SKOS RDF
//! schema — `skos:broader` is the inverse of `skos:narrower` and a
//! sub-property of the transitive `skos:broaderTransitive`, `skos:related`
//! and `skos:exactMatch` are symmetric, `skos:prefLabel` is a sub-property of
//! `rdfs:label`, `skos:Concept` is disjoint with `skos:ConceptScheme`, and so
//! on. All of that is OWL 2 RL, so the regime is the existing OWL 2 RL engine
//! run over a dataset's conformance layer with the bundled SKOS schema
//! (`frontend/public/vocab/skos.ttl`, the W3C file unchanged) added as a
//! premise. The premise lives in [`PREMISE_GRAPH`], reloaded whenever it
//! differs from the bundled file.
//!
//! The schema's own closure (`skos:broader rdfs:subPropertyOf
//! skos:semanticRelation`, `skos:prefLabel a owl:AnnotationProperty`, …) is
//! true of every SKOS dataset and says nothing about this one, so it is
//! pruned from the dataset's entailment graph after the run
//! ([`prune_schema_closure`]).
//!
//! The integrity conditions the schema cannot express in OWL 2 RL (S13, S14,
//! S27, S36, S46) are checked by the built-in SKOS integrity shapes graph
//! instead (`crate::shacl_studio::seed::SKOS_INTEGRITY_GRAPH`).

use crate::store::engine::StoreError;
use crate::store::TripleStore;

/// The regime name a dataset selects (`PUT /api/datasets/{id}/entailment`).
pub const REGIME: &str = "skos";

/// The OWL 2 RL engine does the work; this is the regime it runs as.
pub const ENGINE_REGIME: &str = "owl2-rl";

/// The graph holding the bundled SKOS schema while the regime runs.
pub const PREMISE_GRAPH: &str = "urn:system:entailment:skos-premise";

pub const SKOS_NS: &str = "http://www.w3.org/2004/02/skos/core#";

/// The bundled SKOS schema.
pub fn schema_ttl() -> &'static str {
    crate::data_models::vocab_files::SKOS.ttl
}

/// Number of triples in the bundled schema.
fn schema_len() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        oxigraph::io::RdfParser::from_format(oxigraph::io::RdfFormat::Turtle)
            .for_slice(schema_ttl().as_bytes())
            .filter(Result::is_ok)
            .count()
    })
}

/// Load the bundled schema into [`PREMISE_GRAPH`] unless it is already
/// there. A cheap count check: the graph is system-owned, so only an
/// upgrade that changes the bundled file (or a manual write) can make it
/// differ.
pub fn ensure_premise(store: &TripleStore) -> Result<(), StoreError> {
    let current = store.graph_count_cached(Some(PREMISE_GRAPH)).unwrap_or(0);
    if current == schema_len() {
        return Ok(());
    }
    store.graph_store_put(
        Some(PREMISE_GRAPH),
        schema_ttl(),
        oxigraph::io::RdfFormat::Turtle,
    )
}

/// Remove from `target` what the run derived about the vocabularies
/// themselves: triples whose subject is a SKOS, RDF, RDFS or OWL term, or a
/// blank node of the premise (the `owl:unionOf` range of `skos:member`).
pub fn prune_schema_closure(store: &TripleStore, target: &str) -> Result<(), StoreError> {
    // The namespace without its `#` also covers the ontology IRI itself.
    let skos = SKOS_NS.trim_end_matches('#');
    let target = crate::store::escape_sparql_iri(target);
    store.update(&format!(
        "DELETE {{ GRAPH <{target}> {{ ?s ?p ?o }} }} \
         WHERE {{ GRAPH <{target}> {{ ?s ?p ?o }} \
           FILTER( \
             (isIRI(?s) && ( \
               STRSTARTS(STR(?s), \"{skos}\") || \
               STRSTARTS(STR(?s), \"http://www.w3.org/1999/02/22-rdf-syntax-ns#\") || \
               STRSTARTS(STR(?s), \"http://www.w3.org/2000/01/rdf-schema#\") || \
               STRSTARTS(STR(?s), \"http://www.w3.org/2002/07/owl#\"))) \
             || (isBlank(?s) && EXISTS {{ GRAPH <{PREMISE_GRAPH}> {{ ?s ?sp ?so }} }}) \
           ) }}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_schema_is_the_whole_w3c_file() {
        // vocab/NOTICE.md and the file header both state 252 triples.
        assert_eq!(schema_len(), 252);
    }

    #[test]
    fn the_premise_is_loaded_once() {
        let store = TripleStore::in_memory().unwrap();
        ensure_premise(&store).unwrap();
        assert_eq!(store.graph_count_cached(Some(PREMISE_GRAPH)), Some(252));
        let generation = store.write_generation();
        ensure_premise(&store).unwrap();
        assert_eq!(
            store.write_generation(),
            generation,
            "an intact premise must not be rewritten"
        );
    }

    #[test]
    fn pruning_keeps_what_is_said_about_the_data() {
        let store = TripleStore::in_memory().unwrap();
        ensure_premise(&store).unwrap();
        let t = "urn:entailment:skos:test";
        store
            .update(&format!(
                "INSERT DATA {{ GRAPH <{t}> {{ \
                   <{SKOS_NS}broader> <http://www.w3.org/2000/01/rdf-schema#subPropertyOf> <{SKOS_NS}semanticRelation> . \
                   <http://example.org/a> <{SKOS_NS}narrower> <http://example.org/b> . \
                 }} }}"
            ))
            .unwrap();
        prune_schema_closure(&store, t).unwrap();
        assert_eq!(store.graph_count_cached(Some(t)), Some(1));
    }
}
