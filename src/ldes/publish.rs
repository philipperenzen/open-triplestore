//! The published side of LDES: `GET /api/datasets/:id/ldes` (the
//! `ldes:EventStream`) and `GET /api/datasets/:id/ldes/nodes/:n` (fragments),
//! plus `PUT /api/datasets/:id/ldes` to enable the stream.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use oxigraph::io::{RdfFormat, RdfParser, RdfSerializer};
use oxigraph::model::{Literal, NamedNode, NamedOrBlankNode, Triple};
use serde::Deserialize;

use super::store::{self, Member};
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
}

/// PUT /api/datasets/:id/ldes — enable (or disable) the dataset's stream.
/// Enabling a stream that has no members yet publishes every entity of the
/// dataset's graphs as its first members.
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
    let page_size = body.page_size.unwrap_or(100).clamp(1, 10_000);
    store::set_stream(&state.auth_db, &dataset_id, body.enabled, page_size).map_err(e500)?;
    let mut seeded = 0;
    if body.enabled && store::member_count(&state.auth_db, &dataset_id).map_err(e500)? == 0 {
        let graphs = state
            .auth_db
            .list_dataset_graphs(&dataset_id)
            .map_err(e500)?;
        let st = state.clone();
        let id = dataset_id.clone();
        seeded =
            tokio::task::spawn_blocking(move || super::capture::publish_all(&st, &id, &graphs))
                .await
                .map_err(e500)?;
    }
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "enabled": body.enabled,
        "page_size": page_size,
        "stream": stream_iri(&state.base_url, &dataset_id),
        "members_seeded": seeded,
        "members": store::member_count(&state.auth_db, &dataset_id).map_err(e500)?,
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

fn stream_description(base: &str, ds: &Dataset, first_node: &str) -> Vec<Triple> {
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
        Triple::new(s.clone(), nn(&format!("{TREE}view")), nn(first_node)),
    ];
    if let Some(d) = &ds.description {
        t.push(Triple::new(
            s,
            nn(&format!("{DCT}description")),
            Literal::new_simple_literal(d.clone()),
        ));
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
    enabled_stream(&state, &dataset_id)?;
    let base = state.base_url.as_str();
    let triples = stream_description(base, &ds, &node_iri(base, &dataset_id, 1));
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
    let total = store::member_count(&state.auth_db, &dataset_id).map_err(e500)?;
    let pages = total.div_ceil(cfg.page_size).max(1);
    if n > pages {
        return Err((
            StatusCode::NOT_FOUND,
            format!("node {n} does not exist (the stream has {pages})"),
        ));
    }
    let members =
        store::members_page(&state.auth_db, &dataset_id, n, cfg.page_size).map_err(e500)?;
    let stream = nn(&stream_iri(base, &dataset_id));
    let node = nn(&node_iri(base, &dataset_id, n));
    let mut triples = stream_description(base, &ds, &node_iri(base, &dataset_id, 1));
    triples.push(Triple::new(
        node.clone(),
        nn(&format!("{RDF}type")),
        nn(&format!("{TREE}Node")),
    ));
    // Relation to the next fragment: members there are created at or after
    // its first member's timestamp.
    if n < pages {
        let next_first = store::members_page(&state.auth_db, &dataset_id, n + 1, 1)
            .map_err(e500)?
            .into_iter()
            .next();
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
            nn(&node_iri(base, &dataset_id, n + 1)),
        ));
        if let Some(m) = next_first {
            triples.push(Triple::new(
                rel,
                nn(&format!("{TREE}value")),
                Literal::new_typed_literal(m.created_at, nn(&format!("{XSD}dateTime"))),
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
    let cache = if n < pages {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    render(&headers, &triples, cache)
}
