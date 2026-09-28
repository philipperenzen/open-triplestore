//! The published side of LDES: `GET /api/datasets/:id/ldes` (the
//! `ldes:EventStream`) and `GET /api/datasets/:id/ldes/nodes/:n` (fragments),
//! plus `PUT /api/datasets/:id/ldes` to enable the stream and declare its
//! retention policy.
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
//! gone answers `410 Gone` (LDES Server Primer §5.1), with every relation
//! and `tree:view` pointing past it.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::{Literal, NamedNode, NamedOrBlankNode, Triple};
use serde::Deserialize;

use super::store::{self, Member, RetentionPolicy, SealedNode};
use super::{member_iri, node_iri, stream_iri, DCT, LDES, OTS, TOMBSTONE, TREE, XSD};
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

/// Serialise `triples` (default graph) in the negotiated format, with the
/// stream's prefixes for readable Turtle.
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
    let mut resp = (StatusCode::OK, buf).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(gf.content_type()),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
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

    fn has_members(&self, n: u64) -> bool {
        if self.is_sealed(n) {
            self.sealed[n as usize - 1].members > 0
        } else {
            let k = n - self.sealed_count();
            k >= 1 && (k - 1) * self.page_size < self.tail_count
        }
    }

    /// The first node that still has members — the last node when none has.
    fn view(&self) -> u64 {
        let pages = self.pages();
        (1..=pages).find(|n| self.has_members(*n)).unwrap_or(pages)
    }

    /// Where the relation out of `n` points: the next node with members, or
    /// the last node (the mutable tail is always a valid target).
    fn next(&self, n: u64) -> Option<u64> {
        let pages = self.pages();
        if n >= pages {
            return None;
        }
        Some(
            (n + 1..pages)
                .find(|m| self.has_members(*m))
                .unwrap_or(pages),
        )
    }
}

fn stream_description(
    base: &str,
    ds: &Dataset,
    view_node: &str,
    policy: Option<&RetentionPolicy>,
) -> Vec<Triple> {
    let s = nn(&stream_iri(base, &ds.id));
    let mut t = vec![
        Triple::new(
            s.clone(),
            nn(&format!("{}type", crate::ldes::publish::RDF)),
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
        Triple::new(s.clone(), nn(&format!("{TREE}view")), nn(view_node)),
    ];
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
        let view = nn(view_node);
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
/// entity and `dct:created`; tombstones are typed `ots:Tombstone`.
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
        out.push(Triple::new(mi, nn(&format!("{RDF}type")), nn(TOMBSTONE)));
        return out;
    }
    let parser = RdfParser::from_format(RdfFormat::NTriples);
    for q in parser.for_reader(m.ntriples.as_bytes()).flatten() {
        let subject = match q.subject {
            NamedOrBlankNode::NamedNode(ref n) if n.as_str() == m.entity_iri => {
                NamedOrBlankNode::NamedNode(mi.clone())
            }
            other => other,
        };
        out.push(Triple::new(subject, q.predicate, q.object));
    }
    out
}

/// GET /api/datasets/:id/ldes — the event stream description.
pub async fn get_stream(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    let ds = visible_dataset(&state, user.as_deref(), &dataset_id)?;
    let cfg = enabled_stream(&state, &dataset_id)?;
    let base = state.base_url.as_str();
    let layout = Layout::load(&state, &dataset_id, cfg.page_size)?;
    let policy = store::retention(&state.auth_db, &dataset_id).map_err(e500)?;
    let triples = stream_description(
        base,
        &ds,
        &node_iri(base, &dataset_id, layout.view()),
        policy.as_ref(),
    );
    render(&headers, &triples, "no-cache")
}

/// GET /api/datasets/:id/ldes/nodes/:n — one fragment.
pub async fn get_node(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path((dataset_id, n)): Path<(String, u64)>,
    headers: HeaderMap,
) -> Result<Response, ApiErr> {
    let ds = visible_dataset(&state, user.as_deref(), &dataset_id)?;
    let cfg = enabled_stream(&state, &dataset_id)?;
    if n == 0 {
        return Err((
            StatusCode::NOT_FOUND,
            "nodes are numbered from 1".to_string(),
        ));
    }
    let base = state.base_url.as_str();
    let layout = Layout::load(&state, &dataset_id, cfg.page_size)?;
    let pages = layout.pages();
    if n > pages {
        return Err((
            StatusCode::NOT_FOUND,
            format!("node {n} does not exist (the stream has {pages})"),
        ));
    }
    let members = if layout.is_sealed(n) {
        let s = &layout.sealed[n as usize - 1];
        let members = store::members_between(&state.auth_db, &dataset_id, s.first_id, s.last_id)
            .map_err(e500)?;
        if members.is_empty() {
            // Server Primer §5.1: "For nodes that are no longer available,
            // respond with 410 Gone. Clients will treat such a page as
            // having no members and no relations."
            return Err((
                StatusCode::GONE,
                format!(
                    "node {n} has been compacted away by the stream's retention policy; \
                     the stream continues at node {}",
                    layout.next(n).unwrap_or(pages)
                ),
            ));
        }
        members
    } else {
        store::tail_page(
            &state.auth_db,
            &dataset_id,
            layout.tail_after,
            n - layout.sealed_count(),
            cfg.page_size,
        )
        .map_err(e500)?
    };
    let policy = store::retention(&state.auth_db, &dataset_id).map_err(e500)?;
    let stream = nn(&stream_iri(base, &dataset_id));
    let node = nn(&node_iri(base, &dataset_id, n));
    let mut triples = stream_description(
        base,
        &ds,
        &node_iri(base, &dataset_id, layout.view()),
        policy.as_ref(),
    );
    triples.push(Triple::new(
        node.clone(),
        nn(&format!("{RDF}type")),
        nn(&format!("{TREE}Node")),
    ));
    let immutable = n < pages;
    if immutable {
        // LDES §3.2: a client checks `<> ldes:immutable true` before the
        // Cache-Control header.
        triples.push(Triple::new(
            node.clone(),
            nn(&format!("{LDES}immutable")),
            Literal::new_typed_literal("true", nn(&format!("{XSD}boolean"))),
        ));
    }
    // Relation to the next fragment that still has members: its members
    // were created at or after the bound recorded when this page sealed —
    // a lower bound for every later member, so it stays valid when the
    // pages in between are compacted away.
    if let Some(next) = layout.next(n) {
        let value = if layout.is_sealed(n) {
            Some(layout.sealed[n as usize - 1].next_created_at.clone())
        } else {
            store::tail_page(
                &state.auth_db,
                &dataset_id,
                layout.tail_after,
                n + 1 - layout.sealed_count(),
                1,
            )
            .map_err(e500)?
            .into_iter()
            .next()
            .map(|m| m.created_at)
        };
        let rel = oxigraph::model::BlankNode::default();
        triples.push(Triple::new(
            node.clone(),
            nn(&format!("{TREE}relation")),
            rel.clone(),
        ));
        triples.push(Triple::new(
            rel.clone(),
            nn(&format!("{RDF}type")),
            nn(&format!("{TREE}GreaterThanOrEqualToRelation")),
        ));
        triples.push(Triple::new(
            rel.clone(),
            nn(&format!("{TREE}path")),
            nn(&format!("{DCT}created")),
        ));
        triples.push(Triple::new(
            rel.clone(),
            nn(&format!("{TREE}node")),
            nn(&node_iri(base, &dataset_id, next)),
        ));
        if let Some(v) = value {
            triples.push(Triple::new(
                rel,
                nn(&format!("{TREE}value")),
                Literal::new_typed_literal(v, nn(&format!("{XSD}dateTime"))),
            ));
        }
    }
    for m in &members {
        triples.push(Triple::new(
            stream.clone(),
            nn(&format!("{TREE}member")),
            nn(&member_iri(base, &dataset_id, m.id)),
        ));
        triples.extend(member_triples(base, m));
    }
    let cache = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    render(&headers, &triples, cache)
}
