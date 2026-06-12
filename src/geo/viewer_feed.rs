//! Viewer feed: per-element geometry + 3D-file references for map/3D visualisation.
//!
//! Resolves the standard linked-building-data layering — **BOT** topology
//! (`bot:containsElement` / `bot:hasSubElement`), **OMG/FOG** file references
//! (`omg:hasGeometry` → `fog:as…` URLs), and **GeoSPARQL** geometry
//! (`geo:hasGeometry` → `geo:asWKT` / `geo:asGML`) — into a flat list of
//! elements. Geometries are reprojected to EPSG:4326 (for `[lng, lat]` map
//! layers) via [`super::crs`], so the frontend never needs CRS math.
//! Vocabulary-specific detail (condition scores, inspection data, …) is
//! deliberately *not* flattened here: the client fetches an element's full RDF
//! on selection.

use crate::store::TripleStore;
use serde::Serialize;
use std::collections::BTreeMap;
use utoipa::ToSchema;

use super::crs::{reproject_wkt, Crs};
use super::datatypes::{extract_crs, extract_wkt};
use super::gml::{gml_srs_name, gml_to_wkt};

/// One element (or root object) in the viewer feed.
#[derive(Debug, Clone, Serialize, ToSchema, Default)]
pub struct ViewerElement {
    /// Element IRI.
    pub id: String,
    /// Best label (first `rdfs:label`).
    pub label: Option<String>,
    /// `rdf:type` IRIs.
    pub types: Vec<String>,
    /// Containing element/root IRI (`bot:containsElement`/`bot:hasSubElement`
    /// inverse); `None` for a root object.
    pub parent: Option<String>,
    /// IFC GlobalId — from a data property whose IRI ends in `ifcGuid`, or the
    /// fragment of the `fog:asIfc…` file URL.
    pub ifc_guid: Option<String>,
    /// glTF/GLB file URL (`fog:asGltf…`), the primary 3D-viewer source.
    pub gltf_url: Option<String>,
    /// IFC file URL (`fog:asIfc…`).
    pub ifc_url: Option<String>,
    /// All FOG file references as `(format, url)` — format is the FOG local name
    /// after `as` (e.g. `Gltf_v2.0-glb`, `Laz_v1.4`).
    pub files: Vec<(String, String)>,
    /// CRS URI of the stored geometry (default CRS84 when unprefixed).
    pub source_crs: Option<String>,
    /// Source up-axis of the element's 3D model(s) (`ots:modelUpAxis`, e.g.
    /// "Z" for Z-up STL exports) — viewers rotate into their own convention.
    pub up_axis: Option<String>,
    /// Geometry as WKT in EPSG:4326, `(x y) = (lon lat)` — feeds map layers.
    pub wkt4326: Option<String>,
}

const FOG_AS: &str = "https://w3id.org/fog#as";

/// Build the viewer feed over `data_graphs` (empty = default graph). With
/// `root`, only that object and its directly contained elements are returned.
pub fn build_viewer_feed(
    store: &TripleStore,
    data_graphs: &[String],
    root: Option<&str>,
) -> Vec<ViewerElement> {
    let from: String = data_graphs
        .iter()
        .map(|g| format!("FROM <{g}> "))
        .collect::<Vec<_>>()
        .join("");
    let root_filter = match root {
        Some(r) => format!("FILTER(?el = <{r}> || ?parent = <{r}>)"),
        None => String::new(),
    };
    // Roots (subjects of bot:containsElement that are nobody's child) appear as
    // rows with unbound ?parent; children come from the containment closure. The
    // third arm admits plain geo/omg subjects outside any BOT containment, also
    // as parentless roots — a dataset needs no BOT topology to feed the viewer.
    let query = format!(
        r#"
        PREFIX bot:  <https://w3id.org/bot#>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
        PREFIX omg:  <https://w3id.org/omg#>
        SELECT ?el ?parent ?label ?type ?wkt ?gml ?fp ?file ?guidp ?guid ?up
        {from}
        WHERE {{
            {{ ?parent (bot:containsElement|bot:hasSubElement) ?el . }}
            UNION
            {{ ?el bot:containsElement ?child .
               FILTER NOT EXISTS {{ ?up (bot:containsElement|bot:hasSubElement) ?el }} }}
            UNION
            {{ ?el (geo:hasGeometry|omg:hasGeometry) ?anyg .
               FILTER NOT EXISTS {{ ?up (bot:containsElement|bot:hasSubElement) ?el }} }}
            {root_filter}
            OPTIONAL {{ ?el rdfs:label ?label }}
            OPTIONAL {{ ?el a ?type }}
            OPTIONAL {{ ?el geo:hasGeometry ?g .
                        OPTIONAL {{ ?g geo:asWKT ?wkt }}
                        OPTIONAL {{ ?g geo:asGML ?gml }} }}
            OPTIONAL {{ ?el omg:hasGeometry ?og . ?og ?fp ?file .
                        FILTER(STRSTARTS(STR(?fp), "{FOG_AS}"))
                        OPTIONAL {{ ?og <https://opentriplestore.org/ns#modelUpAxis> ?up }} }}
            OPTIONAL {{ ?el ?guidp ?guid . FILTER(STRENDS(STR(?guidp), "ifcGuid")) }}
        }}
        "#
    );

    let mut elements: BTreeMap<String, ViewerElement> = BTreeMap::new();
    let Ok(oxigraph::sparql::QueryResults::Solutions(solutions)) = store.query(&query) else {
        return Vec::new();
    };
    for sol in solutions.flatten() {
        let Some(id) = sol.get("el").map(term_str) else {
            continue;
        };
        let entry = elements.entry(id.clone()).or_insert_with(|| ViewerElement {
            id,
            ..Default::default()
        });
        if entry.parent.is_none() {
            entry.parent = sol.get("parent").map(term_str);
        }
        if entry.label.is_none() {
            entry.label = sol.get("label").map(term_value);
        }
        if let Some(t) = sol.get("type").map(term_str) {
            if !entry.types.contains(&t) {
                entry.types.push(t);
            }
        }
        if entry.ifc_guid.is_none() {
            entry.ifc_guid = sol.get("guid").map(term_value);
        }
        if entry.up_axis.is_none() {
            entry.up_axis = sol.get("up").map(term_value);
        }
        if let (Some(fp), Some(file)) = (sol.get("fp").map(term_str), sol.get("file")) {
            let format = fp.trim_start_matches(FOG_AS).to_string();
            let url = term_value(file);
            if !entry.files.iter().any(|(f, _)| f == &format) {
                if format.starts_with("Gltf") && entry.gltf_url.is_none() {
                    entry.gltf_url = Some(url.clone());
                }
                if format.starts_with("Ifc") && entry.ifc_url.is_none() {
                    entry.ifc_url = Some(url.clone());
                }
                entry.files.push((format, url));
            }
        }
        if entry.wkt4326.is_none() {
            if let Some(wkt_lit) = sol.get("wkt").map(term_value) {
                apply_wkt(entry, &wkt_lit);
            } else if let Some(gml_lit) = sol.get("gml").map(term_value) {
                apply_gml(entry, &gml_lit);
            }
        }
    }

    // GUID fallback: the fragment of the IFC file URL.
    for el in elements.values_mut() {
        if el.ifc_guid.is_none() {
            el.ifc_guid = el
                .ifc_url
                .as_deref()
                .and_then(|u| u.split_once('#'))
                .map(|(_, frag)| frag.to_string());
        }
    }
    elements.into_values().collect()
}

/// Fill the reprojected geometry fields from a (possibly CRS-prefixed) WKT literal value.
fn apply_wkt(el: &mut ViewerElement, literal_value: &str) {
    let source_uri = extract_crs(literal_value);
    // An explicit but unsupported CRS must not be mislabelled as WGS84 — projected
    // metre coordinates would reach the map as lng/lat. Skip the geometry instead.
    // No prefix keeps the GeoSPARQL default (CRS84).
    let source = match source_uri {
        Some(uri) => match Crs::from_uri(uri) {
            Some(crs) => crs,
            None => return,
        },
        None => Crs::Wgs84,
    };
    el.source_crs = Some(
        source_uri
            .map(str::to_string)
            .unwrap_or_else(|| Crs::Wgs84.to_uri().to_string()),
    );
    el.wkt4326 = reproject_wkt(extract_wkt(literal_value), source, Crs::Wgs84);
}

/// Fill the reprojected geometry fields from a GML literal value (srsName-aware).
fn apply_gml(el: &mut ViewerElement, gml_value: &str) {
    let Some(wkt_body) = gml_to_wkt(gml_value) else {
        return;
    };
    let source_uri = gml_srs_name(gml_value);
    // Same contract as apply_wkt: unsupported explicit srsName → skip, no srsName → CRS84.
    let source = match source_uri.as_deref() {
        Some(uri) => match Crs::from_uri(uri) {
            Some(crs) => crs,
            None => return,
        },
        None => Crs::Wgs84,
    };
    el.source_crs = Some(source_uri.unwrap_or_else(|| Crs::Wgs84.to_uri().to_string()));
    el.wkt4326 = reproject_wkt(&wkt_body, source, Crs::Wgs84);
}

fn term_str(t: &oxigraph::model::Term) -> String {
    match t {
        oxigraph::model::Term::NamedNode(nn) => nn.as_str().to_string(),
        oxigraph::model::Term::BlankNode(b) => format!("_:{}", b.as_str()),
        oxigraph::model::Term::Literal(l) => l.value().to_string(),
        other => other.to_string(),
    }
}

fn term_value(t: &oxigraph::model::Term) -> String {
    match t {
        oxigraph::model::Term::Literal(l) => l.value().to_string(),
        other => term_str(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::RdfFormat;

    const DATA: &str = r#"
        @prefix bot:  <https://w3id.org/bot#> .
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
        @prefix geo:  <http://www.opengis.net/ont/geosparql#> .
        @prefix omg:  <https://w3id.org/omg#> .
        @prefix fog:  <https://w3id.org/fog#> .
        @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
        @prefix ex:   <http://example.org/> .
        ex:Bridge a ex:Brug ; rdfs:label "Bridge" ;
            bot:containsElement ex:Arch ;
            geo:hasGeometry [ geo:asWKT "<http://www.opengis.net/def/crs/EPSG/0/28992> LINESTRING(187320 428330, 187610 428690)"^^geo:wktLiteral ] .
        ex:Arch a ex:Boog ; rdfs:label "Arch" ;
            ex:ifcGuid "1aB2cD3eF4gH5iJ6kL7mNo" ;
            geo:hasGeometry [ geo:asWKT "<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(187420 428470)"^^geo:wktLiteral ] ;
            omg:hasGeometry [ a omg:Geometry ;
                fog:asGltf_v2.0-glb "https://files.example/arch.glb"^^xsd:anyURI ;
                fog:asIfc_v4.0 "https://files.example/bridge.ifc#1aB2cD3eF4gH5iJ6kL7mNo"^^xsd:anyURI ;
                <https://opentriplestore.org/ns#modelUpAxis> "Z" ] .
    "#;

    #[test]
    fn feed_resolves_topology_files_and_reprojects() {
        let store = TripleStore::in_memory().unwrap();
        store.load_str(DATA, RdfFormat::Turtle, None).unwrap();
        let feed = build_viewer_feed(&store, &[], None);
        assert_eq!(feed.len(), 2, "root + one element: {feed:?}");

        let arch = feed.iter().find(|e| e.id.ends_with("Arch")).unwrap();
        assert_eq!(arch.parent.as_deref(), Some("http://example.org/Bridge"));
        assert_eq!(
            arch.gltf_url.as_deref(),
            Some("https://files.example/arch.glb")
        );
        assert_eq!(arch.ifc_guid.as_deref(), Some("1aB2cD3eF4gH5iJ6kL7mNo"));
        // The model-orientation annotation flows through to the client.
        assert_eq!(arch.up_axis.as_deref(), Some("Z"));
        // RD point reprojected to lon/lat near Nijmegen.
        let wkt = arch.wkt4326.as_deref().unwrap();
        let nums: Vec<f64> = wkt
            .trim_start_matches("POINT(")
            .trim_end_matches(')')
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        assert!((nums[0] - 5.86).abs() < 0.05, "lon: {wkt}");
        assert!((nums[1] - 51.85).abs() < 0.05, "lat: {wkt}");

        let bridge = feed.iter().find(|e| e.id.ends_with("Bridge")).unwrap();
        assert!(bridge.parent.is_none(), "root has no parent");
        assert!(
            bridge.wkt4326.as_deref().unwrap().starts_with("LINESTRING"),
            "root keeps its linestring"
        );
    }

    #[test]
    fn unsupported_crs_yields_no_wkt4326() {
        let data = r#"
            @prefix geo: <http://www.opengis.net/ont/geosparql#> .
            @prefix ex:  <http://example.org/> .
            ex:thing geo:hasGeometry [ geo:asWKT "<http://www.opengis.net/def/crs/EPSG/0/25832> POINT(687000 5338000)"^^geo:wktLiteral ] .
        "#;
        let store = TripleStore::in_memory().unwrap();
        store.load_str(data, RdfFormat::Turtle, None).unwrap();
        let feed = build_viewer_feed(&store, &[], None);
        assert_eq!(feed.len(), 1, "{feed:?}");
        assert!(
            feed[0].wkt4326.is_none(),
            "UTM metres must not be emitted as lng/lat: {:?}",
            feed[0].wkt4326
        );
    }

    #[test]
    fn plain_geosparql_without_bot_appears_as_root() {
        let data = r#"
            @prefix geo: <http://www.opengis.net/ont/geosparql#> .
            @prefix ex:  <http://example.org/> .
            ex:thing geo:hasGeometry [ geo:asWKT "POINT(4 52)"^^geo:wktLiteral ] .
        "#;
        let store = TripleStore::in_memory().unwrap();
        store.load_str(data, RdfFormat::Turtle, None).unwrap();
        let feed = build_viewer_feed(&store, &[], None);
        assert_eq!(feed.len(), 1, "{feed:?}");
        assert_eq!(feed[0].id, "http://example.org/thing");
        assert!(feed[0].parent.is_none(), "no BOT containment → root");
        let wkt = feed[0].wkt4326.as_deref().unwrap();
        let nums: Vec<f64> = wkt
            .trim_start_matches("POINT(")
            .trim_end_matches(')')
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        assert_eq!(nums, [4.0, 52.0], "unprefixed WKT stays WGS84: {wkt}");
    }
}
