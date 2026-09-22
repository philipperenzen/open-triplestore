//! Review items: what a refused run leaves for a human, the deterministic
//! fixer, the model-assisted suggestion, and promotion of a corrected
//! candidate.
//!
//! A run the SHACL write gate refuses keeps its candidate graph; that graph
//! is not evidence of anything until someone looks at it. This module turns
//! the refusal into work: **one review item per subject** with violations,
//! carrying the violations and a snapshot of the subject as the candidate
//! describes it. Items live in `urn:system:reviews:<datasource>` — a system
//! graph, outside every dataset's SPARQL scope, served only here.
//!
//! **The fixer** applies two rules and no others. A value below an
//! `sh:minInclusive` bound that is itself non-negative, when the value is
//! negative, is read as a **sign typo** and its sign is dropped; a value past
//! an inclusive bound is **clamped** to the bound. Both change a literal that
//! exists into a literal the constraint names — nothing is invented, and a
//! violation that would need an invented value (a missing required value, a
//! wrong class, a pattern) is left to a human with a 422. A fix is previewed
//! as an RDF Patch and applied through the store's own patch path, into the
//! candidate graph, as one commit.
//!
//! **Promotion** is the release of a corrected candidate: the gate runs
//! again over the graph as it now stands, and only a passing graph takes the
//! production role — recorded as a `ds:Promotion` activity naming who
//! promoted it, on the run's PROV trail beside the run itself.
//!
//! A status is explicit: `needsHuman` (opened), `gathering` (a human is
//! collecting what a decision needs), `corrected` (the fixer or a human
//! changed the candidate), `valid` (re-checked and clean), `approved`,
//! `rejected` (a human decided, with a note), `promoted` (the candidate went
//! to production).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use oxigraph::model::{GraphNameRef, Literal, NamedNodeRef, NamedOrBlankNodeRef, Term};
use oxigraph::sparql::QueryResults;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;
use crate::shacl::report::{Severity, ValidationReport};
use crate::store::{escape_sparql_iri, escape_sparql_literal, TripleStore};

use super::model::*;
use super::registry;
use super::runs::{self, PromoteError, RunContext};

const PROV: &str = "http://www.w3.org/ns/prov#";
const DCT: &str = "http://purl.org/dc/terms/";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

fn prefixes() -> String {
    format!(
        "PREFIX ds: <{DS}>\nPREFIX prov: <{PROV}>\nPREFIX dct: <{DCT}>\nPREFIX xsd: <{XSD}>\n\
         PREFIX rdf: <{RDF}>\n"
    )
}

/// The statuses an item moves through. The first is what a refusal opens
/// with; the machine sets `corrected` and `promoted`; a human sets any.
pub const STATUSES: [&str; 7] = [
    "needsHuman",
    "gathering",
    "corrected",
    "valid",
    "approved",
    "rejected",
    "promoted",
];

/// Cap on the items one refused run opens. A refusal over a whole table with
/// a systematic defect is a mapping problem — the dry-run classifier's
/// territory — not a queue for a human to work through subject by subject.
pub const MAX_ITEMS_ENV: &str = "OTS_REVIEW_MAX_ITEMS";
const DEFAULT_MAX_ITEMS: usize = 500;

fn max_items() -> usize {
    std::env::var(MAX_ITEMS_ENV)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_MAX_ITEMS)
}

pub fn review_graph_iri(source_id: &str) -> String {
    format!("urn:system:reviews:{source_id}")
}

pub fn item_iri(id: &str) -> String {
    format!("urn:review:{id}")
}

fn lex(row: &oxigraph::sparql::QuerySolution, var: &str) -> Option<String> {
    match row.get(var)? {
        Term::NamedNode(n) => Some(n.as_str().to_string()),
        Term::Literal(l) => Some(l.value().to_string()),
        Term::BlankNode(b) => Some(format!("_:{}", b.as_str())),
        #[cfg(feature = "rdf-12")]
        Term::Triple(_) => None,
    }
}

// ───────────────────────────── Items ─────────────────────────────

/// One validation result on the item's subject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Violation {
    /// The constraint as the engine names it: `sh:minInclusive 0`,
    /// `sh:maxInclusive 1000`, `sh:minCount 1`, `sh:datatype <…>`.
    pub constraint: String,
    /// The property path, an IRI for a plain predicate path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The offending value's lexical form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub message: String,
    pub shape: String,
}

impl Violation {
    fn of(r: &crate::shacl::report::ValidationResult) -> Self {
        Violation {
            constraint: r.source_constraint.clone(),
            path: r.path.clone().map(|p| plain_iri(&p)),
            value: r.value.clone(),
            message: r.message.clone(),
            shape: r.source_shape.clone(),
        }
    }
}

/// `<iri>` as `iri`; any other path expression as it is.
fn plain_iri(path: &str) -> String {
    let p = path.trim();
    match p.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        Some(inner) if !inner.contains(['<', '>', ' ']) => inner.to_string(),
        _ => p.to_string(),
    }
}

/// A change the fixer made, or would make.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    /// `sign-typo` or `clamp`.
    pub rule: String,
    pub path: String,
    pub from: String,
    pub to: String,
    pub constraint: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    pub id: String,
    pub iri: String,
    /// The datasource IRI.
    pub source: String,
    /// The run id.
    pub run: String,
    /// The candidate graph the subject lives in.
    pub graph: String,
    /// The mapping id and the version that ran.
    pub mapping: String,
    pub mapping_version: u32,
    pub subject: String,
    pub status: String,
    pub violations: Vec<Violation>,
    /// The subject as the candidate graph describes it, as N-Triples —
    /// refreshed after every fix.
    pub snapshot: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<Fix>,
    /// Who last decided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    /// The note the decision came with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn item_turtle(item: &ReviewItem) -> String {
    let violations = serde_json::to_string(&item.violations).unwrap_or_else(|_| "[]".into());
    let subject = if item.subject.starts_with("_:") {
        format!("\"{}\"", escape_sparql_literal(&item.subject))
    } else {
        format!("<{}>", escape_sparql_iri(&item.subject))
    };
    let mut body = format!(
        "  <{}> a ds:ReviewItem ;\n    ds:id \"{}\" ;\n    ds:source <{}> ;\n    ds:run \"{}\" ;\n    ds:graph <{}> ;\n    ds:mapping <{}> ;\n    ds:mappingVersion \"{}\"^^xsd:integer ;\n    ds:subject {subject} ;\n    ds:status \"{}\" ;\n    ds:violations \"{}\"^^rdf:JSON ;\n    ds:snapshot \"{}\" ;\n",
        escape_sparql_iri(&item.iri),
        escape_sparql_literal(&item.id),
        escape_sparql_iri(&item.source),
        escape_sparql_literal(&item.run),
        escape_sparql_iri(&item.graph),
        escape_sparql_iri(&mapping_iri(&item.mapping)),
        item.mapping_version,
        escape_sparql_literal(&item.status),
        escape_sparql_literal(&violations),
        escape_sparql_literal(&item.snapshot),
    );
    if !item.fixes.is_empty() {
        let fixes = serde_json::to_string(&item.fixes).unwrap_or_else(|_| "[]".into());
        body.push_str(&format!(
            "    ds:fixes \"{}\"^^rdf:JSON ;\n",
            escape_sparql_literal(&fixes)
        ));
    }
    if let Some(r) = &item.reviewer {
        body.push_str(&format!(
            "    prov:wasAttributedTo <{}> ;\n",
            escape_sparql_iri(r)
        ));
    }
    if let Some(d) = &item.decision {
        body.push_str(&format!(
            "    ds:decision \"{}\" ;\n",
            escape_sparql_literal(d)
        ));
    }
    body.push_str(&format!(
        "    dct:created \"{}\" ;\n    dct:modified \"{}\" .\n",
        escape_sparql_literal(&item.created_at),
        escape_sparql_literal(&item.updated_at)
    ));
    body
}

/// The graph an item belongs to, from its datasource IRI.
fn graph_of(item: &ReviewItem) -> String {
    review_graph_iri(item.source.trim_start_matches("urn:source:"))
}

/// Write (or rewrite) one item.
pub fn save(store: &TripleStore, item: &ReviewItem) -> Result<(), String> {
    let graph = escape_sparql_iri(&graph_of(item));
    let iri = escape_sparql_iri(&item.iri);
    let sparql = format!(
        "{}DELETE WHERE {{ GRAPH <{graph}> {{ <{iri}> ?p ?o }} }};\n\
         INSERT DATA {{ GRAPH <{graph}> {{\n{}}} }}",
        prefixes(),
        item_turtle(item)
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

/// Write a batch of new items into one graph, in one update per fifty.
fn insert_all(store: &TripleStore, graph: &str, items: &[ReviewItem]) -> Result<(), String> {
    let graph = escape_sparql_iri(graph);
    for chunk in items.chunks(50) {
        let body: String = chunk.iter().map(item_turtle).collect();
        let sparql = format!(
            "{}INSERT DATA {{ GRAPH <{graph}> {{\n{body}}} }}",
            prefixes()
        );
        store.update(&sparql).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn item_select(graph_pattern: &str, filter: &str) -> String {
    format!(
        "{}SELECT ?i ?id ?source ?run ?graph ?mapping ?version ?subject ?status ?violations ?snapshot \
         ?fixes ?reviewer ?decision ?created ?modified WHERE {{ {graph_pattern} {{\n\
           ?i a ds:ReviewItem ; ds:id ?id ; ds:source ?source ; ds:run ?run ; ds:graph ?graph ;\n\
              ds:mapping ?mapping ; ds:mappingVersion ?version ; ds:subject ?subject ;\n\
              ds:status ?status ; ds:violations ?violations ; ds:snapshot ?snapshot ;\n\
              dct:created ?created ; dct:modified ?modified .\n\
           {filter}\n\
           OPTIONAL {{ ?i ds:fixes ?fixes }}\n\
           OPTIONAL {{ ?i prov:wasAttributedTo ?reviewer }}\n\
           OPTIONAL {{ ?i ds:decision ?decision }}\n\
         }} }} ORDER BY DESC(?created) ?subject",
        prefixes()
    )
}

fn items(store: &TripleStore, graph_pattern: &str, filter: &str) -> Vec<ReviewItem> {
    let Ok(QueryResults::Solutions(rows)) = store.query(&item_select(graph_pattern, filter)) else {
        return Vec::new();
    };
    rows.flatten()
        .filter_map(|row| {
            let id = lex(&row, "id")?;
            Some(ReviewItem {
                iri: item_iri(&id),
                source: lex(&row, "source")?,
                run: lex(&row, "run")?,
                graph: lex(&row, "graph")?,
                mapping: lex(&row, "mapping")?
                    .trim_start_matches("urn:mapping:")
                    .to_string(),
                mapping_version: lex(&row, "version").and_then(|v| v.parse().ok())?,
                subject: lex(&row, "subject")?,
                status: lex(&row, "status")?,
                violations: lex(&row, "violations")
                    .and_then(|v| serde_json::from_str(&v).ok())
                    .unwrap_or_default(),
                snapshot: lex(&row, "snapshot").unwrap_or_default(),
                fixes: lex(&row, "fixes")
                    .and_then(|v| serde_json::from_str(&v).ok())
                    .unwrap_or_default(),
                reviewer: lex(&row, "reviewer"),
                decision: lex(&row, "decision"),
                created_at: lex(&row, "created")?,
                updated_at: lex(&row, "modified")?,
                id,
            })
        })
        .collect()
}

/// A datasource's items, newest first, optionally of one status.
pub fn list(store: &TripleStore, source_id: &str, status: Option<&str>) -> Vec<ReviewItem> {
    let graph = format!(
        "GRAPH <{}>",
        escape_sparql_iri(&review_graph_iri(source_id))
    );
    let filter = match status {
        Some(s) => format!("FILTER(?status = \"{}\")", escape_sparql_literal(s)),
        None => String::new(),
    };
    items(store, &graph, &filter)
}

/// One item, by id, whichever datasource it belongs to.
pub fn find(store: &TripleStore, id: &str) -> Option<ReviewItem> {
    if !valid_id(id) {
        return None;
    }
    items(
        store,
        "GRAPH ?g",
        &format!(
            "FILTER(STRSTARTS(STR(?g), \"urn:system:reviews:\") && ?i = <{}>)",
            escape_sparql_iri(&item_iri(id))
        ),
    )
    .into_iter()
    .next()
}

/// Open one item per subject the gate reported a violation on, up to the
/// configured cap. Returns how many were opened.
pub fn open_items(
    store: &TripleStore,
    source: &SqlSource,
    run: &RunRecord,
    report: &ValidationReport,
) -> Result<usize, String> {
    open_items_capped(store, source, run, report, max_items())
}

/// [`open_items`] with an explicit cap.
pub fn open_items_capped(
    store: &TripleStore,
    source: &SqlSource,
    run: &RunRecord,
    report: &ValidationReport,
    cap: usize,
) -> Result<usize, String> {
    let mut by_subject: Vec<(String, Vec<Violation>)> = Vec::new();
    for r in report
        .results
        .iter()
        .filter(|r| r.severity == Severity::Violation)
    {
        match by_subject.iter_mut().find(|(s, _)| *s == r.focus_node) {
            Some((_, v)) => v.push(Violation::of(r)),
            None => by_subject.push((r.focus_node.clone(), vec![Violation::of(r)])),
        }
    }
    by_subject.truncate(cap);
    let now = registry::now();
    let items: Vec<ReviewItem> = by_subject
        .into_iter()
        .map(|(subject, violations)| {
            let id = uuid::Uuid::new_v4().to_string();
            ReviewItem {
                iri: item_iri(&id),
                id,
                source: source.iri(),
                run: run.id.clone(),
                graph: run.graph.clone(),
                mapping: run.mapping_id.clone(),
                mapping_version: run.mapping_version,
                snapshot: crate::ldes::capture::describe_entity(store, &run.graph, &subject),
                subject,
                status: STATUSES[0].to_string(),
                violations,
                fixes: Vec::new(),
                reviewer: None,
                decision: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            }
        })
        .collect();
    insert_all(store, &review_graph_iri(&source.id), &items)?;
    Ok(items.len())
}

/// Move every item of a run to `status` — what promotion does.
pub fn mark_run(
    store: &TripleStore,
    source_id: &str,
    run_id: &str,
    status: &str,
) -> Result<(), String> {
    let graph = escape_sparql_iri(&review_graph_iri(source_id));
    let now = escape_sparql_literal(&registry::now());
    store
        .update(&format!(
            "{}DELETE {{ GRAPH <{graph}> {{ ?i ds:status ?s ; dct:modified ?m }} }}\n\
             INSERT {{ GRAPH <{graph}> {{ ?i ds:status \"{}\" ; dct:modified \"{now}\" }} }}\n\
             WHERE {{ GRAPH <{graph}> {{ ?i a ds:ReviewItem ; ds:run \"{}\" ; ds:status ?s ; dct:modified ?m }} }}",
            prefixes(),
            escape_sparql_literal(status),
            escape_sparql_literal(run_id)
        ))
        .map_err(|e| e.to_string())
}

/// Every item of a run, for the run's delete cascade.
pub fn delete_for_run(store: &TripleStore, source_id: &str, run_id: &str) -> Result<(), String> {
    let graph = escape_sparql_iri(&review_graph_iri(source_id));
    store
        .update(&format!(
            "{}DELETE {{ GRAPH <{graph}> {{ ?i ?p ?o }} }} WHERE {{ GRAPH <{graph}> {{ ?i a ds:ReviewItem ; ds:run \"{}\" ; ?p ?o }} }}",
            prefixes(),
            escape_sparql_literal(run_id)
        ))
        .map_err(|e| e.to_string())
}

/// A datasource's whole queue, for its delete cascade.
pub fn delete_for_source(store: &TripleStore, source_id: &str) -> Result<(), String> {
    store
        .bulk_delete_graphs(&[review_graph_iri(source_id).as_str()])
        .map_err(|e| e.to_string())
}

// ───────────────────────────── The fixer ─────────────────────────────

/// What the fixer would do to an item.
#[derive(Debug, Clone, Default, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FixPlan {
    pub fixes: Vec<Fix>,
    /// The change as an RDF Patch against the candidate graph.
    pub patch: String,
    /// The violations the fixer leaves alone, and why.
    pub unfixable: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Bound {
    MinInclusive,
    MaxInclusive,
}

/// `sh:minInclusive 0` → the bound kind, its numeric value and its lexical
/// form. Only the inclusive bounds: an exclusive one names no nearest value.
fn parse_bound(constraint: &str) -> Option<(Bound, f64, String)> {
    let (kind, rest) = match constraint.split_once(' ') {
        Some(("sh:minInclusive", r)) => (Bound::MinInclusive, r),
        Some(("sh:maxInclusive", r)) => (Bound::MaxInclusive, r),
        _ => return None,
    };
    let lexical = rest.trim().trim_matches('"').to_string();
    let value: f64 = lexical.parse().ok()?;
    value.is_finite().then_some((kind, value, lexical))
}

/// The literal on `subject → predicate` in `graph` whose lexical form is
/// `value`, as stored — datatype and all.
fn find_literal(
    store: &TripleStore,
    graph: &str,
    subject: &str,
    predicate: &str,
    value: &str,
) -> Option<Literal> {
    let g = NamedNodeRef::new(graph).ok()?;
    let s = NamedNodeRef::new(subject).ok()?;
    let p = NamedNodeRef::new(predicate).ok()?;
    store
        .store()
        .quads_for_pattern(
            Some(NamedOrBlankNodeRef::NamedNode(s)),
            Some(p),
            None,
            Some(GraphNameRef::NamedNode(g)),
        )
        .flatten()
        .find_map(|q| match q.object {
            Term::Literal(l) if l.value() == value => Some(l),
            _ => None,
        })
}

/// The same literal with another lexical form.
fn with_lexical(old: &Literal, lexical: &str) -> Literal {
    match old.language() {
        Some(lang) => Literal::new_language_tagged_literal(lexical, lang)
            .unwrap_or_else(|_| Literal::new_simple_literal(lexical)),
        None => Literal::new_typed_literal(lexical, old.datatype().into_owned()),
    }
}

/// Decide what to do about one violation: the rule and the new lexical form,
/// or why nothing deterministic applies.
fn rule_for(v: &Violation) -> Result<(&'static str, String), String> {
    let (kind, bound, bound_lexical) = parse_bound(&v.constraint).ok_or_else(|| {
        format!(
            "{}: no deterministic fix for {} — a value would have to be invented",
            v.path.as_deref().unwrap_or("?"),
            v.constraint
        )
    })?;
    let value = v
        .value
        .as_deref()
        .ok_or_else(|| format!("{}: the result names no value to correct", v.constraint))?;
    let number: f64 = value
        .trim()
        .parse()
        .map_err(|_| format!("{}: '{value}' is not a number", v.constraint))?;
    match kind {
        Bound::MinInclusive if number < bound => {
            if bound >= 0.0 && number < 0.0 && -number >= bound {
                // A negative where a non-negative is required, and the same
                // magnitude satisfies the bound: the sign is the typo.
                Ok((
                    "sign-typo",
                    value.trim().trim_start_matches('-').to_string(),
                ))
            } else {
                Ok(("clamp", bound_lexical))
            }
        }
        Bound::MaxInclusive if number > bound => Ok(("clamp", bound_lexical)),
        _ => Err(format!(
            "{}: '{value}' satisfies the bound as it stands",
            v.constraint
        )),
    }
}

/// Plan the deterministic fixes for an item against the candidate graph as
/// it is now. Applies nothing.
pub fn plan(store: &TripleStore, item: &ReviewItem) -> FixPlan {
    let mut out = FixPlan::default();
    if NamedNodeRef::new(&item.subject).is_err() {
        out.unfixable.push(format!(
            "subject {} is a blank node; a patch cannot name it",
            item.subject
        ));
        return out;
    }
    let mut lines: Vec<String> = Vec::new();
    for v in &item.violations {
        let (rule, to) = match rule_for(v) {
            Ok(r) => r,
            Err(why) => {
                out.unfixable.push(why);
                continue;
            }
        };
        let Some(path) = v.path.as_deref().filter(|p| NamedNodeRef::new(p).is_ok()) else {
            out.unfixable.push(format!(
                "{}: the path is not a single predicate",
                v.constraint
            ));
            continue;
        };
        let Some(from) = v.value.as_deref() else {
            continue;
        };
        if out.fixes.iter().any(|f| f.path == path && f.from == from) {
            // Two results on one triple: the first rule wins, the second
            // would act on a value that is no longer there.
            continue;
        }
        let Some(old) = find_literal(store, &item.graph, &item.subject, path, from) else {
            out.unfixable.push(format!(
                "{}: '{from}' is no longer on <{path}> in the candidate graph",
                v.constraint
            ));
            continue;
        };
        let new = with_lexical(&old, &to);
        lines.extend(replace_lines(item, path, &old, &new));
        out.fixes.push(Fix {
            rule: rule.to_string(),
            path: path.to_string(),
            from: from.to_string(),
            to,
            constraint: v.constraint.clone(),
        });
    }
    if !out.fixes.is_empty() {
        out.patch = patch_text(item, &lines);
    }
    out
}

/// The `D` / `A` pair that swaps one literal on the item's subject.
fn replace_lines(item: &ReviewItem, path: &str, old: &Literal, new: &Literal) -> [String; 2] {
    let graph = format!("<{}>", escape_sparql_iri(&item.graph));
    let subject = format!("<{}>", escape_sparql_iri(&item.subject));
    let predicate = format!("<{}>", escape_sparql_iri(path));
    [
        format!("D {subject} {predicate} {old} {graph} ."),
        format!("A {subject} {predicate} {new} {graph} ."),
    ]
}

/// One transaction over `lines`, headed by a fresh id and the item it is for.
fn patch_text(item: &ReviewItem, lines: &[String]) -> String {
    format!(
        "H id <urn:uuid:{}> .\nH review <{}> .\nTX .\n{}\nTC .\n",
        uuid::Uuid::new_v4(),
        item.iri,
        lines.join("\n")
    )
}

/// A replacement someone (the model) proposed for `path`, as an RDF Patch
/// against the candidate graph — when the subject holds a literal there to
/// replace. The literal is the one a violation on that path names, else the
/// only one on the path; two candidates is no answer, and nothing is invented.
pub fn proposed_patch(
    store: &TripleStore,
    item: &ReviewItem,
    path: &str,
    value: &str,
) -> Option<String> {
    NamedNodeRef::new(&item.subject).ok()?;
    let named = item
        .violations
        .iter()
        .filter(|v| v.path.as_deref() == Some(path))
        .find_map(|v| v.value.as_deref())
        .and_then(|from| find_literal(store, &item.graph, &item.subject, path, from));
    let old = match named {
        Some(l) => l,
        None => {
            let g = NamedNodeRef::new(&item.graph).ok()?;
            let s = NamedNodeRef::new(&item.subject).ok()?;
            let p = NamedNodeRef::new(path).ok()?;
            let literals: Vec<Literal> = store
                .store()
                .quads_for_pattern(
                    Some(NamedOrBlankNodeRef::NamedNode(s)),
                    Some(p),
                    None,
                    Some(GraphNameRef::NamedNode(g)),
                )
                .flatten()
                .filter_map(|q| match q.object {
                    Term::Literal(l) => Some(l),
                    _ => None,
                })
                .collect();
            match literals.as_slice() {
                [only] => only.clone(),
                _ => return None,
            }
        }
    };
    if old.value() == value {
        return None;
    }
    let new = with_lexical(&old, value);
    Some(patch_text(item, &replace_lines(item, path, &old, &new)))
}

/// Apply a plan through the store's patch path, refresh the snapshot and
/// mark the item corrected.
pub fn apply(store: &TripleStore, item: &mut ReviewItem, plan: &FixPlan) -> Result<(), String> {
    let patch = crate::rdf_patch::parse(&plan.patch)?;
    let sparql = crate::rdf_patch::to_sparql_update(&patch);
    store.update(&sparql).map_err(|e| e.to_string())?;
    item.snapshot = crate::ldes::capture::describe_entity(store, &item.graph, &item.subject);
    item.fixes.extend(plan.fixes.iter().cloned());
    item.status = "corrected".to_string();
    item.updated_at = registry::now();
    save(store, item)
}

// ───────────────────────────── HTTP ─────────────────────────────

type ApiErr = (StatusCode, String);

fn not_found(kind: &str, id: &str) -> ApiErr {
    (StatusCode::NOT_FOUND, format!("{kind} '{id}' not found"))
}

fn internal(m: impl std::fmt::Display) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, m.to_string())
}

fn ctx(state: &AppState) -> RunContext<'_> {
    RunContext {
        store: &state.store,
        auth_db: &state.auth_db,
        base_url: &state.base_url,
    }
}

fn commit(
    state: &AppState,
    user: &AuthenticatedUser,
    item: &ReviewItem,
    what: &str,
    changed: usize,
) {
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Source,
        format!(
            "review item {} on <{}> (run {}) {what}",
            item.id, item.subject, item.run
        ),
        Some(&user.user_id),
        Some(item.iri.clone()),
        vec![item.graph.clone(), graph_of(item)],
        changed,
        changed,
        None,
    );
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct ReviewQuery {
    /// One of the statuses; absent lists every item.
    pub status: Option<String>,
}

/// `GET /api/sources/:id/reviews`
pub async fn list_source_reviews(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ReviewQuery>,
) -> Result<Json<Vec<ReviewItem>>, ApiErr> {
    if registry::get_source(&state.store, &id).is_none() {
        return Err(not_found("datasource", &id));
    }
    let status = q.status.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(s) = status {
        if !STATUSES.contains(&s) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("unknown status '{s}'; one of {}", STATUSES.join(", ")),
            ));
        }
    }
    Ok(Json(list(&state.store, &id, status)))
}

/// `GET /api/reviews/:id`
pub async fn get_item(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<ReviewItem>, ApiErr> {
    find(&state.store, &id)
        .map(Json)
        .ok_or_else(|| not_found("review item", &id))
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusRequest {
    pub status: String,
    pub note: Option<String>,
}

/// `POST /api/reviews/:id/status`
pub async fn set_status(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<StatusRequest>,
) -> Result<Json<ReviewItem>, ApiErr> {
    let mut item = find(&state.store, &id).ok_or_else(|| not_found("review item", &id))?;
    let status = body.status.trim();
    if !STATUSES.contains(&status) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("unknown status '{status}'; one of {}", STATUSES.join(", ")),
        ));
    }
    item.status = status.to_string();
    item.reviewer = Some(super::handlers::actor_iri(&state, &user));
    if let Some(n) = body
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        item.decision = Some(n.to_string());
    }
    item.updated_at = registry::now();
    save(&state.store, &item).map_err(internal)?;
    commit(&state, &user, &item, &format!("set to {status}"), 0);
    Ok(Json(item))
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutofixRequest {
    /// `false` previews the patch; `true` applies it to the candidate graph.
    #[serde(default)]
    pub apply: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AutofixResponse {
    pub item: String,
    pub applied: bool,
    pub status: String,
    pub fixes: Vec<Fix>,
    pub patch: String,
    pub unfixable: Vec<String>,
}

/// `POST /api/reviews/:id/autofix`
pub async fn autofix(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AutofixRequest>,
) -> Result<Json<AutofixResponse>, ApiErr> {
    let mut item = find(&state.store, &id).ok_or_else(|| not_found("review item", &id))?;
    let plan = plan(&state.store, &item);
    if plan.fixes.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "nothing here can be fixed without inventing a value: {}",
                plan.unfixable.join("; ")
            ),
        ));
    }
    if body.apply {
        apply(&state.store, &mut item, &plan).map_err(internal)?;
        commit(
            &state,
            &user,
            &item,
            &format!("corrected by the fixer ({} fix(es))", plan.fixes.len()),
            plan.fixes.len(),
        );
    }
    Ok(Json(AutofixResponse {
        item: item.id.clone(),
        applied: body.apply,
        status: item.status.clone(),
        fixes: plan.fixes,
        patch: plan.patch,
        unfixable: plan.unfixable,
    }))
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub item: String,
    pub model: String,
    /// Always false: a suggestion is never applied by this endpoint.
    pub applied: bool,
    /// Whether the offending values went to the model (the datasource's
    /// `allowModelAssist`), or only the constraints.
    pub values_shared: bool,
    /// The model's answer: `{explanation, replacement?}` when it answered as
    /// asked, else its text under `text`.
    pub suggestion: serde_json::Value,
    /// The replacement as an RDF Patch against the candidate graph, when the
    /// model proposed one that names a literal the subject actually holds.
    /// A proposal, applied by nobody but a human through the patch path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

const SUGGEST_SYSTEM: &str =
    "You review one entity that an RML mapping produced from a relational \
source and that failed SHACL validation. Explain the most likely cause in one or two sentences \
and, only when the constraint and the given value imply one exact corrected value, propose it. \
Never invent data. Answer with JSON only: {\"explanation\": string, \"replacement\": \
{\"path\": string, \"value\": string} | null}.";

/// `POST /api/reviews/:id/suggest` — ask the configured model. Commits
/// nothing; a suggestion is applied by a human through the status and
/// patch paths.
///
/// What leaves the deployment follows the datasource's `allowModelAssist`:
/// with it, the constraint, the path and the offending value; without it,
/// the constraint and the path only, the value withheld — the same rule the
/// proposer is held to.
pub async fn suggest(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Suggestion>, ApiErr> {
    let item = find(&state.store, &id).ok_or_else(|| not_found("review item", &id))?;
    let source_id = item.source.trim_start_matches("urn:source:");
    let share_values = registry::get_source(&state.store, source_id)
        .map(|s| s.allow_model_assist)
        .unwrap_or(false);
    let mut prompt = String::from("Violations on one entity:\n");
    for v in &item.violations {
        prompt.push_str(&format!(
            "- constraint: {}; path: {}",
            v.constraint,
            v.path.as_deref().unwrap_or("(node)")
        ));
        if share_values {
            if let Some(val) = &v.value {
                prompt.push_str(&format!("; value: {val:?}"));
            }
            prompt.push_str(&format!("; message: {}", v.message));
        } else {
            prompt.push_str(
                "; value: [withheld — model assistance is not enabled for this datasource]",
            );
        }
        prompt.push('\n');
    }
    let model = crate::server::llm_sparql::default_model();
    let answer = crate::server::llm_sparql::chat_completion(&model, SUGGEST_SYSTEM, &prompt, 400)
        .await
        .map_err(|e| match e {
            crate::server::error::AppError::ServiceUnavailable(m) => {
                (StatusCode::SERVICE_UNAVAILABLE, m)
            }
            other => (StatusCode::INTERNAL_SERVER_ERROR, other.message()),
        })?;
    let suggestion = serde_json::from_str::<serde_json::Value>(&answer)
        .ok()
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({ "text": answer }));
    // A replacement the model named becomes a patch a human can read and
    // apply — never applied here.
    let patch = match (
        suggestion["replacement"]["path"].as_str(),
        suggestion["replacement"]["value"].as_str(),
    ) {
        (Some(path), Some(value)) => proposed_patch(&state.store, &item, path, value),
        _ => None,
    };
    Ok(Json(Suggestion {
        item: item.id,
        model,
        applied: false,
        values_shared: share_values,
        suggestion,
        patch,
    }))
}

/// Items awaiting someone across every datasource: opened, gathering or
/// corrected but not yet decided or promoted. The metrics endpoint's
/// `reviewQueueDepth`.
pub fn queue_depth(store: &TripleStore) -> u64 {
    let closed = ["valid", "approved", "rejected", "promoted"]
        .iter()
        .map(|s| format!("\"{s}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!(
        "{}SELECT (COUNT(?i) AS ?n) WHERE {{ GRAPH ?g {{ ?i a ds:ReviewItem ; ds:status ?s }} \
         FILTER(STRSTARTS(STR(?g), \"urn:system:reviews:\") && ?s NOT IN ({closed})) }}",
        prefixes()
    );
    let Ok(QueryResults::Solutions(rows)) = store.query(&query) else {
        return 0;
    };
    rows.flatten()
        .next()
        .and_then(|row| lex(&row, "n"))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// `POST /api/runs/:id/promote` — re-gate a kept candidate and swap it in.
pub async fn promote_run(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiErr> {
    let run = registry::get_run(&state.store, &id).ok_or_else(|| not_found("run", &id))?;
    let source = registry::get_source(&state.store, &run.source_id)
        .ok_or_else(|| not_found("datasource", &run.source_id))?;
    let actor = super::handlers::actor_iri(&state, &user);
    let blocking = state.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        runs::promote_candidate(ctx(&blocking), &source, &run, Some(&actor))
    })
    .await
    .map_err(internal)?;
    match outcome {
        Ok(promotion) => {
            if let Err(e) = mark_run(&state.store, &promotion.source.id, &promotion.run.id, "promoted") {
                tracing::warn!(run = %promotion.run.id, "review items not marked promoted: {e}");
            }
            Ok((
                StatusCode::OK,
                Json(json!({
                    "promotion": {
                        "id": promotion.id,
                        "iri": format!("urn:promotion:{}", promotion.id),
                        "actor": promotion.actor,
                        "at": promotion.at,
                    },
                    "run": RunResponse::of(&state.store, &promotion.run),
                    "source": SourceResponse::from(&promotion.source),
                })),
            )
                .into_response())
        }
        Err(PromoteError::Gate(report)) => Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "the SHACL write gate still refuses the candidate; production is unchanged",
                "report": {
                    "conforms": report.conforms,
                    "results_count": report.results_count,
                    "results": report.results,
                },
            })),
        )
            .into_response()),
        Err(PromoteError::Conflict(m)) => Err((StatusCode::CONFLICT, m)),
        Err(PromoteError::Failed(m)) => Err(internal(m)),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/sources/:id/reviews", get(list_source_reviews))
        .route("/api/reviews/:id", get(get_item))
        .route("/api/reviews/:id/status", post(set_status))
        .route("/api/reviews/:id/autofix", post(autofix))
        .route("/api/reviews/:id/suggest", post(suggest))
        .route("/api/runs/:id/promote", post(promote_run))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shacl::report::ValidationResult;
    use oxigraph::io::RdfFormat;

    const EX: &str = "http://example.org/products/";
    const PRICE: &str = "http://example.org/products/ontology#hasPrice";

    fn result(
        focus: &str,
        constraint: &str,
        value: Option<&str>,
        path: Option<&str>,
    ) -> ValidationResult {
        ValidationResult {
            severity: Severity::Violation,
            focus_node: focus.to_string(),
            path: path.map(|p| format!("<{p}>")),
            value: value.map(str::to_string),
            source_shape: "http://example.org/products/ontology#ProductShape".into(),
            source_constraint: constraint.to_string(),
            message: format!("{constraint} failed"),
        }
    }

    fn candidate(store: &TripleStore, graph: &str) {
        store
            .load_str(
                &format!(
                    "<{EX}product_2> <{PRICE}> \"-0.1\"^^<{XSD}decimal> .\n\
                     <{EX}product_2> <{EX}ontology#name> \"Nut\" .\n\
                     <{EX}product_3> <{PRICE}> \"5000.5\"^^<{XSD}decimal> .\n"
                ),
                RdfFormat::NTriples,
                Some(graph),
            )
            .unwrap();
    }

    fn opened(store: &TripleStore) -> (SqlSource, RunRecord) {
        let source = SqlSource {
            id: "shop".into(),
            ..Default::default()
        };
        let run = RunRecord {
            id: "r1".into(),
            source_id: "shop".into(),
            mapping_id: "m".into(),
            mapping_version: 1,
            graph: run_graph_iri("r1"),
            ..Default::default()
        };
        candidate(store, &run.graph);
        let report = ValidationReport {
            conforms: false,
            results: vec![
                result(
                    &format!("{EX}product_2"),
                    "sh:minInclusive 0",
                    Some("-0.1"),
                    Some(PRICE),
                ),
                result(
                    &format!("{EX}product_3"),
                    "sh:maxInclusive 1000",
                    Some("5000.5"),
                    Some(PRICE),
                ),
                result(
                    &format!("{EX}product_3"),
                    "sh:minCount 1",
                    None,
                    Some(&format!("{EX}ontology#name")),
                ),
            ],
            results_count: 3,
            metrics: None,
        };
        assert_eq!(open_items(store, &source, &run, &report).unwrap(), 2);
        (source, run)
    }

    #[test]
    fn a_refusal_opens_one_item_per_subject_with_its_snapshot() {
        let store = TripleStore::in_memory().unwrap();
        let (_, run) = opened(&store);
        let items = list(&store, "shop", None);
        assert_eq!(items.len(), 2);
        let nut = items
            .iter()
            .find(|i| i.subject == format!("{EX}product_2"))
            .unwrap();
        assert_eq!(nut.status, "needsHuman");
        assert_eq!(nut.run, run.id);
        assert_eq!(nut.violations.len(), 1);
        assert_eq!(nut.violations[0].path.as_deref(), Some(PRICE));
        assert!(nut.snapshot.contains("\"-0.1\""), "{}", nut.snapshot);
        assert!(nut.snapshot.contains("\"Nut\""), "{}", nut.snapshot);
        let washer = items
            .iter()
            .find(|i| i.subject == format!("{EX}product_3"))
            .unwrap();
        assert_eq!(washer.violations.len(), 2, "both results on one subject");
        assert_eq!(find(&store, &nut.id).unwrap(), *nut);
        assert_eq!(list(&store, "shop", Some("promoted")).len(), 0);
        assert!(find(&store, "nope").is_none());
        assert!(find(&store, "../x").is_none());

        mark_run(&store, "shop", "r1", "promoted").unwrap();
        assert!(list(&store, "shop", None)
            .iter()
            .all(|i| i.status == "promoted"));
        delete_for_run(&store, "shop", "r1").unwrap();
        assert!(list(&store, "shop", None).is_empty());
    }

    #[test]
    fn the_fixer_flips_a_sign_clamps_to_a_bound_and_invents_nothing() {
        let store = TripleStore::in_memory().unwrap();
        opened(&store);
        let items = list(&store, "shop", None);
        let mut nut = items
            .iter()
            .find(|i| i.subject == format!("{EX}product_2"))
            .cloned()
            .unwrap();
        let p = plan(&store, &nut);
        assert_eq!(p.fixes.len(), 1, "{p:?}");
        assert_eq!(p.fixes[0].rule, "sign-typo");
        assert_eq!(p.fixes[0].to, "0.1");
        assert!(
            p.patch.contains("TX .") && p.patch.contains("TC ."),
            "{}",
            p.patch
        );
        assert!(
            p.patch.contains(&format!(
                "\nD <{EX}product_2> <{PRICE}> \"-0.1\"^^<{XSD}decimal> <urn:run:r1> ."
            )),
            "{}",
            p.patch
        );
        assert!(
            p.patch.contains(&format!(
                "\nA <{EX}product_2> <{PRICE}> \"0.1\"^^<{XSD}decimal> <urn:run:r1> ."
            )),
            "{}",
            p.patch
        );
        // Planning changed nothing.
        assert_eq!(find(&store, &nut.id).unwrap().status, "needsHuman");

        apply(&store, &mut nut, &p).unwrap();
        assert_eq!(nut.status, "corrected");
        assert_eq!(nut.fixes.len(), 1);
        assert!(nut.snapshot.contains("\"0.1\""), "{}", nut.snapshot);
        assert!(!nut.snapshot.contains("\"-0.1\""), "{}", nut.snapshot);
        let stored = find(&store, &nut.id).unwrap();
        assert_eq!(stored.status, "corrected");
        assert_eq!(stored.fixes, nut.fixes);
        // A second plan finds the value gone.
        let again = plan(&store, &stored);
        assert!(again.fixes.is_empty());
        assert!(again.unfixable[0].contains("no longer"), "{again:?}");

        // The washer: the price is clamped, the missing name is not invented.
        let washer = items
            .iter()
            .find(|i| i.subject == format!("{EX}product_3"))
            .cloned()
            .unwrap();
        let p = plan(&store, &washer);
        assert_eq!(p.fixes.len(), 1, "{p:?}");
        assert_eq!(p.fixes[0].rule, "clamp");
        assert_eq!(p.fixes[0].to, "1000");
        assert_eq!(p.unfixable.len(), 1);
        assert!(p.unfixable[0].contains("invented"), "{p:?}");
    }

    #[test]
    fn rules_cover_the_bound_cases_only() {
        let v = |constraint: &str, value: Option<&str>| Violation {
            constraint: constraint.into(),
            path: Some(PRICE.into()),
            value: value.map(str::to_string),
            message: String::new(),
            shape: String::new(),
        };
        assert_eq!(
            rule_for(&v("sh:minInclusive 0", Some("-0.1"))).unwrap(),
            ("sign-typo", "0.1".to_string())
        );
        // A negative bound: below it is a clamp, not a typo.
        assert_eq!(
            rule_for(&v("sh:minInclusive -10", Some("-11"))).unwrap(),
            ("clamp", "-10".to_string())
        );
        // A flipped sign that still misses the bound is a clamp.
        assert_eq!(
            rule_for(&v("sh:minInclusive 5", Some("-2"))).unwrap(),
            ("clamp", "5".to_string())
        );
        assert_eq!(
            rule_for(&v("sh:maxInclusive 1000", Some("5000.0"))).unwrap(),
            ("clamp", "1000".to_string())
        );
        assert!(rule_for(&v("sh:maxInclusive 1000", Some("999"))).is_err());
        assert!(rule_for(&v("sh:maxExclusive 1000", Some("5000"))).is_err());
        assert!(rule_for(&v("sh:minCount 1", None)).is_err());
        assert!(rule_for(&v("sh:datatype <x>", Some("abc"))).is_err());
        assert!(rule_for(&v("sh:minInclusive 0", Some("abc"))).is_err());
        assert_eq!(plain_iri("<http://x/p>"), "http://x/p");
        assert_eq!(
            plain_iri("<http://x/p>/<http://x/q>"),
            "<http://x/p>/<http://x/q>"
        );
    }

    #[test]
    fn the_cap_bounds_what_one_refusal_opens() {
        let store = TripleStore::in_memory().unwrap();
        let (source, run) = {
            let source = SqlSource {
                id: "capped".into(),
                ..Default::default()
            };
            let run = RunRecord {
                id: "r2".into(),
                source_id: "capped".into(),
                mapping_id: "m".into(),
                mapping_version: 1,
                graph: run_graph_iri("r2"),
                ..Default::default()
            };
            (source, run)
        };
        let report = ValidationReport {
            conforms: false,
            results: (1..=3)
                .map(|n| {
                    result(
                        &format!("{EX}p{n}"),
                        "sh:minInclusive 0",
                        Some("-1"),
                        Some(PRICE),
                    )
                })
                .collect(),
            results_count: 3,
            metrics: None,
        };
        // The cap is passed explicitly: the environment is shared by every
        // test in this process, so setting the variable here would race.
        let opened = open_items_capped(&store, &source, &run, &report, 1).unwrap();
        assert_eq!(opened, 1);
        assert_eq!(list(&store, "capped", None).len(), 1);
        delete_for_source(&store, "capped").unwrap();
        assert!(list(&store, "capped", None).is_empty());
    }
}
