//! Blank nodes of a draft, in `PATCH /api/models/:id/versions/:ver/data`.
//!
//! The patch writes its statements with `DELETE DATA` and `INSERT DATA`, and
//! neither reaches a blank node that is already in the draft: SPARQL refuses
//! a blank node in `DELETE DATA`, and a label in `INSERT DATA` is a new node
//! on every request. An anonymous SHACL property shape (`sh:property [ … ]`),
//! an OWL restriction or an RDF list (`sh:in ( … )`) could therefore be read
//! from a draft and never changed or removed.
//!
//! A request can now say which blank nodes of the draft it means. Each entry
//! of `nodes` gives one a label and says how to find it: by what the draft
//! states about it (the statement it is the object of, the statements it
//! has), or by the identifier a read of the draft shows. In `add` and
//! `remove`, `_:label` is then that node, and `remove_nodes` removes a node
//! with everything below it — a whole property shape, a whole list.
//!
//! Nothing changes in what is stored or exported: blank nodes stay blank
//! nodes and keep their identifiers. Such a request is written through
//! [`TripleStore::apply_quad_ops`], which takes terms as they are, in one
//! transaction; a request that declares no node keeps the SPARQL path.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use oxigraph::model::{
    BlankNode, GraphName, GraphNameRef, Literal, NamedNode, NamedOrBlankNode, Quad, QuadRef, Term,
};
use serde_json::json;

use super::models::{BlankNodeLocator, DataModelVersion, PatchVersionRequest, RdfTriple};
use crate::server::error::AppError;
use crate::store::{QuadOp, StoreError, TripleStore};

/// What a patch that names blank nodes of the draft did.
pub struct Applied {
    /// The graphs its statements are in.
    pub graphs: BTreeSet<String>,
    /// Statements written that were not there before.
    pub added: usize,
    /// Statements that were there and are gone.
    pub removed: usize,
}

/// Whether the request names blank nodes of the draft, and so goes through
/// [`apply`] rather than SPARQL.
pub fn names_existing_nodes(body: &PatchVersionRequest) -> bool {
    !body.nodes.is_empty() || !body.remove_nodes.is_empty()
}

/// A blank node in `remove` can only be one the draft already has, and a
/// request without `nodes` has no way to say which. Refused here with what to
/// do about it, before SPARQL refuses it for being in `DELETE DATA`.
pub fn refuse_undeclared_removals(remove: &[RdfTriple]) -> Result<(), AppError> {
    for t in remove {
        for term in [Some(t.s.as_str()), t.o.as_str()].into_iter().flatten() {
            if term.starts_with("_:") {
                return Err(undeclared(term, "remove"));
            }
        }
    }
    Ok(())
}

/// Apply a patch that names blank nodes of the draft: find every declared
/// node, then write the node removals, `remove` and `add`, in that order, in
/// one transaction. A node that cannot be found refuses the whole request.
pub fn apply(
    store: &TripleStore,
    record: &DataModelVersion,
    default_graph: &str,
    body: &PatchVersionRequest,
) -> Result<Applied, AppError> {
    let mut locators: BTreeMap<&str, &BlankNodeLocator> = BTreeMap::new();
    for (key, locator) in &body.nodes {
        let label = bare_label(key);
        BlankNode::new(label).map_err(|_| {
            AppError::BadRequest(format!("nodes: '{key}' is not a blank node label"))
        })?;
        if locators.insert(label, locator).is_some() {
            return Err(AppError::BadRequest(format!(
                "nodes: '{label}' is declared twice"
            )));
        }
    }

    let mut patch = Patch {
        store,
        record,
        default_graph,
        locators,
        found: HashMap::new(),
        resolving: Vec::new(),
    };
    for label in patch.locators.keys().copied().collect::<Vec<_>>() {
        patch.declared(label)?;
    }

    let mut ops: Vec<QuadOp> = Vec::new();
    for label in &body.remove_nodes {
        let Some(found) = patch.found.get(bare_label(label)) else {
            return Err(AppError::BadRequest(format!(
                "remove_nodes: '{label}' is not declared in `nodes`"
            )));
        };
        ops.extend(
            patch
                .node_and_below(&found.node, &found.graph)?
                .into_iter()
                .map(QuadOp::Remove),
        );
    }
    for t in &body.remove {
        ops.push(QuadOp::Remove(patch.quad(t, None, "remove")?));
    }
    // A label `nodes` does not declare is a new node, one per label and
    // request, as it is in `INSERT DATA`.
    let mut minted: HashMap<String, BlankNode> = HashMap::new();
    for t in &body.add {
        ops.push(QuadOp::Add(patch.quad(t, Some(&mut minted), "add")?));
    }

    let graphs = ops
        .iter()
        .filter_map(|op| match &op.quad().graph_name {
            GraphName::NamedNode(g) => Some(g.as_str().to_string()),
            _ => None,
        })
        .collect();
    let (inserted, deleted) = store.apply_quad_ops(&ops)?;
    Ok(Applied {
        graphs,
        added: inserted.len(),
        removed: deleted.len(),
    })
}

/// A declared node, once found.
struct Found {
    node: BlankNode,
    /// The graph it was found in.
    graph: NamedNode,
}

struct Patch<'a> {
    store: &'a TripleStore,
    record: &'a DataModelVersion,
    default_graph: &'a str,
    locators: BTreeMap<&'a str, &'a BlankNodeLocator>,
    found: HashMap<&'a str, Found>,
    /// The labels being looked up, outermost first: a description may use
    /// another declared node, but not itself.
    resolving: Vec<&'a str>,
}

impl<'a> Patch<'a> {
    /// The node `label` is declared to be, or `None` when `nodes` has no
    /// such label.
    fn declared(&mut self, label: &str) -> Result<Option<BlankNode>, AppError> {
        if let Some(found) = self.found.get(label) {
            return Ok(Some(found.node.clone()));
        }
        let Some((&label, &locator)) = self.locators.get_key_value(label) else {
            return Ok(None);
        };
        if self.resolving.contains(&label) {
            return Err(AppError::BadRequest(format!(
                "nodes.{label}: its description depends on itself"
            )));
        }
        self.resolving.push(label);
        let found = self.locate(label, locator);
        self.resolving.pop();
        let found = found?;
        let node = found.node.clone();
        self.found.insert(label, found);
        Ok(Some(node))
    }

    /// Find the one blank node a locator describes.
    fn locate(&mut self, label: &str, locator: &BlankNodeLocator) -> Result<Found, AppError> {
        let graph = self.graph(locator.graph.as_deref())?;
        let in_graph = GraphNameRef::NamedNode(graph.as_ref());
        let described = locator.s.is_some() || locator.p.is_some() || !locator.has.is_empty();

        if let Some(id) = locator.id.as_deref() {
            if described {
                return Err(AppError::BadRequest(format!(
                    "nodes.{label}: give `id` or a description (`s` and `p`, `where`), not both"
                )));
            }
            let node = BlankNode::new(bare_label(id)).map_err(|_| {
                AppError::BadRequest(format!("nodes.{label}: '{id}' is not a blank node label"))
            })?;
            if outgoing(self.store, &node, in_graph)?.is_empty()
                && incoming(self.store, &node, in_graph)?.is_empty()
            {
                return Err(not_found(label, &graph));
            }
            return Ok(Found { node, graph });
        }

        let link = match (locator.s.as_deref(), locator.p.as_deref()) {
            (Some(s), Some(p)) => Some((self.node_or_iri(s, None, "nodes")?, iri(p)?)),
            (None, None) => None,
            _ => {
                return Err(AppError::BadRequest(format!(
                    "nodes.{label}: `s` and `p` go together"
                )))
            }
        };
        let mut has: Vec<(NamedNode, Term)> = Vec::with_capacity(locator.has.len());
        for statement in &locator.has {
            has.push((
                iri(&statement.p)?,
                self.object(&statement.o, None, "nodes")?,
            ));
        }

        // Candidates come from the most selective pattern at hand: the
        // objects of the link, else the subjects of the first statement.
        let quads = self.store.store();
        let mut candidates: Vec<BlankNode> = Vec::new();
        if let Some((s, p)) = &link {
            for q in
                quads.quads_for_pattern(Some(s.as_ref()), Some(p.as_ref()), None, Some(in_graph))
            {
                if let Term::BlankNode(b) = q.map_err(storage)?.object {
                    if !candidates.contains(&b) {
                        candidates.push(b);
                    }
                }
            }
        } else if let Some((p, o)) = has.first() {
            for q in
                quads.quads_for_pattern(None, Some(p.as_ref()), Some(o.as_ref()), Some(in_graph))
            {
                if let NamedOrBlankNode::BlankNode(b) = q.map_err(storage)?.subject {
                    if !candidates.contains(&b) {
                        candidates.push(b);
                    }
                }
            }
        } else {
            return Err(AppError::BadRequest(format!(
                "nodes.{label}: needs `id`, `s` and `p`, or `where`"
            )));
        }
        let mut matching: Vec<BlankNode> = Vec::new();
        for candidate in candidates {
            let mut fits = true;
            for (p, o) in &has {
                if !quads
                    .contains(QuadRef::new(
                        candidate.as_ref(),
                        p.as_ref(),
                        o.as_ref(),
                        in_graph,
                    ))
                    .map_err(storage)?
                {
                    fits = false;
                    break;
                }
            }
            if fits {
                matching.push(candidate);
            }
        }

        match matching.len() {
            1 => Ok(Found {
                node: matching.remove(0),
                graph,
            }),
            0 => Err(not_found(label, &graph)),
            n => Err(AppError::Conflict(json!({
                "error": "blank_node_ambiguous",
                "node": label,
                "matches": n,
                "message": format!(
                    "nodes.{label}: {n} blank nodes in <{graph}> fit this description. \
                     Add a statement to `where` that only one of them has.",
                    graph = graph.as_str()
                ),
            }))),
        }
    }

    /// The statements that make up a blank node within its graph: its own,
    /// those of the blank nodes below it, and the ones that point at it. A
    /// node below it that something else in the graph still points at stays,
    /// with what is below that.
    fn node_and_below(&self, root: &BlankNode, graph: &NamedNode) -> Result<Vec<Quad>, AppError> {
        let in_graph = GraphNameRef::NamedNode(graph.as_ref());

        let mut below: HashSet<BlankNode> = HashSet::from([root.clone()]);
        let mut todo = vec![root.clone()];
        while let Some(node) = todo.pop() {
            for q in outgoing(self.store, &node, in_graph)? {
                if let Term::BlankNode(b) = q.object {
                    if below.insert(b.clone()) {
                        todo.push(b);
                    }
                }
            }
        }

        // Dropping a shared node from the set makes it an outside reference
        // to what is below it, so repeat until nothing more drops out.
        loop {
            let mut shared: Vec<BlankNode> = Vec::new();
            for node in below.iter().filter(|n| *n != root) {
                let from_outside =
                    incoming(self.store, node, in_graph)?
                        .iter()
                        .any(|q| match &q.subject {
                            NamedOrBlankNode::BlankNode(b) => !below.contains(b),
                            NamedOrBlankNode::NamedNode(_) => true,
                        });
                if from_outside {
                    shared.push(node.clone());
                }
            }
            if shared.is_empty() {
                break;
            }
            for node in &shared {
                below.remove(node);
            }
        }

        let mut out = incoming(self.store, root, in_graph)?;
        for node in &below {
            out.extend(outgoing(self.store, node, in_graph)?);
        }
        Ok(out)
    }

    /// A statement of `add` or `remove` as a quad of its target graph.
    fn quad(
        &mut self,
        t: &RdfTriple,
        mut minted: Option<&mut HashMap<String, BlankNode>>,
        field: &str,
    ) -> Result<Quad, AppError> {
        let graph = self.graph(t.graph.as_deref())?;
        let s = self.node_or_iri(&t.s, minted.as_deref_mut(), field)?;
        let p = iri(&t.p)?;
        let o = self.object(&t.o, minted, field)?;
        Ok(Quad::new(s, p, o, graph))
    }

    /// An object: a JSON string is an IRI or a blank node, anything else a
    /// literal.
    fn object(
        &mut self,
        o: &serde_json::Value,
        minted: Option<&mut HashMap<String, BlankNode>>,
        field: &str,
    ) -> Result<Term, AppError> {
        match o {
            serde_json::Value::String(term) => Ok(self.node_or_iri(term, minted, field)?.into()),
            other => Ok(literal(other)?.into()),
        }
    }

    /// `_:label` as the declared node of that name, or, where new nodes can
    /// be made (`minted`), as a new one; anything else as an IRI.
    fn node_or_iri(
        &mut self,
        term: &str,
        minted: Option<&mut HashMap<String, BlankNode>>,
        field: &str,
    ) -> Result<NamedOrBlankNode, AppError> {
        let Some(label) = term.strip_prefix("_:") else {
            return Ok(iri(term)?.into());
        };
        BlankNode::new(label)
            .map_err(|_| AppError::BadRequest(format!("Invalid blank node term: '{term}'")))?;
        if let Some(node) = self.declared(label)? {
            return Ok(node.into());
        }
        match minted {
            Some(minted) => Ok(minted.entry(label.to_string()).or_default().clone().into()),
            None => Err(undeclared(term, field)),
        }
    }

    /// The graph a statement or a locator names, among those of the version.
    fn graph(&self, graph: Option<&str>) -> Result<NamedNode, AppError> {
        let graph = super::handlers::target_graph(graph, self.default_graph, self.record)?;
        NamedNode::new(&graph)
            .map_err(|_| AppError::BadRequest(format!("Invalid graph IRI: '{graph}'")))
    }
}

/// The statements of `graph` that `node` is the subject of.
fn outgoing(
    store: &TripleStore,
    node: &BlankNode,
    graph: GraphNameRef<'_>,
) -> Result<Vec<Quad>, AppError> {
    store
        .store()
        .quads_for_pattern(Some(node.as_ref().into()), None, None, Some(graph))
        .map(|q| q.map_err(storage))
        .collect()
}

/// The statements of `graph` that `node` is the object of.
fn incoming(
    store: &TripleStore,
    node: &BlankNode,
    graph: GraphNameRef<'_>,
) -> Result<Vec<Quad>, AppError> {
    store
        .store()
        .quads_for_pattern(None, None, Some(node.as_ref().into()), Some(graph))
        .map(|q| q.map_err(storage))
        .collect()
}

/// A label as `nodes` keys it: without the `_:` a statement writes in front.
fn bare_label(label: &str) -> &str {
    label.strip_prefix("_:").unwrap_or(label)
}

/// An IRI term, bare or in `<…>`.
fn iri(term: &str) -> Result<NamedNode, AppError> {
    let inner = if term.starts_with('<') && term.ends_with('>') && term.len() >= 2 {
        &term[1..term.len() - 1]
    } else {
        term
    };
    NamedNode::new(inner).map_err(|_| AppError::BadRequest(format!("Invalid IRI term: '{term}'")))
}

/// A literal from its JSON form `{value, lang?, datatype?}`; a bare number,
/// boolean, null or array is a plain literal of its JSON text. The same
/// literal the SPARQL path writes for the same JSON.
fn literal(v: &serde_json::Value) -> Result<Literal, AppError> {
    let serde_json::Value::Object(map) = v else {
        return Ok(Literal::new_simple_literal(v.to_string()));
    };
    let value = map.get("value").and_then(|v| v.as_str()).unwrap_or("");
    if let Some(lang) = map.get("lang").and_then(|v| v.as_str()) {
        crate::store::validate_language_tag(lang).map_err(AppError::BadRequest)?;
        return language_literal(value, lang);
    }
    if let Some(datatype) = map.get("datatype").and_then(|v| v.as_str()) {
        let datatype = NamedNode::new(datatype)
            .map_err(|_| AppError::BadRequest(format!("Invalid datatype IRI: '{datatype}'")))?;
        return Ok(Literal::new_typed_literal(value, datatype));
    }
    Ok(Literal::new_simple_literal(value))
}

/// A language-tagged literal; with RDF 1.2, `lang--ltr` and `lang--rtl` give
/// it a base direction, as they do in SPARQL.
fn language_literal(value: &str, lang: &str) -> Result<Literal, AppError> {
    let invalid = || AppError::BadRequest(format!("Invalid language tag: '{lang}'"));
    #[cfg(feature = "rdf-12")]
    if let Some((tag, direction)) = lang.split_once("--") {
        let direction = match direction {
            "ltr" => oxigraph::model::BaseDirection::Ltr,
            "rtl" => oxigraph::model::BaseDirection::Rtl,
            _ => return Err(invalid()),
        };
        return Literal::new_directional_language_tagged_literal(value, tag, direction)
            .map_err(|_| invalid());
    }
    Literal::new_language_tagged_literal(value, lang).map_err(|_| invalid())
}

fn undeclared(term: &str, field: &str) -> AppError {
    AppError::BadRequest(format!(
        "'{term}' in `{field}` is not declared in `nodes`. A blank node label is local to \
         the request; to refer to a blank node the draft already has, declare it in `nodes`."
    ))
}

fn not_found(label: &str, graph: &NamedNode) -> AppError {
    AppError::Conflict(json!({
        "error": "blank_node_not_found",
        "node": label,
        "message": format!(
            "nodes.{label}: no blank node in <{}> fits this description. \
             The draft may have changed since it was read.",
            graph.as_str()
        ),
    }))
}

fn storage(e: oxigraph::store::StorageError) -> AppError {
    AppError::from(StoreError::from(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_models::models::VersionStatus;

    const G: &str = "http://localhost/data-model/m/version/1";
    const EX: &str = "http://example.org/";

    fn record() -> DataModelVersion {
        DataModelVersion {
            data_model_id: "m".into(),
            version: "1".into(),
            status: VersionStatus::Draft,
            graph_iri: G.into(),
            sub_graphs: vec![],
            created_at: "2026-01-01T00:00:00Z".into(),
            created_by: None,
            derived_from: None,
            notes: None,
            branch: None,
            sub_graph_status: vec![],
        }
    }

    fn store_with(turtle: &str) -> TripleStore {
        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                &format!("@prefix ex: <{EX}> .\n{turtle}"),
                oxigraph::io::RdfFormat::Turtle,
                Some(G),
            )
            .unwrap();
        store
    }

    fn patch(store: &TripleStore, body: serde_json::Value) -> Result<Applied, AppError> {
        let body: PatchVersionRequest = serde_json::from_value(body).unwrap();
        apply(store, &record(), G, &body)
    }

    fn ask(store: &TripleStore, pattern: &str) -> bool {
        let q = format!("PREFIX ex: <{EX}> ASK {{ GRAPH <{G}> {{ {pattern} }} }}");
        matches!(
            store.query(&q),
            Ok(oxigraph::sparql::QueryResults::Boolean(true))
        )
    }

    fn count(store: &TripleStore) -> usize {
        store.count_graph(Some(G)).unwrap()
    }

    /// A node below the removed one that another node still points at stays,
    /// with what is below it; what only the removed node reached goes.
    #[test]
    fn removal_stops_at_a_node_something_else_points_at() {
        let store = store_with(
            "ex:a ex:has [ ex:name \"gone\" ; ex:part _:shared ; ex:part [ ex:name \"own\" ] ] .
             ex:b ex:uses _:shared .
             _:shared ex:name \"kept\" ; ex:part [ ex:name \"below kept\" ] .",
        );
        let applied = patch(
            &store,
            json!({
                "nodes": { "n": { "s": format!("{EX}a"), "p": format!("{EX}has") } },
                "remove_nodes": ["n"],
            }),
        )
        .unwrap();
        // The link, the node's three statements and its own part's name.
        assert_eq!(applied.removed, 5);
        assert!(!ask(&store, "ex:a ex:has ?n"));
        assert!(!ask(&store, "?n ex:name \"gone\""));
        assert!(!ask(&store, "?n ex:name \"own\""));
        assert!(ask(
            &store,
            "ex:b ex:uses ?s . ?s ex:name \"kept\" ; ex:part [ ex:name \"below kept\" ]"
        ));
        assert_eq!(count(&store), 4);
    }

    /// Two paths to one node inside the removed part do not keep it.
    #[test]
    fn removal_takes_a_node_reached_twice_from_within() {
        let store = store_with(
            "ex:a ex:has [ ex:left _:leaf ; ex:right [ ex:down _:leaf ] ] .
             _:leaf ex:name \"leaf\" .",
        );
        patch(
            &store,
            json!({
                "nodes": { "n": { "s": format!("{EX}a"), "p": format!("{EX}has") } },
                "remove_nodes": ["n"],
            }),
        )
        .unwrap();
        assert_eq!(count(&store), 0);
    }

    /// A node that is the subject of nothing else is found by what it states,
    /// and a description may build on another declared node in any order.
    #[test]
    fn finds_a_root_node_and_follows_declared_nodes() {
        let store = store_with(
            "[] a ex:Axiom ; ex:about ex:x ; ex:detail [ ex:level 1 ] .
             [] a ex:Axiom ; ex:about ex:y ; ex:detail [ ex:level 1 ] .",
        );
        patch(
            &store,
            json!({
                "nodes": {
                    "inner": { "s": "_:outer", "p": format!("{EX}detail") },
                    "outer": { "where": [
                        { "p": "http://www.w3.org/1999/02/22-rdf-syntax-ns#type", "o": format!("{EX}Axiom") },
                        { "p": format!("{EX}about"), "o": format!("{EX}y") },
                    ] },
                },
                "remove": [{ "s": "_:inner", "p": format!("{EX}level"),
                    "o": { "value": "1", "datatype": "http://www.w3.org/2001/XMLSchema#integer" } }],
                "add": [{ "s": "_:inner", "p": format!("{EX}level"),
                    "o": { "value": "2", "datatype": "http://www.w3.org/2001/XMLSchema#integer" } }],
            }),
        )
        .unwrap();
        assert!(ask(&store, "?a ex:about ex:y ; ex:detail [ ex:level 2 ]"));
        assert!(ask(&store, "?a ex:about ex:x ; ex:detail [ ex:level 1 ]"));
        assert_eq!(count(&store), 8);
    }

    #[test]
    fn refuses_what_it_cannot_resolve() {
        let store = store_with("ex:a ex:has [ ex:name \"one\" ] , [ ex:name \"two\" ] .");
        let before = count(&store);
        let link = json!({ "s": format!("{EX}a"), "p": format!("{EX}has") });

        let ambiguous = patch(
            &store,
            json!({ "nodes": { "n": link }, "remove_nodes": ["n"] }),
        );
        assert!(matches!(&ambiguous, Err(AppError::Conflict(v))
                if v["error"] == "blank_node_ambiguous" && v["matches"] == 2 && v["node"] == "n"));

        let missing = patch(
            &store,
            json!({ "nodes": { "n": { "s": format!("{EX}a"), "p": format!("{EX}has"),
                "where": [{ "p": format!("{EX}name"), "o": { "value": "three" } }] } } }),
        );
        assert!(
            matches!(&missing, Err(AppError::Conflict(v)) if v["error"] == "blank_node_not_found")
        );

        for (what, body) in [
            (
                "a description that needs itself",
                json!({ "nodes": { "n": { "s": "_:n", "p": format!("{EX}has") } } }),
            ),
            (
                "`s` without `p`",
                json!({ "nodes": { "n": { "s": format!("{EX}a") } } }),
            ),
            ("an empty description", json!({ "nodes": { "n": {} } })),
            (
                "`id` with a description",
                json!({ "nodes": { "n": { "id": "x", "s": format!("{EX}a"), "p": format!("{EX}has") } } }),
            ),
            (
                "an undeclared label in a description",
                json!({ "nodes": { "n": { "s": "_:other", "p": format!("{EX}has") } } }),
            ),
            (
                "an undeclared label in `remove`",
                json!({ "nodes": { "n": { "s": format!("{EX}a"), "p": format!("{EX}has"),
                        "where": [{ "p": format!("{EX}name"), "o": { "value": "one" } }] } },
                    "remove": [{ "s": "_:m", "p": format!("{EX}name"), "o": { "value": "one" } }] }),
            ),
            (
                "an undeclared label in `remove_nodes`",
                json!({ "nodes": { "n": { "s": format!("{EX}a"), "p": format!("{EX}has"),
                        "where": [{ "p": format!("{EX}name"), "o": { "value": "one" } }] } },
                    "remove_nodes": ["m"] }),
            ),
            (
                "a label declared twice",
                json!({ "nodes": {
                    "n": { "s": format!("{EX}a"), "p": format!("{EX}has") },
                    "_:n": { "s": format!("{EX}a"), "p": format!("{EX}has") } } }),
            ),
            (
                "a graph of another version",
                json!({ "nodes": { "n": { "s": format!("{EX}a"), "p": format!("{EX}has"),
                    "graph": "http://localhost/data-model/other/version/1" } } }),
            ),
        ] {
            let result = patch(&store, body);
            assert!(
                matches!(result, Err(AppError::BadRequest(_))),
                "{what} must be a bad request"
            );
        }
        assert_eq!(count(&store), before, "a refused request writes nothing");
    }

    /// A misspelt key in a locator would loosen the description; it is not
    /// read as one.
    #[test]
    fn a_locator_takes_no_unknown_field() {
        let body = json!({ "nodes": { "n": { "s": format!("{EX}a"), "p": format!("{EX}has"),
            "wher": [] } } });
        assert!(serde_json::from_value::<PatchVersionRequest>(body).is_err());
    }

    /// The literal a statement's JSON gives here is the one `INSERT DATA`
    /// stores for it, so the two ways of writing a draft agree on every term.
    #[test]
    fn literals_match_the_sparql_path() {
        let xsd = "http://www.w3.org/2001/XMLSchema#";
        let mut cases = vec![
            json!({ "value": "plain" }),
            json!({ "value": "say \"hi\"\n\tnow \\ then" }),
            json!({ "value": "tekst", "lang": "nl" }),
            json!({ "value": "text", "lang": "EN-gb" }),
            json!({ "value": "1", "datatype": format!("{xsd}integer") }),
            json!({ "value": "01", "datatype": format!("{xsd}integer") }),
            json!({ "value": "x", "datatype": format!("{xsd}string") }),
            json!({ "value": "2026-01-01", "datatype": format!("{xsd}date") }),
            json!({}),
            json!(7),
            json!(true),
            json!(null),
        ];
        if cfg!(feature = "rdf-12") {
            cases.push(json!({ "value": "x", "lang": "ar--rtl" }));
        }
        for case in cases {
            let store = TripleStore::in_memory().unwrap();
            let token = crate::data_models::handlers::rdf_value_to_sparql(&case).unwrap();
            store
                .update(&format!("INSERT DATA {{ <{EX}s> <{EX}p> {token} }}"))
                .unwrap();
            let stored = store
                .quads_for_graph(GraphNameRef::DefaultGraph)
                .unwrap()
                .remove(0)
                .object;
            assert_eq!(Term::from(literal(&case).unwrap()), stored, "{case}");
        }
    }
}
