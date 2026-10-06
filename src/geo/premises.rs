//! Reasoning premises for the GeoSPARQL RDFS Entailment Extension
//! (OGC 11-052r4 Req 25–27, 22-047r1 Req 47–49).
//!
//! Entailment over GeoSPARQL data needs the GeoSPARQL ontology (so a thing
//! with `geo:hasGeometry` is a `geo:Feature`, and `geo:asWKT` is a
//! `geo:hasSerialization`) and the geometry class hierarchies of Simple
//! Features (`sf:Polygon ⊑ sf:Surface ⊑ sf:Geometry ⊑ geo:Geometry`) and GML
//! (`gml:Polygon ⊑ gml:AbstractSurface ⊑ … ⊑ geo:Geometry`). A dataset's
//! conformance layer ([`crate::conformance`]) reasons over its own graphs and
//! the one model it conforms to, so these were never premises unless a
//! dataset happened to conform to the GeoSPARQL model.
//!
//! [`rdfs_premises`] closes that gap: when any graph of a dataset's layer uses
//! a GeoSPARQL term, the seeded copies of the three vocabularies (registry ids
//! `geosparql`, `sf`, `gml-geometries`, each at its latest published version)
//! join the layer. "Uses a GeoSPARQL term" means, in one of the graphs, an
//! index lookup finds
//!
//! * a triple whose predicate is a property of the GeoSPARQL ontology
//!   (`geo:hasGeometry`, `geo:asWKT`, `geo:sfWithin`, …);
//! * `?x rdf:type C` with `C` a class of the GeoSPARQL ontology or of the SF
//!   or GML hierarchy;
//! * `?x rdfs:subClassOf C` or `?x rdfs:subPropertyOf P` with such a `C` or
//!   `P` (a domain model that specialises GeoSPARQL).
//!
//! The term lists come from the bundled files themselves, parsed once. A
//! literal typed `geo:wktLiteral` under a property of the dataset's own is not
//! looked for (that would need a scan). The registry copies are public
//! reference models; nothing is added when the seeder is off
//! (`SEED_STANDARD_VOCABS=false`) and they do not exist.

use std::sync::OnceLock;

use oxigraph::model::{GraphNameRef, NamedNode, NamedNodeRef, Term};

use crate::data_models::vocab_files as vf;
use crate::store::TripleStore;

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_SUB_CLASS_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subClassOf";
const RDFS_SUB_PROPERTY_OF: &str = "http://www.w3.org/2000/01/rdf-schema#subPropertyOf";

/// The registry ids of the premise vocabularies, in the order they are added.
pub const PREMISE_VOCABULARIES: [&str; 3] = ["geosparql", "sf", "gml-geometries"];

/// The terms whose use marks a graph as GeoSPARQL data.
struct Terms {
    properties: Vec<NamedNode>,
    classes: Vec<NamedNode>,
}

fn terms() -> &'static Terms {
    static TERMS: OnceLock<Terms> = OnceLock::new();
    TERMS.get_or_init(|| {
        const PROPERTY_TYPES: [&str; 4] = [
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#Property",
            "http://www.w3.org/2002/07/owl#ObjectProperty",
            "http://www.w3.org/2002/07/owl#DatatypeProperty",
            "http://www.w3.org/2002/07/owl#FunctionalProperty",
        ];
        const CLASS_TYPES: [&str; 2] = [
            "http://www.w3.org/2000/01/rdf-schema#Class",
            "http://www.w3.org/2002/07/owl#Class",
        ];
        let mut properties = Vec::new();
        let mut classes = Vec::new();
        for f in [&vf::GEOSPARQL, &vf::SF, &vf::GML_GEOMETRIES] {
            let Ok(quads) =
                crate::data_models::upload::parse_rdf(f.ttl.as_bytes(), "text/turtle", f.path)
            else {
                continue;
            };
            for q in quads {
                let (oxigraph::model::NamedOrBlankNode::NamedNode(s), Term::NamedNode(o)) =
                    (&q.subject, &q.object)
                else {
                    continue;
                };
                if q.predicate.as_str() != RDF_TYPE {
                    continue;
                }
                let list = if PROPERTY_TYPES.contains(&o.as_str()) {
                    &mut properties
                } else if CLASS_TYPES.contains(&o.as_str()) {
                    &mut classes
                } else {
                    continue;
                };
                if !list.contains(s) {
                    list.push(s.clone());
                }
            }
        }
        Terms {
            properties,
            classes,
        }
    })
}

/// Whether `graph` holds a triple that uses a GeoSPARQL term (module docs).
fn uses_geosparql(store: &TripleStore, graph: &str) -> bool {
    let Ok(g) = NamedNodeRef::new(graph) else {
        return false;
    };
    let g = GraphNameRef::NamedNode(g);
    let st = store.store();
    let any = |p: NamedNodeRef<'_>, o: Option<NamedNodeRef<'_>>| {
        st.quads_for_pattern(None, Some(p), o.map(Into::into), Some(g))
            .next()
            .is_some()
    };
    let t = terms();
    let rdf_type = NamedNodeRef::new_unchecked(RDF_TYPE);
    let sub_class = NamedNodeRef::new_unchecked(RDFS_SUB_CLASS_OF);
    let sub_property = NamedNodeRef::new_unchecked(RDFS_SUB_PROPERTY_OF);
    t.properties.iter().any(|p| any(p.as_ref(), None))
        || t.classes
            .iter()
            .any(|c| any(rdf_type, Some(c.as_ref())) || any(sub_class, Some(c.as_ref())))
        || t.properties
            .iter()
            .any(|p| any(sub_property, Some(p.as_ref())))
}

/// The graphs of the latest published version of each premise vocabulary,
/// when one of `sources` uses a GeoSPARQL term; otherwise nothing. Graphs
/// already in `sources` are not repeated.
pub fn rdfs_premises(store: &TripleStore, base_url: &str, sources: &[String]) -> Vec<String> {
    if !sources.iter().any(|g| uses_geosparql(store, g)) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for id in PREMISE_VOCABULARIES {
        let Some(version) = crate::data_models::registry::get_data_model(store, base_url, id)
            .and_then(|m| m.latest_published)
        else {
            continue;
        };
        let Some(v) = crate::data_models::registry::get_version(store, base_url, id, &version)
        else {
            continue;
        };
        for g in std::iter::once(v.graph_iri).chain(v.sub_graphs) {
            if !sources.contains(&g) && !out.contains(&g) {
                out.push(g);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_term_lists_come_from_the_bundled_files() {
        let t = terms();
        let has = |list: &[NamedNode], iri: &str| list.iter().any(|n| n.as_str() == iri);
        let geo = "http://www.opengis.net/ont/geosparql#";
        for p in [
            "hasGeometry",
            "hasDefaultGeometry",
            "asWKT",
            "asGML",
            "sfWithin",
            "rcc8eq",
        ] {
            assert!(has(&t.properties, &format!("{geo}{p}")), "{p}");
        }
        assert!(has(&t.classes, &format!("{geo}Feature")));
        assert!(has(&t.classes, "http://www.opengis.net/ont/sf#Polygon"));
        assert!(has(&t.classes, "http://www.opengis.net/ont/gml#Polygon"));
    }

    /// Every class of the SF and GML hierarchies reaches `geo:Geometry`
    /// through `rdfs:subClassOf`, so a typo in either file cannot leave a
    /// geometry type outside the hierarchy.
    #[test]
    fn every_sf_and_gml_class_is_a_geo_geometry() {
        use std::collections::HashMap;
        let geometry = "http://www.opengis.net/ont/geosparql#Geometry";
        for f in [&vf::SF, &vf::GML_GEOMETRIES] {
            let quads =
                crate::data_models::upload::parse_rdf(f.ttl.as_bytes(), "text/turtle", f.path)
                    .unwrap();
            let mut supers: HashMap<String, Vec<String>> = HashMap::new();
            let mut classes = Vec::new();
            for q in &quads {
                let oxigraph::model::NamedOrBlankNode::NamedNode(s) = &q.subject else {
                    continue;
                };
                match (q.predicate.as_str(), &q.object) {
                    (RDF_TYPE, Term::NamedNode(o))
                        if o.as_str() == "http://www.w3.org/2002/07/owl#Class" =>
                    {
                        classes.push(s.as_str().to_string())
                    }
                    (RDFS_SUB_CLASS_OF, Term::NamedNode(o)) => supers
                        .entry(s.as_str().to_string())
                        .or_default()
                        .push(o.as_str().to_string()),
                    _ => {}
                }
            }
            assert!(classes.len() >= 18, "{}: {} classes", f.path, classes.len());
            for c in &classes {
                let mut seen = vec![c.clone()];
                let mut i = 0;
                while i < seen.len() && !seen.iter().any(|x| x == geometry) {
                    for up in supers.get(&seen[i]).into_iter().flatten() {
                        if !seen.contains(up) {
                            seen.push(up.clone());
                        }
                    }
                    i += 1;
                }
                assert!(
                    seen.iter().any(|x| x == geometry),
                    "{}: {c} does not reach geo:Geometry",
                    f.path
                );
            }
        }
    }

    #[test]
    fn a_graph_counts_as_geosparql_data_by_its_terms() {
        let store = TripleStore::in_memory().unwrap();
        store
            .update(
                "INSERT DATA { \
                 GRAPH <http://example.org/geo> { <http://example.org/f> \
                   <http://www.opengis.net/ont/geosparql#hasGeometry> <http://example.org/g> } \
                 GRAPH <http://example.org/typed> { <http://example.org/p> a \
                   <http://www.opengis.net/ont/sf#Polygon> } \
                 GRAPH <http://example.org/model> { <http://example.org/Bridge> \
                   <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
                   <http://www.opengis.net/ont/geosparql#Feature> } \
                 GRAPH <http://example.org/plain> { <http://example.org/a> \
                   <http://example.org/p> <http://example.org/b> } }",
            )
            .unwrap();
        assert!(uses_geosparql(&store, "http://example.org/geo"));
        assert!(uses_geosparql(&store, "http://example.org/typed"));
        assert!(uses_geosparql(&store, "http://example.org/model"));
        assert!(!uses_geosparql(&store, "http://example.org/plain"));
        // Without seeded vocabularies there is nothing to add.
        assert!(rdfs_premises(
            &store,
            "http://localhost:7878",
            &["http://example.org/geo".to_string()]
        )
        .is_empty());
    }
}
