//! RDF Patch (the Apache Jena / RDF Delta line format): the change substrate
//! for versions and, in due course, for streaming deltas.
//!
//! * `GET /api/datasets/:id/versions/:ver/diff/:other?format=rdf-patch` (or
//!   `Accept: application/rdf-patch`) — the diff as a patch that transforms
//!   `:ver` into `:other` (`live` for the current graphs), one transaction.
//! * `POST /api/datasets/:id/patch` — apply a patch to the dataset's graphs
//!   atomically, as one commit.
//!
//! Supported lines: `H` (headers), `TX` / `TC` / `TA` (one transaction;
//! `TA` aborts, nothing is applied), `PA` / `PD` (prefixes), `A` / `D`
//! (add / delete a quad — the graph term is required here, since a dataset
//! patch may only touch the dataset's registered graphs). Terms are IRIs,
//! prefixed names, literals, blank nodes and RDF 1.2 triple terms
//! (`<<( s p o )>>`, as an object).
//!
//! A patch is applied through the store's `apply_delta` primitive, not as
//! SPARQL: one transaction, the net effect of its lines (the last line on a
//! quad wins), recorded in the change log. That is what lets a `D` line name
//! a blank node — `DELETE DATA` cannot — with one caveat: `_:label` addresses
//! a stored blank node only when it is the store's own label, as a patch this
//! store exported carries it; a hand-written label matches nothing.
//!
//! Headers the store writes and reads (delta note §5.1): `H id`, `H prev`
//! (the patch this one inverts), `H seq` / `H epoch` (a change's place in the
//! change log), `H dataset`, `H commit`, and `H base-digest` — on apply, the
//! set hash (`store::set_hash`) the graphs the patch names must have before
//! it applies, else 412.

use std::collections::{BTreeSet, HashMap, HashSet};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{BlankNode, GraphName, GraphNameRef, Literal, NamedNode, NamedNodeRef, Quad};

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;
use crate::store::{escape_sparql_iri, TripleStore};

pub const MEDIA_TYPE: &str = "application/rdf-patch";

/// One quad of the patch, each term in canonical N-Triples form: IRIs as
/// `<…>` (prefixed names expanded through the patch's own `PA` declarations),
/// blank nodes as `_:label`, literals quoted and escaped. Every term went
/// through the oxrdf constructors in [`parse`], so [`to_sparql_update`] can
/// concatenate them: nothing in a term can close a `GRAPH { … }` block or
/// start another operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuadText {
    pub s: String,
    pub p: String,
    pub o: String,
    pub g: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Add(QuadText),
    Delete(QuadText),
}

#[derive(Debug, Default, Clone)]
pub struct Patch {
    pub headers: Vec<(String, String)>,
    pub prefixes: Vec<(String, String)>,
    pub ops: Vec<Op>,
    /// A `TA` line was seen: the transaction is void.
    pub aborted: bool,
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
    pub fn graphs(&self) -> BTreeSet<Option<String>> {
        self.ops
            .iter()
            .map(|o| match o {
                Op::Add(q) | Op::Delete(q) => q.g.clone(),
            })
            .collect()
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    /// `H base-digest`: the set hash the patch's graphs must have before it
    /// applies.
    pub fn base_digest(&self) -> Option<&str> {
        self.header("base-digest")
    }

    /// The lines as quads, reduced to their net effect: the last line on a
    /// quad decides whether it ends up present (`A`) or absent (`D`), as
    /// applying the lines one by one would. Returns `(to_add, to_remove)`,
    /// which never share a quad.
    pub fn net_quads(&self) -> Result<(Vec<Quad>, Vec<Quad>), String> {
        let mut last: Vec<(Quad, bool)> = Vec::new();
        let mut index: HashMap<Quad, usize> = HashMap::new();
        for op in &self.ops {
            let (add, q) = match op {
                Op::Add(q) => (true, q),
                Op::Delete(q) => (false, q),
            };
            let quad = q.to_quad()?;
            match index.get(&quad) {
                Some(&i) => last[i].1 = add,
                None => {
                    index.insert(quad.clone(), last.len());
                    last.push((quad, add));
                }
            }
        }
        let (add, remove): (Vec<_>, Vec<_>) = last.into_iter().partition(|(_, add)| *add);
        Ok((
            add.into_iter().map(|(q, _)| q).collect(),
            remove.into_iter().map(|(q, _)| q).collect(),
        ))
    }

    /// The patch that undoes this one: `A` and `D` swapped, lines in reverse
    /// order, a fresh `H id`, and `H prev` naming this patch.
    pub fn inverse(&self) -> Patch {
        let mut headers: Vec<(String, String)> = vec![(
            "id".to_string(),
            format!("urn:uuid:{}", uuid::Uuid::new_v4()),
        )];
        if let Some(id) = self.id() {
            headers.push(("prev".to_string(), id.to_string()));
        }
        headers.extend(
            self.headers
                .iter()
                .filter(|(k, _)| k != "id" && k != "prev" && k != "base-digest")
                .cloned(),
        );
        Patch {
            headers,
            prefixes: Vec::new(),
            ops: self
                .ops
                .iter()
                .rev()
                .map(|op| match op {
                    Op::Add(q) => Op::Delete(q.clone()),
                    Op::Delete(q) => Op::Add(q.clone()),
                })
                .collect(),
            aborted: self.aborted,
        }
    }

    /// The patch as text: its headers and one transaction, every term in
    /// canonical form (no prefix lines needed).
    pub fn render(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.headers {
            out.push_str(&format!("H {k} {} .\n", header_value(v)));
        }
        out.push_str("TX .\n");
        for op in &self.ops {
            let (code, q) = match op {
                Op::Add(q) => ("A", q),
                Op::Delete(q) => ("D", q),
            };
            match &q.g {
                Some(g) => out.push_str(&format!("{code} {} {} {} {g} .\n", q.s, q.p, q.o)),
                None => out.push_str(&format!("{code} {} {} {} .\n", q.s, q.p, q.o)),
            }
        }
        out.push_str(if self.aborted { "TA .\n" } else { "TC .\n" });
        out
    }
}

/// A header value as a patch term: IRIs (and `urn:` names) in angle
/// brackets, anything else as a quoted literal.
fn header_value(v: &str) -> String {
    if v.starts_with('<') || v.starts_with('"') {
        v.to_string()
    } else if v.contains("://") || v.starts_with("urn:") {
        format!("<{v}>")
    } else {
        format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

impl QuadText {
    /// The quad these canonical terms spell.
    pub fn to_quad(&self) -> Result<Quad, String> {
        let line = match &self.g {
            Some(g) => format!("{} {} {} {g} .\n", self.s, self.p, self.o),
            None => format!("{} {} {} .\n", self.s, self.p, self.o),
        };
        RdfParser::from_format(RdfFormat::NQuads)
            .for_reader(line.as_bytes())
            .next()
            .ok_or_else(|| format!("not a quad: {}", line.trim_end()))?
            .map_err(|e| format!("{}: {e}", line.trim_end()))
    }

    pub fn from_quad(q: &Quad) -> QuadText {
        QuadText {
            s: q.subject.to_string(),
            p: q.predicate.to_string(),
            o: q.object.to_string(),
            g: match &q.graph_name {
                GraphName::NamedNode(n) => Some(n.to_string()),
                _ => None,
            },
        }
    }
}

/// One change of the change log as a patch: its removed quads as `D` lines,
/// then its added quads as `A` lines, with its place in the log — `H seq`,
/// `H epoch` — and `H dataset` / `H commit` when known. The id is the
/// change's own (`urn:ots:change:{epoch}:{seq}`), so fetching it twice gives
/// the same patch. `None` for a row that carries no quads (`counts` or
/// `unknown`).
pub fn change_patch(
    row: &crate::store::changes::ChangeRow,
    dataset_iri: Option<&str>,
) -> Option<Patch> {
    use crate::store::changes::{decode_text, Extent};
    if row.extent != Extent::Full {
        return None;
    }
    let seq = row.seq?;
    let mut headers = vec![
        (
            "id".to_string(),
            format!("urn:ots:change:{}:{seq}", row.epoch),
        ),
        ("seq".to_string(), seq.to_string()),
        ("epoch".to_string(), row.epoch.clone()),
    ];
    if let Some(d) = dataset_iri {
        headers.push(("dataset".to_string(), d.to_string()));
    }
    if let Some(c) = &row.commit_iri {
        headers.push(("commit".to_string(), c.clone()));
    }
    let mut ops: Vec<Op> = Vec::new();
    let mut removed: Vec<QuadText> = decode_text(row.removed.as_deref())
        .iter()
        .map(QuadText::from_quad)
        .collect();
    let mut added: Vec<QuadText> = decode_text(row.added.as_deref())
        .iter()
        .map(QuadText::from_quad)
        .collect();
    let key = |q: &QuadText| format!("{} {} {}", q.s, q.p, q.o);
    removed.sort_by_key(key);
    added.sort_by_key(key);
    ops.extend(removed.into_iter().map(Op::Delete));
    ops.extend(added.into_iter().map(Op::Add));
    Some(Patch {
        headers,
        prefixes: Vec::new(),
        ops,
        aborted: false,
    })
}

/// The patch that undoes one change: [`change_patch`] inverted, with the
/// deterministic id `urn:ots:change:{epoch}:{seq}:inverse`.
pub fn change_inverse_patch(
    row: &crate::store::changes::ChangeRow,
    dataset_iri: Option<&str>,
) -> Option<Patch> {
    let forward = change_patch(row, dataset_iri)?;
    let mut inverse = forward.inverse();
    if let Some(id) = forward.id() {
        inverse.headers[0].1 = format!("{id}:inverse");
    }
    Some(inverse)
}

// ── tokenizer ───────────────────────────────────────────────────────────────

/// Split one patch line into RDF terms / keywords, honouring `<…>`, `"…"`
/// (with escapes, language tags and datatypes) and the terminating `.`.
fn tokenize(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        match c {
            // A triple term `<<( s p o )>>`, possibly nested; IRIs and
            // literals inside are skipped whole so a `>>` in them does not
            // end it.
            '<' if chars.get(i + 1) == Some(&'<') => {
                let mut depth = 0usize;
                loop {
                    if i >= chars.len() {
                        return Err("unterminated triple term".into());
                    }
                    match chars[i] {
                        '<' if chars.get(i + 1) == Some(&'<') => {
                            depth += 1;
                            i += 2;
                        }
                        '>' if chars.get(i + 1) == Some(&'>') => {
                            depth -= 1;
                            i += 2;
                            if depth == 0 {
                                break;
                            }
                        }
                        '<' => {
                            while i < chars.len() && chars[i] != '>' {
                                i += 1;
                            }
                            i += 1;
                        }
                        '"' => {
                            i += 1;
                            while i < chars.len() && chars[i] != '"' {
                                if chars[i] == '\\' {
                                    i += 1;
                                }
                                i += 1;
                            }
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
            }
            '<' => {
                while i < chars.len() && chars[i] != '>' {
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("unterminated IRI".into());
                }
                i += 1;
            }
            '"' => {
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\\' {
                        i += 2;
                        continue;
                    }
                    if chars[i] == '"' {
                        break;
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    return Err("unterminated literal".into());
                }
                i += 1;
                // language tag or datatype
                if i < chars.len() && chars[i] == '@' {
                    while i < chars.len() && !chars[i].is_whitespace() {
                        i += 1;
                    }
                } else if i + 1 < chars.len() && chars[i] == '^' && chars[i + 1] == '^' {
                    i += 2;
                    if i < chars.len() && chars[i] == '<' {
                        while i < chars.len() && chars[i] != '>' {
                            i += 1;
                        }
                        if i >= chars.len() {
                            return Err("unterminated datatype IRI".into());
                        }
                        i += 1;
                    } else {
                        while i < chars.len() && !chars[i].is_whitespace() {
                            i += 1;
                        }
                    }
                }
            }
            _ => {
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
            }
        }
        out.push(chars[start..i].iter().collect());
    }
    Ok(out)
}

fn is_iri(t: &str) -> bool {
    t.starts_with('<') && t.ends_with('>') && t.len() > 2
}
fn is_bnode(t: &str) -> bool {
    t.starts_with("_:")
}
fn is_literal(t: &str) -> bool {
    t.starts_with('"')
}
/// Whether `local` is acceptable as the local part of a prefixed name: the
/// SPARQL `PN_LOCAL` production, minus its `\`-escapes. Letters, digits, `_`,
/// `-`, `.`, `:`, `%XX` and non-ASCII characters; nothing else — in particular
/// none of `{ } ; < > " ' \` or whitespace, which is what let a "prefixed
/// name" carry SPARQL syntax into the generated update.
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
/// earlier line.
fn expand_prefixed(t: &str, prefixes: &[(String, String)]) -> Result<NamedNode, String> {
    let Some((pfx, local)) = t.split_once(':') else {
        return Err(format!("`{t}` is not an RDF term"));
    };
    let key = format!("{pfx}:");
    let Some((_, ns)) = prefixes.iter().find(|(p, _)| *p == key) else {
        return Err(format!("`{t}` uses the undeclared prefix `{key}`"));
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

/// Parse one term of an `A`/`D` line into its canonical N-Triples text.
///
/// Every kind of term is rebuilt from an oxrdf value — `NamedNode`,
/// `BlankNode`, `Literal` — so what comes out is exactly what the store's own
/// serializer would print, and nothing the patch author typed reaches the
/// SPARQL text unchecked. Prefixed names (also in a literal's datatype) are
/// expanded here; the generated update declares no prefixes.
fn canonical_term(
    t: &str,
    prefixes: &[(String, String)],
    allow_bnode: bool,
    allow_literal: bool,
    what: &str,
    ln: usize,
) -> Result<String, String> {
    let err = |e: String| format!("line {ln}: {what}: {e}");
    if t.starts_with("<<") {
        if !allow_literal {
            return Err(err(format!("a triple term may only be an object ({t})")));
        }
        return triple_term(t, prefixes, ln);
    }
    if is_iri(t) {
        return NamedNode::new(&t[1..t.len() - 1])
            .map(|n| n.to_string())
            .map_err(|e| err(format!("{t}: {e}")));
    }
    if is_bnode(t) {
        if !allow_bnode {
            return Err(err(format!("must not be a blank node ({t})")));
        }
        return BlankNode::new(&t[2..])
            .map(|b| b.to_string())
            .map_err(|e| err(format!("{t}: {e}")));
    }
    if is_literal(t) {
        if !allow_literal {
            return Err(err(format!("must not be a literal ({t})")));
        }
        let (body, suffix) = split_literal(t).map_err(err)?;
        let full = match suffix.strip_prefix("^^") {
            Some(dt) if !dt.starts_with('<') => {
                format!("{body}^^{}", expand_prefixed(dt, prefixes).map_err(err)?)
            }
            _ => t.to_string(),
        };
        return full
            .parse::<Literal>()
            .map(|l| l.to_string())
            .map_err(|e| err(format!("{t}: {e}")));
    }
    expand_prefixed(t, prefixes)
        .map(|n| n.to_string())
        .map_err(err)
}

/// An RDF 1.2 triple term `<<( s p o )>>` in canonical form, its inner terms
/// checked like any other (a literal or triple term is only an object).
fn triple_term(t: &str, prefixes: &[(String, String)], ln: usize) -> Result<String, String> {
    if !cfg!(feature = "rdf-12") {
        return Err(format!(
            "line {ln}: triple terms need a build with RDF 1.2 support ({t})"
        ));
    }
    let inner = t
        .strip_prefix("<<(")
        .and_then(|r| r.strip_suffix(")>>"))
        .ok_or_else(|| format!("line {ln}: a triple term is written <<( s p o )>> ({t})"))?;
    let toks = tokenize(inner).map_err(|e| format!("line {ln}: {e}"))?;
    if toks.len() != 3 {
        return Err(format!(
            "line {ln}: a triple term holds a subject, a predicate and an object ({t})"
        ));
    }
    let s = canonical_term(&toks[0], prefixes, true, false, "triple-term subject", ln)?;
    let p = canonical_term(
        &toks[1],
        prefixes,
        false,
        false,
        "triple-term predicate",
        ln,
    )?;
    let o = canonical_term(&toks[2], prefixes, true, true, "triple-term object", ln)?;
    Ok(format!("<<( {s} {p} {o} )>>"))
}

/// Parse a patch document.
pub fn parse(text: &str) -> Result<Patch, String> {
    let mut patch = Patch::default();
    let mut in_tx = false;
    let mut tx_seen = false;
    for (idx, raw) in text.lines().enumerate() {
        let ln = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut toks = tokenize(line).map_err(|e| format!("line {ln}: {e}"))?;
        if toks.last().map(String::as_str) != Some(".") {
            return Err(format!("line {ln}: missing terminating `.`"));
        }
        toks.pop();
        let Some(code) = toks.first().cloned() else {
            return Err(format!("line {ln}: empty statement"));
        };
        let args = &toks[1..];
        match code.as_str() {
            "H" => {
                if args.len() < 2 {
                    return Err(format!("line {ln}: H needs a name and a value"));
                }
                let v = args[1..].join(" ");
                let v = v
                    .trim_matches('"')
                    .trim_matches(|c| c == '<' || c == '>')
                    .to_string();
                patch.headers.push((args[0].clone(), v));
            }
            "TX" => {
                if tx_seen {
                    return Err(format!(
                        "line {ln}: only one transaction per patch is supported"
                    ));
                }
                in_tx = true;
                tx_seen = true;
            }
            "TC" => {
                if !in_tx {
                    return Err(format!("line {ln}: TC without TX"));
                }
                in_tx = false;
            }
            "TA" => {
                if !in_tx {
                    return Err(format!("line {ln}: TA without TX"));
                }
                in_tx = false;
                patch.aborted = true;
                patch.ops.clear();
            }
            "PA" => {
                if args.len() != 2 || !args[0].ends_with(':') || !is_iri(&args[1]) {
                    return Err(format!("line {ln}: PA needs `prefix: <iri>`"));
                }
                let ns = args[1][1..args[1].len() - 1].to_string();
                patch.prefixes.retain(|(p, _)| *p != args[0]);
                patch.prefixes.push((args[0].clone(), ns));
            }
            "PD" => {
                if args.len() != 1 {
                    return Err(format!("line {ln}: PD needs a prefix"));
                }
                patch.prefixes.retain(|(p, _)| *p != args[0]);
            }
            "A" | "D" => {
                if args.len() != 3 && args.len() != 4 {
                    return Err(format!(
                        "line {ln}: {code} needs a triple or quad, found {} terms",
                        args.len()
                    ));
                }
                let delete = code == "D";
                let px = &patch.prefixes;
                // Blank nodes in a `D` line too: the apply removes quads
                // through a transaction, which can name them.
                let s = canonical_term(&args[0], px, true, false, "subject", ln)?;
                let p = canonical_term(&args[1], px, false, false, "predicate", ln)?;
                let o = canonical_term(&args[2], px, true, true, "object", ln)?;
                let g = if args.len() == 4 {
                    Some(canonical_term(&args[3], px, false, false, "graph", ln)?)
                } else {
                    None
                };
                let q = QuadText { s, p, o, g };
                patch
                    .ops
                    .push(if delete { Op::Delete(q) } else { Op::Add(q) });
            }
            other => return Err(format!("line {ln}: unknown code `{other}`")),
        }
    }
    if in_tx {
        return Err("transaction not closed (missing TC or TA)".into());
    }
    Ok(patch)
}

// ── SPARQL rendering ────────────────────────────────────────────────────────

/// The patch as one SPARQL Update request: runs of adds / deletes become
/// `INSERT DATA` / `DELETE DATA` blocks in order, grouped by graph, so the
/// sequence semantics of the patch are preserved inside one transaction.
///
/// The terms are the canonical ones [`parse`] produced (prefixed names already
/// expanded), so no `PREFIX` header is emitted and every token is a complete,
/// validated N-Triples term.
pub fn to_sparql_update(patch: &Patch) -> String {
    let mut out = String::new();
    let mut blocks: Vec<String> = Vec::new();
    let mut run: Vec<&QuadText> = Vec::new();
    let mut run_is_add: Option<bool> = None;
    let flush = |blocks: &mut Vec<String>, run: &mut Vec<&QuadText>, is_add: bool| {
        if run.is_empty() {
            return;
        }
        let mut by_graph: Vec<(Option<&str>, Vec<&QuadText>)> = Vec::new();
        for q in run.iter() {
            match by_graph.iter_mut().find(|(g, _)| *g == q.g.as_deref()) {
                Some((_, v)) => v.push(q),
                None => by_graph.push((q.g.as_deref(), vec![q])),
            }
        }
        let mut b = String::from(if is_add {
            "INSERT DATA {\n"
        } else {
            "DELETE DATA {\n"
        });
        for (g, quads) in by_graph {
            let body: String = quads
                .iter()
                .map(|q| format!("    {} {} {} .\n", q.s, q.p, q.o))
                .collect();
            match g {
                Some(g) => b.push_str(&format!("  GRAPH {g} {{\n{body}  }}\n")),
                None => b.push_str(&body),
            }
        }
        b.push('}');
        blocks.push(b);
        run.clear();
    };
    for op in &patch.ops {
        let (is_add, q) = match op {
            Op::Add(q) => (true, q),
            Op::Delete(q) => (false, q),
        };
        if run_is_add.is_some_and(|r| r != is_add) {
            flush(&mut blocks, &mut run, run_is_add.unwrap());
        }
        run_is_add = Some(is_add);
        run.push(q);
    }
    if let Some(is_add) = run_is_add {
        flush(&mut blocks, &mut run, is_add);
    }
    out.push_str(&blocks.join(";\n"));
    out
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
/// missing side is the empty graph.
pub fn generate(
    store: &TripleStore,
    headers: &[(&str, &str)],
    mappings: &[(String, Option<String>, Option<String>)],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("H id <urn:uuid:{}> .\n", uuid::Uuid::new_v4()));
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

// ── HTTP ────────────────────────────────────────────────────────────────────

type ApiErr = (StatusCode, String);

/// POST /api/datasets/:id/patch — apply an RDF Patch to the dataset's graphs.
///
/// One transaction through the store's `apply_delta`: the net effect of the
/// lines, recorded in the change log under one commit. `H base-digest`, when
/// present, must equal the set hash of the graphs the patch names, else 412.
pub async fn apply_patch_handler(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiErr> {
    let e500 = |e: anyhow::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let ds = state
        .auth_db
        .get_dataset(&dataset_id)
        .map_err(e500)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state
        .auth_db
        .can_access_dataset(Some(&user.user_id), &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !(ct.is_empty() || ct.contains("rdf-patch") || ct.starts_with("text/plain")) {
        return Err((
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("send the patch as {MEDIA_TYPE} (got {ct})"),
        ));
    }
    let text =
        String::from_utf8(body.to_vec()).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    let patch =
        parse(&text).map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid RDF Patch: {e}")))?;
    let id = patch.id().unwrap_or("-").to_string();
    if patch.aborted || patch.ops.is_empty() {
        return Ok(Json(serde_json::json!({
            "applied": false,
            "id": id,
            "aborted": patch.aborted,
            "added": 0,
            "removed": 0,
            "reason": if patch.aborted { "the transaction was aborted (TA)" } else { "no A/D lines" },
        })));
    }
    // Every quad names one of the dataset's graphs. `parse` already expanded
    // prefixed graph names through the patch's own PA declarations, so each
    // graph term is a canonical `<iri>`.
    let registered: HashSet<String> = state
        .auth_db
        .list_dataset_graphs(&dataset_id)
        .map_err(e500)?
        .into_iter()
        .collect();
    let mut graphs: Vec<String> = Vec::new();
    for g in patch.graphs() {
        let Some(g) = g else {
            return Err((
                StatusCode::BAD_REQUEST,
                "every A/D line must name a graph: a dataset patch applies to the dataset's registered graphs".to_string(),
            ));
        };
        let iri = g
            .strip_prefix('<')
            .and_then(|g| g.strip_suffix('>'))
            .unwrap_or(&g)
            .to_string();
        if !registered.contains(&iri) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("graph <{iri}> is not registered to dataset {dataset_id}"),
            ));
        }
        if !graphs.contains(&iri) {
            graphs.push(iri);
        }
    }
    let (to_add, to_remove) = patch
        .net_quads()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid RDF Patch: {e}")))?;
    let expected = match patch.base_digest() {
        Some(d) => Some(crate::store::set_hash::SetHash::from_hex(d).ok_or((
            StatusCode::BAD_REQUEST,
            "H base-digest must be a 64-digit hex set hash".to_string(),
        ))?),
        None => None,
    };
    let (added, removed) = (patch.adds(), patch.deletes());
    let stamp = crate::commit_log::CommitStamp::new(&state.base_url);
    let ctx = crate::server::write_effects::context(
        &state,
        Some(&user),
        crate::commit_log::CommitKind::Sparql,
        &stamp,
    );
    let st = state.clone();
    let gs = graphs.clone();
    let stamped = stamp.clone();
    let (effective, written) = tokio::task::spawn_blocking(
        move || -> Result<(crate::store::engine::QuadDelta, crate::server::write_effects::Written), ApiErr> {
            if let Some(expected) = expected {
                let actual = crate::store::set_hash::SetHash::of_graphs(&st.store, &gs);
                if actual != expected {
                    return Err((
                        StatusCode::PRECONDITION_FAILED,
                        format!(
                            "H base-digest does not match: the graphs are now {}",
                            actual.to_hex()
                        ),
                    ));
                }
            }
            let ldes = crate::ldes::capture::Capture::begin(&st, &gs);
            let applied = {
                let _ctx = crate::store::changes::WriteContextGuard::set(ctx);
                st.store.apply_delta(&to_add, &to_remove, true)
            };
            let effective = applied.map_err(|e| match e {
                crate::store::engine::StoreError::ReadOnly(_) => {
                    (StatusCode::SERVICE_UNAVAILABLE, e.to_string())
                }
                other => (
                    StatusCode::BAD_REQUEST,
                    format!("applying the patch failed: {other}"),
                ),
            })?;
            let written = crate::server::write_effects::Written::read(&st, &stamped);
            ldes.finish(&st, &written.rows);
            written.entailment(&st, &gs);
            // The exact quads when the change log has them; else what was
            // applied, which the apply itself returned.
            if !written.sync_text_index(&st, &gs) {
                #[cfg(feature = "text-search")]
                st.text_index_apply_delta(&effective.0, &effective.1, &gs);
            }
            Ok((effective, written))
        },
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))??;
    crate::commit_log::record_stamped(
        &state.store,
        &state.base_url,
        &stamp,
        crate::commit_log::CommitKind::Sparql,
        format!("RDF Patch {id}: +{added} −{removed}"),
        Some(&user.user_id),
        Some(format!(
            "{}/dataset/{}",
            state.base_url.trim_end_matches('/'),
            dataset_id
        )),
        graphs.clone(),
        added,
        removed,
        None,
    );
    let seq_to = written.rows.iter().filter_map(|r| r.seq).max();
    Ok(Json(serde_json::json!({
        "applied": true,
        "id": id,
        "aborted": false,
        "added": added,
        "removed": removed,
        "effective": { "added": effective.0.len(), "removed": effective.1.len() },
        "seq": seq_to,
        "graphs": graphs,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_headers_prefixes_transactions_and_quads() {
        let p = parse(
            "H id <urn:uuid:1> .\nPA ex: <http://example.org/> .\nTX .\nA ex:s ex:p \"v \\\"q\\\" .\"@en <urn:g> .\nD <urn:s> <urn:p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> <urn:g> .\nTC .\n",
        )
        .unwrap();
        assert_eq!(p.id(), Some("urn:uuid:1"));
        assert_eq!(
            p.prefixes,
            vec![("ex:".to_string(), "http://example.org/".to_string())]
        );
        assert_eq!(p.adds(), 1);
        assert_eq!(p.deletes(), 1);
        assert!(!p.aborted);
        let sparql = to_sparql_update(&p);
        assert!(
            !sparql.contains("PREFIX"),
            "terms are expanded at parse time, the update declares no prefixes: {sparql}"
        );
        assert!(
            sparql.contains(
                "INSERT DATA {\n  GRAPH <urn:g> {\n    <http://example.org/s> <http://example.org/p> \"v \\\"q\\\" .\"@en .\n"
            ),
            "{sparql}"
        );
        assert!(
            sparql.contains(";\nDELETE DATA {"),
            "adds then deletes, in order: {sparql}"
        );
    }

    #[test]
    #[cfg(feature = "rdf-12")]
    fn triple_terms_tokenize_and_round_trip() {
        let p = parse(
            "TX .\nA <urn:r> <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( <urn:s> <urn:p> \"a >> b\" )>> <urn:g> .\nTC .\n",
        )
        .unwrap();
        let Op::Add(q) = &p.ops[0] else { panic!() };
        assert_eq!(q.o, "<<( <urn:s> <urn:p> \"a >> b\" )>>");
        let quad = q.to_quad().unwrap();
        assert!(matches!(quad.object, oxigraph::model::Term::Triple(_)));
        // Back through the renderer and the parser: the same quad.
        let again = parse(&p.render()).unwrap();
        let Op::Add(q2) = &again.ops[0] else { panic!() };
        assert_eq!(q2.to_quad().unwrap(), quad);
        // A triple term is only ever an object.
        assert!(
            parse("TX .\nA <<( <urn:s> <urn:p> <urn:o> )>> <urn:p> <urn:o> <urn:g> .\nTC .\n")
                .unwrap_err()
                .contains("only be an object")
        );
        assert!(parse("TX .\nA <urn:s> <urn:p> <<( <urn:s> <urn:p> .\nTC .\n").is_err());
    }

    #[test]
    fn the_last_line_on_a_quad_decides_its_net_effect() {
        let p = parse(
            "TX .\n\
             A <urn:a> <urn:p> <urn:o> <urn:g> .\n\
             D <urn:a> <urn:p> <urn:o> <urn:g> .\n\
             D <urn:b> <urn:p> <urn:o> <urn:g> .\n\
             A <urn:b> <urn:p> <urn:o> <urn:g> .\n\
             D <urn:c> <urn:p> <urn:o> <urn:g> .\n\
             TC .\n",
        )
        .unwrap();
        let (add, remove) = p.net_quads().unwrap();
        let s = |qs: &[Quad]| -> Vec<String> { qs.iter().map(|q| q.subject.to_string()).collect() };
        assert_eq!(s(&add), vec!["<urn:b>"]);
        assert_eq!(s(&remove), vec!["<urn:a>", "<urn:c>"]);
    }

    #[test]
    fn an_inverse_swaps_and_reverses_and_names_its_forward_patch() {
        let p = parse(
            "H id <urn:uuid:1> .\nH base-digest \"00\" .\nTX .\nD <urn:a> <urn:p> <urn:o> <urn:g> .\nA <urn:b> <urn:p> <urn:o> <urn:g> .\nTC .\n",
        )
        .unwrap();
        let inv = p.inverse();
        assert_ne!(inv.id(), Some("urn:uuid:1"));
        assert_eq!(inv.header("prev"), Some("urn:uuid:1"));
        assert_eq!(
            inv.base_digest(),
            None,
            "the forward base is not the inverse's"
        );
        match (&inv.ops[0], &inv.ops[1]) {
            (Op::Delete(d), Op::Add(a)) => {
                assert_eq!(d.s, "<urn:b>");
                assert_eq!(a.s, "<urn:a>");
            }
            other => panic!("{other:?}"),
        }
        let text = inv.render();
        assert!(text.contains("H prev <urn:uuid:1> .\n"), "{text}");
        assert_eq!(parse(&text).unwrap().ops, inv.ops);
    }

    #[test]
    fn a_change_row_is_a_patch_with_its_place_in_the_log() {
        use crate::store::changes::{ChangeRow, Extent, State};
        let row = ChangeRow {
            row: 1,
            seq: Some(42),
            epoch: "ep".into(),
            txn: "t".into(),
            scope: "graph".into(),
            graph_iri: Some("urn:g".into()),
            origin: "update".into(),
            kind: Some("sparql".into()),
            commit_iri: Some("http://x/commit/c1".into()),
            actor_iri: None,
            extent: Extent::Full,
            added: Some("<urn:b> <urn:p> _:x1 <urn:g> .\n".into()),
            removed: Some("<urn:a> <urn:p> <urn:o> <urn:g> .\n".into()),
            added_n: 1,
            removed_n: 1,
            post_count: Some(1),
            has_bnode: true,
            state: State::Committed,
            created_at: "now".into(),
        };
        let p = change_patch(&row, Some("http://x/dataset/d")).unwrap();
        assert_eq!(p.id(), Some("urn:ots:change:ep:42"));
        let text = p.render();
        for h in [
            "H seq \"42\" .",
            "H epoch \"ep\" .",
            "H dataset <http://x/dataset/d> .",
            "H commit <http://x/commit/c1> .",
        ] {
            assert!(text.contains(h), "{h} in {text}");
        }
        assert!(
            text.contains(
                "D <urn:a> <urn:p> <urn:o> <urn:g> .\nA <urn:b> <urn:p> _:x1 <urn:g> .\n"
            ),
            "{text}"
        );
        let inv = change_inverse_patch(&row, None).unwrap();
        assert_eq!(inv.id(), Some("urn:ots:change:ep:42:inverse"));
        let back = parse(&inv.render()).unwrap();
        let (add, remove) = back.net_quads().unwrap();
        assert_eq!(add[0].subject.to_string(), "<urn:a>");
        assert_eq!(remove[0].subject.to_string(), "<urn:b>");
        // No quads, no patch.
        let mut counts = row.clone();
        counts.extent = Extent::Counts;
        assert!(change_patch(&counts, None).is_none());
    }

    #[test]
    fn accepts_blank_node_deletes_and_rejects_unknown_codes_and_open_transactions() {
        // A `D` line may name a blank node: the apply removes quads through
        // a transaction, which can (a SPARQL `DELETE DATA` could not).
        let p = parse("TX .\nD _:b <urn:p> <urn:o> <urn:g> .\nTC .\n").unwrap();
        assert_eq!(p.deletes(), 1);
        // A blank node still cannot be a predicate or a graph.
        assert!(parse("TX .\nD <urn:s> _:p <urn:o> <urn:g> .\nTC .\n")
            .unwrap_err()
            .contains("blank node"));
        assert!(parse("X .\n").unwrap_err().contains("unknown code"));
        assert!(parse("TX .\nA <urn:s> <urn:p> <urn:o> .\n")
            .unwrap_err()
            .contains("not closed"));
        assert!(parse("A <urn:s> <urn:p> <urn:o>\n")
            .unwrap_err()
            .contains("terminating"));
        let aborted = parse("TX .\nA <urn:s> <urn:p> <urn:o> <urn:g> .\nTA .\n").unwrap();
        assert!(aborted.aborted && aborted.ops.is_empty());
    }

    /// A "prefixed name" is only accepted when its prefix was declared and its
    /// local part is a `PN_LOCAL`. The old check took any whitespace-free token
    /// containing `:`, so `ex:o}GRAPH<urn:x>{<a><b><c>` — SPARQL needs no
    /// whitespace between IRIs — closed the registered `GRAPH { … }` block and
    /// wrote to a graph the dataset never registered.
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
        let Op::Add(q) = &p.ops[0] else { panic!() };
        assert_eq!(q.s, "<http://example.org/s>");
        assert_eq!(q.o, "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>");
        assert_eq!(q.g.as_deref(), Some("<http://example.org/g>"));
        // `PD` removes the declaration for the lines after it.
        let after_pd = parse(
            "PA ex: <http://example.org/> .\nPD ex: .\nTX .\nA ex:s <urn:p> <urn:o> <urn:g> .\nTC .\n",
        )
        .unwrap_err();
        assert!(after_pd.contains("undeclared prefix"), "{after_pd}");
        // The generated update is exactly one INSERT DATA into the named graph.
        let sparql = to_sparql_update(&p);
        let parsed = spargebra::SparqlParser::new()
            .parse_update(&sparql)
            .expect("generated update must parse");
        assert_eq!(parsed.operations.len(), 1, "{sparql}");
    }

    #[test]
    fn generates_a_patch_from_two_graphs() {
        let store = TripleStore::in_memory().unwrap();
        store
            .update("INSERT DATA { GRAPH <urn:from> { <urn:a> <urn:p> 1 . <urn:b> <urn:p> 2 } GRAPH <urn:to> { <urn:a> <urn:p> 1 . <urn:c> <urn:p> 3 } }")
            .unwrap();
        let text = generate(
            &store,
            &[("from", "v1"), ("to", "live")],
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
        store.update(&to_sparql_update(&p)).unwrap();
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
