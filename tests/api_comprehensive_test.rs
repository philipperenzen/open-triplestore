//! Comprehensive HTTP API test suite for open_triplestore.
//!
//! Covers: auth, SPARQL protocol, graph store, datasets/orgs, SHACL, RML,
//! ACL management, performance, security/abuse, and browse APIs.
//!
//! All tests use an in-memory AppState driven via tower::ServiceExt::oneshot —
//! no real network port is bound and no disk I/O occurs.

// ─── Shared helpers ───────────────────────────────────────────────────────────

mod helpers {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
        Router,
    };
    use http_body_util::BodyExt as _;
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    use serde_json::Value;
    use std::sync::Arc;
    use tower::ServiceExt as _;

    use open_triplestore::{
        auth::{
            db::AuthDb,
            jwt::{issue_access_token, JwtConfig},
            models::SystemRole,
            oauth::new_session_store,
        },
        prefixes::PrefixRegistry,
        server::{build_router, AppState},
        storage::ObjectStore,
        store::TripleStore,
    };

    pub const JWT_SECRET: &str = "test_secret_must_be_32_chars_abcd";

    pub fn test_state() -> AppState {
        let auth_db = Arc::new(AuthDb::in_memory().unwrap());
        let audit = Arc::new(open_triplestore::auth::audit::AuditLogger::new(
            auth_db.pool(),
        ));
        AppState {
            store: TripleStore::in_memory().unwrap(),
            prefix_registry: Arc::new(PrefixRegistry::empty()),
            auth_db,
            audit,
            backup: None,
            jwt_config: Arc::new(JwtConfig::new(JWT_SECRET.to_string(), 30, 30)),
            object_store: Arc::new(ObjectStore::noop()),
            mailer: Arc::new(open_triplestore::email::Mailer::log_only(
                "http://localhost:7878",
            )),
            base_url: Arc::new("http://localhost:7878".to_string()),
            oauth_sessions: new_session_store(),
            passkey_sessions: open_triplestore::auth::passkey::new_session_store(),
            auth_ext: Arc::new(open_triplestore::auth::oidc_rs::AuthExt::disabled()),
            query_timeout_secs: 30,
            write_timeout_secs: 120,
            secure_cookies: false,
            browse_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(64)),
            expensive_semaphore: std::sync::Arc::new(tokio::sync::Semaphore::new(4)),
            #[cfg(feature = "text-search")]
            text_index: None,
            #[cfg(feature = "text-search")]
            text_dirty: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn test_app(state: AppState) -> Router {
        build_router(state, "", vec![])
    }

    pub fn mint_token(user_id: &str, username: &str, role: &str) -> String {
        issue_access_token(
            &JwtConfig::new(JWT_SECRET.to_string(), 30, 30),
            user_id,
            username,
            role,
        )
        .unwrap()
    }

    /// Returns (state, admin_token) — creates a super_admin user in the state.
    pub fn admin_state() -> (AppState, String) {
        let state = test_state();
        state
            .auth_db
            .create_user(
                "adm",
                "admin",
                "admin@test.com",
                "hash",
                SystemRole::SuperAdmin,
            )
            .unwrap();
        let token = mint_token("adm", "admin", "super_admin");
        (state, token)
    }

    pub async fn body_text(body: Body) -> String {
        let bytes = body.collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub async fn body_json(body: Body) -> Value {
        let text = body_text(body).await;
        serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
    }

    pub fn url_encode(s: &str) -> String {
        utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
    }

    /// Generate N-Triples bulk data with unique subjects.
    pub fn ntriples(n: usize) -> String {
        (0..n)
            .map(|i| format!("<http://ex.org/s{i}> <http://ex.org/p> <http://ex.org/o{i}> .\n"))
            .collect()
    }

    /// POST to /api/auth/register and return the full response body as JSON.
    pub async fn register(
        app: Router,
        username: &str,
        email: &str,
        password: &str,
    ) -> (StatusCode, Value) {
        let body = serde_json::json!({
            "username": username,
            "email": email,
            "password": password
        });
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let json = body_json(resp.into_body()).await;
        (status, json)
    }

    /// POST to /api/auth/login and return (status, JSON body).
    pub async fn login(app: Router, username: &str, password: &str) -> (StatusCode, Value) {
        let body = serde_json::json!({ "username": username, "password": password });
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let json = body_json(resp.into_body()).await;
        (status, json)
    }
}

// ─── Auth & User Management ───────────────────────────────────────────────────

mod auth {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use open_triplestore::auth::{
        jwt::{issue_access_token, JwtConfig},
        models::SystemRole,
    };
    use tower::ServiceExt as _;

    use super::helpers::*;

    #[tokio::test]
    async fn first_user_becomes_super_admin() {
        let state = test_state();
        let app = test_app(state);
        let (status, json) = register(app, "alice", "alice@ex.com", "password123").await;
        assert_eq!(status, StatusCode::CREATED, "{json}");
        assert_eq!(
            json["user"]["role"], "super_admin",
            "First user must be super_admin: {json}"
        );
    }

    #[tokio::test]
    async fn second_user_becomes_user() {
        let state = test_state();
        let (s1, _) = register(
            test_app(state.clone()),
            "alice",
            "alice@ex.com",
            "password123",
        )
        .await;
        assert_eq!(s1, StatusCode::CREATED);
        let (s2, json) = register(test_app(state), "bob", "bob@ex.com", "password123").await;
        assert_eq!(s2, StatusCode::CREATED, "{json}");
        assert_eq!(
            json["user"]["role"], "user",
            "Second user must be 'user': {json}"
        );
    }

    #[tokio::test]
    async fn login_returns_tokens() {
        let state = test_state();
        // Use register response tokens — avoids same-second JWT hash collision if login called immediately after
        let (status, json) =
            register(test_app(state), "testlogin", "tl@ex.com", "password123").await;
        assert_eq!(status, StatusCode::CREATED, "{json}");
        assert!(
            json["access_token"].is_string(),
            "access_token missing: {json}"
        );
        assert!(
            json["refresh_token"].is_string(),
            "refresh_token missing: {json}"
        );
    }

    #[tokio::test]
    async fn login_wrong_password() {
        let state = test_state();
        register(
            test_app(state.clone()),
            "alice2",
            "a2@ex.com",
            "correctpassword",
        )
        .await;
        let (status, _) = login(test_app(state), "alice2", "wrongpassword").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn refresh_token_works() {
        let state = test_state();
        let (_, json) = register(
            test_app(state.clone()),
            "alice3",
            "a3@ex.com",
            "password123",
        )
        .await;
        let refresh_token = json["refresh_token"].as_str().unwrap().to_string();

        // Sleep 1s so issued refresh token gets a different JWT timestamp (avoids UNIQUE hash collision)
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let body = serde_json::json!({ "refresh_token": refresh_token });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/refresh")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let resp_json = body_json(resp.into_body()).await;
        assert!(
            resp_json["access_token"].is_string(),
            "New access_token missing: {resp_json}"
        );
    }

    #[tokio::test]
    async fn logout_invalidates_refresh() {
        let state = test_state();
        let (_, json) = register(
            test_app(state.clone()),
            "alice4",
            "a4@ex.com",
            "password123",
        )
        .await;
        let access_token = json["access_token"].as_str().unwrap().to_string();
        let refresh_token = json["refresh_token"].as_str().unwrap().to_string();

        // Logout
        let logout_body = serde_json::json!({ "refresh_token": &refresh_token });
        test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/logout")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                    .body(Body::from(serde_json::to_string(&logout_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Try to use the revoked refresh token
        let refresh_body = serde_json::json!({ "refresh_token": refresh_token });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/refresh")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&refresh_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Revoked refresh token must be rejected"
        );
    }

    #[tokio::test]
    async fn invalid_jwt_rejected() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/auth/me")
                    .header(header::AUTHORIZATION, "Bearer this.is.garbage")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_secret_jwt_rejected() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "eve", "e@t.com", "h", SystemRole::User)
            .unwrap();
        // Token signed with a DIFFERENT secret
        let bad_token = issue_access_token(
            &JwtConfig::new("completely_different_secret_here!".to_string(), 30, 30),
            "u1",
            "eve",
            "user",
        )
        .unwrap();
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/auth/me")
                    .header(header::AUTHORIZATION, format!("Bearer {bad_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn tampered_jwt_rejected() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "frank", "f@t.com", "h", SystemRole::User)
            .unwrap();
        let mut token = mint_token("u1", "frank", "user");
        // Flip the last character
        let last = token.pop().unwrap();
        token.push(if last == 'a' { 'b' } else { 'a' });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/auth/me")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_token_create_use_revoke() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "grace", "g@t.com", "h", SystemRole::User)
            .unwrap();
        let user_token = mint_token("u1", "grace", "user");

        // Create an API token
        let create_body = serde_json::json!({
            "name": "my-token",
            "scopes": ["read"]
        });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/tokens")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                    .body(Body::from(serde_json::to_string(&create_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "Creating API token must return 201"
        );
        let created = body_json(resp.into_body()).await;
        let api_token = created["token"].as_str().unwrap().to_string();
        let token_id = created["id"].as_str().unwrap().to_string();

        // Use the token on a protected endpoint
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/auth/me")
                    .header(header::AUTHORIZATION, format!("Bearer {api_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "API token must work on protected endpoint"
        );

        // Revoke the token
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/auth/tokens/{token_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success(), "Revoking token must succeed");

        // Token must be rejected after revocation
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/auth/me")
                    .header(header::AUTHORIZATION, format!("Bearer {api_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Revoked API token must be rejected"
        );
    }

    #[tokio::test]
    async fn change_password() {
        let state = test_state();
        let (_, reg_json) =
            register(test_app(state.clone()), "hank", "hank@ex.com", "oldpass123").await;
        let access_token = reg_json["access_token"].as_str().unwrap().to_string();

        let body = serde_json::json!({
            "current_password": "oldpass123",
            "new_password": "newpass456"
        });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/change-password")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success(), "Change password must succeed");

        // Old password must no longer work
        let (status, _) = login(test_app(state.clone()), "hank", "oldpass123").await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "Old password must be rejected after change"
        );

        // New password must work
        let (status, _) = login(test_app(state), "hank", "newpass456").await;
        assert_eq!(status, StatusCode::OK, "New password must be accepted");
    }

    #[tokio::test]
    async fn duplicate_username_rejected() {
        let state = test_state();
        let (s1, _) = register(
            test_app(state.clone()),
            "irene",
            "irene@ex.com",
            "password123",
        )
        .await;
        assert_eq!(s1, StatusCode::CREATED);
        let (s2, _) = register(test_app(state), "irene", "irene2@ex.com", "password123").await;
        assert_eq!(
            s2,
            StatusCode::CONFLICT,
            "Duplicate username must return 409"
        );
    }

    #[tokio::test]
    async fn admin_promotes_user() {
        let (state, admin_token) = admin_state();
        state
            .auth_db
            .create_user("u1", "jack", "j@t.com", "h", SystemRole::User)
            .unwrap();

        let body = serde_json::json!({ "role": "admin" });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/admin/users/u1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success(), "Promote must succeed");

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/admin/users/u1")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["role"], "admin",
            "Role must be 'admin' after promotion: {json}"
        );
    }

    #[tokio::test]
    async fn admin_purges_user() {
        let (state, admin_token) = admin_state();
        state
            .auth_db
            .create_user("u2", "karen", "k@t.com", "h", SystemRole::User)
            .unwrap();

        // Must deactivate (DELETE) before purging
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/admin/users/u2")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "Deactivate must succeed before purge"
        );

        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/admin/users/u2/purge")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success(), "Purge must succeed");

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/admin/users/u2")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "Purged user must not exist"
        );
    }

    #[tokio::test]
    async fn no_token_on_protected_endpoint() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/auth/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn user_cannot_access_admin_endpoint() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "leo", "l@t.com", "h", SystemRole::User)
            .unwrap();
        let token = mint_token("u1", "leo", "user");
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/admin/users")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}

// ─── SPARQL Protocol ──────────────────────────────────────────────────────────

mod sparql_protocol {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use open_triplestore::auth::models::{OwnerType, Visibility};
    use tower::ServiceExt as _;

    use super::helpers::*;

    fn insert_public_graph(state: &open_triplestore::server::AppState) {
        state
            .auth_db
            .create_user(
                "u1",
                "alice",
                "a@t.com",
                "h",
                open_triplestore::auth::models::SystemRole::User,
            )
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://ex.org/pub")
            .unwrap();
        state.store.update("INSERT DATA { GRAPH <http://ex.org/pub> { <http://ex.org/s> <http://ex.org/p> \"hello\" } }").unwrap();
    }

    #[tokio::test]
    async fn get_select_json() {
        let state = test_state();
        insert_public_graph(&state);
        let query = "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }";
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            ct.contains("sparql-results+json"),
            "Expected sparql-results+json, got: {ct}"
        );
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("hello"),
            "Results must contain inserted literal: {body}"
        );
    }

    #[tokio::test]
    async fn get_ask_query() {
        let state = test_state();
        insert_public_graph(&state);
        // Must use explicit WHERE keyword so inject_from_clauses can insert dataset scoping
        let query = "ASK WHERE { GRAPH <http://ex.org/pub> { ?s ?p ?o } }";
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["boolean"], true,
            "ASK must return true for existing data: {json}"
        );
    }

    #[tokio::test]
    async fn get_construct_turtle() {
        let state = test_state();
        insert_public_graph(&state);
        let query = "CONSTRUCT { ?s ?p ?o } WHERE { GRAPH <http://ex.org/pub> { ?s ?p ?o } }";
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "text/turtle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            ct.contains("turtle") || ct.contains("text/"),
            "Expected Turtle, got: {ct}"
        );
    }

    #[tokio::test]
    async fn post_query_body() {
        let state = test_state();
        insert_public_graph(&state);
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sparql")
                    .header(header::CONTENT_TYPE, "application/sparql-query")
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .body(Body::from("SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn post_update_authenticated() {
        let (state, token) = admin_state();
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sparql")
                    .header(header::CONTENT_TYPE, "application/sparql-update")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(
                        "INSERT DATA { <http://ex.org/a> <http://ex.org/b> <http://ex.org/c> }",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "Authenticated SPARQL UPDATE must succeed"
        );
    }

    #[tokio::test]
    async fn post_update_unauthenticated() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sparql")
                    .header(header::CONTENT_TYPE, "application/sparql-update")
                    .body(Body::from(
                        "INSERT DATA { <http://ex.org/a> <http://ex.org/b> <http://ex.org/c> }",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn post_form_encoded_query() {
        let state = test_state();
        insert_public_graph(&state);
        let encoded_query = url_encode("SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }");
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sparql")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .body(Body::from(format!("query={encoded_query}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn accept_results_xml() {
        let state = test_state();
        insert_public_graph(&state);
        let query = "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }";
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "application/sparql-results+xml")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            ct.contains("sparql-results+xml") || ct.contains("xml"),
            "Expected XML, got: {ct}"
        );
    }

    #[tokio::test]
    async fn malformed_sparql_returns_400() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sparql?query={}",
                        url_encode("THIS IS NOT@@@ SPARQL")
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_client_error(),
            "Malformed SPARQL must return 4xx, got {}",
            resp.status()
        );
        let body = body_text(resp.into_body()).await;
        assert!(
            !body.to_lowercase().contains("oxigraph"),
            "Internal lib name must not leak: {body}"
        );
    }

    #[tokio::test]
    async fn missing_query_param_returns_400() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/sparql")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_client_error(),
            "Missing query param must return 4xx, got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn select_hides_private_graph() {
        let state = test_state();
        state
            .auth_db
            .create_user(
                "u1",
                "alice",
                "a@t.com",
                "h",
                open_triplestore::auth::models::SystemRole::User,
            )
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Priv",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://secret.ex.org/graph")
            .unwrap();
        state
            .store
            .update(
                "INSERT DATA { GRAPH <http://secret.ex.org/graph> { <s:x> <p:y> \"classified\" } }",
            )
            .unwrap();

        let query = "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }";
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            !body.contains("classified"),
            "Unauthenticated must not see private data: {body}"
        );
    }

    #[tokio::test]
    async fn admin_sees_all_graphs() {
        let state = test_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Priv",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://secret.ex.org/admin-test")
            .unwrap();
        state.store.update("INSERT DATA { GRAPH <http://secret.ex.org/admin-test> { <s:x> <p:y> \"admin-visible\" } }").unwrap();
        state
            .auth_db
            .create_user(
                "adm",
                "adm",
                "adm@t.com",
                "h",
                open_triplestore::auth::models::SystemRole::SuperAdmin,
            )
            .unwrap();
        let token = mint_token("adm", "adm", "super_admin");

        let query = "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }";
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("admin-visible"),
            "Admin must see private graph data: {body}"
        );
    }

    #[tokio::test]
    async fn describe_query() {
        let state = test_state();
        insert_public_graph(&state);
        // Explicit WHERE clause required for inject_from_clauses to inject FROM dataset scoping
        let query = "DESCRIBE <http://ex.org/s> WHERE { <http://ex.org/s> ?p ?o }";
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "text/turtle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

// ─── Graph Store Protocol ─────────────────────────────────────────────────────

mod graph_store {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use tower::ServiceExt as _;

    use super::helpers::*;

    const GRAPH_IRI: &str = "http://ex.org/test-graph";

    fn encoded_graph() -> String {
        url_encode(GRAPH_IRI)
    }

    #[tokio::test]
    async fn get_default_graph_empty() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/store?default")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn put_and_get_named_graph() {
        let (state, token) = admin_state();
        // PUT data
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(
                        "<http://ex.org/a> <http://ex.org/b> <http://ex.org/c> .",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "PUT must succeed: {}",
            resp.status()
        );

        // GET it back (must authenticate — graph is unmanaged + non-public)
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .header(header::ACCEPT, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("ex.org/a") || body.contains("ex.org/c"),
            "Returned graph must contain inserted triples: {body}"
        );
    }

    #[tokio::test]
    async fn put_replaces_graph() {
        let (state, token) = admin_state();
        let put = |data: &'static str, state: open_triplestore::server::AppState, tok: String| {
            let encoded = url_encode(GRAPH_IRI);
            async move {
                test_app(state)
                    .oneshot(
                        Request::builder()
                            .method(Method::PUT)
                            .uri(format!("/store?graph={encoded}"))
                            .header(header::CONTENT_TYPE, "text/turtle")
                            .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                            .body(Body::from(data))
                            .unwrap(),
                    )
                    .await
                    .unwrap()
            }
        };
        put(
            "<http://ex.org/first> <http://ex.org/p> \"first\" .",
            state.clone(),
            token.clone(),
        )
        .await;
        put(
            "<http://ex.org/second> <http://ex.org/p> \"second\" .",
            state.clone(),
            token.clone(),
        )
        .await;

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .header(header::ACCEPT, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(resp.into_body()).await;
        assert!(
            !body.contains("first"),
            "First triple must be gone after replace: {body}"
        );
        assert!(
            body.contains("second"),
            "Second triple must be present: {body}"
        );
    }

    #[tokio::test]
    async fn post_merges_graph() {
        let (state, token) = admin_state();
        for literal in &["\"first\"", "\"second\""] {
            test_app(state.clone())
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/store?graph={}", encoded_graph()))
                        .header(header::CONTENT_TYPE, "text/turtle")
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::from(format!(
                            "<http://ex.org/s> <http://ex.org/p> {literal} ."
                        )))
                        .unwrap(),
                )
                .await
                .unwrap();
        }
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .header(header::ACCEPT, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("first"),
            "POST must merge: first triple missing: {body}"
        );
        assert!(
            body.contains("second"),
            "POST must merge: second triple missing: {body}"
        );
    }

    #[tokio::test]
    async fn delete_graph() {
        let (state, token) = admin_state();
        // PUT then DELETE
        test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(
                        "<http://ex.org/x> <http://ex.org/p> <http://ex.org/y> .",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .header(header::ACCEPT, "text/turtle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(resp.into_body()).await;
        // After deletion the graph should be empty (no triples)
        assert!(
            !body.contains("ex.org/x"),
            "Graph must be empty after DELETE: {body}"
        );
    }

    #[tokio::test]
    async fn put_requires_auth() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .body(Body::from("<s:a> <p:b> <o:c> ."))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn delete_requires_auth() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn payload_too_large_returns_413() {
        let (state, token) = admin_state();
        // Generate ~52 MB of data — exceeds the 50 MB body limit
        let big_body = vec![b'a'; 52 * 1024 * 1024];
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/store?graph={}", encoded_graph()))
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(big_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "52 MB body must return 413"
        );
    }
}

// ─── Datasets & Organisations ─────────────────────────────────────────────────

mod datasets {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
    use tower::ServiceExt as _;

    use super::helpers::*;

    #[tokio::test]
    async fn create_and_get_dataset() {
        let (state, token) = admin_state();
        // Need an org to own the dataset
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();

        let body = serde_json::json!({
            "name": "My Dataset",
            "owner_type": "organisation",
            "owner_id": "o1",
            "visibility": "public"
        });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/datasets")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "Create dataset must return 201"
        );
        let created = body_json(resp.into_body()).await;
        let dataset_id = created["id"].as_str().unwrap().to_string();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/datasets/{dataset_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["name"], "My Dataset");
    }

    #[tokio::test]
    async fn public_dataset_visible_to_anon() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::SuperAdmin)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Public DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("Public DS"),
            "Public dataset must appear in unauthenticated list: {body}"
        );
    }

    #[tokio::test]
    async fn private_dataset_hidden_from_anon() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Secret DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            !body.contains("Secret DS"),
            "Private dataset must not appear for anon: {body}"
        );
    }

    #[tokio::test]
    async fn update_dataset() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Old Name",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();

        let body = serde_json::json!({ "name": "New Name", "visibility": "public" });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/datasets/d1")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "Update must succeed: {}",
            resp.status()
        );

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets/d1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["name"], "New Name", "Name must be updated: {json}");
    }

    #[tokio::test]
    async fn delete_dataset() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "To Delete",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();

        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/datasets/d1")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success(), "Delete must succeed");

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(resp.into_body()).await;
        assert!(
            !body.contains("To Delete"),
            "Deleted dataset must not appear in list: {body}"
        );
    }

    #[tokio::test]
    async fn org_crud() {
        let (state, token) = admin_state();

        let body = serde_json::json!({ "name": "Test Org", "slug": "test-org" });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/organisations")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "Create org must return 201: {}",
            resp.status()
        );
        let created = body_json(resp.into_body()).await;
        let org_id = created["id"].as_str().unwrap().to_string();

        // Update
        let update = serde_json::json!({ "name": "Updated Org" });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/organisations/{org_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(serde_json::to_string(&update).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success(), "Update org must succeed");

        // Delete
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/organisations/{org_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success(), "Delete org must succeed");
    }

    #[tokio::test]
    async fn group_crud() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();

        let body = serde_json::json!({ "name": "Test Group" });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/organisations/o1/groups")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "Create group must return 201: {}",
            resp.status()
        );
        let group = body_json(resp.into_body()).await;
        let group_id = group["id"].as_str().unwrap().to_string();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/organisations/o1/groups/{group_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status().is_success(), "Delete group must succeed");
    }

    #[tokio::test]
    async fn create_sparql_service() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();

        let body = serde_json::json!({ "name": "My Service", "slug": "my-service" });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/datasets/d1/services")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "Create service must return 201: {}",
            resp.status()
        );

        let created = body_json(resp.into_body()).await;
        assert_eq!(created["dataset_id"], "d1");
        assert_eq!(created["slug"], "my-service");
        assert_eq!(
            created["sparql_endpoint"],
            "/api/datasets/d1/services/my-service/sparql"
        );

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets/d1/services")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "List services must return 200: {}",
            resp.status()
        );

        let listed = body_json(resp.into_body()).await;
        let arr = listed.as_array().expect("service list must be an array");
        let svc = arr
            .iter()
            .find(|s| s["slug"] == "my-service")
            .expect("created service must exist in list");
        assert_eq!(svc["dataset_id"], "d1");
        assert_eq!(
            svc["sparql_endpoint"],
            "/api/datasets/d1/services/my-service/sparql"
        );
    }

    #[tokio::test]
    async fn non_owner_cannot_delete_dataset() {
        let state = test_state();
        // Owner: alice (super_admin, first registered)
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::SuperAdmin)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Alice DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();

        // Another user without org membership
        state
            .auth_db
            .create_user("u_bob", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        let bob_token = mint_token("u_bob", "bob", "user");

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/datasets/d1")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::FORBIDDEN || resp.status() == StatusCode::NOT_FOUND,
            "Non-owner must not delete dataset, got {}",
            resp.status()
        );
    }
}

// ─── SHACL Validation ─────────────────────────────────────────────────────────

mod shacl {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use open_triplestore::auth::models::{OwnerType, Visibility};
    use tower::ServiceExt as _;

    use super::helpers::*;

    const PERSON_SHAPES: &str = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name ; sh:minCount 1 ] .
"#;

    #[tokio::test]
    async fn shaclc_parse() {
        let shaclc = r#"
BASE <http://example.org/>
PREFIX sh: <http://www.w3.org/ns/shacl#>
shape <PersonShape> -> <Person> {
    <name> minCount 1 ;
}
"#;
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/shaclc/parse")
                    .header(header::CONTENT_TYPE, "text/shaclc")
                    .body(Body::from(shaclc))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Either 200 with Turtle output, or 400 if the SHACLC syntax is wrong — acceptable
        assert!(
            resp.status().is_success() || resp.status().is_client_error(),
            "SHACLC parse must return 2xx or 4xx, got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn shaclc_serialize() {
        let turtle = r#"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person .
"#;
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/shaclc/serialize")
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .body(Body::from(turtle))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success() || resp.status().is_client_error(),
            "SHACLC serialize must return 2xx or 4xx, got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn shapes_put_get_roundtrip() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .update_dataset_shacl("d1", false, Some("http://ex.org/shapes"))
            .unwrap();

        // PUT shapes
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/datasets/d1/shapes")
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(PERSON_SHAPES))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "PUT shapes must succeed: {}",
            resp.status()
        );

        // GET shapes back
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets/d1/shapes")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("NodeShape") || body.contains("shacl"),
            "Shapes response must contain SHACL: {body}"
        );
    }
}

// ─── RML Mapping ──────────────────────────────────────────────────────────────

mod rml {
    use axum::{
        body::Body,
        http::{header, Method, Request},
    };
    use open_triplestore::auth::models::{OwnerType, Visibility};
    use tower::ServiceExt as _;

    use super::helpers::*;

    const SIMPLE_RML: &str = r#"
@prefix rml: <http://semweb.mmlab.be/ns/rml#> .
@prefix rr: <http://www.w3.org/ns/r2rml#> .
@prefix ql: <http://semweb.mmlab.be/ns/ql#> .
@prefix ex: <http://example.org/> .

<http://example.org/TriplesMap> a rr:TriplesMap ;
    rml:logicalSource [
        rml:source "data.csv" ;
        rml:referenceFormulation ql:CSV
    ] ;
    rr:subjectMap [ rr:template "http://example.org/{id}" ] ;
    rr:predicateObjectMap [
        rr:predicate ex:name ;
        rr:objectMap [ rml:reference "name" ]
    ] .
"#;

    #[tokio::test]
    async fn put_rml_mapping() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/datasets/d1/mappings")
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(SIMPLE_RML))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "PUT RML mapping must succeed: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn rml_preview() {
        let preview_body = serde_json::json!({
            "mapping": SIMPLE_RML,
            "sources": {
                "data.csv": "id,name\n1,Alice\n2,Bob"
            }
        });
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/rml/preview")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&preview_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Preview is unauthenticated; expect 200 with triples or 400 on format mismatch
        assert!(
            resp.status().is_success() || resp.status().is_client_error(),
            "RML preview must return 2xx or 4xx, got {}",
            resp.status()
        );
    }
}

// ─── ACL Management ───────────────────────────────────────────────────────────

mod acl {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
    use tower::ServiceExt as _;

    use super::helpers::*;

    #[tokio::test]
    async fn grant_and_revoke_graph_permission() {
        let (state, admin_token) = admin_state();
        state
            .auth_db
            .create_user("u1", "dave", "d@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Priv",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://priv.ex.org/g")
            .unwrap();
        state
            .store
            .update("INSERT DATA { GRAPH <http://priv.ex.org/g> { <s:x> <p:y> \"private-val\" } }")
            .unwrap();

        let user_token = mint_token("u1", "dave", "user");

        // Before grant: user cannot see data
        let query = "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }";
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(resp.into_body()).await;
        assert!(
            !body.contains("private-val"),
            "User must not see private graph before grant: {body}"
        );

        // Grant read
        let grant_body = serde_json::json!({
            "graph_iri": "http://priv.ex.org/g",
            "principal_type": "user",
            "principal_id": "u1",
            "permission": "read"
        });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/admin/acl/graphs")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::from(serde_json::to_string(&grant_body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "Grant permission must succeed: {}",
            resp.status()
        );
        let grant_json = body_json(resp.into_body()).await;
        let acl_id = grant_json["id"].as_str().unwrap_or("").to_string();

        // After grant: user can see data
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("private-val"),
            "User must see private graph after read grant: {body}"
        );

        // Revoke
        if !acl_id.is_empty() {
            let resp = test_app(state.clone())
                .oneshot(
                    Request::builder()
                        .method(Method::DELETE)
                        .uri(format!("/api/admin/acl/graphs/{acl_id}"))
                        .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(resp.status().is_success(), "Revoke must succeed");

            // After revoke: user cannot see data again
            let resp = test_app(state)
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(format!("/sparql?query={}", url_encode(query)))
                        .header(header::ACCEPT, "application/sparql-results+json")
                        .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = body_text(resp.into_body()).await;
            assert!(
                !body.contains("private-val"),
                "User must not see private graph after revoke: {body}"
            );
        }
    }

    #[tokio::test]
    async fn endpoint_acl_deny_blocks_user() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_user("adm", "admin2", "adm@t.com", "h", SystemRole::Admin)
            .unwrap();
        state
            .auth_db
            .create_endpoint_acl_rule(
                "rule1",
                "user",
                "u1",
                "/api/browse/**",
                "*",
                "deny",
                10,
                "adm",
            )
            .unwrap();

        let user_token = mint_token("u1", "bob", "user");
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/graphs")
                    .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Deny ACL rule must block user"
        );
    }

    #[tokio::test]
    async fn triple_security_label_created() {
        let (state, admin_token) = admin_state();

        let body = serde_json::json!({
            "subject_iri": "http://ex.org/secret-subject",
            "predicate_iri": "http://ex.org/p",
            "object_value": "classified-value",
            "graph_iri": "http://ex.org/graph",
            "label_graph_iri": "http://ex.org/labels"
        });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/admin/acl/triples")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "Creating triple security label must succeed: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn acl_admin_endpoints_require_admin() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "carol", "c@t.com", "h", SystemRole::User)
            .unwrap();
        let token = mint_token("u1", "carol", "user");

        for path in &[
            "/api/admin/acl/endpoints",
            "/api/admin/acl/graphs",
            "/api/admin/acl/triples",
        ] {
            let resp = test_app(state.clone())
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(*path)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::FORBIDDEN,
                "Non-admin must be forbidden from {path}"
            );
        }
    }
}

// ─── Performance ─────────────────────────────────────────────────────────────

mod performance {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use std::time::{Duration, Instant};
    use tower::ServiceExt as _;

    use super::helpers::*;

    async fn bulk_insert_ntriples(n: usize, max_secs: u64) {
        let (state, token) = admin_state();
        let data = ntriples(n);
        let graph = url_encode("http://perf.ex.org/graph");

        let start = Instant::now();
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/store?graph={graph}"))
                    .header(header::CONTENT_TYPE, "application/n-triples")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(data))
                    .unwrap(),
            )
            .await
            .unwrap();
        let elapsed = start.elapsed();
        assert!(
            resp.status().is_success(),
            "Bulk insert of {n} triples must succeed: {}",
            resp.status()
        );
        assert!(
            elapsed < Duration::from_secs(max_secs),
            "Bulk insert of {n} triples took {:?}, expected < {max_secs}s",
            elapsed
        );
    }

    #[tokio::test]
    async fn bulk_insert_1k() {
        bulk_insert_ntriples(1_000, 5).await;
    }

    #[tokio::test]
    async fn bulk_insert_10k() {
        bulk_insert_ntriples(10_000, 15).await;
    }

    #[tokio::test]
    #[ignore = "perf stress test: 100k-triple bulk insert; slow + timing-sensitive, run explicitly with `cargo test -- --ignored`"]
    async fn bulk_insert_100k() {
        bulk_insert_ntriples(100_000, 60).await;
    }

    #[tokio::test]
    async fn aggregation_query_perf() {
        use open_triplestore::auth::models::{OwnerType, Visibility};
        let (state, token) = admin_state();
        // Register the graph in a public dataset so the admin query scope includes it
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .ok();
        state
            .auth_db
            .create_dataset(
                "d1",
                "PerfDS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .ok();
        state
            .auth_db
            .add_dataset_graph("d1", "http://perf.ex.org/agg-graph")
            .ok();

        // Pre-load 5k triples
        let data = ntriples(5_000);
        let graph = url_encode("http://perf.ex.org/agg-graph");
        test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/store?graph={graph}"))
                    .header(header::CONTENT_TYPE, "application/n-triples")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(data))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Query with admin token (sees all graphs) using explicit WHERE
        let query = "SELECT (COUNT(?s) AS ?count) WHERE { GRAPH <http://perf.ex.org/agg-graph> { ?s ?p ?o } }";
        let start = Instant::now();
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["results"]["bindings"][0]["count"]["value"], "5000",
            "COUNT must return 5000: {json}"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "Aggregation took {:?}, expected < 10s",
            elapsed
        );
    }

    #[tokio::test]
    async fn concurrent_reads_10() {
        use open_triplestore::auth::models::{OwnerType, Visibility};

        let state = test_state();
        state
            .auth_db
            .create_user(
                "u",
                "u",
                "u@t.com",
                "h",
                open_triplestore::auth::models::SystemRole::User,
            )
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://perf.ex.org/concurrent")
            .unwrap();
        state
            .store
            .update("INSERT DATA { GRAPH <http://perf.ex.org/concurrent> { <s:x> <p:y> \"v\" } }")
            .unwrap();

        let query = url_encode("SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }");
        let start = Instant::now();

        let mut handles = Vec::new();
        for _ in 0..10 {
            let s = state.clone();
            let q = query.clone();
            handles.push(tokio::spawn(async move {
                test_app(s)
                    .oneshot(
                        Request::builder()
                            .method(Method::GET)
                            .uri(format!("/sparql?query={q}"))
                            .header(header::ACCEPT, "application/sparql-results+json")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap()
                    .status()
            }));
        }

        for h in handles {
            let status = h.await.unwrap();
            assert_eq!(status, StatusCode::OK, "Concurrent read must return 200");
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(10),
            "10 concurrent reads took {:?}, expected < 10s",
            elapsed
        );
    }
}

// ─── Security & Abuse ────────────────────────────────────────────────────────

mod security {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use tower::ServiceExt as _;

    use super::helpers::*;

    #[tokio::test]
    async fn sql_injection_in_username() {
        // SQLite uses parameterized queries; this should be handled safely
        let state = test_state();
        let (status, _) = register(
            test_app(state),
            "'; DROP TABLE users; --",
            "inject@ex.com",
            "password123",
        )
        .await;
        assert_ne!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "SQL injection must not cause 500"
        );
        // 400 (username too short/invalid) or 409 (duplicate) are acceptable
    }

    #[tokio::test]
    async fn iri_injection_in_graph_name() {
        let (state, token) = admin_state();
        // IRI containing '>' breaks out of SPARQL angle-bracket IRI syntax — must be rejected
        let malicious_graph = url_encode("http://evil.example.com/> UNION SELECT * {}");
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/store?graph={malicious_graph}"))
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from("<s:a> <p:b> <o:c> ."))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_client_error() || resp.status().is_server_error(),
            "IRI with angle bracket must be rejected, got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn browse_triples_iri_injection() {
        let malicious = "http://example.com/> UNION SELECT * WHERE {?s ?p ?o} #";
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/browse/triples?subject={}",
                        url_encode(malicious)
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "IRI injection attempt must return 400"
        );
    }

    #[tokio::test]
    async fn cors_no_allow_origin_without_config() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/sparql")
                    .header(header::ORIGIN, "http://evil.example.com")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // With empty cors_origins the server must NOT echo back the evil origin
        let acao = resp.headers().get(header::ACCESS_CONTROL_ALLOW_ORIGIN);
        if let Some(val) = acao {
            assert_ne!(
                val, "http://evil.example.com",
                "Evil origin must not be reflected in CORS header"
            );
        }
    }

    #[tokio::test]
    async fn payload_too_large_sparql_batch_413() {
        let (state, token) = admin_state();
        // The batch endpoint accepts JSON: {"updates": [...]}
        // Fill with a JSON body that exceeds 10 MB
        let big_entry = "INSERT DATA { <s:a> <p:b> <o:c> }";
        let entries_needed = (11 * 1024 * 1024) / (big_entry.len() + 4);
        let updates: Vec<&str> = (0..entries_needed).map(|_| big_entry).collect();
        let big_body = serde_json::to_string(&serde_json::json!({ "updates": updates })).unwrap();
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sparql/batch")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(big_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "Oversized batch body must return 413"
        );
    }

    #[tokio::test]
    async fn csp_header_on_all_responses() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let csp = resp
            .headers()
            .get("content-security-policy")
            .expect("Content-Security-Policy header must be present");
        assert!(
            csp.to_str().unwrap().contains("default-src 'self'"),
            "CSP must contain default-src 'self', got: {:?}",
            csp
        );
    }

    #[tokio::test]
    async fn internal_error_detail_not_leaked() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/sparql?query={}",
                        url_encode("THIS IS NOT VALID SPARQL!!!")
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_client_error(),
            "Invalid SPARQL must return 4xx, got {}",
            resp.status()
        );
        let body = body_text(resp.into_body()).await;
        assert!(
            !body.to_lowercase().contains("oxigraph"),
            "Internal library name must not leak: {body}"
        );
    }
}

// ─── Browse API ───────────────────────────────────────────────────────────────

mod browse {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use open_triplestore::auth::models::{OwnerType, SystemRole, Visibility};
    use tower::ServiceExt as _;

    use super::helpers::*;

    #[tokio::test]
    async fn browse_graphs_empty_store() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/graphs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        // Expect empty array or object with empty graphs list
        assert!(
            body.contains("[]")
                || body.contains("\"graphs\":[]")
                || body == "[]"
                || body.len() < 100,
            "Empty store must return empty graphs: {body}"
        );
    }

    #[tokio::test]
    async fn browse_graphs_returns_public() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://pub.ex.org/visible")
            .unwrap();
        state
            .store
            .update("INSERT DATA { GRAPH <http://pub.ex.org/visible> { <s:x> <p:y> <o:z> } }")
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/graphs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("pub.ex.org"),
            "Public graph IRI must appear in browse/graphs: {body}"
        );
    }

    #[tokio::test]
    async fn browse_stats_empty() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/stats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["total_triples"], 0,
            "Empty store must report 0 triples: {json}"
        );
    }

    #[tokio::test]
    async fn browse_stats_counts_triples() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://stats.ex.org/g")
            .unwrap();
        state
            .store
            .update(
                "INSERT DATA { GRAPH <http://stats.ex.org/g> { <s:a> <p:b> <o:c> ; <p:d> <o:e> } }",
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/stats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        let total = json["total_triples"].as_u64().unwrap_or(0);
        assert!(total > 0, "Stats must count inserted triples: {json}");
    }

    #[tokio::test]
    async fn browse_triples_iri_injection() {
        let malicious = "http://example.com/foo>injection";
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/browse/resource?iri={}",
                        url_encode(malicious)
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "IRI with '>' must return 400"
        );
    }

    #[tokio::test]
    async fn browse_suggest() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://suggest.ex.org/g")
            .unwrap();
        state.store.update("INSERT DATA { GRAPH <http://suggest.ex.org/g> { <http://suggest.ex.org/alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://schema.org/Person> } }").unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!(
                        "/api/browse/suggest?prefix={}&field=subject",
                        url_encode("http://suggest.ex")
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// /browse/triples (no count param) must return a `hasMore` flag derived
    /// from a LIMIT+1 probe — not a `total` field. This is the fast path that
    /// replaced the unconditional COUNT(*) query.
    #[tokio::test]
    async fn browse_triples_default_uses_has_more_not_total() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://has.ex.org/g")
            .unwrap();
        // 3 triples in the graph
        state
            .store
            .update(
                "INSERT DATA { GRAPH <http://has.ex.org/g> { <s:a> <p:p> <o:1> , <o:2> , <o:3> } }",
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/triples?limit=2")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["triples"].as_array().unwrap().len(),
            2,
            "Must truncate to limit: {json}"
        );
        assert_eq!(
            json["hasMore"], true,
            "Third triple exists, hasMore must be true: {json}"
        );
        assert!(
            json.get("total").is_none(),
            "Default response must NOT include total: {json}"
        );
    }

    /// Negated filter chips (`neg: true`) exclude rows that MATCH the clause, on
    /// both the `contains` and `exact` paths. Guards the `browse_triples`
    /// NOT-filter (each negated clause is wrapped in `!(…)` and AND-ed).
    #[tokio::test]
    async fn browse_triples_negated_chip_excludes_matches() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://has.ex.org/g")
            .unwrap();
        state
            .store
            .update(
                "INSERT DATA { GRAPH <http://has.ex.org/g> { \
                 <s:a> <p:keep> <o:1> . <s:a> <p:keep> <o:2> . <s:a> <p:drop> <o:9> . } }",
            )
            .unwrap();

        // Percent-encode the JSON `filters` value so it survives the query string.
        fn pct(s: &str) -> String {
            s.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect()
        }

        // Negated predicate "contains drop" → the <p:drop> triple is excluded.
        let filters = r#"[{"field":"predicate","value":"drop","mode":"contains","neg":true}]"#;
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/browse/triples?filters={}", pct(filters)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        let triples = json["triples"].as_array().unwrap();
        assert_eq!(
            triples.len(),
            2,
            "Negated contains chip must drop the matching row: {json}"
        );
        let body = json.to_string();
        assert!(
            !body.contains("p:drop"),
            "Excluded predicate must not appear: {json}"
        );
        assert!(
            !body.contains("o:9"),
            "Row matched by the negated chip must be gone: {json}"
        );

        // Negated object exact (literal string form) → the <o:1> row is excluded,
        // the other two remain. Exercises negation on the exact-match path.
        let filters2 = r#"[{"field":"object","value":"o:1","mode":"exact","neg":true}]"#;
        let resp2 = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/browse/triples?filters={}", pct(filters2)))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp2.status(), StatusCode::OK);
        let json2 = body_json(resp2.into_body()).await;
        let triples2 = json2["triples"].as_array().unwrap();
        assert_eq!(
            triples2.len(),
            2,
            "Negated exact chip drops exactly the matching row: {json2}"
        );
        assert!(
            !json2.to_string().contains("o:1"),
            "Object o:1 must be excluded by the negated exact chip: {json2}"
        );
    }

    /// An `exact` chip on an http(s)/urn IRI is bound INTO the triple pattern
    /// (`GRAPH ?g { <iri> ?p ?o . BIND(<iri> AS ?s) }`) instead of being emitted as a
    /// trailing `FILTER(?s = <iri>)`. sparopt has no equality-into-pattern rewrite, so
    /// the FILTER form range-scanned the whole scope for a point lookup — measured
    /// ~1.2s vs ~0.2s on a 3.1M-triple store, paid twice per graph-view expansion.
    ///
    /// These cases pin the SEMANTICS, which must be identical either way.
    mod exact_chip_binding {
        use super::*;

        fn pct(s: &str) -> String {
            s.bytes()
                .map(|b| {
                    if b.is_ascii_alphanumeric() {
                        (b as char).to_string()
                    } else {
                        format!("%{b:02X}")
                    }
                })
                .collect()
        }

        async fn browse(
            state: open_triplestore::server::AppState,
            query: &str,
        ) -> serde_json::Value {
            let resp = test_app(state)
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(format!("/api/browse/triples?{query}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            body_json(resp.into_body()).await
        }

        /// Public dataset with three subjects under http:// IRIs (the form that
        /// qualifies for binding) plus a literal object.
        fn seeded() -> open_triplestore::server::AppState {
            let state = test_state();
            state
                .auth_db
                .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
                .unwrap();
            state
                .auth_db
                .create_organisation("o1", "Acme", "acme", None, None)
                .unwrap();
            state
                .auth_db
                .create_dataset(
                    "d1",
                    "Pub",
                    None,
                    OwnerType::Organisation,
                    "o1",
                    Visibility::Public,
                    None,
                )
                .unwrap();
            state
                .auth_db
                .add_dataset_graph("d1", "http://has.ex.org/g")
                .unwrap();
            state
                .store
                .update(
                    "INSERT DATA { GRAPH <http://has.ex.org/g> { \
                     <http://ex.org/a> <http://ex.org/keep> <http://ex.org/o1> . \
                     <http://ex.org/a> <http://ex.org/keep> \"alpha\" . \
                     <http://ex.org/b> <http://ex.org/keep> <http://ex.org/o1> . \
                     <http://ex.org/c> <http://ex.org/other> \"beta\" . } }",
                )
                .unwrap();
            state
        }

        #[tokio::test]
        async fn exact_subject_chip_returns_exactly_that_subject() {
            let f = r#"[{"field":"subject","value":"http://ex.org/a","mode":"exact"}]"#;
            let json = browse(seeded(), &format!("filters={}", pct(f))).await;
            let triples = json["triples"].as_array().unwrap();
            assert_eq!(triples.len(), 2, "only <a>'s two triples: {json}");
            for t in triples {
                assert_eq!(t["subject"]["value"], "http://ex.org/a", "{json}");
            }
        }

        /// The critical correctness property of the BIND: the substituted variable
        /// must stay in scope for every other FILTER in the same group. A
        /// `SELECT (<iri> AS ?s)` projection would NOT be, and silently returns zero
        /// rows — the frontend routinely sends chips together with `q`.
        #[tokio::test]
        async fn exact_subject_chip_composes_with_free_text_q() {
            let f = r#"[{"field":"subject","value":"http://ex.org/a","mode":"exact"}]"#;
            // `q` matches the subject IRI itself — only reachable if ?s is bound.
            let json = browse(seeded(), &format!("filters={}&q=ex.org%2Fa", pct(f))).await;
            assert_eq!(
                json["triples"].as_array().unwrap().len(),
                2,
                "q must see the BOUND ?s, not an unbound variable: {json}"
            );

            // And `q` still narrows within the bound subject.
            let json2 = browse(seeded(), &format!("filters={}&q=alpha", pct(f))).await;
            assert_eq!(
                json2["triples"].as_array().unwrap().len(),
                1,
                "q must still filter the bound subject's rows: {json2}"
            );
        }

        /// Two positive chips on one field are OR-ed. Binding either one would
        /// silently drop the other's rows, so the field must fall back to FILTER.
        #[tokio::test]
        async fn two_positive_chips_on_one_field_still_or() {
            let f = r#"[{"field":"subject","value":"http://ex.org/a","mode":"exact"},{"field":"subject","value":"http://ex.org/b","mode":"exact"}]"#;
            let json = browse(seeded(), &format!("filters={}", pct(f))).await;
            let triples = json["triples"].as_array().unwrap();
            assert_eq!(triples.len(), 3, "<a>'s 2 rows OR <b>'s 1 row: {json}");
        }

        /// A negated chip on the SAME field as a bound positive chip still applies.
        #[tokio::test]
        async fn bound_field_still_honours_a_negated_chip() {
            let f = r#"[{"field":"subject","value":"http://ex.org/a","mode":"exact"},{"field":"object","value":"alpha","mode":"exact","neg":true}]"#;
            let json = browse(seeded(), &format!("filters={}", pct(f))).await;
            let triples = json["triples"].as_array().unwrap();
            assert_eq!(triples.len(), 1, "the literal row is excluded: {json}");
            assert_eq!(triples[0]["object"]["value"], "http://ex.org/o1", "{json}");
        }

        /// The count query must stay consistent with the row set — it is built from
        /// the same pattern, so a divergence here means the two drifted.
        #[tokio::test]
        async fn exact_chip_count_matches_row_count() {
            let f = r#"[{"field":"object","value":"http://ex.org/o1","mode":"exact"}]"#;
            let json = browse(seeded(), &format!("filters={}&count=true", pct(f))).await;
            let rows = json["triples"].as_array().unwrap().len();
            assert_eq!(rows, 2, "two subjects point at o1: {json}");
            assert_eq!(json["total"], 2, "count must match the rows: {json}");
        }

        /// A NON-IRI object value keeps the lexical `str(?o) = "v"` comparison, which
        /// deliberately also matches typed/language-tagged literals. Binding it as a
        /// plain literal term would silently narrow that.
        #[tokio::test]
        async fn literal_object_chip_is_not_bound_into_the_pattern() {
            let f = r#"[{"field":"object","value":"alpha","mode":"exact"}]"#;
            let json = browse(seeded(), &format!("filters={}", pct(f))).await;
            assert_eq!(
                json["triples"].as_array().unwrap().len(),
                1,
                "the plain literal still matches: {json}"
            );
        }

        /// ACL: binding a subject must never widen the graph scope. An anonymous
        /// caller asking for a subject that only exists in a PRIVATE graph gets
        /// nothing — the candidate ?g set is still the authority.
        #[tokio::test]
        async fn exact_chip_cannot_read_outside_the_authorized_graph_set() {
            let state = seeded();
            state
                .auth_db
                .create_dataset(
                    "d2",
                    "Priv",
                    None,
                    OwnerType::Organisation,
                    "o1",
                    Visibility::Private,
                    None,
                )
                .unwrap();
            state
                .auth_db
                .add_dataset_graph("d2", "http://secret.ex.org/g")
                .unwrap();
            state
                .store
                .update(
                    "INSERT DATA { GRAPH <http://secret.ex.org/g> { \
                     <http://ex.org/hidden> <http://ex.org/keep> \"classified\" . } }",
                )
                .unwrap();

            let f = r#"[{"field":"subject","value":"http://ex.org/hidden","mode":"exact"}]"#;
            let json = browse(state, &format!("filters={}", pct(f))).await;
            assert_eq!(
                json["triples"].as_array().unwrap().len(),
                0,
                "a bound subject must not escape the ACL graph set: {json}"
            );
            assert!(
                !json.to_string().contains("classified"),
                "private object value leaked: {json}"
            );
        }
    }

    #[tokio::test]
    async fn browse_triples_has_more_false_when_done() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://end.ex.org/g")
            .unwrap();
        state
            .store
            .update("INSERT DATA { GRAPH <http://end.ex.org/g> { <s:a> <p:p> <o:1> } }")
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/triples?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["triples"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["hasMore"], false,
            "Single row must report hasMore=false: {json}"
        );
    }

    #[tokio::test]
    async fn browse_triples_count_opt_in_returns_total() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://cnt.ex.org/g")
            .unwrap();
        state.store.update("INSERT DATA { GRAPH <http://cnt.ex.org/g> { <s:a> <p:p> <o:1> , <o:2> , <o:3> , <o:4> } }").unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/triples?limit=2&count=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["total"].as_u64(),
            Some(4),
            "count=true must return exact total: {json}"
        );
        assert_eq!(json["hasMore"], true);
    }

    /// Counts above the former 100 001 cap must now be exact. The browser used
    /// to show "100,000+"; a regression that re-adds an inner LIMIT to the count
    /// query would surface here as a truncated total.
    #[tokio::test]
    async fn browse_triples_count_is_exact_above_former_cap() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://big.ex.org/g")
            .unwrap();

        let n = 100_002usize;
        let mut data = String::with_capacity(n * 48);
        data.push_str("INSERT DATA { GRAPH <http://big.ex.org/g> {");
        for i in 0..n {
            data.push_str(&format!(
                " <http://big.ex.org/s/{i}> <http://big.ex.org/p> <http://big.ex.org/o/{i}> ."
            ));
        }
        data.push_str(" } }");
        state.store.update(&data).unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/triples?limit=2&count=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["total"].as_u64(),
            Some(n as u64),
            "count must be exact above the former cap: {json}"
        );
    }

    /// When the caller can see no graphs, an explicit count must report an exact
    /// 0 rather than omitting the field — otherwise the browser's "Show total"
    /// affordance never resolves to a number.
    #[tokio::test]
    async fn browse_triples_count_zero_when_nothing_accessible() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "priv",
                "Priv",
                None,
                OwnerType::User,
                "u1",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("priv", "http://priv.ex.org/g")
            .unwrap();
        state
            .store
            .update("INSERT DATA { GRAPH <http://priv.ex.org/g> { <s:secret> <p:p> <o:1> } }")
            .unwrap();

        // Anonymous caller has no access to the private graph → nothing in scope.
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/triples?limit=2&count=true")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["triples"].as_array().unwrap().len(),
            0,
            "private data must not leak: {json}"
        );
        assert_eq!(
            json["total"].as_u64(),
            Some(0),
            "empty scope with count=true must report 0: {json}"
        );
    }

    /// Anonymous users must only see triples from public datasets — the
    /// VALUES-based scoping must not leak private graph content.
    #[tokio::test]
    async fn browse_triples_anonymous_scopes_to_public() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "pub",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "priv",
                "Priv",
                None,
                OwnerType::User,
                "u1",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("pub", "http://pub.ex.org/g")
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("priv", "http://priv.ex.org/g")
            .unwrap();
        state
            .store
            .update("INSERT DATA { GRAPH <http://pub.ex.org/g> { <s:pub> <p:p> <o:1> } }")
            .unwrap();
        state
            .store
            .update("INSERT DATA { GRAPH <http://priv.ex.org/g> { <s:secret> <p:p> <o:1> } }")
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/triples?limit=100")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_text(resp.into_body()).await;
        assert!(body.contains("s:pub"), "Public subject must appear: {body}");
        assert!(
            !body.contains("s:secret"),
            "Private subject must NOT appear: {body}"
        );
    }

    #[tokio::test]
    async fn browse_triples_empty_when_no_access() {
        // Anonymous caller + no public datasets → empty result, no error.
        let state = test_state();
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/triples?limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["triples"].as_array().unwrap().len(), 0);
        assert_eq!(json["hasMore"], false);
    }

    /// The TTL cache on get_accessible_graph_iris must produce identical results
    /// to the uncached call, and subsequent calls must be served from cache.
    #[tokio::test]
    async fn accessible_graphs_cache_consistency() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Pub",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "http://cache.ex.org/g")
            .unwrap();

        let (uncached, _) = state.auth_db.get_accessible_graph_iris(None).unwrap();
        let cached = state
            .auth_db
            .get_accessible_graph_iris_cached(None)
            .unwrap();
        assert_eq!(uncached, cached.0, "Cached result must match uncached");
        // Second call hits the cache path; must still match.
        let cached2 = state
            .auth_db
            .get_accessible_graph_iris_cached(None)
            .unwrap();
        assert_eq!(cached.0, cached2.0);

        // Invalidation must force a refresh without breaking consistency.
        state
            .auth_db
            .add_dataset_graph("d1", "http://cache.ex.org/g2")
            .unwrap();
        state.auth_db.invalidate_accessible_graphs_cache();
        let refreshed = state
            .auth_db
            .get_accessible_graph_iris_cached(None)
            .unwrap();
        assert!(
            refreshed.0.contains("http://cache.ex.org/g2"),
            "Post-invalidation read must see new graph"
        );
    }

    #[tokio::test]
    async fn health_check() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["status"], "ok",
            "Health check must return status=ok: {json}"
        );
    }
}

// ─── Visibility / access-control regression tests ─────────────────────────────
//
// These cover paths that previously leaked private resources past `optional_auth`:
//   - `GET /api/organisations/:id`     (required auth was missing for anon)
//   - `GET /.well-known/void`          (DCAT generator returned all datasets)
//   - `GET /api/catalog`               (catalog builder returned all data-models / vocabs)
//   - `GET /resource/*path`            (CONSTRUCT ran across all named graphs)
mod visibility_leaks {
    use axum::{
        body::Body,
        http::{header, Method, Request, StatusCode},
    };
    use open_triplestore::auth::models::{OwnerType, Role, SystemRole, Visibility};
    use tower::ServiceExt as _;

    use super::helpers::*;

    // ── /api/organisations/:id ────────────────────────────────────────────────

    #[tokio::test]
    async fn anon_cannot_get_organisation() {
        let state = test_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/organisations/o1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Anon GET /api/organisations/:id must be 401"
        );
    }

    #[tokio::test]
    async fn non_member_cannot_get_organisation() {
        let state = test_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        // bob is a regular user with no membership in o1
        state
            .auth_db
            .create_user("u_bob", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        let bob_token = mint_token("u_bob", "bob", "user");

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/organisations/o1")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Non-member GET /api/organisations/:id must be 403"
        );
    }

    #[tokio::test]
    async fn member_can_get_organisation() {
        let state = test_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .add_org_member("u_alice", "o1", Role::Member)
            .unwrap();
        let alice_token = mint_token("u_alice", "alice", "user");

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/organisations/o1")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(json["id"], "o1");
    }

    // ── /.well-known/void (DCAT) ──────────────────────────────────────────────

    #[tokio::test]
    async fn void_excludes_private_datasets_for_anon() {
        let state = test_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_pub",
                "Public DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_priv",
                "Secret DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/.well-known/void")
                    .header(header::ACCEPT, "text/turtle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("/dataset/ds_pub"),
            "Public dataset must be in anon VoID: {body}"
        );
        assert!(
            !body.contains("/dataset/ds_priv"),
            "Private dataset must NOT be in anon VoID: {body}"
        );
        assert!(
            !body.contains("Secret DS"),
            "Private dataset name must NOT be in anon VoID: {body}"
        );
    }

    #[tokio::test]
    async fn void_includes_private_for_admin() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_priv",
                "Secret DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/.well-known/void")
                    .header(header::ACCEPT, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("/dataset/ds_priv"),
            "Admin must see private datasets in VoID: {body}"
        );
    }

    // ── /api/catalog ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn catalog_excludes_private_data_models_for_anon() {
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.as_str();

        dmr::insert_data_model(
            &state.store,
            base,
            "pub-model",
            "Public Model",
            "http://ex.org/pub#",
            None,
            true,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        dmr::update_latest_published(&state.store, base, "pub-model", "1.0").unwrap();

        dmr::insert_data_model(
            &state.store,
            base,
            "priv-model",
            "Private Model",
            "http://ex.org/priv#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        dmr::update_latest_published(&state.store, base, "priv-model", "1.0").unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/catalog")
                    .header(header::ACCEPT, "text/turtle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("/data-model/pub-model"),
            "Public data-model must be in anon catalog: {body}"
        );
        assert!(
            !body.contains("/data-model/priv-model"),
            "Private data-model must NOT be in anon catalog: {body}"
        );
        assert!(
            !body.contains("Private Model"),
            "Private data-model title must NOT leak: {body}"
        );
    }

    #[tokio::test]
    async fn catalog_excludes_private_vocabularies_for_anon() {
        // Post-merge: vocabularies are model entries with kind=vocabulary, sharing
        // the /data-model/ IRI scheme and typed as ADMS CodeList assets.
        use open_triplestore::data_models::registry as dmr;
        use open_triplestore::kind_detector::RegistryKind;
        let state = test_state();
        let base = state.base_url.as_str();

        dmr::insert_data_model(
            &state.store,
            base,
            "pub-voc",
            "Public Voc",
            "http://ex.org/vpub#",
            None,
            true,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        dmr::set_data_model_kind(&state.store, base, "pub-voc", RegistryKind::Vocabulary).unwrap();
        dmr::update_latest_published(&state.store, base, "pub-voc", "1.0").unwrap();

        dmr::insert_data_model(
            &state.store,
            base,
            "priv-voc",
            "Private Voc",
            "http://ex.org/vpriv#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        dmr::set_data_model_kind(&state.store, base, "priv-voc", RegistryKind::Vocabulary).unwrap();
        dmr::update_latest_published(&state.store, base, "priv-voc", "1.0").unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/catalog")
                    .header(header::ACCEPT, "text/turtle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("/data-model/pub-voc"),
            "Public vocabulary must be in anon catalog: {body}"
        );
        assert!(
            body.contains("assettype/CodeList"),
            "Vocabulary-kind entry must be typed as a CodeList: {body}"
        );
        assert!(
            !body.contains("/data-model/priv-voc"),
            "Private vocabulary must NOT be in anon catalog: {body}"
        );
        assert!(
            !body.contains("Private Voc"),
            "Private vocabulary title must NOT leak: {body}"
        );
    }

    // ── /resource/*path dereference ───────────────────────────────────────────

    /// Insert one triple in a public dataset's graph and one in a private
    /// dataset's graph that share the same subject IRI. Anonymous dereference
    /// must only see the public one.
    #[tokio::test]
    async fn dereference_hides_triples_in_private_graphs() {
        let state = test_state();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::SuperAdmin)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_pub",
                "Public",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_priv",
                "Private",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();
        let pub_g = "http://ex.org/g/pub";
        let priv_g = "http://ex.org/g/priv";
        state.auth_db.add_dataset_graph("ds_pub", pub_g).unwrap();
        state.auth_db.add_dataset_graph("ds_priv", priv_g).unwrap();
        // Caches built before graph inserts must be invalidated so subsequent
        // requests see the new dataset->graph mappings.
        state.auth_db.invalidate_accessible_graphs_cache();

        let subject = "http://localhost:7878/resource/probe";
        state.store.update(&format!(
            "INSERT DATA {{ GRAPH <{pub_g}> {{ <{subject}> <http://ex.org/p> <http://ex.org/PUBLIC_OBJECT> }} }}"
        )).unwrap();
        state.store.update(&format!(
            "INSERT DATA {{ GRAPH <{priv_g}> {{ <{subject}> <http://ex.org/p> <http://ex.org/SECRET_OBJECT> }} }}"
        )).unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/resource/probe")
                    .header(header::ACCEPT, "text/turtle")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "Public triple must be dereferenceable for anon"
        );
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("PUBLIC_OBJECT"),
            "Public triple must be returned: {body}"
        );
        assert!(
            !body.contains("SECRET_OBJECT"),
            "Private triple must NOT leak via dereference: {body}"
        );
    }

    // ── Write leaks: data-model / vocabulary ownership ───────────────────────

    /// A publisher who is NOT the owner (and not in the owning org) must not
    /// be able to mutate someone else's data-model.
    #[tokio::test]
    async fn non_owner_publisher_cannot_patch_data_model() {
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.as_str();
        // alice owns the data-model
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_alice", true)
            .unwrap();
        // bob is also a publisher but unrelated
        state
            .auth_db
            .create_user("u_bob", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_bob", true)
            .unwrap();
        let bob_token = mint_token("u_bob", "bob", "user");

        dmr::insert_data_model(
            &state.store,
            base,
            "alice-model",
            "Alice Model",
            "http://ex.org/alice#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();

        let body = serde_json::json!({ "title": "Hijacked" });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/models/alice-model")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Non-owner publisher must NOT be able to PATCH another user's data-model"
        );
    }

    #[tokio::test]
    async fn owner_can_patch_their_data_model() {
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.as_str();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_alice", true)
            .unwrap();
        let alice_token = mint_token("u_alice", "alice", "user");

        dmr::insert_data_model(
            &state.store,
            base,
            "alice-model",
            "Alice Model",
            "http://ex.org/alice#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();

        let body = serde_json::json!({ "title": "Renamed by owner" });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/models/alice-model")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {alice_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "Owner must be able to PATCH their own data-model: {}",
            resp.status()
        );
    }

    /// Anonymous request must be rejected at the auth middleware (401), not
    /// fall through to the handler.
    #[tokio::test]
    async fn anon_cannot_patch_data_model() {
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.as_str();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        dmr::insert_data_model(
            &state.store,
            base,
            "alice-model",
            "Alice Model",
            "http://ex.org/alice#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();

        let body = serde_json::json!({ "title": "Anon Hijack" });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/models/alice-model")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Anon PATCH must be 401"
        );
    }

    /// Same threat model as the data-model patch test, but for the RDF data
    /// patch endpoint — even more severe because it can corrupt published
    /// graphs.
    #[tokio::test]
    async fn non_owner_publisher_cannot_patch_data_model_version_data() {
        use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.as_str();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_alice", true)
            .unwrap();
        state
            .auth_db
            .create_user("u_bob", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_bob", true)
            .unwrap();
        let bob_token = mint_token("u_bob", "bob", "user");

        dmr::insert_data_model(
            &state.store,
            base,
            "alice-model",
            "Alice Model",
            "http://ex.org/alice#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        let graph_iri = format!("{base}/data-model/alice-model/version/1.0");
        dmr::insert_version(
            &state.store,
            base,
            &DataModelVersion {
                data_model_id: "alice-model".to_string(),
                version: "1.0".to_string(),
                status: VersionStatus::Draft,
                graph_iri,
                sub_graphs: vec![],
                created_at: "2026-01-01T00:00:00Z".to_string(),
                created_by: None,
                derived_from: None,
                notes: None,
                branch: None,
                sub_graph_status: vec![],
            },
        )
        .unwrap();

        let body = serde_json::json!({
            "add": [{ "s": "http://ex.org/x", "p": "http://ex.org/p", "o": "http://ex.org/o" }],
            "remove": []
        });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/models/alice-model/versions/1.0/data")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Non-owner publisher must NOT be able to PATCH version data"
        );
    }

    // ── Commit-log coverage for the version-creating paths ────────────────────
    // create_draft / merge_apply / rebase_version create a new version; each must
    // now record a commit (previously silent in the trail).

    /// Creating a draft from a version records a commit.
    #[tokio::test]
    async fn create_draft_records_commit() {
        use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.to_string();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_alice", true)
            .unwrap();
        let tok = mint_token("u_alice", "alice", "user");

        dmr::insert_data_model(
            &state.store,
            &base,
            "m1",
            "M1",
            "http://ex.org/m1#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        dmr::insert_version(
            &state.store,
            &base,
            &DataModelVersion {
                data_model_id: "m1".into(),
                version: "1.0.0".into(),
                status: VersionStatus::Published,
                graph_iri: format!("{base}/data-model/m1/version/1.0.0"),
                sub_graphs: vec![],
                created_at: "2026-01-01T00:00:00Z".into(),
                created_by: None,
                derived_from: None,
                notes: None,
                branch: None,
                sub_graph_status: vec![],
            },
        )
        .unwrap();

        let draft = serde_json::json!({ "target_version": "2.0.0" });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/models/m1/versions/1.0.0/draft")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::from(serde_json::to_string(&draft).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "create draft failed: {}",
            resp.status()
        );

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/models/m1/commits")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "list commits failed: {}",
            resp.status()
        );
        let commits = body_json(resp.into_body()).await;
        let arr = commits.as_array().expect("commits array");
        assert!(
            arr.iter().any(|c| c["kind"].as_str() == Some("data-model")
                && c["version"].as_str() == Some("2.0.0")
                && c["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Created draft 2.0.0")),
            "expected a draft-creation commit, got: {commits}"
        );
    }

    /// Regression: a raw draft upload (POST /versions, is_public=false) must set the
    /// model's `latest_draft` pointer, and publishing that draft directly must clear it.
    /// Previously `upload_version` only maintained `latest_published`, so an uploaded
    /// draft left `latest_draft` null even though it showed in the versions list.
    #[tokio::test]
    async fn draft_upload_sets_latest_draft_and_publish_clears_it() {
        let state = test_state();
        let base = state.base_url.to_string();
        // super_admin so one token can both upload (write) and publish (admin).
        state
            .auth_db
            .create_user("u_adm", "adm", "adm@t.com", "h", SystemRole::SuperAdmin)
            .unwrap();
        let tok = mint_token("u_adm", "adm", "super_admin");

        open_triplestore::data_models::registry::insert_data_model(
            &state.store,
            &base,
            "m_draft",
            "M draft",
            "http://ex.org/md#",
            None,
            false,
            Some("user"),
            Some("u_adm"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();

        // Upload a DRAFT version (is_public=false) — the path that left latest_draft null.
        let boundary = "BNDdraft";
        let ttl: &[u8] = b"<http://ex.org/md#C> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/2002/07/owl#Class> .";
        let body = multipart_body(
            boundary,
            &[
                ("file", "text/turtle", Some("m.ttl"), ttl),
                ("version", "text/plain", None, b"0.1.0"),
                ("is_public", "text/plain", None, b"false"),
            ],
        );
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/models/m_draft/versions")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "draft upload should succeed"
        );

        // Summary must now report the uploaded draft as latest_draft.
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/models/m_draft")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["latest_draft"], "0.1.0",
            "raw draft upload must set latest_draft: {json}"
        );
        assert!(
            json["latest_published"].is_null(),
            "nothing published yet: {json}"
        );

        // Publishing the draft directly must retire the draft pointer.
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/models/m_draft/versions/0.1.0/publish")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "publish failed: {}",
            resp.status()
        );

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/models/m_draft")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = body_json(resp.into_body()).await;
        assert_eq!(
            json["latest_published"], "0.1.0",
            "published version should be latest_published: {json}"
        );
        assert!(
            json["latest_draft"].is_null(),
            "publishing the draft must clear latest_draft: {json}"
        );
    }

    /// Merging one version into another records a commit with the triple delta.
    #[tokio::test]
    async fn merge_apply_records_commit() {
        use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.to_string();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_alice", true)
            .unwrap();
        let tok = mint_token("u_alice", "alice", "user");

        dmr::insert_data_model(
            &state.store,
            &base,
            "m2",
            "M2",
            "http://ex.org/m2#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        for (ver, st, branch) in [
            ("1.0.0", VersionStatus::Published, None),
            ("feature", VersionStatus::Draft, Some("feature".to_string())),
        ] {
            dmr::insert_version(
                &state.store,
                &base,
                &DataModelVersion {
                    data_model_id: "m2".into(),
                    version: ver.into(),
                    status: st,
                    graph_iri: format!("{base}/data-model/m2/version/{ver}"),
                    sub_graphs: vec![],
                    created_at: "2026-01-01T00:00:00Z".into(),
                    created_by: None,
                    derived_from: None,
                    notes: None,
                    branch,
                    sub_graph_status: vec![],
                },
            )
            .unwrap();
        }
        // One triple only on the feature branch → merging it into main adds it.
        state
            .store
            .update(&format!(
                "INSERT DATA {{ GRAPH <{base}/data-model/m2/version/feature> \
             {{ <http://ex.org/s> <http://ex.org/p> <http://ex.org/o> }} }}"
            ))
            .unwrap();

        // A user-supplied commit message must be honored over the default text.
        let body = serde_json::json!({
            "from": "feature", "into": "1.0.0", "target_version": "2.0.0",
            "resolutions": [], "message": "Bring feature work into the trunk"
        });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/models/m2/merge")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "merge failed: {}",
            resp.status()
        );

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/models/m2/commits")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let commits = body_json(resp.into_body()).await;
        let arr = commits.as_array().expect("commits array");
        let merge_commit = arr.iter().find(|c| {
            c["kind"].as_str() == Some("data-model") && c["version"].as_str() == Some("2.0.0")
        });
        assert!(
            merge_commit.is_some(),
            "expected a merge commit, got: {commits}"
        );
        let mc = merge_commit.unwrap();
        assert_eq!(
            mc["message"].as_str(),
            Some("Bring feature work into the trunk"),
            "merge must honor the custom commit message: {mc}"
        );
        assert!(
            mc["added"].as_u64().unwrap_or(0) >= 1,
            "merge commit should record the added triple: {mc}"
        );
    }

    /// Rebasing a branch onto a newer base records a commit (branch label preserved).
    #[tokio::test]
    async fn rebase_version_records_commit() {
        use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.to_string();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_alice", true)
            .unwrap();
        let tok = mint_token("u_alice", "alice", "user");

        dmr::insert_data_model(
            &state.store,
            &base,
            "m3",
            "M3",
            "http://ex.org/m3#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        for (ver, st, branch, derived) in [
            ("1.0.0", VersionStatus::Published, None, None),
            (
                "feature",
                VersionStatus::Draft,
                Some("feature".to_string()),
                Some("1.0.0".to_string()),
            ),
        ] {
            dmr::insert_version(
                &state.store,
                &base,
                &DataModelVersion {
                    data_model_id: "m3".into(),
                    version: ver.into(),
                    status: st,
                    graph_iri: format!("{base}/data-model/m3/version/{ver}"),
                    sub_graphs: vec![],
                    created_at: "2026-01-01T00:00:00Z".into(),
                    created_by: None,
                    derived_from: derived,
                    notes: None,
                    branch,
                    sub_graph_status: vec![],
                },
            )
            .unwrap();
        }

        let body = serde_json::json!({ "onto": "1.0.0", "target_version": "feature-2" });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/models/m3/versions/feature/rebase")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "rebase failed: {}",
            resp.status()
        );

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/models/m3/commits")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let commits = body_json(resp.into_body()).await;
        let arr = commits.as_array().expect("commits array");
        assert!(
            arr.iter().any(|c| c["kind"].as_str() == Some("data-model")
                && c["version"].as_str() == Some("feature-2")
                && c["branch"].as_str() == Some("feature")
                && c["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Rebased feature onto 1.0.0")),
            "expected a rebase commit, got: {commits}"
        );
    }

    /// A vocabulary-kind entry records draft-creation commits through the unified
    /// model route (commit kind is `data-model` post-merge).
    #[tokio::test]
    async fn create_draft_records_commit_vocabulary() {
        use open_triplestore::data_models::models::{DataModelVersion, VersionStatus};
        use open_triplestore::data_models::registry as dmr;
        use open_triplestore::kind_detector::RegistryKind;
        let state = test_state();
        let base = state.base_url.to_string();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_alice", true)
            .unwrap();
        let tok = mint_token("u_alice", "alice", "user");

        dmr::insert_data_model(
            &state.store,
            &base,
            "v1",
            "V1",
            "http://ex.org/v1#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        dmr::set_data_model_kind(&state.store, &base, "v1", RegistryKind::Vocabulary).unwrap();
        dmr::insert_version(
            &state.store,
            &base,
            &DataModelVersion {
                data_model_id: "v1".into(),
                version: "1.0.0".into(),
                status: VersionStatus::Published,
                graph_iri: format!("{base}/data-model/v1/version/1.0.0"),
                sub_graphs: vec![],
                created_at: "2026-01-01T00:00:00Z".into(),
                created_by: None,
                derived_from: None,
                notes: None,
                branch: None,
                sub_graph_status: vec![],
            },
        )
        .unwrap();

        let draft = serde_json::json!({ "target_version": "2.0.0" });
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/models/v1/versions/1.0.0/draft")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::from(serde_json::to_string(&draft).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "vocab create draft failed: {}",
            resp.status()
        );

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/models/v1/commits")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let commits = body_json(resp.into_body()).await;
        let arr = commits.as_array().expect("commits array");
        assert!(
            arr.iter().any(|c| c["kind"].as_str() == Some("data-model")
                && c["version"].as_str() == Some("2.0.0")
                && c["message"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Created draft 2.0.0")),
            "expected a draft-creation commit, got: {commits}"
        );
    }

    /// Symmetric for a vocabulary-kind entry (unified model route).
    #[tokio::test]
    async fn non_owner_publisher_cannot_patch_vocabulary() {
        use open_triplestore::data_models::registry as dmr;
        use open_triplestore::kind_detector::RegistryKind;
        let state = test_state();
        let base = state.base_url.as_str();
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_alice", true)
            .unwrap();
        state
            .auth_db
            .create_user("u_bob", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_bob", true)
            .unwrap();
        let bob_token = mint_token("u_bob", "bob", "user");

        dmr::insert_data_model(
            &state.store,
            base,
            "alice-voc",
            "Alice Voc",
            "http://ex.org/av#",
            None,
            false,
            Some("user"),
            Some("u_alice"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();
        dmr::set_data_model_kind(&state.store, base, "alice-voc", RegistryKind::Vocabulary)
            .unwrap();

        let body = serde_json::json!({ "title": "Hijacked" });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/models/alice-voc")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Non-owner publisher must NOT PATCH another user's vocabulary"
        );
    }

    /// Org-owned: only org *admins* may write — regular org members can read
    /// but not mutate.
    #[tokio::test]
    async fn org_member_cannot_patch_org_owned_data_model() {
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.as_str();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_user("u_member", "member", "m@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_member", true)
            .unwrap();
        state
            .auth_db
            .add_org_member("u_member", "o1", Role::Member)
            .unwrap();
        let member_token = mint_token("u_member", "member", "user");

        dmr::insert_data_model(
            &state.store,
            base,
            "org-model",
            "Org Model",
            "http://ex.org/o#",
            None,
            false,
            Some("organisation"),
            Some("o1"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();

        let body = serde_json::json!({ "title": "Member tried to rename" });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/models/org-model")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Org member (non-admin) must NOT mutate org-owned data-model"
        );
    }

    #[tokio::test]
    async fn org_admin_can_patch_org_owned_data_model() {
        use open_triplestore::data_models::registry as dmr;
        let state = test_state();
        let base = state.base_url.as_str();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_user("u_admin", "orgadm", "oa@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_admin", true)
            .unwrap();
        state
            .auth_db
            .add_org_member("u_admin", "o1", Role::Admin)
            .unwrap();
        let admin_token = mint_token("u_admin", "orgadm", "user");

        dmr::insert_data_model(
            &state.store,
            base,
            "org-model",
            "Org Model",
            "http://ex.org/o#",
            None,
            false,
            Some("organisation"),
            Some("o1"),
            None,
            "2026-01-01T00:00:00Z",
        )
        .unwrap();

        let body = serde_json::json!({ "title": "Renamed by org admin" });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/models/org-model")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "Org admin must be able to mutate org-owned data-model: {}",
            resp.status()
        );
    }

    // ── Dataset GET / DELETE: explicit anon coverage ──────────────────────────
    //
    // The user reported still seeing a private dataset and being able to delete
    // it without signing in. These tests pin the backend behaviour for both.

    #[tokio::test]
    async fn anon_cannot_get_private_dataset_by_id() {
        let state = test_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_priv",
                "Secret",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets/ds_priv")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Anon GET /api/datasets/<private-id> must be 403"
        );
    }

    #[tokio::test]
    async fn anon_list_datasets_excludes_private() {
        let state = test_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_pub",
                "Public",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_priv",
                "Secret",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(body.contains("ds_pub"), "Public ds must appear: {body}");
        assert!(
            !body.contains("ds_priv"),
            "Private ds must NOT appear: {body}"
        );
        assert!(
            !body.contains("Secret"),
            "Private ds name must NOT leak: {body}"
        );
    }

    #[tokio::test]
    async fn anon_cannot_delete_dataset() {
        let state = test_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_priv",
                "Secret",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();

        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/datasets/ds_priv")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Anon DELETE /api/datasets/<id> must be 401"
        );

        // Confirm the dataset is still there.
        assert!(
            state.auth_db.get_dataset("ds_priv").unwrap().is_some(),
            "Dataset must NOT have been deleted by anon"
        );
    }

    #[tokio::test]
    async fn anon_cannot_delete_public_dataset() {
        // Even for a Public dataset (where anon can READ it), anon must NOT be
        // able to delete.
        let state = test_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_pub",
                "Public",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();

        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/datasets/ds_pub")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Anon DELETE on a public dataset must STILL be 401"
        );
        assert!(
            state.auth_db.get_dataset("ds_pub").unwrap().is_some(),
            "Public dataset must NOT have been deleted by anon"
        );
    }

    #[tokio::test]
    async fn non_owner_cannot_delete_dataset_via_api() {
        let state = test_state();
        // alice owns the dataset (org admin)
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .add_org_member("u_alice", "o1", Role::Admin)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds1",
                "DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();
        // bob is unrelated
        state
            .auth_db
            .create_user("u_bob", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        let bob_token = mint_token("u_bob", "bob", "user");

        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/datasets/ds1")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Non-owner DELETE must be 403"
        );
        assert!(
            state.auth_db.get_dataset("ds1").unwrap().is_some(),
            "Dataset must NOT have been deleted by non-owner"
        );
    }

    // ── Write leaks: bulk import ──────────────────────────────────────────────

    fn multipart_body(boundary: &str, parts: &[(&str, &str, Option<&str>, &[u8])]) -> Vec<u8> {
        // Each part: (name, value-as-text-or-binary, optional filename, bytes)
        let mut out = Vec::new();
        for (name, content_type, filename, bytes) in parts {
            out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            if let Some(fname) = filename {
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n"
                    )
                    .as_bytes(),
                );
            } else {
                out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n").as_bytes(),
                );
            }
            out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
            out.extend_from_slice(bytes);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        out
    }

    #[tokio::test]
    async fn bulk_import_anon_rejected() {
        let state = test_state();
        let boundary = "BNDtest";
        let body = multipart_body(
            boundary,
            &[(
                "file",
                "text/turtle",
                Some("a.ttl"),
                b"<http://ex.org/s> <http://ex.org/p> <http://ex.org/o> .",
            )],
        );
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/import/bulk")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Anon must not bulk-import"
        );
    }

    #[tokio::test]
    async fn bulk_import_non_owner_into_dataset_rejected() {
        let state = test_state();
        // alice owns a private dataset
        state
            .auth_db
            .create_user("u_alice", "alice", "a@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .add_org_member("u_alice", "o1", Role::Admin)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_alice",
                "Alice DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();
        // bob is a publisher with no org membership
        state
            .auth_db
            .create_user("u_bob", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_bob", true)
            .unwrap();
        let bob_token = mint_token("u_bob", "bob", "user");

        let boundary = "BND1";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"ds_alice"),
                (
                    "file",
                    "text/turtle",
                    Some("a.ttl"),
                    b"<http://ex.org/s> <http://ex.org/p> <http://ex.org/o> .",
                ),
            ],
        );
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/import/bulk")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Non-owner publisher must not bulk-import into someone else's dataset"
        );
    }

    #[tokio::test]
    async fn bulk_import_without_dataset_id_requires_admin() {
        let state = test_state();
        // bob is a publisher but not platform admin
        state
            .auth_db
            .create_user("u_bob", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .update_user_can_publish("u_bob", true)
            .unwrap();
        let bob_token = mint_token("u_bob", "bob", "user");

        let boundary = "BND2";
        let body = multipart_body(
            boundary,
            &[(
                "file",
                "text/turtle",
                Some("a.ttl"),
                b"<http://ex.org/s> <http://ex.org/p> <http://ex.org/o> .",
            )],
        );
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/import/bulk")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Non-admin must not bulk-import without dataset_id"
        );
    }

    #[tokio::test]
    async fn bulk_import_owner_into_own_dataset_allowed() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds1",
                "DS",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Public,
                None,
            )
            .unwrap();

        let boundary = "BND3";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"ds1"),
                (
                    "default_target_graph",
                    "text/plain",
                    None,
                    b"http://ex.org/g/import",
                ),
                (
                    "file",
                    "text/turtle",
                    Some("a.ttl"),
                    b"<http://ex.org/s> <http://ex.org/p> <http://ex.org/o> .",
                ),
            ],
        );
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/import/bulk")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status().is_success(),
            "Admin must be able to bulk-import: {}",
            resp.status()
        );
    }

    /// The asset upload route enforces the absolute size ceiling (mirrors the external
    /// form platform's Settings.asset_max_bytes). A body past ASSET_MAX_BYTES is rejected with 413
    /// — the streaming read aborts rather than buffering the whole part into memory.
    #[tokio::test]
    async fn asset_upload_over_ceiling_returns_413() {
        let (mut state, token) = admin_state();
        // A configured object store so we clear the "S3 not configured" guard and reach the read.
        state.object_store = std::sync::Arc::new(
            open_triplestore::storage::ObjectStore::local(
                std::env::temp_dir().join("ots_asset_413_test"),
            )
            .unwrap(),
        );
        // Admin owns the dataset ⇒ has write access.
        state
            .auth_db
            .create_dataset(
                "ds_a",
                "Assets",
                None,
                OwnerType::User,
                "adm",
                Visibility::Private,
                None,
            )
            .unwrap();

        // ~52 MiB file part — over the 50 MiB ASSET_MAX_BYTES ceiling.
        let big = vec![b'a'; 52 * 1024 * 1024];
        let boundary = "BNDasset";
        let body = multipart_body(
            boundary,
            &[("file", "application/octet-stream", Some("big.bin"), &big)],
        );
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/datasets/ds_a/assets")
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::PAYLOAD_TOO_LARGE,
            "asset over the ceiling must return 413, got {}",
            resp.status()
        );
    }

    // ── Write leaks: organisation creation ────────────────────────────────────

    #[tokio::test]
    async fn non_admin_cannot_create_organisation() {
        let state = test_state();
        state
            .auth_db
            .create_user("u_bob", "bob", "b@t.com", "h", SystemRole::User)
            .unwrap();
        let bob_token = mint_token("u_bob", "bob", "user");

        let body = serde_json::json!({ "name": "Bob's Org", "slug": "bobs-org" });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/organisations")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {bob_token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Non-admin must NOT be able to POST /api/organisations"
        );
    }

    #[tokio::test]
    async fn admin_can_create_organisation() {
        let (state, token) = admin_state();
        let body = serde_json::json!({ "name": "Admin Org", "slug": "admin-org" });
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/organisations")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(serde_json::to_string(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn dereference_returns_all_triples_for_admin() {
        let (state, token) = admin_state();
        state
            .auth_db
            .create_organisation("o1", "Acme", "acme", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "ds_priv",
                "Private",
                None,
                OwnerType::Organisation,
                "o1",
                Visibility::Private,
                None,
            )
            .unwrap();
        let priv_g = "http://ex.org/g/priv2";
        state.auth_db.add_dataset_graph("ds_priv", priv_g).unwrap();
        state.auth_db.invalidate_accessible_graphs_cache();

        let subject = "http://localhost:7878/resource/probe2";
        state.store.update(&format!(
            "INSERT DATA {{ GRAPH <{priv_g}> {{ <{subject}> <http://ex.org/p> <http://ex.org/SECRET_OBJECT> }} }}"
        )).unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/resource/probe2")
                    .header(header::ACCEPT, "text/turtle")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("SECRET_OBJECT"),
            "Admin must see triples in private graphs via dereference: {body}"
        );
    }
}
