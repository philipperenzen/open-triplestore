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
//! * [`as_stored`]: a file's triples as the store holds them, the form a copy
//!   is compared in. The store keeps every literal as written (the vendored
//!   Oxigraph, `vendor/README.md`), so this is the file's own triples.
//! * [`compare_copy`]: whether a stored copy is a file's triples, either
//!   exactly or in the form an older store wrote them ([`earlier_store_form`]:
//!   before the store kept lexical forms it wrote typed literals in a
//!   canonical form, so `"1"^^xsd:nonNegativeInteger` read back as
//!   `"1"^^xsd:integer` and a `+00:00` time zone as `Z`). The seeders use it
//!   to recognise, and restore, copies an older version stored.
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
/// store and read back. The store keeps literals as written, so this returns
/// the same triples; comparing in this form keeps every check right should
/// the store ever write a term differently from how it was given.
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

/// `triples` in the form the store wrote them before it kept lexical forms
/// (Oxigraph 0.5.11 unpatched; `vendor/README.md`): every literal of a
/// datatype it stored as a value — `xsd:boolean`, the numeric types, the
/// twelve types derived from `xsd:integer`, the date, time and duration types
/// and `xsd:dateTimeStamp` — in the canonical form of its value under the
/// primitive type. A lexical form that does not parse stays as it is, as it
/// did then.
pub fn earlier_store_form(triples: &[Triple]) -> Vec<Triple> {
    triples
        .iter()
        .map(|t| {
            Triple::new(
                t.subject.clone(),
                t.predicate.clone(),
                earlier_term(&t.object),
            )
        })
        .collect()
}

fn earlier_term(term: &Term) -> Term {
    match term {
        Term::Literal(l) => earlier_literal(l).map_or_else(|| term.clone(), Term::Literal),
        #[cfg(feature = "rdf-12")]
        Term::Triple(t) => Term::Triple(Box::new(Triple::new(
            t.subject.clone(),
            t.predicate.clone(),
            earlier_term(&t.object),
        ))),
        _ => term.clone(),
    }
}

/// The canonical literal the earlier store held for `l`, or `None` when it
/// held `l` itself.
fn earlier_literal(l: &oxigraph::model::Literal) -> Option<oxigraph::model::Literal> {
    use oxigraph::model::Literal;
    use oxsdatatypes::*;
    const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
    let local = l.datatype().as_str().strip_prefix(XSD)?;
    let v = l.value();
    let canonical = match local {
        "boolean" => Literal::from(v.parse::<Boolean>().ok()?),
        "float" => Literal::from(v.parse::<Float>().ok()?),
        "double" => Literal::from(v.parse::<Double>().ok()?),
        "integer" | "byte" | "short" | "int" | "long" | "unsignedByte" | "unsignedShort"
        | "unsignedInt" | "unsignedLong" | "positiveInteger" | "negativeInteger"
        | "nonPositiveInteger" | "nonNegativeInteger" => Literal::from(v.parse::<Integer>().ok()?),
        "decimal" => Literal::from(v.parse::<Decimal>().ok()?),
        "dateTime" | "dateTimeStamp" => Literal::from(v.parse::<DateTime>().ok()?),
        "time" => Literal::from(v.parse::<Time>().ok()?),
        "date" => Literal::from(v.parse::<Date>().ok()?),
        "gYearMonth" => Literal::from(v.parse::<GYearMonth>().ok()?),
        "gYear" => Literal::from(v.parse::<GYear>().ok()?),
        "gMonthDay" => Literal::from(v.parse::<GMonthDay>().ok()?),
        "gDay" => Literal::from(v.parse::<GDay>().ok()?),
        "gMonth" => Literal::from(v.parse::<GMonth>().ok()?),
        "duration" => Literal::from(v.parse::<Duration>().ok()?),
        "yearMonthDuration" => Literal::from(v.parse::<YearMonthDuration>().ok()?),
        "dayTimeDuration" => Literal::from(v.parse::<DayTimeDuration>().ok()?),
        _ => return None,
    };
    (canonical != *l).then_some(canonical)
}

/// How a stored copy relates to the triples of the file it was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyForm {
    /// Exactly the file's triples.
    Exact,
    /// The file's triples in the form an older store wrote them
    /// ([`earlier_store_form`]), and not exactly: the copy is the file's
    /// content, loaded before the store kept lexical forms, and loading the
    /// file again makes it exact.
    EarlierStore,
    /// Other triples: an edit, another file, a partial load.
    Differs,
}

/// Compare a stored copy with a file's triples (see [`CopyForm`]).
pub fn compare_copy(stored: &[Triple], file: &[Triple]) -> CopyForm {
    if same_triples(stored, file) {
        return CopyForm::Exact;
    }
    let earlier = earlier_store_form(file);
    if earlier != file && same_triples(stored, &earlier) {
        CopyForm::EarlierStore
    } else {
        CopyForm::Differs
    }
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

    /// The store keeps typed literals as written: `as_stored` is the file's
    /// own triples, and a stored copy is exactly the file.
    #[test]
    fn the_store_keeps_typed_literals_as_written() {
        let ttl = "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                   <http://ex.org/a> <http://ex.org/n> \"1\"^^xsd:nonNegativeInteger ;\n\
                   <http://ex.org/t> \"2014-08-28T15:00:00+00:00\"^^xsd:dateTime ;\n\
                   <http://ex.org/s> \"x\"@nl .\n";
        let parsed = parse(ttl);
        assert!(same_triples(&parsed, &as_stored(&parsed).unwrap()));
        let store = TripleStore::in_memory().unwrap();
        let quads = upload::parse_rdf(ttl.as_bytes(), "text/turtle", "t.ttl").unwrap();
        let loaded =
            upload::load_parsed_verbatim(&store, "http://base", "m", "1", quads, true).unwrap();
        let stored = graph_triples(&store, &loaded.sub_graphs[0]).unwrap();
        assert_eq!(compare_copy(&stored, &parsed), CopyForm::Exact);
        assert!(stored.iter().any(|t| t
            .to_string()
            .ends_with("\"1\"^^<http://www.w3.org/2001/XMLSchema#nonNegativeInteger>")));
    }

    /// A copy an older store wrote (typed literals in canonical form) is
    /// recognised as the file's content; any other difference is not.
    #[test]
    fn a_copy_in_the_earlier_store_form_is_recognised() {
        let ttl = "@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                   <http://ex.org/a> <http://ex.org/n> \"1\"^^xsd:nonNegativeInteger ;\n\
                   <http://ex.org/t> \"2014-08-28T15:00:00+00:00\"^^xsd:dateTime ;\n\
                   <http://ex.org/b> \"1\"^^xsd:boolean ;\n\
                   <http://ex.org/x> \"not a number\"^^xsd:integer ;\n\
                   <http://ex.org/s> \"x\"@nl .\n";
        let file = parse(ttl);
        let earlier = earlier_store_form(&file);
        let lines: Vec<String> = earlier.iter().map(Triple::to_string).collect();
        for want in [
            "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            "\"2014-08-28T15:00:00Z\"^^<http://www.w3.org/2001/XMLSchema#dateTime>",
            "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>",
            "\"not a number\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            "\"x\"@nl",
        ] {
            assert!(lines.iter().any(|l| l.ends_with(want)), "{want}: {lines:?}");
        }
        assert_eq!(compare_copy(&earlier, &file), CopyForm::EarlierStore);
        assert_eq!(compare_copy(&file, &file), CopyForm::Exact);
        let edited = parse(&ttl.replace("\"x\"@nl", "\"y\"@nl"));
        assert_eq!(
            compare_copy(&earlier_store_form(&edited), &file),
            CopyForm::Differs
        );
        // A file without such literals has no earlier form to match.
        let plain = parse(TTL);
        assert_eq!(compare_copy(&plain, &plain), CopyForm::Exact);
    }

    #[test]
    fn file_sha256_is_hex() {
        assert_eq!(
            file_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
