//! The ontology profile: what a model version declares, flattened.
//!
//! Computed by the store with a fixed SPARQL set over a version's TBox and its
//! shapes, so an external mapping proposer reads one endpoint instead of
//! re-implementing the traversal.
//!
//! ```text
//! GET /api/models/{id}/versions/{version}/profile → application/json
//! ```
//!
//! # Why this exists
//!
//! The proposer matches a SQL schema onto a published ontology. Doing that
//! needs the class hierarchy *transitively* (a table matches "Item" through
//! three levels of subclassing or not at all), the properties with their
//! domains and ranges, and the SHACL constraints that say what a conforming
//! instance must look like. Re-deriving all of that outside the store means a
//! second implementation of the traversal, kept in step by hand. So the store
//! derives it once, and the proposer reads one document.
//!
//! The proposer never receives a DSN, a row or a credential; this endpoint
//! returns none of those. It returns only what the model version itself
//! declares.
//!
//! # Where the shapes come from
//!
//! A model version's sub-graphs carry no role tag, so "which graph holds the
//! shapes" is not recorded anywhere. Guessing from the `…/version/{v}/shapes`
//! IRI suffix would be a convention this codebase has never enforced: a
//! version whose shapes sit in the base graph would silently profile as having
//! none, and a graph named `shapes` holding no shapes would be reported as a
//! shape source. So the rule here is **content plus bindings**, never the name:
//!
//! 1. **`version-graph`** — any graph of the version (base graph or sub-graph)
//!    that actually contains SHACL: a `sh:NodeShape`, a `sh:PropertyShape`, or
//!    a subject with `sh:path` / `sh:targetClass`. The conventional `…/shapes`
//!    sub-graph is found this way, without the convention being load-bearing.
//! 2. **`validation-binding`** — every shape graph the SHACL Studio validation
//!    layer binds to this version: bindings on the version IRI, on any of its
//!    sub-graph IRIs, and on the model IRI (a model-wide binding applies to
//!    each of its versions, the same inheritance a dataset gets from its
//!    graphs).
//!
//! Both are reported per source in `shape_sources`, each with its `origin` and
//! — for a binding — the `bound_to` IRI that carried it, so a caller can
//! always tell *where* a constraint came from. `shape_discovery` names the rule
//! itself, so a future change to it is detectable rather than silent.
//!
//! # Visibility
//!
//! The same rule the rest of the model registry applies
//! ([`crate::auth::db::AuthDb::can_access_ontology`], as in
//! [`crate::data_models::handlers::get_version`]): a private model answers 404
//! — not 403 — to anyone who may not read it, so the endpoint never confirms
//! that a private ontology exists. The route is additionally admin-gated by the
//! router it is merged into (`src/sources/routes.rs`), but the handler does not
//! rely on that: the check lives here too, because a seam can move.
//!
//! # The response contract
//!
//! Field names are read by an external program, so they are chosen once and
//! frozen. `profile` carries the contract id ([`PROFILE_CONTRACT`]); a
//! breaking change bumps it. Conventions:
//!
//! * snake_case, matching every other JSON response this store serves;
//! * a term is always its full IRI — never a prefixed name, because the
//!   reader would then need this store's prefix table to make sense of it;
//! * a blank-node shape is reported as `_:<label>` and is not resolvable;
//! * text that RDF allows to repeat (labels, descriptions) is always an
//!   array of `{value, lang}`, never a single string, so a multilingual
//!   ontology does not lose everything but one language;
//! * cardinal single-valued SHACL constraints (`sh:datatype`, `sh:class`, …)
//!   are scalars, taking the lexically first when a shape illegally repeats
//!   one, so the response stays deterministic;
//! * every list is sorted by a stable key, so two calls on unchanged data
//!   return byte-identical JSON.
//!
//! ```jsonc
//! {
//!   "profile": "ots-ontology-profile/1",
//!   "model":   { "id", "title", "namespace", "is_public" },
//!   "version": { "version", "status", "graph_iri", "sub_graphs" },
//!   "shape_discovery": "version-graph-content+validation-layer-bindings/1",
//!   "shape_sources":   [ { "graph_iri", "origin", "bound_to", "name" } ],
//!   "classes":         [ { "iri", "labels", "descriptions",
//!                          "direct_super_classes", "super_classes" } ],
//!   "properties":      [ { "iri", "kind", "labels", "descriptions",
//!                          "domains", "ranges", "datatype" } ],
//!   "property_shapes": [ { "shape", "node_shape", "target_classes",
//!                          "graph_iri", "path", "path_chain", "datatype",
//!                          "class", "node", "in_list", "min_count",
//!                          "max_count", "pattern", "name", "message" } ],
//!   "enumerations":    [ { "kind", "iri", "path", "shape", "graph_iri",
//!                          "values" } ],
//!   "counts":          { "classes", "properties", "property_shapes",
//!                        "enumerations" }
//! }
//! ```
//!
//! `super_classes` is the **full chain**, nearest ancestor first, not the
//! direct parents — that is the whole point of computing it here, and
//! `direct_super_classes` is kept beside it so a caller can still see the
//! declared edges.
//!
//! `property_shapes` is **flattened**: a shape nested under `sh:node` is
//! reported as its own entry whose `path_chain` is the full path from the root
//! shape down, so the reader never walks `sh:property` or `sh:node` itself.
//!
//! # Cost
//!
//! Nine SPARQL queries per request, whatever the size of the ontology — never
//! one per class, per shape, or per sub-graph:
//!
//! | # | Query | Over |
//! |---|---|---|
//! | 0 | validation bindings for every target at once | the validation layer |
//! | 1 | graphs that contain SHACL | the version's graphs |
//! | 2 | labels | version + shape graphs |
//! | 3 | descriptions | version + shape graphs |
//! | 4 | classes + direct superclasses | the version's graphs |
//! | 5 | properties + domain/range | the version's graphs |
//! | 6 | property shapes, their parents and constraints | shape graphs |
//! | 7 | RDF list cells | version + shape graphs |
//! | 8 | `owl:oneOf` heads + SKOS schemes | the version's graphs |
//!
//! The transitive closure of the class hierarchy, the RDF-list reconstruction
//! and the `sh:node` flattening are done in memory over those results, because
//! each would otherwise cost a query per term. One further lookup per bound
//! shape graph goes to the identity database, not to SPARQL, to resolve who
//! owns it — which is what decides whether the caller may see it at all.

use std::collections::{BTreeMap, BTreeSet};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Extension, Json, Router};
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use serde::Serialize;

use crate::auth::middleware::AuthenticatedUser;
use crate::data_models::models::{DataModelRecord, DataModelVersion};
use crate::data_models::registry;
use crate::server::AppState;
use crate::store::{escape_sparql_iri, TripleStore};

/// The response contract. A consumer pins this; a breaking change bumps it.
pub const PROFILE_CONTRACT: &str = "ots-ontology-profile/1";

/// The shape-resolution rule, named so a change to it is visible in the
/// response rather than only in these docs.
pub const SHAPE_DISCOVERY: &str = "version-graph-content+validation-layer-bindings/1";

/// Nesting depth `sh:node` is followed to while flattening. A shape graph may
/// be cyclic or merely deep; the chain that a mapping proposer can act on is
/// short, so bound the walk rather than letting one pathological graph decide
/// how long a request takes.
const MAX_SHAPE_DEPTH: usize = 8;

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const SH: &str = "http://www.w3.org/ns/shacl#";
const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
const DCT: &str = "http://purl.org/dc/terms/";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

type ApiResult<T> = Result<T, (StatusCode, String)>;

// ───────────────────────────── Response types ─────────────────────────────

/// A piece of text with the language tag RDF attached to it, if any.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct LangText {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelRef {
    pub id: String,
    pub title: String,
    pub namespace: String,
    pub is_public: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionRef {
    pub version: String,
    pub status: String,
    pub graph_iri: String,
    pub sub_graphs: Vec<String>,
}

/// Where a set of shapes was found. `origin` is a closed vocabulary:
/// `version-graph` (SHACL inside the version itself) or `validation-binding`
/// (a shape graph the validation layer binds to it).
#[derive(Debug, Clone, Serialize)]
pub struct ShapeSourceRef {
    pub graph_iri: String,
    pub origin: &'static str,
    /// The IRI whose binding pulled this graph in — the version, one of its
    /// sub-graphs, or the model. Absent for `version-graph`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_to: Option<String>,
    /// The shape set's name in SHACL Studio, when the graph is one it manages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassProfile {
    pub iri: String,
    pub labels: Vec<LangText>,
    pub descriptions: Vec<LangText>,
    pub direct_super_classes: Vec<String>,
    /// The complete ancestor chain, nearest first, excluding the class itself.
    pub super_classes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PropertyProfile {
    pub iri: String,
    /// `object` | `datatype` | `annotation` | `property` — the most specific
    /// OWL typing found, falling back to plain `rdf:Property`.
    pub kind: &'static str,
    pub labels: Vec<LangText>,
    pub descriptions: Vec<LangText>,
    pub domains: Vec<String>,
    pub ranges: Vec<String>,
    /// The range when it names a literal type: an XSD datatype, `rdf:langString`,
    /// or any range of an `owl:DatatypeProperty`. `null` when the property
    /// points at objects, which is what tells a proposer to map a column to it
    /// only through an IRI template.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datatype: Option<String>,
}

/// One value of an enumeration or an `sh:in` list.
#[derive(Debug, Clone, Serialize)]
pub struct EnumValue {
    pub value: String,
    /// `iri` | `literal` | `bnode`.
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datatype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<LangText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notation: Option<String>,
}

/// A SHACL property shape, flattened: everything it constrains, plus the full
/// path from the root shape, so the reader walks nothing itself.
#[derive(Debug, Clone, Serialize)]
pub struct PropertyShapeProfile {
    pub shape: String,
    /// The root shape this hangs off — `null` for a property shape that is not
    /// attached to one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_shape: Option<String>,
    pub target_classes: Vec<String>,
    pub graph_iri: String,
    /// `sh:path` when it is a plain IRI. A complex path expression (sequence,
    /// alternative, inverse) is a blank node and is reported as `null`, with
    /// the shape still listed so the caller knows it is there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Every path from the root shape down to this one, this shape's own path
    /// last. Length 1 for a shape directly under the root.
    pub path_chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datatype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// `sh:node`, kept even though the nested shape is flattened out: it names
    /// the shape the nested entries belong to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    pub in_list: Vec<EnumValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A closed value set, whichever of the three ways the model expresses one.
#[derive(Debug, Clone, Serialize)]
pub struct Enumeration {
    /// `owl_one_of` | `skos_scheme` | `sh_in`.
    pub kind: &'static str,
    /// The enumerated class or the concept scheme. Absent for `sh_in`, which
    /// belongs to a shape rather than to a term.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iri: Option<String>,
    /// For `sh_in`: the property path the value set constrains.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// For `sh_in`: the shape carrying it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
    pub graph_iri: String,
    pub values: Vec<EnumValue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileCounts {
    pub classes: usize,
    pub properties: usize,
    pub property_shapes: usize,
    pub enumerations: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct OntologyProfile {
    pub profile: &'static str,
    pub model: ModelRef,
    pub version: VersionRef,
    pub shape_discovery: &'static str,
    pub shape_sources: Vec<ShapeSourceRef>,
    pub classes: Vec<ClassProfile>,
    pub properties: Vec<PropertyProfile>,
    pub property_shapes: Vec<PropertyShapeProfile>,
    pub enumerations: Vec<Enumeration>,
    pub counts: ProfileCounts,
}

// ──────────────────────────── Term plumbing ────────────────────────────

/// A term's stable identity as this contract reports it: an IRI verbatim, a
/// blank node as `_:label`. Literals have no identity, so `None`.
fn node_id(t: Option<&Term>) -> Option<String> {
    match t {
        Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
        Some(Term::BlankNode(b)) => Some(format!("_:{}", b.as_str())),
        _ => None,
    }
}

fn iri(t: Option<&Term>) -> Option<String> {
    match t {
        Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
        _ => None,
    }
}

fn text(t: Option<&Term>) -> Option<String> {
    match t {
        Some(Term::Literal(l)) => Some(l.value().to_string()),
        Some(Term::NamedNode(n)) => Some(n.as_str().to_string()),
        _ => None,
    }
}

fn lang_text(t: Option<&Term>) -> Option<LangText> {
    match t {
        Some(Term::Literal(l)) => Some(LangText {
            value: l.value().to_string(),
            lang: l.language().map(str::to_string),
        }),
        _ => None,
    }
}

fn integer(t: Option<&Term>) -> Option<i64> {
    match t {
        Some(Term::Literal(l)) => l.value().trim().parse().ok(),
        _ => None,
    }
}

/// A value of an enumeration, carrying whatever RDF says about it.
fn enum_value(t: &Term) -> EnumValue {
    match t {
        Term::NamedNode(n) => EnumValue {
            value: n.as_str().to_string(),
            kind: "iri",
            datatype: None,
            lang: None,
            labels: Vec::new(),
            notation: None,
        },
        Term::BlankNode(b) => EnumValue {
            value: format!("_:{}", b.as_str()),
            kind: "bnode",
            datatype: None,
            lang: None,
            labels: Vec::new(),
            notation: None,
        },
        Term::Literal(l) => EnumValue {
            value: l.value().to_string(),
            kind: "literal",
            datatype: Some(l.datatype().as_str().to_string()),
            lang: l.language().map(str::to_string),
            labels: Vec::new(),
            notation: None,
        },
        // An RDF 1.2 triple term is reported by its serialisation rather than
        // dropped: an enumeration that contains one is still worth reading.
        #[cfg(feature = "rdf-12")]
        other => EnumValue {
            value: other.to_string(),
            kind: "literal",
            datatype: None,
            lang: None,
            labels: Vec::new(),
            notation: None,
        },
    }
}

fn prefixes() -> String {
    format!(
        "PREFIX rdf: <{RDF}>\nPREFIX rdfs: <{RDFS}>\nPREFIX owl: <{OWL}>\n\
         PREFIX sh: <{SH}>\nPREFIX skos: <{SKOS}>\nPREFIX dct: <{DCT}>\n\
         PREFIX xsd: <{XSD}>\n"
    )
}

/// `VALUES ?g { <a> <b> }` — how every query here is confined to a known set of
/// graphs. Graph IRIs reach this from the registry and the validation layer,
/// both of which store caller-influenced strings, so each is escaped.
fn values_graphs(graphs: &[String]) -> String {
    let mut out = String::from("VALUES ?g {");
    for g in graphs {
        out.push_str(" <");
        out.push_str(&escape_sparql_iri(g));
        out.push('>');
    }
    out.push_str(" }");
    out
}

fn solutions(store: &TripleStore, query: &str) -> Vec<oxigraph::sparql::QuerySolution> {
    match store.query(query) {
        Ok(QueryResults::Solutions(sols)) => sols.flatten().collect(),
        _ => Vec::new(),
    }
}

// ──────────────────────────── Pure derivations ────────────────────────────

/// The complete ancestor chain of `start`, nearest first, from the declared
/// `rdfs:subClassOf` edges. Breadth-first so "nearest" is true of the order,
/// and cycle-safe because an ontology may well declare one.
fn super_class_chain(direct: &BTreeMap<String, BTreeSet<String>>, start: &str) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert(start.to_string());
    let mut chain: Vec<String> = Vec::new();
    let mut frontier: Vec<String> = direct
        .get(start)
        .map(|s| s.iter().cloned().collect())
        .unwrap_or_default();
    while !frontier.is_empty() {
        let mut next: Vec<String> = Vec::new();
        for cls in frontier {
            if !seen.insert(cls.clone()) {
                continue;
            }
            chain.push(cls.clone());
            if let Some(parents) = direct.get(&cls) {
                next.extend(parents.iter().cloned());
            }
        }
        frontier = next;
    }
    chain
}

/// Rebuild an RDF collection from its cells, in list order. `cells` maps a cell
/// to `(rdf:first, rdf:rest)`. Stops at `rdf:nil`, at a dangling cell, and at a
/// cycle — a malformed list must not hang the request.
fn rdf_list(cells: &BTreeMap<String, (Term, String)>, head: &str) -> Vec<Term> {
    let nil = format!("{RDF}nil");
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut cur = head.to_string();
    while cur != nil && seen.insert(cur.clone()) {
        let Some((first, rest)) = cells.get(&cur) else {
            break;
        };
        out.push(first.clone());
        cur = rest.clone();
    }
    out
}

/// Which literal type a property's ranges name, if any. An
/// `owl:DatatypeProperty` says so by its typing; anything else has to name an
/// XSD type (or `rdf:langString`) for the range to be a datatype rather than a
/// class.
fn datatype_for(kind: &str, ranges: &BTreeSet<String>) -> Option<String> {
    let literal_typed = |r: &String| r.starts_with(XSD) || r == &format!("{RDF}langString");
    if let Some(r) = ranges.iter().find(|r| literal_typed(r)) {
        return Some(r.clone());
    }
    if kind == "datatype" {
        return ranges.iter().next().cloned();
    }
    None
}

/// The most specific typing of a property, given every `rdf:type` declared for
/// it. Object beats datatype beats annotation beats plain `rdf:Property`,
/// because a mapping proposer acts on the strongest claim the model makes.
fn property_kind(types: &BTreeSet<String>) -> &'static str {
    let has = |suffix: &str| types.contains(&format!("{OWL}{suffix}"));
    if has("ObjectProperty") {
        "object"
    } else if has("DatatypeProperty") {
        "datatype"
    } else if has("AnnotationProperty") {
        "annotation"
    } else {
        "property"
    }
}

/// What one property shape constrains, before flattening.
#[derive(Debug, Clone, Default)]
struct ShapeFacts {
    graph_iri: String,
    path: Option<String>,
    datatype: Option<String>,
    class: Option<String>,
    node: Option<String>,
    in_head: Option<String>,
    min_count: Option<i64>,
    max_count: Option<i64>,
    pattern: Option<String>,
    name: Option<String>,
    message: Option<String>,
}

impl ShapeFacts {
    /// Merge a further solution row for the same shape. SHACL allows one value
    /// for each of these, so a repeat is a malformed shape: keep the lexically
    /// first so the response cannot flap between requests.
    fn absorb(&mut self, other: ShapeFacts) {
        fn keep_min(slot: &mut Option<String>, v: Option<String>) {
            if let Some(v) = v {
                match slot {
                    Some(cur) if *cur <= v => {}
                    _ => *slot = Some(v),
                }
            }
        }
        if self.graph_iri.is_empty()
            || (!other.graph_iri.is_empty() && other.graph_iri < self.graph_iri)
        {
            self.graph_iri = other.graph_iri;
        }
        keep_min(&mut self.path, other.path);
        keep_min(&mut self.datatype, other.datatype);
        keep_min(&mut self.class, other.class);
        keep_min(&mut self.node, other.node);
        keep_min(&mut self.in_head, other.in_head);
        keep_min(&mut self.pattern, other.pattern);
        keep_min(&mut self.name, other.name);
        keep_min(&mut self.message, other.message);
        if let Some(v) = other.min_count {
            self.min_count = Some(self.min_count.map_or(v, |c| c.min(v)));
        }
        if let Some(v) = other.max_count {
            self.max_count = Some(self.max_count.map_or(v, |c| c.max(v)));
        }
    }
}

/// The shape graph's skeleton, as SPARQL hands it over.
#[derive(Debug, Default)]
struct ShapeGraphFacts {
    facts: BTreeMap<String, ShapeFacts>,
    /// Root shape → the property shapes hanging off it by `sh:property`.
    children: BTreeMap<String, BTreeSet<String>>,
    /// Shape → its `sh:targetClass` values.
    target_classes: BTreeMap<String, BTreeSet<String>>,
    /// Shapes that some property shape points at with `sh:node`.
    nested: BTreeSet<String>,
    /// Property shapes that hang off some parent.
    attached: BTreeSet<String>,
}

/// Flatten the shape graph: one entry per (root shape, path chain), with a
/// shape reached through `sh:node` emitted under the composed path rather than
/// left for the reader to walk.
///
/// A shape is a root when nothing points at it with `sh:node`, or when it
/// declares an `sh:targetClass` — a targeted shape applies on its own even if
/// it is also reused as a nested node shape.
fn flatten_shapes(
    g: &ShapeGraphFacts,
    lists: &BTreeMap<String, (Term, String)>,
) -> Vec<PropertyShapeProfile> {
    let mut out: Vec<PropertyShapeProfile> = Vec::new();

    let roots: Vec<String> = g
        .children
        .keys()
        .filter(|s| !g.nested.contains(*s) || g.target_classes.contains_key(*s))
        .cloned()
        .collect();

    for root in &roots {
        let targets: Vec<String> = g
            .target_classes
            .get(root)
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default();
        // (shape holding the children, path chain reaching it, shapes already
        // entered on this branch) — the last stops an sh:node cycle.
        let mut stack: Vec<(String, Vec<String>, BTreeSet<String>)> =
            vec![(root.clone(), Vec::new(), BTreeSet::from([root.clone()]))];
        while let Some((parent, prefix, visited)) = stack.pop() {
            let Some(children) = g.children.get(&parent) else {
                continue;
            };
            for child in children {
                let facts = g.facts.get(child).cloned().unwrap_or_default();
                let mut chain = prefix.clone();
                if let Some(p) = &facts.path {
                    chain.push(p.clone());
                }
                out.push(entry(
                    child,
                    Some(root),
                    &targets,
                    &facts,
                    chain.clone(),
                    lists,
                ));
                if let Some(node) = &facts.node {
                    if prefix.len() + 1 < MAX_SHAPE_DEPTH && !visited.contains(node) {
                        let mut deeper = visited.clone();
                        deeper.insert(node.clone());
                        stack.push((node.clone(), chain, deeper));
                    }
                }
            }
        }
    }

    // A property shape that hangs off nothing still constrains whatever targets
    // it directly, so it is reported rather than dropped.
    for (shape, facts) in &g.facts {
        if g.attached.contains(shape) {
            continue;
        }
        let chain = facts.path.clone().into_iter().collect::<Vec<_>>();
        let targets: Vec<String> = g
            .target_classes
            .get(shape)
            .map(|t| t.iter().cloned().collect())
            .unwrap_or_default();
        out.push(entry(shape, None, &targets, facts, chain, lists));
    }

    out.sort_by(|a, b| {
        (&a.node_shape, &a.path_chain, &a.shape).cmp(&(&b.node_shape, &b.path_chain, &b.shape))
    });
    out
}

fn entry(
    shape: &str,
    root: Option<&String>,
    targets: &[String],
    facts: &ShapeFacts,
    path_chain: Vec<String>,
    lists: &BTreeMap<String, (Term, String)>,
) -> PropertyShapeProfile {
    PropertyShapeProfile {
        shape: shape.to_string(),
        node_shape: root.cloned(),
        target_classes: targets.to_vec(),
        graph_iri: facts.graph_iri.clone(),
        path: facts.path.clone(),
        path_chain,
        datatype: facts.datatype.clone(),
        class: facts.class.clone(),
        node: facts.node.clone(),
        in_list: facts
            .in_head
            .as_deref()
            .map(|h| rdf_list(lists, h).iter().map(enum_value).collect())
            .unwrap_or_default(),
        min_count: facts.min_count,
        max_count: facts.max_count,
        pattern: facts.pattern.clone(),
        name: facts.name.clone(),
        message: facts.message.clone(),
    }
}

// ─────────────────────────────── The queries ───────────────────────────────

/// Query 1 — which of the version's graphs actually hold SHACL. Content, not
/// the graph's name: see the module docs.
fn shape_bearing_graphs(store: &TripleStore, graphs: &[String]) -> Vec<String> {
    let q = format!(
        "{p}SELECT DISTINCT ?g WHERE {{ {v}\n  GRAPH ?g {{\n\
         {{ ?s a sh:NodeShape }} UNION {{ ?s a sh:PropertyShape }}\n\
         UNION {{ ?s sh:path ?path }} UNION {{ ?s sh:targetClass ?tc }}\n  }} }}",
        p = prefixes(),
        v = values_graphs(graphs)
    );
    let mut out: Vec<String> = solutions(store, &q)
        .iter()
        .filter_map(|row| iri(row.get("g")))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Queries 2 and 3 — labels and descriptions for every IRI term in scope, once,
/// so nothing downstream needs a lookup query per term.
fn text_index(
    store: &TripleStore,
    graphs: &[String],
    path: &str,
) -> BTreeMap<String, BTreeSet<LangText>> {
    let q = format!(
        "{p}SELECT ?s ?t WHERE {{ {v}\n  GRAPH ?g {{ ?s {path} ?t . FILTER(isIRI(?s)) }} }}",
        p = prefixes(),
        v = values_graphs(graphs)
    );
    let mut out: BTreeMap<String, BTreeSet<LangText>> = BTreeMap::new();
    for row in solutions(store, &q) {
        if let (Some(s), Some(t)) = (iri(row.get("s")), lang_text(row.get("t"))) {
            out.entry(s).or_default().insert(t);
        }
    }
    out
}

/// Query 4 — every class and its declared parents. A class counts as declared
/// by its typing *or* by taking part in the hierarchy, so an ontology that
/// subclasses a term it does not type still profiles completely.
fn classes(
    store: &TripleStore,
    graphs: &[String],
    labels: &BTreeMap<String, BTreeSet<LangText>>,
    descriptions: &BTreeMap<String, BTreeSet<LangText>>,
) -> Vec<ClassProfile> {
    let q = format!(
        "{p}SELECT ?cls ?parent WHERE {{ {v}\n  GRAPH ?g {{\n\
         {{ ?cls a ?ctype . FILTER(?ctype IN (owl:Class, rdfs:Class)) }}\n\
         UNION {{ ?cls rdfs:subClassOf ?sup }}\n\
         FILTER(isIRI(?cls))\n\
         OPTIONAL {{ ?cls rdfs:subClassOf ?parent . FILTER(isIRI(?parent)) }}\n  }} }}",
        p = prefixes(),
        v = values_graphs(graphs)
    );
    let mut direct: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in solutions(store, &q) {
        let Some(cls) = iri(row.get("cls")) else {
            continue;
        };
        let parents = direct.entry(cls).or_default();
        if let Some(parent) = iri(row.get("parent")) {
            parents.insert(parent);
        }
    }
    direct
        .keys()
        .map(|cls| ClassProfile {
            iri: cls.clone(),
            labels: labels
                .get(cls)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            descriptions: descriptions
                .get(cls)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            direct_super_classes: direct
                .get(cls)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect(),
            super_classes: super_class_chain(&direct, cls),
        })
        .collect()
}

/// Query 5 — every property with its typing, domain and range.
fn properties(
    store: &TripleStore,
    graphs: &[String],
    labels: &BTreeMap<String, BTreeSet<LangText>>,
    descriptions: &BTreeMap<String, BTreeSet<LangText>>,
) -> Vec<PropertyProfile> {
    let q = format!(
        "{p}SELECT ?prop ?ptype ?domain ?range WHERE {{ {v}\n  GRAPH ?g {{\n\
         ?prop a ?ptype .\n\
         FILTER(?ptype IN (owl:ObjectProperty, owl:DatatypeProperty, owl:AnnotationProperty, rdf:Property))\n\
         FILTER(isIRI(?prop))\n\
         OPTIONAL {{ ?prop rdfs:domain ?domain . FILTER(isIRI(?domain)) }}\n\
         OPTIONAL {{ ?prop rdfs:range ?range . FILTER(isIRI(?range)) }}\n  }} }}",
        p = prefixes(),
        v = values_graphs(graphs)
    );
    let mut types: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut domains: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut ranges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in solutions(store, &q) {
        let Some(prop) = iri(row.get("prop")) else {
            continue;
        };
        if let Some(t) = iri(row.get("ptype")) {
            types.entry(prop.clone()).or_default().insert(t);
        }
        if let Some(d) = iri(row.get("domain")) {
            domains.entry(prop.clone()).or_default().insert(d);
        }
        if let Some(r) = iri(row.get("range")) {
            ranges.entry(prop.clone()).or_default().insert(r);
        }
        types.entry(prop).or_default();
    }
    types
        .iter()
        .map(|(prop, tys)| {
            let kind = property_kind(tys);
            let rng = ranges.get(prop).cloned().unwrap_or_default();
            PropertyProfile {
                iri: prop.clone(),
                kind,
                labels: labels
                    .get(prop)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
                descriptions: descriptions
                    .get(prop)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
                domains: domains
                    .get(prop)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
                datatype: datatype_for(kind, &rng),
                ranges: rng.into_iter().collect(),
            }
        })
        .collect()
}

/// Query 6 — every property shape, the shape it hangs off, and its constraints,
/// in one pass so the parent/child join happens in the engine and blank-node
/// identity never has to survive between two queries.
fn shape_facts(store: &TripleStore, graphs: &[String]) -> ShapeGraphFacts {
    let q = format!(
        "{p}SELECT ?g ?ps ?parent ?target ?path ?dt ?cls ?node ?inlist ?minc ?maxc ?pattern ?name ?msg\n\
         WHERE {{ {v}\n  GRAPH ?g {{\n\
         ?ps sh:path ?rawpath .\n\
         OPTIONAL {{ ?parent sh:property ?ps .\n\
           OPTIONAL {{ ?parent sh:targetClass ?target . FILTER(isIRI(?target)) }} }}\n\
         OPTIONAL {{ ?ps sh:path ?path . FILTER(isIRI(?path)) }}\n\
         OPTIONAL {{ ?ps sh:datatype ?dt }}\n\
         OPTIONAL {{ ?ps sh:class ?cls }}\n\
         OPTIONAL {{ ?ps sh:node ?node }}\n\
         OPTIONAL {{ ?ps sh:in ?inlist }}\n\
         OPTIONAL {{ ?ps sh:minCount ?minc }}\n\
         OPTIONAL {{ ?ps sh:maxCount ?maxc }}\n\
         OPTIONAL {{ ?ps sh:pattern ?pattern }}\n\
         OPTIONAL {{ ?ps sh:name ?name }}\n\
         OPTIONAL {{ ?ps sh:message ?msg }}\n  }} }}",
        p = prefixes(),
        v = values_graphs(graphs)
    );
    let mut out = ShapeGraphFacts::default();
    for row in solutions(store, &q) {
        let Some(ps) = node_id(row.get("ps")) else {
            continue;
        };
        let facts = ShapeFacts {
            graph_iri: iri(row.get("g")).unwrap_or_default(),
            path: iri(row.get("path")),
            datatype: iri(row.get("dt")),
            class: iri(row.get("cls")),
            node: node_id(row.get("node")),
            in_head: node_id(row.get("inlist")),
            min_count: integer(row.get("minc")),
            max_count: integer(row.get("maxc")),
            pattern: text(row.get("pattern")),
            name: text(row.get("name")),
            message: text(row.get("msg")),
        };
        out.facts.entry(ps.clone()).or_default().absorb(facts);
        if let Some(node) = node_id(row.get("node")) {
            out.nested.insert(node);
        }
        if let Some(parent) = node_id(row.get("parent")) {
            out.children
                .entry(parent.clone())
                .or_default()
                .insert(ps.clone());
            out.attached.insert(ps.clone());
            if let Some(target) = iri(row.get("target")) {
                out.target_classes.entry(parent).or_default().insert(target);
            }
        }
    }
    out
}

/// Query 7 — every RDF collection cell in scope. One query rebuilds every list
/// (`sh:in`, `owl:oneOf`) in order, which a `rdf:rest*/rdf:first` path could
/// not do: a property path returns members as a set, and an enumeration's
/// order is part of what it says.
fn list_cells(store: &TripleStore, graphs: &[String]) -> BTreeMap<String, (Term, String)> {
    let q = format!(
        "{p}SELECT ?cell ?first ?rest WHERE {{ {v}\n  GRAPH ?g {{ ?cell rdf:first ?first ; rdf:rest ?rest }} }}",
        p = prefixes(),
        v = values_graphs(graphs)
    );
    let mut out = BTreeMap::new();
    for row in solutions(store, &q) {
        let (Some(cell), Some(first), Some(rest)) = (
            node_id(row.get("cell")),
            row.get("first").cloned(),
            node_id(row.get("rest")),
        ) else {
            continue;
        };
        out.insert(cell, (first, rest));
    }
    out
}

/// Query 8 — the two enumerations the TBox itself carries: `owl:oneOf` classes
/// and SKOS concept schemes. (`sh:in` comes from the shapes, already read.)
fn tbox_enumerations(
    store: &TripleStore,
    graphs: &[String],
    lists: &BTreeMap<String, (Term, String)>,
    labels: &BTreeMap<String, BTreeSet<LangText>>,
) -> Vec<Enumeration> {
    let q = format!(
        "{p}SELECT ?g ?cls ?list ?scheme ?concept ?notation WHERE {{ {v}\n  GRAPH ?g {{\n\
         {{ ?cls owl:oneOf ?list . FILTER(isIRI(?cls)) }}\n\
         UNION {{\n\
           ?scheme a skos:ConceptScheme .\n\
           OPTIONAL {{ ?concept (skos:inScheme|skos:topConceptOf|^skos:hasTopConcept) ?scheme .\n\
             FILTER(isIRI(?concept))\n\
             OPTIONAL {{ ?concept skos:notation ?notation }} }}\n\
         }}\n  }} }}",
        p = prefixes(),
        v = values_graphs(graphs)
    );

    let mut one_of: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut schemes: BTreeMap<String, (String, BTreeMap<String, Option<String>>)> = BTreeMap::new();
    for row in solutions(store, &q) {
        let graph = iri(row.get("g")).unwrap_or_default();
        if let (Some(cls), Some(list)) = (iri(row.get("cls")), node_id(row.get("list"))) {
            one_of.insert(cls, (graph.clone(), list));
        }
        if let Some(scheme) = iri(row.get("scheme")) {
            let entry = schemes.entry(scheme).or_insert((graph, BTreeMap::new()));
            if let Some(concept) = iri(row.get("concept")) {
                let notation = text(row.get("notation"));
                entry.1.entry(concept).or_insert(notation);
            }
        }
    }

    let decorate = |mut v: EnumValue| {
        if v.kind == "iri" {
            v.labels = labels
                .get(&v.value)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
        }
        v
    };

    let mut out: Vec<Enumeration> = Vec::new();
    for (cls, (graph, head)) in one_of {
        out.push(Enumeration {
            kind: "owl_one_of",
            iri: Some(cls),
            path: None,
            shape: None,
            graph_iri: graph,
            values: rdf_list(lists, &head)
                .iter()
                .map(|t| decorate(enum_value(t)))
                .collect(),
        });
    }
    for (scheme, (graph, concepts)) in schemes {
        out.push(Enumeration {
            kind: "skos_scheme",
            iri: Some(scheme),
            path: None,
            shape: None,
            graph_iri: graph,
            values: concepts
                .into_iter()
                .map(|(concept, notation)| {
                    let mut v = decorate(EnumValue {
                        value: concept,
                        kind: "iri",
                        datatype: None,
                        lang: None,
                        labels: Vec::new(),
                        notation: None,
                    });
                    v.notation = notation;
                    v
                })
                .collect(),
        });
    }
    out
}

// ──────────────────────────────── Assembly ────────────────────────────────

/// The model, if the caller may read it. A model that exists but is not visible
/// answers exactly as one that does not, so the endpoint cannot be used to
/// discover that a private ontology is there.
pub fn readable_model(
    state: &AppState,
    user_id: Option<&str>,
    model_id: &str,
) -> ApiResult<DataModelRecord> {
    let missing = || {
        (
            StatusCode::NOT_FOUND,
            format!("Data model '{model_id}' not found"),
        )
    };
    let model =
        registry::get_data_model(&state.store, &state.base_url, model_id).ok_or_else(missing)?;
    let visible = state
        .auth_db
        .can_access_ontology(
            user_id,
            model.is_public,
            model.owner_type.as_deref(),
            model.owner_id.as_deref(),
        )
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if !visible {
        return Err(missing());
    }
    Ok(model)
}

/// The shape graphs that apply to `version`, each carrying where it came from.
/// See the module docs for the rule this implements.
///
/// A graph reached through a validation binding is admitted only when the
/// caller may read the shape graph itself. Being bound to a readable model
/// version is not consent to read a private shape set: its constraints, its
/// `sh:in` value sets and its labels are its owner's, and this endpoint would
/// otherwise be a way around the Studio's own listing rule. A bound graph the
/// Studio has no record for declares no owner and is admitted.
fn shape_sources(
    state: &AppState,
    user_id: Option<&str>,
    model_id: &str,
    version: &DataModelVersion,
) -> Vec<ShapeSourceRef> {
    let mut version_graphs: Vec<String> = vec![version.graph_iri.clone()];
    version_graphs.extend(version.sub_graphs.iter().cloned());
    version_graphs.sort();
    version_graphs.dedup();

    let mut out: Vec<ShapeSourceRef> = shape_bearing_graphs(&state.store, &version_graphs)
        .into_iter()
        .map(|graph_iri| ShapeSourceRef {
            graph_iri,
            origin: "version-graph",
            bound_to: None,
            name: None,
        })
        .collect();

    let studio = crate::shacl_studio::store::ShaclStudioStore::new(state.auth_db.pool());
    let model_iri = format!(
        "{}/data-model/{}",
        state.base_url.trim_end_matches('/'),
        model_id
    );
    let mut targets = vec![model_iri];
    targets.extend(version_graphs.iter().cloned());

    // One query over every target rather than one per target: the target list
    // grows with the version's sub-graphs, so a loop here would make the cost
    // of this endpoint scale with the size of the ontology.
    let orgs = user_id
        .map(|uid| state.auth_db.get_user_org_ids(uid).unwrap_or_default())
        .unwrap_or_default();
    let mut seen: BTreeSet<String> = out.iter().map(|s| s.graph_iri.clone()).collect();
    for (target, graph_iri) in bindings_for_targets(&state.store, &targets) {
        if seen.contains(&graph_iri) {
            continue;
        }
        // A graph the Studio has a record for carries an owner and a
        // visibility, so that record decides. One it does not know declares no
        // owner — it was put in the validation layer by an admin and there is
        // nobody whose privacy it would breach — so it is admitted, as before.
        let set = studio.get_shape_graph_by_iri(&graph_iri).ok().flatten();
        if let Some(set) = &set {
            if !crate::shacl_studio::access::can_access_set(set, user_id, &orgs) {
                continue;
            }
        }
        seen.insert(graph_iri.clone());
        out.push(ShapeSourceRef {
            graph_iri,
            origin: "validation-binding",
            bound_to: Some(target),
            name: set.map(|s| s.name),
        });
    }
    // The module promises byte-identical JSON for unchanged data, and a SELECT
    // returns rows in no guaranteed order.
    out.sort_by(|a, b| (a.origin, &a.graph_iri).cmp(&(b.origin, &b.graph_iri)));
    out
}

/// Every `(target, shape graph)` binding for `targets`, in one query.
///
/// Sorted, so the caller's own ordering is the only thing that decides the
/// result — and so a target bound to the same graph twice cannot flip which
/// target gets the attribution between two calls.
fn bindings_for_targets(store: &TripleStore, targets: &[String]) -> Vec<(String, String)> {
    use oxigraph::sparql::QueryResults;

    if targets.is_empty() {
        return Vec::new();
    }
    let values = targets
        .iter()
        .map(|t| format!("<{}>", crate::store::escape_sparql_iri(t)))
        .collect::<Vec<_>>()
        .join(" ");
    let q = format!(
        "SELECT ?target ?ss WHERE {{ GRAPH <{graph}> {{ VALUES ?target {{ {values} }} \
         ?target <{ots}validatedBy> ?ss }} }}",
        graph = crate::shacl_studio::bindings::VALIDATION_GRAPH,
        ots = crate::shacl_studio::bindings::OTS,
    );
    let Ok(QueryResults::Solutions(sols)) = store.query(&q) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = sols
        .flatten()
        .filter_map(|row| {
            let target = match row.get("target") {
                Some(oxigraph::model::Term::NamedNode(n)) => n.as_str().to_string(),
                _ => return None,
            };
            let ss = match row.get("ss") {
                Some(oxigraph::model::Term::NamedNode(n)) => n.as_str().to_string(),
                _ => return None,
            };
            Some((target, ss))
        })
        .collect();
    out.sort();
    out
}

/// Compute the profile of `version`. Split out from the handler so the same
/// derivation is testable without an HTTP round trip.
pub fn build_profile(
    state: &AppState,
    user_id: Option<&str>,
    model: &DataModelRecord,
    version: &DataModelVersion,
) -> OntologyProfile {
    let mut version_graphs: Vec<String> = vec![version.graph_iri.clone()];
    version_graphs.extend(version.sub_graphs.iter().cloned());
    version_graphs.sort();
    version_graphs.dedup();

    let sources = shape_sources(state, user_id, &model.id, version);
    let shape_graphs: Vec<String> = sources.iter().map(|s| s.graph_iri.clone()).collect();

    // Text and lists are read over both sets: a shape's `sh:in` members and
    // their labels may live in a bound graph the version does not contain.
    let mut all_graphs = version_graphs.clone();
    all_graphs.extend(shape_graphs.iter().cloned());
    all_graphs.sort();
    all_graphs.dedup();

    let store = &state.store;
    let labels = text_index(store, &all_graphs, "rdfs:label|skos:prefLabel|dct:title");
    let descriptions = text_index(
        store,
        &all_graphs,
        "rdfs:comment|skos:definition|dct:description",
    );
    let classes = classes(store, &version_graphs, &labels, &descriptions);
    let properties = properties(store, &version_graphs, &labels, &descriptions);
    let lists = list_cells(store, &all_graphs);
    let facts = shape_facts(store, &shape_graphs);
    let property_shapes = flatten_shapes(&facts, &lists);

    let mut enumerations = tbox_enumerations(store, &version_graphs, &lists, &labels);
    // Flattening emits one entry per (root shape, path chain), so a shape
    // reached from two roots — the normal way to share an `sh:node` target —
    // appears more than once. Its value set is still one enumeration, and
    // counting it twice would tell the proposer the ontology has more than it
    // does.
    let mut enumerated: BTreeSet<(String, Option<String>)> = BTreeSet::new();
    for shape in &property_shapes {
        if shape.in_list.is_empty() || !enumerated.insert((shape.shape.clone(), shape.path.clone()))
        {
            continue;
        }
        enumerations.push(Enumeration {
            kind: "sh_in",
            iri: None,
            path: shape.path.clone(),
            shape: Some(shape.shape.clone()),
            graph_iri: shape.graph_iri.clone(),
            values: shape.in_list.clone(),
        });
    }
    enumerations.sort_by(|a, b| {
        (a.kind, &a.iri, &a.path, &a.shape).cmp(&(b.kind, &b.iri, &b.path, &b.shape))
    });

    OntologyProfile {
        profile: PROFILE_CONTRACT,
        model: ModelRef {
            id: model.id.clone(),
            title: model.title.clone(),
            namespace: model.namespace.clone(),
            is_public: model.is_public,
        },
        version: VersionRef {
            version: version.version.clone(),
            status: version.status.as_str().to_string(),
            graph_iri: version.graph_iri.clone(),
            sub_graphs: version.sub_graphs.clone(),
        },
        shape_discovery: SHAPE_DISCOVERY,
        shape_sources: sources,
        counts: ProfileCounts {
            classes: classes.len(),
            properties: properties.len(),
            property_shapes: property_shapes.len(),
            enumerations: enumerations.len(),
        },
        classes,
        properties,
        property_shapes,
        enumerations,
    }
}

/// `GET /api/models/:id/versions/:ver/profile`
pub async fn get_model_profile(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((id, ver)): Path<(String, String)>,
) -> ApiResult<Json<OntologyProfile>> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let model = readable_model(&state, uid, &id)?;
    let version = registry::get_version(&state.store, &state.base_url, &id, &ver)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Version '{ver}' not found")))?;
    Ok(Json(build_profile(&state, uid, &model, &version)))
}

pub fn routes() -> Router<AppState> {
    // `:ver` matches the parameter name the other `/api/models/:id/versions/…`
    // routes use: the router rejects two names for the same path segment.
    Router::new().route(
        "/api/models/:id/versions/:ver/profile",
        get(get_model_profile),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edges(pairs: &[(&str, &str)]) -> BTreeMap<String, BTreeSet<String>> {
        let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (child, parent) in pairs {
            out.entry(child.to_string())
                .or_default()
                .insert(parent.to_string());
        }
        out
    }

    #[test]
    fn super_class_chain_is_transitive_and_nearest_first() {
        let direct = edges(&[("C", "B"), ("B", "A"), ("A", "Top")]);
        assert_eq!(super_class_chain(&direct, "C"), vec!["B", "A", "Top"]);
        assert_eq!(super_class_chain(&direct, "A"), vec!["Top"]);
        assert!(super_class_chain(&direct, "Top").is_empty());
    }

    #[test]
    fn super_class_chain_covers_multiple_parents_by_depth() {
        let direct = edges(&[("C", "B1"), ("C", "B2"), ("B1", "A"), ("B2", "A")]);
        // Both parents before the shared grandparent, and `A` only once.
        assert_eq!(super_class_chain(&direct, "C"), vec!["B1", "B2", "A"]);
    }

    #[test]
    fn super_class_chain_survives_a_cycle() {
        let direct = edges(&[("A", "B"), ("B", "C"), ("C", "A")]);
        assert_eq!(super_class_chain(&direct, "A"), vec!["B", "C"]);
    }

    fn cell(first: &str, rest: &str) -> (Term, String) {
        (
            Term::NamedNode(oxigraph::model::NamedNode::new(first).unwrap()),
            rest.to_string(),
        )
    }

    #[test]
    fn rdf_list_keeps_declaration_order() {
        let mut cells = BTreeMap::new();
        cells.insert("_:c1".to_string(), cell("urn:a", "_:c2"));
        cells.insert("_:c2".to_string(), cell("urn:b", "_:c3"));
        cells.insert("_:c3".to_string(), cell("urn:c", &format!("{RDF}nil")));
        let values: Vec<String> = rdf_list(&cells, "_:c1")
            .iter()
            .map(|t| enum_value(t).value)
            .collect();
        assert_eq!(values, vec!["urn:a", "urn:b", "urn:c"]);
    }

    #[test]
    fn rdf_list_stops_on_a_dangling_tail_or_a_cycle() {
        let mut cells = BTreeMap::new();
        cells.insert("_:c1".to_string(), cell("urn:a", "_:missing"));
        assert_eq!(rdf_list(&cells, "_:c1").len(), 1);

        let mut looped = BTreeMap::new();
        looped.insert("_:c1".to_string(), cell("urn:a", "_:c2"));
        looped.insert("_:c2".to_string(), cell("urn:b", "_:c1"));
        assert_eq!(rdf_list(&looped, "_:c1").len(), 2);
        assert!(rdf_list(&cells, "urn:unknown").is_empty());
    }

    #[test]
    fn datatype_is_the_literal_typed_range() {
        let xsd_date: BTreeSet<String> = BTreeSet::from([format!("{XSD}date")]);
        assert_eq!(
            datatype_for("datatype", &xsd_date).as_deref(),
            Some(format!("{XSD}date").as_str())
        );
        // An object property pointing at a class has no datatype, even though a
        // range is declared.
        let class_range: BTreeSet<String> = BTreeSet::from(["urn:ex:Thing".to_string()]);
        assert_eq!(datatype_for("object", &class_range), None);
        // A datatype property whose range is a locally defined rdfs:Datatype.
        assert_eq!(
            datatype_for("datatype", &class_range).as_deref(),
            Some("urn:ex:Thing")
        );
        assert_eq!(datatype_for("object", &BTreeSet::new()), None);
    }

    #[test]
    fn property_kind_takes_the_strongest_claim() {
        let both = BTreeSet::from([
            format!("{OWL}ObjectProperty"),
            format!("{OWL}DatatypeProperty"),
        ]);
        assert_eq!(property_kind(&both), "object");
        assert_eq!(
            property_kind(&BTreeSet::from([format!("{OWL}AnnotationProperty")])),
            "annotation"
        );
        assert_eq!(
            property_kind(&BTreeSet::from([format!("{RDF}Property")])),
            "property"
        );
    }

    /// Root shape `S` → property shape `P1` (path `p1`, `sh:node N`), and `N` →
    /// `P2` (path `p2`). Flattened, `P2` must come back with the full chain.
    fn nested_facts() -> ShapeGraphFacts {
        let mut g = ShapeGraphFacts::default();
        g.facts.insert(
            "urn:s:P1".to_string(),
            ShapeFacts {
                graph_iri: "urn:g".to_string(),
                path: Some("urn:ex:p1".to_string()),
                node: Some("urn:s:N".to_string()),
                ..Default::default()
            },
        );
        g.facts.insert(
            "urn:s:P2".to_string(),
            ShapeFacts {
                graph_iri: "urn:g".to_string(),
                path: Some("urn:ex:p2".to_string()),
                ..Default::default()
            },
        );
        g.children.insert(
            "urn:s:S".to_string(),
            BTreeSet::from(["urn:s:P1".to_string()]),
        );
        g.children.insert(
            "urn:s:N".to_string(),
            BTreeSet::from(["urn:s:P2".to_string()]),
        );
        g.target_classes.insert(
            "urn:s:S".to_string(),
            BTreeSet::from(["urn:ex:Root".to_string()]),
        );
        g.nested.insert("urn:s:N".to_string());
        g.attached.insert("urn:s:P1".to_string());
        g.attached.insert("urn:s:P2".to_string());
        g
    }

    #[test]
    fn flatten_composes_the_path_through_sh_node() {
        let out = flatten_shapes(&nested_facts(), &BTreeMap::new());
        assert_eq!(out.len(), 2, "one entry per property shape: {out:?}");
        let nested = out.iter().find(|e| e.shape == "urn:s:P2").unwrap();
        assert_eq!(nested.path_chain, vec!["urn:ex:p1", "urn:ex:p2"]);
        assert_eq!(nested.node_shape.as_deref(), Some("urn:s:S"));
        assert_eq!(nested.target_classes, vec!["urn:ex:Root"]);
        // The nested node shape is not a root of its own: it is nothing's target.
        assert!(out
            .iter()
            .all(|e| e.node_shape.as_deref() == Some("urn:s:S")));
    }

    #[test]
    fn flatten_stops_at_an_sh_node_cycle() {
        let mut g = nested_facts();
        // Make the nested shape point back at the root's branch.
        g.facts.get_mut("urn:s:P2").unwrap().node = Some("urn:s:N".to_string());
        let out = flatten_shapes(&g, &BTreeMap::new());
        assert_eq!(out.len(), 2, "a cycle must not multiply entries: {out:?}");
    }

    #[test]
    fn flatten_reports_an_unattached_property_shape() {
        let mut g = ShapeGraphFacts::default();
        g.facts.insert(
            "urn:s:Lone".to_string(),
            ShapeFacts {
                graph_iri: "urn:g".to_string(),
                path: Some("urn:ex:p".to_string()),
                ..Default::default()
            },
        );
        let out = flatten_shapes(&g, &BTreeMap::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].node_shape, None);
        assert_eq!(out[0].path_chain, vec!["urn:ex:p"]);
    }

    #[test]
    fn repeated_constraint_values_settle_deterministically() {
        let mut facts = ShapeFacts {
            graph_iri: "urn:g".to_string(),
            pattern: Some("^b".to_string()),
            min_count: Some(2),
            ..Default::default()
        };
        facts.absorb(ShapeFacts {
            pattern: Some("^a".to_string()),
            min_count: Some(1),
            max_count: Some(3),
            ..Default::default()
        });
        assert_eq!(facts.pattern.as_deref(), Some("^a"));
        assert_eq!(facts.min_count, Some(1));
        assert_eq!(facts.max_count, Some(3));
        assert_eq!(facts.graph_iri, "urn:g");
    }
}
