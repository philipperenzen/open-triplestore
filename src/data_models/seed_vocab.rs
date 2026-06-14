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
//! bundled under `vocab/{id}/{version}.ttl` or reuse the current file's triples
//! (for standards that share one evolving namespace document — OWL, RDF, RDFS —
//! no version-frozen file exists, so the version record carries the accurate
//! label/date/spec URL while reusing the live vocabulary). OWL profiles
//! (EL/QL/RL/DL/Full) are NOT versions — they are profiles of the one OWL 2
//! language and are not seeded here.
//!
//! Idempotent at the **version** level: a version already present is skipped, so
//! the seeder never clobbers a user edit and backfills only newly-bundled
//! versions on upgrade (the latest-version labels match what shipped previously,
//! so existing entries gain their historical versions without duplication).
//! Best-effort — a failure on one entry is logged and skipped. Opt out with
//! `SEED_STANDARD_VOCABS=false`.
//!
//! Installs seeded before real versioning shipped have these vocabularies pinned
//! at the old synthetic `1.0.0`; [`migrate_synthetic_versions`] drops those
//! (system-owned only) so the seed loop recreates them at their real versions.

use crate::data_models::models::{DataModelVersion, VersionStatus};
use crate::data_models::{registry, upload};
use crate::kind_detector::{self, RegistryKind};
use crate::server::AppState;

const DESC: &str = "Bundled standard vocabulary, seeded as a public reference.";

/// The synthetic version every bundled vocabulary used to be seeded under, before
/// each got its real version(s). Used only by the one-time upgrade migration.
const LEGACY_SYNTHETIC_VERSION: &str = "1.0.0";

// ─── Bundled TTL sources (embedded at compile time) ─────────────────────────────
// Current/latest versions live in the flat files; historical snapshots that have a
// genuinely distinct downloadable document live under vocab/{id}/{version}.ttl.
const RDF_TTL: &str = include_str!("../../frontend/public/vocab/rdf.ttl");
const RDFS_TTL: &str = include_str!("../../frontend/public/vocab/rdfs.ttl");
const OWL_TTL: &str = include_str!("../../frontend/public/vocab/owl.ttl");
const XSD_TTL: &str = include_str!("../../frontend/public/vocab/xsd.ttl");
const SKOS_TTL: &str = include_str!("../../frontend/public/vocab/skos.ttl");
const DCAT_TTL: &str = include_str!("../../frontend/public/vocab/dcat.ttl");
const DCAT2_TTL: &str = include_str!("../../frontend/public/vocab/dcat/2.0.0.ttl");
const DCTERMS_TTL: &str = include_str!("../../frontend/public/vocab/dcterms.ttl");
const DCTERMS_2012_TTL: &str = include_str!("../../frontend/public/vocab/dcterms/2012.06.14.ttl");
const PROV_TTL: &str = include_str!("../../frontend/public/vocab/prov.ttl");
const FOAF_TTL: &str = include_str!("../../frontend/public/vocab/foaf.ttl");
const ORG_TTL: &str = include_str!("../../frontend/public/vocab/org.ttl");
const QB_TTL: &str = include_str!("../../frontend/public/vocab/qb.ttl");
const SCHEMA_TTL: &str = include_str!("../../frontend/public/vocab/schema.ttl");
const SHACL_TTL: &str = include_str!("../../frontend/public/vocab/shacl.ttl");
const TIME_TTL: &str = include_str!("../../frontend/public/vocab/time.ttl");
const VANN_TTL: &str = include_str!("../../frontend/public/vocab/vann.ttl");
const VOID_TTL: &str = include_str!("../../frontend/public/vocab/void.ttl");
const GEOSPARQL_TTL: &str = include_str!("../../frontend/public/vocab/geosparql.ttl");
const GEOSPARQL_10_TTL: &str = include_str!("../../frontend/public/vocab/geosparql/1.0.0.ttl");
const OTS_TTL: &str = include_str!("../../frontend/public/vocab/ots.ttl");

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
    /// Canonical spec URL for this version.
    #[allow(dead_code)]
    spec_url: &'static str,
    status: VersionStatus,
    /// Exactly one non-draft version per vocabulary is the canonical latest.
    latest: bool,
    /// Prior version label, for the `prov:wasDerivedFrom` chain.
    prior: Option<&'static str>,
    /// RDF to load into this version's graph. Several versions of the same vocab
    /// may share one file when no version-frozen document exists.
    ttl: &'static str,
}

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
                official_name: "RDF 1.0 (2004)",
                date: "2004-02-10",
                spec_url: "https://www.w3.org/TR/2004/REC-rdf-concepts-20040210/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                ttl: RDF_TTL,
            },
            StdVersion {
                version: "1.1",
                official_name: "RDF 1.1 (2014)",
                date: "2014-02-25",
                spec_url: "https://www.w3.org/TR/rdf11-concepts/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                ttl: RDF_TTL,
            },
            StdVersion {
                version: "1.2",
                official_name: "RDF 1.2 (Working Draft)",
                date: "2024-01-12",
                spec_url: "https://www.w3.org/TR/rdf12-concepts/",
                status: VersionStatus::Draft,
                latest: false,
                prior: Some("1.1"),
                ttl: RDF_TTL,
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
                official_name: "RDF Schema 1.0 (2004)",
                date: "2004-02-10",
                spec_url: "https://www.w3.org/TR/2004/REC-rdf-schema-20040210/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                ttl: RDFS_TTL,
            },
            StdVersion {
                version: "1.1",
                official_name: "RDF Schema 1.1 (2014)",
                date: "2014-02-25",
                spec_url: "https://www.w3.org/TR/rdf-schema/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                ttl: RDFS_TTL,
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
                official_name: "OWL Web Ontology Language (OWL 1, 2004)",
                date: "2004-02-10",
                spec_url: "https://www.w3.org/TR/2004/REC-owl-features-20040210/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                ttl: OWL_TTL,
            },
            StdVersion {
                version: "2.0",
                official_name: "OWL 2 (2009, 2nd ed. 2012)",
                date: "2012-12-11",
                spec_url: "https://www.w3.org/TR/owl2-overview/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                ttl: OWL_TTL,
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
                ttl: XSD_TTL,
            },
            StdVersion {
                version: "1.1",
                official_name: "XSD 1.1 Datatypes (2012)",
                date: "2012-04-05",
                spec_url: "https://www.w3.org/TR/xmlschema11-2/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                ttl: XSD_TTL,
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
            ttl: SKOS_TTL,
        }],
    },
    StdVocab {
        id: "dcterms",
        title: "DCMI Metadata Terms",
        namespace: "http://purl.org/dc/terms/",
        versions: &[
            StdVersion {
                version: "2008-01-14",
                official_name: "DCMI Metadata Terms (2008-01-14)",
                date: "2008-01-14",
                spec_url:
                    "https://www.dublincore.org/specifications/dublin-core/dcmi-terms/2008-01-14/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                ttl: DCTERMS_2012_TTL,
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
                ttl: DCTERMS_2012_TTL,
            },
            StdVersion {
                version: "2020-01-20",
                official_name: "DCMI Metadata Terms (2020-01-20)",
                date: "2020-01-20",
                spec_url:
                    "https://www.dublincore.org/specifications/dublin-core/dcmi-terms/2020-01-20/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("2012-06-14"),
                ttl: DCTERMS_TTL,
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
                official_name: "DCAT 1 (2014)",
                date: "2014-01-16",
                spec_url: "https://www.w3.org/TR/2014/REC-vocab-dcat-20140116/",
                status: VersionStatus::Published,
                latest: false,
                prior: None,
                ttl: DCAT_TTL,
            },
            StdVersion {
                version: "2.0",
                official_name: "DCAT 2 (2020)",
                date: "2020-02-04",
                spec_url: "https://www.w3.org/TR/vocab-dcat-2/",
                status: VersionStatus::Published,
                latest: false,
                prior: Some("1.0"),
                ttl: DCAT2_TTL,
            },
            StdVersion {
                version: "3.0",
                official_name: "DCAT 3 (2024)",
                date: "2024-08-22",
                spec_url: "https://www.w3.org/TR/vocab-dcat-3/",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("2.0"),
                ttl: DCAT_TTL,
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
            ttl: PROV_TTL,
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
            ttl: FOAF_TTL,
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
            ttl: ORG_TTL,
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
            ttl: QB_TTL,
        }],
    },
    StdVocab {
        id: "schema",
        title: "Schema.org",
        namespace: "https://schema.org/",
        versions: &[StdVersion {
            version: "29.0",
            official_name: "Schema.org 29.0",
            date: "2025-07-01",
            spec_url: "https://schema.org/version/29.0/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            ttl: SCHEMA_TTL,
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
            ttl: SHACL_TTL,
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
            ttl: TIME_TTL,
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
            ttl: VANN_TTL,
        }],
    },
    StdVocab {
        id: "void",
        title: "VoID",
        namespace: "http://rdfs.org/ns/void#",
        versions: &[StdVersion {
            version: "2011-03-06",
            official_name: "Describing Linked Datasets with VoID (2011)",
            date: "2011-03-06",
            spec_url: "https://www.w3.org/TR/void/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            ttl: VOID_TTL,
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
                ttl: GEOSPARQL_10_TTL,
            },
            StdVersion {
                version: "1.1",
                official_name: "OGC GeoSPARQL 1.1 (2024)",
                date: "2024-01-29",
                spec_url: "https://docs.ogc.org/is/22-047r1/22-047r1.html",
                status: VersionStatus::Published,
                latest: true,
                prior: Some("1.0"),
                ttl: GEOSPARQL_TTL,
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
            ttl: OTS_TTL,
        }],
    },
    // ── Standards & Ontologies set: the BIM / sensor / asset-management domain
    // packs the demo data draws on. Each is a curated term subset (header in the
    // .ttl cites the source); these render with full definitions in the UI and
    // are browsable/queryable as public reference models alongside the core
    // standards above. NB: the large ifcOWL and Brick schemas are intentionally
    // *not* bundled here (too large for an include_str! seed) — import them as a
    // user model if needed.
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
            ttl: include_str!("../../frontend/public/vocab/bot.ttl"),
        }],
    },
    StdVocab {
        id: "omg",
        title: "Ontology for Managing Geometry (OMG)",
        namespace: "https://w3id.org/omg#",
        versions: &[StdVersion {
            version: "0.0.1",
            official_name: "Ontology for Managing Geometry 0.0.1",
            date: "2019-01-01",
            spec_url: "https://w3id.org/omg",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            ttl: include_str!("../../frontend/public/vocab/omg.ttl"),
        }],
    },
    StdVocab {
        id: "fog",
        title: "File Ontology for Geometry formats (FOG)",
        namespace: "https://w3id.org/fog#",
        versions: &[StdVersion {
            version: "0.0.1",
            official_name: "File Ontology for Geometry formats 0.0.1",
            date: "2020-01-01",
            spec_url: "https://w3id.org/fog",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            ttl: include_str!("../../frontend/public/vocab/fog.ttl"),
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
            ttl: include_str!("../../frontend/public/vocab/sosa.ttl"),
        }],
    },
    StdVocab {
        id: "bag",
        title: "3DBAG Vocabulary",
        namespace: "https://data.3dbag.nl/def/",
        versions: &[StdVersion {
            version: "1.0",
            official_name: "3DBAG Vocabulary (excerpt)",
            date: "2024-01-01",
            spec_url: "https://docs.3dbag.nl/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            ttl: include_str!("../../frontend/public/vocab/bag.ttl"),
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
            ttl: include_str!("../../frontend/public/vocab/saref.ttl"),
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
            ttl: include_str!("../../frontend/public/vocab/ssn.ttl"),
        }],
    },
    StdVocab {
        id: "otl",
        title: "RWS Object Type Library (OTL) — excerpt",
        namespace: "https://data.rws.nl/otl/def/",
        versions: &[StdVersion {
            version: "excerpt",
            official_name: "RWS Object Type Library (excerpt)",
            date: "2024-01-01",
            spec_url: "https://otl.rws.nl/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            ttl: include_str!("../../frontend/public/vocab/otl.ttl"),
        }],
    },
    StdVocab {
        id: "imbor",
        title: "CROW IMBOR — excerpt",
        namespace: "https://data.crow.nl/imbor/def/",
        versions: &[StdVersion {
            version: "excerpt",
            official_name: "CROW IMBOR (excerpt)",
            date: "2024-01-01",
            spec_url: "https://imbor.crow.nl/",
            status: VersionStatus::Published,
            latest: true,
            prior: None,
            ttl: include_str!("../../frontend/public/vocab/imbor.ttl"),
        }],
    },
];

/// Seed every standard vocabulary version that isn't already in the registry.
/// Returns the number of newly-seeded *versions* (0 once everything is present —
/// the idempotent steady state).
pub fn seed_standard_vocabularies(state: &AppState) -> usize {
    let disabled = std::env::var("SEED_STANDARD_VOCABS")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "false" | "0" | "no" | "off"
            )
        })
        .unwrap_or(false);
    if disabled {
        return 0;
    }
    // Upgrade installs seeded under the old synthetic 1.0.0 before the loop runs,
    // so the loop recreates them at their real versions.
    migrate_synthetic_versions(state);
    let mut seeded = 0usize;
    for v in VOCABS {
        match seed_one(state, v) {
            Ok(n) => seeded += n,
            Err(e) => tracing::warn!("vocabulary seed '{}' skipped: {e}", v.id),
        }
    }
    if seeded > 0 {
        tracing::info!("Seeded {seeded} standard vocabulary version(s) into the model registry");
    }
    seeded
}

/// One-time upgrade for installs seeded before real versioning: a bundled
/// vocabulary whose *only* version is the old synthetic `1.0.0` — and which is
/// still system-owned, i.e. untouched by an admin — is dropped (registry records
/// + its named graphs) so the seed loop recreates it under its real version(s).
///
/// Strictly scoped: it never touches a vocabulary an admin has claimed
/// (`owner_id`/`created_by` set) or versioned beyond the single synthetic entry,
/// nor any user-created model. A vocabulary already at its real version(s) has no
/// `1.0.0` entry, so this no-ops on every later boot. Returns the number upgraded.
fn migrate_synthetic_versions(state: &AppState) -> usize {
    let mut upgraded = 0usize;
    for v in VOCABS {
        // A vocab that legitimately uses the "1.0.0" label would be
        // indistinguishable from the synthetic placeholder — none currently do,
        // but guard anyway.
        if v.versions
            .iter()
            .any(|ver| ver.version == LEGACY_SYNTHETIC_VERSION)
        {
            continue;
        }
        let Some(record) = registry::get_data_model(&state.store, &state.base_url, v.id) else {
            continue; // not seeded yet — the loop creates it correctly
        };
        // Only adopt system-owned entries (seeded with no owner / creator). An
        // admin who claimed or re-versioned the vocab keeps their copy untouched.
        let system_owned = record.owner_id.is_none() && record.created_by.is_none();
        let versions = registry::list_versions(&state.store, &state.base_url, v.id);
        let only_synthetic = versions.len() == 1 && versions[0].version == LEGACY_SYNTHETIC_VERSION;
        if !(system_owned && only_synthetic) {
            continue;
        }
        // Drop the old version's named graphs, then its registry records.
        let mut graphs: Vec<String> = Vec::new();
        for ver in &versions {
            graphs.push(ver.graph_iri.clone());
            graphs.extend(ver.sub_graphs.iter().cloned());
        }
        let refs: Vec<&str> = graphs.iter().map(|s| s.as_str()).collect();
        if let Err(e) = state.store.bulk_delete_graphs(&refs) {
            tracing::warn!("vocab upgrade '{}': graph cleanup failed: {e}", v.id);
            continue;
        }
        if let Err(e) = registry::delete_data_model(&state.store, &state.base_url, v.id) {
            tracing::warn!("vocab upgrade '{}': registry cleanup failed: {e}", v.id);
            continue;
        }
        upgraded += 1;
    }
    if upgraded > 0 {
        tracing::info!(
            "Upgraded {upgraded} bundled vocabularies from the synthetic 1.0.0 to their real versions"
        );
    }
    upgraded
}

/// Seed one vocabulary entry and all of its missing versions. Returns the number
/// of versions newly created this call.
fn seed_one(state: &AppState, v: &StdVocab) -> anyhow::Result<usize> {
    let now = chrono::Utc::now().to_rfc3339();

    // Ensure the registry entry (header) exists, without short-circuiting the
    // whole vocab — a previously single-version entry still needs its newly
    // bundled historical/future versions backfilled below.
    if !registry::data_model_exists(&state.store, &state.base_url, v.id) {
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
            &now,
        )?;
    }

    let mut newly = 0usize;
    let mut kind: Option<RegistryKind> = None;
    let latest = v
        .versions
        .iter()
        .find(|ver| ver.latest)
        .map(|ver| ver.version);

    for ver in v.versions {
        if registry::version_exists(&state.store, &state.base_url, v.id, ver.version) {
            continue;
        }
        let quads = match upload::parse_rdf(ver.ttl.as_bytes(), "text/turtle", "vocab.ttl") {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!(
                    "vocab '{}' version '{}' parse failed: {e}",
                    v.id,
                    ver.version
                );
                continue;
            }
        };
        // The registry stores one kind per entry; derive it from the canonical
        // latest version's content (most representative).
        if ver.latest {
            kind = kind_detector::detect(&quads).primary;
        }

        let result = upload::load_parsed(
            &state.store,
            &state.base_url,
            v.id,
            Some(ver.version),
            quads,
            true, // merge into a single graph per version
        )
        .map_err(|e| anyhow::anyhow!("load {} {}: {e}", v.id, ver.version))?;

        let graph_iri = format!(
            "{}/data-model/{}/version/{}",
            state.base_url, v.id, result.version
        );
        let derived_from = ver
            .prior
            .map(|p| format!("{}/data-model/{}/version/{}", state.base_url, v.id, p));
        let record = DataModelVersion {
            data_model_id: v.id.to_string(),
            version: result.version.clone(),
            status: ver.status,
            graph_iri,
            sub_graphs: result.sub_graphs,
            created_at: format!("{}T00:00:00Z", ver.date),
            created_by: None,
            derived_from,
            notes: Some(ver.official_name.to_string()),
            branch: None,
            sub_graph_status: Vec::new(),
        };
        registry::insert_version(&state.store, &state.base_url, &record)?;
        newly += 1;
    }

    // Only touch the per-entry kind + latest pointer when we actually created a
    // version this run, so steady-state boots stay cheap (no re-parse). The kind
    // is computed only when the latest version itself was (re)loaded.
    if newly > 0 {
        if let Some(k) = kind {
            registry::set_data_model_kind(&state.store, &state.base_url, v.id, k)?;
        }
        if let Some(lv) = latest {
            registry::update_latest_published(&state.store, &state.base_url, v.id, lv)?;
        }
    }
    Ok(newly)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::AppState;
    use crate::store::TripleStore;

    fn total_versions() -> usize {
        VOCABS.iter().map(|v| v.versions.len()).sum()
    }

    /// Every bundled vocabulary is published at its curated latest version (never
    /// a draft, never the synthetic `1.0.0`), and OWL/RDF/DCAT ship multiple
    /// versions out of the box.
    #[test]
    fn seeds_every_vocabulary_with_all_versions() {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());
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
            assert_ne!(
                rec.latest_published.as_deref(),
                Some(LEGACY_SYNTHETIC_VERSION),
                "vocabulary '{}' must not use the synthetic placeholder",
                v.id
            );
        }

        // Multi-version standards ship all of their versions.
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
        // OWL latest is 2.0 (not the 1.2 draft of RDF either).
        let owl = registry::get_data_model(&state.store, &state.base_url, "owl").unwrap();
        assert_eq!(owl.latest_published.as_deref(), Some("2.0"));
        let rdf = registry::get_data_model(&state.store, &state.base_url, "rdf").unwrap();
        assert_eq!(
            rdf.latest_published.as_deref(),
            Some("1.1"),
            "a draft (RDF 1.2) must never be the latest published version"
        );

        // Idempotent: a second pass seeds nothing new.
        assert_eq!(seed_standard_vocabularies(&state), 0);
    }

    /// An install seeded under the old synthetic `1.0.0` (system-owned) is dropped
    /// and recreated at the vocabulary's real version(s) on the next seed.
    #[test]
    fn upgrades_a_system_vocab_pinned_at_the_synthetic_version() {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());

        // Recreate an "old install": seed `owl` at the synthetic 1.0.0, system-owned.
        let owl = VOCABS.iter().find(|v| v.id == "owl").unwrap();
        let ttl = owl.versions[0].ttl;
        let quads = upload::parse_rdf(ttl.as_bytes(), "text/turtle", "owl.ttl").unwrap();
        registry::insert_data_model(
            &state.store,
            &state.base_url,
            owl.id,
            owl.title,
            owl.namespace,
            None,
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
        let graph_iri = format!(
            "{}/data-model/{}/version/{}",
            state.base_url, owl.id, result.version
        );
        registry::insert_version(
            &state.store,
            &state.base_url,
            &DataModelVersion {
                data_model_id: owl.id.to_string(),
                version: result.version.clone(),
                status: VersionStatus::Published,
                graph_iri,
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

        // Seeding migrates it to the real versions and drops the synthetic one.
        seed_standard_vocabularies(&state);
        let owl_rec = registry::get_data_model(&state.store, &state.base_url, "owl").unwrap();
        assert_eq!(
            owl_rec.latest_published.as_deref(),
            Some("2.0"),
            "upgraded to its real latest version"
        );
        assert!(
            !registry::version_exists(
                &state.store,
                &state.base_url,
                "owl",
                LEGACY_SYNTHETIC_VERSION
            ),
            "synthetic 1.0.0 version removed"
        );
        assert!(
            registry::version_exists(&state.store, &state.base_url, "owl", "1.0"),
            "real OWL 1.0 version seeded"
        );
    }
}
