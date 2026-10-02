# GeoSPARQL

OGC GeoSPARQL 1.1 support via the GEOS C++ library. Store geometry data as WKT, GML or GeoJSON literals and query it using standard spatial relation, measurement and aggregate functions. The grade is *Partial* — [Supported Standards](/docs/standards) lists what is not implemented — and this is not an OGC compliance certification.

## Geometry literals

Three serialisations are geometries, and every `geof:` function accepts any of them:

- `geo:wktLiteral`, with an optional CRS prefix (`"<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(187420 428470)"`); no prefix means CRS84.
- `geo:gmlLiteral` — the GML 3.2 geometry subset (points, curves, surfaces and their `Multi*` collections). Its CRS is the geometry's `srsName`, in any of the usual spellings (`EPSG:28992`, `urn:ogc:def:crs:EPSG::28992`, `http://www.opengis.net/gml/srs/epsg.xml#28992`, `http://www.opengis.net/def/crs/EPSG/0/28992`); no `srsName` means CRS84.
- `geo:geoJSONLiteral` — an RFC 7946 geometry object (`Point`, `MultiPoint`, `LineString`, `MultiLineString`, `Polygon`, `MultiPolygon`, `GeometryCollection`), always CRS84 longitude/latitude. A malformed one is not a geometry: functions over it are unbound.

Every function reads a literal's CRS the same way: the WKT prefix or the GML `srsName`. `getSRID` returns it, normalised to the `http://www.opengis.net/def/crs/…` form for GML. Constructive results and `geof:aggUnion` keep it, and binary functions (the sf/eh/rcc8 relations, `geof:relate`, `distance`, the set operations) transform the second operand into the first's CRS. Coordinates are in the CRS's axis order, for WKT and GML alike: EPSG:4326 is latitude first, CRS84 longitude first. An EPSG:4326 `<gml:pos>52 5</gml:pos>` is the same point as `"POINT(5 52)"^^geo:wktLiteral`.

An empty literal is the empty geometry: `""^^geo:wktLiteral` (also with a CRS prefix and nothing after it), `""^^geo:gmlLiteral` and `""^^geo:geoJSONLiteral`. An empty plain string is not a geometry.

`geof:asGeoJSON` serialises any geometry as a `geo:geoJSONLiteral`, reprojecting it to CRS84 on the way.

## Coordinate reference systems

The built-in CRSs are CRS84, EPSG:4326 (latitude first), RD New (EPSG:28992, and EPSG:7415 for its horizontal part) and Web Mercator (EPSG:3857). They are pure-Rust closed forms with no PROJ. Two geometries in the same CRS can always be compared, even an unknown one. Mixing an unknown CRS with any other gives an unbound result.

`geof:transform(g, crs)` reprojects to a built-in CRS, given as an IRI or an `xsd:anyURI` literal, and keeps Z. A transform is unbound when any coordinate lies outside a CRS's domain; it never copies a coordinate through unchanged. The domains are: latitude within ±90°, Web Mercator within ±85.06°, and RD New within EPSG:28992's area of use plus about 50 km (2.5°–8.0° E, 50.3°–54.2° N). The same rule applies wherever operands are harmonised.

## Supported functions

`sf:intersects`, `sf:contains`, `sf:within`, `sf:overlaps`, `sf:touches`, `sf:crosses`, `sf:disjoint`, `sf:equals`, `geof:distance`, `geof:buffer`, `geof:convexHull`, `geof:envelope`, `geof:union`, `geof:intersection`, `geof:asGeoJSON`, and the aggregate `geof:aggUnion`.

## Metres on the ellipsoid

The GeoSPARQL 1.1 metric functions measure on the WGS84 ellipsoid (Karney's geodesic algorithms), in metres or square metres, whatever CRS the operand is written in — it is reprojected first, and a CRS this build cannot reproject gives an unbound result:

| Function | Result |
|---|---|
| `geof:metricDistance(g1, g2)` | Shortest geodesic distance between the two geometries |
| `geof:metricLength(g)` | Geodesic length of the lines, and of a polygon's rings; 0 for points |
| `geof:metricPerimeter(g)` | Geodesic length of a polygon's rings, holes included; for a non-areal geometry its length (0 for points) |
| `geof:metricArea(g)` | Geodesic area; 0 for non-polygons |
| `geof:metricBuffer(g, r)` | A buffer of `r` metres, returned in `g`'s CRS |

### Units of measure

A unit argument is an IRI or an `xsd:anyURI` literal naming one of these units:

| Kind | OGC (`http://www.opengis.net/def/uom/OGC/1.0/`) | QUDT (`http://qudt.org/vocab/unit/`) | EPSG (`http://www.opengis.net/def/uom/EPSG/0/`) |
|---|---|---|---|
| Linear | `metre`, `kilometre`, `centimetre`, `millimetre` | `M`, `KiloM`, `CentiM`, `MilliM`, `FT`, `MI`, `MI_N` | `9001` (metre), `9002` (foot), `9036` (kilometre) |
| Angular | `degree`, `radian` | `DEG`, `RAD` | `9102` (degree), `9101` (radian) |
| Area | — | `M2`, `KiloM2`, `HA` | — |
| Dimensionless | `unity` | `UNITLESS` | — |

The unit argument of `geof:distance` and `geof:buffer` follows the CRS of the (first) operand:

| CRS | Linear unit | Angular unit | No unit, or `unity` |
|---|---|---|---|
| Geographic (CRS84, EPSG:4326) | Geodesic, as the metric functions | Planar degrees, converted | Planar degrees |
| Projected (RD New, Web Mercator) | Planar in the CRS's metres, converted | Unbound | Planar CRS units |
| A CRS this build cannot reproject | Unbound | Unbound | Planar CRS units |

`geof:area(g, unit)` takes an area unit. On a geographic CRS it gives the geodesic area (as `geof:metricArea`); on a projected CRS, the planar area in the CRS's square metres, converted. Any other unit is unbound.

An unknown unit is unbound, and so is an area unit for a distance. The function never falls back to the CRS's own units, which once returned degrees as if they were metres.

## Topology

`geof:ehCoveredBy` uses the DE-9IM mask GeoSPARQL gives it, `TFF*TFT**`, the exact inverse of `ehCovers`. A geometry strictly inside another, not touching its boundary, is `ehInside`, not `ehCoveredBy`. A line along a polygon's boundary is neither.

## The union aggregate

`geof:aggUnion` is a SPARQL aggregate: it folds the geometries of a group into their union, one `geo:wktLiteral`, with or without `GROUP BY` (and in `HAVING`, sub-selects and the `WHERE` of an update):

```sparql
PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>

SELECT ?municipality (geof:aggUnion(?geom) AS ?footprint) WHERE {
  ?parcel <https://example.org/municipality> ?municipality ;
          geo:hasGeometry/geo:asWKT ?geom .
} GROUP BY ?municipality
```

A group in one CRS keeps it; a group mixing CRSs is unioned in CRS84, each geometry reprojected first. It follows SPARQL's aggregate error rules — a value that is not a geometry makes that group's union unbound, as a non-number does to `SUM` — and the union of no geometries is the empty geometry, `GEOMETRYCOLLECTION EMPTY`. `geof:aggUnion(DISTINCT ?g)` is a syntax error (the SPARQL parser takes no `DISTINCT` in a custom aggregate's call); it would change nothing, since a union absorbs duplicates.

## Example query

```sparql
PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>

SELECT ?feature ?geom WHERE {
  ?feature geo:hasGeometry ?g .
  ?g geo:asWKT ?geom .
  FILTER(geof:sfIntersects(?geom,
    "POLYGON((4.8 52.3, 5.0 52.3, 5.0 52.4, 4.8 52.4, 4.8 52.3))"^^geo:wktLiteral))
}
```

Geometry is typically attached with the GeoSPARQL blank-node shape — see the instance-data example in [Linked Data Modelling](/docs/modelling).

## Query rewrite

A triple pattern with one of the 24 topological relations as its predicate matches the relation where the geometries imply it, not only where a triple asserts it. This is the Query Rewrite Extension (OGC 11-052r4 Req 28–30, 22-047r1 §13). It is on by default:

```sparql
PREFIX geo: <http://www.opengis.net/ont/geosparql#>

SELECT ?park WHERE {
  ?park a <https://example.org/Park> ;
        geo:sfWithin <https://example.org/city> .
}
```

The relations are `geo:sfEquals`, `sfDisjoint`, `sfIntersects`, `sfTouches`, `sfCrosses`, `sfWithin`, `sfContains`, `sfOverlaps`, `ehEquals`, `ehDisjoint`, `ehMeet`, `ehOverlap`, `ehCovers`, `ehCoveredBy`, `ehInside`, `ehContains`, `rcc8eq`, `rcc8dc`, `rcc8ec`, `rcc8po`, `rcc8tppi`, `rcc8tpp`, `rcc8ntpp` and `rcc8ntppi`. Each is decided by the `geof:` function of the same name.

The rules follow the standard strictly:

- A side of the relation is either a feature, reached through `geo:hasDefaultGeometry`, or a geometry itself. A feature with a geometry only through `geo:hasGeometry` is not related. Its geometry still is.
- The geometry's literal is read from `geo:asWKT`, `geo:asGML`, `geo:asGeoJSON` or `geo:asKML`. Two serialisations of one geometry, or a relation that is both asserted and derived, give one match: results are the set a materialised relation would give.
- That covers the standard's four rule shapes, feature–feature, feature–geometry, geometry–feature and geometry–geometry. An asserted relation triple still matches, with or without geometries.
- Inside `GRAPH`, both geometries must be in that graph.

What is rewritten: triple patterns with a constant relation predicate, in queries (`SELECT`, `ASK`, `CONSTRUCT`, `DESCRIBE`, sub-queries, `OPTIONAL`, `MINUS`, `FILTER EXISTS`) and in the `WHERE` clause of `DELETE`/`INSERT` updates. An update can therefore materialise a derived relation:

```sparql
PREFIX geo: <http://www.opengis.net/ont/geosparql#>

INSERT { GRAPH <https://example.org/relations> { ?a geo:sfWithin ?b } }
WHERE  { ?a a <https://example.org/Park> ; geo:sfWithin ?b }
```

What is left alone: a variable predicate (`?a ?p ?b`, as the standard allows), property paths (`geo:sfWithin+`), `SERVICE` blocks, which the remote endpoint evaluates, and `INSERT DATA`/`DELETE DATA`. A blank node in a rewritten pattern is treated as the variable it stands for.

Cost: a query that names no relation is not parsed again. A relation pattern with both sides unbound compares every pair of geometries in scope, so bind one side (by type or by IRI) on large data.

`OTS_GEOSPARQL_QUERY_REWRITE=off` (or `0`, `false`, `no`) turns the rewrite off for the server. Relation patterns then match asserted triples only.

## RDFS entailment

The RDFS Entailment Extension (OGC 11-052r4 Req 25–27, 22-047r1 §12) reasons over the GeoSPARQL ontology and two geometry class hierarchies:

- **Simple Features**: OGC's `sf_geometries.ttl` (registry entry `sf`), bundled unchanged: `sf:Polygon` ⊑ `sf:Surface` ⊑ `sf:Geometry` ⊑ `geo:Geometry`, `sf:Triangle` ⊑ `sf:Polygon`, and so on.
- **GML 3.2.1**: registry entry `gml-geometries`. OGC no longer publishes this hierarchy as RDF, so Open Triplestore wrote it from the GML 3.2.1 schema at `https://schemas.opengis.net/gml/3.2.1/`. Each GML geometry element is a class, and `rdfs:subClassOf` follows the element's substitution group, so `gml:Polygon` ⊑ `gml:AbstractSurface` ⊑ `gml:AbstractGeometricPrimitive` ⊑ `gml:AbstractGeometry` ⊑ `geo:Geometry`. The GeoSPARQL 1.0 text gives `gml:Polygon` ⊑ `gml:SurfacePatch` as an example. That does not match the schema, so the file follows the schema. Surface patches, curve segments and `gml:Envelope` are not geometries in GML 3.2.1 and have no class. The file's header lists the schema files it was derived from.

A dataset whose graphs use any GeoSPARQL term gets all three as reasoning premises. It does not need to declare conformance to them. A GeoSPARQL term here is a GeoSPARQL property used as a predicate, or a GeoSPARQL, SF or GML class used as a type or specialised with `rdfs:subClassOf`. `GET /api/datasets/{id}/conformance` lists them under `vocabulary_premises`. With the dataset's entailment set to `rdfs` and `materialize` (see [Reasoning](/docs/reasoning)), queries with `?entailment_dataset={id}` see, for example, `?f a geo:Feature` for every `?f geo:hasGeometry ?g`, `?g geo:hasSerialization ?wkt` for every `geo:asWKT`, and `?p a sf:Surface` for every `sf:Polygon`.
