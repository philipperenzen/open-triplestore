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
        "entity": E, "property": "https://example.org/ps/inspectedBy", "value": "https://example.org/ps/org/inspector", "datatype": "iri"
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

/// The language tag is written raw after `@`, so it must be a tag. It went in
/// unvalidated: `@en } } WHERE { } ; INSERT DATA { GRAPH <urn:probe> {…} } ;
/// INSERT { GRAPH <g> { <e> <p> "v"@en` closed the INSERT template and the
/// operation, ran its own INSERT DATA, and reopened a template so the rest
/// of the generated update still parsed. Now it is a 400 and nothing is
/// written; a real tag such as `en-GB` still works.
#[tokio::test]
async fn language_tags_are_validated_before_they_reach_sparql() {
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
    let app = test_app(state.clone());
    let ask = |q: &str| matches!(state.store.query(q), Ok(QueryResults::Boolean(true)));
    let graphs_before = state.store.store().named_graphs().count();

    let payload = format!(
        "en }} }} WHERE {{ }} ; INSERT DATA {{ GRAPH <urn:probe> {{ <urn:s> <urn:p> <urn:o> }} }} ; INSERT {{ GRAPH <{G}> {{ <{E}> <{P}> \"Viaduct\"@en"
    );
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/ps/properties/state",
        Some(&token),
        Some(json!({ "entity": E, "property": P, "value": "Viaduct", "language": payload })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    assert!(txt.contains("language tag"), "{txt}");
    assert!(
        !ask("ASK { GRAPH <urn:probe> { ?s ?p ?o } }"),
        "the injected INSERT DATA must not have run"
    );
    assert!(
        !ask(&format!("ASK {{ GRAPH <{G}> {{ <{E}> <{P}> ?v }} }}")),
        "no value may have been written"
    );
    assert_eq!(
        state.store.store().named_graphs().count(),
        graphs_before,
        "no graph may have been created"
    );
    for bad in ["", "en GB", "en_GB", "en\" } #"] {
        let (st, _, txt) = req(
            &app,
            Method::POST,
            "/api/datasets/ps/properties/state",
            Some(&token),
            Some(json!({ "entity": E, "property": P, "value": "x", "language": bad })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad:?}: {txt}");
    }

    // A datatype that is not an IRI is refused too.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/ps/properties/state",
        Some(&token),
        Some(json!({ "entity": E, "property": P, "value": "1", "datatype": "xsd:int> <urn:p> <urn:o" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");

    // A well-formed tag works and comes back in the history.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/api/datasets/ps/properties/state",
        Some(&token),
        Some(json!({ "entity": E, "property": P, "value": "River bridge", "language": "en-GB" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert!(ask(&format!(
        "ASK {{ GRAPH <{G}> {{ <{E}> <{P}> \"River bridge\"@en-GB }} }}"
    )));
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
    // Language tags compare case-insensitively; the store normalises them.
    assert!(
        h["states"][0]["language"]
            .as_str()
            .is_some_and(|l| l.eq_ignore_ascii_case("en-GB")),
        "{txt}"
    );
}

// ─── OPM lifecycle, queries, exchange (M3) ──────────────────────────────────

const OPM: &str = "https://w3id.org/opm#";

/// A dataset `id` owned by `adm` with the instances graph [`G`] (or `g`).
fn dataset(state: &open_triplestore::server::AppState, id: &str, g: &str, vis: Visibility) {
    state
        .auth_db
        .create_dataset(id, id, None, OwnerType::User, "adm", vis, None)
        .unwrap();
    state.auth_db.add_dataset_graph(id, g).unwrap();
    state
        .auth_db
        .set_dataset_graph_role(
            id,
            g,
            Some(open_triplestore::auth::models::GraphKind::Instances),
        )
        .unwrap();
}

async fn post(app: &Router, token: &str, uri: &str, body: Value) -> (StatusCode, Value, String) {
    req(app, Method::POST, uri, Some(token), Some(body)).await
}

async fn get(app: &Router, token: Option<&str>, uri: &str) -> (StatusCode, Value, String) {
    req(app, Method::GET, uri, token, None).await
}

fn history_uri(ds: &str, e: &str, p: &str) -> String {
    format!(
        "/api/datasets/{ds}/properties/history?entity={}&property={}",
        url_encode(e),
        url_encode(p)
    )
}

fn as_of_uri(ds: &str, e: &str, p: &str, at: &str) -> String {
    format!(
        "/api/datasets/{ds}/properties/as-of?entity={}&property={}&at={}",
        url_encode(e),
        url_encode(p),
        url_encode(at)
    )
}

fn plain_values(
    state: &open_triplestore::server::AppState,
    g: &str,
    e: &str,
    p: &str,
) -> Vec<String> {
    match state.store.query(&format!(
        "SELECT ?v WHERE {{ GRAPH <{g}> {{ <{e}> <{p}> ?v }} }}"
    )) {
        Ok(QueryResults::Solutions(s)) => s
            .flatten()
            .map(|r| match r.get("v").unwrap() {
                oxigraph::model::Term::Literal(l) => l.value().to_string(),
                other => other.to_string(),
            })
            .collect(),
        _ => vec![],
    }
}

/// Raw HTTP with a non-JSON body.
async fn send_raw(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    content_type: &str,
    body: String,
) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    (st, body_text(resp.into_body()).await)
}

async fn export(app: &Router, token: Option<&str>, ds: &str, accept: &str) -> (StatusCode, String) {
    let mut b = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/datasets/{ds}/properties/export"))
        .header(header::ACCEPT, accept);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    (st, body_text(resp.into_body()).await)
}

/// The triples of an N-Triples document, as a sorted list of lines.
fn ntriples_set(doc: &str) -> Vec<String> {
    let mut lines: Vec<String> = doc
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    lines.sort();
    lines.dedup();
    lines
}

/// OPM deletes a property without removing it: a current `opm:Deleted` state
/// with no value, the plain triple gone from the data graph. History and
/// as-of report the deletion; restore brings the last value back.
#[tokio::test]
async fn deletion_is_a_state_and_restore_brings_the_value_back() {
    let (state, token) = admin_state();
    dataset(&state, "ps", G, Visibility::Private);
    let app = test_app(state.clone());
    let set = |value: &str, from: &str, rel: &str| {
        json!({ "entity": E, "property": P, "value": value, "valid_from": from, "reliability": rel,
                "documentation": ["https://example.org/ps/docs/inspection-1"] })
    };
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/ps/properties/state",
        set("45", "2026-01-01", "confirmed"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/ps/properties/state",
        set("40", "2026-03-01", "assumed"),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");

    // Delete, effective May.
    let (st, d, txt) = post(
        &app,
        &token,
        "/api/datasets/ps/properties/delete",
        json!({ "entity": E, "property": P, "valid_from": "2026-05-01", "note": "removed from the design" }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(d["deleted"], true);
    assert!(
        plain_values(&state, G, E, P).is_empty(),
        "the plain triple is gone"
    );

    let (_, h, txt) = get(&app, Some(&token), &history_uri("ps", E, P)).await;
    let states = h["states"].as_array().unwrap();
    assert_eq!(states.len(), 3, "{txt}");
    assert_eq!(states[0]["deleted"], true, "{txt}");
    assert_eq!(states[0]["current"], true);
    assert!(
        states[0]["value"].is_null(),
        "a deleted state has no value: {txt}"
    );
    assert_eq!(states[0]["note"], "removed from the design");
    assert_eq!(states[1]["current"], false);
    assert_eq!(
        states[1]["documentation"],
        json!(["https://example.org/ps/docs/inspection-1"])
    );

    // As-of across the deletion.
    let (_, a, _) = get(&app, Some(&token), &as_of_uri("ps", E, P, "2026-04-01")).await;
    assert_eq!(a["state"]["value"], "40");
    assert_eq!(a["state"]["deleted"], false);
    let (st, a, txt) = get(&app, Some(&token), &as_of_uri("ps", E, P, "2026-06-01")).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(a["state"]["deleted"], true, "{txt}");
    assert!(a["state"]["value"].is_null());

    // Deleting twice is a conflict; deleting what was never there is a 404.
    let body = json!({ "entity": E, "property": P });
    let (st, _, _) = post(
        &app,
        &token,
        "/api/datasets/ps/properties/delete",
        body.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/ps/properties/delete",
        json!({ "entity": E, "property": "https://example.org/ps/never" }),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "{txt}");

    // Restore: the last value, its reliability and documentation, as a new
    // current state that is a revision of it.
    let (st, r, txt) = post(
        &app,
        &token,
        "/api/datasets/ps/properties/restore",
        body.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(r["value"], "40");
    assert_eq!(r["restored_from"], states[1]["state"], "{txt}");
    assert_eq!(plain_values(&state, G, E, P), vec!["40".to_string()]);
    let (_, h, txt) = get(&app, Some(&token), &history_uri("ps", E, P)).await;
    let states = h["states"].as_array().unwrap();
    assert_eq!(states.len(), 4, "{txt}");
    let current: Vec<_> = states.iter().filter(|s| s["current"] == true).collect();
    assert_eq!(current.len(), 1, "exactly one current state: {txt}");
    assert_eq!(current[0]["value"], "40");
    assert_eq!(current[0]["reliability"], "assumed");
    assert_eq!(current[0]["revision_of"], r["restored_from"]);
    let (st, _, _) = post(&app, &token, "/api/datasets/ps/properties/restore", body).await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "restoring a live property is a conflict"
    );

    // Each step is a commit.
    let (_, _, commits) = get(&app, Some(&token), "/api/datasets/ps/commits").await;
    assert!(commits.contains("Property deleted"), "{commits}");
    assert!(commits.contains("Property restored"), "{commits}");

    // The states graph conforms to the OPM profile throughout.
    let (st, v, txt) = get(&app, Some(&token), "/api/datasets/ps/properties/validate").await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["report"]["conforms"], true, "{txt}");
}

/// `opm:Required` is a reliability like the others; `opm:documentation`
/// must be IRIs.
#[tokio::test]
async fn required_reliability_and_documentation() {
    let (state, token) = admin_state();
    dataset(&state, "ps", G, Visibility::Private);
    let app = test_app(state.clone());
    let (st, s, txt) = post(&app, &token, "/api/datasets/ps/properties/state", json!({
        "entity": E, "property": P, "value": "60", "reliability": "required",
        "documentation": ["https://example.org/ps/docs/brief", "https://example.org/ps/docs/appendix"]
    })).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    assert_eq!(s["reliability"], "required");
    let ask = format!(
        "ASK {{ GRAPH <urn:ots:property-states:ps> {{ <{}> a <{OPM}Required> ; <{OPM}documentation> <https://example.org/ps/docs/brief> }} }}",
        s["state"].as_str().unwrap()
    );
    assert!(matches!(
        state.store.query(&ask),
        Ok(QueryResults::Boolean(true))
    ));
    let (_, h, _) = get(&app, Some(&token), &history_uri("ps", E, P)).await;
    assert_eq!(h["states"][0]["reliability"], "required");
    assert_eq!(h["states"][0]["documentation"].as_array().unwrap().len(), 2);

    for bad in [
        json!(["not an iri"]),
        json!(["https://example.org/x> <urn:p> <urn:o"]),
    ] {
        let (st, _, txt) = post(
            &app,
            &token,
            "/api/datasets/ps/properties/state",
            json!({
                "entity": E, "property": P, "value": "1", "documentation": bad
            }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    }
    let (_, h, _) = get(&app, Some(&token), &history_uri("ps", E, P)).await;
    assert_eq!(
        h["states"].as_array().unwrap().len(),
        1,
        "nothing written by a refused call"
    );
}

/// Listing: per item, per property kind, filters, full history and an item
/// snapshot at a point in time.
#[tokio::test]
async fn listing_filters_and_item_snapshots() {
    let (state, token) = admin_state();
    dataset(&state, "ps", G, Visibility::Private);
    let app = test_app(state.clone());
    let e2 = "https://example.org/ps/bridge/b2";
    let span = "https://example.org/ps/span";
    for (e, p, v, from, rel) in [
        (E, P, "45", "2026-01-01", "confirmed"),
        (E, P, "40", "2026-06-01", "assumed"),
        (E, span, "120", "2026-01-01", "confirmed"),
        (e2, P, "30", "2026-02-01", "derived"),
        (e2, span, "80", "2026-02-01", "required"),
    ] {
        let (st, _, txt) = post(
            &app,
            &token,
            "/api/datasets/ps/properties/state",
            json!({
                "entity": e, "property": p, "value": v, "valid_from": from, "reliability": rel
            }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{txt}");
    }
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/ps/properties/delete",
        json!({
            "entity": e2, "property": span, "valid_from": "2026-07-01"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");

    let list = |q: &str| {
        let app = app.clone();
        let token = token.clone();
        let q = q.to_string();
        async move {
            let (st, v, txt) = get(
                &app,
                Some(&token),
                &format!("/api/datasets/ps/properties?{q}"),
            )
            .await;
            assert_eq!(st, StatusCode::OK, "{q}: {txt}");
            v
        }
    };
    let pairs = |v: &Value| -> Vec<(String, String, String)> {
        v["properties"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p["entity"]
                        .as_str()
                        .unwrap()
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .to_string(),
                    p["property"]
                        .as_str()
                        .unwrap()
                        .rsplit('/')
                        .next()
                        .unwrap()
                        .to_string(),
                    p["states"][0]["value"].as_str().unwrap_or("-").to_string(),
                )
            })
            .collect()
    };
    let t = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());

    // All live properties, latest state each; the deleted one is left out.
    let v = list("").await;
    assert_eq!(v["total"], 3, "{v}");
    assert_eq!(
        pairs(&v),
        vec![
            t("b1", "loadRating", "40"),
            t("b1", "span", "120"),
            t("b2", "loadRating", "30")
        ]
    );
    // Per item, per kind.
    let v = list(&format!("entity={}", url_encode(E))).await;
    assert_eq!(
        pairs(&v),
        vec![t("b1", "loadRating", "40"), t("b1", "span", "120")]
    );
    let v = list(&format!("property={}", url_encode(P))).await;
    assert_eq!(
        pairs(&v),
        vec![t("b1", "loadRating", "40"), t("b2", "loadRating", "30")]
    );
    // Filters.
    let v = list("reliability=confirmed").await;
    assert_eq!(
        pairs(&v),
        vec![t("b1", "span", "120")],
        "the latest b1 rating is assumed"
    );
    let v = list("derived=true").await;
    assert_eq!(pairs(&v), vec![t("b2", "loadRating", "30")]);
    let v = list("deleted=true").await;
    assert_eq!(pairs(&v), vec![t("b2", "span", "-")]);
    let v = list("deleted=any").await;
    assert_eq!(v["total"], 4);
    // Full history: every state, filters per state.
    let v = list(&format!("entity={}&history=full", url_encode(E))).await;
    assert_eq!(
        v["properties"][0]["states"].as_array().unwrap().len(),
        2,
        "{v}"
    );
    let v = list("history=full&reliability=confirmed").await;
    assert_eq!(
        pairs(&v),
        vec![t("b1", "loadRating", "45"), t("b1", "span", "120")],
        "{v}"
    );
    // Item snapshots in time.
    let v = list(&format!("entity={}&at=2026-03-01", url_encode(E))).await;
    assert_eq!(
        pairs(&v),
        vec![t("b1", "loadRating", "45"), t("b1", "span", "120")]
    );
    let v = list(&format!("entity={}&at=2026-03-01", url_encode(e2))).await;
    assert_eq!(
        pairs(&v),
        vec![t("b2", "loadRating", "30"), t("b2", "span", "80")]
    );
    let v = list(&format!("entity={}&at=2026-08-01", url_encode(e2))).await;
    assert_eq!(
        pairs(&v),
        vec![t("b2", "loadRating", "30")],
        "deleted by August"
    );
    let v = list(&format!("entity={}&at=2025-01-01", url_encode(E))).await;
    assert_eq!(v["total"], 0);
    // Paging.
    let v = list("limit=1&offset=1").await;
    assert_eq!(v["total"], 3);
    assert_eq!(pairs(&v), vec![t("b1", "span", "120")]);
    // Bad filters are a 400.
    for q in [
        "history=some",
        "deleted=maybe",
        "reliability=guessed",
        "at=yesterday",
    ] {
        let (st, _, _) = get(
            &app,
            Some(&token),
            &format!("/api/datasets/ps/properties?{q}"),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{q}");
    }
}

/// Export writes canonical OPM (`<item> <kind> <property>`, no server
/// bookkeeping); importing it into another dataset reproduces the history,
/// and exporting that gives the same document. A second import is a no-op.
#[tokio::test]
async fn canonical_export_import_round_trip() {
    let (state, token) = admin_state();
    dataset(&state, "src", G, Visibility::Private);
    let g2 = "https://example.org/ps/copy";
    dataset(&state, "dst", g2, Visibility::Private);
    let app = test_app(state.clone());
    for (v, from, rel) in [
        ("45", "2026-01-01", "confirmed"),
        ("40", "2026-06-01", "assumed"),
    ] {
        let (st, _, txt) = post(
            &app,
            &token,
            "/api/datasets/src/properties/state",
            json!({
                "entity": E, "property": P, "value": v, "valid_from": from, "reliability": rel,
                "note": "inspection", "documentation": ["https://example.org/ps/docs/d1"]
            }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{txt}");
    }
    let name = "https://example.org/ps/name";
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/src/properties/state",
        json!({
            "entity": E, "property": name, "value": "River bridge", "language": "en"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/src/properties/delete",
        json!({
            "entity": E, "property": name
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");

    // Turtle and N-Triples exports; canonical link present, bookkeeping absent.
    let (st, ttl) = export(&app, Some(&token), "src", "text/turtle").await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    assert!(ttl.contains("opm:hasPropertyState"), "{ttl}");
    let (_, nt) = export(&app, Some(&token), "src", "application/n-triples").await;
    let prop = format!(
        "<{}>",
        open_triplestore::property_states::property_iri(E, P)
    );
    assert!(
        nt.contains(&format!("<{E}> <{P}> {prop} .")),
        "canonical link: {nt}"
    );
    assert!(
        !nt.contains("propertyOf") && !nt.contains("dataGraph"),
        "{nt}"
    );
    let (_, jsonld) = export(&app, Some(&token), "src", "application/ld+json").await;
    assert!(serde_json::from_str::<Value>(&jsonld).is_ok(), "{jsonld}");

    // Import into the other dataset.
    let (st, txt) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/dst/properties/import",
        Some(&token),
        "text/turtle",
        ttl.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let report: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(report["imported_states"], 4, "{txt}");
    assert_eq!(report["properties"], 2, "{txt}");
    assert_eq!(report["rejected_count"], 0, "{txt}");
    assert_eq!(report["data_graph"], g2);
    assert_eq!(plain_values(&state, g2, E, P), vec!["40".to_string()]);
    assert!(
        plain_values(&state, g2, E, name).is_empty(),
        "deleted: no plain value"
    );

    // Same history, field by field.
    for p in [P, name] {
        let (_, a, _) = get(&app, Some(&token), &history_uri("src", E, p)).await;
        let (_, b, _) = get(&app, Some(&token), &history_uri("dst", E, p)).await;
        assert_eq!(a["property_iri"], b["property_iri"]);
        assert_eq!(a["states"], b["states"], "{p}");
    }
    // Same canonical document.
    let (_, nt2) = export(&app, Some(&token), "dst", "application/n-triples").await;
    assert_eq!(ntriples_set(&nt), ntriples_set(&nt2));

    // Idempotent.
    let (st, txt) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/dst/properties/import",
        Some(&token),
        "application/n-triples",
        nt.clone(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let report: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(report["imported_states"], 0, "{txt}");
    assert_eq!(report["skipped_duplicates"], 4, "{txt}");

    // The imported states graph conforms to the OPM profile.
    let (_, v, txt) = get(&app, Some(&token), "/api/datasets/dst/properties/validate").await;
    assert_eq!(v["report"]["conforms"], true, "{txt}");
}

/// Canonical OPM loaded straight into a dataset graph is read as it is
/// (history, as-of, listing); importing it makes it managed. A state OPM
/// says is malformed is rejected, not guessed at.
#[tokio::test]
async fn canonical_opm_is_read_and_imported() {
    let (state, token) = admin_state();
    let ig = "https://example.org/opm-profile/instances";
    dataset(&state, "canon", ig, Visibility::Private);
    let sample = include_str!("../examples/seed-bundles/opm-profile/instances.ttl");
    state
        .store
        .load_str(sample, RdfFormat::Turtle, Some(ig))
        .unwrap();
    let app = test_app(state.clone());
    let w = "https://example.org/opm-profile/window-1";
    let width = "https://example.org/opm-profile/width";
    let fire = "https://example.org/opm-profile/fireRating";

    let (st, h, txt) = get(&app, Some(&token), &history_uri("canon", w, width)).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let states = h["states"].as_array().unwrap();
    assert_eq!(states.len(), 2, "{txt}");
    assert_eq!(states[0]["value"], "1.25");
    assert_eq!(states[0]["current"], true);
    assert_eq!(states[0]["reliability"], "confirmed");
    assert_eq!(states[0]["canonical"], true);
    assert_eq!(
        states[0]["valid_from"], "2026-02-14T15:30:00Z",
        "no validFrom: generation time"
    );
    assert_eq!(
        h["property_iri"],
        "https://example.org/opm-profile/window-1-width"
    );
    let (_, a, _) = get(
        &app,
        Some(&token),
        &as_of_uri("canon", w, width, "2026-02-01"),
    )
    .await;
    assert_eq!(a["state"]["value"], "1.2");
    let (_, v, txt) = get(
        &app,
        Some(&token),
        &format!("/api/datasets/canon/properties?entity={}", url_encode(w)),
    )
    .await;
    assert_eq!(
        v["total"], 2,
        "width and height; the fire rating is deleted: {txt}"
    );
    let (_, v, _) = get(
        &app,
        Some(&token),
        "/api/datasets/canon/properties?reliability=required&history=full",
    )
    .await;
    assert_eq!(v["properties"][0]["property"], fire);

    // Import the same file into a second dataset: managed states, same IRIs.
    let g2 = "https://example.org/ps/managed";
    dataset(&state, "managed", g2, Visibility::Private);
    let (st, txt) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/managed/properties/import",
        Some(&token),
        "text/turtle",
        sample.to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let report: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(report["imported_states"], 5, "{txt}");
    assert_eq!(plain_values(&state, g2, w, width), vec!["1.25".to_string()]);
    assert!(plain_values(&state, g2, w, fire).is_empty());
    let (_, h, _) = get(&app, Some(&token), &history_uri("managed", w, width)).await;
    assert_eq!(
        h["states"][0]["state"],
        "https://example.org/opm-profile/window-1-width-2"
    );
    assert_eq!(h["states"][0]["canonical"], false);
    assert_eq!(
        h["states"][0]["documentation"],
        json!(["https://example.org/opm-profile/approval-letter-17"])
    );
    // A new state on an imported property extends its chain.
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/managed/properties/state",
        json!({
            "entity": w, "property": width, "value": "1.3"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (_, h, _) = get(&app, Some(&token), &history_uri("managed", w, width)).await;
    assert_eq!(
        h["property_iri"],
        "https://example.org/opm-profile/window-1-width"
    );
    assert_eq!(h["states"].as_array().unwrap().len(), 3);
    let (_, v, txt) = get(
        &app,
        Some(&token),
        "/api/datasets/managed/properties/validate",
    )
    .await;
    assert_eq!(v["report"]["conforms"], true, "{txt}");

    // Malformed states are rejected; nothing parseable is a 422; non-RDF a 415.
    let broken = r#"
        @prefix opm: <https://w3id.org/opm#> . @prefix schema: <http://schema.org/> .
        @prefix prov: <http://www.w3.org/ns/prov#> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        <https://example.org/x/item> <https://example.org/x/mass> [ opm:hasPropertyState
            [ a opm:CurrentPropertyState ; schema:value 12 ] ,
            [ a opm:OutdatedPropertyState ; prov:generatedAtTime "2026-01-01T00:00:00Z"^^xsd:dateTime ] ,
            [ a opm:OutdatedPropertyState ; schema:value 11 ; prov:generatedAtTime "2025-01-01T00:00:00Z"^^xsd:dateTime ] ] ."#;
    let (st, txt) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/managed/properties/import",
        Some(&token),
        "text/turtle",
        broken.to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let report: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(
        report["imported_states"], 1,
        "only the well-formed state: {txt}"
    );
    assert_eq!(report["rejected_count"], 2, "{txt}");
    // http://schema.org/value is read as the value.
    assert_eq!(
        plain_values(
            &state,
            g2,
            "https://example.org/x/item",
            "https://example.org/x/mass"
        ),
        vec!["11".to_string()]
    );
    let (st, _) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/managed/properties/import",
        Some(&token),
        "text/turtle",
        "<urn:a> <urn:b> <urn:c> .".to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/managed/properties/import",
        Some(&token),
        "text/csv",
        "a,b".to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let (st, _) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/managed/properties/import",
        Some(&token),
        "text/turtle",
        "not turtle {".to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
}

/// The OPM profile shapes catch what OPM forbids.
#[tokio::test]
async fn the_opm_profile_flags_malformed_states() {
    let (state, token) = admin_state();
    dataset(&state, "bad", G, Visibility::Private);
    state
        .auth_db
        .add_dataset_graph("bad", "urn:ots:property-states:bad")
        .unwrap();
    state
        .store
        .load_str(
            r#"@prefix opm: <https://w3id.org/opm#> . @prefix schema: <https://schema.org/> .
               @prefix prov: <http://www.w3.org/ns/prov#> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
               <urn:p1> a opm:Property ; opm:hasPropertyState <urn:s1>, <urn:s2> .
               <urn:s1> a opm:PropertyState, opm:CurrentPropertyState, opm:Assumed, opm:Confirmed ; schema:value 1 ;
                   prov:generatedAtTime "2026-01-01T00:00:00Z"^^xsd:dateTime ; prov:wasAttributedTo <urn:a> .
               <urn:s2> a opm:PropertyState, opm:CurrentPropertyState, opm:Deleted ; schema:value 2 ; prov:wasAttributedTo <urn:a> .
               <urn:s3> a opm:PropertyState, opm:OutdatedPropertyState, opm:Derived ; opm:expression "?a + 1" ; schema:value 3 ;
                   prov:generatedAtTime "2026-01-01T00:00:00Z"^^xsd:dateTime ; prov:wasAttributedTo <urn:a> ."#,
            RdfFormat::Turtle,
            Some("urn:ots:property-states:bad"),
        )
        .unwrap();
    let app = test_app(state.clone());
    let (st, v, txt) = get(&app, Some(&token), "/api/datasets/bad/properties/validate").await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["report"]["conforms"], false, "{txt}");
    let focus: Vec<&str> = v["report"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["severity"] != "Warning" && r["severity"] != "warning")
        .filter_map(|r| r["focus_node"].as_str())
        .collect();
    for f in ["urn:p1", "urn:s1", "urn:s2", "urn:s3"] {
        assert!(focus.iter().any(|x| x.contains(f)), "{f} flagged: {txt}");
    }
    // The shapes are served for use as a dataset shapes graph.
    let (st, _, ttl) = get(&app, None, "/api/properties/profile").await;
    assert_eq!(st, StatusCode::OK);
    assert!(ttl.contains("sh:NodeShape"), "{ttl}");
}

/// A value recorded in a private graph must not reach viewers through the
/// states graph: not in history, listings, as-of or export.
#[tokio::test]
async fn states_of_private_graph_values_stay_private() {
    let (state, token) = admin_state();
    dataset(&state, "pub", G, Visibility::Public);
    let secret = "https://example.org/ps/secret";
    state.auth_db.add_dataset_graph("pub", secret).unwrap();
    state
        .auth_db
        .set_dataset_graph_private("pub", secret, true)
        .unwrap();
    let app = test_app(state.clone());
    let cost = "https://example.org/ps/cost";
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/pub/properties/state",
        json!({
            "entity": E, "property": cost, "value": "1000000", "graph": secret
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/pub/properties/state",
        json!({
            "entity": E, "property": P, "value": "45"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");

    // The writer sees both.
    let (_, v, _) = get(&app, Some(&token), "/api/datasets/pub/properties").await;
    assert_eq!(v["total"], 2);
    // A viewer (anonymous, public dataset) sees only the public one.
    let (st, v, txt) = get(&app, None, "/api/datasets/pub/properties").await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(v["total"], 1, "{txt}");
    assert_eq!(v["properties"][0]["property"], P);
    let (_, h, _) = get(&app, None, &history_uri("pub", E, cost)).await;
    assert_eq!(h["states"].as_array().unwrap().len(), 0, "{h}");
    let (st, _, _) = get(&app, None, &as_of_uri("pub", E, cost, "2030-01-01")).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (_, nt) = export(&app, None, "pub", "application/n-triples").await;
    assert!(!nt.contains("1000000") && !nt.contains(cost), "{nt}");
    assert!(nt.contains(P), "{nt}");
    // And a viewer cannot write.
    state
        .auth_db
        .create_user("eve", "eve", "eve@t.com", "h", SystemRole::User)
        .unwrap();
    let eve = mint_token("eve", "eve", "user");
    for (uri, body) in [
        (
            "/api/datasets/pub/properties/delete",
            json!({ "entity": E, "property": P }),
        ),
        (
            "/api/datasets/pub/properties/restore",
            json!({ "entity": E, "property": P }),
        ),
    ] {
        let (st, _, _) = post(&app, &eve, uri, body).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{uri}");
    }
    let (st, _) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/pub/properties/import",
        Some(&eve),
        "text/turtle",
        nt,
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(plain_values(&state, G, E, P), vec!["45".to_string()]);
}

// ─── OPM calculations (M4) ──────────────────────────────────────────────────

const EX: &str = "https://example.org/calc/";

fn ex(local: &str) -> String {
    format!("{EX}{local}")
}

/// A dataset with two windows: w1 has width and height, w2 only a width;
/// a wall the windows are part of has a height.
async fn calc_fixture() -> (open_triplestore::server::AppState, String, Router) {
    let (state, token) = admin_state();
    dataset(&state, "calc", &ex("instances"), Visibility::Private);
    state
        .store
        .load_str(
            &format!(
                "<{w1}> a <{win}> ; <{part}> <{wall}> . <{w2}> a <{win}> . <{wall}> a <{wl}> .",
                w1 = ex("w1"),
                w2 = ex("w2"),
                win = ex("Window"),
                wl = ex("Wall"),
                wall = ex("wall"),
                part = ex("partOf"),
            ),
            RdfFormat::Turtle,
            Some(&ex("instances")),
        )
        .unwrap();
    let app = test_app(state.clone());
    for (e, p, v) in [
        ("w1", "width", "1.25"),
        ("w1", "height", "1.5"),
        ("w2", "width", "0.8"),
        ("wall", "height", "3.0"),
    ] {
        let (st, _, txt) = post(&app, &token, "/api/datasets/calc/properties/state", json!({
            "entity": ex(e), "property": ex(p), "value": v, "datatype": "xsd:decimal", "reliability": "confirmed"
        })).await;
        assert_eq!(st, StatusCode::CREATED, "{txt}");
    }
    (state, token, app)
}

fn area_calc() -> Value {
    json!({
        "label": "Window area",
        "inferred_property": "ex:area",
        "argument_paths": ["?foi ex:width ?w", "?foi ex:height ?h"],
        "expression": "?w * ?h",
        "prefixes": { "ex": EX },
    })
}

async fn define(app: &Router, token: &str, body: Value) -> (StatusCode, Value, String) {
    post(
        app,
        token,
        "/api/datasets/calc/properties/calculations",
        body,
    )
    .await
}

async fn run_calc(
    app: &Router,
    token: &str,
    method: Method,
    id: &str,
) -> (StatusCode, Value, String) {
    req(
        app,
        method,
        &format!(
            "/api/datasets/calc/properties/calculations/{}",
            url_encode(id)
        ),
        Some(token),
        None,
    )
    .await
}

/// OPM's REST guidance: POST to a calculation derives the property for every
/// feature of interest that has the arguments; PUT recomputes where an
/// argument state has been outdated. A derived state carries the expression
/// and an `rdf:Seq` of the argument states.
#[tokio::test]
async fn calculations_derive_on_post_and_recompute_on_put() {
    let (state, token, app) = calc_fixture().await;
    let (st, c, txt) = define(&app, &token, area_calc()).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let id = c["id"].as_str().unwrap().to_string();
    assert_eq!(c["inferred_property"], ex("area"));
    assert_eq!(
        c["argument_paths"][0],
        format!("?foi <{}> ?w", ex("width")),
        "stored with full IRIs: {txt}"
    );
    assert_eq!(c["expression"], "(?w * ?h)");
    let (_, list, _) = get(
        &app,
        Some(&token),
        "/api/datasets/calc/properties/calculations",
    )
    .await;
    assert_eq!(list["calculations"].as_array().unwrap().len(), 1);

    // POST: w1 gets an area; w2 has no height, so no match.
    let (st, r, txt) = run_calc(&app, &token, Method::POST, &id).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(r["derived_count"], 1, "{txt}");
    assert_eq!(r["derived"][0]["foi"], ex("w1"));
    assert_eq!(
        plain_values(&state, &ex("instances"), &ex("w1"), &ex("area")),
        vec!["1.875".to_string()]
    );
    let (_, w, _) = get(
        &app,
        Some(&token),
        &history_uri("calc", &ex("w1"), &ex("width")),
    )
    .await;
    let (_, h, _) = get(
        &app,
        Some(&token),
        &history_uri("calc", &ex("w1"), &ex("height")),
    )
    .await;
    let (_, a, txt) = get(
        &app,
        Some(&token),
        &history_uri("calc", &ex("w1"), &ex("area")),
    )
    .await;
    let derived = &a["states"][0];
    assert_eq!(derived["reliability"], "derived", "{txt}");
    assert_eq!(derived["expression"], "(?w * ?h)");
    assert_eq!(derived["calculation"], c["calculation"]);
    assert_eq!(
        derived["arguments"],
        json!([w["states"][0]["state"], h["states"][0]["state"]]),
        "the rdf:Seq names the argument states in order: {txt}"
    );
    let seq = derived["derived_from"].as_str().unwrap();
    assert!(matches!(
        state.store.query(&format!("ASK {{ GRAPH <urn:ots:property-states:calc> {{ <{seq}> a <http://www.w3.org/1999/02/22-rdf-syntax-ns#Seq> }} }}")),
        Ok(QueryResults::Boolean(true))
    ));

    // A second POST leaves w1 alone; nothing is outdated yet.
    let (_, r, txt) = run_calc(&app, &token, Method::POST, &id).await;
    assert_eq!(r["derived_count"], 0, "{txt}");
    assert!(
        r["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["foi"] == ex("w1")),
        "{txt}"
    );
    let outdated_uri = format!(
        "/api/datasets/calc/properties/calculations/{}/outdated",
        url_encode(&id)
    );
    let (_, o, _) = get(&app, Some(&token), &outdated_uri).await;
    assert_eq!(o["outdated_count"], 0);

    // A new width outdates the derived area; PUT recomputes it, POST would not.
    let (st, _, txt) = post(
        &app,
        &token,
        "/api/datasets/calc/properties/state",
        json!({
            "entity": ex("w1"), "property": ex("width"), "value": "1.3", "datatype": "xsd:decimal"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (_, o, txt) = get(&app, Some(&token), &outdated_uri).await;
    assert_eq!(o["outdated_count"], 1, "{txt}");
    assert_eq!(o["outdated"][0]["foi"], ex("w1"));
    assert_eq!(
        o["outdated"][0]["outdated_arguments"],
        json!([w["states"][0]["state"]])
    );
    let (st, r, txt) = run_calc(&app, &token, Method::PUT, &id).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(r["derived_count"], 1, "{txt}");
    assert_eq!(
        plain_values(&state, &ex("instances"), &ex("w1"), &ex("area")),
        vec!["1.95".to_string()]
    );
    let (_, o, _) = get(&app, Some(&token), &outdated_uri).await;
    assert_eq!(o["outdated_count"], 0);
    let (_, a, _) = get(
        &app,
        Some(&token),
        &history_uri("calc", &ex("w1"), &ex("area")),
    )
    .await;
    assert_eq!(
        a["states"].as_array().unwrap().len(),
        2,
        "the old derived state is outdated, not removed"
    );
    let (_, r, _) = run_calc(&app, &token, Method::PUT, &id).await;
    assert_eq!(r["derived_count"], 0, "nothing left to recompute");

    // Parentheses survive storage: (w + 1) * h, not w + (1 * h).
    let (st, c2, txt) = define(&app, &token, json!({
        "inferred_property": ex("framedArea"),
        "argument_paths": [format!("?foi <{}> ?w", ex("width")), format!("?foi <{}> ?h", ex("height"))],
        "expression": "(?w + 0.1) * ?h",
    })).await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (_, r, txt) = run_calc(&app, &token, Method::POST, c2["id"].as_str().unwrap()).await;
    assert_eq!(r["derived_count"], 1, "{txt}");
    assert_eq!(
        plain_values(&state, &ex("instances"), &ex("w1"), &ex("framedArea")),
        vec!["2.1".to_string()]
    );

    // One commit per run; the states graph still fits the OPM profile.
    let (_, _, commits) = get(&app, Some(&token), "/api/datasets/calc/commits").await;
    assert!(
        commits.contains("(POST): 1 derived states") && commits.contains("(PUT): 1 derived states"),
        "{commits}"
    );
    let (_, v, txt) = get(&app, Some(&token), "/api/datasets/calc/properties/validate").await;
    assert_eq!(v["report"]["conforms"], true, "{txt}");

    // Deleting the definition keeps what it derived.
    let (st, _, _) = run_calc(&app, &token, Method::DELETE, &id).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _, _) = run_calc(&app, &token, Method::GET, &id).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(
        plain_values(&state, &ex("instances"), &ex("w1"), &ex("area")),
        vec!["1.95".to_string()]
    );
}

/// `opm:foiRestriction`, `opm:pathRestriction` and an argument found on
/// another feature of interest (`?foi ex:partOf/ex:height ?h`): the
/// derivation points at the wall's height state.
#[tokio::test]
async fn calculation_restrictions_and_longer_paths() {
    let (_state, token, app) = calc_fixture().await;
    // Only w2.
    let (st, c, txt) = define(
        &app,
        &token,
        json!({
            "inferred_property": ex("halfWidth"),
            "argument_paths": [format!("?foi <{}> ?w", ex("width"))],
            "expression": "?w / 2",
            "foi_restriction": ex("w2"),
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (_, r, txt) = run_calc(&app, &token, Method::POST, c["id"].as_str().unwrap()).await;
    assert_eq!(r["derived_count"], 1, "{txt}");
    assert_eq!(r["derived"][0]["foi"], ex("w2"));
    assert_eq!(r["derived"][0]["value"], "0.4");
    // Only windows — the wall has a height but is not a window.
    let (_, c, txt) = define(
        &app,
        &token,
        json!({
            "inferred_property": ex("heightCm"),
            "argument_paths": [format!("?foi <{}> ?h", ex("height"))],
            "expression": "?h * 100",
            "path_restriction": format!("?foi a <{}>", ex("Window")),
        }),
    )
    .await;
    assert_eq!(
        c["path_restriction"],
        format!(
            "?foi <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{}>",
            ex("Window")
        ),
        "{txt}"
    );
    let (_, r, txt) = run_calc(&app, &token, Method::POST, c["id"].as_str().unwrap()).await;
    let fois: Vec<&str> = r["derived"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["foi"].as_str().unwrap())
        .collect();
    assert_eq!(fois, vec![ex("w1").as_str()], "{txt}");
    // The wall's height through partOf.
    let (st, c, txt) = define(
        &app,
        &token,
        json!({
            "inferred_property": ex("wallHeight"),
            "argument_paths": ["?foi ex:partOf/ex:height ?wh"],
            "expression": "?wh",
            "prefixes": { "ex": EX },
        }),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{txt}");
    let (_, r, txt) = run_calc(&app, &token, Method::POST, c["id"].as_str().unwrap()).await;
    assert_eq!(r["derived_count"], 1, "{txt}");
    let (_, wall, _) = get(
        &app,
        Some(&token),
        &history_uri("calc", &ex("wall"), &ex("height")),
    )
    .await;
    assert_eq!(
        r["derived"][0]["derived_from"],
        json!([wall["states"][0]["state"]]),
        "{txt}"
    );
}

/// Paths, restrictions and expressions are parsed and allow-listed; none of
/// their text reaches the store, nothing is written for a refused one, and a
/// calculation reads only the dataset's own graphs.
#[tokio::test]
async fn calculation_injection_is_refused() {
    let (state, token, app) = calc_fixture().await;
    let graphs_before = state.store.store().named_graphs().count();
    let bad_paths = [
        "?foi ex:width ?w . SERVICE <http://127.0.0.1:9/sparql> { ?s ?p ?o }",
        "?foi ex:width ?w } UNION { GRAPH ?g { ?foi ?any ?w }",
        "?foi ex:width ?w FILTER EXISTS { GRAPH <urn:secret> { ?a ?b ?c } }",
        "GRAPH <urn:secret> { ?foi ex:width ?w }",
        "?foi ?pred ?w",
        "?foi ex:width ?w . ?foi ex:height ?h",
        "{ SELECT ?foi ?w WHERE { ?foi ex:width ?w } }",
        "?foi ex:width ?w } ; INSERT DATA { GRAPH <urn:probe> { <urn:a> <urn:b> <urn:c> } } #",
        "?foi ex:width ?w OPTIONAL { ?foi ex:height ?w }",
        "?foi ex:width ?w } LIMIT 1 #",
        "?foi (ex:width|ex:height) ?w",
        "?foi ex:width ?w VALUES ?w { 1 }",
        "?foi ex:width ?w . BIND(1 AS ?x)",
        "?foi ex:width ?__v0",
        "ex:w1 ex:width ?w",
        "",
    ];
    for path in bad_paths {
        let (st, _, txt) = define(&app, &token, json!({
            "inferred_property": "ex:x", "argument_paths": [path], "expression": "?w", "prefixes": { "ex": EX },
        })).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{path}: {txt}");
    }
    let bad_expressions = [
        "EXISTS { ?s ?p ?o }",
        "?w * 2) AS ?r) ?x WHERE { ?s ?p ?x } #",
        "?w } ; DROP ALL #",
        "?nope + 1",
        "<http://www.opengis.net/def/function/geosparql/distance>(?w, ?w)",
        "NOW()",
        "RAND() * ?w",
        "SUM(?w)",
        "IRI(STR(?w))",
        "REGEX(STR(?w), \"(a+)+$\")",
        "?w + (SELECT ?x WHERE { ?s ?p ?x })",
    ];
    for expr in bad_expressions {
        let (st, _, txt) = define(&app, &token, json!({
            "inferred_property": "ex:x", "argument_paths": ["?foi ex:width ?w"], "expression": expr, "prefixes": { "ex": EX },
        })).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{expr}: {txt}");
    }
    for (pr, foi) in [
        (
            Some("?foi a ex:Window . GRAPH <urn:secret> { ?foi ?p ?o }"),
            None,
        ),
        (Some("?foi ex:partOf ?wall"), None),
        (Some("?foi a ex:Window } UNION { ?foi ?p ?o"), None),
        (None, Some("not an iri")),
    ] {
        let (st, _, txt) = define(&app, &token, json!({
            "inferred_property": "ex:x", "argument_paths": ["?foi ex:width ?w"], "expression": "?w",
            "prefixes": { "ex": EX }, "path_restriction": pr, "foi_restriction": foi,
        })).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{pr:?} {foi:?}: {txt}");
    }
    // Nothing was stored or written.
    let (_, list, _) = get(
        &app,
        Some(&token),
        "/api/datasets/calc/properties/calculations",
    )
    .await;
    assert_eq!(list["calculations"].as_array().unwrap().len(), 0, "{list}");
    assert!(!matches!(
        state.store.query("ASK { GRAPH <urn:probe> { ?s ?p ?o } }"),
        Ok(QueryResults::Boolean(true))
    ));
    assert_eq!(state.store.store().named_graphs().count(), graphs_before);

    // A calculation written straight into the states graph is re-validated
    // when it is loaded: listed as invalid, refused when run.
    state
        .store
        .update(&format!(
            r#"PREFIX opm: <https://w3id.org/opm#> PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
            INSERT DATA {{ GRAPH <urn:ots:property-states:calc> {{
              <urn:ots:calculation:tampered> a opm:Calculation ; opm:inferredProperty <{x}> ; opm:expression "?w" ;
                opm:argumentPaths ( "?foi <{w}> ?w . SERVICE <http://127.0.0.1:9/> {{ ?s ?p ?o }}" ) .
            }} }}"#,
            x = ex("x"),
            w = ex("width")
        ))
        .unwrap();
    let (_, c, txt) = run_calc(&app, &token, Method::GET, "tampered").await;
    assert_eq!(c["valid"], false, "{txt}");
    let (st, _, txt) = run_calc(&app, &token, Method::POST, "tampered").await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");

    // Another dataset's graph is out of reach: w3's width and height there
    // match the paths, but nothing is derived for it.
    let other = "https://example.org/calc/other-dataset";
    dataset(&state, "other", other, Visibility::Private);
    state
        .store
        .load_str(
            &format!(
                "<{w3}> <{w}> 2.0 ; <{h}> 3.0 .",
                w3 = ex("w3"),
                w = ex("width"),
                h = ex("height")
            ),
            RdfFormat::Turtle,
            Some(other),
        )
        .unwrap();
    for p in ["width", "height"] {
        let (st, _, txt) = post(
            &app,
            &token,
            "/api/datasets/other/properties/state",
            json!({
                "entity": ex("w3"), "property": ex(p), "value": "2.0", "datatype": "xsd:decimal"
            }),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "{txt}");
    }
    let (_, c, _) = define(&app, &token, area_calc()).await;
    let (_, r, txt) = run_calc(&app, &token, Method::POST, c["id"].as_str().unwrap()).await;
    let fois: Vec<&str> = r["derived"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["foi"].as_str().unwrap())
        .collect();
    assert_eq!(fois, vec![ex("w1").as_str()], "{txt}");

    // Only writers define or run calculations.
    state
        .auth_db
        .create_user("eve", "eve", "eve@t.com", "h", SystemRole::User)
        .unwrap();
    let eve = mint_token("eve", "eve", "user");
    let (st, _, _) = define(&app, &eve, area_calc()).await;
    assert!(
        st == StatusCode::NOT_FOUND || st == StatusCode::FORBIDDEN,
        "{st}"
    );
    let (st, _, _) = run_calc(&app, &eve, Method::POST, c["id"].as_str().unwrap()).await;
    assert!(
        st == StatusCode::NOT_FOUND || st == StatusCode::FORBIDDEN,
        "{st}"
    );
}

/// Calculations and derived states travel through canonical export and
/// import; a canonical calculation written with prefixed names imports with
/// the document's prefixes.
#[tokio::test]
async fn calculations_round_trip_through_export_and_import() {
    let (state, token, app) = calc_fixture().await;
    let (_, c, _) = define(&app, &token, area_calc()).await;
    let (_, r, txt) = run_calc(&app, &token, Method::POST, c["id"].as_str().unwrap()).await;
    assert_eq!(r["derived_count"], 1, "{txt}");
    let (_, ttl) = export(&app, Some(&token), "calc", "text/turtle").await;
    assert!(
        ttl.contains("opm:Calculation") && ttl.contains("opm:argumentPaths"),
        "{ttl}"
    );
    let g2 = ex("copy");
    dataset(&state, "copy", &g2, Visibility::Private);
    let (st, txt) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/copy/properties/import",
        Some(&token),
        "text/turtle",
        ttl,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let report: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(report["imported_calculations"], 1, "{txt}");
    let (_, list, _) = get(
        &app,
        Some(&token),
        "/api/datasets/copy/properties/calculations",
    )
    .await;
    assert_eq!(list["calculations"][0]["calculation"], c["calculation"]);
    assert_eq!(list["calculations"][0]["valid"], true);
    let (_, a, _) = get(
        &app,
        Some(&token),
        &history_uri("calc", &ex("w1"), &ex("area")),
    )
    .await;
    let (_, b, _) = get(
        &app,
        Some(&token),
        &history_uri("copy", &ex("w1"), &ex("area")),
    )
    .await;
    assert_eq!(a["states"], b["states"]);

    // A calculation as another OPM tool writes it: prefixed names in the
    // argument paths, resolved with the document's own prefixes.
    let canonical = format!(
        r#"@prefix opm: <https://w3id.org/opm#> . @prefix props: <{EX}> .
        props:volumeCalc a opm:Calculation ;
            opm:inferredProperty props:volume ;
            opm:argumentPaths ( "?foi props:width ?width" "?foi props:height ?height" ) ;
            opm:expression "?width * ?height * 0.1" ."#
    );
    let (st, txt) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/calc/properties/import",
        Some(&token),
        "text/turtle",
        canonical,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let report: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(report["imported_calculations"], 1, "{txt}");
    let (_, r, txt) = run_calc(&app, &token, Method::POST, &ex("volumeCalc")).await;
    assert_eq!(r["derived_count"], 1, "{txt}");
    assert_eq!(r["derived"][0]["value"], "0.1875");
    // An invalid canonical calculation is reported, not stored.
    let evil = r#"@prefix opm: <https://w3id.org/opm#> .
        <urn:evil> a opm:Calculation ; opm:inferredProperty <urn:x> ;
            opm:argumentPaths ( "?foi <urn:p> ?a . SERVICE <http://127.0.0.1:9/> { ?s ?p ?o }" ) ; opm:expression "?a" ."#;
    let (st, txt) = send_raw(
        &app,
        Method::POST,
        "/api/datasets/calc/properties/import",
        Some(&token),
        "text/turtle",
        evil.to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let report: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(report["imported_calculations"], 0, "{txt}");
    assert_eq!(
        report["rejected_calculations"].as_array().unwrap().len(),
        1,
        "{txt}"
    );
}
