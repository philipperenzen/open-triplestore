//! Notify a dataset's owner/administrators when one of their saved queries
//! breaks — either during a version-bump test or when `?version=latest` is
//! requested and the query no longer works.
//!
//! Delivery is best-effort and matches the project's opt-in email model: the
//! event is always logged (and recorded as a test row by the caller), and an
//! email is additionally sent when SMTP is configured (`ALERT_SMTP_*`, `alerting`
//! feature). The recipient is resolved from the account email(s) on
//! OpenTripleStore, per the dataset's owner/admins.

use crate::alerting::{AlertConfig, AlertManager};
use crate::auth::audit::{AuditEventBuilder, AuditEventType, AuditOutcome};
use crate::auth::models::{OwnerType, Role};
use crate::server::AppState;

use super::models::{QueryScope, SavedQuery};

fn looks_like_email(s: &str) -> bool {
    let s = s.trim();
    s.len() >= 3 && s.contains('@') && !s.contains(char::is_whitespace)
}

fn add(out: &mut Vec<String>, email: Option<&str>) {
    if let Some(e) = email {
        let e = e.trim();
        if looks_like_email(e) && !out.iter().any(|x| x == e) {
            out.push(e.to_string());
        }
    }
}

fn add_org(state: &AppState, out: &mut Vec<String>, org_id: &str) {
    if let Ok(Some(org)) = state.auth_db.get_organisation(org_id) {
        add(out, org.contact_email.as_deref());
    }
    if let Ok(members) = state.auth_db.list_org_members(org_id) {
        for (u, role) in members {
            if matches!(role, Role::Admin) {
                add(out, Some(&u.email));
            }
        }
    }
}

fn add_group(state: &AppState, out: &mut Vec<String>, group_id: &str) {
    // Groups have no contact field of their own; notify the parent org's
    // contact plus the group's own admins.
    if let Ok(Some(g)) = state.auth_db.get_group(group_id) {
        add_org(state, out, &g.org_id);
    }
    if let Ok(members) = state.auth_db.list_group_members(group_id) {
        for (u, role) in members {
            if matches!(role, Role::Admin) {
                add(out, Some(&u.email));
            }
        }
    }
}

/// Resolve the set of email addresses to notify about a broken saved query.
pub fn resolve_recipients(
    state: &AppState,
    sq: &SavedQuery,
    dataset_id: Option<&str>,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(did) = dataset_id {
        if let Ok(Some(ds)) = state.auth_db.get_dataset(did) {
            add(&mut out, ds.contact_email.as_deref());
            match ds.owner_type {
                OwnerType::User => {
                    if let Ok(Some(u)) = state.auth_db.get_user_by_id(&ds.owner_id) {
                        add(&mut out, Some(&u.email));
                    }
                }
                OwnerType::Organisation => add_org(state, &mut out, &ds.owner_id),
                OwnerType::Group => add_group(state, &mut out, &ds.owner_id),
            }
        }
    }
    match sq.scope {
        QueryScope::Dataset => {}
        QueryScope::Organisation => add_org(state, &mut out, &sq.owner_id),
        QueryScope::Group => add_group(state, &mut out, &sq.owner_id),
    }
    out
}

/// Best-effort notification that `sq` broke on `version_label`.
pub async fn notify_query_broken(
    state: &AppState,
    sq: &SavedQuery,
    dataset_id: Option<&str>,
    version_label: &str,
    error: &str,
) {
    let recipients = resolve_recipients(state, sq, dataset_id);
    tracing::warn!(
        query_id = %sq.id,
        version = %version_label,
        "saved query '{}' broke on dataset version {}: {}",
        sq.name,
        version_label,
        error
    );
    if recipients.is_empty() {
        return;
    }

    let base = state.base_url.as_str();
    let subject = format!(
        "[OpenTripleStore] Saved query \"{}\" failed on version {}",
        sq.name, version_label
    );
    let body = format!(
        "The saved SPARQL query \"{name}\" ({scope} {owner}) no longer works against \
         dataset version {ver}.\n\nError:\n{err}\n\nReview or repair it in the SPARQL \
         editor (the LLM assistant can help fix it):\n{base}/sparql\n\nYou are receiving \
         this because you own or administer the affected dataset.",
        name = sq.name,
        scope = sq.scope.as_str(),
        owner = sq.owner_id,
        ver = version_label,
        err = error,
        base = base,
    );

    let mgr = AlertManager::new(AlertConfig::from_env(), state.audit.clone());
    let delivered = mgr.send_direct(&recipients, &subject, &body).await;
    state.audit.log(
        AuditEventBuilder::new(AuditEventType::AlertSent, AuditOutcome::Success)
            .resource("saved_query", sq.id.clone())
            .action("notify_query_broken")
            .details(serde_json::json!({
                "version": version_label,
                "recipients": recipients.len(),
                "delivered": delivered,
            })),
    );
}
