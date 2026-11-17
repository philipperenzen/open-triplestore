//! Editing the blank nodes of a model draft over HTTP.
//!
//! Most of a SHACL shapes graph hangs off blank nodes: a property shape is
//! usually `sh:property [ sh:path … ]`, and `sh:in ( … )` is a chain of them.
//! `PATCH /api/models/:id/versions/:ver/data` could not touch either, because
//! a blank node label in a request is a new node. A request can now declare
//! the blank nodes of the draft it means (`nodes`), use them in `add` and
//! `remove`, and remove one with everything below it (`remove_nodes`).
//!
//! The three edits a shapes editor makes are driven here against a draft made
//! with `POST …/draft`, and read back with `GET …/data`: removing an anonymous
//! property shape, changing a value inside one, and replacing the members of
//! a list. The rest pins down what did not change: statements about named
//! nodes, the `If-Match` revision check and who may edit.

mod common;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use common::*;
use open_triplestore::auth::models::SystemRole;
use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
use open_triplestore::data_models::{registry as dmr, upload};
use open_triplestore::server::AppState;
use open_triplestore::store::TripleStore;
use oxigraph::io::RdfFormat;
use oxigraph::sparql::QueryResults;
use serde_json::{json, Value};
use tower::ServiceExt as _;

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const EX: &str = "http://example.org/library#";

/// Two node shapes whose property shapes and value list are all blank nodes.
/// Both constrain `ex:title`, so a property shape is only told apart by the
/// node shape it hangs off together with its path.
const SHAPES: &str = r#"
@prefix sh:  <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <http://example.org/library#> .

ex:BookShape a sh:NodeShape ;
    sh:targetClass ex:Book ;
    sh:property [ sh:path ex:title ; sh:datatype xsd:string ; sh:minCount 1 ; sh:maxCount 1 ] ;
    sh:property [ sh:path ex:isbn ; sh:datatype xsd:string ; sh:maxCount 1 ] ;
    sh:property [ sh:path ex:binding ; sh:in ( "hardcover" "paperback" ) ; sh:minCount 1 ] .

ex:MemberShape a sh:NodeShape ;
    sh:property [ sh:path ex:title ; sh:minCount 1 ] .
"#;

const DRAFT: &str = "/api/models/library/versions/1.1/data";
const SOURCE: &str = "/api/models/library/versions/1.0/data";

/// A private model `library`, owned by the admin, with the shapes above as
/// its published version 1.0 and a draft 1.1 made from it over the API — so
/// the draft's blank nodes are the copies `POST …/draft` makes.
async fn library_with_draft() -> (AppState, String) {
    let (state, token) = admin_state();
    let base = state.base_url.to_string();
    dmr::insert_data_model(
        &state.store,
        &base,
        "library",
        "Library",
        EX,
        None,
        false,
        Some("user"),
        Some("adm"),
        None,
        "2026-01-01T00:00:00Z",
    )
    .unwrap();
    let loaded = upload::parse_and_load(
        &state.store,
        &base,
        "library",
        Some("1.0"),
        SHAPES.as_bytes(),
        "text/turtle",
        "shapes.ttl",
        true,
    )
    .unwrap();
    dmr::insert_version(
        &state.store,
        &base,
        &DataModelVersion {
            data_model_id: "library".to_string(),
            version: "1.0".to_string(),
            status: VersionStatus::Published,
            graph_iri: format!("{base}/data-model/library/version/1.0"),
            sub_graphs: loaded.sub_graphs,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        },
    )
    .unwrap();

    let (status, _, body) = send(
        &state,
        Method::POST,
        "/api/models/library/versions/1.0/draft",
        Some(&token),
        None,
        Some(json!({ "target_version": "1.1" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "draft: {body}");
    (state, token)
}

async fn send(
    state: &AppState,
    method: Method,
    uri: &str,
    token: Option<&str>,
    if_match: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(revision) = if_match {
        req = req.header(header::IF_MATCH, revision);
    }
    let req = match body {
        Some(body) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = test_app(state.clone()).oneshot(req).await.unwrap();
    let (status, headers) = (resp.status(), resp.headers().clone());
    (status, headers, body_text(resp.into_body()).await)
}

async fn patch(state: &AppState, token: &str, body: Value) -> (StatusCode, Value) {
    let (status, _, text) = send(state, Method::PATCH, DRAFT, Some(token), None, Some(body)).await;
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

/// A version as `GET …/data` serves it: its N-Triples, its `ETag`, and the
/// triples loaded into a store of their own to query.
struct Read {
    ntriples: String,
    etag: String,
    graph: TripleStore,
}

async fn read(state: &AppState, token: &str, uri: &str) -> Read {
    let (status, headers, ntriples) = send(
        state,
        Method::GET,
        &format!("{uri}?format=nt"),
        Some(token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "read {uri}: {ntriples}");
    let graph = TripleStore::in_memory().unwrap();
    graph
        .load_str(&ntriples, RdfFormat::NTriples, None)
        .unwrap();
    Read {
        etag: headers[header::ETAG].to_str().unwrap().to_string(),
        ntriples,
        graph,
    }
}

impl Read {
    fn query(&self, body: &str) -> QueryResults<'static> {
        let q = format!(
            "PREFIX sh: <{SH}> PREFIX rdf: <{RDF}> PREFIX xsd: <{XSD}> PREFIX ex: <{EX}> {body}"
        );
        self.graph.query(&q).unwrap()
    }

    fn ask(&self, pattern: &str) -> bool {
        matches!(
            self.query(&format!("ASK {{ {pattern} }}")),
            QueryResults::Boolean(true)
        )
    }

    /// How many ways `pattern` matches.
    fn count(&self, pattern: &str) -> usize {
        let QueryResults::Solutions(mut rows) =
            self.query(&format!("SELECT (COUNT(*) AS ?n) WHERE {{ {pattern} }}"))
        else {
            panic!("a SELECT returns solutions");
        };
        match rows.next().unwrap().unwrap().get("n") {
            Some(oxigraph::model::Term::Literal(n)) => n.value().parse().unwrap(),
            other => panic!("COUNT gave {other:?}"),
        }
    }

    fn triples(&self) -> usize {
        self.count("?s ?p ?o")
    }
}

/// The property shape of `ex:BookShape` for `path`, as a `nodes` entry.
fn book_property(path: &str) -> Value {
    json!({
        "s": format!("{EX}BookShape"),
        "p": format!("{SH}property"),
        "where": [{ "p": format!("{SH}path"), "o": format!("{EX}{path}") }],
    })
}

fn integer(n: u32) -> Value {
    json!({ "value": n.to_string(), "datatype": format!("{XSD}integer") })
}

// ── The three edits ──────────────────────────────────────────────────────────

/// (1) An anonymous property shape is removed from its node shape: the link
/// to it and its own statements go, the other property shapes stay, and the
/// version the draft was made from keeps the shape.
#[tokio::test]
async fn removes_an_anonymous_property_shape() {
    let (state, token) = library_with_draft().await;
    let before = read(&state, &token, DRAFT).await;
    assert!(before.ask("ex:BookShape sh:property [ sh:path ex:isbn ]"));

    let (status, body) = patch(
        &state,
        &token,
        json!({
            "nodes": { "isbn": book_property("isbn") },
            "remove_nodes": ["isbn"],
            "message": "Drop the ISBN constraint",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let after = read(&state, &token, DRAFT).await;
    assert!(!after.ask("?shape sh:path ex:isbn"));
    assert_eq!(after.count("ex:BookShape sh:property ?shape"), 2);
    assert!(after.ask("ex:BookShape sh:property [ sh:path ex:title ; sh:minCount 1 ]"));
    assert!(after.ask("ex:BookShape sh:property [ sh:path ex:binding ; sh:in ?values ]"));
    // The link and the shape's path, datatype and maxCount: nothing else.
    assert_eq!(after.triples(), before.triples() - 4);
    assert_eq!(body["current_revision"], after.etag.trim_matches('"'));
    assert_ne!(after.etag, before.etag);

    // The draft shares its blank nodes with the version it copies; removing
    // one from the draft leaves that version as it was.
    let source = read(&state, &token, SOURCE).await;
    assert!(source.ask("ex:BookShape sh:property [ sh:path ex:isbn ; sh:maxCount 1 ]"));

    // The commit trail records what was written, not the size of the request.
    let (status, _, commits) = send(
        &state,
        Method::GET,
        "/api/models/library/commits",
        Some(&token),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{commits}");
    let commits: Value = serde_json::from_str(&commits).unwrap();
    let commit = commits
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["message"] == "Drop the ISBN constraint")
        .expect("the edit is in the commit trail");
    assert_eq!(
        (&commit["added"], &commit["removed"]),
        (&json!(0), &json!(4))
    );
}

/// (2) `sh:minCount` changes inside an anonymous property shape: the new
/// value lands on the same node, not on a new one, and the property shape of
/// the other node shape with the same path is left alone.
#[tokio::test]
async fn changes_a_value_inside_an_anonymous_property_shape() {
    let (state, token) = library_with_draft().await;
    let before = read(&state, &token, DRAFT).await;

    let (status, body) = patch(
        &state,
        &token,
        json!({
            "nodes": { "title": book_property("title") },
            "remove": [{ "s": "_:title", "p": format!("{SH}minCount"), "o": integer(1) }],
            "add": [{ "s": "_:title", "p": format!("{SH}minCount"), "o": integer(2) }],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let after = read(&state, &token, DRAFT).await;
    assert!(after.ask(
        "ex:BookShape sh:property [ sh:path ex:title ; sh:minCount 2 ; sh:maxCount 1 ; \
         sh:datatype xsd:string ]"
    ));
    assert!(!after.ask("ex:BookShape sh:property [ sh:path ex:title ; sh:minCount 1 ]"));
    assert_eq!(after.count("ex:BookShape sh:property ?shape"), 3);
    assert_eq!(after.count("?shape sh:minCount 2"), 1);
    assert!(after.ask("ex:MemberShape sh:property [ sh:path ex:title ; sh:minCount 1 ]"));
    assert_eq!(after.triples(), before.triples());
}

/// (3) The members of a `sh:in` list are replaced: the old list goes as a
/// whole, cells included, and the new one hangs off the same property shape.
/// The list is found through that shape, itself a declared node.
#[tokio::test]
async fn replaces_the_members_of_a_list() {
    let (state, token) = library_with_draft().await;
    let before = read(&state, &token, DRAFT).await;
    assert_eq!(before.count("?cell rdf:first ?member"), 2);

    let first = format!("{RDF}first");
    let rest = format!("{RDF}rest");
    let (status, body) = patch(
        &state,
        &token,
        json!({
            "nodes": {
                "binding": book_property("binding"),
                "values": { "s": "_:binding", "p": format!("{SH}in") },
            },
            "remove_nodes": ["values"],
            "add": [
                { "s": "_:binding", "p": format!("{SH}in"), "o": "_:c1" },
                { "s": "_:c1", "p": first, "o": { "value": "hardcover" } },
                { "s": "_:c1", "p": rest, "o": "_:c2" },
                { "s": "_:c2", "p": first, "o": { "value": "paperback" } },
                { "s": "_:c2", "p": rest, "o": "_:c3" },
                { "s": "_:c3", "p": first, "o": { "value": "e-book" } },
                { "s": "_:c3", "p": rest, "o": format!("{RDF}nil") },
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let after = read(&state, &token, DRAFT).await;
    assert!(after.ask(
        "ex:BookShape sh:property [ sh:path ex:binding ; sh:minCount 1 ; sh:in ?l1 ] . \
         ?l1 rdf:first \"hardcover\" ; rdf:rest ?l2 . \
         ?l2 rdf:first \"paperback\" ; rdf:rest ?l3 . \
         ?l3 rdf:first \"e-book\" ; rdf:rest rdf:nil"
    ));
    // One list on the shape, and no cell of the old one left behind.
    assert_eq!(after.count("?shape sh:in ?list"), 1);
    assert_eq!(after.count("?cell rdf:first ?member"), 3);
    assert_eq!(after.count("?cell rdf:rest ?next"), 3);
    assert_eq!(after.triples(), before.triples() + 2);
}

/// A node can also be named by the identifier a read of the draft shows for
/// it, which reaches a node at any depth without describing the way to it.
#[tokio::test]
async fn names_a_node_by_the_identifier_a_read_shows() {
    let (state, token) = library_with_draft().await;
    let before = read(&state, &token, DRAFT).await;
    let path_of_isbn = format!(" <{SH}path> <{EX}isbn> .");
    let id = before
        .ntriples
        .lines()
        .find_map(|line| line.strip_suffix(&path_of_isbn))
        .expect("the ISBN property shape is in the read")
        .to_string();
    assert!(id.starts_with("_:"), "a blank node: {id}");

    let (status, body) = patch(
        &state,
        &token,
        json!({
            "nodes": { "isbn": { "id": id } },
            "remove": [{ "s": "_:isbn", "p": format!("{SH}maxCount"), "o": integer(1) }],
            "add": [{ "s": "_:isbn", "p": format!("{SH}pattern"), "o": { "value": "^[0-9-]+$" } }],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let after = read(&state, &token, DRAFT).await;
    assert!(after.ask(
        "ex:BookShape sh:property [ sh:path ex:isbn ; sh:pattern \"^[0-9-]+$\" ; \
         sh:datatype xsd:string ]"
    ));
    assert!(!after.ask("?shape sh:path ex:isbn ; sh:maxCount ?max"));
    assert_eq!(after.triples(), before.triples());

    // An identifier the draft does not have names nothing.
    let (status, body) = patch(
        &state,
        &token,
        json!({ "nodes": { "gone": { "id": "_:nosuchnode" } }, "remove_nodes": ["gone"] }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "blank_node_not_found");
}

// ── Refusals ─────────────────────────────────────────────────────────────────

/// A description that fits no blank node, or more than one, refuses the whole
/// request with `409`, and a blank node `remove` does not declare is a `400`.
/// None of them writes anything, the valid statements of the request included.
#[tokio::test]
async fn refuses_a_node_it_cannot_find_and_writes_nothing() {
    let (state, token) = library_with_draft().await;
    let before = read(&state, &token, DRAFT).await;
    let valid_add = json!([{ "s": format!("{EX}BookShape"), "p": format!("{SH}closed"),
        "o": { "value": "true", "datatype": format!("{XSD}boolean") } }]);

    // Three property shapes hang off ex:BookShape.
    let (status, body) = patch(
        &state,
        &token,
        json!({
            "nodes": { "shape": { "s": format!("{EX}BookShape"), "p": format!("{SH}property") } },
            "remove_nodes": ["shape"],
            "add": valid_add,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "blank_node_ambiguous");
    assert_eq!(body["node"], "shape");
    assert_eq!(body["matches"], 3);

    let (status, body) = patch(
        &state,
        &token,
        json!({
            "nodes": { "title": book_property("title"), "price": book_property("price") },
            "remove_nodes": ["title", "price"],
            "add": valid_add,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "blank_node_not_found");
    assert_eq!(body["node"], "price");

    // With `nodes`, and in the plain form, where it used to be SPARQL's
    // "blank nodes are not allowed in DELETE DATA".
    for nodes in [json!({ "title": book_property("title") }), json!({})] {
        let (status, body) = patch(
            &state,
            &token,
            json!({
                "nodes": nodes,
                "remove": [{ "s": "_:other", "p": format!("{SH}minCount"), "o": integer(1) }],
                "add": valid_add,
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(
            body.to_string().contains("not declared in `nodes`"),
            "{body}"
        );
    }

    let after = read(&state, &token, DRAFT).await;
    assert_eq!(after.etag, before.etag, "nothing was written");
    assert!(!after.ask("ex:BookShape sh:closed ?closed"));
}

// ── What did not change ──────────────────────────────────────────────────────

/// Statements about named nodes are added and removed as before, and a blank
/// node label in `add` is still a new node, one per label and request — also
/// in a request that declares other nodes.
#[tokio::test]
async fn statements_about_named_nodes_work_as_before() {
    let (state, token) = library_with_draft().await;
    let before = read(&state, &token, DRAFT).await;

    let (status, body) = patch(
        &state,
        &token,
        json!({
            "remove": [{ "s": format!("{EX}BookShape"), "p": format!("{SH}targetClass"),
                "o": format!("{EX}Book") }],
            "add": [
                { "s": format!("{EX}BookShape"), "p": format!("{SH}targetClass"),
                    "o": format!("{EX}Publication") },
                { "s": format!("{EX}BookShape"), "p": "http://www.w3.org/2000/01/rdf-schema#label",
                    "o": { "value": "Boek", "lang": "nl" } },
                { "s": format!("{EX}BookShape"), "p": format!("{SH}property"), "o": "_:new" },
                { "s": "_:new", "p": format!("{SH}path"), "o": format!("{EX}author") },
                { "s": "_:new", "p": format!("{SH}minCount"), "o": integer(1) },
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let after = read(&state, &token, DRAFT).await;
    assert!(after.ask("ex:BookShape sh:targetClass ex:Publication"));
    assert!(!after.ask("ex:BookShape sh:targetClass ex:Book"));
    assert!(after.ask("ex:BookShape <http://www.w3.org/2000/01/rdf-schema#label> \"Boek\"@nl"));
    assert!(after.ask("ex:BookShape sh:property [ sh:path ex:author ; sh:minCount 1 ]"));
    assert_eq!(after.count("ex:BookShape sh:property ?shape"), 4);
    assert_eq!(after.triples(), before.triples() + 4);
    assert_eq!(body["current_revision"], after.etag.trim_matches('"'));

    // The same label again, beside a declared node: another new node, and
    // the declared one is the shape made above.
    let (status, body) = patch(
        &state,
        &token,
        json!({
            "nodes": { "author": book_property("author") },
            "add": [
                { "s": "_:author", "p": format!("{SH}node"), "o": "_:new" },
                { "s": "_:new", "p": format!("{SH}class"), "o": format!("{EX}Person") },
            ],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = read(&state, &token, DRAFT).await;
    assert!(after.ask(
        "ex:BookShape sh:property [ sh:path ex:author ; sh:minCount 1 ; \
         sh:node [ sh:class ex:Person ] ]"
    ));
    assert_eq!(after.count("?node sh:class ex:Person"), 1);
    assert_eq!(after.count("?node sh:path ex:author"), 1);
}

/// `If-Match` holds for both forms of the request: the revision a read gave
/// is accepted once, and refused with the current one after the draft moved.
#[tokio::test]
async fn a_stale_revision_is_refused() {
    let (state, token) = library_with_draft().await;
    let loaded = read(&state, &token, DRAFT).await;
    let closed = |value: &str| {
        json!({ "s": format!("{EX}BookShape"), "p": format!("{SH}closed"),
            "o": { "value": value, "datatype": format!("{XSD}boolean") } })
    };

    let (status, headers, body) = send(
        &state,
        Method::PATCH,
        DRAFT,
        Some(&token),
        Some(&loaded.etag),
        Some(json!({ "add": [closed("true")], "remove": [] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let moved = headers[header::ETAG].to_str().unwrap().to_string();
    assert_ne!(moved, loaded.etag);

    let stale_requests = [
        json!({ "add": [closed("false")], "remove": [closed("true")] }),
        json!({ "nodes": { "isbn": book_property("isbn") }, "remove_nodes": ["isbn"] }),
    ];
    for request in &stale_requests {
        let (status, _, body) = send(
            &state,
            Method::PATCH,
            DRAFT,
            Some(&token),
            Some(&loaded.etag),
            Some(request.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["error"], "stale_revision");
        assert_eq!(body["current_revision"], moved.trim_matches('"'));
    }
    let unchanged = read(&state, &token, DRAFT).await;
    assert_eq!(unchanged.etag, moved);
    assert!(unchanged.ask("?shape sh:path ex:isbn"));

    // With the current revision the node edit goes through, and answers
    // with the next one.
    let (status, headers, body) = send(
        &state,
        Method::PATCH,
        DRAFT,
        Some(&token),
        Some(&moved),
        Some(stale_requests[1].clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let after = read(&state, &token, DRAFT).await;
    assert_eq!(headers[header::ETAG].to_str().unwrap(), after.etag);
    assert!(!after.ask("?shape sh:path ex:isbn"));
}

/// Naming blank nodes opens the draft to nobody new: another signed-in user
/// and an anonymous caller are refused as before, and a version that is not a
/// draft cannot be edited this way either.
#[tokio::test]
async fn only_a_writer_of_the_model_edits_a_draft() {
    let (state, token) = library_with_draft().await;
    state
        .auth_db
        .create_user("u_bob", "bob", "bob@test.com", "hash", SystemRole::User)
        .unwrap();
    state
        .auth_db
        .update_user_can_publish("u_bob", true)
        .unwrap();
    let other = mint_token("u_bob", "bob", "user");
    let request = json!({ "nodes": { "isbn": book_property("isbn") }, "remove_nodes": ["isbn"] });

    for caller in [Some(other.as_str()), None] {
        let (status, _, body) = send(
            &state,
            Method::PATCH,
            DRAFT,
            caller,
            None,
            Some(request.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    }

    let (status, _, body) = send(
        &state,
        Method::PATCH,
        SOURCE,
        Some(&token),
        None,
        Some(request),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("Only Draft versions can be edited"), "{body}");

    for uri in [DRAFT, SOURCE] {
        assert!(read(&state, &token, uri)
            .await
            .ask("ex:BookShape sh:property [ sh:path ex:isbn ]"));
    }
}
