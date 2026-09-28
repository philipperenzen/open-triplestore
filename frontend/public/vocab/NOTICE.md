# Vocabularies in this directory — attribution and licences

The Turtle files in this directory are standard vocabularies that Open Triplestore
bundles. The web UI serves them at `/vocab/<file>` to show term definitions, and
the server compiles them in (`src/data_models/seed_vocab.rs`) and loads them into
the store as public, read-only reference models (`/api/models`).

Every file except `ots.ttl` and the Open Triplestore parts of `xsd.ttl` is
third-party work and stays under its publisher's licence, listed below. The Open
Triplestore licence (AGPL-3.0 with the Commons Clause, see `LICENSE`) does not
apply to them. The store keeps triples, not comments, so a file's header does not
survive the load. The server therefore records each seeded copy's licence and
attribution as registry metadata, returned as `attribution` by `/api/models` and
shown on the model's page. Downloads from `/api/models/{id}/…/data` start with
the file's header as `#` comments, then a line saying whether the content is
that file's triples, unchanged, or a copy made or edited in the registry that
may have been modified. They name the licence in `Link` headers (`imbor.ttl`:
`Link` headers only, nothing added to the copy). The seeded graphs hold each
file's triples, which the seeder checks against the file. The store writes
typed literals in its canonical form, with the same values: some
`xsd:nonNegativeInteger` literals in `dcat.ttl`, `dcat/2.0.0.ttl`, `prov.ttl`,
`time.ttl`, `vcard.ttl`, `ssn.ttl`, `bibo.ttl`, `saref.ttl` and `locn.ttl` read
as `xsd:integer`, and two `+00:00` time zones in `pav.ttl` read as `Z`.
`imbor.ttl` has no typed literals and is held exactly. The `imbor` entry accepts
no upload, edit, draft, branch, merge, rebase or publish. The server also serves this file at
`/vocab/NOTICE.md`. This file and the root `NOTICE` carry the full attribution
and licence texts.

How to read the entries:

- **Source** is where the content comes from. "Byte-identical" and "triples
  identical" were checked against that source on 2026-09-23.
- **Changes** lists every difference from the source. Open Triplestore has added a
  comment header to every file except `imbor.ttl`. Where the rest of a file is
  the upstream content, the header ends with a line marking where that starts.
- 17 files came from the Linked Open Vocabularies (LOV) corpus snapshot of
  2025-12-18 (`https://lov.linkeddata.es/lov.nq.gz`, sha256 `7b5522b4…c233`),
  and LOV re-serialized them as Turtle. LOV's own CC BY 4.0 licence covers LOV's
  catalogue. It does not relicense these vocabularies.
- Licence texts are in the appendices at the end of this file. Longer texts are
  in `LICENSES/` in the Open Triplestore source.

## W3C Document License (2023)

These are W3C documents. None of them states its own licence, so W3C's default
applies: the W3C Document License,
https://www.w3.org/copyright/document-license-2023/ (full text: appendix A).
That licence lets anyone copy and distribute a document if every copy carries a
link to the original, the copyright notice and, where one exists, the document's
status. It allows derivative works only in software, in supporting materials for
software and in software documentation, and only with the notice it prescribes.
Open Triplestore's copies are such derivatives, because each adds a comment
header (some are also LOV's Turtle re-serializations, which change the format,
not the triples). So each file's header carries the notice below, plus the
fallback copyright notice the licence prescribes for documents without their
own:

> Copyright © 2023 W3C®. This software or document includes material copied from or derived from [title and URI of the W3C document].

### `rdf.ttl` — The RDF Concepts Vocabulary (RDF)
- **Version:** namespace document of `http://www.w3.org/1999/02/22-rdf-syntax-ns#`, dc:date 2019-12-16
- **Source:** https://www.w3.org/1999/02/22-rdf-syntax-ns.ttl
- **Status:** defined in RDF 1.1 Concepts and Abstract Syntax, W3C Recommendation 25 February 2014, https://www.w3.org/TR/2014/REC-rdf11-concepts-20140225/
- **Copyright:** Copyright © 2019 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** comment header added; otherwise byte-identical

### `rdfs.ttl` — The RDF Schema vocabulary (RDFS)
- **Version:** namespace document of `http://www.w3.org/2000/01/rdf-schema#`. It states no date; the notice uses 2014, the year of RDF Schema 1.1.
- **Source:** https://www.w3.org/2000/01/rdf-schema.ttl
- **Status:** defined in RDF Schema 1.1, W3C Recommendation 25 February 2014, https://www.w3.org/TR/2014/REC-rdf-schema-20140225/
- **Copyright:** Copyright © 2014 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** comment header added; otherwise byte-identical

### `owl.ttl` — The OWL 2 Schema vocabulary (OWL 2)
- **Version:** namespace document of `http://www.w3.org/2002/07/owl#`, owl:versionInfo `$Date: 2009/11/15`
- **Source:** https://www.w3.org/2002/07/owl.ttl
- **Status:** defined in OWL 2 Web Ontology Language (Second Edition), W3C Recommendation 11 December 2012, https://www.w3.org/TR/2012/REC-owl2-overview-20121211/
- **Copyright:** Copyright © 2009 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** comment header added; line endings converted from CRLF to LF (the repository stores `*.ttl` with LF); otherwise identical

### `shacl.ttl` — W3C Shapes Constraint Language (SHACL) Vocabulary
- **Version:** "Version from 2017-07-20". The namespace URI `http://www.w3.org/ns/shacl` now serves a SHACL 1.2 draft, which is a different file.
- **Source:** https://www.w3.org/ns/shacl.ttl
- **Status:** defined in Shapes Constraint Language (SHACL), W3C Recommendation 20 July 2017, https://www.w3.org/TR/2017/REC-shacl-20170720/
- **Copyright:** Copyright © 2017 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** comment header added above the upstream's own two comment lines; otherwise byte-identical

### `prov.ttl` — W3C PROVenance Interchange Ontology (PROV-O)
- **Version:** Recommendation version 2013-04-30. This is PROV-O only, not the larger `https://www.w3.org/ns/prov.ttl`.
- **Source:** https://www.w3.org/ns/prov-o.ttl
- **Status:** defined in PROV-O: The PROV Ontology, W3C Recommendation 30 April 2013, https://www.w3.org/TR/2013/REC-prov-o-20130430/
- **Copyright:** Copyright © 2013 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** comment header added; otherwise byte-identical

### `skos.ttl` — SKOS Vocabulary (SKOS RDF schema)
- **Version:** SKOS Reference of 18 August 2009
- **Source:** http://www.w3.org/2004/02/skos/core.rdf, served as https://www.w3.org/2009/08/skos-reference/skos.rdf
- **Status:** normative RDF schema of SKOS Simple Knowledge Organization System Reference, W3C Recommendation 18 August 2009, https://www.w3.org/TR/2009/REC-skos-reference-20090818/
- **Copyright:** Copyright © 2009 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** converted from RDF/XML to Turtle, with all 252 triples unchanged. The source's XML comments, which say which semantic conditions of the SKOS Reference the schema formalises, were not carried over. A comment header was added.

### `oa.ttl` — Web Annotation Ontology
- **Version:** dcterms:modified 2016-11-12. LOV lists it under its older name, "Open Annotation Data Model".
- **Source:** http://www.w3.org/ns/oa; triples identical to https://www.w3.org/ns/oa.ttl. Obtained via LOV.
- **Status:** defined in Web Annotation Vocabulary, W3C Recommendation 23 February 2017, https://www.w3.org/TR/2017/REC-annotation-vocab-20170223/
- **Copyright:** Copyright © 2016 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023). The ontology itself says that changes to it must come from a W3C Working Group. No triple has been changed.
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `dqv.ttl` — Data Quality Vocabulary (DQV)
- **Version:** 2016-08-26
- **Source:** http://www.w3.org/ns/dqv; triples identical to https://www.w3.org/ns/dqv.ttl. Obtained via LOV.
- **Status:** defined in Data on the Web Best Practices: Data Quality Vocabulary, W3C Working Group Note 15 December 2016, https://www.w3.org/TR/2016/NOTE-vocab-dqv-20161215/
- **Copyright:** Copyright © 2016 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `vcard.ttl` — Ontology for vCard
- **Version:** 2014-05-22 revision. Its triples are identical to the current https://www.w3.org/2006/vcard/ns.ttl, except for 12 triples added there later (a Solid contacts extension), which this older revision lacks.
- **Source:** http://www.w3.org/2006/vcard/ns, obtained via LOV
- **Status:** defined in vCard Ontology - for describing People and Organizations, W3C Interest Group Note 22 May 2014, https://www.w3.org/TR/2014/NOTE-vcard-rdf-20140522/
- **Copyright:** Copyright © 2014 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `vs.ttl` — SemWeb Vocab Status ontology
- **Version:** dc:modified 2011-12-12. By Dan Brickley, Leigh Dodds and Libby Miller.
- **Source:** http://www.w3.org/2003/06/sw-vocab-status/ns; triples identical to https://www.w3.org/2003/06/sw-vocab-status/ns.rdf. Obtained via LOV.
- **Status:** described in Term-centric Semantic Web Vocabulary Annotations, "(Editor's Draft of a potential) W3C Interest Group Note 31 December 2009", https://www.w3.org/2003/06/sw-vocab-status/note
- **Copyright:** Copyright © 2011 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `wgs84.ttl` — WGS84 Geo Positioning: an RDF vocabulary
- **Version:** v1.22, `$Date: 2009/04/20`
- **Source:** http://www.w3.org/2003/01/geo/wgs84_pos; triples identical to https://www.w3.org/2003/01/geo/wgs84_pos.rdf. Obtained via LOV.
- **Status:** an informal document of the W3C Semantic Web Interest Group (https://www.w3.org/2003/01/geo/) with no formal W3C status. It states no copyright or licence, so W3C's site default, the Document License, applies with the notice that licence prescribes.
- **Copyright:** Copyright © 2009 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023)
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `csvw.ttl` — CSVW Namespace Vocabulary Terms
- **Version:** the 2015 revision. Its triples are identical to https://github.com/w3c/csvw/blob/63ee7a7/ns/csvw.ttl (2015-07-02; its owl:versionInfo names commit 6e6daf9), the revision published at `http://www.w3.org/ns/csvw` in 2015-2016. The 2017-06-06 revision at https://www.w3.org/ns/csvw.ttl supersedes it. LOV labels this copy "v2015-12-17".
- **Source:** http://www.w3.org/ns/csvw, obtained via LOV
- **Status:** namespace document of Metadata Vocabulary for Tabular Data, W3C Recommendation 17 December 2015, https://www.w3.org/TR/2015/REC-tabular-metadata-20151217/
- **Copyright:** Copyright © 2015 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023). The 2015 namespace page applied W3C's document-use rules. Only the 2017 revision moved to the permissive Software and Document License.
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `adms.ttl` — Asset Description Metadata Schema (ADMS)
- **Version:** namespace revision 2015-07-22
- **Source:** http://www.w3.org/ns/adms as published until 2023. Its triples are identical to https://www.w3.org/ns/legacy_adms.ttl. `http://www.w3.org/ns/adms` now redirects to a different 2023 SEMIC revision, which is not this file. Obtained via LOV. LOV's copy omits the upstream's first comment line, which says the vocabulary is deprecated and now maintained by SEMIC.
- **Status:** described by Asset Description Metadata Schema (ADMS), W3C Working Group Note 01 August 2013 (Retired August 2023), https://www.w3.org/TR/2013/NOTE-vocab-adms-20130801/. Editors: Andrea Perego, Phil Archer, Makx Dekkers (W3C Government Linked Data Working Group).
- **Copyright:** Copyright © 2015 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/
- **Licence:** W3C Document License (2023). ADMS's definitions derive from ADMS v1.00 of the European Commission's ISA Programme, and that release says: "Copyright © 2012 European Union. This vocabulary is published under the ISA Open Metadata Licence v1.1" (https://interoperable-europe.ec.europa.eu/licence/isa-open-metadata-licence-v11). That licence's conditions are therefore met too: the copyright notice is kept, and its "No Warranty" disclaimer is in appendix E. Full text: `LICENSES/ISA-Open-Metadata-Licence-1.1.txt`.
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `xsd.ttl` — XML Schema Datatypes (partly W3C material)
- **What it is:** written by Open Triplestore, because there is no normative Turtle vocabulary for XSD. Several of its rdfs:comment descriptions are copied from or derived from the XSD specifications:
  - XML Schema Part 2: Datatypes Second Edition, W3C Recommendation 28 October 2004, https://www.w3.org/TR/2004/REC-xmlschema-2-20041028/ — Copyright © 2004 W3C® (MIT, ERCIM, Keio), All Rights Reserved.
  - W3C XML Schema Definition Language (XSD) 1.1 Part 2: Datatypes, W3C Recommendation 5 April 2012, https://www.w3.org/TR/2012/REC-xmlschema11-2-20120405/ — Copyright © 2012 W3C® (MIT, ERCIM, Keio), All Rights Reserved.
- **Licence:** the W3C-derived descriptions stay under the W3C Document License (2023). The file's header keeps both specifications' own copyright notices, as quoted above, and carries the notice quoted at the top of this section, naming both specifications. The rest is Open Triplestore's own work under the project licence.

## W3C Software and Document License (2015)

Full text: appendix B, also `LICENSES/W3C-Software-and-Document-License-2015.txt`,
https://www.w3.org/copyright/software-license-2015/.

### `odrl.ttl` — ODRL Version 2.2 ontology
- **Version:** ODRL 2.2. The triples are identical to https://github.com/w3c/poe/blob/f14648fb/vocab/ODRL22.ttl (2018-03-30), an older revision than the current https://www.w3.org/ns/odrl/2/ODRL22.ttl.
- **Source:** http://www.w3.org/ns/odrl/2/, obtained via LOV
- **Status:** defined in ODRL Vocabulary & Expression 2.2, W3C Recommendation 15 February 2018, https://www.w3.org/TR/2018/REC-odrl-vocab-20180215/. Creators: Michael Steidl, Renato Iannella, Stuart Myles, Víctor Rodríguez-Doncel (W3C Permissions & Obligations Expression Working Group).
- **Licence:** W3C Software and Document License (2015). The Recommendation and the w3c/poe repository both apply this licence. The file's own dcterms:license, kept, points to W3C's 2002 IPR notice.
- **Notice (in the header):** "This software or document includes material copied from or derived from ODRL Version 2.2, http://www.w3.org/ns/odrl/2/. Copyright © 2018 W3C® (MIT, ERCIM, Keio, Beihang)."
- **Changes:** re-serialized as Turtle by LOV; comment header added (2026-07-24, revised 2026-09-23)

### `sosa.ttl` and `ssn.ttl` — Semantic Sensor Network Ontology (SOSA core and SSN)
- **Source:** https://github.com/w3c/sdw/blob/gh-pages/ssn/integrated/sosa.ttl and `…/ssn.ttl`, the same files that `http://www.w3.org/ns/sosa/` and `http://www.w3.org/ns/ssn/` serve
- **Status:** defined in Semantic Sensor Network Ontology, W3C Recommendation 19 October 2017, by the joint W3C/OGC Spatial Data on the Web Working Group, https://www.w3.org/TR/2017/REC-vocab-ssn-20171019/
- **Copyright:** "Copyright 2017 W3C/OGC." (each file's own dcterms:rights, kept)
- **Licences:** each file declares two licences, and Open Triplestore meets the conditions of both:
  - the W3C Software and Document License (2015)
  - the OGC Software License 1.0 (`http://www.opengeospatial.org/ogc/Software`, now https://www.ogc.org/about/policies/software-licenses/ogc-software-license-1-0/; full text: appendix C)
- **Notice (in the headers):** "This software or document includes material copied from or derived from Sensor, Observation, Sample, and Actuator (SOSA) Ontology, http://www.w3.org/ns/sosa/ [resp. Semantic Sensor Network Ontology, http://www.w3.org/ns/ssn/]. Copyright © 2017 W3C® (MIT, ERCIM, Keio, Beihang)."
- **Changes (with dates, as the OGC licence requires):** a comment header was added on 2026-08-12 and revised on 2026-09-23. The rest of each file is byte-identical to the source.

## Creative Commons Attribution 4.0 (CC BY 4.0)

Licence: https://creativecommons.org/licenses/by/4.0/

### `dcat.ttl` — Data Catalog Vocabulary (DCAT) Version 3
- **Source:** https://www.w3.org/ns/dcat.ttl (served from https://www.w3.org/ns/dcat3.ttl)
- **Status:** defined in Data Catalog Vocabulary (DCAT) - Version 3, W3C Recommendation 22 August 2024, https://www.w3.org/TR/2024/REC-vocab-dcat-3-20240822/
- **Copyright:** Copyright © 2024 World Wide Web Consortium. Created by the W3C Dataset Exchange Working Group. The file names its creators, editors and contributors.
- **Licence:** CC BY 4.0, the file's own dcterms:license
- **Changes:** comment header added; otherwise byte-identical

### `dcat/2.0.0.ttl` — Data Catalog Vocabulary (DCAT) Version 2
- **Source:** https://www.w3.org/ns/dcat2.ttl
- **Status:** defined in Data Catalog Vocabulary (DCAT) - Version 2, W3C Recommendation 04 February 2020, https://www.w3.org/TR/2020/REC-vocab-dcat-2-20200204/
- **Copyright:** Copyright © 2020 W3C® (MIT, ERCIM, Keio, Beihang). The file names its creators and contributors.
- **Licence:** CC BY 4.0, the file's own dct:license
- **Changes:** comment header added; otherwise byte-identical

### `time.ttl` — Time Ontology in OWL (OWL-Time)
- **Version:** ontology version 2016, dct:modified 2017-04-06
- **Source:** https://www.w3.org/2006/time.ttl
- **Status:** defined in Time Ontology in OWL, W3C Recommendation 19 October 2017, by the joint W3C/OGC Spatial Data on the Web Working Group, https://www.w3.org/TR/2017/REC-owl-time-20171019/
- **Copyright:** "Copyright © 2006-2017 W3C, OGC. W3C and OGC liability, trademark and document use rules apply." (the file's own dct:rights, kept). The file identifies its creators and contributors.
- **Licence:** CC BY 4.0, the file's own dct:license
- **Changes:** comment header added; otherwise byte-identical

### Dublin Core: `dcterms.ttl`, `dcterms/2012.06.14.ttl`, `dctype.ttl`
DCMI's Schema Use Notice (https://www.dublincore.org/about/copyright/) puts the RDF
schemas on its site under CC BY 4.0 and asks for this notice in the software and
its documentation:

> Portions of this software may use RDF schemas Copyright © 2011 DCMI, the Dublin Core™ Metadata Initiative. These are licensed under the Creative Commons 4.0 Attribution license.

DCMI: http://dublincore.org/ — licence: https://creativecommons.org/licenses/by/4.0/

- `dcterms.ttl` — DCMI Metadata Terms, the current ("Latest") release file, including the DCMI Metadata Element Set (`http://purl.org/dc/elements/1.1/`) terms. **Source:** https://www.dublincore.org/specifications/dublin-core/dcmi-terms/dublin_core_terms.ttl (vendored 2026-06-07). It carries changes DCMI made after the dated 2020-01-20 release, so it is not identical to `http://dublincore.org/2020/01/20/dublin_core_terms.ttl`. **Changes:** comment header added; otherwise byte-identical.
- `dcterms/2012.06.14.ttl` — DCMI Metadata Terms, release of 2012-06-14. **Source:** http://dublincore.org/2012/06/14/dcterms.ttl. **Changes:** comment header added; otherwise byte-identical.
- `dctype.ttl` — DCMI Type Vocabulary, release of 2012-06-14. **Source:** `http://purl.org/dc/dcmitype/`; triples identical to http://dublincore.org/2012/06/14/dctype.ttl. Obtained via LOV. **Changes:** re-serialized as Turtle by LOV; comment header added.

### `bibo.ttl` — The Bibliographic Ontology (BIBO) 1.3
- **Version:** 1.3 (2009-11-04), by Bruce D'Arcus and Frédérick Giasson. This older revision differs slightly from DCMI's current https://www.dublincore.org/specifications/bibo/bibo/bibo.ttl: for example, it lacks bibo:Specification and keeps some older labels.
- **Source:** `http://purl.org/ontology/bibo/`, as captured by LOV
- **Copyright:** Copyright © 2008-2013 by Structured Dynamics LLC
- **Status:** now maintained by DCMI as a DCMI Community Specification, latest update 2016-05-11. The canonical version is on DCMI's site: https://www.dublincore.org/specifications/bibo/bibo/
- **Licence:** CC BY 4.0, under DCMI's Document Notice (https://www.dublincore.org/about/copyright/). The original Structured Dynamics specification was licensed CC BY 1.0, http://creativecommons.org/licenses/by/1.0/, and stated that its copyright does not apply to the ontology terms.
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `cc.ttl` — Creative Commons Rights Expression Language (ccREL) schema
- **Version:** the 2008-03-03 text
- **Source:** http://creativecommons.org/schema.rdf. The triples are identical to https://github.com/creativecommons/cc.licenserdf/blob/master/cc/licenserdf/rdf/schema.rdf, with language tags in lower case. https://creativecommons.org/schema.rdf now serves the same terms with whitespace differences in some literals. Obtained via LOV.
- **Creator:** Creative Commons
- **Licence:** CC BY 4.0, per https://creativecommons.org/policies/. The deprecated cc.licenserdf repository also offers the schema under the MIT licence. No endorsement by Creative Commons is implied.
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `fog.ttl` — File Ontology for Geometry formats (FOG) 0.0.4
- **Authors:** Anna Wagner, Mathias Bonduel, Pieter Pauwels. FOG is a research ontology, not a W3C Linked Building Data Community Group deliverable.
- **Source:** https://w3id.org/fog (https://mathib.github.io/fog-ontology/ontology.ttl, repository https://github.com/mathib/fog-ontology)
- **Licence:** CC BY 4.0, the file's own cc:license
- **Changes:** comment header added; otherwise byte-identical

### `omg.ttl` — Ontology for Managing Geometry (OMG) 0.3
- **Authors:** Anna Wagner (TU Darmstadt), Mathias Bonduel (KU Leuven), Pieter Pauwels (Ghent University). OMG is not a W3C Linked Building Data Community Group deliverable.
- **Source:** https://w3id.org/omg (https://annawagner.github.io/omg/omg.ttl, repository https://github.com/tudaIIB/omg)
- **Licence:** CC BY 4.0. The authors state it on https://w3id.org/omg and in the repository's documentation configuration. The Turtle file itself has no licence statement.
- **Changes:** comment header added; otherwise byte-identical

## Creative Commons Attribution 3.0 (CC BY 3.0)

Licence: http://creativecommons.org/licenses/by/3.0/

### `gr.ttl` — The GoodRelations Ontology for Semantic Web-based E-Commerce
- **Version:** V1, release 2011-10-01, by Martin Hepp
- **Source:** http://purl.org/goodrelations/v1, served from http://www.heppnetz.de/ontologies/goodrelations/v1.owl; triples identical. Obtained via LOV.
- **Attribution requested by the author:** "This work is based on the GoodRelations ontology, developed by Martin Hepp" — http://purl.org/goodrelations/
- **Licence:** CC BY 3.0 Unported (the file's own dc:rights and dcterms:license, kept)
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `voaf.ttl` — Vocabulary of a Friend (VOAF) v2.3
- **Version:** v2.3 (2013-05-24). Creator: Bernard Vatant. Contributors: Pierre-Yves Vandenbussche, Lise Rozat. Publisher: Open Knowledge Foundation. Developed in the Datalift project.
- **Source:** http://purl.org/vocommons/voaf (https://lov.linkeddata.es/vocommons/voaf/v2.3/), obtained via LOV. LOV's copy differs from the publisher's `voaf_v2.3.rdf` in two ways: whitespace inside some literals, and the owl:unionOf axiom of voaf:Vocabulary stated twice.
- **Licence:** CC BY 3.0 (the file's own cc:license, kept)
- **Changes:** re-serialized as Turtle by LOV; comment header added

## Creative Commons Attribution 1.0 (CC BY 1.0)

Licence: http://creativecommons.org/licenses/by/1.0/

### `foaf.ttl` — FOAF Vocabulary Specification 0.99
- **Version:** 0.99, Paddington Edition, 14 January 2014
- **Source:** http://xmlns.com/foaf/spec/index.rdf, identical to http://xmlns.com/foaf/spec/20140114.rdf. Specification: http://xmlns.com/foaf/spec/
- **Copyright:** Copyright © 2000-2014 Dan Brickley and Libby Miller. The specification says this copyright applies to the FOAF Vocabulary Specification and accompanying documentation in RDF.
- **Licence:** CC BY 1.0
- **Changes:** converted from RDF/XML to Turtle, with all 631 triples unchanged; comment header added

### `vann.ttl` — VANN: A vocabulary for annotating vocabulary descriptions
- **Version:** release of 2010-06-07, by Ian Davis
- **Source:** https://vocab.org/vann/vann-vocab-20100607.rdf
- **Copyright:** Copyright © 2005 Ian Davis (the file's own dcterms:rights, kept)
- **Licence:** CC BY 1.0 (the file's own cc:license, kept)
- **Changes:** converted from RDF/XML to Turtle, with the triples unchanged except one: the source gives the cc:Work statement's subject relative to the document, and this copy writes it out as the source URL. Comment header added.

### `sioc.ttl` — SIOC Core Ontology Namespace, Revision 1.35
- **Version:** Revision 1.35 (2010-03-25). The triples are identical to that revision as published at http://rdfs.org/sioc/ns in 2010; the current document is Revision 1.36. Editors: Uldis Bojārs, John G. Breslin.
- **Source:** http://rdfs.org/sioc/ns, obtained via LOV
- **Copyright:** Copyright © 2004-2010 by DERI, NUI Galway. This is the specification's notice at Revision 1.35. http://rdfs.org/sioc/spec/ now names the holder as Data Science Institute (formerly DERI), NUI Galway.
- **Licence:** CC BY 1.0, as stated at http://rdfs.org/sioc/spec/ for the SIOC Core Ontology Specification and accompanying documentation. The publisher states that this copyright does not apply to SIOC data formats, ontology terms, or technology.
- **Changes:** re-serialized as Turtle by LOV; comment header added

### `bot.ttl` — Building Topology Ontology (BOT) 0.3.2
- **Source:** https://raw.githubusercontent.com/w3c-lbd-cg/bot/master/bot.ttl, identical to `bot-0.3.2.ttl` and https://w3id.org/bot
- **Copyright:** Copyright 2017-2020 W3C Linked Building Data Community Group (upstream header, kept). The file names its creators and contributors.
- **Licence:** CC BY 1.0 (the file's own dcterms:license)
- **Changes:** comment header added above the upstream header; otherwise byte-identical

## Creative Commons Attribution-ShareAlike 3.0 (CC BY-SA 3.0)

### `schema.ttl` — Schema.org subset (an adaptation)
- **Adapted from:** Schema.org release 30.0 (2026-03-19). Release files: https://github.com/schemaorg/schemaorg/tree/main/data/releases/30.0; release notes: https://schema.org/docs/releases.html. (Schema.org serves no page at `https://schema.org/version/30.0/`.)
- **Copyright:** Google, Inc., Yahoo, Inc., Microsoft Corporation and Yandex (the Schema.org Sponsors). The Sponsors license their copyrights in the schema under CC BY-SA 3.0, http://creativecommons.org/licenses/by-sa/3.0/ (https://schema.org/docs/terms.html).
- **Changes (the adaptation):** Open Triplestore kept only 27 terms: 12 classes and 15 properties. Each keeps all of its triples from release 30.0 unchanged. Each term is stated a second time under its `http://schema.org/` IRI, with objects still `https://schema.org/`, so that data using either namespace resolves. A comment header was added.
- **Licence of the adaptation:** Open Triplestore releases `schema.ttl` under CC BY-SA 3.0, http://creativecommons.org/licenses/by-sa/3.0/. The Open Triplestore licence (AGPL-3.0 with the Commons Clause) does not apply to it. No endorsement by Schema.org or its Sponsors is implied.

## Open Data Commons Public Domain Dedication and License (PDDL) 1.0

Licence: http://www.opendatacommons.org/licenses/pddl/1.0/. It imposes no
conditions; the credit below is a courtesy.

- `org.ttl` — Core organization ontology (the W3C Organization Ontology), version 0.8. **Source:** https://www.w3.org/ns/org.ttl. **Status:** defined in The Organization Ontology, W3C Recommendation 16 January 2014, https://www.w3.org/TR/2014/REC-vocab-org-20140116/. **Licence:** PDDL 1.0 (the file's own dct:license). **Changes:** comment header added; otherwise byte-identical.
- `qb.ttl` — The RDF Data Cube Vocabulary, version 0.2. **Source:** https://raw.githubusercontent.com/UKGovLD/publishing-statistical-data/master/specs/src/main/vocab/cube.ttl, which is what `http://purl.org/linked-data/cube` returns as Turtle. **Status:** defined in The RDF Data Cube Vocabulary, W3C Recommendation 16 January 2014, https://www.w3.org/TR/2014/REC-vocab-data-cube-20140116/. **Licence:** PDDL 1.0 (the file's own dcterms:license). **Changes:** comment header added; otherwise byte-identical.

## Apache License 2.0

Full text: appendix F, also `LICENSES/Apache-2.0.txt` in the Open Triplestore
source, https://www.apache.org/licenses/LICENSE-2.0. The upstream projects below
have no NOTICE file. Each changed file carries a notice that it was changed, as §4(b)
of the licence requires.

### `pav.ttl` — PAV: Provenance, Authoring and Versioning 2.3.1
- **Version:** 2.3.1 (2014-08-28), by Paolo Ciccarese and Stian Soiland-Reyes
- **Source:** http://purl.org/pav/ (https://pav-ontology.github.io/pav/pav.rdf, repository https://github.com/pav-ontology/pav); triples identical. Obtained via LOV.
- **Copyright:** Copyright 2008-2014 Massachusetts General Hospital; Harvard Medical School; Balboa Systems; University of Manchester (kept in the file)
- **Licence:** Apache-2.0 (the file's own dcterms:license)
- **Changes:** LOV converted it from RDF/XML to Turtle, with the triples unchanged. Open Triplestore added a comment header (2026-07-24, revised 2026-09-23).

### `geosparql.ttl` — OGC GeoSPARQL 1.1 ontology
- **Version:** owl:versionInfo "1.1.1 - 2026-02"
- **Source:** https://github.com/opengeospatial/geosemantics-semantic-resources/blob/main/resources/geosparql-swg/geosparql-1.1/ontologies/geo.ttl, identical to `resources/geosparql-swg/ontologies/geo.ttl` at commit b31d0246
- **Copyright:** "(c) 2021 Open Geospatial Consortium" (the file's own schema:copyrightNotice)
- **Licence:** Apache-2.0: the file's own schema:license and the source repository's LICENSE. OGC's own server (http://www.opengis.net/ont/geosparql) serves the same version with a licence triple that points to the OGC Document License Agreement. Open Triplestore relies on the Apache-2.0 grant.
- **Changes:** comment header added (2026-09-23); otherwise byte-identical

### `geosparql/1.0.0.ttl` — OGC GeoSPARQL 1.0 ontology
- **Version:** GeoSPARQL 1.0 (2012), in the OGC GeoSPARQL Standards Working Group's Turtle serialization
- **Source:** https://github.com/opengeospatial/geosemantics-semantic-resources/blob/main/resources/geosparql-swg/geosparql-1.0/ontologies/geo.ttl. It is identical to `vocabularies/geo.ttl` at commit 9de28a4f on branch `geosparql-1.0` of https://github.com/opengeospatial/ogc-geosparql; that branch's LICENSE is the OGC Document License Agreement.
- **Copyright:** Copyright (c) 2012 Open Geospatial Consortium, the notice of the original RDF/XML release https://schemas.opengis.net/geosparql/1.0/geosparql_vocab_all.rdf
- **Licence:** Apache-2.0, per the source repository's LICENSE and README. The file also contains the DCMI Metadata Element Set definitions (dc:contributor … dc:type), which fall under DCMI's CC BY 4.0 Schema Use Notice quoted above.
- **Changes:** comment header added (2026-09-23); otherwise byte-identical

## BSD 3-Clause License (ETSI)

### `saref.ttl` — SAREF (Smart Applications REFerence ontology) Core v3.1.1
- **Version:** v3.1.1, dcterms:issued 2020-02-11. The file identifies its creators (dcterms:creator), and ETSI is its publisher.
- **Source:** https://saref.etsi.org/core/v3.1.1/saref.ttl
- **Copyright:** Copyright 2019 ETSI
- **Licence:** BSD-3-Clause, per the file's own dcterms:license https://forge.etsi.org/etsi-software-license. The licence text, with ETSI's notice, is in appendix D and in `LICENSES/BSD-3-Clause-ETSI-SAREF.txt`. It is taken verbatim from the saref-core repository's LICENSE, https://labs.etsi.org/rep/saref/saref-core/-/blob/develop-v3.1.1/LICENSE.
- **Changes:** comment header added; otherwise byte-identical

## ISA Open Metadata Licence v1.1

Licence: https://interoperable-europe.ec.europa.eu/licence/isa-open-metadata-licence-v11
(full text: `LICENSES/ISA-Open-Metadata-Licence-1.1.txt`; the "No Warranty"
disclaimer that distributions must keep is in appendix E).

### `locn.ttl` — ISA Programme Location Core Vocabulary (LOCN)
- **Version:** 2015-03-23
- **Source:** http://www.w3.org/ns/locn. The triples are identical to https://www.w3.org/ns/locn.ttl except one relative IRI, `locn.svg`, which LOV resolved to `http://njh.me/locn.svg`. Obtained via LOV.
- **Copyright:** Copyright © European Union, 2012-2015. (the file's own dcterms:rights, kept)
- **Licence:** ISA Open Metadata Licence v1.1. The file's own dcterms:license gives the former Joinup address, which no longer resolves. The Joinup (now Interoperable Europe) page of this vocabulary is https://interoperable-europe.ec.europa.eu/collection/semic-support-centre/solution/core-location-vocabulary/release/100
- **Changes:** re-serialized as Turtle by LOV, with the triples unchanged apart from the `locn.svg` IRI. Comment header added; it also reproduces the "No Warranty" disclaimer.

## IMBOR (Stichting CROW)

### `imbor.ttl` — IMBOR 2025 Vocabulaire
- **What it is:** the IMBOR 2025 Vocabulaire (8,948 SKOS concepts) by Stichting CROW. The file is `imbor2025-vocabulaire.ttl` from CROW's IMBOR 2025 Linked Data release.
- **Source:** the release asset https://github.com/Stichting-CROW/imbor/releases/download/2025/IMBOR-2025.LinkedData.zipfile.zip (release tag `2025`, https://github.com/Stichting-CROW/imbor/releases/tag/2025). ZIP sha256 `910dbcfcd628258f3e383d419b70c8e5ef91a9718382d1420cb4d5fa09d651dc`.
- **Changes:** none. The file is byte-identical to `imbor2025-vocabulaire.ttl` in that ZIP, sha256 `16d9ca3cf13862ddce6ca9a813ccf5174cea1876ed44611806be766d2224ce84`. Open Triplestore adds no header to it and must not alter it (see below).
- **Copyright:** © Stichting CROW. The file credits "IMBOR - Stichting CROW" (dc:creator).
- **Licence:** CROW makes two statements, and Open Triplestore complies with the stricter reading of both:
  - CROW's licence table (https://docs.crow.nl/imbor/techdoc/#licenties) licenses "IMBOR gegevens", all data in the IMBOR ontology including the Vocabulaire, under CC BY 4.0, https://creativecommons.org/licenses/by/4.0/. It licenses "IMBOR Producten", which include the Linked Data release, under the Open Data Commons Attribution License (ODC-BY) 1.0, https://opendatacommons.org/licenses/by/1-0/.
  - CROW's Beheerplan IMBOR, version 1.0 (approved 2 July 2025, https://docs.crow.nl/imbor/beheerplan/), section "Rechtenbeleid", says CROW chose CC BY-ND 4.0, https://creativecommons.org/licenses/by-nd/4.0/, for its open standards.

  The stricter reading means: attribution to Stichting CROW, the licence URIs above, and no altered copies. Open Triplestore therefore redistributes the file only unmodified. The notices that CC BY 4.0 and ODC-BY 1.0 §4.2 ask for are given here, next to the file, and in the root `NOTICE`, not inside the file.

## DOAP

### `doap.ttl` — Description of a Project (DOAP) vocabulary
- **What it is:** the DOAP vocabulary (namespace `http://usefulinc.com/ns/doap#`) by Edd Dumbill, now Edd Wilder-James. The file is LOV's Turtle re-serialization of the DOAP namespace document; LOV labels the version "2012-01-04". Its 696 triples are identical to the document served at `http://usefulinc.com/ns/doap` from at least 2009 to 2015, for example the Internet Archive capture https://web.archive.org/web/20090626130747/http://usefulinc.com/ns/doap.
- **Copyright:** "Copyright © 2004-2009 Edd Dumbill" (the file's own dc:rights). The file states no licence.
- **Licence:** the upstream repository https://github.com/ewilderj/doap (formerly edumbill/doap) has been under the Apache License 2.0 since 2018-03-31. Its current `schema/doap.rdf` gives "Copyright © 2004-2016 Edd Dumbill, 2016-2017 Edd Wilder-James, 2018- The DOAP Authors". That grant covers the material in that repository, which is most of this file.
  - This file also contains 97 Japanese-language labels and comments (96 tagged `@ja`, one `@jp`) and a few other strings that have never been in that repository's `schema/doap.rdf`. The Apache-2.0 grant is not shown to cover them, and no licence has been published for them.
- **Changes:** re-serialized as Turtle by LOV; Open Triplestore added a comment header in 2026-07. That header's "Source" line names https://github.com/edumbill/doap/blob/master/schema/doap.rdf, which is where LOV's catalogue points. The content matches the usefulinc.com namespace document instead.

## Open Triplestore's own vocabulary

- `ots.ttl` — Open Triplestore (OTS) vocabulary, written by Open Triplestore; it contains no third-party material. It is under the project licence (see `LICENSE`).
- `xsd.ttl` — written by Open Triplestore, with W3C-derived descriptions; see its entry above.

## Appendix: licence texts

Reproduced verbatim from the rights holders' sources (retrieved 2026-09-23).
Texts taken from web pages are rendered as plain text.

### A. W3C Document License (2023)

Source: https://www.w3.org/copyright/document-license-2023/

```text
Public documents on the W3C site are provided by the copyright holders under
the following license.

License

By using and/or copying this document, or the W3C document from which this
statement is linked, you (the licensee) agree that you have read, understood,
and will comply with the following terms and conditions:

Permission to copy, and distribute the contents of this document, or the W3C
document from which this statement is linked, in any medium for any purpose
and without fee or royalty is hereby granted, provided that you include the
following on ALL copies of the document, or portions thereof, that you use:

- A link or URL to the original W3C document.

- The pre-existing copyright notice of the original author, or if it doesn't
  exist, a notice (hypertext is preferred, but a textual representation is
  permitted) of the form: "Copyright © [$date-of-document] World Wide Web
  Consortium. https://www.w3.org/copyright/document-license-2023/"

- If it exists, the STATUS of the W3C document.

When space permits, inclusion of the full text of this NOTICE should be
provided. We request that authorship attribution be provided in any software,
documents, or other items or products that you create pursuant to the
implementation of the contents of this document, or any portion thereof.

No right to create modifications or derivatives of W3C documents is granted
pursuant to this license, except as follows: To facilitate implementation of
the technical specifications set forth in this document, anyone may prepare
and distribute derivative works and portions of this document in software, in
supporting materials accompanying software, and in documentation of software,
PROVIDED that all such works include the notice below. HOWEVER, the
publication of derivative works of this document for use as a technical
specification is expressly prohibited.

In addition, "Code Components" —Web IDL in sections clearly marked as Web IDL;
and W3C-defined markup (HTML, CSS, etc.) and computer programming language
code clearly marked as code examples— are licensed under the W3C Software
License.

The notice is:

"Copyright © 2023 W3C®. This software or document includes material copied
from or derived from [title and URI of the W3C document]."

Disclaimers

THIS DOCUMENT IS PROVIDED "AS IS," AND COPYRIGHT HOLDERS MAKE NO
REPRESENTATIONS OR WARRANTIES, EXPRESS OR IMPLIED, INCLUDING, BUT NOT LIMITED
TO, WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE,
NON-INFRINGEMENT, OR TITLE; THAT THE CONTENTS OF THE DOCUMENT ARE SUITABLE FOR
ANY PURPOSE; NOR THAT THE IMPLEMENTATION OF SUCH CONTENTS WILL NOT INFRINGE
ANY THIRD PARTY PATENTS, COPYRIGHTS, TRADEMARKS OR OTHER RIGHTS.

COPYRIGHT HOLDERS WILL NOT BE LIABLE FOR ANY DIRECT, INDIRECT, SPECIAL OR
CONSEQUENTIAL DAMAGES ARISING OUT OF ANY USE OF THE DOCUMENT OR THE
PERFORMANCE OR IMPLEMENTATION OF THE CONTENTS THEREOF.

The name and trademarks of copyright holders may NOT be used in advertising or
publicity pertaining to this document or its contents without specific,
written prior permission. Title to copyright in this document will at all
times remain with copyright holders.
```

### B. W3C Software and Document License (2015)

Source: https://www.w3.org/copyright/software-license-2015/ (also
`LICENSES/W3C-Software-and-Document-License-2015.txt`)

```text
This work is being provided by the copyright holders under the following
license.

License

By obtaining and/or copying this work, you (the licensee) agree that you have
read, understood, and will comply with the following terms and conditions.

Permission to copy, modify, and distribute this work, with or without
modification, for any purpose and without fee or royalty is hereby granted,
provided that you include the following on ALL copies of the work or portions
thereof, including modifications:

- The full text of this NOTICE in a location viewable to users of the
  redistributed or derivative work.

- Any pre-existing intellectual property disclaimers, notices, or terms and
  conditions. If none exist, the W3C Software and Document Short Notice should
  be included.

- Notice of any changes or modifications, through a copyright statement on the
  new code or document such as "This software or document includes material
  copied from or derived from [title and URI of the W3C document]. Copyright ©
  [YEAR] W3C® (MIT, ERCIM, Keio, Beihang)."

Disclaimers

THIS WORK IS PROVIDED "AS IS," AND COPYRIGHT HOLDERS MAKE NO REPRESENTATIONS
OR WARRANTIES, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO, WARRANTIES OF
MERCHANTABILITY OR FITNESS FOR ANY PARTICULAR PURPOSE OR THAT THE USE OF THE
SOFTWARE OR DOCUMENT WILL NOT INFRINGE ANY THIRD PARTY PATENTS, COPYRIGHTS,
TRADEMARKS OR OTHER RIGHTS.

COPYRIGHT HOLDERS WILL NOT BE LIABLE FOR ANY DIRECT, INDIRECT, SPECIAL OR
CONSEQUENTIAL DAMAGES ARISING OUT OF ANY USE OF THE SOFTWARE OR DOCUMENT.

The name and trademarks of copyright holders may NOT be used in advertising or
publicity pertaining to the work without specific, written prior permission.
Title to copyright in this work will at all times remain with copyright
holders.
```

### C. OGC Software License, Version 1.0

Source: https://www.ogc.org/about/policies/software-licenses/ogc-software-license-1-0/
(the address `http://www.opengeospatial.org/ogc/Software` in `sosa.ttl` and
`ssn.ttl` now leads to OGC's software licence page)

```text
This OGC work (including software, documents, or other related items) is being
provided by the copyright holders under the following license. By obtaining,
using and/or copying this work, you (the licensee) agree that you have read,
understood, and will comply with the following terms and conditions:

Permission to use, copy, and modify this software and its documentation, with
or without modification, for any purpose and without fee or royalty is hereby
granted, provided that you include the following on ALL copies of the software
and documentation or portions thereof, including modifications, that you make:

1. The full text of this NOTICE in a location viewable to users of the
   redistributed or derivative work.

2. Any pre-existing intellectual property disclaimers, notices, or terms and
   conditions. If none exist, a short notice of the following form (hypertext
   is preferred, text is permitted) should be used within the body of any
   redistributed or derivative code: “Copyright © [$date-of-document] Open
   Geospatial Consortium, Inc. All Rights Reserved.
   https://www.ogc.org/about-ogc/policies/copyright-notice-and-disclaimers/
   (Hypertext is preferred, but a textual representation is permitted.)

3. Notice of any changes or modifications to the OGC files, including the date
   changes were made. (We recommend you provide URIs to the location from
   which the code is derived.)

THIS SOFTWARE AND DOCUMENTATION IS PROVIDED “AS IS,” AND COPYRIGHT HOLDERS
MAKE NO REPRESENTATIONS OR WARRANTIES, EXPRESS OR IMPLIED, INCLUDING BUT NOT
LIMITED TO, WARRANTIES OF MERCHANTABILITY OR FITNESS FOR ANY PARTICULAR
PURPOSE OR THAT THE USE OF THE SOFTWARE OR DOCUMENTATION WILL NOT INFRINGE ANY
THIRD PARTY PATENTS, COPYRIGHTS, TRADEMARKS OR OTHER RIGHTS.

COPYRIGHT HOLDERS WILL NOT BE LIABLE FOR ANY DIRECT, INDIRECT, SPECIAL OR
CONSEQUENTIAL DAMAGES ARISING OUT OF ANY USE OF THE SOFTWARE OR DOCUMENTATION.

The name and trademarks of copyright holders may NOT be used in advertising or
publicity pertaining to the software without specific, written prior
permission. Title to copyright in this software and any associated
documentation will at all times remain with copyright holders.
```

### D. BSD 3-Clause License of SAREF (ETSI)

Source: https://labs.etsi.org/rep/saref/saref-core/-/blob/develop-v3.1.1/LICENSE
(also `LICENSES/BSD-3-Clause-ETSI-SAREF.txt`)

```text
Copyright 2019 ETSI

Redistribution and use in source and binary forms, with or without 
modification, are permitted provided that the following conditions are met:
1. Redistributions of source code must retain the above copyright notice, 
   this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice, 
   this list of conditions and the following disclaimer in the documentation 
   and/or other materials provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors 
   may be used to endorse or promote products derived from this software without 
   specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND 
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED 
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. 
IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, 
INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, 
BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, 
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF 
LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE 
OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED 
OF THE POSSIBILITY OF SUCH DAMAGE.
```

### E. ISA Open Metadata Licence v1.1 — "No Warranty" disclaimer

Source: https://interoperable-europe.ec.europa.eu/licence/isa-open-metadata-licence-v11
(full text: `LICENSES/ISA-Open-Metadata-Licence-1.1.txt`)

```text
4. No Warranty
EACH WORK IS PROVIDED "AS IS" WITHOUT REPRESENTATIONS, WARRANTIES, OBLIGATIONS AND LIABILITIES OF ANY KIND, EITHER EXPRESS OR IMPLIED, TO THE FULL EXTENT PERMITTED BY LAW INCLUDING, BUT NOT LIMITED TO, ANY IMPLIED WARRANTY OF MERCHANTABILITY, INTEGRATION, SATISFACTORY QUALITY AND FITNESS FOR A PARTICULAR PURPOSE.
EXCEPT IN THE CASES OF WILFUL MISCONDUCT OR DAMAGES DIRECTLY CAUSED TO NATURAL PERSONS, NEITHER EUROPEAN UNION NOR ITS CONTRIBUTOR(S) WILL BE LIABLE FOR ANY INCIDENTAL, CONSEQUENTIAL, DIRECT OR INDIRECT DAMAGES INCLUDING BUT NOT LIMITED TO THE LOSS OF DATA, LOST PROFITS, OR ANY OTHER FINANCIAL LOSS ARISING FROM THE USE OF, OR INABILITY TO USE, EVEN IF THE EUROPEAN UNION HAS BEEN NOTIFIED OF THE POSSIBILITY OF SUCH LOSS, DAMAGES, CLAIMS OR COSTS OR FOR ANY CLAIM BY ANY THIRD PARTY. HOWEVER, THE LICENSOR WILL BE LIABLE UNDER STATUTORY PRODUCT LIABILITY LAWS AS FAR SUCH LAWS APPLY TO THE WORK.
```

### F. Apache License, Version 2.0

Source: https://www.apache.org/licenses/LICENSE-2.0.txt (also
`LICENSES/Apache-2.0.txt`). It applies to `pav.ttl`, `geosparql.ttl` and
`geosparql/1.0.0.ttl`.

```text

                                 Apache License
                           Version 2.0, January 2004
                        http://www.apache.org/licenses/

   TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

   1. Definitions.

      "License" shall mean the terms and conditions for use, reproduction,
      and distribution as defined by Sections 1 through 9 of this document.

      "Licensor" shall mean the copyright owner or entity authorized by
      the copyright owner that is granting the License.

      "Legal Entity" shall mean the union of the acting entity and all
      other entities that control, are controlled by, or are under common
      control with that entity. For the purposes of this definition,
      "control" means (i) the power, direct or indirect, to cause the
      direction or management of such entity, whether by contract or
      otherwise, or (ii) ownership of fifty percent (50%) or more of the
      outstanding shares, or (iii) beneficial ownership of such entity.

      "You" (or "Your") shall mean an individual or Legal Entity
      exercising permissions granted by this License.

      "Source" form shall mean the preferred form for making modifications,
      including but not limited to software source code, documentation
      source, and configuration files.

      "Object" form shall mean any form resulting from mechanical
      transformation or translation of a Source form, including but
      not limited to compiled object code, generated documentation,
      and conversions to other media types.

      "Work" shall mean the work of authorship, whether in Source or
      Object form, made available under the License, as indicated by a
      copyright notice that is included in or attached to the work
      (an example is provided in the Appendix below).

      "Derivative Works" shall mean any work, whether in Source or Object
      form, that is based on (or derived from) the Work and for which the
      editorial revisions, annotations, elaborations, or other modifications
      represent, as a whole, an original work of authorship. For the purposes
      of this License, Derivative Works shall not include works that remain
      separable from, or merely link (or bind by name) to the interfaces of,
      the Work and Derivative Works thereof.

      "Contribution" shall mean any work of authorship, including
      the original version of the Work and any modifications or additions
      to that Work or Derivative Works thereof, that is intentionally
      submitted to Licensor for inclusion in the Work by the copyright owner
      or by an individual or Legal Entity authorized to submit on behalf of
      the copyright owner. For the purposes of this definition, "submitted"
      means any form of electronic, verbal, or written communication sent
      to the Licensor or its representatives, including but not limited to
      communication on electronic mailing lists, source code control systems,
      and issue tracking systems that are managed by, or on behalf of, the
      Licensor for the purpose of discussing and improving the Work, but
      excluding communication that is conspicuously marked or otherwise
      designated in writing by the copyright owner as "Not a Contribution."

      "Contributor" shall mean Licensor and any individual or Legal Entity
      on behalf of whom a Contribution has been received by Licensor and
      subsequently incorporated within the Work.

   2. Grant of Copyright License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      copyright license to reproduce, prepare Derivative Works of,
      publicly display, publicly perform, sublicense, and distribute the
      Work and such Derivative Works in Source or Object form.

   3. Grant of Patent License. Subject to the terms and conditions of
      this License, each Contributor hereby grants to You a perpetual,
      worldwide, non-exclusive, no-charge, royalty-free, irrevocable
      (except as stated in this section) patent license to make, have made,
      use, offer to sell, sell, import, and otherwise transfer the Work,
      where such license applies only to those patent claims licensable
      by such Contributor that are necessarily infringed by their
      Contribution(s) alone or by combination of their Contribution(s)
      with the Work to which such Contribution(s) was submitted. If You
      institute patent litigation against any entity (including a
      cross-claim or counterclaim in a lawsuit) alleging that the Work
      or a Contribution incorporated within the Work constitutes direct
      or contributory patent infringement, then any patent licenses
      granted to You under this License for that Work shall terminate
      as of the date such litigation is filed.

   4. Redistribution. You may reproduce and distribute copies of the
      Work or Derivative Works thereof in any medium, with or without
      modifications, and in Source or Object form, provided that You
      meet the following conditions:

      (a) You must give any other recipients of the Work or
          Derivative Works a copy of this License; and

      (b) You must cause any modified files to carry prominent notices
          stating that You changed the files; and

      (c) You must retain, in the Source form of any Derivative Works
          that You distribute, all copyright, patent, trademark, and
          attribution notices from the Source form of the Work,
          excluding those notices that do not pertain to any part of
          the Derivative Works; and

      (d) If the Work includes a "NOTICE" text file as part of its
          distribution, then any Derivative Works that You distribute must
          include a readable copy of the attribution notices contained
          within such NOTICE file, excluding those notices that do not
          pertain to any part of the Derivative Works, in at least one
          of the following places: within a NOTICE text file distributed
          as part of the Derivative Works; within the Source form or
          documentation, if provided along with the Derivative Works; or,
          within a display generated by the Derivative Works, if and
          wherever such third-party notices normally appear. The contents
          of the NOTICE file are for informational purposes only and
          do not modify the License. You may add Your own attribution
          notices within Derivative Works that You distribute, alongside
          or as an addendum to the NOTICE text from the Work, provided
          that such additional attribution notices cannot be construed
          as modifying the License.

      You may add Your own copyright statement to Your modifications and
      may provide additional or different license terms and conditions
      for use, reproduction, or distribution of Your modifications, or
      for any such Derivative Works as a whole, provided Your use,
      reproduction, and distribution of the Work otherwise complies with
      the conditions stated in this License.

   5. Submission of Contributions. Unless You explicitly state otherwise,
      any Contribution intentionally submitted for inclusion in the Work
      by You to the Licensor shall be under the terms and conditions of
      this License, without any additional terms or conditions.
      Notwithstanding the above, nothing herein shall supersede or modify
      the terms of any separate license agreement you may have executed
      with Licensor regarding such Contributions.

   6. Trademarks. This License does not grant permission to use the trade
      names, trademarks, service marks, or product names of the Licensor,
      except as required for reasonable and customary use in describing the
      origin of the Work and reproducing the content of the NOTICE file.

   7. Disclaimer of Warranty. Unless required by applicable law or
      agreed to in writing, Licensor provides the Work (and each
      Contributor provides its Contributions) on an "AS IS" BASIS,
      WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
      implied, including, without limitation, any warranties or conditions
      of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
      PARTICULAR PURPOSE. You are solely responsible for determining the
      appropriateness of using or redistributing the Work and assume any
      risks associated with Your exercise of permissions under this License.

   8. Limitation of Liability. In no event and under no legal theory,
      whether in tort (including negligence), contract, or otherwise,
      unless required by applicable law (such as deliberate and grossly
      negligent acts) or agreed to in writing, shall any Contributor be
      liable to You for damages, including any direct, indirect, special,
      incidental, or consequential damages of any character arising as a
      result of this License or out of the use or inability to use the
      Work (including but not limited to damages for loss of goodwill,
      work stoppage, computer failure or malfunction, or any and all
      other commercial damages or losses), even if such Contributor
      has been advised of the possibility of such damages.

   9. Accepting Warranty or Additional Liability. While redistributing
      the Work or Derivative Works thereof, You may choose to offer,
      and charge a fee for, acceptance of support, warranty, indemnity,
      or other liability obligations and/or rights consistent with this
      License. However, in accepting such obligations, You may act only
      on Your own behalf and on Your sole responsibility, not on behalf
      of any other Contributor, and only if You agree to indemnify,
      defend, and hold each Contributor harmless for any liability
      incurred by, or claims asserted against, such Contributor by reason
      of your accepting any such warranty or additional liability.

   END OF TERMS AND CONDITIONS

   APPENDIX: How to apply the Apache License to your work.

      To apply the Apache License to your work, attach the following
      boilerplate notice, with the fields enclosed by brackets "[]"
      replaced with your own identifying information. (Don't include
      the brackets!)  The text should be enclosed in the appropriate
      comment syntax for the file format. We also recommend that a
      file or class name and description of purpose be included on the
      same "printed page" as the copyright notice for easier
      identification within third-party archives.

   Copyright [yyyy] [name of copyright owner]

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
```
