//! SWRL rule-engine conformance — semantics, not wiring.
//!
//! The previous coverage was a single smoke test that posted `{}` and asserted
//! the response was "not 404 and not 500", which would have passed if the
//! handler rejected every request. These tests assert what the engine actually
//! does: that a rule *derives* the triples its head declares, that a rule whose
//! body uses an untranslatable builtin is refused rather than fired without its
//! guard, and that execution is gated by the per-graph write ACL.
//!
//! The OWL/XML reader is strict: an atom it does not understand refuses the
//! document instead of vanishing from the rule (the `BuiltInAtom` spelling the
//! OWL API writes used to vanish, taking its guard with it). Rules are checked
//! for safety and typed argument positions before anything runs, the target
//! graph must be an IRI, and the report says whether the fixed point was
//! reached.

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
            r#"<http://ex/p1> a <http://ex/Person> ; <http://ex/name> "Bo" ; <http://ex/nameLength> 2 ."#,
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let app = test_app(state.clone());

    // `stringLength` is not among the translatable builtins. Its variables are
    // bound by body atoms, so the refusal is about the builtin, not safety.
    let xml = r#"<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#">
  <DLSafeRule>
    <Body>
      <ClassAtom>
        <Class IRI="http://ex/Person"/>
        <Variable IRI="urn:swrl:var#x"/>
      </ClassAtom>
      <DataPropertyAtom>
        <DataProperty IRI="http://ex/name"/>
        <Variable IRI="urn:swrl:var#x"/>
        <Variable IRI="urn:swrl:var#n"/>
      </DataPropertyAtom>
      <DataPropertyAtom>
        <DataProperty IRI="http://ex/nameLength"/>
        <Variable IRI="urn:swrl:var#x"/>
        <Variable IRI="urn:swrl:var#len"/>
      </DataPropertyAtom>
      <BuiltinAtom IRI="http://www.w3.org/2003/11/swrlb#stringLength">
        <Variable IRI="urn:swrl:var#len"/>
        <Variable IRI="urn:swrl:var#n"/>
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
    // Refused before anything runs: a rule set that cannot run as written is
    // not run in part.
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        !ask(&state, "<http://ex/p1> a <http://ex/LongName>"),
        "a rule with an untranslatable guard must not assert its head: {body}"
    );
    assert!(
        body.contains("Unsupported SWRL builtin") && body.contains("stringLength"),
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

/// Class and property predicates are validated like arguments. They used to be
/// pasted between `<…>` after only trimming angle brackets, so a body class
/// such as `http://ex/Person> } ; INSERT DATA { GRAPH <urn:probe> {…} } ; …`
/// closed the WHERE clause and appended its own operations — into any graph,
/// since the generated text bypassed `TripleStore::update`. Now the request is
/// refused at parse time and the store is untouched.
#[tokio::test]
async fn swrl_rejects_class_and_property_iris_that_are_not_iris() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            r#"<http://ex/p1> a <http://ex/Person> ."#,
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let app = test_app(state.clone());
    let graphs_before = state.store.store().named_graphs().count();
    let probe_empty = |s: &AppState| !ask(s, "GRAPH <urn:probe> { ?s ?p ?o }");
    assert!(probe_empty(&state));

    // Text form: the body class predicate carries `>`, `;`, `}` and spaces.
    let payload = "http://ex/Person> } ; INSERT DATA { GRAPH <urn:probe> { <urn:s> <urn:p> <urn:o> } } ; INSERT { } WHERE { ?z a <http://ex/Q";
    let (st, body) = post_swrl(
        &app,
        &token,
        json!({
            "rules": format!("{payload}(?x) -> http://ex/T(?x)"),
            "format": "text",
            "max_iterations": 3
        }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("Invalid SWRL predicate IRI"), "{body}");
    assert!(
        probe_empty(&state),
        "the injected INSERT DATA must not have run"
    );
    assert!(
        !ask(&state, "<http://ex/p1> a <http://ex/T>"),
        "the rule itself must not have fired"
    );

    // OWL/XML form: the property IRI carries `>` and `;`.
    let xml = r#"<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#">
  <DLSafeRule>
    <Body>
      <ClassAtom>
        <Class IRI="http://ex/Person"/>
        <Variable IRI="urn:swrl:var#x"/>
      </ClassAtom>
    </Body>
    <Head>
      <ObjectPropertyAtom>
        <ObjectProperty IRI="http://ex/T> } } ; INSERT DATA { GRAPH &lt;urn:probe> { &lt;urn:s> &lt;urn:p> &lt;urn:o> } } ; INSERT { } WHERE { ?z &lt;http://ex/q"/>
        <Variable IRI="urn:swrl:var#x"/>
        <Variable IRI="urn:swrl:var#x"/>
      </ObjectPropertyAtom>
    </Head>
  </DLSafeRule>
</Ontology>"#;
    let (st, body) = post_swrl(
        &app,
        &token,
        json!({ "rules": xml, "format": "xml", "max_iterations": 3 }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("Invalid SWRL property IRI"), "{body}");
    assert!(probe_empty(&state));
    assert_eq!(
        state.store.store().named_graphs().count(),
        graphs_before,
        "no graph may have been created"
    );
}

// ─── Strict OWL/XML reading (SWRL §2 atoms, OWL API encoding) ─────────────

/// Wrap rule bodies and heads in an OWL/XML ontology, one `DLSafeRule` each.
fn owlxml(rules: &[(&str, &str)]) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">\n",
    );
    for (body, head) in rules {
        out.push_str(&format!(
            "  <DLSafeRule>\n    <Body>{body}</Body>\n    <Head>{head}</Head>\n  </DLSafeRule>\n"
        ));
    }
    out.push_str("</Ontology>");
    out
}

fn load(state: &AppState, turtle: &str) {
    state
        .store
        .load_str(turtle, oxigraph::io::RdfFormat::Turtle, None)
        .unwrap();
}

fn json_body(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON ({e}): {body}"))
}

/// Number of quads in the store, all graphs.
fn quad_count(state: &AppState) -> usize {
    state
        .store
        .store()
        .quads_for_pattern(None, None, None, None)
        .count()
}

const V: &str = "urn:swrl:var#";

fn var(name: &str) -> String {
    format!("<Variable IRI=\"{V}{name}\"/>")
}

/// `BuiltInAtom` is how the OWL API and Protégé spell the element. It used to
/// fall through to the reader's catch-all: the atom vanished, its arguments
/// went nowhere, and the rule fired for every Person regardless of age.
#[tokio::test]
async fn swrl_owlxml_builtin_in_atom_guard_is_honoured() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/p1> a <http://ex/Person> ; <http://ex/age> 30 .
           <http://ex/p2> a <http://ex/Person> ; <http://ex/age> 10 ."#,
    );
    let app = test_app(state.clone());
    let body = format!(
        r#"<ClassAtom><Class IRI="http://ex/Person"/>{x}</ClassAtom>
           <DataPropertyAtom><DataProperty IRI="http://ex/age"/>{x}{a}</DataPropertyAtom>
           <BuiltInAtom IRI="http://www.w3.org/2003/11/swrlb#greaterThan">{a}
             <Literal datatypeIRI="http://www.w3.org/2001/XMLSchema#integer">18</Literal>
           </BuiltInAtom>"#,
        x = var("x"),
        a = var("a")
    );
    let head = format!(
        r#"<ClassAtom><Class IRI="http://ex/Adult"/>{}</ClassAtom>"#,
        var("x")
    );
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": owlxml(&[(&body, &head)]), "format": "xml" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(
        ask(&state, "<http://ex/p1> a <http://ex/Adult>"),
        "the rule must still fire where the guard holds: {resp}"
    );
    assert!(
        !ask(&state, "<http://ex/p2> a <http://ex/Adult>"),
        "the BuiltInAtom guard must exclude the 10-year-old: {resp}"
    );
}

/// An atom element the reader does not know refuses the whole document with
/// its name. RDF/XML's `IndividualPropertyAtom` and `DataRangeAtom` used to be
/// dropped, so their rules ran with one condition fewer.
#[tokio::test]
async fn swrl_unknown_atom_element_refuses_document() {
    let (state, token) = admin_state();
    load(&state, r#"<http://ex/p1> a <http://ex/Person> ."#);
    let app = test_app(state.clone());
    let before = quad_count(&state);
    let head = format!(
        r#"<ClassAtom><Class IRI="http://ex/Flagged"/>{}</ClassAtom>"#,
        var("x")
    );
    let person = format!(
        r#"<ClassAtom><Class IRI="http://ex/Person"/>{}</ClassAtom>"#,
        var("x")
    );
    for (extra, named) in [
        (
            format!(
                r#"<IndividualPropertyAtom><ObjectProperty IRI="http://ex/p"/>{}{}</IndividualPropertyAtom>"#,
                var("x"),
                var("y")
            ),
            "IndividualPropertyAtom",
        ),
        (
            format!(
                r#"<DataRangeAtom><Datatype IRI="http://www.w3.org/2001/XMLSchema#integer"/>{}</DataRangeAtom>"#,
                var("x")
            ),
            "DataRangeAtom",
        ),
    ] {
        let body = format!("{person}{extra}");
        let (st, resp) = post_swrl(
            &app,
            &token,
            json!({ "rules": owlxml(&[(&body, &head)]), "format": "xml" }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{named}: {resp}");
        assert!(
            resp.contains(named),
            "the error must name <{named}>: {resp}"
        );
        assert!(
            !ask(&state, "<http://ex/p1> a <http://ex/Flagged>"),
            "{named}"
        );
        assert_eq!(
            quad_count(&state),
            before,
            "{named}: nothing may be written"
        );
    }

    // A stray element inside an atom is refused the same way.
    let body = format!(
        r#"<ClassAtom><Class IRI="http://ex/Person"/><Fancy/>{}</ClassAtom>"#,
        var("x")
    );
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": owlxml(&[(&body, &head)]), "format": "xml" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp.contains("Fancy"), "{resp}");
}

/// A `ClassAtom` over a class expression is refused until expressions are
/// supported. The reader used to take any nested `<Class IRI>`, so
/// `ObjectComplementOf(Person)(?x)` became `Person(?x)` — the opposite.
#[tokio::test]
async fn swrl_class_expression_atom_is_not_flattened() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/p1> a <http://ex/Person> ; <http://ex/knows> <http://ex/p2> .
           <http://ex/p2> a <http://ex/Person> ."#,
    );
    let app = test_app(state.clone());
    let before = quad_count(&state);
    let head = format!(
        r#"<ClassAtom><Class IRI="http://ex/NonPerson"/>{}</ClassAtom>"#,
        var("x")
    );
    for (expr, named) in [
        (
            r#"<ObjectComplementOf><Class IRI="http://ex/Person"/></ObjectComplementOf>"#,
            "ObjectComplementOf",
        ),
        (
            r#"<ObjectSomeValuesFrom><ObjectProperty IRI="http://ex/knows"/><Class IRI="http://ex/Person"/></ObjectSomeValuesFrom>"#,
            "ObjectSomeValuesFrom",
        ),
    ] {
        let body = format!("<ClassAtom>{expr}{}</ClassAtom>", var("x"));
        let (st, resp) = post_swrl(
            &app,
            &token,
            json!({ "rules": owlxml(&[(&body, &head)]), "format": "xml" }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{named}: {resp}");
        assert!(
            resp.contains(named),
            "the error must name <{named}>: {resp}"
        );
        assert!(
            !ask(&state, "<http://ex/p1> a <http://ex/NonPerson>"),
            "{named} must not be read as its inner class"
        );
        assert_eq!(
            quad_count(&state),
            before,
            "{named}: nothing may be written"
        );
    }
}

/// `ObjectInverseOf(p)(?x, ?y)` holds when `p(?y, ?x)` does. The reader used to
/// read it as `p(?x, ?y)`, deriving every conclusion backwards.
#[tokio::test]
async fn swrl_object_inverse_of_keeps_direction() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/a> <http://ex/parentOf> <http://ex/b> ."#,
    );
    let app = test_app(state.clone());
    let body = format!(
        r#"<ObjectPropertyAtom><ObjectInverseOf><ObjectProperty IRI="http://ex/parentOf"/></ObjectInverseOf>{}{}</ObjectPropertyAtom>"#,
        var("x"),
        var("y")
    );
    let head = format!(
        r#"<ObjectPropertyAtom><ObjectProperty IRI="http://ex/childOf"/>{}{}</ObjectPropertyAtom>"#,
        var("x"),
        var("y")
    );
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": owlxml(&[(&body, &head)]), "format": "xml" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(
        ask(&state, "<http://ex/b> <http://ex/childOf> <http://ex/a>"),
        "parentOf⁻(b, a) holds, so b childOf a: {resp}"
    );
    assert!(
        !ask(&state, "<http://ex/a> <http://ex/childOf> <http://ex/b>"),
        "the inverse must not be read forwards: {resp}"
    );
}

/// `stringConcat(?r, ?a, ?b)` holds when `?r` is `?a` followed by `?b`. It used
/// to become `FILTER(CONCAT(?r, ?a, ?b))`, true for any non-empty string.
#[tokio::test]
async fn swrl_string_concat_is_a_real_guard() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/p1> <http://ex/first> "Ada" ; <http://ex/last> "Lovelace" ;
                          <http://ex/full> "AdaLovelace" .
           <http://ex/p2> <http://ex/first> "Bo" ; <http://ex/last> "X" ;
                          <http://ex/full> "Wrong" ."#,
    );
    let app = test_app(state.clone());
    let dp = |p: &str, x: &str, v: &str| {
        format!(
            r#"<DataPropertyAtom><DataProperty IRI="http://ex/{p}"/>{}{}</DataPropertyAtom>"#,
            var(x),
            var(v)
        )
    };
    let body = format!(
        r#"{}{}{}<BuiltInAtom IRI="http://www.w3.org/2003/11/swrlb#stringConcat">{}{}{}</BuiltInAtom>"#,
        dp("first", "x", "f"),
        dp("last", "x", "l"),
        dp("full", "x", "n"),
        var("n"),
        var("f"),
        var("l")
    );
    let head = format!(
        r#"<ClassAtom><Class IRI="http://ex/Consistent"/>{}</ClassAtom>"#,
        var("x")
    );
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": owlxml(&[(&body, &head)]), "format": "xml" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(
        ask(&state, "<http://ex/p1> a <http://ex/Consistent>"),
        "{resp}"
    );
    assert!(
        !ask(&state, "<http://ex/p2> a <http://ex/Consistent>"),
        "\"Wrong\" is not \"Bo\" + \"X\": {resp}"
    );
}

// ─── Rule safety, typed positions, engine hygiene ─────────────────────────

/// SWRL §2–3 safety: a head variable must occur in the body. Such a rule used
/// to run, its INSERT skipping silently. It is refused now — and so is the
/// whole request, so the safe rule beside it does not run either.
#[tokio::test]
async fn swrl_unsafe_head_variable_refused() {
    let (state, token) = admin_state();
    load(&state, r#"<http://ex/p1> a <http://ex/Person> ."#);
    let app = test_app(state.clone());
    let before = quad_count(&state);
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({
            "rules": "http://ex/Person(?x) -> http://ex/Agent(?x)\n\
                      http://ex/Person(?x) -> http://ex/knows(?x, ?y)",
            "format": "text"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert!(
        resp.contains("unsafe rule") && resp.contains("'?y'"),
        "the error must name the unbound head variable: {resp}"
    );
    assert!(!ask(&state, "<http://ex/p1> a <http://ex/Agent>"), "{resp}");
    assert_eq!(quad_count(&state), before);
}

/// A built-in in the head is not SWRL. It used to be skipped with a warning,
/// leaving the rest of the head to fire.
#[tokio::test]
async fn swrl_head_builtin_refused() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/p1> a <http://ex/Person> ; <http://ex/age> 30 ."#,
    );
    let app = test_app(state.clone());
    let before = quad_count(&state);
    let body = format!(
        r#"<DataPropertyAtom><DataProperty IRI="http://ex/age"/>{}{}</DataPropertyAtom>"#,
        var("x"),
        var("a")
    );
    let head = format!(
        r#"<ClassAtom><Class IRI="http://ex/Checked"/>{x}</ClassAtom>
           <BuiltInAtom IRI="http://www.w3.org/2003/11/swrlb#greaterThan">{a}
             <Literal datatypeIRI="http://www.w3.org/2001/XMLSchema#integer">18</Literal>
           </BuiltInAtom>"#,
        x = var("x"),
        a = var("a")
    );
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": owlxml(&[(&body, &head)]), "format": "xml" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp.contains("rule head"), "{resp}");
    assert!(
        !ask(&state, "<http://ex/p1> a <http://ex/Checked>"),
        "{resp}"
    );
    assert_eq!(quad_count(&state), before);
}

/// Object and data property atoms are no longer translated alike: an object
/// property atom binds only individuals and a data property atom only
/// literals, and a constant of the wrong kind is refused.
#[tokio::test]
async fn swrl_object_vs_data_positions_typed() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/a> <http://ex/p> <http://ex/b> , "lit" ."#,
    );
    let app = test_app(state.clone());
    let atom = |kind: &str, p: &str, a: &str, b: &str| {
        format!(
            r#"<{kind}PropertyAtom><{kind}Property IRI="http://ex/{p}"/>{a}{b}</{kind}PropertyAtom>"#
        )
    };
    let (x, y) = (var("x"), var("y"));
    let rules = owlxml(&[
        (
            &atom("Object", "p", &x, &y),
            &atom("Object", "objectValue", &x, &y),
        ),
        (
            &atom("Data", "p", &x, &y),
            &atom("Data", "dataValue", &x, &y),
        ),
    ]);
    let (st, resp) = post_swrl(&app, &token, json!({ "rules": rules, "format": "xml" })).await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(
        ask(
            &state,
            "<http://ex/a> <http://ex/objectValue> <http://ex/b>"
        ),
        "{resp}"
    );
    assert!(
        !ask(&state, r#"<http://ex/a> <http://ex/objectValue> "lit""#),
        "an object property atom must not bind a literal: {resp}"
    );
    assert!(
        ask(&state, r#"<http://ex/a> <http://ex/dataValue> "lit""#),
        "{resp}"
    );
    assert!(
        !ask(&state, "<http://ex/a> <http://ex/dataValue> <http://ex/b>"),
        "a data property atom must not bind an individual: {resp}"
    );

    // Constants of the wrong kind are refused by position.
    let lit = r#"<Literal datatypeIRI="http://www.w3.org/2001/XMLSchema#string">lit</Literal>"#;
    let ind = r#"<NamedIndividual IRI="http://ex/b"/>"#;
    for (body, wanted) in [
        (atom("Object", "p", &x, lit), "found <Literal>"),
        (atom("Data", "p", &x, ind), "found <NamedIndividual>"),
    ] {
        let head = format!(r#"<ClassAtom><Class IRI="http://ex/T"/>{x}</ClassAtom>"#);
        let (st, resp) = post_swrl(
            &app,
            &token,
            json!({ "rules": owlxml(&[(&body, &head)]), "format": "xml" }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
        assert!(resp.contains(wanted), "{resp}");
    }
}

/// Any variable IRI is a variable: the OWL API's `urn:swrl#x`, an ontology
/// namespace, and an `abbreviatedIRI` alike. Only `urn:swrl:var#` and `?` used
/// to be mapped. Each IRI gets its own generated variable, so two IRIs that
/// share a local name (`…#y`) stay two variables.
#[tokio::test]
async fn swrl_variable_iri_forms() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/a> <http://ex/parentOf> <http://ex/b> .
           <http://ex/b> <http://ex/parentOf> <http://ex/c> .
           <http://ex/a> a <http://ex/Person> ."#,
    );
    let app = test_app(state.clone());
    let v = |iri: &str| format!(r#"<Variable IRI="{iri}"/>"#);
    let (x, y, z) = (
        v("urn:swrl#x"),
        v("http://ex/onto#y"),
        v("http://other/ns#y"),
    );
    let op = |p: &str, a: &str, b: &str| {
        format!(
            r#"<ObjectPropertyAtom><ObjectProperty IRI="http://ex/{p}"/>{a}{b}</ObjectPropertyAtom>"#
        )
    };
    let short = r#"<Variable abbreviatedIRI=":p"/>"#;
    let rules = owlxml(&[
        (
            &format!("{}{}", op("parentOf", &x, &y), op("parentOf", &y, &z)),
            &op("grandparentOf", &x, &z),
        ),
        (
            &format!(r#"<ClassAtom><Class IRI="http://ex/Person"/>{short}</ClassAtom>"#),
            &format!(r#"<ClassAtom><Class IRI="http://ex/Agent"/>{short}</ClassAtom>"#),
        ),
    ]);
    let (st, resp) = post_swrl(&app, &token, json!({ "rules": rules, "format": "xml" })).await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(
        ask(
            &state,
            "<http://ex/a> <http://ex/grandparentOf> <http://ex/c>"
        ),
        "{resp}"
    );
    assert!(ask(&state, "<http://ex/a> a <http://ex/Agent>"), "{resp}");
    let report = json_body(&resp);
    assert_eq!(report["triples_inferred"], 2, "{resp}");
}

/// The target graph went into the update as `GRAPH <{target}>` unchecked, so a
/// value carrying `>` could close the clause and append operations. It must be
/// an absolute IRI now; an IRI target receives the derived triples.
#[tokio::test]
async fn swrl_target_graph_must_be_iri() {
    let (state, token) = admin_state();
    load(&state, r#"<http://ex/p1> a <http://ex/Person> ."#);
    let app = test_app(state.clone());
    let before = quad_count(&state);
    let rule = "http://ex/Person(?x) -> http://ex/Agent(?x)";
    for bad in [
        "urn:out> { <urn:s> <urn:p> <urn:o> } } ; DROP ALL ; INSERT DATA { GRAPH <urn:probe",
        "not an iri",
        "relative/graph",
    ] {
        let (st, resp) = post_swrl(
            &app,
            &token,
            json!({ "rules": rule, "format": "text", "target_graph": bad }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}: {resp}");
        assert!(resp.contains("Invalid target_graph"), "{resp}");
        assert_eq!(quad_count(&state), before, "{bad}: nothing may be written");
    }

    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": rule, "format": "text", "target_graph": "urn:swrl:out" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(ask(
        &state,
        "GRAPH <urn:swrl:out> { <http://ex/p1> a <http://ex/Agent> }"
    ));
    assert_eq!(json_body(&resp)["triples_inferred"], 1, "{resp}");
}

/// Hitting `max_iterations` used to look exactly like success. The report now
/// says whether the fixed point was reached and why the loop stopped, and
/// counts only the triples this run wrote.
#[tokio::test]
async fn swrl_reports_non_convergence() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/a> <http://ex/parentOf> <http://ex/b> .
           <http://ex/b> <http://ex/parentOf> <http://ex/c> .
           <http://ex/c> <http://ex/parentOf> <http://ex/d> .
           <http://ex/d> <http://ex/parentOf> <http://ex/e> ."#,
    );
    let rules = "http://ex/parentOf(?x, ?y) -> http://ex/ancestorOf(?x, ?y)\n\
                 http://ex/ancestorOf(?x, ?y) ^ http://ex/ancestorOf(?y, ?z) -> http://ex/ancestorOf(?x, ?z)";

    // One iteration cannot close a chain of four links.
    let app = test_app(state.clone());
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": rules, "format": "text", "max_iterations": 1 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    let report = json_body(&resp);
    assert_eq!(report["converged"], false, "{resp}");
    assert_eq!(report["stop_reason"], "max_iterations", "{resp}");
    assert_eq!(report["iterations"], 1, "{resp}");

    // Enough iterations: converged.
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": rules, "format": "text", "max_iterations": 20 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    let report = json_body(&resp);
    assert_eq!(report["converged"], true, "{resp}");
    assert_eq!(report["stop_reason"], "fixpoint", "{resp}");
    assert!(ask(
        &state,
        "<http://ex/a> <http://ex/ancestorOf> <http://ex/e>"
    ));

    // On a fresh store the count is exactly the closure's ten ancestor pairs,
    // however much else the store holds in other graphs.
    let (fresh, token2) = admin_state();
    load(
        &fresh,
        r#"<http://ex/a> <http://ex/parentOf> <http://ex/b> .
           <http://ex/b> <http://ex/parentOf> <http://ex/c> .
           <http://ex/c> <http://ex/parentOf> <http://ex/d> .
           <http://ex/d> <http://ex/parentOf> <http://ex/e> ."#,
    );
    fresh
        .store
        .update("INSERT DATA { GRAPH <urn:elsewhere> { <urn:s> <urn:p> <urn:o> } }")
        .unwrap();
    let (st, resp) = post_swrl(
        &test_app(fresh.clone()),
        &token2,
        json!({ "rules": rules, "format": "text", "max_iterations": 20 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert_eq!(json_body(&resp)["triples_inferred"], 10, "{resp}");

    // The server's time limit stops the loop too, and says so.
    let (mut timed, token3) = admin_state();
    timed.write_timeout_secs = 0;
    load(
        &timed,
        r#"<http://ex/a> <http://ex/parentOf> <http://ex/b> ."#,
    );
    let (st, resp) = post_swrl(
        &test_app(timed.clone()),
        &token3,
        json!({ "rules": rules, "format": "text" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    let report = json_body(&resp);
    assert_eq!(report["converged"], false, "{resp}");
    assert_eq!(report["stop_reason"], "timeout", "{resp}");
}

// ─── Dataset scope (rules over a dataset's graphs) ────────────────────────

const DS_DATA: &str = "http://example.org/swrl/data";
const DS_OTHER: &str = "http://example.org/swrl/other";

/// A dataset `fam` (owner `adm`) whose instances graph holds a parent chain.
fn family_dataset(state: &AppState) {
    use open_triplestore::auth::models::{GraphKind, OwnerType, Visibility};
    state
        .auth_db
        .create_dataset(
            "fam",
            "Family",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("fam", DS_DATA).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("fam", DS_DATA, Some(GraphKind::Instances))
        .unwrap();
    // A graph of another dataset, readable but outside `fam`'s layer: explicit
    // source graphs are read-checked, and a graph no dataset holds is not
    // readable even for an admin.
    state
        .auth_db
        .create_dataset(
            "kin",
            "Kin",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("kin", DS_OTHER).unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{DS_DATA}> {{ \
               <http://ex/a> <http://ex/parentOf> <http://ex/b> . \
               <http://ex/b> <http://ex/parentOf> <http://ex/c> . }} \
             GRAPH <{DS_OTHER}> {{ <http://ex/c> <http://ex/parentOf> <http://ex/d> . }} }}"
        ))
        .unwrap();
    // Outside the dataset: the default graph extends the chain, and must not
    // be read by a dataset run.
    load(state, "<http://ex/b> <http://ex/parentOf> <http://ex/q> .");
}

const GRANDPARENT: &str =
    "http://ex/parentOf(?x, ?y) ^ http://ex/parentOf(?y, ?z) -> http://ex/grandparentOf(?x, ?z)";

/// Rules used to see only the unnamed default graph, so they never saw a
/// dataset. With `dataset` they read its graphs — and only those — and write
/// to its inference graph unless a target is given.
#[tokio::test]
async fn swrl_dataset_scope_reads_named_graphs() {
    let (state, token) = admin_state();
    family_dataset(&state);
    let app = test_app(state.clone());

    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": GRANDPARENT, "format": "text", "dataset": "fam" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    let report = json_body(&resp);
    let target = "urn:entailment:swrl:fam";
    assert_eq!(report["target_graph"], target, "{resp}");
    assert!(
        report["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g == DS_DATA),
        "{resp}"
    );
    assert!(ask(
        &state,
        &format!("GRAPH <{target}> {{ <http://ex/a> <http://ex/grandparentOf> <http://ex/c> }}")
    ));
    assert!(
        !ask(
            &state,
            &format!(
                "GRAPH <{target}> {{ <http://ex/a> <http://ex/grandparentOf> <http://ex/q> }}"
            )
        ),
        "the default graph is outside the dataset: {resp}"
    );
    assert!(
        !ask(
            &state,
            "<http://ex/a> <http://ex/grandparentOf> <http://ex/c>"
        ),
        "nothing is written to the default graph"
    );

    // Extra source graphs join the dataset's; a target overrides the default.
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({
            "rules": GRANDPARENT, "format": "text", "dataset": "fam",
            "source_graphs": [DS_OTHER], "target_graph": "urn:swrl:out"
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(ask(
        &state,
        "GRAPH <urn:swrl:out> { <http://ex/b> <http://ex/grandparentOf> <http://ex/d> }"
    ));

    // Explicit graphs without a dataset need a named target: what a run
    // derives must be readable by its next iteration.
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": GRANDPARENT, "format": "text", "source_graphs": [DS_DATA] }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp.contains("needs a target_graph"), "{resp}");

    // The derived graph joins queries that opt in with entailment_dataset.
    let q = "SELECT ?o WHERE { <http://ex/a> <http://ex/grandparentOf> ?o }";
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/sparql?query={}&entailment_dataset=fam",
                    url_encode(q)
                ))
                .header(header::ACCEPT, "application/sparql-results+json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let text = body_text(resp.into_body()).await;
    assert!(text.contains("http://ex/c"), "{text}");
}

/// A dataset run reads only what the caller may read, as
/// `/api/reasoning/materialize` does: a viewer's run leaves the dataset's
/// private graph out, an explicit unreadable graph is refused, and only the
/// dataset's writers may fill its inference graph.
#[tokio::test]
async fn swrl_dataset_scope_respects_read_acl() {
    use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
    let (state, _admin) = admin_state();
    for id in ["owner", "mallory"] {
        state
            .auth_db
            .create_user(id, id, &format!("{id}@t.com"), "hash", SystemRole::User)
            .unwrap();
    }
    let owner = mint_token("owner", "owner", "user");
    let mallory = mint_token("mallory", "mallory", "user");
    let (pub_g, priv_g, target) = (
        "http://example.org/g/pub",
        "http://example.org/g/priv",
        "http://example.org/g/target",
    );
    state
        .auth_db
        .create_dataset(
            "victim",
            "Victim",
            None,
            OwnerType::User,
            "owner",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("victim", pub_g).unwrap();
    state.auth_db.add_dataset_graph("victim", priv_g).unwrap();
    state
        .auth_db
        .set_dataset_graph_private("victim", priv_g, true)
        .unwrap();
    state
        .store
        .update(&format!(
            "INSERT DATA {{ GRAPH <{pub_g}> {{ <http://ex/pub> a <http://ex/Thing> }} \
             GRAPH <{priv_g}> {{ <http://ex/secret> a <http://ex/Thing> }} }}"
        ))
        .unwrap();
    for (id, who) in [("acl-o", "owner"), ("acl-m", "mallory")] {
        state
            .auth_db
            .grant_graph_permission(id, target, "user", who, "write", "adm")
            .unwrap();
    }
    let app = test_app(state.clone());
    let rule = "http://ex/Thing(?x) -> http://ex/Copied(?x)";

    // The viewer's run skips the private graph.
    let (st, resp) = post_swrl(
        &app,
        &mallory,
        json!({ "rules": rule, "format": "text", "dataset": "victim", "target_graph": target }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    let sources = json_body(&resp)["sources"].clone();
    assert!(
        !sources.as_array().unwrap().iter().any(|g| g == priv_g),
        "{resp}"
    );
    assert!(ask(
        &state,
        &format!("GRAPH <{target}> {{ <http://ex/pub> a <http://ex/Copied> }}")
    ));
    assert!(
        !ask(
            &state,
            &format!("GRAPH <{target}> {{ <http://ex/secret> ?p ?o }}")
        ),
        "a private-graph triple must not be laundered into the viewer's target"
    );

    // An explicit graph the caller cannot read is refused.
    let (st, resp) = post_swrl(
        &app,
        &mallory,
        json!({ "rules": rule, "format": "text", "source_graphs": [priv_g], "target_graph": target }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{resp}");

    // The dataset's inference graph is its writers' to fill.
    let (st, resp) = post_swrl(
        &app,
        &mallory,
        json!({ "rules": rule, "format": "text", "dataset": "victim" }),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{resp}");
    let (st, resp) = post_swrl(
        &app,
        &owner,
        json!({ "rules": rule, "format": "text", "dataset": "victim" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(ask(
        &state,
        "GRAPH <urn:entailment:swrl:victim> { <http://ex/secret> a <http://ex/Copied> }"
    ));
}

// ─── Rule syntaxes ─────────────────────────────────────────────────────────

/// The SWRL RDF syntax (§5): `swrl:Imp` with argument lists, from Turtle and
/// from RDF/XML. The reader used to read neither the arguments nor the rule.
#[tokio::test]
async fn swrl_rdf_syntax_rules_fire() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/a> <http://ex/parentOf> <http://ex/b> .
           <http://ex/b> <http://ex/parentOf> <http://ex/c> .
           <http://ex/p1> <http://ex/age> 30 . <http://ex/p2> <http://ex/age> 10 ."#,
    );
    let app = test_app(state.clone());
    let turtle = r#"
@prefix ex:   <http://ex/> .
@prefix swrl: <http://www.w3.org/2003/11/swrl#> .
@prefix swrlb: <http://www.w3.org/2003/11/swrlb#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
ex:x a swrl:Variable . ex:y a swrl:Variable . ex:z a swrl:Variable . ex:n a swrl:Variable .
ex:grand a swrl:Imp ;
  swrl:body ( [ a swrl:IndividualPropertyAtom ; swrl:propertyPredicate ex:parentOf ;
                swrl:argument1 ex:x ; swrl:argument2 ex:y ]
              [ a swrl:IndividualPropertyAtom ; swrl:propertyPredicate ex:parentOf ;
                swrl:argument1 ex:y ; swrl:argument2 ex:z ] ) ;
  swrl:head ( [ a swrl:IndividualPropertyAtom ; swrl:propertyPredicate ex:grandparentOf ;
                swrl:argument1 ex:x ; swrl:argument2 ex:z ] ) .
ex:adult a swrl:Imp ;
  swrl:body ( [ a swrl:DatavaluedPropertyAtom ; swrl:propertyPredicate ex:age ;
                swrl:argument1 ex:x ; swrl:argument2 ex:n ]
              [ a swrl:BuiltinAtom ; swrl:builtin swrlb:greaterThanOrEqual ;
                swrl:arguments ( ex:n "18"^^xsd:integer ) ] ) ;
  swrl:head ( [ a swrl:ClassAtom ; swrl:classPredicate ex:Adult ; swrl:argument1 ex:x ] ) .
"#;
    let (st, resp) = post_swrl(&app, &token, json!({ "rules": turtle, "format": "rdf" })).await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(
        ask(
            &state,
            "<http://ex/a> <http://ex/grandparentOf> <http://ex/c>"
        ),
        "{resp}"
    );
    assert!(ask(&state, "<http://ex/p1> a <http://ex/Adult>"), "{resp}");
    assert!(!ask(&state, "<http://ex/p2> a <http://ex/Adult>"), "{resp}");

    let rdfxml = r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:swrl="http://www.w3.org/2003/11/swrl#">
  <swrl:Variable rdf:about="http://ex/v"/>
  <swrl:Imp rdf:about="http://ex/tagged">
    <swrl:body rdf:parseType="Collection">
      <swrl:ClassAtom>
        <swrl:classPredicate rdf:resource="http://ex/Adult"/>
        <swrl:argument1 rdf:resource="http://ex/v"/>
      </swrl:ClassAtom>
    </swrl:body>
    <swrl:head rdf:parseType="Collection">
      <swrl:ClassAtom>
        <swrl:classPredicate rdf:resource="http://ex/Voter"/>
        <swrl:argument1 rdf:resource="http://ex/v"/>
      </swrl:ClassAtom>
    </swrl:head>
  </swrl:Imp>
</rdf:RDF>"#;
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": rdfxml, "format": "rdf", "rdf_format": "rdfxml" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(ask(&state, "<http://ex/p1> a <http://ex/Voter>"), "{resp}");

    // Sent as OWL/XML by mistake, the RDF/XML is pointed at the right format.
    let (st, resp) = post_swrl(&app, &token, json!({ "rules": rdfxml, "format": "xml" })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert!(
        resp.contains("format \\\"rdf\\\"") || resp.contains("format \"rdf\""),
        "{resp}"
    );
}

/// OWL/XML as Protégé saves it: `Prefix` declarations with `abbreviatedIRI`,
/// `xml:base` for relative `IRI`s, and plain literals.
#[tokio::test]
async fn swrl_owlxml_prefixes_and_relative_iris() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/onto#p1> a <http://ex/onto#Person> ; <http://ex/onto#nick> "Bo" ."#,
    );
    let app = test_app(state.clone());
    let xml = r##"<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" xml:base="http://ex/onto"
          ontologyIRI="http://ex/onto">
  <Prefix name="" IRI="http://ex/onto#"/>
  <Prefix name="ex" IRI="http://ex/onto#"/>
  <DLSafeRule>
    <Body>
      <ClassAtom><Class abbreviatedIRI=":Person"/><Variable abbreviatedIRI="ex:x"/></ClassAtom>
      <DataPropertyAtom>
        <DataProperty IRI="#nick"/>
        <Variable abbreviatedIRI="ex:x"/>
        <Literal>Bo</Literal>
      </DataPropertyAtom>
    </Body>
    <Head>
      <ClassAtom><Class abbreviatedIRI="ex:Nicknamed"/><Variable abbreviatedIRI="ex:x"/></ClassAtom>
    </Head>
  </DLSafeRule>
</Ontology>"##;
    let (st, resp) = post_swrl(&app, &token, json!({ "rules": xml, "format": "xml" })).await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(
        ask(&state, "<http://ex/onto#p1> a <http://ex/onto#Nicknamed>"),
        "prefixed names, the relative #nick and the plain literal must all resolve: {resp}"
    );

    // An undeclared prefix is refused by name.
    let bad = xml.replace("ex:Nicknamed", "zz:Nicknamed");
    let (st, resp) = post_swrl(&app, &token, json!({ "rules": bad, "format": "xml" })).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp.contains("zz:"), "{resp}");
}

/// OWL 2 functional syntax, as the OWL API writes `.ofn`: `Prefix(…)`, the
/// rule inside `Ontology(…)` among other axioms, `BuiltInAtom` and a typed
/// literal.
#[tokio::test]
async fn swrl_functional_syntax_rule() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/p1> a <http://ex/Person> ; <http://ex/age> 30 .
           <http://ex/p2> a <http://ex/Person> ; <http://ex/age> 10 ."#,
    );
    let app = test_app(state.clone());
    let ofn = r#"
Prefix(:=<http://ex/>)
Ontology(<http://ex/onto>
  Declaration(Class(:Person))
  SubClassOf(:Adult :Person)
  DLSafeRule(
    Annotation(rdfs:comment "adults")
    Body(
      ClassAtom(:Person Variable(<urn:swrl#x>))
      DataPropertyAtom(:age Variable(<urn:swrl#x>) Variable(<urn:swrl#a>))
      BuiltInAtom(swrlb:greaterThan Variable(<urn:swrl#a>) "17"^^xsd:integer)
    )
    Head(ClassAtom(:Adult Variable(<urn:swrl#x>)))
  )
)"#;
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": ofn, "format": "functional" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(ask(&state, "<http://ex/p1> a <http://ex/Adult>"), "{resp}");
    assert!(!ask(&state, "<http://ex/p2> a <http://ex/Adult>"), "{resp}");
}

/// The SWRLAPI human-readable syntax: prefixes from the request (`""` the
/// default prefix) and from the server's registry (`foaf:`), built-ins, and
/// literals that hold `^` and `,` — which the ad-hoc text form split on.
#[tokio::test]
async fn swrl_human_readable_syntax_with_prefixes_and_builtins() {
    // The bundled prefix.cc/LOV snapshot knows `foaf:`.
    let (state, token) = admin_state_over(test_state_with_bundled_prefixes());
    load(
        &state,
        r#"<http://ex/p1> a <http://xmlns.com/foaf/0.1/Person> ; <http://ex/age> 30 ;
                          <http://ex/motto> "a, ^ b" .
           <http://ex/p2> a <http://xmlns.com/foaf/0.1/Person> ; <http://ex/age> 10 ."#,
    );
    let app = test_app(state.clone());
    let rules = r#"foaf:Person(?p) ^ hasAge(?p, ?a) ^ swrlb:greaterThan(?a, 17) -> Adult(?p)
                   ex:motto(?p, "a, ^ b") -> ex:Quoted(?p)"#;
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({
            "rules": rules,
            "format": "swrlapi",
            "prefixes": { "": "http://ex/", "ex": "http://ex/" }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    // `hasAge` is the default prefix's: http://ex/hasAge, not http://ex/age.
    assert!(!ask(&state, "<http://ex/p1> a <http://ex/Adult>"), "{resp}");
    assert!(ask(&state, "<http://ex/p1> a <http://ex/Quoted>"), "{resp}");

    let rules = rules.replace("hasAge", "age");
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": rules, "format": "swrlapi", "prefixes": { "": "http://ex/", "ex": "http://ex/" } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(ask(&state, "<http://ex/p1> a <http://ex/Adult>"), "{resp}");
    assert!(!ask(&state, "<http://ex/p2> a <http://ex/Adult>"), "{resp}");

    // A bare name without a default prefix is refused by name.
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": "Person(?p) -> foaf:Agent(?p)", "format": "swrlapi" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp.contains("bare name 'Person'"), "{resp}");
}

/// The SWRL §4 XML concrete syntax (RuleML `imp`, `swrlx:` atoms), with the
/// submission's uncle example.
#[tokio::test]
async fn swrl_xml_concrete_syntax() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/fam#ann> <http://ex/fam#hasParent> <http://ex/fam#bob> .
           <http://ex/fam#bob> <http://ex/fam#hasBrother> <http://ex/fam#carl> ."#,
    );
    let app = test_app(state.clone());
    let xml = r##"<?xml version="1.0"?>
<swrlx:Ontology xmlns:swrlx="http://www.w3.org/2003/11/swrlx#"
                xmlns:owlx="http://www.w3.org/2003/05/owl-xml"
                xmlns:ruleml="http://www.w3.org/2003/11/ruleml"
                xml:base="http://ex/fam">
  <ruleml:imp>
    <ruleml:_rlab ruleml:href="#uncle"/>
    <ruleml:_body>
      <swrlx:individualPropertyAtom swrlx:property="#hasParent">
        <ruleml:var>x1</ruleml:var>
        <ruleml:var>x2</ruleml:var>
      </swrlx:individualPropertyAtom>
      <swrlx:individualPropertyAtom swrlx:property="#hasBrother">
        <ruleml:var>x2</ruleml:var>
        <ruleml:var>x3</ruleml:var>
      </swrlx:individualPropertyAtom>
    </ruleml:_body>
    <ruleml:_head>
      <swrlx:individualPropertyAtom swrlx:property="#hasUncle">
        <ruleml:var>x1</ruleml:var>
        <ruleml:var>x3</ruleml:var>
      </swrlx:individualPropertyAtom>
    </ruleml:_head>
  </ruleml:imp>
</swrlx:Ontology>"##;
    let (st, resp) = post_swrl(&app, &token, json!({ "rules": xml, "format": "ruleml" })).await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(
        ask(
            &state,
            "<http://ex/fam#ann> <http://ex/fam#hasUncle> <http://ex/fam#carl>"
        ),
        "{resp}"
    );
}
