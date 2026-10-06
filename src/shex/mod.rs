//! ShEx 2.1 (Shape Expressions) validation.
//!
//! - [`shexc`] — the compact syntax (ShExC) and the ShapeMap language
//! - [`shexj`] — the JSON syntax (ShExJ)
//! - [`shexr`] — schemas stored as RDF (ShExR), read by `IMPORT`
//! - [`check`] — imports and the schema requirements (references, closures,
//!   stratified negation)
//! - [`validate`] — the engine: typing, partitions, node constraints,
//!   the Test semantic-action extension
//! - [`data`] — what a validation reads: a graph-scoped store view, or an
//!   in-memory graph
//!
//! The server validates through [`validate_in`], which reads only the graphs
//! of a [`GraphScope`] and resolves `IMPORT` only from graphs of the store the
//! caller may read ([`StoreImports`]) — never over the network. ShEx 2.next
//! (`EXTENDS`, `ABSTRACT`) is refused by every reader.

// The server binary compiles this module privately and uses only the request
// path; the library surface (ShExR reading, in-memory graphs, the engine's
// builder hooks) is exercised by the conformance runner and unit tests.
#![allow(dead_code, unused_imports)]

pub mod ast;
pub mod check;
pub mod data;
pub mod pattern;
pub mod report;
pub mod shexc;
pub mod shexj;
pub mod shexr;
pub mod validate;
pub mod xsd;

use std::collections::HashMap;

use oxigraph::model::{BlankNode, Graph, GraphNameRef, Literal, NamedNode, Term, Triple};

pub use ast::{Label, Schema};
pub use check::{ImportResolver, NoImports, ResolvedSchema};
pub use data::{Data, GraphData, GraphScope, StoreData};
pub use report::{ShExReport, ShExResult, ShExStatus};
pub use shexc::{MapSelector, MapShape, MapTerm};
pub use validate::{Engine, ShapeSel};

use crate::store::TripleStore;

/// Which syntax a schema text is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaFormat {
    ShExC,
    ShExJ,
    /// ShExJ when the text is a JSON object, ShExC otherwise.
    Auto,
}

impl SchemaFormat {
    /// `shexc`, `shexj` (also `json`), or `None`/`auto`.
    pub fn parse(s: Option<&str>) -> Result<SchemaFormat, String> {
        match s.map(str::to_ascii_lowercase).as_deref() {
            None | Some("auto") | Some("") => Ok(SchemaFormat::Auto),
            Some("shexc") => Ok(SchemaFormat::ShExC),
            Some("shexj") | Some("json") => Ok(SchemaFormat::ShExJ),
            Some(other) => Err(format!(
                "unknown schema format '{other}' (expected shexc or shexj)"
            )),
        }
    }
}

/// Parse a schema in either syntax.
pub fn parse_schema(
    text: &str,
    format: SchemaFormat,
    base: Option<&str>,
) -> Result<Schema, String> {
    let json = match format {
        SchemaFormat::ShExJ => true,
        SchemaFormat::ShExC => false,
        SchemaFormat::Auto => text.trim_start().starts_with('{'),
    };
    if json {
        shexj::parse(text, base)
    } else {
        shexc::parse(text, base)
    }
}

/// Parse ShExC with no base IRI.
pub fn parse_shexc(text: &str) -> Result<Schema, String> {
    shexc::parse(text, None)
}

/// The most triples an imported schema graph may hold.
pub const MAX_IMPORT_TRIPLES: usize = 100_000;

/// Resolves `IMPORT <g>` from the store: `g` must be a named graph the
/// scope reads, holding the imported schema as ShExR. Nothing is fetched.
pub struct StoreImports<'a> {
    pub store: &'a TripleStore,
    pub scope: &'a GraphScope,
}

impl ImportResolver for StoreImports<'_> {
    fn resolve(&self, iri: &str) -> Result<Schema, String> {
        let unreadable = || {
            format!(
                "no readable named graph <{iri}> holds a ShExR schema (imports are read from the store, never fetched)"
            )
        };
        if !self.scope.includes(iri) {
            return Err(unreadable());
        }
        let g = NamedNode::new(iri).map_err(|_| unreadable())?;
        let mut graph = Graph::new();
        for q in self
            .store
            .store()
            .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(g.as_ref())))
            .flatten()
        {
            graph.insert(&Triple::from(q));
            if graph.len() > MAX_IMPORT_TRIPLES {
                return Err(format!(
                    "the graph holds more than {MAX_IMPORT_TRIPLES} triples, too many for a schema"
                ));
            }
        }
        if graph.is_empty() {
            return Err(unreadable());
        }
        shexr::from_graph(&graph)
    }
}

/// A term of a ShapeMap as an RDF term.
pub fn map_term(t: &MapTerm) -> Result<Term, String> {
    Ok(match t {
        MapTerm::Iri(i) => NamedNode::new(i).map_err(|e| e.to_string())?.into(),
        MapTerm::BNode(b) => BlankNode::new(b).map_err(|e| e.to_string())?.into(),
        MapTerm::Literal(l) => match (&l.language, &l.datatype) {
            (Some(lang), _) => Literal::new_language_tagged_literal(&l.value, lang)
                .map_err(|e| e.to_string())?
                .into(),
            (None, Some(dt)) => {
                Literal::new_typed_literal(&l.value, NamedNode::new(dt).map_err(|e| e.to_string())?)
                    .into()
            }
            (None, None) => Literal::new_simple_literal(&l.value).into(),
        },
    })
}

/// The shape map of a request, in any of the forms the API accepts.
#[derive(Debug, Clone)]
pub enum ShapeMapInput {
    /// The ShapeMap language: `<n1>@<S>, {FOCUS a <T>}@START`.
    Text(String),
    /// ShapeMap JSON: `[{"node": "…", "shape": "…"}]`.
    Pairs(Vec<(String, String)>),
    /// The original form: shape label → focus nodes.
    Legacy(HashMap<String, Vec<String>>),
    /// None given: every declared shape is checked on the nodes that use
    /// one of its predicates.
    Discover,
}

fn shape_of(s: &str) -> MapShape {
    if s.eq_ignore_ascii_case("START") {
        return MapShape::Start;
    }
    let s = s.trim();
    let s = s
        .strip_prefix('<')
        .and_then(|x| x.strip_suffix('>'))
        .unwrap_or(s);
    MapShape::Label(Label::from_display(s))
}

fn node_of(s: &str, schema: &ResolvedSchema) -> Result<Term, String> {
    let s = s.trim();
    if let Some(b) = s.strip_prefix("_:") {
        return Ok(BlankNode::new(b).map_err(|e| e.to_string())?.into());
    }
    if s.starts_with('"') {
        let parsed = shexc::parse_shape_map(
            &format!("{s}@START"),
            &schema.prefixes,
            schema.base.as_deref(),
        )?;
        if let Some((MapSelector::Node(t), _)) = parsed.first() {
            return map_term(t);
        }
        return Err(format!("{s} is not a literal"));
    }
    let iri = s
        .strip_prefix('<')
        .and_then(|x| x.strip_suffix('>'))
        .unwrap_or(s);
    Ok(NamedNode::new(iri)
        .map_err(|e| format!("focus node {s}: {e}"))?
        .into())
}

/// Check a shape map's syntax and terms without reading any data.
pub fn check_shape_map(input: &ShapeMapInput, schema: &ResolvedSchema) -> Result<(), String> {
    match input {
        ShapeMapInput::Text(t) => {
            for (selector, _) in
                shexc::parse_shape_map(t, &schema.prefixes, schema.base.as_deref())?
            {
                match selector {
                    MapSelector::Node(n) => {
                        map_term(&n)?;
                    }
                    MapSelector::Subjects(p, o) => {
                        NamedNode::new(p).map_err(|e| e.to_string())?;
                        o.as_ref().map(map_term).transpose()?;
                    }
                    MapSelector::Objects(s, p) => {
                        NamedNode::new(p).map_err(|e| e.to_string())?;
                        s.as_ref().map(map_term).transpose()?;
                    }
                }
            }
        }
        ShapeMapInput::Pairs(pairs) => {
            for (node, _) in pairs {
                node_of(node, schema)?;
            }
        }
        ShapeMapInput::Legacy(map) => {
            for node in map.values().flatten() {
                node_of(node, schema)?;
            }
        }
        ShapeMapInput::Discover => {}
    }
    Ok(())
}

/// The node/shape associations a shape map asks for, resolved against the
/// data (triple-pattern selectors read the scoped data only).
pub fn associations(
    input: &ShapeMapInput,
    schema: &ResolvedSchema,
    data: &dyn Data,
) -> Result<Vec<(Term, ShapeSel)>, String> {
    let sel = |m: &MapShape| match m {
        MapShape::Start => ShapeSel::Start,
        MapShape::Label(l) => ShapeSel::Label(l.clone()),
    };
    let mut out = Vec::new();
    match input {
        ShapeMapInput::Text(t) => {
            for (selector, shape) in
                shexc::parse_shape_map(t, &schema.prefixes, schema.base.as_deref())?
            {
                let shape = sel(&shape);
                match selector {
                    MapSelector::Node(n) => out.push((map_term(&n)?, shape)),
                    MapSelector::Subjects(p, o) => {
                        let p = NamedNode::new(p).map_err(|e| e.to_string())?;
                        let o = o.as_ref().map(map_term).transpose()?;
                        for n in data.subjects(&p, o.as_ref()) {
                            out.push((n, shape.clone()));
                        }
                    }
                    MapSelector::Objects(s, p) => {
                        let p = NamedNode::new(p).map_err(|e| e.to_string())?;
                        let s = s.as_ref().map(map_term).transpose()?;
                        for n in data.objects(s.as_ref(), &p) {
                            out.push((n, shape.clone()));
                        }
                    }
                }
            }
        }
        ShapeMapInput::Pairs(pairs) => {
            for (node, shape) in pairs {
                out.push((node_of(node, schema)?, sel(&shape_of(shape))));
            }
        }
        ShapeMapInput::Legacy(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for shape in keys {
                for node in &map[shape] {
                    out.push((node_of(node, schema)?, sel(&shape_of(shape))));
                }
            }
        }
        ShapeMapInput::Discover => {
            for l in &schema.order {
                for n in validate::candidate_nodes(schema, data, &schema.shapes[l]) {
                    out.push((n, ShapeSel::Label(l.clone())));
                }
            }
        }
    }
    Ok(out)
}

/// Validate a shape map against `data`.
pub fn validate_data(
    schema: &ResolvedSchema,
    data: &dyn Data,
    input: &ShapeMapInput,
) -> Result<ShExReport, String> {
    let pairs = associations(input, schema, data)?;
    let mut engine = Engine::new(schema, data);
    let mut results = Vec::new();
    for (node, sel) in pairs {
        let verdict = engine.validate(&node, &sel);
        if let Some(why) = engine.aborted() {
            return Err(why.to_string());
        }
        let status = match verdict {
            Ok(()) => ShExStatus::Conformant,
            Err(why) => ShExStatus::NonConformant(why),
        };
        results.push(ShExResult {
            focus_node: report::node_display(&node),
            shape: match &sel {
                ShapeSel::Start => "START".to_string(),
                ShapeSel::Label(l) => l.as_display(),
            },
            status,
        });
    }
    Ok(ShExReport::new(results))
}

/// Validate against the graphs of `scope` in `store`.
pub fn validate_in(
    store: &TripleStore,
    scope: &GraphScope,
    schema: &ResolvedSchema,
    input: &ShapeMapInput,
) -> Result<ShExReport, String> {
    validate_data(schema, &StoreData { store, scope }, input)
}

/// The stack a validation thread gets: room for [`validate::MAX_DEPTH`]
/// nested evaluations.
pub const VALIDATION_STACK: usize = 512 << 20;

/// Run `f` on a thread with [`VALIDATION_STACK`] bytes of stack (validation
/// recurses through the data; a server worker's stack is far smaller).
pub fn validate_on_large_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .name("shex-validate".into())
            .stack_size(VALIDATION_STACK)
            .spawn_scoped(s, f)
            .expect("spawn a ShEx validation thread")
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p))
    })
}
