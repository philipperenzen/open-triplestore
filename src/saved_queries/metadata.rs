//! Immutable, append-only RDF metadata for API services.
//!
//! Alongside the operational SQLite store, every API service, each of its
//! revisions, and each version test is projected into the system graph
//! `<urn:system:api-service-registry>` as linked data. The graph is system-owned
//! — user SPARQL is re-scoped to dataset graphs (see `scope_query_to_authorized`)
//! and never touches it — and the projection is strictly append-only (no DELETE),
//! so the metadata is immutable: the current head is simply the highest
//! `aps:revision`, and a deleted service keeps its record plus an `aps:retiredAt`.
//!
//! This mirrors the dataset-version registry (`dataset_versions::registry`); the
//! writes are best-effort (a storage hiccup is logged, never fatal) because the
//! authoritative operational record is the SQLite row.

use crate::store::TripleStore;

use super::models::{QueryScope, QueryTest, SavedQuery};

pub const REGISTRY_GRAPH: &str = "urn:system:api-service-registry";
const APS: &str = "urn:system:vocab/aps#";
const DCT: &str = "http://purl.org/dc/terms/";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const ADMS: &str = "http://www.w3.org/ns/adms#";

fn service_iri(base_url: &str, id: &str) -> String {
    format!("{base_url}/api-service/{id}")
}
fn revision_iri(base_url: &str, id: &str, revision: i64) -> String {
    format!("{base_url}/api-service/{id}/revision/{revision}")
}
fn test_iri(base_url: &str, id: &str, test_id: &str) -> String {
    format!("{base_url}/api-service/{id}/test/{test_id}")
}
/// The dataset's canonical IRI — the same scheme used by the dataset-version
/// registry, so a dataset-scoped service links to the very node datasets publish.
fn dataset_iri(base_url: &str, dataset_id: &str) -> String {
    format!("{base_url}/dataset/{dataset_id}")
}

/// Escape a value for a single-line SPARQL string literal (`"..."`).
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

fn run(store: &TripleStore, what: &str, update: String) {
    // Every write here touches only the single system registry graph, so scope
    // the graph-index refresh to that one graph. A plain `store.update()` would
    // trigger a full-store index rebuild (a scan of every quad in every graph),
    // which on a large store stalls the request thread for seconds per write and
    // can wedge the whole backend when several writes land back-to-back.
    if let Err(e) = store.update_targeted(&update, &[REGISTRY_GRAPH.to_string()], false) {
        tracing::warn!("api-service metadata: {what} failed: {e}");
    }
}

/// Record a newly created service node (written once at creation).
pub fn record_service(store: &TripleStore, base_url: &str, sq: &SavedQuery) {
    let s = service_iri(base_url, &sq.id);
    let desc = sq
        .description
        .as_deref()
        .map(|d| format!("    dct:description \"{}\" ;\n", esc(d)))
        .unwrap_or_default();
    let vis = sq
        .visibility
        .as_deref()
        .map(|v| format!("    aps:visibility \"{}\" ;\n", esc(v)))
        .unwrap_or_default();
    // A dataset-scoped service is part of, and reads from, exactly one dataset —
    // link it to that dataset's canonical IRI so the registry connects the
    // service (and its versioned revisions) back to the dataset as linked data.
    let ds_link = if matches!(sq.scope, QueryScope::Dataset) {
        let ds = dataset_iri(base_url, &sq.owner_id);
        format!("    aps:dataset <{ds}> ;\n    dct:isPartOf <{ds}> ;\n")
    } else {
        String::new()
    };
    let q = format!(
        r#"
        PREFIX aps: <{APS}>
        PREFIX dct: <{DCT}>
        INSERT DATA {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{s}> a aps:ApiService ;
              dct:title "{title}" ;
              aps:slug "{slug}" ;
              aps:scope "{scope}" ;
              aps:owner "{owner}" ;
        {desc}{vis}{ds_link}    dct:creator "{creator}" ;
              dct:created "{created}"^^<{XSD}dateTime> .
          }}
        }}
        "#,
        title = esc(&sq.name),
        slug = esc(&sq.slug),
        scope = sq.scope.as_str(),
        owner = esc(&sq.owner_id),
        creator = esc(&sq.created_by),
        created = esc(&sq.created_at),
    );
    run(store, "record_service", q);
}

/// Record one immutable revision of a service's SPARQL. A revision is a
/// commit-style version: it can carry a custom `name` (`dct:title`/`rdfs:label`)
/// and a `note` (`adms:versionNotes`/`rdfs:comment`), mirroring the
/// dataset-version registry's vocabulary.
#[allow(clippy::too_many_arguments)]
pub fn record_revision(
    store: &TripleStore,
    base_url: &str,
    service_id: &str,
    revision: i64,
    name: Option<&str>,
    note: Option<&str>,
    sparql: &str,
    origin: &str,
    created_by: &str,
    created_at: &str,
) {
    let s = service_iri(base_url, service_id);
    let r = revision_iri(base_url, service_id, revision);
    // A custom name doubles as both a Dublin Core title and an RDFS label; a
    // note as both ADMS version notes and an RDFS comment — so generic RDF
    // browsers and version-aware tools both surface it.
    let title = name
        .map(|n| format!("    dct:title \"{}\" ;\n    rdfs:label \"{}\" ;\n", esc(n), esc(n)))
        .unwrap_or_default();
    let notes = note
        .map(|n| format!("    adms:versionNotes \"{}\" ;\n    rdfs:comment \"{}\" ;\n", esc(n), esc(n)))
        .unwrap_or_default();
    let q = format!(
        r#"
        PREFIX aps: <{APS}>
        PREFIX dct: <{DCT}>
        PREFIX rdfs: <{RDFS}>
        PREFIX owl: <{OWL}>
        PREFIX adms: <{ADMS}>
        INSERT DATA {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{s}> aps:hasRevision <{r}> .
            <{r}> a aps:Revision ;
              aps:revision {revision} ;
              owl:versionInfo "{revision}" ;
        {title}{notes}    aps:sparql "{sparql}" ;
              aps:origin "{origin}" ;
              dct:creator "{creator}" ;
              dct:created "{created}"^^<{XSD}dateTime> .
          }}
        }}
        "#,
        sparql = esc(sparql),
        origin = esc(origin),
        creator = esc(created_by),
        created = esc(created_at),
    );
    run(store, "record_revision", q);
}

/// Record one immutable version-test result.
pub fn record_test(store: &TripleStore, base_url: &str, t: &QueryTest) {
    let s = service_iri(base_url, &t.query_id);
    let ti = test_iri(base_url, &t.query_id, &t.id);
    let prev = t
        .prev_version
        .as_deref()
        .map(|p| format!("    aps:previousVersion \"{}\" ;\n", esc(p)))
        .unwrap_or_default();
    let hash = t
        .result_hash
        .as_deref()
        .map(|h| format!("    aps:resultHash \"{}\" ;\n", esc(h)))
        .unwrap_or_default();
    let rows = t
        .result_rowcount
        .map(|n| format!("    aps:rowCount {n} ;\n"))
        .unwrap_or_default();
    let err = t
        .error_message
        .as_deref()
        .map(|e| format!("    aps:error \"{}\" ;\n", esc(e)))
        .unwrap_or_default();
    let q = format!(
        r#"
        PREFIX aps: <{APS}>
        PREFIX dct: <{DCT}>
        INSERT DATA {{
          GRAPH <{REGISTRY_GRAPH}> {{
            <{s}> aps:hasTest <{ti}> .
            <{ti}> a aps:QueryTest ;
              aps:revision {revision} ;
              aps:status "{status}" ;
              aps:datasetVersion "{version}" ;
        {prev}{hash}{rows}{err}    dct:created "{created}"^^<{XSD}dateTime> .
          }}
        }}
        "#,
        revision = t.revision,
        status = esc(&t.status),
        version = esc(&t.dataset_version),
        created = esc(&t.created_at),
    );
    run(store, "record_test", q);
}

/// Append a retirement marker when a service is deleted (the record itself is
/// kept — metadata is immutable).
pub fn record_retired(store: &TripleStore, base_url: &str, service_id: &str, at: &str) {
    let s = service_iri(base_url, service_id);
    let q = format!(
        r#"
        PREFIX aps: <{APS}>
        INSERT DATA {{
          GRAPH <{REGISTRY_GRAPH}> {{ <{s}> aps:retiredAt "{at}"^^<{XSD}dateTime> . }}
        }}
        "#,
        at = esc(at),
    );
    run(store, "record_retired", q);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TripleStore;
    use oxigraph::sparql::QueryResults;

    fn count(store: &TripleStore, type_iri: &str) -> usize {
        let q = format!(
            "PREFIX aps: <{APS}> SELECT (COUNT(?x) AS ?n) WHERE {{ GRAPH <{REGISTRY_GRAPH}> {{ ?x a <{type_iri}> }} }}"
        );
        if let Ok(QueryResults::Solutions(sols)) = store.query(&q) {
            if let Some(row) = sols.flatten().next() {
                if let Some(oxigraph::model::Term::Literal(l)) = row.values().first().and_then(|v| v.clone()) {
                    return l.value().parse().unwrap_or(0);
                }
            }
        }
        0
    }

    fn ask(store: &TripleStore, pattern: &str) -> bool {
        matches!(
            store.query(&format!("ASK {{ GRAPH <{REGISTRY_GRAPH}> {{ {pattern} }} }}")),
            Ok(QueryResults::Boolean(true))
        )
    }

    #[test]
    fn projects_immutable_records() {
        let store = TripleStore::in_memory().unwrap();
        let base = "http://localhost";
        let sq = SavedQuery {
            id: "svc1".into(),
            scope: super::super::models::QueryScope::Dataset,
            owner_id: "ds1".into(),
            name: "Cities \"big\"".into(),
            slug: "cities".into(),
            description: Some("line1\nline2".into()),
            current_revision: 1,
            parameters: vec![],
            test_parameters: None,
            visibility: None,
            is_active: true,
            created_by: "u1".into(),
            created_at: "2026-05-26T00:00:00+00:00".into(),
            updated_at: "2026-05-26T00:00:00+00:00".into(),
            sparql: Some("SELECT * WHERE { ?s ?p ?o }".into()),
        };
        record_service(&store, base, &sq);
        record_revision(&store, base, "svc1", 1, Some("v1 — initial"), Some("first cut"), "SELECT * WHERE { ?s ?p ?o }", "manual", "u1", &sq.created_at);
        record_revision(&store, base, "svc1", 2, None, None, "ASK {}", "llm_repair", "u1", &sq.created_at);
        let t = QueryTest {
            id: "t1".into(), query_id: "svc1".into(), revision: 2, dataset_id: "ds1".into(),
            dataset_version: "1.0.0".into(), prev_version: None, status: "ok".into(),
            result_hash: Some("abc".into()), result_rowcount: Some(3), error_message: None,
            acknowledged: false, acknowledged_by: None, acknowledged_at: None,
            created_at: sq.created_at.clone(),
        };
        record_test(&store, base, &t);

        assert_eq!(count(&store, &format!("{APS}ApiService")), 1);
        assert_eq!(count(&store, &format!("{APS}Revision")), 2, "both revisions kept (immutable)");
        assert_eq!(count(&store, &format!("{APS}QueryTest")), 1);

        // A dataset-scoped service is linked to its dataset's canonical IRI as
        // linked data (both aps:dataset and dct:isPartOf).
        assert!(
            ask(&store, &format!("<{base}/api-service/svc1> <{APS}dataset> <{base}/dataset/ds1>")),
            "service links to its dataset IRI via aps:dataset"
        );
        assert!(
            ask(&store, &format!("<{base}/api-service/svc1> <{DCT}isPartOf> <{base}/dataset/ds1>")),
            "service is dct:isPartOf its dataset"
        );
        // A revision's custom name is projected as a commit-style title/label.
        assert!(
            ask(&store, &format!("<{base}/api-service/svc1/revision/1> <{DCT}title> \"v1 — initial\"")),
            "revision 1 carries its custom name as dct:title"
        );
        assert!(
            ask(&store, &format!("<{base}/api-service/svc1/revision/1> <{RDFS}label> \"v1 — initial\"")),
            "revision 1 carries its custom name as rdfs:label"
        );
        // Its note is projected as ADMS version notes (commit message).
        assert!(
            ask(&store, &format!("<{base}/api-service/svc1/revision/1> <{ADMS}versionNotes> \"first cut\"")),
            "revision 1 carries its note as adms:versionNotes"
        );
        // Every revision carries a machine-readable version number.
        assert!(
            ask(&store, &format!("<{base}/api-service/svc1/revision/2> <{OWL}versionInfo> \"2\"")),
            "revision 2 carries owl:versionInfo"
        );
    }
}
