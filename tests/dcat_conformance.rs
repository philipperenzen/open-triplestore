//! DCAT 2 catalog conformance tests.
//!
//! Exercises `dcat::generate_dcat_catalog`, which produces a W3C DCAT 2 catalogue
//! (dcat:Catalog / dcat:Dataset / dcat:Distribution) with VoID statistics and
//! Dublin Core metadata, then validates the emitted RDF structurally.

use std::sync::Arc;

use open_triplestore::auth::db::AuthDb;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::dcat::generate_dcat_catalog;
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
    // VoID triples count is emitted (void:triples or void:Dataset typing).
    assert!(
        ttl.contains("void") || ask(&s, "ASK { ?d <http://rdfs.org/ns/void#triples> ?n }"),
        "catalog includes VoID statistics:\n{ttl}"
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
