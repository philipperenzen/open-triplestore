//! The LDES client: `POST /api/ldes/sync` follows a remote stream's
//! `tree:view` and every `tree:relation`, collects the members, keeps the
//! newest version per entity and materialises those into a graph of a local
//! dataset. A bookmark per `(dataset, url)` makes later runs incremental.
//! Every fetch goes through `crate::remote` (allowlist, timeout).

use std::collections::{HashMap, HashSet, VecDeque};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{GraphNameRef, NamedNode, NamedOrBlankNode, Term};
use oxigraph::sparql::{QueryResults, SparqlEvaluator};
use oxigraph::store::Store;
use serde::{Deserialize, Serialize};

use super::{DCT, LDES, TOMBSTONE, TREE};
use crate::auth::middleware::AuthenticatedUser;
use crate::server::content_negotiation::parse_rdf_content_type;
use crate::server::AppState;

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
    pub nodes_visited: usize,
    /// Nodes that answered `410 Gone` — compacted away by the publisher's
    /// retention policy and processed as empty (LDES §3.3).
    pub nodes_gone: usize,
    pub members_seen: usize,
    pub members_skipped_older: usize,
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
/// (`ldes:DurationAgoPolicy` / `ldes:LatestVersionSubset`) folded into the
/// same fields.
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
        "SELECT ?p ?fl ?va ?vd ?vdd ?sf ?dur ?amt WHERE {{ \
           {{ ?x <{LDES}retentionPolicy> ?p }} UNION {{ ?v <{TREE}viewDescription>/<{LDES}retentionPolicy> ?p }} \
           OPTIONAL {{ ?p <{LDES}fullLogDuration> ?fl }} \
           OPTIONAL {{ ?p <{LDES}versionAmount> ?va }} \
           OPTIONAL {{ ?p <{LDES}versionDuration> ?vd }} \
           OPTIONAL {{ ?p <{LDES}versionDeleteDuration> ?vdd }} \
           OPTIONAL {{ ?p <{LDES}startingFrom> ?sf }} \
           OPTIONAL {{ ?p a <{LDES}DurationAgoPolicy> ; <{TREE}value> ?dur }} \
           OPTIONAL {{ ?p a <{LDES}LatestVersionSubset> ; <{LDES}amount> ?amt }} \
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
    }
    out
}

/// One member as read from a fragment.
struct RemoteMember {
    entity: String,
    created: String,
    deleted: bool,
    /// The member's own triples, re-subjected to the entity (N-Triples).
    ntriples: String,
}

fn fetch_into_store(url: &str) -> anyhow::Result<Store> {
    let (ct, body) = crate::remote::get_rdf_blocking(url)?;
    let fmt = parse_rdf_content_type(&ct).unwrap_or(RdfFormat::Turtle);
    let store = Store::new()?;
    let parser = RdfParser::from_format(fmt)
        .with_base_iri(url)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    store.load_from_reader(parser, body.as_bytes())?;
    Ok(store)
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

fn iris(store: &Store, query: &str, var: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(QueryResults::Solutions(sols)) = run(store, query) {
        for s in sols.flatten() {
            if let Some(Term::NamedNode(n)) = s.get(var) {
                out.push(n.as_str().to_string());
            }
        }
    }
    out
}

/// The stream's declared paths (defaults per the LDES spec's usual practice).
fn stream_paths(store: &Store) -> (String, String) {
    let q = format!(
        "SELECT ?tp ?vp WHERE {{ ?s a <{LDES}EventStream> . OPTIONAL {{ ?s <{LDES}timestampPath> ?tp }} OPTIONAL {{ ?s <{LDES}versionOfPath> ?vp }} }} LIMIT 1"
    );
    let (mut tp, mut vp) = (format!("{DCT}created"), format!("{DCT}isVersionOf"));
    if let Some(QueryResults::Solutions(mut sols)) = run(store, &q) {
        if let Some(Ok(s)) = sols.next() {
            if let Some(Term::NamedNode(n)) = s.get("tp") {
                tp = n.as_str().to_string();
            }
            if let Some(Term::NamedNode(n)) = s.get("vp") {
                vp = n.as_str().to_string();
            }
        }
    }
    (tp, vp)
}

/// The members declared in a fragment, with their descriptions re-subjected.
fn members_of(store: &Store, tp: &str, vp: &str) -> Vec<RemoteMember> {
    let q = format!("SELECT ?m ?e ?t WHERE {{ ?c <{TREE}member> ?m . ?m <{vp}> ?e ; <{tp}> ?t }}");
    let mut out = Vec::new();
    let Some(QueryResults::Solutions(sols)) = run(store, &q) else {
        return out;
    };
    for s in sols.flatten() {
        let (Some(Term::NamedNode(m)), Some(Term::NamedNode(e)), Some(Term::Literal(t))) =
            (s.get("m"), s.get("e"), s.get("t"))
        else {
            continue;
        };
        let mut deleted = false;
        let mut nt = String::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut queue: Vec<NamedOrBlankNode> = vec![NamedOrBlankNode::NamedNode(m.clone())];
        while let Some(subj) = queue.pop() {
            for q in store
                .quads_for_pattern(
                    Some(subj.as_ref()),
                    None,
                    None,
                    Some(GraphNameRef::DefaultGraph),
                )
                .flatten()
            {
                let p = q.predicate.as_str();
                if p == vp || p == tp {
                    continue;
                }
                if p == "http://www.w3.org/1999/02/22-rdf-syntax-ns#type" {
                    if let Term::NamedNode(o) = &q.object {
                        if o.as_str() == TOMBSTONE {
                            deleted = true;
                            continue;
                        }
                    }
                }
                if let Term::BlankNode(b) = &q.object {
                    if seen.insert(b.as_str().to_string()) {
                        queue.push(NamedOrBlankNode::BlankNode(b.clone()));
                    }
                }
                let subject = match &q.subject {
                    NamedOrBlankNode::NamedNode(n) if n.as_str() == m.as_str() => {
                        NamedOrBlankNode::NamedNode(e.clone())
                    }
                    other => other.clone(),
                };
                nt.push_str(&format!("{subject} {} {} .\n", q.predicate, q.object));
            }
        }
        out.push(RemoteMember {
            entity: e.as_str().to_string(),
            created: t.value().to_string(),
            deleted,
            ntriples: nt,
        });
    }
    out
}

/// Follow the stream from `url`, applying newer members into `graph_iri`.
pub fn sync(
    state: &AppState,
    url: &str,
    dataset_id: &str,
    graph_iri: &str,
) -> anyhow::Result<SyncReport> {
    NamedNode::new(graph_iri).map_err(|e| anyhow::anyhow!("graph IRI: {e}"))?;
    if !crate::remote::is_allowed(url) {
        anyhow::bail!(
            "{}",
            crate::remote::RemoteError::NotAllowed(url.to_string())
        );
    }
    let bookmark = super::store::sync_bookmark(&state.auth_db, dataset_id, url)?;
    let mut report = SyncReport {
        url: url.to_string(),
        dataset_id: dataset_id.to_string(),
        graph_iri: graph_iri.to_string(),
        ..Default::default()
    };

    // Crawl: the root document, then every tree:view / tree:node reachable.
    let mut queue: VecDeque<String> = VecDeque::from([url.to_string()]);
    let mut visited: HashSet<String> = HashSet::new();
    let mut newest: HashMap<String, RemoteMember> = HashMap::new();
    let mut paths: Option<(String, String)> = None;
    while let Some(doc) = queue.pop_front() {
        if !visited.insert(doc.clone()) {
            continue;
        }
        if visited.len() > 10_000 {
            anyhow::bail!("stream has more than 10 000 nodes; refusing to follow further");
        }
        let store = match fetch_into_store(&doc) {
            Ok(s) => s,
            // LDES §3.3: "A client MUST process 410 Gone as a page with an
            // empty set of relations and an empty set of members."
            Err(e)
                if matches!(
                    e.downcast_ref::<crate::remote::RemoteError>(),
                    Some(crate::remote::RemoteError::Status { status: 410, .. })
                ) =>
            {
                report.nodes_gone += 1;
                continue;
            }
            Err(e) => return Err(e),
        };
        report.nodes_visited += 1;
        let (tp, vp) = paths.get_or_insert_with(|| stream_paths(&store)).clone();
        // The root node's retention policy is context the client keeps
        // (LDES §3.2): a bookmark older than the publisher's window means
        // members may have been compacted before this mirror saw them.
        if report.retention_policy.is_none() {
            if let Some(policy) = retention_of(&store) {
                if let (Some(b), Some(d)) = (
                    bookmark.as_deref(),
                    policy
                        .full_log_duration
                        .as_deref()
                        .and_then(|d| super::store::parse_xsd_duration(d).ok()),
                ) {
                    if let Ok(at) = chrono::DateTime::parse_from_rfc3339(b) {
                        if at.with_timezone(&chrono::Utc) < chrono::Utc::now() - d {
                            report.warnings.push(format!(
                                "the bookmark {b} predates the publisher's retention window \
                                 ({}): members created between them may have been compacted \
                                 away before this sync",
                                policy.full_log_duration.as_deref().unwrap_or("?")
                            ));
                        }
                    }
                }
                report.retention_policy = Some(policy);
            }
        }
        for m in members_of(&store, &tp, &vp) {
            report.members_seen += 1;
            if bookmark.as_deref().is_some_and(|b| m.created.as_str() <= b) {
                report.members_skipped_older += 1;
                continue;
            }
            match newest.get(&m.entity) {
                Some(cur) if cur.created >= m.created => {}
                _ => {
                    newest.insert(m.entity.clone(), m);
                }
            }
        }
        for next in iris(
            &store,
            &format!("SELECT ?n WHERE {{ ?s <{TREE}view> ?n }}"),
            "n",
        )
        .into_iter()
        .chain(iris(
            &store,
            &format!("SELECT ?n WHERE {{ ?r <{TREE}node> ?n }}"),
            "n",
        )) {
            if !visited.contains(&next) {
                queue.push_back(next);
            }
        }
    }

    // Materialise: newest version per entity replaces what the graph holds.
    let mut entities: Vec<&RemoteMember> = newest.values().collect();
    entities.sort_by(|a, b| {
        a.created
            .cmp(&b.created)
            .then_with(|| a.entity.cmp(&b.entity))
    });
    let mut last_ts = bookmark.clone();
    for m in entities {
        let e = crate::store::escape_sparql_iri(&m.entity);
        state.store.update(&format!(
            "DELETE WHERE {{ GRAPH <{graph_iri}> {{ <{e}> ?p ?o }} }}"
        ))?;
        if m.deleted {
            report.entities_deleted += 1;
        } else {
            if !m.ntriples.is_empty() {
                state.store.update(&format!(
                    "INSERT DATA {{ GRAPH <{graph_iri}> {{\n{}\n}} }}",
                    m.ntriples
                ))?;
            }
            report.entities_updated += 1;
        }
        if last_ts.as_deref().is_none_or(|l| m.created.as_str() > l) {
            last_ts = Some(m.created.clone());
        }
    }
    report.last_timestamp = last_ts.clone();
    super::store::set_sync_bookmark(
        &state.auth_db,
        dataset_id,
        url,
        last_ts.as_deref(),
        (report.entities_updated + report.entities_deleted) as u64,
    )?;
    Ok(report)
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
    crate::auth::dataset_graph::gate_dataset_graph_target(
        &state.store,
        &state.auth_db,
        &state.base_url,
        &body.dataset_id,
        &body.graph_iri,
        user.is_admin(),
    )
    .map_err(|m| (StatusCode::FORBIDDEN, m))?;
    if !crate::remote::is_allowed(&body.url) {
        return Err((
            StatusCode::FORBIDDEN,
            crate::remote::RemoteError::NotAllowed(body.url.clone()).to_string(),
        ));
    }
    // The target graph belongs to the dataset (registered if it is new), so
    // the dataset's own stream — if any — and its history see the sync.
    let _ = state
        .auth_db
        .add_dataset_graph(&body.dataset_id, &body.graph_iri);
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
