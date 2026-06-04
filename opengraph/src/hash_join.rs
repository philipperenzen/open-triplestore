//! Hash-join over SPARQL solution sequences.
//!
//! A hash join materialises the *smaller* operand into a `HashMap` keyed on
//! the join variables, then probes with each row of the *larger* operand.
//! This gives O(n + m) complexity vs O(n·m) for the nested-loop join that
//! SPARQL engines commonly fall back to for large intermediate results.
//!
//! # When to use
//!
//! Use hash join when both operands return **> ~1 000 rows** and the join
//! variables are a strict subset of the projection.  For OPTIONAL / MINUS
//! patterns, keep the nested-loop join (hash join does not support nullability
//! semantics without extra bookkeeping).
//!
//! # Example
//!
//! ```rust
//! use opengraph::hash_join::HashJoin;
//!
//! // Simulate two SPARQL result sets as Vec<HashMap<varname, value>>
//! let build = vec![
//!     [("x".to_string(), "Alice".to_string())].into(),
//!     [("x".to_string(), "Bob".to_string())].into(),
//! ];
//! let probe = vec![
//!     [("x".to_string(), "Alice".to_string()), ("y".to_string(), "42".to_string())].into(),
//!     [("x".to_string(), "Carol".to_string()), ("y".to_string(), "7".to_string())].into(),
//! ];
//! let join_vars = vec!["x".to_string()];
//! let results = HashJoin::join(build, probe, &join_vars);
//! assert_eq!(results.len(), 1);  // only Alice matches
//! assert_eq!(results[0]["y"], "42");
//! ```

use std::collections::HashMap;

/// A solution row: variable name → string value.
pub type Row = HashMap<String, String>;

/// Hash-join engine for SPARQL solution sequences.
pub struct HashJoin;

impl HashJoin {
    /// Join two solution sequences on the given join variables.
    ///
    /// The `build` side is materialised into a hash table.  The `probe` side
    /// streams through and looks up matching rows.  Returns all compatible
    /// combined rows (natural join semantics: variables not in the join key
    /// are merged if they are compatible or absent).
    ///
    /// **Compatibility**: two rows are compatible if, for every variable that
    /// appears in *both*, the values are equal.
    pub fn join(build: Vec<Row>, probe: Vec<Row>, join_vars: &[String]) -> Vec<Row> {
        // Phase 1 — build: group build-side rows by join-key tuple.
        let mut table: HashMap<Vec<String>, Vec<Row>> = HashMap::new();
        for row in build {
            let key: Vec<String> = join_vars
                .iter()
                .map(|v| row.get(v).cloned().unwrap_or_default())
                .collect();
            table.entry(key).or_default().push(row);
        }

        // Phase 2 — probe: for each probe row, look up matching build rows.
        let mut results = Vec::new();
        for probe_row in probe {
            let key: Vec<String> = join_vars
                .iter()
                .map(|v| probe_row.get(v).cloned().unwrap_or_default())
                .collect();

            if let Some(build_rows) = table.get(&key) {
                for build_row in build_rows {
                    if let Some(merged) = merge_compatible(build_row, &probe_row) {
                        results.push(merged);
                    }
                }
            }
        }
        results
    }

    /// Estimate whether hash join is beneficial over nested-loop join.
    ///
    /// Returns `true` when both sides exceed the threshold (default 1 000 rows).
    pub fn should_use(build_est: usize, probe_est: usize) -> bool {
        const THRESHOLD: usize = 1_000;
        build_est > THRESHOLD || probe_est > THRESHOLD
    }

    /// Choose which side to build the hash table from (smaller = build side).
    ///
    /// Returns `true` if `left_est ≤ right_est` (i.e. left should be build side).
    pub fn build_side_is_left(left_est: usize, right_est: usize) -> bool {
        left_est <= right_est
    }
}

/// Merge two rows if they are compatible (no conflicting bindings).
/// Returns `None` if there is a conflict.
fn merge_compatible(a: &Row, b: &Row) -> Option<Row> {
    let mut merged = a.clone();
    for (var, val) in b {
        match merged.get(var) {
            Some(existing) if existing != val => return None,
            _ => {
                merged.insert(var.clone(), val.clone());
            }
        }
    }
    Some(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn test_basic_join() {
        let build = vec![row(&[("x", "Alice")]), row(&[("x", "Bob")])];
        let probe = vec![
            row(&[("x", "Alice"), ("y", "42")]),
            row(&[("x", "Carol"), ("y", "7")]),
        ];
        let results = HashJoin::join(build, probe, &["x".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["y"], "42");
        assert_eq!(results[0]["x"], "Alice");
    }

    #[test]
    fn test_no_match() {
        let build = vec![row(&[("x", "Alice")])];
        let probe = vec![row(&[("x", "Bob"), ("y", "1")])];
        let results = HashJoin::join(build, probe, &["x".to_string()]);
        assert!(results.is_empty());
    }

    #[test]
    fn test_multi_var_join() {
        let build = vec![row(&[("x", "A"), ("y", "B")])];
        let probe = vec![
            row(&[("x", "A"), ("y", "B"), ("z", "C")]),
            row(&[("x", "A"), ("y", "X"), ("z", "D")]),
        ];
        let results = HashJoin::join(build, probe, &["x".to_string(), "y".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["z"], "C");
    }

    #[test]
    fn test_many_to_many() {
        // x=1 matches twice in build
        let build = vec![
            row(&[("x", "1"), ("a", "p")]),
            row(&[("x", "1"), ("a", "q")]),
        ];
        let probe = vec![row(&[("x", "1"), ("b", "r")])];
        let results = HashJoin::join(build, probe, &["x".to_string()]);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_compatible_merge() {
        let a = row(&[("x", "1"), ("y", "2")]);
        let b = row(&[("x", "1"), ("z", "3")]);
        let merged = merge_compatible(&a, &b).unwrap();
        assert_eq!(merged["z"], "3");
    }

    #[test]
    fn test_incompatible_conflict() {
        let a = row(&[("x", "1")]);
        let b = row(&[("x", "2")]);
        assert!(merge_compatible(&a, &b).is_none());
    }

    #[test]
    fn test_should_use_threshold() {
        assert!(!HashJoin::should_use(100, 100));
        assert!(HashJoin::should_use(2000, 100));
        assert!(HashJoin::should_use(100, 2000));
    }

    #[test]
    fn test_build_side_selection() {
        assert!(HashJoin::build_side_is_left(500, 1000));
        assert!(!HashJoin::build_side_is_left(1000, 500));
    }
}
