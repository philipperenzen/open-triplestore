//! Commit / provenance trail for triplestore writes.
//!
//! Every data mutation (draft save, version upload, branch creation, raw SPARQL
//! update) can record a commit: a `prov:Activity` describing who changed what,
//! when, and why. Commits are stored as RDF in the named graph
//! `<urn:system:commit-log>`, so the trail is itself queryable linked data and
//! is surfaced per model / vocabulary / dataset in the frontends.

use oxigraph::model::*;
use oxigraph::sparql::QueryResults;
use serde::{Deserialize, Serialize};

use crate::store::TripleStore;

/// Named graph holding the commit trail.
pub const COMMIT_GRAPH: &str = "urn:system:commit-log";

const VER: &str = "urn:system:vocab/";
const DCT: &str = "http://purl.org/dc/terms/";
const PROV: &str = "http://www.w3.org/ns/prov#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// What kind of resource a commit touched. Stored as `ver:kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommitKind {
    DataModel,
    Vocabulary,
    Dataset,
    Sparql,
    Shapes,
    /// A Graph Store Protocol PUT / POST / DELETE.
    GraphStore,
    /// A bulk import (files → dataset graphs).
    Import,
    /// A backup restore — the whole store was replaced.
    Backup,
    /// A datasource, RML mapping or materialisation run.
    Source,
}

impl CommitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CommitKind::DataModel => "data-model",
            CommitKind::Vocabulary => "vocabulary",
            CommitKind::Dataset => "dataset",
            CommitKind::Sparql => "sparql",
            CommitKind::Shapes => "shapes",
            CommitKind::GraphStore => "graph-store",
            CommitKind::Import => "import",
            CommitKind::Backup => "backup",
            CommitKind::Source => "source",
        }
    }
}

/// Record a commit for a write made outside the model / vocabulary / shape
/// editors. Best-effort: a failure is logged and never fails the write the
/// caller already made. The module doc promised "every data mutation"; until
/// this existed, Graph Store writes, imports, dataset versioning and backup
/// restores left no trace, while `GET /api/datasets/:id/commits` presented the
/// log as the dataset's history.
#[allow(clippy::too_many_arguments)]
pub fn record(
    store: &TripleStore,
    base_url: &str,
    kind: CommitKind,
    message: impl Into<String>,
    actor_user_id: Option<&str>,
    subject_iri: Option<String>,
    affected_graphs: Vec<String>,
    added: usize,
    removed: usize,
    version: Option<String>,
) {
    let mut rec = CommitRecord::new(kind, message);
    rec.actor_iri = actor_user_id.map(|u| format!("{}/users/{u}", base_url.trim_end_matches('/')));
    rec.subject_iri = subject_iri;
    rec.affected_graphs = affected_graphs;
    rec.added = added;
    rec.removed = removed;
    rec.version = version;
    if let Err(e) = insert_commit(store, base_url, &rec) {
        tracing::warn!("failed to record {} commit: {e}", rec.kind.as_str());
    }
}

/// A commit whose identity exists before its write runs. The handler puts
/// [`CommitStamp::commit_iri`] into the write's change-log context, so every
/// row the write produces carries it; recording the commit afterwards with
/// [`record_stamped`] then points the commit at exactly those rows —
/// `ver:seqFrom` / `ver:seqTo` and the net counts (delta note §7). It is also
/// how the handler reads back what its own write changed
/// (`ChangeLog::rows_for_commit`).
#[derive(Debug, Clone)]
pub struct CommitStamp {
    pub commit_id: String,
    pub commit_iri: String,
}

impl CommitStamp {
    pub fn new(base_url: &str) -> Self {
        let commit_id = uuid::Uuid::new_v4().to_string();
        let commit_iri = format!("{}/commit/{commit_id}", base_url.trim_end_matches('/'));
        Self {
            commit_id,
            commit_iri,
        }
    }
}

/// As [`record`], for a write that ran under `stamp`: the commit takes the
/// stamp's id and, when the change log recorded the write, the sequence
/// range of its rows and their net counts. `added` / `removed` keep their
/// per-path meaning (delta note §8.5 Q4).
#[allow(clippy::too_many_arguments)]
pub fn record_stamped(
    store: &TripleStore,
    base_url: &str,
    stamp: &CommitStamp,
    kind: CommitKind,
    message: impl Into<String>,
    actor_user_id: Option<&str>,
    subject_iri: Option<String>,
    affected_graphs: Vec<String>,
    added: usize,
    removed: usize,
    version: Option<String>,
) {
    let mut rec = CommitRecord::new(kind, message);
    rec.commit_id = stamp.commit_id.clone();
    rec.actor_iri = actor_user_id.map(|u| format!("{}/users/{u}", base_url.trim_end_matches('/')));
    rec.subject_iri = subject_iri;
    rec.affected_graphs = affected_graphs;
    rec.added = added;
    rec.removed = removed;
    rec.version = version;
    stamp_sequence(store, &stamp.commit_iri, &mut rec);
    if let Err(e) = insert_commit(store, base_url, &rec) {
        tracing::warn!("failed to record {} commit: {e}", rec.kind.as_str());
    }
}

/// Fill a commit's sequence range and net counts from the rows its write
/// stamped; nothing when the log did not record the write. Net counts only
/// when every row knows its delta.
pub fn stamp_sequence(store: &TripleStore, commit_iri: &str, rec: &mut CommitRecord) {
    let rows = store.changes().rows_for_commit(commit_iri);
    let seqs: Vec<i64> = rows.iter().filter_map(|r| r.seq).collect();
    let (Some(from), Some(to)) = (seqs.iter().min(), seqs.iter().max()) else {
        return;
    };
    rec.seq_from = Some(*from);
    rec.seq_to = Some(*to);
    if rows
        .iter()
        .all(|r| r.extent != crate::store::changes::Extent::Unknown)
    {
        rec.net_added = Some(rows.iter().map(|r| r.added_n).sum());
        rec.net_removed = Some(rows.iter().map(|r| r.removed_n).sum());
    }
    store.changes().note_first_seq_commit(commit_iri);
}

/// A single commit in the trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitRecord {
    pub commit_id: String,
    pub kind: CommitKind,
    /// IRI of the actor (user). `None` for anonymous/system writes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_iri: Option<String>,
    /// Resolved username, filled in by the handler layer (not stored as a column).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_username: Option<String>,
    /// Resolved display name, filled in by the handler layer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_display_name: Option<String>,
    pub created_at: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// IRI of the model / vocabulary / dataset the commit belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject_iri: Option<String>,
    #[serde(default)]
    pub affected_graphs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub added: usize,
    pub removed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// First and last change-log sequence number of the commit's write
    /// (`ver:seqFrom` / `ver:seqTo`). Absent for a write the log did not
    /// record, and on every commit made before the log existed — those are
    /// ordered by `created_at`, later ones by `seq_to`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq_from: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq_to: Option<i64>,
    /// Quads the write actually added / removed, net, from its rows. Unlike
    /// `added` / `removed`, whose meaning differs per path, these always
    /// mean the same thing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_added: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_removed: Option<i64>,
}

impl CommitRecord {
    /// Build a commit with a fresh id and `created_at` of now (RFC3339).
    pub fn new(kind: CommitKind, message: impl Into<String>) -> Self {
        Self {
            commit_id: uuid::Uuid::new_v4().to_string(),
            kind,
            actor_iri: None,
            actor_username: None,
            actor_display_name: None,
            created_at: chrono::Utc::now().to_rfc3339(),
            message: message.into(),
            metadata: None,
            subject_iri: None,
            affected_graphs: Vec::new(),
            version: None,
            branch: None,
            added: 0,
            removed: 0,
            parent_revision: None,
            revision: None,
            seq_from: None,
            seq_to: None,
            net_added: None,
            net_removed: None,
        }
    }
}

/// SPARQL string-literal escaping. Delegates to the canonical
/// [`crate::store::escape_sparql_literal`] (handles `\ " \n \r \t`).
fn esc(s: &str) -> String {
    crate::store::escape_sparql_literal(s)
}

/// The plain value of a commit-log field: an IRI's string, a literal's
/// lexical form, a blank node's label. A triple term (RDF 1.2) has no plain
/// value, so it keeps its N-Triples form `<<( s p o )>>` instead of vanishing.
fn term_to_string(t: &Term) -> String {
    match t {
        Term::NamedNode(nn) => nn.as_str().to_string(),
        Term::Literal(lit) => lit.value().to_string(),
        Term::BlankNode(bn) => bn.as_str().to_string(),
        #[cfg(feature = "rdf-12")]
        Term::Triple(_) => t.to_string(),
    }
}

/// Persist a commit as `prov:Activity` triples in the commit-log graph.
///
/// Best-effort: errors are returned so callers can log them, but recording a
/// commit should never abort the mutation that already succeeded.
pub fn insert_commit(
    store: &TripleStore,
    base_url: &str,
    rec: &CommitRecord,
) -> Result<(), crate::store::engine::StoreError> {
    let commit_iri = format!(
        "{}/commit/{}",
        base_url.trim_end_matches('/'),
        rec.commit_id
    );

    let mut lines = vec![
        "    a prov:Activity, ver:Commit ;".to_string(),
        format!("    ver:kind \"{}\" ;", rec.kind.as_str()),
        format!("    rdfs:comment \"{}\" ;", esc(&rec.message)),
        format!("    ver:added \"{}\"^^xsd:integer ;", rec.added),
        format!("    ver:removed \"{}\"^^xsd:integer ;", rec.removed),
    ];
    if let Some(a) = &rec.actor_iri {
        lines.push(format!(
            "    prov:wasAssociatedWith <{}> ;",
            crate::store::escape_sparql_iri(a)
        ));
    }
    if let Some(s) = &rec.subject_iri {
        lines.push(format!(
            "    ver:onSubject <{}> ;",
            crate::store::escape_sparql_iri(s)
        ));
    }
    if let Some(v) = &rec.version {
        lines.push(format!("    ver:onVersion \"{}\" ;", esc(v)));
    }
    if let Some(b) = &rec.branch {
        lines.push(format!("    ver:onBranch \"{}\" ;", esc(b)));
    }
    if let Some(p) = &rec.parent_revision {
        lines.push(format!("    ver:parentRevision \"{}\" ;", esc(p)));
    }
    if let Some(r) = &rec.revision {
        lines.push(format!("    ver:revision \"{}\" ;", esc(r)));
    }
    if let Some(m) = &rec.metadata {
        lines.push(format!("    ver:metadata \"{}\" ;", esc(&m.to_string())));
    }
    // The pointer into the change log's sequence (delta note §2.3: one
    // literal per end, never the delta itself).
    if let Some(n) = rec.seq_from {
        lines.push(format!("    ver:seqFrom \"{n}\"^^xsd:integer ;"));
    }
    if let Some(n) = rec.seq_to {
        lines.push(format!("    ver:seqTo \"{n}\"^^xsd:integer ;"));
    }
    if let Some(n) = rec.net_added {
        lines.push(format!("    ver:netAdded \"{n}\"^^xsd:integer ;"));
    }
    if let Some(n) = rec.net_removed {
        lines.push(format!("    ver:netRemoved \"{n}\"^^xsd:integer ;"));
    }
    for g in &rec.affected_graphs {
        lines.push(format!(
            "    ver:affectedGraph <{}> ;",
            crate::store::escape_sparql_iri(g)
        ));
    }
    // PROV-O: the activity's interval (a commit is instantaneous here).
    lines.push(format!(
        "    prov:startedAtTime \"{0}\"^^xsd:dateTime ;\n    prov:endedAtTime \"{0}\"^^xsd:dateTime ;",
        rec.created_at
    ));
    // Terminate with the timestamp.
    lines.push(format!(
        "    dct:created \"{}\"^^xsd:dateTime .",
        rec.created_at
    ));

    // Per-graph / per-transaction PROV-O attribution: every affected graph is
    // an entity generated by this activity (and attributed to its agent), the
    // agent is typed, and the subject (dataset / model) was influenced. The
    // commit was already a prov:Activity; these make the graph-level links
    // navigable with plain PROV vocabulary, from either end.
    let mut extra: Vec<String> = Vec::new();
    if let Some(a) = &rec.actor_iri {
        extra.push(format!(
            "            <{}> a prov:Agent .",
            crate::store::escape_sparql_iri(a)
        ));
    }
    for g in &rec.affected_graphs {
        let g = crate::store::escape_sparql_iri(g);
        extra.push(format!(
            "            <{g}> a prov:Entity ; prov:wasGeneratedBy <{commit_iri}> ."
        ));
        if let Some(a) = &rec.actor_iri {
            extra.push(format!(
                "            <{g}> prov:wasAttributedTo <{}> .",
                crate::store::escape_sparql_iri(a)
            ));
        }
    }
    if let Some(subj) = &rec.subject_iri {
        extra.push(format!(
            "            <{}> a prov:Entity ; prov:wasInfluencedBy <{commit_iri}> .",
            crate::store::escape_sparql_iri(subj)
        ));
    }
    let extra = extra.join("\n");

    let body = lines.join("\n");
    let q = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        PREFIX prov: <{PROV}>
        PREFIX rdfs: <{RDFS}>
        PREFIX xsd: <{XSD}>
        INSERT DATA {{
          GRAPH <{COMMIT_GRAPH}> {{
            <{commit_iri}>
{body}
{extra}
          }}
        }}
        "#
    );
    store.update(&q)
}

/// How to scope a `list_commits` query.
#[derive(Debug, Clone)]
pub enum CommitScope {
    /// All commits on a given subject (model/vocabulary IRI).
    Subject(String),
    /// All commits touching any of these graphs (dataset's registered graphs).
    Graphs(Vec<String>),
}

/// Optional filters layered on top of the scope.
#[derive(Debug, Clone, Default)]
pub struct CommitQuery {
    pub branch: Option<String>,
    pub version: Option<String>,
    pub limit: Option<usize>,
}

/// Query-string params for the `…/commits` endpoints.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CommitsParams {
    pub branch: Option<String>,
    pub version: Option<String>,
    pub limit: Option<usize>,
}

impl CommitsParams {
    pub fn to_query(&self) -> CommitQuery {
        let clean = |o: &Option<String>| {
            o.clone()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        CommitQuery {
            branch: clean(&self.branch),
            version: clean(&self.version),
            limit: self.limit,
        }
    }
}

/// List commits newest-first for a scope. Returns an empty vec when a graph
/// scope is empty (a dataset with no registered graphs has no commits).
pub fn list_commits(
    store: &TripleStore,
    scope: &CommitScope,
    q: &CommitQuery,
) -> Vec<CommitRecord> {
    let mut where_lines: Vec<String> = vec!["?c a ver:Commit .".to_string()];

    // Scope: a subject (model/vocab) or a set of graphs (dataset). The dataset
    // scope filters on the stored `ver:affectedGraph` triples; `?g` is not
    // projected (SELECT DISTINCT collapses the per-graph row fan-out).
    match scope {
        CommitScope::Subject(subject) => {
            where_lines.push(format!(
                "?c ver:onSubject <{}> .",
                crate::store::escape_sparql_iri(subject)
            ));
        }
        CommitScope::Graphs(graphs) => {
            if graphs.is_empty() {
                return Vec::new();
            }
            let in_list = graphs
                .iter()
                .map(|g| format!("<{}>", crate::store::escape_sparql_iri(g)))
                .collect::<Vec<_>>()
                .join(", ");
            where_lines.push("?c ver:affectedGraph ?g .".to_string());
            where_lines.push(format!("FILTER(?g IN ({in_list}))"));
        }
    }

    if let Some(b) = &q.branch {
        where_lines.push(format!("?c ver:onBranch \"{}\" .", esc(b)));
    }
    if let Some(v) = &q.version {
        where_lines.push(format!("?c ver:onVersion \"{}\" .", esc(v)));
    }

    let optionals = r#"
        OPTIONAL { ?c dct:created ?created }
        OPTIONAL { ?c prov:wasAssociatedWith ?actor }
        OPTIONAL { ?c rdfs:comment ?msg }
        OPTIONAL { ?c ver:onSubject ?subject }
        OPTIONAL { ?c ver:onVersion ?version }
        OPTIONAL { ?c ver:onBranch ?branch }
        OPTIONAL { ?c ver:added ?added }
        OPTIONAL { ?c ver:removed ?removed }
        OPTIONAL { ?c ver:parentRevision ?parent }
        OPTIONAL { ?c ver:revision ?rev }
        OPTIONAL { ?c ver:kind ?kind }
        OPTIONAL { ?c ver:metadata ?meta }
        OPTIONAL { ?c ver:affectedGraph ?ag }
        OPTIONAL { ?c ver:seqFrom ?seqFrom }
        OPTIONAL { ?c ver:seqTo ?seqTo }
        OPTIONAL { ?c ver:netAdded ?netAdded }
        OPTIONAL { ?c ver:netRemoved ?netRemoved }
    "#;

    let where_block = where_lines.join("\n            ");
    let limit_clause = q.limit.map(|n| format!("LIMIT {n}")).unwrap_or_default();

    let query = format!(
        r#"
        PREFIX ver: <{VER}>
        PREFIX dct: <{DCT}>
        PREFIX prov: <{PROV}>
        PREFIX rdfs: <{RDFS}>
        SELECT DISTINCT ?c ?created ?actor ?msg ?subject ?version ?branch ?added ?removed ?parent ?rev ?kind ?meta
               ?seqFrom ?seqTo ?netAdded ?netRemoved
               (GROUP_CONCAT(DISTINCT STR(?ag); separator=" ") AS ?graphs)
        WHERE {{
          GRAPH <{COMMIT_GRAPH}> {{
            {where_block}
            {optionals}
          }}
        }}
        GROUP BY ?c ?created ?actor ?msg ?subject ?version ?branch ?added ?removed ?parent ?rev ?kind ?meta
                 ?seqFrom ?seqTo ?netAdded ?netRemoved
        ORDER BY DESC(?created)
        {limit_clause}
        "#
    );

    let mut out = Vec::new();
    if let Ok(QueryResults::Solutions(sols)) = store.query(&query) {
        for row in sols.flatten() {
            // Look up projected variables by name (robust to projection ordering,
            // which matters for the GROUP_CONCAT alias).
            let g = |name: &str| row.get(name).map(term_to_string);

            let commit_iri = match g("c") {
                Some(v) => v,
                None => continue,
            };
            let commit_id = commit_iri
                .rsplit('/')
                .next()
                .unwrap_or(&commit_iri)
                .to_string();
            let kind = match g("kind").as_deref() {
                Some("vocabulary") => CommitKind::Vocabulary,
                Some("dataset") => CommitKind::Dataset,
                Some("sparql") => CommitKind::Sparql,
                Some("shapes") => CommitKind::Shapes,
                Some("graph-store") => CommitKind::GraphStore,
                Some("import") => CommitKind::Import,
                Some("backup") => CommitKind::Backup,
                Some("source") => CommitKind::Source,
                _ => CommitKind::DataModel,
            };
            let metadata =
                g("meta").and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());

            out.push(CommitRecord {
                commit_id,
                kind,
                actor_iri: g("actor"),
                actor_username: None,
                actor_display_name: None,
                created_at: g("created").unwrap_or_default(),
                message: g("msg").unwrap_or_default(),
                metadata,
                subject_iri: g("subject"),
                // Collapsed into one row per commit by the GROUP_CONCAT above;
                // this used to be left empty on every listing, so the field
                // the API documents was never populated on read.
                affected_graphs: g("graphs")
                    .map(|v| {
                        v.split(' ')
                            .filter(|x| !x.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                version: g("version"),
                branch: g("branch"),
                added: g("added").and_then(|s| s.parse().ok()).unwrap_or(0),
                removed: g("removed").and_then(|s| s.parse().ok()).unwrap_or(0),
                parent_revision: g("parent"),
                revision: g("rev"),
                seq_from: g("seqFrom").and_then(|s| s.parse().ok()),
                seq_to: g("seqTo").and_then(|s| s.parse().ok()),
                net_added: g("netAdded").and_then(|s| s.parse().ok()),
                net_removed: g("netRemoved").and_then(|s| s.parse().ok()),
            });
        }
    }
    out
}

/// Resolve `actor_iri` → username/display_name for a slice of commits, in place.
/// The actor IRI's last path segment is the user id (matches the registry's
/// `dct:creator <…/user/{id}>` convention used elsewhere).
pub fn resolve_actors(auth_db: &crate::auth::db::AuthDb, commits: &mut [CommitRecord]) {
    use std::collections::HashMap;
    let mut cache: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    for c in commits.iter_mut() {
        let Some(iri) = c.actor_iri.clone() else {
            continue;
        };
        let uid = iri.rsplit('/').next().unwrap_or(&iri).to_string();
        // The agent that stands for purged accounts is nobody's account: it
        // has a name to show and no username.
        if uid == DELETED_USER_ID {
            c.actor_username = None;
            c.actor_display_name = Some(DELETED_USER_LABEL.to_string());
            continue;
        }
        let entry =
            cache
                .entry(uid.clone())
                .or_insert_with(|| match auth_db.get_user_by_id(&uid) {
                    Ok(Some(u)) => (Some(u.username), u.display_name),
                    _ => (None, None),
                });
        c.actor_username = entry.0.clone();
        c.actor_display_name = entry.1.clone();
    }
}

/// The last path segment of the agent that stands for purged accounts. An
/// account id is minted by the server and never this word.
pub const DELETED_USER_ID: &str = "deleted";

/// What the trail and its readers call that agent.
pub const DELETED_USER_LABEL: &str = "Deleted user";

/// The agent IRI of an account, as [`record`] writes it:
/// `{base}/users/{user id}`.
pub fn account_iri(base_url: &str, user_id: &str) -> String {
    format!("{}/users/{user_id}", base_url.trim_end_matches('/'))
}

/// The one agent every purged account's writes are attributed to where the
/// operator chose `OTS_PURGE_ATTRIBUTION=deleted-user`:
/// `{base}/users/deleted`. It says a person made the write and that their
/// account is gone, and nothing about which account.
pub fn deleted_user_iri(base_url: &str) -> String {
    account_iri(base_url, DELETED_USER_ID)
}

/// Attribute everything the trail says about the agent `account` to
/// [`deleted_user_iri`]: the `prov:wasAssociatedWith` of its commits and the
/// `prov:wasAttributedTo` of the graphs they generated now name the
/// placeholder, and the account's own description (`a prov:Agent`) is gone.
/// The commits stay, with their times, messages, graphs and counts. One
/// transaction; the change log never records the commit graph, so the write
/// leaves no copy of the IRI there. Returns the commits re-attributed (zero,
/// and nothing written, for an account that never committed).
pub fn reattribute_to_deleted_user(
    store: &TripleStore,
    base_url: &str,
    account: &str,
) -> anyhow::Result<usize> {
    use crate::store::engine::QuadOp;

    let placeholder = NamedNode::new(deleted_user_iri(base_url))?;
    let account = NamedNode::new(account)?;
    if account == placeholder {
        return Ok(0);
    }
    let trail = NamedNode::new_unchecked(COMMIT_GRAPH);
    let in_trail = GraphNameRef::NamedNode(trail.as_ref());
    let associated = NamedNode::new_unchecked(format!("{PROV}wasAssociatedWith"));
    let quads = store.store();

    let mut ops: Vec<QuadOp> = Vec::new();
    let mut commits = 0usize;
    // Wherever the account is named: the object of a commit's or a graph's
    // attribution.
    for q in quads.quads_for_pattern(
        None,
        None,
        Some(TermRef::NamedNode(account.as_ref())),
        Some(in_trail),
    ) {
        let q = q?;
        if q.predicate == associated {
            commits += 1;
        }
        ops.push(QuadOp::Add(Quad::new(
            q.subject.clone(),
            q.predicate.clone(),
            placeholder.clone(),
            q.graph_name.clone(),
        )));
        ops.push(QuadOp::Remove(q));
    }
    // What the trail says about the account itself.
    for q in quads.quads_for_pattern(
        Some(NamedOrBlankNodeRef::NamedNode(account.as_ref())),
        None,
        None,
        Some(in_trail),
    ) {
        ops.push(QuadOp::Remove(q?));
    }
    if ops.is_empty() {
        return Ok(0);
    }
    for (p, o) in [
        (
            NamedNode::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            Term::NamedNode(NamedNode::new_unchecked(format!("{PROV}Agent"))),
        ),
        (
            NamedNode::new_unchecked(format!("{RDFS}label")),
            Term::Literal(Literal::new_simple_literal(DELETED_USER_LABEL)),
        ),
    ] {
        ops.push(QuadOp::Add(Quad::new(
            placeholder.clone(),
            p,
            o,
            trail.clone(),
        )));
    }
    store.apply_quad_ops(&ops)?;
    Ok(commits)
}

/// Take the actor off `commits`, for a caller the actor is not shown to
/// (`OTS_ACTOR_VISIBILITY`): the IRI and the names [`resolve_actors`] fills.
pub fn withhold_actors(commits: &mut [CommitRecord]) {
    for c in commits.iter_mut() {
        c.actor_iri = None;
        c.actor_username = None;
        c.actor_display_name = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;

    fn sample(kind: CommitKind) -> CommitRecord {
        let mut r = CommitRecord::new(kind, "Initial commit");
        r.actor_iri = Some("http://x/users/u1".into());
        r.subject_iri = Some("http://x/data-model/dm1".into());
        r.version = Some("1.0.0".into());
        r.branch = Some("feature-a".into());
        r.affected_graphs = vec!["http://x/g1".into(), "http://x/g2".into()];
        r.added = 5;
        r.removed = 2;
        r.parent_revision = Some("aaa".into());
        r.revision = Some("bbb".into());
        r.metadata = Some(serde_json::json!({"source": "import"}));
        r
    }

    /// A triple term used to render as an empty string, which made every one
    /// of them read back as the same (empty) value.
    #[cfg(feature = "rdf-12")]
    #[test]
    fn triple_term_values_keep_their_form() {
        let t = Term::Triple(Box::new(Triple::new(
            NamedNode::new_unchecked("http://x/s"),
            NamedNode::new_unchecked("http://x/p"),
            NamedNode::new_unchecked("http://x/o"),
        )));
        assert_eq!(
            term_to_string(&t),
            "<<( <http://x/s> <http://x/p> <http://x/o> )>>"
        );
    }

    #[test]
    fn insert_and_list_by_subject() {
        let store = TripleStore::in_memory().unwrap();
        let rec = sample(CommitKind::DataModel);
        insert_commit(&store, "http://x", &rec).unwrap();

        let got = list_commits(
            &store,
            &CommitScope::Subject("http://x/data-model/dm1".into()),
            &CommitQuery::default(),
        );
        assert_eq!(got.len(), 1);
        let c = &got[0];
        assert_eq!(c.message, "Initial commit");
        assert_eq!(c.actor_iri.as_deref(), Some("http://x/users/u1"));
        assert_eq!(c.version.as_deref(), Some("1.0.0"));
        assert_eq!(c.branch.as_deref(), Some("feature-a"));
        assert_eq!(c.added, 5);
        assert_eq!(c.removed, 2);
        assert_eq!(c.kind, CommitKind::DataModel);
        assert_eq!(c.metadata, Some(serde_json::json!({"source": "import"})));
    }

    #[test]
    fn shapes_kind_round_trips() {
        // A shape-graph commit must read back as Shapes, not the DataModel
        // fallback — otherwise shape-graph history would be mislabeled.
        let store = TripleStore::in_memory().unwrap();
        let mut rec = CommitRecord::new(CommitKind::Shapes, "Saved shape graph");
        rec.subject_iri = Some("http://x/shacl/shape-graphs/ss1".into());
        insert_commit(&store, "http://x", &rec).unwrap();

        let got = list_commits(
            &store,
            &CommitScope::Subject("http://x/shacl/shape-graphs/ss1".into()),
            &CommitQuery::default(),
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, CommitKind::Shapes);
    }

    #[test]
    fn list_by_graphs_and_branch_filter() {
        let store = TripleStore::in_memory().unwrap();
        insert_commit(&store, "http://x", &sample(CommitKind::Sparql)).unwrap();

        // Matches when one of the dataset's graphs is touched.
        let hit = list_commits(
            &store,
            &CommitScope::Graphs(vec!["http://x/g2".into(), "http://x/other".into()]),
            &CommitQuery::default(),
        );
        assert_eq!(hit.len(), 1);

        // No match for unrelated graphs.
        let miss = list_commits(
            &store,
            &CommitScope::Graphs(vec!["http://x/nope".into()]),
            &CommitQuery::default(),
        );
        assert!(miss.is_empty());

        // Empty graph scope yields nothing.
        assert!(list_commits(
            &store,
            &CommitScope::Graphs(vec![]),
            &CommitQuery::default()
        )
        .is_empty());

        // Branch filter narrows results.
        let wrong_branch = CommitQuery {
            branch: Some("main".into()),
            ..Default::default()
        };
        assert!(list_commits(
            &store,
            &CommitScope::Subject("http://x/data-model/dm1".into()),
            &wrong_branch,
        )
        .is_empty());
    }

    // SECURITY: actor / subject / affected-graph IRIs are escaped before being
    // interpolated into the commit-log INSERT DATA, so a crafted IRI cannot break
    // out of `<...>` and inject extra triples.
    #[test]
    fn insert_commit_escapes_injected_iris() {
        let store = TripleStore::in_memory().unwrap();
        let mut r = CommitRecord::new(CommitKind::Dataset, "msg");
        r.actor_iri = Some("http://x/a> . <urn:evil> <urn:evil> <urn:evil> . <http://x/b".into());
        r.affected_graphs = vec!["http://x/g1".into()];
        insert_commit(&store, "http://localhost", &r).unwrap();
        let injected = matches!(
            store
                .query("ASK { GRAPH ?g { <urn:evil> <urn:evil> <urn:evil> } }")
                .unwrap(),
            oxigraph::sparql::QueryResults::Boolean(true)
        );
        assert!(!injected, "crafted actor IRI must be escaped, not injected");
    }

    fn ask(store: &TripleStore, q: &str) -> bool {
        matches!(
            store.query(q).unwrap(),
            oxigraph::sparql::QueryResults::Boolean(true)
        )
    }

    /// The account's commits stay and name the placeholder; the account's IRI
    /// is in no quad of the trail any more; another account's commits are
    /// untouched.
    #[test]
    fn a_purged_account_is_reattributed_to_the_deleted_user() {
        let store = TripleStore::in_memory().unwrap();
        let base = "http://x";
        let gone = account_iri(base, "u1");
        let stays = account_iri(base, "u2");
        for (actor, graph, msg) in [
            (Some(&gone), "http://x/g1", "first by u1"),
            (Some(&gone), "http://x/g2", "second by u1"),
            (Some(&stays), "http://x/g1", "by u2"),
            (None, "http://x/g1", "by nobody"),
        ] {
            let mut r = CommitRecord::new(CommitKind::Sparql, msg);
            r.actor_iri = actor.cloned();
            r.affected_graphs = vec![graph.into()];
            insert_commit(&store, base, &r).unwrap();
        }
        let named = |iri: &str| {
            format!(
                "ASK {{ GRAPH <{COMMIT_GRAPH}> {{ {{ <{iri}> ?p ?o }} UNION {{ ?s ?p <{iri}> }} }} }}"
            )
        };
        assert!(ask(&store, &named(&gone)));

        assert_eq!(reattribute_to_deleted_user(&store, base, &gone).unwrap(), 2);

        assert!(!ask(&store, &named(&gone)), "the account is still named");
        let placeholder = deleted_user_iri(base);
        assert_eq!(placeholder, "http://x/users/deleted");
        assert!(ask(
            &store,
            &format!(
                "ASK {{ GRAPH <{COMMIT_GRAPH}> {{ <{placeholder}> a <{PROV}Agent> ; \
                 <{RDFS}label> \"Deleted user\" }} }}"
            )
        ));
        let all = list_commits(
            &store,
            &CommitScope::Graphs(vec!["http://x/g1".into(), "http://x/g2".into()]),
            &CommitQuery::default(),
        );
        assert_eq!(all.len(), 4, "no commit is lost");
        let by = |msg: &str| {
            all.iter()
                .find(|c| c.message == msg)
                .unwrap()
                .actor_iri
                .clone()
        };
        assert_eq!(by("first by u1").as_deref(), Some(placeholder.as_str()));
        assert_eq!(by("second by u1").as_deref(), Some(placeholder.as_str()));
        assert_eq!(by("by u2").as_deref(), Some(stays.as_str()));
        assert_eq!(by("by nobody"), None);

        // The generated graphs follow: none is attributed to the account.
        assert!(ask(
            &store,
            &format!(
                "ASK {{ GRAPH <{COMMIT_GRAPH}> {{ <http://x/g2> <{PROV}wasAttributedTo> <{placeholder}> }} }}"
            )
        ));

        // Nothing to do the second time, and for an account that never wrote.
        assert_eq!(reattribute_to_deleted_user(&store, base, &gone).unwrap(), 0);
        assert_eq!(
            reattribute_to_deleted_user(&store, base, &account_iri(base, "u3")).unwrap(),
            0
        );
        // The placeholder is never re-attributed to itself.
        assert_eq!(
            reattribute_to_deleted_user(&store, base, &placeholder).unwrap(),
            0
        );
    }

    /// The placeholder resolves to a name to show and to no username, without
    /// a lookup: no account has that id.
    #[test]
    fn the_deleted_user_resolves_to_its_label() {
        let db = crate::auth::db::AuthDb::in_memory().unwrap();
        let mut c = CommitRecord::new(CommitKind::Sparql, "m");
        c.actor_iri = Some(deleted_user_iri("http://x"));
        let mut commits = vec![c];
        resolve_actors(&db, &mut commits);
        assert_eq!(commits[0].actor_username, None);
        assert_eq!(
            commits[0].actor_display_name.as_deref(),
            Some(DELETED_USER_LABEL)
        );
    }
}
