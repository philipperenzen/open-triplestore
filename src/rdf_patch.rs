//! RDF Patch (the Apache Jena / RDF Delta line format): the change substrate
//! for versions and, in due course, for streaming deltas.
//!
//! * `GET /api/datasets/:id/versions/:ver/diff/:other?format=rdf-patch` (or
//!   `Accept: application/rdf-patch`) — the diff as a patch that transforms
//!   `:ver` into `:other` (`live` for the current graphs), one transaction.
//! * `POST /api/datasets/:id/patch` — apply a patch to the dataset's graphs
//!   atomically, as one commit.
//!
//! The format (<https://afs.github.io/rdf-delta/rdf-patch.html>): rows of
//! N-Triples-like tokens, each ending with `.` — `H` (headers), `TX` / `TC` /
//! `TA` (transaction blocks; several per patch, a `TA` discards its own block
//! only), `PA` / `PD` (prefixes, the name as a keyword or a quoted string, the
//! namespace as an IRI or a string) and `A` / `D` (add / delete a triple or a
//! quad). A blank node, `_:label` or `<_:label>`, names the store's own blank
//! node with that id, so a patch can delete one and a version diff applies
//! faithfully. Two extensions: prefixed names in `A` / `D` rows expand through
//! the patch's own `PA` declarations, and a dataset patch puts triples written
//! without a graph into the graph its `?graph=` parameter names.

use std::collections::HashSet;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use oxigraph::model::vocab::xsd;
use oxigraph::model::{
    BlankNode, GraphName, GraphNameRef, Literal, NamedNode, NamedNodeRef, NamedOrBlankNode, Quad,
    Term,
};

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;
use crate::store::{escape_sparql_iri, QuadOp, TripleStore};

pub const MEDIA_TYPE: &str = "application/rdf-patch";

/// One `A` / `D` row: a triple, or a quad when it names a graph. Every term
/// was built by the oxrdf constructors, so it is a valid RDF term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchQuad {
    pub subject: NamedOrBlankNode,
    pub predicate: NamedNode,
    pub object: Term,
    /// `None`: the row was a triple.
    pub graph: Option<NamedNode>,
}

impl PatchQuad {
    /// The quad, in `default` when the row named no graph (`None` then).
    pub fn to_quad(&self, default: Option<&NamedNode>) -> Option<Quad> {
        let graph = self.graph.as_ref().or(default)?;
        Some(Quad::new(
            self.subject.clone(),
            self.predicate.clone(),
            self.object.clone(),
            GraphName::NamedNode(graph.clone()),
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Add(PatchQuad),
    Delete(PatchQuad),
}

/// A `PA` / `PD` row: a change to the prefix table of the data the patch is
/// applied to ("Prefixes do not apply to the data of the patch. They are
/// changes to the data the patch is applied to."). The name has no `:`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixOp {
    Add { name: String, namespace: String },
    Delete { name: String },
}

#[derive(Debug, Default, Clone)]
pub struct Patch {
    pub headers: Vec<(String, String)>,
    /// The prefix table the patch's own rows read their prefixed names
    /// against (name without the `:`, namespace): its `PA` / `PD` rows in
    /// order, an aborted block's rows undone. The extension that lets `A` /
    /// `D` rows use prefixed names; see [`Self::prefix_ops`] for what the
    /// rows change.
    pub prefixes: Vec<(String, String)>,
    /// The `PA` / `PD` rows of every committed block, and of rows outside any
    /// block, in patch order: the changes to the target's prefix table.
    pub prefix_ops: Vec<PrefixOp>,
    /// The `A` / `D` rows of every committed block, and of rows outside any
    /// block, in patch order.
    pub ops: Vec<Op>,
    /// Blocks closed with `TC`.
    pub committed: usize,
    /// Blocks closed with `TA`: their rows are discarded.
    pub aborted: usize,
}

impl Patch {
    pub fn id(&self) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == "id")
            .map(|(_, v)| v.as_str())
    }
    pub fn adds(&self) -> usize {
        self.ops.iter().filter(|o| matches!(o, Op::Add(_))).count()
    }
    pub fn deletes(&self) -> usize {
        self.ops
            .iter()
            .filter(|o| matches!(o, Op::Delete(_)))
            .count()
    }
    /// Every value of the header `name`, in order.
    pub fn header_values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    /// The rows as store operations, a row without a graph put in `default`.
    /// `Err` names the first row that has no graph when there is no default.
    pub fn quad_ops(&self, default: Option<&NamedNode>) -> Result<Vec<QuadOp>, String> {
        self.ops
            .iter()
            .map(|op| {
                let (q, add) = match op {
                    Op::Add(q) => (q, true),
                    Op::Delete(q) => (q, false),
                };
                let quad = q.to_quad(default).ok_or_else(|| {
                    "a row names no graph: a dataset patch applies to the dataset's registered \
                     graphs, so write quads or name the graph for triples with ?graph="
                        .to_string()
                })?;
                Ok(if add {
                    QuadOp::Add(quad)
                } else {
                    QuadOp::Remove(quad)
                })
            })
            .collect()
    }
}

// ── tokenizer ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// `<…>`, without the brackets.
    Iri(String),
    /// `_:label` or `<_:label>`: the label.
    BNode(String),
    /// A literal as written: quoted body plus any `@lang` / `^^datatype`.
    Literal(String),
    /// A keyword, an operation code or a prefixed name.
    Word(String),
}

/// Split a patch into rows: each row's line number and its tokens, the
/// terminating `.` dropped. A row ends at its `.`, not at a line break, and
/// `#` outside a term starts a comment that runs to the end of the line.
fn rows(text: &str) -> Result<Vec<(usize, Vec<Tok>)>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut row: Vec<Tok> = Vec::new();
    let mut row_line = 1;
    let mut line = 1;
    let mut i = 0;
    // A bare token runs to whitespace, so a malformed prefixed name stays
    // one token and is refused as such. A trailing `.` is the row's end, as
    // in Turtle (a prefixed name or a blank-node label cannot end with one).
    let bare = |i: &mut usize| -> (String, bool) {
        let start = *i;
        while *i < chars.len() && !chars[*i].is_whitespace() {
            *i += 1;
        }
        let word: String = chars[start..*i].iter().collect();
        match word.strip_suffix('.') {
            Some(w) if !w.is_empty() => (w.trim_end_matches('.').to_string(), true),
            _ => (word, false),
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if row.is_empty() {
            row_line = line;
        }
        let err = |m: &str| format!("line {line}: {m}");
        let mut dot = false;
        match c {
            '#' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '.' => {
                i += 1;
                dot = true;
            }
            '<' if chars.get(i + 1) == Some(&'<') => {
                return Err(err("RDF 1.2 triple terms (`<<( … )>>`) are not supported"));
            }
            '<' => {
                let start = i + 1;
                while i < chars.len() && chars[i] != '>' && chars[i] != '\n' {
                    i += 1;
                }
                if i >= chars.len() || chars[i] != '>' {
                    return Err(err("unterminated IRI"));
                }
                let body: String = chars[start..i].iter().collect();
                i += 1;
                row.push(match body.strip_prefix("_:") {
                    Some(label) => Tok::BNode(label.to_string()),
                    None => Tok::Iri(body),
                });
            }
            '"' => {
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != '"' && chars[i] != '\n' {
                    i += if chars[i] == '\\' { 2 } else { 1 };
                }
                if i >= chars.len() || chars[i] != '"' {
                    return Err(err("unterminated literal"));
                }
                i += 1;
                let body: String = chars[start..i].iter().collect();
                let mut suffix = String::new();
                if chars.get(i) == Some(&'@') {
                    let tag = i;
                    i += 1;
                    while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '-') {
                        i += 1;
                    }
                    suffix = chars[tag..i].iter().collect();
                } else if chars.get(i) == Some(&'^') && chars.get(i + 1) == Some(&'^') {
                    i += 2;
                    if chars.get(i) == Some(&'<') {
                        let dt = i;
                        while i < chars.len() && chars[i] != '>' && chars[i] != '\n' {
                            i += 1;
                        }
                        if i >= chars.len() || chars[i] != '>' {
                            return Err(err("unterminated datatype IRI"));
                        }
                        i += 1;
                        suffix = format!("^^{}", chars[dt..i].iter().collect::<String>());
                    } else {
                        // A prefixed datatype, expanded with the other terms.
                        let (dt, ends) = bare(&mut i);
                        suffix = format!("^^{dt}");
                        dot = ends;
                    }
                }
                row.push(Tok::Literal(format!("{body}{suffix}")));
            }
            '_' if chars.get(i + 1) == Some(&':') => {
                i += 2;
                let (label, ends) = bare(&mut i);
                row.push(Tok::BNode(label));
                dot = ends;
            }
            _ => {
                let (word, ends) = bare(&mut i);
                row.push(Tok::Word(word));
                dot = ends;
            }
        }
        if dot {
            if row.is_empty() {
                return Err(err("empty row"));
            }
            out.push((row_line, std::mem::take(&mut row)));
        }
    }
    if !row.is_empty() {
        return Err(format!("line {row_line}: missing terminating `.`"));
    }
    Ok(out)
}

/// Whether `name` is a Turtle `PN_PREFIX` (or empty, the default prefix).
fn is_pn_prefix(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return true;
    };
    let base = |c: char| c.is_ascii_alphabetic() || (!c.is_ascii() && c.is_alphanumeric());
    base(first)
        && !name.ends_with('.')
        && chars.all(|c| base(c) || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
}

/// Whether `local` is acceptable as the local part of a prefixed name: the
/// SPARQL `PN_LOCAL` production, minus its `\`-escapes. Letters, digits, `_`,
/// `-`, `.`, `:`, `%XX` and non-ASCII characters; nothing else — in particular
/// none of `{ } ; < > " ' \` or whitespace, which is what once let a
/// "prefixed name" carry SPARQL syntax into a generated update.
fn is_pn_local(local: &str) -> bool {
    let mut chars = local.chars();
    while let Some(c) = chars.next() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '.' | ':' => {}
            '%' => {
                for _ in 0..2 {
                    if !chars.next().is_some_and(|h| h.is_ascii_hexdigit()) {
                        return false;
                    }
                }
            }
            c if !c.is_ascii() && !c.is_whitespace() && !c.is_control() => {}
            _ => return false,
        }
    }
    true
}

/// Expand `pfx:local` through the patch's `PA` declarations into a validated
/// IRI. The prefix must have been declared (and not `PD`-removed) on an
/// earlier row.
fn expand_prefixed(t: &str, prefixes: &[(String, String)]) -> Result<NamedNode, String> {
    let Some((pfx, local)) = t.split_once(':') else {
        return Err(format!("`{t}` is not an RDF term"));
    };
    let Some((_, ns)) = prefixes.iter().find(|(p, _)| p == pfx) else {
        return Err(format!("`{t}` uses the undeclared prefix `{pfx}:`"));
    };
    if !is_pn_local(local) {
        return Err(format!(
            "`{t}` is not a prefixed name: the local part may only contain letters, digits, `_ - . : %XX`"
        ));
    }
    NamedNode::new(format!("{ns}{local}"))
        .map_err(|e| format!("`{t}` expands to an invalid IRI: {e}"))
}

/// Split a literal token into its quoted body and its `@lang` / `^^dt` suffix.
fn split_literal(t: &str) -> Result<(&str, &str), String> {
    let bytes = t.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Ok((&t[..=i], &t[i + 1..])),
            _ => i += 1,
        }
    }
    Err(format!("unterminated literal {t}"))
}

fn literal(t: &str, prefixes: &[(String, String)]) -> Result<Literal, String> {
    let (body, suffix) = split_literal(t)?;
    let full = match suffix.strip_prefix("^^") {
        Some(dt) if !dt.starts_with('<') => format!("{body}^^{}", expand_prefixed(dt, prefixes)?),
        _ => t.to_string(),
    };
    full.parse::<Literal>().map_err(|e| format!("{t}: {e}"))
}

/// A string written as a plain literal (`"…"`): its value.
fn plain_string(t: &str) -> Option<String> {
    let l = t.parse::<Literal>().ok()?;
    (l.language().is_none() && l.datatype() == xsd::STRING).then(|| l.value().to_string())
}

/// A blank node by its label, which names the store's node with that id.
fn blank_node(label: &str) -> Result<BlankNode, String> {
    BlankNode::new(label).map_err(|_| {
        format!(
            "`_:{label}` is not a blank-node label this store uses: letters, digits, `_` and `-`, \
             with `.` only inside"
        )
    })
}

/// What a term position accepts.
#[derive(Clone, Copy, PartialEq)]
enum Pos {
    Subject,
    Predicate,
    Object,
    Graph,
}

fn term(t: &Tok, prefixes: &[(String, String)], pos: Pos) -> Result<Term, String> {
    let what = match pos {
        Pos::Subject => "subject",
        Pos::Predicate => "predicate",
        Pos::Object => "object",
        Pos::Graph => "graph",
    };
    let at = |e: String| format!("{what}: {e}");
    match t {
        Tok::Iri(iri) => NamedNode::new(iri)
            .map(Term::from)
            .map_err(|e| at(format!("<{iri}>: {e}"))),
        Tok::BNode(label) if matches!(pos, Pos::Subject | Pos::Object) => {
            blank_node(label).map(Term::from).map_err(at)
        }
        Tok::BNode(label) => Err(at(format!("must not be a blank node (_:{label})"))),
        Tok::Literal(l) if pos == Pos::Object => literal(l, prefixes).map(Term::from).map_err(at),
        Tok::Literal(l) => Err(at(format!("must not be a literal ({l})"))),
        Tok::Word(w) => expand_prefixed(w, prefixes).map(Term::from).map_err(at),
    }
}

fn quad(args: &[Tok], prefixes: &[(String, String)]) -> Result<PatchQuad, String> {
    let named = |t: Term| match t {
        Term::NamedNode(n) => n,
        _ => unreachable!("a predicate or graph position only yields IRIs"),
    };
    let subject = match term(&args[0], prefixes, Pos::Subject)? {
        Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n),
        Term::BlankNode(b) => NamedOrBlankNode::BlankNode(b),
        _ => unreachable!("a subject position only yields IRIs and blank nodes"),
    };
    Ok(PatchQuad {
        subject,
        predicate: named(term(&args[1], prefixes, Pos::Predicate)?),
        object: term(&args[2], prefixes, Pos::Object)?,
        graph: match args.get(3) {
            Some(g) => Some(named(term(g, prefixes, Pos::Graph)?)),
            None => None,
        },
    })
}

/// A `PA` / `PD` prefix name: a keyword or a quoted string, without the
/// trailing `:` (one is tolerated, as earlier versions of this parser
/// required it).
fn prefix_name(t: &Tok) -> Result<String, String> {
    let name = match t {
        Tok::Word(w) => w.strip_suffix(':').unwrap_or(w).to_string(),
        Tok::Literal(l) => plain_string(l).ok_or_else(|| format!("{l} is not a prefix name"))?,
        other => return Err(format!("{other:?} is not a prefix name")),
    };
    if !is_pn_prefix(&name) {
        return Err(format!("`{name}` is not a prefix name"));
    }
    Ok(name)
}

/// A header's value: an IRI, a string's value, or the term as written.
fn header_value(t: &Tok) -> String {
    match t {
        Tok::Iri(i) => i.clone(),
        Tok::BNode(b) => format!("_:{b}"),
        Tok::Literal(l) => match l.parse::<Literal>() {
            Ok(lit) => lit.value().to_string(),
            Err(_) => l.clone(),
        },
        Tok::Word(w) => w.clone(),
    }
}

/// A prefix table: names without the `:`, and their namespaces.
type Prefixes = Vec<(String, String)>;

/// Parse a patch document.
pub fn parse(text: &str) -> Result<Patch, String> {
    let mut patch = Patch::default();
    // The open block: its rows, its prefix rows, and the prefix table to
    // restore on `TA`.
    let mut open: Option<(Vec<Op>, Vec<PrefixOp>, Prefixes)> = None;
    for (ln, toks) in rows(text)? {
        let at = |e: String| format!("line {ln}: {e}");
        let (code, args) = match toks.split_first() {
            Some((Tok::Word(code), args)) => (code.as_str(), args),
            Some((other, _)) => return Err(at(format!("unknown code `{other:?}`"))),
            None => return Err(at("empty row".into())),
        };
        match code {
            "H" => {
                if args.len() != 2 {
                    return Err(at("H needs a name and one value".into()));
                }
                let name = match &args[0] {
                    Tok::Word(w) => w.clone(),
                    Tok::Literal(l) => {
                        plain_string(l).ok_or_else(|| at(format!("{l} is not a header name")))?
                    }
                    other => return Err(at(format!("{other:?} is not a header name"))),
                };
                patch.headers.push((name, header_value(&args[1])));
            }
            "TX" => {
                if open.is_some() {
                    return Err(at(
                        "TX inside an open transaction: blocks do not nest, close it with TC or TA"
                            .into(),
                    ));
                }
                open = Some((Vec::new(), Vec::new(), patch.prefixes.clone()));
            }
            "TC" => {
                let Some((ops, prefix_ops, _)) = open.take() else {
                    return Err(at("TC without TX".into()));
                };
                patch.ops.extend(ops);
                patch.prefix_ops.extend(prefix_ops);
                patch.committed += 1;
            }
            "TA" => {
                let Some((_, _, prefixes)) = open.take() else {
                    return Err(at("TA without TX".into()));
                };
                patch.prefixes = prefixes;
                patch.aborted += 1;
            }
            "PA" => {
                if args.len() != 2 {
                    return Err(at("PA needs a prefix name and a namespace".into()));
                }
                let name = prefix_name(&args[0]).map_err(at)?;
                let ns = match &args[1] {
                    Tok::Iri(i) => i.clone(),
                    Tok::Literal(l) => {
                        plain_string(l).ok_or_else(|| at(format!("PA: {l} is not a namespace")))?
                    }
                    other => return Err(at(format!("PA: {other:?} is not a namespace"))),
                };
                NamedNode::new(&ns).map_err(|e| at(format!("PA: <{ns}>: {e}")))?;
                patch.prefixes.retain(|(p, _)| *p != name);
                patch.prefixes.push((name.clone(), ns.clone()));
                let op = PrefixOp::Add {
                    name,
                    namespace: ns,
                };
                match &mut open {
                    Some((_, prefix_ops, _)) => prefix_ops.push(op),
                    None => patch.prefix_ops.push(op),
                }
            }
            "PD" => {
                if args.len() != 1 {
                    return Err(at("PD needs a prefix name".into()));
                }
                let name = prefix_name(&args[0]).map_err(at)?;
                patch.prefixes.retain(|(p, _)| *p != name);
                let op = PrefixOp::Delete { name };
                match &mut open {
                    Some((_, prefix_ops, _)) => prefix_ops.push(op),
                    None => patch.prefix_ops.push(op),
                }
            }
            "A" | "D" => {
                if args.len() != 3 && args.len() != 4 {
                    return Err(at(format!(
                        "{code} needs a triple or quad, found {} terms",
                        args.len()
                    )));
                }
                let q = quad(args, &patch.prefixes).map_err(at)?;
                let op = if code == "D" {
                    Op::Delete(q)
                } else {
                    Op::Add(q)
                };
                match &mut open {
                    Some((ops, _, _)) => ops.push(op),
                    None => patch.ops.push(op),
                }
            }
            other => return Err(at(format!("unknown code `{other}`"))),
        }
    }
    if open.is_some() {
        return Err("transaction not closed (missing TC or TA)".into());
    }
    Ok(patch)
}

// ── generation ──────────────────────────────────────────────────────────────

fn triples_of(store: &TripleStore, graph: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    let Ok(g) = NamedNodeRef::new(graph) else {
        return set;
    };
    for q in store
        .store()
        .quads_for_pattern(None, None, None, Some(GraphNameRef::NamedNode(g)))
        .flatten()
    {
        set.insert(format!("{} {} {}", q.subject, q.predicate, q.object));
    }
    set
}

/// A patch that transforms the `from` graphs into the `to` graphs, expressed
/// against `target` graph IRIs: `(target, from, to)` per graph, where a
/// missing side is the empty graph. Blank nodes are written with the store's
/// own ids, so the patch applies faithfully to a store that holds them. It
/// has the given `H id`, an `H prev` (the patch it follows in a chain or
/// log) and the `PA` / `PD` rows that change the target's prefix table
/// (`None`: a `PD`), written first in the block.
pub fn render(
    store: &TripleStore,
    id: &str,
    prev: Option<&str>,
    headers: &[(&str, &str)],
    prefix_changes: &[(String, Option<String>)],
    mappings: &[(String, Option<String>, Option<String>)],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("H id <{}> .\n", escape_sparql_iri(id)));
    if let Some(prev) = prev {
        out.push_str(&format!("H prev <{}> .\n", escape_sparql_iri(prev)));
    }
    for (k, v) in headers {
        if v.starts_with('<') || v.starts_with('"') {
            out.push_str(&format!("H {k} {v} .\n"));
        } else if v.contains("://") || v.starts_with("urn:") {
            out.push_str(&format!("H {k} <{v}> .\n"));
        } else {
            out.push_str(&format!("H {k} \"{}\" .\n", v.replace('"', "\\\"")));
        }
    }
    out.push_str("TX .\n");
    for (name, ns) in prefix_changes {
        match ns {
            Some(ns) => out.push_str(&format!("PA \"{name}\" <{}> .\n", escape_sparql_iri(ns))),
            None => out.push_str(&format!("PD \"{name}\" .\n")),
        }
    }
    for (target, from, to) in mappings {
        let from_set = from
            .as_deref()
            .map(|g| triples_of(store, g))
            .unwrap_or_default();
        let to_set = to
            .as_deref()
            .map(|g| triples_of(store, g))
            .unwrap_or_default();
        let g = format!("<{}>", escape_sparql_iri(target));
        let mut dels: Vec<&String> = from_set.difference(&to_set).collect();
        let mut adds: Vec<&String> = to_set.difference(&from_set).collect();
        dels.sort();
        adds.sort();
        for t in dels {
            out.push_str(&format!("D {t} {g} .\n"));
        }
        for t in adds {
            out.push_str(&format!("A {t} {g} .\n"));
        }
    }
    out.push_str("TC .\n");
    out
}

/// The `PA` / `PD` rows that turn prefix table `from` into `to` (`None`: a
/// `PD`): removed labels first, then new and repointed ones, each by label.
pub fn prefix_changes(
    from: &[(String, String)],
    to: &[(String, String)],
) -> Vec<(String, Option<String>)> {
    let mut out: Vec<(String, Option<String>)> = from
        .iter()
        .filter(|(l, _)| !to.iter().any(|(t, _)| t == l))
        .map(|(l, _)| (l.clone(), None))
        .collect();
    out.sort();
    let mut added: Vec<(String, Option<String>)> = to
        .iter()
        .filter(|(l, ns)| !from.iter().any(|(f, fns)| f == l && fns == ns))
        .map(|(l, ns)| (l.clone(), Some(ns.clone())))
        .collect();
    added.sort();
    out.extend(added);
    out
}

/// Whether `name` may name a prefix in a dataset's table: a Turtle
/// `PN_PREFIX`, or empty for the default prefix (`:`).
pub fn is_prefix_name(name: &str) -> bool {
    is_pn_prefix(name)
}

// ── HTTP ────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, serde::Deserialize)]
pub struct PatchParams {
    /// A registered graph of the dataset that receives the rows written
    /// without a graph. Without it such rows are refused.
    pub graph: Option<String>,
}

/// A refused patch request: the response it is answered with, boxed so the
/// handler's `Result` stays small.
pub struct Refused(Box<Response>);

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        *self.0
    }
}

impl From<Response> for Refused {
    fn from(r: Response) -> Self {
        Refused(Box::new(r))
    }
}

impl From<crate::server::error::AppError> for Refused {
    fn from(e: crate::server::error::AppError) -> Self {
        Refused(Box::new(e.into_response()))
    }
}

fn refuse(status: StatusCode, message: impl Into<String>) -> Refused {
    Refused(Box::new((status, message.into()).into_response()))
}

/// The SHACL write gates a Graph Store write to the same graphs passes —
/// `gate_writes` pipelines, validation-layer bindings and the owning
/// dataset's `shacl_on_write` shapes — run over what each graph the patch
/// touches would hold after it. `Err` carries the first refusal's report, as
/// `writer` may see it. A failed gate lookup refuses.
fn check_gates(
    state: &AppState,
    writer: &AuthenticatedUser,
    ops: &[QuadOp],
    graphs: &[String],
) -> Result<(), crate::shacl::report::ValidationReport> {
    use crate::shacl_studio::gate;
    let studio = crate::shacl_studio::store::ShaclStudioStore::new(state.auth_db.pool());
    let ctx = gate::GateContext {
        main_store: &state.store,
        auth_db: &state.auth_db,
        studio: &studio,
        base_url: &state.base_url,
        writer: Some(writer),
    };
    for g in graphs {
        if !gate::import_gates_apply(ctx, g) {
            continue;
        }
        let name = NamedNode::new(g).map_err(gate::gate_error)?;
        let mut future: HashSet<Quad> = state
            .store
            .quads_for_graph(GraphNameRef::NamedNode(name.as_ref()))
            .map_err(gate::gate_error)?
            .into_iter()
            .collect();
        for op in ops {
            let q = op.quad();
            if q.graph_name.as_ref() != GraphNameRef::NamedNode(name.as_ref()) {
                continue;
            }
            match op {
                QuadOp::Add(q) => future.insert(q.clone()),
                QuadOp::Remove(q) => future.remove(q),
            };
        }
        let future: Vec<Quad> = future.into_iter().collect();
        gate::check_import_gates(ctx, g, &future)?;
    }
    Ok(())
}

/// What applying a patch to a dataset did.
#[derive(Debug, Default)]
pub struct Applied {
    pub added: usize,
    pub removed: usize,
    /// The registered graphs its rows touched, in first-touch order.
    pub graphs: Vec<String>,
    /// Its `PA` / `PD` rows, applied to the dataset's prefix table.
    pub prefix_rows: usize,
}

/// The dataset `dataset_id`, when `user` may write it: 404 when they may not
/// see it, 403 when they may only read it.
pub(crate) fn writable_dataset(
    state: &AppState,
    user: &AuthenticatedUser,
    dataset_id: &str,
) -> Result<crate::auth::models::Dataset, Refused> {
    let e500 = |e: anyhow::Error| refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let ds = state
        .auth_db
        .get_dataset(dataset_id)
        .map_err(e500)?
        .ok_or_else(|| refuse(StatusCode::NOT_FOUND, "Dataset not found"))?;
    if !state
        .auth_db
        .can_access_dataset(Some(&user.user_id), &ds)
        .map_err(e500)?
    {
        return Err(refuse(StatusCode::NOT_FOUND, "Dataset not found"));
    }
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err(refuse(StatusCode::FORBIDDEN, "Write access required"));
    }
    Ok(ds)
}

/// The body of a patch request as text, refusing another media type.
pub(crate) fn patch_text(headers: &HeaderMap, body: &Bytes) -> Result<String, Refused> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !(ct.is_empty() || ct.contains("rdf-patch") || ct.starts_with("text/plain")) {
        return Err(refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("send the patch as {MEDIA_TYPE} (got {ct})"),
        ));
    }
    String::from_utf8(body.to_vec()).map_err(|e| refuse(StatusCode::BAD_REQUEST, e.to_string()))
}

/// Apply `patch` to the dataset's graphs and prefix table, as `user` (who may
/// write it): every quad in one of its registered graphs (the one it names,
/// or for a triple the registered graph `default_graph` names), the SHACL
/// write gates passed, the rows in one store transaction, then the `PA` /
/// `PD` rows on the prefix table. A refusal changes nothing.
pub(crate) async fn apply_to_dataset(
    state: &AppState,
    user: &AuthenticatedUser,
    dataset_id: &str,
    patch: &Patch,
    default_graph: Option<&str>,
) -> Result<Applied, Refused> {
    let e500 = |e: anyhow::Error| refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let bad = |m: String| refuse(StatusCode::BAD_REQUEST, m);
    let registered: HashSet<String> = state
        .auth_db
        .list_dataset_graphs(dataset_id)
        .map_err(e500)?
        .into_iter()
        .collect();
    let not_registered = |iri: &str| {
        bad(format!(
            "graph <{iri}> is not registered to dataset {dataset_id}"
        ))
    };
    let default = match default_graph {
        Some(iri) => {
            if !registered.contains(iri) {
                return Err(not_registered(iri));
            }
            Some(NamedNode::new(iri).map_err(|e| bad(format!("?graph=: {e}")))?)
        }
        None => None,
    };
    let ops = patch.quad_ops(default.as_ref()).map_err(bad)?;
    let mut graphs: Vec<String> = Vec::new();
    for op in &ops {
        let GraphName::NamedNode(g) = &op.quad().graph_name else {
            unreachable!("quad_ops puts every row in a named graph");
        };
        let iri = g.as_str();
        if !registered.contains(iri) {
            return Err(not_registered(iri));
        }
        if !graphs.iter().any(|x| x == iri) {
            graphs.push(iri.to_string());
        }
    }
    let mut applied = Applied {
        graphs: graphs.clone(),
        prefix_rows: patch.prefix_ops.len(),
        ..Applied::default()
    };
    if !ops.is_empty() {
        let st = state.clone();
        let gs = graphs.clone();
        let writer = user.clone();
        let (added, removed) =
            tokio::task::spawn_blocking(move || -> Result<(usize, usize), Refused> {
                use crate::server::error::AppError;
                check_gates(&st, &writer, &ops, &gs).map_err(AppError::ValidationFailed)?;
                let before = crate::ldes::capture::before(&st, &gs);
                let counts = st.store.apply_quad_ops(&ops).map_err(AppError::from)?;
                crate::ldes::capture::after(&st, before);
                crate::entailment::after_write(&st, &gs);
                Ok(counts)
            })
            .await
            .map_err(|e| refuse(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
        applied.added = added;
        applied.removed = removed;
        for g in &graphs {
            crate::server::routes::sync_text_index_after_graph_write(state, Some(g.clone())).await;
        }
        crate::commit_log::record(
            &state.store,
            &state.base_url,
            crate::commit_log::CommitKind::Sparql,
            format!(
                "RDF Patch {}: +{added} −{removed}",
                patch.id().unwrap_or("-")
            ),
            Some(&user.user_id),
            Some(format!(
                "{}/dataset/{}",
                state.base_url.trim_end_matches('/'),
                dataset_id
            )),
            graphs,
            added,
            removed,
            None,
        );
    }
    // "Prefixes do not apply to the data of the patch. They are changes to
    // the data the patch is applied to": the dataset's prefix table.
    if !patch.prefix_ops.is_empty() {
        let rows: Vec<(String, Option<String>)> = patch
            .prefix_ops
            .iter()
            .map(|op| match op {
                PrefixOp::Add { name, namespace } => (name.clone(), Some(namespace.clone())),
                PrefixOp::Delete { name } => (name.clone(), None),
            })
            .collect();
        state
            .auth_db
            .apply_dataset_prefix_ops(dataset_id, &rows, Some(&user.user_id))
            .map_err(e500)?;
    }
    Ok(applied)
}

/// POST /api/datasets/:id/patch — apply an RDF Patch to the dataset's graphs
/// and prefix table. Not journaled: the dataset's patch log
/// (`/api/datasets/:id/log`) holds only what is appended to it.
pub async fn apply_patch_handler(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    Query(params): Query<PatchParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, Refused> {
    let Some(Extension(user)) = user else {
        return Err(refuse(StatusCode::UNAUTHORIZED, "Authentication required"));
    };
    writable_dataset(&state, &user, &dataset_id)?;
    let text = patch_text(&headers, &body)?;
    let patch = parse(&text)
        .map_err(|e| refuse(StatusCode::BAD_REQUEST, format!("invalid RDF Patch: {e}")))?;
    let id = patch.id().unwrap_or("-").to_string();
    let applied =
        apply_to_dataset(&state, &user, &dataset_id, &patch, params.graph.as_deref()).await?;
    Ok(Json(applied_json(&patch, &id, &applied)))
}

/// The response to an applied patch: the rows as written, beside the net
/// `added` / `removed` they made.
pub(crate) fn applied_json(patch: &Patch, id: &str, applied: &Applied) -> serde_json::Value {
    let transactions = serde_json::json!({
        "committed": patch.committed,
        "aborted": patch.aborted,
        "add_rows": patch.adds(),
        "delete_rows": patch.deletes(),
        "prefix_rows": applied.prefix_rows,
    });
    let changes = !patch.ops.is_empty() || !patch.prefix_ops.is_empty();
    let mut out = serde_json::json!({
        "applied": changes,
        "id": id,
        "aborted": patch.aborted > 0,
        "transactions": transactions,
        "added": applied.added,
        "removed": applied.removed,
        "graphs": applied.graphs,
    });
    if !changes {
        out["reason"] = serde_json::json!(if patch.aborted > 0 {
            "every transaction was aborted (TA)"
        } else {
            "no A/D/PA/PD rows"
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(p: &Patch, i: usize) -> &PatchQuad {
        match &p.ops[i] {
            Op::Add(q) => q,
            Op::Delete(_) => panic!("row {i} is a delete"),
        }
    }

    #[test]
    fn parses_headers_prefixes_transactions_and_quads() {
        let p = parse(
            "H id <urn:uuid:1> .\nPA ex: <http://example.org/> .\nTX .\nA ex:s ex:p \"v \\\"q\\\" .\"@en <urn:g> .\nD <urn:s> <urn:p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> <urn:g> .\nTC .\n",
        )
        .unwrap();
        assert_eq!(p.id(), Some("urn:uuid:1"));
        assert_eq!(
            p.prefixes,
            vec![("ex".to_string(), "http://example.org/".to_string())]
        );
        assert_eq!(
            (p.adds(), p.deletes(), p.committed, p.aborted),
            (1, 1, 1, 0)
        );
        let q = add(&p, 0);
        assert_eq!(q.subject.to_string(), "<http://example.org/s>");
        assert_eq!(q.object.to_string(), "\"v \\\"q\\\" .\"@en");
        assert_eq!(q.graph.as_ref().unwrap().as_str(), "urn:g");
    }

    #[test]
    fn rejects_unknown_codes_and_open_or_nested_transactions() {
        assert!(parse("X .\n").unwrap_err().contains("unknown code"));
        assert!(parse("TX .\nA <urn:s> <urn:p> <urn:o> .\n")
            .unwrap_err()
            .contains("not closed"));
        assert!(parse("A <urn:s> <urn:p> <urn:o>\n")
            .unwrap_err()
            .contains("terminating"));
        assert!(parse("TX .\nTX .\nTC .\nTC .\n")
            .unwrap_err()
            .contains("do not nest"));
        assert!(parse("TC .\n").unwrap_err().contains("without TX"));
        let aborted = parse("TX .\nA <urn:s> <urn:p> <urn:o> <urn:g> .\nTA .\n").unwrap();
        assert!(aborted.aborted == 1 && aborted.ops.is_empty());
    }

    /// A "prefixed name" is only accepted when its prefix was declared and its
    /// local part is a `PN_LOCAL`. An old check took any whitespace-free token
    /// containing `:`, so `ex:o}GRAPH<urn:x>{<a><b><c>` — SPARQL needs no
    /// whitespace between IRIs — closed the registered `GRAPH { … }` block of
    /// the update the patch was then applied as, and wrote to a graph the
    /// dataset never registered.
    #[test]
    fn prefixed_names_must_be_declared_and_well_formed() {
        let undeclared = parse("TX .\nA ex:s <urn:p> <urn:o> <urn:g> .\nTC .\n").unwrap_err();
        assert!(undeclared.contains("undeclared prefix"), "{undeclared}");

        let escape = parse(
            "PA ex: <http://example.org/> .\nTX .\nA ex:s ex:p ex:o}GRAPH<urn:x>{<urn:a><urn:b><urn:c> <urn:g> .\nTC .\n",
        )
        .unwrap_err();
        assert!(escape.contains("not a prefixed name"), "{escape}");

        for bad in [
            "ex:a;b", "ex:a<b", "ex:a>b", "ex:a\"b", "ex:a'b", "ex:a\\b", "ex:a%2", "ex:{",
        ] {
            let text = format!(
                "PA ex: <http://example.org/> .\nTX .\nA <urn:s> <urn:p> {bad} <urn:g> .\nTC .\n"
            );
            assert!(parse(&text).is_err(), "{bad} must be refused");
        }

        // A datatype may be prefixed too, and is expanded like any other IRI.
        let p = parse(
            "PA xsd: <http://www.w3.org/2001/XMLSchema#> .\nPA ex: <http://example.org/> .\nTX .\nA ex:s ex:p \"1\"^^xsd:integer ex:g .\nTC .\n",
        )
        .unwrap();
        let q = add(&p, 0);
        assert_eq!(q.subject.to_string(), "<http://example.org/s>");
        assert_eq!(
            q.object.to_string(),
            "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        );
        assert_eq!(q.graph.as_ref().unwrap().as_str(), "http://example.org/g");
        // `PD` removes the declaration for the rows after it.
        let after_pd = parse(
            "PA ex: <http://example.org/> .\nPD ex: .\nTX .\nA ex:s <urn:p> <urn:o> <urn:g> .\nTC .\n",
        )
        .unwrap_err();
        assert!(after_pd.contains("undeclared prefix"), "{after_pd}");
    }

    #[test]
    fn tokens_end_rows_without_spaces_and_comments_run_to_the_line_end() {
        let p = parse(
            "TX. # a comment\nA <urn:s> <urn:p> \"x\"@en-GB. A _:b1 <urn:p> \"1\"^^<urn:dt>.\nA <urn:s>\n  <urn:p> _:b2 .\nTC.\n",
        )
        .unwrap();
        assert_eq!(p.adds(), 3);
        assert_eq!(add(&p, 0).object.to_string(), "\"x\"@en-gb");
        assert_eq!(add(&p, 1).subject.to_string(), "_:b1");
        assert_eq!(add(&p, 2).object.to_string(), "_:b2");
    }

    #[test]
    fn generates_a_patch_from_two_graphs() {
        let store = TripleStore::in_memory().unwrap();
        store
            .update("INSERT DATA { GRAPH <urn:from> { <urn:a> <urn:p> 1 . <urn:b> <urn:p> 2 } GRAPH <urn:to> { <urn:a> <urn:p> 1 . <urn:c> <urn:p> 3 } }")
            .unwrap();
        let text = render(
            &store,
            "urn:uuid:1",
            None,
            &[("from", "v1"), ("to", "live")],
            &[],
            &[(
                "urn:target".to_string(),
                Some("urn:from".to_string()),
                Some("urn:to".to_string()),
            )],
        );
        assert!(text.contains("H from \"v1\" .\n"), "{text}");
        assert!(text.contains("D <urn:b> <urn:p> \"2\"^^<http://www.w3.org/2001/XMLSchema#integer> <urn:target> .\n"), "{text}");
        assert!(text.contains("A <urn:c> <urn:p> \"3\"^^<http://www.w3.org/2001/XMLSchema#integer> <urn:target> .\n"), "{text}");
        assert!(
            !text.contains("<urn:a>"),
            "unchanged triples are not in the patch: {text}"
        );
        // It round-trips through the parser and applies.
        let p = parse(&text).unwrap();
        assert_eq!((p.adds(), p.deletes()), (1, 1));
        store
            .update("INSERT DATA { GRAPH <urn:target> { <urn:a> <urn:p> 1 . <urn:b> <urn:p> 2 } }")
            .unwrap();
        assert_eq!(
            store.apply_quad_ops(&p.quad_ops(None).unwrap()).unwrap(),
            (1, 1)
        );
        let ask = |q: &str| {
            matches!(
                store.query(q),
                Ok(oxigraph::sparql::QueryResults::Boolean(true))
            )
        };
        assert!(ask("ASK { GRAPH <urn:target> { <urn:c> <urn:p> 3 } }"));
        assert!(!ask("ASK { GRAPH <urn:target> { <urn:b> <urn:p> 2 } }"));
    }
}
