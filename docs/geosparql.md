# GeoSPARQL

OGC GeoSPARQL 1.1 support via the GEOS C++ library. Store geometry data as WKT, GML, GeoJSON or KML literals and query it using standard spatial relation, measurement and aggregate functions. GeoSPARQL 1.0 is graded *Full*, and 1.1 *Full* for every conformance class but the optional DGGS class (no DGGS literals) — see [Supported Standards](/docs/standards). These are the project's own grades, not an OGC compliance certification.

## Geometry literals

Four serialisations are geometries, and every `geof:` function accepts any of them:

- `geo:wktLiteral`, with an optional CRS prefix (`"<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(187420 428470)"`); no prefix means CRS84.
- `geo:gmlLiteral` — a GML 3.2 geometry of the [supported profile](#supported-gml-profile). Its CRS is the geometry's `srsName`, in any of the usual spellings (`EPSG:28992`, `urn:ogc:def:crs:EPSG::28992`, `http://www.opengis.net/gml/srs/epsg.xml#28992`, `http://www.opengis.net/def/crs/EPSG/0/28992`); no `srsName` means CRS84.
- `geo:geoJSONLiteral` — an RFC 7946 geometry object (`Point`, `MultiPoint`, `LineString`, `MultiLineString`, `Polygon`, `MultiPolygon`, `GeometryCollection`), always CRS84 longitude/latitude. An altitude on every position is kept as Z. A malformed one is not a geometry: functions over it are unbound.
- `geo:kmlLiteral` — a KML 2.2 geometry element: `Point`, `LineString`, `LinearRing`, `Polygon` (`outerBoundaryIs`, `innerBoundaryIs`) or `MultiGeometry`, possibly inside a `Placemark`. Coordinates are `lon,lat[,alt]` tuples, always CRS84; an altitude on every tuple is kept as Z. `Model`, `gx:Track` and `gx:MultiTrack` are outside the profile, and malformed KML is not a geometry.

Every function reads a literal's CRS the same way: the WKT prefix or the GML `srsName`. `getSRID` returns it, normalised to the `http://www.opengis.net/def/crs/…` form for GML. Constructive results and `geof:aggUnion` keep it, and binary functions (the sf/eh/rcc8 relations, `geof:relate`, `distance`, the set operations) transform the second operand into the first's CRS. Coordinates are in the CRS's axis order, for WKT and GML alike: EPSG:4326 is latitude first, CRS84 longitude first. An EPSG:4326 `<gml:pos>52 5</gml:pos>` is the same point as `"POINT(5 52)"^^geo:wktLiteral`.

An empty literal is the empty geometry: `""^^geo:wktLiteral` (also with a CRS prefix and nothing after it), `""^^geo:gmlLiteral` and `""^^geo:geoJSONLiteral`. An empty plain string is not a geometry.

### Serialisation functions

| Function | Result |
|---|---|
| `geof:asWKT(g)` | A `geo:wktLiteral` in `g`'s CRS (a GML `srsName` becomes the WKT prefix), Z kept |
| `geof:asGML(g [, profile])` | A `geo:gmlLiteral` of the profile below, with `g`'s CRS as `srsName` (CRS84 written out when `g` names none) and `srsDimension="3"` for Z. Any `profile` string is accepted; the output is always this GML 3.2 profile |
| `geof:asGeoJSON(g)` | A `geo:geoJSONLiteral`, reprojected to CRS84, Z as altitude |
| `geof:asKML(g)` | A `geo:kmlLiteral`, reprojected to CRS84, Z as altitude |

A function that returns a geometry (`buffer`, `union`, `envelope`, `transform` and the rest) returns it in the serialisation and CRS of its first operand, as GeoSPARQL 1.1 §10.9.1 says: the buffer of a GML literal in RD New is a GML literal with the same `srsName`. GeoJSON and KML exist only in CRS84, so a GeoJSON or KML operand transformed into another CRS comes back as a WKT literal. `geof:aggUnion` returns the group's serialisation when every value shares one, and a WKT literal otherwise. The empty geometry is an empty `Multi*` element in GML (so it keeps its `srsName`) and the empty literal in KML.

## Supported GML profile

GeoSPARQL 1.1 (Req 22) asks an implementation to document the GML it reads. A `geo:gmlLiteral` is the first GML 3.2 geometry element of its value (GML 2's `outerBoundaryIs`, `innerBoundaryIs`, `coordinates` and `coord` are read too):

| GML element | Read as |
|---|---|
| `Point` | `POINT` |
| `LineString`, a standalone `LinearRing` | `LINESTRING` |
| `Curve` of `LineStringSegment`s, `OrientableCurve`, `CompositeCurve` | `LINESTRING`: segments and members joined end to start, a reversed (`orientation="-"`) `OrientableCurve` reversed |
| `Polygon`, `PolygonPatch`, `Triangle`, `Rectangle` | `POLYGON`. A ring is a `LinearRing` or a `Ring` of linear `curveMember`s that meet end to start, closed, with four or more positions |
| `Surface` | its one patch as a `POLYGON`; several patches as a `MULTIPOLYGON` (each patch a polygon, not a hole) |
| `PolyhedralSurface`, `Tin`, `TriangulatedSurface`, `CompositeSurface` | `MULTIPOLYGON` of the patches or members |
| `OrientableSurface` | its base surface |
| `MultiPoint`, `MultiCurve`/`MultiLineString`, `MultiSurface`/`MultiPolygon` | the `MULTI*` type; one with no members is `MULTI* EMPTY` |
| `MultiGeometry` | `GEOMETRYCOLLECTION` |
| `Envelope` | the `POLYGON` between `lowerCorner` and `upperCorner` (a degenerate envelope is its point or line) |

Coordinates come from `gml:pos`, `gml:posList`, `gml:coordinates` (with its `cs`, `ts` and `decimal` attributes), `gml:pointProperty`/`gml:pointRep` and GML 2's `gml:coord`; text in any other element (`gml:name`, `gml:description`) is not a coordinate. `srsDimension="3"`, on the geometry or on a `pos`/`posList`, is kept as Z; a `gml:pos` without one is 2D or 3D by its number count. A `posList` without one is 2D.

Outside the profile, and so not a geometry (functions over it are unbound): arcs and other curved segments (`Arc`, `ArcString`, `Circle`, `ArcByCenterPoint`, splines, clothoids), curved patches (`Cone`, `Cylinder`, `Sphere`), `Solid`, `CompositeSolid`, `MultiSolid`, any `srsDimension` but 2 and 3, a `posList` whose numbers do not divide into positions, a member that does not read (an unresolved `xlink:href` included), and nesting deeper than 32 elements. Arcs are refused rather than read as straight lines between their control points, which would be a different shape.

## Coordinate reference systems

The built-in CRSs are CRS84, EPSG:4326 (latitude first), RD New (EPSG:28992, and EPSG:7415 for its horizontal part) and Web Mercator (EPSG:3857). They are pure-Rust closed forms with no PROJ. Two geometries in the same CRS can always be compared, even an unknown one. Mixing an unknown CRS with any other gives an unbound result.

`geof:transform(g, crs)` reprojects to a built-in CRS, given as an IRI or an `xsd:anyURI` literal, and keeps Z. A transform is unbound when any coordinate lies outside a CRS's domain; it never copies a coordinate through unchanged. The domains are: latitude within ±90°, Web Mercator within ±85.06°, and RD New within EPSG:28992's area of use plus about 50 km (2.5°–8.0° E, 50.3°–54.2° N). The same rule applies wherever operands are harmonised.

## Supported functions

The service description (`GET /sparql` with `Accept: text/turtle`) lists every registered `geof:` function and aggregate; it is generated from the same registry the engine installs.

- Topology: `geof:sfEquals`, `sfDisjoint`, `sfIntersects`, `sfTouches`, `sfCrosses`, `sfWithin`, `sfContains`, `sfOverlaps`; `ehEquals`, `ehDisjoint`, `ehMeet`, `ehOverlap`, `ehCovers`, `ehCoveredBy`, `ehInside`, `ehContains`; `rcc8eq`, `rcc8dc`, `rcc8ec`, `rcc8po`, `rcc8tppi`, `rcc8tpp`, `rcc8ntpp`, `rcc8ntppi`; `geof:relate`.
- Constructive: `geof:boundary`, `buffer`, `convexHull`, `concaveHull`, `boundingCircle`, `centroid`, `difference`, `envelope`, `intersection`, `symDifference`, `union`, `transform`, `geometryN`.
- Measures: `geof:distance`, `area`, `length`, `perimeter`, `getSRID`, `minX`, `maxX`, `minY`, `maxY`, `minZ`, `maxZ`, and the metric family below.
- Geometry properties: `geof:dimension`, `coordinateDimension`, `spatialDimension`, `is3D`, `isMeasured`, `isEmpty`, `isSimple`, `geometryType`, `numGeometries`.
- Serialisation: `geof:asWKT`, `asGML`, `asGeoJSON`, `asKML`.
- Aggregates: `geof:aggUnion`, `aggBoundingBox`, `aggBoundingCircle`, `aggCentroid`, `aggConvexHull`, `aggConcaveHull`.

### Geometry properties and the other non-topological functions

| Function | Result |
|---|---|
| `geof:dimension(g)` | Topological dimension (`xsd:integer`): 0 points, 1 curves, 2 surfaces, a collection's largest; unbound for an empty collection |
| `geof:coordinateDimension(g)` | Numbers per position: 2, 3 with Z or M, 4 with both |
| `geof:spatialDimension(g)` | 2, or 3 with Z |
| `geof:is3D(g)`, `geof:isMeasured(g)` | Whether positions have Z, M. M is read from the WKT (`POINT M`, `POINT ZM`); GML, GeoJSON and KML have none |
| `geof:isEmpty(g)`, `geof:isSimple(g)` | Empty; free of self-intersection and self-tangency |
| `geof:geometryType(g)` | The class as an IRI: the GML element for a GML literal (`gml:Surface`, `gml:Envelope`, in `http://www.opengis.net/ont/gml#`), the Simple Features class otherwise (`sf:Polygon`, in `http://www.opengis.net/ont/sf#`) |
| `geof:numGeometries(g)`, `geof:geometryN(g, n)` | A collection's member count, and its `n`th member counting from 1 (a geometry that is not a collection is its own first member); `geometryN` is unbound out of range |
| `geof:minX(g)` … `geof:maxZ(g)` | The extreme ordinate (`xsd:double`) in the literal's CRS; unbound for the empty geometry, and for Z on a 2D geometry |
| `geof:centroid(g)` | The centroid (the empty point for the empty geometry) |
| `geof:boundingCircle(g)` | The minimum bounding circle of the positions (Welzl's algorithm), as a 64-segment polygon drawn *around* the circle so it covers every position; a single position is its own point |
| `geof:concaveHull(g, targetPercent)` | A concave hull without holes (GEOS 3.11 `GEOSConcaveHull`). `targetPercent` runs from 0, the most concave hull, to 1, the convex hull, as in GEOS and PostGIS 3.3+; outside that range the result is unbound. **Default 0.5** when it is left out |
| `geof:length(g, unit)`, `geof:perimeter(g, unit)` | The length of the lines and a polygon's rings; a non-areal geometry's perimeter is its length, so the two agree, as `metricLength` and `metricPerimeter` do. Units as for `geof:distance` below |

`getSRID` and `geometryType` return IRIs, not `xsd:anyURI` literals.

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

## Aggregates

The six GeoSPARQL 1.1 spatial aggregates are SPARQL aggregates, with or without `GROUP BY` (and in `HAVING`, sub-selects and the `WHERE` of an update). Each folds the geometries of a group into one geometry:

| Aggregate | Result |
|---|---|
| `geof:aggUnion(?g)` | The union |
| `geof:aggBoundingBox(?g)` | The envelope of all of them |
| `geof:aggBoundingCircle(?g)` | Their minimum bounding circle, as `geof:boundingCircle` |
| `geof:aggCentroid(?g)` | The centroid of the group as one collection: a geometry the group holds twice counts twice, and only the highest dimension counts |
| `geof:aggConvexHull(?g)` | The convex hull |
| `geof:aggConcaveHull(?g)` | The concave hull at the **default target 0.5**, as `geof:concaveHull` without a target |

`geof:aggConcaveHull` takes one argument. GeoSPARQL 1.1 gives it a second, `targetPercent`, but the SPARQL parser this store uses allows one expression in a custom aggregate call. For another target, use `geof:concaveHull(geof:aggUnion(?g), 0.2)`.

```sparql
PREFIX geo: <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>

SELECT ?municipality (geof:aggUnion(?geom) AS ?footprint) WHERE {
  ?parcel <https://example.org/municipality> ?municipality ;
          geo:hasGeometry/geo:asWKT ?geom .
} GROUP BY ?municipality
```

A group in one CRS keeps it; a group mixing CRSs is combined in CRS84, each geometry reprojected first. The result has the group's serialisation when every value shares one, and is a `geo:wktLiteral` otherwise. Every aggregate follows SPARQL's aggregate error rules — a value that is not a geometry makes that group's result unbound, as a non-number does to `SUM` — and the aggregate of no geometries is the empty geometry, `GEOMETRYCOLLECTION EMPTY`. The result does not depend on the order the solutions arrive in.

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
