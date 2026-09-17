//! Source profiling: what a datasource's tables actually contain.
//!
//! A profile is RDF in a per-source graph, one version per profiling run, so
//! the drift baseline is simply the previous version and drift is the version
//! diff this store already computes.
//!
//! **Why the store profiles.** The external mapping proposer runs offline and
//! never receives a DSN, a row or a credential, so it cannot look at the
//! source itself. It reads this graph instead. Everything written here has
//! therefore already left the database, which fixes what may be written:
//! metadata and aggregates, plus the values of a *code list* — because a code
//! list with its values withheld would be useless to the reader it exists for.
//! Free text is summarised by length and shape, never by content.
//!
//! **What makes a version diffable.** Every node in a profile graph has a
//! stable IRI derived from the source, table and column, so version *n* and
//! version *n-1* line up triple for triple and a diff is drift. Nothing that
//! changes on its own — a timestamp, a run id, a duration — is written into
//! the profile graph; the PROV activity carrying all of that goes into
//! `urn:system:sources` beside the run and rollback activities, so a clock
//! tick never reads as a change in the data.
//!
//! **Vocabulary.** `csvw:` describes tables and columns, `void:` counts
//! entities, `prov:`/`dct:` carry provenance, and `dsprof:` covers only what
//! those lack: the statistics themselves (distinct and NULL counts, the
//! numeric summary, the detected shape) and the structural hash.
//!
//! A profile graph belongs to no dataset, exactly like `urn:system:sources`,
//! so it is outside a caller's SPARQL scope and is served through
//! `GET /api/sources/:id/profile` rather than through `POST /sparql`.

use std::time::Instant;

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header::CONTENT_TYPE, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Json, Router};
use oxigraph::sparql::QueryResults;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use ots_plugin_api::sources::{
    ColumnProfile, DetectedPattern, TableKind, TableProfile, ValueKind,
    LOW_CARDINALITY_MAX_VALUE_LEN,
};

use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;
use crate::store::{escape_sparql_iri, escape_sparql_literal, TripleStore};

use super::model::{source_iri, valid_id, SOURCES_GRAPH};
use super::{registry, runs};

/// The profiling vocabulary. Provisional base IRI, versioned with the
/// platform vocabularies under `w3id.org`, like [`super::model::DS`].
pub const PROF: &str = "https://w3id.org/open-triplestore/profile#";

/// The label [`PROF`] is declared under, here and in every served document.
///
/// Deliberately not `prof:`: the bundled prefix registry binds that to the
/// W3C Profiles Vocabulary, so anywhere the store expands or shortens a CURIE
/// against the registry — editor completion, prefix-aware display, a person
/// typing `prof:distinctValues` — `prof:` already means something else. It
/// reads as the datasource vocabularies' own, beside `ds:`.
pub const PROF_LABEL: &str = "dsprof";

const CSVW: &str = "http://www.w3.org/ns/csvw#";
const VOID: &str = "http://rdfs.org/ns/void#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const PROV: &str = "http://www.w3.org/ns/prov#";
const DCT: &str = "http://purl.org/dc/terms/";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The recipe [`structural_hash`] follows. It is part of the hashed text, so
/// a future change to what "structure" means cannot masquerade as drift in
/// the source.
const HASH_RECIPE: &str = "ots-structural-hash/1";

fn prefixes() -> String {
    let ds = super::model::DS;
    format!(
        "PREFIX csvw: <{CSVW}>\nPREFIX void: <{VOID}>\nPREFIX {PROF_LABEL}: <{PROF}>\n\
         PREFIX rdf: <{RDF}>\nPREFIX prov: <{PROV}>\nPREFIX dct: <{DCT}>\n\
         PREFIX xsd: <{XSD}>\nPREFIX ds: <{ds}>\n"
    )
}

// ──────────────────────────────── Naming ────────────────────────────────

/// The profile series of a source: the subject every version is a version of.
pub fn profile_graph_iri(source_id: &str) -> String {
    format!("urn:source:{source_id}:profile")
}

/// One profiling run's graph. The IRI *is* the version entity's IRI, as with
/// a mapping version, so provenance points at exactly the triples produced.
pub fn profile_version_graph_iri(source_id: &str, version: u32) -> String {
    format!("urn:source:{source_id}:profile:version:{version}")
}

/// The activity that produced a version. Distinct from the graph: an activity
/// does not generate itself.
pub fn profile_activity_iri(source_id: &str, version: u32) -> String {
    format!("{}:activity", profile_version_graph_iri(source_id, version))
}

/// Percent-encode a catalogue name so it is one unambiguous IRI segment.
///
/// Injective: `%` is itself encoded, so two different table names cannot
/// collide on one IRI and quietly merge their statistics.
fn segment(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A table's node. Version-independent on purpose: the same table keeps the
/// same IRI in every profile version, which is what lets a diff pair them.
fn table_iri(source_id: &str, table: &str) -> String {
    format!("urn:source:{source_id}:table:{}", segment(table))
}
fn schema_iri(source_id: &str, table: &str) -> String {
    format!("{}:schema", table_iri(source_id, table))
}
fn column_iri(source_id: &str, table: &str, column: &str) -> String {
    format!("{}:column:{}", table_iri(source_id, table), segment(column))
}
/// A ranked value of a code list. Keyed by rank, so the node identity is
/// "the most common value", and a diff reports that it changed.
fn value_iri(source_id: &str, table: &str, column: &str, rank: usize) -> String {
    format!("{}:value:{rank}", column_iri(source_id, table, column))
}
fn foreign_key_iri(source_id: &str, table: &str, index: usize) -> String {
    format!("{}:foreignKey:{index}", table_iri(source_id, table))
}

// ─────────────────────────── The structural hash ───────────────────────────

/// A fingerprint of a table's *structure*, so a later re-map can tell which
/// tables actually changed.
///
/// It covers, and only covers:
///
/// * the table name;
/// * every column, in catalogue order: position, name, generic type, the
///   dialect's native type, and whether it is nullable;
/// * the primary key, in key order;
/// * every unique key, sorted — these are structure because the join planner
///   pushes a join down on them;
/// * every foreign key, sorted.
///
/// It deliberately does **not** cover row counts, NULL or distinct counts,
/// value distributions, detected patterns or any timestamp. A hash that moved
/// on every INSERT would answer "has anything changed?" with "yes" forever,
/// which is the same as not having one.
pub fn structural_hash(profile: &TableProfile) -> String {
    // Separators are escaped inside every field, so no two different
    // structures can canonicalise to the same text.
    fn esc(value: &str) -> String {
        value
            .replace('\\', "\\\\")
            .replace('\t', "\\t")
            .replace('\n', "\\n")
    }

    let mut canonical = format!("{HASH_RECIPE}\ntable\t{}\n", esc(&profile.table));
    for c in &profile.columns {
        canonical.push_str(&format!(
            "column\t{}\t{}\t{:?}\t{}\t{}\n",
            c.position,
            esc(&c.name),
            c.generic_type,
            esc(&c.native_type),
            c.nullable
        ));
    }
    canonical.push_str(&format!(
        "pk\t{}\n",
        profile
            .primary_key
            .iter()
            .map(|c| esc(c))
            .collect::<Vec<_>>()
            .join("\t")
    ));
    let mut keys: Vec<String> = profile
        .unique_keys
        .iter()
        .map(|k| k.iter().map(|c| esc(c)).collect::<Vec<_>>().join("\t"))
        .collect();
    keys.sort();
    for k in keys {
        canonical.push_str(&format!("unique\t{k}\n"));
    }
    let mut foreign: Vec<String> = profile
        .foreign_keys
        .iter()
        .map(|f| {
            format!(
                "{}\t{}\t{}",
                f.columns
                    .iter()
                    .map(|c| esc(c))
                    .collect::<Vec<_>>()
                    .join(","),
                esc(&f.ref_table),
                f.ref_columns
                    .iter()
                    .map(|c| esc(c))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect();
    foreign.sort();
    for f in foreign {
        canonical.push_str(&format!("fk\t{f}\n"));
    }

    Sha256::digest(canonical.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ──────────────────────────── RDF serialisation ────────────────────────────

fn iri(value: &str) -> String {
    format!("<{}>", escape_sparql_iri(value))
}
fn lit(value: &str) -> String {
    format!("\"{}\"", escape_sparql_literal(value))
}
fn int(value: u64) -> String {
    format!("\"{value}\"^^xsd:integer")
}

/// An `xsd:decimal` lexical form, or `None` for a value that has none.
///
/// A non-finite statistic is omitted rather than written: `"NaN"^^xsd:decimal`
/// is not a decimal, and a consumer that trusted the datatype would be wrong.
fn decimal(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    let mut s = format!("{value:.6}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.push('0');
        }
    }
    Some(format!("\"{s}\"^^xsd:decimal"))
}

/// The XSD type a column's values take, following the same natural-datatype
/// mapping a bare `rr:column` gets in a run.
fn xsd_datatype(kind: ValueKind) -> Option<&'static str> {
    Some(match kind {
        ValueKind::Text | ValueKind::Uuid | ValueKind::Json => "xsd:string",
        ValueKind::Integer => "xsd:integer",
        ValueKind::Decimal => "xsd:decimal",
        ValueKind::Float => "xsd:double",
        ValueKind::Boolean => "xsd:boolean",
        ValueKind::Date => "xsd:date",
        ValueKind::Time => "xsd:time",
        ValueKind::DateTime => "xsd:dateTime",
        ValueKind::Binary => "xsd:hexBinary",
        // A type the dialect would not name is not given one here either.
        ValueKind::Other => return None,
    })
}

fn pattern_iri(pattern: DetectedPattern) -> &'static str {
    match pattern {
        DetectedPattern::Email => "dsprof:Email",
        DetectedPattern::Iri => "dsprof:Iri",
        DetectedPattern::Uuid => "dsprof:Uuid",
        DetectedPattern::Date => "dsprof:Date",
        DetectedPattern::Code => "dsprof:Code",
        DetectedPattern::Phone => "dsprof:Phone",
    }
}

fn column_triples(source_id: &str, table: &str, column: &ColumnProfile) -> String {
    let c = iri(&column_iri(source_id, table, &column.name));
    let mut out = format!(
        "  {c} a csvw:Column ;\n    csvw:name {} ;\n    csvw:number {} ;\n    \
         csvw:required \"{}\"^^xsd:boolean ;\n    dsprof:nativeType {} .\n",
        lit(&column.name),
        int(column.position as u64),
        !column.nullable,
        lit(&column.native_type),
    );
    if let Some(dt) = xsd_datatype(column.generic_type) {
        out.push_str(&format!("  {c} csvw:datatype {dt} .\n"));
    }
    if let Some(d) = column.distinct_count {
        out.push_str(&format!("  {c} dsprof:distinctValues {} .\n", int(d)));
    }
    if let Some(n) = column.null_count {
        out.push_str(&format!("  {c} dsprof:nullCount {} .\n", int(n)));
    }
    if let Some(r) = column.cardinality_ratio.and_then(decimal) {
        out.push_str(&format!("  {c} dsprof:cardinalityRatio {r} .\n"));
    }
    if let Some(l) = column.mean_length.and_then(decimal) {
        out.push_str(&format!("  {c} dsprof:meanLength {l} .\n"));
    }
    if let Some(n) = &column.numeric {
        for (predicate, value) in [
            ("dsprof:min", n.min),
            ("dsprof:max", n.max),
            ("dsprof:mean", n.mean),
            ("dsprof:p50", n.p50),
            ("dsprof:p99", n.p99),
        ] {
            if let Some(v) = decimal(value) {
                out.push_str(&format!("  {c} {predicate} {v} .\n"));
            }
        }
    }
    if let Some(p) = column.pattern {
        out.push_str(&format!("  {c} dsprof:pattern {} .\n", pattern_iri(p)));
        if let Some(conf) = column.pattern_confidence.and_then(decimal) {
            out.push_str(&format!("  {c} dsprof:patternConfidence {conf} .\n"));
        }
    }
    // The values of a code list, and nothing else that is a value.
    //
    // The ceiling is re-checked here, not only in the driver that read the
    // values. This is the one place row content can reach the graph the offline
    // proposer reads, and a driver added later — a plugin, outside this crate —
    // would otherwise have to remember the rule for it to hold. A column with
    // any over-long value contributes no list at all, for the same reason the
    // reader drops it: a list quietly missing a member would let a proposer
    // build an enumeration that is wrong with nothing to reveal it.
    let within_ceiling = column
        .top_values
        .iter()
        .all(|v| v.value.chars().count() <= LOW_CARDINALITY_MAX_VALUE_LEN);
    for (i, v) in column
        .top_values
        .iter()
        .enumerate()
        .take_while(|_| within_ceiling)
    {
        let rank = i + 1;
        let node = iri(&value_iri(source_id, table, &column.name, rank));
        out.push_str(&format!(
            "  {c} dsprof:topValue {node} .\n  {node} rdf:value {} ;\n    dsprof:occurrences {} ;\n    \
             dsprof:rank {} .\n",
            lit(&v.value),
            int(v.count),
            int(rank as u64),
        ));
    }
    out
}

fn table_triples(source_id: &str, profile: &TableProfile) -> String {
    let t = iri(&table_iri(source_id, &profile.table));
    let schema = iri(&schema_iri(source_id, &profile.table));
    let mut out = format!(
        "  {t} a csvw:Table, void:Dataset ;\n    dct:title {} ;\n    ds:source {} ;\n    \
         csvw:tableSchema {schema} ;\n    dsprof:structuralHash {} ;\n    dsprof:sampledRows {} .\n",
        lit(&profile.table),
        iri(&source_iri(source_id)),
        lit(&structural_hash(profile)),
        int(profile.sampled_rows),
    );
    if profile.kind == TableKind::View {
        out.push_str(&format!("  {t} dsprof:isView \"true\"^^xsd:boolean .\n"));
    }
    if let Some(rows) = profile.row_count {
        out.push_str(&format!("  {t} void:entities {} .\n", int(rows)));
    }
    out.push_str(&format!("  {schema} a csvw:Schema .\n"));
    for c in &profile.columns {
        out.push_str(&format!(
            "  {schema} csvw:column {} .\n",
            iri(&column_iri(source_id, &profile.table, &c.name))
        ));
        out.push_str(&column_triples(source_id, &profile.table, c));
    }
    for k in &profile.primary_key {
        out.push_str(&format!("  {schema} csvw:primaryKey {} .\n", lit(k)));
    }
    for (i, f) in profile.foreign_keys.iter().enumerate() {
        let fk = iri(&foreign_key_iri(source_id, &profile.table, i + 1));
        let reference = iri(&format!(
            "{}:reference",
            foreign_key_iri(source_id, &profile.table, i + 1)
        ));
        out.push_str(&format!(
            "  {schema} csvw:foreignKey {fk} .\n  {fk} a csvw:ForeignKey ; csvw:reference \
             {reference} .\n  {reference} a csvw:TableReference ; csvw:resource {} .\n",
            iri(&table_iri(source_id, &f.ref_table))
        ));
        for c in &f.columns {
            out.push_str(&format!("  {fk} csvw:columnReference {} .\n", lit(c)));
        }
        for c in &f.ref_columns {
            out.push_str(&format!(
                "  {reference} csvw:columnReference {} .\n",
                lit(c)
            ));
        }
    }
    out
}

/// Write one profile version into its own graph.
///
/// The graph is dropped first in the same request, so a version number left
/// behind by an interrupted run is overwritten rather than merged into — two
/// profiles in one graph would report every column twice.
pub fn write_profile(
    store: &TripleStore,
    source_id: &str,
    version: u32,
    profiles: &[TableProfile],
) -> Result<(), String> {
    let graph = profile_version_graph_iri(source_id, version);
    let mut body = String::new();
    for p in profiles {
        body.push_str(&table_triples(source_id, p));
    }
    // One prologue at the head, inherited by both operations: SPARQL Update
    // allows exactly one per request.
    let sparql = format!(
        "{pfx}DROP SILENT GRAPH {g};\nINSERT DATA {{ GRAPH {g} {{\n{body}}} }}",
        pfx = prefixes(),
        g = iri(&graph),
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

/// What one profiling run is, apart from its statistics: who ran it, when,
/// how long it took and how much it looked at.
#[derive(Debug, Clone, Default)]
pub struct ProfilingRun {
    pub version: u32,
    pub actor: Option<String>,
    pub started_at: String,
    pub ended_at: String,
    pub duration_ms: u64,
    pub tables: usize,
}

/// Record the profiling run: the PROV activity and the version entity, in
/// `urn:system:sources` beside the run and rollback activities.
///
/// `ds:source` is what makes it show up in the datasource's PROV trail, so
/// `GET /api/sources/:id/provenance` answers "when was this last profiled, by
/// whom" without a second endpoint.
pub fn record_profiling(
    store: &TripleStore,
    source_id: &str,
    run: &ProfilingRun,
) -> Result<(), String> {
    let ProfilingRun {
        version,
        actor,
        started_at,
        ended_at,
        duration_ms,
        tables,
    } = run;
    let (version, duration_ms, tables) = (*version, *duration_ms, *tables);
    let (started_at, ended_at) = (started_at.as_str(), ended_at.as_str());
    let actor = actor.as_deref();
    let entity = iri(&profile_version_graph_iri(source_id, version));
    let activity = iri(&profile_activity_iri(source_id, version));
    let source = iri(&source_iri(source_id));
    let series = iri(&profile_graph_iri(source_id));

    let mut body = format!(
        "  {activity} a prov:Activity, dsprof:Profiling ;\n    ds:source {source} ;\n    \
         prov:used {source} ;\n    prov:generated {entity} ;\n    \
         prov:startedAtTime \"{started}\"^^xsd:dateTime ;\n    \
         prov:endedAtTime \"{ended}\"^^xsd:dateTime ;\n    ds:durationMs {} ;\n    \
         dct:created {} .\n",
        int(duration_ms),
        lit(started_at),
        started = escape_sparql_literal(started_at),
        ended = escape_sparql_literal(ended_at),
    );
    if let Some(a) = actor {
        body.push_str(&format!(
            "  {activity} prov:wasAssociatedWith {} .\n",
            iri(a)
        ));
    }
    body.push_str(&format!(
        "  {entity} a dsprof:SourceProfile, prov:Entity ;\n    ds:source {source} ;\n    \
         ds:version {} ;\n    dct:isVersionOf {series} ;\n    prov:wasGeneratedBy {activity} ;\n    \
         dct:created {} ;\n    dsprof:tables {} .\n  {series} ds:source {source} ; \
         dct:hasVersion {entity} .\n",
        int(version as u64),
        lit(started_at),
        int(tables as u64),
    ));

    let sparql = format!(
        "{pfx}INSERT DATA {{ GRAPH <{SOURCES_GRAPH}> {{\n{body}}} }}",
        pfx = prefixes()
    );
    store.update(&sparql).map_err(|e| e.to_string())
}

/// Delete every profile a datasource has: the version graphs, the version
/// entities, the profiling activities and the series they hang off.
///
/// Called when the datasource itself is deleted. A profile outlives the
/// database it describes otherwise: [`versions`] would still return the old
/// sequence, so re-registering the same id continues it, and
/// `GET /api/sources/:id/profile?version=1` would serve the *previous*
/// database's profile — code-list values included — as this datasource's own
/// history.
pub fn delete_profiles(store: &TripleStore, source_id: &str) -> Result<(), String> {
    let source = iri(&source_iri(source_id));
    let series = iri(&profile_graph_iri(source_id));

    // The version graph's IRI is the version entity's, so the recorded
    // versions name every graph to drop.
    let mut sparql = prefixes();
    for version in versions(store, source_id) {
        sparql.push_str(&format!(
            "DROP SILENT GRAPH {} ;\n",
            iri(&profile_version_graph_iri(source_id, version))
        ));
    }
    // One prologue for the request, so these are chained rather than sent
    // separately: a half-deleted profile is a profile.
    sparql.push_str(&format!(
        "DELETE {{ GRAPH <{SOURCES_GRAPH}> {{ ?s ?p ?o }} }}\n\
         WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ ?s a dsprof:SourceProfile ; ds:source {source} . \
         ?s ?p ?o }} }} ;\n\
         DELETE {{ GRAPH <{SOURCES_GRAPH}> {{ ?s ?p ?o }} }}\n\
         WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ ?s a dsprof:Profiling ; ds:source {source} . \
         ?s ?p ?o }} }} ;\n\
         DELETE WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ {series} ?p ?o }} }}"
    ));
    store.update(&sparql).map_err(|e| e.to_string())
}

/// Every profile version recorded for a source, oldest first.
pub fn versions(store: &TripleStore, source_id: &str) -> Vec<u32> {
    let query = format!(
        "{pfx}SELECT ?v WHERE {{ GRAPH <{SOURCES_GRAPH}> {{ ?p a dsprof:SourceProfile ; ds:source {s} ; ds:version ?v }} }}",
        pfx = prefixes(),
        s = iri(&source_iri(source_id)),
    );
    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(solutions)) = store.query(&query) {
        for solution in solutions.flatten() {
            if let Some(oxigraph::model::Term::Literal(l)) = solution.get("v") {
                if let Ok(v) = l.value().parse::<u32>() {
                    out.push(v);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The newest profile version, if the source has ever been profiled.
pub fn latest_version(store: &TripleStore, source_id: &str) -> Option<u32> {
    versions(store, source_id).into_iter().max()
}

/// One profile version as Turtle: the statistics from its own graph, plus the
/// activity and version entity from `urn:system:sources`.
///
/// Both halves travel together because a reader needs the numbers *and* what
/// produced them, while the graph itself stays free of anything that changes
/// on its own — that is what keeps a version diff meaningful.
pub fn profile_turtle(store: &TripleStore, source_id: &str, version: u32) -> String {
    // The version graph and the version entity are one IRI, as with a mapping
    // version: the graph *is* what the activity generated.
    let entity = iri(&profile_version_graph_iri(source_id, version));
    let graph = &entity;
    let activity = iri(&profile_activity_iri(source_id, version));
    let query = format!(
        "{pfx}CONSTRUCT {{ ?s ?p ?o }} WHERE {{\n\
           {{ GRAPH {graph} {{ ?s ?p ?o }} }}\n\
           UNION {{ GRAPH <{SOURCES_GRAPH}> {{ VALUES ?s {{ {entity} {activity} }} ?s ?p ?o }} }}\n\
         }}",
        pfx = prefixes(),
    );

    let ds = super::model::DS;
    let mut out = format!(
        "@prefix csvw: <{CSVW}> .\n@prefix void: <{VOID}> .\n@prefix {PROF_LABEL}: <{PROF}> .\n\
         @prefix prov: <{PROV}> .\n@prefix dct:  <{DCT}> .\n@prefix ds:   <{ds}> .\n\n"
    );
    if let Ok(QueryResults::Graph(triples)) = store.query(&query) {
        for triple in triples.flatten() {
            // `Triple`'s Display is N-Triples, which is a subset of Turtle.
            out.push_str(&triple.to_string());
            out.push('\n');
        }
    }
    out
}

// ─────────────────────────────── HTTP ───────────────────────────────

type ApiResult<T> = Result<T, (StatusCode, String)>;

fn not_found(kind: &str, id: &str) -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, format!("{kind} '{id}' not found"))
}
fn internal(message: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, message.to_string())
}

/// What a caller may narrow a profiling run to.
/// A misspelled field is refused rather than defaulted: `{"table": [...]}`
/// silently defaulting to the empty list would profile the whole replica,
/// which is the one thing a narrowing request must never turn into.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileRequest {
    /// Tables and views to profile. Omitted or empty profiles everything the
    /// catalogue lists — narrow it when a replica holds one table nobody
    /// wants scanned today.
    #[serde(default)]
    pub tables: Vec<String>,
}

impl ProfileRequest {
    /// Parse a request body: absent means "everything", present must parse.
    ///
    /// Explicit because `Option<Json<T>>` turns *every* rejection — bad JSON,
    /// the wrong shape, the wrong content type — into `None`, and `None` here
    /// means "profile every table". A body the server could not read would
    /// then silently widen the run to the whole replica, on the one endpoint
    /// whose documented mitigation for a large replica is narrowing.
    fn parse(body: &[u8]) -> Result<Self, (StatusCode, String)> {
        if body.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self::default());
        }
        let bad = |message: String| (StatusCode::BAD_REQUEST, message);
        // Via `Value`, because serde also accepts a struct written as a JSON
        // array, and `[]` would then read as "profile everything".
        let value: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| bad(format!("the profile request body is not JSON: {e}")))?;
        if !value.is_object() {
            return Err(bad(
                "the profile request body is a JSON object, optionally with 'tables'".to_string(),
            ));
        }
        serde_json::from_value(value).map_err(|e| {
            bad(format!(
                "the profile request body is not a ProfileRequest: {e}"
            ))
        })
    }
}

/// A profiling run's answer. Counts and hashes only: the values live in the
/// profile graph, where the code-list rule applies to them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileResponse {
    pub source: String,
    pub version: u32,
    pub graph: String,
    pub activity: String,
    pub started_at: String,
    pub duration_ms: u64,
    /// The version a drift diff should compare against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_version: Option<u32>,
    pub tables: Vec<TableSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableSummary {
    pub table: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<u64>,
    pub columns: usize,
    /// Columns whose values are a code list, and therefore carry their values.
    pub code_lists: usize,
    pub structural_hash: String,
}

impl TableSummary {
    fn of(profile: &TableProfile) -> Self {
        TableSummary {
            table: profile.table.clone(),
            kind: match profile.kind {
                TableKind::Table => "table",
                TableKind::View => "view",
            },
            rows: profile.row_count,
            columns: profile.columns.len(),
            code_lists: profile
                .columns
                .iter()
                .filter(|c| !c.top_values.is_empty())
                .count(),
            structural_hash: structural_hash(profile),
        }
    }
}

/// Why profiling did not happen. Separated so a table the caller misspelled
/// is a 400 and a database that would not answer is a 502.
enum ProfileFailure {
    UnknownTable(String),
    Source(String),
}

/// `POST /api/sources/:id/profile` — re-profile into a new version.
pub async fn create_profile(
    Extension(user): Extension<AuthenticatedUser>,
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Response> {
    // Read the narrowing before queueing: a request the server cannot read is
    // refused now rather than after waiting for the expensive slot.
    let wanted = ProfileRequest::parse(&body)?.tables;

    // Profiling aggregates over whole tables; bound it with the other
    // expensive operations rather than letting N of them meet on one replica.
    let _permit = state.expensive_semaphore.acquire().await.map_err(|_| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Server overloaded".to_string(),
        )
    })?;

    let source =
        registry::get_source(&state.store, &id).ok_or_else(|| not_found("datasource", &id))?;

    let started_at = registry::now();
    let clock = Instant::now();
    let probe = source.clone();
    let profiles =
        tokio::task::spawn_blocking(move || {
            let mut conn = runs::connect(&probe).map_err(ProfileFailure::Source)?;
            // Names only: full introspection structures *and* counts the rows
            // of every table in the database, including the ones this run
            // narrows away, and each profiled table then counts them again.
            let catalogue: Vec<String> = conn
                .table_names()
                .map_err(|e| ProfileFailure::Source(runs::scrub(&e.to_string(), &probe, None)))?;
            let tables = if wanted.is_empty() {
                catalogue
            } else {
                for t in &wanted {
                    if !catalogue.contains(t) {
                        return Err(ProfileFailure::UnknownTable(t.clone()));
                    }
                }
                wanted
            };
            let mut out = Vec::with_capacity(tables.len());
            for t in tables {
                out.push(conn.profile(&t).map_err(|e| {
                    ProfileFailure::Source(runs::scrub(&e.to_string(), &probe, None))
                })?);
            }
            Ok::<_, ProfileFailure>(out)
        })
        .await
        .map_err(internal)?
        .map_err(|e| match e {
            ProfileFailure::UnknownTable(t) => (
                StatusCode::BAD_REQUEST,
                format!("datasource '{id}' has no table or view named '{t}'"),
            ),
            ProfileFailure::Source(m) => (StatusCode::BAD_GATEWAY, m),
        })?;

    let duration_ms = clock.elapsed().as_millis() as u64;
    let ended_at = registry::now();
    let previous_version = latest_version(&state.store, &id);
    let version = previous_version.unwrap_or(0) + 1;

    write_profile(&state.store, &id, version, &profiles).map_err(internal)?;
    let actor = format!(
        "{}/users/{}",
        state.base_url.trim_end_matches('/'),
        user.user_id
    );
    record_profiling(
        &state.store,
        &id,
        &ProfilingRun {
            version,
            actor: Some(actor),
            started_at: started_at.clone(),
            ended_at,
            duration_ms,
            tables: profiles.len(),
        },
    )
    .map_err(internal)?;

    let graph = profile_version_graph_iri(&id, version);
    crate::commit_log::record(
        &state.store,
        &state.base_url,
        crate::commit_log::CommitKind::Source,
        format!("datasource '{id}' profiled, version {version}"),
        Some(&user.user_id),
        Some(source.iri()),
        vec![graph.clone()],
        0,
        0,
        Some(version.to_string()),
    );

    Ok((
        StatusCode::CREATED,
        Json(ProfileResponse {
            source: source.iri(),
            version,
            graph,
            activity: profile_activity_iri(&id, version),
            started_at,
            duration_ms,
            previous_version,
            tables: profiles.iter().map(TableSummary::of).collect(),
        }),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct ProfileVersionParam {
    pub version: Option<u32>,
}

/// `GET /api/sources/:id/profile` — the newest profile, or `?version=n`.
///
/// Served here rather than through `POST /sparql` because a profile graph
/// belongs to no dataset and is therefore outside a caller's query scope.
pub async fn get_profile(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<ProfileVersionParam>,
) -> ApiResult<Response> {
    if !valid_id(&id) || registry::get_source(&state.store, &id).is_none() {
        return Err(not_found("datasource", &id));
    }
    let latest = latest_version(&state.store, &id).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            format!("datasource '{id}' has not been profiled; POST to this path to profile it"),
        )
    })?;
    let version = params.version.unwrap_or(latest);
    if version == 0 || version > latest {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("datasource '{id}' has profile versions 1..{latest}"),
        ));
    }
    let turtle = profile_turtle(&state.store, &id, version);
    Ok((StatusCode::OK, [(CONTENT_TYPE, "text/turtle")], turtle).into_response())
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/api/sources/:id/profile",
        get(get_profile).post(create_profile),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ots_plugin_api::sources::{ForeignKey, NumericSummary, ValueCount};

    fn column(name: &str, position: u32, native: &str, kind: ValueKind) -> ColumnProfile {
        ColumnProfile {
            name: name.into(),
            position,
            native_type: native.into(),
            generic_type: kind,
            nullable: true,
            primary_key: false,
            distinct_count: None,
            null_count: None,
            cardinality_ratio: None,
            top_values: Vec::new(),
            numeric: None,
            mean_length: None,
            pattern: None,
            pattern_confidence: None,
        }
    }

    fn sample_profile() -> TableProfile {
        let mut key = column("k", 1, "INTEGER", ValueKind::Integer);
        key.nullable = false;
        key.primary_key = true;
        key.distinct_count = Some(4);
        key.null_count = Some(0);
        key.cardinality_ratio = Some(1.0);

        let mut code = column("state", 2, "TEXT", ValueKind::Text);
        code.distinct_count = Some(2);
        code.null_count = Some(1);
        code.cardinality_ratio = Some(0.5);
        code.mean_length = Some(4.5);
        code.pattern = Some(DetectedPattern::Code);
        code.pattern_confidence = Some(1.0);
        code.top_values = vec![
            ValueCount {
                value: "alpha".into(),
                count: 2,
            },
            ValueCount {
                value: "beta".into(),
                count: 1,
            },
        ];

        let mut amount = column("amount", 3, "REAL", ValueKind::Float);
        amount.distinct_count = Some(3);
        amount.null_count = Some(1);
        amount.numeric = Some(NumericSummary {
            min: 0.25,
            max: 4.0,
            mean: 1.5,
            p50: 1.0,
            p99: 4.0,
        });

        TableProfile {
            table: "entry".into(),
            kind: TableKind::Table,
            row_count: Some(4),
            columns: vec![key, code, amount],
            primary_key: vec!["k".into()],
            unique_keys: vec![vec!["k".into()]],
            foreign_keys: vec![ForeignKey {
                columns: vec!["lookup_id".into()],
                ref_table: "lookup".into(),
                ref_columns: vec!["lookup_id".into()],
            }],
            sampled_rows: 4,
        }
    }

    #[test]
    fn the_structural_hash_ignores_everything_that_is_not_structure() {
        let base = sample_profile();
        let h = structural_hash(&base);
        assert_eq!(h.len(), 64, "a full SHA-256 in hex");

        // Rows, counts, distributions and the sample all move under it.
        let mut churned = base.clone();
        churned.row_count = Some(1_000_000);
        churned.sampled_rows = 200;
        churned.columns[1].null_count = Some(99);
        churned.columns[1].distinct_count = Some(7);
        churned.columns[1].top_values.clear();
        churned.columns[2].numeric = None;
        assert_eq!(structural_hash(&churned), h, "statistics are not structure");

        // Structure does not.
        for mutate in [
            (|p: &mut TableProfile| p.table = "other".into()) as fn(&mut TableProfile),
            |p| p.columns.push(column("added", 4, "TEXT", ValueKind::Text)),
            |p| p.columns[1].name = "renamed".into(),
            |p| p.columns[1].native_type = "VARCHAR(8)".into(),
            |p| p.columns[1].generic_type = ValueKind::Json,
            |p| p.columns[1].nullable = false,
            |p| p.primary_key = vec!["state".into()],
            |p| p.unique_keys.push(vec!["state".into()]),
            |p| p.foreign_keys.clear(),
        ] {
            let mut changed = base.clone();
            mutate(&mut changed);
            assert_ne!(structural_hash(&changed), h, "{changed:?}");
        }
    }

    #[test]
    fn the_hash_cannot_be_forged_by_a_name_containing_a_separator() {
        let mut a = sample_profile();
        a.columns[1].name = "x\ty".into();
        let mut b = sample_profile();
        b.columns[1].name = "x".into();
        b.columns[1].native_type = "y\tTEXT".into();
        assert_ne!(structural_hash(&a), structural_hash(&b));
    }

    #[test]
    fn a_key_column_order_is_part_of_the_structure() {
        let mut a = sample_profile();
        a.primary_key = vec!["k".into(), "state".into()];
        let mut b = sample_profile();
        b.primary_key = vec!["state".into(), "k".into()];
        assert_ne!(
            structural_hash(&a),
            structural_hash(&b),
            "a composite key is ordered"
        );
    }

    #[test]
    fn patterns_are_detected_by_shape_and_only_by_shape() {
        use DetectedPattern::*;
        for (value, expected) in [
            ("a.b@c.example", Some(Email)),
            ("a@b", None),
            ("a@b.c", None),
            ("http://example.org/a#b", Some(Iri)),
            ("urn:x:1", Some(Iri)),
            ("mailto:a@b.example", Some(Iri)),
            ("f81d4fae-7dec-11d0-a765-00a0c91e6bf6", Some(Uuid)),
            // An unhyphenated UUID is a token, and `Code` is what "this is a
            // token" looks like: the heuristic reports shape, not meaning.
            ("f81d4fae7dec11d0a76500a0c91e6bf6", Some(Code)),
            ("2026-09-17", Some(Date)),
            ("2026-09-17T08:30:00Z", Some(Date)),
            ("17/09/2026", None),
            ("+31 20 123 4567", Some(Phone)),
            ("020-1234567", Some(Phone)),
            ("0201234567", None),
            ("ACTIVE", Some(Code)),
            ("a-1.b_2", Some(Code)),
            ("the quick brown fox", None),
            ("", None),
            ("   ", None),
        ] {
            assert_eq!(DetectedPattern::detect(value), expected, "{value:?}");
        }
    }

    #[test]
    fn a_profile_version_is_rdf_that_reuses_csvw_and_void() {
        let store = TripleStore::in_memory().unwrap();
        write_profile(&store, "s", 1, &[sample_profile()]).unwrap();

        let graph = profile_version_graph_iri("s", 1);
        let ask = |pattern: &str| {
            let q = format!(
                "{pfx}ASK {{ GRAPH <{graph}> {{ {pattern} }} }}",
                pfx = prefixes()
            );
            matches!(store.query(&q), Ok(QueryResults::Boolean(true)))
        };
        assert!(ask("<urn:source:s:table:entry> a csvw:Table, void:Dataset"));
        assert!(ask("<urn:source:s:table:entry> void:entities 4"));
        assert!(ask(
            "<urn:source:s:table:entry> csvw:tableSchema/csvw:primaryKey \"k\""
        ));
        assert!(ask(
            "<urn:source:s:table:entry:column:state> a csvw:Column ; csvw:name \"state\" ; \
             csvw:number 2 ; csvw:required false ; dsprof:nullCount 1"
        ));
        assert!(ask(
            "<urn:source:s:table:entry:column:k> csvw:required true ; csvw:datatype xsd:integer"
        ));
        assert!(ask(
            "<urn:source:s:table:entry:column:state> dsprof:topValue ?v . \
             ?v rdf:value \"alpha\" ; dsprof:occurrences 2 ; dsprof:rank 1"
        ));
        assert!(ask(
            "<urn:source:s:table:entry:column:amount> dsprof:min 0.25 ; dsprof:p99 4.0"
        ));
        assert!(ask(
            "<urn:source:s:table:entry:schema> csvw:foreignKey/csvw:reference/csvw:resource \
             <urn:source:s:table:lookup>"
        ));
        // A free-text-shaped column gets no numeric summary at any point.
        assert!(!ask(
            "<urn:source:s:table:entry:column:state> dsprof:min ?m"
        ));
        // Nothing that changes on its own lives in the profile graph.
        assert!(!ask("?s dct:created ?c"));
        assert!(!ask("?s a prov:Activity"));
    }

    #[test]
    fn re_profiling_writes_a_new_version_and_leaves_the_old_one_intact() {
        let store = TripleStore::in_memory().unwrap();
        let mut first = sample_profile();
        first.row_count = Some(4);
        write_profile(&store, "s", 1, &[first]).unwrap();
        record_profiling(
            &store,
            "s",
            &ProfilingRun {
                version: 1,
                started_at: "2026-09-17T08:00:00Z".into(),
                ended_at: "2026-09-17T08:00:01Z".into(),
                tables: 1,
                ..ProfilingRun::default()
            },
        )
        .unwrap();

        let mut second = sample_profile();
        second.row_count = Some(9);
        write_profile(&store, "s", 2, &[second]).unwrap();
        record_profiling(
            &store,
            "s",
            &ProfilingRun {
                version: 2,
                started_at: "2026-09-17T09:00:00Z".into(),
                ended_at: "2026-09-17T09:00:01Z".into(),
                tables: 1,
                ..ProfilingRun::default()
            },
        )
        .unwrap();

        assert_eq!(versions(&store, "s"), vec![1, 2]);
        assert_eq!(latest_version(&store, "s"), Some(2));
        assert!(latest_version(&store, "other").is_none());

        // Both versions are readable, and the drift between them is exactly
        // the triple delta the store already computes across two graphs.
        let v1 = profile_version_graph_iri("s", 1);
        let v2 = profile_version_graph_iri("s", 2);
        assert!(store.count_graph(Some(&v1)).unwrap() > 0, "v1 still there");
        let (ahead, behind) = crate::data_models::diff::triple_delta(&store, &[v1], &[v2]);
        assert_eq!(
            (ahead, behind),
            (1, 1),
            "stable IRIs mean one changed statistic is one moved triple"
        );
        let ask = |graph: &str, rows: u64| {
            let q = format!(
                "{pfx}ASK {{ GRAPH <{graph}> {{ <urn:source:s:table:entry> void:entities {rows} }} }}",
                pfx = prefixes()
            );
            matches!(store.query(&q), Ok(QueryResults::Boolean(true)))
        };
        assert!(ask(&profile_version_graph_iri("s", 1), 4));
        assert!(ask(&profile_version_graph_iri("s", 2), 9));
    }

    #[test]
    fn a_profiling_run_is_a_prov_activity_on_the_source() {
        let store = TripleStore::in_memory().unwrap();
        record_profiling(
            &store,
            "s",
            &ProfilingRun {
                version: 1,
                actor: Some("http://x/users/adm".into()),
                started_at: "2026-09-17T08:00:00Z".into(),
                ended_at: "2026-09-17T08:00:02Z".into(),
                duration_ms: 2_000,
                tables: 3,
            },
        )
        .unwrap();
        let q = format!(
            "{pfx}ASK {{ GRAPH <{SOURCES_GRAPH}> {{ \
               <urn:source:s:profile:version:1:activity> a prov:Activity, dsprof:Profiling ; \
                 ds:source <urn:source:s> ; prov:used <urn:source:s> ; \
                 prov:generated <urn:source:s:profile:version:1> ; \
                 prov:wasAssociatedWith <http://x/users/adm> ; \
                 prov:startedAtTime ?st ; prov:endedAtTime ?et ; ds:durationMs 2000 . \
               <urn:source:s:profile:version:1> a dsprof:SourceProfile ; ds:version 1 . }} }}",
            pfx = prefixes()
        );
        assert!(matches!(store.query(&q), Ok(QueryResults::Boolean(true))));
        // The PROV lives in the system graph, never in the profile graph.
        assert_eq!(
            store
                .count_graph(Some(&profile_version_graph_iri("s", 1)))
                .unwrap(),
            0
        );
    }

    #[test]
    fn the_writer_drops_a_value_list_no_driver_should_have_offered() {
        // Defence in depth: the driver that reads the values already applies the
        // ceiling, but a driver added as a plugin lives outside this crate. The
        // one place row content can reach the graph enforces it too.
        let mut p = sample_profile();
        p.columns[1].top_values = vec![
            ValueCount {
                value: "short".to_string(),
                count: 10,
            },
            ValueCount {
                value: "x".repeat(LOW_CARDINALITY_MAX_VALUE_LEN + 1),
                count: 5,
            },
        ];
        let out = column_triples("s", "t", &p.columns[1]);
        assert!(
            !out.contains("dsprof:topValue"),
            "one over-long value takes the whole list with it: {out}"
        );
        assert!(
            !out.contains("xxxxx"),
            "and the value itself never lands: {out}"
        );
        // A list entirely within the ceiling is still written.
        p.columns[1].top_values.pop();
        let ok = column_triples("s", "t", &p.columns[1]);
        assert!(
            ok.contains("dsprof:topValue") && ok.contains("short"),
            "{ok}"
        );
    }

    #[test]
    fn names_that_would_break_an_iri_or_a_literal_are_escaped() {
        let store = TripleStore::in_memory().unwrap();
        let mut p = sample_profile();
        p.table = "odd name/with space".into();
        p.columns[1].name = "co\"l }".into();
        p.columns[1].top_values = vec![ValueCount {
            value: "quote \" brace } newline \n".into(),
            count: 2,
        }];
        write_profile(&store, "s", 1, &[p.clone()]).unwrap();

        let graph = profile_version_graph_iri("s", 1);
        assert!(store.count_graph(Some(&graph)).unwrap() > 0);
        let q = format!(
            "{pfx}SELECT ?n WHERE {{ GRAPH <{graph}> {{ <{}> csvw:name ?n }} }}",
            column_iri("s", &p.table, &p.columns[1].name),
            pfx = prefixes()
        );
        let Ok(QueryResults::Solutions(mut rows)) = store.query(&q) else {
            panic!("hostile names must still be queryable")
        };
        let row = rows.next().expect("the column survived").unwrap();
        assert_eq!(
            row.get("n").map(|t| t.to_string()),
            Some("\"co\\\"l }\"".to_string())
        );
    }

    #[test]
    fn iri_segments_cannot_collide() {
        assert_eq!(segment("a b"), "a%20b");
        assert_eq!(segment("a%20b"), "a%2520b");
        assert_ne!(segment("a b"), segment("a%20b"));
        assert_ne!(
            table_iri("s", "a:b"),
            table_iri("s", "a"),
            "a colon in a name cannot reach into the IRI's structure"
        );
    }

    #[test]
    fn deleting_a_source_s_profiles_leaves_nothing_a_later_source_can_inherit() {
        let store = TripleStore::in_memory().unwrap();
        for (id, version) in [("s", 1), ("s", 2), ("other", 1)] {
            write_profile(&store, id, version, &[sample_profile()]).unwrap();
            record_profiling(
                &store,
                id,
                &ProfilingRun {
                    version,
                    started_at: "2026-09-17T08:00:00Z".into(),
                    ended_at: "2026-09-17T08:00:01Z".into(),
                    tables: 1,
                    ..ProfilingRun::default()
                },
            )
            .unwrap();
        }
        assert_eq!(versions(&store, "s"), vec![1, 2]);

        delete_profiles(&store, "s").unwrap();

        // The next profile of a re-registered id starts over at version 1.
        assert!(versions(&store, "s").is_empty());
        assert_eq!(latest_version(&store, "s"), None);
        for version in [1, 2] {
            assert_eq!(
                store
                    .count_graph(Some(&profile_version_graph_iri("s", version)))
                    .unwrap(),
                0,
                "version {version}'s graph outlived its datasource"
            );
            assert_eq!(
                profile_turtle(&store, "s", version)
                    .lines()
                    .filter(|l| l.starts_with('<'))
                    .count(),
                0,
                "version {version} is still served"
            );
        }
        // …including the activity, the series and everything hanging off them.
        let ask = |pattern: &str| {
            let q = format!(
                "{pfx}ASK {{ GRAPH <{SOURCES_GRAPH}> {{ {pattern} }} }}",
                pfx = prefixes()
            );
            matches!(store.query(&q), Ok(QueryResults::Boolean(true)))
        };
        assert!(!ask("?s a dsprof:Profiling ; ds:source <urn:source:s>"));
        assert!(!ask("<urn:source:s:profile> ?p ?o"));
        assert!(!ask("<urn:source:s:profile:version:1:activity> ?p ?o"));

        // Deleting a datasource that was never profiled is not an error.
        delete_profiles(&store, "never-profiled").unwrap();

        // Another datasource's profiles are untouched.
        assert_eq!(versions(&store, "other"), vec![1]);
        assert!(
            store
                .count_graph(Some(&profile_version_graph_iri("other", 1)))
                .unwrap()
                > 0
        );
    }

    #[test]
    fn the_profile_label_is_not_one_the_store_already_binds() {
        let bundled = crate::prefixes::dataset::PrefixDataset::bundled();
        assert_eq!(
            bundled.lookup("prof").map(|e| e.namespace.as_str()),
            Some("http://www.w3.org/ns/dx/prof/"),
            "the collision this label avoids"
        );
        assert!(
            bundled.lookup(PROF_LABEL).is_none(),
            "{PROF_LABEL} is bound to something else"
        );
        assert!(prefixes().contains(&format!("PREFIX {PROF_LABEL}: <{PROF}>")));

        // Every term written into a profile graph uses it, and no other label
        // expands to this vocabulary.
        let body = format!(
            "{}{}",
            table_triples("s", &sample_profile()),
            prefixes().replace(&format!("PREFIX {PROF_LABEL}:"), "")
        );
        for (i, _) in body.match_indices("prof:") {
            assert!(
                body[..i].ends_with("ds"),
                "a bare 'prof:' term at byte {i}: {body}"
            );
        }
    }

    #[test]
    fn a_body_the_server_cannot_read_never_means_profile_everything() {
        // Absent, and blank, are the documented "everything".
        assert!(ProfileRequest::parse(b"").unwrap().tables.is_empty());
        assert!(ProfileRequest::parse(b" \n").unwrap().tables.is_empty());
        assert!(ProfileRequest::parse(b"{}").unwrap().tables.is_empty());
        assert_eq!(
            ProfileRequest::parse(br#"{"tables":["entry"]}"#)
                .unwrap()
                .tables,
            vec!["entry".to_string()]
        );
        // Anything else is refused, rather than widened to the whole replica.
        for body in [
            &br#"{"tables":"entry"}"#[..],
            b"{not json",
            b"[]",
            b"[\"entry\"]",
            b"null",
            b"{\"tables\":[1]}",
            // A misspelled field would otherwise default to "everything".
            &br#"{"table":["entry"]}"#[..],
        ] {
            let (status, _) = ProfileRequest::parse(body)
                .expect_err(&format!("{} parsed", String::from_utf8_lossy(body)));
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn a_decimal_is_written_only_when_it_is_one() {
        assert_eq!(decimal(0.25).as_deref(), Some("\"0.25\"^^xsd:decimal"));
        assert_eq!(decimal(3.0).as_deref(), Some("\"3.0\"^^xsd:decimal"));
        assert_eq!(decimal(-0.5).as_deref(), Some("\"-0.5\"^^xsd:decimal"));
        assert_eq!(decimal(f64::NAN), None);
        assert_eq!(decimal(f64::INFINITY), None);
    }
}
