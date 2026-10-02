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
        "versionDeletePath",
        "versionDeleteObject",
        "pollingInterval",
        "retentionPolicy",
    ] {
        assert!(
            count(&d, &format!("?s <{LDES}{p}> ?o")) <= 1,
            "§6.1 ldes:{p} has cardinality 0..1: {ttl}"
        );
    }
    assert_eq!(
        count(&d, &format!("?s <{TREE}shape> ?o")),
        1,
        "§6.1 tree:shape has cardinality 0..1 (TREE: \"exactly one\"), and the stream declares it: {ttl}"
    );
}

/// Server Primer §6.2: "On the event stream, 0 or more tree:member triples
/// are provided. The objects MUST be IRIs." §6.3: "On all relations, exactly
/// one tree:node MUST be present. The object MUST be an IRI"; a
/// GreaterThanOrEqualToRelation or LessThanOrEqualToRelation "MUST specify
/// exactly one tree:path … and tree:value". The root (node 0) carries the
/// relations; sealed nodes link nowhere and the last tail page too.
#[tokio::test]
async fn members_are_iris_and_every_relation_has_one_node_path_and_value() {
    let (state, token) = admin_state();
    setup(&state, "rel");
    let app = test_app(state.clone());
    three_pages(&app, &token, "rel").await;
    // Root: two bounds for each of the sealed nodes 1 and 2, one for the tail.
    for (n, expected) in [(0, 5), (1, 0), (2, 0), (3, 0)] {
        let uri = format!("/api/datasets/rel/ldes/nodes/{n}");
        let (st, _, ttl) = req(&app, Method::GET, &uri, None, None).await;
        assert_eq!(st, StatusCode::OK, "{ttl}");
        let d = doc(&ttl, &format!("{}{uri}", state.base_url));
        assert!(
            !ask(&d, &format!("?s <{TREE}member> ?m . FILTER(!isIRI(?m))")),
            "§6.2 node {n}: every tree:member object is an IRI: {ttl}"
        );
        let relations = count(&d, &format!("?node <{TREE}relation> ?r"));
        assert_eq!(relations, expected, "node {n}: {ttl}");
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
                    "?node <{TREE}relation> ?r . ?r a ?kind ; <{TREE}path> ?p ; <{TREE}value> ?v . \
                     FILTER(?kind IN (<{TREE}GreaterThanOrEqualToRelation>, <{TREE}LessThanOrEqualToRelation>) \
                            && datatype(?v) = <http://www.w3.org/2001/XMLSchema#dateTime>)"
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

/// One relation of a document: from, to, the relation class's local name
/// and the `tree:value` lexical form.
#[derive(Debug, Clone, PartialEq)]
struct Rel {
    from: String,
    to: String,
    kind: String,
    value: Option<String>,
}

fn relations(d: &TripleStore) -> Vec<Rel> {
    let q = format!(
        "SELECT ?from ?to ?kind ?v WHERE {{ ?from <{TREE}relation> ?r . ?r <{TREE}node> ?to . \
         OPTIONAL {{ ?r a ?kind }} OPTIONAL {{ ?r <{TREE}value> ?v }} }}"
    );
    let iri = |t: Option<&oxigraph::model::Term>| {
        t.map(|t| t.to_string().trim_matches(['<', '>']).to_string())
            .unwrap_or_default()
    };
    let mut out: Vec<Rel> = match d.query(&q) {
        Ok(QueryResults::Solutions(s)) => s
            .flatten()
            .map(|r| Rel {
                from: iri(r.get("from")),
                to: iri(r.get("to")),
                kind: iri(r.get("kind")).trim_start_matches(TREE).to_string(),
                value: match r.get("v") {
                    Some(oxigraph::model::Term::Literal(l)) => Some(l.value().to_string()),
                    _ => None,
                },
            })
            .collect(),
        _ => vec![],
    };
    out.sort_by(|a, b| (&a.to, &a.kind).cmp(&(&b.to, &b.kind)));
    out
}

fn instant(s: &str) -> chrono::DateTime<chrono::FixedOffset> {
    chrono::DateTime::parse_from_rfc3339(s).unwrap_or_else(|e| panic!("{s}: {e}"))
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
    // b1's first version prunable: page 1 keeps member 2 only and links
    // nowhere; the root bounds it from both sides and links the tail (page
    // 2) from member 3's timestamp on.
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
    assert!(
        relations(&d1).is_empty(),
        "a sealed page links nowhere: {n1}"
    );
    let (st, _, n2) = get(2).await;
    assert_eq!(st, StatusCode::OK, "{n2}");
    let d2 = doc(&n2, &node(2));
    assert_eq!(member_ids(&d2), vec![(3, format!("{EX}b1"))], "{n2}");
    let (st, _, r) = get(0).await;
    assert_eq!(st, StatusCode::OK, "{r}");
    let rels = relations(&doc(&r, &node(0)));
    let kinds = |to: &str| -> Vec<String> {
        rels.iter()
            .filter(|x| x.to == to)
            .map(|x| x.kind.clone())
            .collect()
    };
    assert_eq!(
        kinds(&node(1)),
        vec!["GreaterThanOrEqualToRelation", "LessThanOrEqualToRelation"],
        "{r}"
    );
    let upper = rels
        .iter()
        .find(|x| x.to == node(1) && x.kind == "LessThanOrEqualToRelation")
        .and_then(|x| x.value.clone())
        .unwrap();
    assert!(
        instant(&created_of(&d1, 2)) <= instant(&upper),
        "member 2 is within the upper bound recorded at sealing: {r}"
    );
    let tail = rels.iter().find(|x| x.to == node(2)).unwrap();
    assert_eq!(tail.kind, "GreaterThanOrEqualToRelation", "{r}");
    assert_eq!(
        tail.value.as_deref(),
        Some(created_of(&d2, 3).as_str()),
        "the tail's bound is member 3's dct:created: {r}"
    );

    // b2 again (id 4): member 2 is pruned, page 1 is empty → 410 Gone, and
    // nothing links to it any more.
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
        gone.contains("nodes/0"),
        "the body points at the root node: {gone}"
    );
    let (st, _, s) = req(&app, Method::GET, "/api/datasets/prune/ldes", None, None).await;
    assert_eq!(st, StatusCode::OK, "{s}");
    let ds = doc(&s, &format!("{base}/api/datasets/prune/ldes"));
    assert_eq!(view_of(&ds), node(0), "tree:view is the root node: {s}");
    let (_, _, r) = get(0).await;
    let rels = relations(&doc(&r, &node(0)));
    assert!(
        rels.iter().all(|x| x.to != node(1)),
        "§5.1 no relation points at the compacted node: {r}"
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
    assert_eq!(view_of(&d2), node(0), "every page names the root: {n2}");

    // b3 (id 5) seals page 2 as ids 3..=4; page 3 is the tail. The root
    // bounds page 2 around members 3 and 4 and links the tail from member
    // 5's timestamp; the compacted page 1 is still 410 and unlinked.
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
    assert!(relations(&d2).is_empty(), "{n2}");
    let (st, _, n3) = get(3).await;
    assert_eq!(st, StatusCode::OK, "{n3}");
    let d3 = doc(&n3, &node(3));
    assert_eq!(member_ids(&d3), vec![(5, format!("{EX}b3"))]);
    let (_, _, r) = get(0).await;
    let rels = relations(&doc(&r, &node(0)));
    let bound = |kind: &str, to: &str| {
        rels.iter()
            .find(|x| x.to == to && x.kind == kind)
            .and_then(|x| x.value.clone())
            .unwrap_or_else(|| panic!("no {kind} to {to}: {r}"))
    };
    for id in [3, 4] {
        let t = instant(&created_of(&d2, id));
        assert!(
            instant(&bound("GreaterThanOrEqualToRelation", &node(2))) <= t
                && t <= instant(&bound("LessThanOrEqualToRelation", &node(2))),
            "member {id} lies within the root's bounds on page 2: {r}"
        );
    }
    assert_eq!(
        bound("GreaterThanOrEqualToRelation", &node(3)),
        created_of(&d3, 5),
        "{r}"
    );
    assert!(rels.iter().all(|x| x.to != node(1)), "{r}");
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
    // ago; entity c: a version 50 days ago and a tombstone 30 days ago —
    // appended in time order (LDES §4.1 forbids anything else).
    let ids: Vec<i64> = [
        ("c", 50, false),
        ("a", 40, false),
        ("c", 30, true),
        ("a", 20, false),
        ("b", 10, false),
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
    assert_eq!(remaining(), vec![ids[4], ids[5]]);

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

// ─── Publisher MUSTs and SHOULDs (LDES 1.0 §4.1–§4.3, Server Primer §2–§4) ──

/// LDES §4.1: "When ldes:timestampPath is set, no member can be added to the
/// LDES with a timestamp earlier than the latest published member." A write
/// that computed its `now` before a concurrent one but appends after it is
/// stamped with the later time — also after retention removed the member
/// that set it.
#[test]
fn a_member_is_never_stamped_earlier_than_the_latest_published_member() {
    use chrono::{Duration, Utc};
    use open_triplestore::ldes::store::{insert_member, last_created_at, member, set_stream};
    let (state, _) = admin_state();
    let db = &state.auth_db;
    state
        .auth_db
        .create_dataset(
            "mono",
            "Monotonic",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    set_stream(db, "mono", true, 100).unwrap();
    let later = Utc::now().to_rfc3339();
    let earlier = (Utc::now() - Duration::seconds(30)).to_rfc3339();
    let a = insert_member(db, "mono", &format!("{EX}a"), G, &later, false, "").unwrap();
    let b = insert_member(db, "mono", &format!("{EX}b"), G, &earlier, false, "").unwrap();
    assert_eq!(member(db, "mono", a).unwrap().unwrap().created_at, later);
    assert_eq!(
        member(db, "mono", b).unwrap().unwrap().created_at,
        later,
        "§4.1 the late append is raised to the latest published timestamp"
    );
    // Retention removes every member; the mark stays on the stream.
    {
        let conn = db.pool().get().unwrap();
        conn.execute("DELETE FROM ldes_members WHERE dataset_id = 'mono'", [])
            .unwrap();
    }
    assert_eq!(last_created_at(db, "mono").unwrap(), Some(later.clone()));
    let c = insert_member(db, "mono", &format!("{EX}c"), G, &earlier, false, "").unwrap();
    assert_eq!(member(db, "mono", c).unwrap().unwrap().created_at, later);

    // Concurrent writers that each take `now` and then append: the log is
    // non-decreasing in append order whatever the interleaving.
    let threads: Vec<_> = (0..8)
        .map(|t| {
            let db = state.auth_db.clone();
            std::thread::spawn(move || {
                for i in 0..25 {
                    let now = Utc::now().to_rfc3339();
                    if (t + i) % 3 == 0 {
                        std::thread::yield_now();
                    }
                    insert_member(&db, "mono", &format!("{EX}t{t}"), G, &now, false, "").unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    let conn = db.pool().get().unwrap();
    let mut st = conn
        .prepare("SELECT created_at FROM ldes_members WHERE dataset_id = 'mono' ORDER BY id")
        .unwrap();
    let stamps: Vec<String> = st.query_map([], |r| r.get(0)).unwrap().flatten().collect();
    assert_eq!(stamps.len(), 201);
    for w in stamps.windows(2) {
        assert!(
            instant(&w[0]) <= instant(&w[1]),
            "§4.1 {} is appended after {}",
            w[1],
            w[0]
        );
    }
}

/// The page's members, each with the set of blank nodes its description uses.
fn member_blank_nodes(d: &TripleStore) -> Vec<(String, String)> {
    let q = format!(
        "SELECT DISTINCT ?m ?b WHERE {{ ?c <{TREE}member> ?m . ?m ?p ?b . FILTER(isBlank(?b)) }}"
    );
    match d.query(&q) {
        Ok(QueryResults::Solutions(s)) => s
            .flatten()
            .map(|r| {
                (
                    r.get("m").unwrap().to_string(),
                    r.get("b").unwrap().to_string(),
                )
            })
            .collect(),
        _ => vec![],
    }
}

/// A member is the entity's description including its blank-node closure,
/// so an edit inside that closure is a new version (LDES §4.3: each version
/// object carries the entity's state). Rewriting identical content under
/// fresh blank-node labels is not a change. Two versions on one page keep
/// separate blank nodes (RDF 1.1 §3.4: a blank node is scoped to its
/// document, so shared labels would merge the versions' structures).
#[tokio::test]
async fn an_edit_inside_a_blank_node_publishes_a_version_and_a_relabel_does_not() {
    let (state, token) = admin_state();
    setup(&state, "bn");
    let app = test_app(state.clone());
    let turtle = |lat: u32| {
        format!(
            "<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"one\" ; <{EX}address> [ <{EX}street> \"Main\" ; <{EX}geo> [ <{EX}lat> {lat} ] ] . \
             <{EX}b2> a <{EX}Bridge> ; <{EX}name> \"two\" ."
        )
    };
    put_entity(&app, &token, turtle(52)).await;
    let (st, txt) = put_stream(
        &app,
        &token,
        "bn",
        json!({ "enabled": true, "page_size": 10 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    let members = |app: Router| async move {
        let (st, _, ttl) = req(
            &app,
            Method::GET,
            "/api/datasets/bn/ldes/nodes/1",
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{ttl}");
        ttl
    };
    let base = format!("{}/api/datasets/bn/ldes/nodes/1", state.base_url);
    assert_eq!(
        member_ids(&doc(&members(app.clone()).await, &base)).len(),
        2
    );

    // An edit two blank nodes deep, keeping every blank node: one new
    // version of b1, none of b2 — and the new version's stored blank-node
    // labels are the old version's.
    let (st, _, txt) = req(
        &app,
        Method::POST,
        "/sparql",
        Some(&token),
        Some((
            "application/sparql-update",
            format!(
                "DELETE {{ GRAPH <{G}> {{ ?g <{EX}lat> 52 }} }} INSERT {{ GRAPH <{G}> {{ ?g <{EX}lat> 53 }} }} \
                 WHERE {{ GRAPH <{G}> {{ ?g <{EX}lat> 52 }} }}"
            ),
        )),
    )
    .await;
    assert!(st.is_success(), "{st} {txt}");
    let ttl = members(app.clone()).await;
    let d = doc(&ttl, &base);
    let ids = member_ids(&d);
    assert_eq!(ids.len(), 3, "{ttl}");
    assert_eq!(
        ids[2].1,
        format!("{EX}b1"),
        "the new version is b1's: {ttl}"
    );
    assert!(
        ask(
            &d,
            &format!(
                "?m <http://purl.org/dc/terms/isVersionOf> <{EX}b1> ; <{EX}address> [ <{EX}geo> [ <{EX}lat> 53 ] ] . \
                 FILTER(STRENDS(STR(?m), \"/members/{}\"))",
                ids[2].0
            )
        ),
        "the version carries the edited closure: {ttl}"
    );
    let blank = member_blank_nodes(&d);
    let of = |m: i64| -> Vec<&String> {
        blank
            .iter()
            .filter(|(mi, _)| mi.ends_with(&format!("/members/{m}>")))
            .map(|(_, b)| b)
            .collect()
    };
    assert_eq!(of(ids[0].0).len(), 1, "{ttl}");
    assert_ne!(
        of(ids[0].0),
        of(ids[2].0),
        "two versions on one page do not share a blank node: {ttl}"
    );
    for lat in [52, 53] {
        assert_eq!(
            count(
                &d,
                &format!(
                    "?m <http://purl.org/dc/terms/isVersionOf> <{EX}b1> ; <{EX}address> ?a . ?a <{EX}geo> ?g . ?g <{EX}lat> {lat}"
                )
            ),
            1,
            "exactly one version holds lat {lat}: {ttl}"
        );
    }

    // Same content again under fresh blank-node labels (a PUT replaces the
    // graph): no new member.
    put_entity(&app, &token, turtle(53)).await;
    let ttl = members(app.clone()).await;
    assert_eq!(
        member_ids(&doc(&ttl, &base)).len(),
        3,
        "re-labelling identical blank nodes is not a new version: {ttl}"
    );
}

/// LDES §4.3: a stream whose members include deletes declares
/// `ldes:versionDeletePath` / `ldes:versionDeleteObject`, and each delete
/// carries that object on that path. Server Primer §6.1 puts them on the
/// root node's event stream.
#[tokio::test]
async fn deletes_are_declared_and_typed_with_the_declared_object() {
    let (state, token) = admin_state();
    setup(&state, "del");
    let app = test_app(state.clone());
    let (st, txt) = put_stream(
        &app,
        &token,
        "del",
        json!({ "enabled": true, "page_size": 10 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    put_entity(
        &app,
        &token,
        format!("<{EX}b1> a <{EX}Bridge> ; <{EX}name> \"one\" ."),
    )
    .await;
    let uri = "/api/datasets/del/ldes/nodes/1";
    let (st, _, ttl) = req(&app, Method::GET, uri, None, None).await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    let d = doc(&ttl, &format!("{}{uri}", state.base_url));
    let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    let as_delete = "https://www.w3.org/ns/activitystreams#Delete";
    assert!(
        ask(
            &d,
            &format!(
                "?s a <{LDES}EventStream> ; <{LDES}versionDeletePath> <{rdf_type}> ; <{LDES}versionDeleteObject> <{as_delete}>"
            )
        ),
        "§4.3 the delete path and object are declared: {ttl}"
    );
    // b2 vanished: its tombstone carries the declared object on the path.
    assert_eq!(
        count(
            &d,
            &format!(
                "?m <http://purl.org/dc/terms/isVersionOf> <{EX}b2> ; <{rdf_type}> <{as_delete}>"
            )
        ),
        1,
        "{ttl}"
    );
    assert!(
        ask(
            &d,
            &format!(
                "?m <http://purl.org/dc/terms/isVersionOf> <{EX}b2> ; a <https://opentriplestore.org/ns#Tombstone>"
            )
        ),
        "existing consumers keep their type: {ttl}"
    );
    assert_eq!(
        count(&d, &format!("?m a <{as_delete}>")),
        1,
        "versions that are not deletes do not carry it: {ttl}"
    );
}

/// LDES §4.2: "When building a processor to validate the members of an LDES,
/// the processor MUST pass each tree:member object as the target for the
/// given sh:NodeShape" — every member of every page conforms to the
/// declared `tree:shape`, tombstones included, and a member that lost its
/// `dct:isVersionOf` would not.
#[tokio::test]
async fn every_member_conforms_to_the_declared_shape() {
    let (state, token) = admin_state();
    setup(&state, "shape");
    let app = test_app(state.clone());
    three_pages(&app, &token, "shape").await;
    let (st, _, _) = req(
        &app,
        Method::DELETE,
        &format!("/store?graph={}", url_encode(G)),
        Some(&token),
        None,
    )
    .await;
    assert!(st.is_success(), "{st}");
    // The page's own shape, targeted at its members (§4.2); `break_one`
    // first removes one member's dct:isVersionOf, which must not conform.
    let validate = |ttl: &str, break_one: bool| {
        let d = doc(ttl, "http://localhost/");
        let shape = match d.query(&format!("SELECT ?sh WHERE {{ ?s <{TREE}shape> ?sh }}")) {
            Ok(QueryResults::Solutions(mut s)) => {
                s.next().unwrap().unwrap().get("sh").unwrap().to_string()
            }
            _ => panic!("no tree:shape: {ttl}"),
        };
        let store = TripleStore::in_memory().unwrap();
        for g in ["urn:test:data", "urn:test:shapes"] {
            store.load_str(ttl, RdfFormat::Turtle, Some(g)).unwrap();
        }
        store
            .update(&format!(
                "INSERT DATA {{ GRAPH <urn:test:shapes> {{ {shape} <http://www.w3.org/ns/shacl#targetObjectsOf> <{TREE}member> }} }}"
            ))
            .unwrap();
        if break_one {
            store
                .update(&format!(
                    "DELETE {{ GRAPH <urn:test:data> {{ ?m <http://purl.org/dc/terms/isVersionOf> ?e }} }} \
                     WHERE {{ {{ SELECT ?m ?e WHERE {{ GRAPH <urn:test:data> {{ ?c <{TREE}member> ?m . \
                     ?m <http://purl.org/dc/terms/isVersionOf> ?e }} }} LIMIT 1 }} }}"
                ))
                .unwrap();
        }
        open_triplestore::shacl::engine::validate(
            &store,
            "urn:test:shapes",
            &["urn:test:data".to_string()],
        )
        .unwrap()
    };
    let mut members = 0;
    for n in 1..=5 {
        let uri = format!("/api/datasets/shape/ldes/nodes/{n}");
        let (st, _, ttl) = req(&app, Method::GET, &uri, None, None).await;
        if st == StatusCode::NOT_FOUND {
            break;
        }
        assert_eq!(st, StatusCode::OK, "{ttl}");
        members += member_ids(&doc(&ttl, &format!("{}{uri}", state.base_url))).len();
        let report = validate(&ttl, false);
        assert!(
            report.conforms,
            "§4.2 node {n}: {:?}\n{ttl}",
            report.results
        );
        assert!(
            !validate(&ttl, true).conforms,
            "node {n}: the shape has teeth"
        );
    }
    assert!(
        members >= 8,
        "five writes and three tombstones were checked"
    );
}

/// TREE §2: "apart from the root node, [a node] has exactly one other
/// tree:Node of the search tree linking into it"; Server Primer §4: "Use two
/// relations towards one node, one with the lower bound and another with
/// the upper bound"; TREE §3: a relation constrains the whole subtree
/// reachable through it — so every member on a sealed node lies within the
/// root's bounds on it, and every member from the tail on is at or after its
/// bound.
#[tokio::test]
async fn the_root_bounds_each_sealed_node_and_every_node_has_one_parent() {
    let (state, token) = admin_state();
    setup(&state, "tree");
    let app = test_app(state.clone());
    three_pages(&app, &token, "tree").await;
    let node = |n: u64| format!("{}/api/datasets/tree/ldes/nodes/{n}", state.base_url);
    let mut all: Vec<Rel> = Vec::new();
    let mut created: Vec<Vec<String>> = Vec::new();
    for n in 0..=3u64 {
        let uri = format!("/api/datasets/tree/ldes/nodes/{n}");
        let (st, _, ttl) = req(&app, Method::GET, &uri, None, None).await;
        assert_eq!(st, StatusCode::OK, "{ttl}");
        let d = doc(&ttl, &node(n));
        all.extend(relations(&d));
        created.push(
            member_ids(&d)
                .iter()
                .map(|(id, _)| created_of(&d, *id))
                .collect(),
        );
    }
    assert!(created[0].is_empty(), "the root carries no members");
    for n in 1..=3u64 {
        let parents: std::collections::BTreeSet<&str> = all
            .iter()
            .filter(|r| r.to == node(n))
            .map(|r| r.from.as_str())
            .collect();
        assert_eq!(
            parents.into_iter().collect::<Vec<_>>(),
            vec![node(0).as_str()],
            "TREE §2 node {n} has exactly one parent, the root"
        );
    }
    let bound = |kind: &str, n: u64| {
        all.iter()
            .find(|r| r.to == node(n) && r.kind == kind)
            .and_then(|r| r.value.as_deref())
            .map(instant)
    };
    for n in 1..=2u64 {
        let (lo, hi) = (
            bound("GreaterThanOrEqualToRelation", n).unwrap(),
            bound("LessThanOrEqualToRelation", n).unwrap(),
        );
        for t in &created[n as usize] {
            assert!(
                lo <= instant(t) && instant(t) <= hi,
                "node {n}: {t} in [{lo}, {hi}]"
            );
        }
    }
    let tail_lo = bound("GreaterThanOrEqualToRelation", 3).unwrap();
    assert!(
        bound("LessThanOrEqualToRelation", 3).is_none(),
        "the tail grows: no upper bound"
    );
    for t in &created[3] {
        assert!(tail_lo <= instant(t), "tail: {t} >= {tail_lo}");
    }
}

/// Server Primer §2: "It SHOULD provide an ETag header on responses"; LDES
/// §3.3: a client "SHOULD support the If-None-Match request header … and
/// process the 304 Not Modified response". Each representation has its own
/// tag, and the mutable tail's tag changes when it does.
#[tokio::test]
async fn documents_carry_an_etag_and_answer_if_none_match_with_304() {
    let (state, token) = admin_state();
    setup(&state, "etag");
    let app = test_app(state.clone());
    three_pages(&app, &token, "etag").await;
    let get = |uri: &'static str, accept: &'static str, inm: Option<String>| {
        let app = app.clone();
        async move {
            let mut b = Request::builder().uri(uri).header(header::ACCEPT, accept);
            if let Some(t) = inm {
                b = b.header(header::IF_NONE_MATCH, t);
            }
            let resp = app.oneshot(b.body(Body::empty()).unwrap()).await.unwrap();
            let st = resp.status();
            let h = resp.headers().clone();
            (st, h, body_text(resp.into_body()).await)
        }
    };
    let tag = |h: &HeaderMap| h.get(header::ETAG).unwrap().to_str().unwrap().to_string();
    for uri in [
        "/api/datasets/etag/ldes",
        "/api/datasets/etag/ldes/nodes/0",
        "/api/datasets/etag/ldes/nodes/1",
        "/api/datasets/etag/ldes/nodes/3",
        "/api/datasets/etag/ldes/members/1",
    ] {
        let (st, h, _) = get(uri, "text/turtle", None).await;
        assert_eq!(st, StatusCode::OK, "{uri}");
        let t = tag(&h);
        assert!(
            t.starts_with('"') && t.ends_with('"'),
            "{uri}: a strong entity tag, {t}"
        );
        let (st, h2, body) = get(uri, "text/turtle", None).await;
        assert_eq!(
            (st, tag(&h2)),
            (StatusCode::OK, t.clone()),
            "{uri}: stable across requests"
        );
        let (st, h3, body304) = get(uri, "text/turtle", Some(t.clone())).await;
        assert_eq!(st, StatusCode::NOT_MODIFIED, "§3.3 {uri}");
        assert!(body304.is_empty(), "{uri}: 304 has no body");
        assert_eq!(tag(&h3), t, "{uri}: 304 repeats the tag");
        assert!(h3.get(header::CACHE_CONTROL).is_some(), "{uri}");
        let (st, _, _) = get(uri, "text/turtle", Some(format!("W/\"x\", {t}"))).await;
        assert_eq!(
            st,
            StatusCode::NOT_MODIFIED,
            "{uri}: a list matches any member"
        );
        let (st, h4, _) = get(uri, "application/n-triples", Some(t.clone())).await;
        assert_eq!(
            st,
            StatusCode::OK,
            "{uri}: another representation, another tag"
        );
        assert_ne!(tag(&h4), t, "{uri}");
        assert!(!body.is_empty());
    }
    // The tail changes: the old tag no longer matches.
    let (_, h, _) = get("/api/datasets/etag/ldes/nodes/3", "text/turtle", None).await;
    let before = tag(&h);
    post_entity(&app, &token, "b6", "six").await;
    let (st, h, _) = get(
        "/api/datasets/etag/ldes/nodes/3",
        "text/turtle",
        Some(before.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_ne!(tag(&h), before);
}

/// LDES §3.1 (a client "SHOULD check whether an ldes:pollingInterval was
/// set") and Server Primer §3: an `xsd:integer` number of seconds on the
/// event stream, 60 by default, settable per stream.
#[tokio::test]
async fn the_stream_declares_a_polling_interval() {
    let (state, token) = admin_state();
    setup(&state, "poll");
    let app = test_app(state.clone());
    let interval = |app: Router| async move {
        let (_, _, ttl) = req(&app, Method::GET, "/api/datasets/poll/ldes", None, None).await;
        let d = doc(&ttl, "http://localhost/");
        match d.query(&format!(
            "SELECT ?v WHERE {{ ?s <{LDES}pollingInterval> ?v }}"
        )) {
            Ok(QueryResults::Solutions(mut s)) => match s.next().unwrap().unwrap().get("v") {
                Some(oxigraph::model::Term::Literal(l)) => {
                    assert_eq!(
                        l.datatype().as_str(),
                        "http://www.w3.org/2001/XMLSchema#integer"
                    );
                    l.value().to_string()
                }
                other => panic!("{other:?}"),
            },
            _ => panic!("{ttl}"),
        }
    };
    let (st, txt) = put_stream(&app, &token, "poll", json!({ "enabled": true })).await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(interval(app.clone()).await, "60");
    let (st, txt) = put_stream(
        &app,
        &token,
        "poll",
        json!({ "enabled": true, "polling_interval": 3600 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert_eq!(interval(app.clone()).await, "3600");
    let (st, txt) = put_stream(
        &app,
        &token,
        "poll",
        json!({ "enabled": true, "polling_interval": 0 }),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{txt}");
    let (st, _) = put_stream(
        &app,
        &token,
        "poll",
        json!({ "enabled": true, "page_size": 5 }),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        interval(app.clone()).await,
        "3600",
        "absent leaves it alone"
    );
}

/// TREE member extraction: "when no quads of a member have been found, the
/// member will be dereferenced" — every member IRI a page names answers
/// with that member's quads, immutably; one that is not in the stream is
/// 404.
#[tokio::test]
async fn member_iris_dereference_to_the_member() {
    let (state, token) = admin_state();
    setup(&state, "deref");
    let app = test_app(state.clone());
    three_pages(&app, &token, "deref").await;
    let uri = "/api/datasets/deref/ldes/nodes/1";
    let (_, _, ttl) = req(&app, Method::GET, uri, None, None).await;
    let page = doc(&ttl, &format!("{}{uri}", state.base_url));
    let ids = member_ids(&page);
    assert_eq!(ids.len(), 2, "{ttl}");
    for (id, entity) in ids {
        let muri = format!("/api/datasets/deref/ldes/members/{id}");
        let miri = format!("{}{muri}", state.base_url);
        let (st, h, body) = req(&app, Method::GET, &muri, None, None).await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert!(
            h.get(header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap()
                .contains("immutable"),
            "a member never changes"
        );
        let d = doc(&body, &miri);
        assert!(ask(&d, &format!("?s <{TREE}member> <{miri}>")), "{body}");
        assert!(
            ask(
                &d,
                &format!(
                    "<{miri}> <http://purl.org/dc/terms/isVersionOf> <{entity}> ; <{EX}name> ?n"
                )
            ),
            "{body}"
        );
        assert_eq!(
            count(&d, &format!("<{miri}> ?p ?o")),
            count(&page, &format!("<{miri}> ?p ?o")),
            "the same quads as on the page: {body}"
        );
    }
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/datasets/deref/ldes/members/999999",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    state
        .auth_db
        .create_dataset(
            "other",
            "Other",
            None,
            OwnerType::User,
            "adm",
            Visibility::Public,
            None,
        )
        .unwrap();
    let (st, _) = put_stream(&app, &token, "other", json!({ "enabled": true })).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _, _) = req(
        &app,
        Method::GET,
        "/api/datasets/other/ldes/members/1",
        None,
        None,
    )
    .await;
    assert_eq!(
        st,
        StatusCode::NOT_FOUND,
        "a member of another stream is not this one's"
    );
}

/// Server Primer §2: "If the server is overloaded, it MUST provide a 429 Too
/// Many Requests." A stream whose renders are all in flight answers 429 with
/// `Retry-After` for every stream document, and serves again once a slot
/// frees.
#[tokio::test]
async fn an_overloaded_stream_answers_429_with_retry_after() {
    let (state, token) = admin_state();
    setup(&state, "busy");
    let app = test_app(state.clone());
    three_pages(&app, &token, "busy").await;
    let mut held = Vec::new();
    while let Some(slot) = open_triplestore::ldes::publish::try_enter("busy") {
        held.push(slot);
        assert!(held.len() < 10_000, "the gate has a limit");
    }
    for uri in [
        "/api/datasets/busy/ldes",
        "/api/datasets/busy/ldes/nodes/0",
        "/api/datasets/busy/ldes/nodes/1",
        "/api/datasets/busy/ldes/members/1",
    ] {
        let (st, h, _) = req(&app, Method::GET, uri, None, None).await;
        assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "§2 {uri}");
        assert!(h.get(header::RETRY_AFTER).is_some(), "{uri}");
    }
    held.pop();
    let (st, _, ttl) = req(
        &app,
        Method::GET,
        "/api/datasets/busy/ldes/nodes/1",
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ttl}");
    drop(held);
}
