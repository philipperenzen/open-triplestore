//! Seed the standard RDF vocabularies into the model/vocabulary registry as
//! **public, system-owned** entries, so OWL, RDF, RDFS, SKOS, DCAT, PROV, … are
//! browsable and queryable under `/api/models` out of the box — each with **all
//! of its real published versions** (e.g. OWL 1 / OWL 2, RDF 1.0/1.1/1.2-draft,
//! DCAT 1/2/3, GeoSPARQL 1.0/1.1, DCMI 2008/2012/2020), not a single synthetic
//! placeholder.
//!
//! The TTL sources are the canonical files the web UI uses for term lookup
//! (`frontend/public/vocab/*.ttl`), embedded at compile time so the seed needs no
//! network or filesystem at runtime. Each vocabulary's *current* version is its
//! flat `vocab/{id}.ttl` file; older versions either have a distinct snapshot
//! bundled under `vocab/{id}/{version}.ttl` or reuse another bundled file's
//! triples, and then say so in their `official_name`: RDF 1.0 / 1.2 and RDFS 1.0
//! reuse the RDF 1.1 / RDFS 1.1 namespace documents, OWL 1 the OWL 2 one, DCAT 1
//! the DCAT 3 file and DCMI 2008-01-14 the 2012-06-14 file. OWL profiles
//! (EL/QL/RL/DL/Full) are NOT versions — they are profiles of the one OWL 2
//! language and are not seeded here.
//!
//! **Licences.** Every bundled file keeps its publisher's licence (per file in
//! `frontend/public/vocab/NOTICE.md`). The store keeps triples only, so a file's
//! comment header, which carries its notices, does not survive the parse. The
//! seeder therefore records each file's licence and attribution
//! ([`vocab_files`], a [`ContentAttribution`]) as registry metadata on the
//! seeded entry and on every seeded version, never in the vocabulary's graph;
//! `/api/models` serves it and the UI shows it. The graphs themselves are loaded
//! verbatim ([`upload::load_parsed_verbatim`]): no triple is added, so each
//! stored copy holds exactly the bundled file's triples. IMBOR allows no altered
//! copies and the W3C Document License files are derivatives only with the
//! prescribed notice, so this matters.
//!
//! **"Unchanged" is checked, not assumed.** Each seeded version's registry
//! record also keeps the SHA-256 of the file it was loaded from or last
//! checked against and a digest of the triples as stored
//! ([`registry::set_seed_check`]). A record calls the copy the file's triples,
//! unchanged (and downloads say so), only when the seeder loaded it verbatim
//! or found it equal to the file, compared in the store's canonical literal
//! forms ([`content_digest::as_stored`]). An edit through the API (a PATCH,
//! version metadata stamped on publish) and a direct write into the graph (a
//! SPARQL Update or a Graph Store Protocol request, see
//! [`crate::data_models::write_guard`]) mark the record as possibly modified.
//!
//! **Only the seeder's own records.** Every entry and version the seeder
//! creates carries the registry marker `ver:seededBy` ([`SEEDED_BY`]). A record
//! from before the marker counts as the seeder's only when everything about it
//! reads as an earlier seeder wrote it: no creator on the version or the entry,
//! a registry id and version label from the seeder's own tables, the graph the
//! seeder loads into and no other, and exactly the notes and creation date the
//! seeder gave that version ([`version_owned`]). It then gets the marker. Any
//! other record (created through the API, promoted from a dataset, registered
//! by a seed bundle or a LOV install, or edited beyond recognition) is left
//! alone and the seeder only logs it: no version is added to such an entry, no
//! latest pointer is moved and no licence record is written there.
//!
//! **Never lose or overwrite anyone's data.** On every start the seeder checks
//! its own copies ([`sync_seeded_records`]):
//!
//! * a copy whose licence allows other copies and that differs from the
//!   bundled file (an admin's edit, or an earlier build's file) is never
//!   modified: it is kept exactly as stored, and its record says it differs
//!   from the file or may have been modified;
//! * a copy whose licence allows no altered copies (IMBOR) is checked on every
//!   start. One that differs is first copied, verified, into a new private
//!   version of the entry (`{version}-kept-{n}`: no creator, deprecated,
//!   withheld from everyone who may not write the entry), and only then is the
//!   canonical version restored to the file's triples, in one transaction, from
//!   a staging graph ([`verify_no_derivatives`]). A failure at any step leaves
//!   the stored data where it was and a record that does not say "unchanged";
//! * nothing is ever deleted: a version an earlier seeder made and this build
//!   no longer ships (IMBOR's hand-authored "excerpt") is kept, deprecated and
//!   withheld.
//!
//! Idempotent at the **version** level: a version already present is not
//! seeded again, so the seeder backfills only newly-bundled versions on upgrade
//! (the latest-version labels match what shipped previously, so existing
//! entries gain their historical versions without duplication). Best-effort —
//! a failure on one entry is logged and skipped. Opt out with
//! `SEED_STANDARD_VOCABS=false`: that stops seeding and the checks of the
//! other vocabularies, but not the per-start check and repair of a copy the
//! seeder already made of a vocabulary whose licence allows no altered copies.
//! The seeder does nothing on a node whose store is read-only (a replica, or a
//! cluster member that does not lead): the leader seeds and checks, and its
//! writes replicate.
//!
//! Installs seeded before real versioning shipped have these vocabularies at the
//! old synthetic `1.0.0`. That version is kept (deprecated, with a licence
//! record saying what it is); the real versions are added next to it and the
//! latest pointer moves to the real latest version.

use std::collections::HashMap;

use oxigraph::model::{GraphName, NamedNode, Quad, Triple};

use crate::data_models::models::{ContentAttribution, DataModelVersion, VersionStatus};
use crate::data_models::vocab_files::{self as vf, BundledFile};
use crate::data_models::{content_digest, registry, upload};
use crate::kind_detector::{self, RegistryKind};
use crate::server::AppState;

const DESC: &str = "Bundled standard vocabulary, seeded as a public reference.";

/// The synthetic version every bundled vocabulary was seeded under before
/// each got its real version(s) (releases before 0.4.0).
const LEGACY_SYNTHETIC_VERSION: &str = "1.0.0";

// ─── Seed table ─────────────────────────────────────────────────────────────────

/// One published (or draft) version of a standard vocabulary. The latest version's
/// label matches what shipped previously (single-version seeding) so existing
/// installs gain only their historical versions without duplication.
struct StdVersion {
    /// Registry/IRI version label (real published version of that standard). Must
    /// be IRI-safe (no spaces, `/` or `#`).
    version: &'static str,
    /// Official spec name for this version (stored as the version notes).
    official_name: &'static str,
    /// Publication date `yyyy-mm-dd` (drives `created_at` → chronological order).
    date: &'static str,
    /// Canonical spec URL for this version (recorded as the attribution's
    /// `specification_url`).
    spec_url: &'static str,
    status: VersionStatus,
    /// Exactly one non-draft version per vocabulary is the canonical latest.
    latest: bool,
    /// Prior version label, for the `prov:wasDerivedFrom` chain.
    prior: Option<&'static str>,
    /// The bundled file loaded into this version's graph, with its licence
    /// record. Several versions of the same vocab may share one file when no
    /// version-frozen document exists.
    file: &'static BundledFile,
}

impl StdVersion {
    /// The licence and attribution recorded for this version, `unchanged` as
    /// the seeder's check of the stored copy found it.
    fn attribution(&self, unchanged: bool) -> Option<ContentAttribution> {
        self.file.attribution(Some(self.spec_url), unchanged)
    }

    /// [`Self::attribution`], with `stored_copy` for a copy that is not
    /// unchanged when a more specific explanation is at hand.
    fn record(&self, unchanged: bool, stored_copy: Option<&str>) -> Option<ContentAttribution> {
        let mut a = self.attribution(unchanged)?;
        if let (false, Some(text)) = (unchanged, stored_copy) {
            a.stored_copy = text.to_string();
        }
        Some(a)
    }
}

/// Version notes that older seeders wrote and this one words differently, as
/// `(registry id, version, old notes)`. A seeded version whose notes still read
/// exactly like this gets the current `official_name`; notes anyone edited are
/// left alone.
const SUPERSEDED_NOTES: &[(&str, &str, &str)] = &[
    ("rdf", "1.0", "RDF 1.0 (2004)"),
    ("rdf", "1.2", "RDF 1.2 (Working Draft)"),
    ("rdfs", "1.0", "RDF Schema 1.0 (2004)"),
    ("owl", "1.0", "OWL Web Ontology Language (OWL 1, 2004)"),
    ("dcterms", "2008-01-14", "DCMI Metadata Terms (2008-01-14)"),
    ("dcterms", "2020-01-20", "DCMI Metadata Terms (2020-01-20)"),
    ("dcat", "1.0", "DCAT 1 (2014)"),
    ("schema", "29.0", "Schema.org 29.0"),
];

struct StdVocab {
    /// Registry id (also the IRI slug under `/data-model/{id}`).
    id: &'static str,
    title: &'static str,
    namespace: &'static str,
    versions: &'static [StdVersion],
}

const VOCABS: &[StdVocab] = &[
    StdVocab {
        id: "rdf",
        title: "RDF",
        namespace: "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
        versions: &[
            StdVersion {
                version: "1.0",
                official_name: "RDF 1.0 (2004) — served with the RDF 1.1 namespace document",
                date: "2004-02-10",
                spec_url: "https://www.w3.org/TR/2004/REC-rdf-concepts-20040210/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                file: &vf::RDF,
            },
            StdVersion {
                version: "1.1",
                official_name: "RDF 1.1 (2014)",
                date: "2014-02-25",
                spec_url: "https://www.w3.org/TR/rdf11-concepts/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                file: &vf::RDF,
            },
            StdVersion {
                version: "1.2",
                official_name: "RDF 1.2 (Working Draft) — served with the RDF 1.1 namespace document",
                date: "2024-01-12",
                spec_url: "https://www.w3.org/TR/rdf12-concepts/",
                status: VersionStatus::Draft,
                latest: false,
                prior: Some("1.1"),
                file: &vf::RDF,
            },
        ],
    },
    StdVocab {
        id: "rdfs",
        title: "RDF Schema",
        namespace: "http://www.w3.org/2000/01/rdf-schema#",
        versions: &[
            StdVersion {
                version: "1.0",
                official_name: "RDF Schema 1.0 (2004) — served with the RDFS 1.1 namespace document",
                date: "2004-02-10",
                spec_url: "https://www.w3.org/TR/2004/REC-rdf-schema-20040210/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                file: &vf::RDFS,
            },
            StdVersion {
                version: "1.1",
                official_name: "RDF Schema 1.1 (2014)",
                date: "2014-02-25",
                spec_url: "https://www.w3.org/TR/rdf-schema/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                file: &vf::RDFS,
            },
        ],
    },
    StdVocab {
        id: "owl",
        title: "OWL",
        namespace: "http://www.w3.org/2002/07/owl#",
        versions: &[
            StdVersion {
                version: "1.0",
                official_name: "OWL Web Ontology Language (OWL 1, 2004) — served with the OWL 2 namespace document",
                date: "2004-02-10",
                spec_url: "https://www.w3.org/TR/2004/REC-owl-features-20040210/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                file: &vf::OWL,
            },
            StdVersion {
                version: "2.0",
                official_name: "OWL 2 (2009, 2nd ed. 2012)",
                date: "2012-12-11",
                spec_url: "https://www.w3.org/TR/owl2-overview/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                file: &vf::OWL,
            },
        ],
    },
    StdVocab {
        id: "xsd",
        title: "XML Schema Datatypes",
        namespace: "http://www.w3.org/2001/XMLSchema#",
        versions: &[
            StdVersion {
                version: "1.0",
                official_name: "XML Schema Datatypes 1.0 (2004)",
                date: "2004-10-28",
                spec_url: "https://www.w3.org/TR/xmlschema-2/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                file: &vf::XSD,
            },
            StdVersion {
                version: "1.1",
                official_name: "XSD 1.1 Datatypes (2012)",
                date: "2012-04-05",
                spec_url: "https://www.w3.org/TR/xmlschema11-2/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                file: &vf::XSD,
            },
        ],
    },
    StdVocab {
        id: "skos",
        title: "SKOS",
        namespace: "http://www.w3.org/2004/02/skos/core#",
        versions: &[StdVersion {
            version: "2009-08-18",
            official_name: "SKOS Reference (2009)",
            date: "2009-08-18",
            spec_url: "https://www.w3.org/TR/skos-reference/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::SKOS,
        }],
    },
    StdVocab {
        id: "dcterms",
        title: "DCMI Metadata Terms",
        namespace: "http://purl.org/dc/terms/",
        versions: &[
            StdVersion {
                version: "2008-01-14",
                official_name: "DCMI Metadata Terms (2008-01-14) — served with the 2012-06-14 release file",
                date: "2008-01-14",
                spec_url:
                    "https://www.dublincore.org/specifications/dublin-core/dcmi-terms/2008-01-14/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                file: &vf::DCTERMS_2012,
            },
            StdVersion {
                version: "2012-06-14",
                official_name: "DCMI Metadata Terms (2012-06-14)",
                date: "2012-06-14",
                spec_url:
                    "https://www.dublincore.org/specifications/dublin-core/dcmi-terms/2012-06-14/",
                status: VersionStatus::Published,
                latest: false,
                prior: Some("2008-01-14"),
                file: &vf::DCTERMS_2012,
            },
            StdVersion {
                version: "2020-01-20",
                official_name: "DCMI Metadata Terms (2020-01-20, with DCMI's later changes)",
                date: "2020-01-20",
                spec_url:
                    "https://www.dublincore.org/specifications/dublin-core/dcmi-terms/2020-01-20/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("2012-06-14"),
                file: &vf::DCTERMS,
            },
        ],
    },
    StdVocab {
        id: "dcat",
        title: "DCAT",
        namespace: "http://www.w3.org/ns/dcat#",
        versions: &[
            StdVersion {
                version: "1.0",
                official_name: "DCAT 1 (2014) — served with the DCAT 3 vocabulary file",
                date: "2014-01-16",
                spec_url: "https://www.w3.org/TR/2014/REC-vocab-dcat-20140116/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                file: &vf::DCAT,
            },
            StdVersion {
                version: "2.0",
                official_name: "DCAT 2 (2020)",
                date: "2020-02-04",
                spec_url: "https://www.w3.org/TR/vocab-dcat-2/",
                status: VersionStatus::Published,
                latest: false,
                prior: Some("1.0"),
                file: &vf::DCAT_2,
            },
            StdVersion {
                version: "3.0",
                official_name: "DCAT 3 (2024)",
                date: "2024-08-22",
                spec_url: "https://www.w3.org/TR/vocab-dcat-3/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("2.0"),
                file: &vf::DCAT,
            },
        ],
    },
    StdVocab {
        id: "prov",
        title: "PROV-O",
        namespace: "http://www.w3.org/ns/prov#",
        versions: &[StdVersion {
            version: "2013-04-30",
            official_name: "PROV-O (2013)",
            date: "2013-04-30",
            spec_url: "https://www.w3.org/TR/2013/REC-prov-o-20130430/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::PROV,
        }],
    },
    StdVocab {
        id: "foaf",
        title: "FOAF",
        namespace: "http://xmlns.com/foaf/0.1/",
        versions: &[StdVersion {
            version: "0.99",
            official_name: "FOAF 0.99 (Paddington Edition, 2014)",
            date: "2014-01-14",
            spec_url: "http://xmlns.com/foaf/spec/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::FOAF,
        }],
    },
    StdVocab {
        id: "org",
        title: "Organization Ontology",
        namespace: "http://www.w3.org/ns/org#",
        versions: &[StdVersion {
            version: "0.8",
            official_name: "The Organization Ontology (2014)",
            date: "2014-01-16",
            spec_url: "https://www.w3.org/TR/vocab-org/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::ORG,
        }],
    },
    StdVocab {
        id: "qb",
        title: "RDF Data Cube",
        namespace: "http://purl.org/linked-data/cube#",
        versions: &[StdVersion {
            version: "0.2",
            official_name: "The RDF Data Cube Vocabulary (2014)",
            date: "2014-01-16",
            spec_url: "https://www.w3.org/TR/vocab-data-cube/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::QB,
        }],
    },
    StdVocab {
        id: "schema",
        title: "Schema.org",
        namespace: "https://schema.org/",
        // schema.ttl is a 27-term subset adapted from release 30.0 (2026-03-19;
        // CC BY-SA 3.0, see vocab/NOTICE.md). The version *label* stays at the
        // 29.0 that shipped first: it is part of the version's graph IRI, and a
        // new label would give existing installs a second copy of the same
        // content. The notes and the licence record say 30.0. The release notes
        // are the live link: https://schema.org/version/30.0/ (and /29.0/)
        // answer 404 (checked 2026-09-23).
        versions: &[StdVersion {
            version: "29.0",
            official_name: "Schema.org subset, adapted from release 30.0 (CC BY-SA 3.0)",
            date: "2026-03-19",
            spec_url: "https://schema.org/docs/releases.html#v30.0",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::SCHEMA,
        }],
    },
    StdVocab {
        id: "shacl",
        title: "SHACL",
        namespace: "http://www.w3.org/ns/shacl#",
        versions: &[StdVersion {
            version: "2017-07-20",
            official_name: "Shapes Constraint Language (SHACL) (2017)",
            date: "2017-07-20",
            spec_url: "https://www.w3.org/TR/shacl/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::SHACL,
        }],
    },
    StdVocab {
        id: "time",
        title: "OWL-Time",
        namespace: "http://www.w3.org/2006/time#",
        versions: &[StdVersion {
            version: "2016",
            official_name: "Time Ontology in OWL (2016/2017)",
            date: "2017-10-19",
            spec_url: "https://www.w3.org/TR/owl-time/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::TIME,
        }],
    },
    StdVocab {
        id: "vann",
        title: "VANN",
        namespace: "http://purl.org/vocab/vann/",
        versions: &[StdVersion {
            version: "1.1",
            official_name: "VANN (2010)",
            date: "2010-06-07",
            spec_url: "https://vocab.org/vann/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::VANN,
        }],
    },
    StdVocab {
        id: "geosparql",
        title: "GeoSPARQL",
        namespace: "http://www.opengis.net/ont/geosparql#",
        versions: &[
            StdVersion {
                version: "1.0",
                official_name: "OGC GeoSPARQL 1.0 (2012)",
                date: "2012-09-10",
                spec_url: "https://www.ogc.org/standard/geosparql/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                file: &vf::GEOSPARQL_1_0,
            },
            StdVersion {
                version: "1.1",
                official_name: "OGC GeoSPARQL 1.1 (2024)",
                date: "2024-01-29",
                spec_url: "https://docs.ogc.org/is/22-047r1/22-047r1.html",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                file: &vf::GEOSPARQL,
            },
        ],
    },
    StdVocab {
        id: "ots",
        title: "Open Triplestore Vocabulary",
        namespace: "https://opentriplestore.org/ns#",
        versions: &[StdVersion {
            version: "1.0",
            official_name: "Open Triplestore Vocabulary 1.0",
            date: "2025-01-01",
            spec_url: "https://opentriplestore.org/ns#",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::OTS,
        }],
    },
    // ── Standards & Ontologies set: the BIM / sensor / asset-management domain
    // packs the demo data draws on. Each is the complete published vocabulary
    // (the header in the .ttl and vocab/NOTICE.md give source and licence); these
    // render with full definitions in the UI and are browsable/queryable as
    // public reference models alongside the core standards above. NB: the large
    // ifcOWL and Brick schemas are intentionally *not* bundled here (too large
    // for an include_str! seed) — import them as a user model if needed.
    // ── Standards & Ontologies set (from the 3D geospatial platform): single
    // published version each, in the multi-version schema. ──
    StdVocab {
        id: "bot",
        title: "Building Topology Ontology (BOT)",
        namespace: "https://w3id.org/bot#",
        versions: &[StdVersion {
            version: "0.3.2",
            official_name: "Building Topology Ontology 0.3.2",
            date: "2021-01-01",
            spec_url: "https://w3c-lbd-cg.github.io/bot/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::BOT,
        }],
    },
    StdVocab {
        id: "omg",
        title: "Ontology for Managing Geometry (OMG)",
        namespace: "https://w3id.org/omg#",
        versions: &[StdVersion {
            version: "0.3",
            official_name: "Ontology for Managing Geometry 0.3",
            date: "2019-01-01",
            spec_url: "https://w3id.org/omg",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::OMG,
        }],
    },
    StdVocab {
        id: "fog",
        title: "File Ontology for Geometry formats (FOG)",
        namespace: "https://w3id.org/fog#",
        versions: &[StdVersion {
            version: "0.0.4",
            official_name: "File Ontology for Geometry formats 0.0.4",
            date: "2020-01-01",
            spec_url: "https://w3id.org/fog",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::FOG,
        }],
    },
    StdVocab {
        id: "sosa",
        title: "Sensor, Observation, Sample, and Actuator (SOSA)",
        namespace: "http://www.w3.org/ns/sosa/",
        versions: &[StdVersion {
            version: "2017-10-19",
            official_name: "SOSA / SSN (2017)",
            date: "2017-10-19",
            spec_url: "https://www.w3.org/TR/vocab-ssn/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::SOSA,
        }],
    },
    StdVocab {
        id: "saref",
        title: "Smart Applications REFerence ontology (SAREF)",
        namespace: "https://saref.etsi.org/core/",
        versions: &[StdVersion {
            version: "3.1.1",
            official_name: "ETSI SAREF Core 3.1.1",
            date: "2020-01-01",
            spec_url: "https://saref.etsi.org/core/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::SAREF,
        }],
    },
    StdVocab {
        id: "ssn",
        title: "Semantic Sensor Network ontology (SSN)",
        namespace: "http://www.w3.org/ns/ssn/",
        versions: &[StdVersion {
            version: "2017-10-19",
            official_name: "Semantic Sensor Network (2017)",
            date: "2017-10-19",
            spec_url: "https://www.w3.org/TR/vocab-ssn/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::SSN,
        }],
    },
    StdVocab {
        id: "imbor",
        title: "CROW IMBOR Vocabulaire",
        namespace: "https://data.crow.nl/imbor/term/",
        versions: &[
            // imbor2025-vocabulaire.ttl from CROW's IMBOR 2025 Linked Data
            // release, byte-identical and with no added header (8,948 SKOS
            // concepts). It must stay unmodified: CROW's Beheerplan names
            // CC BY-ND 4.0. Licence and provenance: vocab/NOTICE.md.
            StdVersion {
                version: "2025",
                official_name: "IMBOR 2025 Vocabulaire (Stichting CROW)",
                date: "2025-07-01",
                spec_url: "https://github.com/Stichting-CROW/imbor/releases/tag/2025",
                status: VersionStatus::Published,
                latest: true,
                prior: None,
                file: &vf::IMBOR,
            },
        ],
    },
    // ── LOV expansion pack ────────────────────────────────────────────────────
    // Full vocabularies extracted from the bundled LOV corpus snapshot of
    // 2025-12-18 (https://lov.linkeddata.es/). LOV's CC BY 4.0 covers only its
    // catalogue; each vocabulary keeps its publisher's licence — see the per-file
    // headers and frontend/public/vocab/NOTICE.md.
    StdVocab {
        id: "voaf",
        title: "Vocabulary of a Friend (VOAF)",
        namespace: "http://purl.org/vocommons/voaf#",
        versions: &[StdVersion {
            version: "2.3",
            official_name: "Vocabulary of a Friend (VOAF) v2.3",
            date: "2013-05-24",
            spec_url: "https://lov.linkeddata.es/vocommons/voaf/v2.3/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::VOAF,
        }],
    },
    StdVocab {
        id: "adms",
        title: "Asset Description Metadata Schema (ADMS)",
        namespace: "http://www.w3.org/ns/adms#",
        versions: &[StdVersion {
            version: "2015-07-22",
            official_name: "Asset Description Metadata Schema (W3C Note 2015)",
            date: "2015-07-22",
            spec_url: "https://www.w3.org/TR/vocab-adms/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::ADMS,
        }],
    },
    StdVocab {
        id: "vcard",
        title: "vCard Ontology",
        namespace: "http://www.w3.org/2006/vcard/ns#",
        versions: &[StdVersion {
            version: "2014-05-22",
            official_name: "vCard Ontology (W3C Note 2014)",
            date: "2014-05-22",
            spec_url: "https://www.w3.org/TR/vcard-rdf/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::VCARD,
        }],
    },
    StdVocab {
        id: "sioc",
        title: "SIOC Core Ontology",
        namespace: "http://rdfs.org/sioc/ns#",
        versions: &[StdVersion {
            version: "1.35",
            official_name: "Semantically-Interlinked Online Communities (SIOC) v1.35",
            date: "2010-03-25",
            spec_url: "http://rdfs.org/sioc/spec/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::SIOC,
        }],
    },
    StdVocab {
        id: "bibo",
        title: "Bibliographic Ontology (BIBO)",
        namespace: "http://purl.org/ontology/bibo/",
        versions: &[StdVersion {
            version: "1.3",
            official_name: "The Bibliographic Ontology v1.3",
            date: "2009-11-04",
            spec_url: "https://github.com/structureddynamics/Bibliographic-Ontology-BIBO",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::BIBO,
        }],
    },
    StdVocab {
        id: "oa",
        title: "Web Annotation Vocabulary",
        namespace: "http://www.w3.org/ns/oa#",
        versions: &[StdVersion {
            version: "2016-11-12",
            official_name: "Web Annotation Vocabulary (W3C Recommendation 2017)",
            date: "2016-11-12",
            spec_url: "https://www.w3.org/TR/annotation-vocab/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::OA,
        }],
    },
    StdVocab {
        id: "odrl",
        title: "Open Digital Rights Language (ODRL)",
        namespace: "http://www.w3.org/ns/odrl/2/",
        versions: &[StdVersion {
            version: "2.2",
            official_name: "ODRL Version 2.2 Ontology (W3C Recommendation 2018)",
            date: "2017-09-16",
            spec_url: "https://www.w3.org/TR/odrl-vocab/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::ODRL,
        }],
    },
    StdVocab {
        id: "cc",
        title: "Creative Commons Rights Expression Language (ccREL)",
        namespace: "http://creativecommons.org/ns#",
        versions: &[StdVersion {
            version: "2008-03-03",
            official_name: "Creative Commons Rights Expression Language",
            date: "2008-03-03",
            spec_url: "https://creativecommons.org/ns",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::CC,
        }],
    },
    StdVocab {
        id: "vs",
        title: "SemWeb Vocab Status Ontology",
        namespace: "http://www.w3.org/2003/06/sw-vocab-status/ns#",
        versions: &[StdVersion {
            version: "2011-12-12",
            official_name: "SemWeb Vocab Status Ontology (2011)",
            date: "2011-12-12",
            spec_url: "https://www.w3.org/2003/06/sw-vocab-status/ns",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::VS,
        }],
    },
    StdVocab {
        id: "wgs84",
        title: "WGS84 Geo Positioning",
        namespace: "http://www.w3.org/2003/01/geo/wgs84_pos#",
        versions: &[StdVersion {
            version: "1.22",
            official_name: "WGS84 Geo Positioning Vocabulary v1.22",
            date: "2009-04-20",
            spec_url: "https://www.w3.org/2003/01/geo/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::WGS84,
        }],
    },
    StdVocab {
        id: "gr",
        title: "GoodRelations",
        namespace: "http://purl.org/goodrelations/v1#",
        versions: &[StdVersion {
            version: "1.0",
            official_name: "GoodRelations Ontology V1 (2011-10-01 release)",
            date: "2011-10-01",
            spec_url: "http://purl.org/goodrelations/v1",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::GR,
        }],
    },
    StdVocab {
        id: "doap",
        title: "Description of a Project (DOAP)",
        namespace: "http://usefulinc.com/ns/doap#",
        versions: &[StdVersion {
            version: "2012-01-04",
            official_name: "Description of a Project vocabulary (2012)",
            date: "2012-01-04",
            spec_url: "https://github.com/ewilderj/doap",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::DOAP,
        }],
    },
    StdVocab {
        id: "dctype",
        title: "DCMI Type Vocabulary",
        namespace: "http://purl.org/dc/dcmitype/",
        versions: &[StdVersion {
            version: "2012-06-14",
            official_name: "DCMI Type Vocabulary (2012-06-14)",
            date: "2012-06-14",
            spec_url: "https://www.dublincore.org/specifications/dublin-core/dcmi-terms/#section-7",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::DCTYPE,
        }],
    },
    StdVocab {
        id: "locn",
        title: "ISA Location Core Vocabulary (LOCN)",
        namespace: "http://www.w3.org/ns/locn#",
        versions: &[StdVersion {
            version: "2015-03-23",
            official_name: "ISA Programme Location Core Vocabulary v1.00",
            date: "2015-03-23",
            spec_url: "https://www.w3.org/ns/locn",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::LOCN,
        }],
    },
    StdVocab {
        id: "dqv",
        title: "Data Quality Vocabulary (DQV)",
        namespace: "http://www.w3.org/ns/dqv#",
        versions: &[StdVersion {
            version: "2016-08-26",
            official_name: "Data Quality Vocabulary (W3C Note 2016)",
            date: "2016-08-26",
            spec_url: "https://www.w3.org/TR/vocab-dqv/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::DQV,
        }],
    },
    StdVocab {
        id: "pav",
        title: "Provenance, Authoring and Versioning (PAV)",
        namespace: "http://purl.org/pav/",
        versions: &[StdVersion {
            version: "2.3.1",
            official_name: "PAV — Provenance, Authoring and Versioning v2.3.1",
            date: "2014-08-28",
            spec_url: "https://pav-ontology.github.io/pav/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::PAV,
        }],
    },
    StdVocab {
        id: "csvw",
        title: "CSV on the Web Vocabulary (CSVW)",
        namespace: "http://www.w3.org/ns/csvw#",
        versions: &[StdVersion {
            version: "2015-12-17",
            official_name: "CSV on the Web Vocabulary (W3C Recommendation 2015)",
            date: "2015-12-17",
            spec_url: "https://www.w3.org/ns/csvw",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            file: &vf::CSVW,
        }],
    },
];

// ─── Records the seeder owns ────────────────────────────────────────────────────

/// The `ver:seededBy` marker on every entry and version record the seeder
/// creates, and on each older record it has proven to be its own.
pub const SEEDED_BY: &str = "open-triplestore:standard-vocabularies";

/// The `ver:seededBy` marker on a copy the seeder kept aside: the stored copy
/// of a version whose licence allows no altered copies, as it was when a check
/// found it different from the bundled file ([`keep_aside`]).
pub const KEPT_BY: &str = "open-triplestore:standard-vocabularies:kept";

/// Creation dates an earlier seeder gave a version, where this build's `date`
/// differs, as `(registry id, version, date)`.
const EARLIER_DATES: &[(&str, &str, &str)] = &[("schema", "29.0", "2025-07-01")];

/// A version an earlier seeder created in an entry this build still seeds,
/// and which this build no longer ships.
struct RetiredVersion {
    id: &'static str,
    version: &'static str,
    /// The notes that seeder wrote.
    notes: &'static str,
    /// The creation date that seeder gave it.
    date: &'static str,
}

/// Releases 0.4.0 to 0.6.0 seeded IMBOR's hand-authored "excerpt", which
/// named terms in CROW's namespace that CROW never published. It is kept,
/// deprecated and withheld ([`enforce_no_derivatives`]). The `bag` and `otl`
/// entries those releases also seeded are entries of their own that this
/// build does not seed; they are left as they are.
const RETIRED_SEEDED_VERSIONS: &[RetiredVersion] = &[RetiredVersion {
    id: "imbor",
    version: "excerpt",
    notes: "CROW IMBOR (excerpt)",
    date: "2024-01-01",
}];

/// Whether `id` is the registry id of a vocabulary this build seeds. Other
/// seeders (seed bundles) leave these ids to it, whatever the boot order.
pub fn reserves_id(id: &str) -> bool {
    VOCABS.iter().any(|v| v.id == id)
}

fn has_no_derivatives(v: &StdVocab) -> bool {
    v.versions.iter().any(|ver| ver.file.no_derivatives)
}

/// The registry records of one vocabulary's entry and versions, read once.
struct Snapshot {
    model: Option<registry::RecordProvenance>,
    versions: HashMap<String, registry::RecordProvenance>,
}

impl Snapshot {
    fn read(state: &AppState, v: &StdVocab) -> Self {
        let (model, versions) = registry::record_provenance(&state.store, &state.base_url, v.id);
        Self { model, versions }
    }
}

/// How a version record came to count as the seeder's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owned {
    /// It carries the seeder's marker.
    Marked,
    /// It predates the marker and reads exactly as an earlier seeder wrote it.
    Legacy,
}

/// Whether the record of version `label` of `v` is one the seeder created: no
/// creator, the one graph the seeder loads into and no other, and either the
/// seeder's marker or, for a record from before the marker, exactly the notes
/// and creation date an earlier seeder wrote for that version (see
/// [`legacy_fingerprint`]). Whether the entry is the seeder's is checked
/// separately ([`entry_owned`]).
fn version_owned(
    state: &AppState,
    v: &StdVocab,
    label: &str,
    p: &registry::RecordProvenance,
) -> Option<Owned> {
    if p.created_by.is_some() {
        return None;
    }
    let graph = registry::version_record_iri(&state.base_url, v.id, label);
    if p.graph_iri.as_deref() != Some(graph.as_str()) || p.sub_graphs.iter().any(|g| *g != graph) {
        return None;
    }
    match p.seeded_by.as_deref() {
        Some(SEEDED_BY) => Some(Owned::Marked),
        Some(_) => None,
        None => legacy_fingerprint(v, label, p).then_some(Owned::Legacy),
    }
}

/// Whether a version record with no marker reads exactly as an earlier seeder
/// wrote it: a version this build seeds, with the notes (current or
/// superseded) and the `{date}T00:00:00Z` creation date a seeder gave it; a
/// retired version with its notes and date; or the synthetic `1.0.0` of the
/// seeders before 0.4.0, which wrote no notes. Seed bundles and LOV installs
/// write notes of their own, and every record made through the API has a
/// creator, so none of them reads like this.
fn legacy_fingerprint(v: &StdVocab, label: &str, p: &registry::RecordProvenance) -> bool {
    let dated = |date: &str| p.created_at.as_deref() == Some(format!("{date}T00:00:00Z").as_str());
    let notes = p.notes.as_deref();
    if let Some(ver) = v.versions.iter().find(|x| x.version == label) {
        let notes_ok = notes == Some(ver.official_name)
            || SUPERSEDED_NOTES
                .iter()
                .any(|(id, vv, old)| *id == v.id && *vv == label && notes == Some(*old));
        let date_ok = dated(ver.date)
            || EARLIER_DATES
                .iter()
                .any(|(id, vv, d)| *id == v.id && *vv == label && dated(d));
        return notes_ok && date_ok;
    }
    if let Some(r) = RETIRED_SEEDED_VERSIONS
        .iter()
        .find(|r| r.id == v.id && r.version == label)
    {
        return notes == Some(r.notes) && dated(r.date);
    }
    label == LEGACY_SYNTHETIC_VERSION && notes.is_none()
}

/// Whether `v`'s registry entry is the seeder's: no creator, and either the
/// seeder's marker or at least one version the seeder made there. An entry
/// created through the API or promoted from a dataset has a creator; one a
/// seed bundle or a LOV install registered holds none of the seeder's
/// versions. An admin may have claimed the seeder's entry (an owner): it stays
/// the seeder's.
fn entry_owned(state: &AppState, v: &StdVocab, snap: &Snapshot) -> bool {
    let Some(m) = &snap.model else {
        return false;
    };
    if m.created_by.is_some() {
        return false;
    }
    match m.seeded_by.as_deref() {
        Some(SEEDED_BY) => true,
        Some(_) => false,
        None => snap
            .versions
            .iter()
            .any(|(label, p)| version_owned(state, v, label, p).is_some()),
    }
}

fn warn_not_owned(v: &StdVocab) {
    tracing::warn!(
        "vocabulary '{}': the registry entry '{}' was not created by the vocabulary seeder, or \
         no longer reads as it wrote it; it is left as it is and the bundled vocabulary is not \
         seeded into it",
        v.id,
        v.id
    );
}

// Test-only fault injection: `fault` fails at the named step of a repair.
#[cfg(test)]
thread_local! {
    static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// A step boundary of a repair; tests can make it fail.
fn fault(step: &'static str) -> anyhow::Result<()> {
    #[cfg(test)]
    if FAIL_AT.with(|f| f.get()) == Some(step) {
        anyhow::bail!("injected failure after step '{step}'");
    }
    let _ = step;
    Ok(())
}

// ─── Seeding and checking ───────────────────────────────────────────────────────

/// What a start does with the bundled vocabularies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Seed missing versions and check every seeded copy.
    Seed,
    /// `SEED_STANDARD_VOCABS=false`: seed nothing, and only check (and
    /// repair) the copies of vocabularies whose licence allows no altered
    /// copies.
    VerifyOnly,
}

/// Seed every standard vocabulary version that isn't already in the registry,
/// and check the copies seeded earlier. Returns the number of newly-seeded
/// *versions* (0 once everything is present — the idempotent steady state).
/// With `SEED_STANDARD_VOCABS=false` nothing is seeded, but copies the seeder
/// made earlier of a vocabulary whose licence allows no altered copies are
/// still checked and repaired.
pub fn seed_standard_vocabularies(state: &AppState) -> usize {
    let disabled = std::env::var("SEED_STANDARD_VOCABS")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off"
            )
        })
        .unwrap_or(false);
    seed_standard_vocabularies_with(state, !disabled)
}

/// [`seed_standard_vocabularies`], with the opt-out given (`seeding: false`
/// is `SEED_STANDARD_VOCABS=false`).
pub(crate) fn seed_standard_vocabularies_with(state: &AppState, seeding: bool) -> usize {
    // A replica, or a cluster member that does not lead, refuses every write:
    // the leader seeds and checks, and its writes replicate here.
    if state.store.replication().read_only() {
        tracing::debug!("vocabulary seed skipped: this node's store is read-only");
        return 0;
    }
    if !seeding {
        // The opt-out stops seeding. It does not lift the licence duties on a
        // copy an earlier start seeded of a vocabulary that allows no altered
        // copies: that copy is still checked, and repaired, on every start.
        // Nothing is created: an install that never seeded it is untouched.
        let mut repaired = 0usize;
        for v in VOCABS.iter().filter(|v| has_no_derivatives(v)) {
            match sync_seeded_records(state, v, Mode::VerifyOnly) {
                Ok(n) => repaired += n,
                Err(e) => tracing::warn!("vocabulary '{}': check failed: {e:#}", v.id),
            }
        }
        if repaired > 0 {
            tracing::info!(
                "SEED_STANDARD_VOCABS=false: nothing seeded; {repaired} record(s) of previously \
                 seeded vocabularies whose licence allows no altered copies checked and updated"
            );
        }
        return 0;
    }
    let mut seeded = 0usize;
    let mut synced = 0usize;
    for v in VOCABS {
        match seed_one(state, v) {
            Ok(n) => seeded += n,
            Err(e) => tracing::warn!("vocabulary seed '{}' skipped: {e:#}", v.id),
        }
        match sync_seeded_records(state, v, Mode::Seed) {
            Ok(n) => synced += n,
            Err(e) => tracing::warn!("vocabulary '{}': check failed: {e:#}", v.id),
        }
    }
    if seeded > 0 {
        tracing::info!("Seeded {seeded} standard vocabulary version(s) into the model registry");
    }
    if synced > 0 {
        tracing::info!("Updated {synced} record(s) or graph(s) of previously seeded vocabularies");
    }
    seeded
}

/// The registry's JSON form of an attribution, for change detection.
fn record_json(a: Option<&ContentAttribution>) -> Option<String> {
    a.and_then(|a| serde_json::to_string(a).ok())
}

/// What a check of a seeded version's stored copy found.
struct Checked {
    /// The stored triples are exactly the bundled file's.
    unchanged: bool,
    /// For a copy that is not: the `stored_copy` text its record keeps (an
    /// earlier, more specific explanation), else the generic one.
    keep_text: Option<String>,
    /// Registry records and graphs written.
    writes: usize,
}

/// Bring the records of the seeder's own copies up to date, on every start:
///
/// * each seeded version's stored copy is checked against its bundled file:
///   a copy whose licence allows other copies whenever the file differs from
///   the one the last check used ([`check_copy`]; such a copy is never
///   modified), a copy whose licence allows no altered copies on every start
///   ([`verify_no_derivatives`]);
/// * the licence record on the entry and on each seeded version, where it is
///   missing or differs from this build's; it calls the copy "unchanged" only
///   when the check found it so, and is replaced only if it still reads as it
///   did when this pass began (a concurrent PATCH or direct write that marked
///   it as possibly modified wins);
/// * version notes that still read exactly as an older seeder wrote them
///   ([`SUPERSEDED_NOTES`]);
/// * the marker on older records proven to be the seeder's;
/// * for a vocabulary whose licence allows no altered copies, versions an
///   earlier seeder made there that this build no longer ships, and a latest
///   pointer at a version that is not a checked copy
///   ([`enforce_no_derivatives`]).
///
/// Only the seeder's own entry and versions are touched ([`entry_owned`],
/// [`version_owned`]); any other record is logged and left alone. A
/// steady-state start costs three registry reads and one hash of each bundled
/// file per vocabulary, plus one read of each graph whose licence allows no
/// altered copies (IMBOR), and no writes. Returns the number of records and
/// graphs updated.
fn sync_seeded_records(state: &AppState, v: &StdVocab, mode: Mode) -> anyhow::Result<usize> {
    let snap = Snapshot::read(state, v);
    let Some(model) = snap.model.as_ref() else {
        return Ok(0);
    };
    if !entry_owned(state, v, &snap) {
        // In `Seed` mode `seed_one` has said so already.
        if mode == Mode::VerifyOnly {
            warn_not_owned(v);
        }
        return Ok(0);
    }
    let mut updated = 0usize;
    let entry_iri = registry::data_model_iri(&state.base_url, v.id);
    if model.seeded_by.is_none() {
        registry::set_seeded_by(&state.store, &entry_iri, SEEDED_BY)?;
        updated += 1;
    }

    let mut unchanged_by_version: HashMap<&str, bool> = HashMap::new();
    for ver in v.versions {
        if mode == Mode::VerifyOnly && !ver.file.no_derivatives {
            continue;
        }
        let Some(p) = snap.versions.get(ver.version) else {
            continue;
        };
        let ver_iri = registry::version_record_iri(&state.base_url, v.id, ver.version);
        match version_owned(state, v, ver.version, p) {
            None => {
                tracing::warn!(
                    "vocabulary '{}' version '{}': the registry record was not created by the \
                     vocabulary seeder, or no longer reads as it wrote it; left as it is",
                    v.id,
                    ver.version
                );
                continue;
            }
            Some(Owned::Legacy) => {
                registry::set_seeded_by(&state.store, &ver_iri, SEEDED_BY)?;
                updated += 1;
            }
            Some(Owned::Marked) => {}
        }
        let mut current = p.attribution_json.clone();
        let checked = if ver.file.no_derivatives {
            verify_no_derivatives(state, v, ver, p, &snap, &mut current)?
        } else {
            check_copy(state, v, ver, p)?
        };
        updated += checked.writes;
        unchanged_by_version.insert(ver.version, checked.unchanged);

        let expected = ver.record(checked.unchanged, checked.keep_text.as_deref());
        if record_json(expected.as_ref()) != current {
            if registry::replace_attribution_if(
                &state.store,
                &ver_iri,
                current.as_deref(),
                expected.as_ref(),
            )? {
                updated += 1;
            } else {
                tracing::warn!(
                    "vocabulary '{}' version '{}': its licence record changed while it was \
                     checked; left as the other writer set it (the next start checks again)",
                    v.id,
                    ver.version
                );
            }
        }
        if mode == Mode::Seed {
            let superseded = SUPERSEDED_NOTES
                .iter()
                .find(|(id, version, _)| *id == v.id && *version == ver.version);
            if let Some((_, _, old)) = superseded {
                if p.notes.as_deref() == Some(*old) {
                    registry::update_version_notes(
                        &state.store,
                        &state.base_url,
                        v.id,
                        ver.version,
                        Some(ver.official_name),
                    )?;
                    updated += 1;
                }
            }
        }
    }

    // The entry describes its latest version's content, in that copy's state.
    if let Some(latest) = v.versions.iter().find(|ver| ver.latest) {
        if let Some(&unchanged) = unchanged_by_version.get(latest.version) {
            let expected = latest.attribution(unchanged);
            if record_json(expected.as_ref()) != model.attribution_json {
                if registry::replace_attribution_if(
                    &state.store,
                    &entry_iri,
                    model.attribution_json.as_deref(),
                    expected.as_ref(),
                )? {
                    updated += 1;
                } else {
                    tracing::warn!(
                        "vocabulary '{}': the entry's licence record changed while it was \
                         checked; left as the other writer set it",
                        v.id
                    );
                }
            }
        }
    }

    if mode == Mode::Seed {
        updated += settle_synthetic_version(state, v, &snap)?;
    }
    if has_no_derivatives(v) {
        updated += enforce_no_derivatives(state, v, &snap)?;
    }
    Ok(updated)
}

/// The bundled file's triples as the store holds them, and the parse they
/// come from.
fn file_triples(ver: &StdVersion) -> anyhow::Result<(Vec<Quad>, Vec<Triple>)> {
    let quads = upload::parse_rdf(ver.file.ttl.as_bytes(), "text/turtle", "vocab.ttl")
        .map_err(|e| anyhow::anyhow!("parse {}: {e}", ver.file.path))?;
    let triples = content_digest::as_stored(&content_digest::quads_as_triples(&quads))?;
    Ok((quads, triples))
}

/// Check a seeded copy whose licence allows other copies against its bundled
/// file, and record the check ([`registry::set_seed_check`]). Nothing is
/// modified, whatever the check finds:
///
/// * checked against this very file on an earlier start: its record holds the
///   outcome, and every change since (a PATCH, a publish, a direct write) has
///   marked it — no read of the graph;
/// * the stored triples equal the file's: unchanged;
/// * they differ: the copy may hold an admin's edit, or an earlier build's
///   file (with the `owl:versionInfo` triple releases before 0.7 added). It is
///   kept exactly as stored, and its record says it differs from the file or
///   may have been modified. To take the current file instead, delete the
///   entry: the next start seeds it anew.
fn check_copy(
    state: &AppState,
    v: &StdVocab,
    ver: &StdVersion,
    p: &registry::RecordProvenance,
) -> anyhow::Result<Checked> {
    let file_sha = content_digest::file_sha256(ver.file.ttl.as_bytes());
    let recorded: Option<ContentAttribution> = p
        .attribution_json
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok());
    let keep_text = recorded
        .as_ref()
        .filter(|a| !a.unchanged)
        .map(|a| a.stored_copy.clone());
    // Open Triplestore's own vocabulary has no record to hold the outcome.
    if p.seed_source_sha256.as_deref() == Some(file_sha.as_str())
        && (recorded.is_some() || !ver.file.third_party)
    {
        return Ok(Checked {
            unchanged: recorded.as_ref().is_none_or(|a| a.unchanged),
            keep_text,
            writes: 0,
        });
    }
    let graph = registry::version_record_iri(&state.base_url, v.id, ver.version);
    let stored = content_digest::graph_triples(&state.store, &graph)?;
    let (_, file) = file_triples(ver)?;
    if content_digest::same_triples(&stored, &file) {
        let digest = content_digest::triples_digest(&stored);
        registry::set_seed_check(&state.store, &graph, &file_sha, Some(&digest))?;
        return Ok(Checked {
            unchanged: true,
            keep_text: None,
            writes: 1,
        });
    }
    // An earlier digest stays: it still names what the seeder last vouched for.
    registry::set_seed_check(&state.store, &graph, &file_sha, None)?;
    tracing::warn!(
        "vocabulary '{}' version '{}': the stored copy differs from vocab/{}; it may hold edits \
         made in this registry, so it is kept exactly as stored, and its licence record says it \
         may have been modified",
        v.id,
        ver.version,
        ver.file.path
    );
    Ok(Checked {
        unchanged: false,
        keep_text,
        writes: 1,
    })
}

/// Check, on every start, a seeded copy whose licence allows no altered
/// copies (IMBOR), and repair it without losing anything:
///
/// * the stored graph still digests as recorded at the last check against
///   this very file, and its record says unchanged: nothing to do;
/// * its triples equal the file's (in the store's canonical literal forms):
///   the check is recorded;
/// * they differ (a write outside the registry, an earlier build's altered
///   file, an interrupted repair):
///   1. the record stops calling the copy unchanged, so from here on it is
///      withheld from everyone who may not write the entry;
///   2. the stored copy is copied, in one update, into a new private version
///      of the entry ([`keep_aside`]), and the copy is verified triple for
///      triple (an earlier start that stopped after this step is recognised
///      by the copy's content, so no second copy is made);
///   3. the file's triples are loaded into a staging graph, verified, and
///      moved into the version's graph in one transaction, which also empties
///      the staging graph;
///   4. the version's graph is verified against the file and the check
///      recorded; only then does the record say unchanged again.
///
/// A failure at any step returns an error and leaves the stored data where
/// it was — the old copy in the version's graph or in the kept version, the
/// file's triples in the version's graph — with a record that does not say
/// unchanged. The next start picks up from there.
fn verify_no_derivatives(
    state: &AppState,
    v: &StdVocab,
    ver: &StdVersion,
    p: &registry::RecordProvenance,
    snap: &Snapshot,
    current: &mut Option<String>,
) -> anyhow::Result<Checked> {
    let file_sha = content_digest::file_sha256(ver.file.ttl.as_bytes());
    let graph = registry::version_record_iri(&state.base_url, v.id, ver.version);
    let stored = content_digest::graph_triples(&state.store, &graph)?;
    let digest = content_digest::triples_digest(&stored);
    let recorded: Option<ContentAttribution> = current
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok());
    if p.seed_source_sha256.as_deref() == Some(file_sha.as_str())
        && p.seed_content_digest.as_deref() == Some(digest.as_str())
        && recorded
            .as_ref()
            .is_some_and(|a| a.unchanged && a.no_derivatives)
    {
        return Ok(Checked {
            unchanged: true,
            keep_text: None,
            writes: 0,
        });
    }
    let (quads, file) = file_triples(ver)?;
    if content_digest::same_triples(&stored, &file) {
        registry::set_seed_check(&state.store, &graph, &file_sha, Some(&digest))?;
        return Ok(Checked {
            unchanged: true,
            keep_text: None,
            writes: 1,
        });
    }

    let mut writes = 0usize;
    // 1. Not called unchanged from here on, whatever happens next.
    if recorded.as_ref().is_none_or(|a| a.unchanged) {
        let label = ver.record(false, None);
        registry::set_attribution(&state.store, &graph, label.as_ref())?;
        *current = record_json(label.as_ref());
        writes += 1;
    }
    fault("labelled")?;
    // 2. Keep the stored copy, verified, before anything replaces it.
    let kept = if stored.is_empty() {
        None
    } else {
        Some(keep_aside(state, v, ver, snap, &graph, &stored, &file_sha)?)
    };
    // 3. Restore the file's triples atomically.
    restore_atomically(state, v, ver, &graph, quads, &file)?;
    fault("restored")?;
    // 4. Verify and record.
    let now = content_digest::graph_triples(&state.store, &graph)?;
    if !content_digest::same_triples(&now, &file) {
        anyhow::bail!(
            "{} {}: after the restore the graph does not hold the triples of vocab/{}",
            v.id,
            ver.version,
            ver.file.path
        );
    }
    registry::set_seed_check(
        &state.store,
        &graph,
        &file_sha,
        Some(&content_digest::triples_digest(&now)),
    )?;
    writes += 2;
    state.mark_vocab_registry_dirty();
    #[cfg(feature = "text-search")]
    state.mark_text_dirty();
    tracing::warn!(
        "vocabulary '{}' version '{}': the stored copy differed from vocab/{}, whose licence \
         allows no altered copies; {}the version holds the file's triples again",
        v.id,
        ver.version,
        ver.file.path,
        match &kept {
            Some(label) => format!(
                "the stored copy is kept as version '{label}' (withheld from everyone who may \
                 not write the entry), and "
            ),
            None => "the graph was empty, and ".to_string(),
        }
    );
    Ok(Checked {
        unchanged: true,
        keep_text: None,
        writes,
    })
}

/// Copy the stored graph of version `ver` aside, into a new version of the
/// entry named `{version}-kept-{n}`: no creator, deprecated, with a licence
/// record that allows no altered copies and does not call it unchanged, so
/// the registry serves it to no one who may not write the entry and refuses
/// every edit of it. Its notes say what it is and why it is kept. Returns the
/// new version's label; when an earlier start already kept this very content
/// (it stopped before the restore), that version's label, its records
/// completed.
fn keep_aside(
    state: &AppState,
    v: &StdVocab,
    ver: &StdVersion,
    snap: &Snapshot,
    graph: &str,
    stored: &[Triple],
    file_sha: &str,
) -> anyhow::Result<String> {
    let prefix = format!("{}-kept-", ver.version);
    let kept_iri = |label: &str| registry::version_record_iri(&state.base_url, v.id, label);
    let same_content = |g: &str| -> anyhow::Result<bool> {
        let copy = content_digest::graph_triples(&state.store, g)?;
        Ok(copy.len() == stored.len() && content_digest::same_triples(&copy, stored))
    };
    // A copy an earlier start made already holds this content (it stopped
    // before the restore, registered or not): recognised by the content and
    // completed, so no second copy is made. Labels are taken in order, so such
    // a copy sits before the first label with no record and an empty graph,
    // which is where a new copy goes.
    let mut free = None;
    for n in 1..=1000 {
        let label = format!("{prefix}{n}");
        let g = kept_iri(&label);
        let record = snap.versions.get(&label);
        if record.is_some() || registry::version_exists(&state.store, &state.base_url, v.id, &label)
        {
            let ours = record.is_some_and(|kp| {
                kp.created_by.is_none() && kp.graph_iri.as_deref() == Some(g.as_str())
            });
            if ours && same_content(&g)? {
                let copy = content_digest::graph_triples(&state.store, &g)?;
                register_kept(state, v, ver, &label, &copy, file_sha, true)?;
                return Ok(label);
            }
            continue;
        }
        if content_digest::graph_triples(&state.store, &g)?.is_empty() {
            free = Some(label);
            break;
        }
        if same_content(&g)? {
            let copy = content_digest::graph_triples(&state.store, &g)?;
            register_kept(state, v, ver, &label, &copy, file_sha, false)?;
            return Ok(label);
        }
    }
    let label = free.ok_or_else(|| anyhow::anyhow!("no free label {prefix}N to keep a copy"))?;
    let kept_graph = kept_iri(&label);
    // The copy, in one update: blank nodes and literal forms as stored.
    state.store.update(&format!(
        "INSERT {{ GRAPH <{kept_graph}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}"
    ))?;
    fault("copied")?;
    let copy = content_digest::graph_triples(&state.store, &kept_graph)?;
    if copy.len() != stored.len() || !content_digest::same_triples(&copy, stored) {
        anyhow::bail!(
            "the copy of {} {} in <{kept_graph}> holds {} triples, the stored copy {}; nothing \
             was replaced",
            v.id,
            ver.version,
            copy.len(),
            stored.len()
        );
    }
    fault("verified")?;
    register_kept(state, v, ver, &label, &copy, file_sha, false)?;
    Ok(label)
}

/// Register (or, with `existing`, complete the records of) a kept copy.
fn register_kept(
    state: &AppState,
    v: &StdVocab,
    ver: &StdVersion,
    label: &str,
    copy: &[Triple],
    file_sha: &str,
    existing: bool,
) -> anyhow::Result<()> {
    let iri = registry::version_record_iri(&state.base_url, v.id, label);
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    if !existing {
        let notes = format!(
            "Kept by Open Triplestore on {today}: version {version} as it was stored ({n} \
             triples) when its check against the bundled file vocab/{file} found that it no \
             longer held exactly the file's triples. The licence allows no altered copies, so \
             version {version} was restored to the file's triples. This copy is kept so that \
             nothing stored in this registry is lost; it is served to no one without write access \
             to this entry, and cannot be edited or published.",
            version = ver.version,
            n = copy.len(),
            file = ver.file.path,
        );
        registry::insert_version(
            &state.store,
            &state.base_url,
            &DataModelVersion {
                data_model_id: v.id.to_string(),
                version: label.to_string(),
                status: VersionStatus::Deprecated,
                graph_iri: iri.clone(),
                sub_graphs: vec![iri.clone()],
                created_at: chrono::Utc::now().to_rfc3339(),
                created_by: None,
                derived_from: Some(ver.version.to_string()),
                notes: Some(notes),
                branch: None,
                sub_graph_status: Vec::new(),
            },
        )?;
    }
    registry::set_seed_check(
        &state.store,
        &iri,
        file_sha,
        Some(&content_digest::triples_digest(copy)),
    )?;
    registry::set_seeded_by(&state.store, &iri, KEPT_BY)?;
    if registry::get_attribution(&state.store, &iri).is_none() {
        let text = format!(
            "The stored copy of version {version} as it was when it no longer held exactly the \
             triples of the bundled file vocab/{file}: it may hold edits made in this registry, \
             so it is not that file. The licence allows no altered copies, so the registry \
             serves this copy to no one without write access to this entry; version {version} \
             holds the file's triples again.",
            version = ver.version,
            file = ver.file.path,
        );
        registry::set_attribution(&state.store, &iri, ver.record(false, Some(&text)).as_ref())?;
    }
    Ok(())
}

/// Replace the triples of `graph` with the bundled file's, atomically: the
/// file is loaded into a staging graph (`urn:system:…`, never served) and
/// verified there, then one update — one transaction — empties `graph`,
/// copies the staging graph into it and empties the staging graph. A failure
/// before that update leaves `graph` as it was; the update itself happens
/// entirely or not at all.
fn restore_atomically(
    state: &AppState,
    v: &StdVocab,
    ver: &StdVersion,
    graph: &str,
    quads: Vec<Quad>,
    file: &[Triple],
) -> anyhow::Result<()> {
    let staging = format!("urn:system:vocab-seed-staging:{}:{}", v.id, ver.version);
    let staging_node = NamedNode::new(staging.as_str())
        .map_err(|e| anyhow::anyhow!("staging graph IRI {staging}: {e}"))?;
    // Whatever an interrupted earlier restore left there.
    state.store.bulk_delete_graphs(&[staging.as_str()])?;
    let staged: Vec<Quad> = quads
        .into_iter()
        .map(|q| {
            Quad::new(
                q.subject,
                q.predicate,
                q.object,
                GraphName::NamedNode(staging_node.clone()),
            )
        })
        .collect();
    state
        .store
        .bulk_insert_quads(staged, std::slice::from_ref(&staging))?;
    fault("staged")?;
    let in_staging = content_digest::graph_triples(&state.store, &staging)?;
    if !content_digest::same_triples(&in_staging, file) {
        anyhow::bail!(
            "{} {}: the staging graph does not hold the triples of vocab/{}; nothing was replaced",
            v.id,
            ver.version,
            ver.file.path
        );
    }
    state.store.update(&format!(
        "DELETE WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }} ;\n\
         INSERT {{ GRAPH <{graph}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{staging}> {{ ?s ?p ?o }} }} ;\n\
         DELETE WHERE {{ GRAPH <{staging}> {{ ?s ?p ?o }} }}"
    ))?;
    Ok(())
}

/// For a vocabulary whose licence allows no altered copies (IMBOR), the
/// registry must serve nothing under the seeder's entry but the checked,
/// unchanged copy of the file. Nothing is deleted:
///
/// * a version an earlier seeder created there and this build no longer
///   ships ([`RETIRED_SEEDED_VERSIONS`]) is kept. It has no licence record of
///   the file, so the registry already serves it to no one who may not write
///   the entry; it is also deprecated, so it reads as what it is;
/// * a latest pointer at any version other than a checked copy is moved to
///   the shipped latest version, when that version is the seeder's, published
///   and checked. Otherwise the pointer stays, and the version it points at
///   is withheld all the same. The version it pointed at is kept.
///
/// Returns the number of records changed.
fn enforce_no_derivatives(
    state: &AppState,
    v: &StdVocab,
    snap: &Snapshot,
) -> anyhow::Result<usize> {
    let mut changed = 0usize;
    for r in RETIRED_SEEDED_VERSIONS.iter().filter(|r| r.id == v.id) {
        let Some(p) = snap.versions.get(r.version) else {
            continue;
        };
        let iri = registry::version_record_iri(&state.base_url, v.id, r.version);
        match version_owned(state, v, r.version, p) {
            None => {
                tracing::warn!(
                    "vocabulary '{}': version '{}' is not the one an earlier seeder made; left \
                     as it is",
                    v.id,
                    r.version
                );
                continue;
            }
            Some(Owned::Legacy) => {
                registry::set_seeded_by(&state.store, &iri, SEEDED_BY)?;
                changed += 1;
            }
            Some(Owned::Marked) => {}
        }
        if p.status == Some(VersionStatus::Published) {
            registry::update_version_status(
                &state.store,
                &state.base_url,
                v.id,
                r.version,
                VersionStatus::Deprecated,
            )?;
            state.mark_vocab_registry_dirty();
            tracing::warn!(
                "vocabulary '{}': version '{}', which an earlier build seeded and this one no \
                 longer ships, is kept, deprecated and served to no one who may not write the \
                 entry",
                v.id,
                r.version
            );
            changed += 1;
        }
    }

    let Some(latest) = v.versions.iter().find(|ver| ver.latest) else {
        return Ok(changed);
    };
    let Some(record) = registry::get_data_model(&state.store, &state.base_url, v.id) else {
        return Ok(changed);
    };
    let is_checked = |label: &str| {
        registry::get_attribution(
            &state.store,
            &registry::version_record_iri(&state.base_url, v.id, label),
        )
        .is_some_and(|a| a.no_derivatives && a.unchanged)
    };
    let current = record.latest_published.as_deref();
    if current == Some(latest.version) || current.is_some_and(is_checked) {
        return Ok(changed);
    }
    let target_ok = snap.versions.get(latest.version).is_some_and(|p| {
        version_owned(state, v, latest.version, p).is_some()
            && p.status == Some(VersionStatus::Published)
    }) && is_checked(latest.version);
    if target_ok {
        registry::update_latest_published(&state.store, &state.base_url, v.id, latest.version)?;
        state.mark_vocab_registry_dirty();
        tracing::warn!(
            "vocabulary '{}': its latest published version was '{}', which is not a checked copy \
             of vocab/{}; the licence allows no altered copies, so '{}' is the latest again (the \
             other version is kept)",
            v.id,
            current.unwrap_or("-"),
            latest.file.path,
            latest.version
        );
        changed += 1;
    } else {
        tracing::warn!(
            "vocabulary '{}': its latest published version '{}' is not a checked copy of vocab/{} \
             and '{}' cannot take its place (not published, or not checked); the pointer is left \
             as it is, and that version is served to no one who may not write the entry",
            v.id,
            current.unwrap_or("-"),
            latest.file.path,
            latest.version
        );
    }
    Ok(changed)
}

/// An install seeded before real versioning holds the vocabulary at the
/// synthetic `1.0.0`. That copy is kept: it gets a licence record saying what
/// it is (never "unchanged") and, once the real latest version is there, it
/// is deprecated. Returns the number of records changed.
fn settle_synthetic_version(
    state: &AppState,
    v: &StdVocab,
    snap: &Snapshot,
) -> anyhow::Result<usize> {
    if v.versions
        .iter()
        .any(|ver| ver.version == LEGACY_SYNTHETIC_VERSION)
    {
        return Ok(0);
    }
    let Some(p) = snap.versions.get(LEGACY_SYNTHETIC_VERSION) else {
        return Ok(0);
    };
    let Some(owned) = version_owned(state, v, LEGACY_SYNTHETIC_VERSION, p) else {
        return Ok(0);
    };
    let Some(latest) = v.versions.iter().find(|ver| ver.latest) else {
        return Ok(0);
    };
    let iri = registry::version_record_iri(&state.base_url, v.id, LEGACY_SYNTHETIC_VERSION);
    let mut changed = 0usize;
    if owned == Owned::Legacy {
        registry::set_seeded_by(&state.store, &iri, SEEDED_BY)?;
        changed += 1;
    }
    if p.attribution_json.is_none() {
        let text = format!(
            "Seeded by a release of Open Triplestore before 0.4.0 under the placeholder version \
             label {LEGACY_SYNTHETIC_VERSION}, from that release's copy of the bundled file \
             vocab/{}, possibly with an owl:versionInfo triple that release added. It may differ \
             from the current file and may since have been edited in this registry, so it is not \
             that file.",
            latest.file.path
        );
        if registry::replace_attribution_if(
            &state.store,
            &iri,
            None,
            latest.record(false, Some(&text)).as_ref(),
        )? {
            changed += 1;
        }
    }
    if p.status == Some(VersionStatus::Published)
        && registry::version_exists(&state.store, &state.base_url, v.id, latest.version)
    {
        registry::update_version_status(
            &state.store,
            &state.base_url,
            v.id,
            LEGACY_SYNTHETIC_VERSION,
            VersionStatus::Deprecated,
        )?;
        changed += 1;
    }
    Ok(changed)
}

/// Seed one vocabulary entry and all of its missing versions. Returns the
/// number of versions newly created this call.
///
/// Only into the seeder's own entry ([`entry_owned`]): an entry anyone else
/// made under this id is logged and left alone. A missing version is loaded
/// only into an empty graph, or registered as it is when its graph already
/// holds exactly the file's triples (an earlier start stopped between the
/// load and the record); a graph holding anything else is left alone. The
/// latest pointer moves to the shipped latest version only from no version or
/// from one of the seeder's own.
fn seed_one(state: &AppState, v: &StdVocab) -> anyhow::Result<usize> {
    let snap = Snapshot::read(state, v);
    let entry_iri = registry::data_model_iri(&state.base_url, v.id);
    let latest = v.versions.iter().find(|ver| ver.latest);
    match &snap.model {
        None => {
            registry::insert_data_model(
                &state.store,
                &state.base_url,
                v.id,
                v.title,
                v.namespace,
                Some(DESC),
                true, // public
                None, // system-owned
                None,
                None,
                &chrono::Utc::now().to_rfc3339(),
            )?;
            // The entry's licence record follows once its latest version is
            // loaded and checked ([`sync_seeded_records`], right after this).
            registry::set_seeded_by(&state.store, &entry_iri, SEEDED_BY)?;
        }
        Some(_) if entry_owned(state, v, &snap) => {}
        Some(_) => {
            warn_not_owned(v);
            return Ok(0);
        }
    }

    let mut newly: Vec<&str> = Vec::new();
    let mut kind: Option<RegistryKind> = None;
    for ver in v.versions {
        if snap.versions.contains_key(ver.version)
            || registry::version_exists(&state.store, &state.base_url, v.id, ver.version)
        {
            continue;
        }
        let graph = registry::version_record_iri(&state.base_url, v.id, ver.version);
        let (quads, file) = match file_triples(ver) {
            Ok(x) => x,
            Err(e) => {
                tracing::warn!("vocab '{}' version '{}': {e:#}", v.id, ver.version);
                continue;
            }
        };
        let existing = content_digest::graph_triples(&state.store, &graph)?;
        if !existing.is_empty() && !content_digest::same_triples(&existing, &file) {
            tracing::warn!(
                "vocabulary '{}' version '{}': the graph <{graph}> already holds triples that \
                 are not those of vocab/{}, and no version record; the version is not seeded and \
                 the graph is left as it is",
                v.id,
                ver.version,
                ver.file.path
            );
            continue;
        }
        // The registry stores one kind per entry; derive it from the canonical
        // latest version's content (most representative).
        if ver.latest {
            kind = kind_detector::detect(&quads).primary;
        }
        if existing.is_empty() {
            // Verbatim: the graph gets exactly the file's triples (no added
            // owl:versionInfo), so the stored copy is the bundled file's content.
            upload::load_parsed_verbatim(
                &state.store,
                &state.base_url,
                v.id,
                ver.version,
                quads,
                true, // merge into a single graph per version
            )
            .map_err(|e| anyhow::anyhow!("load {} {}: {e}", v.id, ver.version))?;
        }
        let stored = content_digest::graph_triples(&state.store, &graph)?;
        let unchanged = content_digest::same_triples(&stored, &file);
        let record = DataModelVersion {
            data_model_id: v.id.to_string(),
            version: ver.version.to_string(),
            status: ver.status,
            graph_iri: graph.clone(),
            sub_graphs: vec![graph.clone()],
            created_at: format!("{}T00:00:00Z", ver.date),
            created_by: None,
            derived_from: ver
                .prior
                .map(|p| format!("{}/data-model/{}/version/{}", state.base_url, v.id, p)),
            notes: Some(ver.official_name.to_string()),
            branch: None,
            sub_graph_status: Vec::new(),
        };
        registry::insert_version(&state.store, &state.base_url, &record)?;
        registry::set_seeded_by(&state.store, &graph, SEEDED_BY)?;
        registry::set_attribution(&state.store, &graph, ver.attribution(unchanged).as_ref())?;
        // The digest of the graph as stored, so a later start can tell the
        // seeder's own content from an edit.
        registry::set_seed_check(
            &state.store,
            &graph,
            &content_digest::file_sha256(ver.file.ttl.as_bytes()),
            unchanged
                .then(|| content_digest::triples_digest(&stored))
                .as_deref(),
        )?;
        newly.push(ver.version);
    }

    // Only touch the per-entry kind + latest pointer when we actually created a
    // version this run, so steady-state boots stay cheap (no re-parse). The kind
    // is computed only when the latest version itself was (re)loaded.
    if !newly.is_empty() {
        if let Some(k) = kind {
            registry::set_data_model_kind(&state.store, &state.base_url, v.id, k)?;
        }
        if let Some(lv) = latest.map(|ver| ver.version) {
            let current = registry::get_data_model(&state.store, &state.base_url, v.id)
                .and_then(|m| m.latest_published);
            let movable = match current.as_deref() {
                None => true,
                Some(c) if c == lv => false,
                Some(c) => {
                    newly.contains(&c)
                        || snap
                            .versions
                            .get(c)
                            .is_some_and(|p| version_owned(state, v, c, p).is_some())
                }
            };
            if movable && registry::version_exists(&state.store, &state.base_url, v.id, lv) {
                registry::update_latest_published(&state.store, &state.base_url, v.id, lv)?;
            }
        }
    }
    Ok(newly.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::AppState;
    use crate::store::TripleStore;

    fn fresh() -> AppState {
        AppState::test_default_with_store(TripleStore::in_memory().unwrap())
    }

    fn total_versions() -> usize {
        VOCABS.iter().map(|v| v.versions.len()).sum()
    }

    fn vocab(id: &str) -> &'static StdVocab {
        VOCABS.iter().find(|v| v.id == id).unwrap()
    }

    fn version_iri(state: &AppState, id: &str, ver: &str) -> String {
        registry::version_record_iri(&state.base_url, id, ver)
    }

    fn ask(state: &AppState, q: &str) -> bool {
        matches!(
            state.store.query(q),
            Ok(oxigraph::sparql::QueryResults::Boolean(true))
        )
    }

    fn update(state: &AppState, q: &str) {
        state.store.update(q).unwrap();
    }

    /// Make a version record look like one an older build left behind: no
    /// licence record, no seed check and no marker. Its notes and creation
    /// date stay as the seeder wrote them.
    fn age(state: &AppState, iri: &str) {
        registry::set_attribution(&state.store, iri, None).unwrap();
        update(
            state,
            &format!(
                "DELETE WHERE {{ GRAPH <{g}> {{ <{iri}> <urn:system:vocab/seedSourceSha256> ?s }} }};\n\
                 DELETE WHERE {{ GRAPH <{g}> {{ <{iri}> <urn:system:vocab/seedContentDigest> ?d }} }};\n\
                 DELETE WHERE {{ GRAPH <{g}> {{ <{iri}> <urn:system:vocab/seededBy> ?m }} }}",
                g = registry::REGISTRY_GRAPH
            ),
        );
    }

    /// Age an entry and all of its versions.
    fn age_entry(state: &AppState, id: &str) {
        let entry = registry::data_model_iri(&state.base_url, id);
        age(state, &entry);
        for ver in vocab(id).versions {
            age(state, &version_iri(state, id, ver.version));
        }
    }

    /// Make the recorded seed check name another file, as if an earlier build
    /// had shipped a different one.
    fn record_other_file(state: &AppState, iri: &str, digest: Option<&str>) {
        registry::set_seed_check(&state.store, iri, "sha-of-an-earlier-file", digest).unwrap();
    }

    fn record(state: &AppState, id: &str, ver: &str) -> ContentAttribution {
        registry::get_attribution(&state.store, &version_iri(state, id, ver)).unwrap()
    }

    fn stored_triples(state: &AppState, id: &str, ver: &str) -> Vec<Triple> {
        content_digest::graph_triples(&state.store, &version_iri(state, id, ver)).unwrap()
    }

    /// A file's triples as the store holds them.
    fn file_as_stored(f: &BundledFile) -> Vec<Triple> {
        content_digest::as_stored(&content_digest::quads_as_triples(
            &upload::parse_rdf(f.ttl.as_bytes(), "text/turtle", f.path).unwrap(),
        ))
        .unwrap()
    }

    fn seeded_by(state: &AppState, iri: &str) -> Option<String> {
        let q = format!(
            "SELECT ?m WHERE {{ GRAPH <{}> {{ <{iri}> <urn:system:vocab/seededBy> ?m }} }}",
            registry::REGISTRY_GRAPH
        );
        match state.store.query(&q) {
            Ok(oxigraph::sparql::QueryResults::Solutions(mut rows)) => rows
                .next()
                .and_then(|r| r.ok())
                .and_then(|r| match r.get("m") {
                    Some(oxigraph::model::Term::Literal(l)) => Some(l.value().to_string()),
                    _ => None,
                }),
            _ => None,
        }
    }

    const IMBOR_SUBJECT: &str =
        "https://data.crow.nl/imbor/term/74e825e1-9b93-4dd5-8fab-52783fdb758b";
    const SKOS_DEFINITION: &str = "http://www.w3.org/2004/02/skos/core#definition";
    const CROW_NOTE: &str = "(Bron: CB-NL/RWS-OTL 2016)";
    const ALTERED_NOTE: &str = "(Bron: CB-NL/asset-management 2016)";

    /// The `skos:definition` of [`IMBOR_SUBJECT`] in `graph` that contains
    /// `text`, or "".
    fn imbor_definition(state: &AppState, graph: &str, text: &str) -> String {
        let q = format!(
            "SELECT ?o WHERE {{ GRAPH <{graph}> {{ <{IMBOR_SUBJECT}> <{SKOS_DEFINITION}> ?o }} }}"
        );
        let Ok(oxigraph::sparql::QueryResults::Solutions(rows)) = state.store.query(&q) else {
            panic!("query failed");
        };
        rows.flatten()
            .filter_map(|r| match r.get("o") {
                Some(oxigraph::model::Term::Literal(l)) if l.value().contains(text) => {
                    Some(l.value().to_string())
                }
                _ => None,
            })
            .next()
            .unwrap_or_default()
    }

    /// Put the altered source note 0.5.0/0.6.0 seeded into IMBOR's graph.
    fn alter_imbor(state: &AppState) -> String {
        let iri = version_iri(state, "imbor", "2025");
        let good = imbor_definition(state, &iri, CROW_NOTE);
        assert!(!good.is_empty(), "CROW's note is in the seeded copy");
        let bad = good.replace(CROW_NOTE, ALTERED_NOTE);
        update(
            state,
            &format!(
                "DELETE DATA {{ GRAPH <{iri}> {{ <{IMBOR_SUBJECT}> <{SKOS_DEFINITION}> \"{g}\"@nl }} }};\n\
                 INSERT DATA {{ GRAPH <{iri}> {{ <{IMBOR_SUBJECT}> <{SKOS_DEFINITION}> \"{b}\"@nl }} }}",
                g = crate::store::escape_sparql_literal(&good),
                b = crate::store::escape_sparql_literal(&bad),
            ),
        );
        bad
    }

    /// One vocabulary's start: seed it, then check it.
    fn seed_only(state: &AppState, id: &str) {
        seed_one(state, vocab(id)).unwrap();
        sync_seeded_records(state, vocab(id), Mode::Seed).unwrap();
    }

    /// The versions of the `imbor` entry kept aside by the seeder.
    fn kept_versions(state: &AppState) -> Vec<DataModelVersion> {
        registry::list_versions(&state.store, &state.base_url, "imbor")
            .into_iter()
            .filter(|v| v.version.starts_with("2025-kept-"))
            .collect()
    }

    /// Every bundled vocabulary is published at its curated latest version (never
    /// a draft, never the synthetic `1.0.0`), and OWL/RDF/DCAT ship multiple
    /// versions out of the box. Every record carries the seeder's marker.
    #[test]
    fn seeds_every_vocabulary_with_all_versions() {
        let state = fresh();
        let seeded = seed_standard_vocabularies(&state);
        assert_eq!(seeded, total_versions(), "every bundled version seeded");

        for v in VOCABS {
            let rec = registry::get_data_model(&state.store, &state.base_url, v.id)
                .unwrap_or_else(|| panic!("vocabulary '{}' should be seeded", v.id));
            let latest = v
                .versions
                .iter()
                .find(|ver| ver.latest)
                .map(|ver| ver.version);
            assert_eq!(
                rec.latest_published.as_deref(),
                latest,
                "vocabulary '{}' published at its curated latest version",
                v.id
            );
            assert_eq!(
                seeded_by(&state, &registry::data_model_iri(&state.base_url, v.id)).as_deref(),
                Some(SEEDED_BY)
            );
            for ver in v.versions {
                assert_eq!(
                    seeded_by(&state, &version_iri(&state, v.id, ver.version)).as_deref(),
                    Some(SEEDED_BY),
                    "{} {}",
                    v.id,
                    ver.version
                );
            }
        }

        for (id, ver) in [
            ("owl", "1.0"),
            ("owl", "2.0"),
            ("rdf", "1.0"),
            ("rdf", "1.1"),
            ("rdf", "1.2"),
            ("dcat", "1.0"),
            ("dcat", "2.0"),
            ("dcat", "3.0"),
            ("geosparql", "1.0"),
            ("geosparql", "1.1"),
        ] {
            assert!(
                registry::version_exists(&state.store, &state.base_url, id, ver),
                "{id} should ship version {ver}"
            );
        }
        let rdf = registry::get_data_model(&state.store, &state.base_url, "rdf").unwrap();
        assert_eq!(
            rdf.latest_published.as_deref(),
            Some("1.1"),
            "a draft (RDF 1.2) must never be the latest published version"
        );

        // Idempotent: a second pass seeds nothing new.
        assert_eq!(seed_standard_vocabularies(&state), 0);
    }

    /// An install seeded under the old synthetic `1.0.0` keeps that copy
    /// (deprecated, with a record that never calls it unchanged) and gains
    /// the real versions, the latest pointer moving to the real latest.
    #[test]
    fn a_synthetic_version_from_before_0_4_is_kept_next_to_the_real_ones() {
        let state = fresh();
        let owl = vocab("owl");
        let quads = upload::parse_rdf(
            owl.versions[0].file.ttl.as_bytes(),
            "text/turtle",
            "owl.ttl",
        )
        .unwrap();
        registry::insert_data_model(
            &state.store,
            &state.base_url,
            owl.id,
            owl.title,
            owl.namespace,
            Some(DESC),
            true,
            None,
            None,
            None,
            "2020-01-01T00:00:00+00:00",
        )
        .unwrap();
        let result = upload::load_parsed(
            &state.store,
            &state.base_url,
            owl.id,
            Some(LEGACY_SYNTHETIC_VERSION),
            quads,
            true,
        )
        .unwrap();
        let graph_iri = version_iri(&state, owl.id, LEGACY_SYNTHETIC_VERSION);
        registry::insert_version(
            &state.store,
            &state.base_url,
            &DataModelVersion {
                data_model_id: owl.id.to_string(),
                version: result.version.clone(),
                status: VersionStatus::Published,
                graph_iri: graph_iri.clone(),
                sub_graphs: result.sub_graphs,
                created_at: "2020-01-01T00:00:00+00:00".to_string(),
                created_by: None,
                derived_from: None,
                notes: None,
                branch: None,
                sub_graph_status: Vec::new(),
            },
        )
        .unwrap();
        registry::update_latest_published(
            &state.store,
            &state.base_url,
            owl.id,
            LEGACY_SYNTHETIC_VERSION,
        )
        .unwrap();
        let before = content_digest::triples_digest(
            &content_digest::graph_triples(&state.store, &graph_iri).unwrap(),
        );

        seed_standard_vocabularies(&state);
        let owl_rec = registry::get_data_model(&state.store, &state.base_url, "owl").unwrap();
        assert_eq!(owl_rec.latest_published.as_deref(), Some("2.0"));
        assert!(registry::version_exists(
            &state.store,
            &state.base_url,
            "owl",
            "1.0"
        ));
        let synthetic = registry::get_version(
            &state.store,
            &state.base_url,
            "owl",
            LEGACY_SYNTHETIC_VERSION,
        )
        .expect("the synthetic version is kept");
        assert_eq!(synthetic.status, VersionStatus::Deprecated);
        assert_eq!(
            content_digest::triples_digest(
                &content_digest::graph_triples(&state.store, &graph_iri).unwrap()
            ),
            before,
            "its graph is untouched"
        );
        let a = record(&state, "owl", LEGACY_SYNTHETIC_VERSION);
        assert!(!a.unchanged);
        assert!(a.stored_copy.contains("placeholder version label 1.0.0"));
    }

    /// Every seeded entry and version carries its file's licence record (Open
    /// Triplestore's own vocabulary none), and a second boot changes nothing.
    #[test]
    fn every_seeded_record_carries_its_licence_record() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        for v in VOCABS {
            let latest = v.versions.iter().find(|ver| ver.latest).unwrap();
            assert_eq!(
                registry::get_attribution(
                    &state.store,
                    &registry::data_model_iri(&state.base_url, v.id)
                ),
                latest.attribution(true),
                "entry {}",
                v.id
            );
            for ver in v.versions {
                let got = registry::get_attribution(
                    &state.store,
                    &version_iri(&state, v.id, ver.version),
                );
                assert_eq!(got, ver.attribution(true), "{} {}", v.id, ver.version);
                if v.id != "ots" {
                    let a = got.unwrap();
                    assert_eq!(a.file, ver.file.path);
                    assert_eq!(a.specification_url.as_deref(), Some(ver.spec_url));
                }
            }
        }
        for v in VOCABS {
            assert_eq!(
                sync_seeded_records(&state, v, Mode::Seed).unwrap(),
                0,
                "steady state rewrites nothing for {}",
                v.id
            );
        }
    }

    /// An install seeded before licence records: records without one, the
    /// `owl:versionInfo` triple the old loader added, and notes in an older
    /// wording. The next boot proves the records are the seeder's and marks
    /// them, adds the licence records and rewords only notes nobody edited.
    /// The graphs are never modified: the added triple stays, and the records
    /// say the copies differ from the file. Records created through the API
    /// are left alone.
    #[test]
    fn an_older_install_gets_licence_records_and_keeps_its_graphs() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        let rdf = vocab("rdf");
        let quads =
            upload::parse_rdf(rdf.versions[1].file.ttl.as_bytes(), "text/turtle", "rdf").unwrap();
        let subject = upload::injected_version_info(&quads, &state.base_url, "rdf")
            .expect("rdf.ttl states no owl:versionInfo");
        let owl_version_info = "http://www.w3.org/2002/07/owl#versionInfo";
        age_entry(&state, "rdf");
        for ver in rdf.versions {
            let iri = version_iri(&state, "rdf", ver.version);
            update(
                &state,
                &format!(
                    "INSERT DATA {{ GRAPH <{iri}> {{ <{s}> <{owl_version_info}> \"{v}\" }} }}",
                    s = subject.as_str(),
                    v = ver.version
                ),
            );
        }
        registry::update_version_notes(
            &state.store,
            &state.base_url,
            "rdf",
            "1.0",
            Some("RDF 1.0 (2004)"),
        )
        .unwrap();
        // A version someone created through the API: not the seeder's.
        let user_graph = version_iri(&state, "rdf", "9.9");
        registry::insert_version(
            &state.store,
            &state.base_url,
            &DataModelVersion {
                data_model_id: "rdf".into(),
                version: "9.9".into(),
                status: VersionStatus::Draft,
                graph_iri: user_graph.clone(),
                sub_graphs: Vec::new(),
                created_at: "2026-01-01T00:00:00Z".into(),
                created_by: Some(format!("{}/users/u1", state.base_url)),
                derived_from: None,
                notes: Some("RDF 1.0 (2004)".into()),
                branch: None,
                sub_graph_status: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(
            seed_standard_vocabularies(&state),
            0,
            "no version re-seeded"
        );

        for ver in rdf.versions {
            let iri = version_iri(&state, "rdf", ver.version);
            assert_eq!(seeded_by(&state, &iri).as_deref(), Some(SEEDED_BY));
            let a = record(&state, "rdf", ver.version);
            assert!(!a.unchanged, "{}: it holds an added triple", ver.version);
            assert_eq!(a.licenses, ver.attribution(true).unwrap().licenses);
            assert!(
                ask(
                    &state,
                    &format!("ASK {{ GRAPH <{iri}> {{ ?s <{owl_version_info}> ?o }} }}")
                ),
                "the graph is kept exactly as stored: {iri}"
            );
        }
        let entry = registry::get_attribution(
            &state.store,
            &registry::data_model_iri(&state.base_url, "rdf"),
        )
        .unwrap();
        assert!(!entry.unchanged);
        let notes = |ver: &str| {
            registry::get_version(&state.store, &state.base_url, "rdf", ver)
                .unwrap()
                .notes
        };
        assert_eq!(
            notes("1.0").as_deref(),
            Some("RDF 1.0 (2004) — served with the RDF 1.1 namespace document")
        );
        assert_eq!(notes("9.9").as_deref(), Some("RDF 1.0 (2004)"));
        assert!(registry::get_attribution(&state.store, &user_graph).is_none());
        assert!(seeded_by(&state, &user_graph).is_none());
        // Labelled once; the next boot writes nothing.
        assert_eq!(sync_seeded_records(&state, rdf, Mode::Seed).unwrap(), 0);
    }

    /// A seeded record whose notes an admin edited no longer reads as the
    /// seeder wrote it: without the marker it is not proven to be the
    /// seeder's, so it is left alone.
    #[test]
    fn an_unmarked_record_whose_notes_were_edited_is_left_alone() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        age_entry(&state, "rdf");
        registry::update_version_notes(
            &state.store,
            &state.base_url,
            "rdf",
            "1.2",
            Some("Our notes"),
        )
        .unwrap();
        seed_standard_vocabularies(&state);
        let iri = version_iri(&state, "rdf", "1.2");
        assert!(seeded_by(&state, &iri).is_none());
        assert!(registry::get_attribution(&state.store, &iri).is_none());
        // The others are proven by their notes and dates.
        assert!(record(&state, "rdf", "1.1").unchanged);
    }

    /// A file that states its own `owl:versionInfo` never had one added: an
    /// aged IMBOR copy is checked, found unchanged and labelled.
    #[test]
    fn an_aged_intact_imbor_copy_is_found_unchanged() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        let iri = version_iri(&state, "imbor", "2025");
        age_entry(&state, "imbor");
        sync_seeded_records(&state, vocab("imbor"), Mode::Seed).unwrap();
        assert!(ask(
            &state,
            &format!(
                "ASK {{ GRAPH <{iri}> {{ ?s <http://www.w3.org/2002/07/owl#versionInfo> \
                 \"IMBOR2025\" }} }}"
            )
        ));
        let a = record(&state, "imbor", "2025");
        assert!(a.no_derivatives && a.unchanged);
        assert!(kept_versions(&state).is_empty(), "nothing to keep aside");
    }

    /// The stored copies the seeder loads digest exactly like the files'
    /// parses, and every seeded version records the check, so the next boot
    /// takes the cheap path: no parse, no write.
    #[test]
    fn every_seeded_version_records_its_check() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        for v in VOCABS {
            let (_, versions) = registry::record_provenance(&state.store, &state.base_url, v.id);
            for ver in v.versions {
                let p = &versions[ver.version];
                assert_eq!(
                    p.seed_source_sha256.as_deref(),
                    Some(content_digest::file_sha256(ver.file.ttl.as_bytes()).as_str()),
                    "{} {}",
                    v.id,
                    ver.version
                );
                assert_eq!(
                    p.seed_content_digest.as_deref(),
                    Some(
                        content_digest::triples_digest(&stored_triples(&state, v.id, ver.version))
                            .as_str()
                    ),
                    "{} {}: the recorded digest is the stored graph's",
                    v.id,
                    ver.version
                );
                if v.id != "ots" {
                    assert!(record(&state, v.id, ver.version).unchanged);
                }
            }
        }
    }

    /// Releases 0.5.0 and 0.6.0 seeded IMBOR from a file with one altered
    /// source note. On the next boot the stored copy is first kept aside, as a
    /// private, creator-less, deprecated version withheld by its licence
    /// record, and only then is the canonical version restored to CROW's
    /// release. An XSD copy with an earlier wording of one comment (licence
    /// allows other copies) is never modified: it is kept and labelled.
    #[test]
    fn an_aged_imbor_copy_is_kept_aside_and_the_release_restored() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        let iri = version_iri(&state, "imbor", "2025");
        let bad = alter_imbor(&state);
        age_entry(&state, "imbor");
        assert_eq!(imbor_definition(&state, &iri, ALTERED_NOTE), bad);

        let xsd_iri = version_iri(&state, "xsd", "1.1");
        let old_comment = "Hex-encoded arbitrary binary data.";
        update(
            &state,
            &format!(
                "DELETE WHERE {{ GRAPH <{xsd_iri}> {{ <http://www.w3.org/2001/XMLSchema#hexBinary> \
                 <http://www.w3.org/2000/01/rdf-schema#comment> ?o }} }};\n\
                 INSERT DATA {{ GRAPH <{xsd_iri}> {{ <http://www.w3.org/2001/XMLSchema#hexBinary> \
                 <http://www.w3.org/2000/01/rdf-schema#comment> \"{old_comment}\"@en }} }}"
            ),
        );
        age(&state, &xsd_iri);

        assert_eq!(seed_standard_vocabularies(&state), 0, "nothing seeded anew");

        // The canonical version is CROW's release again, and says so.
        assert!(imbor_definition(&state, &iri, ALTERED_NOTE).is_empty());
        assert!(!imbor_definition(&state, &iri, CROW_NOTE).is_empty());
        assert!(content_digest::same_triples(
            &stored_triples(&state, "imbor", "2025"),
            &file_as_stored(&vf::IMBOR)
        ));
        let a = record(&state, "imbor", "2025");
        assert!(a.unchanged && a.no_derivatives);

        // The altered copy is kept, private and withheld.
        let kept = kept_versions(&state);
        assert_eq!(kept.len(), 1, "{kept:?}");
        let k = &kept[0];
        assert_eq!(k.version, "2025-kept-1");
        assert!(k.created_by.is_none());
        assert_eq!(k.status, VersionStatus::Deprecated);
        assert!(k
            .notes
            .as_deref()
            .unwrap()
            .starts_with("Kept by Open Triplestore"));
        assert_eq!(imbor_definition(&state, &k.graph_iri, ALTERED_NOTE), bad);
        let ka = record(&state, "imbor", "2025-kept-1");
        assert!(ka.no_derivatives && !ka.unchanged);
        assert_eq!(
            seeded_by(&state, &version_iri(&state, "imbor", "2025-kept-1")).as_deref(),
            Some(KEPT_BY)
        );
        let entry = registry::get_data_model(&state.store, &state.base_url, "imbor").unwrap();
        assert_eq!(entry.latest_published.as_deref(), Some("2025"));

        // XSD: kept exactly as stored, and labelled.
        assert!(ask(
            &state,
            &format!("ASK {{ GRAPH <{xsd_iri}> {{ ?s ?p \"{old_comment}\"@en }} }}")
        ));
        assert!(!record(&state, "xsd", "1.1").unchanged);

        // Repaired and recorded: the next boot changes nothing.
        assert_eq!(
            sync_seeded_records(&state, vocab("imbor"), Mode::Seed).unwrap(),
            0
        );
        assert_eq!(kept_versions(&state).len(), 1);
    }

    /// A seeded version an admin edited in place (RDF 1.2 is seeded as a
    /// Draft) is never overwritten: it is kept and labelled as possibly
    /// modified, on this and every later boot.
    #[test]
    fn an_edited_seeded_draft_is_kept_and_labelled_possibly_modified() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        let rdf = vocab("rdf");
        let iri = version_iri(&state, "rdf", "1.2");
        let edit = "<http://ex.org/our-term> <http://www.w3.org/2000/01/rdf-schema#label> \"ours\"";
        update(
            &state,
            &format!("INSERT DATA {{ GRAPH <{iri}> {{ {edit} }} }}"),
        );
        for aged in [true, false] {
            if aged {
                age(&state, &iri);
            } else {
                record_other_file(&state, &iri, Some("digest-of-the-seeders-earlier-content"));
            }
            sync_seeded_records(&state, rdf, Mode::Seed).unwrap();
            assert!(
                ask(&state, &format!("ASK {{ GRAPH <{iri}> {{ {edit} }} }}")),
                "the admin's triple is kept"
            );
            let a = record(&state, "rdf", "1.2");
            assert!(!a.unchanged);
            assert!(!a.stored_copy.contains("unchanged"), "{}", a.stored_copy);
            let pre = vf::download_preamble(&a, "/n").unwrap();
            assert!(!pre.contains("unchanged"), "{pre}");
            assert!(pre.contains("may have been modified"));
            assert_eq!(sync_seeded_records(&state, rdf, Mode::Seed).unwrap(), 0);
            assert!(!record(&state, "rdf", "1.2").unchanged);
        }
        assert!(record(&state, "rdf", "1.1").unchanged);
    }

    /// The reviewer's scenario: an admin's SPARQL edit to a seeded Published
    /// copy (BOT) survives the upgrade and every later boot, including one
    /// that ships a different file, and the copy is labelled as possibly
    /// modified. Nothing is reloaded over it.
    #[test]
    fn an_admin_edit_to_a_published_seeded_copy_survives_upgrade_and_every_boot() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        let iri = version_iri(&state, "bot", "0.3.2");
        let edit = "<https://w3id.org/bot#Building> \
                    <http://www.w3.org/2004/02/skos/core#altLabel> \"gebouw\"@nl";
        update(
            &state,
            &format!("INSERT DATA {{ GRAPH <{iri}> {{ {edit} }} }}"),
        );
        // An install from before the licence records (0.5/0.6).
        age_entry(&state, "bot");
        for boot in 0..3 {
            if boot == 2 {
                // A later release ships a different bot.ttl.
                record_other_file(&state, &iri, None);
            }
            seed_standard_vocabularies(&state);
            assert!(
                ask(&state, &format!("ASK {{ GRAPH <{iri}> {{ {edit} }} }}")),
                "boot {boot}: the admin's triple survives"
            );
            assert!(!record(&state, "bot", "0.3.2").unchanged, "boot {boot}");
        }
        let entry = registry::get_attribution(
            &state.store,
            &registry::data_model_iri(&state.base_url, "bot"),
        )
        .unwrap();
        assert!(!entry.unchanged);
    }

    /// A seeded copy that holds exactly the seeder's earlier content still
    /// differs from the file this build ships: it is never modified, only
    /// labelled.
    #[test]
    fn a_differing_copy_is_never_modified_even_when_it_is_the_seeders_earlier_content() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        let rdf = vocab("rdf");
        let iri = version_iri(&state, "rdf", "1.2");
        update(
            &state,
            &format!(
                "DELETE DATA {{ GRAPH <{iri}> {{ <http://www.w3.org/1999/02/22-rdf-syntax-ns#Seq> \
                 <http://www.w3.org/2000/01/rdf-schema#subClassOf> \
                 <http://www.w3.org/2000/01/rdf-schema#Container> }} }}"
            ),
        );
        let earlier = content_digest::triples_digest(&stored_triples(&state, "rdf", "1.2"));
        record_other_file(&state, &iri, Some(&earlier));
        sync_seeded_records(&state, rdf, Mode::Seed).unwrap();
        assert_eq!(
            content_digest::triples_digest(&stored_triples(&state, "rdf", "1.2")),
            earlier,
            "not reloaded"
        );
        assert!(!record(&state, "rdf", "1.2").unchanged);
    }

    /// The upgrade path for every bundled file: an install without seed checks
    /// whose copies are intact gets each one checked and found unchanged,
    /// blank nodes and the store's literal forms included, so nothing is
    /// reloaded and nothing is labelled as possibly modified.
    #[test]
    fn intact_copies_from_before_the_seed_checks_are_found_unchanged() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        let mut before = Vec::new();
        for v in VOCABS {
            age(&state, &registry::data_model_iri(&state.base_url, v.id));
            for ver in v.versions {
                let iri = version_iri(&state, v.id, ver.version);
                before.push((
                    v.id,
                    ver.version,
                    content_digest::triples_digest(&stored_triples(&state, v.id, ver.version)),
                ));
                age(&state, &iri);
            }
        }
        seed_standard_vocabularies(&state);
        for (id, version, digest) in before {
            assert_eq!(
                content_digest::triples_digest(&stored_triples(&state, id, version)),
                digest,
                "{id} {version} was reloaded"
            );
            if id != "ots" {
                assert!(record(&state, id, version).unchanged, "{id} {version}");
            }
            assert_eq!(
                seeded_by(&state, &version_iri(&state, id, version)).as_deref(),
                Some(SEEDED_BY),
                "{id} {version} proven and marked"
            );
        }
        assert!(kept_versions(&state).is_empty());
    }

    /// IMBOR's copy is re-verified on every start: a write outside the
    /// registry (here a raw SPARQL Update) is kept aside and undone on the
    /// next boot, even with `SEED_STANDARD_VOCABS=false`, which seeds nothing
    /// and leaves the other vocabularies' copies alone.
    #[test]
    fn a_raw_write_into_imbor_is_kept_aside_and_undone_even_with_seeding_off() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        let iri = version_iri(&state, "imbor", "2025");
        let added = "<https://data.crow.nl/imbor/term/ours> \
                     <http://www.w3.org/2004/02/skos/core#prefLabel> \"ours\"@nl";
        update(
            &state,
            &format!("INSERT DATA {{ GRAPH <{iri}> {{ {added} }} }}"),
        );
        let bot = version_iri(&state, "bot", "0.3.2");
        let bot_edit =
            "<https://w3id.org/bot#x> <http://www.w3.org/2000/01/rdf-schema#label> \"x\"";
        update(
            &state,
            &format!("INSERT DATA {{ GRAPH <{bot}> {{ {bot_edit} }} }}"),
        );
        record_other_file(&state, &bot, None);

        assert_eq!(seed_standard_vocabularies_with(&state, false), 0);

        assert!(!ask(
            &state,
            &format!("ASK {{ GRAPH <{iri}> {{ {added} }} }}")
        ));
        assert!(content_digest::same_triples(
            &stored_triples(&state, "imbor", "2025"),
            &file_as_stored(&vf::IMBOR)
        ));
        assert!(record(&state, "imbor", "2025").unchanged);
        let kept = kept_versions(&state);
        assert_eq!(kept.len(), 1);
        assert!(ask(
            &state,
            &format!("ASK {{ GRAPH <{}> {{ {added} }} }}", kept[0].graph_iri)
        ));
        // Not checked with seeding off: BOT keeps its record as it was.
        assert!(record(&state, "bot", "0.3.2").unchanged);
        assert_eq!(
            sync_seeded_records(&state, vocab("imbor"), Mode::VerifyOnly).unwrap(),
            0
        );
    }

    /// With seeding off, an install that never seeded IMBOR stays untouched.
    #[test]
    fn seeding_off_creates_nothing() {
        let state = fresh();
        assert_eq!(seed_standard_vocabularies_with(&state, false), 0);
        assert!(registry::list_data_models(&state.store).is_empty());
    }

    /// A failure at any step of the IMBOR repair loses nothing: the stored
    /// data is either still in the version's graph or in a kept copy, the
    /// graph is never left empty or half-written, the record never says
    /// unchanged meanwhile, and the next start completes the repair without
    /// a second copy.
    #[test]
    fn a_failure_at_any_step_of_the_repair_loses_nothing() {
        for step in ["labelled", "copied", "verified", "staged", "restored"] {
            let state = fresh();
            seed_only(&state, "imbor");
            let iri = version_iri(&state, "imbor", "2025");
            let bad = alter_imbor(&state);
            let file = file_as_stored(&vf::IMBOR);

            FAIL_AT.with(|f| f.set(Some(step)));
            let result = sync_seeded_records(&state, vocab("imbor"), Mode::Seed);
            FAIL_AT.with(|f| f.set(None));
            assert!(result.is_err(), "{step}: the injected failure surfaces");

            assert!(!record(&state, "imbor", "2025").unchanged, "{step}");
            let stored = stored_triples(&state, "imbor", "2025");
            let in_version = imbor_definition(&state, &iri, ALTERED_NOTE) == bad;
            let in_kept = (1..=3).any(|n| {
                let g = version_iri(&state, "imbor", &format!("2025-kept-{n}"));
                imbor_definition(&state, &g, ALTERED_NOTE) == bad
            });
            assert!(in_version || in_kept, "{step}: the altered copy survives");
            assert!(
                in_version && stored.len() == file.len()
                    || content_digest::same_triples(&stored, &file),
                "{step}: the version's graph is the old copy or the file, never partial"
            );

            seed_only(&state, "imbor");
            assert!(record(&state, "imbor", "2025").unchanged, "{step}");
            assert!(content_digest::same_triples(
                &stored_triples(&state, "imbor", "2025"),
                &file
            ));
            let kept = kept_versions(&state);
            assert_eq!(kept.len(), 1, "{step}: one kept copy: {kept:?}");
            assert_eq!(
                imbor_definition(&state, &kept[0].graph_iri, ALTERED_NOTE),
                bad,
                "{step}"
            );
            assert!(
                !ask(
                    &state,
                    "ASK { GRAPH <urn:system:vocab-seed-staging:imbor:2025> { ?s ?p ?o } }"
                ),
                "{step}: the staging graph is empty"
            );
        }
    }

    /// Releases 0.4.0 to 0.6.0 seeded a hand-authored IMBOR "excerpt". It is
    /// kept (graph and record), deprecated and withheld; a latest pointer at
    /// anything but the checked copy moves back. A version someone made is
    /// kept too, and an excerpt that does not read as the seeder wrote it is
    /// left exactly as it is.
    #[test]
    fn the_retired_imbor_excerpt_is_kept_deprecated_and_withheld() {
        for seeders in [true, false] {
            let state = fresh();
            seed_standard_vocabularies(&state);
            let excerpt = version_iri(&state, "imbor", "excerpt");
            update(
                &state,
                &format!(
                    "INSERT DATA {{ GRAPH <{excerpt}> {{ <https://data.crow.nl/imbor/def/Areaal> \
                     <http://www.w3.org/2000/01/rdf-schema#label> \"Asset area\"@en }} }}"
                ),
            );
            for (label, created_at, notes, creator) in [
                (
                    "excerpt",
                    "2024-01-01T00:00:00Z",
                    if seeders {
                        "CROW IMBOR (excerpt)"
                    } else {
                        "Our excerpt"
                    },
                    None,
                ),
                ("2025-ours", "2026-01-01T00:00:00Z", "ours", Some("u1")),
            ] {
                registry::insert_version(
                    &state.store,
                    &state.base_url,
                    &DataModelVersion {
                        data_model_id: "imbor".into(),
                        version: label.into(),
                        status: VersionStatus::Published,
                        graph_iri: version_iri(&state, "imbor", label),
                        sub_graphs: vec![version_iri(&state, "imbor", label)],
                        created_at: created_at.into(),
                        created_by: creator.map(|u| format!("{}/users/{u}", state.base_url)),
                        derived_from: None,
                        notes: Some(notes.into()),
                        branch: None,
                        sub_graph_status: Vec::new(),
                    },
                )
                .unwrap();
            }
            registry::update_latest_published(&state.store, &state.base_url, "imbor", "2025-ours")
                .unwrap();

            seed_standard_vocabularies(&state);

            let ex = registry::get_version(&state.store, &state.base_url, "imbor", "excerpt")
                .expect("the excerpt is kept");
            assert!(ask(
                &state,
                &format!("ASK {{ GRAPH <{excerpt}> {{ ?s ?p ?o }} }}")
            ));
            assert!(
                registry::get_attribution(&state.store, &excerpt).is_none(),
                "no licence record of CROW's file on it: withheld"
            );
            if seeders {
                assert_eq!(ex.status, VersionStatus::Deprecated);
            } else {
                assert_eq!(ex.status, VersionStatus::Published, "not the seeder's");
                assert!(seeded_by(&state, &excerpt).is_none());
            }
            assert!(registry::version_exists(
                &state.store,
                &state.base_url,
                "imbor",
                "2025-ours"
            ));
            let entry = registry::get_data_model(&state.store, &state.base_url, "imbor").unwrap();
            assert_eq!(entry.latest_published.as_deref(), Some("2025"));
        }
    }

    /// The pointer never moves to a version that is not published: when the
    /// checked copy was deprecated, the pointer stays where it is (that
    /// version is withheld all the same).
    #[test]
    fn the_latest_pointer_never_moves_to_a_deprecated_version() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        registry::insert_version(
            &state.store,
            &state.base_url,
            &DataModelVersion {
                data_model_id: "imbor".into(),
                version: "2025-ours".into(),
                status: VersionStatus::Published,
                graph_iri: version_iri(&state, "imbor", "2025-ours"),
                sub_graphs: Vec::new(),
                created_at: "2026-01-01T00:00:00Z".into(),
                created_by: Some(format!("{}/users/u1", state.base_url)),
                derived_from: None,
                notes: None,
                branch: None,
                sub_graph_status: Vec::new(),
            },
        )
        .unwrap();
        registry::update_latest_published(&state.store, &state.base_url, "imbor", "2025-ours")
            .unwrap();
        registry::update_version_status(
            &state.store,
            &state.base_url,
            "imbor",
            "2025",
            VersionStatus::Deprecated,
        )
        .unwrap();
        seed_standard_vocabularies(&state);
        let entry = registry::get_data_model(&state.store, &state.base_url, "imbor").unwrap();
        assert_eq!(entry.latest_published.as_deref(), Some("2025-ours"));
        let v = registry::get_version(&state.store, &state.base_url, "imbor", "2025").unwrap();
        assert_eq!(
            v.status,
            VersionStatus::Deprecated,
            "the admin's status stays"
        );
    }

    /// A model a user created with the id `imbor` (through the API, so with a
    /// creator) is never touched: no version is added, no licence record is
    /// written, so no guard locks its owner out, and its latest pointer stays.
    /// The same holds for an entry a seed bundle or a LOV install made (no
    /// creator, but none of the seeder's versions).
    #[test]
    fn an_entry_the_seeder_did_not_create_is_never_touched() {
        for creator in [Some("u1"), None] {
            let state = fresh();
            let (owner_type, owner_id) = match creator {
                Some(_) => (Some("user"), Some("u1")),
                None => (Some("organisation"), Some("org-1")),
            };
            registry::insert_data_model(
                &state.store,
                &state.base_url,
                "imbor",
                "IMBOR",
                "https://example.org/imbor#",
                None,
                true,
                owner_type,
                owner_id,
                creator
                    .map(|u| format!("{}/users/{u}", state.base_url))
                    .as_deref(),
                "2026-01-01T00:00:00Z",
            )
            .unwrap();
            let ours = version_iri(&state, "imbor", "2025");
            update(
                &state,
                &format!(
                    "INSERT DATA {{ GRAPH <{ours}> {{ <https://example.org/imbor#a> \
                     <http://www.w3.org/2000/01/rdf-schema#label> \"ours\" }} }}"
                ),
            );
            registry::insert_version(
                &state.store,
                &state.base_url,
                &DataModelVersion {
                    data_model_id: "imbor".into(),
                    version: "2025".into(),
                    status: VersionStatus::Published,
                    graph_iri: ours.clone(),
                    sub_graphs: vec![ours.clone()],
                    created_at: "2025-07-01T00:00:00Z".into(),
                    created_by: creator.map(|u| format!("{}/users/{u}", state.base_url)),
                    derived_from: None,
                    notes: Some(match creator {
                        Some(_) => "IMBOR 2025 Vocabulaire (Stichting CROW)".into(),
                        None => "Seeded by bundle 'b'".into(),
                    }),
                    branch: None,
                    sub_graph_status: Vec::new(),
                },
            )
            .unwrap();
            registry::update_latest_published(&state.store, &state.base_url, "imbor", "2025")
                .unwrap();

            for _ in 0..2 {
                seed_standard_vocabularies(&state);
                seed_standard_vocabularies_with(&state, false);
            }
            assert!(
                registry::no_derivatives_attribution(&state.store, &state.base_url, "imbor")
                    .is_none()
            );
            assert_eq!(
                registry::list_versions(&state.store, &state.base_url, "imbor").len(),
                1,
                "no version added"
            );
            assert!(ask(
                &state,
                &format!("ASK {{ GRAPH <{ours}> {{ ?s ?p \"ours\" }} }}")
            ));
            assert_eq!(stored_triples(&state, "imbor", "2025").len(), 1);
            assert!(
                seeded_by(&state, &registry::data_model_iri(&state.base_url, "imbor")).is_none()
            );
            // The other vocabularies are seeded as usual.
            assert!(registry::version_exists(
                &state.store,
                &state.base_url,
                "rdf",
                "1.1"
            ));
        }
    }

    /// A graph named like a missing version that already holds someone's
    /// triples is never loaded over.
    #[test]
    fn a_missing_versions_graph_holding_other_triples_is_left_alone() {
        let state = fresh();
        let g = version_iri(&state, "skos", "2009-08-18");
        update(
            &state,
            &format!(
                "INSERT DATA {{ GRAPH <{g}> {{ <http://ex.org/a> <http://ex.org/b> \"c\" }} }}"
            ),
        );
        seed_standard_vocabularies(&state);
        assert!(!registry::version_exists(
            &state.store,
            &state.base_url,
            "skos",
            "2009-08-18"
        ));
        assert_eq!(stored_triples(&state, "skos", "2009-08-18").len(), 1);
    }

    /// A read-only store (a replica, a cluster member that does not lead)
    /// runs no seed and no check.
    #[test]
    fn a_read_only_store_is_left_to_the_leader() {
        use crate::store::replication::{Mode as RMode, ReplicationConfig, Scope};
        let store =
            TripleStore::in_memory()
                .unwrap()
                .with_replication(ReplicationConfig::follower(
                    "http://leader.internal:7878",
                    RMode::Hot,
                    Scope::All,
                ));
        let state = AppState::test_default_with_store(store);
        assert!(state.store.replication().read_only());
        assert_eq!(seed_standard_vocabularies(&state), 0);
        assert_eq!(seed_standard_vocabularies_with(&state, false), 0);
    }

    /// A licence record's compare and set loses to a concurrent writer: the
    /// label a PATCH or a direct write set is never overwritten with the
    /// seeder's older view.
    #[test]
    fn a_concurrent_possibly_modified_label_is_not_overwritten() {
        let state = fresh();
        seed_standard_vocabularies(&state);
        let iri = version_iri(&state, "rdf", "1.1");
        let snapshot = registry::attribution_json(&state.store, &iri);
        registry::mark_possibly_modified(&state.store, &iri, vf::edited_stored_copy).unwrap();
        let applied = registry::replace_attribution_if(
            &state.store,
            &iri,
            snapshot.as_deref(),
            vocab("rdf").versions[1].attribution(true).as_ref(),
        )
        .unwrap();
        assert!(!applied);
        assert!(!record(&state, "rdf", "1.1").unchanged);
    }

    #[test]
    fn reserved_ids_are_the_seeded_ones() {
        assert!(reserves_id("imbor") && reserves_id("rdf"));
        assert!(!reserves_id("imbor-otl") && !reserves_id("bag"));
    }
}
