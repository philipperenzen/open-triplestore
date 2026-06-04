//! Account-tier authorization helpers.
//!
//! Per-resource access (datasets, models, vocabularies) is resolved in
//! [`crate::auth::db`] via the `effective_*_role` / `can_*` methods, which sit
//! on top of [`crate::auth::models::AccessLevel`]. This module holds the
//! complementary **account-tier** rules — the admin gate and the role-hierarchy
//! invariants that prevent privilege escalation — so they are defined once and
//! named by intent rather than re-derived inline at every call site.
//!
//! Helpers come in two shapes:
//! * `require_*` return a ready-to-`?` `Result<(), Denied>` for handler guards.
//! * `can_*` return a plain `bool` so callers can attach a context-specific
//!   error message (e.g. "Cannot deactivate user with equal or higher role").

use axum::http::StatusCode;

use super::middleware::AuthenticatedUser;
use super::models::SystemRole;

/// A `(status, message)` rejection — the exact error shape auth handlers return.
pub type Denied = (StatusCode, String);

fn forbidden(msg: &str) -> Denied {
    (StatusCode::FORBIDDEN, msg.to_string())
}

/// Require that `actor` is admin-level or above (admin or super-admin).
pub fn require_admin(actor: &AuthenticatedUser) -> Result<(), Denied> {
    if actor.is_admin() {
        Ok(())
    } else {
        Err(forbidden("Admin access required"))
    }
}

/// May `actor` confer/assign the account role `role`?
///
/// You can only grant a role up to your own tier — this is the no-privilege-
/// escalation rule, and it also blocks self-promotion (a user assigning
/// themselves a stronger role).
pub fn can_grant_role(actor: &AuthenticatedUser, role: SystemRole) -> bool {
    role.level() <= actor.role.level()
}

/// May `actor` administer (modify / reset / deactivate / purge) the account
/// identified by `target_id` with role `target_role`?
///
/// The hierarchy rule: you may act on accounts **strictly below** your own
/// tier, and always on your own account. (Action-specific rules — e.g. "you
/// cannot deactivate yourself" — are enforced separately by the caller.)
pub fn can_administer_user(
    actor: &AuthenticatedUser,
    target_id: &str,
    target_role: SystemRole,
) -> bool {
    target_id == actor.user_id || target_role.level() < actor.role.level()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(id: &str, role: SystemRole) -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: id.to_string(),
            role,
            can_publish: false,
            write_access: true,
        }
    }

    #[test]
    fn require_admin_gates() {
        assert!(require_admin(&actor("a", SystemRole::Admin)).is_ok());
        assert!(require_admin(&actor("s", SystemRole::SuperAdmin)).is_ok());
        assert!(require_admin(&actor("u", SystemRole::User)).is_err());
    }

    #[test]
    fn can_grant_role_blocks_escalation() {
        let admin = actor("a", SystemRole::Admin);
        assert!(can_grant_role(&admin, SystemRole::User));
        assert!(can_grant_role(&admin, SystemRole::Admin));
        assert!(!can_grant_role(&admin, SystemRole::SuperAdmin));

        let su = actor("s", SystemRole::SuperAdmin);
        assert!(can_grant_role(&su, SystemRole::SuperAdmin));
    }

    #[test]
    fn can_administer_user_respects_hierarchy_and_self() {
        let admin = actor("admin1", SystemRole::Admin);
        // Strictly-below target: allowed.
        assert!(can_administer_user(&admin, "u1", SystemRole::User));
        // Equal-tier other: denied.
        assert!(!can_administer_user(&admin, "admin2", SystemRole::Admin));
        // Higher-tier other: denied.
        assert!(!can_administer_user(&admin, "root", SystemRole::SuperAdmin));
        // Self at equal tier: allowed (hierarchy rule; action rules are separate).
        assert!(can_administer_user(&admin, "admin1", SystemRole::Admin));
    }
}
