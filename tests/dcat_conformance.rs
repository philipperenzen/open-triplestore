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

/// The whole-instance catalogue as Turtle.
fn generate_dcat_catalog(
    base_url: &str,
    store: &TripleStore,
    auth_db: &Arc<AuthDb>,
    user_id: Option<&str>,
) -> String {
    let bytes = generate_catalog_bytes(base_url, store, auth_db, user_id, None, RdfFormat::Turtle)
        .expect("catalogue serialises");
    String::from_utf8(bytes).expect("Turtle is UTF-8")
}

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

// ── Official DCAT-AP 3.0.1 / DCAT-AP-NL 3 shapes, and VoID ───────────────────

const SEMIC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/semic-dcat-ap-3.0.1"
);
const GEONOVUM: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/geonovum-dcat-ap-nl-3"
);
const VOID: &str = "http://rdfs.org/ns/void#";

/// A registry that exercises every branch of the catalogue generator: an
/// organisation-, a user- and a group-owned dataset; geometry (so the OGC API
/// is a data service); an LDES stream; released and draft versions; a
/// linkset-role graph; a private graph; temporal coverage, frequency,
/// keywords, spatial coverage; a dataset without a licence of its own (the
/// catalogue's applies) and one without a contact point (its organisation's
/// or the catalogue's applies).
fn rich_fixture() -> (TripleStore, Arc<AuthDb>) {
    use open_triplestore::auth::models::{GraphKind, SystemRole};
    use open_triplestore::dataset_versions::models::VersionStatus::*;
    use open_triplestore::dataset_versions::registry::insert_version;
    let store = TripleStore::in_memory().unwrap();
    let db = Arc::new(AuthDb::in_memory().unwrap());
    db.create_user("u1", "ada", "ada@example.org", "h", SystemRole::User)
        .unwrap();
    db.create_user(
        "root",
        "root",
        "root@example.org",
        "h",
        SystemRole::SuperAdmin,
    )
    .unwrap();
    db.create_organisation(
        "o1",
        "Acme Data",
        "acme",
        Some("Publishes census data."),
        None,
    )
    .unwrap();
    db.update_organisation(
        "o1",
        "Acme Data",
        Some("Publishes census data."),
        Some("https://acme.example.org/"),
        Some("00000001234567890000"),
        Some("Acme data desk"),
        Some("desk@acme.example.org"),
        None,
        Some("FormalOrganization"),
        None,
    )
    .unwrap();
    db.create_group("g1", "o1", "Survey team", None).unwrap();
    let theme = |t: &str| format!("[\"{EU}data-theme/{t}\"]");
    for (id, name, owner_type, owner, t) in [
        (
            "census",
            "Census 2020",
            OwnerType::Organisation,
            "o1",
            "SOCI",
        ),
        ("notes", "Field notes", OwnerType::User, "u1", "ENVI"),
        ("survey", "Survey results", OwnerType::Group, "g1", "GOVE"),
    ] {
        db.create_dataset(
            id,
            name,
            Some(&format!("{name}, described.")),
            owner_type,
            owner,
            Visibility::Public,
            None,
        )
        .unwrap();
        db.update_dataset_metadata(
            id,
            (id == "census").then_some("http://creativecommons.org/licenses/by/4.0/"),
            Some(&theme(t)),
            Some("[\"open data\"]"),
            (id == "notes").then_some("Ada"),
            (id == "notes").then_some("ada@example.org"),
            None,
            Some("completed"),
            None,
            Some("http://sws.geonames.org/2750405/"),
            None,
        )
        .unwrap();
    }
    db.update_dataset_coverage(
        "census",
        Some("2020-01-01"),
        Some("2020-12-31"),
        Some("http://publications.europa.eu/resource/authority/frequency/DECENNIAL"),
    )
    .unwrap();
    let g_data = "https://example.org/census/instances";
    let g_links = "https://example.org/census/links";
    let g_private = "https://example.org/census/embargoed";
    for g in [g_data, g_links, g_private] {
        db.add_dataset_graph("census", g).unwrap();
    }
    db.set_dataset_graph_role("census", g_data, Some(GraphKind::Instances))
        .unwrap();
    db.set_dataset_graph_role("census", g_links, Some(GraphKind::Linkset))
        .unwrap();
    db.set_dataset_graph_private("census", g_private, true)
        .unwrap();
    store
        .load_str(
            "@prefix ex: <https://example.org/census/> . \
             @prefix geo: <http://www.opengis.net/ont/geosparql#> . \
             ex:p1 a ex:Person ; ex:age 31 ; ex:livesIn ex:t1 . \
             ex:p2 a ex:Person ; ex:age 47 . \
             ex:t1 a ex:Town ; geo:hasGeometry ex:t1g . \
             ex:t1g geo:asWKT \"POINT(5.1 52.1)\"^^geo:wktLiteral .",
            RdfFormat::Turtle,
            Some(g_data),
        )
        .unwrap();
    store
        .load_str(
            "<https://example.org/census/t1> <http://www.w3.org/2002/07/owl#sameAs> <http://www.wikidata.org/entity/Q803> . \
             <https://example.org/census/p1> <http://www.w3.org/2000/01/rdf-schema#seeAlso> <http://www.wikidata.org/entity/Q42> .",
            RdfFormat::Turtle,
            Some(g_links),
        )
        .unwrap();
    store
        .load_str(
            "<https://example.org/census/s1> a <https://example.org/secret/Informant> .",
            RdfFormat::Turtle,
            Some(g_private),
        )
        .unwrap();
    db.add_dataset_graph("notes", "https://example.org/notes/g")
        .unwrap();
    store
        .load_str(
            "<https://example.org/notes/n1> <http://purl.org/dc/terms/title> \"A note\" .",
            RdfFormat::Turtle,
            Some("https://example.org/notes/g"),
        )
        .unwrap();
    open_triplestore::ldes::store::set_stream(&db, "notes", true, 100).unwrap();
    for v in [
        version(
            "census",
            "1.0.0",
            Published,
            "2026-01-01T00:00:00Z",
            None,
            Some("First count"),
        ),
        version(
            "census",
            "1.1.0",
            Deprecated,
            "2026-02-01T00:00:00Z",
            Some("1.0.0"),
            None,
        ),
        version(
            "census",
            "1.2.0",
            Published,
            "2026-03-01T00:00:00Z",
            Some("1.1.0"),
            None,
        ),
        version(
            "census",
            "2.0.0",
            Draft,
            "2026-04-01T00:00:00Z",
            Some("1.2.0"),
            None,
        ),
    ] {
        insert_version(&store, BASE, &v).unwrap();
    }
    (store, db)
}

fn ap_options(profile: Profile) -> CatalogOptions {
    let mut o = CatalogOptions::new(BASE, profile);
    o.publisher_name = "Example Municipality".into();
    o.publisher_identifier = Some("00000009876543210000".into());
    o.publisher_type = Some("http://purl.org/adms/publishertype/LocalAuthority".into());
    o.license = Some("http://creativecommons.org/publicdomain/zero/1.0/".into());
    o.contact_name = Some("Open data office".into());
    o.contact_email = Some("opendata@example.org".into());
    o
}

/// Validate `ttl` against the shape files, returning (violations, warnings)
/// as readable lines.
fn validate_against(ttl: &str, shape_files: &[String]) -> (Vec<String>, Vec<String>) {
    use open_triplestore::shacl::report::Severity;
    let s = TripleStore::in_memory().unwrap();
    s.load_str(ttl, RdfFormat::Turtle, Some("urn:test:catalogue"))
        .unwrap();
    for f in shape_files {
        let shapes = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{f}: {e}"));
        s.load_str(&shapes, RdfFormat::Turtle, Some("urn:test:shapes"))
            .unwrap_or_else(|e| panic!("{f} does not parse: {e}"));
    }
    let pruned = prune_undefined_property_shapes(&s, "urn:test:shapes");
    assert_eq!(
        pruned, SEMIC_UNDEFINED_PROPERTY_SHAPES,
        "the upstream shapes' undefined property shapes changed; recheck PROVENANCE.md"
    );
    let report = open_triplestore::shacl::validate(
        &s,
        "urn:test:shapes",
        &["urn:test:catalogue".to_string()],
    )
    .expect("validation runs");
    let line = |r: &open_triplestore::shacl::report::ValidationResult| {
        format!(
            "{} {} {:?} = {:?} [{}]: {}",
            r.focus_node, r.source_constraint, r.path, r.value, r.source_shape, r.message
        )
    };
    let mut violations: Vec<String> = report
        .results
        .iter()
        .filter(|r| matches!(r.severity, Severity::Violation))
        .map(line)
        .collect();
    let mut warnings: Vec<String> = report
        .results
        .iter()
        .filter(|r| !matches!(r.severity, Severity::Violation))
        .map(line)
        .collect();
    violations.sort();
    warnings.sort();
    (violations, warnings)
}

/// `sh:property` links in SEMIC's DCAT-AP 3.0.1 shapes (`dcat-ap-SHACL.ttl`
/// and `ranges.ttl` together) to property shapes that neither file defines:
/// no `sh:path`, no constraint, no triple at all. A shapes graph with a
/// pathless property shape is ill-formed, and the engine refuses all of it
/// (fail-closed), so the runner drops those links before validating. They
/// constrain nothing; the vendored files stay byte-identical (PROVENANCE.md
/// lists the five).
const SEMIC_UNDEFINED_PROPERTY_SHAPES: usize = 5;

/// Drop `?shape sh:property ?ps` where `?ps` has no `sh:path`; returns how
/// many links were dropped.
fn prune_undefined_property_shapes(s: &TripleStore, graph: &str) -> usize {
    let q = format!(
        "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph}> {{ ?s <http://www.w3.org/ns/shacl#property> ?ps \
         FILTER NOT EXISTS {{ ?ps <http://www.w3.org/ns/shacl#path> ?p }} }} }}"
    );
    let n = match s.query(&q).unwrap() {
        QueryResults::Solutions(mut rows) => match rows.next().unwrap().unwrap().get("n") {
            Some(oxigraph::model::Term::Literal(l)) => l.value().parse().unwrap(),
            _ => 0,
        },
        _ => 0,
    };
    s.update(&format!(
        "DELETE {{ GRAPH <{graph}> {{ ?s <http://www.w3.org/ns/shacl#property> ?ps }} }} \
         WHERE {{ GRAPH <{graph}> {{ ?s <http://www.w3.org/ns/shacl#property> ?ps \
         FILTER NOT EXISTS {{ ?ps <http://www.w3.org/ns/shacl#path> ?p }} }} }}"
    ))
    .unwrap();
    n
}

fn semic_shapes() -> Vec<String> {
    ["dcat-ap-SHACL.ttl", "ranges.ttl"]
        .iter()
        .map(|f| format!("{SEMIC}/{f}"))
        .collect()
}

// The DCAT-AP 3.0.1 catalogue satisfies SEMIC's published shapes (mandatory
// properties, cardinalities, node kinds and ranges): no violation over a
// fixture that takes every branch of the generator.
#[test]
fn dcat_ap_catalogue_satisfies_the_semic_dcat_ap_301_shapes() {
    let (store, db) = rich_fixture();
    let (_, ttl, profile_warnings) = catalogue(&store, &db, &ap_options(Profile::DcatAp), None);
    let (violations, warnings) = validate_against(&ttl, &semic_shapes());
    assert!(
        violations.is_empty(),
        "{} violation(s) of the DCAT-AP 3.0.1 shapes:\n{}\n---\n{ttl}",
        violations.len(),
        violations.join("\n")
    );
    eprintln!(
        "DCAT-AP 3.0.1: 0 violations, {} other results; profile warnings: {profile_warnings:?}",
        warnings.len()
    );
}

// The DCAT-AP-NL 3 catalogue satisfies SEMIC's shapes plus Geonovum's
// DCAT-AP-NL 3 shapes (mandatory, class ranges, code-list ranges; the
// recommended ones are sh:Warning and only reported). Geonovum's shapes are
// fetched by `tests/fixtures/geonovum-dcat-ap-nl-3/fetch.sh`, not vendored.
#[test]
fn dcat_ap_nl_catalogue_satisfies_the_geonovum_dcat_ap_nl_3_shapes() {
    let nl: Vec<String> = [
        "dcat-ap-nl-SHACL.ttl",
        "dcat-ap-nl-SHACL-klassebereik.ttl",
        "dcat-ap-nl-SHACL-klassebereik-codelijsten.ttl",
        "dcat-ap-nl-SHACL-aanbevolen.ttl",
    ]
    .iter()
    .map(|f| format!("{GEONOVUM}/{f}"))
    .collect();
    if nl.iter().any(|f| !std::path::Path::new(f).exists()) {
        assert!(
            std::env::var("OTS_TEST_DCAT_AP_NL_REQUIRED").as_deref() != Ok("1"),
            "the DCAT-AP-NL 3 shapes are missing: run tests/fixtures/geonovum-dcat-ap-nl-3/fetch.sh"
        );
        eprintln!("skipped: DCAT-AP-NL 3 shapes not fetched (tests/fixtures/geonovum-dcat-ap-nl-3/fetch.sh)");
        return;
    }
    let (store, db) = rich_fixture();
    let (_, ttl, profile_warnings) = catalogue(&store, &db, &ap_options(Profile::DcatApNl), None);
    let mut shapes = semic_shapes();
    shapes.extend(nl);
    let (violations, warnings) = validate_against(&ttl, &shapes);
    assert!(
        violations.is_empty(),
        "{} violation(s) of the DCAT-AP-NL 3 shapes:\n{}\n---\n{ttl}",
        violations.len(),
        violations.join("\n")
    );
    assert!(
        profile_warnings.is_empty(),
        "a fully described registry leaves nothing to warn about: {profile_warnings:?}"
    );
    eprintln!(
        "DCAT-AP-NL 3: 0 violations, {} warnings (recommended properties):\n{}",
        warnings.len(),
        warnings.join("\n")
    );
}

/// The value of `<s> <p> ?n` as a number.
fn number(s: &TripleStore, subject: &str, predicate: &str) -> Option<u64> {
    let q = format!("SELECT ?n WHERE {{ {subject} <{predicate}> ?n }}");
    match s.query(&q).ok()? {
        QueryResults::Solutions(mut rows) => match rows.next()?.ok()?.get("n")? {
            oxigraph::model::Term::Literal(l) => l.value().parse().ok(),
            _ => None,
        },
        _ => None,
    }
}

// VoID per dataset: statistics, class and property partitions, vocabularies,
// example resources, features, data dumps and the SPARQL endpoint — over the
// dataset's graphs the caller may read.
#[test]
fn void_describes_each_dataset() {
    let (store, db) = rich_fixture();
    let (s, ttl, _) = catalogue(&store, &db, &CatalogOptions::new(BASE, Profile::Dcat), None);
    let d = format!("<{BASE}/dataset/census>");
    // Instances (8 triples) + linkset (2); the private graph is left out.
    assert_eq!(number(&s, &d, &format!("{VOID}triples")), Some(10), "{ttl}");
    assert_eq!(
        number(&s, &d, &format!("{VOID}documents")),
        Some(2),
        "{ttl}"
    );
    assert_eq!(number(&s, &d, &format!("{VOID}classes")), Some(2), "{ttl}");
    assert!(
        number(&s, &d, &format!("{VOID}distinctSubjects")).unwrap() >= 4,
        "{ttl}"
    );
    assert!(ask(&s, &format!("ASK {{ {d} <{VOID}classPartition> ?c . ?c <{VOID}class> <https://example.org/census/Person> ; <{VOID}entities> 2 }}")), "{ttl}");
    assert!(ask(&s, &format!("ASK {{ {d} <{VOID}propertyPartition> ?c . ?c <{VOID}property> <https://example.org/census/age> ; <{VOID}triples> 2 }}")), "{ttl}");
    assert!(ask(&s, &format!("ASK {{ {d} <{VOID}vocabulary> <https://example.org/census/> , <http://www.opengis.net/ont/geosparql#> }}")), "{ttl}");
    assert!(ask(&s, &format!("ASK {{ {d} <{VOID}exampleResource> ?x ; <{VOID}sparqlEndpoint> <{BASE}/sparql> ; <{VOID}feature> <http://www.w3.org/ns/formats/Turtle> ; <{VOID}dataDump> <{BASE}/store?graph=https%3A%2F%2Fexample%2Eorg%2Fcensus%2Finstances> }}")), "{ttl}");
}

// A linkset-role graph is a void:Linkset: the dataset is its subjects'
// target, the namespace its links point into is its objects' target, and its
// link predicates are listed.
#[test]
fn void_describes_linksets() {
    let (store, db) = rich_fixture();
    let (s, ttl, _) = catalogue(&store, &db, &CatalogOptions::new(BASE, Profile::Dcat), None);
    let ls = "<https://example.org/census/links>";
    assert!(ask(&s, &format!("ASK {{ <{BASE}/dataset/census> <{VOID}subset> {ls} . {ls} a <{VOID}Linkset> ; <{VOID}subjectsTarget> <{BASE}/dataset/census> ; <{VOID}objectsTarget> ?t ; <{VOID}linkPredicate> <http://www.w3.org/2002/07/owl#sameAs> , <http://www.w3.org/2000/01/rdf-schema#seeAlso> ; <{VOID}triples> 2 . ?t <{VOID}uriSpace> \"http://www.wikidata.org/entity/\" }}")), "{ttl}");
}

// Partitions name the vocabulary of the data, so they are computed only per
// dataset over the graphs the caller may read: never at the anonymous
// aggregate level, and a private graph's classes stay out of an anonymous
// caller's partitions while its readers see them.
#[test]
fn void_partitions_never_leak_unreadable_graphs() {
    let (store, db) = rich_fixture();
    let secret = "https://example.org/secret/Informant";
    let (s, ttl, _) = catalogue(&store, &db, &CatalogOptions::new(BASE, Profile::Dcat), None);
    assert!(!ttl.contains(secret), "anonymous: {ttl}");
    assert!(
        !ask(&s, &format!("ASK {{ <{BASE}/dataset> <{VOID}classPartition>|<{VOID}propertyPartition>|<{VOID}vocabulary>|<{VOID}exampleResource> ?x }}")),
        "the aggregate carries counts only:\n{ttl}"
    );
    let (s, ttl, _) = catalogue(
        &store,
        &db,
        &CatalogOptions::new(BASE, Profile::Dcat),
        Some("root"),
    );
    assert!(ask(&s, &format!("ASK {{ <{BASE}/dataset/census> <{VOID}classPartition> ?c . ?c <{VOID}class> <{secret}> }}")), "an administrator reads the private graph:\n{ttl}");
    assert!(
        !ask(
            &s,
            &format!("ASK {{ <{BASE}/dataset> <{VOID}classPartition> ?x }}")
        ),
        "not even an administrator's aggregate has partitions:\n{ttl}"
    );
}

// The shapes bite: a theme the catalogue has no label for violates the
// DCAT-AP `skos:Concept` shape (and is a profile warning), so the zero
// violations above are not vacuous.
#[test]
fn the_semic_shapes_catch_an_unlabelled_theme() {
    let (store, db) = rich_fixture();
    db.update_dataset_metadata(
        "notes",
        None,
        Some("[\"https://example.org/themes/birds\"]"),
        None,
        Some("Ada"),
        Some("ada@example.org"),
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let (_, ttl, profile_warnings) = catalogue(&store, &db, &ap_options(Profile::DcatAp), None);
    let (violations, _) = validate_against(&ttl, &semic_shapes());
    assert!(
        violations
            .iter()
            .any(|v| v.contains("https://example.org/themes/birds") && v.contains("prefLabel")),
        "{violations:#?}"
    );
    assert!(
        profile_warnings
            .iter()
            .any(|w| w.contains("https://example.org/themes/birds")),
        "{profile_warnings:?}"
    );
}
