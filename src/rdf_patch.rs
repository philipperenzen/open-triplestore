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
//! patch may only touch the dataset's registered graphs). Deleting a triple
//! whose subject or object is a blank node is refused: the patch is applied
//! as SPARQL `DELETE DATA`, which cannot name blank nodes.

use std::collections::{BTreeSet, HashSet};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use oxigraph::model::{GraphNameRef, NamedNodeRef};

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;
use crate::store::{escape_sparql_iri, TripleStore};

pub const MEDIA_TYPE: &str = "application/rdf-patch";

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
fn is_prefixed(t: &str) -> bool {
    !is_iri(t)
        && !is_bnode(t)
        && !is_literal(t)
        && t.contains(':')
        && !t.contains(char::is_whitespace)
}

fn check_term(
    t: &str,
    allow_bnode: bool,
    allow_literal: bool,
    what: &str,
    ln: usize,
) -> Result<(), String> {
    if is_iri(t) {
        oxigraph::model::NamedNode::new(&t[1..t.len() - 1])
            .map_err(|e| format!("line {ln}: {what} {t}: {e}"))?;
        Ok(())
    } else if is_bnode(t) {
        if allow_bnode {
            Ok(())
        } else {
            Err(format!("line {ln}: {what} must not be a blank node ({t})"))
        }
    } else if is_literal(t) {
        if allow_literal {
            Ok(())
        } else {
            Err(format!("line {ln}: {what} must not be a literal ({t})"))
        }
    } else if is_prefixed(t) {
        Ok(())
    } else {
        Err(format!("line {ln}: `{t}` is not an RDF term"))
    }
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
                check_term(&args[0], !delete, false, "subject", ln)?;
                check_term(&args[1], false, false, "predicate", ln)?;
                check_term(&args[2], !delete, true, "object", ln)?;
                let g = if args.len() == 4 {
                    check_term(&args[3], false, false, "graph", ln)?;
                    Some(args[3].clone())
                } else {
                    None
                };
                let q = QuadText {
                    s: args[0].clone(),
                    p: args[1].clone(),
                    o: args[2].clone(),
                    g,
                };
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
pub fn to_sparql_update(patch: &Patch) -> String {
    let mut out = String::new();
    for (pfx, ns) in &patch.prefixes {
        out.push_str(&format!("PREFIX {pfx} <{ns}>\n"));
    }
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
    // Every quad names one of the dataset's graphs (prefixed graph names are
    // expanded through the patch's own PA declarations).
    let registered: HashSet<String> = state
        .auth_db
        .list_dataset_graphs(&dataset_id)
        .map_err(e500)?
        .into_iter()
        .collect();
    let expand = |t: &str| -> Option<String> {
        if is_iri(t) {
            return Some(t[1..t.len() - 1].to_string());
        }
        let (pfx, local) = t.split_once(':')?;
        let pfx = format!("{pfx}:");
        patch
            .prefixes
            .iter()
            .find(|(p, _)| *p == pfx)
            .map(|(_, ns)| format!("{ns}{local}"))
    };
    let mut graphs: Vec<String> = Vec::new();
    for g in patch.graphs() {
        let Some(g) = g else {
            return Err((
                StatusCode::BAD_REQUEST,
                "every A/D line must name a graph: a dataset patch applies to the dataset's registered graphs".to_string(),
            ));
        };
        let iri = expand(&g).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                format!("graph {g} uses an undeclared prefix"),
            )
        })?;
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
    let update = to_sparql_update(&patch);
    let (added, removed) = (patch.adds(), patch.deletes());
    let st = state.clone();
    let gs = graphs.clone();
    let update_text = update.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let before = crate::ldes::capture::before(&st, &gs);
        st.store.update(&update_text).map_err(|e| e.to_string())?;
        crate::ldes::capture::after(&st, before);
        Ok(())
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("applying the patch failed: {e}"),
        )
    })?;
    for g in &graphs {
        crate::server::routes::sync_text_index_after_graph_write(&state, Some(g.clone())).await;
    }
    crate::commit_log::record(
        &state.store,
        &state.base_url,
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
    Ok(Json(serde_json::json!({
        "applied": true,
        "id": id,
        "aborted": false,
        "added": added,
        "removed": removed,
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
            sparql.starts_with("PREFIX ex: <http://example.org/>\n"),
            "{sparql}"
        );
        assert!(
            sparql.contains(
                "INSERT DATA {\n  GRAPH <urn:g> {\n    ex:s ex:p \"v \\\"q\\\" .\"@en .\n"
            ),
            "{sparql}"
        );
        assert!(
            sparql.contains(";\nDELETE DATA {"),
            "adds then deletes, in order: {sparql}"
        );
    }

    #[test]
    fn rejects_blank_node_deletes_unknown_codes_and_open_transactions() {
        assert!(parse("TX .\nD _:b <urn:p> <urn:o> <urn:g> .\nTC .\n")
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
