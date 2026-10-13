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

    // A built-in outside the swrlb: namespace (here a same-named function in
    // another one) is not a §8 built-in. Its variables are bound by body
    // atoms, so the refusal is about the builtin, not safety.
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
      <BuiltinAtom IRI="http://example.org/fn#stringLength">
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
/// its name. RDF/XML's `IndividualPropertyAtom` (and `DataRangeAtom`, read
/// natively now) used to be dropped, so their rules ran with one condition
/// fewer.
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
            format!(r#"<IndividualTypeAtom>{}</IndividualTypeAtom>"#, var("x")),
            "IndividualTypeAtom",
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

/// A `ClassAtom` over a class expression is read as that expression, never
/// as the class inside it: the reader used to take any nested `<Class IRI>`,
/// so `ObjectComplementOf(Person)(?x)` became `Person(?x)` — the opposite.
/// Without a regime nothing computes who belongs to an expression, so such a
/// rule is refused by name rather than run as a rule that never fires.
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
            resp.contains(named) && resp.contains("regime"),
            "the error must name <{named}> and the regime it needs: {resp}"
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

// ─── Built-ins (SWRL §8), evaluated natively ──────────────────────────────

/// Run SWRLAPI-syntax `rules` (prefix `ex:` = `http://ex/`) over `data`,
/// expecting success.
async fn run_swrlapi(state: &AppState, token: &str, rules: &str) -> serde_json::Value {
    let (st, resp) = post_swrl(
        &test_app(state.clone()),
        token,
        json!({ "rules": rules, "format": "swrlapi", "prefixes": { "ex": "http://ex/" } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    json_body(&resp)
}

/// Whether `<http://ex/{s}> <http://ex/{p}> ?v` holds for a `?v` passing
/// `filter` (a SPARQL expression over `?v`).
fn value_holds(state: &AppState, s: &str, p: &str, filter: &str) -> bool {
    ask(
        state,
        &format!("<http://ex/{s}> <http://ex/{p}> ?v FILTER({filter})"),
    )
}

/// One table row: a rule deriving `ex:a ex:<out> ?r`, and the filter its
/// value must pass (`None`: the rule must derive nothing).
type Case = (&'static str, &'static str, Option<&'static str>);

async fn check_cases(data: &str, cases: &[Case]) {
    let (state, token) = admin_state();
    load(&state, data);
    let rules: Vec<&str> = cases.iter().map(|(r, ..)| *r).collect();
    let report = run_swrlapi(&state, &token, &rules.join("\n")).await;
    assert_eq!(report["converged"], true, "{report}");
    for (rule, out, filter) in cases {
        match filter {
            Some(f) => assert!(
                value_holds(&state, "a", out, f),
                "{rule}\n  expected ex:a ex:{out} ?v with {f}; report: {report}"
            ),
            None => assert!(
                !ask(&state, &format!("<http://ex/a> <http://ex/{out}> ?v")),
                "{rule}\n  must derive nothing for ex:{out}"
            ),
        }
    }
}

/// §8.1 comparisons: value comparison across numeric types, dates and
/// strings; `equal` also binds one side from the other.
#[tokio::test]
async fn swrl_builtins_8_1_comparisons() {
    let data = r#"@prefix ex: <http://ex/> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:a ex:n 5 ; ex:m 5.0 ; ex:s "abc" ;
             ex:d "2024-01-01"^^xsd:date ; ex:e "2023-01-01"^^xsd:date ."#;
    check_cases(
        data,
        &[
            (
                "ex:n(?x, ?v) ^ ex:m(?x, ?w) ^ swrlb:equal(?v, ?w) -> ex:eq(?x, true)",
                "eq",
                Some("?v = true"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:equal(?c, ?v) -> ex:copy(?x, ?c)",
                "copy",
                Some("?v = 5"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:notEqual(?v, 6) -> ex:ne6(?x, true)",
                "ne6",
                Some("?v"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:notEqual(?v, 5) -> ex:ne5(?x, true)",
                "ne5",
                None,
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:lessThan(?v, 6) -> ex:lt(?x, true)",
                "lt",
                Some("?v"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:greaterThan(?v, 6) -> ex:gt(?x, true)",
                "gt",
                None,
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:lessThanOrEqual(?v, 5) -> ex:le(?x, true)",
                "le",
                Some("?v"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:greaterThanOrEqual(?v, 5.0) -> ex:ge(?x, true)",
                "ge",
                Some("?v"),
            ),
            (
                "ex:d(?x, ?d) ^ ex:e(?x, ?e) ^ swrlb:greaterThan(?d, ?e) -> ex:later(?x, true)",
                "later",
                Some("?v"),
            ),
            (
                "ex:s(?x, ?s) ^ swrlb:lessThan(?s, \"abd\") -> ex:before(?x, true)",
                "before",
                Some("?v"),
            ),
        ],
    )
    .await;
}

/// §8.2 math: every built-in binds its first argument; `add` and `subtract`
/// also solve for one unbound operand; a non-number makes it false.
#[tokio::test]
async fn swrl_builtins_8_2_math() {
    let data = r#"@prefix ex: <http://ex/> . ex:a ex:n 7 ; ex:f 2.5 ; ex:g 3.14159 ; ex:z 0 ."#;
    check_cases(
        data,
        &[
            (
                "ex:n(?x, ?v) ^ swrlb:add(?r, ?v, 1, 2) -> ex:add(?x, ?r)",
                "add",
                Some("?v = 10"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:add(?v, ?y, 3) -> ex:solved(?x, ?y)",
                "solved",
                Some("?v = 4"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:subtract(?r, ?v, 2) -> ex:sub(?x, ?r)",
                "sub",
                Some("?v = 5"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:subtract(1, ?v, ?y) -> ex:subSolved(?x, ?y)",
                "subSolved",
                Some("?v = 6"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:multiply(?r, ?v, 2, 3) -> ex:mul(?x, ?r)",
                "mul",
                Some("?v = 42"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:divide(?r, ?v, 2) -> ex:div(?x, ?r)",
                "div",
                Some("?v = 3.5"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:integerDivide(?r, ?v, 2) -> ex:idiv(?x, ?r)",
                "idiv",
                Some("?v = 3"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:mod(?r, ?v, 3) -> ex:mod(?x, ?r)",
                "mod",
                Some("?v = 1"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:pow(?r, ?v, 2) -> ex:pow(?x, ?r)",
                "pow",
                Some("?v = 49"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:unaryPlus(?r, ?v) -> ex:plus(?x, ?r)",
                "plus",
                Some("?v = 7"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:unaryMinus(?r, ?v) -> ex:minus(?x, ?r)",
                "minus",
                Some("?v = -7"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:unaryMinus(?m, ?v) ^ swrlb:abs(?r, ?m) -> ex:abs(?x, ?r)",
                "abs",
                Some("?v = 7"),
            ),
            (
                "ex:f(?x, ?v) ^ swrlb:ceiling(?r, ?v) -> ex:ceil(?x, ?r)",
                "ceil",
                Some("?v = 3"),
            ),
            (
                "ex:f(?x, ?v) ^ swrlb:floor(?r, ?v) -> ex:floor(?x, ?r)",
                "floor",
                Some("?v = 2"),
            ),
            (
                "ex:f(?x, ?v) ^ swrlb:round(?r, ?v) -> ex:round(?x, ?r)",
                "round",
                Some("?v = 3"),
            ),
            (
                "ex:f(?x, ?v) ^ swrlb:roundHalfToEven(?r, ?v) -> ex:even(?x, ?r)",
                "even",
                Some("?v = 2"),
            ),
            (
                "ex:g(?x, ?v) ^ swrlb:roundHalfToEven(?r, ?v, 2) -> ex:even2(?x, ?r)",
                "even2",
                Some("?v = 3.14"),
            ),
            (
                "ex:z(?x, ?v) ^ swrlb:sin(?r, ?v) -> ex:sin(?x, ?r)",
                "sin",
                Some("?v = 0"),
            ),
            (
                "ex:z(?x, ?v) ^ swrlb:cos(?r, ?v) -> ex:cos(?x, ?r)",
                "cos",
                Some("?v = 1"),
            ),
            (
                "ex:z(?x, ?v) ^ swrlb:tan(?r, ?v) -> ex:tan(?x, ?r)",
                "tan",
                Some("?v = 0"),
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:add(?r, ?v, \"x\") -> ex:typeError(?x, ?r)",
                "typeError",
                None,
            ),
            (
                "ex:n(?x, ?v) ^ swrlb:divide(?r, ?v, 0) -> ex:divZero(?x, ?r)",
                "divZero",
                None,
            ),
        ],
    )
    .await;
}

/// §8.3 boolean: `booleanNot` binds, checks, and solves.
#[tokio::test]
async fn swrl_builtins_8_3_boolean() {
    let data = r#"@prefix ex: <http://ex/> . ex:a ex:flag true ."#;
    check_cases(
        data,
        &[
            (
                "ex:flag(?x, ?b) ^ swrlb:booleanNot(?r, ?b) -> ex:not(?x, ?r)",
                "not",
                Some("?v = false"),
            ),
            (
                "ex:flag(?x, ?b) ^ swrlb:booleanNot(false, ?b) -> ex:wasTrue(?x, true)",
                "wasTrue",
                Some("?v"),
            ),
            (
                "ex:flag(?x, ?b) ^ swrlb:booleanNot(?b, ?y) -> ex:solved(?x, ?y)",
                "solved",
                Some("?v = false"),
            ),
            (
                "ex:flag(?x, ?b) ^ swrlb:booleanNot(true, ?b) -> ex:never(?x, true)",
                "never",
                None,
            ),
        ],
    )
    .await;
}

/// §8.4 strings, including XPath regular expressions and `tokenize`, which
/// binds one token per solution.
#[tokio::test]
async fn swrl_builtins_8_4_strings() {
    let data = r#"@prefix ex: <http://ex/> .
        ex:a ex:name "Ada Lovelace" ; ex:code "ab-12-cd" ; ex:sp "  a   b  " ."#;
    check_cases(
        data,
        &[
            ("ex:name(?x, ?n) ^ swrlb:stringEqualIgnoreCase(?n, \"ADA LOVELACE\") -> ex:ieq(?x, true)", "ieq", Some("?v")),
            ("ex:name(?x, ?n) ^ swrlb:stringConcat(?r, ?n, \"!\") -> ex:concat(?x, ?r)", "concat", Some("?v = \"Ada Lovelace!\"")),
            ("ex:name(?x, ?n) ^ swrlb:substring(?r, ?n, 1, 3) -> ex:sub3(?x, ?r)", "sub3", Some("?v = \"Ada\"")),
            ("ex:name(?x, ?n) ^ swrlb:substring(?r, ?n, 5) -> ex:subRest(?x, ?r)", "subRest", Some("?v = \"Lovelace\"")),
            ("ex:name(?x, ?n) ^ swrlb:stringLength(?r, ?n) -> ex:len(?x, ?r)", "len", Some("?v = 12")),
            ("ex:sp(?x, ?n) ^ swrlb:normalizeSpace(?r, ?n) -> ex:norm(?x, ?r)", "norm", Some("?v = \"a b\"")),
            ("ex:name(?x, ?n) ^ swrlb:upperCase(?r, ?n) -> ex:upper(?x, ?r)", "upper", Some("?v = \"ADA LOVELACE\"")),
            ("ex:name(?x, ?n) ^ swrlb:lowerCase(?r, ?n) -> ex:lower(?x, ?r)", "lower", Some("?v = \"ada lovelace\"")),
            ("ex:code(?x, ?n) ^ swrlb:translate(?r, ?n, \"abc\", \"ABC\") -> ex:tr(?x, ?r)", "tr", Some("?v = \"AB-12-Cd\"")),
            ("ex:name(?x, ?n) ^ swrlb:contains(?n, \"Love\") -> ex:has(?x, true)", "has", Some("?v")),
            ("ex:name(?x, ?n) ^ swrlb:contains(?n, \"xyz\") -> ex:hasNot(?x, true)", "hasNot", None),
            ("ex:name(?x, ?n) ^ swrlb:containsIgnoreCase(?n, \"LOVE\") -> ex:hasI(?x, true)", "hasI", Some("?v")),
            ("ex:name(?x, ?n) ^ swrlb:startsWith(?n, \"Ada\") -> ex:starts(?x, true)", "starts", Some("?v")),
            ("ex:name(?x, ?n) ^ swrlb:endsWith(?n, \"lace\") -> ex:ends(?x, true)", "ends", Some("?v")),
            ("ex:name(?x, ?n) ^ swrlb:substringBefore(?r, ?n, \" \") -> ex:before(?x, ?r)", "before", Some("?v = \"Ada\"")),
            ("ex:name(?x, ?n) ^ swrlb:substringAfter(?r, ?n, \" \") -> ex:after(?x, ?r)", "after", Some("?v = \"Lovelace\"")),
            ("ex:name(?x, ?n) ^ swrlb:matches(?n, \"^ada\", \"i\") -> ex:re(?x, true)", "re", Some("?v")),
            ("ex:name(?x, ?n) ^ swrlb:matches(?n, \"^Bob\") -> ex:reNot(?x, true)", "reNot", None),
            ("ex:code(?x, ?n) ^ swrlb:replace(?r, ?n, \"([a-z]+)\", \"<$1>\") -> ex:repl(?x, ?r)", "repl", Some("?v = \"<ab>-12-<cd>\"")),
            ("ex:code(?x, ?n) ^ swrlb:tokenize(?t, ?n, \"-\") -> ex:token(?x, ?t)", "token", Some("?v = \"12\"")),
        ],
    )
    .await;

    // tokenize binds each token: three solutions, three triples.
    let (state, token) = admin_state();
    load(&state, data);
    run_swrlapi(
        &state,
        &token,
        "ex:code(?x, ?n) ^ swrlb:tokenize(?t, ?n, \"-\") -> ex:token(?x, ?t)",
    )
    .await;
    let n = match state
        .store
        .query("SELECT (COUNT(*) AS ?n) WHERE { <http://ex/a> <http://ex/token> ?t }")
    {
        Ok(QueryResults::Solutions(mut s)) => s
            .next()
            .and_then(|r| r.ok())
            .and_then(|r| r.get("n").map(|t| t.to_string()))
            .unwrap_or_default(),
        _ => String::new(),
    };
    assert!(n.starts_with("\"3\""), "three tokens: {n}");

    // A regular expression that cannot compile refuses the rule.
    let (st, resp) = post_swrl(
        &test_app(state.clone()),
        &token,
        json!({
            "rules": "ex:code(?x, ?n) ^ swrlb:matches(?n, \"[\") -> ex:T(?x)",
            "format": "swrlapi",
            "prefixes": { "ex": "http://ex/" }
        }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp.contains("swrlb:matches"), "{resp}");
}

/// §8.5 dates, times and durations: the constructors build values from
/// components and split values into components; the arithmetic follows
/// XPath (a leap day, month-end clamping).
#[tokio::test]
async fn swrl_builtins_8_5_dates_times_durations() {
    let data = r#"@prefix ex: <http://ex/> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:a ex:born "1990-05-17"^^xsd:date ; ex:at "2024-02-29T10:30:00Z"^^xsd:dateTime ;
             ex:t "10:30:00"^^xsd:time ; ex:ym "P1Y2M"^^xsd:yearMonthDuration ;
             ex:dt "P1DT2H"^^xsd:dayTimeDuration ."#;
    check_cases(
        data,
        &[
            ("ex:ym(?x, ?d) ^ swrlb:yearMonthDuration(?r, 1, 2) ^ swrlb:equal(?r, ?d) -> ex:ymBuilt(?x, ?r)", "ymBuilt", Some("str(?v) = \"P1Y2M\"")),
            ("ex:ym(?x, ?d) ^ swrlb:yearMonthDuration(?d, ?y, ?m) -> ex:ymMonths(?x, ?m)", "ymMonths", Some("?v = 2")),
            ("ex:dt(?x, ?d) ^ swrlb:dayTimeDuration(?r, 1, 2, 0, 0) -> ex:dtBuilt(?x, ?r)", "dtBuilt", Some("str(?v) = \"P1DT2H\"")),
            ("ex:dt(?x, ?d) ^ swrlb:dayTimeDuration(?d, ?dd, ?h, ?mi, ?s) -> ex:dtHours(?x, ?h)", "dtHours", Some("?v = 2")),
            ("ex:at(?x, ?t) ^ swrlb:dateTime(?t, ?y, ?mo, ?d, ?h, ?mi, ?s, ?tz) -> ex:tz(?x, ?tz)", "tz", Some("?v = \"Z\"")),
            ("ex:at(?x, ?t) ^ swrlb:dateTime(?t, ?y, ?mo, ?d, ?h, ?mi, ?s) -> ex:year(?x, ?y)", "year", Some("?v = 2024")),
            ("ex:at(?x, ?t) ^ swrlb:dateTime(?r, 2024, 1, 2, 3, 4, 5, \"Z\") -> ex:dtm(?x, ?r)", "dtm", Some("str(?v) = \"2024-01-02T03:04:05Z\"")),
            ("ex:born(?x, ?b) ^ swrlb:date(?b, ?y, ?m, ?d) -> ex:bornYear(?x, ?y)", "bornYear", Some("?v = 1990")),
            ("ex:born(?x, ?b) ^ swrlb:date(?r, 2024, 2, 29) -> ex:leap(?x, ?r)", "leap", Some("str(?v) = \"2024-02-29\"")),
            ("ex:born(?x, ?b) ^ swrlb:date(?r, 2023, 2, 29) -> ex:noLeap(?x, ?r)", "noLeap", None),
            ("ex:t(?x, ?t) ^ swrlb:time(?r, 9, 5, 0) -> ex:time(?x, ?r)", "time", Some("str(?v) = \"09:05:00\"")),
            ("ex:ym(?x, ?d) ^ swrlb:addYearMonthDurations(?r, ?d, ?d) -> ex:ymAdd(?x, ?r)", "ymAdd", Some("str(?v) = \"P2Y4M\"")),
            ("ex:ym(?x, ?d) ^ swrlb:subtractYearMonthDurations(?r, ?d, \"P2M\"^^xsd:yearMonthDuration) -> ex:ymSub(?x, ?r)", "ymSub", Some("str(?v) = \"P1Y\"")),
            ("ex:ym(?x, ?d) ^ swrlb:multiplyYearMonthDuration(?r, ?d, 2) -> ex:ymMul(?x, ?r)", "ymMul", Some("str(?v) = \"P2Y4M\"")),
            ("ex:ym(?x, ?d) ^ swrlb:divideYearMonthDurations(?r, ?d, 2) -> ex:ymDiv(?x, ?r)", "ymDiv", Some("str(?v) = \"P7M\"")),
            ("ex:dt(?x, ?d) ^ swrlb:addDayTimeDurations(?r, ?d, ?d) -> ex:dtAdd(?x, ?r)", "dtAdd", Some("str(?v) = \"P2DT4H\"")),
            ("ex:dt(?x, ?d) ^ swrlb:subtractDayTimeDurations(?r, ?d, \"PT2H\"^^xsd:dayTimeDuration) -> ex:dtSub(?x, ?r)", "dtSub", Some("str(?v) = \"P1D\"")),
            ("ex:dt(?x, ?d) ^ swrlb:multiplyDayTimeDurations(?r, ?d, 2) -> ex:dtMul(?x, ?r)", "dtMul", Some("str(?v) = \"P2DT4H\"")),
            ("ex:dt(?x, ?d) ^ swrlb:divideDayTimeDuration(?r, ?d, 2) -> ex:dtDiv(?x, ?r)", "dtDiv", Some("str(?v) = \"PT13H\"")),
            ("ex:born(?x, ?b) ^ swrlb:subtractDates(?r, \"2024-03-01\"^^xsd:date, \"2024-02-28\"^^xsd:date) -> ex:days(?x, ?r)", "days", Some("str(?v) = \"P2D\"")),
            ("ex:t(?x, ?t) ^ swrlb:subtractTimes(?r, \"12:00:00\"^^xsd:time, ?t) -> ex:tdiff(?x, ?r)", "tdiff", Some("str(?v) = \"PT1H30M\"")),
            ("ex:at(?x, ?t) ^ ex:ym(?x, ?d) ^ swrlb:addYearMonthDurationToDateTime(?r, ?t, ?d) -> ex:dtYm(?x, ?r)", "dtYm", Some("str(?v) = \"2025-04-29T10:30:00Z\"")),
            ("ex:at(?x, ?t) ^ ex:dt(?x, ?d) ^ swrlb:addDayTimeDurationToDateTime(?r, ?t, ?d) -> ex:dtDt(?x, ?r)", "dtDt", Some("str(?v) = \"2024-03-01T12:30:00Z\"")),
            ("ex:at(?x, ?t) ^ ex:ym(?x, ?d) ^ swrlb:subtractYearMonthDurationFromDateTime(?r, ?t, ?d) -> ex:dtYmSub(?x, ?r)", "dtYmSub", Some("str(?v) = \"2022-12-29T10:30:00Z\"")),
            ("ex:at(?x, ?t) ^ ex:dt(?x, ?d) ^ swrlb:subtractDayTimeDurationFromDateTime(?r, ?t, ?d) -> ex:dtDtSub(?x, ?r)", "dtDtSub", Some("str(?v) = \"2024-02-28T08:30:00Z\"")),
            ("ex:born(?x, ?b) ^ ex:ym(?x, ?d) ^ swrlb:addYearMonthDurationToDate(?r, ?b, ?d) -> ex:dYm(?x, ?r)", "dYm", Some("str(?v) = \"1991-07-17\"")),
            ("ex:born(?x, ?b) ^ swrlb:addDayTimeDurationToDate(?r, ?b, \"P1D\"^^xsd:dayTimeDuration) -> ex:dDt(?x, ?r)", "dDt", Some("str(?v) = \"1990-05-18\"")),
            ("ex:born(?x, ?b) ^ ex:ym(?x, ?d) ^ swrlb:subtractYearMonthDurationFromDate(?r, ?b, ?d) -> ex:dYmSub(?x, ?r)", "dYmSub", Some("str(?v) = \"1989-03-17\"")),
            ("ex:born(?x, ?b) ^ swrlb:subtractDayTimeDurationFromDate(?r, ?b, \"P17D\"^^xsd:dayTimeDuration) -> ex:dDtSub(?x, ?r)", "dDtSub", Some("str(?v) = \"1990-04-30\"")),
            ("ex:t(?x, ?t) ^ swrlb:addDayTimeDurationToTime(?r, ?t, \"PT2H\"^^xsd:dayTimeDuration) -> ex:tAdd(?x, ?r)", "tAdd", Some("str(?v) = \"12:30:00\"")),
            ("ex:t(?x, ?t) ^ swrlb:subtractDayTimeDurationFromTime(?r, ?t, \"PT2H\"^^xsd:dayTimeDuration) -> ex:tSub(?x, ?r)", "tSub", Some("str(?v) = \"08:30:00\"")),
            ("ex:at(?x, ?t) ^ swrlb:subtractDateTimesYieldingDayTimeDuration(?r, ?t, \"2024-02-28T10:30:00Z\"^^xsd:dateTime) -> ex:yDt(?x, ?r)", "yDt", Some("str(?v) = \"P1D\"")),
            ("ex:at(?x, ?t) ^ swrlb:subtractDateTimesYieldingYearMonthDuration(?r, ?t, \"2023-01-29T10:30:00Z\"^^xsd:dateTime) -> ex:yYm(?x, ?r)", "yYm", Some("str(?v) = \"P1Y1M\"")),
        ],
    )
    .await;
}

/// §8.6 URIs: `resolveURI`, and `anyURI` building and splitting a URI.
#[tokio::test]
async fn swrl_builtins_8_6_uris() {
    let data = r#"@prefix ex: <http://ex/> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
        ex:a ex:base "http://example.org/a/" ; ex:u "http://example.org:8080/p?q=1#f"^^xsd:anyURI ."#;
    check_cases(
        data,
        &[
            ("ex:base(?x, ?b) ^ swrlb:resolveURI(?r, \"b/c\", ?b) -> ex:resolved(?x, ?r)", "resolved", Some("str(?v) = \"http://example.org/a/b/c\"")),
            ("ex:base(?x, ?b) ^ swrlb:anyURI(?r, \"http\", \"example.org\", \"8080\", \"/p\", \"q=1\", \"f\") -> ex:built(?x, ?r)", "built", Some("str(?v) = \"http://example.org:8080/p?q=1#f\"")),
            ("ex:u(?x, ?u) ^ swrlb:anyURI(?u, ?s, ?h, ?port, ?path, ?q, ?f) -> ex:host(?x, ?h)", "host", Some("?v = \"example.org\"")),
            ("ex:u(?x, ?u) ^ swrlb:anyURI(?u, ?s, ?h, ?port, ?path, ?q, ?f) -> ex:port(?x, ?port)", "port", Some("?v = \"8080\"")),
        ],
    )
    .await;
}

/// §8.7 lists: RDF lists in the data are read; constructive built-ins mint
/// deterministic `urn:ots:swrl:list:<hash>` nodes (decision D11) whose cells
/// are written with the head, so a second run derives nothing new.
#[tokio::test]
async fn swrl_builtins_8_7_lists() {
    let data = r#"@prefix ex: <http://ex/> .
        ex:a ex:items (1 2 3) ; ex:more (3 4) ; ex:mid (2 3) ; ex:none () ."#;
    check_cases(
        data,
        &[
            ("ex:items(?x, ?l) ^ swrlb:member(?e, ?l) -> ex:has(?x, ?e)", "has", Some("?v = 2")),
            ("ex:items(?x, ?l) ^ swrlb:length(?n, ?l) -> ex:len(?x, ?n)", "len", Some("?v = 3")),
            ("ex:items(?x, ?l) ^ swrlb:first(?f, ?l) -> ex:first(?x, ?f)", "first", Some("?v = 1")),
            ("ex:items(?x, ?l) ^ swrlb:rest(?r, ?l) ^ swrlb:length(?n, ?r) -> ex:restLen(?x, ?n)", "restLen", Some("?v = 2")),
            ("ex:items(?x, ?l) ^ ex:more(?x, ?m) ^ swrlb:listConcat(?c, ?l, ?m) ^ swrlb:length(?n, ?c) -> ex:catLen(?x, ?n)", "catLen", Some("?v = 5")),
            ("ex:items(?x, ?l) ^ ex:more(?x, ?m) ^ swrlb:listIntersection(?i, ?l, ?m) ^ swrlb:first(?f, ?i) -> ex:common(?x, ?f)", "common", Some("?v = 3")),
            ("ex:items(?x, ?l) ^ ex:more(?x, ?m) ^ swrlb:listSubtraction(?s, ?l, ?m) ^ swrlb:length(?n, ?s) -> ex:diffLen(?x, ?n)", "diffLen", Some("?v = 2")),
            ("ex:items(?x, ?l) ^ ex:mid(?x, ?m) ^ swrlb:sublist(?l, ?m) -> ex:hasMid(?x, true)", "hasMid", Some("?v")),
            ("ex:items(?x, ?l) ^ ex:more(?x, ?m) ^ swrlb:sublist(?l, ?m) -> ex:hasMore(?x, true)", "hasMore", None),
            ("ex:none(?x, ?z) ^ swrlb:empty(?z) -> ex:hasEmpty(?x, true)", "hasEmpty", Some("?v")),
            ("ex:items(?x, ?l) ^ swrlb:empty(?l) -> ex:itemsEmpty(?x, true)", "itemsEmpty", None),
        ],
    )
    .await;

    // A constructed list in the head is minted, its cells written.
    let (state, token) = admin_state();
    load(&state, data);
    let rule =
        "ex:items(?x, ?l) ^ ex:more(?x, ?m) ^ swrlb:listConcat(?c, ?l, ?m) -> ex:cat(?x, ?c)";
    let first = run_swrlapi(&state, &token, rule).await;
    assert!(
        ask(
            &state,
            "<http://ex/a> <http://ex/cat> ?c . \
             ?c <http://www.w3.org/1999/02/22-rdf-syntax-ns#rest>/<http://www.w3.org/1999/02/22-rdf-syntax-ns#rest>/\
                <http://www.w3.org/1999/02/22-rdf-syntax-ns#rest>/<http://www.w3.org/1999/02/22-rdf-syntax-ns#rest>/\
                <http://www.w3.org/1999/02/22-rdf-syntax-ns#first> 4 \
             FILTER(STRSTARTS(STR(?c), \"urn:ots:swrl:list:\"))"
        ),
        "{first}"
    );
    let again = run_swrlapi(&state, &token, rule).await;
    assert_eq!(
        again["triples_inferred"], 0,
        "the same list is the same node: {again}"
    );
}

/// The common use the engine could not run before: `add(?z, ?x, 1)` binds
/// `?z`, here in the OWL API's OWL/XML encoding.
#[tokio::test]
async fn swrl_builtin_binds_first_argument() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"<http://ex/p1> <http://ex/age> 30 . <http://ex/p2> <http://ex/age> 41 ."#,
    );
    let app = test_app(state.clone());
    let body = format!(
        r#"<DataPropertyAtom><DataProperty IRI="http://ex/age"/>{x}{a}</DataPropertyAtom>
           <BuiltInAtom IRI="http://www.w3.org/2003/11/swrlb#add">{n}{a}
             <Literal datatypeIRI="http://www.w3.org/2001/XMLSchema#integer">1</Literal>
           </BuiltInAtom>"#,
        x = var("x"),
        a = var("a"),
        n = var("n")
    );
    let head = format!(
        r#"<DataPropertyAtom><DataProperty IRI="http://ex/nextAge"/>{}{}</DataPropertyAtom>"#,
        var("x"),
        var("n")
    );
    let rules = owlxml(&[(&body, &head)]);
    let (st, resp) = post_swrl(&app, &token, json!({ "rules": rules, "format": "xml" })).await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(value_holds(&state, "p1", "nextAge", "?v = 31"), "{resp}");
    assert!(value_holds(&state, "p2", "nextAge", "?v = 42"), "{resp}");
    let report = json_body(&resp);
    assert_eq!(report["triples_inferred"], 2, "{resp}");
    assert_eq!(report["converged"], true, "{resp}");
}

/// A built-in whose unbound arguments have infinitely many solutions is
/// refused before anything runs, naming the built-in and the arguments.
#[tokio::test]
async fn swrl_infinite_builtin_pattern_refused() {
    let (state, token) = admin_state();
    load(&state, r#"<http://ex/p1> <http://ex/age> 30 ."#);
    let before = quad_count(&state);
    for (rule, named) in [
        (
            "ex:age(?x, ?a) ^ swrlb:add(?a, ?y, ?z) -> ex:p(?x, ?y)",
            "swrlb:add",
        ),
        (
            "ex:age(?x, ?a) ^ swrlb:lessThan(?a, ?y) -> ex:p(?x, ?y)",
            "swrlb:lessThan",
        ),
        (
            "ex:age(?x, ?a) ^ swrlb:member(?a, ?l) -> ex:p(?x, ?l)",
            "swrlb:member",
        ),
    ] {
        let (st, resp) = post_swrl(
            &test_app(state.clone()),
            &token,
            json!({ "rules": rule, "format": "swrlapi", "prefixes": { "ex": "http://ex/" } }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{rule}: {resp}");
        assert!(
            resp.contains(named) && resp.contains("infinitely many"),
            "{rule}: {resp}"
        );
    }
    assert_eq!(quad_count(&state), before, "nothing may be written");
}

// ─── DataRange and class-expression atoms (SW-F) ──────────────────────────

/// `DataRangeAtom` evaluated natively: datatypes by value space, facets,
/// `DataOneOf` (which also enumerates an unbound variable), complements.
#[tokio::test]
async fn swrl_data_range_atom() {
    let (state, token) = admin_state();
    load(
        &state,
        r#"@prefix ex: <http://ex/> . @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
           ex:p1 ex:age 30 . ex:p2 ex:age 12 . ex:p3 ex:age "thirty" .
           ex:p4 ex:age "18"^^xsd:byte ."#,
    );
    let app = test_app(state.clone());
    let ofn = r#"Prefix(:=<http://ex/>)
DLSafeRule(Body(
    DataPropertyAtom(:age Variable(<urn:v#x>) Variable(<urn:v#a>))
    DataRangeAtom(DatatypeRestriction(xsd:integer xsd:minInclusive "18"^^xsd:integer) Variable(<urn:v#a>))
  ) Head(ClassAtom(:Adult Variable(<urn:v#x>))))
DLSafeRule(Body(
    DataPropertyAtom(:age Variable(<urn:v#x>) Variable(<urn:v#a>))
    DataRangeAtom(DataComplementOf(xsd:integer) Variable(<urn:v#a>))
  ) Head(ClassAtom(:Odd Variable(<urn:v#x>))))
DLSafeRule(Body(
    ClassAtom(:Adult Variable(<urn:v#x>))
    DataRangeAtom(DataOneOf("gold" "silver") Variable(<urn:v#t>))
  ) Head(DataPropertyAtom(:tier Variable(<urn:v#x>) Variable(<urn:v#t>))))"#;
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": ofn, "format": "functional" }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    assert!(ask(&state, "<http://ex/p1> a <http://ex/Adult>"), "{resp}");
    assert!(
        ask(&state, "<http://ex/p4> a <http://ex/Adult>"),
        "xsd:byte 18 is an integer ≥ 18: {resp}"
    );
    assert!(!ask(&state, "<http://ex/p2> a <http://ex/Adult>"), "{resp}");
    assert!(!ask(&state, "<http://ex/p3> a <http://ex/Adult>"), "{resp}");
    assert!(ask(&state, "<http://ex/p3> a <http://ex/Odd>"), "{resp}");
    assert!(!ask(&state, "<http://ex/p1> a <http://ex/Odd>"), "{resp}");
    assert!(value_holds(&state, "p1", "tier", "?v = \"gold\""), "{resp}");
    assert!(
        value_holds(&state, "p1", "tier", "?v = \"silver\""),
        "{resp}"
    );

    // A data range over a datatype the engine does not know is refused.
    let unknown = r#"DLSafeRule(Body(
        DataPropertyAtom(<http://ex/age> Variable(<urn:v#x>) Variable(<urn:v#a>))
        DataRangeAtom(<http://ex/myType> Variable(<urn:v#a>))
      ) Head(ClassAtom(<http://ex/T> Variable(<urn:v#x>))))"#;
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": unknown, "format": "functional" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
    assert!(resp.contains("myType"), "{resp}");
}

/// Class-expression atoms run jointly with a regime: the expression becomes
/// an auxiliary class `urn:ots:swrl:aux:<hash>` equivalent to it, OWL 2 RL
/// materialises its members, and the rule reads them. In the head an
/// intersection is asserted through the same class.
#[tokio::test]
async fn swrl_class_expression_atom_via_regime() {
    use open_triplestore::auth::models::{GraphKind, OwnerType, Visibility};
    const G: &str = "http://example.org/swrl/ce";
    let (state, token) = admin_state();
    // An explicit source graph no dataset holds is unreadable, even for an
    // admin: the data lives in a dataset.
    state
        .auth_db
        .create_dataset(
            "ce",
            "CE",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("ce", G).unwrap();
    state
        .auth_db
        .set_dataset_graph_role("ce", G, Some(GraphKind::Instances))
        .unwrap();
    state
        .store
        .load_str(
            r#"<http://ex/p1> a <http://ex/Person> ; <http://ex/knows> <http://ex/p2> .
               <http://ex/p2> a <http://ex/Person> .
               <http://ex/p3> a <http://ex/Person> ."#,
            oxigraph::io::RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    let app = test_app(state.clone());
    let body = format!(
        r#"<ClassAtom><ObjectSomeValuesFrom><ObjectProperty IRI="http://ex/knows"/><Class IRI="http://ex/Person"/></ObjectSomeValuesFrom>{x}</ClassAtom>"#,
        x = var("x")
    );
    let head = format!(
        r#"<ClassAtom><ObjectIntersectionOf><Class IRI="http://ex/Social"/><Class IRI="http://ex/Tagged"/></ObjectIntersectionOf>{x}</ClassAtom>"#,
        x = var("x")
    );
    let rules = owlxml(&[(&body, &head)]);
    let out = "urn:swrl:ce:out";
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({
            "rules": rules,
            "format": "xml",
            "regime": "owl2-rl",
            "dataset": "ce",
            "target_graph": out
        }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{resp}");
    let report = json_body(&resp);
    assert_eq!(report["converged"], true, "{resp}");
    assert_eq!(report["regime"], "owl2-rl", "{resp}");
    assert!(
        ask(
            &state,
            &format!("GRAPH <{out}> {{ <http://ex/p1> a <http://ex/Social>, <http://ex/Tagged> }}")
        ),
        "p1 knows a Person, so the head intersection holds of it: {resp}"
    );
    assert!(
        !ask(
            &state,
            &format!("GRAPH <{out}> {{ <http://ex/p3> a <http://ex/Social> }}")
        ),
        "p3 knows nobody: {resp}"
    );
    assert!(
        ask(
            &state,
            &format!(
                "GRAPH <{out}> {{ ?aux <http://www.w3.org/2002/07/owl#equivalentClass> ?e \
                 FILTER(STRSTARTS(STR(?aux), \"urn:ots:swrl:aux:\")) }}"
            )
        ),
        "the auxiliary class axioms are written to the target: {resp}"
    );

    // A regime needs a scope to read.
    let (st, resp) = post_swrl(
        &app,
        &token,
        json!({ "rules": rules, "format": "xml", "regime": "owl2-rl" }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{resp}");
}
