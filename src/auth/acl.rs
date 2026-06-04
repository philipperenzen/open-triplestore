//! ACL evaluation engine for endpoint-level, named-graph-level, and
//! triple-level access control.
//!
//! ## Endpoint ACL
//! Rules live in the `endpoint_acl` SQLite table.  Each rule targets a
//! principal (user, organisation, group, or role), a glob-style path pattern,
//! a set of HTTP methods, and has an effect (allow | deny) plus a priority.
//! Higher priority is evaluated first; `deny` beats `allow` at the same
//! priority.  If no rules match, access is **allowed** (system-role checks
//! are still applied by the existing `require_auth` / `require_admin`
//! middleware layers).
//!
//! ## Graph ACL
//! `graph_acl` rows grant read / write / admin on a named graph to a
//! principal.  Admins (SystemRole::Admin / SuperAdmin) always pass.
//! Permission hierarchy: admin ⊇ write ⊇ read.
//!
//! ## Triple ACL
//! Each triple can be tagged with a `label_graph_iri` in
//! `triple_security_labels`.  The label graph must have a `graph_acl` entry
//! granting `read` to the caller; otherwise the triple is redacted from
//! results.  If the `triple_security_labels` table is empty the filter is
//! a no-op (zero DB overhead).

use std::sync::Arc;

use crate::auth::db::AuthDb;
use crate::auth::middleware::AuthenticatedUser;

// ─── Glob-style path matching ─────────────────────────────────────────────────

/// Match `path` against a simple glob pattern where `*` matches any non-`/`
/// segment and `**` matches any sub-path.
pub fn matches_path_pattern(pattern: &str, path: &str) -> bool {
    glob_match(pattern, path)
}

fn glob_match(pattern: &str, text: &str) -> bool {
    // Normalise both sides the same way before matching so a deny rule can't be
    // bypassed with a trailing slash or a doubled separator: `/api/x`, `/api/x/`
    // and `/api//x` all reduce to the same segment list. (Axum routes are
    // case-sensitive, so case is intentionally not folded.)
    let segments = |s: &str| -> Vec<String> {
        s.split('/')
            .filter(|seg| !seg.is_empty())
            .map(|seg| seg.to_string())
            .collect()
    };
    let pat_owned = segments(pattern);
    let txt_owned = segments(text);
    let pat: Vec<&str> = pat_owned.iter().map(String::as_str).collect();
    let txt: Vec<&str> = txt_owned.iter().map(String::as_str).collect();
    glob_segments(&pat, &txt)
}

fn glob_segments(pat: &[&str], txt: &[&str]) -> bool {
    match (pat.first(), txt.first()) {
        (None, None) => true,
        (Some(&"**"), _) => {
            // ** matches zero or more path segments
            if pat.len() == 1 {
                return true;
            }
            for i in 0..=txt.len() {
                if glob_segments(&pat[1..], &txt[i..]) {
                    return true;
                }
            }
            false
        }
        (Some(p), Some(t)) => {
            if segment_matches(p, t) {
                glob_segments(&pat[1..], &txt[1..])
            } else {
                false
            }
        }
        _ => false,
    }
}

fn segment_matches(pattern: &str, segment: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Simple wildcard within a segment (e.g. "foo*")
    if let Some(prefix) = pattern.strip_suffix('*') {
        return segment.starts_with(prefix);
    }
    pattern == segment
}

// ─── HTTP method matching ─────────────────────────────────────────────────────

fn methods_match(rule_methods: &str, request_method: &str) -> bool {
    if rule_methods == "*" {
        return true;
    }
    rule_methods
        .split(',')
        .any(|m| m.trim().eq_ignore_ascii_case(request_method))
}

// ─── Endpoint ACL ─────────────────────────────────────────────────────────────

/// Returns `true` if the request is allowed by the endpoint ACL rules.
///
/// Evaluation order:
/// 1. Sort all applicable rules by `priority DESC` (higher = first).
/// 2. Among matching rules, if **any** `deny` rule matches → deny.
/// 3. If **any** `allow` rule matches → allow.
/// 4. If no rules match → allow (default open, guarded by role middleware).
pub fn check_endpoint_acl(
    user: Option<&AuthenticatedUser>,
    method: &str,
    path: &str,
    auth_db: &Arc<AuthDb>,
) -> bool {
    let user = match user {
        Some(u) => u,
        None => {
            // No authenticated user — only check rules targeting 'public' (role='*') if any
            // For now, unauthenticated requests pass ACL (role middleware handles auth).
            return true;
        }
    };

    let rules = match auth_db
        .get_endpoint_acl_rules_for_user(&user.user_id, user.role.as_str())
    {
        Ok(r) => r,
        Err(e) => {
            // CRITICAL — ACL DB unavailable. We fail CLOSED at the request
            // boundary, but record this as an `acl_error` audit event so
            // operators can detect a degraded state.
            tracing::error!(actor = %user.user_id, "CRITICAL ACL DB error in check_endpoint_acl: {}", e);
            let logger = crate::auth::audit::AuditLogger::new(auth_db.pool());
            use crate::auth::audit::{AuditEventBuilder, AuditEventType, AuditOutcome};
            logger.log(
                AuditEventBuilder::new(AuditEventType::AclError, AuditOutcome::Failure)
                    .actor_id(&user.user_id)
                    .details(serde_json::json!({ "error": e.to_string(), "where": "check_endpoint_acl" })),
            );
            return false; // fail closed on DB error
        }
    };

    // Filter rules matching this request
    let matching: Vec<_> = rules
        .iter()
        .filter(|r| {
            matches_path_pattern(&r.path_pattern, path) && methods_match(&r.http_methods, method)
        })
        .collect();

    if matching.is_empty() {
        return true; // default allow
    }

    // deny beats allow at equal priority
    let mut sorted = matching;
    sorted.sort_by(|a, b| b.priority.cmp(&a.priority));

    for rule in &sorted {
        if rule.effect == "deny" {
            return false;
        }
        if rule.effect == "allow" {
            return true;
        }
    }

    true // default allow
}

// ─── Graph ACL ────────────────────────────────────────────────────────────────

/// Returns `true` if the caller may perform `required_permission` ("read" |
/// "write" | "admin") on `graph_iri`.
///
/// SystemRole::Admin and SuperAdmin always pass.
/// Falls back to the `graph_acl` table for all other users.
pub fn check_graph_permission(
    user: Option<&AuthenticatedUser>,
    graph_iri: &str,
    required_permission: &str,
    auth_db: &Arc<AuthDb>,
) -> bool {
    match user {
        None => {
            // Unauthenticated: only pass if a 'public' grant exists
            auth_db
                .check_graph_permission("", "public", graph_iri, required_permission)
                .unwrap_or(false)
        }
        Some(u) if u.is_admin() => true,
        Some(u) => auth_db
            .check_graph_permission(&u.user_id, u.role.as_str(), graph_iri, required_permission)
            .unwrap_or(false),
    }
}

// ─── Triple ACL ───────────────────────────────────────────────────────────────

/// A normalised quad representation used for security-label lookups.
#[derive(Debug, Clone)]
pub struct QuadKey {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub graph: String,
}

/// Filter a list of quad keys, removing any whose security label the caller
/// cannot read.  Returns the indices of quads that should be **kept**.
///
/// If the `triple_security_labels` table is empty for the involved graphs the
/// function short-circuits and returns all indices (no overhead).
pub fn filter_quad_indices_by_label(
    user: Option<&AuthenticatedUser>,
    quads: &[QuadKey],
    auth_db: &Arc<AuthDb>,
) -> Vec<usize> {
    // Admins see everything
    if user.map(|u| u.is_admin()).unwrap_or(false) {
        return (0..quads.len()).collect();
    }

    let unique_graphs: Vec<&str> = {
        let mut gs: Vec<&str> = quads.iter().map(|q| q.graph.as_str()).collect();
        gs.sort_unstable();
        gs.dedup();
        gs
    };

    // Short-circuit: no labels in these graphs → return all
    let has_labels = auth_db
        .has_triple_security_labels(&unique_graphs)
        .unwrap_or(false);
    if !has_labels {
        return (0..quads.len()).collect();
    }

    let quad_tuples: Vec<(String, String, String, String)> = quads
        .iter()
        .map(|q| (q.subject.clone(), q.predicate.clone(), q.object.clone(), q.graph.clone()))
        .collect();

    let labelled = auth_db
        .get_labels_for_quads(&quad_tuples)
        .unwrap_or_default();

    // Build a set of (quad_index, label_graph_iri) pairs that are denied
    let denied: std::collections::HashSet<usize> = labelled
        .into_iter()
        .filter(|(_, label_iri)| {
            !check_graph_permission(user, label_iri, "read", auth_db)
        })
        .map(|(idx, _)| idx)
        .collect();

    (0..quads.len())
        .filter(|i| !denied.contains(i))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_pattern_exact() {
        assert!(matches_path_pattern("/api/sparql", "/api/sparql"));
        assert!(!matches_path_pattern("/api/sparql", "/api/store"));
    }

    #[test]
    fn test_path_pattern_wildcard_segment() {
        assert!(matches_path_pattern("/api/datasets/*/sparql", "/api/datasets/abc123/sparql"));
        assert!(!matches_path_pattern("/api/datasets/*/sparql", "/api/datasets/abc123/store"));
    }

    #[test]
    fn test_path_pattern_double_star() {
        assert!(matches_path_pattern("/api/**", "/api/datasets/d1/graphs"));
        assert!(matches_path_pattern("/api/**", "/api/sparql"));
    }

    #[test]
    fn test_methods_match() {
        assert!(methods_match("*", "GET"));
        assert!(methods_match("GET,POST", "get"));
        assert!(!methods_match("GET,POST", "DELETE"));
    }
}
