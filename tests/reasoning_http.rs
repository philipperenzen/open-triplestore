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
