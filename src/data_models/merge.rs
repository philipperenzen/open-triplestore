//! Three-way merge over RDF triples at (subject, predicate) granularity.
//!
//! Given a common ancestor (`base`), a source branch tip (`ours`), and a target
//! (`theirs`), each (subject, predicate) key is resolved by comparing the set of
//! objects on each side against the ancestor:
//!
//! - if only one side changed the objects → take that side (auto-merge);
//! - if neither changed → keep the ancestor's objects;
//! - if both changed to *different* object sets → a conflict needing resolution.
//!
//! The model is registry-agnostic: callers flatten each version's graphs into a
//! triple list (see [`super::diff::collect_triples`]) and supply them here.

use std::collections::{BTreeMap, BTreeSet};

type Triple = (String, String, String);
type SpKey = (String, String); // (subject, predicate)
type ObjSet = BTreeSet<String>;

/// One unresolved (subject, predicate) conflict.
#[derive(Debug, serde::Serialize)]
pub struct MergeConflict {
    pub subject: String,
    pub predicate: String,
    pub base: Vec<String>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
}

/// Result of a merge preview.
#[derive(Debug, serde::Serialize)]
pub struct MergePreview {
    pub base_version: Option<String>,
    pub clean: bool,
    pub conflicts: Vec<MergeConflict>,
    /// Triples the auto-merge adds relative to the target (`into`).
    pub auto_added: usize,
    /// Triples the auto-merge removes relative to the target (`into`).
    pub auto_removed: usize,
}

/// A caller-supplied resolution for one conflicting (subject, predicate).
#[derive(Debug, serde::Deserialize)]
pub struct ConflictResolution {
    pub subject: String,
    pub predicate: String,
    /// "ours" | "theirs" | "base" | "custom"
    pub choice: String,
    /// Object terms (N-Triples form) when `choice == "custom"`.
    #[serde(default)]
    pub objects: Vec<String>,
}

/// Query params for `GET .../merge/preview`.
#[derive(Debug, serde::Deserialize)]
pub struct MergeParams {
    pub from: String,
    pub into: String,
}

/// Body for `POST .../merge`.
#[derive(Debug, serde::Deserialize)]
pub struct MergeRequest {
    pub from: String,
    pub into: String,
    #[serde(default)]
    pub target_version: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub resolutions: Vec<ConflictResolution>,
    /// Optional commit message recorded with the merge (falls back to a default).
    #[serde(default)]
    pub message: Option<String>,
}

/// Lowest common ancestor: the first version in `ours_chain` (self-first,
/// oldest-last) that also appears in `theirs_chain`.
pub fn lca(ours_chain: &[String], theirs_chain: &[String]) -> Option<String> {
    let theirs: BTreeSet<&String> = theirs_chain.iter().collect();
    ours_chain.iter().find(|v| theirs.contains(v)).cloned()
}

fn index(triples: &[Triple]) -> BTreeMap<SpKey, ObjSet> {
    let mut map: BTreeMap<SpKey, ObjSet> = BTreeMap::new();
    for (s, p, o) in triples {
        map.entry((s.clone(), p.clone()))
            .or_default()
            .insert(o.clone());
    }
    map
}

/// Compute conflicts and the auto-merged object map for non-conflicting keys.
/// Conflict keys are omitted from the returned map (resolved separately).
pub fn three_way(
    base: &[Triple],
    ours: &[Triple],
    theirs: &[Triple],
) -> (Vec<MergeConflict>, BTreeMap<SpKey, ObjSet>) {
    let bi = index(base);
    let oi = index(ours);
    let ti = index(theirs);

    let mut keys: BTreeSet<SpKey> = BTreeSet::new();
    keys.extend(bi.keys().cloned());
    keys.extend(oi.keys().cloned());
    keys.extend(ti.keys().cloned());

    let empty = ObjSet::new();
    let mut conflicts = Vec::new();
    let mut merged: BTreeMap<SpKey, ObjSet> = BTreeMap::new();

    for k in keys {
        let b = bi.get(&k).unwrap_or(&empty);
        let o = oi.get(&k).unwrap_or(&empty);
        let t = ti.get(&k).unwrap_or(&empty);
        let ours_changed = o != b;
        let theirs_changed = t != b;

        if ours_changed && theirs_changed && o != t {
            conflicts.push(MergeConflict {
                subject: k.0.clone(),
                predicate: k.1.clone(),
                base: b.iter().cloned().collect(),
                ours: o.iter().cloned().collect(),
                theirs: t.iter().cloned().collect(),
            });
        } else if ours_changed {
            if !o.is_empty() {
                merged.insert(k, o.clone());
            }
        } else if theirs_changed {
            if !t.is_empty() {
                merged.insert(k, t.clone());
            }
        } else if !b.is_empty() {
            merged.insert(k, b.clone());
        }
    }

    (conflicts, merged)
}

/// Build a preview: conflicts plus auto-merge add/remove counts vs the target.
pub fn preview(
    base_version: Option<String>,
    base: &[Triple],
    ours: &[Triple],
    theirs: &[Triple],
) -> MergePreview {
    let (conflicts, merged) = three_way(base, ours, theirs);
    let merged_triples = flatten(&merged);
    let merged_set: BTreeSet<&Triple> = merged_triples.iter().collect();
    let theirs_set: BTreeSet<&Triple> = theirs.iter().collect();
    let auto_added = merged_set.difference(&theirs_set).count();
    let auto_removed = theirs_set.difference(&merged_set).count();
    MergePreview {
        base_version,
        clean: conflicts.is_empty(),
        conflicts,
        auto_added,
        auto_removed,
    }
}

/// Apply conflict resolutions to the auto-merged map, producing the final triple
/// set for the merged version.
pub fn resolve(
    base: &[Triple],
    ours: &[Triple],
    theirs: &[Triple],
    resolutions: &[ConflictResolution],
) -> Vec<Triple> {
    let (conflicts, mut merged) = three_way(base, ours, theirs);
    let bi = index(base);
    let oi = index(ours);
    let ti = index(theirs);
    let empty = ObjSet::new();

    let res_by_key: BTreeMap<SpKey, &ConflictResolution> = resolutions
        .iter()
        .map(|r| ((r.subject.clone(), r.predicate.clone()), r))
        .collect();

    for c in &conflicts {
        let k = (c.subject.clone(), c.predicate.clone());
        let chosen: ObjSet = match res_by_key.get(&k).map(|r| r.choice.as_str()) {
            Some("ours") => oi.get(&k).unwrap_or(&empty).clone(),
            Some("theirs") => ti.get(&k).unwrap_or(&empty).clone(),
            Some("base") => bi.get(&k).unwrap_or(&empty).clone(),
            Some("custom") => res_by_key[&k].objects.iter().cloned().collect(),
            // Unresolved conflict defaults to "ours" so apply never silently drops data.
            _ => oi.get(&k).unwrap_or(&empty).clone(),
        };
        if !chosen.is_empty() {
            merged.insert(k, chosen);
        }
    }

    flatten(&merged)
}

fn flatten(map: &BTreeMap<SpKey, ObjSet>) -> Vec<Triple> {
    let mut out = Vec::new();
    for ((s, p), objs) in map {
        for o in objs {
            out.push((s.clone(), p.clone(), o.clone()));
        }
    }
    out
}
