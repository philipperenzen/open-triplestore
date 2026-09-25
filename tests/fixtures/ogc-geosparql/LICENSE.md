# Licence of the vendored OGC GeoSPARQL 1.1 files

`validator.ttl` and `examples/*.ttl` are unmodified copies from the Open
Geospatial Consortium's
[opengeospatial/ogc-geosparql](https://github.com/opengeospatial/ogc-geosparql)
repository at commit `523098e714bb077a800f048be2940942b66310c8` (see
`PROVENANCE.md`). They are third-party material: Open Triplestore's
AGPL-3.0 + Commons Clause licence does not apply to them.

## Apache License 2.0 — the licence relied on

The upstream repository has no licence file. The
[License section of its README](https://github.com/opengeospatial/ogc-geosparql/blob/523098e714bb077a800f048be2940942b66310c8/README.md#license)
at that commit licenses the repository's software and data under the Apache
Software License 2.0, per the OGC's policy on
[Licensing for Geospatial Software](https://www.ogc.org/about/policies/software-licenses/)
(OGC documents, such as the Standard itself, fall under the OGC Document
License Agreement instead).

These files are redistributed under the Apache License 2.0. Its full text is in
[`LICENSE-Apache-2.0.txt`](LICENSE-Apache-2.0.txt), verbatim from
<https://www.apache.org/licenses/LICENSE-2.0.txt>. The files are unmodified and
upstream ships no NOTICE file.

## `validator.ttl`: its own licence and rights metadata

`validator.ttl` also names a licence and a rights holder in its own metadata,
kept unchanged in the file:

```turtle
dcterms:license "https://www.ogc.org/license"^^xsd:anyURI ;
dcterms:rights "(c) 2022 Open Geospatial Consortium" ;
```

<https://www.ogc.org/license> redirects to the OGC Document License Agreement,
<https://www.ogc.org/about/policies/document-license-agreement/>. The OGC's
current copy of the validator declares Apache-2.0 instead (see
`PROVENANCE.md`).

Running these files is not an OGC compliance certification: only the OGC may
authorise marks that indicate compliance with its standards.
