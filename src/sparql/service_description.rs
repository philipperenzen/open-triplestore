//! SPARQL Service Description generator.
//!
//! Generates RDF (Turtle) describing the capabilities of this SPARQL endpoint,
//! per the W3C SPARQL 1.1 Service Description specification.

/// Generate a SPARQL Service Description as Turtle.
///
/// `default_graph_triples` is the `void:triples` count for the default graph.
/// `named_graphs` pairs each named-graph IRI with its own triple count, emitted
/// as `sd:graph [ a sd:Graph ; void:triples N ]` per the SPARQL 1.1 Service
/// Description recommendation.
pub fn generate(default_graph_triples: usize, named_graphs: &[(&str, usize)]) -> String {
    let mut desc = String::new();

    desc.push_str(
        r#"@prefix sd: <http://www.w3.org/ns/sparql-service-description#> .
@prefix void: <http://rdfs.org/ns/void#> .
@prefix geo: <http://www.opengis.net/ont/geosparql#> .
@prefix geof: <http://www.opengis.net/def/function/geosparql/> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

<> a sd:Service ;
    sd:endpoint <sparql> ;
    sd:supportedLanguage sd:SPARQL11Query, sd:SPARQL11Update ;
    sd:resultFormat
        <http://www.w3.org/ns/formats/SPARQL_Results_JSON> ,
        <http://www.w3.org/ns/formats/SPARQL_Results_XML> ,
        <http://www.w3.org/ns/formats/SPARQL_Results_CSV> ,
        <http://www.w3.org/ns/formats/SPARQL_Results_TSV> ,
        <http://www.w3.org/ns/formats/Turtle> ,
        <http://www.w3.org/ns/formats/N-Triples> ,
        <http://www.w3.org/ns/formats/RDF_XML> ,
        <http://www.w3.org/ns/formats/N-Quads> ,
        <http://www.w3.org/ns/formats/TriG> ;
    sd:feature sd:UnionDefaultGraph, sd:BasicFederatedQuery ;
    sd:extensionFunction
"#,
    );

    // List all GeoSPARQL extension functions
    let geo_functions = [
        "sfContains",
        "sfCrosses",
        "sfDisjoint",
        "sfEquals",
        "sfIntersects",
        "sfOverlaps",
        "sfTouches",
        "sfWithin",
        "ehContains",
        "ehCoveredBy",
        "ehCovers",
        "ehDisjoint",
        "ehEquals",
        "ehInside",
        "ehMeet",
        "ehOverlap",
        "rcc8dc",
        "rcc8ec",
        "rcc8po",
        "rcc8tppi",
        "rcc8tpp",
        "rcc8ntpp",
        "rcc8ntppi",
        "rcc8eq",
        "boundary",
        "buffer",
        "convexHull",
        "difference",
        "envelope",
        "intersection",
        "symDifference",
        "union",
        "distance",
        "area",
        "getSRID",
    ];

    for (i, func) in geo_functions.iter().enumerate() {
        let sep = if i < geo_functions.len() - 1 {
            " ,"
        } else {
            " ;"
        };
        desc.push_str(&format!("        geof:{}{}\n", func, sep));
    }

    // Dataset description
    desc.push_str(&format!(
        r#"    sd:defaultDataset [
        a sd:Dataset ;
        sd:defaultGraph [
            a sd:Graph ;
            void:triples {}
        ]"#,
        default_graph_triples
    ));

    // Named graphs — each carries its own triple count via sd:graph / void:triples.
    for (graph_iri, triples) in named_graphs {
        desc.push_str(&format!(
            r#" ;
        sd:namedGraph [
            a sd:NamedGraph ;
            sd:name <{}> ;
            sd:graph [
                a sd:Graph ;
                void:triples {}
            ]
        ]"#,
            graph_iri, triples
        ));
    }

    desc.push_str("\n    ] .\n");

    desc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_basic() {
        let desc = generate(42, &[]);
        assert!(desc.contains("sd:Service"));
        assert!(desc.contains("sd:SPARQL11Query"));
        assert!(desc.contains("void:triples 42"));
        assert!(desc.contains("geof:sfContains"));
        assert!(desc.contains("geof:distance"));
    }

    #[test]
    fn test_generate_with_named_graphs() {
        let desc = generate(100, &[("http://example.org/graph1", 7)]);
        assert!(desc.contains("http://example.org/graph1"));
        assert!(desc.contains("sd:namedGraph"));
    }

    #[test]
    fn test_named_graph_reports_triple_count() {
        let desc = generate(
            5,
            &[
                ("http://example.org/g1", 42),
                ("http://example.org/g2", 1000),
            ],
        );
        // Default graph count.
        assert!(desc.contains("void:triples 5"));
        // Each named graph exposes its own count via sd:graph / void:triples.
        assert!(desc.contains("sd:graph"));
        assert!(
            desc.contains("void:triples 42"),
            "first named graph must report its own count:\n{desc}"
        );
        assert!(
            desc.contains("void:triples 1000"),
            "second named graph must report its own count:\n{desc}"
        );
    }
}
