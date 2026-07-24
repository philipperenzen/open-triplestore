//! Guest self-registration toggle lifecycle (Unified Accounts).
//!
//! Pins: the admin toggle default (off), the registration decision matrix,
//! the OFF-sweep (guests bulk-disabled with the specific sign-in message,
//! individually-deactivated guests untouched), the ON-sweep (exactly the
//! toggled-off guests come back), and the features flag the register UI reads.
//!
//! The OTS_DISABLE_REGISTRATION env branch itself is covered by the pure
//! [`decide_registration`] matrix — no test mutates process env (tests run in
//! parallel; env is process-global).

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    use crate::auth::handlers::{
        decide_registration, RegistrationDecision, GUEST_DISABLED_MESSAGE, GUEST_DISABLED_REASON,
        GUEST_SELF_REGISTRATION_SETTING,
    };
    use crate::auth::jwt::issue_access_token;
    use crate::auth::models::SystemRole;
    use crate::auth::password;
    use crate::server::{build_router, AppState};

    fn state() -> AppState {
        AppState::test_default_with_store(crate::store::TripleStore::in_memory().unwrap())
    }

    async fn send(
        state: &AppState,
        method: Method,
        uri: &str,
        bearer: Option<&str>,
        json: Option<serde_json::Value>,
    ) -> (StatusCode, String) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match json {
            Some(v) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let resp = build_router(state.clone(), "", vec![])
            .oneshot(request)
            .await
            .unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).to_string())
    }

    fn seed_user(state: &AppState, name: &str, role: SystemRole) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let hash = password::hash_password("s3cret-password-123").unwrap();
        state
            .auth_db
            .create_user(&id, name, &format!("{name}@example.org"), &hash, role)
            .unwrap();
        id
    }

    fn admin_token(state: &AppState, id: &str, name: &str) -> String {
        issue_access_token(&state.jwt_config, id, name, "admin").unwrap()
    }

    // ── decision matrix (covers the env branch without touching process env) ──

    #[test]
    fn registration_decision_matrix() {
        use RegistrationDecision::*;
        // Registration open → always Open, toggle irrelevant.
        assert_eq!(decide_registration(false, 0, false), Open);
        assert_eq!(decide_registration(false, 5, true), Open);
        // First-account bootstrap stays open even when disabled.
        assert_eq!(decide_registration(true, 0, false), Open);
        // Disabled + users: toggle decides Guest vs Closed.
        assert_eq!(decide_registration(true, 3, true), GuestOnly);
        assert_eq!(decide_registration(true, 3, false), Closed);
    }

    // ── toggle endpoint + sweep lifecycle ─────────────────────────────────────

    #[tokio::test]
    async fn toggle_defaults_off_and_requires_admin() {
        let st = state();
        let admin = seed_user(&st, "root", SystemRole::Admin);
        let user = seed_user(&st, "bob", SystemRole::User);
        let admin_tok = admin_token(&st, &admin, "root");
        let user_tok = issue_access_token(&st.jwt_config, &user, "bob", "user").unwrap();

        // Default off; visible in the public features probe too.
        let (s, body) = send(
            &st,
            Method::GET,
            "/api/admin/settings/guest-registration",
            Some(&admin_tok),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert!(body.contains("\"enabled\":false"), "{body}");
        let (s, body) = send(&st, Method::GET, "/api/auth/features", None, None).await;
        assert_eq!(s, StatusCode::OK);
        assert!(body.contains("\"guest_self_registration\":false"), "{body}");

        // Non-admin cannot read or flip it.
        let (s, _) = send(
            &st,
            Method::GET,
            "/api/admin/settings/guest-registration",
            Some(&user_tok),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let (s, _) = send(
            &st,
            Method::PUT,
            "/api/admin/settings/guest-registration",
            Some(&user_tok),
            Some(serde_json::json!({ "enabled": true })),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn off_sweep_disables_guests_with_message_and_on_sweep_restores() {
        let st = state();
        let admin = seed_user(&st, "root", SystemRole::Admin);
        let admin_tok = admin_token(&st, &admin, "root");
        let _user = seed_user(&st, "bob", SystemRole::User);
        let guest = seed_user(&st, "gia", SystemRole::Guest);
        let guest2 = seed_user(&st, "gus", SystemRole::Guest);
        // A guest an admin deactivated INDIVIDUALLY (no reason stamp) must
        // never be touched by the sweeps.
        st.auth_db.set_user_active(&guest2, false).unwrap();

        // Enable, then disable: the sweep reports exactly the one active guest.
        let (s, _) = send(
            &st,
            Method::PUT,
            "/api/admin/settings/guest-registration",
            Some(&admin_tok),
            Some(serde_json::json!({ "enabled": true })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let (s, body) = send(
            &st,
            Method::PUT,
            "/api/admin/settings/guest-registration",
            Some(&admin_tok),
            Some(serde_json::json!({ "enabled": false })),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert!(body.contains("\"guests_swept\":1"), "{body}");

        // The swept guest: login now returns the SPECIFIC message (password is
        // correct, so this is not an enumeration oracle)…
        let (s, body) = send(
            &st,
            Method::POST,
            "/api/auth/login",
            None,
            Some(serde_json::json!({ "username": "gia", "password": "s3cret-password-123" })),
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert!(body.contains(GUEST_DISABLED_MESSAGE), "{body}");

        // …and a still-valid session token is refused with the same message on
        // introspection (what the suite's client apps surface).
        let guest_tok = issue_access_token(&st.jwt_config, &guest, "gia", "guest").unwrap();
        let (s, body) = send(&st, Method::GET, "/api/auth/me", Some(&guest_tok), None).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        assert!(body.contains(GUEST_DISABLED_MESSAGE), "{body}");

        // Bookkeeping: the reason is stamped; the individually-disabled guest kept its state.
        assert_eq!(
            st.auth_db.deactivation_reason(&guest).unwrap().as_deref(),
            Some(GUEST_DISABLED_REASON)
        );
        assert_eq!(st.auth_db.deactivation_reason(&guest2).unwrap(), None);

        // Toggle back ON: exactly the swept guest returns; sign-in works again.
        let (s, body) = send(
            &st,
            Method::PUT,
            "/api/admin/settings/guest-registration",
            Some(&admin_tok),
            Some(serde_json::json!({ "enabled": true })),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert!(body.contains("\"guests_swept\":1"), "{body}");
        let (s, _) = send(&st, Method::GET, "/api/auth/me", Some(&guest_tok), None).await;
        assert_eq!(s, StatusCode::OK);
        let inactive = st.auth_db.get_user_by_id(&guest2).unwrap().unwrap();
        assert!(
            !inactive.is_active,
            "individually-disabled guest must stay disabled"
        );
    }

    #[tokio::test]
    async fn setting_persists_via_app_settings() {
        let st = state();
        // Unset → the caller's default applies.
        assert!(!st
            .auth_db
            .app_setting_bool(GUEST_SELF_REGISTRATION_SETTING, false));
        assert!(st
            .auth_db
            .app_setting_bool(GUEST_SELF_REGISTRATION_SETTING, true));
        // Set → stored value wins over any default; upsert overwrites.
        st.auth_db
            .set_app_setting(GUEST_SELF_REGISTRATION_SETTING, "true")
            .unwrap();
        assert!(st
            .auth_db
            .app_setting_bool(GUEST_SELF_REGISTRATION_SETTING, false));
        st.auth_db
            .set_app_setting(GUEST_SELF_REGISTRATION_SETTING, "false")
            .unwrap();
        assert!(!st
            .auth_db
            .app_setting_bool(GUEST_SELF_REGISTRATION_SETTING, true));
    }
}
