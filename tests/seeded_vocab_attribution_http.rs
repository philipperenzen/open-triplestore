//! The bundled vocabularies the server seeds as public reference models carry
//! their licence and attribution over `/api/models`: as a registry record on
//! every entry and version, in the `Link` headers and comment preamble of every
//! download, and through `/vocab/NOTICE.md` — while the stored graphs stay
//! exactly the bundled files' triples. A download calls its content the
//! bundled file, unchanged, only for a copy the seeder checked; drafts,
//! branches, merges, rebases and edited copies keep the licence record and say
//! they may have been modified. IMBOR (no altered copies) gets no added notice,
//! and its entry refuses every way of making or publishing other content: an
//! upload, an edit, a draft, a branch, a merge, a rebase or a publish.
//!
//! Drives the real Axum router via `tower::ServiceExt::oneshot`.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::data_models::{registry, seed_vocab, upload, vocab_files};
use tower::ServiceExt as _;

const W3C_DL: &str = "https://www.w3.org/copyright/document-license-2023/";

async fn get(state: &open_triplestore::server::AppState, uri: &str) -> axum::response::Response {
    test_app(state.clone())
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn links(resp: &axum::response::Response) -> Vec<String> {
    resp.headers()
        .get_all(header::LINK)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

/// Triples in a version graph.
fn graph_size(state: &open_triplestore::server::AppState, graph: &str) -> usize {
    let q = format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}");
    match state.store.query(&q) {
        Ok(oxigraph::sparql::QueryResults::Solutions(mut rows)) => {
            let row = rows.next().unwrap().unwrap();
            match row.get("n") {
                Some(oxigraph::model::Term::Literal(l)) => l.value().parse().unwrap(),
                other => panic!("unexpected count {other:?}"),
            }
        }
        _ => panic!("count query failed"),
    }
}

#[tokio::test]
async fn seeded_vocabularies_serve_their_licence_record() {
    let (state, token) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);
    let base = state.base_url.to_string();

    // ── The entry: licence, copyright, notice, status, source, NOTICE link ──
    let resp = get(&state, "/api/models/rdf").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let rdf = body_json(resp.into_body()).await;
    let a = &rdf["attribution"];
    assert_eq!(a["file"], "rdf.ttl");
    assert_eq!(a["licenses"][0]["uri"], W3C_DL);
    assert_eq!(a["licenses"][0]["name"], "W3C Document License (2023)");
    assert!(a["copyright"][0]
        .as_str()
        .unwrap()
        .starts_with("Copyright © 2019 World Wide Web Consortium."));
    assert!(a["notice"]
        .as_str()
        .unwrap()
        .starts_with("Copyright © 2023 W3C®. This software or document includes material"));
    assert!(a["status"]
        .as_str()
        .unwrap()
        .contains("W3C Recommendation 25 February 2014"));
    assert_eq!(
        a["source_url"],
        "https://www.w3.org/1999/02/22-rdf-syntax-ns.ttl"
    );
    assert_eq!(a["notice_url"], format!("{base}/vocab/NOTICE.md"));
    assert!(a["header"]
        .as_str()
        .unwrap()
        .starts_with("The RDF Concepts Vocabulary (RDF)"));
    // The record describes the model's latest version's file.
    assert_eq!(
        a["specification_url"],
        "https://www.w3.org/TR/rdf11-concepts/"
    );

    // ── The list: every seeded entry has one; IMBOR is no-derivatives ──
    let list = body_json(get(&state, "/api/models").await.into_body()).await;
    let list = list.as_array().expect("array of entries");
    let imbor = list.iter().find(|m| m["id"] == "imbor").expect("imbor");
    assert_eq!(imbor["attribution"]["no_derivatives"], true);
    assert_eq!(imbor["attribution"]["copyright"][0], "© Stichting CROW");
    let uris: Vec<&str> = imbor["attribution"]["licenses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["uri"].as_str().unwrap())
        .collect();
    assert_eq!(
        uris,
        [
            "https://creativecommons.org/licenses/by/4.0/",
            "https://opendatacommons.org/licenses/by/1-0/"
        ]
    );
    let ots = list.iter().find(|m| m["id"] == "ots").expect("ots");
    assert!(
        ots["attribution"].is_null(),
        "Open Triplestore's own vocabulary needs no third-party notice"
    );

    // ── Versions: each carries its own file's record and specification ──
    let versions = body_json(get(&state, "/api/models/dcat/versions").await.into_body()).await;
    let by_version = |v: &str| {
        versions
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["version"] == v)
            .cloned()
            .unwrap()
    };
    assert_eq!(by_version("2.0")["attribution"]["file"], "dcat/2.0.0.ttl");
    assert_eq!(by_version("3.0")["attribution"]["file"], "dcat.ttl");
    assert_eq!(
        by_version("1.0")["attribution"]["specification_url"],
        "https://www.w3.org/TR/2014/REC-vocab-dcat-20140116/"
    );
    let one = body_json(
        get(&state, "/api/models/schema/versions/29.0")
            .await
            .into_body(),
    )
    .await;
    assert_eq!(
        one["attribution"]["licenses"][0]["uri"],
        "http://creativecommons.org/licenses/by-sa/3.0/"
    );
    assert_eq!(
        one["notes"],
        "Schema.org subset, adapted from release 30.0 (CC BY-SA 3.0)"
    );

    // ── Download: Link headers + the file's header as a comment preamble ──
    let resp = get(&state, "/api/models/rdf/versions/1.1/data?format=turtle").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let l = links(&resp);
    assert!(l.contains(&format!("<{W3C_DL}>; rel=\"license\"")), "{l:?}");
    assert!(l.contains(&format!("<{base}/vocab/NOTICE.md>; rel=\"describedby\"")));
    let body = body_text(resp.into_body()).await;
    assert!(
        body.starts_with("# The RDF Concepts Vocabulary (RDF)"),
        "download starts with the file's header: {}",
        &body[..200.min(body.len())]
    );
    assert!(body.contains(&format!(
        "# Full attribution and licence texts: {base}/vocab/NOTICE.md"
    )));
    let served = upload::parse_rdf(body.as_bytes(), "text/turtle", "rdf.ttl").unwrap();
    let bundled =
        upload::parse_rdf(vocab_files::RDF.ttl.as_bytes(), "text/turtle", "rdf.ttl").unwrap();
    assert_eq!(
        served.len(),
        bundled.len(),
        "the download holds exactly the bundled file's triples"
    );

    // IMBOR: Link headers only — no notice written into the copy.
    let resp = get(&state, "/api/models/imbor/latest/data?format=nt").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(links(&resp)
        .iter()
        .any(|l| l.contains("https://opendatacommons.org/licenses/by/1-0/")));
    let body = body_text(resp.into_body()).await;
    assert!(!body.starts_with('#'), "IMBOR gets no added notice");

    // Term dereference names the licence too.
    let resp = get(
        &state,
        "/api/models/skos/term?iri=http%3A%2F%2Fwww.w3.org%2F2004%2F02%2Fskos%2Fcore%23Concept",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(links(&resp).iter().any(|l| l.contains(W3C_DL)));

    // ── The notice itself, served by the API server ──
    let resp = get(&state, "/vocab/NOTICE.md").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/plain"));
    let text = body_text(resp.into_body()).await;
    assert!(text.contains("IMBOR 2025 Vocabulaire"));

    // ── Drafts: copies inherit the record; IMBOR cannot be copied ──
    let post = |uri: String, body: serde_json::Value| {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let resp = test_app(state.clone())
        .oneshot(post(
            "/api/models/imbor/versions/2025/draft".into(),
            serde_json::json!({ "target_version": "2025-edit" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(!registry::version_exists(
        &state.store,
        &base,
        "imbor",
        "2025-edit"
    ));

    let resp = test_app(state.clone())
        .oneshot(post(
            "/api/models/rdf/versions/1.1/draft".into(),
            serde_json::json!({ "target_version": "1.1.1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let draft = body_json(
        get(&state, "/api/models/rdf/versions/1.1.1")
            .await
            .into_body(),
    )
    .await;
    assert_eq!(draft["attribution"]["licenses"][0]["uri"], W3C_DL);
    assert!(draft["attribution"]["stored_copy"]
        .as_str()
        .unwrap()
        .starts_with("Copied in this registry from version 1.1"));
}

/// The seeded graphs hold exactly the bundled files' triples — nothing added,
/// including for files that state no `owl:versionInfo` (the loader used to add
/// one) and for IMBOR.
#[test]
fn seeded_graphs_hold_exactly_the_bundled_triples() {
    let state = test_state();
    seed_vocab::seed_standard_vocabularies(&state);
    let base = state.base_url.to_string();
    for (id, version, file) in [
        ("rdf", "1.1", &vocab_files::RDF),
        ("skos", "2009-08-18", &vocab_files::SKOS),
        ("foaf", "0.99", &vocab_files::FOAF),
        ("imbor", "2025", &vocab_files::IMBOR),
    ] {
        let bundled = upload::parse_rdf(file.ttl.as_bytes(), "text/turtle", file.path).unwrap();
        let graph = registry::version_record_iri(&base, id, version);
        assert_eq!(
            graph_size(&state, &graph),
            bundled.len(),
            "{id} {version}: stored graph differs from vocab/{}",
            file.path
        );
    }
}

/// A user's own model carries no licence record.
#[tokio::test]
async fn user_models_have_no_attribution() {
    let (state, token) = admin_state();
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/models")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    serde_json::json!({ "title": "Mine", "namespace": "http://ex.org/mine#" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let created = body_json(resp.into_body()).await;
    assert_eq!(created["id"], "mine");
    assert!(created.get("attribution").is_some_and(|a| a.is_null()));
}

/// The OpenAPI document describes the licence record and the typed responses.
#[test]
fn openapi_documents_the_licence_record() {
    let v = serde_json::to_value(open_triplestore::server::openapi::openapi_spec()).unwrap();
    let schemas = &v["components"]["schemas"];
    for name in [
        "ContentAttribution",
        "LicenseRef",
        "DataModelResponse",
        "DataModelVersionResponse",
    ] {
        assert!(schemas[name].is_object(), "schema {name} missing");
    }
    let props = &schemas["ContentAttribution"]["properties"];
    for field in [
        "licenses",
        "copyright",
        "notice",
        "status",
        "source_url",
        "notice_url",
        "no_derivatives",
        "unchanged",
    ] {
        assert!(
            props[field].is_object(),
            "ContentAttribution.{field} missing"
        );
    }
    let get_entry = &v["paths"]["/api/models/{id}"]["get"]["responses"]["200"];
    assert_eq!(
        get_entry["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/DataModelResponse"
    );
    assert!(v["paths"]["/vocab/NOTICE.md"]["get"].is_object());
}

fn json_request(
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    b.body(Body::from(body.to_string())).unwrap()
}

async fn send(
    state: &open_triplestore::server::AppState,
    req: Request<Body>,
) -> axum::response::Response {
    test_app(state.clone()).oneshot(req).await.unwrap()
}

async fn get_as(
    state: &open_triplestore::server::AppState,
    uri: &str,
    token: &str,
) -> axum::response::Response {
    send(
        state,
        Request::builder()
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

/// The comment line a download's preamble ends its notice with.
fn served_line(body: &str) -> String {
    body.lines()
        .find(|l| l.starts_with("# ---- Served by Open Triplestore"))
        .unwrap_or_default()
        .to_string()
}

/// IMBOR's licence allows no altered copies. Every way of making or publishing
/// content in its entry is refused, merge and rebase included, and a copy an
/// earlier release let an admin make there is kept but not served publicly.
#[tokio::test]
async fn the_imbor_entry_refuses_every_way_to_alter_it() {
    let (state, token) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);
    let base = state.base_url.to_string();
    let tok = Some(token.as_str());

    // The seeded copy is checked and served to everyone.
    let v = body_json(
        get(&state, "/api/models/imbor/versions/2025")
            .await
            .into_body(),
    )
    .await;
    assert_eq!(v["attribution"]["unchanged"], true);
    assert_eq!(v["attribution"]["no_derivatives"], true);
    assert_eq!(
        get(&state, "/api/models/imbor/versions/2025/data?format=nt")
            .await
            .status(),
        StatusCode::OK
    );

    // A merge of a version into itself is no merge.
    let resp = send(
        &state,
        json_request(
            Method::POST,
            "/api/models/imbor/merge",
            tok,
            serde_json::json!({ "from": "2025", "into": "2025" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(!registry::version_exists(
        &state.store,
        &base,
        "imbor",
        "2025-merge-2025"
    ));

    // A branch copy an earlier release let an admin make in the entry.
    let ours = "2025-ours";
    let graphs = upload::clone_graphs_as_draft(&state.store, &base, "imbor", "2025", ours).unwrap();
    registry::insert_version(
        &state.store,
        &base,
        &open_triplestore::data_models::models::DataModelVersion {
            data_model_id: "imbor".into(),
            version: ours.into(),
            status: open_triplestore::data_models::models::VersionStatus::Draft,
            graph_iri: registry::version_record_iri(&base, "imbor", ours),
            sub_graphs: graphs,
            created_at: "2026-01-01T00:00:00Z".into(),
            created_by: Some(format!("{base}/users/adm")),
            derived_from: Some("2025".into()),
            notes: None,
            branch: Some("ours".into()),
            sub_graph_status: Vec::new(),
        },
    )
    .unwrap();

    let refused: Vec<(Method, String, serde_json::Value)> = vec![
        (
            Method::POST,
            "/api/models/imbor/merge".into(),
            serde_json::json!({ "from": ours, "into": "2025" }),
        ),
        (
            Method::POST,
            "/api/models/imbor/merge".into(),
            serde_json::json!({ "from": "2025", "into": ours, "branch": "x" }),
        ),
        (
            Method::POST,
            format!("/api/models/imbor/versions/{ours}/rebase"),
            serde_json::json!({ "onto": "2025" }),
        ),
        (
            Method::POST,
            "/api/models/imbor/branches".into(),
            serde_json::json!({ "branch": "b", "from_version": "2025" }),
        ),
        (
            Method::POST,
            format!("/api/models/imbor/versions/{ours}/draft"),
            serde_json::json!({ "target_version": "2025-copy" }),
        ),
        (
            Method::PATCH,
            format!("/api/models/imbor/versions/{ours}/data"),
            serde_json::json!({
                "add": [{ "s": "https://data.crow.nl/imbor/term/x",
                          "p": "http://www.w3.org/2000/01/rdf-schema#label",
                          "o": { "value": "ours" } }],
                "remove": []
            }),
        ),
        (
            Method::PATCH,
            "/api/models/imbor/versions/2025/data".into(),
            serde_json::json!({ "add": [], "remove": [] }),
        ),
        (
            Method::POST,
            format!("/api/models/imbor/versions/{ours}/publish"),
            serde_json::json!({}),
        ),
    ];
    for (method, uri, body) in refused {
        let resp = send(&state, json_request(method.clone(), &uri, tok, body)).await;
        let status = resp.status();
        let text = body_text(resp.into_body()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}: {text}");
        assert!(
            text.contains("allows no altered copies"),
            "{method} {uri}: {text}"
        );
    }

    // An upload into the entry is refused too.
    let boundary = "XIMBORX";
    let upload_body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"version\"\r\n\r\n2026\r\n\
         --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"imbor.ttl\"\r\n\
         Content-Type: text/turtle\r\n\r\n\
         <https://data.crow.nl/imbor/term/x> <http://www.w3.org/2000/01/rdf-schema#label> \"x\" .\r\n\
         --{boundary}--\r\n"
    );
    let resp = send(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/models/imbor/versions")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(upload_body))
            .unwrap(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Nothing was created, and the seeded copy is still the latest.
    let versions = body_json(get(&state, "/api/models/imbor/versions").await.into_body()).await;
    let labels: Vec<&str> = versions
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["version"].as_str().unwrap())
        .collect();
    assert_eq!(labels.len(), 2, "{labels:?}");
    let entry = body_json(get(&state, "/api/models/imbor").await.into_body()).await;
    assert_eq!(entry["latest_published"], "2025");

    // The earlier copy is kept, but only users who may write the entry get it.
    for uri in [
        format!("/api/models/imbor/versions/{ours}/data"),
        format!("/api/models/imbor/diff?from=2025&to={ours}"),
        format!("/api/models/imbor/merge/preview?from={ours}&into=2025"),
        format!(
            "/api/models/imbor/term?version={ours}&iri=https%3A%2F%2Fdata.crow.nl%2Fimbor%2Fterm%2F74e825e1-9b93-4dd5-8fab-52783fdb758b"
        ),
    ] {
        assert_eq!(get(&state, &uri).await.status(), StatusCode::FORBIDDEN, "{uri}");
        assert_eq!(get_as(&state, &uri, &token).await.status(), StatusCode::OK, "{uri}");
    }
}

/// Only a copy the seeder checked is called the bundled file, unchanged. A
/// draft, a branch, a merge, a rebase or an in-place edit keeps the licence
/// record, and its downloads say it may have been modified.
#[tokio::test]
async fn copies_keep_their_licence_and_are_never_called_unchanged() {
    let (state, token) = admin_state();
    seed_vocab::seed_standard_vocabularies(&state);
    let tok = Some(token.as_str());

    // The seeded copy: unchanged.
    let body = body_text(
        get(&state, "/api/models/rdf/versions/1.1/data?format=turtle")
            .await
            .into_body(),
    )
    .await;
    assert!(served_line(&body).contains("vocab/rdf.ttl, unchanged"));

    // A draft of it.
    let resp = send(
        &state,
        json_request(
            Method::POST,
            "/api/models/rdf/versions/1.1/draft",
            tok,
            serde_json::json!({ "target_version": "1.1.1" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = get(&state, "/api/models/rdf/versions/1.1.1/data?format=turtle").await;
    assert!(links(&resp).iter().any(|l| l.contains(W3C_DL)));
    let body = body_text(resp.into_body()).await;
    let line = served_line(&body);
    assert!(!line.contains("unchanged"), "{line}");
    assert!(line.contains("may have been modified"), "{line}");
    assert!(body.starts_with("# The RDF Concepts Vocabulary (RDF)"));

    // A branch, a merge and a rebase carry the record too.
    let resp = send(
        &state,
        json_request(
            Method::POST,
            "/api/models/rdf/branches",
            tok,
            serde_json::json!({ "branch": "b", "from_version": "1.1" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send(
        &state,
        json_request(
            Method::POST,
            "/api/models/rdf/merge",
            tok,
            serde_json::json!({ "from": "1.1-b", "into": "1.1" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp = send(
        &state,
        json_request(
            Method::POST,
            "/api/models/rdf/versions/1.1-b/rebase",
            tok,
            serde_json::json!({ "onto": "1.0" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED);
    for (ver, how) in [
        ("1.1-b", "Copied in this registry from version 1.1"),
        (
            "1.1-merge-1.1-b",
            "Merged in this registry from version 1.1-b into version 1.1",
        ),
        (
            "1.1-b-rebase-1.0",
            "Rebased in this registry: version 1.1-b onto version 1.0",
        ),
    ] {
        let v = body_json(
            get(&state, &format!("/api/models/rdf/versions/{ver}"))
                .await
                .into_body(),
        )
        .await;
        let a = &v["attribution"];
        assert_eq!(a["licenses"][0]["uri"], W3C_DL, "{ver}");
        assert_eq!(a["unchanged"], false, "{ver}");
        assert!(
            a["stored_copy"].as_str().unwrap().starts_with(how),
            "{ver}: {a}"
        );
        let resp = get(
            &state,
            &format!("/api/models/rdf/versions/{ver}/data?format=turtle"),
        )
        .await;
        assert!(links(&resp).iter().any(|l| l.contains(W3C_DL)), "{ver}");
        let body = body_text(resp.into_body()).await;
        assert!(
            served_line(&body).contains("may have been modified"),
            "{ver}"
        );
    }

    // The seeded RDF 1.2 is a Draft, so it can be edited in place: its record
    // then says it may have been modified, and the next boot keeps both the
    // edit and the label.
    let resp = send(
        &state,
        json_request(
            Method::PATCH,
            "/api/models/rdf/versions/1.2/data",
            tok,
            serde_json::json!({
                "add": [{ "s": "http://ex.org/ours",
                          "p": "http://www.w3.org/2000/01/rdf-schema#label",
                          "o": { "value": "ours" } }],
                "remove": []
            }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    seed_vocab::seed_standard_vocabularies(&state);
    let v = body_json(
        get(&state, "/api/models/rdf/versions/1.2")
            .await
            .into_body(),
    )
    .await;
    assert_eq!(v["attribution"]["unchanged"], false);
    let body = body_text(
        get(&state, "/api/models/rdf/versions/1.2/data?format=turtle")
            .await
            .into_body(),
    )
    .await;
    assert!(body.contains("http://ex.org/ours"), "the edit is kept");
    assert!(served_line(&body).contains("may have been modified"));

    // Published as the latest, an edited copy is what the entry describes.
    let resp = send(
        &state,
        json_request(
            Method::POST,
            "/api/models/rdf/versions/1.1.1/publish",
            tok,
            serde_json::json!({}),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let entry = body_json(get(&state, "/api/models/rdf").await.into_body()).await;
    assert_eq!(entry["latest_published"], "1.1.1");
    assert_eq!(entry["attribution"]["unchanged"], false);
    assert!(!entry["attribution"]["stored_copy"]
        .as_str()
        .unwrap()
        .contains("unchanged"));
    let list = body_json(get(&state, "/api/models").await.into_body()).await;
    let rdf = list
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "rdf")
        .unwrap();
    assert_eq!(rdf["attribution"]["unchanged"], false);
}
