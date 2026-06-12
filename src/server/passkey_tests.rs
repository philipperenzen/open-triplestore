//! Integration tests for WebAuthn passkey registration, login, and removal,
//! driven end-to-end through the HTTP API with a software authenticator
//! (`webauthn-authenticator-rs` SoftPasskey — no hardware or browser).
//!
//! Same harness as `account_lifecycle_tests`: each request runs through a
//! FRESH router (fresh rate-limiter state) over one shared in-memory AppState,
//! so the passkey challenge store survives across the start/finish pairs.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;
    use webauthn_authenticator_rs::prelude::Url;
    use webauthn_authenticator_rs::softpasskey::SoftPasskey;
    use webauthn_authenticator_rs::WebauthnAuthenticator;
    use webauthn_rs_proto::{
        AllowCredentials, CreationChallengeResponse, PublicKeyCredential,
        RegisterPublicKeyCredential, RequestChallengeResponse,
    };

    use crate::auth::db::AuthDb;
    use crate::auth::jwt::{issue_access_token, JwtConfig};
    use crate::auth::models::SystemRole;
    use crate::auth::password;
    use crate::prefixes::PrefixRegistry;
    use crate::server::{build_router, AppState};
    use crate::storage::ObjectStore;
    use crate::store::TripleStore;

    const TEST_JWT_SECRET: &str = "test_secret_must_be_32_chars_abcd";
    /// Must match the AppState base_url — WebAuthn binds challenges to the origin.
    const ORIGIN: &str = "http://localhost:7878";

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
            mailer: Arc::new(crate::email::Mailer::log_only(ORIGIN)),
            base_url: Arc::new(ORIGIN.to_string()),
            oauth_sessions: crate::auth::oauth::new_session_store(),
            passkey_sessions: crate::auth::passkey::new_session_store(),
            auth_ext: Arc::new(crate::auth::oidc_rs::AuthExt::disabled()),
            query_timeout_secs: 30,
            secure_cookies: false,
            browse_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(64)),
            expensive_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(4)),
            #[cfg(feature = "text-search")]
            text_index: None,
            #[cfg(feature = "text-search")]
            text_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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

    /// Register a passkey for the given bearer through the HTTP flow, driving
    /// the authenticator side with SoftPasskey. Returns the browser-side
    /// credential (whose `raw_id` identifies it for later logins) and the
    /// stored credential's row id.
    async fn enroll_passkey(
        state: &AppState,
        bearer: &str,
        authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
        name: &str,
    ) -> (RegisterPublicKeyCredential, String) {
        let (status, body, text) = send(
            state,
            Method::POST,
            "/api/auth/passkeys/register/start",
            Some(bearer),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let challenge_id = body["challenge_id"].as_str().unwrap().to_string();
        let options: CreationChallengeResponse =
            serde_json::from_value(body["options"].clone()).expect("creation options");

        let credential = authenticator
            .do_registration(Url::parse(ORIGIN).unwrap(), options)
            .expect("software authenticator registration");

        let (status, body, text) = send(
            state,
            Method::POST,
            "/api/auth/passkeys/register/finish",
            Some(bearer),
            Some(serde_json::json!({
                "challenge_id": challenge_id,
                "name": name,
                "credential": serde_json::to_value(&credential).unwrap(),
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{text}");
        assert_eq!(body["name"], name);
        let row_id = body["id"].as_str().unwrap().to_string();
        (credential, row_id)
    }

    /// Run the public login flow for a known credential. SoftPasskey stores
    /// non-resident keys, so the (unsigned, client-side) allow list from the
    /// discoverable challenge is populated with the credential id — exactly
    /// what a browser does when its platform store knows the key.
    async fn passkey_login(
        state: &AppState,
        authenticator: &mut WebauthnAuthenticator<SoftPasskey>,
        registered: &RegisterPublicKeyCredential,
    ) -> (StatusCode, serde_json::Value, String) {
        let (status, body, text) =
            send(state, Method::POST, "/api/auth/passkeys/login/start", None, None).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let challenge_id = body["challenge_id"].as_str().unwrap().to_string();
        let mut options: RequestChallengeResponse =
            serde_json::from_value(body["options"].clone()).expect("request options");
        assert!(
            options.public_key.allow_credentials.is_empty(),
            "discoverable challenge must not enumerate credentials"
        );
        options.public_key.allow_credentials = vec![AllowCredentials {
            type_: "public-key".to_string(),
            id: registered.raw_id.clone(),
            transports: None,
        }];

        let assertion: PublicKeyCredential = authenticator
            .do_authentication(Url::parse(ORIGIN).unwrap(), options)
            .expect("software authenticator assertion");

        send(
            state,
            Method::POST,
            "/api/auth/passkeys/login/finish",
            None,
            Some(serde_json::json!({
                "challenge_id": challenge_id,
                "credential": serde_json::to_value(&assertion).unwrap(),
            })),
        )
        .await
    }

    // ─── Full lifecycle ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn passkey_register_login_and_remove_lifecycle() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        let token = bearer_for("u1", "alice");
        let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));

        // No passkeys to start with.
        let (status, body, _) =
            send(&state, Method::GET, "/api/auth/passkeys", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body.as_array().unwrap().len(), 0);

        // Registration via start → authenticator → finish.
        let (registered, row_id) =
            enroll_passkey(&state, &token, &mut authenticator, "Test key").await;

        // It shows up in the list, unused.
        let (status, body, _) =
            send(&state, Method::GET, "/api/auth/passkeys", Some(&token), None).await;
        assert_eq!(status, StatusCode::OK);
        let list = body.as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["name"], "Test key");
        assert!(list[0]["last_used_at"].is_null());

        // Passkey login issues a session that works against /api/auth/me.
        let (status, body, text) = passkey_login(&state, &mut authenticator, &registered).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        let access = body["access_token"].as_str().expect("session token").to_string();
        assert_eq!(body["user"]["username"], "alice");
        let (status, body, _) = send(&state, Method::GET, "/api/auth/me", Some(&access), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["username"], "alice");

        // Sign-in is recorded with method=passkey, and usage metadata updated.
        let events = state.audit.list(10, 0, Some("login_success"), None, None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].details.as_ref().unwrap()["method"], "passkey");
        let (_, body, _) = send(&state, Method::GET, "/api/auth/passkeys", Some(&token), None).await;
        assert!(body[0]["last_used_at"].is_string(), "last_used_at after login");
        let registered_events = state
            .audit
            .list(10, 0, Some("passkey_registered"), None, None)
            .unwrap();
        assert_eq!(registered_events.len(), 1);

        // Removal demands the password: wrong → 401 and the key stays.
        let (status, _, _) = send(
            &state,
            Method::DELETE,
            &format!("/api/auth/passkeys/{row_id}"),
            Some(&token),
            Some(serde_json::json!({ "password": "wrong-password" })),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _, text) = send(
            &state,
            Method::DELETE,
            &format!("/api/auth/passkeys/{row_id}"),
            Some(&token),
            Some(serde_json::json!({ "password": "longenough123" })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{text}");
        let (_, body, _) = send(&state, Method::GET, "/api/auth/passkeys", Some(&token), None).await;
        assert_eq!(body.as_array().unwrap().len(), 0);
        let removed_events = state
            .audit
            .list(10, 0, Some("passkey_removed"), None, None)
            .unwrap();
        assert_eq!(removed_events.len(), 1);

        // The removed credential no longer signs in.
        let (status, _, _) = passkey_login(&state, &mut authenticator, &registered).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    // ─── Challenge hygiene ────────────────────────────────────────────────────

    #[tokio::test]
    async fn register_challenge_is_single_use_and_owner_bound() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        seed_user(&state, "u2", "bob", "bob@example.org", "longenough123");
        let alice = bearer_for("u1", "alice");
        let bob = bearer_for("u2", "bob");
        let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));

        // Bogus challenge id → 400.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/passkeys/register/finish",
            Some(&alice),
            Some(serde_json::json!({
                "challenge_id": "no-such-challenge",
                "credential": { "id": "x", "rawId": "eA", "type": "public-key",
                                 "response": { "attestationObject": "eA", "clientDataJSON": "eA" },
                                 "extensions": {} },
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Alice's challenge cannot be finished by Bob…
        let (status, body, _) = send(
            &state,
            Method::POST,
            "/api/auth/passkeys/register/start",
            Some(&alice),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let challenge_id = body["challenge_id"].as_str().unwrap().to_string();
        let options: CreationChallengeResponse =
            serde_json::from_value(body["options"].clone()).unwrap();
        let credential = authenticator
            .do_registration(Url::parse(ORIGIN).unwrap(), options)
            .unwrap();
        let finish_body = serde_json::json!({
            "challenge_id": challenge_id,
            "credential": serde_json::to_value(&credential).unwrap(),
        });
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/passkeys/register/finish",
            Some(&bob),
            Some(finish_body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "cross-user finish must fail");

        // …and consuming it (even unsuccessfully) spends it: Alice gets 400 too.
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/passkeys/register/finish",
            Some(&alice),
            Some(finish_body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "challenge is single-use");
    }

    #[tokio::test]
    async fn login_assertion_cannot_replay() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        let token = bearer_for("u1", "alice");
        let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
        let (registered, _) = enroll_passkey(&state, &token, &mut authenticator, "Key").await;

        // Start one login, capture the assertion, finish it twice.
        let (_, body, _) =
            send(&state, Method::POST, "/api/auth/passkeys/login/start", None, None).await;
        let challenge_id = body["challenge_id"].as_str().unwrap().to_string();
        let mut options: RequestChallengeResponse =
            serde_json::from_value(body["options"].clone()).unwrap();
        options.public_key.allow_credentials = vec![AllowCredentials {
            type_: "public-key".to_string(),
            id: registered.raw_id.clone(),
            transports: None,
        }];
        let assertion = authenticator
            .do_authentication(Url::parse(ORIGIN).unwrap(), options)
            .unwrap();
        let finish_body = serde_json::json!({
            "challenge_id": challenge_id,
            "credential": serde_json::to_value(&assertion).unwrap(),
        });

        let (status, _, text) = send(
            &state,
            Method::POST,
            "/api/auth/passkeys/login/finish",
            None,
            Some(finish_body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{text}");

        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/passkeys/login/finish",
            None,
            Some(finish_body),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "assertion replay must fail");
    }

    // ─── Account-state interactions ───────────────────────────────────────────

    #[tokio::test]
    async fn passkey_login_rejects_deactivated_account() {
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        let token = bearer_for("u1", "alice");
        let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
        let (registered, _) = enroll_passkey(&state, &token, &mut authenticator, "Key").await;

        state.auth_db.set_user_active("u1", false).unwrap();
        let (status, _, _) = passkey_login(&state, &mut authenticator, &registered).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn user_verified_passkey_skips_totp_challenge() {
        // A passkey login is user-verified (PIN/biometric), so an account with
        // TOTP enabled gets a session directly instead of an mfa_token detour.
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        let token = bearer_for("u1", "alice");
        let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(true));
        let (registered, _) = enroll_passkey(&state, &token, &mut authenticator, "Key").await;

        // Enable TOTP directly in the DB (the HTTP enrollment is covered by
        // account_lifecycle_tests).
        let secret_enc = crate::auth::secret::encrypt_secret(
            &crate::auth::totp::generate_secret(),
            TEST_JWT_SECRET,
        )
        .unwrap();
        state.auth_db.set_totp_secret("u1", Some(&secret_enc)).unwrap();
        state.auth_db.set_totp_enabled("u1", true).unwrap();

        // Password login demands the second factor…
        let (status, body, _) = send(
            &state,
            Method::POST,
            "/api/auth/login",
            None,
            Some(serde_json::json!({ "username": "alice", "password": "longenough123" })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["mfa_required"], true);

        // …while the user-verified passkey signs straight in.
        let (status, body, text) = passkey_login(&state, &mut authenticator, &registered).await;
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(body["access_token"].is_string(), "direct session: {body}");
    }

    #[tokio::test]
    async fn registration_without_user_verification_is_rejected() {
        // The RP requires user verification for passkeys; an authenticator
        // that cannot (or will not) verify the user must not enroll.
        let state = test_state();
        seed_user(&state, "u1", "alice", "alice@example.org", "longenough123");
        let token = bearer_for("u1", "alice");
        let mut authenticator = WebauthnAuthenticator::new(SoftPasskey::new(false));

        let (status, body, _) = send(
            &state,
            Method::POST,
            "/api/auth/passkeys/register/start",
            Some(&token),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let challenge_id = body["challenge_id"].as_str().unwrap().to_string();
        let options: CreationChallengeResponse =
            serde_json::from_value(body["options"].clone()).unwrap();
        let Ok(credential) = authenticator.do_registration(Url::parse(ORIGIN).unwrap(), options)
        else {
            return; // authenticator already refused — equally acceptable
        };
        let (status, _, _) = send(
            &state,
            Method::POST,
            "/api/auth/passkeys/register/finish",
            Some(&token),
            Some(serde_json::json!({
                "challenge_id": challenge_id,
                "credential": serde_json::to_value(&credential).unwrap(),
            })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "UV-less attestation must be rejected");
    }
}
