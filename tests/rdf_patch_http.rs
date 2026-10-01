//! RDF Patch (7.5): a version diff is served as a patch, and a patch applies
//! to another dataset atomically as one commit — with the guards a dataset
//! patch needs (registered graphs only, TA aborts). A `D` line may name a
//! blank node: the apply removes quads through a transaction, and a label the
//! store exported addresses the stored node (a hand-written one matches
//! nothing).

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
    // Accepted, and a no-op: a hand-written label names no stored node.
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["effective"]["removed"], 0, "{txt}");
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

/// Only well-formed, declared prefixed names reach the generated update. The
/// old check accepted any whitespace-free token containing `:`, and the
/// registered-graph gate only looked at the parsed graph position — so an
/// object token like `ex:o}GRAPH<urn:x>{<urn:a><urn:b><urn:c>` (SPARQL needs
/// no whitespace between IRIs) closed the registered `GRAPH { … }` block and
/// wrote into a graph the dataset never registered.
#[tokio::test]
async fn patch_rejects_undeclared_and_malformed_prefixed_names() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "dst",
            "dst",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("dst", G2).unwrap();
    let app = test_app(state.clone());
    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));
    let graphs_before = state.store.store().named_graphs().count();
    let post = |body: String| {
        let app = app.clone();
        let token = token.clone();
        async move {
            req(
                &app,
                Method::POST,
                "/api/datasets/dst/patch",
                Some(&token),
                Some("application/rdf-patch"),
                None,
                &body,
            )
            .await
        }
    };

    // An undeclared prefix is refused, even in a well-shaped local part.
    let (st, _, txt) = post(format!("TX .\nA ex:s <urn:p> <urn:o> <{G2}> .\nTC .\n")).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("undeclared prefix"), "{txt}");

    // A declared prefix with a local part carrying `}`, `<`, `>` and `{`.
    let (st, _, txt) = post(format!(
        "PA ex: <http://example.org/> .\nTX .\nA ex:s ex:p ex:o}}GRAPH<urn:x>{{<urn:a><urn:b><urn:c> <{G2}> .\nTC .\n"
    ))
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("not a prefixed name"), "{txt}");
    assert!(
        !ask("ASK { GRAPH <urn:x> { ?s ?p ?o } }"),
        "nothing may have been written to the unregistered graph"
    );
    assert!(
        !ask(&format!("ASK {{ GRAPH <{G2}> {{ ?s ?p ?o }} }}")),
        "nothing may have been written to the registered graph either"
    );
    // The same with `;`, `"` and whitespace-free `<`: refused.
    for bad in ["ex:o;DROP", "ex:o\"x", "ex:a<urn:b>"] {
        let (st, _, txt) = post(format!(
            "PA ex: <http://example.org/> .\nTX .\nA <urn:s> <urn:p> {bad} <{G2}> .\nTC .\n"
        ))
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}: {txt}");
    }
    assert_eq!(
        state.store.store().named_graphs().count(),
        graphs_before,
        "no graph may have been created"
    );

    // A proper prefixed name — declared, PN_LOCAL — still applies.
    let (st, _, txt) = post(format!(
        "PA ex: <http://example.org/> .\nTX .\nA ex:s ex:p ex:o-1.v2 <{G2}> .\nTC .\n"
    ))
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(ask(&format!(
        "ASK {{ GRAPH <{G2}> {{ <http://example.org/s> <http://example.org/p> <http://example.org/o-1.v2> }} }}"
    )));
}

// ─── Through the change log ────────────────────────────────────────────────

async fn recording_dataset() -> (open_triplestore::server::AppState, String, Router, String) {
    let store = open_triplestore::store::TripleStore::in_memory()
        .unwrap()
        .with_change_capture(
            open_triplestore::store::changes::DEFAULT_MAX_SCAN,
            open_triplestore::store::changes::DEFAULT_MAX_PAYLOAD,
        );
    let (state, token) = admin_state_with_store(store);
    let app = test_app(state.clone());
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets",
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "name": "pc", "owner_type": "user", "owner_id": "adm", "visibility": "private" })
            .to_string(),
    )
    .await;
    assert!(st.is_success(), "{txt}");
    let id = serde_json::from_str::<Value>(&txt).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/graphs"),
        Some(&token),
        Some("application/json"),
        None,
        &json!({ "graph_iri": G }).to_string(),
    )
    .await;
    assert!(st.is_success(), "{txt}");
    (state, token, app, id)
}

/// A change exported by the store carries the store's own blank-node labels,
/// so its inverse, applied through the patch route, removes exactly those
/// nodes — which `DELETE DATA` never could.
#[tokio::test]
async fn an_exported_change_removes_its_blank_nodes_through_the_patch_route() {
    let (state, token, app, id) = recording_dataset().await;
    state
        .store
        .load_str(
            "<urn:feature:1> <urn:geom> _:g . _:g <urn:wkt> \"POINT(1 2)\" .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    let seq = state.store.changes().last_seq();
    let (st, ct, patch) = req(
        &app,
        Method::GET,
        &format!("/api/datasets/{id}/changes/{seq}/inverse"),
        Some(&token),
        None,
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{patch}");
    assert!(ct.starts_with("application/rdf-patch"));
    assert!(
        patch.contains("D _:"),
        "the inverse deletes the blank node: {patch}"
    );
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/patch"),
        Some(&token),
        Some("application/rdf-patch"),
        None,
        &patch,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["effective"]["removed"], 2, "{txt}");
    assert!(r["seq"].is_i64(), "{txt}");
    assert_eq!(state.store.count_graph(Some(G)).unwrap(), 0);
}

/// `H base-digest` makes the apply conditional on the graphs being what the
/// patch was made against.
#[tokio::test]
async fn a_patch_whose_base_digest_no_longer_matches_is_refused() {
    let (state, token, app, id) = recording_dataset().await;
    state
        .store
        .load_str("<urn:s> <urn:p> \"1\" .", RdfFormat::Turtle, Some(G))
        .unwrap();
    let digest =
        open_triplestore::store::set_hash::SetHash::of_graphs(&state.store, &[G.to_string()])
            .to_hex();
    let patch = |d: &str| {
        format!("H base-digest \"{d}\" .\nTX .\nA <urn:s2> <urn:p> \"2\" <{G}> .\nTC .\n")
    };
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/patch"),
        Some(&token),
        Some("application/rdf-patch"),
        None,
        &patch(&digest),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    // The same digest again: the graph moved on.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/patch"),
        Some(&token),
        Some("application/rdf-patch"),
        None,
        &patch(&digest),
    )
    .await;
    assert_eq!(st, StatusCode::PRECONDITION_FAILED, "{txt}");
    assert_eq!(state.store.count_graph(Some(G)).unwrap(), 2);
    let (st, _, _) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/patch"),
        Some(&token),
        Some("application/rdf-patch"),
        None,
        &patch("not-hex"),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// The route needs an account: anonymous used to reach the handler and fail
/// its extractor with a 500.
#[tokio::test]
async fn an_anonymous_patch_is_unauthorized_not_an_error() {
    let (_state, _token, app, id) = recording_dataset().await;
    let (st, _, _) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/{id}/patch"),
        None,
        Some("application/rdf-patch"),
        None,
        &format!("TX .\nA <urn:s> <urn:p> \"1\" <{G}> .\nTC .\n"),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}
