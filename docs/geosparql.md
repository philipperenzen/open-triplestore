# GeoSPARQL

OGC GeoSPARQL 1.1 support via the GEOS C++ library. Store geometry data as WKT, GML or GeoJSON literals and query it using standard spatial relation functions. The grade is *Partial* — [Supported Standards](/docs/standards) lists what is not implemented.

## Geometry literals

Three serialisations are geometries, and every `geof:` function accepts any of them:

- `geo:wktLiteral`, with an optional CRS prefix (`"<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(187420 428470)"`); no prefix means CRS84.
- `geo:gmlLiteral` — the GML 3.2 geometry subset (points, curves, surfaces and their `Multi*` collections).
- `geo:geoJSONLiteral` — an RFC 7946 geometry object (`Point`, `MultiPoint`, `LineString`, `MultiLineString`, `Polygon`, `MultiPolygon`, `GeometryCollection`), always CRS84 longitude/latitude. A malformed one is not a geometry: functions over it are unbound.

`geof:asGeoJSON` serialises any geometry as a `geo:geoJSONLiteral`, reprojecting it to CRS84 on the way.

## Supported functions

`sf:intersects`, `sf:contains`, `sf:within`, `sf:overlaps`, `sf:touches`, `sf:crosses`, `sf:disjoint`, `sf:equals`, `geof:distance`, `geof:buffer`, `geof:convexHull`, `geof:envelope`, `geof:union`, `geof:intersection`, `geof:asGeoJSON`.

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
