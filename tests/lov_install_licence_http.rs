//! A vocabulary installed from the LOV corpus carries its licence over
//! `/api/models`, as the bundled vocabularies do: a licence record on the
//! version, `rel="license"` links on downloads, a served licence page at
//! `/api/vocab/notice` — and one whose licence allows only unaltered copies
//! (CC BY-ND) cannot be copied into an editable draft.
//!
//! Installs made by earlier releases are made private when this project may
//! not serve them publicly and get a licence record, with their graphs and
//! notes left as they are; a user's model that copies their note is not
//! taken for one.
//!
//! Drives the real Axum router via `tower::ServiceExt::oneshot`.

mod common;

use std::io::Write as _;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::{registry, upload};
use open_triplestore::server::AppState;
use open_triplestore::vocab_search::install;
use tower::ServiceExt as _;

/// The note every earlier release's installer wrote.
const EARLIER_NOTE: &str =
    "Installed from the bundled LOV corpus (snapshot 2025-12-18, CC BY 4.0).";

/// Two quads in the named graph `graph`.
fn two_quads(graph: &str) -> String {
    format!(
        "<{graph}#Thing> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
         <http://www.w3.org/2002/07/owl#Class> <{graph}> .\n\
         <{graph}#Thing> <http://www.w3.org/2000/01/rdf-schema#label> \"Thing\"@en <{graph}> .\n"
    )
}

/// A gzipped N-Quads corpus holding `graphs`, under a fresh temp directory.
fn corpus(graphs: &[&str]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ots-lov-install-licence-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lov.nq.gz");
    let mut enc =
        flate2::write::GzEncoder::new(std::fs::File::create(&path).unwrap(), Default::default());
    for g in graphs {
        enc.write_all(two_quads(g).as_bytes()).unwrap();
    }
    enc.finish().unwrap();
    path
}

fn request(
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    match body {
        Some(json) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

#[tokio::test]
async fn lov_installs_carry_their_licence_and_no_derivatives_is_enforced() {
    let (state, token) = admin_state();
    let base = state.base_url.to_string();
    let pna_uri = state
        .vocab_catalog
        .lov_by_prefix("pna")
        .expect("pna")
        .uri
        .clone();
    let gr_uri = state
        .vocab_catalog
        .lov_by_prefix("gr")
        .expect("gr")
        .uri
        .clone();
    *state.vocab_corpus.write().unwrap() = Some(corpus(&[&pna_uri, &gr_uri]));
    let app = || test_app(state.clone());

    // Install both: pna is CC BY-ND 3.0, gr CC BY 3.0.
    let mut versions = std::collections::HashMap::new();
    for prefix in ["pna", "gr"] {
        let resp = app()
            .oneshot(request(
                Method::POST,
                "/api/vocab/install",
                Some(&token),
                Some(serde_json::json!({ "vocab": prefix })),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{prefix}");
        let out = body_json(resp.into_body()).await;
        assert_eq!(out["is_public"], true, "{prefix}");
        assert_eq!(out["no_derivatives"], prefix == "pna", "{prefix}");
        versions.insert(prefix, out["version"].as_str().unwrap().to_string());
    }
    let pna_ver = &versions["pna"];

    // The version's licence record, served with an absolute notice link.
    let resp = app()
        .oneshot(request(Method::GET, "/api/models/pna/versions", None, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let list = body_json(resp.into_body()).await;
    let a = &list.as_array().unwrap()[0]["attribution"];
    assert_eq!(a["no_derivatives"], true);
    assert_eq!(a["unchanged"], true);
    assert_eq!(a["licenses"][0]["name"], "CC BY-ND 3.0");
    assert_eq!(
        a["licenses"][0]["uri"],
        "https://creativecommons.org/licenses/by-nd/3.0/"
    );
    assert_eq!(
        a["notice_url"],
        format!("{base}/api/vocab/notice?vocab=pna")
    );

    // Anonymous downloads carry the licence in a Link header and no added
    // notice (the licence allows only unaltered copies).
    let resp = app()
        .oneshot(request(
            Method::GET,
            &format!("/api/models/pna/versions/{pna_ver}/data?format=nt"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let links: Vec<String> = resp
        .headers()
        .get_all(header::LINK)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(
        links.contains(
            &"<https://creativecommons.org/licenses/by-nd/3.0/>; rel=\"license\"".to_string()
        ),
        "{links:?}"
    );
    let body = body_text(resp.into_body()).await;
    assert!(
        !body.starts_with('#'),
        "no preamble on a no-derivatives copy"
    );

    // No editable copy of it.
    let resp = app()
        .oneshot(request(
            Method::POST,
            &format!("/api/models/pna/versions/{pna_ver}/draft"),
            Some(&token),
            Some(serde_json::json!({ "target_version": "9.9.9" })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(!registry::version_exists(
        &state.store,
        &base,
        "pna",
        "9.9.9"
    ));

    // CC BY allows changes: a draft is made, and it keeps the licence.
    let resp = app()
        .oneshot(request(
            Method::POST,
            &format!("/api/models/gr/versions/{}/draft", versions["gr"]),
            Some(&token),
            Some(serde_json::json!({ "target_version": "9.9.9" })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let a = registry::get_attribution(
        &state.store,
        &registry::version_record_iri(&base, "gr", "9.9.9"),
    )
    .expect("the draft keeps the licence record");
    assert_eq!(
        a.licenses[0].uri,
        "https://creativecommons.org/licenses/by/3.0/"
    );
    assert!(!a.unchanged);

    // The licence page the record links to.
    let resp = app()
        .oneshot(request(
            Method::GET,
            "/api/vocab/notice?vocab=pna",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/plain"));
    let page = body_text(resp.into_body()).await;
    assert!(
        page.contains("Licence: CC BY-ND 3.0, https://creativecommons.org/licenses/by-nd/3.0/"),
        "{page}"
    );
    assert!(page.contains("only unaltered copies"), "{page}");
    let resp = app()
        .oneshot(request(
            Method::GET,
            "/api/vocab/notice?vocab=no-such-vocab",
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// A multipart body of text fields and one Turtle file.
fn multipart(boundary: &str, fields: &[(&str, &str)], ttl: &str) -> Body {
    let mut out = String::new();
    for (name, value) in fields {
        out.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    out.push_str(&format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"m.ttl\"\r\n\
         Content-Type: text/turtle\r\n\r\n{ttl}\r\n--{boundary}--\r\n"
    ));
    Body::from(out)
}

fn upload_request(id: &str, token: &str, fields: &[(&str, &str)], ttl: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/models/{id}/versions"))
        .header(header::CONTENT_TYPE, "multipart/form-data; boundary=BNDX")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(multipart("BNDX", fields, ttl))
        .unwrap()
}

/// An install as an earlier release made it, by the admin `adm`: a public
/// entry with no owner under the LOV prefix and namespace, version 2.0, its
/// graph loaded with an `owl:versionInfo` triple added.
fn earlier_install(state: &AppState, prefix: &str) {
    let base = state.base_url.to_string();
    let v = state
        .vocab_catalog
        .lov_by_prefix(prefix)
        .expect(prefix)
        .clone();
    let creator = format!("{base}/users/adm");
    registry::insert_data_model(
        &state.store,
        &base,
        prefix,
        "Earlier install",
        &v.nsp,
        None,
        true,
        None,
        None,
        Some(&creator),
        "2024-01-01T00:00:00Z",
    )
    .unwrap();
    let quads = upload::parse_quads(
        two_quads(&v.uri).as_bytes(),
        oxigraph::io::RdfFormat::NQuads,
    )
    .unwrap();
    let loaded =
        upload::load_parsed(&state.store, &base, prefix, Some("2.0"), quads, true).unwrap();
    registry::insert_version(
        &state.store,
        &base,
        &DataModelVersion {
            data_model_id: prefix.to_string(),
            version: "2.0".into(),
            status: VersionStatus::Published,
            graph_iri: registry::version_record_iri(&base, prefix, "2.0"),
            sub_graphs: loaded.sub_graphs,
            created_at: "2024-01-01T00:00:00Z".into(),
            created_by: Some(creator),
            derived_from: None,
            notes: Some(EARLIER_NOTE.into()),
            branch: None,
            sub_graph_status: Vec::new(),
        },
    )
    .unwrap();
    registry::update_latest_published(&state.store, &base, prefix, "2.0").unwrap();
}

/// What a boot does ([`install::migrate_legacy_installs`], as the server
/// wires it).
fn boot_check(state: &AppState) -> Vec<install::LegacyInstall> {
    let non_admin =
        |u: &str| matches!(state.auth_db.get_user_by_id(u), Ok(Some(x)) if !x.role.is_admin());
    install::migrate_legacy_installs(
        &state.store,
        &state.base_url,
        &state.vocab_catalog,
        &non_admin,
    )
}

fn triples_in(state: &AppState, graph: &str) -> usize {
    let q = format!("SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH <{graph}> {{ ?s ?p ?o }} }}");
    match state.store.query(&q).unwrap() {
        oxigraph::sparql::QueryResults::Solutions(mut s) => {
            match s.next().unwrap().unwrap().get("n") {
                Some(oxigraph::model::Term::Literal(l)) => l.value().parse().unwrap(),
                other => panic!("{other:?}"),
            }
        }
        _ => panic!("not a SELECT"),
    }
}

#[tokio::test]
async fn earlier_installs_are_made_private_and_a_users_copy_of_their_note_changes_nothing() {
    let (state, token) = admin_state();
    let base = state.base_url.to_string();
    let app = || test_app(state.clone());
    earlier_install(&state, "react"); // CC BY-NC 4.0: not redistributable
    earlier_install(&state, "pna"); // CC BY-ND 3.0: unaltered copies only

    // A user's own model named after a LOV prefix, with LOV's namespace for
    // it and the earlier installer's note copied into its version notes.
    state
        .auth_db
        .create_user("u_joe", "joe", "j@t.com", "h", SystemRole::User)
        .unwrap();
    let joe = mint_token("u_joe", "joe", "user");
    let gml_nsp = state
        .vocab_catalog
        .lov_by_prefix("gml")
        .unwrap()
        .nsp
        .clone();
    let resp = app()
        .oneshot(request(
            Method::POST,
            "/api/models",
            Some(&joe),
            Some(serde_json::json!({ "title": "GML", "namespace": gml_nsp })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let ttl = "<http://example.org/gml-ext#A> a <http://www.w3.org/2002/07/owl#Class> .";
    let resp = app()
        .oneshot(upload_request(
            "gml",
            &joe,
            &[("version", "1.0"), ("notes", EARLIER_NOTE)],
            ttl,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app()
        .oneshot(request(Method::GET, "/api/models/react", None, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "public before the upgrade");

    let done = boot_check(&state);
    assert_eq!(done.len(), 2, "{done:?}");
    assert!(done.iter().all(|d| d.made_private && d.recorded_licence));

    // No longer served to everyone.
    for uri in [
        "/api/models/react",
        "/api/models/pna/versions/2.0/data?format=nt",
    ] {
        let resp = app()
            .oneshot(request(Method::GET, uri, None, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{uri}");
    }

    // The licence record says what is known, and no more; the note and the
    // graph are as they were.
    let resp = app()
        .oneshot(request(
            Method::GET,
            "/api/models/pna/versions",
            Some(&token),
            None,
        ))
        .await
        .unwrap();
    let list = body_json(resp.into_body()).await;
    let v = &list.as_array().unwrap()[0];
    assert_eq!(v["notes"], EARLIER_NOTE);
    let a = &v["attribution"];
    assert_eq!(a["no_derivatives"], true);
    assert_eq!(a["unchanged"], false);
    assert_eq!(a["licenses"][0]["name"], "CC BY-ND 3.0");
    assert!(
        a["changes"]
            .as_str()
            .unwrap()
            .contains("owl:versionInfo \"2.0\""),
        "{a}"
    );
    for id in ["react", "pna"] {
        let graph = registry::version_record_iri(&base, id, "2.0");
        assert_eq!(triples_in(&state, &graph), 3, "{id}: nothing removed");
    }
    // An altered copy of a no-derivatives vocabulary is not made.
    let resp = app()
        .oneshot(request(
            Method::POST,
            "/api/models/pna/versions/2.0/draft",
            Some(&token),
            Some(serde_json::json!({ "target_version": "9.9.9" })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // The user's model: no licence record, its note kept, and its owner can
    // go on versioning it.
    let resp = app()
        .oneshot(request(
            Method::GET,
            "/api/models/gml/versions",
            Some(&joe),
            None,
        ))
        .await
        .unwrap();
    let list = body_json(resp.into_body()).await;
    let v = &list.as_array().unwrap()[0];
    assert!(v["attribution"].is_null(), "{v}");
    assert_eq!(v["notes"], EARLIER_NOTE);
    let resp = app()
        .oneshot(upload_request("gml", &joe, &[("version", "1.1")], ttl))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "not locked");
    assert!(registry::no_derivatives_attribution(&state.store, &base, "gml").is_none());

    // An admin who has checked the terms makes 'react' public again: later
    // boots leave that choice alone.
    let resp = app()
        .oneshot(request(
            Method::PATCH,
            "/api/models/react",
            Some(&token),
            Some(serde_json::json!({ "is_public": true })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(boot_check(&state).is_empty());
    let resp = app()
        .oneshot(request(Method::GET, "/api/models/react", None, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
