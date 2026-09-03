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
    assert_eq!(
        links_iri, "urn:icdd:linkset:main",
        "the linkset keeps its index IRI"
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
    assert!(matches!(tmp.query(&format!("ASK {{ GRAPH <urn:idx> {{ ?c a <{CT}ContainerDescription> ; <{CT}conformanceIndicator> \"ICDD-Part1-Container\" ; <{CT}containsLinkset> <urn:icdd:linkset:main> . <urn:icdd:linkset:main> <{CT}filename> ?f }} }}")), Ok(QueryResults::Boolean(true))), "the linkset keeps its IRI in the exported index: {index}");

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
