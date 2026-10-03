//! The published side of LDES: `GET /api/datasets/:id/ldes` (the
//! `ldes:EventStream`), `GET /api/datasets/:id/ldes/nodes/:n` (node 0 is the
//! root, fragments are numbered from 1), `GET /api/datasets/:id/ldes/members/:m`
//! (one member, dereferenced), plus `PUT /api/datasets/:id/ldes` to enable the
//! stream and declare its retention policy and polling interval.
//!
//! A stream is readable by everyone who can access the dataset, so it carries
//! only what a dataset viewer may see: graphs marked private are never
//! published — neither seeded when the stream is enabled nor captured on
//! later writes (see [`super::capture`]).
//!
//! Fragments are frozen once full ([`store::seal_full_pages`]): a sealed
//! node is an id range, and retention only ever deletes rows inside a range.
//! So a page served as immutable never gains, loses to another page, or
//! reorders a member — it can only shrink, and a page whose members are all
//! gone answers `410 Gone` (LDES Server Primer §5.1), with no relation
//! pointing at it.
//!
//! The search tree is the one the Server Primer §4 recommends: one root node
//! (node 0, the `tree:view`) with "two relations towards one node, one with
//! the lower bound and another with the upper bound" for every sealed node
//! that still has members, and a lower-bounded relation to the first page of
//! the unsealed tail, whose pages chain forward. A sealed node links nowhere,
//! so the subtree under each root relation (TREE §3) is exactly that node and
//! its bounds hold.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::{BlankNode, Literal, NamedNode, NamedOrBlankNode, Term, Triple};
use serde::Deserialize;

use super::store::{self, Member, RetentionPolicy, SealedNode};
use super::{
    member_iri, node_iri, stream_iri, AS, AS_DELETE, DCT, LDES, OTS, SH, TOMBSTONE, TREE, XSD,
};
use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::Dataset;
use crate::server::content_negotiation::negotiate_graph_format;
use crate::server::AppState;

type ApiErr = (StatusCode, String);

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn nn(iri: &str) -> NamedNode {
    NamedNode::new_unchecked(iri)
}

fn visible_dataset(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
    id: &str,
) -> Result<Dataset, ApiErr> {
    let ds = state
        .auth_db
        .get_dataset(id)
        .map_err(e500)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    let uid = user.map(|u| u.user_id.as_str());
    if !state.auth_db.can_access_dataset(uid, &ds).map_err(e500)? {
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }
    Ok(ds)
}

#[derive(Debug, Deserialize)]
pub struct StreamBody {
    pub enabled: bool,
    #[serde(default)]
    pub page_size: Option<u64>,
    /// The retention policy to declare and enforce. Absent: unchanged. An
    /// empty object (`{}`) clears it — the stream keeps every member again.
    #[serde(default)]
    pub retention: Option<RetentionPolicy>,
    /// `ldes:pollingInterval` in seconds (at least 1). Absent: unchanged
    /// (60 until one is set).
    #[serde(default)]
    pub polling_interval: Option<u64>,
}

/// PUT /api/datasets/:id/ldes — enable (or disable) the dataset's stream and
/// set its retention policy. Enabling a stream that has no members yet
/// publishes every entity of the dataset's non-private graphs as its first
/// members. A policy is enforced only after it is declared, never before:
/// declaring more retention than is enforced is harmless, the reverse is the
/// spec violation (LDES §4.4).
pub async fn put_stream(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<StreamBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = visible_dataset(&state, Some(&user), &dataset_id)?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    if let Some(policy) = &body.retention {
        policy
            .validate()
            .map_err(|e| (StatusCode::BAD_REQUEST, format!("retention: {e}")))?;
    }
    if body.polling_interval == Some(0) {
        return Err((
            StatusCode::BAD_REQUEST,
            "polling_interval: must be at least 1 second".to_string(),
        ));
    }
    let page_size = body.page_size.unwrap_or(100).clamp(1, 10_000);
    // Pages that are already full keep the bounds they were served with:
    // seal them under the old page size before the new one applies.
    if let Some(old) = store::stream(&state.auth_db, &dataset_id).map_err(e500)? {
        if old.page_size != page_size {
            store::seal_full_pages(&state.auth_db, &dataset_id, old.page_size).map_err(e500)?;
        }
    }
    store::set_stream(&state.auth_db, &dataset_id, body.enabled, page_size).map_err(e500)?;
    if let Some(policy) = &body.retention {
        store::set_retention(&state.auth_db, &dataset_id, policy).map_err(e500)?;
    }
    if body.polling_interval.is_some() {
        store::set_polling_interval(&state.auth_db, &dataset_id, body.polling_interval)
            .map_err(e500)?;
    }
    let mut seeded = 0;
    if body.enabled && store::member_count(&state.auth_db, &dataset_id).map_err(e500)? == 0 {
        let graphs: Vec<String> = state
            .auth_db
            .list_dataset_graph_entries(&dataset_id)
            .map_err(e500)?
            .into_iter()
            .filter(|e| !e.private)
            .map(|e| e.graph_iri)
            .collect();
        let st = state.clone();
        let id = dataset_id.clone();
        seeded =
            tokio::task::spawn_blocking(move || super::capture::publish_all(&st, &id, &graphs))
                .await
                .map_err(e500)?;
    }
    // Seal what is full and apply the policy now, not on the next write.
    let pruned = {
        let st = state.clone();
        let id = dataset_id.clone();
        tokio::task::spawn_blocking(move || super::capture::settle(&st, &id, true))
            .await
            .map_err(e500)?
    };
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "enabled": body.enabled,
        "page_size": page_size,
        "stream": stream_iri(&state.base_url, &dataset_id),
        "members_seeded": seeded,
        "members_pruned": pruned,
        "members": store::member_count(&state.auth_db, &dataset_id).map_err(e500)?,
        "retention": store::retention(&state.auth_db, &dataset_id).map_err(e500)?,
        "polling_interval": store::stream(&state.auth_db, &dataset_id)
            .map_err(e500)?
            .and_then(|c| c.polling_interval)
            .unwrap_or(store::DEFAULT_POLLING_INTERVAL),
    })))
}

fn enabled_stream(state: &AppState, dataset_id: &str) -> Result<store::StreamConfig, ApiErr> {
    match store::stream(&state.auth_db, dataset_id).map_err(e500)? {
        Some(cfg) if cfg.enabled => Ok(cfg),
        _ => Err((
            StatusCode::NOT_FOUND,
            "This dataset does not publish an event stream".to_string(),
        )),
    }
}

// ─── Load gate (Server Primer §2) ───────────────────────────────────────────

/// "If the server is overloaded, it MUST provide a 429 Too Many Requests.
/// The client will then retry later." A stream document is rendered on the
/// blocking pool (SQLite reads plus serialisation); at most
/// `OTS_LDES_MAX_IN_FLIGHT` renders run at once (default 64), and at most
/// `OTS_LDES_MAX_IN_FLIGHT_PER_STREAM` (default 16) for one dataset, so one
/// busy stream cannot starve the rest. A request beyond either is answered
/// `429` with `Retry-After` at once rather than queued.
struct Gate {
    total: usize,
    per_stream: HashMap<String, usize>,
}

fn gate() -> &'static Mutex<Gate> {
    static GATE: OnceLock<Mutex<Gate>> = OnceLock::new();
    GATE.get_or_init(|| {
        Mutex::new(Gate {
            total: 0,
            per_stream: HashMap::new(),
        })
    })
}

fn limit(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

/// A slot in the load gate, released on drop.
pub struct InFlight {
    dataset_id: String,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut g = gate().lock().unwrap_or_else(|p| p.into_inner());
        g.total = g.total.saturating_sub(1);
        if let Some(n) = g.per_stream.get_mut(&self.dataset_id) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                g.per_stream.remove(&self.dataset_id);
            }
        }
    }
}

/// Take a slot for rendering one of `dataset_id`'s stream documents, or
/// `None` when the server or that stream is at its limit.
pub fn try_enter(dataset_id: &str) -> Option<InFlight> {
    let total_limit = limit("OTS_LDES_MAX_IN_FLIGHT", 64);
    let stream_limit = limit("OTS_LDES_MAX_IN_FLIGHT_PER_STREAM", 16);
    let mut g = gate().lock().unwrap_or_else(|p| p.into_inner());
    let current = g.per_stream.get(dataset_id).copied().unwrap_or(0);
    if g.total >= total_limit || current >= stream_limit {
        return None;
    }
    g.total += 1;
    g.per_stream.insert(dataset_id.to_string(), current + 1);
    Some(InFlight {
        dataset_id: dataset_id.to_string(),
    })
}

fn too_busy() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, HeaderValue::from_static("1"))],
        "The event stream is busy; retry shortly.",
    )
        .into_response()
}

// ─── Rendering ──────────────────────────────────────────────────────────────

/// A strong validator over the bytes actually sent: each negotiated
/// representation gets its own.
fn etag(content_type: &str, body: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(content_type.as_bytes());
    h.update([0u8]);
    h.update(body);
    let digest = h.finalize();
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

/// RFC 9110 §13.1.2: `If-None-Match` matches `*` or any listed entity tag,
/// compared weakly (a `W/` prefix is ignored).
fn none_match(headers: &HeaderMap, tag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim())
        .any(|t| t == "*" || t.trim_start_matches("W/") == tag)
}

/// Serialise `triples` (default graph) in the negotiated format, with the
/// stream's prefixes for readable Turtle. Every document carries an `ETag`
/// (Server Primer §2: "It SHOULD provide an ETag header on responses") and
/// answers a matching `If-None-Match` with `304 Not Modified` (LDES §3.3).
/// Blank-node labels are fixed by the renderer, so an unchanged page
/// serialises to the same bytes and keeps its tag.
fn render(
    headers: &HeaderMap,
    triples: &[Triple],
    cache: &'static str,
) -> Result<Response, ApiErr> {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/turtle");
    let gf = negotiate_graph_format(accept);
    let fmt = gf.to_rdf_format();
    let mut ser = RdfSerializer::from_format(fmt);
    if matches!(fmt, RdfFormat::Turtle | RdfFormat::TriG) {
        for (p, iri) in [
            ("ldes", LDES),
            ("tree", TREE),
            ("dct", DCT),
            ("xsd", XSD),
            ("ots", OTS),
            ("sh", SH),
            ("as", AS),
        ] {
            ser = ser.with_prefix(p, iri).map_err(e500)?;
        }
    }
    let mut buf = Vec::new();
    let mut w = ser.for_writer(&mut buf);
    for t in triples {
        w.serialize_triple(t.as_ref()).map_err(e500)?;
    }
    w.finish().map_err(e500)?;
    let tag = etag(gf.content_type(), &buf);
    let mut resp = if none_match(headers, &tag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut r = (StatusCode::OK, buf).into_response();
        r.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(gf.content_type()),
        );
        r
    };
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(header::VARY, HeaderValue::from_static("Accept"));
    if let Ok(v) = HeaderValue::from_str(&tag) {
        h.insert(header::ETAG, v);
    }
    Ok(resp)
}

/// The stream's fragment layout: the sealed nodes, then the unsealed tail
/// paged from the last sealed id — which, for a stream sealed nowhere, is
/// the whole stream paged from the start, as it always was.
struct Layout {
    sealed: Vec<SealedNode>,
    tail_after: i64,
    tail_count: u64,
    page_size: u64,
}

impl Layout {
    fn load(state: &AppState, dataset_id: &str, page_size: u64) -> Result<Self, ApiErr> {
        let sealed = store::sealed_nodes(&state.auth_db, dataset_id).map_err(e500)?;
        let tail_after = sealed.last().map(|s| s.last_id).unwrap_or(0);
        let tail_count =
            store::count_after(&state.auth_db, dataset_id, tail_after).map_err(e500)?;
        Ok(Self {
            sealed,
            tail_after,
            tail_count,
            page_size: page_size.max(1),
        })
    }

    fn sealed_count(&self) -> u64 {
        self.sealed.len() as u64
    }

    /// The tail is at least one page: an empty stream is one empty node.
    fn pages(&self) -> u64 {
        self.sealed_count() + self.tail_count.div_ceil(self.page_size).max(1)
    }

    fn is_sealed(&self, n: u64) -> bool {
        n >= 1 && n <= self.sealed_count()
    }

    /// The first page of the unsealed tail.
    fn tail_start(&self) -> u64 {
        self.sealed_count() + 1
    }

    /// The earliest `dct:created` in the tail from page `n` on: a lower bound
    /// for every member reachable from page `n`, now and later (later
    /// members are never earlier than the newest one, LDES §4.1).
    fn tail_floor(
        &self,
        state: &AppState,
        dataset_id: &str,
        n: u64,
    ) -> Result<Option<String>, ApiErr> {
        let skip = (n - self.tail_start()) * self.page_size;
        let stamps = store::tail_created_from(&state.auth_db, dataset_id, self.tail_after, skip)
            .map_err(e500)?;
        Ok(store::extreme(stamps.iter().map(String::as_str), false))
    }
}

/// The stream's shape (LDES §4.2, TREE `tree:shape`): what every member
/// carries — one `dct:isVersionOf` IRI and one `dct:created` dateTime. Open,
/// so an entity's own properties are allowed whatever they are.
fn shape_triples(s: &NamedNode) -> Vec<Triple> {
    let shape = nn(&format!("{}#member-shape", s.as_str()));
    let version_of = BlankNode::new_unchecked("shapeVersionOf");
    let created = BlankNode::new_unchecked("shapeCreated");
    let sh = |l: &str| nn(&format!("{SH}{l}"));
    let one = || Literal::new_typed_literal("1", nn(&format!("{XSD}integer")));
    vec![
        Triple::new(s.clone(), nn(&format!("{TREE}shape")), shape.clone()),
        Triple::new(shape.clone(), nn(&format!("{RDF}type")), sh("NodeShape")),
        Triple::new(shape.clone(), sh("property"), version_of.clone()),
        Triple::new(
            version_of.clone(),
            sh("path"),
            nn(&format!("{DCT}isVersionOf")),
        ),
        Triple::new(version_of.clone(), sh("minCount"), one()),
        Triple::new(version_of.clone(), sh("maxCount"), one()),
        Triple::new(version_of, sh("nodeKind"), sh("IRI")),
        Triple::new(shape, sh("property"), created.clone()),
        Triple::new(created.clone(), sh("path"), nn(&format!("{DCT}created"))),
        Triple::new(created.clone(), sh("minCount"), one()),
        Triple::new(created.clone(), sh("maxCount"), one()),
        Triple::new(created, sh("datatype"), nn(&format!("{XSD}dateTime"))),
    ]
}

fn stream_description(
    base: &str,
    ds: &Dataset,
    cfg: &store::StreamConfig,
    policy: Option<&RetentionPolicy>,
) -> Vec<Triple> {
    let s = nn(&stream_iri(base, &ds.id));
    let view_node = node_iri(base, &ds.id, 0);
    let mut t = vec![
        Triple::new(
            s.clone(),
            nn(&format!("{RDF}type")),
            nn(&format!("{LDES}EventStream")),
        ),
        Triple::new(
            s.clone(),
            nn(&format!("{DCT}title")),
            Literal::new_simple_literal(format!("{} — event stream", ds.name)),
        ),
        Triple::new(
            s.clone(),
            nn(&format!("{LDES}timestampPath")),
            nn(&format!("{DCT}created")),
        ),
        Triple::new(
            s.clone(),
            nn(&format!("{LDES}versionOfPath")),
            nn(&format!("{DCT}isVersionOf")),
        ),
        // LDES §4.3: how a client tells a delete from a version.
        Triple::new(
            s.clone(),
            nn(&format!("{LDES}versionDeletePath")),
            nn(&format!("{RDF}type")),
        ),
        Triple::new(
            s.clone(),
            nn(&format!("{LDES}versionDeleteObject")),
            nn(AS_DELETE),
        ),
        Triple::new(
            s.clone(),
            nn(&format!("{LDES}pollingInterval")),
            Literal::new_typed_literal(
                cfg.polling_interval
                    .unwrap_or(store::DEFAULT_POLLING_INTERVAL)
                    .to_string(),
                nn(&format!("{XSD}integer")),
            ),
        ),
        Triple::new(s.clone(), nn(&format!("{TREE}view")), nn(&view_node)),
    ];
    t.extend(shape_triples(&s));
    if let Some(d) = &ds.description {
        t.push(Triple::new(
            s.clone(),
            nn(&format!("{DCT}description")),
            Literal::new_simple_literal(d.clone()),
        ));
    }
    // The retention policy sits on the root node (LDES §4.4: "A retention
    // policy will be described on the root node"), as an IRI (Server Primer
    // §6.1.1) whose description travels with every document that names it —
    // a policy IRI "without further statements in the current page" would
    // mean the view keeps no members at all.
    if let Some(p) = policy {
        let view = nn(&view_node);
        let pol = nn(&format!("{}#retention", s.as_str()));
        t.push(Triple::new(
            view.clone(),
            nn(&format!("{RDF}type")),
            nn(&format!("{LDES}EventSource")),
        ));
        t.push(Triple::new(
            view,
            nn(&format!("{LDES}retentionPolicy")),
            pol.clone(),
        ));
        t.push(Triple::new(
            pol.clone(),
            nn(&format!("{RDF}type")),
            nn(&format!("{LDES}RetentionPolicy")),
        ));
        let duration = |v: &str| Literal::new_typed_literal(v, nn(&format!("{XSD}duration")));
        if let Some(d) = &p.full_log_duration {
            t.push(Triple::new(
                pol.clone(),
                nn(&format!("{LDES}fullLogDuration")),
                duration(d),
            ));
        }
        if let Some(n) = p.version_amount {
            t.push(Triple::new(
                pol.clone(),
                nn(&format!("{LDES}versionAmount")),
                Literal::new_typed_literal(n.to_string(), nn(&format!("{XSD}integer"))),
            ));
        }
        if let Some(d) = &p.version_duration {
            t.push(Triple::new(
                pol.clone(),
                nn(&format!("{LDES}versionDuration")),
                duration(d),
            ));
        }
        if let Some(d) = &p.version_delete_duration {
            t.push(Triple::new(
                pol.clone(),
                nn(&format!("{LDES}versionDeleteDuration")),
                duration(d),
            ));
        }
        if let Some(at) = &p.starting_from {
            t.push(Triple::new(
                pol,
                nn(&format!("{LDES}startingFrom")),
                Literal::new_typed_literal(at.clone(), nn(&format!("{XSD}dateTime"))),
            ));
        }
    }
    t
}

pub(crate) const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// A member as triples: the version object carries the entity's properties
/// (re-subjected from the entity to the member IRI), `dct:isVersionOf` the
/// entity and `dct:created`; tombstones are typed `as:Delete` (the declared
/// `ldes:versionDeleteObject`) and `ots:Tombstone`. Blank nodes are labelled
/// per member: two versions of one entity on a page share stored labels, and
/// must not share nodes in the document.
fn member_triples(base: &str, m: &Member) -> Vec<Triple> {
    let mi = nn(&member_iri(base, &m.dataset_id, m.id));
    let entity = nn(&m.entity_iri);
    let mut out = vec![
        Triple::new(mi.clone(), nn(&format!("{DCT}isVersionOf")), entity.clone()),
        Triple::new(
            mi.clone(),
            nn(&format!("{DCT}created")),
            Literal::new_typed_literal(m.created_at.clone(), nn(&format!("{XSD}dateTime"))),
        ),
    ];
    if m.deleted {
        out.push(Triple::new(
            mi.clone(),
            nn(&format!("{RDF}type")),
            nn(AS_DELETE),
        ));
        out.push(Triple::new(mi, nn(&format!("{RDF}type")), nn(TOMBSTONE)));
        return out;
    }
    let relabel = |b: &BlankNode| BlankNode::new_unchecked(format!("m{}_{}", m.id, b.as_str()));
    let parser = RdfParser::from_format(RdfFormat::NTriples);
    for q in parser.for_reader(m.ntriples.as_bytes()).flatten() {
        let subject = match q.subject {
            NamedOrBlankNode::NamedNode(ref n) if n.as_str() == m.entity_iri => {
                NamedOrBlankNode::NamedNode(mi.clone())
            }
            NamedOrBlankNode::BlankNode(ref b) => NamedOrBlankNode::BlankNode(relabel(b)),
            other => other,
        };
        let object = match q.object {
            Term::BlankNode(ref b) => Term::BlankNode(relabel(b)),
            other => other,
        };
        out.push(Triple::new(subject, q.predicate, object));
    }
    out
}

/// A relation out of `from` to `to`, typed and bounded on `dct:created` when
/// `bound` is given, a plain `tree:Relation` otherwise. Relation blank nodes
/// are numbered by the caller so a page renders to the same bytes each time.
fn relation(
    triples: &mut Vec<Triple>,
    label: &mut u32,
    from: &NamedNode,
    to: &NamedNode,
    bound: Option<(&str, &str)>,
) {
    let rel = BlankNode::new_unchecked(format!("rel{label}"));
    *label += 1;
    triples.push(Triple::new(
        from.clone(),
        nn(&format!("{TREE}relation")),
        rel.clone(),
    ));
    triples.push(Triple::new(
        rel.clone(),
        nn(&format!("{TREE}node")),
        to.clone(),
    ));
    let Some((kind, value)) = bound else {
        triples.push(Triple::new(
            rel,
            nn(&format!("{RDF}type")),
            nn(&format!("{TREE}Relation")),
        ));
        return;
    };
    triples.push(Triple::new(
        rel.clone(),
        nn(&format!("{RDF}type")),
        nn(&format!("{TREE}{kind}")),
    ));
    triples.push(Triple::new(
        rel.clone(),
        nn(&format!("{TREE}path")),
        nn(&format!("{DCT}created")),
    ));
    triples.push(Triple::new(
        rel,
        nn(&format!("{TREE}value")),
        Literal::new_typed_literal(value, nn(&format!("{XSD}dateTime"))),
    ));
}

/// Which stream document a request asks for.
enum Doc {
    Stream,
    Node(u64),
    Member(i64),
}

/// Render `doc` on the blocking pool behind the load gate.
async fn serve(
    state: AppState,
    user: Option<AuthenticatedUser>,
    dataset_id: String,
    headers: HeaderMap,
    doc: Doc,
) -> Result<Response, ApiErr> {
    let Some(slot) = try_enter(&dataset_id) else {
        return Ok(too_busy());
    };
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let ds = visible_dataset(&state, user.as_ref(), &dataset_id)?;
        let cfg = enabled_stream(&state, &dataset_id)?;
        match doc {
            Doc::Stream => stream_doc(&state, &ds, &cfg, &headers),
            Doc::Node(0) => root_doc(&state, &ds, &cfg, &headers),
            Doc::Node(n) => node_doc(&state, &ds, &cfg, n, &headers),
            Doc::Member(id) => member_doc(&state, &ds, &cfg, id, &headers),
        }
    })
    .await
    .map_err(e500)?
}

/// GET /api/datasets/:id/ldes — the event stream description.
pub async fn get_stream(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    let user = user.map(|Extension(u)| u);
    serve(state, user, dataset_id, headers, Doc::Stream).await
}

/// GET /api/datasets/:id/ldes/nodes/:n — the root node (0) or a fragment.
pub async fn get_node(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, n)): Path<(String, u64)>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    let user = user.map(|Extension(u)| u);
    serve(state, user, dataset_id, headers, Doc::Node(n)).await
}

/// GET /api/datasets/:id/ldes/members/:m — one member, dereferenced (TREE
/// member extraction falls back to dereferencing a member IRI it found no
/// quads for). Members are immutable; one removed by retention is 404.
pub async fn get_member(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, id)): Path<(String, i64)>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    let user = user.map(|Extension(u)| u);
    serve(state, user, dataset_id, headers, Doc::Member(id)).await
}

fn stream_doc(
    state: &AppState,
    ds: &Dataset,
    cfg: &store::StreamConfig,
    headers: &HeaderMap,
) -> Result<Response, ApiErr> {
    let policy = store::retention(&state.auth_db, &ds.id).map_err(e500)?;
    let triples = stream_description(&state.base_url, ds, cfg, policy.as_ref());
    render(headers, &triples, "no-cache")
}

/// The root node: no members, a bounded pair of relations to every sealed
/// node that still has members, and a lower-bounded relation to the tail.
fn root_doc(
    state: &AppState,
    ds: &Dataset,
    cfg: &store::StreamConfig,
    headers: &HeaderMap,
) -> Result<Response, ApiErr> {
    let base = state.base_url.as_str();
    let layout = Layout::load(state, &ds.id, cfg.page_size)?;
    let policy = store::retention(&state.auth_db, &ds.id).map_err(e500)?;
    let root = nn(&node_iri(base, &ds.id, 0));
    let mut triples = stream_description(base, ds, cfg, policy.as_ref());
    triples.push(Triple::new(
        root.clone(),
        nn(&format!("{RDF}type")),
        nn(&format!("{TREE}Node")),
    ));
    let mut label = 0;
    for (i, s) in layout.sealed.iter().enumerate() {
        if s.members == 0 {
            continue; // 410 Gone: nothing links to it (Primer §5.1)
        }
        let to = nn(&node_iri(base, &ds.id, i as u64 + 1));
        let lower = s.min_created_at.as_deref();
        let upper = s.max_created_at.as_deref();
        if lower.is_none() && upper.is_none() {
            relation(&mut triples, &mut label, &root, &to, None);
        }
        if let Some(v) = lower {
            let b = Some(("GreaterThanOrEqualToRelation", v));
            relation(&mut triples, &mut label, &root, &to, b);
        }
        if let Some(v) = upper {
            let b = Some(("LessThanOrEqualToRelation", v));
            relation(&mut triples, &mut label, &root, &to, b);
        }
    }
    // The tail: its earliest member, or — when retention emptied it — the
    // newest timestamp ever published, below which no later member falls.
    let start = layout.tail_start();
    let floor = match layout.tail_floor(state, &ds.id, start)? {
        Some(v) => Some(v),
        None => store::last_created_at(&state.auth_db, &ds.id).map_err(e500)?,
    };
    let to = nn(&node_iri(base, &ds.id, start));
    let b = floor
        .as_deref()
        .map(|v| ("GreaterThanOrEqualToRelation", v));
    relation(&mut triples, &mut label, &root, &to, b);
    render(headers, &triples, "no-cache")
}

fn node_doc(
    state: &AppState,
    ds: &Dataset,
    cfg: &store::StreamConfig,
    n: u64,
    headers: &HeaderMap,
) -> Result<Response, ApiErr> {
    let base = state.base_url.as_str();
    let dataset_id = ds.id.as_str();
    let layout = Layout::load(state, dataset_id, cfg.page_size)?;
    let pages = layout.pages();
    if n > pages {
        return Err((
            StatusCode::NOT_FOUND,
            format!("node {n} does not exist (the stream has {pages})"),
        ));
    }
    let sealed = layout.is_sealed(n);
    let members = if sealed {
        let s = &layout.sealed[n as usize - 1];
        let members = store::members_between(&state.auth_db, dataset_id, s.first_id, s.last_id)
            .map_err(e500)?;
        if members.is_empty() {
            // Server Primer §5.1: "For nodes that are no longer available,
            // respond with 410 Gone. Clients will treat such a page as
            // having no members and no relations."
            return Err((
                StatusCode::GONE,
                format!(
                    "node {n} has been compacted away by the stream's retention policy; \
                     the root node {} links every node that still has members",
                    node_iri(base, dataset_id, 0)
                ),
            ));
        }
        members
    } else {
        store::tail_page(
            &state.auth_db,
            dataset_id,
            layout.tail_after,
            n - layout.sealed_count(),
            cfg.page_size,
        )
        .map_err(e500)?
    };
    let policy = store::retention(&state.auth_db, dataset_id).map_err(e500)?;
    let stream = nn(&stream_iri(base, dataset_id));
    let node = nn(&node_iri(base, dataset_id, n));
    let mut triples = stream_description(base, ds, cfg, policy.as_ref());
    triples.push(Triple::new(
        node.clone(),
        nn(&format!("{RDF}type")),
        nn(&format!("{TREE}Node")),
    ));
    if sealed {
        // LDES §3.2: a client checks `<> ldes:immutable true` before the
        // Cache-Control header. A sealed node links nowhere: the root's
        // bounds on it describe everything reachable from it.
        triples.push(Triple::new(
            node.clone(),
            nn(&format!("{LDES}immutable")),
            Literal::new_typed_literal("true", nn(&format!("{XSD}boolean"))),
        ));
    } else if n < pages {
        // The tail chains forward; the bound covers every later tail page.
        let floor = layout.tail_floor(state, dataset_id, n + 1)?;
        let to = nn(&node_iri(base, dataset_id, n + 1));
        let b = floor
            .as_deref()
            .map(|v| ("GreaterThanOrEqualToRelation", v));
        relation(&mut triples, &mut 0, &node, &to, b);
    }
    for m in &members {
        triples.push(Triple::new(
            stream.clone(),
            nn(&format!("{TREE}member")),
            nn(&member_iri(base, dataset_id, m.id)),
        ));
        triples.extend(member_triples(base, m));
    }
    let cache = if sealed {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    render(headers, &triples, cache)
}

fn member_doc(
    state: &AppState,
    ds: &Dataset,
    _cfg: &store::StreamConfig,
    id: i64,
    headers: &HeaderMap,
) -> Result<Response, ApiErr> {
    let base = state.base_url.as_str();
    let m = store::member(&state.auth_db, &ds.id, id)
        .map_err(e500)?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                format!(
                    "member {id} is not in this stream (or was removed by its retention policy)"
                ),
            )
        })?;
    let mut triples = vec![Triple::new(
        nn(&stream_iri(base, &ds.id)),
        nn(&format!("{TREE}member")),
        nn(&member_iri(base, &ds.id, m.id)),
    )];
    triples.extend(member_triples(base, &m));
    render(headers, &triples, "public, max-age=31536000, immutable")
}
