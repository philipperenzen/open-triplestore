//! Proposals kept for review (`docs/notes/repair-layer-design.md` §7.3):
//! files under `{data_dir}/repair-proposals/{dataset}/`, never the RDF store
//! — a store write would dirty the query accelerator and rebuild both of its
//! copies of the whole store, for a review artefact.
//!
//! Each proposal is `{hash}.json` (report, every action, PROV activity,
//! status) and `{hash}.patch`. The directory is not backed up and not
//! quarantined by recovery (no file ends in `.log`): a proposal is
//! ephemeral, like the text index, and can be computed again from its base
//! marker and rules. The newest 20 per dataset are kept, anything older than
//! `OTS_REPAIR_PROPOSAL_TTL_DAYS` days (default 30) expires on the next list or
//! insert, and a dataset that no longer exists loses its directory on the
//! next insert. States: `proposed → applied | rejected | superseded |
//! expired`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::proposal::Proposal;
use crate::server::AppState;

/// Proposals kept per dataset.
pub const KEEP_PER_DATASET: usize = 20;
pub const DEFAULT_TTL_DAYS: i64 = 30;

fn ttl_days() -> i64 {
    std::env::var("OTS_REPAIR_PROPOSAL_TTL_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(DEFAULT_TTL_DAYS)
}

/// A stored proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stored {
    pub id: String,
    pub dataset_id: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    /// The report as computed (its `status` is the one at creation).
    pub report: serde_json::Value,
    /// Every action, for paging.
    pub actions: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
}

impl Stored {
    pub fn base(&self) -> &serde_json::Value {
        &self.report["base"]
    }

    pub fn patch(&self) -> &str {
        self.report["patch"].as_str().unwrap_or("")
    }

    /// The summary a listing shows.
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "proposal_id": self.id,
            "status": self.status,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
            "summary": self.report["summary"],
            "partial": self.report["partial"],
            "base": self.report["base"],
            "applied_commit": self.applied_commit,
        })
    }
}

/// The hash part of a proposal id (`urn:ots:proposal:{hash}`), when it is
/// one: 32 hex digits.
pub fn hash_of(id: &str) -> Option<&str> {
    let h = id.strip_prefix("urn:ots:proposal:").unwrap_or(id);
    (h.len() == 32 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h)
}

/// A directory name for a dataset id: the id itself when it is plain, else a
/// digest of it (a dataset id never reaches the filesystem raw).
fn dataset_dir_name(dataset_id: &str) -> String {
    let plain = !dataset_id.is_empty()
        && dataset_id != "."
        && dataset_id != ".."
        && dataset_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if plain {
        dataset_id.to_string()
    } else {
        format!(
            "x-{}",
            &hex::encode(Sha256::digest(dataset_id.as_bytes()))[..24]
        )
    }
}

/// Where proposals of a dataset live, by storage.
enum Backend {
    Dir(PathBuf),
    Memory,
}

fn backend(state: &AppState, dataset_id: &str) -> Backend {
    match super::runtime(&state.store).proposal_root() {
        Some(root) => Backend::Dir(root.join(dataset_dir_name(dataset_id))),
        None => Backend::Memory,
    }
}

fn mem_key(dataset_id: &str, hash: &str) -> String {
    format!("{}/{hash}", dataset_dir_name(dataset_id))
}

fn write(state: &AppState, dataset_id: &str, stored: &Stored) -> anyhow::Result<()> {
    let hash = hash_of(&stored.id).ok_or_else(|| anyhow::anyhow!("not a proposal id"))?;
    let json = serde_json::to_vec_pretty(stored)?;
    match backend(state, dataset_id) {
        Backend::Dir(dir) => {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("dataset.txt"), dataset_id)?;
            // Written beside and renamed, so a reader never sees half a file.
            let tmp = dir.join(format!("{hash}.json.tmp"));
            std::fs::write(&tmp, &json)?;
            std::fs::rename(&tmp, dir.join(format!("{hash}.json")))?;
            std::fs::write(dir.join(format!("{hash}.patch")), stored.patch())?;
        }
        Backend::Memory => {
            let rt = super::runtime(&state.store);
            rt.memory()
                .lock()
                .map_err(|_| anyhow::anyhow!("proposal store poisoned"))?
                .insert(mem_key(dataset_id, hash), json);
        }
    }
    Ok(())
}

fn remove(state: &AppState, dataset_id: &str, hash: &str) {
    match backend(state, dataset_id) {
        Backend::Dir(dir) => {
            let _ = std::fs::remove_file(dir.join(format!("{hash}.json")));
            let _ = std::fs::remove_file(dir.join(format!("{hash}.patch")));
        }
        Backend::Memory => {
            if let Ok(mut m) = super::runtime(&state.store).memory().lock() {
                m.remove(&mem_key(dataset_id, hash));
            }
        }
    }
}

fn read_all(state: &AppState, dataset_id: &str) -> Vec<Stored> {
    let mut out: Vec<Stored> = Vec::new();
    match backend(state, dataset_id) {
        Backend::Dir(dir) => {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                return out;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&p) {
                    if let Ok(s) = serde_json::from_slice::<Stored>(&bytes) {
                        out.push(s);
                    }
                }
            }
        }
        Backend::Memory => {
            let prefix = format!("{}/", dataset_dir_name(dataset_id));
            if let Ok(m) = super::runtime(&state.store).memory().lock() {
                for (k, v) in m.iter() {
                    if k.starts_with(&prefix) {
                        if let Ok(s) = serde_json::from_slice::<Stored>(v) {
                            out.push(s);
                        }
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

/// Expire what is past the TTL and what is beyond the newest
/// [`KEEP_PER_DATASET`]; returns what is left, newest first.
fn prune(state: &AppState, dataset_id: &str) -> Vec<Stored> {
    let all = read_all(state, dataset_id);
    let cutoff = chrono::Utc::now() - chrono::Duration::days(ttl_days());
    let mut kept = Vec::new();
    for s in all {
        let too_old = chrono::DateTime::parse_from_rfc3339(&s.created_at)
            .map(|t| t.with_timezone(&chrono::Utc) < cutoff)
            .unwrap_or(false);
        if too_old || kept.len() >= KEEP_PER_DATASET {
            if let Some(h) = hash_of(&s.id) {
                remove(state, dataset_id, h);
            }
            continue;
        }
        kept.push(s);
    }
    kept
}

/// Directories of datasets that no longer exist go (dataset deletion has no
/// hook the repair layer may add, so the next insert cleans up).
fn sweep_orphans(state: &AppState) {
    let Some(root) = super::runtime(&state.store).proposal_root() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for e in entries.flatten() {
        let dir = e.path();
        let Ok(id) = std::fs::read_to_string(dir.join("dataset.txt")) else {
            continue;
        };
        if matches!(state.auth_db.get_dataset(id.trim()), Ok(None)) {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

/// Keep a proposal (`persist: true`).
pub fn save(state: &AppState, dataset_id: &str, proposal: &Proposal) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut report = proposal.report.clone();
    report["status"] = serde_json::json!("proposed");
    let stored = Stored {
        id: proposal.id.clone(),
        dataset_id: dataset_id.to_string(),
        status: "proposed".into(),
        created_at: now.clone(),
        updated_at: now,
        report,
        actions: proposal.actions.clone(),
        applied_commit: None,
        status_reason: None,
    };
    write(state, dataset_id, &stored)?;
    prune(state, dataset_id);
    sweep_orphans(state);
    Ok(())
}

/// The dataset's proposals, newest first (expired ones dropped).
pub fn list(state: &AppState, dataset_id: &str) -> Vec<Stored> {
    prune(state, dataset_id)
}

/// One proposal.
pub fn get(state: &AppState, dataset_id: &str, id: &str) -> Option<Stored> {
    let hash = hash_of(id)?;
    let s = match backend(state, dataset_id) {
        Backend::Dir(dir) => std::fs::read(dir.join(format!("{hash}.json")))
            .ok()
            .and_then(|b| serde_json::from_slice::<Stored>(&b).ok()),
        Backend::Memory => super::runtime(&state.store)
            .memory()
            .lock()
            .ok()
            .and_then(|m| m.get(&mem_key(dataset_id, hash)).cloned())
            .and_then(|b| serde_json::from_slice::<Stored>(&b).ok()),
    }?;
    (s.dataset_id == dataset_id).then_some(s)
}

/// Record a new status.
pub fn set_status(
    state: &AppState,
    mut stored: Stored,
    status: &str,
    reason: Option<String>,
    commit: Option<String>,
) -> anyhow::Result<Stored> {
    stored.status = status.to_string();
    stored.updated_at = chrono::Utc::now().to_rfc3339();
    if reason.is_some() {
        stored.status_reason = reason;
    }
    if commit.is_some() {
        stored.applied_commit = commit;
    }
    let ds = stored.dataset_id.clone();
    write(state, &ds, &stored)?;
    Ok(stored)
}

/// The graphs a proposal's base marker is about: the ones its run read,
/// or `fallback` for a proposal that does not record them.
pub fn base_graphs(stored: &Stored, fallback: &[String]) -> Vec<String> {
    let recorded: Vec<String> = stored.base()["graphs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|g| g.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if recorded.is_empty() {
        fallback.to_vec()
    } else {
        recorded
    }
}

/// Whether the dataset moved since the proposal's base marker (§7.3): a
/// change-log row touching its graphs after the base sequence (when both
/// carry one, in one epoch), else a newer commit than the base commit.
/// `graphs` is used when the proposal does not record the graphs it read.
pub fn is_stale(state: &AppState, stored: &Stored, graphs: &[String]) -> bool {
    let graphs = &base_graphs(stored, graphs);
    let base = stored.base();
    let changes = state.store.changes();
    if let (Some(seq), Some(epoch)) = (base["sequence"].as_i64(), base["epoch"].as_str()) {
        if changes.enabled() && changes.epoch() == epoch {
            return touched_since(state, graphs, seq);
        }
    }
    let base_commit = base["commit"].as_str();
    let newest = super::handlers::newest_commit(state, graphs);
    newest.as_deref() != base_commit
}

/// Whether a change-log row after `seq` touches `graphs` (or is
/// store-scoped, which bears on every graph).
pub fn touched_since(state: &AppState, graphs: &[String], seq: i64) -> bool {
    let changes = state.store.changes();
    if graphs.is_empty() {
        return !changes.rows_after(seq, 1).is_empty();
    }
    graphs
        .iter()
        .any(|g| !changes.rows_after_in(seq, 1, Some(g.as_str())).is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposal(n: usize) -> Proposal {
        let id = format!("urn:ots:proposal:{n:032x}");
        Proposal {
            id: id.clone(),
            patch: format!("H id <{id}> .\nTX .\nTC .\n"),
            report: serde_json::json!({ "proposal_id": id, "patch": format!("H id <{id}> .\nTX .\nTC .\n"), "base": {} }),
            actions: Vec::new(),
        }
    }

    /// The newest [`KEEP_PER_DATASET`] are kept; one past the TTL expires.
    #[test]
    fn retention_keeps_the_newest_and_expires_the_old() {
        let state =
            AppState::test_default_with_store(crate::store::TripleStore::in_memory().unwrap());
        for n in 0..(KEEP_PER_DATASET + 3) {
            save(&state, "ds", &proposal(n)).unwrap();
            // created_at has nanosecond precision; keep the order strict.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let kept = list(&state, "ds");
        assert_eq!(kept.len(), KEEP_PER_DATASET);
        assert_eq!(
            kept[0].id,
            proposal(KEEP_PER_DATASET + 2).id,
            "newest first"
        );
        assert!(
            get(&state, "ds", &proposal(0).id).is_none(),
            "the oldest went"
        );
        // Past the TTL.
        let old = get(&state, "ds", &proposal(KEEP_PER_DATASET + 2).id).unwrap();
        let mut aged = old.clone();
        aged.created_at =
            (chrono::Utc::now() - chrono::Duration::days(DEFAULT_TTL_DAYS + 1)).to_rfc3339();
        write(&state, "ds", &aged).unwrap();
        assert_eq!(list(&state, "ds").len(), KEEP_PER_DATASET - 1);
        // Another dataset's proposals are its own.
        assert!(get(&state, "other", &proposal(5).id).is_none());
    }

    #[test]
    fn proposal_ids_and_directory_names() {
        let h = "0123456789abcdef0123456789abcdef";
        assert_eq!(hash_of(&format!("urn:ots:proposal:{h}")), Some(h));
        assert_eq!(hash_of(h), Some(h));
        assert_eq!(hash_of("urn:ots:proposal:../../etc"), None);
        assert_eq!(hash_of("urn:ots:proposal:short"), None);
        assert_eq!(dataset_dir_name("ds-1_a"), "ds-1_a");
        assert!(dataset_dir_name("../x").starts_with("x-"));
        assert!(dataset_dir_name("a/b").starts_with("x-"));
        assert!(dataset_dir_name("..").starts_with("x-"));
    }
}
