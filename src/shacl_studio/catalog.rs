//! Shape **discovery** across the whole store — powers the Studio "Shapes"
//! catalog. The user wanted to *see all the shapes* (to pick from and compose a
//! new shape graph), wherever they live: in a dedicated shape graph, or
//! "joined with instance data/models" inside a data graph.
//!
//! Real stores hold *tens of thousands* of shapes (e.g. a large information
//! model is thousands of `sh:NodeShape`s), so discovery is **graph-first**:
//! [`catalog_graph_summary`] lists the source graphs with shape counts (cheap),
//! and [`catalog_shapes`] returns the shapes of **one** graph on demand. Internal
//! `urn:system:` graphs (validation layer, commit log, dataset metadata, version
//! snapshots, the built-in SHACL-SHACL graph) are skipped — they are plumbing.

use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use serde::Serialize;

use crate::store::engine::StoreError;
use crate::store::TripleStore;

/// A graph that contains SHACL shapes, with its shape counts. The cheap,
/// always-loaded top level of the catalog.
#[derive(Debug, Clone, Serialize)]
pub struct GraphSummary {
    pub graph: String,
    pub node_count: usize,
    pub property_count: usize,
}

/// One discovered shape (a `sh:NodeShape` or `sh:PropertyShape` subject).
#[derive(Debug, Clone, Serialize)]
pub struct CatalogShape {
    /// The named graph the shape lives in.
    pub graph: String,
    /// The shape's IRI. (Blank-node shapes are not surfaced — they are inner
    /// parts of a named shape, not independently pickable.)
    pub shape: String,
    /// `"node"` or `"property"`.
    pub kind: String,
    pub label: Option<String>,
    /// `sh:targetClass` values, if any.
    pub target_classes: Vec<String>,
    /// `sh:path` (for property shapes / shapes that declare one), as a hint.
    pub path: Option<String>,
}

fn opt_str(t: Option<&Term>) -> Option<String> {
    match t {
        Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
        Some(Term::Literal(l)) => Some(l.value().to_string()),
        _ => None,
    }
}

fn kind_of(t: Option<&Term>) -> String {
    match t {
        Some(Term::NamedNode(n)) if n.as_str().ends_with("PropertyShape") => "property",
        _ => "node",
    }
    .to_string()
}

/// Every non-system named graph that contains IRI-named SHACL shapes, with a
/// breakdown of node vs property shapes. One `GROUP BY` query — cheap even when
/// the store holds tens of thousands of shapes.
pub fn catalog_graph_summary(store: &TripleStore) -> Vec<GraphSummary> {
    let q = r#"
        PREFIX sh: <http://www.w3.org/ns/shacl#>
        SELECT ?g ?t (COUNT(DISTINCT ?shape) AS ?n) WHERE {
          GRAPH ?g {
            ?shape a ?t .
            FILTER(?t IN (sh:NodeShape, sh:PropertyShape))
            FILTER(isIRI(?shape))
          }
          FILTER(!STRSTARTS(STR(?g), "urn:system:"))
        }
        GROUP BY ?g ?t
        ORDER BY ?g
    "#;
    let mut by_graph: std::collections::BTreeMap<String, GraphSummary> =
        std::collections::BTreeMap::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(q) {
        for row in sols.flatten() {
            let vals = row.values();
            let get = |i: usize| vals.get(i).and_then(|t| t.as_ref());
            let Some(Term::NamedNode(g)) = get(0) else {
                continue;
            };
            let is_property =
                matches!(get(1), Some(Term::NamedNode(n)) if n.as_str().ends_with("PropertyShape"));
            let n: usize = match get(2) {
                Some(Term::Literal(l)) => l.value().parse().unwrap_or(0),
                _ => 0,
            };
            let e = by_graph
                .entry(g.as_str().to_string())
                .or_insert_with(|| GraphSummary {
                    graph: g.as_str().to_string(),
                    node_count: 0,
                    property_count: 0,
                });
            if is_property {
                e.property_count += n;
            } else {
                e.node_count += n;
            }
        }
    }
    by_graph.into_values().collect()
}

/// The IRI-named SHACL shapes in **one** graph. Aggregated per shape (a shape
/// with several `sh:targetClass` values collapses into one entry).
pub fn catalog_shapes(store: &TripleStore, graph: &str) -> Vec<CatalogShape> {
    let q = format!(
        r#"
        PREFIX sh: <http://www.w3.org/ns/shacl#>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        SELECT ?shape ?t ?label ?tc ?path WHERE {{
          GRAPH <{graph}> {{
            ?shape a ?t .
            FILTER(?t IN (sh:NodeShape, sh:PropertyShape))
            FILTER(isIRI(?shape))
            OPTIONAL {{ ?shape rdfs:label ?label }}
            OPTIONAL {{ ?shape sh:targetClass ?tc }}
            OPTIONAL {{ ?shape sh:path ?path }}
          }}
        }}
        ORDER BY ?shape
    "#
    );
    let mut acc: std::collections::BTreeMap<String, CatalogShape> =
        std::collections::BTreeMap::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
        for row in sols.flatten() {
            let vals = row.values();
            let get = |i: usize| vals.get(i).and_then(|t| t.as_ref());
            let Some(Term::NamedNode(shape)) = get(0) else {
                continue;
            };
            let shape_iri = shape.as_str().to_string();
            let entry = acc
                .entry(shape_iri.clone())
                .or_insert_with(|| CatalogShape {
                    graph: graph.to_string(),
                    shape: shape_iri,
                    kind: kind_of(get(1)),
                    label: None,
                    target_classes: Vec::new(),
                    path: None,
                });
            if entry.label.is_none() {
                entry.label = opt_str(get(2));
            }
            if let Some(Term::NamedNode(tc)) = get(3) {
                let tc = tc.as_str().to_string();
                if !entry.target_classes.contains(&tc) {
                    entry.target_classes.push(tc);
                }
            }
            if entry.path.is_none() {
                entry.path = opt_str(get(4));
            }
        }
    }
    acc.into_values().collect()
}

/// Copy each selected shape — together with its full reachable closure
/// (blank-node guts and any shapes it references) — from its source graph into
/// `target_graph`. This is the **compose** primitive: "add an existing shape to
/// this shape graph". Copy semantics — the shape becomes part of the target
/// graph and is independent of the source thereafter.
///
/// `(!<…never…>)*` is the standard "any predicate, any depth" path, so the whole
/// subtree rooted at the shape travels. The copy runs entirely inside the store,
/// so blank-node identity is preserved without label collisions, and re-running
/// is idempotent (identical triples are no-ops). Returns the number of shapes
/// processed.
pub fn import_shapes_into(
    store: &TripleStore,
    target_graph: &str,
    shapes: &[(String, String)],
) -> Result<usize, StoreError> {
    for (src, shape) in shapes {
        let q = format!(
            r#"INSERT {{ GRAPH <{target_graph}> {{ ?s ?p ?o }} }}
               WHERE {{ GRAPH <{src}> {{
                 <{shape}> (!<urn:opentriplestore:never>)* ?s .
                 ?s ?p ?o .
               }} }}"#
        );
        store.update(&q)?;
    }
    Ok(shapes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::RdfFormat;

    fn seed(store: &TripleStore) {
        store
            .graph_store_put(
                Some("urn:shapes:a"),
                r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
                   @prefix ex: <http://ex/> .
                   ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person .
                   ex:NameShape a sh:PropertyShape ; sh:path ex:name ."#,
                RdfFormat::Turtle,
            )
            .unwrap();
        store
            .graph_store_put(
                Some("urn:data:mixed"),
                r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
                   @prefix ex: <http://ex/> .
                   ex:Alice a ex:Person .
                   ex:MixedShape a sh:NodeShape ; sh:targetClass ex:Person ."#,
                RdfFormat::Turtle,
            )
            .unwrap();
        store
            .graph_store_put(
                Some("urn:system:shapes:shacl-shacl"),
                r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
                   @prefix ex: <http://ex/> .
                   ex:MetaShape a sh:NodeShape ; sh:targetClass sh:NodeShape ."#,
                RdfFormat::Turtle,
            )
            .unwrap();
    }

    #[test]
    fn summary_counts_by_graph_and_skips_system() {
        let store = TripleStore::in_memory().unwrap();
        seed(&store);
        let summary = catalog_graph_summary(&store);
        let graphs: Vec<&str> = summary.iter().map(|g| g.graph.as_str()).collect();
        assert!(
            graphs.contains(&"urn:shapes:a"),
            "dedicated shape graph listed"
        );
        assert!(
            graphs.contains(&"urn:data:mixed"),
            "data graph with shapes listed"
        );
        assert!(
            !graphs.contains(&"urn:system:shapes:shacl-shacl"),
            "urn:system: graph skipped"
        );

        let a = summary.iter().find(|g| g.graph == "urn:shapes:a").unwrap();
        assert_eq!(a.node_count, 1, "PersonShape");
        assert_eq!(a.property_count, 1, "NameShape");
    }

    #[test]
    fn shapes_are_scoped_to_one_graph() {
        let store = TripleStore::in_memory().unwrap();
        seed(&store);
        let shapes = catalog_shapes(&store, "urn:shapes:a");
        let iris: Vec<&str> = shapes.iter().map(|s| s.shape.as_str()).collect();
        assert!(iris.contains(&"http://ex/PersonShape"));
        assert!(iris.contains(&"http://ex/NameShape"));
        assert!(
            !iris.contains(&"http://ex/MixedShape"),
            "shape from a different graph not included"
        );

        let person = shapes
            .iter()
            .find(|s| s.shape == "http://ex/PersonShape")
            .unwrap();
        assert_eq!(person.kind, "node");
        assert_eq!(person.target_classes, vec!["http://ex/Person".to_string()]);
        let name = shapes
            .iter()
            .find(|s| s.shape == "http://ex/NameShape")
            .unwrap();
        assert_eq!(name.kind, "property");
        assert_eq!(name.path.as_deref(), Some("http://ex/name"));

        // The mixed data graph is independently browsable.
        let mixed = catalog_shapes(&store, "urn:data:mixed");
        assert_eq!(mixed.len(), 1);
        assert_eq!(mixed[0].shape, "http://ex/MixedShape");
    }

    #[test]
    fn empty_store_has_no_summary() {
        let store = TripleStore::in_memory().unwrap();
        assert!(catalog_graph_summary(&store).is_empty());
    }

    #[test]
    fn import_copies_shape_with_blank_node_closure() {
        let store = TripleStore::in_memory().unwrap();
        store
            .graph_store_put(
                Some("urn:shapes:src"),
                r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
                   @prefix ex: <http://ex/> .
                   ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
                     sh:property [ sh:path ex:name ; sh:minCount 1 ] ."#,
                RdfFormat::Turtle,
            )
            .unwrap();

        let n = import_shapes_into(
            &store,
            "urn:shapes:dst",
            &[("urn:shapes:src".into(), "http://ex/PersonShape".into())],
        )
        .unwrap();
        assert_eq!(n, 1);

        let dst = catalog_shapes(&store, "urn:shapes:dst");
        assert_eq!(dst.len(), 1);
        assert_eq!(dst[0].shape, "http://ex/PersonShape");

        let q = r#"PREFIX sh: <http://www.w3.org/ns/shacl#>
                   PREFIX ex: <http://ex/>
                   ASK { GRAPH <urn:shapes:dst> { ex:PersonShape sh:property ?b . ?b sh:path ex:name ; sh:minCount 1 } }"#;
        assert!(
            matches!(store.query(q), Ok(QueryResults::Boolean(true))),
            "blank-node closure copied"
        );

        import_shapes_into(
            &store,
            "urn:shapes:dst",
            &[("urn:shapes:src".into(), "http://ex/PersonShape".into())],
        )
        .unwrap();
        assert_eq!(
            catalog_shapes(&store, "urn:shapes:dst").len(),
            1,
            "re-import is idempotent"
        );
    }
}
