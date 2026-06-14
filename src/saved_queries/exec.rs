//! Resolve which graphs a saved query reads (per requested dataset version) and
//! inject its parameters, producing a query ready for the scoped executor.

use std::collections::{HashMap, HashSet};

use crate::auth::models::OwnerType;
use crate::dataset_versions::registry;
use crate::server::error::AppError;
use crate::server::AppState;

use super::models::{QueryScope, SavedQuery};
use super::params;
use super::store::SavedQueryStore;

/// Which dataset version's data an API call should read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionRequest {
    /// Most recent version the query is known to work against; else latest
    /// published; else live data.
    Default,
    /// Live (current) data — errors are surfaced to the caller (and the owner
    /// is notified) rather than silently falling back.
    Latest,
    /// A specific version label.
    Pinned(String),
}

impl VersionRequest {
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(str::trim) {
            None | Some("") => VersionRequest::Default,
            Some("latest") | Some("live") | Some("current") => VersionRequest::Latest,
            Some(v) => VersionRequest::Pinned(v.to_string()),
        }
    }

    pub fn is_latest(&self) -> bool {
        matches!(self, VersionRequest::Latest)
    }
}

/// A saved query prepared for execution.
pub struct PreparedRun {
    /// SPARQL with parameters injected (not yet graph-scoped).
    pub query: String,
    /// Graphs the query may read; passed to the scoped executor as the
    /// authorised set.
    pub graphs: HashSet<String>,
    /// Resolved dataset version label, if the run is pinned to a snapshot.
    pub version_label: Option<String>,
    /// True when reading live data rather than a frozen snapshot.
    pub is_live: bool,
}

/// Inject parameters and resolve the graph set for a saved query run.
pub fn prepare_run(
    state: &AppState,
    sq_store: &SavedQueryStore,
    query: &SavedQuery,
    user_id: Option<&str>,
    provided: &HashMap<String, String>,
    req: &VersionRequest,
) -> Result<PreparedRun, AppError> {
    let sparql = query
        .sparql
        .clone()
        .ok_or_else(|| AppError::Internal("saved query has no body".to_string()))?;
    let injected = params::inject(&sparql, &query.parameters, provided)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let (graphs, version_label, is_live) = match query.scope {
        QueryScope::Dataset => {
            resolve_dataset_scope(state, sq_store, &query.id, &query.owner_id, user_id, req)?
        }
        QueryScope::Organisation | QueryScope::Group => {
            (resolve_owner_union(state, query, user_id)?, None, true)
        }
    };

    Ok(PreparedRun {
        query: injected,
        graphs,
        version_label,
        is_live,
    })
}

fn live_graphs(state: &AppState, dataset_id: &str) -> Result<HashSet<String>, AppError> {
    Ok(state
        .auth_db
        .list_dataset_graphs(dataset_id)
        .map_err(|e| AppError::Internal(e.to_string()))?
        .into_iter()
        .collect())
}

/// Whether `user_id` may write the dataset (editor/admin). Anonymous callers and
/// users with no effective role never can. Mirrors the dataset-service path in
/// `routes.rs`, which gates private-graph visibility on write access.
fn caller_can_write_dataset(state: &AppState, dataset_id: &str, user_id: Option<&str>) -> bool {
    let uid = match user_id {
        Some(u) => u,
        None => return false,
    };
    let dataset = match state.auth_db.get_dataset(dataset_id) {
        Ok(Some(d)) => d,
        _ => return false,
    };
    state
        .auth_db
        .can_write_dataset(uid, &dataset)
        .unwrap_or(false)
}

/// Drop graphs flagged `private` for callers who cannot write the dataset, so a
/// viewer (or anonymous user on a public dataset API service) cannot read a
/// sub-graph the owner marked private. Mirrors the dataset-service filter in
/// `routes.rs` (`list_dataset_graph_entries` + the `private` flag). Writers and
/// admins keep full visibility.
fn filter_private_for_reader(
    state: &AppState,
    dataset_id: &str,
    user_id: Option<&str>,
    graphs: HashSet<String>,
) -> HashSet<String> {
    if caller_can_write_dataset(state, dataset_id, user_id) {
        return graphs;
    }
    let private: HashSet<String> = state
        .auth_db
        .list_dataset_graph_entries(dataset_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.private)
        .map(|e| e.graph_iri)
        .collect();
    graphs
        .into_iter()
        .filter(|g| !private.contains(g))
        .collect()
}

fn resolve_dataset_scope(
    state: &AppState,
    sq_store: &SavedQueryStore,
    query_id: &str,
    dataset_id: &str,
    user_id: Option<&str>,
    req: &VersionRequest,
) -> Result<(HashSet<String>, Option<String>, bool), AppError> {
    let base = state.base_url.as_str();
    let (graphs, version_label, is_live) = match req {
        VersionRequest::Latest => (live_graphs(state, dataset_id)?, None, true),
        VersionRequest::Pinned(v) => {
            let ver = registry::get_version(&state.store, base, dataset_id, v)
                .ok_or_else(|| AppError::NotFound(format!("dataset version '{v}' not found")))?;
            (
                ver.snapshot_graphs.into_iter().collect(),
                Some(ver.version),
                false,
            )
        }
        VersionRequest::Default => resolve_default_scope(state, sq_store, query_id, dataset_id)?,
    };

    // Subtract private graphs for non-writers, regardless of whether the graphs
    // came from live data or a frozen snapshot — a snapshot can still contain a
    // graph the owner has since (or always) marked private.
    let graphs = filter_private_for_reader(state, dataset_id, user_id, graphs);
    Ok((graphs, version_label, is_live))
}

/// Resolve the graph set for a `Default` version request: the newest version with
/// a passing test, else the latest published version, else the newest version,
/// else live data.
fn resolve_default_scope(
    state: &AppState,
    sq_store: &SavedQueryStore,
    query_id: &str,
    dataset_id: &str,
) -> Result<(HashSet<String>, Option<String>, bool), AppError> {
    let base = state.base_url.as_str();
    let versions = registry::list_versions(&state.store, base, dataset_id);
    // Most recent version (newest first) with a passing recorded test.
    for v in &versions {
        if let Some(t) = sq_store
            .latest_test_for_version(query_id, &v.version)
            .map_err(|e| AppError::Internal(e.to_string()))?
        {
            if t.status == "ok" {
                return Ok((
                    v.snapshot_graphs.iter().cloned().collect(),
                    Some(v.version.clone()),
                    false,
                ));
            }
        }
    }
    // Else the latest published version.
    let (published, _draft) = registry::get_pointers(&state.store, base, dataset_id);
    if let Some(pl) = published {
        if let Some(ver) = registry::get_version(&state.store, base, dataset_id, &pl) {
            return Ok((
                ver.snapshot_graphs.into_iter().collect(),
                Some(ver.version),
                false,
            ));
        }
    }
    // Else the newest version that exists.
    if let Some(v) = versions.first() {
        return Ok((
            v.snapshot_graphs.iter().cloned().collect(),
            Some(v.version.clone()),
            false,
        ));
    }
    // Else live data (no versions captured yet).
    Ok((live_graphs(state, dataset_id)?, None, true))
}

/// Graphs an organisation/group query may read: the union of the live graphs of
/// every dataset owned by that scope which the caller can access. Org/group
/// queries have no snapshot lineage, so they always read live data.
fn resolve_owner_union(
    state: &AppState,
    query: &SavedQuery,
    user_id: Option<&str>,
) -> Result<HashSet<String>, AppError> {
    let want = match query.scope {
        QueryScope::Organisation => OwnerType::Organisation,
        QueryScope::Group => OwnerType::Group,
        QueryScope::Dataset => return Ok(HashSet::new()),
    };
    let datasets = state
        .auth_db
        .list_datasets()
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut graphs = HashSet::new();
    for ds in datasets {
        if ds.owner_type != want || ds.owner_id != query.owner_id {
            continue;
        }
        if state
            .auth_db
            .can_access_dataset(user_id, &ds)
            .map_err(|e| AppError::Internal(e.to_string()))?
        {
            for g in state
                .auth_db
                .list_dataset_graphs(&ds.id)
                .map_err(|e| AppError::Internal(e.to_string()))?
            {
                graphs.insert(g);
            }
        }
    }
    Ok(graphs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_request_parsing() {
        assert_eq!(VersionRequest::parse(None), VersionRequest::Default);
        assert_eq!(VersionRequest::parse(Some("")), VersionRequest::Default);
        assert_eq!(
            VersionRequest::parse(Some("latest")),
            VersionRequest::Latest
        );
        assert_eq!(VersionRequest::parse(Some("live")), VersionRequest::Latest);
        assert_eq!(
            VersionRequest::parse(Some("1.2.0")),
            VersionRequest::Pinned("1.2.0".to_string())
        );
    }
}
