//! CityJSON (3D BAG) → RDF ingestion (spec §4.1).
//!
//! CityJSON is the JSON encoding of the CityGML data model: a `transform`
//! (scale + translate) that dequantises a shared integer `vertices` array, and a
//! flat `CityObjects` map whose geometries reference vertices by index, carrying
//! per-surface semantics across one or more LoDs.
//!
//! This converter mints a stable IRI per CityObject (spec §4.3), emits BOT
//! parent/child topology and the object's attributes, and externalises geometry
//! per LoD into **both** a `geo:asWKT` `POLYHEDRALSURFACE Z` (so GeoSPARQL 1.1
//! clients read what they can) and a loss-free `ots:cityjsonGeometryLiteral`
//! (the original CityJSON geometry object, keeping LoD + semantic surface tags),
//! with PROV-O lineage back to the source. The result is the same geometry
//! surface BAG buildings and BIM elements share.

use axum::body::Bytes;
use axum::extract::{Extension, Multipart, Path, Query, State};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::Value;

use crate::auth::middleware::AuthenticatedUser;
use crate::server::error::AppError;
use crate::server::AppState;

/// `bag:` — the 3D BAG vocabulary namespace (spec §4.1).
const BAG_NS: &str = "https://data.3dbag.nl/def/";
/// `ots:` — Open Triplestore definitions namespace.
const OTS_NS: &str = "https://open-triplestore.org/def/";
const GEO_NS: &str = "http://www.opengis.net/ont/geosparql#";
const BOT_NS: &str = "https://w3id.org/bot#";
const PROV_NS: &str = "http://www.w3.org/ns/prov#";
const DCT_NS: &str = "http://purl.org/dc/terms/";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDFS_LABEL: &str = "http://www.w3.org/2000/01/rdf-schema#label";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const WKT_LITERAL: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";
const CITYJSON_LITERAL: &str = "https://open-triplestore.org/def/cityjsonGeometryLiteral";

/// Options for a CityJSON conversion.
pub struct CityJsonOptions {
    /// IRI prefix for minted resources; feature IRIs become `{inst_base}bag/{id}`.
    pub inst_base: String,
    /// PROV source IRI (the stored asset URL or original download), if known.
    pub source_url: Option<String>,
    /// ISO-8601 conversion timestamp for `prov:generatedAtTime` (optional).
    pub generated_at: Option<String>,
    /// Skip flat (zero vertical extent) geometry, e.g. 3DBAG LoD0 footprints, so
    /// only the volumetric massing (LoD2.2 Solids) is emitted. Used when lifting
    /// CityJSON purely to feed the 3D engine / 3D-Tiles pipeline, where flat
    /// footprints would otherwise mesh as redundant ground-level polygons that
    /// z-fight the solids. Default `false` — the importer keeps every LoD.
    pub volumetric_only: bool,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct CityJsonStats {
    pub objects: usize,
    pub geometries: usize,
    pub triples: usize,
    /// CRS detected from `metadata.referenceSystem`, if any.
    pub crs: Option<String>,
}

/// What a CityJSON import produced.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CityJsonImportOutcome {
    pub asset_id: Option<String>,
    pub asset_url: Option<String>,
    pub graph: String,
    pub stats: CityJsonStats,
}

// ─── Pure converter ───────────────────────────────────────────────────────────

/// Convert a parsed CityJSON document into N-Triples plus stats. Pure and
/// deterministic (no I/O), so it is unit-tested directly and reused by both the
/// streaming importer and the `?preview=` dry-run.
pub fn convert_cityjson(
    doc: &Value,
    opts: &CityJsonOptions,
) -> Result<(String, CityJsonStats), String> {
    if doc.get("type").and_then(Value::as_str) != Some("CityJSON") {
        return Err("not a CityJSON document".to_string());
    }

    // Dequantise the shared vertex array via the transform.
    let (scale, translate) = transform_of(doc);
    let vertices: Vec<[f64; 3]> = doc
        .get("vertices")
        .and_then(Value::as_array)
        .map(|vs| {
            vs.iter()
                .map(|v| {
                    let a = v.as_array();
                    let g = |i: usize| {
                        a.and_then(|x| x.get(i))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0)
                    };
                    [
                        g(0) * scale[0] + translate[0],
                        g(1) * scale[1] + translate[1],
                        g(2) * scale[2] + translate[2],
                    ]
                })
                .collect()
        })
        .unwrap_or_default();

    let crs_uri = doc
        .get("metadata")
        .and_then(|m| m.get("referenceSystem"))
        .and_then(Value::as_str)
        .map(normalise_crs_uri);

    let objects = doc
        .get("CityObjects")
        .and_then(Value::as_object)
        .ok_or("CityJSON has no CityObjects")?;

    let inst = opts.inst_base.trim_end_matches('/');
    let mut out = String::new();
    let mut stats = CityJsonStats {
        crs: crs_uri.clone(),
        ..Default::default()
    };

    for (id, obj) in objects {
        stats.objects += 1;
        let feature = format!("{inst}/bag/{}", iri_safe(id));

        // Type → bag:{Type} + geo:Feature + bot:Element.
        let otype = obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("CityObject");
        triple_iri(
            &mut out,
            &feature,
            RDF_TYPE,
            &format!("{BAG_NS}{}", iri_safe(otype)),
            &mut stats,
        );
        triple_iri(
            &mut out,
            &feature,
            RDF_TYPE,
            &format!("{GEO_NS}Feature"),
            &mut stats,
        );
        triple_iri(
            &mut out,
            &feature,
            RDF_TYPE,
            &format!("{BOT_NS}Element"),
            &mut stats,
        );
        // Descriptive triples (label / identificatie / attributes) are skipped
        // in volumetric-only mode: that mode exists solely to feed the 3D-Tiles
        // mesh pipeline, and the viewer-profile graph
        // ([`convert_cityjson_viewer_buildings`]) already asserts the same
        // facts about the same IRIs — duplicating them across two graphs would
        // double every row in non-DISTINCT SPARQL results over the union.
        if !opts.volumetric_only {
            triple_lit(&mut out, &feature, RDFS_LABEL, id, None, None, &mut stats);
            triple_lit(
                &mut out,
                &feature,
                &format!("{BAG_NS}identificatie"),
                id,
                None,
                None,
                &mut stats,
            );

            // Attributes → bag:{key} typed literals.
            if let Some(attrs) = obj.get("attributes").and_then(Value::as_object) {
                for (k, v) in attrs {
                    emit_attribute(&mut out, &feature, k, v, &mut stats);
                }
            }
        }

        // Topology: parents (dct:isPartOf) and children (bot:containsElement).
        for parent in obj
            .get("parents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(p) = parent.as_str() {
                triple_iri(
                    &mut out,
                    &feature,
                    &format!("{DCT_NS}isPartOf"),
                    &format!("{inst}/bag/{}", iri_safe(p)),
                    &mut stats,
                );
            }
        }
        for child in obj
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(c) = child.as_str() {
                triple_iri(
                    &mut out,
                    &feature,
                    &format!("{BOT_NS}containsElement"),
                    &format!("{inst}/bag/{}", iri_safe(c)),
                    &mut stats,
                );
            }
        }

        // Geometry, one node per LoD.
        let geoms = obj
            .get("geometry")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut seen_lods: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        for geom in &geoms {
            let lod_raw = geom.get("lod").map(value_to_lod_string).unwrap_or_default();
            let lod_key = sanitise_lod(&lod_raw);
            let n = seen_lods.entry(lod_key.clone()).or_insert(0);
            let suffix = if *n == 0 {
                lod_key.clone()
            } else {
                format!("{lod_key}-{n}")
            };
            *n += 1;
            let gnode = format!("{feature}/geom/lod{suffix}");

            let faces = collect_surfaces(geom.get("boundaries").unwrap_or(&Value::Null));
            // In volumetric-only mode, drop flat geometry (LoD0 footprints) so only
            // the 3D massing reaches the store — keeps the 3D-Tiles pipeline clean.
            if opts.volumetric_only && z_extent(&faces, &vertices) < 0.05 {
                continue;
            }
            let wkt = polyhedral_wkt(&faces, &vertices);
            if wkt.is_none() && geom.get("type").and_then(Value::as_str) == Some("MultiPoint") {
                continue; // non-surface geometry; skip silently
            }

            triple_iri(
                &mut out,
                &feature,
                &format!("{GEO_NS}hasGeometry"),
                &gnode,
                &mut stats,
            );
            triple_iri(
                &mut out,
                &gnode,
                RDF_TYPE,
                &format!("{GEO_NS}Geometry"),
                &mut stats,
            );
            if !lod_raw.is_empty() {
                triple_lit(
                    &mut out,
                    &gnode,
                    &format!("{BAG_NS}lod"),
                    &lod_raw,
                    None,
                    None,
                    &mut stats,
                );
            }
            if let Some(wkt) = wkt {
                let lexical = match &crs_uri {
                    Some(c) => format!("<{c}> {wkt}"),
                    None => wkt,
                };
                triple_lit(
                    &mut out,
                    &gnode,
                    &format!("{GEO_NS}asWKT"),
                    &lexical,
                    Some(WKT_LITERAL),
                    None,
                    &mut stats,
                );
            }
            // Loss-free CityJSON geometry literal (boundaries + semantics + LoD).
            let cj = serde_json::to_string(geom).unwrap_or_default();
            triple_lit(
                &mut out,
                &gnode,
                &format!("{OTS_NS}asCityJSON"),
                &cj,
                Some(CITYJSON_LITERAL),
                None,
                &mut stats,
            );

            // PROV-O lineage.
            if let Some(src) = &opts.source_url {
                triple_iri(
                    &mut out,
                    &gnode,
                    &format!("{PROV_NS}wasDerivedFrom"),
                    src,
                    &mut stats,
                );
            }
            if let Some(ts) = &opts.generated_at {
                triple_lit(
                    &mut out,
                    &gnode,
                    &format!("{PROV_NS}generatedAtTime"),
                    ts,
                    Some(&format!("{XSD}dateTime")),
                    None,
                    &mut stats,
                );
            }
            stats.geometries += 1;
        }
    }

    Ok((out, stats))
}

// ─── Viewer-profile converter (per-building linked data) ─────────────────────

/// Options for [`convert_cityjson_viewer_buildings`].
pub struct CityJsonViewerOptions {
    /// IRI prefix for minted resources; feature IRIs become `{inst_base}bag/{id}`
    /// — the SAME IRIs the WKT-Z lift mints, so the two graphs describe one
    /// resource (geometry solids in the tiles graph, viewer facts here).
    pub inst_base: String,
    /// URL of the shared CityJSON file the client renders (site-relative or
    /// absolute). The zone references it whole; each building references
    /// `{file_url}#{objectId}` so a pick resolves to its own element.
    pub file_url: String,
    /// Label / comment for the zone element that groups the buildings.
    pub zone_label: String,
    pub zone_comment: String,
    /// License IRI, attribution string and source IRI for the zone (PROV/DCT).
    pub license: Option<String>,
    pub attribution: Option<String>,
    pub source: Option<String>,
}

/// Convert a CityJSON document into the **viewer-feed layer**: one `bot:Zone`
/// holding a whole-file `fog:asCityjson` reference (the client renders the block
/// once), plus one element per root `Building` CityObject carrying its real
/// attributes, a WGS84 footprint polygon for the 2D map, a `#objectId` fragment
/// reference for per-building picking, and `owl:sameAs` links into the national
/// BAG registry (for `NL.IMBAG.Pand.*` identifiers).
///
/// Complements [`convert_cityjson`] (which externalises the full geometry): this
/// profile deliberately emits NO volumetric WKT so a feed-visible graph never
/// doubles the client-side CityJSON render.
pub fn convert_cityjson_viewer_buildings(
    doc: &Value,
    opts: &CityJsonViewerOptions,
) -> Result<(String, CityJsonStats), String> {
    use crate::geo::crs::{transform_xy, Crs};
    const OWL_SAMEAS: &str = "http://www.w3.org/2002/07/owl#sameAs";
    const RDFS_SEEALSO: &str = "http://www.w3.org/2000/01/rdf-schema#seeAlso";
    const RDFS_COMMENT: &str = "http://www.w3.org/2000/01/rdf-schema#comment";
    const OMG_NS: &str = "https://w3id.org/omg#";
    const FOG_AS_CITYJSON: &str = "https://w3id.org/fog#asCityjson";
    const BAG_PAND_LD: &str = "https://bag.basisregistraties.overheid.nl/bag/id/pand/";
    const BAG_API_ITEM: &str = "https://api.3dbag.nl/collections/pand/items/";

    if doc.get("type").and_then(Value::as_str) != Some("CityJSON") {
        return Err("not a CityJSON document".to_string());
    }
    let (scale, translate) = transform_of(doc);
    let vertices: Vec<[f64; 3]> = doc
        .get("vertices")
        .and_then(Value::as_array)
        .map(|vs| {
            vs.iter()
                .map(|v| {
                    let a = v.as_array();
                    let g = |i: usize| {
                        a.and_then(|x| x.get(i))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0)
                    };
                    [
                        g(0) * scale[0] + translate[0],
                        g(1) * scale[1] + translate[1],
                        g(2) * scale[2] + translate[2],
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    let crs_uri = doc
        .get("metadata")
        .and_then(|m| m.get("referenceSystem"))
        .and_then(Value::as_str)
        .map(normalise_crs_uri);
    // Source planar CRS for footprint reprojection; CRS84 (no-op) when absent.
    let source_crs = crs_uri
        .as_deref()
        .and_then(Crs::from_uri)
        .unwrap_or(Crs::Wgs84);
    let to_lonlat = |x: f64, y: f64| transform_xy(source_crs, Crs::Wgs84, x, y);

    let objects = doc
        .get("CityObjects")
        .and_then(Value::as_object)
        .ok_or("CityJSON has no CityObjects")?;

    let inst = opts.inst_base.trim_end_matches('/');
    let zone = format!("{inst}/bag/zone");
    let mut out = String::new();
    let mut stats = CityJsonStats {
        crs: crs_uri.clone(),
        ..Default::default()
    };

    // ── The zone: one whole-file render + provenance/attribution. ──
    triple_iri(
        &mut out,
        &zone,
        RDF_TYPE,
        &format!("{BOT_NS}Zone"),
        &mut stats,
    );
    triple_iri(
        &mut out,
        &zone,
        RDF_TYPE,
        &format!("{GEO_NS}Feature"),
        &mut stats,
    );
    triple_lit(
        &mut out,
        &zone,
        RDFS_LABEL,
        &opts.zone_label,
        None,
        Some("en"),
        &mut stats,
    );
    triple_lit(
        &mut out,
        &zone,
        RDFS_COMMENT,
        &opts.zone_comment,
        None,
        Some("en"),
        &mut stats,
    );
    if let Some(l) = &opts.license {
        triple_iri(&mut out, &zone, &format!("{DCT_NS}license"), l, &mut stats);
    }
    if let Some(a) = &opts.attribution {
        triple_lit(
            &mut out,
            &zone,
            &format!("{DCT_NS}rightsHolder"),
            a,
            None,
            None,
            &mut stats,
        );
    }
    if let Some(s) = &opts.source {
        triple_iri(&mut out, &zone, &format!("{DCT_NS}source"), s, &mut stats);
    }
    let zone_model = format!("{zone}/model");
    triple_iri(
        &mut out,
        &zone,
        &format!("{OMG_NS}hasGeometry"),
        &zone_model,
        &mut stats,
    );
    triple_iri(
        &mut out,
        &zone_model,
        RDF_TYPE,
        &format!("{OMG_NS}Geometry"),
        &mut stats,
    );
    triple_lit(
        &mut out,
        &zone_model,
        FOG_AS_CITYJSON,
        &opts.file_url,
        Some(&format!("{XSD}anyURI")),
        None,
        &mut stats,
    );
    // Zone map anchor: centroid of every vertex, reprojected to lon/lat.
    if !vertices.is_empty() {
        let (sx, sy) = vertices
            .iter()
            .fold((0.0, 0.0), |(ax, ay), v| (ax + v[0], ay + v[1]));
        let n = vertices.len() as f64;
        if let Some((lon, lat)) = to_lonlat(sx / n, sy / n) {
            let gnode = format!("{zone}/anchor");
            triple_iri(
                &mut out,
                &zone,
                &format!("{GEO_NS}hasGeometry"),
                &gnode,
                &mut stats,
            );
            triple_iri(
                &mut out,
                &gnode,
                RDF_TYPE,
                &format!("{GEO_NS}Geometry"),
                &mut stats,
            );
            triple_lit(
                &mut out,
                &gnode,
                &format!("{GEO_NS}asWKT"),
                &format!("POINT({} {})", fmt5(lon), fmt5(lat)),
                Some(WKT_LITERAL),
                None,
                &mut stats,
            );
        }
    }

    // ── One element per root Building. ──
    for (id, obj) in objects {
        if obj.get("type").and_then(Value::as_str) != Some("Building") {
            continue;
        }
        stats.objects += 1;
        let feature = format!("{inst}/bag/{}", iri_safe(id));
        triple_iri(
            &mut out,
            &feature,
            RDF_TYPE,
            &format!("{BAG_NS}Building"),
            &mut stats,
        );
        triple_iri(
            &mut out,
            &feature,
            RDF_TYPE,
            &format!("{BOT_NS}Building"),
            &mut stats,
        );
        triple_iri(
            &mut out,
            &feature,
            RDF_TYPE,
            &format!("{GEO_NS}Feature"),
            &mut stats,
        );
        triple_iri(
            &mut out,
            &zone,
            &format!("{BOT_NS}containsElement"),
            &feature,
            &mut stats,
        );

        let attrs = obj.get("attributes").and_then(Value::as_object);
        let year = attrs
            .and_then(|a| a.get("oorspronkelijkbouwjaar"))
            .and_then(Value::as_i64);
        // "NL.IMBAG.Pand.0268100000007417" → registry digits "0268100000007417".
        let digits = id
            .strip_prefix("NL.IMBAG.Pand.")
            .filter(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()));
        let label = match (digits, year) {
            (Some(d), Some(y)) => format!("Pand {d} ({y})"),
            (Some(d), None) => format!("Pand {d}"),
            (None, Some(y)) => format!("{id} ({y})"),
            (None, None) => id.clone(),
        };
        triple_lit(
            &mut out, &feature, RDFS_LABEL, &label, None, None, &mut stats,
        );
        triple_lit(
            &mut out,
            &feature,
            &format!("{BAG_NS}identificatie"),
            id,
            None,
            None,
            &mut stats,
        );
        if let Some(attrs) = attrs {
            for (k, v) in attrs {
                if k != "identificatie" {
                    emit_attribute(&mut out, &feature, k, v, &mut stats);
                }
            }
        }
        if let Some(d) = digits {
            // The official, dereferenceable government registry URI (303 → doc).
            triple_iri(
                &mut out,
                &feature,
                OWL_SAMEAS,
                &format!("{BAG_PAND_LD}{d}"),
                &mut stats,
            );
            triple_iri(
                &mut out,
                &feature,
                RDFS_SEEALSO,
                &format!("{BAG_API_ITEM}{}", iri_safe(id)),
                &mut stats,
            );
        }

        // Per-building model reference: the shared file + `#objectId` fragment.
        let model = format!("{feature}/model");
        triple_iri(
            &mut out,
            &feature,
            &format!("{OMG_NS}hasGeometry"),
            &model,
            &mut stats,
        );
        triple_iri(
            &mut out,
            &model,
            RDF_TYPE,
            &format!("{OMG_NS}Geometry"),
            &mut stats,
        );
        triple_lit(
            &mut out,
            &model,
            FOG_AS_CITYJSON,
            &format!("{}#{}", opts.file_url, id),
            Some(&format!("{XSD}anyURI")),
            None,
            &mut stats,
        );

        // 2D footprint: the largest flat face over the building and its parts
        // (3DBAG puts the LoD0 footprint on the Building object itself),
        // reprojected to a CRS84 lon/lat POLYGON for the map's Areas layer.
        let mut rings: Vec<Vec<usize>> = Vec::new();
        let mut collect_obj = |o: &Value| {
            for geom in o
                .get("geometry")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                for face in collect_surfaces(geom.get("boundaries").unwrap_or(&Value::Null)) {
                    if let Some(outer) = face.into_iter().next() {
                        rings.push(outer);
                    }
                }
            }
        };
        collect_obj(obj);
        for child in obj
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Some(c) = objects.get(child) {
                collect_obj(c);
            }
        }
        let footprint = rings
            .iter()
            .filter(|r| r.len() >= 3)
            .max_by(|a, b| {
                let area = |r: &Vec<usize>| ring_area_xy(r, &vertices);
                area(a)
                    .partial_cmp(&area(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .and_then(|ring| {
                let mut coords: Vec<String> = ring
                    .iter()
                    .filter_map(|&i| vertices.get(i))
                    .filter_map(|v| to_lonlat(v[0], v[1]))
                    .map(|(lon, lat)| format!("{} {}", fmt5(lon), fmt5(lat)))
                    .collect();
                if coords.len() < 3 {
                    return None;
                }
                if coords.first() != coords.last() {
                    coords.push(coords[0].clone());
                }
                Some(format!("POLYGON(({}))", coords.join(",")))
            });
        if let Some(wkt) = footprint {
            let gnode = format!("{feature}/footprint");
            triple_iri(
                &mut out,
                &feature,
                &format!("{GEO_NS}hasGeometry"),
                &gnode,
                &mut stats,
            );
            triple_iri(
                &mut out,
                &gnode,
                RDF_TYPE,
                &format!("{GEO_NS}Geometry"),
                &mut stats,
            );
            triple_lit(
                &mut out,
                &gnode,
                &format!("{GEO_NS}asWKT"),
                &wkt,
                Some(WKT_LITERAL),
                None,
                &mut stats,
            );
            stats.geometries += 1;
        }
    }
    Ok((out, stats))
}

/// Planar (XY) shoelace area of a vertex-index ring — picks the footprint face.
fn ring_area_xy(ring: &[usize], vertices: &[[f64; 3]]) -> f64 {
    let pts: Vec<&[f64; 3]> = ring.iter().filter_map(|&i| vertices.get(i)).collect();
    if pts.len() < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        a += pts[i][0] * pts[j][1] - pts[j][0] * pts[i][1];
    }
    (a / 2.0).abs()
}

/// Lon/lat formatting: 1e-5° ≈ 1 m — plenty for map footprints, keeps N-Triples lean.
fn fmt5(v: f64) -> String {
    let s = format!("{v:.5}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

// ─── Geometry flattening + WKT ────────────────────────────────────────────────

/// One face: its rings (exterior first), each a list of vertex indices.
type Face = Vec<Vec<usize>>;

/// Flatten CityJSON `boundaries` of any nesting (MultiSurface / Solid / MultiSolid
/// / CompositeSolid) to a flat list of faces. A *surface* is recognised as an
/// array of rings whose first ring is an array of vertex-index numbers.
fn collect_surfaces(boundaries: &Value) -> Vec<Face> {
    let mut faces = Vec::new();
    fn walk(v: &Value, out: &mut Vec<Face>) {
        let Some(arr) = v.as_array() else { return };
        let Some(first) = arr.first() else { return };
        // Is `v` a surface? surface = [ring, ...]; ring = [index, ...].
        let is_surface = first
            .as_array()
            .and_then(|ring| ring.first())
            .map(Value::is_number)
            .unwrap_or(false);
        if is_surface {
            let face: Face = arr
                .iter()
                .filter_map(|ring| {
                    ring.as_array().map(|r| {
                        r.iter()
                            .filter_map(|i| i.as_u64().map(|n| n as usize))
                            .collect()
                    })
                })
                .collect();
            out.push(face);
        } else {
            for e in arr {
                walk(e, out);
            }
        }
    }
    walk(boundaries, &mut faces);
    faces
}

/// Build a `POLYHEDRALSURFACE Z (...)` WKT from faces + dequantised vertices.
/// Each ring is closed (first vertex repeated) for WKT validity. Returns `None`
/// when there are no usable faces.
fn polyhedral_wkt(faces: &[Face], vertices: &[[f64; 3]]) -> Option<String> {
    let mut face_strs = Vec::new();
    for face in faces {
        let mut rings = Vec::new();
        for ring in face {
            if ring.len() < 3 {
                continue;
            }
            let mut coords: Vec<String> = ring
                .iter()
                .filter_map(|&i| vertices.get(i))
                .map(|c| format!("{} {} {}", fmt(c[0]), fmt(c[1]), fmt(c[2])))
                .collect();
            if coords.len() < 3 {
                continue;
            }
            // close the ring
            if coords.first() != coords.last() {
                coords.push(coords[0].clone());
            }
            rings.push(format!("({})", coords.join(",")));
        }
        if !rings.is_empty() {
            face_strs.push(format!("({})", rings.join(",")));
        }
    }
    if face_strs.is_empty() {
        return None;
    }
    Some(format!("POLYHEDRALSURFACE Z ({})", face_strs.join(",")))
}

/// Vertical (Z) extent in source units of a face set — `max(z) - min(z)` over the
/// referenced vertices. Used to spot flat footprints (≈ 0) versus 3D solids.
fn z_extent(faces: &[Face], vertices: &[[f64; 3]]) -> f64 {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for face in faces {
        for ring in face {
            for &i in ring {
                if let Some(v) = vertices.get(i) {
                    lo = lo.min(v[2]);
                    hi = hi.max(v[2]);
                }
            }
        }
    }
    if hi >= lo {
        hi - lo
    } else {
        0.0
    }
}

/// Compact metric formatting: up to mm precision, trailing zeros trimmed.
fn fmt(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".to_string()
    } else {
        s.to_string()
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn transform_of(doc: &Value) -> ([f64; 3], [f64; 3]) {
    let t = doc.get("transform");
    let arr3 = |key: &str, dflt: [f64; 3]| -> [f64; 3] {
        t.and_then(|t| t.get(key))
            .and_then(Value::as_array)
            .map(|a| {
                let g = |i: usize| a.get(i).and_then(Value::as_f64).unwrap_or(dflt[i]);
                [g(0), g(1), g(2)]
            })
            .unwrap_or(dflt)
    };
    (
        arr3("scale", [1.0, 1.0, 1.0]),
        arr3("translate", [0.0, 0.0, 0.0]),
    )
}

/// Normalise a CityJSON `referenceSystem` (`EPSG:7415`, `urn:ogc:def:crs:EPSG::7415`,
/// or an OGC CRS URL) into the canonical OGC EPSG URI used in WKT prefixes.
fn normalise_crs_uri(s: &str) -> String {
    let code = s
        .rsplit([':', '/'])
        .find(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()));
    match code {
        Some(c) => format!("http://www.opengis.net/def/crs/EPSG/0/{c}"),
        None => s.to_string(),
    }
}

fn value_to_lod_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// `"2.2"` → `"22"`, `"2"` → `"2"`; keep only alphanumerics.
fn sanitise_lod(lod: &str) -> String {
    let s: String = lod.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if s.is_empty() {
        "x".to_string()
    } else {
        s
    }
}

/// Make an IRI path segment safe: replace whitespace and IRI-reserved/illegal
/// characters with `_` (BAG identificaties are already path-safe).
fn iri_safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_whitespace() || "<>\"{}|\\^`".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect()
}

fn emit_attribute(
    out: &mut String,
    subject: &str,
    key: &str,
    v: &Value,
    stats: &mut CityJsonStats,
) {
    let pred = format!("{BAG_NS}{}", iri_safe(key));
    match v {
        Value::String(s) => triple_lit(out, subject, &pred, s, None, None, stats),
        Value::Bool(b) => triple_lit(
            out,
            subject,
            &pred,
            &b.to_string(),
            Some(&format!("{XSD}boolean")),
            None,
            stats,
        ),
        Value::Number(n) => {
            let dt = if n.is_i64() || n.is_u64() {
                format!("{XSD}integer")
            } else {
                format!("{XSD}decimal")
            };
            triple_lit(out, subject, &pred, &n.to_string(), Some(&dt), None, stats);
        }
        _ => {} // arrays/objects/null skipped
    }
}

fn triple_iri(out: &mut String, s: &str, p: &str, o: &str, stats: &mut CityJsonStats) {
    out.push('<');
    out.push_str(s);
    out.push_str("> <");
    out.push_str(p);
    out.push_str("> <");
    out.push_str(o);
    out.push_str("> .\n");
    stats.triples += 1;
}

fn triple_lit(
    out: &mut String,
    s: &str,
    p: &str,
    value: &str,
    datatype: Option<&str>,
    lang: Option<&str>,
    stats: &mut CityJsonStats,
) {
    out.push('<');
    out.push_str(s);
    out.push_str("> <");
    out.push_str(p);
    out.push_str("> \"");
    nt_escape(out, value);
    out.push('"');
    match (datatype, lang) {
        (Some(dt), _) => {
            out.push_str("^^<");
            out.push_str(dt);
            out.push('>');
        }
        (None, Some(l)) => {
            out.push('@');
            out.push_str(l);
        }
        _ => {}
    }
    out.push_str(" .\n");
    stats.triples += 1;
}

/// Escape a string for an N-Triples literal (RDF 1.1 §A).
fn nt_escape(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
}

/// Does this look like a CityJSON document by filename / content type?
pub fn is_cityjson_file(filename: &str, content_type: &str) -> bool {
    let f = filename.to_ascii_lowercase();
    let ct = content_type.to_ascii_lowercase();
    f.ends_with(".city.json")
        || f.ends_with(".cityjson")
        || ct.contains("city+json")
        || ct.contains("application/city+json")
}

// ─── Importer (asset + load + register), mirrors import_ifc_bytes ─────────────

/// Import a CityJSON document into `dataset_id`: keep the original as a
/// downloadable asset, convert to RDF, load the graph and register it.
#[allow(clippy::too_many_arguments)]
pub async fn import_cityjson_bytes(
    state: &AppState,
    dataset_id: &str,
    user_id: &str,
    file_name: &str,
    bytes: Vec<u8>,
    target_graph: Option<String>,
    public_asset: bool,
    generated_at: Option<String>,
    volumetric_only: bool,
) -> Result<CityJsonImportOutcome, String> {
    let base = state.base_url.trim_end_matches('/').to_string();

    // 1. Keep the original file as a dataset asset (the lossless source).
    let (asset_id, asset_url) = if state.object_store.is_configured() {
        let asset_id = uuid::Uuid::new_v4().to_string();
        let file_name_clean = crate::assets::sanitize_filename(file_name);
        let s3_key = format!("datasets/{dataset_id}/{asset_id}/{file_name_clean}");
        let declared = "application/city+json";
        let kind = crate::assets::classify(declared, &file_name_clean, &bytes);
        let meta = crate::assets::extract_for(kind, &bytes, declared, &file_name_clean);
        let size = bytes.len() as i64;
        state
            .object_store
            .upload(&s3_key, Bytes::from(bytes.clone()), declared)
            .await
            .map_err(|e| format!("asset upload failed: {e}"))?;
        let asset = state
            .auth_db
            .create_asset(
                &asset_id,
                dataset_id,
                &file_name_clean,
                declared,
                &s3_key,
                size,
                user_id,
                public_asset,
            )
            .map_err(|e| format!("asset record failed: {e}"))?;
        if let Err(e) =
            crate::server::routes::insert_asset_triples(state, &asset, dataset_id, kind, &meta)
        {
            tracing::warn!("cityjson import: asset metadata insert failed: {e}");
        }
        let assets_graph = crate::server::routes::assets_graph_iri(&state.base_url, dataset_id);
        let _ = state.auth_db.add_dataset_graph(dataset_id, &assets_graph);
        let url = format!("{base}/api/datasets/{dataset_id}/assets/{asset_id}/download");
        (Some(asset_id), Some(url))
    } else {
        (None, None)
    };

    let graph = target_graph
        .filter(|g| !g.trim().is_empty())
        .unwrap_or_else(|| format!("{base}/dataset/{dataset_id}/cityjson"));

    // 2. Parse + convert + load on the blocking pool.
    let inst_base = format!("{base}/dataset/{dataset_id}/");
    let opts = CityJsonOptions {
        inst_base,
        source_url: asset_url.clone(),
        generated_at,
        volumetric_only,
    };
    let store = state.store.clone();
    let graph_c = graph.clone();
    let stats = tokio::task::spawn_blocking(move || -> Result<CityJsonStats, String> {
        let doc: Value =
            serde_json::from_slice(&bytes).map_err(|e| format!("invalid JSON: {e}"))?;
        let (ntriples, stats) = convert_cityjson(&doc, &opts)?;
        use oxigraph::io::RdfFormat;
        store
            .graph_store_put(Some(&graph_c), &ntriples, RdfFormat::NTriples)
            .map_err(|e| format!("loading CityJSON graph failed: {e}"))?;
        Ok(stats)
    })
    .await
    .map_err(|e| format!("cityjson conversion task failed: {e}"))??;

    // 3. Register the graph on the dataset.
    let _ = state.auth_db.add_dataset_graph(dataset_id, &graph);
    let _ = super::handlers::detect_and_store_graph_role(state, dataset_id, &graph);
    state.auth_db.invalidate_accessible_graphs_cache();
    #[cfg(feature = "text-search")]
    state.mark_text_dirty();

    Ok(CityJsonImportOutcome {
        asset_id,
        asset_url,
        graph,
        stats,
    })
}

// ─── HTTP handler ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, serde::Deserialize)]
pub struct IngestQuery {
    /// Dry run: parse + convert and return stats + a sample, without loading.
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, serde::Serialize)]
pub struct IngestResponse {
    pub graph: Option<String>,
    pub asset_id: Option<String>,
    pub asset_url: Option<String>,
    pub stats: CityJsonStats,
    /// First lines of the generated N-Triples (preview only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    pub preview: bool,
}

/// `POST /api/datasets/:dataset_id/ingest/cityjson` (multipart/form-data).
///
/// Fields: `file` (the CityJSON document), optional `target_graph`, optional
/// `public` (`true` makes the stored source asset public). Query `?preview=true`
/// performs a dry run (convert + stats + sample N-Triples, no write).
pub async fn ingest_cityjson(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Query(q): Query<IngestQuery>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    // Authorisation: caller must have write access to this dataset.
    let dataset = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .ok_or_else(|| AppError::NotFound(format!("Dataset '{dataset_id}' not found")))?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &dataset)
        .map_err(|e| AppError::Internal(e.to_string()))?
    {
        return Err(AppError::Unauthorized(
            "Write access to this dataset required".to_string(),
        ));
    }

    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name = "upload.city.json".to_string();
    let mut target_graph: Option<String> = None;
    let mut public_asset = false;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        match field.name().unwrap_or("") {
            "file" => {
                if let Some(fname) = field.file_name() {
                    if !fname.is_empty() {
                        file_name = crate::assets::sanitize_filename(fname);
                    }
                }
                file_bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Failed to read file: {e}")))?
                        .to_vec(),
                );
            }
            "target_graph" => {
                target_graph = field.text().await.ok().filter(|s| !s.trim().is_empty());
            }
            "public" => {
                public_asset = field.text().await.map(|s| s == "true").unwrap_or(false);
            }
            _ => {}
        }
    }

    let bytes =
        file_bytes.ok_or_else(|| AppError::BadRequest("Missing 'file' field".to_string()))?;

    // Per-graph write boundary: a non-admin may only target a graph already
    // registered to this dataset or under its canonical IRI namespace (mirrors
    // the bulk-import / Graph Store Protocol gate).
    if !user.is_admin() {
        if let Some(t) = target_graph.as_deref() {
            let namespace = format!(
                "{}/dataset/{}",
                state.base_url.trim_end_matches('/'),
                dataset_id
            );
            let registered = state
                .auth_db
                .list_dataset_graphs(&dataset_id)
                .unwrap_or_default();
            let owned_by_other = state
                .auth_db
                .graph_has_other_dataset_refs(t, &dataset_id)
                .unwrap_or(true);
            let in_scope = registered.iter().any(|g| g == t) || t.starts_with(&namespace);
            if owned_by_other || !in_scope {
                return Err(AppError::Forbidden(format!(
                    "Target graph <{t}> is outside dataset '{dataset_id}'"
                )));
            }
        }
    }

    if q.preview {
        let base = state.base_url.trim_end_matches('/').to_string();
        let opts = CityJsonOptions {
            inst_base: format!("{base}/dataset/{dataset_id}/"),
            source_url: None,
            generated_at: None,
            volumetric_only: false,
        };
        let doc: Value = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::BadRequest(format!("invalid JSON: {e}")))?;
        let (nt, stats) = convert_cityjson(&doc, &opts).map_err(AppError::BadRequest)?;
        let sample: String = nt.lines().take(60).collect::<Vec<_>>().join("\n");
        return Ok(Json(IngestResponse {
            graph: None,
            asset_id: None,
            asset_url: None,
            stats,
            sample: Some(sample),
            preview: true,
        }));
    }

    let generated_at = Some(chrono::Utc::now().to_rfc3339());
    let outcome = import_cityjson_bytes(
        &state,
        &dataset_id,
        &user.user_id,
        &file_name,
        bytes,
        target_graph,
        public_asset,
        generated_at,
        false, // HTTP imports keep every LoD; volumetric-only is a seed-time concern
    )
    .await
    .map_err(AppError::BadRequest)?;

    Ok(Json(IngestResponse {
        graph: Some(outcome.graph),
        asset_id: outcome.asset_id,
        asset_url: outcome.asset_url,
        stats: outcome.stats,
        sample: None,
        preview: false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> CityJsonOptions {
        CityJsonOptions {
            inst_base: "https://ex.org/dataset/d1/".to_string(),
            source_url: Some("https://ex.org/src.city.json".to_string()),
            generated_at: None,
            volumetric_only: false,
        }
    }

    /// A minimal CityJSON: one Building (Solid LoD2.2) — a 1×1×1 box at RD-ish
    /// coordinates, plus a quantising transform so dequantisation is exercised.
    fn cube_doc() -> Value {
        serde_json::json!({
            "type": "CityJSON",
            "version": "2.0",
            "metadata": { "referenceSystem": "https://www.opengis.net/def/crs/EPSG/0/7415" },
            "transform": { "scale": [1.0, 1.0, 1.0], "translate": [84000.0, 447000.0, 0.0] },
            "CityObjects": {
                "NL.IMBAG.Pand.001": {
                    "type": "Building",
                    "attributes": { "bouwjaar": 1998, "status": "Pand in gebruik" },
                    "children": ["NL.IMBAG.Pand.001-0"],
                    "geometry": [{
                        "type": "Solid",
                        "lod": "2.2",
                        "boundaries": [[
                            [[0,1,2,3]], [[4,7,6,5]],
                            [[0,4,5,1]], [[1,5,6,2]], [[2,6,7,3]], [[3,7,4,0]]
                        ]]
                    }]
                }
            },
            "vertices": [
                [0,0,0],[1,0,0],[1,1,0],[0,1,0],
                [0,0,1],[1,0,1],[1,1,1],[0,1,1]
            ]
        })
    }

    #[test]
    fn converts_building_solid() {
        let (nt, stats) = convert_cityjson(&cube_doc(), &opts()).unwrap();
        assert_eq!(stats.objects, 1);
        assert_eq!(stats.geometries, 1);
        assert_eq!(
            stats.crs.as_deref(),
            Some("http://www.opengis.net/def/crs/EPSG/0/7415")
        );
        // Feature IRI minted under {inst_base}bag/{id}.
        assert!(nt.contains("<https://ex.org/dataset/d1/bag/NL.IMBAG.Pand.001>"));
        // Typed as bag:Building + geo:Feature.
        assert!(nt.contains("<https://data.3dbag.nl/def/Building>"));
        // Attributes carried through with datatypes.
        assert!(nt.contains("\"1998\"^^<http://www.w3.org/2001/XMLSchema#integer>"));
        assert!(nt.contains("\"Pand in gebruik\""));
        // BOT topology to the child.
        assert!(nt.contains("<https://w3id.org/bot#containsElement>"));
        // Geometry node + WKT-Z with the dequantised, CRS-prefixed coordinate.
        assert!(nt.contains("/geom/lod22>"));
        assert!(nt.contains("POLYHEDRALSURFACE Z"));
        assert!(nt.contains("<http://www.opengis.net/def/crs/EPSG/0/7415> POLYHEDRALSURFACE Z"));
        assert!(nt.contains("84000 447000 0")); // translate applied
                                                // Loss-free CityJSON literal.
        assert!(nt.contains("^^<https://open-triplestore.org/def/cityjsonGeometryLiteral>"));
        // PROV lineage.
        assert!(nt
            .contains("<http://www.w3.org/ns/prov#wasDerivedFrom> <https://ex.org/src.city.json>"));
        // bag:lod retained.
        assert!(nt.contains("<https://data.3dbag.nl/def/lod> \"2.2\""));
    }

    #[test]
    fn wkt_z_reparses_with_engine_volume() {
        // The emitted WKT-Z must round-trip through the 3D engine to a unit cube.
        #[cfg(feature = "geometry3d")]
        {
            let (nt, _) = convert_cityjson(&cube_doc(), &opts()).unwrap();
            // pull the WKT body out of the asWKT literal
            let marker = "POLYHEDRALSURFACE Z";
            let start = nt.find(marker).unwrap();
            let end = nt[start..].find("\"^^").unwrap() + start;
            let wkt = &nt[start..end];
            let g = crate::geo::geom3d::parse_wkt3d(wkt).unwrap();
            assert!((g.volume() - 1.0).abs() < 1e-6, "vol {}", g.volume());
        }
    }

    #[test]
    fn rejects_non_cityjson() {
        let v = serde_json::json!({ "type": "FeatureCollection" });
        assert!(convert_cityjson(&v, &opts()).is_err());
    }

    #[test]
    fn volumetric_only_skips_flat_footprints() {
        // A 3DBAG-shaped object: a flat LoD0 footprint (z all 0) + a LoD2.2 Solid.
        // volumetric_only must drop the footprint and keep the solid.
        let doc = serde_json::json!({
            "type": "CityJSON",
            "version": "2.0",
            "transform": { "scale": [1,1,1], "translate": [0,0,0] },
            "CityObjects": {
                "pand": { "type": "Building", "geometry": [
                    { "type": "MultiSurface", "lod": "0", "boundaries": [[[0,1,2,3]]] }
                ]},
                "pand-0": { "type": "BuildingPart", "geometry": [{
                    "type": "Solid", "lod": "2.2",
                    "boundaries": [[
                        [[0,1,2,3]], [[4,7,6,5]],
                        [[0,4,5,1]], [[1,5,6,2]], [[2,6,7,3]], [[3,7,4,0]]
                    ]]
                }]}
            },
            "vertices": [
                [0,0,0],[1,0,0],[1,1,0],[0,1,0],
                [0,0,1],[1,0,1],[1,1,1],[0,1,1]
            ]
        });
        let mut o = opts();
        o.volumetric_only = true;
        let (nt, stats) = convert_cityjson(&doc, &o).unwrap();
        assert_eq!(stats.geometries, 1, "only the solid is meshed: {nt}");
        assert!(nt.contains("POLYHEDRALSURFACE Z"), "the solid survives");
        // The flat footprint's geometry node (lod0) is gone.
        assert!(
            !nt.contains("/geom/lod0>"),
            "flat LoD0 footprint dropped: {nt}"
        );

        // Without the flag both geometries are kept (default behaviour intact).
        let (_nt2, stats2) = convert_cityjson(&doc, &opts()).unwrap();
        assert_eq!(stats2.geometries, 2, "default keeps every LoD");
    }

    #[test]
    fn viewer_profile_emits_zone_buildings_links_and_footprints() {
        // A 3DBAG-shaped doc: Building (LoD0 footprint + attributes) + its
        // BuildingPart child (LoD2.2 Solid), georeferenced in EPSG:7415.
        let doc = serde_json::json!({
            "type": "CityJSON",
            "version": "2.0",
            "metadata": { "referenceSystem": "https://www.opengis.net/def/crs/EPSG/0/7415" },
            "transform": { "scale": [1.0, 1.0, 1.0], "translate": [185700.0, 428100.0, 0.0] },
            "CityObjects": {
                "NL.IMBAG.Pand.0268100000007417": {
                    "type": "Building",
                    "attributes": {
                        "oorspronkelijkbouwjaar": 1920,
                        "status": "Pand in gebruik",
                        "b3_h_dak_max": 13.139
                    },
                    "children": ["NL.IMBAG.Pand.0268100000007417-0"],
                    "geometry": [
                        { "type": "MultiSurface", "lod": "0", "boundaries": [[[0,1,2,3]]] }
                    ]
                },
                "NL.IMBAG.Pand.0268100000007417-0": {
                    "type": "BuildingPart",
                    "parents": ["NL.IMBAG.Pand.0268100000007417"],
                    "geometry": [{
                        "type": "Solid", "lod": "2.2",
                        "boundaries": [[
                            [[0,1,2,3]], [[4,7,6,5]],
                            [[0,4,5,1]], [[1,5,6,2]], [[2,6,7,3]], [[3,7,4,0]]
                        ]]
                    }]
                }
            },
            "vertices": [
                [0,0,0],[10,0,0],[10,8,0],[0,8,0],
                [0,0,7],[10,0,7],[10,8,7],[0,8,7]
            ]
        });
        let opts = CityJsonViewerOptions {
            inst_base: "https://ex.org/dataset/d1/".to_string(),
            file_url: "/samples/block.city.json".to_string(),
            zone_label: "Test block".to_string(),
            zone_comment: "A block".to_string(),
            license: Some("https://creativecommons.org/licenses/by/4.0/".to_string()),
            attribution: Some("© 3DBAG by tudelft3d and 3DGI".to_string()),
            source: Some("https://docs.3dbag.nl/en/copyright/".to_string()),
        };
        let (nt, stats) = convert_cityjson_viewer_buildings(&doc, &opts).unwrap();
        // Only the root Building becomes an element (the part is geometry detail).
        assert_eq!(stats.objects, 1, "{nt}");

        // Zone: whole-file render + attribution + centroid anchor.
        assert!(nt.contains("<https://ex.org/dataset/d1/bag/zone> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://w3id.org/bot#Zone>"));
        assert!(
            nt.contains("\"/samples/block.city.json\"^^<http://www.w3.org/2001/XMLSchema#anyURI>")
        );
        assert!(nt.contains(
            "<http://purl.org/dc/terms/license> <https://creativecommons.org/licenses/by/4.0/>"
        ));
        assert!(
            nt.contains("/bag/zone/anchor> <http://www.opengis.net/ont/geosparql#asWKT> \"POINT(")
        );

        // Building: typed, labelled with the registry number + year, attributed.
        let b = "<https://ex.org/dataset/d1/bag/NL.IMBAG.Pand.0268100000007417>";
        assert!(nt.contains(&format!(
            "{b} <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://w3id.org/bot#Building>"
        )));
        assert!(nt.contains("\"Pand 0268100000007417 (1920)\""));
        assert!(nt.contains("\"1920\"^^<http://www.w3.org/2001/XMLSchema#integer>"));
        // Zone → building containment (drives the viewer tree).
        assert!(nt.contains(&format!(
            "<https://ex.org/dataset/d1/bag/zone> <https://w3id.org/bot#containsElement> {b}"
        )));
        // The dereferenceable national-registry identity + the 3DBAG API doc.
        assert!(nt.contains(&format!("{b} <http://www.w3.org/2002/07/owl#sameAs> <https://bag.basisregistraties.overheid.nl/bag/id/pand/0268100000007417>")));
        assert!(nt.contains(
            "<https://api.3dbag.nl/collections/pand/items/NL.IMBAG.Pand.0268100000007417>"
        ));
        // Per-building pick fragment into the shared file.
        assert!(nt.contains("\"/samples/block.city.json#NL.IMBAG.Pand.0268100000007417\"^^<http://www.w3.org/2001/XMLSchema#anyURI>"));
        // Footprint: an unprefixed CRS84 lon/lat POLYGON near Nijmegen — the RD
        // coordinates must NOT leak through unprojected.
        assert!(
            nt.contains("/footprint> <http://www.opengis.net/ont/geosparql#asWKT> \"POLYGON((5.8"),
            "{nt}"
        );
        assert!(
            !nt.contains("185700 428100"),
            "no raw RD in the viewer graph"
        );
        // And no volumetric WKT: rendering stays with the client-side CityJSON.
        assert!(!nt.contains("POLYHEDRALSURFACE"));
    }

    #[test]
    fn multisurface_and_lod_dedup() {
        let doc = serde_json::json!({
            "type": "CityJSON",
            "transform": { "scale": [1,1,1], "translate": [0,0,0] },
            "CityObjects": { "x": { "type": "Building", "geometry": [
                { "type": "MultiSurface", "lod": "1.2", "boundaries": [[[0,1,2]]] },
                { "type": "MultiSurface", "lod": "1.2", "boundaries": [[[0,1,2]]] }
            ]}},
            "vertices": [[0,0,0],[1,0,0],[0,1,0]]
        });
        let (nt, stats) = convert_cityjson(&doc, &opts()).unwrap();
        assert_eq!(stats.geometries, 2);
        // Two same-LoD geometries get distinct nodes (…/lod12 and …/lod12-1).
        assert!(nt.contains("/geom/lod12>"));
        assert!(nt.contains("/geom/lod12-1>"));
    }
}
