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
    <ct:description>Handover of the Waalbrug inspection</ct:description>
    <ct:conformanceIndicator>ICDD-Part1-Container</ct:conformanceIndicator>
    <ct:publishedBy><ct:Party rdf:about="urn:party:rws"><ct:name>Rijkswaterstaat</ct:name></ct:Party></ct:publishedBy>
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
    let data = "<urn:asset:waalbrug> a <urn:Bridge> ; <urn:span> 244 .\n";
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
    assert_eq!(r["description"], "Handover of the Waalbrug inspection");
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
        "ASK {{ GRAPH <{data_iri}> {{ <urn:asset:waalbrug> <urn:span> 244 }} }}"
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
    assert!(
        names
            .iter()
            .filter(|n| n.starts_with("Payload triples/") && n.ends_with(".ttl"))
            .count()
            >= 2,
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
            "ASK {{ GRAPH <{data_b}> {{ <urn:asset:waalbrug> <urn:span> 244 }} }}"
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
