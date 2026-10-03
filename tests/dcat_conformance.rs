//! DCAT 3 catalogue conformance tests.
//!
//! Exercises the dataset catalogue (`dcat::generate_dcat_catalog`,
//! `dcat::build_catalog_report` with explicit `CatalogOptions`, so no process
//! environment is involved) and the model registry's catalogue, and checks the
//! emitted RDF against DCAT 3: catalogue / dataset / distribution / data
//! service structure, versions per DCAT 3 §11, range typing, temporal
//! coverage and update frequency, catalogue records under the application
//! profiles, VoID statistics, and access control.

use std::sync::Arc;

use open_triplestore::auth::db::AuthDb;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::dcat::catalog::{build_catalog_report, check_coverage, serialize_catalog};
use open_triplestore::dcat::{generate_dcat_catalog, CatalogOptions, Profile};
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;

const DCAT: &str = "http://www.w3.org/ns/dcat#";
const DCT: &str = "http://purl.org/dc/terms/";

fn setup() -> (TripleStore, Arc<AuthDb>) {
    let store = TripleStore::in_memory().unwrap();
    let db = Arc::new(AuthDb::in_memory().unwrap());
    db.create_organisation("o1", "Acme Data", "acme", None, None)
        .unwrap();
    db.create_dataset(
        "d1",
        "Census 2020",
        Some("National population census"),
        OwnerType::Organisation,
        "o1",
        Visibility::Public,
        None,
    )
    .unwrap();
    (store, db)
}

/// Parse the generated Turtle into a queryable store.
fn parse(ttl: &str) -> TripleStore {
    let s = TripleStore::in_memory().unwrap();
    s.load_str(ttl, RdfFormat::Turtle, None)
        .unwrap_or_else(|e| panic!("DCAT output is not valid Turtle: {e}\n---\n{ttl}"));
    s
}

fn ask(s: &TripleStore, q: &str) -> bool {
    matches!(s.query(q), Ok(QueryResults::Boolean(true)))
}

fn count(s: &TripleStore, q: &str) -> usize {
    match s.query(q).unwrap() {
        QueryResults::Solutions(sols) => sols.count(),
        _ => 0,
    }
}

// The catalogue is a well-formed DCAT 2 graph: a dcat:Catalog containing a
// dcat:Dataset with a Dublin Core title.
#[test]
fn dcat_catalog_and_dataset() {
    let (store, db) = setup();
    let ttl = generate_dcat_catalog("http://localhost:7878", &store, &db, None);
    let s = parse(&ttl);
    assert!(
        ask(&s, &format!("ASK {{ ?c a <{DCAT}Catalog> }}")),
        "must declare a dcat:Catalog"
    );
    assert!(
        ask(&s, &format!("ASK {{ ?d a <{DCAT}Dataset> }}")),
        "must declare a dcat:Dataset"
    );
    assert!(
        ask(
            &s,
            &format!("ASK {{ ?c a <{DCAT}Catalog> ; <{DCT}title> ?t }}")
        ),
        "catalog has a dct:title"
    );
    assert!(
        ttl.contains("Census 2020"),
        "dataset title appears in the catalog"
    );
}

// The catalog links its datasets via dcat:dataset, and each dataset exposes at
// least one dcat:Distribution (e.g. the SPARQL endpoint).
#[test]
fn dcat_dataset_membership_and_distribution() {
    let (store, db) = setup();
    let ttl = generate_dcat_catalog("http://localhost:7878", &store, &db, None);
    let s = parse(&ttl);
    assert!(
        ask(
            &s,
            &format!("ASK {{ ?c a <{DCAT}Catalog> ; <{DCAT}dataset> ?d . ?d a <{DCAT}Dataset> }}")
        ),
        "catalog dcat:dataset links to a dcat:Dataset"
    );
    assert!(
        count(
            &s,
            &format!("SELECT ?dist WHERE {{ ?d <{DCAT}distribution> ?dist }}")
        ) >= 1,
        "dataset has at least one dcat:Distribution"
    );
}

// Datasets carry VoID statistics (triple counts etc.) for the contained data.
#[test]
fn dcat_void_statistics() {
    let (store, db) = setup();
    // Put some data into the dataset's graph so VoID stats are non-trivial.
    store
        .update(
            "INSERT DATA { GRAPH <urn:dataset:d1> { <http://ex/s> <http://ex/p> <http://ex/o> } }",
        )
        .unwrap();
    let ttl = generate_dcat_catalog("http://localhost:7878", &store, &db, None);
    let s = parse(&ttl);
    // A void:triples statistic is emitted. (This used to be
    // `ttl.contains("void") || ask(...)`, and the catalog always begins with
    // `@prefix void:`, so the left disjunct was unconditionally true and the
    // assertion could not fail even with every statistic removed.)
    assert!(
        ask(&s, "ASK { ?d <http://rdfs.org/ns/void#triples> ?n }"),
        "catalog includes a void:triples statistic:\n{ttl}"
    );
}

// Access control: an unauthenticated catalog request lists only PUBLIC datasets;
// a private dataset is excluded.
#[test]
fn dcat_access_control_hides_private() {
    let (store, db) = setup();
    db.create_dataset(
        "secret",
        "Secret Dataset",
        None,
        OwnerType::Organisation,
        "o1",
        Visibility::Private,
        None,
    )
    .unwrap();
    let anon = generate_dcat_catalog("http://localhost:7878", &store, &db, None);
    assert!(anon.contains("Census 2020"), "public dataset is listed");
    assert!(
        !anon.contains("Secret Dataset"),
        "private dataset must NOT appear in an unauthenticated catalog"
    );
}

// ── DCAT 3 semantics ─────────────────────────────────────────────────────────

const BASE: &str = "http://localhost:7878";
const ADMS: &str = "http://www.w3.org/ns/adms#";
const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
const EU: &str = "http://publications.europa.eu/resource/authority/";

/// The catalogue under `profile`, built with explicit options (no process
/// environment), parsed back, with its profile warnings.
fn catalogue(
    store: &TripleStore,
    db: &Arc<AuthDb>,
    opts: &CatalogOptions,
    user: Option<&str>,
) -> (TripleStore, String, Vec<String>) {
    let report = build_catalog_report(opts, store, db, user, None);
    let ttl =
        String::from_utf8(serialize_catalog(&report.triples, RdfFormat::Turtle).unwrap()).unwrap();
    (parse(&ttl), ttl, report.warnings)
}

fn version(
    ds: &str,
    v: &str,
    status: open_triplestore::dataset_versions::models::VersionStatus,
    created: &str,
    derived_from: Option<&str>,
    notes: Option<&str>,
) -> open_triplestore::dataset_versions::models::DatasetVersion {
    open_triplestore::dataset_versions::models::DatasetVersion {
        dataset_id: ds.into(),
        version: v.into(),
        status,
        graph_iri: format!("{BASE}/dataset/{ds}/version/{v}"),
        snapshot_graphs: vec![],
        source_map: vec![],
        created_at: created.into(),
        created_by: None,
        derived_from: derived_from.map(str::to_string),
        notes: notes.map(str::to_string),
        branch: None,
        conforms_to_model: Some("census-model".into()),
        conforms_to_version: Some("1.0.0".into()),
    }
}

// DCAT 3 §11: a released version is a dcat:Dataset of its own, linked from the
// dataset by dcat:hasVersion and back by dcat:isVersionOf, labelled by
// dcat:version, chained by dcat:previousVersion; the newest is the
// dcat:hasCurrentVersion. A draft is not a release and is not catalogued, and
// the live dataset does not borrow a version's label.
#[test]
fn dcat3_versions_are_datasets_of_their_own() {
    use open_triplestore::dataset_versions::models::VersionStatus::*;
    use open_triplestore::dataset_versions::registry::insert_version;
    let (store, db) = setup();
    for v in [
        version(
            "d1",
            "1.0.0",
            Published,
            "2026-01-01T00:00:00Z",
            None,
            Some("First release"),
        ),
        version(
            "d1",
            "1.1.0",
            Published,
            "2026-02-01T00:00:00Z",
            Some("1.0.0"),
            None,
        ),
        version(
            "d1",
            "2.0.0-rc.1",
            Draft,
            "2026-03-01T00:00:00Z",
            Some("1.1.0"),
            None,
        ),
    ] {
        insert_version(&store, BASE, &v).unwrap();
    }
    let (s, ttl, _) = catalogue(&store, &db, &CatalogOptions::new(BASE, Profile::Dcat), None);
    let d = format!("<{BASE}/dataset/d1>");
    let v1 = format!("<{BASE}/dataset/d1/version/1.0.0>");
    let v2 = format!("<{BASE}/dataset/d1/version/1.1.0>");
    for v in [&v1, &v2] {
        assert!(
            ask(&s, &format!("ASK {{ {d} <{DCAT}hasVersion> {v} . {v} a <{DCAT}Dataset> ; <{DCAT}isVersionOf> {d} ; <{DCT}issued> ?i ; <{DCT}title> ?t ; <{DCAT}distribution> ?x }}")),
            "{v} is a described version:\n{ttl}"
        );
    }
    assert!(ask(&s, &format!("ASK {{ {v1} <{DCAT}version> \"1.0.0\" ; <{ADMS}versionNotes> ?n ; <{ADMS}status> <{EU}dataset-status/COMPLETED> }}")), "{ttl}");
    assert!(
        ask(&s, &format!("ASK {{ {v2} <{DCAT}previousVersion> {v1} }}")),
        "{ttl}"
    );
    assert!(
        ask(&s, &format!("ASK {{ {d} <{DCAT}hasCurrentVersion> {v2} }}")),
        "{ttl}"
    );
    assert!(
        ask(
            &s,
            &format!(
                "ASK {{ {v1} <{DCT}conformsTo> <{BASE}/data-model/census-model/version/1.0.0> }}"
            )
        ),
        "a version keeps the model version it was cut against:\n{ttl}"
    );
    assert!(
        !ttl.contains("2.0.0-rc.1"),
        "a draft is not catalogued:\n{ttl}"
    );
    assert!(
        !ask(&s, &format!("ASK {{ {d} <{DCAT}version> ?v }}")),
        "the live dataset carries no version label:\n{ttl}"
    );
    assert!(
        !ttl.contains("1%2E0"),
        "version IRIs are the registry's, not re-encoded:\n{ttl}"
    );
}

// The SPARQL endpoint is a dcat:DataService that says which datasets it
// serves, and its endpoint description is the SPARQL service description
// (which `GET /sparql` returns), not the instance homepage.
#[test]
fn dcat3_data_service_serves_its_datasets() {
    let (store, db) = setup();
    let mut opts = CatalogOptions::new(BASE, Profile::Dcat);
    opts.contact_name = Some("Data desk".into());
    opts.contact_email = Some("data@example.org".into());
    let (s, ttl, _) = catalogue(&store, &db, &opts, None);
    let svc = format!("<{BASE}/sparql>");
    assert!(ask(&s, &format!("ASK {{ <{BASE}/catalog> <{DCAT}service> {svc} . {svc} a <{DCAT}DataService> ; <{DCAT}servesDataset> <{BASE}/dataset/d1> , <{BASE}/dataset> ; <{DCAT}endpointURL> {svc} ; <{DCAT}endpointDescription> {svc} ; <{DCT}publisher> ?p ; <{DCT}accessRights> ?a }}")), "{ttl}");
    assert!(
        !ask(
            &s,
            &format!("ASK {{ {svc} <{DCAT}endpointDescription> <{BASE}/> }}")
        ),
        "{ttl}"
    );
    for subject in [format!("<{BASE}/catalog>"), svc.clone()] {
        assert!(
            ask(&s, &format!("ASK {{ {subject} <{DCAT}contactPoint> ?c . ?c <http://www.w3.org/2006/vcard/ns#fn> \"Data desk\" ; <http://www.w3.org/2006/vcard/ns#hasEmail> <mailto:data@example.org> }}")),
            "{subject} has the configured contact point:\n{ttl}"
        );
    }
}

// Objects are typed with the classes the DCAT ranges name, and a theme is a
// labelled skos:Concept.
#[test]
fn dcat3_objects_carry_their_range_classes() {
    let (store, db) = setup();
    db.update_dataset_metadata(
        "d1",
        Some("http://creativecommons.org/licenses/by/4.0/"),
        Some(&format!("[\"{EU}data-theme/SOCI\"]")),
        None,
        None,
        None,
        None,
        Some("completed"),
        None,
        None,
        None,
    )
    .unwrap();
    let (s, ttl, _) = catalogue(
        &store,
        &db,
        &CatalogOptions::new(BASE, Profile::DcatAp),
        None,
    );
    let d = format!("<{BASE}/dataset/d1>");
    for (pattern, what) in [
        (format!("{d} <{DCT}license> ?o . ?o a <{DCT}LicenseDocument>"), "licence"),
        (format!("{d} <{DCT}accessRights> ?o . ?o a <{DCT}RightsStatement>"), "access rights"),
        (format!("{d} <{DCT}language> ?o . ?o a <{DCT}LinguisticSystem>"), "language"),
        (format!("{d} <{DCAT}landingPage> ?o . ?o a <http://xmlns.com/foaf/0.1/Document>"), "landing page"),
        (format!("{d} <{DCAT}theme> ?o . ?o a <{SKOS}Concept> ; <{SKOS}prefLabel> \"Population and society\"@en"), "theme"),
        (format!("{d} <{ADMS}status> ?o . ?o a <{SKOS}Concept> ; <{SKOS}prefLabel> ?l"), "status"),
        (format!("{d} <{DCAT}distribution> ?x . ?x <{DCAT}mediaType> ?o . ?o a <{DCT}MediaType>"), "media type"),
        (format!("{d} <{DCAT}distribution> ?x . ?x <{DCT}format> ?o . ?o a <{DCT}MediaTypeOrExtent>"), "file type"),
        (format!("{d} <{DCAT}distribution> ?x . ?x <{DCT}conformsTo> ?o . ?o a <{DCT}Standard>"), "standard"),
        (format!("<{BASE}/catalog> <{DCAT}themeTaxonomy> ?o . ?o a <{SKOS}ConceptScheme> ; <{DCT}title> ?t"), "theme taxonomy"),
    ] {
        assert!(ask(&s, &format!("ASK {{ {pattern} }}")), "{what} is typed:\n{ttl}");
    }
    assert!(
        !ask(
            &s,
            &format!("ASK {{ ?x <{DCAT}mediaType> ?m . FILTER(isLiteral(?m)) }}")
        ),
        "{ttl}"
    );
}

// DCAT-AP 3 recommends dct:temporal and dct:accrualPeriodicity; both come
// from the dataset's own fields, checked and normalised on the way in.
#[test]
fn dcat3_temporal_coverage_and_update_frequency() {
    let (store, db) = setup();
    let c = check_coverage(Some("2020-01-01"), Some("2020-12-31"), Some("annual")).unwrap();
    assert_eq!(
        c.accrual_periodicity.as_deref(),
        Some("http://publications.europa.eu/resource/authority/frequency/ANNUAL")
    );
    db.update_dataset_coverage(
        "d1",
        c.temporal_start.as_deref(),
        c.temporal_end.as_deref(),
        c.accrual_periodicity.as_deref(),
    )
    .unwrap();
    let (s, ttl, _) = catalogue(&store, &db, &CatalogOptions::new(BASE, Profile::Dcat), None);
    assert!(ask(&s, &format!("ASK {{ <{BASE}/dataset/d1> <{DCT}temporal> ?t . ?t a <{DCT}PeriodOfTime> ; <{DCAT}startDate> \"2020-01-01\"^^<http://www.w3.org/2001/XMLSchema#date> ; <{DCAT}endDate> \"2020-12-31\"^^<http://www.w3.org/2001/XMLSchema#date> }}")), "{ttl}");
    assert!(ask(&s, &format!("ASK {{ <{BASE}/dataset/d1> <{DCT}accrualPeriodicity> <{EU}frequency/ANNUAL> . <{EU}frequency/ANNUAL> a <{DCT}Frequency> }}")), "{ttl}");

    // Malformed input is refused, not stored.
    assert!(check_coverage(Some("2020-13-01"), None, None).is_err());
    assert!(check_coverage(Some("2021-01-01"), Some("2020-01-01"), None).is_err());
    assert!(check_coverage(None, None, Some("every full moon")).is_err());
    assert!(check_coverage(None, None, Some("javascript:alert(1)")).is_err());
}

// Under the application profiles every dataset gets a dcat:CatalogRecord
// (with the language DCAT-AP-NL asks for), an organisation-owned dataset's
// creator is its publisher, a dataset without a contact point is answered for
// by its organisation, and a missing theme is reported, not invented.
#[test]
fn dcat_ap_records_creators_contacts_and_theme_warnings() {
    let (store, db) = setup();
    db.update_organisation(
        "o1",
        "Acme Data",
        None,
        None,
        None,
        Some("Acme data desk"),
        Some("desk@acme.example"),
        None,
        Some("FormalOrganization"),
        None,
    )
    .unwrap();
    let opts = CatalogOptions::new(BASE, Profile::DcatApNl);
    let (s, ttl, warnings) = catalogue(&store, &db, &opts, None);
    let d = format!("<{BASE}/dataset/d1>");
    assert!(ask(&s, &format!("ASK {{ <{BASE}/catalog> <{DCAT}record> ?r . ?r a <{DCAT}CatalogRecord> ; <http://xmlns.com/foaf/0.1/primaryTopic> {d} ; <{DCT}modified> ?m ; <{DCT}language> <{EU}language/NLD> ; <{DCT}conformsTo> ?profile }}")), "{ttl}");
    assert!(
        ask(
            &s,
            &format!(
                "ASK {{ {d} <{DCT}creator> <{BASE}/org/o1> ; <{DCT}publisher> <{BASE}/org/o1> }}"
            )
        ),
        "{ttl}"
    );
    assert!(ask(&s, &format!("ASK {{ {d} <{DCAT}contactPoint> ?c . ?c <http://www.w3.org/2006/vcard/ns#fn> \"Acme data desk\" }}")), "{ttl}");
    assert!(
        !ask(&s, &format!("ASK {{ {d} <{DCAT}theme> ?t }}")),
        "no theme is invented:\n{ttl}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("`d1`") && w.contains("dcat:theme")),
        "the missing theme is a profile warning: {warnings:?}"
    );
    // Plain DCAT has no records.
    let (_, plain, _) = catalogue(&store, &db, &CatalogOptions::new(BASE, Profile::Dcat), None);
    assert!(!plain.contains("CatalogRecord"), "{plain}");
}

// An unknown profile is an error (startup refuses it), not silently `dcat`.
#[test]
fn an_unknown_profile_is_refused() {
    assert_eq!(Profile::parse("dcat-ap-nl").unwrap(), Profile::DcatApNl);
    assert_eq!(Profile::parse("").unwrap(), Profile::Dcat);
    assert!(Profile::parse("dcat-ap-be").is_err());
}

// The governance metadata graph speaks the same DCAT 3: a status code becomes
// its EU dataset-status IRI instead of a relative IRI that made the whole
// Turtle document fail to load, and coverage is described.
#[test]
fn the_dataset_metadata_graph_loads_with_codes_and_coverage() {
    use open_triplestore::auth::dataset_graph::build_dataset_metadata_ttl;
    let (_store, db) = setup();
    db.update_dataset_metadata(
        "d1",
        None,
        None,
        Some("[\"census\"]"),
        None,
        None,
        None,
        Some("completed"),
        None,
        None,
        None,
    )
    .unwrap();
    db.update_dataset_coverage("d1", Some("2020-01-01"), None, None)
        .unwrap();
    let ds = db.get_dataset("d1").unwrap().unwrap();
    let ttl = build_dataset_metadata_ttl(BASE, &ds, &[]);
    let s = parse(&ttl);
    assert!(ask(&s, &format!("ASK {{ <{BASE}/dataset/d1> <{ADMS}status> <{EU}dataset-status/COMPLETED> ; <{DCT}identifier> \"d1\" ; <https://opentriplestore.org/ns#visibility> \"public\" ; <{DCT}temporal> ?t }}")), "{ttl}");
}

// The model registry's catalogue: versions are resources described per DCAT
// 3 §11 (not a literal dcat:hasVersion), media types are IANA IRIs typed
// dct:MediaType (not literals), and only serialisations the data endpoint
// actually serves are offered.
#[test]
fn the_registry_catalogue_is_dcat3() {
    use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
    use open_triplestore::data_models::registry as dmr;
    let (store, db) = setup();
    dmr::insert_data_model(
        &store,
        BASE,
        "census-model",
        "Census \"model\"",
        "http://example.org/census#",
        Some("Classes of the census."),
        true,
        Some("organisation"),
        Some("o1"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    for (v, at, from) in [
        ("1.0.0", "2026-01-02T00:00:00Z", None),
        ("1.1.0", "2026-02-02T00:00:00Z", Some("1.0.0")),
    ] {
        dmr::insert_version(
            &store,
            BASE,
            &DataModelVersion {
                data_model_id: "census-model".into(),
                version: v.into(),
                status: VersionStatus::Published,
                graph_iri: format!("{BASE}/data-model/census-model/version/{v}"),
                sub_graphs: vec![],
                created_at: at.into(),
                created_by: None,
                derived_from: from.map(str::to_string),
                notes: None,
                branch: None,
                sub_graph_status: vec![],
            },
        )
        .unwrap();
    }
    dmr::update_latest_published(&store, BASE, "census-model", "1.1.0").unwrap();
    let opts = CatalogOptions::new(BASE, Profile::Dcat);
    let triples =
        open_triplestore::catalog::builder::build_registry_catalog(&opts, &store, &db, None);
    let ttl = String::from_utf8(serialize_catalog(&triples, RdfFormat::Turtle).unwrap()).unwrap();
    let s = parse(&ttl);
    let m = format!("<{BASE}/data-model/census-model>");
    let v1 = format!("<{BASE}/data-model/census-model/version/1.0.0>");
    let v2 = format!("<{BASE}/data-model/census-model/version/1.1.0>");
    assert!(ask(&s, &format!("ASK {{ <{BASE}/catalog/registry> a <{DCAT}Catalog> ; <{DCAT}dataset> {m} . {m} <{DCT}title> \"Census \\\"model\\\"\"@en ; <{DCAT}hasVersion> {v1}, {v2} ; <{DCAT}hasCurrentVersion> {v2} ; <{DCT}publisher> <{BASE}/org/o1> ; <http://xmlns.com/foaf/0.1/page> <http://example.org/census#> }}")), "{ttl}");
    assert!(ask(&s, &format!("ASK {{ {v2} a <{DCAT}Dataset> ; <{DCAT}isVersionOf> {m} ; <{DCAT}version> \"1.1.0\" ; <{DCAT}previousVersion> {v1} ; <{DCAT}distribution> ?d . ?d <{DCAT}mediaType> <https://www.iana.org/assignments/media-types/text/turtle> . <https://www.iana.org/assignments/media-types/text/turtle> a <{DCT}MediaType> }}")), "{ttl}");
    assert!(
        !ask(
            &s,
            &format!("ASK {{ ?x <{DCAT}hasVersion> ?v . FILTER(isLiteral(?v)) }}")
        ),
        "{ttl}"
    );
    assert!(
        !ttl.contains("rdf+xml") && !ttl.contains("ld+json"),
        "the data endpoint serves neither RDF/XML nor JSON-LD, so neither is offered:\n{ttl}"
    );
}
