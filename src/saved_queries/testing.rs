//! Re-test a dataset's saved queries against a newly created version.
//!
//! When a dataset gains a version, every active saved query is re-run against
//! that version's snapshot and the outcome recorded in its test history:
//! `ok` (works, same results as before), `changed` (works but the result set
//! differs from the previous version), or `error` (no longer runs). The history
//! is durable and reviewable; broken/changed entries can be acknowledged by
//! owners/admins. This pass only records — owner email goes out on the
//! `?version=latest` API path (see [`super::notify`]).

use std::collections::{HashMap, HashSet};

use uuid::Uuid;

use crate::dataset_versions::registry;
use crate::server::routes::{resolve_prefixes, scope_query_to_authorized};
use crate::server::AppState;

use super::fingerprint;
use super::models::{QueryTest, SavedQuery};
use super::store::SavedQueryStore;

/// Build the value map for an automatic test from the query's stored
/// `test_parameters` JSON object.
fn test_values(sq: &SavedQuery) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Some(serde_json::Value::Object(obj)) = &sq.test_parameters {
        for (k, v) in obj {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            m.insert(k.clone(), s);
        }
    }
    m
}

/// Re-run every active saved query of a dataset against `new_version`, recording
/// each outcome. Best-effort: failures are logged, never propagated.
pub async fn test_queries_for_new_version(state: &AppState, dataset_id: &str, new_version: &str) {
    let sq_store = SavedQueryStore::new(state.auth_db.pool());
    let queries = match sq_store.list_active_dataset_queries(dataset_id) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!("saved-query version test: list failed: {e}");
            return;
        }
    };
    if queries.is_empty() {
        return;
    }

    let base = state.base_url.as_str();
    let version = match registry::get_version(&state.store, base, dataset_id, new_version) {
        Some(v) => v,
        None => {
            tracing::warn!("saved-query version test: version {new_version} not found");
            return;
        }
    };
    let graph_set: HashSet<String> = version.snapshot_graphs.iter().cloned().collect();

    for sq in queries {
        let sparql = match &sq.sparql {
            Some(s) => s.clone(),
            None => continue,
        };
        // Baseline = most recent prior recorded test that produced a hash.
        let prior = sq_store.list_tests(&sq.id).ok().and_then(|tests| {
            tests
                .into_iter()
                .find(|t| t.dataset_version != new_version && t.result_hash.is_some())
        });
        let prev_label = prior.as_ref().map(|t| t.dataset_version.clone());

        let provided = test_values(&sq);
        let injected = match super::params::inject(&sparql, &sq.parameters, &provided) {
            Ok(q) => q,
            Err(e) => {
                // A parameterised query with no usable test values cannot be
                // auto-tested; record an error explaining the configuration gap.
                record(
                    state,
                    &sq_store,
                    &sq,
                    new_version,
                    prev_label.as_deref(),
                    "error",
                    None,
                    None,
                    Some(&format!("auto-test skipped: {e}")),
                );
                continue;
            }
        };

        let scoped = scope_query_to_authorized(&injected, &graph_set);
        let final_q = resolve_prefixes(state, &scoped).await.unwrap_or(scoped);
        let store = state.store.clone();
        let fp = tokio::task::spawn_blocking(move || match store.query(&final_q) {
            Ok(r) => fingerprint::fingerprint(r),
            Err(e) => Err(e.to_string()),
        })
        .await
        .unwrap_or_else(|e| Err(format!("test task panicked: {e}")));

        match fp {
            Ok(f) => {
                let status = match prior.as_ref().and_then(|t| t.result_hash.as_deref()) {
                    Some(prev) if prev != f.hash => "changed",
                    _ => "ok",
                };
                record(
                    state,
                    &sq_store,
                    &sq,
                    new_version,
                    prev_label.as_deref(),
                    status,
                    Some(&f.hash),
                    f.rowcount,
                    None,
                );
            }
            Err(msg) => record(
                state,
                &sq_store,
                &sq,
                new_version,
                prev_label.as_deref(),
                "error",
                None,
                None,
                Some(&msg),
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn record(
    state: &AppState,
    sq_store: &SavedQueryStore,
    sq: &SavedQuery,
    version: &str,
    prev_version: Option<&str>,
    status: &str,
    result_hash: Option<&str>,
    rowcount: Option<i64>,
    error: Option<&str>,
) {
    let test = QueryTest {
        id: Uuid::new_v4().to_string(),
        query_id: sq.id.clone(),
        revision: sq.current_revision,
        dataset_id: sq.owner_id.clone(),
        dataset_version: version.to_string(),
        prev_version: prev_version.map(String::from),
        status: status.to_string(),
        result_hash: result_hash.map(String::from),
        result_rowcount: rowcount,
        error_message: error.map(String::from),
        acknowledged: false,
        acknowledged_by: None,
        acknowledged_at: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(e) = sq_store.insert_test(&test) {
        tracing::warn!("saved-query version test: failed to record result for {}: {e}", sq.id);
    }
    // Immutable linked-data metadata mirror.
    super::metadata::record_test(&state.store, &state.base_url, &test);
}

/// Fire the version-test pass as a detached background task (non-blocking).
pub fn spawn_version_tests(state: &AppState, dataset_id: &str, new_version: &str) {
    let state = state.clone();
    let dataset_id = dataset_id.to_string();
    let new_version = new_version.to_string();
    tokio::spawn(async move {
        test_queries_for_new_version(&state, &dataset_id, &new_version).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::models::{OwnerType, Visibility};
    use crate::dataset_versions::models::VersionStatus;
    use crate::dataset_versions::snapshot_as_version;
    use crate::saved_queries::models::{CreateSavedQueryRequest, QueryScope};
    use crate::store::TripleStore;

    const GRAPH: &str = "urn:test:graph";

    fn create_req(sparql: &str) -> CreateSavedQueryRequest {
        CreateSavedQueryRequest {
            name: "All triples".into(),
            slug: None,
            description: None,
            sparql: sparql.into(),
            parameters: vec![],
            test_parameters: None,
            visibility: None,
            version_name: None,
            note: None,
        }
    }

    fn snapshot(state: &AppState, ds: &str, ver: &str) {
        snapshot_as_version(
            &state.store,
            &state.base_url,
            ds,
            ver,
            &[GRAPH.to_string()],
            VersionStatus::Published,
            Some("http://localhost/users/u1"),
            None,
        )
        .unwrap();
    }

    /// A version bump records `ok` first, then `changed` once the underlying
    /// data differs from the previous version's snapshot.
    #[tokio::test]
    async fn version_bump_records_ok_then_changed() {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());
        let ds = "ds-test";
        state
            .auth_db
            .create_dataset(ds, "Test", None, OwnerType::User, "u1", Visibility::Private, None)
            .unwrap();
        state.auth_db.add_dataset_graph(ds, GRAPH).unwrap();
        state
            .store
            .update(&format!("INSERT DATA {{ GRAPH <{GRAPH}> {{ <urn:a> <urn:p> \"1\" }} }}"))
            .unwrap();

        let sqs = SavedQueryStore::new(state.auth_db.pool());
        let sq = sqs
            .create(QueryScope::Dataset, ds, &create_req("SELECT * WHERE { ?s ?p ?o }"), "u1")
            .unwrap();

        snapshot(&state, ds, "1.0.0");
        test_queries_for_new_version(&state, ds, "1.0.0").await;
        let tests = sqs.list_tests(&sq.id).unwrap();
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].status, "ok");
        assert_eq!(tests[0].result_rowcount, Some(1));

        // Change the data, snapshot a new version, re-test.
        state
            .store
            .update(&format!("INSERT DATA {{ GRAPH <{GRAPH}> {{ <urn:b> <urn:p> \"2\" }} }}"))
            .unwrap();
        snapshot(&state, ds, "2.0.0");
        test_queries_for_new_version(&state, ds, "2.0.0").await;
        let tests = sqs.list_tests(&sq.id).unwrap();
        assert_eq!(tests.len(), 2);
        assert_eq!(tests[0].dataset_version, "2.0.0");
        assert_eq!(tests[0].status, "changed");
        assert_eq!(tests[0].prev_version.as_deref(), Some("1.0.0"));
    }

    /// A query that no longer parses/evaluates is recorded as `error`.
    #[tokio::test]
    async fn broken_query_records_error() {
        let state = AppState::test_default_with_store(TripleStore::in_memory().unwrap());
        let ds = "ds-broken";
        state
            .auth_db
            .create_dataset(ds, "Test", None, OwnerType::User, "u1", Visibility::Private, None)
            .unwrap();
        state.auth_db.add_dataset_graph(ds, GRAPH).unwrap();
        let sqs = SavedQueryStore::new(state.auth_db.pool());
        let sq = sqs
            .create(QueryScope::Dataset, ds, &create_req("SELECT ?s WHERE { ?s ?p ?o "), "u1")
            .unwrap();
        snapshot(&state, ds, "1.0.0");
        test_queries_for_new_version(&state, ds, "1.0.0").await;
        let tests = sqs.list_tests(&sq.id).unwrap();
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].status, "error");
        assert!(tests[0].error_message.is_some());
    }
}
