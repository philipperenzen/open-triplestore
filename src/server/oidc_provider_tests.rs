//! End-to-end tests for this store's OIDC provider (Unified Accounts):
//! discovery/JWKS shape, the SPA-driven authorize (consent check + code mint),
//! the token exchange with PKCE, refresh rotation (with replay refusal), and
//! that a provider access token authenticates against the regular API
//! (`/api/auth/me`) like any first-class credential.

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use base64::Engine;
    use http_body_util::BodyExt as _;
    use sha2::{Digest, Sha256};
    use tower::ServiceExt as _;

    use crate::auth::jwt::issue_access_token;
    use crate::auth::models::SystemRole;
    use crate::auth::password;
    use crate::server::{build_router, AppState};

    const REDIRECT: &str = "http://localhost:5190/auth/callback";

    fn state_with_client() -> AppState {
        let st = AppState::test_default_with_store(crate::store::TripleStore::in_memory().unwrap());
        st.auth_db
            .upsert_oauth_client(
                "otl-viewer",
                "OTL Viewer",
                &[REDIRECT.to_string()],
                true,
                None,
            )
            .unwrap();
        st
    }

    fn seed_user(state: &AppState, name: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4().to_string();
        let hash = password::hash_password("s3cret-password-123").unwrap();
        state
            .auth_db
            .create_user(
                &id,
                name,
                &format!("{name}@example.org"),
                &hash,
                SystemRole::User,
            )
            .unwrap();
        let token = issue_access_token(&state.jwt_config, &id, name, "user").unwrap();
        (id, token)
    }

    async fn send(
        state: &AppState,
        method: Method,
        uri: &str,
        bearer: Option<&str>,
        body: Option<(&str, String)>, // (content-type, payload)
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = bearer {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = match body {
            Some((ct, payload)) => builder
                .header(header::CONTENT_TYPE, ct)
                .body(Body::from(payload))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let resp = build_router(state.clone(), "", vec![])
            .oneshot(request)
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, json)
    }

    fn pkce_pair() -> (String, String) {
        let verifier = "a".repeat(43);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        (verifier, challenge)
    }

    async fn mint_code(st: &AppState, session: &str, challenge: &str) -> String {
        let (s, body) = send(
            st,
            Method::POST,
            "/api/oauth/authorize",
            Some(session),
            Some((
                "application/json",
                serde_json::json!({
                    "client_id": "otl-viewer",
                    "redirect_uri": REDIRECT,
                    "scope": "openid profile email",
                    "state": "xyz",
                    "nonce": "n-1",
                    "code_challenge": challenge,
                    "code_challenge_method": "S256",
                    "decision": "approve",
                })
                .to_string(),
            )),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let redirect_to = body["redirect_to"].as_str().unwrap();
        assert!(redirect_to.starts_with(REDIRECT), "{redirect_to}");
        assert!(redirect_to.contains("state=xyz"), "{redirect_to}");
        let code = redirect_to
            .split("code=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_string();
        assert!(code.starts_with("otc_"));
        code
    }

    fn token_form(code: &str, verifier: &str) -> String {
        format!(
            "grant_type=authorization_code&code={code}&redirect_uri={}&client_id=otl-viewer&code_verifier={verifier}",
            url::form_urlencoded::byte_serialize(REDIRECT.as_bytes()).collect::<String>(),
        )
    }

    #[tokio::test]
    async fn discovery_and_jwks_shape() {
        let st = state_with_client();
        let (s, body) = send(
            &st,
            Method::GET,
            "/.well-known/openid-configuration",
            None,
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["issuer"], "http://localhost");
        assert_eq!(body["code_challenge_methods_supported"][0], "S256");
        assert!(body["token_endpoint"]
            .as_str()
            .unwrap()
            .ends_with("/oauth/token"));

        let (s, body) = send(&st, Method::GET, "/oauth/jwks", None, None).await;
        assert_eq!(s, StatusCode::OK);
        let key = &body["keys"][0];
        assert_eq!(key["kty"], "EC");
        assert_eq!(key["crv"], "P-256");
        assert_eq!(key["alg"], "ES256");
        assert!(key["x"].as_str().is_some() && key["y"].as_str().is_some());
    }

    #[tokio::test]
    async fn authorize_checks_client_redirect_and_consent() {
        let st = state_with_client();
        let (_uid, session) = seed_user(&st, "alice");
        let (_verifier, challenge) = pkce_pair();

        // Unknown client / disallowed redirect are rejected before any minting.
        let bad = serde_json::json!({
            "client_id": "nope", "redirect_uri": REDIRECT,
            "code_challenge": challenge, "code_challenge_method": "S256",
        });
        let (s, _) = send(
            &st,
            Method::POST,
            "/api/oauth/authorize",
            Some(&session),
            Some(("application/json", bad.to_string())),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        let bad2 = serde_json::json!({
            "client_id": "otl-viewer", "redirect_uri": "http://evil.example/cb",
            "code_challenge": challenge, "code_challenge_method": "S256",
        });
        let (s, _) = send(
            &st,
            Method::POST,
            "/api/oauth/authorize",
            Some(&session),
            Some(("application/json", bad2.to_string())),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);

        // First check: consent required; after approve: remembered.
        let check = serde_json::json!({
            "client_id": "otl-viewer", "redirect_uri": REDIRECT,
            "scope": "openid profile",
            "code_challenge": challenge, "code_challenge_method": "S256",
            "decision": "check",
        });
        let (s, body) = send(
            &st,
            Method::POST,
            "/api/oauth/authorize",
            Some(&session),
            Some(("application/json", check.to_string())),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(body["requires_consent"], true);
        assert_eq!(body["client_name"], "OTL Viewer");

        let _code = mint_code(&st, &session, &challenge).await;
        let (s, body) = send(
            &st,
            Method::POST,
            "/api/oauth/authorize",
            Some(&session),
            Some(("application/json", check.to_string())),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            body["requires_consent"], false,
            "consent should be remembered"
        );

        // Anonymous callers can't reach the mint at all.
        let (s, _) = send(
            &st,
            Method::POST,
            "/api/oauth/authorize",
            None,
            Some(("application/json", check.to_string())),
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn full_code_flow_with_pkce_and_middleware_acceptance() {
        let st = state_with_client();
        let (uid, session) = seed_user(&st, "alice");
        let (verifier, challenge) = pkce_pair();
        let code = mint_code(&st, &session, &challenge).await;

        let (s, body) = send(
            &st,
            Method::POST,
            "/oauth/token",
            None,
            Some((
                "application/x-www-form-urlencoded",
                token_form(&code, &verifier),
            )),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        assert_eq!(body["token_type"], "Bearer");
        assert_eq!(body["scope"], "openid profile email");
        let access = body["access_token"].as_str().unwrap().to_string();
        let refresh = body["refresh_token"].as_str().unwrap().to_string();
        assert!(refresh.starts_with("otr_"));
        assert!(body["id_token"].as_str().is_some());

        // The provider access token is a first-class credential on the API.
        let (s, me) = send(&st, Method::GET, "/api/auth/me", Some(&access), None).await;
        assert_eq!(s, StatusCode::OK, "{me}");
        assert_eq!(me["id"], serde_json::Value::String(uid));
        assert_eq!(me["username"], "alice");

        // …and userinfo answers with standard claims.
        let (s, info) = send(&st, Method::GET, "/oauth/userinfo", Some(&access), None).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(info["preferred_username"], "alice");

        // A code is single-use.
        let (s, body) = send(
            &st,
            Method::POST,
            "/oauth/token",
            None,
            Some((
                "application/x-www-form-urlencoded",
                token_form(&code, &verifier),
            )),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_grant");
    }

    #[tokio::test]
    async fn pkce_and_redirect_are_enforced() {
        let st = state_with_client();
        let (_uid, session) = seed_user(&st, "alice");
        let (_verifier, challenge) = pkce_pair();

        // Wrong verifier.
        let code = mint_code(&st, &session, &challenge).await;
        let (s, body) = send(
            &st,
            Method::POST,
            "/oauth/token",
            None,
            Some((
                "application/x-www-form-urlencoded",
                token_form(&code, "wrong-verifier-wrong-verifier-wrong-verif"),
            )),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_grant", "{body}");

        // Redirect mismatch at exchange time.
        let code = mint_code(&st, &session, &challenge).await;
        let form = format!(
            "grant_type=authorization_code&code={code}&redirect_uri=http%3A%2F%2Fevil.example%2Fcb&client_id=otl-viewer&code_verifier={}",
            "a".repeat(43),
        );
        let (s, body) = send(
            &st,
            Method::POST,
            "/oauth/token",
            None,
            Some(("application/x-www-form-urlencoded", form)),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_grant");

        // Public client without PKCE never even gets a code.
        let (s, _) = send(
            &st,
            Method::POST,
            "/api/oauth/authorize",
            Some(&session),
            Some((
                "application/json",
                serde_json::json!({
                    "client_id": "otl-viewer", "redirect_uri": REDIRECT,
                    "decision": "approve",
                })
                .to_string(),
            )),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn refresh_rotates_and_replay_is_refused() {
        let st = state_with_client();
        let (_uid, session) = seed_user(&st, "alice");
        let (verifier, challenge) = pkce_pair();
        let code = mint_code(&st, &session, &challenge).await;
        let (_s, body) = send(
            &st,
            Method::POST,
            "/oauth/token",
            None,
            Some((
                "application/x-www-form-urlencoded",
                token_form(&code, &verifier),
            )),
        )
        .await;
        let refresh1 = body["refresh_token"].as_str().unwrap().to_string();

        let form =
            format!("grant_type=refresh_token&refresh_token={refresh1}&client_id=otl-viewer");
        let (s, body) = send(
            &st,
            Method::POST,
            "/oauth/token",
            None,
            Some(("application/x-www-form-urlencoded", form.clone())),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{body}");
        let refresh2 = body["refresh_token"].as_str().unwrap().to_string();
        assert_ne!(refresh1, refresh2, "refresh must rotate");
        assert!(body["access_token"].as_str().is_some());

        // Replaying the rotated-out token fails.
        let (s, body) = send(
            &st,
            Method::POST,
            "/oauth/token",
            None,
            Some(("application/x-www-form-urlencoded", form)),
        )
        .await;
        assert_eq!(s, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "invalid_grant");
    }
}
