//! The bundled vocabulary files (`frontend/public/vocab/`) and the licence and
//! attribution of each.
//!
//! The server compiles these files in and seeds them into the model registry as
//! public reference models (`seed_vocab.rs`). A file's comment header carries
//! its notices, but the store keeps triples only, so the header would be lost
//! with the parse. [`BundledFile::attribution`] therefore turns this table, plus
//! the header itself, into a [`ContentAttribution`] that the seeder records as
//! registry metadata next to each seeded entry and version. It is never written
//! into the vocabulary's own graph: the stored triples stay exactly those of
//! the file (IMBOR allows no altered copies; the W3C Document License files are
//! derivatives only with the prescribed notice).
//!
//! Every licence URI, copyright line, notice, status and source below is taken
//! verbatim from the file's own header or from `frontend/public/vocab/NOTICE.md`;
//! a test checks each one against those two texts.

use super::models::{ContentAttribution, LicenseRef};

/// Where this server serves the full attribution and licence texts of every
/// bundled vocabulary ([`NOTICE_MD`]).
pub const NOTICE_PATH: &str = "/vocab/NOTICE.md";

/// `frontend/public/vocab/NOTICE.md`, compiled in so the server serves it at
/// [`NOTICE_PATH`] even when it does not serve the web UI.
pub const NOTICE_MD: &str = include_str!("../../frontend/public/vocab/NOTICE.md");

/// A licence, as named in the file headers.
#[derive(Debug, Clone, Copy)]
pub struct Licence {
    pub name: &'static str,
    pub uri: &'static str,
}

/// One bundled vocabulary file and its licence record.
#[derive(Debug)]
pub struct BundledFile {
    /// Path under `frontend/public/vocab/`.
    pub path: &'static str,
    /// The file's content, compiled in.
    pub ttl: &'static str,
    /// `false` for Open Triplestore's own vocabulary, which needs no notice.
    pub third_party: bool,
    pub licenses: &'static [Licence],
    /// Copyright notices or, where a source states none, its creator credit.
    pub copyright: &'static [&'static str],
    /// The statement the licence asks every copy to carry.
    pub notice: Option<&'static str>,
    /// The source document's status (W3C and W3C/OGC documents).
    pub status: Option<&'static str>,
    pub source: &'static str,
    /// Only where the header has no "Changes…" statement to take verbatim.
    pub changes: Option<&'static str>,
    pub remarks: Option<&'static str>,
    /// The rights holder allows no altered copies.
    pub no_derivatives: bool,
    /// How the store's canonical form of typed literals makes some of this
    /// file's triples read differently from the file (the values are the
    /// same), starting with how many triples it touches; `None` when it
    /// touches none. A test checks it against a round-trip through the store.
    pub store_form: Option<&'static str>,
}

const DEFAULTS: BundledFile = BundledFile {
    path: "",
    ttl: "",
    third_party: true,
    licenses: &[],
    copyright: &[],
    notice: None,
    status: None,
    source: "",
    changes: None,
    remarks: None,
    no_derivatives: false,
    store_form: None,
};

/// `bundled!("rdf.ttl", { field: value, … })`: a [`BundledFile`] whose `ttl` is
/// that very file, so the path and the compiled-in content cannot drift apart.
macro_rules! bundled {
    ($path:literal, { $($field:ident : $value:expr),* $(,)? }) => {
        BundledFile {
            path: $path,
            ttl: include_str!(concat!("../../frontend/public/vocab/", $path)),
            $($field: $value,)*
            ..DEFAULTS
        }
    };
}

/// The notice the W3C Document License (2023) prescribes for derivative works,
/// completed with the document's title and URI as its header gives them.
macro_rules! w3c_notice {
    ($document:literal) => {
        concat!(
            "Copyright © 2023 W3C®. This software or document includes material copied from or \
             derived from ",
            $document
        )
    };
}

// ─── Licences ──────────────────────────────────────────────────────────────────

const W3C_DOCUMENT_LICENSE: Licence = Licence {
    name: "W3C Document License (2023)",
    uri: "https://www.w3.org/copyright/document-license-2023/",
};
const W3C_SOFTWARE_AND_DOCUMENT_LICENSE: Licence = Licence {
    name: "W3C Software and Document License (2015)",
    uri: "https://www.w3.org/copyright/software-license-2015/",
};
const OGC_SOFTWARE_LICENSE: Licence = Licence {
    name: "OGC Software License 1.0",
    uri: "https://www.ogc.org/about/policies/software-licenses/ogc-software-license-1-0/",
};
const CC_BY_4: Licence = Licence {
    name: "CC BY 4.0",
    uri: "https://creativecommons.org/licenses/by/4.0/",
};
const CC_BY_3_UNPORTED: Licence = Licence {
    name: "CC BY 3.0 Unported",
    uri: "http://creativecommons.org/licenses/by/3.0/",
};
const CC_BY_3: Licence = Licence {
    name: "CC BY 3.0",
    uri: "http://creativecommons.org/licenses/by/3.0/",
};
const CC_BY_1: Licence = Licence {
    name: "CC BY 1.0",
    uri: "http://creativecommons.org/licenses/by/1.0/",
};
const CC_BY_SA_3: Licence = Licence {
    name: "CC BY-SA 3.0",
    uri: "http://creativecommons.org/licenses/by-sa/3.0/",
};
const PDDL_1: Licence = Licence {
    name: "Open Data Commons Public Domain Dedication and License (PDDL) 1.0",
    uri: "http://www.opendatacommons.org/licenses/pddl/1.0/",
};
const ISA_OPEN_METADATA_1_1: Licence = Licence {
    name: "ISA Open Metadata Licence v1.1",
    uri: "https://interoperable-europe.ec.europa.eu/licence/isa-open-metadata-licence-v11",
};
const ODC_BY_1: Licence = Licence {
    name: "Open Data Commons Attribution License (ODC-BY) 1.0",
    uri: "https://opendatacommons.org/licenses/by/1-0/",
};

/// DCMI's Schema Use Notice, which it asks software using its RDF schemas to carry.
const DCMI_SCHEMA_NOTICE: &str = "Portions of this software may use RDF schemas Copyright © 2011 \
     DCMI, the Dublin Core™ Metadata Initiative. These are licensed under the Creative Commons 4.0 \
     Attribution license.";

// ─── W3C Document License (2023) ───────────────────────────────────────────────

pub const RDF: BundledFile = bundled!("rdf.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2019 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!(
        "The RDF Concepts Vocabulary (RDF), https://www.w3.org/1999/02/22-rdf-syntax-ns.ttl."
    )),
    status: Some("Defined in RDF 1.1 Concepts and Abstract Syntax, W3C Recommendation \
                  25 February 2014, https://www.w3.org/TR/2014/REC-rdf11-concepts-20140225/"),
    source: "https://www.w3.org/1999/02/22-rdf-syntax-ns.ttl",
});

pub const RDFS: BundledFile = bundled!("rdfs.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2014 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!(
        "The RDF Schema vocabulary (RDFS), https://www.w3.org/2000/01/rdf-schema.ttl."
    )),
    status: Some("Defined in RDF Schema 1.1, W3C Recommendation 25 February 2014, \
                  https://www.w3.org/TR/2014/REC-rdf-schema-20140225/"),
    source: "https://www.w3.org/2000/01/rdf-schema.ttl",
});

pub const OWL: BundledFile = bundled!("owl.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2009 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!(
        "The OWL 2 Schema vocabulary (OWL 2), https://www.w3.org/2002/07/owl.ttl."
    )),
    status: Some("Defined in OWL 2 Web Ontology Language (Second Edition), W3C Recommendation \
                  11 December 2012, https://www.w3.org/TR/2012/REC-owl2-overview-20121211/"),
    source: "https://www.w3.org/2002/07/owl.ttl",
});

pub const SHACL: BundledFile = bundled!("shacl.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2017 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!(
        "W3C Shapes Constraint Language (SHACL) Vocabulary, https://www.w3.org/ns/shacl.ttl."
    )),
    status: Some("Defined in Shapes Constraint Language (SHACL), W3C Recommendation 20 July 2017, \
                  https://www.w3.org/TR/2017/REC-shacl-20170720/"),
    source: "https://www.w3.org/ns/shacl.ttl",
});

pub const PROV: BundledFile = bundled!("prov.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2013 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!(
        "W3C PROVenance Interchange Ontology (PROV-O), https://www.w3.org/ns/prov-o.ttl."
    )),
    status: Some("Defined in PROV-O: The PROV Ontology, W3C Recommendation 30 April 2013, \
                  https://www.w3.org/TR/2013/REC-prov-o-20130430/"),
    source: "https://www.w3.org/ns/prov-o.ttl",
    store_form: Some("1 xsd:nonNegativeInteger literal (an OWL cardinality) reads as xsd:integer"),
});

pub const SKOS: BundledFile = bundled!("skos.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2009 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!(
        "SKOS Vocabulary, https://www.w3.org/2009/08/skos-reference/skos.rdf."
    )),
    status: Some("Defined in SKOS Simple Knowledge Organization System Reference, W3C \
                  Recommendation 18 August 2009, \
                  https://www.w3.org/TR/2009/REC-skos-reference-20090818/"),
    source: "http://www.w3.org/2004/02/skos/core.rdf",
});

pub const OA: BundledFile = bundled!("oa.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2016 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!("Web Annotation Vocabulary, https://www.w3.org/ns/oa.")),
    status: Some("Defined in Web Annotation Vocabulary, W3C Recommendation 23 February 2017, \
                  https://www.w3.org/TR/2017/REC-annotation-vocab-20170223/"),
    source: "http://www.w3.org/ns/oa",
    remarks: Some("The ontology itself says that changes to it must come from a W3C Working \
                   Group. No triple has been changed."),
});

pub const DQV: BundledFile = bundled!("dqv.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2016 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!("Data Quality Vocabulary, https://www.w3.org/ns/dqv.")),
    status: Some("Defined in Data on the Web Best Practices: Data Quality Vocabulary, W3C Working \
                  Group Note 15 December 2016, \
                  https://www.w3.org/TR/2016/NOTE-vocab-dqv-20161215/"),
    source: "http://www.w3.org/ns/dqv",
});

pub const VCARD: BundledFile = bundled!("vcard.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2014 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!("Ontology for vCard, https://www.w3.org/2006/vcard/ns.")),
    status: Some("Defined in vCard Ontology - for describing People and Organizations, W3C \
                  Interest Group Note 22 May 2014, \
                  https://www.w3.org/TR/2014/NOTE-vcard-rdf-20140522/"),
    source: "http://www.w3.org/2006/vcard/ns",
    store_form: Some("12 xsd:nonNegativeInteger literals (OWL cardinalities) read as xsd:integer"),
});

pub const VS: BundledFile = bundled!("vs.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2011 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!(
        "SemWeb Vocab Status ontology, https://www.w3.org/2003/06/sw-vocab-status/ns."
    )),
    status: Some("Described in Term-centric Semantic Web Vocabulary Annotations, (Editor's Draft \
                  of a potential) W3C Interest Group Note 31 December 2009, \
                  https://www.w3.org/2003/06/sw-vocab-status/note"),
    source: "http://www.w3.org/2003/06/sw-vocab-status/ns",
});

pub const WGS84: BundledFile = bundled!("wgs84.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2009 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!(
        "WGS84 Geo Positioning: an RDF vocabulary, http://www.w3.org/2003/01/geo/wgs84_pos."
    )),
    status: Some("An informal document of the W3C Semantic Web Interest Group \
                  (https://www.w3.org/2003/01/geo/) with no formal W3C status."),
    source: "http://www.w3.org/2003/01/geo/wgs84_pos",
});

pub const CSVW: BundledFile = bundled!("csvw.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2015 World Wide Web Consortium. \
                  https://www.w3.org/copyright/document-license-2023/"],
    notice: Some(w3c_notice!("CSVW Namespace Vocabulary Terms, http://www.w3.org/ns/csvw.")),
    status: Some("namespace document of Metadata Vocabulary for Tabular Data, W3C Recommendation \
                  17 December 2015, https://www.w3.org/TR/2015/REC-tabular-metadata-20151217/"),
    source: "http://www.w3.org/ns/csvw",
});

pub const ADMS: BundledFile = bundled!("adms.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE, ISA_OPEN_METADATA_1_1],
    copyright: &[
        "Copyright © 2015 World Wide Web Consortium. \
         https://www.w3.org/copyright/document-license-2023/",
        "Copyright © 2012 European Union.",
    ],
    notice: Some(w3c_notice!(
        "Asset Description Metadata Schema (ADMS), https://www.w3.org/ns/legacy_adms."
    )),
    status: Some("described by Asset Description Metadata Schema (ADMS), W3C Working Group Note \
                  01 August 2013 (Retired August 2023), \
                  https://www.w3.org/TR/2013/NOTE-vocab-adms-20130801/"),
    source: "https://www.w3.org/ns/legacy_adms.ttl",
    remarks: Some("Its definitions derive from ADMS v1.00 of the European Commission's ISA \
                   Programme, published under the ISA Open Metadata Licence v1.1, whose \
                   \"No Warranty\" disclaimer is reproduced in the full notice."),
});

/// Written by Open Triplestore; only some descriptions are W3C material.
pub const XSD: BundledFile = bundled!("xsd.ttl", {
    licenses: &[W3C_DOCUMENT_LICENSE],
    copyright: &[
        "Copyright © 2004 W3C® (MIT, ERCIM, Keio), All Rights Reserved",
        "Copyright © 2012 W3C® (MIT, ERCIM, Keio), All Rights Reserved",
    ],
    notice: Some(w3c_notice!(
        "XML Schema Part 2: Datatypes Second Edition, \
         https://www.w3.org/TR/2004/REC-xmlschema-2-20041028/, and W3C XML Schema Definition \
         Language (XSD) 1.1 Part 2: Datatypes, \
         https://www.w3.org/TR/2012/REC-xmlschema11-2-20120405/."
    )),
    status: Some("Several descriptions are copied from or derived from XML Schema Part 2: \
                  Datatypes Second Edition, W3C Recommendation 28 October 2004, \
                  https://www.w3.org/TR/2004/REC-xmlschema-2-20041028/ (Copyright © 2004 W3C® \
                  (MIT, ERCIM, Keio), All Rights Reserved) and W3C XML Schema Definition \
                  Language (XSD) 1.1 Part 2: Datatypes, W3C Recommendation 5 April 2012, \
                  https://www.w3.org/TR/2012/REC-xmlschema11-2-20120405/ (Copyright © 2012 W3C® \
                  (MIT, ERCIM, Keio), All Rights Reserved)."),
    source: "https://www.w3.org/TR/2012/REC-xmlschema11-2-20120405/",
    changes: Some("Written by Open Triplestore, because there is no normative Turtle vocabulary \
                   for XSD; several of its descriptions are copied from or derived from the two \
                   XSD Recommendations named in its status."),
    remarks: Some("Those W3C-derived descriptions remain under the W3C Document License (2023), \
                   https://www.w3.org/copyright/document-license-2023/; the rest of this file is \
                   Open Triplestore's own work under the project's licence (see LICENSE)."),
});

// ─── W3C Software and Document License (2015), OGC Software License 1.0 ───────

pub const ODRL: BundledFile = bundled!("odrl.ttl", {
    licenses: &[W3C_SOFTWARE_AND_DOCUMENT_LICENSE],
    copyright: &["Copyright © 2018 W3C® (MIT, ERCIM, Keio, Beihang)."],
    notice: Some("This software or document includes material copied from or derived from ODRL \
                  Version 2.2, http://www.w3.org/ns/odrl/2/."),
    status: Some("Defined in ODRL Vocabulary & Expression 2.2, W3C Recommendation 15 February \
                  2018, https://www.w3.org/TR/2018/REC-odrl-vocab-20180215/"),
    source: "http://www.w3.org/ns/odrl/2/",
    remarks: Some("The Recommendation and the w3c/poe repository both apply this licence. The \
                   file's own dcterms:license, kept, points to W3C's 2002 IPR notice."),
});

pub const SOSA: BundledFile = bundled!("sosa.ttl", {
    licenses: &[W3C_SOFTWARE_AND_DOCUMENT_LICENSE, OGC_SOFTWARE_LICENSE],
    copyright: &["Copyright 2017 W3C/OGC."],
    notice: Some("This software or document includes material copied from or derived from \
                  Sensor, Observation, Sample, and Actuator (SOSA) Ontology, \
                  http://www.w3.org/ns/sosa/. Copyright © 2017 W3C® (MIT, ERCIM, Keio, \
                  Beihang)."),
    status: Some("Defined in Semantic Sensor Network Ontology, W3C Recommendation 19 October \
                  2017, by the joint W3C/OGC Spatial Data on the Web Working Group, \
                  https://www.w3.org/TR/2017/REC-vocab-ssn-20171019/"),
    source: "https://github.com/w3c/sdw/blob/gh-pages/ssn/integrated/sosa.ttl",
});

pub const SSN: BundledFile = bundled!("ssn.ttl", {
    licenses: &[W3C_SOFTWARE_AND_DOCUMENT_LICENSE, OGC_SOFTWARE_LICENSE],
    copyright: &["Copyright 2017 W3C/OGC."],
    notice: Some("This software or document includes material copied from or derived from \
                  Semantic Sensor Network Ontology, http://www.w3.org/ns/ssn/. Copyright © 2017 \
                  W3C® (MIT, ERCIM, Keio, Beihang)."),
    status: Some("Defined in Semantic Sensor Network Ontology, W3C Recommendation 19 October \
                  2017, by the joint W3C/OGC Spatial Data on the Web Working Group, \
                  https://www.w3.org/TR/2017/REC-vocab-ssn-20171019/"),
    source: "https://github.com/w3c/sdw/blob/gh-pages/ssn/integrated/ssn.ttl",
    store_form: Some("25 xsd:nonNegativeInteger literals (OWL cardinalities) read as xsd:integer"),
});

// ─── Creative Commons Attribution 4.0 ──────────────────────────────────────────

pub const DCAT: BundledFile = bundled!("dcat.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Copyright © 2024 World Wide Web Consortium."],
    status: Some("Defined in Data Catalog Vocabulary (DCAT) - Version 3, W3C Recommendation \
                  22 August 2024, https://www.w3.org/TR/2024/REC-vocab-dcat-3-20240822/"),
    source: "https://www.w3.org/ns/dcat.ttl",
    store_form: Some("1 xsd:nonNegativeInteger literal (an OWL cardinality) reads as xsd:integer"),
});

pub const DCAT_2: BundledFile = bundled!("dcat/2.0.0.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Copyright © 2020 W3C® (MIT, ERCIM, Keio, Beihang)."],
    status: Some("Defined in Data Catalog Vocabulary (DCAT) - Version 2, W3C Recommendation \
                  04 February 2020, https://www.w3.org/TR/2020/REC-vocab-dcat-2-20200204/"),
    source: "https://www.w3.org/ns/dcat2.ttl",
    store_form: Some("1 xsd:nonNegativeInteger literal (an OWL cardinality) reads as xsd:integer"),
});

pub const TIME: BundledFile = bundled!("time.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Copyright © 2006-2017 W3C, OGC. W3C and OGC liability, trademark and document \
                  use rules apply."],
    status: Some("Defined in Time Ontology in OWL, W3C Recommendation 19 October 2017, by the \
                  joint W3C/OGC Spatial Data on the Web Working Group, \
                  https://www.w3.org/TR/2017/REC-owl-time-20171019/"),
    source: "https://www.w3.org/2006/time.ttl",
    store_form: Some("32 xsd:nonNegativeInteger literals (OWL cardinalities) read as xsd:integer"),
});

pub const DCTERMS: BundledFile = bundled!("dcterms.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Copyright © 2011 DCMI, the Dublin Core™ Metadata Initiative."],
    notice: Some(DCMI_SCHEMA_NOTICE),
    source: "https://www.dublincore.org/specifications/dublin-core/dcmi-terms/dublin_core_terms.ttl",
});

pub const DCTERMS_2012: BundledFile = bundled!("dcterms/2012.06.14.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Copyright © 2011 DCMI, the Dublin Core™ Metadata Initiative."],
    notice: Some(DCMI_SCHEMA_NOTICE),
    source: "http://dublincore.org/2012/06/14/dcterms.ttl",
});

pub const DCTYPE: BundledFile = bundled!("dctype.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Copyright © 2011 DCMI, the Dublin Core™ Metadata Initiative."],
    notice: Some(DCMI_SCHEMA_NOTICE),
    source: "http://purl.org/dc/dcmitype/",
});

pub const BIBO: BundledFile = bundled!("bibo.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Copyright © 2008-2013 by Structured Dynamics LLC."],
    status: Some("BIBO is now maintained by DCMI: DCMI Community Specification, latest update \
                  2016-05-11."),
    source: "http://purl.org/ontology/bibo/",
    remarks: Some("The original Structured Dynamics specification was licensed CC BY 1.0 and \
                   stated that its copyright does not apply to the ontology terms."),
    store_form: Some("10 xsd:nonNegativeInteger literals (OWL cardinalities) read as xsd:integer"),
});

pub const CC: BundledFile = bundled!("cc.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Creator: Creative Commons."],
    source: "http://creativecommons.org/schema.rdf",
    remarks: Some("No endorsement by Creative Commons is implied."),
});

pub const FOG: BundledFile = bundled!("fog.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Authors: Anna Wagner, Mathias Bonduel, Pieter Pauwels"],
    source: "https://w3id.org/fog",
});

pub const OMG: BundledFile = bundled!("omg.ttl", {
    licenses: &[CC_BY_4],
    copyright: &["Authors: Anna Wagner (TU Darmstadt), Mathias Bonduel (KU Leuven), Pieter \
                  Pauwels (Ghent University)"],
    source: "https://w3id.org/omg",
    remarks: Some("The authors state it on https://w3id.org/omg and in the repository's \
                   documentation configuration. The Turtle file itself has no licence \
                   statement."),
});

// ─── Creative Commons Attribution 3.0 and 1.0 ──────────────────────────────────

pub const GR: BundledFile = bundled!("gr.ttl", {
    licenses: &[CC_BY_3_UNPORTED],
    notice: Some("This work is based on the GoodRelations ontology, developed by Martin Hepp. \
                  http://purl.org/goodrelations/"),
    source: "http://purl.org/goodrelations/v1",
});

pub const VOAF: BundledFile = bundled!("voaf.ttl", {
    licenses: &[CC_BY_3],
    copyright: &["Creator: Bernard Vatant; contributors: Pierre-Yves Vandenbussche, Lise Rozat; \
                  publisher: Open Knowledge Foundation"],
    source: "http://purl.org/vocommons/voaf",
});

pub const FOAF: BundledFile = bundled!("foaf.ttl", {
    licenses: &[CC_BY_1],
    copyright: &["Copyright © 2000-2014 Dan Brickley and Libby Miller. This copyright applies to \
                  the FOAF Vocabulary Specification and accompanying documentation in RDF."],
    source: "http://xmlns.com/foaf/spec/index.rdf",
});

pub const VANN: BundledFile = bundled!("vann.ttl", {
    licenses: &[CC_BY_1],
    copyright: &["Copyright © 2005 Ian Davis."],
    source: "https://vocab.org/vann/vann-vocab-20100607.rdf",
});

pub const SIOC: BundledFile = bundled!("sioc.ttl", {
    licenses: &[CC_BY_1],
    copyright: &["Copyright © 2004-2010 by DERI, NUI Galway."],
    source: "http://rdfs.org/sioc/ns",
    remarks: Some("The publisher states that this copyright does not apply to SIOC data formats, \
                   ontology terms, or technology."),
});

pub const BOT: BundledFile = bundled!("bot.ttl", {
    licenses: &[Licence {
        name: "CC BY 1.0",
        uri: "https://creativecommons.org/licenses/by/1.0/",
    }],
    // The upstream's own comment header, below the added one: lost in the store.
    copyright: &["Copyright 2017-2020 W3C Linked Building Data Community Group"],
    source: "https://raw.githubusercontent.com/w3c-lbd-cg/bot/master/bot.ttl",
});

// ─── Creative Commons Attribution-ShareAlike 3.0 ───────────────────────────────

pub const SCHEMA: BundledFile = bundled!("schema.ttl", {
    licenses: &[CC_BY_SA_3],
    copyright: &["Copyright Google, Inc., Yahoo, Inc., Microsoft Corporation and Yandex (the \
                  Schema.org Sponsors)."],
    notice: Some("This adaptation is released under CC BY-SA 3.0, \
                  http://creativecommons.org/licenses/by-sa/3.0/; the Open Triplestore licence \
                  terms (AGPL-3.0 with the Commons Clause) do not apply to it. No endorsement by \
                  Schema.org or its Sponsors is implied."),
    // Release 30.0's data. https://schema.org/version/30.0/ answers 404
    // (checked 2026-09-23); this is where the release is published.
    source: "https://github.com/schemaorg/schemaorg/tree/main/data/releases/30.0",
    changes: Some("An adaptation of Schema.org release 30.0 (2026-03-19): Open Triplestore kept \
                   only 27 terms, 12 classes and 15 properties, each with all of its triples \
                   from release 30.0 unchanged, and states each term a second time under its \
                   http://schema.org/ IRI (objects stay https://schema.org/) so data using \
                   either namespace resolves. A comment header was added."),
});

// ─── Open Data Commons PDDL 1.0 ────────────────────────────────────────────────

pub const ORG: BundledFile = bundled!("org.ttl", {
    licenses: &[PDDL_1],
    status: Some("Defined in The Organization Ontology, W3C Recommendation 16 January 2014, \
                  https://www.w3.org/TR/2014/REC-vocab-org-20140116/"),
    source: "https://www.w3.org/ns/org.ttl",
});

pub const QB: BundledFile = bundled!("qb.ttl", {
    licenses: &[PDDL_1],
    status: Some("Defined in The RDF Data Cube Vocabulary, W3C Recommendation 16 January 2014, \
                  https://www.w3.org/TR/2014/REC-vocab-data-cube-20140116/"),
    source: "https://raw.githubusercontent.com/UKGovLD/publishing-statistical-data/master/specs/\
             src/main/vocab/cube.ttl",
});

// ─── Apache License 2.0 ────────────────────────────────────────────────────────

pub const PAV: BundledFile = bundled!("pav.ttl", {
    licenses: &[Licence {
        name: "Apache License 2.0",
        uri: "http://www.apache.org/licenses/LICENSE-2.0",
    }],
    copyright: &["Copyright 2008-2014 Massachusetts General Hospital; Harvard Medical School; \
                  Balboa Systems; University of Manchester."],
    source: "http://purl.org/pav/",
    store_form: Some("2 xsd:dateTime literals read with the time zone Z instead of +00:00"),
});

pub const GEOSPARQL: BundledFile = bundled!("geosparql.ttl", {
    licenses: &[Licence {
        name: "Apache License 2.0",
        uri: "https://www.apache.org/licenses/LICENSE-2.0",
    }],
    copyright: &["(c) 2021 Open Geospatial Consortium."],
    source: "https://github.com/opengeospatial/geosemantics-semantic-resources/blob/main/\
             resources/geosparql-swg/geosparql-1.1/ontologies/geo.ttl",
    remarks: Some("OGC's own server (http://www.opengis.net/ont/geosparql) offers the same \
                   version with a licence triple pointing to the OGC Document License Agreement; \
                   Open Triplestore relies on the Apache-2.0 grant."),
});

pub const GEOSPARQL_1_0: BundledFile = bundled!("geosparql/1.0.0.ttl", {
    licenses: &[
        Licence {
            name: "Apache License 2.0",
            uri: "https://www.apache.org/licenses/LICENSE-2.0",
        },
        CC_BY_4,
    ],
    copyright: &[
        "Copyright (c) 2012 Open Geospatial Consortium.",
        "Copyright © 2011 DCMI, the Dublin Core™ Metadata Initiative.",
    ],
    notice: Some(DCMI_SCHEMA_NOTICE),
    source: "https://github.com/opengeospatial/geosemantics-semantic-resources/blob/main/\
             resources/geosparql-swg/geosparql-1.0/ontologies/geo.ttl",
    remarks: Some("Includes the DCMI Metadata Element Set definitions (dc:contributor … \
                   dc:type), which fall under DCMI's CC BY 4.0 Schema Use Notice."),
});

// ─── BSD 3-Clause (ETSI) ───────────────────────────────────────────────────────

pub const SAREF: BundledFile = bundled!("saref.ttl", {
    licenses: &[Licence {
        name: "BSD-3-Clause",
        uri: "https://forge.etsi.org/etsi-software-license",
    }],
    copyright: &["Copyright 2019 ETSI"],
    source: "https://saref.etsi.org/core/v3.1.1/saref.ttl",
    remarks: Some("The licence's conditions and disclaimer, with ETSI's copyright notice, are in \
                   the full notice."),
    store_form: Some("12 xsd:nonNegativeInteger literals (OWL cardinalities) read as xsd:integer"),
});

// ─── ISA Open Metadata Licence v1.1 ────────────────────────────────────────────

pub const LOCN: BundledFile = bundled!("locn.ttl", {
    licenses: &[ISA_OPEN_METADATA_1_1],
    copyright: &["Copyright © European Union, 2012-2015."],
    notice: Some(
        "EACH WORK IS PROVIDED \"AS IS\" WITHOUT REPRESENTATIONS, WARRANTIES, OBLIGATIONS AND \
         LIABILITIES OF ANY KIND, EITHER EXPRESS OR IMPLIED, TO THE FULL EXTENT PERMITTED BY LAW \
         INCLUDING, BUT NOT LIMITED TO, ANY IMPLIED WARRANTY OF MERCHANTABILITY, INTEGRATION, \
         SATISFACTORY QUALITY AND FITNESS FOR A PARTICULAR PURPOSE. EXCEPT IN THE CASES OF \
         WILFUL MISCONDUCT OR DAMAGES DIRECTLY CAUSED TO NATURAL PERSONS, NEITHER EUROPEAN UNION \
         NOR ITS CONTRIBUTOR(S) WILL BE LIABLE FOR ANY INCIDENTAL, CONSEQUENTIAL, DIRECT OR \
         INDIRECT DAMAGES INCLUDING BUT NOT LIMITED TO THE LOSS OF DATA, LOST PROFITS, OR ANY \
         OTHER FINANCIAL LOSS ARISING FROM THE USE OF, OR INABILITY TO USE, EVEN IF THE EUROPEAN \
         UNION HAS BEEN NOTIFIED OF THE POSSIBILITY OF SUCH LOSS, DAMAGES, CLAIMS OR COSTS OR FOR \
         ANY CLAIM BY ANY THIRD PARTY. HOWEVER, THE LICENSOR WILL BE LIABLE UNDER STATUTORY \
         PRODUCT LIABILITY LAWS AS FAR SUCH LAWS APPLY TO THE WORK."
    ),
    source: "http://www.w3.org/ns/locn",
    remarks: Some("The notice above is the licence's \"No Warranty\" disclaimer, which every \
                   distribution must keep."),
    store_form: Some("2 xsd:nonNegativeInteger literals (VOAF term counts) read as xsd:integer"),
});

// ─── IMBOR (Stichting CROW) ────────────────────────────────────────────────────

/// Byte-identical to CROW's release file and allowed no altered copies: it has
/// no header, and nothing may be added to it or to its stored graph.
pub const IMBOR: BundledFile = bundled!("imbor.ttl", {
    licenses: &[CC_BY_4, ODC_BY_1],
    copyright: &["© Stichting CROW"],
    source: "https://github.com/Stichting-CROW/imbor/releases/tag/2025",
    changes: Some("None: the file is byte-identical to imbor2025-vocabulaire.ttl in CROW's IMBOR \
                   2025 Linked Data release."),
    remarks: Some("CROW licenses the IMBOR data under CC BY 4.0 and the IMBOR products, which \
                   include this Linked Data release, under ODC-BY 1.0. CROW's Beheerplan IMBOR \
                   (version 1.0, 2 July 2025) names CC BY-ND 4.0 \
                   (https://creativecommons.org/licenses/by-nd/4.0/) for its open standards. \
                   Open Triplestore follows the stricter reading: attribution to Stichting CROW, \
                   the licence URIs, and no altered copies."),
    no_derivatives: true,
});

// ─── DOAP (licence unresolved) ─────────────────────────────────────────────────

pub const DOAP: BundledFile = bundled!("doap.ttl", {
    copyright: &["Copyright © 2004-2009 Edd Dumbill"],
    source: "http://usefulinc.com/ns/doap",
    changes: Some("Re-serialized as Turtle by LOV; Open Triplestore added a comment header in \
                   2026-07."),
    remarks: Some("The file states no licence. The upstream repository \
                   https://github.com/ewilderj/doap has been under the Apache License 2.0 since \
                   2018-03-31, which covers most of this file; its 97 Japanese-language labels \
                   and comments and a few other strings have never been in that repository, and \
                   no licence has been published for them."),
});

// ─── Open Triplestore's own vocabulary ─────────────────────────────────────────

pub const OTS: BundledFile = bundled!("ots.ttl", {
    third_party: false,
    source: "https://opentriplestore.org/ns#",
});

/// Every bundled file.
pub const ALL: &[&BundledFile] = &[
    &RDF,
    &RDFS,
    &OWL,
    &SHACL,
    &PROV,
    &SKOS,
    &OA,
    &DQV,
    &VCARD,
    &VS,
    &WGS84,
    &CSVW,
    &ADMS,
    &XSD,
    &ODRL,
    &SOSA,
    &SSN,
    &DCAT,
    &DCAT_2,
    &TIME,
    &DCTERMS,
    &DCTERMS_2012,
    &DCTYPE,
    &BIBO,
    &CC,
    &FOG,
    &OMG,
    &GR,
    &VOAF,
    &FOAF,
    &VANN,
    &SIOC,
    &BOT,
    &SCHEMA,
    &ORG,
    &QB,
    &PAV,
    &GEOSPARQL,
    &GEOSPARQL_1_0,
    &SAREF,
    &LOCN,
    &IMBOR,
    &DOAP,
    &OTS,
];

// ─── Header and attribution ────────────────────────────────────────────────────

/// Whether `line` is the marker after which a file's upstream content starts.
fn is_upstream_marker(line: &str) -> bool {
    line.starts_with("# ----") && line.contains("starts below")
}

impl BundledFile {
    /// The file's own comment header: its leading `#` lines up to the marker
    /// line (or the first line that is not a comment), without the `# `
    /// prefixes.
    pub fn header_lines(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for line in self.ttl.lines() {
            if !line.starts_with('#') || is_upstream_marker(line) {
                break;
            }
            let text = line.strip_prefix("# ").unwrap_or(&line[1..]);
            out.push(text.trim_end());
        }
        out
    }

    /// [`Self::header_lines`] joined by newlines, `None` for a file without one.
    pub fn header(&self) -> Option<String> {
        let lines = self.header_lines();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// The header's "Changes…" statement, whitespace-normalised, unless the
    /// table gives one.
    pub fn changes(&self) -> Option<String> {
        if let Some(c) = self.changes {
            return Some(normalize(c));
        }
        let header = normalize(&self.header_lines().join(" "));
        let start = header.find("Changes")?;
        let rest = &header[start..];
        let end = rest.find(" Full attribution").unwrap_or(rest.len());
        Some(rest[..end].trim().to_string())
    }

    /// The licence and attribution record the registry stores for a seeded copy
    /// of this file. `unchanged` says whether the seeder found the stored
    /// triples to be exactly this file's (see `seed_vocab`); the record states
    /// "unchanged" only then. `None` for Open Triplestore's own vocabulary.
    pub fn attribution(
        &self,
        specification_url: Option<&str>,
        unchanged: bool,
    ) -> Option<ContentAttribution> {
        if !self.third_party {
            return None;
        }
        let stored_copy =
            seeded_stored_copy(self.path, self.no_derivatives, unchanged, self.store_form);
        Some(ContentAttribution {
            file: self.path.to_string(),
            licenses: self
                .licenses
                .iter()
                .map(|l| LicenseRef {
                    name: l.name.to_string(),
                    uri: l.uri.to_string(),
                })
                .collect(),
            copyright: self.copyright.iter().map(|c| normalize(c)).collect(),
            notice: self.notice.map(normalize),
            status: self.status.map(normalize),
            source_url: self.source.to_string(),
            specification_url: specification_url.map(str::to_string),
            changes: self.changes(),
            stored_copy,
            unchanged,
            remarks: self.remarks.map(normalize),
            no_derivatives: self.no_derivatives,
            header: self.header(),
            notice_url: NOTICE_PATH.to_string(),
        })
    }
}

/// How a seeded copy of the bundled file `file` relates to that file, as its
/// record's `stored_copy` says it: "unchanged" only for a copy the seeder has
/// checked; otherwise that it may have been modified and is not the file.
/// `store_form` is [`BundledFile::store_form`].
pub fn seeded_stored_copy(
    file: &str,
    no_derivatives: bool,
    unchanged: bool,
    store_form: Option<&str>,
) -> String {
    let form = store_form
        .map(|f| {
            format!(
                " The store writes typed literals in its canonical form, the same values, so {f}."
            )
        })
        .unwrap_or_default();
    match (unchanged, no_derivatives) {
        (true, true) => format!(
            "The store holds the triples of the bundled file vocab/{file}, unchanged: the seeder \
             checked them against the file.{form} API downloads are serialized anew from them, a \
             change of format only, and carry no added notice: the licence and attribution \
             travel in their HTTP Link headers and in this record."
        ),
        (true, false) => format!(
            "The store holds the triples of the bundled file vocab/{file}, unchanged: the seeder \
             checked them against the file.{form} It keeps no comments, so this record carries \
             the file's header; API downloads are serialized anew from the triples and start \
             with that header as a comment."
        ),
        (false, true) => format!(
            "This copy was loaded from the bundled file vocab/{file}, but its triples differ from \
             the file's or may since have been edited in this registry, so it is not that file. \
             The licence allows no altered copies, so the registry serves this copy to no one \
             without write access to this entry."
        ),
        (false, false) => format!(
            "This copy was loaded from the bundled file vocab/{file}, but its triples differ from \
             the file's or may since have been edited in this registry, so it is not that file. \
             It keeps no comments, so this record carries the file's header; API downloads start \
             with that header as a comment and say that the copy may have been modified."
        ),
    }
}

/// Whether `file` is the path of one of the bundled vocabulary files, as a
/// licence record's `file` names it.
pub fn is_bundled_path(file: &str) -> bool {
    ALL.iter().any(|f| f.path == file)
}

/// How registry texts name the source a licence record's `file` points at: a
/// bundled vocabulary file as "the bundled file vocab/{file}", anything else
/// (a LOV corpus graph, a seed bundle's payload files) as the record gives it.
pub fn source_phrase(file: &str) -> String {
    if is_bundled_path(file) {
        format!("the bundled file vocab/{file}")
    } else {
        file.to_string()
    }
}

/// `stored_copy` for a version edited in place through the registry's API (a
/// PATCH, or version metadata stamped on publish): it may have been
/// modified, so it is not its source.
pub fn edited_stored_copy(a: &ContentAttribution) -> String {
    if is_bundled_path(&a.file) {
        return seeded_stored_copy(&a.file, a.no_derivatives, false, None);
    }
    let withheld = if a.no_derivatives {
        " The licence allows no altered copies, so the registry serves this copy to no one \
         without write access to this entry."
    } else {
        ""
    };
    format!(
        "This copy was loaded from {}, but it may since have been edited in this registry, so it \
         is not that source.{withheld}",
        a.file
    )
}

/// `stored_copy` for a version whose graph was written directly (a SPARQL
/// Update or a Graph Store Protocol request) after it was loaded or checked:
/// it may have been modified, so it is not its source.
pub fn written_stored_copy(a: &ContentAttribution) -> String {
    let withheld = if a.no_derivatives {
        " The licence allows no altered copies, so the registry serves this copy to no one \
         without write access to this entry."
    } else {
        ""
    };
    format!(
        "This copy was loaded from {}, but its graph was written directly in this registry \
         since (a SPARQL Update or a Graph Store Protocol request), so it may have been modified \
         and is not that source.{withheld}",
        source_phrase(&a.file)
    )
}

/// Collapse every run of whitespace to one space (the table's strings are
/// wrapped with `\` continuations; the headers are wrapped at 80 columns).
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `Link` header values for a response serving attributed content (a download
/// or a term's description): each licence, the source and the full notice.
pub fn link_header_values(a: &ContentAttribution, base_url: &str) -> Vec<String> {
    let mut links: Vec<String> = a
        .licenses
        .iter()
        .map(|l| format!("<{}>; rel=\"license\"", l.uri))
        .collect();
    links.push(format!("<{}>; rel=\"via\"", a.source_url));
    links.push(format!("<{base_url}{NOTICE_PATH}>; rel=\"describedby\""));
    links
}

/// The `#` comment preamble a download of a seeded copy starts with: the file's
/// header verbatim, any copyright line the header does not repeat (BOT's lives
/// in the upstream comments the store dropped), and where the copy comes from.
/// It calls the content the bundled file's triples, unchanged, only when the
/// record says the seeder checked them (`unchanged`); a draft, branch, merge,
/// rebase or edited copy is said to be derived from the file and possibly
/// modified. `None` for content that must not be altered.
pub fn download_preamble(a: &ContentAttribution, notice_url: &str) -> Option<String> {
    if a.no_derivatives {
        return None;
    }
    let mut out = String::new();
    let header = a.header.as_deref().unwrap_or("");
    for line in header.lines() {
        if line.is_empty() {
            out.push_str("#\n");
        } else {
            out.push_str(&format!("# {line}\n"));
        }
    }
    let flat_header = normalize(header);
    for c in &a.copyright {
        if !flat_header.contains(c.as_str()) {
            out.push_str(&format!("# {c}\n"));
        }
    }
    if header.is_empty() {
        for l in &a.licenses {
            out.push_str(&format!("# Licence: {}, {}\n", l.name, l.uri));
        }
        if let Some(n) = &a.notice {
            out.push_str(&format!("# {n}\n"));
        }
    }
    let source = source_phrase(&a.file);
    if a.unchanged {
        out.push_str(&format!(
            "# ---- Served by Open Triplestore: the triples of {source}, unchanged, serialized \
             anew from the store (which writes typed literals in their canonical form). ----\n"
        ));
    } else {
        out.push_str(&format!(
            "# ---- Served by Open Triplestore: derived from {source}; this copy may have been \
             modified in this registry, so it is not that source. ----\n"
        ));
    }
    out.push_str(&format!(
        "# Full attribution and licence texts: {notice_url}\n\n"
    ));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whitespace-normalised text of a file's whole leading comment block (the
    /// added header *and* any upstream header below it).
    fn leading_comments(f: &BundledFile) -> String {
        let lines: Vec<&str> = f
            .ttl
            .lines()
            .take_while(|l| l.starts_with('#'))
            .map(|l| l.trim_start_matches('#'))
            .collect();
        normalize(&lines.join(" "))
    }

    fn notice_md() -> String {
        normalize(&NOTICE_MD.replace("**", "").replace('`', ""))
    }

    /// Sources checked live elsewhere, because the text cites a dead link.
    const SOURCES_CHECKED_ELSEWHERE: &[&str] =
        &["https://github.com/schemaorg/schemaorg/tree/main/data/releases/30.0"];

    /// Every licence URI, copyright line, notice, status and source in the
    /// table appears verbatim in the file's own comments or in NOTICE.md, so a
    /// typo or a drifting header fails here instead of shipping a wrong notice.
    #[test]
    fn every_notice_field_is_verbatim_from_the_header_or_notice_md() {
        let notice = notice_md();
        for f in ALL.iter().filter(|f| f.third_party) {
            let comments = leading_comments(f);
            let found = |s: &str| {
                let s = normalize(s);
                comments.contains(&s) || notice.contains(&s)
            };
            for l in f.licenses {
                assert!(
                    found(l.uri),
                    "{}: licence URI {} not in its texts",
                    f.path,
                    l.uri
                );
            }
            for c in f.copyright {
                assert!(found(c), "{}: copyright {c:?} not in its texts", f.path);
            }
            if let Some(n) = f.notice {
                assert!(found(n), "{}: notice {n:?} not in its texts", f.path);
            }
            if let Some(s) = f.status {
                assert!(found(s), "{}: status {s:?} not in its texts", f.path);
            }
            assert!(
                found(f.source) || SOURCES_CHECKED_ELSEWHERE.contains(&f.source),
                "{}: source {} not in its texts",
                f.path,
                f.source
            );
            assert!(notice.contains(f.path), "{}: no entry in NOTICE.md", f.path);
        }
    }

    /// Each third-party file yields a complete record: a licence (except DOAP,
    /// whose file states none), a source, the changes and a notice link.
    #[test]
    fn every_third_party_file_has_a_complete_record() {
        for f in ALL {
            let Some(a) = f.attribution(None, true) else {
                assert!(!f.third_party, "{} lost its record", f.path);
                continue;
            };
            if f.path != "doap.ttl" {
                assert!(!a.licenses.is_empty(), "{}: no licence", f.path);
            }
            assert!(!a.source_url.is_empty(), "{}: no source", f.path);
            assert!(a.changes.is_some(), "{}: no changes statement", f.path);
            assert!(
                a.changes
                    .as_deref()
                    .is_some_and(|c| !c.contains("Full attribution")),
                "{}: changes ran past its sentence",
                f.path
            );
            assert_eq!(a.notice_url, NOTICE_PATH);
            assert_eq!(a.file, f.path);
            if f.path == "imbor.ttl" {
                assert!(a.header.is_none(), "IMBOR carries no header");
            } else {
                assert!(a.header.is_some(), "{}: header not extracted", f.path);
            }
        }
    }

    /// The header stops at the marker: the upstream comments below it (dcat 2's
    /// `# baseURI:` lines) are not part of Open Triplestore's notice.
    #[test]
    fn header_stops_at_the_upstream_marker() {
        let h = DCAT_2.header().unwrap();
        assert!(h.starts_with("Data Catalog Vocabulary (DCAT) — Version 2."));
        assert!(h.contains("Licence: CC BY 4.0"));
        assert!(!h.contains("baseURI"), "upstream comments leaked in: {h}");
        assert!(!h.contains("----"), "marker leaked in");
    }

    /// Only IMBOR is no-derivatives, and a download of it gets no preamble;
    /// every other download starts with the file's header as comments.
    #[test]
    fn preamble_is_withheld_only_from_imbor() {
        for f in ALL {
            let Some(a) = f.attribution(None, true) else {
                continue;
            };
            let pre = download_preamble(&a, "https://example.org/vocab/NOTICE.md");
            if f.path == "imbor.ttl" {
                assert!(a.no_derivatives);
                assert!(pre.is_none(), "IMBOR must not get an added notice");
            } else {
                assert!(!a.no_derivatives, "{}", f.path);
                let pre = pre.unwrap();
                assert!(
                    pre.lines().all(|l| l.is_empty() || l.starts_with('#')),
                    "{}: preamble must be comments only",
                    f.path
                );
                assert!(pre.contains("https://example.org/vocab/NOTICE.md"));
            }
        }
        // BOT's copyright sits in the upstream comments the store drops, so the
        // preamble restores it.
        let bot = download_preamble(&BOT.attribution(None, true).unwrap(), "/n").unwrap();
        assert!(bot.contains("# Copyright 2017-2020 W3C Linked Building Data Community Group"));
    }

    /// The preamble calls a download the bundled file's triples, unchanged,
    /// only for a checked copy; any other copy is said to be derived from the
    /// file and possibly modified, and the record's text agrees.
    #[test]
    fn preamble_says_unchanged_only_for_a_checked_copy() {
        let line = |a: &ContentAttribution| {
            download_preamble(a, "/n")
                .unwrap()
                .lines()
                .find(|l| l.starts_with("# ---- Served by Open Triplestore"))
                .unwrap()
                .to_string()
        };
        let checked = DCAT.attribution(None, true).unwrap();
        assert!(line(&checked).contains("vocab/dcat.ttl, unchanged"));
        assert!(checked.stored_copy.contains("unchanged"));

        let copy = DCAT.attribution(None, false).unwrap();
        let l = line(&copy);
        assert!(!l.contains("unchanged"), "{l}");
        assert!(l.contains("may have been modified"), "{l}");
        assert!(!copy.stored_copy.contains("unchanged"));
        assert!(copy.stored_copy.contains("may since have been edited"));
        // The file's own header, with its change notice, still leads.
        assert!(download_preamble(&copy, "/n")
            .unwrap()
            .starts_with("# Data Catalog Vocabulary (DCAT)"));

        // A record stored before the flag existed reads as not checked.
        let mut json = serde_json::to_value(&checked).unwrap();
        json.as_object_mut().unwrap().remove("unchanged");
        let old: ContentAttribution = serde_json::from_value(json).unwrap();
        assert!(!old.unchanged);
    }

    /// `store_form` names every file whose triples the store writes in
    /// another form, with the number of triples it touches.
    #[test]
    fn store_form_matches_a_round_trip_through_the_store() {
        use crate::data_models::{content_digest as cd, upload};
        for f in ALL {
            let parsed = cd::quads_as_triples(
                &upload::parse_rdf(f.ttl.as_bytes(), "text/turtle", f.path).unwrap(),
            );
            let stored: std::collections::HashSet<String> = cd::as_stored(&parsed)
                .unwrap()
                .iter()
                .map(|t| t.to_string())
                .collect();
            let touched = parsed
                .iter()
                .map(|t| t.to_string())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .filter(|t| !stored.contains(t))
                .count();
            match f.store_form {
                None => assert_eq!(
                    touched, 0,
                    "{}: the store rewrites {touched} triples",
                    f.path
                ),
                Some(text) => {
                    let n: usize = text.split(' ').next().unwrap().parse().unwrap();
                    assert_eq!(n, touched, "{}: {text}", f.path);
                }
            }
        }
        assert!(
            IMBOR.store_form.is_none(),
            "IMBOR is held exactly as CROW wrote it"
        );
    }

    /// The W3C derivative notice keeps the licence's exact wording.
    #[test]
    fn w3c_notice_matches_the_licence_form() {
        let n = RDF.attribution(None, true).unwrap().notice.unwrap();
        assert!(n.starts_with(
            "Copyright © 2023 W3C®. This software or document includes material copied from or \
             derived from The RDF Concepts Vocabulary (RDF)"
        ));
        assert!(NOTICE_MD.contains("from or derived from [title and URI of the W3C document]"));
    }
}
