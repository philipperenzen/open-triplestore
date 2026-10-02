//! RDF Patch (7.5): a version diff is served as a patch, and a patch applies
//! to another dataset atomically as one commit — with the guards a dataset
//! patch needs (registered graphs only, `?graph=` for triples, TA aborts, the
//! SHACL write gates) and blank nodes addressed by the store's own ids.

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
    assert!(txt.contains("?graph="), "{txt}");
    // `?graph=` names the registered graph that receives triples …
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/dst/patch?graph={}", url_encode(G2)),
        Some(&token),
        Some("application/rdf-patch"),
        None,
        "TX .\nA <urn:t:5> <urn:p> \"five\" .\nTC .\n",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(ask(&format!(
        "ASK {{ GRAPH <{G2}> {{ <urn:t:5> <urn:p> \"five\" }} }}"
    )));
    // … and only a registered one.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/api/datasets/dst/patch?graph={}", url_encode(G)),
        Some(&token),
        Some("application/rdf-patch"),
        None,
        "TX .\nA <urn:t:6> <urn:p> \"six\" .\nTC .\n",
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("not registered"), "{txt}");
    // A blank-node delete names the store's node with that id: none here,
    // so nothing is removed.
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
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["removed"], 0, "{txt}");
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

fn graph_triples(state: &open_triplestore::server::AppState, g: &str) -> Vec<String> {
    let name = oxigraph::model::NamedNode::new(g).unwrap();
    let mut out: Vec<String> = state
        .store
        .quads_for_graph(oxigraph::model::GraphNameRef::NamedNode(name.as_ref()))
        .unwrap()
        .into_iter()
        .map(|q| format!("{} {} {}", q.subject, q.predicate, q.object))
        .collect();
    out.sort();
    out
}

/// A version diff over blank-node data names the store's own blank nodes, so
/// it deletes and changes them, and applied to a copy holding the same nodes
/// it reproduces the source exactly — where `INSERT DATA` used to mint fresh
/// nodes and blank-node deletes were refused.
#[tokio::test]
async fn blank_node_diff_round_trips() {
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
    }
    state
        .store
        .load_str(
            "<urn:doc> <urn:part> [ <urn:v> 1 ; <urn:name> \"a\" ] .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    state
        .store
        .update(&format!(
            "INSERT {{ GRAPH <{G2}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{G}> {{ ?s ?p ?o }} }}"
        ))
        .unwrap();
    let app = test_app(state.clone());
    let post = |uri: String, body: String| {
        let app = app.clone();
        let token = token.clone();
        async move {
            req(
                &app,
                Method::POST,
                &uri,
                Some(&token),
                Some("application/rdf-patch"),
                None,
                &body,
            )
            .await
        }
    };
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

    // Change the blank node in place, by its id, and add a second one.
    let part = graph_triples(&state, G)
        .into_iter()
        .find_map(|t| t.strip_prefix("<urn:doc> <urn:part> ").map(str::to_string))
        .expect("the part");
    assert!(part.starts_with("_:"), "{part}");
    let int = |n: u8| format!("\"{n}\"^^<http://www.w3.org/2001/XMLSchema#integer>");
    let (st, _, txt) = post(
        "/api/datasets/src/patch".to_string(),
        format!(
            "TX .\nD {part} <urn:v> {} <{G}> .\nA {part} <urn:v> {} <{G}> .\nA _:extra <urn:name> \"b\" <{G}> .\nTC .\n",
            int(1),
            int(2)
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(
        (r["added"].clone(), r["removed"].clone()),
        (json!(2), json!(1)),
        "{txt}"
    );
    assert_eq!(
        graph_triples(&state, G)
            .iter()
            .filter(|t| t.starts_with(&part))
            .count(),
        2,
        "the same node, changed — not a fresh copy"
    );

    // The diff names that node, and applied to dst reproduces src.
    let (st, _, patch) = req(
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
    assert!(
        patch.contains(&format!("D {part} <urn:v> {} <{G}> .", int(1))),
        "{patch}"
    );
    assert!(
        patch.contains(&format!("A {part} <urn:v> {} <{G}> .", int(2))),
        "{patch}"
    );
    let retargeted = patch.replace(&format!("<{G}>"), &format!("<{G2}>"));
    let (st, _, txt) = post("/api/datasets/dst/patch".to_string(), retargeted.clone()).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(graph_triples(&state, G2), graph_triples(&state, G));
    // Applying it again changes nothing.
    let (st, _, txt) = post("/api/datasets/dst/patch".to_string(), retargeted).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(
        (r["added"].clone(), r["removed"].clone()),
        (json!(0), json!(0)),
        "{txt}"
    );
}

/// A patch passes the SHACL write gates a Graph Store write to the same graph
/// does, over what the graph would hold after it: a refusal is a 422 with
/// the report, and nothing is applied.
#[tokio::test]
async fn patch_passes_the_shacl_write_gates() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "d",
            "d",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("d", G2).unwrap();
    state
        .auth_db
        .update_dataset_shacl("d", true, Some("urn:shapes:d"))
        .unwrap();
    state
        .store
        .load_str(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .
             <urn:S> a sh:NodeShape ; sh:targetClass <urn:Thing> ;
               sh:property [ sh:path <urn:name> ; sh:minCount 1 ] .",
            RdfFormat::Turtle,
            Some("urn:shapes:d"),
        )
        .unwrap();
    let app = test_app(state.clone());
    let post = |body: String| {
        let app = app.clone();
        let token = token.clone();
        async move {
            req(
                &app,
                Method::POST,
                &format!("/api/datasets/d/patch?graph={}", url_encode(G2)),
                Some(&token),
                Some("application/rdf-patch"),
                None,
                &body,
            )
            .await
        }
    };
    let a_type = "<urn:x> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Thing>";

    let (st, _, txt) = post(format!("TX .\nA {a_type} .\nTC .\n")).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");
    assert!(txt.contains("SHACL validation failed"), "{txt}");
    assert!(graph_triples(&state, G2).is_empty(), "nothing applied");

    let (st, _, txt) = post(format!(
        "TX .\nA {a_type} .\nA <urn:x> <urn:name> \"x\" .\nTC .\n"
    ))
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");

    // Deleting the name would leave the graph violating: refused.
    let (st, _, txt) = post("TX .\nD <urn:x> <urn:name> \"x\" .\nTC .\n".to_string()).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");
    assert_eq!(
        graph_triples(&state, G2).len(),
        2,
        "the name is still there"
    );
}
