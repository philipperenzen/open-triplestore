# GeoSPARQL

Full OGC GeoSPARQL 1.1 support via the GEOS C++ library. Store geometry data as WKT or GML literals and query it using standard spatial relation functions.

## Supported functions

`sf:intersects`, `sf:contains`, `sf:within`, `sf:overlaps`, `sf:touches`, `sf:crosses`, `sf:disjoint`, `sf:equals`, `geof:distance`, `geof:buffer`, `geof:convexHull`, `geof:envelope`, `geof:union`, `geof:intersection`.

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
