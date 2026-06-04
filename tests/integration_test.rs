//! Integration tests for the local triple store.
//!
//! Tests the full stack: loading data, querying, GeoSPARQL functions,
//! SPARQL UPDATE operations, and store persistence.

use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

/// Helper to create a fresh in-memory store for testing.
fn test_store() -> open_triplestore::store::TripleStore {
    open_triplestore::store::TripleStore::in_memory().unwrap()
}

/// Helper to load Turtle data into a store.
fn load_turtle(store: &open_triplestore::store::TripleStore, ttl: &str) {
    store.load_str(ttl, RdfFormat::Turtle, None).unwrap();
}

/// Helper to get SELECT query results as a vec of rows (each row = vec of string values).
fn select_results(
    store: &open_triplestore::store::TripleStore,
    query: &str,
) -> Vec<Vec<String>> {
    match store.query(query).unwrap() {
        QueryResults::Solutions(solutions) => {
            let vars: Vec<String> = solutions
                .variables()
                .iter()
                .map(|v| v.as_str().to_string())
                .collect();
            solutions
                .into_iter()
                .map(|sol| {
                    let sol = sol.unwrap();
                    vars.iter()
                        .map(|v| {
                            sol.get(v.as_str())
                                .map(|t| t.to_string())
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .collect()
        }
        _ => panic!("Expected SELECT results"),
    }
}

/// Helper to get ASK query result.
fn ask_result(store: &open_triplestore::store::TripleStore, query: &str) -> bool {
    match store.query(query).unwrap() {
        QueryResults::Boolean(b) => b,
        _ => panic!("Expected ASK result"),
    }
}

// ═══════════════════════════════════════════════════════════
// Basic SPARQL Query Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_select_basic() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:name "Alice" .
        ex:bob ex:name "Bob" .
        ex:charlie ex:name "Charlie" .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?name WHERE { ?s <http://example.org/name> ?name } ORDER BY ?name",
    );
    assert_eq!(results.len(), 3);
    assert!(results[0][0].contains("Alice"));
    assert!(results[1][0].contains("Bob"));
    assert!(results[2][0].contains("Charlie"));
}

#[test]
fn test_select_with_filter() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:a ex:value 10 .
        ex:b ex:value 20 .
        ex:c ex:value 30 .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?v WHERE { ?s <http://example.org/value> ?v FILTER(?v > 15) } ORDER BY ?v",
    );
    assert_eq!(results.len(), 2);
}

#[test]
fn test_select_optional() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:name "Alice" ; ex:age 30 .
        ex:bob ex:name "Bob" .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?name ?age WHERE { ?s <http://example.org/name> ?name . OPTIONAL { ?s <http://example.org/age> ?age } } ORDER BY ?name",
    );
    assert_eq!(results.len(), 2);
    // Alice has age, Bob doesn't
    assert!(!results[0][1].is_empty()); // Alice's age
    assert!(results[1][1].is_empty()); // Bob's age (unbound)
}

#[test]
fn test_select_union() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:firstName "Alice" .
        ex:bob ex:givenName "Bob" .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?name WHERE { { ?s <http://example.org/firstName> ?name } UNION { ?s <http://example.org/givenName> ?name } } ORDER BY ?name",
    );
    assert_eq!(results.len(), 2);
}

#[test]
fn test_ask() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:name "Alice" .
    "#,
    );

    assert!(ask_result(
        &store,
        "ASK { <http://example.org/alice> <http://example.org/name> \"Alice\" }"
    ));
    assert!(!ask_result(
        &store,
        "ASK { <http://example.org/alice> <http://example.org/name> \"Bob\" }"
    ));
}

#[test]
fn test_construct() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:name "Alice" .
    "#,
    );

    let results = store
        .query(
            "CONSTRUCT { ?s <http://example.org/label> ?name } WHERE { ?s <http://example.org/name> ?name }",
        )
        .unwrap();

    match results {
        QueryResults::Graph(triples) => {
            let count = triples.count();
            assert_eq!(count, 1);
        }
        _ => panic!("Expected CONSTRUCT graph results"),
    }
}

#[test]
fn test_describe() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:name "Alice" ; ex:age 30 .
    "#,
    );

    let results = store
        .query("DESCRIBE <http://example.org/alice>")
        .unwrap();

    match results {
        QueryResults::Graph(triples) => {
            let count = triples.count();
            assert!(count >= 2); // at least name and age
        }
        _ => panic!("Expected DESCRIBE graph results"),
    }
}

// ═══════════════════════════════════════════════════════════
// Aggregation Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_aggregate_count() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:a ex:p "1" .
        ex:b ex:p "2" .
        ex:c ex:p "3" .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT (COUNT(*) as ?count) WHERE { ?s ?p ?o }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("3"));
}

#[test]
fn test_aggregate_sum_avg() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:a ex:value 10 .
        ex:b ex:value 20 .
        ex:c ex:value 30 .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT (SUM(?v) as ?sum) (AVG(?v) as ?avg) WHERE { ?s <http://example.org/value> ?v }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("60"));
    assert!(results[0][1].contains("20"));
}

#[test]
fn test_group_by() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:a ex:type "X" ; ex:value 10 .
        ex:b ex:type "X" ; ex:value 20 .
        ex:c ex:type "Y" ; ex:value 30 .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?type (COUNT(?s) as ?count) WHERE { ?s <http://example.org/type> ?type . ?s <http://example.org/value> ?v } GROUP BY ?type ORDER BY ?type",
    );
    assert_eq!(results.len(), 2);
}

// ═══════════════════════════════════════════════════════════
// Property Path Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_property_path_sequence() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:knows ex:bob .
        ex:bob ex:knows ex:charlie .
    "#,
    );

    // Sequence path: alice -> knows -> knows -> charlie
    let results = select_results(
        &store,
        "SELECT ?person WHERE { <http://example.org/alice> <http://example.org/knows>/<http://example.org/knows> ?person }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("charlie"));
}

#[test]
fn test_property_path_transitive() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:a ex:parent ex:b .
        ex:b ex:parent ex:c .
        ex:c ex:parent ex:d .
    "#,
    );

    // Transitive closure: all ancestors of a
    let results = select_results(
        &store,
        "SELECT ?ancestor WHERE { <http://example.org/a> <http://example.org/parent>+ ?ancestor }",
    );
    assert_eq!(results.len(), 3); // b, c, d
}

#[test]
fn test_property_path_inverse() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:knows ex:bob .
    "#,
    );

    // Inverse path: who knows bob?
    let results = select_results(
        &store,
        "SELECT ?person WHERE { <http://example.org/bob> ^<http://example.org/knows> ?person }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("alice"));
}

// ═══════════════════════════════════════════════════════════
// Subquery Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_subquery() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:a ex:value 10 .
        ex:b ex:value 20 .
        ex:c ex:value 30 .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?s ?v WHERE { ?s <http://example.org/value> ?v . { SELECT (MAX(?val) as ?maxVal) WHERE { ?x <http://example.org/value> ?val } } FILTER(?v = ?maxVal) }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][1].contains("30"));
}

// ═══════════════════════════════════════════════════════════
// BIND and VALUES Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_bind() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:a ex:value 10 .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?doubled WHERE { ?s <http://example.org/value> ?v . BIND(?v * 2 AS ?doubled) }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("20"));
}

#[test]
fn test_values() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:name "Alice" .
        ex:bob ex:name "Bob" .
        ex:charlie ex:name "Charlie" .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?name WHERE { VALUES ?s { <http://example.org/alice> <http://example.org/charlie> } ?s <http://example.org/name> ?name } ORDER BY ?name",
    );
    assert_eq!(results.len(), 2);
    assert!(results[0][0].contains("Alice"));
    assert!(results[1][0].contains("Charlie"));
}

// ═══════════════════════════════════════════════════════════
// Negation Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_not_exists() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:name "Alice" ; ex:email "alice@example.org" .
        ex:bob ex:name "Bob" .
    "#,
    );

    // Find people without email
    let results = select_results(
        &store,
        "SELECT ?name WHERE { ?s <http://example.org/name> ?name . FILTER NOT EXISTS { ?s <http://example.org/email> ?e } }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("Bob"));
}

#[test]
fn test_minus() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:name "Alice" ; ex:email "alice@example.org" .
        ex:bob ex:name "Bob" .
    "#,
    );

    // MINUS must be inside the WHERE clause group pattern per SPARQL 1.1 grammar
    let results = select_results(
        &store,
        "SELECT ?name WHERE { ?s <http://example.org/name> ?name . MINUS { ?s <http://example.org/email> ?e } }",
    );
    // MINUS semantics: Bob has no email, so should remain
    assert!(!results.is_empty());
}

// ═══════════════════════════════════════════════════════════
// SPARQL Update Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_insert_data() {
    let store = test_store();
    store
        .update("INSERT DATA { <http://example.org/s> <http://example.org/p> \"value\" }")
        .unwrap();
    assert_eq!(store.len().unwrap(), 1);
}

#[test]
fn test_delete_data() {
    let store = test_store();
    store
        .update("INSERT DATA { <http://example.org/s> <http://example.org/p> \"value\" }")
        .unwrap();
    store
        .update("DELETE DATA { <http://example.org/s> <http://example.org/p> \"value\" }")
        .unwrap();
    assert_eq!(store.len().unwrap(), 0);
}

#[test]
fn test_insert_where() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:name "Alice" .
    "#,
    );

    store
        .update(
            "INSERT { ?s <http://example.org/label> ?name } WHERE { ?s <http://example.org/name> ?name }",
        )
        .unwrap();

    assert!(ask_result(
        &store,
        "ASK { <http://example.org/alice> <http://example.org/label> \"Alice\" }"
    ));
}

#[test]
fn test_delete_insert_where() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:status "active" .
    "#,
    );

    store
        .update(
            r#"DELETE { ?s <http://example.org/status> "active" }
               INSERT { ?s <http://example.org/status> "archived" }
               WHERE  { ?s <http://example.org/status> "active" }"#,
        )
        .unwrap();

    assert!(ask_result(
        &store,
        "ASK { <http://example.org/alice> <http://example.org/status> \"archived\" }",
    ));
    assert!(!ask_result(
        &store,
        "ASK { <http://example.org/alice> <http://example.org/status> \"active\" }",
    ));
}

// ═══════════════════════════════════════════════════════════
// Named Graph Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_named_graphs() {
    let store = test_store();
    store
        .load_str(
            "<http://example.org/s1> <http://example.org/p> \"in-graph-1\" .",
            RdfFormat::NTriples,
            Some("http://example.org/graph1"),
        )
        .unwrap();
    store
        .load_str(
            "<http://example.org/s2> <http://example.org/p> \"in-graph-2\" .",
            RdfFormat::NTriples,
            Some("http://example.org/graph2"),
        )
        .unwrap();

    // Query specific graph
    let results = select_results(
        &store,
        "SELECT ?o WHERE { GRAPH <http://example.org/graph1> { ?s <http://example.org/p> ?o } }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("in-graph-1"));

    // List named graphs
    let graphs = store.named_graphs().unwrap();
    assert_eq!(graphs.len(), 2);
}

// ═══════════════════════════════════════════════════════════
// SPARQL Built-in Function Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_string_functions() {
    let store = test_store();

    // STRLEN
    let results = select_results(&store, "SELECT (STRLEN(\"hello\") AS ?len) WHERE {}");
    assert!(results[0][0].contains("5"));

    // UCASE / LCASE
    let results = select_results(&store, "SELECT (UCASE(\"hello\") AS ?upper) WHERE {}");
    assert!(results[0][0].contains("HELLO"));

    let results = select_results(&store, "SELECT (LCASE(\"HELLO\") AS ?lower) WHERE {}");
    assert!(results[0][0].contains("hello"));

    // CONCAT
    let results =
        select_results(&store, "SELECT (CONCAT(\"hello\", \" \", \"world\") AS ?str) WHERE {}");
    assert!(results[0][0].contains("hello world"));

    // CONTAINS
    let results = select_results(
        &store,
        "SELECT (CONTAINS(\"hello world\", \"world\") AS ?has) WHERE {}",
    );
    assert!(results[0][0].contains("true"));

    // SUBSTR
    let results = select_results(&store, "SELECT (SUBSTR(\"hello\", 2, 3) AS ?sub) WHERE {}");
    assert!(results[0][0].contains("ell"));
}

#[test]
fn test_numeric_functions() {
    let store = test_store();

    let results = select_results(&store, "SELECT (ABS(-5) AS ?v) WHERE {}");
    assert!(results[0][0].contains("5"));

    let results = select_results(&store, "SELECT (CEIL(4.2) AS ?v) WHERE {}");
    assert!(results[0][0].contains("5"));

    let results = select_results(&store, "SELECT (FLOOR(4.8) AS ?v) WHERE {}");
    assert!(results[0][0].contains("4"));

    let results = select_results(&store, "SELECT (ROUND(4.5) AS ?v) WHERE {}");
    assert!(results[0][0].contains("5") || results[0][0].contains("4")); // implementation-dependent rounding
}

#[test]
fn test_type_functions() {
    let store = test_store();

    // isIRI
    let results = select_results(
        &store,
        "SELECT (isIRI(<http://example.org/a>) AS ?v) WHERE {}",
    );
    assert!(results[0][0].contains("true"));

    // isLiteral
    let results = select_results(&store, "SELECT (isLiteral(\"hello\") AS ?v) WHERE {}");
    assert!(results[0][0].contains("true"));

    // DATATYPE
    let results = select_results(&store, "SELECT (DATATYPE(42) AS ?dt) WHERE {}");
    assert!(results[0][0].contains("integer"));
}

#[test]
fn test_hash_functions() {
    let store = test_store();

    // MD5
    let results = select_results(&store, "SELECT (MD5(\"hello\") AS ?h) WHERE {}");
    assert!(!results[0][0].is_empty());

    // SHA256
    let results = select_results(&store, "SELECT (SHA256(\"hello\") AS ?h) WHERE {}");
    assert!(!results[0][0].is_empty());
}

#[test]
fn test_datetime_functions() {
    let store = test_store();

    // NOW()
    let results = select_results(&store, "SELECT (NOW() AS ?now) WHERE {}");
    assert!(!results[0][0].is_empty());

    // YEAR, MONTH, DAY
    let results = select_results(
        &store,
        "SELECT (YEAR(\"2024-06-15T10:30:00\"^^<http://www.w3.org/2001/XMLSchema#dateTime>) AS ?y) WHERE {}",
    );
    assert!(results[0][0].contains("2024"));
}

// ═══════════════════════════════════════════════════════════
// GeoSPARQL Integration Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_geosparql_contains_in_query() {
    let store = test_store();

    let results = select_results(
        &store,
        r#"
        PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
        PREFIX geo: <http://www.opengis.net/ont/geosparql#>
        SELECT ?result WHERE {
            BIND(geof:sfContains(
                "POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"^^geo:wktLiteral,
                "POINT(5 5)"^^geo:wktLiteral
            ) AS ?result)
        }
        "#,
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("true"));
}

#[test]
fn test_geosparql_distance_in_query() {
    let store = test_store();

    let results = select_results(
        &store,
        r#"
        PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
        PREFIX geo: <http://www.opengis.net/ont/geosparql#>
        SELECT ?dist WHERE {
            BIND(geof:distance(
                "POINT(0 0)"^^geo:wktLiteral,
                "POINT(3 4)"^^geo:wktLiteral
            ) AS ?dist)
        }
        "#,
    );
    assert_eq!(results.len(), 1);
    // Distance should be 5.0
    let dist_str = &results[0][0];
    assert!(dist_str.contains("5"));
}

#[test]
fn test_geosparql_intersects_in_query() {
    let store = test_store();

    let results = select_results(
        &store,
        r#"
        PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
        PREFIX geo: <http://www.opengis.net/ont/geosparql#>
        SELECT ?result WHERE {
            BIND(geof:sfIntersects(
                "POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"^^geo:wktLiteral,
                "POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))"^^geo:wktLiteral
            ) AS ?result)
        }
        "#,
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("true"));
}

#[test]
fn test_geosparql_with_loaded_data() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        @prefix geo: <http://www.opengis.net/ont/geosparql#> .
        @prefix sf: <http://www.opengis.net/ont/sf#> .

        ex:park a geo:Feature ;
            geo:hasGeometry [
                a sf:Polygon ;
                geo:asWKT "POLYGON((0 0, 100 0, 100 100, 0 100, 0 0))"^^geo:wktLiteral
            ] .

        ex:house a geo:Feature ;
            geo:hasGeometry [
                a sf:Point ;
                geo:asWKT "POINT(50 50)"^^geo:wktLiteral
            ] .

        ex:lake a geo:Feature ;
            geo:hasGeometry [
                a sf:Point ;
                geo:asWKT "POINT(200 200)"^^geo:wktLiteral
            ] .
    "#,
    );

    // Find features whose geometry is within the park
    let results = select_results(
        &store,
        r#"
        PREFIX ex: <http://example.org/>
        PREFIX geo: <http://www.opengis.net/ont/geosparql#>
        PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
        SELECT ?feature WHERE {
            ex:park geo:hasGeometry/geo:asWKT ?parkWkt .
            ?feature geo:hasGeometry/geo:asWKT ?featureWkt .
            FILTER(?feature != ex:park)
            FILTER(geof:sfContains(?parkWkt, ?featureWkt))
        }
        "#,
    );

    // House is inside park, lake is outside
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("house"));
}

#[test]
fn test_geosparql_convex_hull_in_query() {
    let store = test_store();

    let results = select_results(
        &store,
        r#"
        PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
        PREFIX geo: <http://www.opengis.net/ont/geosparql#>
        SELECT ?hull WHERE {
            BIND(geof:convexHull(
                "MULTIPOINT((0 0), (10 0), (5 10), (3 3))"^^geo:wktLiteral
            ) AS ?hull)
        }
        "#,
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("POLYGON"));
}

#[test]
fn test_geosparql_buffer_in_query() {
    let store = test_store();

    let results = select_results(
        &store,
        r#"
        PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
        PREFIX geo: <http://www.opengis.net/ont/geosparql#>
        SELECT ?buffered WHERE {
            BIND(geof:buffer(
                "POINT(0 0)"^^geo:wktLiteral,
                "5.0"^^<http://www.w3.org/2001/XMLSchema#double>
            ) AS ?buffered)
        }
        "#,
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("POLYGON"));
}

#[test]
fn test_geosparql_union_difference() {
    let store = test_store();

    // Union
    let results = select_results(
        &store,
        r#"
        PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
        PREFIX geo: <http://www.opengis.net/ont/geosparql#>
        SELECT ?result WHERE {
            BIND(geof:union(
                "POLYGON((0 0, 5 0, 5 5, 0 5, 0 0))"^^geo:wktLiteral,
                "POLYGON((3 3, 8 3, 8 8, 3 8, 3 3))"^^geo:wktLiteral
            ) AS ?result)
        }
        "#,
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("POLYGON"));

    // Difference
    let results = select_results(
        &store,
        r#"
        PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
        PREFIX geo: <http://www.opengis.net/ont/geosparql#>
        SELECT ?result WHERE {
            BIND(geof:difference(
                "POLYGON((0 0, 10 0, 10 10, 0 10, 0 0))"^^geo:wktLiteral,
                "POLYGON((5 5, 15 5, 15 15, 5 15, 5 5))"^^geo:wktLiteral
            ) AS ?result)
        }
        "#,
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("POLYGON"));
}

// ═══════════════════════════════════════════════════════════
// Graph Store Protocol Tests (via store API)
// ═══════════════════════════════════════════════════════════

#[test]
fn test_graph_store_put_get_delete() {
    let store = test_store();

    // PUT data into a named graph
    store
        .graph_store_put(
            Some("http://example.org/mygraph"),
            "<http://example.org/s> <http://example.org/p> \"value\" .",
            RdfFormat::NTriples,
        )
        .unwrap();

    // GET the graph
    let data = store
        .graph_store_get(Some("http://example.org/mygraph"), RdfFormat::NTriples)
        .unwrap();
    let data_str = String::from_utf8(data).unwrap();
    assert!(data_str.contains("example.org"));

    // DELETE the graph
    store
        .graph_store_delete(Some("http://example.org/mygraph"))
        .unwrap();

    // Verify it's empty
    let data = store
        .graph_store_get(Some("http://example.org/mygraph"), RdfFormat::NTriples)
        .unwrap();
    assert!(data.is_empty() || String::from_utf8(data).unwrap().trim().is_empty());
}

#[test]
fn test_graph_store_post_merge() {
    let store = test_store();

    // POST data into default graph
    store
        .graph_store_post(
            None,
            "<http://example.org/s1> <http://example.org/p> \"v1\" .",
            RdfFormat::NTriples,
        )
        .unwrap();

    // POST more data (merge)
    store
        .graph_store_post(
            None,
            "<http://example.org/s2> <http://example.org/p> \"v2\" .",
            RdfFormat::NTriples,
        )
        .unwrap();

    assert_eq!(store.len().unwrap(), 2);
}

// ═══════════════════════════════════════════════════════════
// RDF Data Model Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_blank_nodes() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:alice ex:knows [ ex:name "Anonymous" ] .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?name WHERE { <http://example.org/alice> <http://example.org/knows> ?bn . ?bn <http://example.org/name> ?name }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("Anonymous"));
}

#[test]
fn test_language_tagged_literals() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:paris ex:name "Paris"@en, "Paris"@fr, "Pariisi"@fi .
    "#,
    );

    // Filter by language
    let results = select_results(
        &store,
        "SELECT ?name WHERE { <http://example.org/paris> <http://example.org/name> ?name . FILTER(LANG(?name) = \"fi\") }",
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("Pariisi"));
}

#[test]
fn test_typed_literals() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:a ex:intVal 42 .
        ex:a ex:floatVal 3.14 .
        ex:a ex:boolVal true .
        ex:a ex:dateVal "2024-01-15"^^xsd:date .
    "#,
    );

    let results = select_results(
        &store,
        "SELECT ?v WHERE { <http://example.org/a> <http://example.org/intVal> ?v }",
    );
    assert!(results[0][0].contains("42"));
}

#[test]
fn test_multiple_rdf_formats() {
    let store = test_store();

    // Load N-Triples
    store
        .load_str(
            "<http://example.org/s1> <http://example.org/p> \"ntriples\" .\n",
            RdfFormat::NTriples,
            None,
        )
        .unwrap();

    // Load Turtle
    store
        .load_str(
            "@prefix ex: <http://example.org/> .\nex:s2 ex:p \"turtle\" .\n",
            RdfFormat::Turtle,
            None,
        )
        .unwrap();

    assert_eq!(store.len().unwrap(), 2);

    // Dump as N-Triples
    let nt = store.dump(RdfFormat::NTriples, None).unwrap();
    let nt_str = String::from_utf8(nt).unwrap();
    assert!(nt_str.contains("ntriples"));
    assert!(nt_str.contains("turtle"));
}

// ═══════════════════════════════════════════════════════════
// Persistence Tests
// ═══════════════════════════════════════════════════════════

#[test]
#[ignore = "Oxigraph RocksDB TryFromIntError on macOS arm64 - works in Docker/Linux"]
fn test_persistence_across_reopens() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Open, load, close
    {
        let store = open_triplestore::store::TripleStore::open(tmp.path()).unwrap();
        store
            .load_str(
                "<http://example.org/s> <http://example.org/p> \"persistent\" .",
                RdfFormat::NTriples,
                None,
            )
            .unwrap();
        assert_eq!(store.len().unwrap(), 1);
    }

    // Reopen and verify
    {
        let store = open_triplestore::store::TripleStore::open(tmp.path()).unwrap();
        assert_eq!(store.len().unwrap(), 1);
        assert!(ask_result(
            &store,
            "ASK { <http://example.org/s> <http://example.org/p> \"persistent\" }"
        ));
    }
}

// ═══════════════════════════════════════════════════════════
// Concurrent Access Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_concurrent_queries() {
    let store = test_store();
    load_turtle(
        &store,
        r#"
        @prefix ex: <http://example.org/> .
        ex:a ex:value 1 .
        ex:b ex:value 2 .
        ex:c ex:value 3 .
    "#,
    );

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || {
                let results = select_results(
                    &store,
                    "SELECT (COUNT(*) as ?c) WHERE { ?s ?p ?o }",
                );
                assert!(results[0][0].contains("3"));
            })
        })
        .collect();

    for h in handles {
        h.join().unwrap();
    }
}

// ═══════════════════════════════════════════════════════════
// SPARQL 1.2 / RDF-star Tests
// ═══════════════════════════════════════════════════════════

#[test]
fn test_rdf_star_embedded_triples() {
    let store = test_store();

    // Load RDF-star data using SPARQL UPDATE with embedded triples
    store
        .update(
            r#"
            INSERT DATA {
                << <http://example.org/alice> <http://example.org/knows> <http://example.org/bob> >>
                    <http://example.org/certainty> "0.9"^^<http://www.w3.org/2001/XMLSchema#double> .
            }
            "#,
        )
        .unwrap();

    // Query the embedded triple metadata
    let results = select_results(
        &store,
        r#"
        SELECT ?certainty WHERE {
            << <http://example.org/alice> <http://example.org/knows> <http://example.org/bob> >>
                <http://example.org/certainty> ?certainty .
        }
        "#,
    );
    assert_eq!(results.len(), 1);
    assert!(results[0][0].contains("0.9"));
}
