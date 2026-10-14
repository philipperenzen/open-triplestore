//! The rule vocabulary in RDF (`docs/notes/repair-layer-design.md` §3.1,
//! §3.5): reading `ots:Rule` resources out of a graph, and writing rules
//! back as Turtle so a compiled set can be saved, edited and run again as
//! authored rules.
//!
//! ```turtle
//! <urn:rules:deck> a ots:Rule ;
//!   ots:construct """CONSTRUCT { ?this ex:hasDeck ?w . ?w a ex:Deck }
//!                    WHERE { GRAPH ?g { ?this a ex:Bridge } }""" ;
//!   ots:nulls ( "w" ) ;
//!   ots:message "{?this} needs a deck" .
//! ```
//!
//! Variables are not RDF terms, so the lists of `ots:nulls` and `ots:equate`
//! hold their names as literals (`"w"` or `"?w"`). A rule's construct may use
//! prefixed names declared in its own text or through `sh:prefixes`, as a
//! SHACL-AF `sh:SPARQLRule` does; `sh:construct` is accepted in place of
//! `ots:construct`.

use oxigraph::model::Term;

use super::rules::{Confidence, GraphScope, MergeMode, Origin, Policy, RuleError, RuleSpec, OTS};
use crate::store::TripleStore;

const SH: &str = "http://www.w3.org/ns/shacl#";
const RDF_FIRST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
const RDF_REST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
const RDF_NIL: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#nil";
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const OWL_IMPORTS_LIMIT: usize = 256;

fn subject_key(t: &Term) -> Option<String> {
    match t {
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        Term::BlankNode(b) => Some(format!("_:{}", b.as_str())),
        _ => None,
    }
}

fn one(store: &TripleStore, graph: &str, subject: &str, p: &str) -> Option<Term> {
    store
        .objects_for_subject_in_graph(subject, p, Some(graph))
        .into_iter()
        .next()
}

fn literal(store: &TripleStore, graph: &str, subject: &str, p: &str) -> Option<String> {
    match one(store, graph, subject, p)? {
        Term::Literal(l) => Some(l.value().to_string()),
        _ => None,
    }
}

fn iri(store: &TripleStore, graph: &str, subject: &str, p: &str) -> Option<String> {
    match one(store, graph, subject, p)? {
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        _ => None,
    }
}

fn boolean(store: &TripleStore, graph: &str, subject: &str, p: &str) -> bool {
    literal(store, graph, subject, p).is_some_and(|v| v == "true" || v == "1")
}

/// The items of an RDF list, as literal values or IRIs.
fn list(store: &TripleStore, graph: &str, head: &Term) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = head.clone();
    for _ in 0..OWL_IMPORTS_LIMIT {
        if matches!(&cur, Term::NamedNode(n) if n.as_str() == RDF_NIL) {
            break;
        }
        let Some(key) = subject_key(&cur) else { break };
        match one(store, graph, &key, RDF_FIRST) {
            Some(Term::Literal(l)) => out.push(l.value().to_string()),
            Some(Term::NamedNode(n)) => out.push(n.as_str().to_string()),
            _ => break,
        }
        match one(store, graph, &key, RDF_REST) {
            Some(next) => cur = next,
            None => break,
        }
    }
    out
}

/// `PREFIX` declarations from a rule's `sh:prefixes` (SHACL-AF §5.2.1).
fn prefixes(store: &TripleStore, graph: &str, subject: &str) -> String {
    let mut out = String::new();
    for decl_owner in
        store.objects_for_subject_in_graph(subject, &format!("{SH}prefixes"), Some(graph))
    {
        let Some(owner) = subject_key(&decl_owner) else {
            continue;
        };
        for decl in store.objects_for_subject_in_graph(&owner, &format!("{SH}declare"), Some(graph))
        {
            let Some(d) = subject_key(&decl) else {
                continue;
            };
            let (Some(prefix), Some(ns)) = (
                literal(store, graph, &d, &format!("{SH}prefix")),
                literal(store, graph, &d, &format!("{SH}namespace"))
                    .or_else(|| iri(store, graph, &d, &format!("{SH}namespace"))),
            ) else {
                continue;
            };
            out.push_str(&format!("PREFIX {prefix}: <{ns}>\n"));
        }
    }
    out
}

/// Every `ots:Rule` in `graph`, in IRI order.
pub fn load_rules(
    store: &TripleStore,
    graph: &str,
    origin: Origin,
) -> Result<Vec<RuleSpec>, RuleError> {
    let ty = oxigraph::model::NamedNode::new_unchecked(RDF_TYPE);
    let rule_class = oxigraph::model::NamedNode::new_unchecked(format!("{OTS}Rule"));
    let Ok(g) = oxigraph::model::NamedNode::new(graph) else {
        return Ok(Vec::new());
    };
    let mut subjects: Vec<String> = store
        .store()
        .quads_for_pattern(
            None,
            Some(ty.as_ref()),
            Some(rule_class.as_ref().into()),
            Some(g.as_ref().into()),
        )
        .flatten()
        .filter_map(|q| subject_key(&Term::from(q.subject)))
        .collect();
    subjects.sort();
    subjects.dedup();
    let mut out = Vec::new();
    for s in subjects {
        let err = |message: String| RuleError {
            rule: s.clone(),
            message,
        };
        let construct = literal(store, graph, &s, &format!("{OTS}construct"))
            .or_else(|| literal(store, graph, &s, &format!("{SH}construct")))
            .ok_or_else(|| err("ots:construct is missing".into()))?;
        let construct = format!("{}{construct}", prefixes(store, graph, &s));
        // A blank-node rule gets an IRI derived from its text.
        let rule_iri = if let Some(label) = s.strip_prefix("_:") {
            let _ = label;
            format!(
                "urn:ots:rule:authored:{}",
                &hex::encode(<sha2::Sha256 as sha2::Digest>::digest(construct.as_bytes()))[..16]
            )
        } else {
            s.clone()
        };
        let mut spec = RuleSpec::new(rule_iri, construct, origin);
        spec.source_graph = Some(graph.to_string());
        if let Some(head) = one(store, graph, &s, &format!("{OTS}nulls")) {
            spec.nulls = list(store, graph, &head);
        }
        if let Some(head) = one(store, graph, &s, &format!("{OTS}equate")) {
            let pair = list(store, graph, &head);
            if pair.len() != 2 {
                return Err(err(format!(
                    "ots:equate needs exactly two variables, found {}",
                    pair.len()
                )));
            }
            spec.equate = Some((pair[0].clone(), pair[1].clone()));
        }
        spec.retract = literal(store, graph, &s, &format!("{OTS}retract"));
        spec.merge_mode = match iri(store, graph, &s, &format!("{OTS}mergeMode")).as_deref() {
            Some(m) if m == format!("{OTS}Rewrite") => MergeMode::Rewrite,
            Some(m) if m == format!("{OTS}SameAsOnly") => MergeMode::SameAsOnly,
            None => MergeMode::SameAsOnly,
            Some(other) => return Err(err(format!("unknown ots:mergeMode <{other}>"))),
        };
        spec.graph_scope = match iri(store, graph, &s, &format!("{OTS}graphScope")).as_deref() {
            Some(m) if m == format!("{OTS}PerGraph") => GraphScope::PerGraph,
            Some(m) if m == format!("{OTS}Union") => GraphScope::Union,
            None => spec.graph_scope,
            Some(other) => return Err(err(format!("unknown ots:graphScope <{other}>"))),
        };
        spec.target_graph = iri(store, graph, &s, &format!("{OTS}targetGraph"))
            .filter(|g| g != &format!("{OTS}FocusGraph"));
        if let Some(p) = literal(store, graph, &s, &format!("{OTS}priority")) {
            spec.priority = p
                .trim()
                .parse()
                .map_err(|_| err(format!("ots:priority `{p}` is not a number")))?;
        }
        spec.policy = match iri(store, graph, &s, &format!("{OTS}policy")).as_deref() {
            Some(m) if m == format!("{OTS}Report") => Policy::Report,
            Some(m) if m == format!("{OTS}Repair") => Policy::Repair,
            None => Policy::Repair,
            Some(other) => return Err(err(format!("unknown ots:policy <{other}>"))),
        };
        if let Some(c) = literal(store, graph, &s, &format!("{OTS}confidence"))
            .or_else(|| iri(store, graph, &s, &format!("{OTS}confidence")))
        {
            spec.confidence = Confidence::parse(&c)
                .ok_or_else(|| err(format!("unknown ots:confidence `{c}`")))?;
        }
        spec.derived_from = iri(store, graph, &s, &format!("{OTS}derivedFrom"));
        spec.destructive = boolean(store, graph, &s, &format!("{OTS}destructive"));
        spec.message = literal(store, graph, &s, &format!("{OTS}message"));
        if boolean(store, graph, &s, &format!("{SH}deactivated")) {
            continue;
        }
        out.push(spec);
    }
    Ok(out)
}

fn turtle_string(s: &str) -> String {
    if s.contains('\n') || s.contains('"') {
        format!(
            "\"\"\"{}\"\"\"",
            s.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"")
        )
    } else {
        format!("\"{}\"", crate::store::escape_sparql_literal(s))
    }
}

/// The rules as Turtle, one `ots:Rule` each, ready to save as a rules graph
/// and edit. Rules the vocabulary cannot carry (the native policy rules)
/// are written as comments.
pub fn to_turtle(rules: &[RuleSpec]) -> String {
    let mut out = format!("@prefix ots: <{OTS}> .\n");
    for r in rules {
        out.push('\n');
        if let Some(native) = &r.native {
            out.push_str(&format!(
                "# <{}> is computed by the engine ({}) and has no ots:Rule form.\n",
                r.iri,
                serde_json::to_string(native).unwrap_or_default()
            ));
            continue;
        }
        out.push_str(&format!(
            "<{}> a ots:Rule ;\n",
            crate::store::escape_sparql_iri(&r.iri)
        ));
        let mut props: Vec<String> =
            vec![format!("  ots:construct {}", turtle_string(&r.construct))];
        if !r.nulls.is_empty() {
            let items: Vec<String> = r.nulls.iter().map(|n| turtle_string(n)).collect();
            props.push(format!("  ots:nulls ( {} )", items.join(" ")));
        }
        if let Some((a, b)) = &r.equate {
            props.push(format!(
                "  ots:equate ( {} {} )",
                turtle_string(a),
                turtle_string(b)
            ));
        }
        if let Some(t) = &r.retract {
            props.push(format!("  ots:retract {}", turtle_string(t)));
        }
        if r.merge_mode == MergeMode::Rewrite {
            props.push("  ots:mergeMode ots:Rewrite".into());
        }
        props.push(format!(
            "  ots:graphScope {}",
            match r.graph_scope {
                GraphScope::PerGraph => "ots:PerGraph",
                GraphScope::Union => "ots:Union",
            }
        ));
        if let Some(g) = &r.target_graph {
            props.push(format!(
                "  ots:targetGraph <{}>",
                crate::store::escape_sparql_iri(g)
            ));
        }
        if r.priority != 0.0 {
            props.push(format!("  ots:priority {}", r.priority));
        }
        if r.policy == Policy::Report {
            props.push("  ots:policy ots:Report".into());
        }
        props.push(format!(
            "  ots:confidence \"{}\"",
            match r.confidence {
                Confidence::Certain => "certain",
                Confidence::Policy => "policy",
                Confidence::Heuristic => "heuristic",
            }
        ));
        if let Some(d) = &r.derived_from {
            if !d.starts_with("_:") {
                props.push(format!(
                    "  ots:derivedFrom <{}>",
                    crate::store::escape_sparql_iri(d)
                ));
            }
        }
        if r.destructive {
            props.push("  ots:destructive true".into());
        }
        if let Some(m) = &r.message {
            props.push(format!("  ots:message {}", turtle_string(m)));
        }
        out.push_str(&props.join(" ;\n"));
        out.push_str(" .\n");
        if r.guard.is_some() {
            out.push_str(&format!(
                "# <{}> was compiled with its constraint's own satisfaction check; saved as above it runs with the head guard instead.\n",
                r.iri
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::RdfFormat;

    const RULES: &str = r#"
@prefix ots: <https://opentriplestore.org/ns#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
<urn:r:deck> a ots:Rule ;
  ots:construct "PREFIX ex: <http://example.org/> CONSTRUCT { ?this ex:hasDeck ?w . ?w a ex:Deck } WHERE { GRAPH ?g { ?this a ex:Bridge } }" ;
  ots:nulls ( "?w" ) ;
  ots:graphScope ots:PerGraph ;
  ots:priority 2.5 ;
  ots:confidence "certain" ;
  ots:message "{?this} needs a deck" .
<urn:r:key> a ots:Rule ;
  sh:construct "CONSTRUCT { } WHERE { ?x <urn:code> ?k . ?y <urn:code> ?k FILTER(?x != ?y) }" ;
  ots:equate ( "x" "y" ) ;
  ots:mergeMode ots:Rewrite ;
  ots:policy ots:Report .
<urn:r:off> a ots:Rule ; ots:construct "CONSTRUCT { ?x <urn:p> 1 } WHERE { ?x <urn:q> 1 }" ; sh:deactivated true .
<urn:r:prefixed> a ots:Rule ;
  ots:construct "CONSTRUCT { ?x ex:p 1 } WHERE { ?x ex:q 1 }" ;
  sh:prefixes [ sh:declare [ sh:prefix "ex" ; sh:namespace "http://example.org/" ] ] .
"#;

    #[test]
    fn rules_are_read_from_a_graph() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(RULES, RdfFormat::Turtle, Some("urn:rules"))
            .unwrap();
        let rules = load_rules(&store, "urn:rules", Origin::Authored).unwrap();
        let iris: Vec<&str> = rules.iter().map(|r| r.iri.as_str()).collect();
        assert_eq!(
            iris,
            ["urn:r:deck", "urn:r:key", "urn:r:prefixed"],
            "deactivated rule skipped"
        );
        let deck = &rules[0];
        assert_eq!(deck.nulls, ["?w"]);
        assert_eq!(deck.graph_scope, GraphScope::PerGraph);
        assert_eq!(deck.priority, 2.5);
        assert_eq!(deck.source_graph.as_deref(), Some("urn:rules"));
        let key = &rules[1];
        assert_eq!(key.equate, Some(("x".into(), "y".into())));
        assert_eq!(key.merge_mode, MergeMode::Rewrite);
        assert_eq!(key.policy, Policy::Report);
        assert!(rules[2]
            .construct
            .starts_with("PREFIX ex: <http://example.org/>"));
        for r in rules {
            crate::repair::rules::prepare(r).unwrap();
        }
    }

    #[test]
    fn rules_survive_a_turtle_round_trip() {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(RULES, RdfFormat::Turtle, Some("urn:rules"))
            .unwrap();
        let rules = load_rules(&store, "urn:rules", Origin::Authored).unwrap();
        let ttl = to_turtle(&rules);
        let back = TripleStore::in_memory().unwrap();
        back.load_str(&ttl, RdfFormat::Turtle, Some("urn:saved"))
            .unwrap();
        let again = load_rules(&back, "urn:saved", Origin::Authored).unwrap();
        assert_eq!(again.len(), rules.len());
        for (a, b) in rules.iter().zip(&again) {
            assert_eq!(a.iri, b.iri);
            assert_eq!(a.construct, b.construct);
            assert_eq!(a.nulls, b.nulls);
            assert_eq!(a.equate, b.equate);
            assert_eq!(a.merge_mode, b.merge_mode);
            assert_eq!(a.policy, b.policy);
            assert_eq!(a.message, b.message);
        }
    }
}
