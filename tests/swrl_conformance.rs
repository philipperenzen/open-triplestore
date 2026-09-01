//! SWRL rule-engine conformance — semantics, not wiring.
//!
//! The previous coverage was a single smoke test that posted `{}` and asserted
//! the response was "not 404 and not 500", which would have passed if the
//! handler rejected every request. These tests assert what the engine actually
//! does: that a rule *derives* the triples its head declares, that a rule whose
//! body uses an untranslatable builtin is refused rather than fired without its
//! guard, and that execution is gated by the per-graph write ACL.

#![cfg(feature = "swrl")]

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::server::AppState;
use oxigraph::sparql::QueryResults;
use serde_json::json;
use tower::ServiceExt as _;

async fn post_swrl(app: &Router, token: &str, body: serde_json::Value) -> (StatusCode, String) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/swrl/execute")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    (status, body_text(resp.into_body()).await)
}

fn ask(state: &AppState, pattern: &str) -> bool {
    matches!(
        state.store.query(&format!("ASK {{ {pattern} }}")),
        Ok(QueryResults::Boolean(true))
    )
}

/// The engine must actually infer: `parentOf(?x,?y) ^ parentOf(?y,?z) ->
/// grandparentOf(?x,?z)` over asserted parent links derives the grandparent
/// triple that was never asserted.
#[tokio::test]
async fn swrl_rule_derives_new_triples() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            r#"<http://ex/a> <http://ex/parentOf> <http://ex/b> .
               <http://ex/b> <http://ex/parentOf> <http://ex/c> ."#,
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let app = test_app(state.clone());

    assert!(
        !ask(
            &state,
            "<http://ex/a> <http://ex/grandparentOf> <http://ex/c>"
        ),
        "the conclusion must not be asserted up front"
    );

    // The text form takes each predicate verbatim as an IRI, so bare names would
    // become relative IRIs and fail to parse.
    let (st, body) = post_swrl(
        &app,
        &token,
        json!({
            "rules": "http://ex/parentOf(?x, ?y) ^ http://ex/parentOf(?y, ?z) \
                      -> http://ex/grandparentOf(?x, ?z)",
            "format": "text",
            "max_iterations": 5
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "rule execution failed: {body}");

    assert!(
        ask(
            &state,
            "<http://ex/a> <http://ex/grandparentOf> <http://ex/c>"
        ),
        "the rule must derive the grandparent triple; response was: {body}"
    );
}

/// A rule whose body uses a builtin the engine cannot translate must be
/// reported as failed and must not fire. The engine used to drop the
/// untranslatable FILTER and run the rest, asserting the head for every
/// binding — so this rule would have tagged every Person as LongName.
///
/// Only the OWL/XML form can carry a builtin: the text form turns every
/// two-argument atom into a property atom (`parse_single_atom`), so it cannot
/// express `swrlb:` builtins at all.
#[tokio::test]
async fn swrl_unsupported_builtin_does_not_fire_unguarded() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            r#"<http://ex/p1> a <http://ex/Person> ; <http://ex/name> "Bo" ."#,
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let app = test_app(state.clone());

    // `stringLength` is not among the translatable builtins.
    let xml = r#"<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#">
  <DLSafeRule>
    <Body>
      <ClassAtom>
        <Class IRI="http://ex/Person"/>
        <Variable IRI="urn:swrl:var#x"/>
      </ClassAtom>
      <BuiltinAtom IRI="http://www.w3.org/2003/11/swrlb#stringLength">
        <Variable IRI="urn:swrl:var#n"/>
        <Variable IRI="urn:swrl:var#len"/>
      </BuiltinAtom>
    </Body>
    <Head>
      <ClassAtom>
        <Class IRI="http://ex/LongName"/>
        <Variable IRI="urn:swrl:var#x"/>
      </ClassAtom>
    </Head>
  </DLSafeRule>
</Ontology>"#;

    let (st, body) = post_swrl(
        &app,
        &token,
        json!({ "rules": xml, "format": "xml", "max_iterations": 3 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "endpoint should answer: {body}");
    assert!(
        !ask(&state, "<http://ex/p1> a <http://ex/LongName>"),
        "a rule with an untranslatable guard must not assert its head: {body}"
    );
    assert!(
        body.contains("Unsupported SWRL builtin"),
        "the failure must be reported to the caller: {body}"
    );
}

/// Execution INSERTs into `target_graph`. The handler took no authenticated
/// user at all, so any caller could materialise triples into any graph —
/// including another tenant's and the shared `urn:entailment:*` graphs.
#[tokio::test]
async fn swrl_execute_denies_write_to_ungranted_graph() {
    let (state, _admin) = admin_state();
    state
        .auth_db
        .create_user(
            "mallory",
            "mallory",
            "mallory@test.com",
            "hash",
            open_triplestore::auth::models::SystemRole::User,
        )
        .unwrap();
    let mallory = mint_token("mallory", "mallory", "user");
    let app = test_app(state.clone());

    let (st, body) = post_swrl(
        &app,
        &mallory,
        json!({
            "rules": "Person(?x) -> Tagged(?x)",
            "format": "text",
            "target_graph": "urn:entailment:owl2-rl"
        }),
    )
    .await;
    assert!(
        st == StatusCode::FORBIDDEN || st == StatusCode::UNAUTHORIZED,
        "a non-admin must not write a graph they hold no grant on, got {st}: {body}"
    );
}
