//! Security regression tests for the federated-login (OIDC / SAML) paths.
//!
//! Covers:
//!  * S6 — successful/failed SSO logins must emit audit events. The OIDC/SAML
//!    HTTP callbacks cannot be driven end-to-end without a live IdP (code
//!    exchange / signed-assertion verification), so we test the exact mechanism
//!    the `saml_acs` / `oidc_callback` handlers rely on: an `AuditLogger`
//!    `LoginSuccess` / `LoginFailure` row carrying the SSO provider details, with
//!    the actor recovered from the just-issued access token (see
//!    `oauth_handlers::audit_sso_login_success` / `audit_sso_login_failure`).
//!  * S13 — the returning-user branch of `provision_or_link_user` must keep
//!    returning the existing linked user (and refresh its identity record)
//!    instead of swallowing/aborting on the housekeeping upsert.

mod common;
use common::*;

use std::sync::Arc;

use open_triplestore::auth::{
    audit::{AuditEventBuilder, AuditEventType, AuditLogger, AuditOutcome},
    db::AuthDb,
    jwt::{issue_access_token, verify_token, JwtConfig},
    models::{OauthProviderCreate, SystemRole},
    oauth::provision_or_link_user,
};

/// Build a minimal OIDC provider row and return its id.
fn make_oidc_provider(db: &Arc<AuthDb>, slug: &str, auto_provision: bool) -> String {
    let create = OauthProviderCreate {
        name: format!("Provider {slug}"),
        slug: slug.to_string(),
        provider_type: "oidc".to_string(),
        client_id: Some("client-123".to_string()),
        client_secret: None,
        client_secret_enc: None,
        discovery_url: Some("https://idp.example.com/.well-known/openid-configuration".to_string()),
        tenant_id: None,
        entity_id: None,
        sso_url: None,
        idp_certificate: None,
        scopes: None,
        role_claim_map: None,
        auto_provision,
        default_role: Some("user".to_string()),
        is_active: true,
    };
    db.create_oauth_provider(&create).unwrap().id
}

/// S13: a returning SSO user (an identity link already exists for this
/// provider+subject) is found and returned, and the identity record's
/// `last_login_at` is refreshed rather than the call silently failing.
#[test]
fn returning_user_is_relinked_and_returned() {
    let db = Arc::new(AuthDb::in_memory().unwrap());
    let provider_id = make_oidc_provider(&db, "acme", true);
    let provider = db.get_oauth_provider_by_id(&provider_id).unwrap().unwrap();

    // Existing local user with an established identity link.
    let user = db
        .create_user(
            "u-return",
            "returning",
            "returning@example.com",
            "oauth:acme:ext-sub-1",
            SystemRole::User,
        )
        .unwrap();
    db.upsert_oauth_identity("id-1", &user.id, &provider_id, "ext-sub-1", None)
        .unwrap();

    // The returning-user branch must return the SAME user without error.
    let resolved = provision_or_link_user(
        "ext-sub-1",
        Some("returning@example.com"),
        true,
        "Returning User",
        SystemRole::User,
        &provider,
        &db,
    )
    .unwrap();
    assert_eq!(
        resolved.id, user.id,
        "must resolve to the linked local user"
    );

    // The housekeeping upsert ran: last_login_at is populated and no duplicate
    // identity was created.
    let identities = db.list_oauth_identities_for_user(&user.id).unwrap();
    assert_eq!(identities.len(), 1, "must not create a duplicate identity");
    assert!(
        identities[0].last_login_at.is_some(),
        "returning-user login must refresh last_login_at"
    );
}

/// S6: the audit mechanism used by the SSO callbacks records a `LoginSuccess`
/// row carrying the SSO provider details, with the actor recovered from the
/// freshly issued access token (id / username / role).
#[test]
fn sso_login_success_audit_event_is_recorded() {
    let db = Arc::new(AuthDb::in_memory().unwrap());
    let logger = AuditLogger::new(db.pool());
    let jwt = JwtConfig::new(JWT_SECRET.to_string(), 30, 30);

    // Token issued by the flow for the provisioned user.
    let access = issue_access_token(&jwt, "u-sso", "ssouser", "user").unwrap();

    // Mirror oauth_handlers::audit_sso_login_success.
    let mut b = AuditEventBuilder::new(AuditEventType::LoginSuccess, AuditOutcome::Success)
        .details(serde_json::json!({
            "auth_method": "sso",
            "provider_type": "oidc",
            "provider_slug": "acme",
        }));
    let claims = verify_token(&jwt, &access).unwrap();
    b = b.actor(claims.sub, claims.username, claims.role);
    logger.log(b);

    let events = logger
        .list(
            10,
            0,
            Some(AuditEventType::LoginSuccess.as_str()),
            None,
            None,
        )
        .unwrap();
    assert_eq!(events.len(), 1, "exactly one LoginSuccess must be recorded");
    let ev = &events[0];
    assert_eq!(ev.event_type, "login_success");
    assert_eq!(ev.actor_id.as_deref(), Some("u-sso"));
    assert_eq!(ev.actor_username.as_deref(), Some("ssouser"));
    let details = ev.details.as_ref().expect("details present");
    assert_eq!(details["auth_method"], "sso");
    assert_eq!(details["provider_type"], "oidc");
    assert_eq!(details["provider_slug"], "acme");
}

/// S6 (failure path): a rejected SSO assertion/callback records a `LoginFailure`
/// row with the provider details and a redacted reason, and no actor.
#[test]
fn sso_login_failure_audit_event_is_recorded() {
    let db = Arc::new(AuthDb::in_memory().unwrap());
    let logger = AuditLogger::new(db.pool());

    // Mirror oauth_handlers::audit_sso_login_failure.
    let b = AuditEventBuilder::new(AuditEventType::LoginFailure, AuditOutcome::Failure).details(
        serde_json::json!({
            "auth_method": "sso",
            "provider_type": "saml",
            "provider_slug": "corp",
            "reason": "assertion_rejected",
        }),
    );
    logger.log(b);

    let events = logger
        .list(
            10,
            0,
            Some(AuditEventType::LoginFailure.as_str()),
            None,
            None,
        )
        .unwrap();
    assert_eq!(events.len(), 1, "exactly one LoginFailure must be recorded");
    let ev = &events[0];
    assert_eq!(ev.event_type, "login_failure");
    assert_eq!(ev.outcome, "failure");
    assert!(ev.actor_id.is_none(), "failed SSO login has no known actor");
    let details = ev.details.as_ref().expect("details present");
    assert_eq!(details["provider_type"], "saml");
    assert_eq!(details["reason"], "assertion_rejected");
}
