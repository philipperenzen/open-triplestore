//! A stream page in the store's own layout, a mapping read backwards, and a
//! SQLite target: the rows come out as the members said.

use ots_writeback::ldes::{parse_fragment, Format};
use ots_writeback::rml::invert;
use ots_writeback::sql::{Sqlite, Target};

const MAPPING: &str = r#"
@prefix rr:  <http://www.w3.org/ns/r2rml#> .
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix ql:  <http://semweb.mmlab.be/ns/ql#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <http://example.org/ontology#> .

ex:ProductsMap a rr:TriplesMap ;
  rml:logicalSource [ rml:source <urn:source:shop> ; rml:referenceFormulation ql:SQL2008 ; rr:tableName "products" ] ;
  rr:subjectMap [ rr:template "http://example.org/products/product_{product_id}" ; rr:class ex:Product ] ;
  rr:predicateObjectMap [ rr:predicate ex:name ; rr:objectMap [ rr:column "name" ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:hasPrice ; rr:objectMap [ rr:column "price" ; rr:datatype xsd:decimal ] ] ;
  rr:predicateObjectMap [ rr:predicate ex:inStock ; rr:objectMap [ rr:column "in_stock" ; rr:datatype xsd:boolean ] ] .
"#;

/// Three members: a new product, a changed one, and one that disappeared —
/// plus an entity no rule recognises.
const PAGE: &str = r#"
<http://store/api/datasets/shop/ldes> <https://w3id.org/tree#member> <http://store/api/datasets/shop/ldes/members/21> .
<http://store/api/datasets/shop/ldes/members/21> <http://purl.org/dc/terms/isVersionOf> <http://example.org/products/product_9> .
<http://store/api/datasets/shop/ldes/members/21> <http://purl.org/dc/terms/created> "2026-02-01T00:00:00Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
<http://store/api/datasets/shop/ldes/members/21> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/ontology#Product> .
<http://store/api/datasets/shop/ldes/members/21> <http://example.org/ontology#name> "Washer" .
<http://store/api/datasets/shop/ldes/members/21> <http://example.org/ontology#hasPrice> "0.05"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<http://store/api/datasets/shop/ldes/members/21> <http://example.org/ontology#inStock> "true"^^<http://www.w3.org/2001/XMLSchema#boolean> .
<http://store/api/datasets/shop/ldes/members/21> <http://example.org/ontology#colour> "zinc" .
<http://store/api/datasets/shop/ldes> <https://w3id.org/tree#member> <http://store/api/datasets/shop/ldes/members/22> .
<http://store/api/datasets/shop/ldes/members/22> <http://purl.org/dc/terms/isVersionOf> <http://example.org/products/product_7> .
<http://store/api/datasets/shop/ldes/members/22> <http://purl.org/dc/terms/created> "2026-02-01T00:00:01Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
<http://store/api/datasets/shop/ldes/members/22> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/ontology#Product> .
<http://store/api/datasets/shop/ldes/members/22> <http://example.org/ontology#name> "Bolt M8" .
<http://store/api/datasets/shop/ldes/members/22> <http://example.org/ontology#hasPrice> "0.30"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<http://store/api/datasets/shop/ldes> <https://w3id.org/tree#member> <http://store/api/datasets/shop/ldes/members/23> .
<http://store/api/datasets/shop/ldes/members/23> <http://purl.org/dc/terms/isVersionOf> <http://example.org/products/product_8> .
<http://store/api/datasets/shop/ldes/members/23> <http://purl.org/dc/terms/created> "2026-02-01T00:00:02Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
<http://store/api/datasets/shop/ldes/members/23> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://opentriplestore.org/ns#Tombstone> .
<http://store/api/datasets/shop/ldes> <https://w3id.org/tree#member> <http://store/api/datasets/shop/ldes/members/24> .
<http://store/api/datasets/shop/ldes/members/24> <http://purl.org/dc/terms/isVersionOf> <http://example.org/categories/1> .
<http://store/api/datasets/shop/ldes/members/24> <http://purl.org/dc/terms/created> "2026-02-01T00:00:03Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
<http://store/api/datasets/shop/ldes/members/24> <http://example.org/ontology#label> "Fasteners" .
"#;

#[test]
fn a_page_becomes_rows_and_the_worker_is_idempotent() {
    let (rules, warnings) = invert(MAPPING).unwrap();
    assert_eq!(rules.len(), 1);
    assert!(warnings.is_empty(), "{warnings:?}");
    let fragment = parse_fragment(PAGE, Format::NTriples).unwrap();
    assert_eq!(fragment.members.len(), 4);

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE products (product_id INTEGER PRIMARY KEY, name TEXT NOT NULL, price REAL, \
                                in_stock INTEGER, colour TEXT DEFAULT 'grey');
         INSERT INTO products VALUES (7, 'Bolt', 0.25, 1, 'silver');
         INSERT INTO products VALUES (8, 'Nut', 1.50, 0, 'grey');",
    )
    .unwrap();
    let mut target = Sqlite(conn);

    let plan = ots_writeback::plan(&fragment.members, &rules);
    assert_eq!(plan.changes.len(), 3, "{plan:?}");
    assert_eq!(
        plan.skipped,
        vec!["http://example.org/categories/1".to_string()]
    );
    target.apply(&plan.changes).unwrap();

    let rows: Vec<(i64, String, f64, Option<i64>, String)> = target
        .0
        .prepare(
            "SELECT product_id, name, price, in_stock, colour FROM products ORDER BY product_id",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec![
            // Changed: the name and the price moved, the columns the member
            // did not carry stayed.
            (
                7,
                "Bolt M8".to_string(),
                0.30,
                Some(1),
                "silver".to_string()
            ),
            // New: the row the member described, the unmapped predicate
            // (colour) untouched — the table's default applies.
            (9, "Washer".to_string(), 0.05, Some(1), "grey".to_string()),
        ]
    );

    // Applying the same page again changes nothing.
    target.apply(&plan.changes).unwrap();
    let count: i64 = target
        .0
        .query_row("SELECT COUNT(*) FROM products", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);
}
