# 3D Linked-Data Geospatial Platform

Open Triplestore extends its spec-compliant **GeoSPARQL 1.1** core (2D/2.5D) with
an **additive, namespaced 3D layer** — volumetric geometry, an OGC API – Features
facade, a 3D Tiles tiling plane, and a CesiumJS viewer with click-to-SPARQL.

> **Conformance posture.** Every 3D capability is additive and lives under its own
> namespace (`ots-geof:`) and its own serialisations, so GeoSPARQL 1.1's `geof:`
> functions and `geo:wktLiteral` semantics stay byte-for-byte conformant. A client
> that knows only GeoSPARQL 1.1 keeps working; a 3D-aware client opts in. The full
> OGC GeoSPARQL 1.1 suite (101 tests) runs green with the 3D layer enabled.

## 1. 3D geometry engine (`ots-geof:`)

A `Geometry3D` type system (`POINT Z`, `LINESTRING Z`, `POLYGON Z`,
`POLYHEDRALSURFACE Z`, `TIN Z`, `SOLID`, `MULTI*`, modelled on ISO 19107 /
CityGML) parses the ISO-13249 **WKT-Z** forms that the 2D GEOS path leaves alone.

3D SPARQL functions are registered under
`https://open-triplestore.org/def/function/geo3d/` (prefix `ots-geof:`):

| Family | Functions |
|---|---|
| Metric | `distance3d`, `volume`, `area3d`, `zMin`, `zMax`, `height` |
| Constructive | `boundingBox3d`, `centroid3d`, `footprint2d`, `extrude` |
| Topological (AABB broad-phase) | `sf3dIntersects`, `sf3dDisjoint` |

```sparql
PREFIX geo:      <http://www.opengis.net/ont/geosparql#>
PREFIX ots-geof: <https://open-triplestore.org/def/function/geo3d/>

# Volume and height of every volumetric building
SELECT ?b (ots-geof:volume(?w) AS ?m3) (ots-geof:height(?w) AS ?h)
WHERE { ?b geo:hasGeometry/geo:asWKT ?w .
        FILTER(CONTAINS(STR(?w), "POLYHEDRALSURFACE")) }
```

Volumetric literals: `geo:asWKT` (`POLYHEDRALSURFACE Z`, for 1.1 clients) plus a
loss-free `ots:cityjsonGeometryLiteral`. A 3D R*-tree (`[f64;3]` AABBs) backs the
two-phase broad/narrow query. CRS: `EPSG:28992` (RD New), `EPSG:7415` (RD New +
NAP), `CRS84`/`CRS84h`, Web Mercator; the kernel is pure-Rust (the exact
`parry3d`/SFCGAL solid algebra is gated behind a future feature). Enabled by the
`geometry3d` cargo feature (part of `full`).

## 2. CityJSON (3D BAG) ingestion

`POST /api/datasets/:id/ingest/cityjson` (multipart `file`, optional
`target_graph`/`public`; `?preview=true` dry-runs). `.city.json`/`.cityjson`
files dropped into `POST /api/import/bulk` are converted the same way.

The converter dequantises the shared vertex array, mints stable IRIs
(`{base}/dataset/{id}/bag/{identificatie}`, geometry `…/geom/lod{lod}`), emits BOT
parent/child topology + attributes, externalises geometry per LoD into **both**
`geo:asWKT POLYHEDRALSURFACE Z` (CRS-prefixed) **and** the loss-free
`ots:asCityJSON` literal, and records PROV-O lineage.

## 3. OGC API – Features (Core)

A thin, conformant facade over the SPARQL/Geo engine, under `/api/ogc`:

| Path | Returns |
|---|---|
| `GET /api/ogc` | Landing page |
| `GET /api/ogc/conformance` | Core + OAS30 + GeoJSON conformance classes |
| `GET /api/ogc/collections` | One collection per accessible dataset |
| `GET /api/ogc/collections/{id}/items` | GeoJSON `FeatureCollection` (`bbox`, `limit`, `offset`) |
| `GET /api/ogc/collections/{id}/items/{featureId}` | One feature by IRI |

GeoJSON (`application/geo+json`) is the mandatory encoding; CQL2 is out of scope
for now. Public datasets are reachable anonymously.

## 4. 3D Tiles + the binding contract

`GET /api/datasets/:id/3dtiles/tileset.json` and `…/3dtiles/content.glb` generate
**3D Tiles 1.1**: a glTF (GLB) carrying `EXT_mesh_features` (per-feature ids) and
`EXT_structural_metadata` (a property table with one STRING column **`iri`** =
the RDF subject = the viewer's lookup key). Geometry is triangulated by the 3D
engine and reprojected to **ECEF (EPSG:4978)**.

**The binding invariant:** every spatial object has one canonical IRI that is (a)
the RDF subject, (b) the `iri` property-table value in the tile, and (c) the key
the viewer sends back to `/sparql`. Get this contract right and the rest is
plumbing.

## 5. Viewers and gating

- **CesiumJS 3D-Tiles viewer** (`/datasets/:id/cesium`): streams the tileset;
  click a feature → read its `iri` from the tile metadata → `SELECT ?p ?o WHERE {
  <iri> ?p ?o }` in a side panel. Cesium's native globe/terrain depth handling
  means satellite imagery and 3D coexist correctly.
- **MapLibre + Three.js viewer** (`/datasets/:id/viewer`): the 2D map (located
  features as dots/lines/areas) with to-scale glTF/IFC/CityJSON/STL models.
- **Capability gating:** `GET /api/datasets/:id/geo-stats` returns
  `{has_coordinates, has_models, has_3d_geometry, has_3d, element_count}`. The
  dataset page shows a *3D & map viewer* tile when there's 3D data, a plain *Map
  viewer* when there are only coordinates, and nothing when there's no geometry;
  the table/graph data-explorer shows a **Map** tab only when the current result
  scope has mappable geometry.

## 6. Examples

The bundled **3D & Map Viewer Demo** dataset exercises every supported geo
feature/format: mixed 2D GeoSPARQL geometry (point/line/polygon), native WKT-Z
volumetric solids (`EPSG:7415`), 3D BAG LoD2.2 CityJSON (real + a small authored
sample), STL/glTF/IFC models, a SOSA/SSN sensor + observation layer, and an
OTL/IMBOR asset-alignment example (the NEN 2660-2 vocabulary itself is not
bundled — it is not freely redistributable).

## 7. Standards

OGC GeoSPARQL 1.1 · OGC CityJSON 2.0 / CityGML · OGC API – Features 1.0 · OGC 3D
Tiles 1.1 (glTF `EXT_mesh_features` / `EXT_structural_metadata`) · W3C BOT ·
OMG/FOG · W3C SOSA/SSN · ISO 19107 · EPSG 28992/7415/4978.
