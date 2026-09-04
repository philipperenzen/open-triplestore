//! RDF Patch (7.5): a version diff is served as a patch, and a patch applies
//! to another dataset atomically as one commit — with the guards a dataset
//! patch needs (registered graphs only, no blank-node deletes, TA aborts).

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const G: &str = "https://example.org/patch/src";
const G2: &str = "https://example.org/patch/dst";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    ct: Option<&str>,
    accept: Option<&str>,
    body: &str,
) -> (StatusCode, String, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = ct {
        b = b.header(header::CONTENT_TYPE, c);
    }
    if let Some(a) = accept {
        b = b.header(header::ACCEPT, a);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    (st, ct, body_text(resp.into_body()).await)
}

#[tokio::test]
async fn version_diff_as_patch_applies_to_another_dataset() {
    let (state, token) = admin_state();
    for (id, g) in [("src", G), ("dst", G2)] {
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
        state.auth_db.add_dataset_graph(id, g).unwrap();
        state
            .store
            .load_str(
                "<urn:t:1> <urn:p> \"one\" . <urn:t:2> <urn:p> \"two\" .",
                RdfFormat::Turtle,
                Some(g),
            )
            .unwrap();
    }
    let app = test_app(state.clone());
    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));

    // Cut v1 of src, then change src: drop t2, add t3.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/src/versions",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "version": "1.0.0" }).to_string(),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        &format!("/store?graph={}", url_encode(G)),
        Some(&token),
        Some("text/turtle"),
        None,
        "<urn:t:1> <urn:p> \"one\" . <urn:t:3> <urn:p> \"three\" .",
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");

    // The diff v1 → live as an RDF Patch, by query parameter and by Accept.
    let (st, ct, patch) = req(
        &app,
        Method::GET,
        "/api/datasets/src/versions/1.0.0/diff/live?format=rdf-patch",
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{patch}");
    assert!(ct.contains("rdf-patch"), "{ct}");
    assert!(
        patch.contains("H from \"1.0.0\" .") && patch.contains("H to \"live\" ."),
        "{patch}"
    );
    assert!(
        patch.contains(&format!("D <urn:t:2> <urn:p> \"two\" <{G}> .")),
        "{patch}"
    );
    assert!(
        patch.contains(&format!("A <urn:t:3> <urn:p> \"three\" <{G}> .")),
        "{patch}"
    );
    assert!(
        !patch.contains("<urn:t:1>"),
        "unchanged triples are not in the patch: {patch}"
    );
    assert!(
        patch.contains("TX .\n") && patch.trim_end().ends_with("TC ."),
        "{patch}"
    );
    let (st, ct, by_accept) = req(
        &app,
        Method::GET,
        "/api/datasets/src/versions/1.0.0/diff/live",
        Some(&token),
        None,
        Some("application/rdf-patch"),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        ct.contains("rdf-patch") && by_accept.contains("TX ."),
        "{ct} {by_accept}"
    );
    // The JSON diff is unchanged.
    let (st, ct, j) = req(
        &app,
        Method::GET,
        "/api/datasets/src/versions/1.0.0/diff/live",
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(ct.contains("json"), "{ct}");
    let j: Value = serde_json::from_str(&j).unwrap();
    assert_eq!(j["added"], 1);
    assert_eq!(j["removed"], 1);

    // Apply it to dst (retargeting the graph): dst now matches src.
    let retargeted = patch.replace(&format!("<{G}>"), &format!("<{G2}>"));
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/dst/patch",
        Some(&token),
        Some("application/rdf-patch"),
        None,
        &retargeted,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["applied"], true, "{txt}");
    assert_eq!(r["added"], 1);
    assert_eq!(r["removed"], 1);
    assert!(
        ask(&format!(
            "ASK {{ GRAPH <{G2}> {{ <urn:t:3> <urn:p> \"three\" }} }}"
        )),
        "t3 added"
    );
    assert!(
        !ask(&format!(
            "ASK {{ GRAPH <{G2}> {{ <urn:t:2> <urn:p> \"two\" }} }}"
        )),
        "t2 removed"
    );
    assert!(
        ask(&format!(
            "ASK {{ GRAPH <{G2}> {{ <urn:t:1> <urn:p> \"one\" }} }}"
        )),
        "t1 untouched"
    );
    let (_, _, commits) = req(
        &app,
        Method::GET,
        "/api/datasets/dst/commits",
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert!(commits.contains("RDF Patch"), "{commits}");

    // Prefixed names through PA; the graph itself may be prefixed too.
    let prefixed = "PA ex: <urn:t:> .\nPA g: <https://example.org/patch/> .\nTX .\nA ex:4 <urn:p> \"four\" g:dst .\nTC .\n".to_string();
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/dst/patch",
        Some(&token),
        Some("application/rdf-patch"),
        None,
        &prefixed,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(ask(&format!(
        "ASK {{ GRAPH <{G2}> {{ <urn:t:4> <urn:p> \"four\" }} }}"
    )));

    // Guards.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/dst/patch",
        Some(&token),
        Some("application/rdf-patch"),
        None,
        "TX .\nA <urn:x> <urn:p> \"y\" <urn:not-registered> .\nTC .\n",
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("not registered"), "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/dst/patch",
        Some(&token),
        Some("application/rdf-patch"),
        None,
        "TX .\nA <urn:x> <urn:p> \"y\" .\nTC .\n",
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/dst/patch",
        Some(&token),
        Some("application/rdf-patch"),
        None,
        &format!("TX .\nD _:b <urn:p> \"y\" <{G2}> .\nTC .\n"),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("blank node"), "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/dst/patch",
        Some(&token),
        Some("application/rdf-patch"),
        None,
        &format!("TX .\nA <urn:t:9> <urn:p> \"nine\" <{G2}> .\nTA .\n"),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["applied"], false);
    assert!(
        !ask(&format!("ASK {{ GRAPH <{G2}> {{ <urn:t:9> ?p ?o }} }}")),
        "an aborted transaction applies nothing"
    );
    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/datasets/dst/patch",
        Some(&token),
        Some("application/rdf-patch"),
        None,
        "this is not a patch\n",
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    state
        .auth_db
        .create_user("eve", "eve", "eve@t.com", "h", SystemRole::User)
        .unwrap();
    let eve = mint_token("eve", "eve", "user");
    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/datasets/dst/patch",
        Some(&eve),
        Some("application/rdf-patch"),
        None,
        &format!("TX .\nA <urn:t:9> <urn:p> \"nine\" <{G2}> .\nTC .\n"),
    )
    .await;
    assert!(
        st == StatusCode::NOT_FOUND || st == StatusCode::FORBIDDEN,
        "{st}"
    );
}
