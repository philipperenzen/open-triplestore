//! Integration tests for the account-lifecycle flows: registration input
//! validation, email verification, self-service password reset, forgot
//! username, email change, and TOTP two-factor login.
//!
//! Each request builds a fresh router (so per-IP rate-limiter state never
//! accumulates across calls) over a shared in-memory AppState.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    use crate::auth::db::AuthDb;
    use crate::auth::jwt::{hash_token, issue_access_token, JwtConfig};
    use crate::auth::models::SystemRole;
    use crate::auth::{password, totp};
    use crate::prefixes::PrefixRegistry;
    use crate::server::{build_router, AppState};
    use crate::storage::ObjectStore;
    use crate::store::TripleStore;

    const TEST_JWT_SECRET: &str = "test_secret_must_be_32_chars_abcd";

    fn test_state() -> AppState {
        let auth_db = Arc::new(AuthDb::in_memory().unwrap());
        let audit = Arc::new(crate::auth::audit::AuditLogger::new(auth_db.pool()));
        AppState {
            store: TripleStore::in_memory().unwrap(),
            prefix_registry: Arc::new(PrefixRegistry::empty()),
            auth_db,
            audit,
            backup: None,
            jwt_config: Arc::new(JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30)),
            object_store: Arc::new(ObjectStore::noop()),
            mailer: Arc::new(crate::email::Mailer::log_only("http://localhost:7878")),
            base_url: Arc::new("http://localhost:7878".to_string()),
            oauth_sessions: crate::auth::oauth::new_session_store(),
            passkey_sessions: crate::auth::passkey::new_session_store(),
            auth_ext: Arc::new(crate::auth::oidc_rs::AuthExt::disabled()),
            query_timeout_secs: 30,
            write_timeout_secs: 120,
            secure_cookies: false,
            browse_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(64)),
            expensive_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(4)),
            #[cfg(feature = "text-search")]
            text_index: None,
            #[cfg(feature = "text-search")]
            text_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            vocab_catalog: Arc::new(crate::vocab_search::catalog::VocabCatalog::bundled()),
            vocab_registry_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            vocab_corpus: Arc::new(std::sync::RwLock::new(None)),
            #[cfg(feature = "vocab-search")]
            vocab_engine: None,
        }
    }

    /// Drive one request through a FRESH router (fresh rate-limiter state).
    async fn send(
        state: &AppState,
        method: Method,
        uri: &str,
        bearer: Option<&str>,
        json: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value, String) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match json {
            Some(v) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
            None => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        };
        let resp = build_router(state.clone(), "", vec![])
            .oneshot(request)
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        (status, value, text)
    }

    /// Pre-seed an admin so a register-under-test is never the FIRST user
    /// (the first registration kicks off the demo seeder).
    fn seed_admin(state: &AppState) {
        state
            .auth_db
            .create_user(
                "admin0",
                "admin",
                "admin@test.com",
                "h",
                SystemRole::SuperAdmin,
            )
            .unwrap();
    }

    /// Create a password-login-capable user directly in the DB.
    fn seed_user(state: &AppState, id: &str, username: &str, email: &str, pw: &str) {
        let hash = password::hash_password(pw).unwrap();
        state
            .auth_db
            .create_user(id, username, email, &hash, SystemRole::User)
            .unwrap();
    }

    fn bearer_for(user_id: &str, username: &str) -> String {
        issue_access_token(
            &JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30),
            user_id,
            username,
            "user",
        )
        .unwrap()
    }

    /// Insert an email action token directly and return the raw token value.
    fn mint_email_token(
        state: &AppState,
        user_id: &str,
        kind: &str,
        new_email: Option<&str>,
    ) -> String {
        let raw = format!("test-token-{}", uuid::Uuid::new_v4());
        let expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        state
            .auth_db
            .create_email_token(
                &uuid::Uuid::new_v4().to_string(),
                user_id,
                kind,
                &hash_token(&raw),
                new_email,
                &expires,
            )
            .unwrap();
        raw
    }

    // ─── Registration validation ──────────────────────────────────────────────

    #[tokio::test]
    async fn register_rejects_faulty_emails_and_usernames() {
        let state = test_state();
        seed_admin(&state);

        for bad_email in [
            "plainaddress",
            "user@localhost",
            "user@",
            "@example.org",
            "user@@example.org",
            "user@-bad.org",
            "us er@example.org",
        ] {
            let (status, _, text) = send(
                &state,
                Method::POST,
                "/api/auth/register",
                None,
                Some(serde_json::json!({
                    "username": "validname",
                    "email": bad_email,
                    "password": "longenough123",
                })),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "email {bad_email:?} must be rejected, got {status}: {text}"
            );
        }

        for bad_username in ["ab", "-lead", "has space", "emoji😀", "semi;colon"] {
            let (status, _, text) = send(
                &state,
                Method::POST,
                "/api/auth/register",
                None,
                Some(serde_json::json!({
                    "username": bad_username,
                    "email": "ok@example.org",
                    "password": "longenough123",
                })),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "username {bad_username:?} must be rejected, got {status}: {text}"
            );
        }

        // A valid registration goes through, is unverified, and a duplicate
        // email afterwards yields a clean 409 (not a UNIQUE-constraint 500).
        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/register",
            None,
            Some(serde_json::json!({
                "username": "alice",
                "email": "alice@example.org",
                "password": "longenough123",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{text}");
        assert_eq!(body["user"]["email_verified"], false);

        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/register",
            None,
            Some(serde_json::json!({
                "username": "alice2",
                "email": "alice@example.org",
                "password": "longenough123",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    // ─── Email verification ───────────────────────────────────────────────────

    #[tokio::test]
    async fn verify_email_token_is_single_use() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        assert!(
            !state
                .auth_db
                .get_user_by_id("u1")
                .unwrap()
                .unwrap()
                .email_verified
        );

        let raw = mint_email_token(&state, "u1", "verify_email", None);

        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/verify-email",
            None,
            Some(serde_json::json!({ "token": raw })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(body["verified"], true);
        assert!(
            state
                .auth_db
                .get_user_by_id("u1")
                .unwrap()
                .unwrap()
                .email_verified
        );

        // Replay must fail with the generic message.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/verify-email",
            None,
            Some(serde_json::json!({ "token": raw })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Garbage tokens fail identically.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/verify-email",
            None,
            Some(serde_json::json!({ "token": "no-such-token" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // ─── Password reset ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn password_reset_flow_rotates_credentials_and_sessions() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "oldpassword1");

        let raw = mint_email_token(&state, "u1", "reset_password", None);

        // Too-short replacement password is rejected up front.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/reset-password",
            None,
            Some(serde_json::json!({ "token": raw, "new_password": "short" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _, text) = send(
            &state,
            Method::POST,
            "/api/auth/reset-password",
            None,
            Some(serde_json::json!({ "token": raw, "new_password": "newpassword1" })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{text}");

        // Completing the reset proves mailbox control.
        assert!(
            state
                .auth_db
                .get_user_by_id("u1")
                .unwrap()
                .unwrap()
                .email_verified
        );

        // The token is spent.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/reset-password",
            None,
            Some(serde_json::json!({ "token": raw, "new_password": "anotherpass1" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Old password no longer logs in; the new one does.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/login",
            None,
            Some(serde_json::json!({ "username": "alice", "password": "oldpassword1" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/login",
            None,
            Some(serde_json::json!({ "username": "alice", "password": "newpassword1" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(body["access_token"].is_string());
    }

    #[tokio::test]
    async fn forgot_password_and_username_are_enumeration_safe() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");

        let (s1, _, b1) = send(
            &state,
            Method::POST,
            "/api/auth/forgot-password",
            None,
            Some(serde_json::json!({ "identifier": "alice" })),
        )
        .await;
        let (s2, _, b2) = send(
            &state,
            Method::POST,
            "/api/auth/forgot-password",
            None,
            Some(serde_json::json!({ "identifier": "no-such-user" })),
        )
        .await;
        assert_eq!(s1, StatusCode::OK);
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(b1, b2, "responses must be indistinguishable");

        let (s1, _, b1) = send(
            &state,
            Method::POST,
            "/api/auth/forgot-username",
            None,
            Some(serde_json::json!({ "email": "alice@example.org" })),
        )
        .await;
        let (s2, _, b2) = send(
            &state,
            Method::POST,
            "/api/auth/forgot-username",
            None,
            Some(serde_json::json!({ "email": "stranger@example.org" })),
        )
        .await;
        assert_eq!(s1, StatusCode::OK);
        assert_eq!(s2, StatusCode::OK);
        assert_eq!(b1, b2, "responses must be indistinguishable");
    }

    // ─── Email change ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn change_email_requires_password_and_validates() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        seed_user(&state, "u2", "bob", "bob@example.org", "longenough123");
        let token = bearer_for("u1", "alice");

        // Wrong password → 401.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/change-email",
            Some(&token),
            Some(serde_json::json!({ "new_email": "new@example.org", "password": "wrong" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Invalid address → 400.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/change-email",
            Some(&token),
            Some(serde_json::json!({ "new_email": "not-an-email", "password": "longenough123" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Someone else's address → 409.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/change-email",
            Some(&token),
            Some(
                serde_json::json!({ "new_email": "bob@example.org", "password": "longenough123" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // Valid: with the log-only mailer (no SMTP) the change applies
        // immediately and the address is flagged unverified.
        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/change-email",
            Some(&token),
            Some(
                serde_json::json!({ "new_email": "new@example.org", "password": "longenough123" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(body["pending"], false);
        let user = state.auth_db.get_user_by_id("u1").unwrap().unwrap();
        assert_eq!(user.email, "new@example.org");
        assert!(!user.email_verified);
    }

    #[tokio::test]
    async fn update_me_rejects_direct_email_change() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        let token = bearer_for("u1", "alice");

        let (status, _, text) = send(
            &state,
            Method::PUT,
            "/api/auth/me",
            Some(&token),
            Some(serde_json::json!({ "email": "sneaky@example.org" })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{text}");
        assert!(text.contains("change-email"), "{text}");

        // Sending the unchanged address stays a no-op success.
        let (status, _, text) = send(
            &state,
            Method::PUT,
            "/api/auth/me",
            Some(&token),
            Some(serde_json::json!({ "email": "alice@example.org" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
    }

    // ─── TOTP two-factor login ────────────────────────────────────────────────

    #[tokio::test]
    async fn totp_enroll_then_login_challenge_and_recovery_codes() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        let token = bearer_for("u1", "alice");

        // Setup → secret + otpauth URL.
        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/setup",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let secret = body["secret"].as_str().unwrap().to_string();
        assert!(body["otpauth_url"]
            .as_str()
            .unwrap()
            .starts_with("otpauth://totp/"));

        // Enable with a live code → recovery codes (exactly once).
        let code = totp::code_at_step(&secret, totp::current_step()).unwrap();
        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/enable",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let recovery: Vec<String> = body["recovery_codes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(recovery.len(), 10);
        assert!(
            state
                .auth_db
                .get_user_by_id("u1")
                .unwrap()
                .unwrap()
                .totp_enabled
        );

        // Password login now returns an MFA challenge instead of a session.
        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/login",
            None,
            Some(serde_json::json!({ "username": "alice", "password": "longenough123" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(body["mfa_required"], true);
        let mfa_token = body["mfa_token"].as_str().unwrap().to_string();
        assert!(
            body.get("access_token").is_none(),
            "no session before the second factor"
        );

        // The challenge token must NOT work as an access token.
        let (status, _, _) =
            send(&state, Method::GET, "/api/auth/me", Some(&mfa_token), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // The code consumed during enablement can't be replayed; the next
        // step's code (within clock skew) completes the login.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/verify",
            None,
            Some(serde_json::json!({ "mfa_token": mfa_token, "code": code })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "TOTP replay must be rejected"
        );

        let next_code = totp::code_at_step(&secret, totp::current_step() + 1).unwrap();
        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/verify",
            None,
            Some(serde_json::json!({ "mfa_token": mfa_token, "code": next_code })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(body["access_token"].is_string());

        // Recovery codes finish a login too — exactly once each.
        let (_, body, _) = send(
            &state,
            Method::POST,
            "/api/auth/login",
            None,
            Some(serde_json::json!({ "username": "alice", "password": "longenough123" })),
        )
        .await;
        let mfa_token2 = body["mfa_token"].as_str().unwrap().to_string();
        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/verify",
            None,
            Some(serde_json::json!({ "mfa_token": mfa_token2, "code": recovery[0] })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(body["access_token"].is_string());

        let (_, body, _) = send(
            &state,
            Method::POST,
            "/api/auth/login",
            None,
            Some(serde_json::json!({ "username": "alice", "password": "longenough123" })),
        )
        .await;
        let mfa_token3 = body["mfa_token"].as_str().unwrap().to_string();
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/verify",
            None,
            Some(serde_json::json!({ "mfa_token": mfa_token3, "code": recovery[0] })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "recovery codes are single-use"
        );
    }

    #[tokio::test]
    async fn totp_disable_requires_password_and_code() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        let token = bearer_for("u1", "alice");

        let (_, body, _) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/setup",
            Some(&token),
            None,
        )
        .await;
        let secret = body["secret"].as_str().unwrap().to_string();
        let code = totp::code_at_step(&secret, totp::current_step()).unwrap();
        let (status, _, text) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/enable",
            Some(&token),
            Some(serde_json::json!({ "code": code })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");

        // Wrong password → 401; bad code → 401.
        let next = totp::code_at_step(&secret, totp::current_step() + 1).unwrap();
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/disable",
            Some(&token),
            Some(serde_json::json!({ "password": "wrong", "code": next })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/disable",
            Some(&token),
            Some(serde_json::json!({ "password": "longenough123", "code": "000000" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Correct password + fresh code → disabled, plain login again.
        let (status, _, text) = send(
            &state,
            Method::POST,
            "/api/auth/2fa/disable",
            Some(&token),
            Some(serde_json::json!({ "password": "longenough123", "code": next })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
        let user = state.auth_db.get_user_by_id("u1").unwrap().unwrap();
        assert!(!user.totp_enabled);

        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/auth/login",
            None,
            Some(serde_json::json!({ "username": "alice", "password": "longenough123" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(
            body["access_token"].is_string(),
            "plain session after disable"
        );
    }

    // ─── Admin validation ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn admin_create_user_validates_email_and_marks_verified() {
        let state = test_state();
        seed_admin(&state);
        let token = issue_access_token(
            &JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30),
            "admin0",
            "admin",
            "super_admin",
        )
        .unwrap();

        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/admin/users",
            Some(&token),
            Some(serde_json::json!({
                "username": "carol",
                "email": "not-an-email",
                "password": "longenough123",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, body, text) = send(
            &state,
            Method::POST,
            "/api/admin/users",
            Some(&token),
            Some(serde_json::json!({
                "username": "carol",
                "email": "carol@example.org",
                "password": "longenough123",
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{text}");
        let id = body["id"].as_str().unwrap();
        assert!(
            state
                .auth_db
                .get_user_by_id(id)
                .unwrap()
                .unwrap()
                .email_verified,
            "admin-provisioned addresses count as verified"
        );
    }
}
