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
    /// Real-world largest extent of the model in metres (`ots:modelSizeMeters`) —
    /// lets the map scale a unit-less STL (a landmark) to true size instead of
    /// guessing. `None` when the model's own units are already trustworthy.
    pub size_meters: Option<f64>,
    /// Geometry as WKT in EPSG:4326, `(x y) = (lon lat)` — feeds map layers.
    pub wkt4326: Option<String>,
}

const FOG_AS: &str = "https://w3id.org/fog#as";

/// A `tiles3d-*` graph holds CityJSON lifted to volumetric WKT-Z **solely** to
/// feed the 3D-Tiles pipeline (`/3dtiles`, which reads every registered graph).
/// The 2D map, the OGC API – Features endpoint and the DCAT capability probe all
/// render the same blocks from their client-side CityJSON file-links instead, so
/// every feed-style reader skips these graphs (the same way they skip `/ifcowl`)
/// to avoid doubling the geometry and swamping the element list. The 3D-Tiles
/// route deliberately does NOT apply this filter.
pub(crate) fn is_tiles3d_graph(graph: &str) -> bool {
    graph.contains("/tiles3d-")
}

/// Lightweight geo capability summary for a dataset/scope — drives the UI gating
/// (show a 2D map when there are coordinates; show the 3D viewer only when there
/// is 3D data). Computed with cheap `ASK`/`COUNT` queries rather than building
/// the full feed, so it is safe to call per-dataset on list pages.
#[derive(Debug, Clone, Serialize, ToSchema, Default)]
pub struct GeoStats {
    /// Any feature carries a `geo:asWKT` / `geo:asGML` geometry (mappable in 2D).
    pub has_coordinates: bool,
    /// Any feature links a loadable 3D model file (glTF/STL/CityJSON/CityGML/IFC).
    pub has_models: bool,
    /// Any stored WKT geometry is volumetric (`POLYHEDRALSURFACE`/`TIN`/`SOLID`/Z).
    pub has_3d_geometry: bool,
    /// Convenience: a 3D viewer is worthwhile (models or volumetric geometry).
    pub has_3d: bool,
    /// Number of distinct geometry-bearing features.
    pub element_count: usize,
}

/// Compute the [`GeoStats`] for `data_graphs` (empty = default graph).
pub fn dataset_geo_stats(store: &TripleStore, data_graphs: &[String]) -> GeoStats {
    let from: String = data_graphs
        .iter()
        .map(|g| format!("FROM <{g}> "))
        .collect::<Vec<_>>()
        .join("");

    let ask = |where_clause: &str| -> bool {
        let q = format!(
            "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
             PREFIX omg: <https://w3id.org/omg#>\n\
             ASK {from} WHERE {{ {where_clause} }}"
        );
        matches!(
            store.query(&q),
            Ok(oxigraph::sparql::QueryResults::Boolean(true))
        )
    };

    // Cheap early-out: every flag below requires a `geo:hasGeometry` or
    // `omg:hasGeometry` link (the same superset the element count walks), so a
    // dataset with neither — the common case on a catalog/list page — is fully
    // described by the default (all-false) stats after a single ASK, instead of
    // running three more scans and a COUNT(DISTINCT) that can only return zero.
    if !ask("?el (geo:hasGeometry|omg:hasGeometry) ?x") {
        return GeoStats::default();
    }

    let has_coordinates =
        ask("?s geo:hasGeometry ?g . { ?g geo:asWKT ?w } UNION { ?g geo:asGML ?w }");
    let has_models = ask("?el omg:hasGeometry ?g . ?g ?p ?f . \
         FILTER(STRSTARTS(STR(?p), \"https://w3id.org/fog#as\")) \
         FILTER(REGEX(STR(?p), \"Gltf|Stl|Cityjson|Citygml|Ifc|Obj\", \"i\"))");
    let has_3d_geometry = ask(
        "?s geo:hasGeometry/geo:asWKT ?w . BIND(UCASE(STR(?w)) AS ?u) \
         FILTER(CONTAINS(?u, \"POLYHEDRALSURFACE\") || CONTAINS(?u, \"TIN Z\") \
             || CONTAINS(?u, \"TIN (\") || CONTAINS(?u, \"SOLID\") \
             || CONTAINS(?u, \" Z (\") || CONTAINS(?u, \" Z(\"))",
    );

    let element_count = {
        let q = format!(
            "PREFIX geo: <http://www.opengis.net/ont/geosparql#>\n\
             PREFIX omg: <https://w3id.org/omg#>\n\
             SELECT (COUNT(DISTINCT ?el) AS ?c) {from} \
             WHERE {{ ?el (geo:hasGeometry|omg:hasGeometry) ?x }}"
        );
        match store.query(&q) {
            Ok(oxigraph::sparql::QueryResults::Solutions(mut sols)) => sols
                .next()
                .and_then(|s| s.ok())
                .and_then(|s| s.get("c").map(term_value))
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0),
            _ => 0,
        }
    };

    GeoStats {
        has_coordinates,
        has_models,
        has_3d_geometry,
        has_3d: has_models || has_3d_geometry,
        element_count,
    }
}

/// Build the viewer feed over `data_graphs` (empty = default graph). With
/// `root`, only that object and its directly contained elements are returned.
pub fn build_viewer_feed(
    store: &TripleStore,
    data_graphs: &[String],
    root: Option<&str>,
) -> Vec<ViewerElement> {
    build_viewer_feed_opts(store, data_graphs, root, false)
}

/// As [`build_viewer_feed`], but with `located_only` to fetch just the elements
/// that carry actual coordinates (`geo:asWKT`/`geo:asGML`) plus their model
/// references — the subset the 2D map renders. On a big BIM dataset the IFC
/// sub-elements (walls/beams/…) inherit their location and number in the
/// thousands; they matter only to the structure tree, not the map. Skipping the
/// BOT containment closure for the map turns a multi-second whole-building scan
/// into a sub-second query, so the map paints immediately while the full feed
/// (for the tree) streams in behind it.
pub fn build_viewer_feed_opts(
    store: &TripleStore,
    data_graphs: &[String],
    root: Option<&str>,
    located_only: bool,
) -> Vec<ViewerElement> {
    let from: String = data_graphs
        .iter()
        .map(|g| format!("FROM <{g}> "))
        .collect::<Vec<_>>()
        .join("");
    // Selection of candidate ?el (+ optional ?parent). Located mode anchors on a
    // real coordinate; full mode walks the BOT hierarchy.
    //
    // Containment follows the BOT hierarchy — bot:containsElement / hasSubElement
    // (used by the IFC importer) plus bot:hasStorey / hasSpace / hasElement (the
    // Site→Building→Storey→Space→Element decomposition). Roots (containment
    // subjects that are nobody's child) appear as rows with unbound ?parent;
    // children come from the closure. The third arm admits plain geo/omg subjects
    // outside any BOT topology, also as parentless roots — a dataset needs no BOT
    // topology to feed the viewer.
    let selection = if located_only {
        // Only coordinate-bearing features; ?parent stays unbound (the map's
        // located elements are roots/anchors — the tree resolves parents).
        "?el geo:hasGeometry ?gg . { ?gg geo:asWKT ?w0 } UNION { ?gg geo:asGML ?g0 }".to_string()
    } else {
        let root_filter = match root {
            Some(r) => format!("FILTER(?el = <{r}> || ?parent = <{r}>)"),
            None => String::new(),
        };
        format!(
            r#"{{ ?parent (bot:containsElement|bot:hasSubElement|bot:hasStorey|bot:hasSpace|bot:hasElement) ?el . }}
            UNION
            {{ ?el (bot:containsElement|bot:hasStorey|bot:hasSpace|bot:hasElement) ?child .
               FILTER NOT EXISTS {{ ?up (bot:containsElement|bot:hasSubElement|bot:hasStorey|bot:hasSpace|bot:hasElement) ?el }} }}
            UNION
            {{ ?el (geo:hasGeometry|omg:hasGeometry) ?anyg .
               FILTER NOT EXISTS {{ ?up (bot:containsElement|bot:hasSubElement|bot:hasStorey|bot:hasSpace|bot:hasElement) ?el }} }}
            {root_filter}"#
        )
    };
    // ifcGuid lives on a known predicate (props#ifcGuid); binding it directly
    // avoids the unbounded `?el ?p ?o` predicate scan a STRENDS filter forces.
    let query = format!(
        r#"
        PREFIX bot:  <https://w3id.org/bot#>
        PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
        PREFIX omg:  <https://w3id.org/omg#>
        SELECT ?el ?parent ?label ?type ?wkt ?gml ?fp ?file ?guid ?up ?msize
        {from}
        WHERE {{
            {selection}
            OPTIONAL {{ ?el rdfs:label ?label }}
            OPTIONAL {{ ?el a ?type }}
            OPTIONAL {{ ?el geo:hasGeometry ?g .
                        OPTIONAL {{ ?g geo:asWKT ?wkt }}
                        OPTIONAL {{ ?g geo:asGML ?gml }} }}
            OPTIONAL {{ ?el omg:hasGeometry ?og . ?og ?fp ?file .
                        FILTER(STRSTARTS(STR(?fp), "{FOG_AS}"))
                        OPTIONAL {{ ?og <https://opentriplestore.org/ns#modelUpAxis> ?up }}
                        OPTIONAL {{ ?og <https://opentriplestore.org/ns#modelSizeMeters> ?msize }} }}
            OPTIONAL {{ ?el <https://w3id.org/props#ifcGuid> ?guid }}
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
        if entry.size_meters.is_none() {
            entry.size_meters = sol
                .get("msize")
                .map(term_value)
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|m| m.is_finite() && *m > 0.0);
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
    fn geo_stats_detects_2d_models_and_3d() {
        let store = TripleStore::in_memory().unwrap();
        store.load_str(DATA, RdfFormat::Turtle, None).unwrap();
        let s = dataset_geo_stats(&store, &[]);
        assert!(s.has_coordinates, "DATA has geo:asWKT");
        assert!(s.has_models, "DATA has fog:asGltf + fog:asIfc");
        assert!(s.has_3d, "models ⇒ 3D viewer worthwhile");
        assert!(!s.has_3d_geometry, "DATA's WKT is 2D (LINESTRING/POINT)");
        assert_eq!(s.element_count, 2, "Bridge + Arch");

        // A volumetric solid flips has_3d_geometry.
        let store3d = TripleStore::in_memory().unwrap();
        store3d
            .load_str(
                "@prefix geo: <http://www.opengis.net/ont/geosparql#> .\n\
                 @prefix ex: <http://example.org/> .\n\
                 ex:b geo:hasGeometry [ geo:asWKT \"POLYHEDRALSURFACE Z (((0 0 0,1 0 0,1 1 0,0 0 0)))\"^^geo:wktLiteral ] .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();
        let s3 = dataset_geo_stats(&store3d, &[]);
        assert!(s3.has_coordinates && s3.has_3d_geometry && s3.has_3d);
        assert!(!s3.has_models);
    }

    #[test]
    fn geo_stats_early_out_on_non_geo_dataset() {
        // A dataset with no geo:/omg: geometry link (the common catalog/list case)
        // must report empty stats via the single-ASK early-out, not four scans.
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                "@prefix ex: <http://example.org/> .\n\
                 ex:a ex:name \"Alice\" ; ex:knows ex:b .\n\
                 ex:b ex:name \"Bob\" .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();
        let s = dataset_geo_stats(&store, &[]);
        assert!(!s.has_coordinates && !s.has_models && !s.has_3d_geometry && !s.has_3d);
        assert_eq!(s.element_count, 0);
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

    #[test]
    fn located_only_drops_uncoordinated_subelements() {
        // A multi-level BIM tree: one located Site → Building → Storey → many
        // walls, none of which carry coordinates of their own (they inherit the
        // Site's location). This is the shape that makes the full feed slow.
        let mut data = String::from(
            "@prefix bot:  <https://w3id.org/bot#> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             @prefix geo:  <http://www.opengis.net/ont/geosparql#> .\n\
             @prefix ex:   <http://example.org/> .\n\
             ex:Site geo:hasGeometry [ geo:asWKT \"POINT(4 52)\"^^geo:wktLiteral ] ;\n\
                 bot:containsElement ex:Building .\n\
             ex:Building bot:hasStorey ex:Storey .\n\
             ex:Storey a bot:Storey ;\n",
        );
        for i in 0..50 {
            data.push_str(&format!("    bot:hasElement ex:Wall{i} ;\n"));
        }
        data.push_str("    rdfs:label \"Storey\" .\n");
        for i in 0..50 {
            data.push_str(&format!("ex:Wall{i} rdfs:label \"Wall {i}\" .\n"));
        }
        let store = TripleStore::in_memory().unwrap();
        store.load_str(&data, RdfFormat::Turtle, None).unwrap();

        // Full feed walks the whole BOT closure → Site + Building + Storey + 50 walls.
        let full = build_viewer_feed(&store, &[], None);
        assert_eq!(
            full.len(),
            53,
            "full feed includes every sub-element: {}",
            full.len()
        );

        // Located feed (the map's fast path) returns ONLY the coordinate-bearing
        // Site — its size does not grow with the tree, however deep it gets. This
        // is the contract that keeps the map paint fast on big buildings.
        let located = build_viewer_feed_opts(&store, &[], None, true);
        assert_eq!(located.len(), 1, "located feed stays tiny: {located:?}");
        assert_eq!(located[0].id, "http://example.org/Site");
        assert!(located[0].wkt4326.is_some(), "Site keeps its coordinate");
    }
}
