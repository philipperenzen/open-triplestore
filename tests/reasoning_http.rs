//! `POST /api/reasoning/materialize` over HTTP: entailment graphs are rebuilt,
//! not accumulated, and the report counts what this run added.
//!
//! Materialisation only ever INSERTed into `urn:entailment:*`, so a source
//! triple that was later deleted kept its stale consequences forever — and they
//! were still folded into every `?entailment=` query. Separately, every
//! materializer reported `triples_added` as the target graph's SIZE, so a run
//! that inferred nothing still claimed thousands of additions.

#![cfg(feature = "rdfs-entailment")]

mod common;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use open_triplestore::server::AppState;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const RDFS_TG: &str = "urn:entailment:rdfs";

async fn materialize(state: &AppState, token: &str, regime: &str) -> (StatusCode, Value) {
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/reasoning/materialize")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(json!({ "regime": regime }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    (st, body_json(resp.into_body()).await)
}

fn entailed(state: &AppState, pattern: &str) -> bool {
    matches!(
        state
            .store
            .query(&format!("ASK {{ GRAPH <{RDFS_TG}> {{ {pattern} }} }}")),
        Ok(QueryResults::Boolean(true))
    )
}

fn count_entailed(state: &AppState) -> usize {
    match state.store.query(&format!(
        "SELECT (COUNT(*) AS ?c) WHERE {{ GRAPH <{RDFS_TG}> {{ ?s ?p ?o }} }}"
    )) {
        Ok(QueryResults::Solutions(mut s)) => s
            .next()
            .and_then(|r| r.ok())
            .and_then(|r| r.get("c").map(|t| t.to_string()))
            .and_then(|t| t.trim_start_matches('"').split('"').next()?.parse().ok())
            .unwrap_or(0),
        _ => 0,
    }
}

/// Deleting a source triple and re-materialising removes its consequences.
#[tokio::test]
async fn rematerialising_drops_stale_inferences() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
             @prefix ex: <http://example.org/> . \
             ex:Dog rdfs:subClassOf ex:Animal . \
             ex:rex a ex:Dog .",
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();

    let (st, body) = materialize(&state, &token, "rdfs").await;
    assert_eq!(st, StatusCode::OK, "first run: {body}");
    assert!(
        entailed(
            &state,
            "<http://example.org/rex> a <http://example.org/Animal>"
        ),
        "rdfs9 must derive rex a Animal"
    );

    // The source of that inference goes away.
    state
        .store
        .update("DELETE DATA { <http://example.org/rex> a <http://example.org/Dog> }")
        .unwrap();

    let (st, body) = materialize(&state, &token, "rdfs").await;
    assert_eq!(st, StatusCode::OK, "second run: {body}");
    assert!(
        !entailed(
            &state,
            "<http://example.org/rex> a <http://example.org/Animal>"
        ),
        "a consequence whose premise was deleted must not survive re-materialisation"
    );
}

/// Over HTTP the entailment graph is rebuilt from scratch each run, so the
/// report's `triples_added` is the size of the freshly rebuilt graph — and that
/// size must SHRINK when a premise is removed. Without the rebuild, stale
/// consequences stayed put and the count could only ever grow. (The
/// delta-versus-size distinction itself is pinned at store level, where no
/// clearing happens: see `test_el_idempotent` and `test_rdfs_rerun_adds_nothing`.)
#[tokio::test]
async fn rebuilt_graph_shrinks_when_a_premise_is_removed() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
             @prefix ex: <http://example.org/> . \
             ex:Dog rdfs:subClassOf ex:Animal . \
             ex:rex a ex:Dog . ex:fido a ex:Dog .",
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();

    let (st, first) = materialize(&state, &token, "rdfs").await;
    assert_eq!(st, StatusCode::OK, "{first}");
    let added_first = first["triples_added"].as_u64().unwrap();
    let size_first = count_entailed(&state) as u64;
    assert!(added_first > 0, "a fresh run must add something: {first}");
    assert_eq!(
        added_first, size_first,
        "into an empty entailment graph, added == size: {first}"
    );

    // Remove one premise. The rebuilt graph loses its consequences, so both
    // the reported count and the graph itself get smaller.
    state
        .store
        .update("DELETE DATA { <http://example.org/fido> a <http://example.org/Dog> }")
        .unwrap();
    let (st, second) = materialize(&state, &token, "rdfs").await;
    assert_eq!(st, StatusCode::OK, "{second}");
    let added_second = second["triples_added"].as_u64().unwrap();
    assert!(
        added_second < added_first,
        "after removing a premise the rebuilt graph must be smaller: \
         first={added_first} second={added_second}"
    );
    assert_eq!(
        count_entailed(&state) as u64,
        added_second,
        "the reported count must be exactly what the rebuilt graph holds"
    );
}

/// `?entailment=owl2-dl` is advertised in the OpenAPI spec and docs/owl2-dl.md,
/// but the entailment match had no arm for it: the value fell through to
/// `None`, the query ran with no entailment graph at all, and the client got a
/// 200 with the un-entailed answer. Now it selects `urn:entailment:owl2-dl`
/// exactly as the other regimes select theirs.
#[cfg(feature = "owl2-dl")]
#[tokio::test]
async fn entailment_owl2_dl_selects_the_dl_graph() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            "<urn:e:s> <urn:e:p> <urn:e:o> .",
            oxigraph::io::RdfFormat::Turtle,
            Some("urn:entailment:owl2-dl"),
        )
        .unwrap();
    let ask = |regime: Option<&str>| {
        let app = test_app(state.clone());
        let token = token.clone();
        let q = url_encode("ASK { <urn:e:s> <urn:e:p> <urn:e:o> }");
        let uri = match regime {
            Some(r) => format!("/sparql?query={q}&entailment={r}"),
            None => format!("/sparql?query={q}"),
        };
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(uri)
                        .header(header::ACCEPT, "application/sparql-results+json")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let v = body_json(resp.into_body()).await;
            v["boolean"]
                .as_bool()
                .unwrap_or_else(|| panic!("not an ASK result: {v}"))
        }
    };
    let baseline = ask(None).await;
    assert!(
        ask(Some("owl2-dl")).await,
        "owl2-dl must fold the DL entailment graph into the query"
    );
    assert_eq!(
        ask(Some("owl2-ql")).await,
        baseline,
        "another regime must not pull in the DL graph"
    );
}

/// `POST /api/reasoning/rewrite` spells out the TBox in its answer (every
/// subclass it unions in), so it reads the TBox only from graphs the caller
/// may read. It read the whole unnamed default graph for anyone, and none of
/// the named graphs `/sparql` would have scoped the same caller to.
#[cfg(feature = "owl2-ql")]
#[tokio::test]
async fn rewrite_reads_only_the_callers_graphs() {
    use open_triplestore::auth::models::SystemRole;
    const MINE: &str = "http://example.org/g/mine";
    const SECRET: &str = "http://example.org/g/secret";
    let (state, _admin) = admin_state();
    let load = |ttl: &str, graph: Option<&str>| {
        state
            .store
            .load_str(ttl, oxigraph::io::RdfFormat::Turtle, graph)
            .unwrap()
    };
    load(
        "<http://example.org/Lecturer> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://example.org/Staff> .",
        Some(MINE),
    );
    load(
        "<http://example.org/SecretAgent> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://example.org/Staff> .",
        Some(SECRET),
    );
    load(
        "<http://example.org/Hidden> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://example.org/Staff> .",
        None,
    );
    state
        .auth_db
        .create_user("reader", "reader", "reader@t.com", "hash", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .grant_graph_permission("rule-1", MINE, "user", "reader", "read", "adm")
        .unwrap();
    let rewrite = |user: &str, role: &str| {
        let app = test_app(state.clone());
        let token = mint_token(user, user, role);
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/reasoning/rewrite")
                        .header(header::CONTENT_TYPE, "application/json")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::from(
                            json!({ "query": "SELECT ?x WHERE { ?x a <http://example.org/Staff> }" })
                                .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            body_json(resp.into_body()).await["rewritten"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };

    let rewritten = rewrite("reader", "user").await;
    assert!(rewritten.contains("Lecturer"), "{rewritten}");
    assert!(!rewritten.contains("SecretAgent"), "{rewritten}");
    assert!(!rewritten.contains("Hidden"), "{rewritten}");

    // A caller who may read nothing gets the query back unchanged.
    state
        .auth_db
        .create_user("nobody", "nobody", "nobody@t.com", "hash", SystemRole::User)
        .unwrap();
    let rewritten = rewrite("nobody", "user").await;
    assert!(!rewritten.contains("Lecturer"), "{rewritten}");
    assert!(!rewritten.contains("Hidden"), "{rewritten}");
}

// ── Identity policy (owl:sameAs) — P1 item 1 ─────────────────────────────────
//
// What a dataset's materialisation does with owl:sameAs is a per-dataset (or
// per-organisation) setting. `sameas-narrow` — the built-in default — keeps
// linkset-role graphs out of the premises, so a cross-source sameAs never
// leaks attributes between representations; `sameas-full` is the old
// behaviour; `sameas-off` keeps sameAs as plain data. Typed correspondences
// (prov:specializationOf, prov:alternateOf, skos:*Match) never propagate.
#[cfg(feature = "owl2-rl")]
mod identity_policy {
    use super::*;
    use axum::Router;
    use open_triplestore::auth::models::{GraphKind, OwnerType, Role, SystemRole, Visibility};
    use oxigraph::io::RdfFormat;

    const INST: &str = "https://example.org/idp/instances";
    const LINKS: &str = "https://example.org/idp/linkset";
    const EX: &str = "https://example.org/idp/";

    async fn req(
        app: &Router,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value, String) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
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

    /// A dataset `d` (owned by `owner`): an instance graph with an asset that has
    /// a status and a registration record with a registrar, plus a linkset graph
    /// that says the asset and the record are owl:sameAs.
    fn dataset(state: &AppState, id: &str, owner: (OwnerType, &str)) -> (String, String) {
        state
            .auth_db
            .create_dataset(id, id, None, owner.0, owner.1, Visibility::Public, None)
            .unwrap();
        let inst = format!("{INST}/{id}");
        let links = format!("{LINKS}/{id}");
        state.auth_db.add_dataset_graph(id, &inst).unwrap();
        state
            .auth_db
            .set_dataset_graph_role(id, &inst, Some(GraphKind::Instances))
            .unwrap();
        state.auth_db.add_dataset_graph(id, &links).unwrap();
        state
            .auth_db
            .set_dataset_graph_role(id, &links, Some(GraphKind::Linkset))
            .unwrap();
        state
            .store
            .load_str(
                &format!(
                    "<{EX}asset1> <{EX}status> \"in service\" . \
                     <{EX}record1> <{EX}registeredBy> \"registry\" ."
                ),
                RdfFormat::Turtle,
                Some(&inst),
            )
            .unwrap();
        state
            .store
            .load_str(
                &format!("<{EX}asset1> <http://www.w3.org/2002/07/owl#sameAs> <{EX}record1> ."),
                RdfFormat::Turtle,
                Some(&links),
            )
            .unwrap();
        (inst, links)
    }

    fn entailed(state: &AppState, ds: &str, pattern: &str) -> bool {
        let g = format!("urn:entailment:owl2-rl:{ds}");
        matches!(
            state
                .store
                .query(&format!("ASK {{ GRAPH <{g}> {{ {pattern} }} }}")),
            Ok(QueryResults::Boolean(true))
        )
    }

    /// Materialise owl2-rl for `ds` through the settings endpoint.
    async fn materialize(app: &Router, token: &str, ds: &str, identity: Option<&str>) -> Value {
        let mut body = json!({ "regime": "owl2-rl", "mode": "materialize" });
        if let Some(i) = identity {
            body["identity"] = json!(i);
        }
        let (st, v, txt) = req(
            app,
            Method::PUT,
            &format!("/api/datasets/{ds}/entailment"),
            token,
            Some(body),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        v
    }

    #[tokio::test]
    async fn default_is_narrow_and_keeps_the_linkset_out_of_the_premises() {
        let (state, token) = admin_state();
        let (inst, links) = dataset(&state, "idp-narrow", (OwnerType::User, "adm"));
        let app = test_app(state.clone());

        let v = materialize(&app, &token, "idp-narrow", None).await;
        assert_eq!(v["identity"], "sameas-narrow", "{v}");
        assert_eq!(v["identity_source"], "default", "{v}");
        // The record's registrar did not leak onto the asset, nor the asset's
        // status onto the record: the linkset's sameAs was not a premise.
        assert!(
            !entailed(
                &state,
                "idp-narrow",
                &format!("<{EX}asset1> <{EX}registeredBy> ?x")
            ),
            "a linkset sameAs must not propagate under sameas-narrow"
        );
        assert!(!entailed(
            &state,
            "idp-narrow",
            &format!("<{EX}record1> <{EX}status> ?x")
        ));

        let (st, v, txt) = req(
            &app,
            Method::GET,
            "/api/datasets/idp-narrow/entailment",
            &token,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(v["identity"], "sameas-narrow", "{txt}");
        let sources: Vec<String> = v["reasoning_sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect();
        assert!(sources.contains(&inst), "{txt}");
        assert!(
            !sources.contains(&links),
            "the linkset graph is not an effective reasoning source: {txt}"
        );
    }

    #[tokio::test]
    async fn full_propagates_across_the_linkset() {
        let (state, token) = admin_state();
        dataset(&state, "idp-full", (OwnerType::User, "adm"));
        let app = test_app(state.clone());

        let v = materialize(&app, &token, "idp-full", Some("sameas-full")).await;
        assert_eq!(v["identity"], "sameas-full", "{v}");
        assert_eq!(v["identity_source"], "dataset", "{v}");
        assert!(
            entailed(
                &state,
                "idp-full",
                &format!("<{EX}asset1> <{EX}registeredBy> \"registry\"")
            ),
            "eq-rep-s over the linkset sameAs"
        );
        assert!(
            entailed(
                &state,
                "idp-full",
                &format!("<{EX}record1> <{EX}status> \"in service\"")
            ),
            "eq-sym + eq-rep-s in the other direction"
        );

        // Back to narrow through the dedicated endpoint: re-materialised at once.
        let (st, v, txt) = req(
            &app,
            Method::PUT,
            "/api/datasets/idp-full/identity",
            &token,
            Some(json!({ "policy": "sameas-narrow" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(v["policy"], "sameas-narrow", "{txt}");
        assert_eq!(v["source"], "dataset", "{txt}");
        assert!(
            !entailed(
                &state,
                "idp-full",
                &format!("<{EX}asset1> <{EX}registeredBy> ?x")
            ),
            "the change re-materialised the dataset without the linkset"
        );
    }

    #[tokio::test]
    async fn off_disables_the_equality_rules_even_inside_the_dataset() {
        let (state, token) = admin_state();
        let (inst, _) = dataset(&state, "idp-off", (OwnerType::User, "adm"));
        // A within-source sameAs: two IRIs for one record, inside the instance graph.
        state
            .store
            .load_str(
                &format!(
                    "<{EX}asset1> <http://www.w3.org/2002/07/owl#sameAs> <{EX}asset1-dup> . \
                     <{EX}asset1-dup> <{EX}inspected> \"2026-01-01\" ."
                ),
                RdfFormat::Turtle,
                Some(&inst),
            )
            .unwrap();
        let app = test_app(state.clone());

        materialize(&app, &token, "idp-off", Some("sameas-narrow")).await;
        assert!(
            entailed(
                &state,
                "idp-off",
                &format!("<{EX}asset1> <{EX}inspected> ?x")
            ),
            "narrow: a sameAs inside the dataset's own graph merges the two IRIs"
        );

        let (st, v, txt) = req(
            &app,
            Method::PUT,
            "/api/datasets/idp-off/identity",
            &token,
            Some(json!({ "policy": "sameas-off" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(v["policy"], "sameas-off", "{txt}");
        assert!(
            !entailed(
                &state,
                "idp-off",
                &format!("<{EX}asset1> <{EX}inspected> ?x")
            ),
            "off: the equality rules do not run at all"
        );
        assert!(
            !entailed(
                &state,
                "idp-off",
                &format!("<{EX}asset1-dup> <http://www.w3.org/2002/07/owl#sameAs> <{EX}asset1>")
            ),
            "off: not even eq-sym"
        );
    }

    #[tokio::test]
    async fn typed_correspondences_never_propagate_whatever_the_policy() {
        let (state, token) = admin_state();
        let (inst, _) = dataset(&state, "idp-corr", (OwnerType::User, "adm"));
        state
            .store
            .load_str(
                &format!(
                    "<{EX}elem4711> <http://www.w3.org/ns/prov#specializationOf> <{EX}asset1> . \
                     <{EX}elem4711> <{EX}globalId> \"2F\" . \
                     <{EX}asset1> <http://www.w3.org/ns/prov#alternateOf> <{EX}footprint1> . \
                     <{EX}footprint1> <{EX}area> 12 . \
                     <{EX}asset1> <http://www.w3.org/2004/02/skos/core#exactMatch> <{EX}concept1> . \
                     <{EX}concept1> <{EX}code> \"C1\" ."
                ),
                RdfFormat::Turtle,
                Some(&inst),
            )
            .unwrap();
        let app = test_app(state.clone());
        materialize(&app, &token, "idp-corr", Some("sameas-full")).await;
        for (what, pattern) in [
            (
                "prov:specializationOf",
                format!("<{EX}asset1> <{EX}globalId> ?x"),
            ),
            ("prov:alternateOf", format!("<{EX}asset1> <{EX}area> ?x")),
            ("skos:exactMatch", format!("<{EX}asset1> <{EX}code> ?x")),
        ] {
            assert!(
                !entailed(&state, "idp-corr", &pattern),
                "{what} must never feed the equality rules"
            );
        }
        // …while the real sameAs (full) did propagate, so the run was live.
        assert!(entailed(
            &state,
            "idp-corr",
            &format!("<{EX}asset1> <{EX}registeredBy> ?x")
        ));
    }

    #[tokio::test]
    async fn organisation_setting_is_inherited_and_overridable() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_user("bob", "bob", "bob@test.com", "hash", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .add_org_member("bob", "o1", Role::Member)
            .unwrap();
        let bob = mint_token("bob", "bob", "user");
        dataset(&state, "idp-org", (OwnerType::Organisation, "o1"));
        let app = test_app(state.clone());

        // Nothing set anywhere: default.
        let (st, v, txt) = req(
            &app,
            Method::GET,
            "/api/datasets/idp-org/identity",
            &token,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(v["policy"], "sameas-narrow", "{txt}");
        assert_eq!(v["source"], "default", "{txt}");
        assert!(v["setting"].is_null(), "{txt}");
        assert_eq!(v["options"].as_array().map(|a| a.len()), Some(3), "{txt}");
        assert!(
            v["description"].as_str().is_some_and(|d| !d.is_empty()),
            "{txt}"
        );

        // A member may read the organisation's policy but not set it.
        let (st, _, txt) = req(
            &app,
            Method::GET,
            "/api/organisations/o1/identity",
            &bob,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        let (st, _, txt) = req(
            &app,
            Method::PUT,
            "/api/organisations/o1/identity",
            &bob,
            Some(json!({ "policy": "sameas-full" })),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::FORBIDDEN,
            "a member cannot set the org policy: {txt}"
        );

        // The admin sets it for the organisation: the dataset inherits it.
        materialize(&app, &token, "idp-org", None).await;
        let (st, v, txt) = req(
            &app,
            Method::PUT,
            "/api/organisations/o1/identity",
            &token,
            Some(json!({ "policy": "sameas-full" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(v["policy"], "sameas-full", "{txt}");
        let (_, v, txt) = req(
            &app,
            Method::GET,
            "/api/datasets/idp-org/identity",
            &token,
            None,
        )
        .await;
        assert_eq!(v["policy"], "sameas-full", "{txt}");
        assert_eq!(v["source"], "organisation", "{txt}");
        assert!(
            entailed(
                &state,
                "idp-org",
                &format!("<{EX}asset1> <{EX}registeredBy> ?x")
            ),
            "the inheriting dataset was re-materialised under the organisation's policy"
        );

        // The dataset overrides the organisation…
        let (st, v, txt) = req(
            &app,
            Method::PUT,
            "/api/datasets/idp-org/identity",
            &token,
            Some(json!({ "policy": "sameas-off" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(v["source"], "dataset", "{txt}");
        assert!(!entailed(
            &state,
            "idp-org",
            &format!("<{EX}asset1> <{EX}registeredBy> ?x")
        ));

        // …and dropping the override falls back to the organisation.
        let (st, v, txt) = req(
            &app,
            Method::DELETE,
            "/api/datasets/idp-org/identity",
            &token,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(v["policy"], "sameas-full", "{txt}");
        assert_eq!(v["source"], "organisation", "{txt}");
        assert!(entailed(
            &state,
            "idp-org",
            &format!("<{EX}asset1> <{EX}registeredBy> ?x")
        ));

        // An unknown value is a 400, on both endpoints.
        let (st, _, _) = req(
            &app,
            Method::PUT,
            "/api/datasets/idp-org/identity",
            &token,
            Some(json!({ "policy": "sameas-maybe" })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        let (st, _, _) = req(
            &app,
            Method::PUT,
            "/api/datasets/idp-org/entailment",
            &token,
            Some(json!({ "regime": "owl2-rl", "identity": "sameas-maybe" })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }
}

// ── Inconsistency and non-convergence are 422s, not 500s ─────────────────────
//
// An ontology that entails `false` is a property of the caller's data, so the
// run answers 422 with the rule that fired; the consequences derived before
// the check stay in the target graph. The per-dataset endpoint answers the
// same and records the outcome, which GET …/entailment reports.
#[cfg(feature = "owl2-rl")]
mod consistency {
    use super::*;
    use axum::Router;
    use open_triplestore::auth::models::{GraphKind, OwnerType, Visibility};
    use oxigraph::io::RdfFormat;

    const RL_TG: &str = "urn:entailment:owl2-rl";
    const EX: &str = "http://example.org/";
    /// A class under one of two disjoint classes, and an individual in both:
    /// cax-dw only fires on the type cax-sco derives.
    const INCONSISTENT: &str = "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> . \
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
         @prefix owl: <http://www.w3.org/2002/07/owl#> . \
         @prefix ex: <http://example.org/> . \
         ex:Cat rdfs:subClassOf ex:Animal . \
         ex:Animal owl:disjointWith ex:Mineral . \
         ex:felix rdf:type ex:Cat , ex:Mineral .";

    async fn req(
        app: &Router,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value, String) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
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
    async fn inconsistent_ontology_is_a_422_naming_the_rule() {
        let (state, token) = admin_state();
        state
            .store
            .load_str(INCONSISTENT, RdfFormat::Turtle, None)
            .unwrap();

        let (st, body) = materialize(&state, &token, "owl2-rl").await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["consistent"], json!(false), "{body}");
        assert_eq!(body["rule"], "cax-dw", "{body}");
        assert_eq!(body["regime"], "owl2-rl", "{body}");
        assert_eq!(body["target_graph"], RL_TG, "{body}");
        assert!(
            body["detail"].as_str().is_some_and(|d| !d.is_empty()),
            "{body}"
        );
        // What was derived before the check is kept.
        assert!(
            matches!(
                state.store.query(&format!(
                    "ASK {{ GRAPH <{RL_TG}> {{ <{EX}felix> a <{EX}Animal> }} }}"
                )),
                Ok(QueryResults::Boolean(true))
            ),
            "cax-sco's consequence stays in the target graph"
        );
    }

    #[tokio::test]
    async fn a_consistent_run_says_so_and_a_regime_without_checks_does_not() {
        let (state, token) = admin_state();
        state
            .store
            .load_str(
                "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
                 @prefix ex: <http://example.org/> . \
                 ex:Cat rdfs:subClassOf ex:Animal . ex:felix a ex:Cat .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();
        let (st, body) = materialize(&state, &token, "owl2-rl").await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["consistent"], json!(true), "{body}");
        // RDFS has no inconsistency rules: it cannot claim consistency.
        let (st, body) = materialize(&state, &token, "rdfs").await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(body["consistent"].is_null(), "{body}");
    }

    /// `eq_ref` in the body turns eq-ref on for that run; it is off without it.
    /// A new inconsistency rule (prp-pdw) is a 422 that names it.
    #[tokio::test]
    async fn eq_ref_is_a_request_option_and_new_rules_name_themselves() {
        let (state, token) = admin_state();
        state
            .store
            .load_str(
                "@prefix ex: <http://example.org/> . ex:x ex:p ex:y .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();
        let reflexive = |state: &AppState| {
            matches!(
                state.store.query(&format!(
                    "ASK {{ GRAPH <{RL_TG}> {{ <{EX}x> <http://www.w3.org/2002/07/owl#sameAs> <{EX}x> }} }}"
                )),
                Ok(QueryResults::Boolean(true))
            )
        };
        let (st, body) = materialize(&state, &token, "owl2-rl").await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(!reflexive(&state), "eq-ref is off by default");
        let app = test_app(state.clone());
        let (st, _, txt) = req(
            &app,
            Method::POST,
            "/api/reasoning/materialize",
            &token,
            Some(json!({ "regime": "owl2-rl", "eq_ref": true })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert!(reflexive(&state), "eq_ref: true writes x owl:sameAs x");

        state
            .store
            .load_str(
                "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
                 @prefix ex: <http://example.org/> . \
                 ex:p owl:propertyDisjointWith ex:q . ex:x ex:q ex:y .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();
        let (st, body) = materialize(&state, &token, "owl2-rl").await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["rule"], "prp-pdw", "{body}");
    }

    #[tokio::test]
    async fn dataset_run_answers_422_and_records_the_outcome() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_dataset(
                "inc",
                "inc",
                None,
                OwnerType::User,
                "adm",
                Visibility::Public,
                None,
            )
            .unwrap();
        let inst = "https://example.org/inc/instances";
        state.auth_db.add_dataset_graph("inc", inst).unwrap();
        state
            .auth_db
            .set_dataset_graph_role("inc", inst, Some(GraphKind::Instances))
            .unwrap();
        state
            .store
            .load_str(INCONSISTENT, RdfFormat::Turtle, Some(inst))
            .unwrap();
        let app = test_app(state.clone());

        let (st, v, txt) = req(
            &app,
            Method::PUT,
            "/api/datasets/inc/entailment",
            &token,
            Some(json!({ "regime": "owl2-rl", "mode": "materialize" })),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{txt}");
        assert_eq!(v["consistent"], json!(false), "{txt}");
        assert_eq!(v["rule"], "cax-dw", "{txt}");
        assert_eq!(v["target_graph"], "urn:entailment:owl2-rl:inc", "{txt}");

        let (st, v, txt) = req(
            &app,
            Method::GET,
            "/api/datasets/inc/entailment",
            &token,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        assert_eq!(v["consistent"], json!(false), "{txt}");
        assert_eq!(v["inconsistency"]["rule"], "cax-dw", "{txt}");
        assert!(v["last_run_at"].is_string(), "{txt}");

        // Repair the data: the next run is consistent and the record follows.
        state
            .store
            .update(&format!(
                "DELETE DATA {{ GRAPH <{inst}> {{ <{EX}felix> a <{EX}Mineral> }} }}"
            ))
            .unwrap();
        let (st, _, txt) = req(
            &app,
            Method::PUT,
            "/api/datasets/inc/entailment",
            &token,
            Some(json!({ "regime": "owl2-rl", "mode": "materialize" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{txt}");
        let (_, v, txt) = req(
            &app,
            Method::GET,
            "/api/datasets/inc/entailment",
            &token,
            None,
        )
        .await;
        assert_eq!(v["consistent"], json!(true), "{txt}");
        assert!(v["inconsistency"].is_null(), "{txt}");
    }
}

/// `POST /api/reasoning/materialize` with `owl2-ql` writes the ground closure
/// (it wrote only the subclass/subproperty closure, so `?entailment=owl2-ql`
/// answered nothing about individuals), reports the axioms outside the
/// profile it did not use, and `?entailment=owl2-ql` rewrites blank nodes
/// existentially.
#[cfg(feature = "owl2-ql")]
#[tokio::test]
async fn owl2_ql_materialises_ground_atoms_and_rewrites_blank_nodes() {
    use open_triplestore::auth::models::{OwnerType, Visibility};
    const DATA: &str = "http://example.org/g/ql";
    let (state, token) = admin_state();
    // `/sparql` reads registered graphs (plus the entailment graph).
    state
        .auth_db
        .create_dataset(
            "qlds",
            "QL",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("qlds", DATA).unwrap();
    state
        .store
        .load_str(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
             @prefix ex: <http://example.org/> . \
             ex:Parent rdfs:subClassOf ex:Person , \
                 [ owl:onProperty ex:hasChild ; owl:someValuesFrom ex:Person ] . \
             ex:ancestorOf a owl:TransitiveProperty . \
             ex:ann a ex:Parent .",
            oxigraph::io::RdfFormat::Turtle,
            Some(DATA),
        )
        .unwrap();
    let resp = test_app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/reasoning/materialize")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(
                    json!({ "regime": "owl2-ql", "source_graphs": [DATA] }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let st = resp.status();
    let body = body_json(resp.into_body()).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["ignored_axioms"], 1, "{body}");
    assert_eq!(
        body["ignored_sample"][0]["axiom"], "owl:TransitiveProperty",
        "{body}"
    );
    assert!(matches!(
        state.store.query(
            "ASK { GRAPH <urn:entailment:owl2-ql> \
               { <http://example.org/ann> a <http://example.org/Person> } }"
        ),
        Ok(QueryResults::Boolean(true))
    ));

    let ask = |q: &str, regime: Option<&str>| {
        let app = test_app(state.clone());
        let token = token.clone();
        let q = url_encode(q);
        let uri = match regime {
            Some(r) => format!("/sparql?query={q}&entailment={r}"),
            None => format!("/sparql?query={q}"),
        };
        async move {
            let resp = app
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(uri)
                        .header(header::ACCEPT, "application/sparql-results+json")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let v = body_json(resp.into_body()).await;
            v["boolean"]
                .as_bool()
                .unwrap_or_else(|| panic!("not an ASK result: {v}"))
        }
    };
    let child = "ASK { <http://example.org/ann> <http://example.org/hasChild> \
                 [ a <http://example.org/Person> ] }";
    assert!(ask(child, Some("owl2-ql")).await);
    assert!(!ask(child, None).await, "no entailment, no anonymous child");
    assert!(
        !ask(
            "ASK { <http://example.org/ann> <http://example.org/hasChild> ?c }",
            Some("owl2-ql")
        )
        .await,
        "a variable binds only to named individuals"
    );
    assert!(
        ask(
            "ASK { <http://example.org/ann> a <http://example.org/Person> }",
            Some("owl2-ql")
        )
        .await
    );
}

/// An inconsistent ontology fails the QL run instead of reporting success.
#[cfg(feature = "owl2-ql")]
#[tokio::test]
async fn owl2_ql_inconsistency_fails_the_run() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
             @prefix ex: <http://example.org/> . \
             ex:Cat owl:disjointWith ex:Dog . \
             ex:tom a ex:Cat , ex:Dog .",
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let (st, body) = materialize(&state, &token, "owl2-ql").await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["consistent"], json!(false), "{body}");
    assert!(
        body["rule"].as_str().is_some_and(|r| !r.is_empty()),
        "{body}"
    );
}

/// An OWL 2 EL run reports the axioms outside the profile it left out, and
/// a report with none leaves the field out.
#[cfg(feature = "owl2-el")]
#[tokio::test]
async fn owl2_el_reports_ignored_axioms() {
    let (state, token) = admin_state();
    state
        .store
        .load_str(
            "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
             @prefix owl: <http://www.w3.org/2002/07/owl#> . \
             @prefix ex: <http://example.org/> . \
             ex:A rdfs:subClassOf [ owl:unionOf ( ex:B ex:C ) ] . \
             ex:A rdfs:subClassOf ex:D . ex:x a ex:A .",
            oxigraph::io::RdfFormat::Turtle,
            None,
        )
        .unwrap();
    let (st, body) = materialize(&state, &token, "owl2-el").await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["ignored"][0]["construct"], "ObjectUnionOf", "{body}");
    assert_eq!(body["ignored"][0]["count"], 1, "{body}");
    let (_, body) = materialize(&state, &token, "rdfs").await;
    assert!(body.get("ignored").is_none(), "{body}");
}

// ── OWL 2 DL backends over HTTP (card O6) ─────────────────────────────────────
//
// `owl2-dl` needs a configured backend (503 without one, D5); input outside
// OWL 2 DL is a 422 listing the violations (D6); `POST /api/reasoning/check`
// answers 200 with true / false / unknown, an inconsistent input carrying the
// materialisation 422's fields; a timeout is a 504 whose result is unknown;
// `?async=true` turns either call into a job.
#[cfg(feature = "owl2-dl")]
mod owl2_dl_backend {
    use super::*;
    use axum::Router;
    use open_triplestore::auth::models::SystemRole;
    use open_triplestore::reasoning::dl_config::{DlBackendKind, DlConfig};
    use oxigraph::io::RdfFormat;
    use std::sync::Arc;
    use std::time::Duration;

    const DL_TG: &str = "urn:entailment:owl2-dl";
    const INCONSISTENT: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> . \
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
         @prefix ex: <http://example.org/> . \
         ex:Cat rdfs:subClassOf ex:Animal . ex:Animal owl:disjointWith ex:Mineral . \
         ex:felix a ex:Cat , ex:Mineral .";
    const PLAIN: &str = "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> . \
         @prefix ex: <http://example.org/> . ex:Cat rdfs:subClassOf ex:Animal . ex:felix a ex:Cat .";

    fn with_backend(state: &mut AppState, cfg: DlConfig) {
        state.dl = Arc::new(cfg);
    }

    fn native_state() -> (AppState, String) {
        let (mut state, token) = admin_state();
        with_backend(
            &mut state,
            DlConfig::default().with_backend(DlBackendKind::Native),
        );
        (state, token)
    }

    async fn call(
        app: &Router,
        method: Method,
        uri: &str,
        token: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value, axum::http::HeaderMap) {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        let body = match body {
            Some(v) => {
                b = b.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let st = resp.status();
        let headers = resp.headers().clone();
        (st, body_json(resp.into_body()).await, headers)
    }

    #[tokio::test]
    async fn owl2_dl_without_a_backend_is_503() {
        let (state, token) = admin_state();
        state
            .store
            .load_str(PLAIN, RdfFormat::Turtle, None)
            .unwrap();
        let (st, body) = materialize(&state, &token, "owl2-dl").await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|e| e.contains("OTS_DL_BACKEND")),
            "{body}"
        );
    }

    #[tokio::test]
    async fn owl2_dl_native_reports_its_backend_and_incompleteness() {
        let (state, token) = native_state();
        state
            .store
            .load_str(PLAIN, RdfFormat::Turtle, None)
            .unwrap();
        let (st, body) = materialize(&state, &token, "owl2-dl").await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["backend"], "native", "{body}");
        assert_eq!(body["complete"], json!(false), "{body}");
        assert_eq!(body["consistent"], json!(true), "{body}");
        assert!(
            body["warnings"].as_array().is_some_and(|w| !w.is_empty()),
            "{body}"
        );
    }

    #[tokio::test]
    async fn owl2_dl_inconsistency_is_the_same_422_as_rl() {
        let (state, token) = native_state();
        state
            .store
            .load_str(INCONSISTENT, RdfFormat::Turtle, None)
            .unwrap();
        let (st, body) = materialize(&state, &token, "owl2-dl").await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["consistent"], json!(false), "{body}");
        assert_eq!(body["rule"], "cax-dw", "{body}");
        assert_eq!(body["regime"], "owl2-dl", "{body}");
        assert_eq!(body["target_graph"], DL_TG, "{body}");
    }

    #[tokio::test]
    async fn owl2_dl_input_outside_the_profile_is_422_with_violations() {
        let (state, token) = native_state();
        state
            .store
            .load_str(
                "@prefix ex: <http://example.org/> . ex:a ex:code ex:b . ex:c ex:code \"42\" .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();
        let (st, body) = materialize(&state, &token, "owl2-dl").await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_eq!(body["in_profile"], json!(false), "{body}");
        assert!(
            body["violations"]
                .as_array()
                .is_some_and(|v| v.iter().any(|x| x["rule"] == "property-punning")),
            "{body}"
        );
    }

    // ── POST /api/reasoning/check ─────────────────────────────────────────────

    #[tokio::test]
    async fn check_inconsistent_premise_is_200_false_with_the_rule() {
        let (state, token) = native_state();
        let app = test_app(state);
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/check",
            &token,
            Some(json!({ "task": "consistency", "premise": INCONSISTENT })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["result"], "false", "{v}");
        assert_eq!(v["consistent"], json!(false), "{v}");
        assert_eq!(v["rule"], "cax-dw", "{v}");
        assert_eq!(v["regime"], "owl2-dl", "{v}");
        assert_eq!(v["backend"], "native", "{v}");
        assert_eq!(v["complete"], json!(false), "{v}");
        assert!(v["detail"].as_str().is_some_and(|d| !d.is_empty()), "{v}");
    }

    #[tokio::test]
    async fn check_consistency_the_native_rules_cannot_prove_is_unknown() {
        let (state, token) = native_state();
        state
            .store
            .load_str(PLAIN, RdfFormat::Turtle, None)
            .unwrap();
        let app = test_app(state);
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/check",
            &token,
            Some(json!({ "task": "consistency" })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["result"], "unknown", "{v}");
        assert!(v["consistent"].is_null(), "{v}");
    }

    #[tokio::test]
    async fn check_entailment_and_its_missing_conclusion() {
        let (state, token) = native_state();
        let app = test_app(state);
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/check",
            &token,
            Some(json!({
                "task": "entailment",
                "premise": PLAIN,
                "conclusion": "<http://example.org/felix> a <http://example.org/Animal> .",
            })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["result"], "true", "{v}");
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/check",
            &token,
            Some(json!({ "task": "entailment", "premise": PLAIN })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    }

    #[tokio::test]
    async fn check_profile_needs_no_backend_and_other_tasks_do() {
        let (state, token) = admin_state();
        let app = test_app(state);
        let premise =
            "@prefix ex: <http://example.org/> . ex:a ex:code ex:b . ex:c ex:code \"42\" .";
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/check",
            &token,
            Some(json!({ "task": "profile", "premise": premise })),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["result"], "false", "{v}");
        assert_eq!(v["in_profile"], json!(false), "{v}");
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/check",
            &token,
            Some(json!({ "task": "consistency", "premise": PLAIN })),
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    }

    #[tokio::test]
    async fn check_premise_and_dataset_are_exclusive() {
        let (state, token) = native_state();
        let app = test_app(state);
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/check",
            &token,
            Some(json!({ "task": "consistency", "premise": PLAIN, "source_graphs": ["urn:g"] })),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    }

    /// A sidecar that answers after `delay`.
    fn slow_sidecar(delay: Duration) -> String {
        use axum::routing::post;
        let app = Router::new()
            .route(
                "/v1/check",
                post(move || async move {
                    tokio::time::sleep(delay).await;
                    axum::Json(json!({ "result": "true" }))
                }),
            )
            .route(
                "/v1/reason",
                post(move || async move {
                    tokio::time::sleep(delay).await;
                    axum::Json(json!({ "consistent": true, "inferred": "" }))
                }),
            );
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                tx.send(listener.local_addr().unwrap()).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        format!("http://{}", rx.recv().unwrap())
    }

    #[tokio::test]
    async fn check_timeout_is_504_with_an_unknown_result() {
        let (mut state, token) = admin_state();
        let mut cfg = DlConfig::default().with_backend(DlBackendKind::Sidecar);
        cfg.sidecar_url = Some(slow_sidecar(Duration::from_secs(4)));
        cfg.timeout = Duration::from_secs(1);
        with_backend(&mut state, cfg);
        let app = test_app(state);
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/check",
            &token,
            Some(json!({ "task": "consistency", "premise": PLAIN })),
        )
        .await;
        assert_eq!(st, StatusCode::GATEWAY_TIMEOUT, "{v}");
        assert_eq!(v["result"], "unknown", "{v}");
        assert_eq!(v["backend"], "sidecar", "{v}");
    }

    // ── ?async=true and GET /api/reasoning/jobs/{id} ─────────────────────────

    async fn wait_for_job(app: &Router, token: &str, location: &str) -> Value {
        for _ in 0..200 {
            let (st, v, _) = call(app, Method::GET, location, token, None).await;
            assert_eq!(st, StatusCode::OK, "{v}");
            if v["status"] == "succeeded" || v["status"] == "failed" {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("job {location} did not finish");
    }

    #[tokio::test]
    async fn async_materialize_answers_202_and_the_job_holds_the_report() {
        let (state, token) = native_state();
        state
            .store
            .load_str(PLAIN, RdfFormat::Turtle, None)
            .unwrap();
        let app = test_app(state.clone());
        let (st, v, headers) = call(
            &app,
            Method::POST,
            "/api/reasoning/materialize?async=true",
            &token,
            Some(json!({ "regime": "owl2-dl" })),
        )
        .await;
        assert_eq!(st, StatusCode::ACCEPTED, "{v}");
        let location = v["location"].as_str().unwrap().to_string();
        assert_eq!(
            headers.get(header::LOCATION).and_then(|h| h.to_str().ok()),
            Some(location.as_str())
        );
        let job = wait_for_job(&app, &token, &location).await;
        assert_eq!(job["status"], "succeeded", "{job}");
        assert_eq!(job["http_status"], 200, "{job}");
        assert_eq!(job["result"]["backend"], "native", "{job}");
        assert!(
            matches!(
                state.store.query(&format!(
                    "ASK {{ GRAPH <{DL_TG}> {{ <http://example.org/felix> a <http://example.org/Animal> }} }}"
                )),
                Ok(QueryResults::Boolean(true))
            ),
            "the background run materialised"
        );
    }

    #[tokio::test]
    async fn async_inconsistent_run_records_the_422_body() {
        let (state, token) = native_state();
        state
            .store
            .load_str(INCONSISTENT, RdfFormat::Turtle, None)
            .unwrap();
        let app = test_app(state);
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/materialize?async=true",
            &token,
            Some(json!({ "regime": "owl2-dl" })),
        )
        .await;
        assert_eq!(st, StatusCode::ACCEPTED, "{v}");
        let job = wait_for_job(&app, &token, v["location"].as_str().unwrap()).await;
        assert_eq!(job["status"], "failed", "{job}");
        assert_eq!(job["http_status"], 422, "{job}");
        assert_eq!(job["result"]["rule"], "cax-dw", "{job}");
        assert_eq!(job["result"]["consistent"], json!(false), "{job}");
    }

    #[tokio::test]
    async fn a_job_is_visible_only_to_whoever_started_it() {
        let (state, token) = native_state();
        state
            .auth_db
            .create_user("other", "other", "other@test.com", "hash", SystemRole::User)
            .unwrap();
        let other = mint_token("other", "other", "user");
        let app = test_app(state);
        let (st, v, _) = call(
            &app,
            Method::POST,
            "/api/reasoning/check?async=true",
            &token,
            Some(json!({ "task": "profile", "premise": PLAIN })),
        )
        .await;
        assert_eq!(st, StatusCode::ACCEPTED, "{v}");
        let location = v["location"].as_str().unwrap().to_string();
        let job = wait_for_job(&app, &token, &location).await;
        assert_eq!(job["result"]["result"], "true", "{job}");
        let (st, _, _) = call(&app, Method::GET, &location, &other, None).await;
        assert_eq!(st, StatusCode::NOT_FOUND);
    }
}
