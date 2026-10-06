//! The three vocabularies an RML document may be written in, read as one.
//!
//! * **R2RML** (`http://www.w3.org/ns/r2rml#`, `rr:`) — triples maps, term
//!   maps, joins, graphs.
//! * **Legacy RML** (`http://semweb.mmlab.be/ns/rml#`, with reference
//!   formulations in `http://semweb.mmlab.be/ns/ql#`) — logical sources,
//!   references and iterators on top of R2RML.
//! * **RML-Core / RML-IO** (`http://w3id.org/rml/`) — the W3C Knowledge Graph
//!   Construction Community Group's vocabulary, which carries every term in
//!   one namespace and adds language and datatype maps, join expression
//!   maps, more term types and source descriptions.
//!
//! The parser reads R2RML and legacy RML. [`normalise`] rewrites every
//! RML-Core term that has an R2RML or legacy RML counterpart to it, and
//! leaves the terms only RML-Core has (`rml:datatypeMap`, `rml:childMap`,
//! `rml:URI`, `rml:RelativePathSource`, …) for the parser to read in their
//! own namespace. The vocabularies can be mixed in one document.
//!
//! Modules this engine does not implement — RML-FNML, RML-CC, RML-LV and
//! RML-star — are refused by name rather than read as if their terms were
//! absent, which would map something other than what the document says.

use oxigraph::model::{GraphNameRef, NamedNode, NamedOrBlankNode, Quad, Term};

use crate::store::engine::TripleStore;

pub const RR: &str = "http://www.w3.org/ns/r2rml#";
pub const RML_LEGACY: &str = "http://semweb.mmlab.be/ns/rml#";
pub const QL: &str = "http://semweb.mmlab.be/ns/ql#";
pub const RML: &str = "http://w3id.org/rml/";

/// RML-Core local names whose R2RML counterpart has the same local name.
const TO_R2RML: &[&str] = &[
    "TriplesMap",
    "subjectMap",
    "subject",
    "predicateObjectMap",
    "predicate",
    "predicateMap",
    "object",
    "objectMap",
    "graphMap",
    "graph",
    "constant",
    "template",
    "termType",
    "class",
    "datatype",
    "language",
    "parentTriplesMap",
    "joinCondition",
    "child",
    "parent",
    "sqlVersion",
    "IRI",
    "BlankNode",
    "Literal",
    "defaultGraph",
    "RefObjectMap",
    "SubjectMap",
    "PredicateObjectMap",
    "PredicateMap",
    "ObjectMap",
    "GraphMap",
];

/// RML-Core local names whose legacy RML counterpart has the same local name.
const TO_LEGACY: &[&str] = &[
    "logicalSource",
    "reference",
    "iterator",
    "referenceFormulation",
    "source",
    "query",
    "languageMap",
    "LogicalSource",
];

/// RML-Core reference formulations, as the parser knows them.
const TO_QL: &[&str] = &["JSONPath", "CSV", "XPath"];

/// Terms of the RML modules this engine does not implement, by the module
/// that defines them.
const UNIMPLEMENTED: &[(&str, &str)] = &[
    ("functionExecution", "RML-FNML"),
    ("function", "RML-FNML"),
    ("input", "RML-FNML"),
    ("returnMap", "RML-FNML"),
    ("return", "RML-FNML"),
    ("parameterMap", "RML-FNML"),
    ("inputValueMap", "RML-FNML"),
    ("gather", "RML-CC"),
    ("gatherAs", "RML-CC"),
    ("strategy", "RML-CC"),
    ("cartesianProduct", "RML-CC"),
    ("allowEmptyListAndContainer", "RML-CC"),
    ("viewOn", "RML-LV"),
    ("field", "RML-LV"),
    ("leftJoin", "RML-LV"),
    ("innerJoin", "RML-LV"),
    ("quotedTriplesMap", "RML-star"),
    ("StarMap", "RML-star"),
    ("logicalTarget", "RML-IO targets"),
];

/// What [`normalise`] found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Vocabulary {
    /// The document uses the RML-Core / RML-IO namespace.
    pub rml_core: bool,
}

fn rewrite(iri: &str) -> Option<String> {
    let local = iri.strip_prefix(RML)?;
    if TO_R2RML.contains(&local) {
        return Some(format!("{RR}{local}"));
    }
    if TO_LEGACY.contains(&local) {
        return Some(format!("{RML_LEGACY}{local}"));
    }
    if TO_QL.contains(&local) {
        return Some(format!("{QL}{local}"));
    }
    None
}

fn named(n: &NamedNode) -> NamedNode {
    match rewrite(n.as_str()) {
        Some(iri) => NamedNode::new_unchecked(iri),
        None => n.clone(),
    }
}

/// Copy the mapping in `graph` of `store` into a fresh store, with every
/// RML-Core term that has an R2RML or legacy RML counterpart rewritten to it.
pub fn normalise(
    store: &TripleStore,
    graph: Option<&str>,
) -> Result<(TripleStore, Vocabulary), String> {
    let graph_name = match graph {
        Some(g) => GraphNameRef::NamedNode(
            oxigraph::model::NamedNodeRef::new(g).map_err(|e| format!("graph <{g}>: {e}"))?,
        ),
        None => GraphNameRef::DefaultGraph,
    };
    let quads = store
        .quads_for_graph(graph_name)
        .map_err(|e| format!("reading the mapping: {e}"))?;
    let mut vocabulary = Vocabulary::default();
    let mut out = Vec::with_capacity(quads.len());
    for q in quads {
        let predicate = q.predicate.as_str();
        if let Some(local) = predicate.strip_prefix(RML) {
            vocabulary.rml_core = true;
            if let Some((_, module)) = UNIMPLEMENTED.iter().find(|(l, _)| *l == local) {
                return Err(format!(
                    "the mapping uses rml:{local}, from {module}, which this engine does not \
                     implement (it implements RML-Core, RML-IO sources and R2RML)"
                ));
            }
        }
        if let Term::NamedNode(o) = &q.object {
            if let Some(local) = o.as_str().strip_prefix(RML) {
                vocabulary.rml_core = true;
                if let Some((_, module)) = UNIMPLEMENTED.iter().find(|(l, _)| *l == local) {
                    return Err(format!(
                        "the mapping uses rml:{local}, from {module}, which this engine does not \
                         implement (it implements RML-Core, RML-IO sources and R2RML)"
                    ));
                }
            }
        }
        let subject = match &q.subject {
            NamedOrBlankNode::NamedNode(n) => NamedOrBlankNode::NamedNode(named(n)),
            other => other.clone(),
        };
        let object = match &q.object {
            Term::NamedNode(n) => Term::NamedNode(named(n)),
            other => other.clone(),
        };
        out.push(Quad::new(
            subject,
            named(&q.predicate),
            object,
            oxigraph::model::GraphName::DefaultGraph,
        ));
    }
    let normalised =
        TripleStore::in_memory().map_err(|e| format!("Failed to create temp store: {e}"))?;
    normalised
        .insert_quads(out)
        .map_err(|e| format!("copying the mapping: {e}"))?;
    Ok((normalised, vocabulary))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(ttl: &str) -> Result<(TripleStore, Vocabulary), String> {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(ttl, oxigraph::io::RdfFormat::Turtle, None)
            .unwrap();
        normalise(&store, None)
    }

    #[test]
    fn rml_core_terms_become_their_r2rml_and_legacy_counterparts() {
        let (store, v) = norm(
            "@prefix rml: <http://w3id.org/rml/> .
             <http://x/TM> a rml:TriplesMap ;
               rml:logicalSource [ rml:referenceFormulation rml:JSONPath ; rml:iterator \"$\" ] ;
               rml:subjectMap [ rml:template \"http://x/{$.id}\" ; rml:termType rml:URI ] .",
        )
        .unwrap();
        assert!(v.rml_core);
        let ask = |q: &str| match store.query(q).unwrap() {
            oxigraph::sparql::QueryResults::Boolean(b) => b,
            _ => false,
        };
        assert!(ask(
            "ASK { <http://x/TM> a <http://www.w3.org/ns/r2rml#TriplesMap> ; \
             <http://semweb.mmlab.be/ns/rml#logicalSource> \
               [ <http://semweb.mmlab.be/ns/rml#referenceFormulation> \
                 <http://semweb.mmlab.be/ns/ql#JSONPath> ] ; \
             <http://www.w3.org/ns/r2rml#subjectMap> \
               [ <http://www.w3.org/ns/r2rml#template> ?t ; \
                 <http://www.w3.org/ns/r2rml#termType> <http://w3id.org/rml/URI> ] }"
        ));
    }

    #[test]
    fn an_r2rml_document_is_left_as_it_is() {
        let (store, v) = norm(
            "@prefix rr: <http://www.w3.org/ns/r2rml#> .
             <http://x/TM> a rr:TriplesMap .",
        )
        .unwrap();
        assert!(!v.rml_core);
        assert_eq!(store.len().unwrap(), 1);
    }

    #[test]
    fn unimplemented_modules_are_refused_by_name() {
        let Err(err) = norm(
            "@prefix rml: <http://w3id.org/rml/> .
             <http://x/TM> a rml:TriplesMap ;
               rml:predicateObjectMap [ rml:objectMap [ rml:gather ( [ rml:reference \"a\" ] ) ] ] .",
        ) else {
            panic!("an RML-CC term is refused")
        };
        assert!(
            err.contains("rml:gather") && err.contains("RML-CC"),
            "{err}"
        );
    }
}
