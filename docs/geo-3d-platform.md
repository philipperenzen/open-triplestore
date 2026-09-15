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
| Validity | `isClosed3d` — watertightness: every edge shared by exactly two faces (unbound for face-less geometry) |

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
sample), STL/glTF/IFC models, and a SOSA/SSN sensor + observation layer.

## 7. Standards

OGC GeoSPARQL 1.1 · OGC CityJSON 2.0 / CityGML · OGC API – Features 1.0 · OGC 3D
Tiles 1.1 (glTF `EXT_mesh_features` / `EXT_structural_metadata`) · W3C BOT ·
OMG/FOG · W3C SOSA/SSN · ISO 19107 · EPSG 28992/7415/4978.

## 8. Embedding these viewers elsewhere

The map, 3D scene and Cesium globe are all available as chrome-less
`/embed/…` pages for iframing into external sites, and every underlying API
(GeoJSON, 3D Tiles, viewer feed, model files) is directly consumable from any
web app — see **Embedding & Web Apps** (`docs/embedding.md`).

## 9. IFC → linked data: the lift

An IFC file (`POST /api/datasets/:id/import/ifc`) becomes two graphs: the
**BOT layer** the platform queries, `…/dataset/{id}/building`, and, when
asked for, a complete **ifcOWL** instance lift beside it. The BOT layer is
the contract the viewer feed, the IDS importer and the SHACL Studio `ifc`
shapes read, and it is stable: `bot:Site/Building/Storey/Space/Element`,
`bot:hasBuilding/hasStorey/hasSpace/containsElement/hasSubElement`, the
schema's ifcOWL class on every node, `rdfs:label`, `props:ifcGuid` /
`props:uuid`, a `fog:asIfc_v4|v2x3` file link per element, the flat
`props:{Pset}_{Prop}` literal per property, and the WGS84 anchor (a bare
`POINT(lon lat)`, i.e. CRS84) on the site. Everything below is emitted
*beside* that, never instead of it — `tests/ifc_lift.rs` pins the contract
as exact triples.

**The lift's own vocabulary** lives at `{base_url}/ns/ifc-lift#`, under the
instance that serves the lifted graphs so every term is dereferenceable
there, and ships as the `ifc-lift` seed bundle (`examples/seed-bundles/ifc-lift`;
opt out with `SEED_IFC_LIFT=false`). The importer derives the namespace from
the graph IRI it writes into (`{base_url}/dataset/…` → `{base_url}`; a custom
target graph → its origin). A test holds the emitter to the ontology: every
lift-namespace term it produces is declared there.

| What | Emitted as |
|---|---|
| IFC 4.3 facilities (`IfcBridge`, `IfcRoad`, `IfcRailway`, `IfcMarineFacility` and their parts) | `bot:Zone` plus `ifcl:Bridge` … `ifcl:BridgePart`; zone-in-zone aggregation is `bot:containsZone` (with the feed edge). buildingSMART published ifcOWL up to IFC4 ADD2, so only the entities new in 4.3 are typed under the lift namespace (`ifcl:IfcAlignment`); every entity IFC4 already had keeps its IFC4 ifcOWL IRI, so shapes written for IFC4 — the IDS importer's output — match 4.3 models |
| Quantity sets (`IfcElementQuantity`: the seven simple kinds, complex quantities recursed) | `ifcl:hasQuantity` → a node with `qudt:numericValue`, `qudt:hasUnit`, `ifcl:quantitySet`, `ifcl:quantityKind`; plus the flat `props:{Qto}_{Name}` literal |
| Property values (single, enumerated, list, bounded, complex; the IFC4 set form of `RelatingPropertyDefinition`) | `ifcl:hasProperty` → a node with `ifcl:value`(s), `ifcl:ifcType` (the IFC measure type), the unit, the bounds of a bounded value; the flat literal stays |
| Units | QUDT 2.1 IRIs, from the project defaults (`IfcUnitAssignment`), a value's own unit, or the measure type's unit type. The prefix composes with the whole unit: KILO+GRAM is `unit:KiloGM`, MILLI+SQUARE_METRE is `unit:MilliM2`. Nothing is guessed — an unmapped unit is `ifcl:unitLabel`, a conversion factor against a mapped base when there is one, and a count in the import stats |
| Classification (`IfcRelAssociatesClassification`) | a `skos:Concept` per reference (notation, label, definition, `skos:broader` through the chain) in a `skos:ConceptScheme` per `IfcClassification`; absolute `Location`s are kept as the IRIs. The element gets `ifcl:hasClassification` and the flat `props:ifcClassification` literal the IDS importer targets |
| Material (`IfcRelAssociatesMaterial`, through layer, profile and constituent sets and material lists) | `ifcl:hasMaterial`, `nen2660:consistsOf`, `props:ifcMaterial` |
| Relations, beside every BOT edge | NEN 2660-2: containment is `nen2660:contains` (location, not parthood), spatial aggregation `nen2660:hasPart`, element decomposition and nesting `nen2660:hasTechnicalPart`, `IfcRelConnectsPathElements` / `IfcRelConnectsElements` `nen2660:connectsObject`. The `nen2660-relations` bundle's shapes run clean over a lifted model |
| Map conversion (`IfcMapConversion` → `IfcProjectedCRS`) | `ifcl:mapConversion` → a node with eastings, northings, height, `ifcl:mapRotation` (`atan2(XAxisOrdinate, XAxisAbscissa)`, degrees), scale, `ifcl:projectedCrs` and the datums. When the EPSG code is one the CRS registry knows (28992, 7415, 4326, 3857) the node is also a `geo:Geometry` whose `geo:asWKT` is the origin as a CRS-qualified point, and a site without its own `RefLatitude`/`RefLongitude` gets its WGS84 anchor from that origin reprojected; an unknown code gets no CRS prefix. Read, never applied — the geometry a viewer receives is already placed (the TrueNorth argument in `src/ifc/rdf.rs` applies verbatim) |

Not lifted: type-object property sets (`IfcRelDefinesByType`), derived units
(`IfcDerivedUnit` is always unmapped), and `IfcRelNests` as distinct from
aggregation (both are parthood here). No IFC 4.3 model exists in the
repository or the boot seed, so 4.3 behaviour is verified against the
hand-authored fixtures in `tests/fixtures/ifc` only.
