//! LDP `PATCH` confined to the target resource.
//!
//! The body is SPARQL Update, but it is not run against the store. It is
//! parsed and restricted to `INSERT DATA`, `DELETE DATA` and
//! `DELETE/INSERT … WHERE` on the default graph (no `GRAPH`, `LOAD`, `CLEAR`,
//! `CREATE`, `DROP`, `SERVICE`, `WITH`, `USING`), then evaluated on a scratch
//! store holding only the resource's own editable triples. The `WHERE` clause
//! therefore sees nothing else in the store, and the result is checked before
//! it is written back as a difference: no triple may have another LDP resource
//! (an IRI under `{base}/ldp/`) as its subject, and none may be server-managed
//! (`ldp:*` predicates, `rdf:type ldp:*`, the Non-RDF Source storage triples).
//! Containment and membership triples are never in the scratch store and never
//! change. Triples about subjects outside `/ldp/` are allowed, as they are in a
//! `PUT` body: an LDP-RS may describe related things.
//!
//! Triples whose object is a blank node stay out of the scratch store too: a
//! blank node has no identity across the round trip, so moving one through it
//! would re-mint it on every `PATCH` and orphan whatever else describes it. A
//! `PATCH` can add blank-node structure; removing it takes a `PUT`.
//!
//! Whatever the body says, a `PATCH` can change the target resource and
//! nothing another WAC check protects.

use std::collections::BTreeSet;

use oxigraph::model::{NamedNode, Term};
use oxigraph::sparql::QueryResults;
use spargebra::algebra::GraphPattern;
use spargebra::term::{
    GraphName, GraphNamePattern, GroundTerm, GroundTermPattern, NamedNodePattern, TermPattern,
};
use spargebra::{GraphUpdateOperation, SparqlParser};

use crate::store::TripleStore;

const LDP_NS: &str = "http://www.w3.org/ns/ldp#";
const LDP_INTERNAL_NS: &str = "urn:ldp:";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// Why a `PATCH` was not applied.
#[derive(Debug)]
pub enum PatchError {
    /// The update does not parse, or fails while it runs on the scratch store: 400.
    BadRequest(String),
    /// The update is of a shape `PATCH` does not allow, or its result reaches
    /// beyond the resource: 403.
    NotAllowed(String),
    /// The store failed: 500.
    Internal(String),
}

/// A triple about the resource that clients may not write or remove.
fn server_managed(predicate: &NamedNode, object: &Term) -> bool {
    let p = predicate.as_str();
    p.starts_with(LDP_NS)
        || p.starts_with(LDP_INTERNAL_NS)
        || (p == RDF_TYPE && matches!(object, Term::NamedNode(n) if n.as_str().starts_with(LDP_NS)))
}

/// The static form of [`server_managed`], for the update's own quads and
/// templates: a variable predicate cannot be judged here and is left to the
/// check of the result.
fn managed_static(predicate: &str, object_iri: Option<&str>) -> Result<(), String> {
    let managed = predicate.starts_with(LDP_NS)
        || predicate.starts_with(LDP_INTERNAL_NS)
        || (predicate == RDF_TYPE && object_iri.is_some_and(|o| o.starts_with(LDP_NS)));
    if managed {
        Err(format!(
            "PATCH may not change server-managed triples (<{predicate}>)"
        ))
    } else {
        Ok(())
    }
}

fn pattern_forbidden(p: &GraphPattern) -> Option<&'static str> {
    use GraphPattern as GP;
    // `FILTER EXISTS { GRAPH … }` / `{ SERVICE … }` reach as far as the bare
    // pattern would.
    if let Some(kw) = crate::sparql::exists_patterns(p)
        .into_iter()
        .find_map(pattern_forbidden)
    {
        return Some(kw);
    }
    match p {
        GP::Graph { .. } => Some("GRAPH"),
        GP::Service { .. } => Some("SERVICE"),
        GP::Bgp { .. } | GP::Path { .. } | GP::Values { .. } => None,
        GP::Join { left, right, .. }
        | GP::Lateral { left, right, .. }
        | GP::Union { left, right, .. }
        | GP::Minus { left, right, .. }
        | GP::LeftJoin { left, right, .. } => {
            pattern_forbidden(left).or_else(|| pattern_forbidden(right))
        }
        GP::Filter { inner, .. }
        | GP::Extend { inner, .. }
        | GP::OrderBy { inner, .. }
        | GP::Project { inner, .. }
        | GP::Distinct { inner, .. }
        | GP::Reduced { inner, .. }
        | GP::Slice { inner, .. }
        | GP::Group { inner, .. } => pattern_forbidden(inner),
    }
}

/// Whether `subject` is an LDP resource other than the target.
pub fn other_ldp_subject(subject: &str, resource_iri: &str, ldp_root: &str) -> bool {
    subject.starts_with(ldp_root)
        && crate::ldp::wac::canonical(subject) != crate::ldp::wac::canonical(resource_iri)
}

fn other_subject(subject: &str, resource_iri: &str, ldp_root: &str) -> Result<(), String> {
    if other_ldp_subject(subject, resource_iri, ldp_root) {
        Err(format!(
            "PATCH may only change <{resource_iri}>; the update names another resource, <{subject}>"
        ))
    } else {
        Ok(())
    }
}

/// Refuse every shape of update that could reach beyond the resource's own
/// triples in the default graph.
fn check_shape(
    update: &spargebra::Update,
    resource_iri: &str,
    ldp_root: &str,
) -> Result<(), String> {
    let default_only = |g: &GraphName| match g {
        GraphName::DefaultGraph => Ok(()),
        GraphName::NamedNode(n) => Err(format!(
            "PATCH may not name a graph (GRAPH <{}>)",
            n.as_str()
        )),
    };
    let default_only_pattern = |g: &GraphNamePattern| match g {
        GraphNamePattern::DefaultGraph => Ok(()),
        GraphNamePattern::NamedNode(n) => Err(format!(
            "PATCH may not name a graph (GRAPH <{}>)",
            n.as_str()
        )),
        GraphNamePattern::Variable(_) => Err("PATCH may not name a graph (GRAPH ?g)".to_string()),
    };
    for op in &update.operations {
        match op {
            GraphUpdateOperation::InsertData { data } => {
                for q in data {
                    default_only(&q.graph_name)?;
                    if let spargebra::term::NamedOrBlankNode::NamedNode(s) = &q.subject {
                        other_subject(s.as_str(), resource_iri, ldp_root)?;
                    }
                    let o = match &q.object {
                        spargebra::term::Term::NamedNode(n) => Some(n.as_str()),
                        _ => None,
                    };
                    managed_static(q.predicate.as_str(), o)?;
                }
            }
            GraphUpdateOperation::DeleteData { data } => {
                for q in data {
                    default_only(&q.graph_name)?;
                    other_subject(q.subject.as_str(), resource_iri, ldp_root)?;
                    let o = match &q.object {
                        GroundTerm::NamedNode(n) => Some(n.as_str()),
                        _ => None,
                    };
                    managed_static(q.predicate.as_str(), o)?;
                }
            }
            GraphUpdateOperation::DeleteInsert {
                delete,
                insert,
                using,
                pattern,
            } => {
                if using.is_some() {
                    return Err("PATCH may not use USING".to_string());
                }
                for q in delete {
                    default_only_pattern(&q.graph_name)?;
                    if let GroundTermPattern::NamedNode(s) = &q.subject {
                        other_subject(s.as_str(), resource_iri, ldp_root)?;
                    }
                    if let NamedNodePattern::NamedNode(p) = &q.predicate {
                        let o = match &q.object {
                            GroundTermPattern::NamedNode(n) => Some(n.as_str()),
                            _ => None,
                        };
                        managed_static(p.as_str(), o)?;
                    }
                }
                for q in insert {
                    default_only_pattern(&q.graph_name)?;
                    if let TermPattern::NamedNode(s) = &q.subject {
                        other_subject(s.as_str(), resource_iri, ldp_root)?;
                    }
                    if let NamedNodePattern::NamedNode(p) = &q.predicate {
                        let o = match &q.object {
                            TermPattern::NamedNode(n) => Some(n.as_str()),
                            _ => None,
                        };
                        managed_static(p.as_str(), o)?;
                    }
                }
                if let Some(kw) = pattern_forbidden(pattern) {
                    return Err(format!("PATCH may not use {kw}"));
                }
            }
            GraphUpdateOperation::Load { .. } => return Err("PATCH may not use LOAD".to_string()),
            GraphUpdateOperation::Clear { .. } => return Err("PATCH may not use CLEAR".to_string()),
            GraphUpdateOperation::Create { .. } => {
                return Err("PATCH may not use CREATE".to_string())
            }
            GraphUpdateOperation::Drop { .. } => return Err("PATCH may not use DROP".to_string()),
        }
    }
    Ok(())
}

/// Triples as N-Triples lines, the working form of a resource's state.
type Lines = BTreeSet<String>;

/// The editable triples `<resource_iri> ?p ?o` in `store`, as N-Triples lines:
/// not server-managed and not about a blank node.
fn editable_state(store: &TripleStore, resource_iri: &str) -> Result<Lines, String> {
    let q = format!("SELECT ?p ?o WHERE {{ <{resource_iri}> ?p ?o }}");
    let mut editable = Lines::new();
    if let QueryResults::Solutions(sols) = store.query(&q).map_err(|e| e.to_string())? {
        for sol in sols {
            let sol = sol.map_err(|e| e.to_string())?;
            let (Some(Term::NamedNode(p)), Some(o)) = (sol.get("p"), sol.get("o")) else {
                continue;
            };
            if !server_managed(p, o) && !matches!(o, Term::BlankNode(_)) {
                editable.insert(format!("<{resource_iri}> {p} {o} ."));
            }
        }
    }
    Ok(editable)
}

fn data_block(lines: &Lines) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

/// Apply `sparql` to `<resource_iri>` and nothing another resource's ACL
/// protects. `ldp_root` is `{base}/ldp/`. Returns whether anything changed.
pub fn apply(
    store: &TripleStore,
    resource_iri: &str,
    ldp_root: &str,
    sparql: &str,
) -> Result<bool, PatchError> {
    let parser = SparqlParser::new()
        .with_base_iri(resource_iri)
        .map_err(|e| PatchError::BadRequest(e.to_string()))?;
    let update = parser
        .parse_update(sparql)
        .map_err(|e| PatchError::BadRequest(format!("SPARQL Update syntax: {e}")))?;
    check_shape(&update, resource_iri, ldp_root).map_err(PatchError::NotAllowed)?;

    let before = editable_state(store, resource_iri).map_err(PatchError::Internal)?;

    // Evaluate on a scratch store that holds only the editable triples.
    let scratch = TripleStore::in_memory().map_err(|e| PatchError::Internal(e.to_string()))?;
    if !before.is_empty() {
        scratch
            .update(&format!("INSERT DATA {{ {} }}", data_block(&before)))
            .map_err(|e| PatchError::Internal(e.to_string()))?;
    }
    // The parsed form: relative IRIs (`<>` for the resource) are resolved.
    scratch
        .update(&update.to_string())
        .map_err(|e| PatchError::BadRequest(format!("SPARQL Update failed: {e}")))?;

    // Check the result before anything reaches the store.
    let mut after = Lines::new();
    let q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o }";
    if let QueryResults::Solutions(sols) = scratch
        .query(q)
        .map_err(|e| PatchError::Internal(e.to_string()))?
    {
        for sol in sols {
            let sol = sol.map_err(|e| PatchError::Internal(e.to_string()))?;
            let (Some(s), Some(Term::NamedNode(p)), Some(o)) =
                (sol.get("s"), sol.get("p"), sol.get("o"))
            else {
                continue;
            };
            if let Term::NamedNode(n) = s {
                if other_ldp_subject(n.as_str(), resource_iri, ldp_root) {
                    return Err(PatchError::NotAllowed(format!(
                        "PATCH may only change <{resource_iri}>; the result has triples about another resource, {s}"
                    )));
                }
            }
            if server_managed(p, o) {
                return Err(PatchError::NotAllowed(format!(
                    "PATCH may not change the server-managed triple {s} {p} {o}"
                )));
            }
            after.insert(format!("{s} {p} {o} ."));
        }
    }

    let removed: Lines = before.difference(&after).cloned().collect();
    let added: Lines = after.difference(&before).cloned().collect();
    if removed.is_empty() && added.is_empty() {
        return Ok(false);
    }
    let mut write = String::new();
    if !removed.is_empty() {
        write.push_str(&format!("DELETE DATA {{ {} }} ;\n", data_block(&removed)));
    }
    if !added.is_empty() {
        write.push_str(&format!("INSERT DATA {{ {} }} ;\n", data_block(&added)));
    }
    store
        .update(write.trim_end_matches(";\n"))
        .map_err(|e| PatchError::Internal(e.to_string()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "http://localhost/ldp/";
    const R: &str = "http://localhost/ldp/r";
    const OTHER: &str = "http://localhost/ldp/other";
    const OUTSIDE: &str = "http://example.org/thing";

    fn store() -> TripleStore {
        let s = TripleStore::in_memory().unwrap();
        s.update(&format!(
            "INSERT DATA {{ \
               <{R}> <http://example.org/p> \"v\" . \
               <{R}> a <http://www.w3.org/ns/ldp#RDFSource> . \
               <{R}> <http://www.w3.org/ns/ldp#contains> <{R}/child> . \
               <{OTHER}> <http://example.org/p> \"other\" . \
               GRAPH <http://victim/> {{ <http://s> <http://p> \"g\" }} }}"
        ))
        .unwrap();
        s
    }

    fn ask(s: &TripleStore, q: &str) -> bool {
        matches!(s.query(q), Ok(QueryResults::Boolean(true)))
    }

    fn unchanged_elsewhere(s: &TripleStore) {
        assert!(ask(
            s,
            &format!("ASK {{ <{OTHER}> <http://example.org/p> \"other\" }}")
        ));
        assert!(ask(
            s,
            "ASK { GRAPH <http://victim/> { <http://s> <http://p> \"g\" } }"
        ));
        assert!(ask(
            s,
            &format!("ASK {{ <{R}> a <http://www.w3.org/ns/ldp#RDFSource> }}")
        ));
        assert!(ask(
            s,
            &format!("ASK {{ <{R}> <http://www.w3.org/ns/ldp#contains> <{R}/child> }}")
        ));
    }

    #[test]
    fn allowed_shapes_change_only_the_resource() {
        let s = store();
        assert!(apply(
            &s,
            R,
            ROOT,
            &format!("INSERT DATA {{ <{R}> <http://example.org/q> 1 }}")
        )
        .unwrap());
        assert!(apply(
            &s,
            R,
            ROOT,
            "DELETE { <> <http://example.org/p> ?o } INSERT { <> <http://example.org/p> \"new\" } WHERE { <> <http://example.org/p> ?o }"
        )
        .unwrap());
        assert!(!apply(
            &s,
            R,
            ROOT,
            &format!("DELETE DATA {{ <{R}> <http://example.org/none> 1 }}")
        )
        .unwrap());
        assert!(ask(
            &s,
            &format!("ASK {{ <{R}> <http://example.org/p> \"new\" }}")
        ));
        assert!(!ask(
            &s,
            &format!("ASK {{ <{R}> <http://example.org/p> \"v\" }}")
        ));
        assert!(ask(
            &s,
            &format!("ASK {{ <{R}> <http://example.org/q> 1 }}")
        ));
        // Blank nodes and subjects outside /ldp/ are a resource's to describe.
        assert!(apply(
            &s,
            R,
            ROOT,
            &format!("INSERT DATA {{ <{R}> <http://example.org/p> [ <http://example.org/q> 1 ] . <{OUTSIDE}> <http://example.org/q> 2 }}")
        )
        .unwrap());
        assert!(ask(
            &s,
            &format!("ASK {{ <{R}> <http://example.org/p> ?b . ?b <http://example.org/q> 1 }}")
        ));
        assert!(ask(
            &s,
            &format!("ASK {{ <{OUTSIDE}> <http://example.org/q> 2 }}")
        ));
        // The WHERE clause sees only the resource's ground triples: a wildcard
        // delete empties those, nothing else; blank-node structure stays.
        assert!(apply(&s, R, ROOT, "DELETE { ?s ?p ?o } WHERE { ?s ?p ?o }").unwrap());
        assert!(!ask(
            &s,
            &format!("ASK {{ <{R}> <http://example.org/q> ?o }}")
        ));
        assert!(ask(
            &s,
            &format!("ASK {{ <{R}> <http://example.org/p> ?b . ?b <http://example.org/q> 1 }}")
        ));
        assert!(ask(
            &s,
            &format!("ASK {{ <{OUTSIDE}> <http://example.org/q> 2 }}")
        ));
        unchanged_elsewhere(&s);
    }

    #[test]
    fn forbidden_shapes_are_refused_and_change_nothing() {
        let s = store();
        for body in [
            format!("INSERT DATA {{ <{OTHER}> <http://example.org/p> \"x\" }}"),
            format!("DELETE WHERE {{ <{OTHER}> ?p ?o }}"),
            format!("INSERT DATA {{ GRAPH <http://victim/> {{ <{R}> <http://example.org/p> \"x\" }} }}"),
            format!("DELETE {{ <{R}> <http://example.org/p> ?o }} WHERE {{ GRAPH <http://victim/> {{ ?s ?p ?o }} }}"),
            format!("INSERT {{ <{R}> <http://example.org/p> ?o }} WHERE {{ GRAPH ?g {{ ?s ?p ?o }} }}"),
            format!("INSERT {{ <{R}> <http://example.org/p> ?o }} WHERE {{ SERVICE <http://x/sparql> {{ ?s ?p ?o }} }}"),
            // The same reach from inside an EXISTS expression.
            format!("INSERT {{ <{R}> <http://example.org/p> \"hit\" }} WHERE {{ FILTER EXISTS {{ GRAPH <http://victim/> {{ ?s ?p ?o }} }} }}"),
            format!("INSERT {{ <{R}> <http://example.org/p> ?x }} WHERE {{ BIND(NOT EXISTS {{ SERVICE <http://x/sparql> {{ ?s ?p ?o }} }} AS ?x) }}"),
            "WITH <http://victim/> DELETE { ?s ?p ?o } WHERE { ?s ?p ?o }".to_string(),
            format!("DELETE {{ <{R}> ?p ?o }} USING <http://victim/> WHERE {{ ?s ?p ?o }}"),
            "CLEAR ALL".to_string(),
            "DROP ALL".to_string(),
            "DROP GRAPH <http://victim/>".to_string(),
            "CREATE GRAPH <http://new/>".to_string(),
            "LOAD <http://example.org/data.ttl>".to_string(),
            // Server-managed triples.
            format!("DELETE DATA {{ <{R}> a <http://www.w3.org/ns/ldp#RDFSource> }}"),
            format!("INSERT DATA {{ <{R}> <http://www.w3.org/ns/ldp#contains> <{OTHER}> }}"),
            // Another resource reached through a variable subject.
            format!("INSERT {{ ?other <http://example.org/p> \"x\" }} WHERE {{ VALUES ?other {{ <{OTHER}> }} }}"),
        ] {
            let err = apply(&s, R, ROOT, &body).unwrap_err();
            assert!(matches!(err, PatchError::NotAllowed(_)), "{body}: {err:?}");
            unchanged_elsewhere(&s);
            assert!(ask(&s, &format!("ASK {{ <{R}> <http://example.org/p> \"v\" }}")), "{body}");
        }
        assert!(matches!(
            apply(&s, R, ROOT, "this is not sparql"),
            Err(PatchError::BadRequest(_))
        ));
    }
}
