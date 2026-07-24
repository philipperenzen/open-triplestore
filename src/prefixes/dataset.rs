//! Bundled prefix dataset — the internal replacement for live prefix.cc.
//!
//! The snapshot at `data/prefixes-snapshot.json` (embedded at compile time)
//! merges the full prefix.cc dump with the `vann:preferredNamespacePrefix`
//! pairs from the Linked Open Vocabularies catalog, pre-validated by
//! `scripts/build_prefix_dataset.py` against the same label/namespace rules
//! the registry enforces.  Entries keep the prefix.cc popularity order as a
//! `rank`, which drives search ordering and reverse-lookup tie-breaks
//! (prefix.cc itself resolves a namespace to its most-used prefix).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Where a prefix mapping came from, in resolution-priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PrefixSource {
    /// Registered on this triplestore (model/vocabulary registry).
    Platform,
    /// The bundled prefix.cc snapshot.
    #[serde(rename = "prefix.cc")]
    PrefixCc,
    /// The bundled LOV catalog (prefixes prefix.cc does not know).
    Lov,
    /// Confirmed earlier via the optional live prefix.cc fallback.
    Cache,
}

/// One prefix → namespace mapping.
#[derive(Debug, Clone, Serialize)]
pub struct PrefixEntry {
    pub prefix: String,
    pub namespace: String,
    /// Popularity rank (1 = most used on prefix.cc; LOV additions follow).
    pub rank: u32,
    pub source: PrefixSource,
}

#[derive(Deserialize)]
struct SnapshotEntry {
    prefix: String,
    namespace: String,
    rank: u32,
    source: String,
}

#[derive(Deserialize)]
struct Snapshot {
    format_version: u32,
    prefixes: Vec<SnapshotEntry>,
}

/// Immutable, in-memory view of the bundled snapshot.
pub struct PrefixDataset {
    entries: Vec<PrefixEntry>,
    by_label: HashMap<String, usize>,
    /// Namespace → best-ranked entry index.
    by_namespace: HashMap<String, usize>,
}

const SNAPSHOT_JSON: &str = include_str!("data/prefixes-snapshot.json");

impl PrefixDataset {
    /// Load the compile-time embedded snapshot.
    ///
    /// Panics only on a corrupt embedded asset, which `cargo test` catches.
    pub fn bundled() -> Self {
        Self::from_json(SNAPSHOT_JSON).expect("embedded prefixes-snapshot.json is valid")
    }

    /// An empty dataset (tests that need a registry with no local knowledge).
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            by_label: HashMap::new(),
            by_namespace: HashMap::new(),
        }
    }

    fn from_json(json: &str) -> anyhow::Result<Self> {
        let snapshot: Snapshot = serde_json::from_str(json)?;
        anyhow::ensure!(
            snapshot.format_version == 1,
            "unsupported prefix snapshot format {}",
            snapshot.format_version
        );
        let mut entries = Vec::with_capacity(snapshot.prefixes.len());
        for e in snapshot.prefixes {
            let source = match e.source.as_str() {
                "prefix.cc" => PrefixSource::PrefixCc,
                "lov" => PrefixSource::Lov,
                other => anyhow::bail!("unknown prefix source {other:?}"),
            };
            entries.push(PrefixEntry {
                prefix: e.prefix,
                namespace: e.namespace,
                rank: e.rank,
                source,
            });
        }
        // Ranked order so "first match wins" below means "most popular wins".
        entries.sort_by_key(|e| e.rank);
        let mut by_label = HashMap::with_capacity(entries.len());
        let mut by_namespace = HashMap::with_capacity(entries.len());
        for (i, e) in entries.iter().enumerate() {
            by_label.entry(e.prefix.clone()).or_insert(i);
            by_namespace.entry(e.namespace.clone()).or_insert(i);
        }
        Ok(Self {
            entries,
            by_label,
            by_namespace,
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    // Part of the len/is_empty pair (clippy: len_without_is_empty); the binary
    // target only calls len().
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn lookup(&self, label: &str) -> Option<&PrefixEntry> {
        self.by_label.get(label).map(|&i| &self.entries[i])
    }

    /// Exact-namespace reverse lookup (most popular prefix wins).
    pub fn reverse(&self, namespace: &str) -> Option<&PrefixEntry> {
        self.by_namespace.get(namespace).map(|&i| &self.entries[i])
    }

    /// All entries in popularity order.
    pub fn entries(&self) -> &[PrefixEntry] {
        &self.entries
    }

    /// Ranked substring search over labels and namespaces.
    ///
    /// Label matches outrank namespace matches; within a tier, exact >
    /// starts-with > contains, with the popularity rank as tie-break.
    pub fn search<'a>(&'a self, query: &str, limit: usize) -> Vec<&'a PrefixEntry> {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return self.entries.iter().take(limit).collect();
        }
        let mut scored: Vec<(i64, &PrefixEntry)> = self
            .entries
            .iter()
            .filter_map(|e| {
                let label = e.prefix.to_ascii_lowercase();
                let score = if label == q {
                    4_000
                } else if label.starts_with(&q) {
                    3_000 - label.len() as i64
                } else if label.contains(&q) {
                    2_000 - label.len() as i64
                } else if e.namespace.to_ascii_lowercase().contains(&q) {
                    1_000
                } else {
                    return None;
                };
                Some((score, e))
            })
            .collect();
        scored.sort_by_key(|(score, e)| (-score, e.rank));
        scored.into_iter().take(limit).map(|(_, e)| e).collect()
    }

    /// The namespace whose IRI is the longest prefix of `iri`, if any.
    ///
    /// Used for CURIE shrinking: `http://xmlns.com/foaf/0.1/name` →
    /// the `foaf` entry plus local name `name`.
    pub fn shrink<'a>(&'a self, iri: &str) -> Option<(&'a PrefixEntry, String)> {
        let mut best: Option<(&PrefixEntry, usize)> = None;
        for e in &self.entries {
            if iri.starts_with(e.namespace.as_str()) && iri.len() > e.namespace.len() {
                let ns_len = e.namespace.len();
                let better = match best {
                    Some((_, best_len)) => ns_len > best_len,
                    None => true,
                };
                if better {
                    best = Some((e, ns_len));
                }
            }
        }
        best.map(|(e, ns_len)| (e, iri[ns_len..].to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_snapshot_loads() {
        let ds = PrefixDataset::bundled();
        assert!(
            ds.len() > 3_000,
            "expected >3000 prefixes, got {}",
            ds.len()
        );
    }

    #[test]
    fn well_known_lookups() {
        let ds = PrefixDataset::bundled();
        assert_eq!(
            ds.lookup("foaf").map(|e| e.namespace.as_str()),
            Some("http://xmlns.com/foaf/0.1/")
        );
        assert_eq!(
            ds.lookup("rdf").map(|e| e.namespace.as_str()),
            Some("http://www.w3.org/1999/02/22-rdf-syntax-ns#")
        );
        assert!(ds.lookup("no-such-prefix-xyz").is_none());
    }

    #[test]
    fn reverse_prefers_popular_prefix() {
        let ds = PrefixDataset::bundled();
        // Multiple prefixes map to the FOAF namespace; the popular one wins.
        assert_eq!(
            ds.reverse("http://xmlns.com/foaf/0.1/")
                .map(|e| e.prefix.as_str()),
            Some("foaf")
        );
    }

    #[test]
    fn search_ranks_exact_before_substring() {
        let ds = PrefixDataset::bundled();
        let hits = ds.search("foaf", 10);
        assert_eq!(hits[0].prefix, "foaf");
        assert!(hits.len() > 1, "substring matches should follow");
    }

    #[test]
    fn shrink_longest_namespace_wins() {
        let ds = PrefixDataset::bundled();
        let (entry, local) = ds
            .shrink("http://xmlns.com/foaf/0.1/name")
            .expect("shrinks");
        assert_eq!(entry.prefix, "foaf");
        assert_eq!(local, "name");
        assert!(ds.shrink("http://example.invalid/nothing#x").is_none());
    }
}
