//! LDES / TREE spec rules over the published stream, one assertion per rule,
//! each naming the clause it encodes. There is no LDES or TREE test corpus to
//! vendor — the specs ship prose and a vocabulary — so these are derived
//! rules written from the LDES 1.0.0 specification, its Server Primer §6
//! ("Validating the pages") and the TREE specification, not an external
//! oracle. tests/ldes_http.rs covers behaviour; this file covers conformance.

mod common;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use common::*;
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::server::AppState;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::json;

use tower::ServiceExt as _;

const G: &str = "https://example.org/ldes-conf/instances";
const EX: &str = "https://example.org/ldes-conf/";
const LDES: &str = "https://w3id.org/ldes#";
const TREE: &str = "https://w3id.org/tree#";

async fn req(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    body: Option<(&str, String)>,
) -> (StatusCode, HeaderMap, String) {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::ACCEPT, "text/turtle");
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let body = match body {
        Some((ct, s)) => {
            b = b.header(header::CONTENT_TYPE, ct);
            Body::from(s)
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let st = resp.status();
    let hdrs = resp.headers().clone();
    (st, hdrs, body_text(resp.into_body()).await)
}

fn setup(state: &AppState, dataset: &str) {
    state
        .auth_db
        .create_dataset(
            dataset,
            "LDES conformance",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph(dataset, G).unwrap();
    state
        .store
        .load_str(
            &format!("<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"one\" . <{EX}b2> a <{EX}Bridge> ; <{EX}name> \"two\" ."),
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
}

/// A fetched document as a store (default graph), for SPARQL over it.
fn doc(turtle: &str, base: &str) -> TripleStore {
    let tmp = TripleStore::in_memory().unwrap();
    let parser = oxigraph::io::RdfParser::from_format(RdfFormat::Turtle)
        .with_base_iri(base)
        .unwrap();
    tmp.store()
        .load_from_reader(parser, turtle.as_bytes())
        .expect("valid Turtle");
    tmp
}

fn count(tmp: &TripleStore, pattern: &str) -> usize {
    match tmp.query(&format!("SELECT * WHERE {{ {pattern} }}")) {
        Ok(QueryResults::Solutions(s)) => s.count(),
        Ok(_) => panic!("{pattern}: not a SELECT result"),
        Err(e) => panic!("{pattern}: {e}"),
    }
}

fn ask(tmp: &TripleStore, pattern: &str) -> bool {
    matches!(
        tmp.query(&format!("ASK {{ {pattern} }}")),
        Ok(QueryResults::Boolean(true))
    )
}

/// Enable a stream with tiny pages and write enough to get three fragments.
async fn three_pages(app: &Router, token: &str, ds: &str) {
    let (st, _, txt) = req(
        app,
        Method::PUT,
        &format!("/api/datasets/{ds}/ldes"),
        Some(token),
        Some((
            "application/json",
            json!({ "enabled": true, "page_size": 2 }).to_string(),
        )),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    for (i, name) in ["three", "four", "five"].iter().enumerate() {
        let (st, _, _) = req(
            app,
            Method::POST,
            &format!("/store?graph={}", url_encode(G)),
            Some(token),
            Some((
                "text/turtle",
                format!("<{EX}b{}> a <{EX}Bridge> ; <{EX}name> \"{name}\" .", i + 3),
            )),
        )
        .await;
        assert!(st.is_success(), "{st}");
    }
}

/// Server Primer §6.1: "A root node MUST link the event stream to the view
/// using the tree:view property"; ldes:timestampPath and ldes:versionOfPath
/// "have a cardinality of 0 or 1". TREE §8.2.8 / LDES §4.2: tree:shape 0..1.
#[tokio::test]
async fn the_root_node_links_the_view_and_declares_each_path_at_most_once() {
    let (state, token) = admin_state();
    setup(&state, "root");
    let app = test_app(state.clone());
    three_pages(&app, &token, "root").await;
    let base = format!("{}/api/datasets/root/ldes", state.base_url);
    let (st, _, ttl) = req(&app, Method::GET, "/api/datasets/root/ldes", None, None).await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    let d = doc(&ttl, &base);
    assert_eq!(
        count(&d, &format!("?s a <{LDES}EventStream> ; <{TREE}view> ?v")),
        1,
        "§6.1 exactly one tree:view: {ttl}"
    );
    assert!(
        ask(&d, &format!("?s <{TREE}view> ?v . FILTER(isIRI(?v))")),
        "§6.1 the view is an IRI: {ttl}"
    );
    for p in [
        "timestampPath",
        "versionOfPath",
        "sequencePath",
        "versionTimestampPath",
        "retentionPolicy",
    ] {
        assert!(
            count(&d, &format!("?s <{LDES}{p}> ?o")) <= 1,
            "§6.1 ldes:{p} has cardinality 0..1: {ttl}"
        );
    }
    assert!(
        count(&d, &format!("?s <{TREE}shape> ?o")) <= 1,
        "§6.1 tree:shape has cardinality 0..1: {ttl}"
    );
}

/// Server Primer §6.2: "On the event stream, 0 or more tree:member triples
/// are provided. The objects MUST be IRIs." §6.3: "On all relations, exactly
/// one tree:node MUST be present. The object MUST be an IRI"; a
/// GreaterThanOrEqualToRelation "MUST specify exactly one tree:path … and
/// tree:value".
#[tokio::test]
async fn members_are_iris_and_every_relation_has_one_node_path_and_value() {
    let (state, token) = admin_state();
    setup(&state, "rel");
    let app = test_app(state.clone());
    three_pages(&app, &token, "rel").await;
    for n in 1..=3 {
        let uri = format!("/api/datasets/rel/ldes/nodes/{n}");
        let (st, _, ttl) = req(&app, Method::GET, &uri, None, None).await;
        assert_eq!(st, StatusCode::OK, "{ttl}");
        let d = doc(&ttl, &format!("{}{uri}", state.base_url));
        assert!(
            !ask(&d, &format!("?s <{TREE}member> ?m . FILTER(!isIRI(?m))")),
            "§6.2 node {n}: every tree:member object is an IRI: {ttl}"
        );
        let relations = count(&d, &format!("?node <{TREE}relation> ?r"));
        if n < 3 {
            assert_eq!(relations, 1, "node {n} links to the next: {ttl}");
        } else {
            assert_eq!(relations, 0, "the last node links nowhere: {ttl}");
        }
        assert_eq!(
            count(
                &d,
                &format!(
                    "?node <{TREE}relation> ?r . ?r <{TREE}node> ?next . FILTER(isIRI(?next))"
                )
            ),
            relations,
            "§6.3 node {n}: exactly one tree:node, an IRI, per relation: {ttl}"
        );
        assert_eq!(
            count(
                &d,
                &format!(
                    "?node <{TREE}relation> ?r . ?r a <{TREE}GreaterThanOrEqualToRelation> ; <{TREE}path> ?p ; <{TREE}value> ?v . FILTER(datatype(?v) = <http://www.w3.org/2001/XMLSchema#dateTime>)"
                )
            ),
            relations,
            "§6.3 node {n}: exactly one tree:path and one xsd:dateTime tree:value per relation: {ttl}"
        );
    }
}

/// Server Primer §2 (content negotiation): a document served by `Accept`
/// carries `Vary: Accept`, so a cache keyed on the URL does not hand a
/// JSON-LD client the Turtle body.
#[tokio::test]
async fn negotiated_documents_vary_on_accept() {
    let (state, token) = admin_state();
    setup(&state, "vary");
    let app = test_app(state.clone());
    three_pages(&app, &token, "vary").await;
    for uri in ["/api/datasets/vary/ldes", "/api/datasets/vary/ldes/nodes/1"] {
        let (st, hdrs, ttl) = req(&app, Method::GET, uri, None, None).await;
        assert_eq!(st, StatusCode::OK, "{ttl}");
        let vary = hdrs
            .get(header::VARY)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            vary.to_ascii_lowercase().contains("accept"),
            "§2 {uri}: Vary names Accept, got {vary:?}"
        );
    }
}

/// LDES §3.2: a client checks "whether the triple <> ldes:immutable true is
/// set; then whether the Cache-Control HTTP response header is set to
/// immutable" — a full page carries both, the last page neither. Primer §6.2:
/// ldes:immutable "SHOULD NOT be made explicit using a false value".
#[tokio::test]
async fn full_pages_declare_ldes_immutable_and_the_last_page_does_not() {
    let (state, token) = admin_state();
    setup(&state, "imm");
    let app = test_app(state.clone());
    three_pages(&app, &token, "imm").await;
    for n in 1..=3 {
        let uri = format!("/api/datasets/imm/ldes/nodes/{n}");
        let (st, hdrs, ttl) = req(&app, Method::GET, &uri, None, None).await;
        assert_eq!(st, StatusCode::OK, "{ttl}");
        let node = format!("{}{uri}", state.base_url);
        let d = doc(&ttl, &node);
        let header_immutable = hdrs
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .contains("immutable");
        let triple_immutable = ask(&d, &format!("<{node}> <{LDES}immutable> true"));
        assert_eq!(
            header_immutable,
            n < 3,
            "node {n}: Cache-Control immutable iff the page is full"
        );
        assert_eq!(
            triple_immutable,
            n < 3,
            "§3.2 node {n}: <> ldes:immutable true iff the page is full: {ttl}"
        );
        assert!(
            !ask(&d, &format!("?s <{LDES}immutable> false")),
            "§6.2 node {n}: ldes:immutable false is never made explicit: {ttl}"
        );
    }
}

// ─── Retention (LDES 1.0 §4.4, Server Primer §5.1 and §6.1.1) ───────────────

async fn put_stream(
    app: &Router,
    token: &str,
    ds: &str,
    body: serde_json::Value,
) -> (StatusCode, String) {
    let (st, _, txt) = req(
        app,
        Method::PUT,
        &format!("/api/datasets/{ds}/ldes"),
        Some(token),
        Some(("application/json", body.to_string())),
    )
    .await;
    (st, txt)
}

async fn post_entity(app: &Router, token: &str, local: &str, name: &str) {
    let (st, _, txt) = req(
        app,
        Method::POST,
        &format!("/store?graph={}", url_encode(G)),
        Some(token),
        Some((
            "text/turtle",
            format!("<{EX}{local}> a <{EX}Bridge> ; <{EX}name> \"{name}\" ."),
        )),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
}

async fn put_entity(app: &Router, token: &str, turtle: String) {
    let (st, _, txt) = req(
        app,
        Method::PUT,
        &format!("/store?graph={}", url_encode(G)),
        Some(token),
        Some(("text/turtle", turtle)),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
}

/// `(member id, entity)` pairs of a fragment, in member order.
fn member_ids(d: &TripleStore) -> Vec<(i64, String)> {
    let q = format!(
        "SELECT ?m ?e WHERE {{ ?c <{TREE}member> ?m . ?m <http://purl.org/dc/terms/isVersionOf> ?e }}"
    );
    let mut rows: Vec<(i64, String)> = match d.query(&q) {
        Ok(QueryResults::Solutions(s)) => s
            .flatten()
            .map(|r| {
                let m = r.get("m").unwrap().to_string();
                let id: i64 = m
                    .trim_end_matches('>')
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .parse()
                    .unwrap();
                (
                    id,
                    r.get("e")
                        .unwrap()
                        .to_string()
                        .trim_matches(['<', '>'])
                        .to_string(),
                )
            })
            .collect(),
        _ => vec![],
    };
    rows.sort();
    rows
}

fn relation(d: &TripleStore) -> Option<(String, String)> {
    let q = format!(
        "SELECT ?next ?v WHERE {{ ?n <{TREE}relation> ?r . ?r <{TREE}node> ?next ; <{TREE}value> ?v }}"
    );
    match d.query(&q) {
        Ok(QueryResults::Solutions(mut s)) => s.next().and_then(|r| r.ok()).map(|r| {
            (
                r.get("next")
                    .unwrap()
                    .to_string()
                    .trim_matches(['<', '>'])
                    .to_string(),
                r.get("v")
                    .map(|v| match v {
                        oxigraph::model::Term::Literal(l) => l.value().to_string(),
                        other => other.to_string(),
                    })
                    .unwrap(),
            )
        }),
        _ => None,
    }
}

/// A member's `dct:created`, read through SPARQL like the relation's bound
/// is, so both come back in the evaluator's canonical `xsd:dateTime` form.
fn created_of(d: &TripleStore, member_id: i64) -> String {
    let q = format!(
        "SELECT ?t WHERE {{ ?m <http://purl.org/dc/terms/created> ?t . FILTER(STRENDS(STR(?m), \"/members/{member_id}\")) }}"
    );
    match d.query(&q) {
        Ok(QueryResults::Solutions(mut s)) => s
            .next()
            .and_then(|r| r.ok())
            .map(|r| match r.get("t") {
                Some(oxigraph::model::Term::Literal(l)) => l.value().to_string(),
                other => format!("{other:?}"),
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn view_of(d: &TripleStore) -> String {
    match d.query(&format!("SELECT ?v WHERE {{ ?s <{TREE}view> ?v }}")) {
        Ok(QueryResults::Solutions(mut s)) => s
            .next()
            .and_then(|r| r.ok())
            .map(|r| {
                r.get("v")
                    .unwrap()
                    .to_string()
                    .trim_matches(['<', '>'])
                    .to_string()
            })
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// LDES §4.4: "A retention policy will be described on the root node";
/// Server Primer §6.1.1: at most one `ldes:retentionPolicy`, "The value of
/// ldes:retentionPolicy MUST be an IRI referring to a retention policy
/// description", each property at most once with its datatype. And §4.4's
/// consequence for where the description lives: a policy IRI "without
/// further statements in the current page" means "this view keeps no
/// members at all" — so every page that names it describes it.
#[tokio::test]
async fn a_retention_policy_is_an_iri_on_the_root_node_described_on_every_page() {
    let (state, token) = admin_state();
    setup(&state, "pol");
    let app = test_app(state.clone());
    let (st, txt) = put_stream(
        &app,
        &token,
        "pol",
        json!({ "enabled": true, "page_size": 2, "retention": {
            "full_log_duration": "P30D", "version_amount": 2,
            "version_delete_duration": "P7D", "starting_from": "2026-01-01T00:00:00Z" } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let v: serde_json::Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(v["retention"]["full_log_duration"], "P30D", "{txt}");
    assert_eq!(v["retention"]["version_amount"], 2, "{txt}");
    post_entity(&app, &token, "b3", "three").await;

    let stream = format!("{}/api/datasets/pol/ldes", state.base_url);
    for uri in [
        "/api/datasets/pol/ldes",
        "/api/datasets/pol/ldes/nodes/1",
        "/api/datasets/pol/ldes/nodes/2",
    ] {
        let (st, _, ttl) = req(&app, Method::GET, uri, None, None).await;
        assert_eq!(st, StatusCode::OK, "{ttl}");
        let d = doc(&ttl, &format!("{}{uri}", state.base_url));
        let view = view_of(&d);
        assert_eq!(
            count(&d, &format!("<{view}> <{LDES}retentionPolicy> ?p")),
            1,
            "§6.1.1 {uri}: exactly one ldes:retentionPolicy, on the root node: {ttl}"
        );
        assert!(
            ask(
                &d,
                &format!("<{view}> <{LDES}retentionPolicy> ?p . FILTER(isIRI(?p))")
            ),
            "§6.1.1 {uri}: the policy is an IRI: {ttl}"
        );
        assert!(
            ask(&d, &format!("<{view}> a <{LDES}EventSource>")),
            "§4.4 {uri}: the root node is an ldes:EventSource: {ttl}"
        );
        assert!(
            ask(
                &d,
                &format!("<{stream}#retention> a <{LDES}RetentionPolicy>")
            ),
            "§4.4 {uri}: the policy is described in this page: {ttl}"
        );
        for (p, lexical, dt) in [
            ("fullLogDuration", "P30D", "duration"),
            ("versionAmount", "2", "integer"),
            ("versionDeleteDuration", "P7D", "duration"),
            ("startingFrom", "2026-01-01T00:00:00Z", "dateTime"),
        ] {
            assert_eq!(
                count(
                    &d,
                    &format!(
                        "?pol <{LDES}{p}> ?v . FILTER(str(?v) = \"{lexical}\" && datatype(?v) = <http://www.w3.org/2001/XMLSchema#{dt}>)"
                    )
                ),
                1,
                "§6.1.1 {uri}: exactly one ldes:{p}, typed xsd:{dt}: {ttl}"
            );
        }
        assert_eq!(
            count(&d, &format!("?pol <{LDES}versionDuration> ?v")),
            0,
            "an undeclared property is absent: {ttl}"
        );
    }

    // A malformed policy is refused before anything is stored.
    for bad in [
        json!({ "full_log_duration": "30 days" }),
        json!({ "version_amount": 0 }),
        json!({ "starting_from": "yesterday" }),
        json!({ "version_duration": "P1D" }),
    ] {
        let (st, txt) = put_stream(
            &app,
            &token,
            "pol",
            json!({ "enabled": true, "retention": bad }),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    }
    // An empty policy clears it: the stream keeps every member again.
    let (st, txt) = put_stream(
        &app,
        &token,
        "pol",
        json!({ "enabled": true, "retention": {} }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let (_, _, ttl) = req(&app, Method::GET, "/api/datasets/pol/ldes", None, None).await;
    let d = doc(&ttl, &stream);
    assert_eq!(
        count(&d, &format!("?s <{LDES}retentionPolicy> ?p")),
        0,
        "no policy is declared once cleared: {ttl}"
    );
}

/// The invariant that makes retention safe under `Cache-Control: immutable`
/// (Server Primer §5.1: "Do not modify the content of immutable pages;
/// instead, stop linking to them, redirect, or make them 410 Gone"; "update
/// the search tree so that no relations point to removed nodes"): once a
/// page is full, its member→node assignment is frozen. Pruning only ever
/// removes members from a sealed page — never moves one onto or off it, and
/// never changes the bound its relation carries — and a page whose members
/// are all gone answers 410, with `tree:view` and every relation pointing
/// past it.
#[tokio::test]
async fn pruning_only_shrinks_sealed_pages_and_empties_them_to_410() {
    std::env::set_var("OTS_LDES_SWEEP_INTERVAL_SECS", "0");
    let (state, token) = admin_state();
    setup(&state, "prune");
    let app = test_app(state.clone());
    let base = state.base_url.to_string();
    let node = |n: u64| format!("{base}/api/datasets/prune/ldes/nodes/{n}");
    let get = |n: u64| {
        let app = app.clone();
        async move {
            let uri = format!("/api/datasets/prune/ldes/nodes/{n}");
            req(&app, Method::GET, &uri, None, None).await
        }
    };

    // Keep only the newest version of each entity. The two seeded members
    // (b1 = 1, b2 = 2) fill page 1, which stays the mutable tail for now.
    let (st, txt) = put_stream(
        &app,
        &token,
        "prune",
        json!({ "enabled": true, "page_size": 2, "retention": { "version_amount": 1 } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let (st, hdrs, n1) = get(1).await;
    assert_eq!(st, StatusCode::OK, "{n1}");
    assert!(hdrs
        .get(header::CACHE_CONTROL)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("no-cache"));
    let d = doc(&n1, &node(1));
    assert_eq!(
        member_ids(&d),
        vec![(1, format!("{EX}b1")), (2, format!("{EX}b2"))],
        "{n1}"
    );

    // A third member (b1 again, id 3) seals page 1 as ids 1..=2 and makes
    // b1's first version prunable: page 1 keeps member 2 only, its relation
    // to page 2 carries the bound recorded at sealing — member 3's timestamp.
    put_entity(
        &app,
        &token,
        format!("<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"uno\" . <{EX}b2> a <{EX}Bridge> ; <{EX}name> \"two\" ."),
    )
    .await;
    let (st, hdrs, n1) = get(1).await;
    assert_eq!(st, StatusCode::OK, "{n1}");
    assert!(
        hdrs.get(header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("immutable"),
        "page 1 is sealed: {n1}"
    );
    let d1 = doc(&n1, &node(1));
    assert_eq!(
        member_ids(&d1),
        vec![(2, format!("{EX}b2"))],
        "the old b1 is pruned, b2 stays on the page it was on: {n1}"
    );
    let (next, bound) = relation(&d1).expect("a sealed page links onward");
    assert_eq!(next, node(2), "{n1}");
    let (st, _, n2) = get(2).await;
    assert_eq!(st, StatusCode::OK, "{n2}");
    let d2 = doc(&n2, &node(2));
    assert_eq!(member_ids(&d2), vec![(3, format!("{EX}b1"))], "{n2}");
    assert_eq!(
        bound,
        created_of(&d2, 3),
        "the relation's bound is member 3's dct:created: {n2}"
    );

    // b2 again (id 4): member 2 is pruned, page 1 is empty → 410 Gone, and
    // the stream now starts at page 2.
    put_entity(
        &app,
        &token,
        format!("<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"uno\" . <{EX}b2> a <{EX}Bridge> ; <{EX}name> \"dos\" ."),
    )
    .await;
    let (st, _, gone) = get(1).await;
    assert_eq!(
        st,
        StatusCode::GONE,
        "§5.1 an emptied sealed node is 410: {gone}"
    );
    assert!(
        gone.contains("node 2"),
        "the body says where the stream continues: {gone}"
    );
    let (st, _, s) = req(&app, Method::GET, "/api/datasets/prune/ldes", None, None).await;
    assert_eq!(st, StatusCode::OK, "{s}");
    let ds = doc(&s, &format!("{base}/api/datasets/prune/ldes"));
    assert_eq!(
        view_of(&ds),
        node(2),
        "tree:view skips the compacted node: {s}"
    );
    let (st, hdrs, n2) = get(2).await;
    assert_eq!(st, StatusCode::OK, "{n2}");
    assert!(hdrs
        .get(header::CACHE_CONTROL)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("no-cache"));
    let d2 = doc(&n2, &node(2));
    assert_eq!(
        member_ids(&d2),
        vec![(3, format!("{EX}b1")), (4, format!("{EX}b2"))],
        "{n2}"
    );
    assert_eq!(
        view_of(&d2),
        node(2),
        "every page's tree:view skips it too: {n2}"
    );

    // b3 (id 5) seals page 2 as ids 3..=4; page 3 is the tail. Page 2's
    // relation carries member 5's timestamp; the compacted page 1 is still
    // 410 and nothing links to it.
    post_entity(&app, &token, "b3", "three").await;
    let (st, hdrs, n2) = get(2).await;
    assert_eq!(st, StatusCode::OK, "{n2}");
    assert!(hdrs
        .get(header::CACHE_CONTROL)
        .unwrap()
        .to_str()
        .unwrap()
        .contains("immutable"));
    let d2 = doc(&n2, &node(2));
    assert_eq!(
        member_ids(&d2),
        vec![(3, format!("{EX}b1")), (4, format!("{EX}b2"))],
        "sealing changed nothing on the page: {n2}"
    );
    let (next, bound) = relation(&d2).unwrap();
    assert_eq!(next, node(3), "{n2}");
    let (st, _, n3) = get(3).await;
    assert_eq!(st, StatusCode::OK, "{n3}");
    assert_eq!(bound, created_of(&doc(&n3, &node(3)), 5), "{n3}");
    assert_eq!(
        member_ids(&doc(&n3, &node(3))),
        vec![(5, format!("{EX}b3"))]
    );
    let (st, _, _) = get(1).await;
    assert_eq!(st, StatusCode::GONE);
    let (st, _, _) = get(4).await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "beyond the tail is still 404, not 410"
    );
}

/// The sweep, with an explicit clock: the full-log window keeps everything
/// inside it, version_amount keeps the newest versions outside it,
/// version_delete_duration bounds tombstones, starting_from cuts the past.
#[test]
fn the_sweep_applies_the_declared_windows() {
    use chrono::{Duration, TimeZone, Utc};
    use open_triplestore::ldes::store::{insert_member, prune, RetentionPolicy};
    let (state, _) = admin_state();
    let db = &state.auth_db;
    let now = Utc.with_ymd_and_hms(2026, 9, 15, 12, 0, 0).unwrap();
    let at = |days_ago: i64| (now - Duration::days(days_ago)).to_rfc3339();
    // entity a: versions 40, 20 and 3 days ago; entity b: one version 10 days
    // ago; entity c: a version 50 days ago and a tombstone 30 days ago.
    let ids: Vec<i64> = [
        ("a", 40, false),
        ("a", 20, false),
        ("b", 10, false),
        ("c", 50, false),
        ("c", 30, true),
        ("a", 3, false),
    ]
    .iter()
    .map(|(e, d, del)| {
        insert_member(db, "sweep", &format!("{EX}{e}"), G, &at(*d), *del, "").unwrap()
    })
    .collect();
    let remaining = || -> Vec<i64> {
        let conn = db.pool().get().unwrap();
        let mut st = conn
            .prepare("SELECT id FROM ldes_members WHERE dataset_id = 'sweep' ORDER BY id")
            .unwrap();
        st.query_map([], |r| r.get(0)).unwrap().flatten().collect()
    };

    // Full log for 7 days, one version beyond it, deletions for 35 days.
    let policy = RetentionPolicy {
        full_log_duration: Some("P7D".into()),
        version_amount: Some(1),
        version_delete_duration: Some("P35D".into()),
        ..Default::default()
    };
    let deleted = prune(db, "sweep", &policy, now, Duration::zero()).unwrap();
    // Kept: a@3d (in the window), b@10d (its newest version), c's tombstone
    // @30d (within 35 days). Gone: a@40d, a@20d (older versions), c@50d (a
    // deleted entity's live version is not its newest version).
    assert_eq!(deleted, 3);
    assert_eq!(remaining(), vec![ids[2], ids[4], ids[5]]);

    // Tombstones past their window go too.
    let policy = RetentionPolicy {
        version_delete_duration: Some("P20D".into()),
        version_amount: Some(1),
        ..Default::default()
    };
    assert_eq!(
        prune(db, "sweep", &policy, now, Duration::zero()).unwrap(),
        1
    );
    assert_eq!(remaining(), vec![ids[2], ids[5]]);

    // starting_from cuts everything before it, whatever else would keep it.
    let policy = RetentionPolicy {
        starting_from: Some((now - Duration::days(5)).to_rfc3339()),
        ..Default::default()
    };
    assert_eq!(
        prune(db, "sweep", &policy, now, Duration::zero()).unwrap(),
        1
    );
    assert_eq!(remaining(), vec![ids[5]]);

    // The safety buffer keeps a member the declared window would drop.
    let policy = RetentionPolicy {
        full_log_duration: Some("P2D".into()),
        ..Default::default()
    };
    assert_eq!(
        prune(db, "sweep", &policy, now, Duration::days(2)).unwrap(),
        0
    );
    assert_eq!(
        prune(db, "sweep", &policy, now, Duration::zero()).unwrap(),
        1
    );
    assert!(remaining().is_empty());
    // An empty policy prunes nothing.
    assert_eq!(
        prune(
            db,
            "sweep",
            &RetentionPolicy::default(),
            now,
            Duration::zero()
        )
        .unwrap(),
        0
    );
}

/// A second instance on a local listener publishes with a retention policy;
/// this instance syncs. LDES §3.3: "A client MUST process 410 Gone as a page
/// with an empty set of relations and an empty set of members"; §3.2: the
/// client keeps "the retention policy of the root node" as context.
#[tokio::test]
async fn the_client_treats_a_gone_node_as_empty_and_keeps_the_publisher_policy() {
    std::env::set_var("OTS_LDES_SWEEP_INTERVAL_SECS", "0");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let (mut remote, remote_token) = admin_state();
    remote.base_url = std::sync::Arc::new(origin.clone());
    setup(&remote, "src");
    let remote_app = test_app(remote.clone());
    {
        let app = remote_app.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
    }
    // Two seeded members, then both entities rewritten twice: page 1 (ids
    // 1..=2) is sealed and then emptied by version_amount = 1. (A full-log
    // window would keep everything written today, so none is declared yet.)
    let (st, txt) = put_stream(
        &remote_app,
        &remote_token,
        "src",
        json!({ "enabled": true, "page_size": 2, "retention": { "version_amount": 1 } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    for name in ["uno", "dos"] {
        put_entity(
            &remote_app,
            &remote_token,
            format!("<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"{name}\" . <{EX}b2> a <{EX}Bridge> ; <{EX}name> \"{name}\" ."),
        )
        .await;
    }
    let (st, _, _) = req(
        &remote_app,
        Method::GET,
        "/api/datasets/src/ldes/nodes/1",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::GONE, "the publisher compacted page 1");

    std::env::set_var("OTS_REMOTE_ALLOWLIST", format!("{origin}/"));
    let (local, token) = admin_state();
    local
        .auth_db
        .create_dataset(
            "mirror",
            "Mirror",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    let app = test_app(local.clone());
    let target = "https://example.org/mirror/instances";
    let sync = |url: String| {
        let app = app.clone();
        let token = token.clone();
        async move {
            let (st, _, txt) = req(
                &app,
                Method::POST,
                "/api/ldes/sync",
                Some(&token),
                Some((
                    "application/json",
                    json!({ "url": url, "dataset_id": "mirror", "graph_iri": target }).to_string(),
                )),
            )
            .await;
            (st, txt)
        }
    };

    // Entering at the compacted node: an empty page, not a failed sync.
    let (st, txt) = sync(format!("{origin}/api/datasets/src/ldes/nodes/1")).await;
    assert_eq!(
        st,
        StatusCode::OK,
        "§3.3 a 410 page is empty, not an error: {txt}"
    );
    let r: serde_json::Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["nodes_gone"], 1, "{txt}");
    assert_eq!(r["nodes_visited"], 0, "{txt}");
    assert_eq!(r["entities_updated"], 0, "{txt}");

    // From the root: the surviving members arrive, and the policy is kept.
    let (st, txt) = sync(format!("{origin}/api/datasets/src/ldes")).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: serde_json::Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(r["entities_updated"], 2, "{txt}");
    assert_eq!(
        r["nodes_gone"], 0,
        "tree:view already skips the compacted node: {txt}"
    );
    assert_eq!(r["retention_policy"]["version_amount"], 1, "§3.2 {txt}");
    assert!(
        r["retention_policy"]["full_log_duration"].is_null(),
        "{txt}"
    );
    assert_eq!(r["retention_policy"]["legacy"], false, "{txt}");
    assert!(r["warnings"].as_array().unwrap().is_empty(), "{txt}");
    assert!(
        matches!(
            local.store.query(&format!(
                "ASK {{ GRAPH <{target}> {{ <{EX}b1> <{EX}name> \"dos\" }} }}"
            )),
            Ok(QueryResults::Boolean(true))
        ),
        "the newest version was mirrored"
    );

    // The publisher now declares a full-log window; a bookmark older than it
    // is a hole the caller is told about.
    let (st, txt) = put_stream(
        &remote_app,
        &remote_token,
        "src",
        json!({ "enabled": true, "page_size": 2, "retention": { "version_amount": 1, "full_log_duration": "P1D" } }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    open_triplestore::ldes::store::set_sync_bookmark(
        &local.auth_db,
        "mirror",
        &format!("{origin}/api/datasets/src/ldes"),
        Some("2020-01-01T00:00:00+00:00"),
        0,
    )
    .unwrap();
    // set_sync_bookmark keeps the newer timestamp; force the old one.
    {
        let conn = local.auth_db.pool().get().unwrap();
        conn.execute(
            "UPDATE ldes_sync_state SET last_timestamp = '2020-01-01T00:00:00+00:00' WHERE dataset_id = 'mirror'",
            [],
        )
        .unwrap();
    }
    let (st, txt) = sync(format!("{origin}/api/datasets/src/ldes")).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let r: serde_json::Value = serde_json::from_str(&txt).unwrap();
    let warnings = r["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().contains("retention window")),
        "§4.4 the client warns about a bookmark before the window: {txt}"
    );
}
