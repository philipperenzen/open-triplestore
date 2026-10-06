//! The LDES client: `POST /api/ldes/sync` replicates a remote event stream
//! into a graph of a local dataset, following the LDES 1.0 consumer
//! specification in unordered mode:
//!
//! * §3.1 initialisation: the IRI given is the stream, its root node, a
//!   redirect to either, or a page with exactly one `tree:view`; context comes
//!   from the root node.
//! * §3.3 HTTP: every fetch goes through `crate::remote` (allowlist re-checked
//!   on every redirect, timeout, retries with back-off on 408/425/429/5xx,
//!   `If-None-Match`); `410 Gone` is an empty page.
//! * §3.4 member extraction: `<stream> tree:member ?m`, the star pattern of
//!   `m` in the default graph and every quad in the named graph `m`, blank
//!   nodes followed once.
//! * §3.2 state: a bookmark on the timestamp (as `xsd:dateTime` values, with
//!   the members that carry the bookmark's own timestamp), pages known to be
//!   immutable (never fetched again), ETags and relations of mutable pages.
//! * §4.3 versions: the declared `versionOfPath`, version timestamp and
//!   sequence paths and the create / update / delete markers; paths are SHACL
//!   property paths, parsed by the SHACL engine's parser.
//!
//! The newest version per entity is materialised into the target graph when
//! the run has finished (the run's end is its finalisation, §3).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use chrono::{DateTime, FixedOffset};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{
    BlankNode, GraphName, GraphNameRef, NamedNode, NamedOrBlankNode, Quad, Term, Triple,
};
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use serde::{Deserialize, Serialize};

use super::store::{SyncEntity, SyncPage, SyncRun, SyncState};
use super::{LDES, TOMBSTONE, TREE, XSD};
use crate::auth::middleware::AuthenticatedUser;
use crate::remote::RemoteError;
use crate::server::content_negotiation::parse_rdf_content_type;
use crate::server::AppState;
use crate::shacl::shapes::{parse_property_path_with, PropertyPath};

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// The most nodes one run follows.
const MAX_NODES: usize = 10_000;

/// How deep a chain of blank nodes under an entity is removed when a newer
/// version replaces it.
const BLANK_NODE_DEPTH: usize = 6;

#[derive(Debug, Deserialize)]
pub struct SyncBody {
    pub url: String,
    pub dataset_id: String,
    pub graph_iri: String,
}

#[derive(Debug, Default, Serialize)]
pub struct SyncReport {
    pub url: String,
    pub dataset_id: String,
    pub graph_iri: String,
    /// The `ldes:EventStream` and its root node, as initialisation found them
    /// (LDES §3.1); `None` when the entry page was gone.
    pub stream: Option<String>,
    pub root_node: Option<String>,
    /// `ldes:pollingInterval` in seconds: how long to wait before the next
    /// run (LDES §3: the client SHOULD use it between runs).
    pub polling_interval: Option<u64>,
    /// The stream's `tree:shape` IRIs (context, LDES §4.2).
    pub shapes: Vec<String>,
    pub nodes_visited: usize,
    /// Nodes that answered `410 Gone` — compacted away by the publisher's
    /// retention policy and processed as empty (LDES §3.3).
    pub nodes_gone: usize,
    /// Mutable nodes that answered `304 Not Modified` to the ETag kept from
    /// the previous run; their relations were followed from the state.
    pub nodes_not_modified: usize,
    /// Immutable nodes an earlier run processed: not fetched again (§3.2).
    pub nodes_skipped_immutable: usize,
    /// Nodes not fetched because every relation to them bounds their members
    /// below the bookmark (§3.2 MAY).
    pub nodes_pruned: usize,
    /// Requests retried after a 408/425/429/5xx (§3.3).
    pub retries: u32,
    pub members_seen: usize,
    /// Members an earlier run already emitted (behind or at the bookmark).
    pub members_skipped_older: usize,
    /// Members not applied because a newer version of the same entity was
    /// seen in this run or applied by an earlier one (§4.3).
    pub versions_superseded: usize,
    pub entities_updated: usize,
    pub entities_deleted: usize,
    pub last_timestamp: Option<String>,
    /// The retention policy the publisher declares on its root node, kept as
    /// context (LDES §3.2); `None` when the stream keeps every member.
    pub retention_policy: Option<RemoteRetention>,
    /// Anything the client noticed that the caller should know — for now,
    /// a bookmark older than the publisher's retention window, which means
    /// members may have been compacted before this mirror saw them.
    pub warnings: Vec<String>,
}

/// A publisher's retention policy as read from its root node — the LDES 1.0
/// properties, or the discouraged-but-supported legacy classes
/// (`ldes:DurationAgoPolicy`, `ldes:LatestVersionSubset`,
/// `ldes:PointInTimePolicy`) folded into the same fields.
#[derive(Debug, Default, Clone, Serialize)]
pub struct RemoteRetention {
    pub policy: String,
    pub full_log_duration: Option<String>,
    pub version_amount: Option<u64>,
    pub version_duration: Option<String>,
    pub version_delete_duration: Option<String>,
    pub starting_from: Option<String>,
    /// The policy used the pre-1.0 classes.
    pub legacy: bool,
}

/// The retention policy declared in a document, if any: on the root node
/// directly or through `tree:viewDescription` — "the client MUST look for a
/// retention policy in both ways" (LDES §4.4).
fn retention_of(store: &Store) -> Option<RemoteRetention> {
    let q = format!(
        "SELECT ?p ?fl ?va ?vd ?vdd ?sf ?dur ?amt ?pit WHERE {{ \
           {{ ?x <{LDES}retentionPolicy> ?p }} UNION {{ ?v <{TREE}viewDescription>/<{LDES}retentionPolicy> ?p }} \
           OPTIONAL {{ ?p <{LDES}fullLogDuration> ?fl }} \
           OPTIONAL {{ ?p <{LDES}versionAmount> ?va }} \
           OPTIONAL {{ ?p <{LDES}versionDuration> ?vd }} \
           OPTIONAL {{ ?p <{LDES}versionDeleteDuration> ?vdd }} \
           OPTIONAL {{ ?p <{LDES}startingFrom> ?sf }} \
           OPTIONAL {{ ?p a <{LDES}DurationAgoPolicy> ; <{TREE}value> ?dur }} \
           OPTIONAL {{ ?p a <{LDES}LatestVersionSubset> ; <{LDES}amount> ?amt }} \
           OPTIONAL {{ ?p a <{LDES}PointInTimePolicy> ; <{LDES}pointInTime> ?pit }} \
         }}"
    );
    let QueryResults::Solutions(sols) = run(store, &q)? else {
        return None;
    };
    let mut out: Option<RemoteRetention> = None;
    let lit = |t: Option<&Term>| match t {
        Some(Term::Literal(l)) => Some(l.value().to_string()),
        _ => None,
    };
    for s in sols.flatten() {
        let r = out.get_or_insert_with(|| RemoteRetention {
            policy: s.get("p").map(|t| t.to_string()).unwrap_or_default(),
            ..Default::default()
        });
        r.full_log_duration = r.full_log_duration.take().or(lit(s.get("fl")));
        r.version_amount = r
            .version_amount
            .or(lit(s.get("va")).and_then(|v| v.parse().ok()));
        r.version_duration = r.version_duration.take().or(lit(s.get("vd")));
        r.version_delete_duration = r.version_delete_duration.take().or(lit(s.get("vdd")));
        r.starting_from = r.starting_from.take().or(lit(s.get("sf")));
        if let Some(d) = lit(s.get("dur")) {
            r.legacy = true;
            r.full_log_duration.get_or_insert(d);
        }
        if let Some(a) = lit(s.get("amt")).and_then(|v| v.parse().ok()) {
            r.legacy = true;
            r.version_amount.get_or_insert(a);
        }
        // LDES §4.4: "data generated before a specific time is not retained"
        // — what ldes:startingFrom says in the 1.0 vocabulary.
        if let Some(t) = lit(s.get("pit")) {
            r.legacy = true;
            r.starting_from.get_or_insert(t);
        }
    }
    out
}

/// Run a SELECT against a fetched document.
fn run<'a>(store: &'a Store, query: &str) -> Option<QueryResults<'a>> {
    SparqlEvaluator::new()
        .parse_query(query)
        .ok()?
        .on_store(store)
        .execute()
        .ok()
}

// ─── Terms and SHACL paths over a fetched document ──────────────────────────

/// A term in the lexical form the SHACL path parser works on: an IRI,
/// `_:label` for a blank node, a literal's value.
fn lexical(t: &Term) -> String {
    match t {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        Term::Literal(l) => l.value().to_string(),
        #[allow(unreachable_patterns)]
        other => other.to_string(),
    }
}

fn subject_of(lexical: &str) -> Option<NamedOrBlankNode> {
    match lexical.strip_prefix("_:") {
        Some(label) => BlankNode::new(label).ok().map(NamedOrBlankNode::from),
        None => NamedNode::new(lexical).ok().map(NamedOrBlankNode::from),
    }
}

/// The objects of `subject predicate ?o` in any graph of the document.
fn objects(store: &Store, subject: &str, predicate: &str) -> Vec<Term> {
    let (Some(s), Ok(p)) = (subject_of(subject), NamedNode::new(predicate)) else {
        return Vec::new();
    };
    store
        .quads_for_pattern(Some(s.as_ref()), Some(p.as_ref()), None, None)
        .flatten()
        .map(|q| q.object)
        .collect()
}

/// The subjects of `?s predicate object` in any graph of the document.
fn subjects(store: &Store, predicate: &str, object: &str) -> Vec<String> {
    let (Ok(p), Ok(o)) = (NamedNode::new(predicate), NamedNode::new(object)) else {
        return Vec::new();
    };
    let mut out: Vec<String> = store
        .quads_for_pattern(None, Some(p.as_ref()), Some(o.as_ref().into()), None)
        .flatten()
        .map(|q| lexical(&q.subject.into()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The SHACL property path (SHACL §2.3) that `node` denotes in the document.
fn path_at(store: &Store, node: &Term) -> Option<PropertyPath> {
    parse_property_path_with(&lexical(node), &|s, p| {
        objects(store, s, p).iter().map(lexical).collect()
    })
}

fn same_path(a: &PropertyPath, b: &PropertyPath) -> bool {
    a.to_sparql() == b.to_sparql()
}

/// An `xsd:dateTime` value. A timestamp without a timezone is read as UTC.
fn parse_datetime(lexical: &str) -> Option<DateTime<FixedOffset>> {
    let s = lexical.trim();
    DateTime::parse_from_rfc3339(s).ok().or_else(|| {
        chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
            .ok()
            .map(|n| n.and_utc().fixed_offset())
    })
}

// ─── Context (LDES §4) ──────────────────────────────────────────────────────

/// A version marker: the member is a create, update or delete when `path`
/// reaches `object` from it (LDES §4.3).
#[derive(Debug, Clone)]
struct Marker {
    path: PropertyPath,
    object: Term,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Create,
    Update,
    Delete,
    /// No marker matched: replace whatever the entity held.
    Upsert,
}

/// What the client keeps from the root node.
#[derive(Debug)]
struct Context {
    stream: String,
    timestamp: Option<PropertyPath>,
    sequence: Option<PropertyPath>,
    version_of: Option<PropertyPath>,
    version_timestamp: Option<PropertyPath>,
    version_sequence: Option<PropertyPath>,
    create: Option<Marker>,
    update: Option<Marker>,
    delete: Option<Marker>,
    polling_interval: Option<u64>,
    shapes: Vec<String>,
}

impl Context {
    /// Read the stream's declarations, from the first of `docs` (the root
    /// node first, then the page the client was given) that has them.
    fn read(stream: &str, docs: &[&Store]) -> Self {
        let first = |prop: &str| -> Option<(&Store, Term)> {
            docs.iter().find_map(|d| {
                objects(d, stream, &format!("{LDES}{prop}"))
                    .into_iter()
                    .next()
                    .map(|t| (*d, t))
            })
        };
        let path = |prop: &str| first(prop).and_then(|(d, t)| path_at(d, &t));
        // LDES §4.3: each marker path "defaults to rdf:type".
        let marker = |kind: &str| -> Option<Marker> {
            let (_, object) = first(&format!("version{kind}Object"))?;
            let path = path(&format!("version{kind}Path"))
                .unwrap_or_else(|| PropertyPath::Predicate(RDF_TYPE.to_string()));
            Some(Marker { path, object })
        };
        let shapes = docs
            .iter()
            .flat_map(|d| objects(d, stream, &format!("{TREE}shape")))
            .map(|t| lexical(&t))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        Context {
            stream: stream.to_string(),
            timestamp: path("timestampPath"),
            sequence: path("sequencePath"),
            version_of: path("versionOfPath"),
            version_timestamp: path("versionTimestampPath"),
            version_sequence: path("versionSequencePath"),
            create: marker("Create"),
            update: marker("Update"),
            delete: marker("Delete"),
            polling_interval: first("pollingInterval").and_then(|(_, t)| match t {
                Term::Literal(l) => l.value().trim().parse().ok(),
                _ => None,
            }),
            shapes,
        }
    }
}

// ─── Versions ───────────────────────────────────────────────────────────────

/// A sequence value (`ldes:sequencePath` / `ldes:versionSequencePath`),
/// compared like XPath's `lt`: numbers as numbers, date-times as instants,
/// anything else as strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Sequence {
    value: String,
    datatype: String,
}

impl Sequence {
    fn cmp(&self, other: &Self) -> Ordering {
        let num = |s: &Sequence| -> Option<f64> {
            let dt = s.datatype.strip_prefix(XSD)?;
            matches!(
                dt,
                "integer"
                    | "decimal"
                    | "double"
                    | "float"
                    | "long"
                    | "int"
                    | "short"
                    | "byte"
                    | "nonNegativeInteger"
                    | "positiveInteger"
                    | "unsignedLong"
                    | "unsignedInt"
            )
            .then(|| s.value.trim().parse().ok())
            .flatten()
        };
        if let (Some(a), Some(b)) = (num(self), num(other)) {
            return a.partial_cmp(&b).unwrap_or(Ordering::Equal);
        }
        if let (Some(a), Some(b)) = (parse_datetime(&self.value), parse_datetime(&other.value)) {
            return a.cmp(&b);
        }
        self.value.cmp(&other.value)
    }
}

/// What orders the versions of one entity (LDES §4.3): the version
/// timestamp (else the member timestamp), then the version sequence (else
/// the member sequence). Stored per entity as JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct VersionKey {
    time: Option<String>,
    seq: Option<Sequence>,
}

impl VersionKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let t = |k: &VersionKey| k.time.as_deref().and_then(parse_datetime);
        let by_time = match (t(self), t(other)) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => Ordering::Equal,
        };
        by_time.then_with(|| match (&self.seq, &other.seq) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => Ordering::Equal,
        })
    }
}

// ─── Members (LDES §3.4) ────────────────────────────────────────────────────

/// One member as read from a page.
struct RemoteMember {
    iri: String,
    entity: String,
    /// The member timestamp (`ldes:timestampPath`): what the bookmark uses.
    timestamp: Option<(DateTime<FixedOffset>, String)>,
    version: VersionKey,
    kind: Kind,
    /// The entity's description to materialise (N-Triples) and the IRI
    /// subjects it writes.
    ntriples: String,
    subjects: Vec<String>,
}

/// Every quad reachable from `seeds` the way LDES §3.4 extracts a member:
/// for each focus node its star pattern in the default graph and every quad
/// in the named graph it names; a blank-node object becomes a new focus
/// node, and no blank node is processed twice.
fn closure(store: &Store, seeds: Vec<NamedOrBlankNode>) -> Vec<Quad> {
    let mut out: Vec<Quad> = Vec::new();
    let mut seen_quads: HashSet<Quad> = HashSet::new();
    let mut seen: HashSet<NamedOrBlankNode> = seeds.iter().cloned().collect();
    let mut queue = seeds;
    while let Some(focus) = queue.pop() {
        let graph = match &focus {
            NamedOrBlankNode::NamedNode(n) => GraphNameRef::NamedNode(n.as_ref()),
            NamedOrBlankNode::BlankNode(b) => GraphNameRef::BlankNode(b.as_ref()),
        };
        let star = store.quads_for_pattern(
            Some(focus.as_ref()),
            None,
            None,
            Some(GraphNameRef::DefaultGraph),
        );
        let named = store.quads_for_pattern(None, None, None, Some(graph));
        for q in star.chain(named).flatten() {
            if let Term::BlankNode(b) = &q.object {
                let b = NamedOrBlankNode::BlankNode(b.clone());
                if seen.insert(b.clone()) {
                    queue.push(b);
                }
            }
            if seen_quads.insert(q.clone()) {
                out.push(q);
            }
        }
    }
    out
}

/// The values `path` reaches from `focus` over the member's quads (all graphs
/// merged): a single predicate directly, anything else with SPARQL over a
/// scratch store.
fn path_values(quads: &[Quad], focus: &NamedNode, path: &PropertyPath) -> Vec<Term> {
    if let PropertyPath::Predicate(p) = path {
        return quads
            .iter()
            .filter(|q| {
                q.predicate.as_str() == p
                    && matches!(&q.subject, NamedOrBlankNode::NamedNode(s) if s == focus)
            })
            .map(|q| q.object.clone())
            .collect();
    }
    let Ok(scratch) = Store::new() else {
        return Vec::new();
    };
    for q in quads {
        let _ = scratch.insert(&Quad::new(
            q.subject.clone(),
            q.predicate.clone(),
            q.object.clone(),
            GraphName::DefaultGraph,
        ));
    }
    let q = format!(
        "SELECT DISTINCT ?v WHERE {{ <{}> {} ?v }}",
        focus.as_str(),
        path.to_sparql()
    );
    let mut out = Vec::new();
    if let Some(QueryResults::Solutions(sols)) = run(&scratch, &q) {
        for s in sols.flatten() {
            if let Some(v) = s.get("v") {
                out.push(v.clone());
            }
        }
    }
    out
}

fn first_literal(values: &[Term]) -> Option<&oxigraph::model::Literal> {
    values.iter().find_map(|t| match t {
        Term::Literal(l) => Some(l),
        _ => None,
    })
}

fn datetime_at(
    quads: &[Quad],
    focus: &NamedNode,
    path: Option<&PropertyPath>,
) -> Option<(DateTime<FixedOffset>, String)> {
    let values = path_values(quads, focus, path?);
    values.iter().find_map(|t| match t {
        Term::Literal(l) => parse_datetime(l.value()).map(|d| (d, l.value().to_string())),
        _ => None,
    })
}

fn sequence_at(quads: &[Quad], focus: &NamedNode, path: Option<&PropertyPath>) -> Option<Sequence> {
    let values = path_values(quads, focus, path?);
    first_literal(&values).map(|l| Sequence {
        value: l.value().to_string(),
        datatype: l.datatype().as_str().to_string(),
    })
}

/// The members of a page: `<stream> tree:member ?m` (LDES §3.4), each with
/// its entity, timestamp, version key, kind and the description to write.
fn members_of(store: &Store, ctx: &Context) -> Vec<RemoteMember> {
    let mut out = Vec::new();
    let mut listed: HashSet<NamedNode> = HashSet::new();
    for t in objects(store, &ctx.stream, &format!("{TREE}member")) {
        // "the object of the tree:member triple can only be an IRI" (§2).
        let Term::NamedNode(m) = t else { continue };
        if !listed.insert(m.clone()) {
            continue;
        }
        out.push(member(store, ctx, m));
    }
    out
}

fn member(store: &Store, ctx: &Context, m: NamedNode) -> RemoteMember {
    let quads = closure(store, vec![NamedOrBlankNode::NamedNode(m.clone())]);
    let entity = ctx
        .version_of
        .as_ref()
        .and_then(|p| {
            path_values(&quads, &m, p)
                .into_iter()
                .find_map(|t| match t {
                    Term::NamedNode(n) => Some(n),
                    _ => None,
                })
        })
        .unwrap_or_else(|| m.clone());
    let versioned = entity != m;
    let timestamp = datetime_at(&quads, &m, ctx.timestamp.as_ref());
    let version = VersionKey {
        time: match &ctx.version_timestamp {
            Some(p) => datetime_at(&quads, &m, Some(p)).map(|(_, s)| s),
            None => timestamp.as_ref().map(|(_, s)| s.clone()),
        },
        seq: sequence_at(
            &quads,
            &m,
            ctx.version_sequence.as_ref().or(ctx.sequence.as_ref()),
        ),
    };
    let marks = |marker: &Option<Marker>| {
        marker
            .as_ref()
            .is_some_and(|mk| path_values(&quads, &m, &mk.path).contains(&mk.object))
    };
    // A stream that declares no delete object: this project's publishers
    // before LDES 1.0 typed their tombstones ots:Tombstone only.
    let legacy_tombstone = ctx.delete.is_none()
        && quads.iter().any(|q| {
            q.subject == NamedOrBlankNode::NamedNode(m.clone())
                && q.predicate.as_str() == RDF_TYPE
                && matches!(&q.object, Term::NamedNode(o) if o.as_str() == TOMBSTONE)
        });
    let kind = if marks(&ctx.delete) || legacy_tombstone {
        Kind::Delete
    } else if marks(&ctx.create) {
        Kind::Create
    } else if marks(&ctx.update) {
        Kind::Update
    } else {
        Kind::Upsert
    };
    let (ntriples, subjects) = description(store, ctx, &quads, &m, &entity, versioned);
    RemoteMember {
        iri: m.as_str().to_string(),
        entity: entity.as_str().to_string(),
        timestamp,
        version,
        kind,
        ntriples,
        subjects,
    }
}

/// What a member says about its entity, as N-Triples for the target graph.
///
/// When the member IRI names a graph that holds quads, that graph is the
/// payload (LDES §4.3: the consumer "MAY assume the payload of the upsert is
/// in the named graph") and the member's default-graph triples are version
/// metadata. Otherwise the member's own triples are the payload: for a
/// versioned stream re-subjected from the version to the entity, without the
/// stream's own version properties.
fn description(
    store: &Store,
    ctx: &Context,
    quads: &[Quad],
    m: &NamedNode,
    entity: &NamedNode,
    versioned: bool,
) -> (String, Vec<String>) {
    let in_graph_m = |q: &Quad| matches!(&q.graph_name, GraphName::NamedNode(g) if g == m);
    let payload: Vec<Triple> = if quads.iter().any(in_graph_m) {
        // The named graph's quads, and what their blank nodes lead to.
        let mut seeds: Vec<NamedOrBlankNode> = Vec::new();
        let mut triples: Vec<Triple> = Vec::new();
        for q in quads.iter().filter(|q| in_graph_m(q)) {
            if let Term::BlankNode(b) = &q.object {
                seeds.push(NamedOrBlankNode::BlankNode(b.clone()));
            }
            triples.push(Triple::new(
                q.subject.clone(),
                q.predicate.clone(),
                q.object.clone(),
            ));
        }
        if !seeds.is_empty() {
            triples.extend(
                closure(store, seeds)
                    .into_iter()
                    .map(|q| Triple::new(q.subject, q.predicate, q.object)),
            );
        }
        triples
    } else {
        let simple = |p: &Option<PropertyPath>| match p {
            Some(PropertyPath::Predicate(iri)) => Some(iri.clone()),
            _ => None,
        };
        let stripped: HashSet<String> = if versioned {
            [
                &ctx.version_of,
                &ctx.timestamp,
                &ctx.version_timestamp,
                &ctx.sequence,
                &ctx.version_sequence,
            ]
            .into_iter()
            .filter_map(simple)
            .collect()
        } else {
            HashSet::new()
        };
        let markers: Vec<(String, &Term)> = [&ctx.create, &ctx.update, &ctx.delete]
            .into_iter()
            .flatten()
            .filter_map(|mk| match &mk.path {
                PropertyPath::Predicate(p) => Some((p.clone(), &mk.object)),
                _ => None,
            })
            .collect();
        let is_m = |s: &NamedOrBlankNode| matches!(s, NamedOrBlankNode::NamedNode(n) if n == m);
        quads
            .iter()
            .filter(|q| q.graph_name.is_default_graph())
            .filter(|q| {
                !(is_m(&q.subject)
                    && (stripped.contains(q.predicate.as_str())
                        || markers
                            .iter()
                            .any(|(p, o)| q.predicate.as_str() == p && &q.object == *o)
                        || (q.predicate.as_str() == RDF_TYPE
                            && matches!(&q.object, Term::NamedNode(o) if o.as_str() == TOMBSTONE))))
            })
            .map(|q| {
                let subject = if is_m(&q.subject) {
                    NamedOrBlankNode::NamedNode(entity.clone())
                } else {
                    q.subject.clone()
                };
                Triple::new(subject, q.predicate.clone(), q.object.clone())
            })
            .collect()
    };
    let mut nt = String::new();
    let mut subjects: Vec<String> = vec![entity.as_str().to_string()];
    for t in &payload {
        if let NamedOrBlankNode::NamedNode(s) = &t.subject {
            if !subjects.iter().any(|x| x == s.as_str()) {
                subjects.push(s.as_str().to_string());
            }
        }
        nt.push_str(&format!("{t} .\n"));
    }
    (nt, subjects)
}

// ─── Fetching and relations ─────────────────────────────────────────────────

/// A fetched page.
struct Page {
    /// The URL it was served from after redirects: its base IRI.
    url: String,
    store: Store,
    etag: Option<String>,
    immutable_header: bool,
}

enum Fetched {
    Gone,
    NotModified,
    Page(Page),
}

fn fetch(url: &str, etag: Option<&str>, report: &mut SyncReport) -> anyhow::Result<Fetched> {
    let doc = match crate::remote::get_rdf_blocking(url, etag) {
        Ok(d) => d,
        // LDES §3.3: "A client MUST process 410 Gone as a page with an empty
        // set of relations and an empty set of members."
        Err(RemoteError::Status { status: 410, .. }) => return Ok(Fetched::Gone),
        Err(e) => return Err(e.into()),
    };
    report.retries += doc.retries;
    let Some(body) = doc.body else {
        return Ok(Fetched::NotModified);
    };
    let fmt = parse_rdf_content_type(&doc.content_type).unwrap_or(RdfFormat::Turtle);
    let store = Store::new()?;
    let parser = RdfParser::from_format(fmt)
        .with_base_iri(&doc.url)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    // Parsed here rather than by the store's loader so a JSON-LD page's
    // remote @context goes through the document loader.
    let quads = crate::jsonld::with_loader(parser.for_reader(body.as_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("<{}> is not valid RDF: {e}", doc.url))?;
    store.extend(quads)?;
    Ok(Fetched::Page(Page {
        url: doc.url,
        store,
        etag: doc.etag,
        immutable_header: doc.immutable,
    }))
}

/// One `tree:Relation` out of a page.
struct Relation {
    node: String,
    types: Vec<String>,
    path: Option<PropertyPath>,
    value: Option<String>,
}

/// LDES §3.5.1: `<> tree:relation ?r . ?r tree:node ?n` with `<>` the page
/// (the URL it was served from, or the node IRI that led here).
fn relations_of(store: &Store, page_iris: &[&str]) -> Vec<Relation> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for page in page_iris {
        for r in objects(store, page, &format!("{TREE}relation")) {
            let r = lexical(&r);
            if !seen.insert(r.clone()) {
                continue;
            }
            let types: Vec<String> = objects(store, &r, RDF_TYPE).iter().map(lexical).collect();
            let path = objects(store, &r, &format!("{TREE}path"))
                .first()
                .and_then(|t| path_at(store, t));
            let value = objects(store, &r, &format!("{TREE}value"))
                .into_iter()
                .find_map(|t| match t {
                    Term::Literal(l) => Some(l.value().to_string()),
                    _ => None,
                });
            for n in objects(store, &r, &format!("{TREE}node")) {
                if let Term::NamedNode(n) = n {
                    out.push(Relation {
                        node: n.as_str().to_string(),
                        types: types.clone(),
                        path: path.clone(),
                        value: value.clone(),
                    });
                }
            }
        }
    }
    out
}

/// Whether the relations to one node (combined with a logical AND, LDES
/// §3.5.2) put every member it can hold before `bookmark` on the stream's
/// timestamp path — a node the client may skip (LDES §3.2 MAY).
fn below_bookmark(
    relations: &[&Relation],
    timestamp: Option<&PropertyPath>,
    bookmark: Option<&DateTime<FixedOffset>>,
) -> bool {
    let (Some(tp), Some(b)) = (timestamp, bookmark) else {
        return false;
    };
    relations.iter().any(|r| {
        let (Some(path), Some(v)) = (&r.path, r.value.as_deref().and_then(parse_datetime)) else {
            return false;
        };
        if !same_path(path, tp) {
            return false;
        }
        r.types.iter().any(|t| {
            (t == &format!("{TREE}LessThanRelation") && v <= *b)
                || (t == &format!("{TREE}LessThanOrEqualToRelation") && v < *b)
        })
    })
}

fn is_immutable(page: &Page, node: &str) -> bool {
    if page.immutable_header {
        return true;
    }
    [page.url.as_str(), node].iter().any(|p| {
        objects(&page.store, p, &format!("{LDES}immutable"))
            .iter()
            .any(|t| matches!(t, Term::Literal(l) if l.value() == "true" || l.value() == "1"))
    })
}

// ─── Initialisation (LDES §3.1) ─────────────────────────────────────────────

/// The stream IRI, its root node and — when the entry page is the root — the
/// already fetched root page.
struct Start {
    stream: String,
    root: String,
    root_page: Option<Page>,
}

fn initialise(entry_url: &str, entry: Page) -> anyhow::Result<(Start, Page)> {
    let view = format!("{TREE}view");
    // "?s tree:view <> with <> the base IRI (after redirect)".
    let streams = subjects(&entry.store, &view, &entry.url);
    match streams.len() {
        1 => {
            let root = entry.url.clone();
            let stream = streams.into_iter().next().unwrap_or_default();
            return Ok((
                Start {
                    stream,
                    root,
                    root_page: None,
                },
                entry,
            ));
        }
        0 => {}
        n => anyhow::bail!(
            "<{}> is the tree:view of {n} event streams; LDES §3.1 requires exactly one",
            entry.url
        ),
    }
    // "I tree:view ?o": I is the stream, ?o its root node.
    let mut found: Vec<(String, String)> = Vec::new();
    for i in [entry_url, entry.url.as_str()] {
        for o in objects(&entry.store, i, &view) {
            if let Term::NamedNode(o) = o {
                let pair = (i.to_string(), o.as_str().to_string());
                if !found.contains(&pair) {
                    found.push(pair);
                }
            }
        }
    }
    match found.len() {
        1 => {
            let (stream, root) = found.remove(0);
            Ok((
                Start {
                    stream,
                    root,
                    root_page: None,
                },
                entry,
            ))
        }
        0 => anyhow::bail!(
            "<{entry_url}> is neither an event stream with a tree:view nor its root node \
             (LDES §3.1): sync from the stream IRI or its root node"
        ),
        n => anyhow::bail!("<{entry_url}> has {n} tree:view links; LDES §3.1 requires exactly one"),
    }
}

// ─── The sync run ───────────────────────────────────────────────────────────

/// Follow the stream from `url`, applying newer members into `graph_iri`.
pub fn sync(
    state: &AppState,
    url: &str,
    dataset_id: &str,
    graph_iri: &str,
) -> anyhow::Result<SyncReport> {
    NamedNode::new(graph_iri).map_err(|e| anyhow::anyhow!("graph IRI: {e}"))?;
    if !crate::remote::is_allowed(url) {
        anyhow::bail!("{}", RemoteError::NotAllowed(url.to_string()));
    }
    let prev = super::store::sync_state(&state.auth_db, dataset_id, url)?;
    let mut report = SyncReport {
        url: url.to_string(),
        dataset_id: dataset_id.to_string(),
        graph_iri: graph_iri.to_string(),
        last_timestamp: prev.bookmark.clone(),
        ..Default::default()
    };

    let entry = match fetch(url, None, &mut report)? {
        Fetched::Page(p) => p,
        // Entering at a compacted node: an empty page, not a failed sync.
        Fetched::Gone => {
            report.nodes_gone += 1;
            return Ok(report);
        }
        Fetched::NotModified => anyhow::bail!("<{url}> answered 304 to an unconditional request"),
    };
    let (mut start, entry) = initialise(url, entry)?;
    let entry = if start.root == entry.url {
        start.root_page = Some(entry);
        None
    } else {
        Some(entry)
    };
    // The root node is always fetched in full: it carries the context.
    let root_page = match start.root_page.take() {
        Some(p) => p,
        None => match fetch(&start.root, None, &mut report)? {
            Fetched::Page(p) => p,
            Fetched::Gone => {
                report.nodes_gone += 1;
                report.stream = Some(start.stream);
                report.root_node = Some(start.root);
                return Ok(report);
            }
            Fetched::NotModified => {
                anyhow::bail!("<{}> answered 304 to an unconditional request", start.root)
            }
        },
    };
    let docs: Vec<&Store> = std::iter::once(&root_page.store)
        .chain(entry.as_ref().map(|e| &e.store))
        .collect();
    let ctx = Context::read(&start.stream, &docs);
    report.stream = Some(start.stream.clone());
    report.root_node = Some(start.root.clone());
    report.polling_interval = ctx.polling_interval;
    report.shapes = ctx.shapes.clone();
    report.retention_policy = retention_of(&root_page.store)
        .or_else(|| entry.as_ref().and_then(|e| retention_of(&e.store)));
    retention_warnings(&prev, &mut report);
    drop(entry);

    let crawl = crawl(&start.root, root_page, &ctx, &prev, &mut report)?;
    materialise(state, url, dataset_id, graph_iri, &prev, crawl, &mut report)?;
    Ok(report)
}

/// A bookmark from before the publisher's window is a hole the caller is
/// told about: members between the two may never reach this mirror.
fn retention_warnings(prev: &SyncState, report: &mut SyncReport) {
    let (Some(b), Some(policy)) = (prev.bookmark.as_deref(), report.retention_policy.as_ref())
    else {
        return;
    };
    let Some(at) = parse_datetime(b) else {
        return;
    };
    let at = at.with_timezone(&chrono::Utc);
    let mut warnings = Vec::new();
    if let Some(d) = policy
        .full_log_duration
        .as_deref()
        .and_then(|d| super::store::parse_xsd_duration(d).ok())
    {
        if at < chrono::Utc::now() - d {
            warnings.push(format!(
                "the bookmark {b} predates the publisher's retention window \
                 ({}): members created between them may have been compacted \
                 away before this sync",
                policy.full_log_duration.as_deref().unwrap_or("?")
            ));
        }
    }
    if let Some(from) = policy.starting_from.as_deref() {
        if parse_datetime(from).is_some_and(|f| at < f.with_timezone(&chrono::Utc)) {
            warnings.push(format!(
                "the bookmark {b} predates the publisher's retention window, which \
                 starts at {from}: members created between them are not served"
            ));
        }
    }
    report.warnings.extend(warnings);
}

/// What one run collected before anything is written.
struct Crawl {
    /// The newest unapplied member per entity.
    newest: HashMap<String, RemoteMember>,
    /// The newest member timestamp seen, and the members carrying it.
    max_time: Option<(DateTime<FixedOffset>, String)>,
    at_max: Vec<String>,
    pages: Vec<(String, SyncPage)>,
}

fn crawl(
    root: &str,
    root_page: Page,
    ctx: &Context,
    prev: &SyncState,
    report: &mut SyncReport,
) -> anyhow::Result<Crawl> {
    let bookmark = prev.bookmark.as_deref().and_then(parse_datetime);
    let at_bookmark: HashSet<&str> = prev.bookmark_members.iter().map(String::as_str).collect();
    // Members without a timestamp that earlier runs saw on pages that were
    // still mutable: the only record that they were emitted.
    let known_untimed: HashSet<&str> = prev
        .pages
        .values()
        .flat_map(|p| p.members.iter().map(String::as_str))
        .collect();
    let mut out = Crawl {
        newest: HashMap::new(),
        max_time: None,
        at_max: Vec::new(),
        pages: Vec::new(),
    };
    let mut seen_members: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<String> = VecDeque::from([root.to_string()]);
    let mut queued: HashSet<String> = HashSet::from([root.to_string()]);
    let mut root_page = Some(root_page);
    let mut fetched = 0usize;
    while let Some(node) = queue.pop_front() {
        fetched += 1;
        if fetched > MAX_NODES {
            anyhow::bail!("stream has more than 10 000 nodes; refusing to follow further");
        }
        let known = prev.pages.get(&node);
        let page = match root_page.take() {
            Some(p) => p,
            None => match fetch(&node, known.and_then(|k| k.etag.as_deref()), report)? {
                Fetched::Page(p) => p,
                Fetched::Gone => {
                    report.nodes_gone += 1;
                    // Gone is permanent: nothing to fetch next time either.
                    out.pages.push((
                        node,
                        SyncPage {
                            immutable: true,
                            ..Default::default()
                        },
                    ));
                    continue;
                }
                Fetched::NotModified => {
                    report.nodes_not_modified += 1;
                    for next in known.map(|k| k.links.clone()).unwrap_or_default() {
                        enqueue(next, &mut queue, &mut queued, prev, &[], ctx, report);
                    }
                    continue;
                }
            },
        };
        report.nodes_visited += 1;
        let immutable = is_immutable(&page, &node);
        let mut untimed: Vec<String> = Vec::new();
        for m in members_of(&page.store, ctx) {
            if !seen_members.insert(m.iri.clone()) {
                continue;
            }
            report.members_seen += 1;
            let emitted = match (&m.timestamp, &bookmark) {
                (Some((t, _)), Some(b)) => {
                    t < b || (t == b && at_bookmark.contains(m.iri.as_str()))
                }
                (None, _) => known_untimed.contains(m.iri.as_str()),
                (Some(_), None) => false,
            };
            if m.timestamp.is_none() {
                untimed.push(m.iri.clone());
            }
            if emitted {
                report.members_skipped_older += 1;
                continue;
            }
            if let Some((t, lex)) = &m.timestamp {
                match out.max_time.as_ref().map(|(mt, _)| t.cmp(mt)) {
                    None | Some(Ordering::Greater) => {
                        out.max_time = Some((*t, lex.clone()));
                        out.at_max = vec![m.iri.clone()];
                    }
                    Some(Ordering::Equal) => out.at_max.push(m.iri.clone()),
                    Some(Ordering::Less) => {}
                }
            }
            match out.newest.get(&m.entity) {
                // Ties go to the later find: with no order to go by, the
                // newer discovery is the better guess.
                Some(cur) if m.version.cmp(&cur.version) == Ordering::Less => {
                    report.versions_superseded += 1;
                }
                Some(_) => {
                    report.versions_superseded += 1;
                    out.newest.insert(m.entity.clone(), m);
                }
                None => {
                    out.newest.insert(m.entity.clone(), m);
                }
            }
        }
        let relations = relations_of(&page.store, &[page.url.as_str(), node.as_str()]);
        let mut links: Vec<String> = Vec::new();
        for r in &relations {
            if !links.contains(&r.node) {
                links.push(r.node.clone());
            }
        }
        for next in &links {
            let to: Vec<&Relation> = relations.iter().filter(|r| &r.node == next).collect();
            enqueue(
                next.clone(),
                &mut queue,
                &mut queued,
                prev,
                &to,
                ctx,
                report,
            );
        }
        // The root is fetched in full every run; it is never recorded as a
        // page to skip.
        out.pages.push((
            node.clone(),
            SyncPage {
                immutable: immutable && node != root,
                etag: page.etag.clone(),
                links,
                members: untimed,
            },
        ));
    }
    Ok(out)
}

/// Queue `next` unless it was queued already, an earlier run processed it as
/// immutable, or its relations bound it below the bookmark.
fn enqueue(
    next: String,
    queue: &mut VecDeque<String>,
    queued: &mut HashSet<String>,
    prev: &SyncState,
    relations: &[&Relation],
    ctx: &Context,
    report: &mut SyncReport,
) {
    if !queued.insert(next.clone()) {
        return;
    }
    if prev.pages.get(&next).is_some_and(|p| p.immutable) {
        report.nodes_skipped_immutable += 1;
        return;
    }
    let bookmark = prev.bookmark.as_deref().and_then(parse_datetime);
    if below_bookmark(relations, ctx.timestamp.as_ref(), bookmark.as_ref()) {
        report.nodes_pruned += 1;
        return;
    }
    queue.push_back(next);
}

/// Remove what an entity's last applied version wrote: the triples of each
/// subject and the blank nodes under them, up to [`BLANK_NODE_DEPTH`] deep.
fn delete_group(state: &AppState, graph_iri: &str, subjects: &[String]) -> anyhow::Result<()> {
    if subjects.is_empty() {
        return Ok(());
    }
    let values: String = subjects
        .iter()
        .map(|s| format!("<{}>", crate::store::escape_sparql_iri(s)))
        .collect::<Vec<_>>()
        .join(" ");
    // Every branch binds ?s itself (a union branch is evaluated on its own,
    // so a VALUES outside it would leave ?s unbound inside it).
    let mut branches = vec![format!("{{ VALUES ?s {{ {values} }} }}")];
    for depth in 1..=BLANK_NODE_DEPTH {
        let mut chain = format!("VALUES ?n {{ {values} }} ");
        let mut from = "?n".to_string();
        for i in 1..depth {
            chain.push_str(&format!("{from} ?x{i} ?b{i} . FILTER(isBlank(?b{i})) "));
            from = format!("?b{i}");
        }
        chain.push_str(&format!("{from} ?x{depth} ?s . FILTER(isBlank(?s))"));
        branches.push(format!("{{ {chain} }}"));
    }
    state.store.update(&format!(
        "DELETE {{ GRAPH <{graph_iri}> {{ ?s ?p ?o }} }} WHERE {{ GRAPH <{graph_iri}> {{ \
           {} ?s ?p ?o }} }}",
        branches.join(" UNION ")
    ))?;
    Ok(())
}

fn materialise(
    state: &AppState,
    url: &str,
    dataset_id: &str,
    graph_iri: &str,
    prev: &SyncState,
    crawl: Crawl,
    report: &mut SyncReport,
) -> anyhow::Result<()> {
    let mut entities: Vec<RemoteMember> = crawl.newest.into_values().collect();
    entities.sort_by(|a, b| {
        a.version
            .cmp(&b.version)
            .then_with(|| a.entity.cmp(&b.entity))
    });
    let mut applied: Vec<(String, SyncEntity)> = Vec::new();
    for m in entities {
        let before = super::store::sync_entity(&state.auth_db, dataset_id, url, &m.entity)?;
        let stored_version = before
            .as_ref()
            .and_then(|b| serde_json::from_str::<VersionKey>(&b.version).ok());
        // An older version than the one applied, arriving late (LDES §4.3:
        // versions may be published out of order).
        if stored_version.is_some_and(|v| m.version.cmp(&v) == Ordering::Less) {
            report.versions_superseded += 1;
            continue;
        }
        // Without a record (a mirror from before the record existed), the
        // entity's own triples are what an earlier sync wrote.
        let mut subjects = before
            .map(|b| b.subjects)
            .unwrap_or_else(|| vec![m.entity.clone()]);
        if m.kind != Kind::Create {
            delete_group(state, graph_iri, &subjects)?;
            subjects.clear();
        }
        if m.kind == Kind::Delete {
            report.entities_deleted += 1;
        } else {
            if !m.ntriples.is_empty() {
                state.store.update(&format!(
                    "INSERT DATA {{ GRAPH <{graph_iri}> {{\n{}\n}} }}",
                    m.ntriples
                ))?;
            }
            for s in m.subjects {
                if !subjects.contains(&s) {
                    subjects.push(s);
                }
            }
            report.entities_updated += 1;
        }
        applied.push((
            m.entity,
            SyncEntity {
                version: serde_json::to_string(&m.version)?,
                subjects,
            },
        ));
    }

    // The bookmark moves to the newest timestamp seen; members that share
    // it are remembered, so one published later with the same timestamp is
    // still emitted (LDES §3.2).
    let old = prev.bookmark.as_deref().and_then(parse_datetime);
    let (bookmark, at_bookmark) = match (&crawl.max_time, old) {
        (Some((t, lex)), Some(o)) if *t == o => {
            let mut members = prev.bookmark_members.clone();
            for m in crawl.at_max {
                if !members.contains(&m) {
                    members.push(m);
                }
            }
            (
                Some(prev.bookmark.clone().unwrap_or_else(|| lex.clone())),
                members,
            )
        }
        (Some((t, lex)), o) if o.is_none_or(|o| *t > o) => (Some(lex.clone()), crawl.at_max),
        _ => (prev.bookmark.clone(), prev.bookmark_members.clone()),
    };
    report.last_timestamp = bookmark.clone();
    super::store::save_sync_run(
        &state.auth_db,
        dataset_id,
        url,
        &SyncRun {
            bookmark: bookmark.as_deref(),
            bookmark_members: &at_bookmark,
            applied: (report.entities_updated + report.entities_deleted) as u64,
            pages: &crawl.pages,
            entities: &applied,
        },
    )
}

/// POST /api/ldes/sync
pub async fn sync_handler(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<SyncBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let e500 = |e: anyhow::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let ds = state
        .auth_db
        .get_dataset(&body.dataset_id)
        .map_err(e500)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    // The same gate as registering the graph to the dataset: inside its
    // boundary for non-admins, and never a model-registry graph for anyone.
    let claim = crate::auth::dataset_graph::gate_dataset_graph_target(
        &state.store,
        &state.auth_db,
        &state.base_url,
        &body.dataset_id,
        &body.graph_iri,
        &user,
    )
    .map_err(|m| (StatusCode::FORBIDDEN, m))?;
    if !crate::remote::is_allowed(&body.url) {
        return Err((
            StatusCode::FORBIDDEN,
            RemoteError::NotAllowed(body.url.clone()).to_string(),
        ));
    }
    // The target graph belongs to the dataset (registered if it is new), so
    // the dataset's own stream — if any — and its history see the sync.
    let _ = crate::auth::dataset_graph::register_claimed_graph(
        &state.auth_db,
        &body.dataset_id,
        &body.graph_iri,
        claim,
    );
    let st = state.clone();
    let (url, ds_id, graph) = (
        body.url.clone(),
        body.dataset_id.clone(),
        body.graph_iri.clone(),
    );
    let uid = user.user_id.clone();
    let federated_identity = crate::federation::identity_for(&state, &user.user_id);
    let report = tokio::task::spawn_blocking(move || {
        let _identity = crate::federation::IdentityGuard::set(federated_identity);
        let before = super::capture::before(&st, std::slice::from_ref(&graph));
        let r = sync(&st, &url, &ds_id, &graph);
        super::capture::after(&st, before);
        crate::entailment::after_write(&st, std::slice::from_ref(&graph));
        if let Ok(r) = &r {
            crate::commit_log::record(
                &st.store,
                &st.base_url,
                crate::commit_log::CommitKind::Import,
                format!(
                    "LDES sync from <{url}>: {} entities updated, {} deleted",
                    r.entities_updated, r.entities_deleted
                ),
                Some(&uid),
                Some(format!(
                    "{}/dataset/{}",
                    st.base_url.trim_end_matches('/'),
                    ds_id
                )),
                vec![graph.clone()],
                r.entities_updated,
                r.entities_deleted,
                None,
            );
        }
        r
    })
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
    Ok(Json(report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(time: Option<&str>, seq: Option<(&str, &str)>) -> VersionKey {
        VersionKey {
            time: time.map(str::to_string),
            seq: seq.map(|(v, dt)| Sequence {
                value: v.to_string(),
                datatype: format!("{XSD}{dt}"),
            }),
        }
    }

    #[test]
    fn timestamps_compare_as_instants_not_strings() {
        // Lexically "…T10:00:00+02:00" > "…T09:00:00Z"; as instants it is earlier.
        let a = parse_datetime("2026-01-01T10:00:00+02:00").unwrap();
        let b = parse_datetime("2026-01-01T09:00:00Z").unwrap();
        assert!(a < b);
        assert_eq!(
            parse_datetime("2026-01-01T09:00:00Z"),
            parse_datetime("2026-01-01T09:00:00.000+00:00")
        );
        assert!(
            parse_datetime("2026-01-01T09:00:00").is_some(),
            "no timezone: UTC"
        );
        assert!(parse_datetime("yesterday").is_none());
    }

    #[test]
    fn versions_order_by_time_then_sequence() {
        let early = key(Some("2026-01-01T00:00:00Z"), Some(("9", "integer")));
        let late = key(Some("2026-01-02T00:00:00Z"), Some(("1", "integer")));
        assert_eq!(early.cmp(&late), Ordering::Less, "time first");
        let two = key(Some("2026-01-01T00:00:00Z"), Some(("2", "integer")));
        let ten = key(Some("2026-01-01T00:00:00Z"), Some(("10", "integer")));
        assert_eq!(two.cmp(&ten), Ordering::Less, "integers numerically");
        let s2 = key(None, Some(("b", "string")));
        let s1 = key(None, Some(("a", "string")));
        assert_eq!(s2.cmp(&s1), Ordering::Greater);
        assert_eq!(key(None, None).cmp(&key(None, None)), Ordering::Equal);
        let json = serde_json::to_string(&ten).unwrap();
        assert_eq!(serde_json::from_str::<VersionKey>(&json).unwrap(), ten);
    }
}
