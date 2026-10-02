//! RDF Patch conformance, one test per clause of the format page
//! (<https://afs.github.io/rdf-delta/rdf-patch.html>) and of the patch log
//! page (<https://afs.github.io/rdf-delta/rdf-patch-logs.html>): the parser
//! in `open_triplestore::rdf_patch`, the store's transactional quad path,
//! `TripleStore::apply_quad_ops`, that a dataset patch is applied through,
//! the dataset prefix table `PA` / `PD` change, and a dataset's patch log
//! (`/api/datasets/:id/log`). Other HTTP behaviour is in
//! `tests/rdf_patch_http.rs`.

mod common;

use std::collections::HashSet;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use common::{admin_state, body_text, test_app};
use open_triplestore::auth::models::{OwnerType, Visibility};
use open_triplestore::rdf_patch::{parse, prefix_changes, render, Op, Patch, PrefixOp};
use open_triplestore::server::AppState;
use open_triplestore::store::{QuadOp, TripleStore};
use oxigraph::io::RdfFormat;
use oxigraph::model::{BlankNode, GraphNameRef, NamedNode, NamedOrBlankNode, Quad, Term};
use tower::ServiceExt as _;

const G: &str = "https://example.org/patch/g";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

fn graph() -> NamedNode {
    NamedNode::new(G).unwrap()
}

/// Apply `patch` to `store`, triples going to [`G`]: the net (added, removed).
fn apply(store: &TripleStore, patch: &Patch) -> (usize, usize) {
    store
        .apply_quad_ops(&patch.quad_ops(Some(&graph())).unwrap())
        .unwrap()
}

fn quads(store: &TripleStore) -> HashSet<Quad> {
    store
        .quads_for_graph(GraphNameRef::NamedNode(graph().as_ref()))
        .unwrap()
        .into_iter()
        .collect()
}

fn ntriples(store: &TripleStore) -> HashSet<String> {
    quads(store)
        .into_iter()
        .map(|q| format!("{} {} {}", q.subject, q.predicate, q.object))
        .collect()
}

/// The blank node in subject position of `G`'s only triple with `predicate`.
fn bnode_with(store: &TripleStore, predicate: &str) -> BlankNode {
    quads(store)
        .into_iter()
        .find_map(|q| match q.subject {
            NamedOrBlankNode::BlankNode(b) if q.predicate.as_str() == predicate => Some(b),
            _ => None,
        })
        .expect("a blank-node subject")
}

/// §Example: the page's first example, with its quoted `PA` forms inside the
/// block, parses and adds its three triples.
#[test]
fn the_spec_example_applies() {
    let p = parse(
        r#"TX .
PA "rdf" "http://www.w3.org/1999/02/22-rdf-syntax-ns#" .
PA "owl" "http://www.w3.org/2002/07/owl#" .
PA "rdfs" "http://www.w3.org/2000/01/rdf-schema#" .
A <http://example/SubClass> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> .
A <http://example/SubClass> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <http://example/SUPER_CLASS> .
A <http://example/SubClass> <http://www.w3.org/2000/01/rdf-schema#label> "SubClass" .
TC .
"#,
    )
    .unwrap();
    assert_eq!((p.adds(), p.deletes(), p.committed), (3, 0, 1));
    assert_eq!(
        p.prefixes
            .iter()
            .map(|(n, _)| n.as_str())
            .collect::<Vec<_>>(),
        ["rdf", "owl", "rdfs"]
    );
    let store = TripleStore::in_memory().unwrap();
    assert_eq!(apply(&store, &p), (3, 0));
    assert!(ntriples(&store).contains(&format!(
        "<http://example/SubClass> <{RDF_TYPE}> <http://www.w3.org/2002/07/owl#Class>"
    )));
}

/// §Example (patch log form) and §Header: `H word RDFTerm .`, the word
/// unquoted or quoted, the value any RDF term.
#[test]
fn headers_are_word_and_term_pairs() {
    let p = parse(
        "H id <uuid:0686c69d-8f89-4496-acb5-744f0157a8db> .\nH prev <uuid:3ee0eca0-6d5f-4b4d-85db-f69ab1167eb1> .\nH \"note\" \"a string\" .\nTX .\nTC .\n",
    )
    .unwrap();
    assert_eq!(p.id(), Some("uuid:0686c69d-8f89-4496-acb5-744f0157a8db"));
    assert_eq!(
        p.headers,
        vec![
            (
                "id".to_string(),
                "uuid:0686c69d-8f89-4496-acb5-744f0157a8db".to_string()
            ),
            (
                "prev".to_string(),
                "uuid:3ee0eca0-6d5f-4b4d-85db-f69ab1167eb1".to_string()
            ),
            ("note".to_string(), "a string".to_string()),
        ]
    );
    assert!(parse("H id .\n").is_err(), "a header needs a value");
    assert!(
        parse("H id <urn:a> <urn:b> .\n").is_err(),
        "a header has one value"
    );
}

/// §Structure: "a series of rows, each row ends with a . (DOT)" — a row is
/// not a line: rows may share a line or span several, and the dot may touch
/// the last token.
#[test]
fn rows_end_with_a_dot_not_a_line_break() {
    let p = parse("TX . A <urn:s> <urn:p>\n  <urn:o> . A <urn:s> <urn:p> \"x\".\nTC.").unwrap();
    assert_eq!((p.adds(), p.committed), (2, 1));
    assert!(
        parse("TX .\nA <urn:s> <urn:p> <urn:o>\nTC .\n").is_err(),
        "without its dot the row runs on into the next line"
    );
    let unterminated = parse("TX .\nTC").unwrap_err();
    assert!(unterminated.contains("terminating"), "{unterminated}");
}

/// §Preferred Style: "comments start # and run to end of line".
#[test]
fn comments_run_to_the_end_of_the_line() {
    let p = parse(
        "# a patch\nTX . # open\nA <urn:s> <http://example/p#x> <urn:o> . # an IRI may hold a #\nTC .\n",
    )
    .unwrap();
    assert_eq!(p.adds(), 1);
    let Op::Add(q) = &p.ops[0] else { panic!() };
    assert_eq!(q.predicate.as_str(), "http://example/p#x");
}

/// §Structure: "Multiple transaction blocks are allowed for multiple sets of
/// changes in one patch", applied in order.
#[test]
fn multiple_transaction_blocks_apply_in_order() {
    let p = parse(
        "TX .\nA <urn:s> <urn:p> \"1\" .\nTC .\nTX .\nD <urn:s> <urn:p> \"1\" .\nA <urn:s> <urn:p> \"2\" .\nTC .\n",
    )
    .unwrap();
    assert_eq!(p.committed, 2);
    let store = TripleStore::in_memory().unwrap();
    assert_eq!(
        apply(&store, &p),
        (1, 0),
        "the net change: \"1\" came and went"
    );
    assert_eq!(
        ntriples(&store),
        HashSet::from(["<urn:s> <urn:p> \"2\"".to_string()])
    );
}

/// §Transactions: blocks delimit changes; they do not nest, and `TC` / `TA`
/// close an open block only.
#[test]
fn blocks_do_not_nest_and_must_be_closed() {
    assert!(parse("TX .\nTX .\nTC .\nTC .\n")
        .unwrap_err()
        .contains("do not nest"));
    assert!(parse("TC .\n").unwrap_err().contains("TC without TX"));
    assert!(parse("TA .\n").unwrap_err().contains("TA without TX"));
    assert!(parse("TX .\nA <urn:s> <urn:p> <urn:o> .\n")
        .unwrap_err()
        .contains("not closed"));
}

/// §Transactions: `TA` aborts its own block — its rows, prefix changes
/// included, are discarded — and leaves the blocks around it in force.
#[test]
fn abort_discards_only_its_own_block() {
    let p = parse(
        "TX .\nA <urn:s> <urn:p> \"kept 1\" .\nTC .\nTX .\nPA x <http://example/x#> .\nA <urn:s> <urn:p> \"dropped\" .\nTA .\nTX .\nA <urn:s> <urn:p> \"kept 2\" .\nTC .\n",
    )
    .unwrap();
    assert_eq!((p.committed, p.aborted), (2, 1));
    assert!(
        p.prefixes.is_empty(),
        "the aborted block's PA is undone too"
    );
    let store = TripleStore::in_memory().unwrap();
    apply(&store, &p);
    assert_eq!(
        ntriples(&store),
        HashSet::from([
            "<urn:s> <urn:p> \"kept 1\"".to_string(),
            "<urn:s> <urn:p> \"kept 2\"".to_string(),
        ])
    );
}

/// §Transactions: "Transactions should be applied atomically" — a patch that
/// fails anywhere changes nothing.
#[test]
fn a_patch_applies_atomically() {
    let store = TripleStore::in_memory().unwrap();
    let broken =
        "TX .\nA <urn:s> <urn:p> \"1\" .\nTC .\nTX .\nA <urn:s> <urn:p> <not an iri> .\nTC .\n";
    assert!(parse(broken).is_err());
    assert!(quads(&store).is_empty());
    // One transaction for the whole patch: a later row sees an earlier one.
    let p = parse("TX .\nA <urn:s> <urn:p> \"1\" .\nD <urn:s> <urn:p> \"1\" .\nA <urn:t> <urn:p> \"2\" .\nTC .\n").unwrap();
    assert_eq!(apply(&store, &p), (1, 0));
    assert_eq!(
        ntriples(&store),
        HashSet::from(["<urn:t> <urn:p> \"2\"".to_string()])
    );
}

/// §Prefixes: "The prefix name is without the trailing colon. It can be given
/// as a quoted string or unquoted string (keyword)". The namespace is an IRI,
/// or a string as in the page's example; `PD` removes one.
#[test]
fn prefix_rows_take_keyword_or_quoted_names() {
    let p = parse(
        "PA rdf <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\nPA \"owl\" \"http://www.w3.org/2002/07/owl#\" .\nPA \"\" <http://example/> .\nPA legacy: <http://example/legacy#> .\nPD \"owl\" .\nPD legacy .\n",
    )
    .unwrap();
    assert_eq!(
        p.prefixes,
        vec![
            (
                "rdf".to_string(),
                "http://www.w3.org/1999/02/22-rdf-syntax-ns#".to_string()
            ),
            (String::new(), "http://example/".to_string()),
        ]
    );
    for bad in [
        "PA \"a b\" <http://example/> .",
        "PA 1x <http://example/> .",
        "PA x. <http://example/> .",
        "PA x \"not an iri\" .",
        "PA x <http://example/> <http://example/> .",
        "PD .",
    ] {
        assert!(parse(bad).is_err(), "{bad} must be refused");
    }
}

/// §Prefixes: "Prefixes do not apply to the data of the patch. They are
/// changes to the data the patch is applied to." No quad changes for them.
/// (That a row may still use a declared prefixed name is this store's
/// documented extension, also shown here.)
#[test]
fn prefix_changes_are_not_data_changes() {
    let p = parse("TX .\nPA ex <http://example/> .\nPD ex .\nTC .\n").unwrap();
    assert!(p.ops.is_empty());
    let ext = parse("TX .\nPA ex <http://example/> .\nA ex:s ex:p ex:o .\nTC .\n").unwrap();
    let Op::Add(q) = &ext.ops[0] else { panic!() };
    assert_eq!(q.subject.to_string(), "<http://example/s>");
}

/// §Quads and Triples: rows are "written like N-Quads, 3 or 4 RDF terms",
/// S-P-O or S-P-O-G.
#[test]
fn rows_are_triples_or_quads() {
    let p =
        parse("TX .\nA <urn:s> <urn:p> <urn:o> .\nA <urn:s> <urn:p> <urn:o> <urn:other> .\nTC .\n")
            .unwrap();
    let ops = p.quad_ops(Some(&graph())).unwrap();
    assert_eq!(ops[0].quad().graph_name.to_string(), format!("<{G}>"));
    assert_eq!(ops[1].quad().graph_name.to_string(), "<urn:other>");
    assert!(
        p.quad_ops(None).is_err(),
        "a triple has no graph unless one is given"
    );
    for bad in [
        "A <urn:s> <urn:p> .",
        "A <urn:s> <urn:p> <urn:o> <urn:g> <urn:x> .",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

/// §Quads and Triples: terms in N-Triples syntax — IRIs, language-tagged and
/// typed literals with escapes; a literal only as the object.
#[test]
fn terms_are_n_triples_terms() {
    let p = parse(
        "A <urn:s> <urn:p> \"a \\\"quoted\\\" line\\n\"@en-GB .\nA <urn:s> <urn:p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\nA <urn:s> <urn:p> \"\\u00E9\" .\n",
    )
    .unwrap();
    let objects: Vec<String> = p
        .ops
        .iter()
        .map(|o| match o {
            Op::Add(q) | Op::Delete(q) => q.object.to_string(),
        })
        .collect();
    assert_eq!(
        objects,
        [
            "\"a \\\"quoted\\\" line\\n\"@en-gb",
            "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            "\"é\"",
        ]
    );
    for bad in [
        "A \"lit\" <urn:p> <urn:o> .",
        "A <urn:s> \"lit\" <urn:o> .",
        "A <urn:s> <urn:p> <urn:o> \"lit\" .",
        "A <urn:s> <urn:p> \"open .",
        "A <urn:s> <urn:p> <urn:o .",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

/// §Blank nodes: "blank node labels refer to the 'system identifier' for the
/// blank node", "written as _:label or <_:label>". A row can delete a blank
/// node already in the data and link to it, without minting a new one.
#[test]
fn blank_node_labels_name_the_stores_nodes() {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            "[] <urn:name> \"n\" ; <urn:status> \"draft\" .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    let b = bnode_with(&store, "urn:name");
    let id = b.as_str();
    let p = parse(&format!(
        "TX .\nD _:{id} <urn:status> \"draft\" .\nA <_:{id}> <urn:status> \"final\" .\nA <urn:doc> <urn:about> _:{id} .\nTC .\n"
    ))
    .unwrap();
    assert_eq!(apply(&store, &p), (2, 1));
    let triples = ntriples(&store);
    assert_eq!(
        triples,
        HashSet::from([
            format!("_:{id} <urn:name> \"n\""),
            format!("_:{id} <urn:status> \"final\""),
            format!("<urn:doc> <urn:about> _:{id}"),
        ]),
        "the same node throughout, no fresh one"
    );
    // A label the data does not hold: deleting it is a no-op, adding it
    // creates the node with that id.
    let p = parse("D _:nowhere <urn:p> \"x\" .\nA _:fresh1 <urn:p> \"x\" .\n").unwrap();
    assert_eq!(apply(&store, &p), (1, 0));
    assert!(ntriples(&store).contains("_:fresh1 <urn:p> \"x\""));
}

/// §Blank nodes: "_ is illegal as a IRI scheme" — `<_:label>` is a blank node,
/// never an IRI; and a blank node is a subject or an object only.
#[test]
fn bracketed_labels_are_blank_nodes_in_node_positions_only() {
    let p = parse("A <_:b1> <urn:p> <_:b2> .\n").unwrap();
    let Op::Add(q) = &p.ops[0] else { panic!() };
    assert_eq!(
        q.subject,
        NamedOrBlankNode::BlankNode(BlankNode::new("b1").unwrap())
    );
    assert_eq!(q.object, Term::BlankNode(BlankNode::new("b2").unwrap()));
    for bad in [
        "A <urn:s> _:p <urn:o> .",
        "A <urn:s> <_:p> <urn:o> .",
        "A <urn:s> <urn:p> <urn:o> _:g .",
        "A <_:has space> <urn:p> <urn:o> .",
    ] {
        assert!(parse(bad).is_err(), "{bad}");
    }
}

/// §Blank nodes: "In order to synchronize datasets, changes involving blank
/// nodes may need to refer to a blank node already in the data." A diff
/// written as a patch and applied to a copy reproduces the target exactly,
/// blank nodes included.
#[test]
fn a_diff_with_blank_nodes_applies_faithfully() {
    let store = TripleStore::in_memory().unwrap();
    store
        .load_str(
            "[] <urn:name> \"a\" ; <urn:part> [ <urn:v> 1 ] . <urn:x> <urn:p> \"keep\" .",
            RdfFormat::Turtle,
            Some(G),
        )
        .unwrap();
    // A copy of the graph, same nodes, under another name.
    let copy = "https://example.org/patch/copy";
    store
        .update(&format!(
            "INSERT {{ GRAPH <{copy}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{G}> {{ ?s ?p ?o }} }}"
        ))
        .unwrap();
    // Change the original: one blank node's value, one new blank node.
    let inner = bnode_with(&store, "urn:v").as_str().to_string();
    let edit = parse(&format!(
        "TX .\nD _:{inner} <urn:v> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\nA _:{inner} <urn:v> \"2\"^^<http://www.w3.org/2001/XMLSchema#integer> .\nA _:extra <urn:name> \"b\" .\nTC .\n"
    ))
    .unwrap();
    apply(&store, &edit);
    // The diff copy → original, retargeted at the copy, applied to it.
    let text = render(
        &store,
        "urn:uuid:diff",
        None,
        &[],
        &[],
        &[(
            copy.to_string(),
            Some(copy.to_string()),
            Some(G.to_string()),
        )],
    );
    let p = parse(&text).unwrap();
    assert_eq!((p.adds(), p.deletes()), (2, 1), "{text}");
    store.apply_quad_ops(&p.quad_ops(None).unwrap()).unwrap();
    let in_copy: HashSet<String> = store
        .quads_for_graph(GraphNameRef::NamedNode(
            NamedNode::new(copy).unwrap().as_ref(),
        ))
        .unwrap()
        .into_iter()
        .map(|q| format!("{} {} {}", q.subject, q.predicate, q.object))
        .collect();
    assert_eq!(in_copy, ntriples(&store));
}

/// §Quads and Triples, read with the store's set semantics: adding a quad
/// that is present, or deleting one that is absent, changes nothing.
#[test]
fn adding_present_and_deleting_absent_quads_change_nothing() {
    let store = TripleStore::in_memory().unwrap();
    let p = parse(
        "A <urn:s> <urn:p> <urn:o> .\nA <urn:s> <urn:p> <urn:o> .\nD <urn:t> <urn:p> <urn:o> .\n",
    )
    .unwrap();
    assert_eq!(apply(&store, &p), (1, 0));
    assert_eq!(apply(&store, &p), (0, 0));
    let ops = [QuadOp::Remove(
        p.quad_ops(Some(&graph())).unwrap()[0].quad().clone(),
    )];
    assert_eq!(store.apply_quad_ops(&ops).unwrap(), (0, 1));
    assert!(quads(&store).is_empty());
}

/// RDF 1.2 triple terms have no RDF Patch syntax yet (owner decision D10):
/// refused with a message, not misread as an IRI.
#[test]
fn triple_terms_are_refused() {
    let e = parse("A <urn:s> <urn:p> <<( <urn:a> <urn:b> <urn:c> )>> .\n").unwrap_err();
    assert!(e.contains("triple terms"), "{e}");
}

// ── Prefixes: changes to the data the patch is applied to ───────────────────

/// §Prefixes: `PA` / `PD` are changes to the prefixes of the data the patch
/// is applied to, in patch order; an aborted block's go with it.
#[test]
fn prefix_rows_are_ordered_changes_and_abort_with_their_block() {
    let p = parse(
        "PA \"a\" <http://example/a#> .\nTX .\nPD \"a\" .\nPA \"b\" <http://example/b#> .\nTC .\nTX .\nPA \"c\" <http://example/c#> .\nTA .\n",
    )
    .unwrap();
    assert_eq!(
        p.prefix_ops,
        vec![
            PrefixOp::Add {
                name: "a".into(),
                namespace: "http://example/a#".into()
            },
            PrefixOp::Delete { name: "a".into() },
            PrefixOp::Add {
                name: "b".into(),
                namespace: "http://example/b#".into()
            },
        ]
    );
    // A diff between two prefix tables is the PD / PA rows between them.
    let from = vec![
        ("a".to_string(), "http://example/a#".to_string()),
        ("b".to_string(), "http://example/b#".to_string()),
    ];
    let to = vec![
        ("b".to_string(), "http://example/b2#".to_string()),
        ("c".to_string(), "http://example/c#".to_string()),
    ];
    assert_eq!(
        prefix_changes(&from, &to),
        vec![
            ("a".to_string(), None),
            ("b".to_string(), Some("http://example/b2#".to_string())),
            ("c".to_string(), Some("http://example/c#".to_string())),
        ]
    );
}

// ── HTTP: a dataset's prefix table and patch log ────────────────────────────

const LG: &str = "https://example.org/patch-log/g";

async fn call(
    app: &Router,
    method: Method,
    uri: &str,
    token: Option<&str>,
    ct: Option<&str>,
    body: &str,
) -> (StatusCode, String) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(c) = ct {
        b = b.header(header::CONTENT_TYPE, c);
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let st = resp.status();
    (st, body_text(resp.into_body()).await)
}

/// A private dataset `log` with one registered graph, optionally holding data.
fn log_dataset(data: Option<&str>) -> (AppState, String, Router) {
    let (state, token) = admin_state();
    state
        .auth_db
        .create_dataset(
            "log",
            "log",
            None,
            OwnerType::User,
            "adm",
            Visibility::Private,
            None,
        )
        .unwrap();
    state.auth_db.add_dataset_graph("log", LG).unwrap();
    if let Some(d) = data {
        state
            .store
            .load_str(d, RdfFormat::Turtle, Some(LG))
            .unwrap();
    }
    let app = test_app(state.clone());
    (state, token, app)
}

async fn append(app: &Router, token: &str, text: &str) -> (StatusCode, String) {
    call(
        app,
        Method::POST,
        "/api/datasets/log/log",
        Some(token),
        Some("application/rdf-patch"),
        text,
    )
    .await
}

fn holds(state: &AppState, s: &str) -> bool {
    matches!(
        state
            .store
            .query(&format!("ASK {{ GRAPH <{LG}> {{ <{s}> ?p ?o }} }}")),
        Ok(oxigraph::sparql::QueryResults::Boolean(true))
    )
}

/// §Prefixes, applied: a patch's `PA` / `PD` rows change the dataset's
/// prefix table and leave its data alone.
#[tokio::test]
async fn an_applied_patch_changes_the_datasets_prefix_table() {
    let (state, token, app) = log_dataset(None);
    let (st, txt) = call(
        &app,
        Method::POST,
        "/api/datasets/log/patch",
        Some(&token),
        Some("application/rdf-patch"),
        "TX .\nPA \"ex\" <http://example/> .\nPA \"old\" <http://example/old#> .\nPD \"old\" .\nTC .\n",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(txt.contains("\"applied\":true"), "{txt}");
    assert_eq!(
        state.auth_db.dataset_prefix_pairs("log").unwrap(),
        vec![("ex".to_string(), "http://example/".to_string())]
    );
    assert_eq!(
        state.store.count_graph(Some(LG)).unwrap(),
        0,
        "no data changed"
    );
}

/// §Identifying Patches: "There must be exactly one id header row"; "There
/// is at most one prev header row".
#[tokio::test]
async fn a_logged_patch_has_one_id_and_at_most_one_prev() {
    let (state, token, app) = log_dataset(None);
    for bad in [
        "TX .\nA <urn:a> <urn:p> \"1\" <https://example.org/patch-log/g> .\nTC .\n",
        "H id <urn:uuid:1> .\nH id <urn:uuid:2> .\nTX .\nTC .\n",
        "H id <urn:uuid:1> .\nH prev <urn:uuid:0> .\nH prev <urn:uuid:9> .\nTX .\nTC .\n",
    ] {
        let (st, txt) = append(&app, &token, bad).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{bad}: {txt}");
    }
    assert!(!holds(&state, "urn:a"), "a refused patch is not applied");
}

/// §Identifying Patches: "A missing prev header means it is the first patch
/// in a log and the log would need to be empty"; prev "must match the id of
/// the current latest log entry. If there is a mismatch … the patch is
/// rejected." A rejected patch changes nothing.
#[tokio::test]
async fn prev_must_name_the_latest_entry_and_only_the_first_has_none() {
    let (state, token, app) = log_dataset(None);
    let g = LG;
    let (st, txt) = append(
        &app,
        &token,
        "H id <urn:uuid:prev-1> .\nH prev <urn:uuid:nothing> .\nTX .\nTC .\n",
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "prev on an empty log: {txt}");
    let (st, txt) = append(
        &app,
        &token,
        &format!("H id <urn:uuid:prev-1> .\nTX .\nA <urn:a> <urn:p> \"1\" <{g}> .\nTC .\n"),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(txt.contains("\"version\":1"), "{txt}");
    let (st, txt) = append(
        &app,
        &token,
        &format!("H id <urn:uuid:prev-2> .\nTX .\nA <urn:b> <urn:p> \"1\" <{g}> .\nTC .\n"),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "no prev on a non-empty log: {txt}"
    );
    assert!(txt.contains("urn:uuid:prev-1"), "names the latest: {txt}");
    let (st, txt) = append(
        &app,
        &token,
        &format!(
            "H id <urn:uuid:prev-2> .\nH prev <urn:uuid:other> .\nTX .\nA <urn:b> <urn:p> \"1\" <{g}> .\nTC .\n"
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{txt}");
    assert!(!holds(&state, "urn:b"), "a rejected patch is not applied");
    let (st, txt) = append(
        &app,
        &token,
        &format!(
            "H id <urn:uuid:prev-2> .\nH prev <urn:uuid:prev-1> .\nTX .\nA <urn:b> <urn:p> \"1\" <{g}> .\nTC .\n"
        ),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{txt}");
    assert!(txt.contains("\"version\":2"), "{txt}");
    assert!(holds(&state, "urn:b"));
}

/// §Identifying Patches: the id is "any global unique identifier": a log
/// holds an id once.
#[tokio::test]
async fn an_id_is_appended_once() {
    let (_state, token, app) = log_dataset(None);
    let (st, _) = append(&app, &token, "H id <urn:uuid:dup> .\nTX .\nTC .\n").await;
    assert_eq!(st, StatusCode::OK);
    let (st, txt) = append(
        &app,
        &token,
        "H id <urn:uuid:dup> .\nH prev <urn:uuid:dup> .\nTX .\nTC .\n",
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{txt}");
}

/// §Naming: `/patch/{version}`, `/patch/{id}` and `/current` serve a patch;
/// "A patch in a log is never changed once appended"; versions go up.
#[tokio::test]
async fn patches_are_served_by_version_id_and_current_unchanged() {
    let (_state, token, app) = log_dataset(None);
    let first = format!(
        "H id <urn:uuid:6f1d2c3e-0000-4000-8000-000000000001> .\nTX .\nA <urn:a> <urn:p> \"x\" <{LG}> .\nTC .\n"
    );
    let second = format!(
        "H id <urn:uuid:6f1d2c3e-0000-4000-8000-000000000002> .\nH prev <urn:uuid:6f1d2c3e-0000-4000-8000-000000000001> .\nTX .\nD <urn:a> <urn:p> \"x\" <{LG}> .\nTC .\n"
    );
    assert_eq!(append(&app, &token, &first).await.0, StatusCode::OK);
    assert_eq!(append(&app, &token, &second).await.0, StatusCode::OK);
    for (uri, want) in [
        ("/api/datasets/log/log/patch/1", &first),
        (
            "/api/datasets/log/log/patch/6f1d2c3e-0000-4000-8000-000000000001",
            &first,
        ),
        (
            "/api/datasets/log/log/patch/urn:uuid:6f1d2c3e-0000-4000-8000-000000000002",
            &second,
        ),
        ("/api/datasets/log/log/patch/2", &second),
        ("/api/datasets/log/log/current", &second),
    ] {
        let (st, txt) = call(&app, Method::GET, uri, Some(&token), None, "").await;
        assert_eq!(st, StatusCode::OK, "{uri}: {txt}");
        assert_eq!(&txt, want, "{uri}");
    }
    let (st, _) = call(
        &app,
        Method::GET,
        "/api/datasets/log/log/patch/3",
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// §Applying Patches: "Given a known starting state … applying the log one
/// or more times will result in the same new state." `/init` is that
/// starting state (version 0); replaying every entry onto it, once or
/// twice, gives the dataset — version-cut entries included.
#[tokio::test]
async fn replaying_the_log_from_init_reproduces_the_dataset() {
    let (state, token, app) = log_dataset(Some(
        "<urn:a> <urn:p> \"1\" . <urn:b> <urn:p> \"2\" . _:n <urn:p> \"3\" .",
    ));
    let bnode = state
        .store
        .quads_for_graph(GraphNameRef::NamedNode(
            NamedNode::new(LG).unwrap().as_ref(),
        ))
        .unwrap()
        .into_iter()
        .find_map(|q| match q.subject {
            NamedOrBlankNode::BlankNode(b) => Some(b),
            _ => None,
        })
        .unwrap();
    let patches = [
        format!("H id <urn:uuid:r1> .\nTX .\nD <urn:a> <urn:p> \"1\" <{LG}> .\nA <urn:c> <urn:p> \"3\" <{LG}> .\nTC .\n"),
        format!(
            "H id <urn:uuid:r2> .\nH prev <urn:uuid:r1> .\nTX .\nD _:{} <urn:p> \"3\" <{LG}> .\nA <urn:d> <urn:p> \"4\" <{LG}> .\nTC .\n",
            bnode.as_str()
        ),
    ];
    for p in &patches {
        let (st, txt) = append(&app, &token, p).await;
        assert_eq!(st, StatusCode::OK, "{txt}");
    }
    // A version cut is journaled too, chained after the last patch.
    let (st, txt) = call(
        &app,
        Method::POST,
        "/api/datasets/log/versions",
        Some(&token),
        Some("application/json"),
        r#"{"version":"1.0.0"}"#,
    )
    .await;
    assert!(st.is_success(), "{txt}");
    let (st, log) = call(
        &app,
        Method::GET,
        "/api/datasets/log/log",
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{log}");
    let log: serde_json::Value = serde_json::from_str(&log).unwrap();
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3, "{log}");
    assert_eq!(entries[2]["kind"], "version");
    assert_eq!(entries[2]["prev"], "urn:uuid:r2");
    assert!(
        log["init"]["version"].is_string(),
        "a non-empty dataset's version 0 is a version: {log}"
    );

    let (st, init) = call(
        &app,
        Method::GET,
        "/api/datasets/log/log/init",
        Some(&token),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{init}");
    let live: HashSet<Quad> = state
        .store
        .quads_for_graph(GraphNameRef::NamedNode(
            NamedNode::new(LG).unwrap().as_ref(),
        ))
        .unwrap()
        .into_iter()
        .collect();
    for rounds in [1, 2] {
        let replica = TripleStore::in_memory().unwrap();
        replica.load_str(&init, RdfFormat::TriG, None).unwrap();
        // Blank nodes keep the store's ids only through a patch: load the
        // starting state's blank node under the id the dataset gave it.
        let parsed: Vec<Quad> = replica
            .quads_for_graph(GraphNameRef::NamedNode(
                NamedNode::new(LG).unwrap().as_ref(),
            ))
            .unwrap();
        for q in parsed {
            if let NamedOrBlankNode::BlankNode(_) = q.subject {
                replica
                    .apply_quad_ops(&[
                        QuadOp::Remove(q.clone()),
                        QuadOp::Add(Quad::new(
                            bnode.clone(),
                            q.predicate,
                            q.object,
                            q.graph_name,
                        )),
                    ])
                    .unwrap();
            }
        }
        for _ in 0..rounds {
            for v in 1..=3 {
                let (st, text) = call(
                    &app,
                    Method::GET,
                    &format!("/api/datasets/log/log/patch/{v}"),
                    Some(&token),
                    None,
                    "",
                )
                .await;
                assert_eq!(st, StatusCode::OK, "{text}");
                let p = parse(&text).unwrap();
                replica.apply_quad_ops(&p.quad_ops(None).unwrap()).unwrap();
            }
        }
        let got: HashSet<Quad> = replica
            .quads_for_graph(GraphNameRef::NamedNode(
                NamedNode::new(LG).unwrap().as_ref(),
            ))
            .unwrap()
            .into_iter()
            .collect();
        assert_eq!(got, live, "{rounds} replay(s)");
    }
}
