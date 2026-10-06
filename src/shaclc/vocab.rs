//! Vocabulary shared by the SHACL-C parser and serializer: the keyword sets of
//! the W3C SHACL Compact Syntax grammar (CG report, `SHACLC.g4`) and the
//! datatype test its `propertyType` production applies.

pub const SH: &str = "http://www.w3.org/ns/shacl#";
pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
pub const OWL: &str = "http://www.w3.org/2002/07/owl#";

pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
pub const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
pub const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
pub const RDFS_CLASS: &str = "http://www.w3.org/2000/01/rdf-schema#Class";
pub const OWL_ONTOLOGY: &str = "http://www.w3.org/2002/07/owl#Ontology";
pub const OWL_IMPORTS: &str = "http://www.w3.org/2002/07/owl#imports";

/// The prefixes a parser starts with (CG report, "Grammar and Production
/// Rules"): a document may use them without declaring them.
pub const INITIAL_PREFIXES: &[(&str, &str)] =
    &[("rdf", RDF), ("rdfs", RDFS), ("sh", SH), ("xsd", XSD)];

/// `nodeParam`: the `name=value` parameters of a node shape body.
pub const NODE_PARAMS: &[&str] = &[
    "targetNode",
    "targetObjectsOf",
    "targetSubjectsOf",
    "deactivated",
    "severity",
    "message",
    "class",
    "datatype",
    "nodeKind",
    "minExclusive",
    "minInclusive",
    "maxExclusive",
    "maxInclusive",
    "minLength",
    "maxLength",
    "pattern",
    "flags",
    "languageIn",
    "equals",
    "disjoint",
    "closed",
    "ignoredProperties",
    "hasValue",
    "in",
];

/// `propertyParam`: the `name=value` parameters of a property shape.
pub const PROPERTY_PARAMS: &[&str] = &[
    "deactivated",
    "severity",
    "message",
    "class",
    "datatype",
    "nodeKind",
    "minExclusive",
    "minInclusive",
    "maxExclusive",
    "maxInclusive",
    "minLength",
    "maxLength",
    "pattern",
    "flags",
    "languageIn",
    "uniqueLang",
    "equals",
    "disjoint",
    "lessThan",
    "lessThanOrEquals",
    "qualifiedValueShape",
    "qualifiedMinCount",
    "qualifiedMaxCount",
    "qualifiedValueShapesDisjoint",
    "closed",
    "ignoredProperties",
    "hasValue",
    "in",
];

/// `nodeKind`: the bare node-kind keywords, each naming `sh:<keyword>`.
pub const NODE_KINDS: &[&str] = &[
    "BlankNode",
    "IRI",
    "Literal",
    "BlankNodeOrIRI",
    "BlankNodeOrLiteral",
    "IRIOrLiteral",
];

pub fn is_node_param(word: &str) -> bool {
    NODE_PARAMS.contains(&word)
}

pub fn is_property_param(word: &str) -> bool {
    PROPERTY_PARAMS.contains(&word)
}

pub fn is_node_kind(word: &str) -> bool {
    NODE_KINDS.contains(&word)
}

/// The `propertyType` test: "If ?iri is one of the RDF datatypes supported by
/// SPARQL 1.1 (such as xsd:string) then produce a triple ?property sh:datatype
/// ?iri, otherwise ?property sh:class ?iri."
///
/// Read here as every IRI in the XSD namespace plus the RDF-namespace
/// datatypes (`rdf:langString`, `rdf:HTML`, `rdf:XMLLiteral`, `rdf:JSON`). Any
/// other datatype — `geo:wktLiteral`, say — must be written `datatype=…`; a
/// bare IRI outside this set is a class. The serializer uses the same test to
/// decide when the bare form round-trips.
pub fn is_builtin_datatype(iri: &str) -> bool {
    if let Some(local) = iri.strip_prefix(XSD) {
        return !local.is_empty();
    }
    matches!(
        iri.strip_prefix(RDF),
        Some("langString" | "HTML" | "XMLLiteral" | "JSON")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_datatypes() {
        assert!(is_builtin_datatype(&format!("{XSD}string")));
        assert!(is_builtin_datatype(&format!("{XSD}nonNegativeInteger")));
        assert!(is_builtin_datatype(&format!("{RDF}langString")));
        assert!(!is_builtin_datatype(XSD));
        assert!(!is_builtin_datatype(&format!("{RDF}type")));
        assert!(!is_builtin_datatype(
            "http://www.opengis.net/ont/geosparql#wktLiteral"
        ));
    }
}
