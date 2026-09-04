//! Selectable entailment per dataset (7.3): a dataset picks a regime and a
//! materialisation mode; writes re-materialise into the dataset's own
//! entailment graph; queries opt in with `entailment_dataset`, over GET and
//! both POST flavours; switching off clears the graph.

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{GraphKind, OwnerType, SystemRole, Visibility};
use oxigraph::io::RdfFormat;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const MODEL: &str = "https://example.org/ent/model";
const DATA: &str = "https://example.org/ent/instances";
const EX: &str = "https://example.org/ent/";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    ct: Option<&str>,
    body: &str,
) -> (StatusCode, Value, String) {
    let mut b = Request::builder().method(method).uri(uri).header(
        header::ACCEPT,
        "application/sparql-results+json, application/json",
    );
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = ct {
        b = b.header(header::CONTENT_TYPE, c);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    let text = body_text(resp.into_body()).await;
    (st, serde_json::from_str(&text).unwrap_or(Value::Null), text)
}

fn rows(v: &Value) -> usize {
    v["results"]["bindings"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0)
}

#[tokio::test]
async fn dataset_regime_materialises_on_write_and_joins_queries_on_request() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "ent",
            "Entailment",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("ent", MODEL).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("ent", MODEL, Some(GraphKind::Model))
        .unwrap();
    state.auth_db.add_dataset_graph("ent", DATA).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("ent", DATA, Some(GraphKind::Instances))
        .unwrap();
    state
        .store
        .load_str(
            &format!(
                "<{EX}Bridge> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <{EX}Asset> ."
            ),
            RdfFormat::Turtle,
            Some(MODEL),
        )
        .unwrap();
    state
        .store
        .load_str(
            &format!("<{EX}b1> a <{EX}Bridge> ."),
            RdfFormat::Turtle,
            Some(DATA),
        )
        .unwrap();
    let app = test_app(state.clone());
    let q = format!("SELECT ?b WHERE {{ ?b a <{EX}Asset> }}");
    let enc = url_encode(&q);

    // Nothing configured: no inferred Asset.
    let (st, v, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/ent/entailment",
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["mode"], "off");
    let (st, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(rows(&v), 0, "{txt}");

    // Select RDFS + materialize: runs immediately.
    let (st, v, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/ent/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "rdfs", "mode": "materialize" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["graph"], "urn:entailment:rdfs:ent");
    assert!(v["triples"].as_i64().unwrap() >= 1, "{txt}");
    let (st, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(
        rows(&v),
        1,
        "b1 is an Asset through the dataset's entailment graph: {txt}"
    );
    let (_, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(
        rows(&v),
        0,
        "without opting in, the entailment graph stays out: {txt}"
    );

    // POST, both flavours.
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/sparql?entailment_dataset=ent",
        Some(&token),
        Some("application/sparql-query"),
        &q,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(rows(&v), 1, "{txt}");
    let (st, v, txt) = req(
        &app,
        Method::POST,
        "/sparql",
        Some(&token),
        Some("application/x-www-form-urlencoded"),
        &format!("query={enc}&entailment_dataset=ent"),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(rows(&v), 1, "{txt}");

    // A write re-materialises: a second bridge appears as an Asset.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        &format!("/store?graph={}", url_encode(DATA)),
        Some(&token),
        Some("text/turtle"),
        &format!("<{EX}b2> a <{EX}Bridge> ."),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(rows(&v), 2, "{txt}");
    // …and a delete drops the consequence (the graph is rebuilt, not appended).
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        &format!("/store?graph={}", url_encode(DATA)),
        Some(&token),
        Some("text/turtle"),
        &format!("<{EX}b2> a <{EX}Bridge> ."),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let (_, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(
        rows(&v),
        1,
        "b1 was removed, so its inferred type is gone: {txt}"
    );
    let (_, v, _) = req(
        &app,
        Method::GET,
        "/api/datasets/ent/entailment",
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(v["regime"], "rdfs");
    assert!(v["last_run_at"].is_string());

    // Off clears the graph; unknown regimes are refused; strangers cannot configure.
    let (st, _, txt) = req(
        &app,
        Method::PUT,
        "/api/datasets/ent/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "rdfs", "mode": "off" }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let (_, v, txt) = req(
        &app,
        Method::GET,
        &format!("/sparql?query={enc}&entailment_dataset=ent"),
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(rows(&v), 0, "{txt}");
    let (st, _, _) = req(
        &app,
        Method::PUT,
        "/api/datasets/ent/entailment",
        Some(&token),
        Some("application/json"),
        &json!({ "regime": "magic" }).to_string(),
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
        Method::PUT,
        "/api/datasets/ent/entailment",
        Some(&eve),
        Some("application/json"),
        &json!({ "regime": "rdfs" }).to_string(),
    )
    .await;
    assert!(
        st == StatusCode::FORBIDDEN || st == StatusCode::NOT_FOUND,
        "{st}"
    );
}
