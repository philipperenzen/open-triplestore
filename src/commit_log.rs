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
}

impl CommitKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CommitKind::DataModel => "data-model",
            CommitKind::Vocabulary => "vocabulary",
            CommitKind::Dataset => "dataset",
            CommitKind::Sparql => "sparql",
            CommitKind::Shapes => "shapes",
        }
    }
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
        }
    }
}

/// SPARQL string-literal escaping. Delegates to the canonical
/// [`crate::store::escape_sparql_literal`] (handles `\ " \n \r \t`).
fn esc(s: &str) -> String {
    crate::store::escape_sparql_literal(s)
}

fn term_to_string(t: &Term) -> String {
    match t {
        Term::NamedNode(nn) => nn.as_str().to_string(),
        Term::Literal(lit) => lit.value().to_string(),
        Term::BlankNode(bn) => bn.as_str().to_string(),
        #[cfg(feature = "rdf-12")]
        Term::Triple(_) => String::new(),
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
        format!("    a prov:Activity, ver:Commit ;"),
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
    for g in &rec.affected_graphs {
        lines.push(format!(
            "    ver:affectedGraph <{}> ;",
            crate::store::escape_sparql_iri(g)
        ));
    }
    // Terminate with the timestamp.
    lines.push(format!(
        "    dct:created \"{}\"^^xsd:dateTime .",
        rec.created_at
    ));

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
        WHERE {{
          GRAPH <{COMMIT_GRAPH}> {{
            {where_block}
            {optionals}
          }}
        }}
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
                // Not projected in listings (the dataset scope filters on the
                // stored triples directly); kept on the struct for writes.
                affected_graphs: Vec::new(),
                version: g("version"),
                branch: g("branch"),
                added: g("added").and_then(|s| s.parse().ok()).unwrap_or(0),
                removed: g("removed").and_then(|s| s.parse().ok()).unwrap_or(0),
                parent_revision: g("parent"),
                revision: g("rev"),
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
}
