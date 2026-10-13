//! Linked-document containers with the ICDD profile (7.1): an ICDD archive
//! imports into a dataset (documents → assets, linksets and payload triples →
//! role-typed graphs, the index → a catalogue graph), a dataset exports as
//! an ICDD archive, and the export re-imports.

mod common;

use std::io::{Cursor, Read, Write};

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::Value;
use tower::ServiceExt as _;

const CT: &str = "https://standards.iso.org/iso/21597/-1/ed-1/en/Container#";
const LS: &str = "https://standards.iso.org/iso/21597/-1/ed-1/en/Linkset#";

fn sample_icdd() -> Vec<u8> {
    let index = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ct="{CT}">
  <ct:ContainerDescription rdf:about="urn:icdd:bridge-handover">
    <ct:description>Handover of the bridge inspection</ct:description>
    <ct:conformanceIndicator>ICDD-Part1-Container</ct:conformanceIndicator>
    <ct:publishedBy><ct:Party rdf:about="urn:party:example-authority"><ct:name>Example Road Authority</ct:name></ct:Party></ct:publishedBy>
    <ct:containsDocument>
      <ct:InternalDocument rdf:about="urn:icdd:doc:report">
        <ct:filename>report.txt</ct:filename><ct:filetype>txt</ct:filetype>
        <ct:description>Inspection report</ct:description>
      </ct:InternalDocument>
    </ct:containsDocument>
    <ct:containsDocument>
      <ct:ExternalDocument rdf:about="urn:icdd:doc:norm"><ct:url>https://example.org/norms/NEN2767</ct:url></ct:ExternalDocument>
    </ct:containsDocument>
    <ct:containsLinkset>
      <ct:Linkset rdf:about="urn:icdd:linkset:main"><ct:filename>links.ttl</ct:filename></ct:Linkset>
    </ct:containsLinkset>
  </ct:ContainerDescription>
</rdf:RDF>
"#
    );
    let links = format!(
        "@prefix ls: <{LS}> .\n<urn:icdd:link:1> a ls:Link ; ls:hasLinkElement [ a ls:LinkElement ; ls:hasDocument <urn:icdd:doc:report> ] , [ a ls:LinkElement ; ls:hasDocument <urn:icdd:doc:norm> ] .\n"
    );
    let data = "<urn:asset:bridge-1> a <urn:Bridge> ; <urn:span> 120 .\n";
    let mut buf = Vec::new();
    {
        let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for (name, bytes) in [
            ("Index.rdf", index.as_bytes()),
            (
                "Payload documents/report.txt",
                b"Deck OK, bearings worn.\n".as_slice(),
            ),
            ("Payload triples/links.ttl", links.as_bytes()),
            ("Payload triples/data.ttl", data.as_bytes()),
        ] {
            w.start_file(name, opts).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap();
    }
    buf
}

/// A minimal ICDD: one linkset, named `about` in the index.
fn linkset_icdd(about: &str) -> Vec<u8> {
    let index = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ct="{CT}">
  <ct:ContainerDescription rdf:about="urn:icdd:minimal">
    <ct:containsLinkset>
      <ct:Linkset rdf:about="{about}"><ct:filename>links.ttl</ct:filename></ct:Linkset>
    </ct:containsLinkset>
  </ct:ContainerDescription>
</rdf:RDF>
"#
    );
    let links = format!("@prefix ls: <{LS}> .\n<urn:icdd:link:1> a ls:Link .\n");
    let mut buf = Vec::new();
    {
        let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for (name, bytes) in [
            ("Index.rdf", index.as_bytes()),
            ("Payload triples/links.ttl", links.as_bytes()),
        ] {
            w.start_file(name, opts).unwrap();
            w.write_all(bytes).unwrap();
        }
        w.finish().unwrap();
    }
    buf
}

fn count(state: &open_triplestore::server::AppState, graph: &str) -> u64 {
    match state.store.query(&format!(
        "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}"
    )) {
        Ok(QueryResults::Solutions(sol)) => sol
            .flatten()
            .next()
            .and_then(|r| r.get("n").map(|t| t.to_string()))
            .and_then(|t| t.trim_start_matches('"').split('"').next()?.parse().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

async fn send(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    ct: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = ct {
        b = b.header(header::CONTENT_TYPE, c);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (st, bytes, ct)
}

#[tokio::test]
async fn icdd_container_imports_exports_and_round_trips() {
    let (mut state, token) = admin_state();
    // The test app ships a no-op object store; documents need a real one.
    let tmp = std::env::temp_dir().join(format!("ots-containers-{}", uuid::Uuid::new_v4()));
    state.object_store =
        std::sync::Arc::new(open_triplestore::storage::ObjectStore::local(tmp).unwrap());
    for id in ["a", "b"] {
        state
            .auth_db
            .create_dataset(
                id,
                id,
                None,
                OwnerType::User,
                "adm",
                Visibility::Private,
                None,
            )
            .unwrap();
    }
    let app = test_app(state.clone());
    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));

    // Import.
    let (st, body, _) = send(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        Some(&token),
        Some("application/zip"),
        sample_icdd(),
    )
    .await;
    let txt = String::from_utf8_lossy(&body).into_owned();
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["profile"], "icdd");
    assert_eq!(r["container"], "urn:icdd:bridge-handover");
    assert_eq!(r["description"], "Handover of the bridge inspection");
    let docs = r["documents"].as_array().unwrap();
    let report = docs
        .iter()
        .find(|d| d["filename"] == "report.txt")
        .expect("report document");
    let asset_id = report["asset_id"].as_str().unwrap().to_string();
    assert!(
        docs.iter()
            .any(|d| d["external_url"] == "https://example.org/norms/NEN2767"),
        "{txt}"
    );
    let graphs = r["graphs"].as_array().unwrap();
    let role_of = |file: &str| {
        graphs.iter().find(|g| g["file"] == file).map(|g| {
            (
                g["role"].as_str().unwrap().to_string(),
                g["iri"].as_str().unwrap().to_string(),
            )
        })
    };
    let (links_role, links_iri) = role_of("links.ttl").expect("linkset graph");
    assert_eq!(links_role, "linkset");
    assert!(
        links_iri.starts_with("http://localhost:7878/dataset/a/"),
        "a linkset IRI outside the dataset is re-homed under it: {links_iri}"
    );
    assert!(
        ask(&format!("ASK {{ GRAPH <{}> {{ <{links_iri}> <https://opentriplestore.org/ns#sourceIri> <urn:icdd:linkset:main> }} }}", r["index_graph"].as_str().unwrap())),
        "the index graph records the linkset's original IRI"
    );
    let (data_role, data_iri) = role_of("data.ttl").expect("payload graph");
    assert_eq!(data_role, "instances");
    assert!(ask(&format!(
        "ASK {{ GRAPH <{data_iri}> {{ <urn:asset:bridge-1> <urn:span> 120 }} }}"
    )));
    assert!(ask(&format!(
        "ASK {{ GRAPH <{links_iri}> {{ <urn:icdd:link:1> a <{LS}Link> }} }}"
    )));
    let index_graph = r["index_graph"].as_str().unwrap().to_string();
    assert!(ask(&format!("ASK {{ GRAPH <{index_graph}> {{ <urn:icdd:bridge-handover> a <{CT}ContainerDescription> ; <https://opentriplestore.org/ns#importedInto> <http://localhost:7878/dataset/a> }} }}")));
    assert!(ask(&format!("ASK {{ GRAPH <{index_graph}> {{ <urn:icdd:doc:report> <https://opentriplestore.org/ns#downloadUrl> ?u }} }}")), "documents link to their asset URLs");
    // Roles on the registry, the document as an asset in the container folder.
    let entries = state.auth_db.list_dataset_graph_entries("a").unwrap();
    assert!(entries.iter().any(|e| e.graph_iri == index_graph
        && e.graph_role == Some(open_triplestore::auth::models::GraphKind::Catalog)));
    let assets = state.auth_db.list_dataset_assets("a").unwrap();
    let a = assets
        .iter()
        .find(|a| a.id == asset_id)
        .expect("asset record");
    assert_eq!(a.filename, "report.txt");
    assert!(a.folder.starts_with("containers/"), "{}", a.folder);
    let (bytes, _) = state.object_store.download(&a.s3_key).await.unwrap();
    assert_eq!(&bytes[..], b"Deck OK, bearings worn.\n");
    // History.
    let (_, body, _) = send(
        &app,
        Method::GET,
        "/api/datasets/a/commits",
        Some(&token),
        None,
        Vec::new(),
    )
    .await;
    assert!(String::from_utf8_lossy(&body).contains("container import"));

    // Export.
    let (st, zip_bytes, ct) = send(
        &app,
        Method::GET,
        "/api/datasets/a/containers/export?profile=icdd",
        Some(&token),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ct, "application/zip");
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes.clone())).unwrap();
    // The layout's three folders are explicit entries.
    for dir in [
        "Ontology resources/",
        "Payload documents/",
        "Payload triples/",
    ] {
        assert!(
            (0..archive.len()).any(|i| archive.by_index(i).unwrap().name() == dir),
            "{dir} folder entry"
        );
    }
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    assert!(names.contains(&"Index.rdf".to_string()), "{names:?}");
    assert!(
        names
            .iter()
            .any(|n| n.ends_with("report.txt") && n.starts_with("Payload documents/")),
        "{names:?}"
    );
    // The linkset in RDF/XML under Payload triples/; the instance graph as an
    // RDF document under Payload documents/ (filenames are relative to it).
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("Payload triples/") && n.ends_with(".rdf")),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("Payload documents/") && n.ends_with(".rdf")),
        "{names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with("README.txt")),
        "{names:?}"
    );
    let mut index = String::new();
    archive
        .by_name("Index.rdf")
        .unwrap()
        .read_to_string(&mut index)
        .unwrap();
    let tmp = open_triplestore::store::TripleStore::in_memory().unwrap();
    tmp.load_str(&index, RdfFormat::RdfXml, Some("urn:idx"))
        .expect("Index.rdf is valid RDF/XML");
    assert!(matches!(tmp.query(&format!("ASK {{ GRAPH <urn:idx> {{ ?c a <{CT}ContainerDescription> ; <{CT}conformanceIndicator> \"ICDD-Part1-Container\" ; <{CT}containsLinkset> ?l . ?l <{CT}filename> ?f . FILTER(STRSTARTS(STR(?l), \"http://localhost:7878/dataset/a/\")) }} }}")), Ok(QueryResults::Boolean(true))), "the exported index lists the linkset by its graph IRI: {index}");

    // Documents keep their index IRIs, so the linkset still resolves; every
    // internal document has its name and container link.
    assert!(matches!(tmp.query(&format!("ASK {{ GRAPH <urn:idx> {{ ?c <{CT}containsDocument> <urn:icdd:doc:report> . <urn:icdd:doc:report> <{CT}filename> \"report.txt\" ; <{CT}name> ?n ; <{CT}belongsToContainer> ?c }} }}")), Ok(QueryResults::Boolean(true))), "{index}");
    assert!(matches!(tmp.query(&format!("ASK {{ GRAPH <urn:idx> {{ <urn:icdd:doc:norm> a <{CT}ExternalDocument> ; <{CT}url> \"https://example.org/norms/NEN2767\" }} }}")), Ok(QueryResults::Boolean(true))), "{index}");
    assert!(matches!(tmp.query(&format!("ASK {{ GRAPH <urn:idx> {{ ?c <{CT}publishedBy> ?p . ?p a <{CT}Person> ; <{CT}name> \"admin\" }} }}")), Ok(QueryResults::Boolean(true))), "a typed party, not ct:Party: {index}");
    assert!(!matches!(tmp.query(&format!("ASK {{ GRAPH <urn:idx> {{ ?d <{CT}filename> ?f FILTER(STRSTARTS(?f, \"Payload\")) }} }}")), Ok(QueryResults::Boolean(true))), "{index}");

    // Round trip: the export imports into another dataset with the same data.
    let (st, body, _) = send(
        &app,
        Method::POST,
        "/api/datasets/b/containers/import",
        Some(&token),
        Some("application/zip"),
        zip_bytes,
    )
    .await;
    let txt = String::from_utf8_lossy(&body).into_owned();
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let r2: Value = serde_json::from_str(&txt).unwrap();
    let b_graphs = r2["graphs"].as_array().unwrap();
    assert!(b_graphs.iter().any(|g| g["role"] == "linkset"), "{txt}");
    let data_b = b_graphs
        .iter()
        .find(|g| g["role"] == "instances")
        .expect("instances graph in b")["iri"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        ask(&format!(
            "ASK {{ GRAPH <{data_b}> {{ <urn:asset:bridge-1> <urn:span> 120 }} }}"
        )),
        "{txt}"
    );
    assert_eq!(state.auth_db.list_dataset_assets("b").unwrap().len(), 1);

    // Guards: not an archive; unknown profile; stranger.
    let (st, _, _) = send(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        Some(&token),
        Some("application/zip"),
        b"nope".to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _, _) = send(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import?profile=bcf",
        Some(&token),
        Some("application/zip"),
        sample_icdd(),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    state
        .auth_db
        .create_user("eve", "eve", "eve@t.com", "h", SystemRole::User)
        .unwrap();
    let eve = mint_token("eve", "eve", "user");
    let (st, _, _) = send(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        Some(&eve),
        Some("application/zip"),
        sample_icdd(),
    )
    .await;
    assert!(
        st == StatusCode::NOT_FOUND || st == StatusCode::FORBIDDEN,
        "{st}"
    );
    let (st, _, _) = send(
        &app,
        Method::GET,
        "/api/datasets/a/containers/export",
        Some(&eve),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn icdd_payload_graph_outside_the_dataset_is_rehomed() {
    let (state, token) = admin_state();
    for id in ["a", "v"] {
        state
            .auth_db
            .create_dataset(
                id,
                id,
                None,
                OwnerType::User,
                "adm",
                Visibility::Private,
                None,
            )
            .unwrap();
    }
    // The victim: another dataset's graph, registered to it, with data.
    let victim = "http://localhost:7878/dataset/v/data";
    state.auth_db.add_dataset_graph("v", victim).unwrap();
    state
        .store
        .load_str(
            "<urn:v:1> <urn:p> \"one\" . <urn:v:2> <urn:p> \"two\" .",
            RdfFormat::Turtle,
            Some(victim),
        )
        .unwrap();
    let before = count(&state, victim);
    assert_eq!(before, 2);
    let app = test_app(state.clone());

    // An index whose linkset claims the victim graph's IRI.
    let (st, body, _) = send(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        Some(&token),
        Some("application/zip"),
        linkset_icdd(victim),
    )
    .await;
    let txt = String::from_utf8_lossy(&body).into_owned();
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    let linkset_iri = r["graphs"][0]["iri"].as_str().unwrap().to_string();

    assert_eq!(
        count(&state, victim),
        before,
        "the victim graph is untouched"
    );
    assert!(
        !state
            .auth_db
            .list_dataset_graphs("a")
            .unwrap()
            .contains(&victim.to_string()),
        "the victim graph is not registered to the importing dataset"
    );
    assert_ne!(linkset_iri, victim);
    assert!(
        linkset_iri.starts_with("http://localhost:7878/dataset/a/"),
        "the payload lands under the importing dataset's namespace: {linkset_iri}"
    );
    assert_eq!(count(&state, &linkset_iri), 1, "the payload was loaded");
    assert!(
        state
            .auth_db
            .list_dataset_graphs("a")
            .unwrap()
            .contains(&linkset_iri),
        "the re-homed graph is registered to the importing dataset"
    );
    let index_graph = r["index_graph"].as_str().unwrap();
    assert!(
        matches!(
            state.store.query(&format!(
                "ASK {{ GRAPH <{index_graph}> {{ <{linkset_iri}> <https://opentriplestore.org/ns#sourceIri> <{victim}> }} }}"
            )),
            Ok(QueryResults::Boolean(true))
        ),
        "the index graph keeps the original IRI as ots:sourceIri"
    );
    assert!(
        r["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap_or("").contains(victim)),
        "the response warns about the re-homed payload: {txt}"
    );
}

#[tokio::test]
async fn icdd_payload_graph_inside_the_dataset_namespace_is_honoured() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "a",
            "a",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    let app = test_app(state.clone());
    let own = "http://localhost:7878/dataset/a/links/main";
    let (st, body, _) = send(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        Some(&token),
        Some("application/zip"),
        linkset_icdd(own),
    )
    .await;
    let txt = String::from_utf8_lossy(&body).into_owned();
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["graphs"][0]["iri"], own, "{txt}");
    assert_eq!(count(&state, own), 1);
    assert!(state
        .auth_db
        .list_dataset_graphs("a")
        .unwrap()
        .contains(&own.to_string()));
}

#[tokio::test]
async fn export_hides_private_graphs_from_viewers() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "p",
            "p",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    let open = "http://localhost:7878/dataset/p/open";
    let secret = "http://localhost:7878/dataset/p/secret";
    for (g, marker) in [(open, "OPEN-MARKER"), (secret, "SECRET-MARKER")] {
        state.auth_db.add_dataset_graph("p", g).unwrap();
        state
            .store
            .load_str(
                &format!("<urn:x:{marker}> <urn:p> \"{marker}\" ."),
                RdfFormat::Turtle,
                Some(g),
            )
            .unwrap();
    }
    state
        .auth_db
        .set_dataset_graph_private("p", secret, true)
        .unwrap();
    let app = test_app(state.clone());

    let unzip_all = |bytes: Vec<u8>| -> String {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut all = String::new();
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).unwrap();
            all.push_str(f.name());
            all.push('\n');
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            all.push_str(&s);
        }
        all
    };

    // Anonymous on a public dataset: the private graph is not in the archive.
    let (st, bytes, _) = send(
        &app,
        Method::GET,
        "/api/datasets/p/containers/export",
        None,
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let anon = unzip_all(bytes);
    assert!(anon.contains("OPEN-MARKER"), "{anon}");
    assert!(
        !anon.contains("SECRET-MARKER") && !anon.contains(secret),
        "a viewer's export must not carry the private graph:\n{anon}"
    );

    // The owner gets everything.
    let (st, bytes, _) = send(
        &app,
        Method::GET,
        "/api/datasets/p/containers/export",
        Some(&token),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let owner = unzip_all(bytes);
    assert!(
        owner.contains("OPEN-MARKER") && owner.contains("SECRET-MARKER"),
        "{owner}"
    );
}

// ── Part 1 validation, conformant export, complete import (cards 14–16) ─────

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// Build a ZIP; a name ending in `/` is a folder entry.
fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
        let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        for (name, bytes) in files {
            if let Some(dir) = name.strip_suffix('/') {
                w.add_directory(dir, opts).unwrap();
            } else {
                w.start_file(*name, opts).unwrap();
                w.write_all(bytes).unwrap();
            }
        }
        w.finish().unwrap();
    }
    buf
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(b))
}

const MEASUREMENTS: &[u8] = b"span,deflection\n1,0.4\n2,0.6\n";

/// A Part 1 container using every document kind: internal (with versions,
/// an alternative, a creator), external, folder (with a sub-folder), secured
/// (`checksum`, default: the right SHA-256) and encrypted; typed parties; a
/// directed binary link in an RDF/XML linkset. Ontology files are not
/// included (ISO's files are not in this repository).
fn part1_icdd(checksum: Option<&str>, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let sum = checksum
        .map(str::to_string)
        .unwrap_or_else(|| sha256_hex(MEASUREMENTS));
    let index = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="{RDF}" xmlns:ct="{CT}">
  <ct:ContainerDescription rdf:about="urn:icdd:handover">
    <ct:conformanceIndicator>ICDD-Part1-Container</ct:conformanceIndicator>
    <ct:description>Handover of the bridge inspection</ct:description>
    <ct:versionID>2</ct:versionID>
    <ct:publishedBy rdf:resource="urn:party:authority"/>
    <ct:createdBy rdf:resource="urn:party:inspector"/>
    <ct:containsDocument rdf:resource="urn:icdd:doc:report"/>
    <ct:containsDocument rdf:resource="urn:icdd:doc:report-v1"/>
    <ct:containsDocument rdf:resource="urn:icdd:doc:norm"/>
    <ct:containsDocument rdf:resource="urn:icdd:doc:drawings"/>
    <ct:containsDocument rdf:resource="urn:icdd:doc:measurements"/>
    <ct:containsDocument rdf:resource="urn:icdd:doc:sealed"/>
    <ct:containsLinkset rdf:resource="urn:icdd:linkset:main"/>
  </ct:ContainerDescription>
  <ct:Organisation rdf:about="urn:party:authority"><ct:name>Example Road Authority</ct:name></ct:Organisation>
  <ct:Person rdf:about="urn:party:inspector"><ct:name>A. Inspector</ct:name></ct:Person>
  <ct:InternalDocument rdf:about="urn:icdd:doc:report">
    <ct:name>Inspection report</ct:name>
    <ct:filename>reports/report.txt</ct:filename><ct:filetype>txt</ct:filetype><ct:format>text/plain</ct:format>
    <ct:belongsToContainer rdf:resource="urn:icdd:handover"/>
    <ct:versionID>2</ct:versionID><ct:versionDescription>After the second visit</ct:versionDescription>
    <ct:priorVersion rdf:resource="urn:icdd:doc:report-v1"/>
    <ct:alternativeDocument rdf:resource="urn:icdd:doc:norm"/>
    <ct:createdBy rdf:resource="urn:party:inspector"/>
    <ct:requested rdf:datatype="http://www.w3.org/2001/XMLSchema#boolean">false</ct:requested>
  </ct:InternalDocument>
  <ct:InternalDocument rdf:about="urn:icdd:doc:report-v1">
    <ct:name>Inspection report (first visit)</ct:name>
    <ct:filename>reports/report-v1.txt</ct:filename><ct:filetype>txt</ct:filetype>
    <ct:belongsToContainer rdf:resource="urn:icdd:handover"/>
    <ct:versionID>1</ct:versionID>
  </ct:InternalDocument>
  <ct:ExternalDocument rdf:about="urn:icdd:doc:norm">
    <ct:name>Inspection norm</ct:name><ct:url>https://example.org/norms/inspection</ct:url>
  </ct:ExternalDocument>
  <ct:FolderDocument rdf:about="urn:icdd:doc:drawings">
    <ct:name>Drawings</ct:name><ct:foldername>drawings</ct:foldername>
    <ct:belongsToContainer rdf:resource="urn:icdd:handover"/>
  </ct:FolderDocument>
  <ct:SecuredDocument rdf:about="urn:icdd:doc:measurements">
    <ct:name>Measurements</ct:name>
    <ct:filename>measurements.csv</ct:filename><ct:filetype>csv</ct:filetype>
    <ct:belongsToContainer rdf:resource="urn:icdd:handover"/>
    <ct:checksum>{sum}</ct:checksum><ct:checksumAlgorithm>SHA-256</ct:checksumAlgorithm>
  </ct:SecuredDocument>
  <ct:EncryptedDocument rdf:about="urn:icdd:doc:sealed">
    <ct:name>Sealed annex</ct:name>
    <ct:filename>sealed.bin</ct:filename><ct:filetype>bin</ct:filetype>
    <ct:belongsToContainer rdf:resource="urn:icdd:handover"/>
    <ct:encryptionAlgorithm>AES-256-GCM</ct:encryptionAlgorithm>
  </ct:EncryptedDocument>
  <ct:Linkset rdf:about="urn:icdd:linkset:main"><ct:filename>links.rdf</ct:filename></ct:Linkset>
</rdf:RDF>
"#
    );
    let links = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="{RDF}" xmlns:ls="{LS}">
  <ls:DirectedBinaryLink rdf:about="urn:icdd:link:1">
    <ls:hasFromLinkElement><ls:LinkElement rdf:about="urn:icdd:le:1"><ls:hasDocument rdf:resource="urn:icdd:doc:report"/></ls:LinkElement></ls:hasFromLinkElement>
    <ls:hasToLinkElement><ls:LinkElement rdf:about="urn:icdd:le:2"><ls:hasDocument rdf:resource="urn:icdd:doc:measurements"/></ls:LinkElement></ls:hasToLinkElement>
  </ls:DirectedBinaryLink>
</rdf:RDF>
"#
    );
    let mut files: Vec<(&str, &[u8])> = vec![
        ("Index.rdf", index.as_bytes()),
        ("Ontology resources/", b""),
        ("Payload documents/", b""),
        ("Payload triples/", b""),
        (
            "Payload documents/reports/report.txt",
            b"Deck OK, bearings worn.\n",
        ),
        ("Payload documents/reports/report-v1.txt", b"Deck OK.\n"),
        ("Payload documents/drawings/plan.txt", b"plan view\n"),
        (
            "Payload documents/drawings/details/section.txt",
            b"section A-A\n",
        ),
        ("Payload documents/measurements.csv", MEASUREMENTS),
        ("Payload documents/sealed.bin", b"\x00\x13\x37opaque"),
        ("Payload triples/links.rdf", links.as_bytes()),
    ];
    files.extend_from_slice(extra);
    zip_of(&files)
}

/// Like `send`, returning the response headers too.
async fn send_h(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, Vec<u8>, axum::http::HeaderMap) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let headers = resp.headers().clone();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (st, bytes, headers)
}

fn json(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(b)))
}

fn codes(report: &Value, severity: &str) -> Vec<String> {
    report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == severity)
        .map(|f| f["code"].as_str().unwrap().to_string())
        .collect()
}

fn state_with_store(ids: &[&str]) -> (open_triplestore::server::AppState, String) {
    let (mut state, token) = admin_state();
    let tmp = std::env::temp_dir().join(format!("ots-containers-{}", uuid::Uuid::new_v4()));
    state.object_store =
        std::sync::Arc::new(open_triplestore::storage::ObjectStore::local(tmp).unwrap());
    for id in ids {
        state
            .auth_db
            .create_dataset(
                id,
                id,
                None,
                OwnerType::User,
                "adm",
                Visibility::Private,
                None,
            )
            .unwrap();
    }
    (state, token)
}

fn archive_index(zip_bytes: &[u8]) -> (Vec<String>, String) {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes.to_vec())).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    let mut index = String::new();
    archive
        .by_name("Index.rdf")
        .unwrap()
        .read_to_string(&mut index)
        .unwrap();
    (names, index)
}

fn index_ask(index: &str, body: &str) -> bool {
    let tmp = open_triplestore::store::TripleStore::in_memory().unwrap();
    tmp.load_str(index, RdfFormat::RdfXml, Some("urn:idx"))
        .expect("Index.rdf is valid RDF/XML");
    matches!(
        tmp.query(&format!("ASK {{ GRAPH <urn:idx> {{ {body} }} }}")),
        Ok(QueryResults::Boolean(true))
    )
}

#[tokio::test]
async fn icdd_validate_endpoint_reports_part1_findings() {
    let (state, token) = state_with_store(&[]);
    let app = test_app(state);

    // A conformant Part 1 container: no violation; without ISO's ontology
    // files it is flagged as not Part 1-conformant (a warning).
    let (st, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        part1_icdd(None, &[]),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let r = json(&body);
    assert_eq!(r["profile"], "icdd");
    assert_eq!(r["conforms"], true, "{r:#}");
    assert_eq!(r["violations"], 0, "{r:#}");
    let warnings = codes(&r, "warning");
    assert!(
        warnings.iter().all(|c| c == "ontology-resource-missing"),
        "{r:#}"
    );
    assert_eq!(warnings.len(), 2, "Container.rdf and Linkset.rdf: {r:#}");
    assert!(
        codes(&r, "info").contains(&"encrypted-document".to_string()),
        "{r:#}"
    );

    // The pre-0.7 sample: abstract ct:Party, documents without ct:name or
    // ct:belongsToContainer, a Turtle linkset.
    let (_, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate?profile=icdd",
        Some(&token),
        sample_icdd(),
    )
    .await;
    let r = json(&body);
    assert_eq!(r["conforms"], false);
    let v = codes(&r, "violation");
    for want in [
        "shape:PartyShape",
        "shape:DocumentShape",
        "shape:InternalDocumentShape",
    ] {
        assert!(v.iter().any(|c| c == want), "{want}: {r:#}");
    }
    assert!(
        codes(&r, "warning").contains(&"linkset-syntax".to_string()),
        "{r:#}"
    );
    let party = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["code"] == "shape:PartyShape")
        .unwrap();
    assert!(
        party["focus"]
            .as_str()
            .unwrap()
            .contains("urn:party:example-authority"),
        "{party:#}"
    );

    // Checksum mismatch.
    let (_, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        part1_icdd(Some(&"0".repeat(64)), &[]),
    )
    .await;
    let r = json(&body);
    assert!(
        codes(&r, "violation").contains(&"checksum-mismatch".to_string()),
        "{r:#}"
    );

    // A Part 1 container may not extend the ICDD classes.
    let ext = format!(
        r#"<?xml version="1.0"?><rdf:RDF xmlns:rdf="{RDF}" xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#">
<rdf:Description rdf:about="https://example.org/ns#Drawing"><rdfs:subClassOf rdf:resource="{CT}InternalDocument"/></rdf:Description></rdf:RDF>"#
    );
    let (_, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        part1_icdd(None, &[("Ontology resources/drawings.rdf", ext.as_bytes())]),
    )
    .await;
    let r = json(&body);
    assert!(
        codes(&r, "violation").contains(&"part1-extension".to_string()),
        "{r:#}"
    );

    // An unlisted payload document, and a link with three elements.
    let three = format!(
        r#"<?xml version="1.0"?><rdf:RDF xmlns:rdf="{RDF}" xmlns:ls="{LS}">
<ls:BinaryLink rdf:about="urn:icdd:link:9">
  <ls:hasLinkElement><ls:LinkElement><ls:hasDocument rdf:resource="urn:icdd:doc:report"/></ls:LinkElement></ls:hasLinkElement>
  <ls:hasLinkElement><ls:LinkElement><ls:hasDocument rdf:resource="urn:icdd:doc:report-v1"/></ls:LinkElement></ls:hasLinkElement>
  <ls:hasLinkElement><ls:LinkElement><ls:hasDocument rdf:resource="urn:icdd:doc:elsewhere"/></ls:LinkElement></ls:hasLinkElement>
</ls:BinaryLink></rdf:RDF>"#
    );
    let mut zipped = part1_icdd(None, &[("Payload documents/stray.txt", b"?")]);
    // Swap the linkset for the three-element one.
    let entries: Vec<(String, Vec<u8>)> = {
        let mut a = zip::ZipArchive::new(Cursor::new(zipped.clone())).unwrap();
        (0..a.len())
            .map(|i| {
                let mut f = a.by_index(i).unwrap();
                let mut b = Vec::new();
                f.read_to_end(&mut b).unwrap();
                (f.name().to_string(), b)
            })
            .collect()
    };
    let swapped: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| {
            if n == "Payload triples/links.rdf" {
                (n.as_str(), three.as_bytes())
            } else {
                (n.as_str(), b.as_slice())
            }
        })
        .collect();
    zipped = zip_of(&swapped);
    let (_, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        zipped,
    )
    .await;
    let r = json(&body);
    assert!(
        codes(&r, "violation").contains(&"shape:BinaryLinkShape".to_string()),
        "{r:#}"
    );
    let w = codes(&r, "warning");
    assert!(w.contains(&"unlisted-document".to_string()), "{r:#}");
    assert!(w.contains(&"link-unknown-document".to_string()), "{r:#}");

    // Two container descriptions; a document missing from the archive; an
    // index that is not Index.rdf.
    let two = format!(
        r#"<?xml version="1.0"?><rdf:RDF xmlns:rdf="{RDF}" xmlns:ct="{CT}">
<ct:ContainerDescription rdf:about="urn:c:1"/><ct:ContainerDescription rdf:about="urn:c:2"/></rdf:RDF>"#
    );
    let (_, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        zip_of(&[("Index.rdf", two.as_bytes())]),
    )
    .await;
    let r = json(&body);
    let v = codes(&r, "violation");
    assert!(v.contains(&"container-description".to_string()), "{r:#}");
    let gone = format!(
        r#"<?xml version="1.0"?><rdf:RDF xmlns:rdf="{RDF}" xmlns:ct="{CT}">
<ct:ContainerDescription rdf:about="urn:c:1"><ct:containsDocument rdf:resource="urn:d:1"/></ct:ContainerDescription>
<ct:InternalDocument rdf:about="urn:d:1"><ct:filename>gone.pdf</ct:filename></ct:InternalDocument></rdf:RDF>"#
    );
    let (_, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        zip_of(&[("Index.rdf", gone.as_bytes())]),
    )
    .await;
    let r = json(&body);
    assert!(
        codes(&r, "violation").contains(&"document-missing".to_string()),
        "{r:#}"
    );
    let ttl = format!("<urn:c:1> a <{CT}ContainerDescription> .");
    let (_, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        zip_of(&[("index.ttl", ttl.as_bytes())]),
    )
    .await;
    assert!(codes(&json(&body), "violation").contains(&"index-name".to_string()));
    let (_, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        zip_of(&[("readme.txt", b"hi")]),
    )
    .await;
    assert!(codes(&json(&body), "violation").contains(&"index-missing".to_string()));

    // Guards: anonymous, not an archive, unknown profile.
    let (st, _, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        None,
        part1_icdd(None, &[]),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, _, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        b"nope".to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate?profile=bcf",
        Some(&token),
        part1_icdd(None, &[]),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn icdd_strict_import_refuses_nonconformant_containers() {
    let (state, token) = state_with_store(&["a"]);
    let app = test_app(state.clone());
    let assets =
        |s: &open_triplestore::server::AppState| s.auth_db.list_dataset_assets("a").unwrap().len();

    // strict=true: a checksum mismatch refuses the whole container.
    let (st, body, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import?strict=true",
        Some(&token),
        part1_icdd(Some(&"0".repeat(64)), &[]),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let r = json(&body);
    assert_eq!(r["validation"]["conforms"], false);
    assert!(codes(&r["validation"], "violation").contains(&"checksum-mismatch".to_string()));
    assert_eq!(assets(&state), 0, "nothing stored");
    assert!(state.auth_db.list_dataset_graphs("a").unwrap().is_empty());

    // Without strict the import goes ahead and flags the document.
    let (st, body, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        Some(&token),
        part1_icdd(Some(&"0".repeat(64)), &[]),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let r = json(&body);
    assert_eq!(r["validation"]["conforms"], false);
    let m = r["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["iri"] == "urn:icdd:doc:measurements")
        .unwrap();
    assert_eq!(m["checksum_status"], "mismatch", "{m:#}");

    // Several container descriptions are refused even without strict.
    let two = format!(
        r#"<?xml version="1.0"?><rdf:RDF xmlns:rdf="{RDF}" xmlns:ct="{CT}">
<ct:ContainerDescription rdf:about="urn:c:1"/><ct:ContainerDescription rdf:about="urn:c:2"/></rdf:RDF>"#
    );
    let (st, body, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        Some(&token),
        zip_of(&[("Index.rdf", two.as_bytes())]),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(String::from_utf8_lossy(&body).contains("exactly one"));

    // A Part 1 class extension under strict.
    let ext = format!(
        r#"<?xml version="1.0"?><rdf:RDF xmlns:rdf="{RDF}" xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#">
<rdf:Description rdf:about="https://example.org/ns#Drawing"><rdfs:subClassOf rdf:resource="{CT}InternalDocument"/></rdf:Description></rdf:RDF>"#
    );
    let (st, _, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import?strict=true",
        Some(&token),
        part1_icdd(None, &[("Ontology resources/drawings.rdf", ext.as_bytes())]),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    // Anonymous import is a 401, not a missing-extension 500.
    let (st, _, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        None,
        part1_icdd(None, &[]),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn icdd_part1_import_handles_every_document_kind() {
    let (state, token) = state_with_store(&["a"]);
    let app = test_app(state.clone());
    let (st, body, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import?strict=true",
        Some(&token),
        part1_icdd(None, &[]),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let r = json(&body);
    assert_eq!(r["validation"]["conforms"], true);
    assert_eq!(r["conformance"][0], "ICDD-Part1-Container");
    assert_eq!(r["version_id"], "2");
    assert_eq!(r["published_by"]["kind"], "organisation");
    assert_eq!(r["published_by"]["name"], "Example Road Authority");
    assert_eq!(r["created_by"]["kind"], "person");
    let docs = r["documents"].as_array().unwrap();
    let doc = |iri: &str| {
        docs.iter()
            .find(|d| d["iri"] == iri)
            .unwrap_or_else(|| panic!("{iri}: {r:#}"))
    };

    // Internal: name, versions, alternatives, creator, requested.
    let report = doc("urn:icdd:doc:report");
    assert_eq!(report["kind"], "internal");
    assert_eq!(report["name"], "Inspection report");
    assert_eq!(report["version_id"], "2");
    assert_eq!(report["version_description"], "After the second visit");
    assert_eq!(report["prior_version"], "urn:icdd:doc:report-v1");
    assert_eq!(report["alternatives"][0], "urn:icdd:doc:norm");
    assert_eq!(report["created_by"]["name"], "A. Inspector");
    assert_eq!(report["requested"], false);
    let report_asset = report["asset_id"].as_str().unwrap();
    // External.
    let norm = doc("urn:icdd:doc:norm");
    assert_eq!(norm["kind"], "external");
    assert_eq!(norm["external_url"], "https://example.org/norms/inspection");
    // Secured: verified.
    let m = doc("urn:icdd:doc:measurements");
    assert_eq!(m["kind"], "secured");
    assert_eq!(m["checksum_status"], "verified");
    // Encrypted: an opaque, flagged file.
    let sealed = doc("urn:icdd:doc:sealed");
    assert_eq!(sealed["kind"], "encrypted");
    assert_eq!(sealed["encrypted"], true);
    assert_eq!(sealed["encryption_algorithm"], "AES-256-GCM");
    // Folder: an asset sub-folder, nested folders kept.
    let drawings = doc("urn:icdd:doc:drawings");
    assert_eq!(drawings["kind"], "folder");
    let folder = drawings["asset_folder"].as_str().unwrap().to_string();
    assert!(
        folder.starts_with("containers/") && folder.ends_with("/drawings"),
        "{folder}"
    );
    assert_eq!(drawings["files"].as_array().unwrap().len(), 2);

    let assets = state.auth_db.list_dataset_assets("a").unwrap();
    let by_name = |n: &str| {
        assets
            .iter()
            .find(|a| a.filename == n)
            .unwrap_or_else(|| panic!("{n}"))
    };
    assert_eq!(by_name("plan.txt").folder, folder);
    assert_eq!(by_name("section.txt").folder, format!("{folder}/details"));
    assert!(by_name("report.txt").folder.ends_with("/reports"));
    assert_eq!(by_name("report.txt").id, report_asset);
    assert_eq!(
        by_name("report.txt").title.as_deref(),
        Some("Inspection report"),
        "ct:name becomes the asset title"
    );
    assert_eq!(
        by_name("sealed.bin").content_type,
        "application/octet-stream"
    );
    assert_eq!(
        assets.len(),
        6,
        "report, report-v1, plan, section, measurements, sealed"
    );
    let graphs = r["graphs"].as_array().unwrap();
    assert!(
        graphs
            .iter()
            .any(|g| g["role"] == "linkset" && g["triples"].as_u64().unwrap() > 0),
        "{r:#}"
    );

    // ISO's ontology files in a container are recognised, not loaded as data.
    let stand_in = format!(
        r#"<?xml version="1.0"?><rdf:RDF xmlns:rdf="{RDF}" xmlns:owl="http://www.w3.org/2002/07/owl#"><owl:Ontology rdf:about="https://example.org/stand-in"/></rdf:RDF>"#
    );
    let (st, body, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        Some(&token),
        part1_icdd(
            None,
            &[
                ("Ontology resources/Container.rdf", stand_in.as_bytes()),
                ("Ontology resources/Linkset.rdf", stand_in.as_bytes()),
            ],
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let r = json(&body);
    assert_eq!(
        r["standard_ontologies"].as_array().unwrap().len(),
        2,
        "{r:#}"
    );
    assert!(
        r["graphs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|g| g["role"] != "model"),
        "{r:#}"
    );
    assert_eq!(r["validation"]["warnings"], 0, "{r:#}");
}

#[tokio::test]
async fn icdd_export_validates_and_round_trips_index_metadata() {
    let (state, token) = state_with_store(&["a", "b"]);
    let app = test_app(state.clone());
    let (st, body, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import?strict=true",
        Some(&token),
        part1_icdd(None, &[]),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );

    let (st, zip_bytes, h) = send_h(
        &app,
        Method::GET,
        "/api/datasets/a/containers/export",
        Some(&token),
        Vec::new(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(h[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .contains("a.icdd"));
    assert_eq!(h["x-container-conforms"], "true", "{h:?}");
    assert!(
        h["x-container-validation"]
            .to_str()
            .unwrap()
            .starts_with("violations=0"),
        "{h:?}"
    );

    // The export passes the validator: no violation.
    let (st, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        zip_bytes.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let rep = json(&body);
    assert_eq!(rep["violations"], 0, "{rep:#}");
    assert!(
        codes(&rep, "warning")
            .iter()
            .all(|c| c == "ontology-resource-missing"),
        "{rep:#}"
    );

    let (names, index) = archive_index(&zip_bytes);
    for want in [
        "Payload documents/reports/report.txt",
        "Payload documents/reports/report-v1.txt",
        "Payload documents/drawings/plan.txt",
        "Payload documents/drawings/details/section.txt",
        "Payload documents/measurements.csv",
        "Payload documents/sealed.bin",
    ] {
        assert!(names.contains(&want.to_string()), "{want}: {names:?}");
    }
    assert!(
        names
            .iter()
            .any(|n| n.starts_with("Payload triples/") && n.ends_with(".rdf")),
        "{names:?}"
    );
    // The imported metadata goes back out under the documents' own IRIs.
    let ask = |b: &str| index_ask(&index, b);
    assert!(ask(&format!("<urn:icdd:doc:report> a <{CT}InternalDocument> ; <{CT}name> \"Inspection report\" ; <{CT}filename> \"reports/report.txt\" ; <{CT}versionID> \"2\" ; <{CT}versionDescription> \"After the second visit\" ; <{CT}priorVersion> <urn:icdd:doc:report-v1> ; <{CT}alternativeDocument> <urn:icdd:doc:norm> ; <{CT}createdBy> <urn:party:inspector> . <urn:party:inspector> a <{CT}Person> ; <{CT}name> \"A. Inspector\"")), "{index}");
    assert!(ask(&format!("<urn:icdd:doc:norm> a <{CT}ExternalDocument> ; <{CT}name> \"Inspection norm\" ; <{CT}url> \"https://example.org/norms/inspection\"")), "{index}");
    assert!(ask(&format!("<urn:icdd:doc:drawings> a <{CT}FolderDocument> ; <{CT}foldername> \"drawings\" ; <{CT}name> \"Drawings\"")), "{index}");
    assert!(ask(&format!("<urn:icdd:doc:measurements> a <{CT}SecuredDocument> ; <{CT}checksum> \"{}\" ; <{CT}checksumAlgorithm> \"SHA-256\"", sha256_hex(MEASUREMENTS))), "{index}");
    assert!(ask(&format!("<urn:icdd:doc:sealed> a <{CT}EncryptedDocument> ; <{CT}encryptionAlgorithm> \"AES-256-GCM\"")), "{index}");
    assert!(ask(&format!("?c a <{CT}ContainerDescription> ; <{CT}conformanceIndicator> \"ICDD-Part1-Container\" ; <{CT}publishedBy> ?p ; <{CT}creationDate> ?d . ?p a <{CT}Person> ; <{CT}name> \"admin\"")), "{index}");
    assert!(ask("?o a <http://www.w3.org/2002/07/owl#Ontology> ; <http://www.w3.org/2002/07/owl#imports> <https://standards.iso.org/iso/21597/-1/ed-1/en/Container>"), "{index}");

    // And imports strictly into another dataset with the same documents.
    let (st, body, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/b/containers/import?strict=true",
        Some(&token),
        zip_bytes,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let r = json(&body);
    let kinds: Vec<&str> = r["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap())
        .collect();
    for k in ["internal", "external", "folder", "secured", "encrypted"] {
        assert!(kinds.contains(&k), "{k}: {r:#}");
    }
    assert_eq!(state.auth_db.list_dataset_assets("b").unwrap().len(), 6);
    let m = r["documents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["iri"] == "urn:icdd:doc:measurements")
        .unwrap();
    assert_eq!(m["checksum_status"], "verified");
}

#[tokio::test]
async fn icdd_export_keeps_folder_paths_and_never_collides() {
    let (state, token) = state_with_store(&["a"]);
    // Three assets named report.txt: root, folder a/, folder b/.
    for (i, folder) in ["", "a", "b"].iter().enumerate() {
        let id = format!("asset-{i}");
        let key = format!("datasets/a/{id}/report.txt");
        state
            .object_store
            .upload(
                &key,
                axum::body::Bytes::from(format!("copy {i}\n")),
                "text/plain",
            )
            .await
            .unwrap();
        state
            .auth_db
            .create_asset(
                &id,
                "a",
                "report.txt",
                "text/plain",
                &key,
                7,
                "adm",
                false,
                folder,
            )
            .unwrap();
    }
    let app = test_app(state.clone());
    // Twice the same container: both imports name their document report.txt.
    for _ in 0..2 {
        let (st, _, _) = send_h(
            &app,
            Method::POST,
            "/api/datasets/a/containers/import",
            Some(&token),
            sample_icdd(),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
    }
    let (st, zip_bytes, h) = send_h(
        &app,
        Method::GET,
        "/api/datasets/a/containers/export",
        Some(&token),
        Vec::new(),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&zip_bytes)
    );
    assert_eq!(h["x-container-conforms"], "true", "{h:?}");
    let (names, index) = archive_index(&zip_bytes);
    for want in [
        "Payload documents/report.txt",
        "Payload documents/a/report.txt",
        "Payload documents/b/report.txt",
    ] {
        assert!(names.contains(&want.to_string()), "{want}: {names:?}");
    }
    let files: Vec<&String> = names.iter().filter(|n| !n.ends_with('/')).collect();
    let unique: std::collections::HashSet<String> =
        files.iter().map(|n| n.to_ascii_lowercase()).collect();
    assert_eq!(unique.len(), files.len(), "no duplicate entries: {names:?}");
    let (_, body, _) = send_h(
        &app,
        Method::POST,
        "/api/containers/validate",
        Some(&token),
        zip_bytes,
    )
    .await;
    let rep = json(&body);
    assert_eq!(rep["violations"], 0, "{rep:#}\n{index}");
}

#[tokio::test]
async fn icdd_export_embeds_operator_ontology_files() {
    let (state, token) = state_with_store(&["a"]);
    let app = test_app(state.clone());
    let (st, _, _) = send_h(
        &app,
        Method::POST,
        "/api/datasets/a/containers/import",
        Some(&token),
        part1_icdd(None, &[]),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // Not configured: the export says it is not Part 1-conformant.
    std::env::remove_var("OTS_ICDD_ONTOLOGY_DIR");
    let (_, _, h) = send_h(
        &app,
        Method::GET,
        "/api/datasets/a/containers/export",
        Some(&token),
        Vec::new(),
    )
    .await;
    assert!(
        h["x-container-findings"]
            .to_str()
            .unwrap()
            .contains("ontology-resource-missing"),
        "{h:?}"
    );

    // Configured with stand-in files (this repository does not carry ISO's):
    // they are embedded byte for byte and the warning is gone.
    let dir = std::env::temp_dir().join(format!("ots-icdd-onto-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let stand_in = |n: &str| {
        format!(
            r#"<?xml version="1.0"?><rdf:RDF xmlns:rdf="{RDF}" xmlns:owl="http://www.w3.org/2002/07/owl#"><owl:Ontology rdf:about="https://example.org/stand-in/{n}"/></rdf:RDF>"#
        )
    };
    std::fs::write(dir.join("Container.rdf"), stand_in("Container")).unwrap();
    std::fs::write(dir.join("Linkset.rdf"), stand_in("Linkset")).unwrap();
    std::env::set_var("OTS_ICDD_ONTOLOGY_DIR", &dir);
    let (st, zip_bytes, h) = send_h(
        &app,
        Method::GET,
        "/api/datasets/a/containers/export",
        Some(&token),
        Vec::new(),
    )
    .await;
    std::env::remove_var("OTS_ICDD_ONTOLOGY_DIR");
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        h["x-container-validation"], "violations=0; warnings=0",
        "{h:?}"
    );
    assert!(h.get("x-container-findings").is_none(), "{h:?}");
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
    let mut got = String::new();
    archive
        .by_name("Ontology resources/Container.rdf")
        .unwrap()
        .read_to_string(&mut got)
        .unwrap();
    assert_eq!(got, stand_in("Container"));
    assert!(archive.by_name("Ontology resources/Linkset.rdf").is_ok());
}
