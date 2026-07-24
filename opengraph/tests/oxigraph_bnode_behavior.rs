//! Empirical probe: does Oxigraph preserve the *label* of a blank node across an
//! insert → query round-trip, and across a store reopen? This determines which
//! durable-blank-node mode the backend should use:
//!
//! * If labels are preserved, importing with stable canonical labels is enough.
//! * If Oxigraph re-labels blank nodes internally, only Skolemization (turning
//!   them into real IRIs) can survive the round-trip — IRIs are always preserved.

use opengraph::oxigraph::store::Store;
use opengraph::oxrdf::{BlankNode, GraphName, NamedNode, NamedOrBlankNode, Quad, Term};

fn probe_quad(label: &str) -> Quad {
    Quad::new(
        NamedOrBlankNode::BlankNode(BlankNode::new_unchecked(label)),
        NamedNode::new("http://ex/p").unwrap(),
        Term::NamedNode(NamedNode::new("http://ex/o").unwrap()),
        GraphName::DefaultGraph,
    )
}

#[test]
fn observe_blank_node_label_roundtrip() {
    let store = Store::new().unwrap();
    store.insert(&probe_quad("probe123")).unwrap();

    let quads: Vec<Quad> = store.iter().map(|r| r.unwrap()).collect();
    assert_eq!(quads.len(), 1);

    let label = match &quads[0].subject {
        NamedOrBlankNode::BlankNode(b) => b.as_str().to_string(),
        other => panic!("expected blank node, got {other:?}"),
    };
    println!("OXIGRAPH_BNODE_ROUNDTRIP_LABEL = {label}");
    println!("OXIGRAPH_PRESERVES_LABEL = {}", label == "probe123");
}

#[test]
fn observe_skolem_iri_roundtrip() {
    // An IRI must always survive verbatim — the durability guarantee Skolem relies on.
    let store = Store::new().unwrap();
    let iri = "https://data.example.org/.well-known/genid/abc123";
    store
        .insert(&Quad::new(
            NamedOrBlankNode::NamedNode(NamedNode::new(iri).unwrap()),
            NamedNode::new("http://ex/p").unwrap(),
            Term::NamedNode(NamedNode::new("http://ex/o").unwrap()),
            GraphName::DefaultGraph,
        ))
        .unwrap();
    let quads: Vec<Quad> = store.iter().map(|r| r.unwrap()).collect();
    let s = match &quads[0].subject {
        NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
        other => panic!("expected IRI, got {other:?}"),
    };
    println!("OXIGRAPH_IRI_ROUNDTRIP = {s}");
    assert_eq!(s, iri, "IRIs must round-trip verbatim");
}
