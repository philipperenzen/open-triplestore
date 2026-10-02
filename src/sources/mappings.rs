//! Mapping storage: standard RML as graphs in the store.
//!
//! Each frozen version lives in its own named graph whose IRI *is* the version
//! IRI (`urn:mapping:<id>:version:<n>`), so a run's `prov:used` points at
//! exactly the triples that executed, and the commit log / version diff /
//! rollback machinery applies to a mapping like any other graph.

use oxigraph::io::RdfFormat;

use crate::rml::model::{ObjectMap, RmlMapping, Semantics, SourceRef};
use crate::rml::parse_from_store_as;
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
    #[error("supply either 'rml' or 'yarrrml', not both — they would disagree")]
    BothForms,
    #[error("unknown semantics '{0}'; expected 'r2rml' (the default) or 'legacy'")]
    Semantics(String),
    #[error("the YARRRML could not be translated: {0}")]
    Yarrrml(String),
    #[error("{0}")]
    Storage(String),
}

/// Parse and check a mapping submitted for `source_id`.
pub fn validate_rml(rml: &str, source_id: &str) -> Result<RmlMapping, MappingError> {
    validate_rml_as(rml, source_id, Semantics::default())
}

/// [`validate_rml`], parsed under the rules it would be frozen with.
pub fn validate_rml_as(
    rml: &str,
    source_id: &str,
    semantics: Semantics,
) -> Result<RmlMapping, MappingError> {
    let store = TripleStore::in_memory().map_err(|e| MappingError::Storage(e.to_string()))?;
    store
        .load_str(rml, RdfFormat::Turtle, None)
        .map_err(|e| MappingError::Invalid(format!("Failed to parse RML Turtle: {e}")))?;
    let mapping = parse_from_store_as(&store, None, semantics).map_err(MappingError::Invalid)?;
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

    if mapping.has_graph_maps() {
        return Err(MappingError::GraphMap);
    }
    for tm in &mapping.triples_maps {
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

/// The term-generation rules a request asks a new version to be frozen
/// under: R2RML's unless it pins the legacy ones, which keeps the IRIs and
/// blank nodes an older version produced (see [`Semantics`]).
pub fn requested_semantics(requested: Option<&str>) -> Result<Semantics, MappingError> {
    match requested.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(Semantics::default()),
        Some(s) => Semantics::parse(s).ok_or_else(|| MappingError::Semantics(s.to_string())),
    }
}

/// Write a version's RML into its own graph, stamped with the rules it runs
/// under. A version is frozen: it is written once, and a change creates the
/// next one.
pub fn store_version(
    store: &TripleStore,
    id: &str,
    version: u32,
    rml: &str,
    semantics: Semantics,
) -> Result<(), MappingError> {
    let graph = mapping_version_iri(id, version);
    store
        .graph_store_put(Some(&graph), rml, RdfFormat::Turtle)
        .map_err(|e| MappingError::Storage(e.to_string()))?;
    super::registry::put_version_semantics(store, id, version, semantics)
        .map_err(MappingError::Storage)
}

/// Parse a stored version back into an executable mapping, under the rules
/// it was frozen with — a version from before the stamp runs as it always did.
pub fn load(store: &TripleStore, id: &str, version: u32) -> Result<RmlMapping, MappingError> {
    let graph = mapping_version_iri(id, version);
    let semantics = super::registry::version_semantics(store, id, version);
    parse_from_store_as(store, Some(&graph), semantics).map_err(MappingError::Invalid)
}

/// A stored version as Turtle, for the API and the Studio editor.
///
/// `resolve` declares a prefix for each namespace the document uses — the
/// deployment's registry, for a document a person edits; an editor over
/// `<http://www.w3.org/ns/r2rml#predicateObjectMap>` in full is not one
/// anyone writes in.
pub fn turtle<F>(store: &TripleStore, id: &str, version: u32, resolve: F) -> Option<String>
where
    F: Fn(&str) -> Option<(String, String)>,
{
    let graph = mapping_version_iri(id, version);
    let bytes = store
        .dump_prefixed(RdfFormat::Turtle, Some(&graph), resolve)
        .ok()?;
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
        store_version(&store, "m", 1, &rml, Semantics::R2rml).unwrap();
        let loaded = load(&store, "m", 1).expect("loads");
        assert_eq!(loaded.semantics, Semantics::R2rml);
        assert_eq!(loaded.triples_maps.len(), 1);
        assert_eq!(loaded.datasources(), vec!["urn:source:legacy"]);
        let ttl = turtle(&store, "m", 1, |_| None).expect("serialises");
        assert!(ttl.contains("urn:source:legacy"), "{ttl}");
        assert_eq!(
            turtle(&store, "m", 2, |_| None),
            None,
            "an unwritten version is absent"
        );
    }

    #[test]
    fn versions_are_independent_graphs() {
        let store = TripleStore::in_memory().unwrap();
        store_version(
            &store,
            "m",
            1,
            &mapping_for("urn:source:legacy"),
            Semantics::R2rml,
        )
        .unwrap();
        let v2 = mapping_for("urn:source:legacy").replace("ex:p", "ex:renamed");
        store_version(&store, "m", 2, &v2, Semantics::R2rml).unwrap();
        assert!(turtle(&store, "m", 1, |_| None)
            .unwrap()
            .contains("example.org/p"));
        assert!(turtle(&store, "m", 2, |_| None)
            .unwrap()
            .contains("renamed"));
        assert!(!turtle(&store, "m", 2, |_| None)
            .unwrap()
            .contains("example.org/p>"));
    }

    #[test]
    fn a_version_keeps_the_rules_it_was_frozen_with() {
        // A template object map with no rr:termType: a literal under the
        // legacy rules, an IRI under R2RML's (§7.4).
        let rml = format!(
            "{PFX}
             ex:M a rr:TriplesMap ;
               rml:logicalSource [ rml:source <urn:source:legacy> ; rr:tableName \"t\" ] ;
               rr:subjectMap [ rr:template \"http://x/{{id}}\" ] ;
               rr:predicateObjectMap [ rr:predicate ex:p ;
                 rr:objectMap [ rr:template \"http://x/c/{{c}}\" ] ] ."
        );
        let store = TripleStore::in_memory().unwrap();
        store_version(&store, "m", 1, &rml, Semantics::Legacy).unwrap();
        store_version(&store, "m", 2, &rml, Semantics::R2rml).unwrap();
        // Version 3 is written the way a version frozen before the stamp
        // existed was: RML graph only.
        store
            .graph_store_put(Some(&mapping_version_iri("m", 3)), &rml, RdfFormat::Turtle)
            .unwrap();

        let term_type = |v: u32| {
            let m = load(&store, "m", v).unwrap();
            let ObjectMap::Term(t) = &m.triples_maps[0].predicate_object_maps[0].object else {
                panic!("a term object map")
            };
            (m.semantics, t.term_type.clone())
        };
        use crate::rml::model::TermType;
        assert_eq!(term_type(1), (Semantics::Legacy, TermType::Literal));
        assert_eq!(term_type(2), (Semantics::R2rml, TermType::IRI));
        assert_eq!(
            term_type(3),
            (Semantics::Legacy, TermType::Literal),
            "an unstamped version is a legacy one"
        );
    }

    #[test]
    fn a_request_may_pin_the_legacy_rules_and_nothing_else() {
        assert_eq!(requested_semantics(None).unwrap(), Semantics::R2rml);
        assert_eq!(requested_semantics(Some(" ")).unwrap(), Semantics::R2rml);
        assert_eq!(
            requested_semantics(Some("legacy")).unwrap(),
            Semantics::Legacy
        );
        assert_eq!(
            requested_semantics(Some("r2rml")).unwrap(),
            Semantics::R2rml
        );
        let err = requested_semantics(Some("newest")).unwrap_err();
        assert!(err.to_string().contains("'newest'"), "{err}");
    }

    #[test]
    fn a_graph_map_on_the_subject_map_or_via_rr_graph_is_refused_too() {
        for placement in [
            "rr:subjectMap [ rr:template \"http://x/{id}\" ; rr:graph <http://x/g> ] ;",
            "rr:subjectMap [ rr:template \"http://x/{id}\" ; rr:graphMap [ rr:template \"http://x/{c}\" ] ] ;",
        ] {
            let err = validate_rml(
                &format!(
                    "{PFX}
                     ex:M a rr:TriplesMap ;
                       rml:logicalSource [ rml:source <urn:source:legacy> ; rr:tableName \"t\" ] ;
                       {placement}
                       rr:predicateObjectMap [ rr:predicate ex:p ; rr:objectMap [ rr:column \"c\" ] ] ."
                ),
                "legacy",
            )
            .unwrap_err();
            assert_eq!(err, MappingError::GraphMap, "{placement}");
        }
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
