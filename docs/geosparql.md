# GeoSPARQL

OGC GeoSPARQL 1.1 support via the GEOS C++ library. Store geometry data as WKT, GML or GeoJSON literals and query it using standard spatial relation, measurement and aggregate functions. The grade is *Partial* — [Supported Standards](/docs/standards) lists what is not implemented.

## Geometry literals

Three serialisations are geometries, and every `geof:` function accepts any of them:

- `geo:wktLiteral`, with an optional CRS prefix (`"<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(187420 428470)"`); no prefix means CRS84.
- `geo:gmlLiteral` — the GML 3.2 geometry subset (points, curves, surfaces and their `Multi*` collections).
- `geo:geoJSONLiteral` — an RFC 7946 geometry object (`Point`, `MultiPoint`, `LineString`, `MultiLineString`, `Polygon`, `MultiPolygon`, `GeometryCollection`), always CRS84 longitude/latitude. A malformed one is not a geometry: functions over it are unbound.

`geof:asGeoJSON` serialises any geometry as a `geo:geoJSONLiteral`, reprojecting it to CRS84 on the way.

## Supported functions

`sf:intersects`, `sf:contains`, `sf:within`, `sf:overlaps`, `sf:touches`, `sf:crosses`, `sf:disjoint`, `sf:equals`, `geof:distance`, `geof:buffer`, `geof:convexHull`, `geof:envelope`, `geof:union`, `geof:intersection`, `geof:asGeoJSON`, and the aggregate `geof:aggUnion`.

## Metres on the ellipsoid

The GeoSPARQL 1.1 metric functions measure on the WGS84 ellipsoid (Karney's geodesic algorithms), in metres or square metres, whatever CRS the operand is written in — it is reprojected first, and a CRS this build cannot reproject gives an unbound result:

| Function | Result |
|---|---|
| `geof:metricDistance(g1, g2)` | Shortest geodesic distance between the two geometries |
| `geof:metricLength(g)` | Geodesic length of the lines, and of a polygon's rings; 0 for points |
| `geof:metricPerimeter(g)` | Geodesic length of a polygon's rings, holes included; 0 for non-polygons |
| `geof:metricArea(g)` | Geodesic area; 0 for non-polygons |
| `geof:metricBuffer(g, r)` | A buffer of `r` metres, returned in `g`'s CRS |

The unit argument of `geof:distance` and `geof:buffer` follows the CRS of the (first) operand:

| CRS | Linear unit (`uom:metre`, `kilometre`, …) | Angular unit (`uom:degree`, `radian`) | No unit |
|---|---|---|---|
| Geographic (CRS84, EPSG:4326) | Geodesic, as the metric functions | Planar degrees, converted | Planar degrees |
| Projected (RD New, Web Mercator) | Planar in the CRS's metres, converted | Planar CRS units | Planar CRS units |

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
