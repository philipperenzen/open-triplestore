//! Time-evolving properties (6.5): states accumulate in the dataset's
//! provenance-role states graph, the data graph always holds the current
//! value as a plain triple, and history / as-of read the chain.

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

const G: &str = "https://example.org/ps/instances";
const E: &str = "https://example.org/ps/bridge/b1";
const P: &str = "https://example.org/ps/loadRating";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let st = resp.status();
    let text = body_text(resp.into_body()).await;
    (st, serde_json::from_str(&text).unwrap_or(Value::Null), text)
}

#[tokio::test]
async fn states_keep_history_and_the_data_graph_keeps_the_current_value() {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "ps",
            "Property states",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("ps", G).unwrap();
    state
        .store
        .load_str(
            &format!("<{E}> a <https://example.org/ps/Bridge> ; <{P}> 30 ."),
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    let app = test_app(state.clone());
    let values = |q: &str| -> Vec<String> {
        match state.store.query(q) {
            Ok(QueryResults::Solutions(s)) => s
                .flatten()
                .map(|r| match r.get("v").unwrap() {
                    oxigraph::model::Term::Literal(l) => l.value().to_string(),
                    other => other.to_string(),
                })
                .collect(),
            _ => vec![],
        }
    };
    let current = || {
        values(&format!(
            "SELECT ?v WHERE {{ GRAPH <{G}> {{ <{E}> <{P}> ?v }} }}"
        ))
    };

    // State 1 (confirmed, valid from January) replaces the loaded value.
    let (st, v, txt) = req(&app, Method::POST, "/api/datasets/ps/properties/state", Some(&token), Some(json!({
        "entity": E, "property": P, "value": "45", "valid_from": "2026-01-01", "reliability": "confirmed", "note": "inspection"
    }))).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(
        v["data_graph"], G,
        "the instances graph holds the current value: {txt}"
    );
    assert_eq!(
        current(),
        vec!["45".to_string()],
        "single current value in the data graph"
    );

    // State 2 (assumed, valid from June).
    let (st, _, txt) = req(&app, Method::POST, "/api/datasets/ps/properties/state", Some(&token), Some(json!({
        "entity": E, "property": P, "value": "40", "valid_from": "2026-06-01T00:00:00Z", "reliability": "assumed"
    }))).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(
        current(),
        vec!["40".to_string()],
        "the newer state is the current value"
    );

    // History: two states, newest first, exactly one current.
    let (st, h, txt) = req(
        &app,
        Method::GET,
        &format!(
            "/api/datasets/ps/properties/history?entity={}&property={}",
            url_encode(E),
            url_encode(P)
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let states = h["states"].as_array().unwrap();
    assert_eq!(states.len(), 2, "{txt}");
    assert_eq!(states[0]["value"], "40");
    assert_eq!(states[0]["current"], true);
    assert_eq!(states[0]["reliability"], "assumed");
    assert_eq!(states[1]["value"], "45");
    assert_eq!(states[1]["current"], false);
    assert_eq!(states[1]["reliability"], "confirmed");
    assert_eq!(states[1]["note"], "inspection");
    assert!(
        states[1]["attributed_to"]
            .as_str()
            .unwrap()
            .ends_with("/users/adm"),
        "{txt}"
    );

    // As-of: March → 45; July → 40; 2025 → nothing.
    let (st, a, txt) = req(
        &app,
        Method::GET,
        &format!(
            "/api/datasets/ps/properties/as-of?entity={}&property={}&at=2026-03-01",
            url_encode(E),
            url_encode(P)
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(a["state"]["value"], "45", "{txt}");
    let (_, a, _) = req(
        &app,
        Method::GET,
        &format!(
            "/api/datasets/ps/properties/as-of?entity={}&property={}&at=2026-07-01T12:00:00Z",
            url_encode(E),
            url_encode(P)
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(a["state"]["value"], "40");
    let (st, _, _) = req(
        &app,
        Method::GET,
        &format!(
            "/api/datasets/ps/properties/as-of?entity={}&property={}&at=2025-01-01",
            url_encode(E),
            url_encode(P)
        ),
        Some(&token),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // The states graph is registered with the provenance role, and the OPM
    // vocabulary is what is stored.
    let (_, c, txt) = req(
        &app,
        Method::GET,
        "/api/datasets/ps/conformance",
        Some(&token),
        None,
    )
    .await;
    assert!(
        txt.contains("urn:ots:property-states:ps") && txt.contains("provenance"),
        "{c}"
    );
    let opm = values("SELECT ?v WHERE { GRAPH <urn:ots:property-states:ps> { ?s a <https://w3id.org/opm#CurrentPropertyState> ; <https://schema.org/value> ?v } }");
    assert_eq!(opm, vec!["40".to_string()], "exactly one current OPM state");

    // Typed and IRI values; an unknown reliability is refused.
    let (st, _, txt) = req(&app, Method::POST, "/api/datasets/ps/properties/state", Some(&token), Some(json!({
        "entity": E, "property": "https://example.org/ps/inspectedBy", "value": "https://example.org/ps/org/rws", "datatype": "iri"
    }))).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/ps/properties/state",
        Some(&token),
        Some(json!({
            "entity": E, "property": P, "value": "1", "reliability": "guessed"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");

    // It is in the dataset's history.
    let (_, _, commits) = req(
        &app,
        Method::GET,
        "/api/datasets/ps/commits",
        Some(&token),
        None,
    )
    .await;
    assert!(commits.contains("Property state"), "{commits}");

    // A stranger can neither read nor write a private dataset's states.
    state
        .auth_db
        .create_user("eve", "eve", "eve@t.com", "h", SystemRole::User)
        .unwrap();
    let other = mint_token("eve", "eve", "user");
    let (st, _, _) = req(
        &app,
        Method::GET,
        &format!(
            "/api/datasets/ps/properties/history?entity={}&property={}",
            url_encode(E),
            url_encode(P)
        ),
        Some(&other),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _, _) = req(
        &app,
        Method::POST,
        "/api/datasets/ps/properties/state",
        Some(&other),
        Some(json!({ "entity": E, "property": P, "value": "0" })),
    )
    .await;
    assert!(
        st == StatusCode::NOT_FOUND || st == StatusCode::FORBIDDEN,
        "{st}"
    );
}
