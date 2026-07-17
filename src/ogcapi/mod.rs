//! **OGC API – Features – Part 1: Core** (v1.0) — a thin, conformant HTTP facade
//! over the existing SPARQL/Geo engine.
//!
//! Each dataset the caller can access is exposed as one *collection*; its
//! geometry-bearing viewer-feed elements ([`crate::geo::viewer_feed`]) are the
//! *features*. Geometry is already reprojected to WGS84 by the feed, so a feature
//! is just `{ type, geometry: GeoJSON(wkt4326), properties: {…} }` with the
//! element IRI as its stable `id` — the same IRI used as the RDF subject and the
//! 3D-Tiles metadata key, keeping the binding contract intact across phases.
//!
//! The mandatory encoding is GeoJSON (`application/geo+json`). Anonymous callers
//! reach public datasets exactly like the viewer feed; CQL2 and transactions are
//! out of scope (Core only).

pub mod wkt;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::Dataset;
use crate::geo::viewer_feed::{build_viewer_feed, dataset_geo_stats, ViewerElement};
use crate::server::AppState;

use self::wkt::{wkt_bbox, wkt_to_geojson, BBox};

/// The `application/geo+json` media type — the mandatory Features encoding.
const GEOJSON_CT: &str = "application/geo+json";

type ApiError = (StatusCode, String);

/// All OGC API – Features routes. Mounted under `/api/ogc`. Every handler is
/// anonymous-capable (`Option<Extension<AuthenticatedUser>>` + `can_access_dataset`),
/// so the `optional_auth` layer must wrap this router at the merge site.
pub fn ogcapi_routes() -> Router<AppState> {
    Router::new()
        .route("/api/ogc", get(landing))
        .route("/api/ogc/", get(landing))
        .route("/api/ogc/conformance", get(conformance))
        .route("/api/ogc/collections", get(collections))
        .route("/api/ogc/collections/:collectionId", get(collection))
        .route(
            "/api/ogc/collections/:collectionId/items",
            get(collection_items),
        )
        .route(
            "/api/ogc/collections/:collectionId/items/:featureId",
            get(collection_item),
        )
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// A GeoJSON JSON response carrying the `application/geo+json` content type.
fn geojson(value: Value) -> Response {
    let mut resp = Json(value).into_response();
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(GEOJSON_CT));
    resp
}

/// `{base}/api/ogc{suffix}` — absolute links per the API base path.
fn ogc_url(base: &str, suffix: &str) -> String {
    format!("{}/api/ogc{}", base.trim_end_matches('/'), suffix)
}

/// Load a dataset by id and enforce access, mirroring `routes::viewer_feed`.
fn load_accessible_dataset(
    state: &AppState,
    user: &Option<Extension<AuthenticatedUser>>,
    collection_id: &str,
) -> Result<Dataset, ApiError> {
    let dataset = state
        .auth_db
        .get_dataset(collection_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Collection not found".to_string()))?;
    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    if !state
        .auth_db
        .can_access_dataset(user_id, &dataset)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    {
        return Err((StatusCode::FORBIDDEN, "Access denied".to_string()));
    }
    Ok(dataset)
}

/// The dataset's data graphs that feed the viewer, with the verbose ifcOWL lift
/// graph (`…/ifcowl`) and the 3D-Tiles-only `tiles3d-*` lift graphs excluded —
/// exactly as the viewer-feed, geo-stats, geo-stats-batch and DCAT callers do.
/// The ifcOWL graph is the full 1:1 IFC schema (millions of triples) and carries
/// none of the BOT/geo geometry the feed reads; the `tiles3d-*` graphs hold
/// CityJSON lifted to WKT-Z purely for the 3D-Tiles pipeline, so surfacing them
/// here would make the Features API disagree with the 2D map. Centralised so the
/// exclusion can't drift between the OGC handlers.
fn feed_data_graphs(state: &AppState, dataset_id: &str) -> Result<Vec<String>, ApiError> {
    Ok(state
        .auth_db
        .list_dataset_graphs(dataset_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .into_iter()
        .filter(|g| !g.ends_with("/ifcowl") && !crate::geo::viewer_feed::is_tiles3d_graph(g))
        .collect())
}

/// The geometry-bearing viewer-feed elements of a dataset (whole dataset scope).
fn dataset_features(state: &AppState, dataset_id: &str) -> Result<Vec<ViewerElement>, ApiError> {
    let data_graphs = feed_data_graphs(state, dataset_id)?;
    Ok(build_viewer_feed(&state.store, &data_graphs, None))
}

/// Build a single GeoJSON Feature from a located viewer element, or `None` when
/// the element has no parsable WGS84 geometry (it is then omitted from output).
fn element_to_feature(el: &ViewerElement) -> Option<Value> {
    let geometry = wkt_to_geojson(el.wkt4326.as_deref()?)?;
    let mut props = json!({
        "label": el.label,
        "types": el.types,
    });
    if let Some(obj) = props.as_object_mut() {
        if let Some(gltf) = &el.gltf_url {
            obj.insert("gltf_url".to_string(), json!(gltf));
        }
        if let Some(ifc) = &el.ifc_url {
            obj.insert("ifc_url".to_string(), json!(ifc));
        }
        if let Some(guid) = &el.ifc_guid {
            obj.insert("ifc_guid".to_string(), json!(guid));
        }
        if !el.files.is_empty() {
            let files: Vec<Value> = el
                .files
                .iter()
                .map(|(fmt, url)| json!({ "format": fmt, "url": url }))
                .collect();
            obj.insert("files".to_string(), json!(files));
        }
    }
    Some(json!({
        "type": "Feature",
        "id": el.id,
        "geometry": geometry,
        "properties": props,
    }))
}

/// The WGS84 spatial extent of a dataset's features, `[minx, miny, maxx, maxy]`,
/// or `None` when nothing is mappable. Cheap enough for the collections list
/// because the feed is already built per request.
fn features_extent(elements: &[ViewerElement]) -> Option<[f64; 4]> {
    let mut acc: Option<BBox> = None;
    for el in elements {
        let Some(wkt) = el.wkt4326.as_deref() else {
            continue;
        };
        let Some(bb) = wkt_bbox(wkt) else { continue };
        acc = Some(match acc {
            None => bb,
            Some(a) => BBox {
                min_x: a.min_x.min(bb.min_x),
                min_y: a.min_y.min(bb.min_y),
                max_x: a.max_x.max(bb.max_x),
                max_y: a.max_y.max(bb.max_y),
            },
        });
    }
    acc.map(|b| [b.min_x, b.min_y, b.max_x, b.max_y])
}

/// CRS84 — the single CRS this facade publishes (axis order lon,lat).
const CRS84: &str = "http://www.opengis.net/def/crs/OGC/1.3/CRS84";

/// JSON metadata document for one collection (used by both `collections` and
/// the single-collection endpoint).
fn collection_doc(base: &str, dataset: &Dataset) -> Value {
    let id = &dataset.id;
    let self_link = ogc_url(base, &format!("/collections/{id}"));
    let items_link = ogc_url(base, &format!("/collections/{id}/items"));
    let title = if dataset.name.is_empty() {
        id.clone()
    } else {
        dataset.name.clone()
    };
    let mut doc = json!({
        "id": id,
        "title": title,
        "description": dataset.description,
        "crs": [CRS84],
        "storageCrs": CRS84,
        "links": [
            { "rel": "self", "type": "application/json",
              "title": "This collection", "href": self_link },
            { "rel": "items", "type": GEOJSON_CT,
              "title": "Features", "href": items_link },
        ],
    });
    // Spatial extent is computed lazily by the caller (it needs the feed) and
    // injected via `with_extent` to avoid double-building the feed.
    if let Some(obj) = doc.as_object_mut() {
        obj.entry("itemType").or_insert(json!("feature"));
    }
    doc
}

/// Attach a spatial extent (`[minx,miny,maxx,maxy]`) to a collection document.
fn with_extent(mut doc: Value, bbox: Option<[f64; 4]>) -> Value {
    if let (Some(obj), Some(bbox)) = (doc.as_object_mut(), bbox) {
        obj.insert(
            "extent".to_string(),
            json!({
                "spatial": {
                    "bbox": [bbox],
                    "crs": CRS84,
                }
            }),
        );
    }
    doc
}

// ── handlers ────────────────────────────────────────────────────────────────

/// `GET /api/ogc/` — landing page with links to conformance and collections.
async fn landing(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let base = state.base_url.as_str();
    Ok(Json(json!({
        "title": "Open Triplestore — OGC API Features",
        "description": "OGC API – Features (Core) over the linked-data geospatial engine.",
        "links": [
            { "rel": "self", "type": "application/json",
              "title": "This document", "href": ogc_url(base, "/") },
            { "rel": "conformance", "type": "application/json",
              "title": "Conformance classes", "href": ogc_url(base, "/conformance") },
            { "rel": "data", "type": "application/json",
              "title": "Collections", "href": ogc_url(base, "/collections") },
            { "rel": "service-desc", "type": "application/vnd.oai.openapi+json;version=3.0",
              "title": "API definition", "href": format!("{}/api-docs/openapi.json", base.trim_end_matches('/')) },
        ],
    })))
}

/// `GET /api/ogc/conformance` — the Core + GeoJSON + OAS30 conformance classes.
async fn conformance() -> impl IntoResponse {
    Json(json!({
        "conformsTo": [
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core",
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/oas30",
            "http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/geojson",
        ]
    }))
}

/// `GET /api/ogc/collections` — one collection per accessible dataset.
async fn collections(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    let base = state.base_url.as_str();
    let user_id = user.as_ref().map(|u| u.user_id.as_str());
    let datasets = state
        .auth_db
        .list_accessible_datasets(user_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut docs: Vec<Value> = Vec::with_capacity(datasets.len());
    for d in &datasets {
        // Only advertise a spatial extent when the dataset actually has features.
        // Gate the (relatively heavy) full feed build behind the cheap geo-stats
        // ASK early-out: a geometry-free dataset — the common case on a multi-
        // dataset list — is decided by a single ASK instead of building a whole
        // viewer feed per dataset (the old N+1). Datasets with geometry still get
        // their extent, so the response shape is unchanged.
        let graphs = feed_data_graphs(&state, &d.id).unwrap_or_default();
        let extent = if dataset_geo_stats(&state.store, &graphs).has_coordinates {
            features_extent(&build_viewer_feed(&state.store, &graphs, None))
        } else {
            None
        };
        docs.push(with_extent(collection_doc(base, d), extent));
    }

    Ok(Json(json!({
        "links": [
            { "rel": "self", "type": "application/json",
              "title": "Collections", "href": ogc_url(base, "/collections") },
        ],
        "collections": docs,
    })))
}

/// `GET /api/ogc/collections/:collectionId` — one collection's metadata.
async fn collection(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
) -> Result<impl IntoResponse, ApiError> {
    let dataset = load_accessible_dataset(&state, &user, &collection_id)?;
    let base = state.base_url.as_str();
    // Cheap geo-stats ASK gate before the full feed build (see `collections`).
    let graphs = feed_data_graphs(&state, &dataset.id).unwrap_or_default();
    let extent = if dataset_geo_stats(&state.store, &graphs).has_coordinates {
        features_extent(&build_viewer_feed(&state.store, &graphs, None))
    } else {
        None
    };
    Ok(Json(with_extent(collection_doc(base, &dataset), extent)))
}

/// Query parameters for the items endpoint (OGC API – Features Core §7.15).
#[derive(Debug, Deserialize, Default)]
pub struct ItemsQuery {
    /// `minx,miny,maxx,maxy` in WGS84 lon/lat.
    pub bbox: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;

/// Parse a `bbox=minx,miny,maxx,maxy` string into a [`BBox`].
fn parse_bbox(raw: &str) -> Result<BBox, ApiError> {
    let nums: Vec<f64> = raw
        .split(',')
        .map(|t| t.trim().parse::<f64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                "bbox must be four comma-separated numbers".to_string(),
            )
        })?;
    if nums.len() != 4 {
        return Err((
            StatusCode::BAD_REQUEST,
            "bbox must have exactly four values: minx,miny,maxx,maxy".to_string(),
        ));
    }
    Ok(BBox {
        min_x: nums[0].min(nums[2]),
        min_y: nums[1].min(nums[3]),
        max_x: nums[0].max(nums[2]),
        max_y: nums[1].max(nums[3]),
    })
}

/// `GET /api/ogc/collections/:collectionId/items` — a GeoJSON FeatureCollection.
async fn collection_items(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path(collection_id): Path<String>,
    Query(q): Query<ItemsQuery>,
) -> Result<Response, ApiError> {
    let dataset = load_accessible_dataset(&state, &user, &collection_id)?;
    let base = state.base_url.as_str();

    let bbox = match q.bbox.as_deref() {
        Some(raw) if !raw.is_empty() => Some(parse_bbox(raw)?),
        _ => None,
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0);

    let elements = dataset_features(&state, &dataset.id)?;

    // Match = every element with a parsable geometry passing the bbox filter.
    let matched: Vec<&ViewerElement> = elements
        .iter()
        .filter(|el| {
            let Some(wkt) = el.wkt4326.as_deref() else {
                return false;
            };
            match bbox {
                None => wkt_to_geojson(wkt).is_some(),
                Some(filter) => wkt_bbox(wkt).is_some_and(|bb| bb.intersects(&filter)),
            }
        })
        .collect();

    let number_matched = matched.len();
    let features: Vec<Value> = matched
        .into_iter()
        .skip(offset)
        .take(limit)
        .filter_map(element_to_feature)
        .collect();
    let number_returned = features.len();

    // RFC-8288 paging links: self + next (when more rows remain past this page).
    let qs = |off: usize| -> String {
        let mut parts = vec![format!("limit={limit}"), format!("offset={off}")];
        if let Some(raw) = q.bbox.as_deref().filter(|s| !s.is_empty()) {
            parts.push(format!("bbox={raw}"));
        }
        parts.join("&")
    };
    let items_path = format!("/collections/{}/items", dataset.id);
    let mut links = vec![json!({
        "rel": "self",
        "type": GEOJSON_CT,
        "title": "This page",
        "href": format!("{}?{}", ogc_url(base, &items_path), qs(offset)),
    })];
    if offset + number_returned < number_matched {
        links.push(json!({
            "rel": "next",
            "type": GEOJSON_CT,
            "title": "Next page",
            "href": format!("{}?{}", ogc_url(base, &items_path), qs(offset + number_returned)),
        }));
    }
    links.push(json!({
        "rel": "collection",
        "type": "application/json",
        "title": "The collection",
        "href": ogc_url(base, &format!("/collections/{}", dataset.id)),
    }));

    Ok(geojson(json!({
        "type": "FeatureCollection",
        "features": features,
        "numberMatched": number_matched,
        "numberReturned": number_returned,
        "links": links,
        "timeStamp": now_rfc3339(),
    })))
}

/// `GET /api/ogc/collections/:collectionId/items/:featureId` — a single Feature.
///
/// `featureId` is matched against element ids. Because the id is a full IRI it is
/// usually percent-encoded by clients; Ax' `Path` decodes that for us. We also
/// accept the IRI's local name (fragment / last path segment) as a convenience.
async fn collection_item(
    user: Option<Extension<AuthenticatedUser>>,
    State(state): State<AppState>,
    Path((collection_id, feature_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let dataset = load_accessible_dataset(&state, &user, &collection_id)?;
    let elements = dataset_features(&state, &dataset.id)?;

    let el = elements
        .iter()
        .find(|e| e.id == feature_id)
        .or_else(|| elements.iter().find(|e| local_name(&e.id) == feature_id))
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Feature not found".to_string()))?;

    let feature = element_to_feature(el)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Feature has no geometry".to_string()))?;

    // Augment the bare Feature with self/collection links per Core §7.16.
    let base = state.base_url.as_str();
    let mut feature = feature;
    if let Some(obj) = feature.as_object_mut() {
        obj.insert(
            "links".to_string(),
            json!([
                { "rel": "self", "type": GEOJSON_CT, "title": "This feature",
                  "href": ogc_url(base, &format!("/collections/{}/items/{}", dataset.id, feature_id)) },
                { "rel": "collection", "type": "application/json", "title": "The collection",
                  "href": ogc_url(base, &format!("/collections/{}", dataset.id)) },
            ]),
        );
    }
    Ok(geojson(feature))
}

/// The local name of an IRI: the part after the last `#` or `/`.
fn local_name(iri: &str) -> &str {
    iri.rsplit(['#', '/']).next().unwrap_or(iri)
}

/// An RFC-3339 timestamp for the FeatureCollection `timeStamp`. OGC API –
/// Features Core §7.15.4 types this property as `date-time` (RFC-3339), so a
/// bare epoch-seconds value is non-conformant and breaks strict clients. `chrono`
/// is already a dependency; format UTC with a `Z` offset.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::viewer_feed::ViewerElement;

    fn el(id: &str, wkt: Option<&str>) -> ViewerElement {
        ViewerElement {
            id: id.to_string(),
            wkt4326: wkt.map(|s| s.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn parse_bbox_orders_corners() {
        // Reversed corners are normalised to min/max.
        let bb = parse_bbox("4,52,3,51").unwrap();
        assert_eq!(
            bb,
            BBox {
                min_x: 3.0,
                min_y: 51.0,
                max_x: 4.0,
                max_y: 52.0
            }
        );
    }

    #[test]
    fn parse_bbox_rejects_wrong_arity() {
        assert!(parse_bbox("1,2,3").is_err());
        assert!(parse_bbox("1,2,3,4,5").is_err());
        assert!(parse_bbox("a,b,c,d").is_err());
    }

    #[test]
    fn element_without_geometry_is_skipped() {
        assert!(element_to_feature(&el("urn:x", None)).is_none());
        assert!(element_to_feature(&el("urn:x", Some("garbage"))).is_none());
    }

    #[test]
    fn timestamp_is_rfc3339_not_epoch_seconds() {
        // OGC API – Features Core §7.15.4 types `timeStamp` as date-time (RFC-3339).
        // A bare epoch-seconds value is non-conformant; assert the format parses.
        let ts = now_rfc3339();
        assert!(
            chrono::DateTime::parse_from_rfc3339(&ts).is_ok(),
            "timeStamp must be RFC-3339, got {ts:?}"
        );
        // Must NOT be a bare integer (the previous, non-conformant behaviour).
        assert!(
            ts.parse::<u64>().is_err(),
            "timeStamp must not be bare epoch seconds, got {ts:?}"
        );
    }

    #[test]
    fn element_with_point_becomes_feature() {
        let mut e = el("http://ex/a", Some("POINT(5 51)"));
        e.label = Some("A".into());
        e.gltf_url = Some("http://files/a.glb".into());
        let f = element_to_feature(&e).unwrap();
        assert_eq!(f["type"], "Feature");
        assert_eq!(f["id"], "http://ex/a");
        assert_eq!(f["geometry"]["type"], "Point");
        assert_eq!(f["properties"]["label"], "A");
        assert_eq!(f["properties"]["gltf_url"], "http://files/a.glb");
    }

    #[test]
    fn bbox_filter_includes_and_excludes() {
        let els = vec![
            el("a", Some("POINT(5 51)")),  // inside
            el("b", Some("POINT(50 50)")), // outside
            el("c", None),                 // no geometry
        ];
        let filter = BBox {
            min_x: 4.0,
            min_y: 50.0,
            max_x: 6.0,
            max_y: 52.0,
        };
        let kept: Vec<&str> = els
            .iter()
            .filter(|e| {
                e.wkt4326
                    .as_deref()
                    .and_then(wkt_bbox)
                    .is_some_and(|bb| bb.intersects(&filter))
            })
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(kept, vec!["a"]);
    }

    #[test]
    fn extent_unions_all_features() {
        let els = vec![
            el("a", Some("POINT(0 0)")),
            el("b", Some("POINT(10 20)")),
            el("c", None),
        ];
        assert_eq!(features_extent(&els), Some([0.0, 0.0, 10.0, 20.0]));
        assert_eq!(features_extent(&[el("x", None)]), None);
    }

    #[test]
    fn local_name_extracts_fragment_or_segment() {
        assert_eq!(local_name("http://ex/path/Thing"), "Thing");
        assert_eq!(local_name("http://ex/ns#Thing"), "Thing");
        assert_eq!(local_name("urn:foo"), "urn:foo");
    }

    #[test]
    fn ogc_url_joins_cleanly() {
        assert_eq!(
            ogc_url("http://h", "/collections"),
            "http://h/api/ogc/collections"
        );
        assert_eq!(
            ogc_url("http://h/", "/collections"),
            "http://h/api/ogc/collections"
        );
    }
}
