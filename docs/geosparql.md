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
