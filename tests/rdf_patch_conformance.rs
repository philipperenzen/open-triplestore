//! RDF Patch conformance, one test per clause of the format page
//! (<https://afs.github.io/rdf-delta/rdf-patch.html>): the parser in
//! `open_triplestore::rdf_patch` and the store's transactional quad path,
//! `TripleStore::apply_quad_ops`, that a dataset patch is applied through.
//! The HTTP endpoint is covered in `tests/rdf_patch_http.rs`.

use std::collections::HashSet;

use open_triplestore::rdf_patch::{generate, parse, Op, Patch};
use open_triplestore::store::{QuadOp, TripleStore};
use oxigraph::io::RdfFormat;
use oxigraph::model::{BlankNode, GraphNameRef, NamedNode, NamedOrBlankNode, Quad, Term};

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
    let text = generate(
        &store,
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
