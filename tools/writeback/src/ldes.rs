//! An LDES fragment read forwards: the members it carries, and where the
//! stream continues.
//!
//! The store's fragments are `tree:Node`s of an `ldes:EventStream`. Each
//! `tree:member` is a version object: `dct:isVersionOf` the entity,
//! `dct:created` the moment, the entity's properties re-subjected onto the
//! member, or `a ots:Tombstone` when the entity disappeared. A node relates
//! to the next through `tree:relation` / `tree:node`; the stream description
//! points at the first node through `tree:view`.

use oxrdf::{NamedOrBlankNode, Term, Triple};

const TREE: &str = "https://w3id.org/tree#";
const DCT: &str = "http://purl.org/dc/terms/";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const TOMBSTONE: &str = "https://opentriplestore.org/ns#Tombstone";
const XSD_BOOLEAN: &str = "http://www.w3.org/2001/XMLSchema#boolean";

/// One member: what changed about one entity.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberChange {
    /// The member's number in the stream, from its IRI (`…/members/<n>`);
    /// 0 when the IRI does not end that way.
    pub member_id: i64,
    pub member_iri: String,
    pub entity: String,
    pub created_at: String,
    pub deleted: bool,
    /// The entity's properties at that moment, in document order.
    pub properties: Vec<(String, Term)>,
}

impl MemberChange {
    pub fn types(&self) -> Vec<String> {
        self.properties
            .iter()
            .filter(|(p, _)| p == RDF_TYPE)
            .filter_map(|(_, o)| match o {
                Term::NamedNode(n) => Some(n.as_str().to_string()),
                _ => None,
            })
            .collect()
    }

    /// The first value under `predicate`, as the text a column takes: a
    /// literal's lexical form, an IRI as itself. A blank node has no text.
    /// A boolean becomes `1` / `0`, which every target reads as a boolean
    /// and an integer column stores as a flag.
    pub fn value_of(&self, predicate: &str) -> Option<String> {
        self.properties
            .iter()
            .filter(|(p, _)| p == predicate)
            .find_map(|(_, o)| match o {
                Term::Literal(l) if l.datatype().as_str() == XSD_BOOLEAN => Some(match l.value() {
                    "true" | "1" => "1".to_string(),
                    "false" | "0" => "0".to_string(),
                    other => other.to_string(),
                }),
                Term::Literal(l) => Some(l.value().to_string()),
                Term::NamedNode(n) => Some(n.as_str().to_string()),
                _ => None,
            })
    }
}

/// A fragment, parsed.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Fragment {
    /// Members in stream order (by number, then by creation time).
    pub members: Vec<MemberChange>,
    /// The node the stream continues at, if any.
    pub next: Option<String>,
    /// The first node of the stream, when the document is its description.
    pub view: Option<String>,
}

/// The formats a fragment may arrive in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    NTriples,
    Turtle,
}

impl Format {
    /// From a `Content-Type`, defaulting to Turtle.
    pub fn of_content_type(ct: &str) -> Self {
        if ct.starts_with("application/n-triples") {
            Format::NTriples
        } else {
            Format::Turtle
        }
    }
}

pub fn parse_triples(text: &str, format: Format) -> Result<Vec<Triple>, String> {
    let mut out = Vec::new();
    match format {
        Format::NTriples => {
            for t in oxttl::NTriplesParser::new().for_reader(text.as_bytes()) {
                out.push(t.map_err(|e| format!("the fragment is not valid N-Triples: {e}"))?);
            }
        }
        Format::Turtle => {
            for t in oxttl::TurtleParser::new().for_reader(text.as_bytes()) {
                out.push(t.map_err(|e| format!("the fragment is not valid Turtle: {e}"))?);
            }
        }
    }
    Ok(out)
}

fn node_of(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(NamedOrBlankNode::NamedNode(n.clone())),
        Term::BlankNode(b) => Some(NamedOrBlankNode::BlankNode(b.clone())),
        _ => None,
    }
}

fn iri_of(t: &Term) -> Option<String> {
    match t {
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    }
}

/// The trailing number of a member IRI.
pub fn member_number(iri: &str) -> i64 {
    iri.rsplit('/')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// Read a fragment's members and relations.
pub fn parse_fragment(text: &str, format: Format) -> Result<Fragment, String> {
    let triples = parse_triples(text, format)?;
    let mut fragment = Fragment::default();

    let member_pred = format!("{TREE}member");
    let member_nodes: Vec<NamedOrBlankNode> = triples
        .iter()
        .filter(|t| t.predicate.as_str() == member_pred)
        .filter_map(|t| node_of(&t.object))
        .collect();
    let is_version_of = format!("{DCT}isVersionOf");
    let created = format!("{DCT}created");
    for m in member_nodes {
        let about: Vec<&Triple> = triples.iter().filter(|t| t.subject == m).collect();
        let Some(entity) = about
            .iter()
            .find(|t| t.predicate.as_str() == is_version_of)
            .and_then(|t| iri_of(&t.object))
        else {
            continue;
        };
        let created_at = about
            .iter()
            .find(|t| t.predicate.as_str() == created)
            .and_then(|t| match &t.object {
                Term::Literal(l) => Some(l.value().to_string()),
                _ => None,
            })
            .unwrap_or_default();
        let deleted = about.iter().any(|t| {
            t.predicate.as_str() == RDF_TYPE && iri_of(&t.object).as_deref() == Some(TOMBSTONE)
        });
        let properties: Vec<(String, Term)> = about
            .iter()
            .filter(|t| {
                t.predicate.as_str() != is_version_of
                    && t.predicate.as_str() != created
                    && !(t.predicate.as_str() == RDF_TYPE
                        && iri_of(&t.object).as_deref() == Some(TOMBSTONE))
            })
            .map(|t| (t.predicate.as_str().to_string(), t.object.clone()))
            .collect();
        let member_iri = match &m {
            NamedOrBlankNode::NamedNode(n) => n.as_str().to_string(),
            NamedOrBlankNode::BlankNode(b) => format!("_:{}", b.as_str()),
        };
        fragment.members.push(MemberChange {
            member_id: member_number(&member_iri),
            member_iri,
            entity,
            created_at,
            deleted,
            properties,
        });
    }
    fragment
        .members
        .sort_by(|a, b| (a.member_id, &a.created_at).cmp(&(b.member_id, &b.created_at)));

    // The next node: the first relation that names one.
    let relation = format!("{TREE}relation");
    let node_pred = format!("{TREE}node");
    for rel in triples
        .iter()
        .filter(|t| t.predicate.as_str() == relation)
        .filter_map(|t| node_of(&t.object))
    {
        if let Some(next) = triples
            .iter()
            .find(|t| t.subject == rel && t.predicate.as_str() == node_pred)
            .and_then(|t| iri_of(&t.object))
        {
            fragment.next = Some(next);
            break;
        }
    }
    let view = format!("{TREE}view");
    fragment.view = triples
        .iter()
        .find(|t| t.predicate.as_str() == view)
        .and_then(|t| iri_of(&t.object));
    Ok(fragment)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fragment in the store's own layout.
    pub const FRAGMENT: &str = r#"
<http://store/api/datasets/shop/ldes> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://w3id.org/ldes#EventStream> .
<http://store/api/datasets/shop/ldes> <https://w3id.org/tree#view> <http://store/api/datasets/shop/ldes/nodes/1> .
<http://store/api/datasets/shop/ldes/nodes/1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://w3id.org/tree#Node> .
<http://store/api/datasets/shop/ldes/nodes/1> <https://w3id.org/tree#relation> _:r1 .
_:r1 <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://w3id.org/tree#GreaterThanOrEqualToRelation> .
_:r1 <https://w3id.org/tree#node> <http://store/api/datasets/shop/ldes/nodes/2> .
<http://store/api/datasets/shop/ldes> <https://w3id.org/tree#member> <http://store/api/datasets/shop/ldes/members/12> .
<http://store/api/datasets/shop/ldes/members/12> <http://purl.org/dc/terms/isVersionOf> <http://example.org/products/product_7> .
<http://store/api/datasets/shop/ldes/members/12> <http://purl.org/dc/terms/created> "2026-01-02T00:00:00Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
<http://store/api/datasets/shop/ldes/members/12> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.org/ontology#Product> .
<http://store/api/datasets/shop/ldes/members/12> <http://example.org/ontology#name> "Bolt" .
<http://store/api/datasets/shop/ldes/members/12> <http://example.org/ontology#hasPrice> "0.25"^^<http://www.w3.org/2001/XMLSchema#decimal> .
<http://store/api/datasets/shop/ldes> <https://w3id.org/tree#member> <http://store/api/datasets/shop/ldes/members/11> .
<http://store/api/datasets/shop/ldes/members/11> <http://purl.org/dc/terms/isVersionOf> <http://example.org/products/product_8> .
<http://store/api/datasets/shop/ldes/members/11> <http://purl.org/dc/terms/created> "2026-01-01T00:00:00Z"^^<http://www.w3.org/2001/XMLSchema#dateTime> .
<http://store/api/datasets/shop/ldes/members/11> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://opentriplestore.org/ns#Tombstone> .
"#;

    #[test]
    fn a_fragment_yields_ordered_members_and_the_next_node() {
        let f = parse_fragment(FRAGMENT, Format::NTriples).unwrap();
        assert_eq!(f.members.len(), 2);
        assert_eq!(f.members[0].member_id, 11);
        assert!(f.members[0].deleted);
        assert_eq!(f.members[0].entity, "http://example.org/products/product_8");
        assert!(
            f.members[0].properties.is_empty(),
            "a tombstone carries nothing"
        );
        let bolt = &f.members[1];
        assert_eq!(bolt.member_id, 12);
        assert!(!bolt.deleted);
        assert_eq!(bolt.created_at, "2026-01-02T00:00:00Z");
        assert_eq!(bolt.types(), vec!["http://example.org/ontology#Product"]);
        assert_eq!(
            bolt.value_of("http://example.org/ontology#name").as_deref(),
            Some("Bolt")
        );
        assert_eq!(
            bolt.value_of("http://example.org/ontology#hasPrice")
                .as_deref(),
            Some("0.25")
        );
        assert_eq!(bolt.value_of("http://example.org/ontology#colour"), None);
        assert_eq!(
            f.next.as_deref(),
            Some("http://store/api/datasets/shop/ldes/nodes/2")
        );
        assert_eq!(
            f.view.as_deref(),
            Some("http://store/api/datasets/shop/ldes/nodes/1")
        );
        assert_eq!(member_number("http://x/members/42"), 42);
        assert_eq!(member_number("http://x/members/"), 0);
        assert_eq!(
            Format::of_content_type("application/n-triples; charset=utf-8"),
            Format::NTriples
        );
        assert_eq!(Format::of_content_type("text/turtle"), Format::Turtle);
        assert!(parse_fragment("<a> <b>", Format::NTriples).is_err());
    }
}
