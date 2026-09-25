//! Digests that tie a stored graph to the bundled file it was loaded from, so
//! the seeder states "unchanged" only for a copy it has checked.
//!
//! * [`file_sha256`]: the SHA-256 of a bundled file's bytes. Cheap and needs
//!   no parse; it tells the seeder whether this build ships the very file an
//!   earlier boot checked a stored copy against.
//! * [`triples_digest`]: a SHA-256 over a graph's triples as the store holds
//!   them (sorted N-Triples lines, blank node labels as stored). The seeder
//!   records it for every graph it loads or checks, so a later check can tell
//!   cheaply whether a graph still holds exactly the content it vouched for
//!   ([`graphs_digest`] for content spread over several graphs).
//! * [`same_triples`]: whether two triple sets are the same graph, blank nodes
//!   compared up to renaming (a fresh parse names them anew).
//! * [`as_stored`]: a file's triples as the store holds them. Oxigraph keeps
//!   typed literals in a canonical form (`"1"^^xsd:nonNegativeInteger` becomes
//!   `"1"^^xsd:integer`, a `+00:00` time zone becomes `Z`), so no stored copy
//!   is closer to a file than this, and a copy is compared with it.
//!
//! All of them feed registry metadata; nothing is written into a graph.

use oxigraph::model::{Graph, GraphName, GraphNameRef, NamedNodeRef, Quad, Term, Triple};
use sha2::{Digest, Sha256};

use crate::store::engine::StoreError;
use crate::store::TripleStore;

/// Hex SHA-256 of a bundled file's bytes.
pub fn file_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The triples of one named graph, as the store holds them.
pub fn graph_triples(store: &TripleStore, graph_iri: &str) -> Result<Vec<Triple>, StoreError> {
    let nn = NamedNodeRef::new(graph_iri)
        .map_err(|e| StoreError::Parse(format!("invalid graph IRI {graph_iri}: {e}")))?;
    Ok(store
        .quads_for_graph(GraphNameRef::NamedNode(nn))?
        .into_iter()
        .map(Triple::from)
        .collect())
}

/// `triples` as this store holds them: loaded into a throw-away in-memory
/// store and read back, so typed literals take the store's canonical form.
pub fn as_stored(triples: &[Triple]) -> Result<Vec<Triple>, StoreError> {
    let tmp = oxigraph::store::Store::new()?;
    tmp.extend(
        triples
            .iter()
            .map(|t| t.clone().in_graph(GraphName::DefaultGraph)),
    )?;
    tmp.iter()
        .map(|q| q.map(Triple::from).map_err(StoreError::from))
        .collect()
}

/// The triples of parsed quads, graph names dropped (the seeder merges a
/// file into one graph).
pub fn quads_as_triples(quads: &[Quad]) -> Vec<Triple> {
    quads.iter().cloned().map(Triple::from).collect()
}

/// Hex SHA-256 over the sorted, de-duplicated N-Triples lines of `triples`.
/// Blank nodes count by their labels, so two digests match only for the same
/// stored graph (or the same parse), not for a re-parse of the same file.
pub fn triples_digest(triples: &[Triple]) -> String {
    let mut lines: Vec<String> = triples.iter().map(Triple::to_string).collect();
    lines.sort_unstable();
    lines.dedup();
    let mut h = Sha256::new();
    for l in &lines {
        h.update(l.as_bytes());
        h.update(b"\n");
    }
    hex::encode(h.finalize())
}

/// The graphs holding a version's content: its base graph and its
/// sub-graphs, each once, base graph first.
pub fn version_graphs(graph_iri: &str, sub_graphs: &[String]) -> Vec<String> {
    let mut out = vec![graph_iri.to_string()];
    for g in sub_graphs {
        if !out.contains(g) {
            out.push(g.clone());
        }
    }
    out
}

/// The digest of content stored in `graphs`: [`triples_digest`] of the one
/// graph, or, for content spread over several, a SHA-256 over each graph's
/// IRI and digest, graphs in IRI order (so the order the registry lists them
/// in does not matter).
pub fn graphs_digest(store: &TripleStore, graphs: &[String]) -> Result<String, StoreError> {
    if let [one] = graphs {
        return Ok(triples_digest(&graph_triples(store, one)?));
    }
    let mut sorted: Vec<&String> = graphs.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut h = Sha256::new();
    for g in sorted {
        h.update(g.as_bytes());
        h.update(b"\t");
        h.update(triples_digest(&graph_triples(store, g)?).as_bytes());
        h.update(b"\n");
    }
    Ok(hex::encode(h.finalize()))
}

/// Whether a triple may involve a blank node (a triple term counts as one, to
/// be safe: canonicalizing is always correct, only slower).
fn has_blank_node(t: &Triple) -> bool {
    match &t.object {
        Term::BlankNode(_) => true,
        #[cfg(feature = "rdf-12")]
        Term::Triple(_) => true,
        _ => t.subject.is_blank_node(),
    }
}

/// Whether `a` and `b` are the same set of triples, blank nodes compared up to
/// renaming. Graphs without blank nodes are compared as sets; the others after
/// canonicalizing their blank node labels.
pub fn same_triples(a: &[Triple], b: &[Triple]) -> bool {
    let mut ga: Graph = a.iter().cloned().collect();
    let mut gb: Graph = b.iter().cloned().collect();
    if ga.len() != gb.len() {
        return false;
    }
    if a.iter().any(has_blank_node) || b.iter().any(has_blank_node) {
        use oxrdf::dataset::CanonicalizationAlgorithm;
        ga.canonicalize(CanonicalizationAlgorithm::Unstable);
        gb.canonicalize(CanonicalizationAlgorithm::Unstable);
    }
    ga == gb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_models::upload;

    fn parse(ttl: &str) -> Vec<Triple> {
        quads_as_triples(&upload::parse_rdf(ttl.as_bytes(), "text/turtle", "t.ttl").unwrap())
    }

    const TTL: &str = "@prefix ex: <http://ex.org/> .\n\
                       ex:a ex:p [ ex:q \"x\" ] .\n\
                       ex:a ex:r \"y\"@en .\n";

    /// Two parses of one file are the same graph, though each names its blank
    /// nodes anew; one changed literal makes them differ.
    #[test]
    fn same_triples_ignores_blank_node_labels() {
        let (a, b) = (parse(TTL), parse(TTL));
        assert!(same_triples(&a, &b));
        assert_ne!(triples_digest(&a), triples_digest(&b), "labels differ");
        let c = parse(&TTL.replace("\"x\"", "\"z\""));
        assert!(!same_triples(&a, &c));
        let d = parse(&TTL.replace("\"y\"@en", "\"y\"@nl"));
        assert!(!same_triples(&a, &d));
    }

    /// The digest of what the store holds equals the digest of the quads it
    /// was given when they hold no typed literal the store rewrites: blank
    /// node labels survive the store.
    #[test]
    fn a_stored_graph_digests_like_the_quads_loaded_into_it() {
        let store = TripleStore::in_memory().unwrap();
        let quads = upload::parse_rdf(TTL.as_bytes(), "text/turtle", "t.ttl").unwrap();
        let loaded =
            upload::load_parsed_verbatim(&store, "http://base", "m", "1", quads.clone(), true)
                .unwrap();
        let stored = graph_triples(&store, &loaded.sub_graphs[0]).unwrap();
        assert_eq!(
            triples_digest(&stored),
            triples_digest(&quads_as_triples(&quads))
        );
    }

    /// The store writes some typed literals in canonical form; `as_stored`
    /// predicts exactly what it holds.
    #[test]
    fn as_stored_predicts_the_stores_literal_forms() {
        let ttl = "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                   <http://ex.org/a> <http://ex.org/n> \"1\"^^xsd:nonNegativeInteger ;\n\
                   <http://ex.org/t> \"2014-08-28T15:00:00+00:00\"^^xsd:dateTime ;\n\
                   <http://ex.org/s> \"x\"@nl .\n";
        let parsed = parse(ttl);
        let predicted = as_stored(&parsed).unwrap();
        assert!(
            !same_triples(&parsed, &predicted),
            "the store changes the form"
        );
        let store = TripleStore::in_memory().unwrap();
        let quads = upload::parse_rdf(ttl.as_bytes(), "text/turtle", "t.ttl").unwrap();
        let loaded =
            upload::load_parsed_verbatim(&store, "http://base", "m", "1", quads, true).unwrap();
        let stored = graph_triples(&store, &loaded.sub_graphs[0]).unwrap();
        assert!(same_triples(&stored, &predicted));
        assert!(stored.iter().any(|t| t
            .to_string()
            .ends_with("\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>")));
    }

    #[test]
    fn file_sha256_is_hex() {
        assert_eq!(
            file_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
