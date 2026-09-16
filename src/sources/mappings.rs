//! Mapping storage: standard RML as graphs in the store.
//!
//! Each frozen version lives in its own named graph whose IRI *is* the version
//! IRI (`urn:mapping:<id>:version:<n>`), so a run's `prov:used` points at
//! exactly the triples that executed, and the commit log / version diff /
//! rollback machinery applies to a mapping like any other graph.

use oxigraph::io::RdfFormat;

use crate::rml::model::{ObjectMap, RmlMapping, SourceRef};
use crate::rml::parse_from_store;
use crate::store::TripleStore;

use super::model::*;

/// Why a submitted mapping was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MappingError {
    #[error("the mapping is not valid RML: {0}")]
    Invalid(String),
    #[error(
        "the mapping reads {found}, but it is registered against {expected}; a mapping belongs to \
         exactly one datasource"
    )]
    WrongSource { found: String, expected: String },
    #[error(
        "the mapping reads no registered datasource; a relational logical source names one as \
         rml:source <urn:source:ID>"
    )]
    NoSource,
    #[error(
        "the mapping reads more than one datasource ({0}); split it into one mapping per source so \
         each run has a single connection and a single write gate"
    )]
    MultipleSources(String),
    #[error(
        "a mapping executed as a run may not declare rr:graphMap: the run graph is the unit the \
         write gate validates and the role swap promotes, so every triple has to land in it"
    )]
    GraphMap,
    #[error("YARRRML authoring is not available yet; submit RML (Turtle) as 'rml'")]
    YarrrmlUnavailable,
    #[error("{0}")]
    Storage(String),
}

/// Parse and check a mapping submitted for `source_id`.
pub fn validate_rml(rml: &str, source_id: &str) -> Result<RmlMapping, MappingError> {
    let mapping = crate::rml::parse_rml(rml).map_err(MappingError::Invalid)?;
    check(&mapping, source_id)?;
    Ok(mapping)
}

/// Check an already-parsed mapping against the datasource it is registered
/// for. Separate from [`validate_rml`] so a caller that needs the parsed
/// mapping (to derive the source from it) does not parse twice.
pub fn check(mapping: &RmlMapping, source_id: &str) -> Result<(), MappingError> {
    let expected = source_iri(source_id);
    let sources = mapping.datasources();
    match sources.len() {
        0 => return Err(MappingError::NoSource),
        1 => {
            // An `rr:logicalTable` with no `rml:source` binds to the run's
            // source implicitly, which is what the empty IRI stands for.
            if !sources[0].is_empty() && sources[0] != expected {
                return Err(MappingError::WrongSource {
                    found: sources[0].clone(),
                    expected,
                });
            }
        }
        _ => return Err(MappingError::MultipleSources(sources.join(", "))),
    }

    for tm in &mapping.triples_maps {
        if tm.graph_map.is_some() {
            return Err(MappingError::GraphMap);
        }
        for pom in &tm.predicate_object_maps {
            if pom.graph_map.is_some() {
                return Err(MappingError::GraphMap);
            }
        }
        if !matches!(tm.logical_source.source, SourceRef::Datasource(_)) {
            return Err(MappingError::Invalid(format!(
                "TriplesMap <{}> reads a file, not the datasource; a mapping run against a \
                 datasource must read it",
                tm.iri
            )));
        }
    }
    Ok(())
}

/// Write a version's RML into its own graph. A version is frozen: it is
/// written once, and a change creates the next one.
pub fn store_version(
    store: &TripleStore,
    id: &str,
    version: u32,
    rml: &str,
) -> Result<(), MappingError> {
    let graph = mapping_version_iri(id, version);
    store
        .graph_store_put(Some(&graph), rml, RdfFormat::Turtle)
        .map_err(|e| MappingError::Storage(e.to_string()))
}

/// Parse a stored version back into an executable mapping.
pub fn load(store: &TripleStore, id: &str, version: u32) -> Result<RmlMapping, MappingError> {
    let graph = mapping_version_iri(id, version);
    parse_from_store(store, Some(&graph)).map_err(MappingError::Invalid)
}

/// A stored version as Turtle, for the API and the Studio editor.
pub fn turtle(store: &TripleStore, id: &str, version: u32) -> Option<String> {
    let graph = mapping_version_iri(id, version);
    let bytes = store.dump(RdfFormat::Turtle, Some(&graph)).ok()?;
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// Every `rr:parentTriplesMap` the mapping resolves through a join, for the
/// Studio matrix view and the dry-run's reference pull-in (phase 2).
pub fn join_parents(mapping: &RmlMapping) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for tm in &mapping.triples_maps {
        for pom in &tm.predicate_object_maps {
            if let ObjectMap::Ref(r) = &pom.object {
                out.push((tm.iri.clone(), r.parent_triples_map.clone()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PFX: &str = "@prefix rr: <http://www.w3.org/ns/r2rml#> .\n\
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .\n\
@prefix ex: <http://example.org/> .\n";

    fn mapping_for(src: &str) -> String {
        format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <{src}> ; rr:tableName \"t\" ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [ rr:column \"c\" ] ] ."
        )
    }

    #[test]
    fn a_mapping_must_read_the_source_it_is_registered_against() {
        assert!(validate_rml(&mapping_for("urn:source:legacy"), "legacy").is_ok());
        let err = validate_rml(&mapping_for("urn:source:other"), "legacy").unwrap_err();
        assert!(matches!(err, MappingError::WrongSource { .. }), "{err}");
        assert!(err.to_string().contains("urn:source:other"), "{err}");
    }

    #[test]
    fn a_file_mapping_is_not_a_datasource_mapping() {
        let err = validate_rml(
            &format!(
                "{PFX}
                 ex:M a rr:TriplesMap ;
                   rml:logicalSource [ rml:source \"people.csv\" ;
                                       rml:referenceFormulation <http://semweb.mmlab.be/ns/ql#CSV> ] ;
                   rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
                   rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [ rml:reference \"c\" ] ] ."
            ),
            "legacy",
        )
        .unwrap_err();
        assert_eq!(err, MappingError::NoSource);
    }

    #[test]
    fn two_datasources_in_one_mapping_are_refused() {
        let err = validate_rml(
            &format!(
                "{PFX}
                 ex:A a rr:TriplesMap ;
                   rml:logicalSource [ rml:source <urn:source:a> ; rr:tableName \"t\" ] ;
                   rr:subjectMap [ rr:template \"http://x/a{{id}}\" ] .
                 ex:B a rr:TriplesMap ;
                   rml:logicalSource [ rml:source <urn:source:b> ; rr:tableName \"t\" ] ;
                   rr:subjectMap [ rr:template \"http://x/b{{id}}\" ] ."
            ),
            "a",
        )
        .unwrap_err();
        assert!(matches!(err, MappingError::MultipleSources(_)), "{err}");
    }

    #[test]
    fn a_graph_map_is_refused_because_the_run_graph_is_the_unit_of_promotion() {
        let err = validate_rml(
            &format!(
                "{PFX}
                 ex:M a rr:TriplesMap ;
                   rml:logicalSource [ rml:source <urn:source:legacy> ; rr:tableName \"t\" ] ;
                   rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
                   rr:graphMap [ rr:constant <http://x/elsewhere> ] ;
                   rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [ rr:column \"c\" ] ] ."
            ),
            "legacy",
        )
        .unwrap_err();
        assert_eq!(err, MappingError::GraphMap);
    }

    #[test]
    fn a_version_round_trips_through_its_own_graph() {
        let store = TripleStore::in_memory().unwrap();
        let rml = mapping_for("urn:source:legacy");
        store_version(&store, "m", 1, &rml).unwrap();
        let loaded = load(&store, "m", 1).expect("loads");
        assert_eq!(loaded.triples_maps.len(), 1);
        assert_eq!(loaded.datasources(), vec!["urn:source:legacy"]);
        let ttl = turtle(&store, "m", 1).expect("serialises");
        assert!(ttl.contains("urn:source:legacy"), "{ttl}");
        assert_eq!(turtle(&store, "m", 2), None, "an unwritten version is absent");
    }

    #[test]
    fn versions_are_independent_graphs() {
        let store = TripleStore::in_memory().unwrap();
        store_version(&store, "m", 1, &mapping_for("urn:source:legacy")).unwrap();
        let v2 = mapping_for("urn:source:legacy").replace("ex:p", "ex:renamed");
        store_version(&store, "m", 2, &v2).unwrap();
        assert!(turtle(&store, "m", 1).unwrap().contains("example.org/p"));
        assert!(turtle(&store, "m", 2).unwrap().contains("renamed"));
        assert!(!turtle(&store, "m", 2).unwrap().contains("example.org/p>"));
    }

    #[test]
    fn join_parents_are_reported_for_the_studio_view() {
        let m = crate::rml::parse_rml(&format!(
            "{PFX}
             ex:Child a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"c\" ] ;
               rr:subjectMap [ rr:template \"http://x/c{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:parent ; rr:objectMap [
                 rr:parentTriplesMap ex:Parent ;
                 rr:joinCondition [ rr:child \"pid\" ; rr:parent \"id\" ] ] ] .
             ex:Parent a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:s> ; rr:tableName \"p\" ] ;
               rr:subjectMap [ rr:template \"http://x/p{{id}}\" ] ."
        ))
        .unwrap();
        assert_eq!(
            join_parents(&m),
            vec![(
                "http://example.org/Child".to_string(),
                "http://example.org/Parent".to_string()
            )]
        );
    }
}
