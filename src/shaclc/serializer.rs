//! SHACL RDF → W3C SHACL Compact Syntax, losslessly or not at all.
//!
//! The compact syntax covers a subset of SHACL Core. The serializer writes
//! every triple of the shapes graph that the syntax can express, keeps track
//! of which triples it wrote, and reports every other triple as a [`Loss`]
//! instead of dropping it: [`serialize_graph`] answers [`SerializeError::Losses`]
//! unless the document carries the whole graph, and [`serialize_graph_lossy`]
//! returns the partial document together with its losses for callers that ask
//! for it explicitly.
//!
//! Losslessness is checked, not assumed: the document is parsed back and
//! must be isomorphic to the triples the serializer claims to have written.

use super::parser::parse_document;
use super::vocab::*;
use crate::store::engine::TripleStore;
use oxrdf::dataset::CanonicalizationAlgorithm;
use oxrdf::{
    BlankNodeRef, Graph, GraphNameRef, LiteralRef, NamedNodeRef, NamedOrBlankNodeRef, TermRef,
    TripleRef,
};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

/// A triple the compact syntax cannot carry, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Loss {
    /// Subject, predicate and object in N-Triples form (blank node labels
    /// are the store's and only identify the node within this report).
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerializeError {
    /// The graph holds triples the compact syntax cannot express.
    Losses(Vec<Loss>),
    /// Reading the graph failed.
    Store(String),
    /// The serializer produced a document that does not parse back to what
    /// it wrote — a bug, never a property of the input.
    Internal(String),
}

impl fmt::Display for SerializeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SerializeError::Losses(l) => write!(
                f,
                "the shapes graph cannot be written in SHACL Compact Syntax without losing {} triple{}",
                l.len(),
                if l.len() == 1 { "" } else { "s" }
            ),
            SerializeError::Store(e) => write!(f, "reading the shapes graph failed: {e}"),
            SerializeError::Internal(e) => write!(f, "SHACL-C serializer error: {e}"),
        }
    }
}

impl std::error::Error for SerializeError {}

/// Prefixes the serializer knows without a registry.
const WELL_KNOWN: &[(&str, &str)] = &[
    ("sh", SH),
    ("rdf", RDF),
    ("rdfs", RDFS),
    ("owl", OWL),
    ("xsd", XSD),
    ("schema", "http://schema.org/"),
    ("dct", "http://purl.org/dc/terms/"),
    ("foaf", "http://xmlns.com/foaf/0.1/"),
    ("skos", "http://www.w3.org/2004/02/skos/core#"),
    ("void", "http://rdfs.org/ns/void#"),
    ("dcat", "http://www.w3.org/ns/dcat#"),
];

fn well_known(ns: &str) -> Option<(String, String)> {
    WELL_KNOWN
        .iter()
        .find(|(_, n)| *n == ns)
        .map(|(p, n)| (p.to_string(), n.to_string()))
}

fn read_graph(store: &TripleStore, shapes_graph: &str) -> Result<Graph, SerializeError> {
    let g = NamedNodeRef::new(shapes_graph)
        .map_err(|e| SerializeError::Store(format!("invalid graph IRI <{shapes_graph}>: {e}")))?;
    let quads = store
        .quads_for_graph(GraphNameRef::NamedNode(g))
        .map_err(|e| SerializeError::Store(e.to_string()))?;
    let mut graph = Graph::new();
    for q in &quads {
        graph.insert(q.as_ref());
    }
    Ok(graph)
}

/// Serialize the named graph `shapes_graph`, with the built-in prefixes.
pub fn serialize(store: &TripleStore, shapes_graph: &str) -> Result<String, SerializeError> {
    serialize_with(store, shapes_graph, well_known)
}

/// Serialize the named graph `shapes_graph`; `resolve_ns` names a prefix for
/// a namespace (typically the instance's prefix registry), falling back to
/// the built-in ones.
pub fn serialize_with<F>(
    store: &TripleStore,
    shapes_graph: &str,
    resolve_ns: F,
) -> Result<String, SerializeError>
where
    F: Fn(&str) -> Option<(String, String)>,
{
    serialize_graph(&read_graph(store, shapes_graph)?, resolve_ns)
}

/// [`serialize_graph_lossy`] over a stored graph.
pub fn serialize_lossy_with<F>(
    store: &TripleStore,
    shapes_graph: &str,
    resolve_ns: F,
) -> Result<(String, Vec<Loss>), SerializeError>
where
    F: Fn(&str) -> Option<(String, String)>,
{
    serialize_graph_lossy(&read_graph(store, shapes_graph)?, resolve_ns)
}

/// The whole graph as SHACL-C, or [`SerializeError::Losses`] listing every
/// triple the syntax cannot carry.
pub fn serialize_graph<F>(graph: &Graph, resolve_ns: F) -> Result<String, SerializeError>
where
    F: Fn(&str) -> Option<(String, String)>,
{
    let (text, losses) = serialize_graph_lossy(graph, resolve_ns)?;
    if losses.is_empty() {
        Ok(text)
    } else {
        Err(SerializeError::Losses(losses))
    }
}

/// The SHACL-C document for every triple the syntax can express, and the
/// list of the triples it cannot. The document starts with a comment block
/// naming each loss, so it never reads as the whole graph.
pub fn serialize_graph_lossy<F>(
    graph: &Graph,
    resolve_ns: F,
) -> Result<(String, Vec<Loss>), SerializeError>
where
    F: Fn(&str) -> Option<(String, String)>,
{
    let mut w = Writer::new(graph, &resolve_ns);
    let body = w.document();
    let header = w.header();
    let mut text = String::new();
    let losses = w.losses();
    if !losses.is_empty() {
        text.push_str(&format!(
            "# INCOMPLETE: {} triple{} of the shapes graph have no SHACL Compact Syntax form and are NOT in this document:\n",
            losses.len(),
            if losses.len() == 1 { "" } else { "s" }
        ));
        for l in &losses {
            let line = format!(
                "{} {} {} .  ({})",
                l.subject, l.predicate, l.object, l.reason
            );
            text.push_str("#   ");
            text.push_str(&line.replace(['\n', '\r'], " "));
            text.push('\n');
        }
        text.push('\n');
    }
    text.push_str(&header);
    text.push_str(&body);

    // Verify: the document must parse back to exactly what was written.
    let mut written = Graph::new();
    for t in &w.used {
        written.insert(*t);
    }
    let mut reparsed = parse_document(&text, None)
        .map_err(|e| SerializeError::Internal(format!("output does not parse: {e}\n{text}")))?
        .graph;
    written.canonicalize(CanonicalizationAlgorithm::Unstable);
    reparsed.canonicalize(CanonicalizationAlgorithm::Unstable);
    if written != reparsed {
        return Err(SerializeError::Internal(format!(
            "output does not parse back to the triples written ({} written, {} parsed)",
            written.len(),
            reparsed.len()
        )));
    }
    Ok((text, losses))
}

// ── Lexical helpers ─────────────────────────────────────────────────────

fn is_pn_chars_base(c: char) -> bool {
    c.is_ascii_alphabetic()
        || matches!(c,
            '\u{00C0}'..='\u{00D6}' | '\u{00D8}'..='\u{00F6}' | '\u{00F8}'..='\u{02FF}'
            | '\u{0370}'..='\u{037D}' | '\u{037F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
            | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}')
}

fn is_pn_chars(c: char) -> bool {
    is_pn_chars_base(c)
        || c == '_'
        || c == '-'
        || c.is_ascii_digit()
        || c == '\u{00B7}'
        || ('\u{0300}'..='\u{036F}').contains(&c)
        || ('\u{203F}'..='\u{2040}').contains(&c)
}

fn valid_prefix_label(label: &str) -> bool {
    if label.is_empty() {
        return true;
    }
    let mut chars = label.chars();
    if !chars.next().is_some_and(is_pn_chars_base) || label.ends_with('.') {
        return false;
    }
    chars.all(|c| is_pn_chars(c) || c == '.')
}

/// A local name that needs no escapes (letters, digits, `_ - . :`, not
/// ending in `.`). Anything else is written as a full IRI instead.
fn valid_plain_local(local: &str) -> bool {
    if local.is_empty() {
        return true;
    }
    let mut chars = local.chars();
    let first = chars.next().unwrap_or(' ');
    (is_pn_chars_base(first) || first == '_' || first == ':' || first.is_ascii_digit())
        && !local.ends_with('.')
        && chars.all(|c| is_pn_chars(c) || c == '.' || c == ':')
}

fn escape_iri(iri: &str) -> String {
    let mut out = String::with_capacity(iri.len() + 2);
    out.push('<');
    for c in iri.chars() {
        if c <= ' ' || "<>\"{}|^`\\=".contains(c) {
            out.push_str(&format!("\\u{:04X}", c as u32));
        } else {
            out.push(c);
        }
    }
    out.push('>');
    out
}

fn escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn matches_integer(s: &str) -> bool {
    let d = s.strip_prefix(['+', '-']).unwrap_or(s);
    !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())
}

fn matches_decimal(s: &str) -> bool {
    let d = s.strip_prefix(['+', '-']).unwrap_or(s);
    match d.split_once('.') {
        Some((i, f)) => {
            i.chars().all(|c| c.is_ascii_digit())
                && !f.is_empty()
                && f.chars().all(|c| c.is_ascii_digit())
        }
        None => false,
    }
}

fn matches_double(s: &str) -> bool {
    let d = s.strip_prefix(['+', '-']).unwrap_or(s);
    let Some(epos) = d.find(['e', 'E']) else {
        return false;
    };
    let (mantissa, exp) = (&d[..epos], &d[epos + 1..]);
    let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
    if exp.is_empty() || !exp.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    match mantissa.split_once('.') {
        // [0-9]+ '.' [0-9]* | '.'? [0-9]+
        Some((i, f)) => {
            i.chars().all(|c| c.is_ascii_digit())
                && f.chars().all(|c| c.is_ascii_digit())
                && !(i.is_empty() && f.is_empty())
        }
        None => !mantissa.is_empty() && mantissa.chars().all(|c| c.is_ascii_digit()),
    }
}

fn valid_langtag(tag: &str) -> bool {
    let mut parts = tag.split('-');
    let first = parts.next().unwrap_or("");
    !first.is_empty()
        && first.chars().all(|c| c.is_ascii_alphabetic())
        && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric()))
}

// ── Writer ──────────────────────────────────────────────────────────────

/// A piece of output and the triples it carries.
type Piece<'g> = (String, Vec<TripleRef<'g>>);

struct Writer<'g, 'f> {
    graph: &'g Graph,
    resolve_ns: &'f dyn Fn(&str) -> Option<(String, String)>,
    /// namespace → label, for namespaces the resolver named.
    ns_labels: BTreeMap<String, String>,
    used_labels: BTreeMap<String, String>,
    obj_refs: HashMap<BlankNodeRef<'g>, usize>,
    used: HashSet<TripleRef<'g>>,
    /// Why a triple was not written, where something more specific than
    /// "no compact form" is known.
    reasons: HashMap<TripleRef<'g>, String>,
    base: Option<String>,
    imports: Vec<String>,
}

fn sh(local: &str) -> String {
    format!("{SH}{local}")
}

impl<'g, 'f> Writer<'g, 'f> {
    fn new(graph: &'g Graph, resolve_ns: &'f dyn Fn(&str) -> Option<(String, String)>) -> Self {
        let mut obj_refs = HashMap::new();
        for t in graph.iter() {
            if let TermRef::BlankNode(b) = t.object {
                *obj_refs.entry(b).or_insert(0) += 1;
            }
        }
        Writer {
            graph,
            resolve_ns,
            ns_labels: BTreeMap::new(),
            used_labels: BTreeMap::new(),
            obj_refs,
            used: HashSet::new(),
            reasons: HashMap::new(),
            base: None,
            imports: Vec::new(),
        }
    }

    fn triples_of(&self, s: NamedOrBlankNodeRef<'g>) -> Vec<TripleRef<'g>> {
        let mut v: Vec<TripleRef<'g>> = self.graph.triples_for_subject(s).collect();
        v.sort_by_key(|t| (t.predicate.as_str().to_string(), t.object.to_string()));
        v
    }

    fn inlinable(&self, b: BlankNodeRef<'g>) -> bool {
        self.obj_refs.get(&b).copied() == Some(1)
    }

    fn note(&mut self, t: TripleRef<'g>, reason: impl Into<String>) {
        self.reasons.entry(t).or_insert_with(|| reason.into());
    }

    // ── terms ──

    fn label_for(&mut self, ns: &str) -> Option<String> {
        if let Some(l) = self.ns_labels.get(ns) {
            return Some(l.clone());
        }
        let (label, declared) = (self.resolve_ns)(ns).or_else(|| well_known(ns))?;
        if declared != ns || !valid_prefix_label(&label) {
            return None;
        }
        // A label names one namespace in a document.
        if self.ns_labels.values().any(|l| *l == label) {
            return None;
        }
        self.ns_labels.insert(ns.to_string(), label.clone());
        Some(label)
    }

    fn iri(&mut self, iri: &str) -> String {
        let split = iri.rfind(['#', '/']).map(|i| i + 1);
        if let Some(i) = split {
            let (ns, local) = iri.split_at(i);
            if valid_plain_local(local) {
                if let Some(label) = self.label_for(ns) {
                    self.used_labels.insert(label.clone(), ns.to_string());
                    return format!("{label}:{local}");
                }
            }
        }
        escape_iri(iri)
    }

    fn literal(&mut self, l: LiteralRef<'g>) -> Option<String> {
        let dt = l.datatype().as_str();
        let v = l.value();
        if let Some(lang) = l.language() {
            if dt != format!("{RDF}langString") || !valid_langtag(lang) {
                return None; // directional or an unwritable tag
            }
            return Some(format!("{}@{lang}", escape_string(v)));
        }
        let local = dt.strip_prefix(XSD);
        Some(match local {
            Some("string") => escape_string(v),
            Some("boolean") if v == "true" || v == "false" => v.to_string(),
            Some("integer") if matches_integer(v) => v.to_string(),
            Some("decimal") if matches_decimal(v) => v.to_string(),
            Some("double") if matches_double(v) => v.to_string(),
            _ => {
                if dt == format!("{RDF}dirLangString") {
                    return None;
                }
                format!("{}^^{}", escape_string(v), self.iri(dt))
            }
        })
    }

    /// An `iriOrLiteral`.
    fn iri_or_literal(&mut self, t: TermRef<'g>) -> Option<String> {
        match t {
            TermRef::NamedNode(n) => Some(self.iri(n.as_str())),
            TermRef::Literal(l) => self.literal(l),
            _ => None,
        }
    }

    /// The members of a well-formed RDF list whose cells are used only here.
    fn list(&self, head: TermRef<'g>) -> Option<(Vec<TermRef<'g>>, Vec<TripleRef<'g>>)> {
        let mut items = Vec::new();
        let mut triples = Vec::new();
        let mut seen = HashSet::new();
        let mut cur = head;
        loop {
            match cur {
                TermRef::NamedNode(n) if n.as_str() == RDF_NIL => return Some((items, triples)),
                TermRef::BlankNode(b) if self.inlinable(b) && seen.insert(b) => {
                    let ts = self.triples_of(b.into());
                    if ts.len() != 2 {
                        return None;
                    }
                    let first = ts.iter().find(|t| t.predicate.as_str() == RDF_FIRST)?;
                    let rest = ts.iter().find(|t| t.predicate.as_str() == RDF_REST)?;
                    items.push(first.object);
                    cur = rest.object;
                    triples.extend(ts);
                }
                _ => return None,
            }
        }
    }

    /// `iriOrLiteralOrArray`
    fn value(&mut self, o: TermRef<'g>) -> Option<Piece<'g>> {
        if let TermRef::NamedNode(n) = o {
            if n.as_str() == RDF_NIL {
                return Some(("[]".into(), Vec::new()));
            }
        }
        if let TermRef::BlankNode(_) = o {
            let (items, triples) = self.list(o)?;
            let mut parts = Vec::new();
            for i in items {
                parts.push(self.iri_or_literal(i)?);
            }
            return Some((format!("[{}]", parts.join(" ")), triples));
        }
        Some((self.iri_or_literal(o)?, Vec::new()))
    }

    // ── paths ──
    //
    // Precedence levels: 0 alternative, 1 sequence, 2 element-or-inverse,
    // 3 element (may carry a modifier), 4 primary.

    fn path(&mut self, t: TermRef<'g>, level: u8) -> Option<Piece<'g>> {
        let paren = |s: String, need: bool| if need { format!("({s})") } else { s };
        match t {
            TermRef::NamedNode(n) => Some((self.iri(n.as_str()), Vec::new())),
            TermRef::BlankNode(b) if self.inlinable(b) => {
                let ts = self.triples_of(b.into());
                if ts.iter().any(|t| t.predicate.as_str() == RDF_FIRST) {
                    let (items, mut triples) = self.list(t)?;
                    if items.len() < 2 {
                        return None;
                    }
                    let mut parts = Vec::new();
                    for i in items {
                        let (s, tr) = self.path(i, 2)?;
                        parts.push(s);
                        triples.extend(tr);
                    }
                    return Some((paren(parts.join("/"), level > 1), triples));
                }
                if ts.len() != 1 {
                    return None;
                }
                let only = ts[0];
                let pred = only.predicate.as_str().strip_prefix(SH)?;
                let mut triples = vec![only];
                let text = match pred {
                    "alternativePath" => {
                        let (items, lt) = self.list(only.object)?;
                        if items.len() < 2 {
                            return None;
                        }
                        triples.extend(lt);
                        let mut parts = Vec::new();
                        for i in items {
                            let (s, tr) = self.path(i, 1)?;
                            parts.push(s);
                            triples.extend(tr);
                        }
                        paren(parts.join("|"), level > 0)
                    }
                    "inversePath" => {
                        let (s, tr) = self.path(only.object, 3)?;
                        triples.extend(tr);
                        paren(format!("^{s}"), level > 2)
                    }
                    "zeroOrMorePath" | "oneOrMorePath" | "zeroOrOnePath" => {
                        let m = match pred {
                            "zeroOrMorePath" => "*",
                            "oneOrMorePath" => "+",
                            _ => "?",
                        };
                        let (s, tr) = self.path(only.object, 4)?;
                        triples.extend(tr);
                        paren(format!("{s}{m}"), level > 3)
                    }
                    _ => return None,
                };
                Some((text, triples))
            }
            _ => None,
        }
    }

    // ── node shape bodies ──

    /// The constraint lines of a node shape body for `node`, skipping the
    /// triples in `skip` (already written by the caller).
    fn body(&mut self, node: NamedOrBlankNodeRef<'g>, skip: &HashSet<TripleRef<'g>>) -> Piece<'g> {
        let mut params = Vec::new();
        let mut props = Vec::new();
        let mut consumed = Vec::new();
        for t in self.triples_of(node) {
            if skip.contains(&t) {
                continue;
            }
            let is_prop = t.predicate.as_str() == sh("property");
            if let Some((text, tr)) = self.node_triple(t) {
                consumed.push(t);
                consumed.extend(tr);
                if is_prop {
                    props.push(text);
                } else {
                    params.push(text);
                }
            }
        }
        let mut lines = params;
        lines.extend(props);
        (
            lines
                .iter()
                .map(|l| format!("{l} ."))
                .collect::<Vec<_>>()
                .join("\n"),
            consumed,
        )
    }

    /// One `nodeValue` (`param=value`) from a single triple.
    fn node_value(&mut self, t: TripleRef<'g>) -> Option<Piece<'g>> {
        let param = t.predicate.as_str().strip_prefix(SH)?;
        if !is_node_param(param) {
            return None;
        }
        let (v, tr) = self.value(t.object)?;
        Some((format!("{param}={v}"), tr))
    }

    /// A blank node carrying exactly one triple, as `f(triple)`, possibly
    /// under a single `sh:not` (giving `!…`). Used for `|` members and `!`.
    fn single<F>(&mut self, member: TermRef<'g>, allow_not: bool, f: &F) -> Option<Piece<'g>>
    where
        F: Fn(&mut Self, TripleRef<'g>) -> Option<Piece<'g>>,
    {
        let TermRef::BlankNode(b) = member else {
            return None;
        };
        if !self.inlinable(b) {
            return None;
        }
        // A member's own `rdf:type sh:NodeShape` is implied (see `implied`).
        let ts: Vec<TripleRef<'g>> = self
            .triples_of(b.into())
            .into_iter()
            .filter(|t| !is_type(*t, "NodeShape"))
            .collect();
        if ts.len() != 1 {
            return None;
        }
        let only = ts[0];
        if allow_not && only.predicate.as_str() == sh("not") {
            let (s, mut tr) = self.single(only.object, false, f)?;
            tr.push(only);
            return Some((format!("!{s}"), tr));
        }
        let (s, mut tr) = f(self, only)?;
        tr.push(only);
        Some((s, tr))
    }

    /// `sh:or ( m1 m2 … )` with each member expressible by `f`.
    fn or_list<F>(&mut self, list: TermRef<'g>, f: &F) -> Option<Piece<'g>>
    where
        F: Fn(&mut Self, TripleRef<'g>) -> Option<Piece<'g>>,
    {
        let (members, mut triples) = self.list(list)?;
        if members.len() < 2 {
            return None;
        }
        let mut parts = Vec::new();
        for m in members {
            let (s, tr) = self.single(m, true, f)?;
            parts.push(s);
            triples.extend(tr);
        }
        Some((parts.join("|"), triples))
    }

    /// A triple of a node shape body (the triple itself is the caller's).
    fn node_triple(&mut self, t: TripleRef<'g>) -> Option<Piece<'g>> {
        let p = t.predicate.as_str();
        if p == sh("property") {
            let TermRef::BlankNode(b) = t.object else {
                self.note(t, "a named (IRI) property shape has no compact form; only `sh:property [ … ]` does");
                return None;
            };
            if !self.inlinable(b) {
                self.note(t, "the property shape node is shared, and the compact syntax writes each one inline once");
                return None;
            }
            return self.property(b);
        }
        if p == sh("or") {
            let r = self.or_list(t.object, &|w: &mut Self, x| w.node_value(x));
            if r.is_none() {
                self.note(t, "sh:or at node level is written `a=x|b=y`: a list of two or more blank nodes, each with one node parameter (optionally under one sh:not)");
            }
            return r;
        }
        if p == sh("not") {
            let r = self
                .single(t.object, false, &|w: &mut Self, x| w.node_value(x))
                .map(|(s, tr)| (format!("!{s}"), tr));
            if r.is_none() {
                self.note(t, "sh:not at node level is written `!param=value`: a blank node with exactly one node parameter");
            }
            return r;
        }
        let r = self.node_value(t);
        if r.is_none() {
            if let Some(local) = p.strip_prefix(SH) {
                if is_node_param(local) {
                    self.note(
                        t,
                        "the value is a blank node that is not a list of IRIs and literals",
                    );
                } else {
                    self.note(
                        t,
                        format!("sh:{local} is not a node parameter of the compact syntax"),
                    );
                }
            }
        }
        r
    }

    /// A `propertyAtom` from one triple of a property shape (or of a `|` /
    /// `!` member inside one).
    fn atom(&mut self, t: TripleRef<'g>) -> Option<Piece<'g>> {
        let local = t.predicate.as_str().strip_prefix(SH)?;
        match (local, t.object) {
            ("datatype", TermRef::NamedNode(n)) if is_builtin_datatype(n.as_str()) => {
                Some((self.iri(n.as_str()), Vec::new()))
            }
            ("class", TermRef::NamedNode(n)) if !is_builtin_datatype(n.as_str()) => {
                Some((self.iri(n.as_str()), Vec::new()))
            }
            ("nodeKind", TermRef::NamedNode(n))
                if n.as_str().strip_prefix(SH).is_some_and(is_node_kind) =>
            {
                Some((n.as_str()[SH.len()..].to_string(), Vec::new()))
            }
            ("node", TermRef::NamedNode(n)) => {
                Some((format!("@{}", self.iri(n.as_str())), Vec::new()))
            }
            ("node", TermRef::BlankNode(b)) if self.inlinable(b) => {
                let (body, tr) = self.body(b.into(), &HashSet::new());
                let text = if body.is_empty() {
                    "{ }".to_string()
                } else {
                    format!("{{\n{}\n}}", indent(&body))
                };
                Some((text, tr))
            }
            (param, o) if is_property_param(param) => {
                let (v, tr) = self.value(o)?;
                Some((format!("{param}={v}"), tr))
            }
            _ => None,
        }
    }

    fn property(&mut self, b: BlankNodeRef<'g>) -> Option<Piece<'g>> {
        let ts = self.triples_of(b.into());
        let paths: Vec<TripleRef<'g>> = ts
            .iter()
            .copied()
            .filter(|t| t.predicate.as_str() == sh("path"))
            .collect();
        if paths.len() != 1 {
            for t in &ts {
                self.note(
                    *t,
                    "a property shape needs exactly one sh:path to be written",
                );
            }
            return None;
        }
        let Some((path, mut consumed)) = self.path(paths[0].object, 0) else {
            for t in &ts {
                self.note(*t, "the property shape's path has no compact form (a shared or malformed path node)");
            }
            return None;
        };
        consumed.push(paths[0]);

        let integer = format!("{XSD}integer");
        let count_of = |t: &TripleRef<'g>| match t.object {
            TermRef::Literal(l)
                if l.datatype().as_str() == integer && matches_integer(l.value()) =>
            {
                Some(l.value().to_string())
            }
            _ => None,
        };
        let mins: Vec<TripleRef<'g>> = ts
            .iter()
            .copied()
            .filter(|t| t.predicate.as_str() == sh("minCount") && !is_zero_min_count(*t))
            .collect();
        let maxs: Vec<TripleRef<'g>> = ts
            .iter()
            .copied()
            .filter(|t| t.predicate.as_str() == sh("maxCount"))
            .collect();
        let mut min = None;
        let mut max = None;
        if let [m] = mins.as_slice() {
            match count_of(m) {
                Some(v) => {
                    min = Some(v);
                    consumed.push(*m);
                }
                None => self.note(*m, "a count is written `[min..max]` with an xsd:integer"),
            }
        } else {
            for m in &mins {
                self.note(*m, "`[min..max]` carries one sh:minCount");
            }
        }
        if let [m] = maxs.as_slice() {
            match count_of(m) {
                Some(v) => {
                    max = Some(v);
                    consumed.push(*m);
                }
                None => self.note(*m, "a count is written `[min..max]` with an xsd:integer"),
            }
        } else {
            for m in &maxs {
                self.note(*m, "`[min..max]` carries one sh:maxCount");
            }
        }

        let mut parts = vec![path];
        if min.is_some() || max.is_some() {
            parts.push(format!(
                "[{}..{}]",
                min.as_deref().unwrap_or("0"),
                max.as_deref().unwrap_or("*")
            ));
        }
        for t in ts {
            let p = t.predicate.as_str();
            if p == sh("path") || p == sh("minCount") || p == sh("maxCount") {
                continue;
            }
            let piece = if p == sh("or") {
                let r = self.or_list(t.object, &|w: &mut Self, x| w.atom(x));
                if r.is_none() {
                    self.note(t, "sh:or in a property shape is written `a|b`: a list of two or more blank nodes, each with one constraint (optionally under one sh:not)");
                }
                r
            } else if p == sh("not") {
                let r = self
                    .single(t.object, false, &|w: &mut Self, x| w.atom(x))
                    .map(|(s, tr)| (format!("!{s}"), tr));
                if r.is_none() {
                    self.note(t, "sh:not in a property shape is written `!x`: a blank node with exactly one constraint (sh:not with a named shape has no compact form)");
                }
                r
            } else {
                let r = self.atom(t);
                if r.is_none() {
                    if p == RDF_TYPE {
                        self.note(t, "the compact syntax types no property shape: the rdf:type triple is not written");
                    } else if let Some(local) = p.strip_prefix(SH) {
                        if is_property_param(local) {
                            self.note(
                                t,
                                "the value is a blank node that is not a list of IRIs and literals",
                            );
                        } else {
                            self.note(
                                t,
                                format!(
                                    "sh:{local} is not a property parameter of the compact syntax"
                                ),
                            );
                        }
                    }
                }
                r
            };
            if let Some((s, tr)) = piece {
                parts.push(s);
                consumed.push(t);
                consumed.extend(tr);
            }
        }
        Some((parts.join(" "), consumed))
    }

    // ── document ──

    fn commit(&mut self, triples: &[TripleRef<'g>]) {
        self.used.extend(triples.iter().copied());
    }

    fn document(&mut self) -> String {
        let mut out = String::new();
        let rdf_type = NamedNodeRef::new_unchecked(RDF_TYPE);

        // BASE / IMPORTS from the one owl:Ontology.
        let mut ontologies: Vec<NamedNodeRef<'g>> = self
            .graph
            .subjects_for_predicate_object(rdf_type, NamedNodeRef::new_unchecked(OWL_ONTOLOGY))
            .filter_map(|s| match s {
                NamedOrBlankNodeRef::NamedNode(n) => Some(n),
                _ => None,
            })
            .collect();
        ontologies.sort_by_key(|n| n.as_str().to_string());
        if let Some(o) = ontologies.first().copied() {
            self.base = Some(o.as_str().to_string());
            let mut consumed = vec![TripleRef::new(
                o,
                rdf_type,
                NamedNodeRef::new_unchecked(OWL_ONTOLOGY),
            )];
            for t in self.triples_of(o.into()) {
                if t.predicate.as_str() == OWL_IMPORTS {
                    if let TermRef::NamedNode(i) = t.object {
                        self.imports.push(i.as_str().to_string());
                        consumed.push(t);
                    }
                }
            }
            self.commit(&consumed);
            for other in ontologies.iter().skip(1) {
                for t in self.triples_of((*other).into()) {
                    self.note(
                        t,
                        "a SHACL-C document has one BASE, so it names one owl:Ontology",
                    );
                }
            }
        }

        // Shapes: IRI subjects typed sh:NodeShape.
        let node_shape = NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#NodeShape");
        let rdfs_class = NamedNodeRef::new_unchecked(RDFS_CLASS);
        let mut shapes: Vec<NamedOrBlankNodeRef<'g>> = self
            .graph
            .subjects_for_predicate_object(rdf_type, node_shape)
            .collect();
        shapes.sort_by_key(|s| s.to_string());
        for s in shapes {
            let NamedOrBlankNodeRef::NamedNode(iri) = s else {
                for t in self.triples_of(s) {
                    self.note(
                        t,
                        "a top-level node shape must be an IRI: `shape <iri> { … }`",
                    );
                }
                continue;
            };
            let ts = self.triples_of(s);
            let type_ns = TripleRef::new(iri, rdf_type, node_shape);
            let type_class = TripleRef::new(iri, rdf_type, rdfs_class);
            let targets: Vec<TripleRef<'g>> = ts
                .iter()
                .copied()
                .filter(|t| t.predicate.as_str() == sh("targetClass"))
                .collect();
            // `skip`: triples the header handles (written or reported);
            // `written`: the ones it writes.
            let mut skip: HashSet<TripleRef<'g>> = HashSet::new();
            let mut written = vec![type_ns];
            skip.insert(type_ns);
            let header = if self.graph.contains(type_class) && targets.is_empty() {
                skip.insert(type_class);
                written.push(type_class);
                format!("shapeClass {}", self.iri(iri.as_str()))
            } else {
                let mut h = format!("shape {}", self.iri(iri.as_str()));
                let mut classes = Vec::new();
                for t in &targets {
                    skip.insert(*t);
                    if let TermRef::NamedNode(c) = t.object {
                        classes.push(self.iri(c.as_str()));
                        written.push(*t);
                    } else {
                        self.note(*t, "`->` takes IRIs; a blank-node or literal target class has no compact form");
                    }
                }
                if !classes.is_empty() {
                    h.push_str(" -> ");
                    h.push_str(&classes.join(" "));
                }
                if self.graph.contains(type_class) {
                    self.note(type_class, "`shapeClass` cannot carry sh:targetClass, and `shape … ->` types no rdfs:Class");
                    skip.insert(type_class);
                }
                h
            };
            self.commit(&written);
            let (body, consumed) = self.body(s, &skip);
            self.commit(&consumed);
            if body.is_empty() {
                out.push_str(&format!("{header} {{\n}}\n\n"));
            } else {
                out.push_str(&format!("{header} {{\n{}\n}}\n\n", indent(&body)));
            }
        }
        out
    }

    fn header(&mut self) -> String {
        let mut h = String::new();
        if let Some(b) = &self.base {
            h.push_str(&format!("BASE {}\n", escape_iri(b)));
        }
        for i in &self.imports {
            h.push_str(&format!("IMPORTS {}\n", escape_iri(i)));
        }
        if !h.is_empty() {
            h.push('\n');
        }
        for (label, ns) in &self.used_labels {
            h.push_str(&format!("PREFIX {label}: {}\n", escape_iri(ns)));
        }
        if !self.used_labels.is_empty() {
            h.push('\n');
        }
        h
    }

    /// Whether `t` is one of the few triples whose omission leaves the
    /// validation semantics unchanged, given what was written:
    ///
    /// - `rdf:type sh:PropertyShape` on a property shape whose `sh:path` was
    ///   written (a property shape is whatever has an `sh:path`, SHACL §2.2);
    /// - `rdf:type sh:NodeShape` on a blank-node shape that was written (a
    ///   nested `{ … }` body or a `|` / `!` member) and has no `sh:path`;
    /// - `sh:minCount 0` on a written property shape (it holds for every
    ///   focus node; `[0..n]` writes no triple for it).
    ///
    /// They are neither written nor reported. docs/shacl.md lists the same
    /// set under "Implied triples"; keep the two in step.
    fn implied(&self, t: TripleRef<'g>, written_bnodes: &HashSet<BlankNodeRef<'g>>) -> bool {
        let NamedOrBlankNodeRef::BlankNode(b) = t.subject else {
            return false;
        };
        let path_written = || {
            self.graph
                .triples_for_subject(b)
                .any(|x| x.predicate.as_str() == sh("path") && self.used.contains(&x))
        };
        if is_type(t, "PropertyShape") || is_zero_min_count(t) {
            return path_written();
        }
        if is_type(t, "NodeShape") {
            let has_path = self
                .graph
                .triples_for_subject(b)
                .any(|x| x.predicate.as_str() == sh("path"));
            return !has_path && written_bnodes.contains(&b);
        }
        false
    }

    fn losses(&self) -> Vec<Loss> {
        let mut written_bnodes: HashSet<BlankNodeRef<'g>> = HashSet::new();
        for t in &self.used {
            if let NamedOrBlankNodeRef::BlankNode(b) = t.subject {
                written_bnodes.insert(b);
            }
            if let TermRef::BlankNode(b) = t.object {
                written_bnodes.insert(b);
            }
        }
        let mut out: Vec<Loss> = self
            .graph
            .iter()
            .filter(|t| !self.used.contains(t) && !self.implied(*t, &written_bnodes))
            .map(|t| Loss {
                subject: t.subject.to_string(),
                predicate: t.predicate.to_string(),
                object: t.object.to_string(),
                reason: self
                    .reasons
                    .get(&t)
                    .cloned()
                    .unwrap_or_else(|| default_reason(t)),
            })
            .collect();
        out.sort_by(|a, b| {
            (&a.subject, &a.predicate, &a.object).cmp(&(&b.subject, &b.predicate, &b.object))
        });
        out
    }
}

/// `?s rdf:type sh:<local>`.
fn is_type(t: TripleRef<'_>, local: &str) -> bool {
    t.predicate.as_str() == RDF_TYPE
        && matches!(t.object, TermRef::NamedNode(n) if n.as_str().strip_prefix(SH) == Some(local))
}

/// `?s sh:minCount 0` (any integer lexical form of zero).
fn is_zero_min_count(t: TripleRef<'_>) -> bool {
    t.predicate.as_str() == sh("minCount")
        && matches!(t.object, TermRef::Literal(l)
            if l.datatype().as_str().strip_prefix(XSD) == Some("integer")
                && l.value().parse::<i64>() == Ok(0))
}

fn default_reason(t: TripleRef<'_>) -> String {
    let p = t.predicate.as_str();
    if p == RDF_FIRST || p == RDF_REST {
        "part of an RDF list the compact syntax could not write".into()
    } else if t.subject.is_blank_node() {
        "on a blank node the compact syntax could not write inline (not a property shape, list, path or nested shape it can express)".into()
    } else {
        "the subject is not a node shape (`shape`/`shapeClass`) or the ontology named by BASE"
            .into()
    }
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("\t{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxigraph::io::{RdfFormat, RdfParser};

    fn graph(ttl: &str) -> Graph {
        let mut g = Graph::new();
        for q in RdfParser::from_format(RdfFormat::Turtle).for_slice(ttl.as_bytes()) {
            g.insert(q.expect("turtle").as_ref());
        }
        g
    }

    const PFX: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.org/> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n";

    fn resolve(ns: &str) -> Option<(String, String)> {
        (ns == "http://example.org/").then(|| ("ex".to_string(), ns.to_string()))
    }

    #[test]
    fn lossless_shape_round_trips() {
        let g = graph(&format!(
            "{PFX}ex:S a sh:NodeShape ; sh:targetClass ex:A, ex:B ; sh:closed true ; sh:ignoredProperties ( rdf:type ) ;
             sh:property [ sh:path ( ex:a [ sh:zeroOrMorePath ex:b ] ) ; sh:minCount 1 ; sh:datatype xsd:string ; sh:pattern \"a\\\\d\\n\" ] ;
             sh:property [ sh:path [ sh:inversePath ex:c ] ; sh:class ex:C ; sh:or ( [ sh:datatype xsd:string ] [ sh:not [ sh:class ex:D ] ] ) ] ."
        ));
        let text = serialize_graph(&g, resolve).expect("lossless");
        assert!(text.contains("shape ex:S -> ex:A ex:B {"), "{text}");
        assert!(text.contains("ex:a/ex:b*"), "{text}");
        assert!(text.contains("^ex:c"), "{text}");
        assert!(text.contains("xsd:string|!ex:D"), "{text}");
    }

    #[test]
    fn losses_are_listed_not_dropped() {
        let g = graph(&format!(
            "{PFX}ex:S a sh:NodeShape ; sh:sparql [ sh:select \"SELECT $this WHERE {{}}\" ] ;
             sh:property [ a sh:PropertyShape ; sh:path ex:p ; sh:minCount 0 ; sh:name \"p\" ] ."
        ));
        match serialize_graph(&g, resolve) {
            Err(SerializeError::Losses(l)) => {
                let preds: Vec<&str> = l.iter().map(|x| x.predicate.as_str()).collect();
                assert!(preds.iter().any(|p| p.contains("sparql")), "{l:?}");
                assert!(preds.iter().any(|p| p.contains("#name")), "{l:?}");
                // Implied triples are not losses.
                assert!(!preds.iter().any(|p| p.contains("minCount")), "{l:?}");
                assert!(
                    !preds.iter().any(|p| p.contains("22-rdf-syntax-ns#type")),
                    "{l:?}"
                );
            }
            other => panic!("expected losses, got {other:?}"),
        }
        let (text, losses) = serialize_graph_lossy(&g, resolve).expect("lossy");
        assert!(text.starts_with("# INCOMPLETE"), "{text}");
        assert!(!losses.is_empty());
    }

    #[test]
    fn datatype_outside_the_builtin_set_is_written_with_its_parameter() {
        let g = graph(&format!(
            "{PFX}ex:S a sh:NodeShape ; sh:property [ sh:path ex:g ; sh:datatype ex:wkt ] ; sh:property [ sh:path ex:h ; sh:class xsd:string ] ."
        ));
        let text = serialize_graph(&g, resolve).expect("lossless");
        assert!(text.contains("datatype=ex:wkt"), "{text}");
        assert!(text.contains("class=xsd:string"), "{text}");
    }

    #[test]
    fn iris_with_characters_iriref_forbids_are_escaped() {
        let g = graph(&format!("{PFX}<http://example.org/x?a=b> a sh:NodeShape ."));
        let text = serialize_graph(&g, resolve).expect("lossless");
        assert!(text.contains("\\u003D"), "{text}");
    }

    /// `ttl` serializes without a loss, and the re-parsed document holds
    /// every triple but the `implied` ones.
    fn implied_round_trip(ttl: &str, implied: usize) {
        let g = graph(&format!("{PFX}{ttl}"));
        let text = serialize_graph(&g, resolve).unwrap_or_else(|e| panic!("{e:?}"));
        let back = parse_document(&text, None).expect("re-parse").graph;
        assert_eq!(
            back.len() + implied,
            g.len(),
            "only the implied triples are omitted:\n{text}"
        );
    }

    #[test]
    fn property_shape_type_is_implied() {
        implied_round_trip(
            "ex:S a sh:NodeShape ; sh:property [ a sh:PropertyShape ; sh:path ex:p ; sh:minCount 1 ] .",
            1,
        );
    }

    #[test]
    fn node_shape_type_on_nested_and_member_shapes_is_implied() {
        implied_round_trip(
            "ex:S a sh:NodeShape ;
               sh:property [ sh:path ex:p ;
                 sh:node [ a sh:NodeShape ; sh:property [ sh:path ex:q ; sh:minCount 1 ] ] ;
                 sh:or ( [ a sh:NodeShape ; sh:datatype xsd:string ] [ a sh:NodeShape ; sh:class ex:C ] ) ] ;
               sh:not [ a sh:NodeShape ; sh:class ex:D ] .",
            4,
        );
    }

    #[test]
    fn min_count_zero_is_implied() {
        implied_round_trip(
            "ex:S a sh:NodeShape ; sh:property [ sh:path ex:p ; sh:minCount 0 ; sh:maxCount 3 ] .",
            1,
        );
    }

    #[test]
    fn implied_only_where_the_shape_is_written() {
        // A named property shape cannot be written, so its type is a loss
        // along with the rest of it, not an implied triple.
        let g = graph(&format!(
            "{PFX}ex:S a sh:NodeShape ; sh:property ex:P . ex:P a sh:PropertyShape ; sh:path ex:p ; sh:minCount 0 ."
        ));
        match serialize_graph(&g, resolve) {
            Err(SerializeError::Losses(l)) => {
                assert!(
                    l.iter().any(|x| x.object.contains("PropertyShape")),
                    "{l:?}"
                );
                assert!(l.iter().any(|x| x.predicate.contains("minCount")), "{l:?}");
            }
            other => panic!("expected losses, got {other:?}"),
        }
    }

    #[test]
    fn double_lexical_forms() {
        assert!(matches_double("1e3"));
        assert!(matches_double("1.5E-3"));
        assert!(matches_double(".5e1"));
        assert!(matches_double("1.e2"));
        assert!(!matches_double("1.5"));
        assert!(!matches_double("e3"));
        assert!(!matches_double("INF"));
    }
}
