//! Regression tests for the security hardening pass.
//!
//! Each test pins a specific fix so the vulnerability cannot silently return.
//! They build a fully in-memory `AppState` and drive the router via
//! `tower::ServiceExt::oneshot` (no real port is bound), matching the harness in
//! `security_tests.rs`. Run as part of `cargo test --all-features`.
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    use crate::auth::db::AuthDb;
    use crate::auth::jwt::{issue_access_token, JwtConfig};
    use crate::auth::models::{ApiScope, OwnerType, SystemRole, Visibility};
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
        }
    }

    fn test_app(state: AppState) -> axum::Router {
        build_router(state, "", vec![])
    }

    async fn body_text(body: Body) -> String {
        let bytes = body.collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn token(user_id: &str, username: &str, role: &str) -> String {
        issue_access_token(
            &JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30),
            user_id,
            username,
            role,
        )
        .unwrap()
    }

    /// Create a user with a real Argon2 password hash (so the login endpoint works).
    fn create_user_with_password(
        state: &AppState,
        id: &str,
        username: &str,
        password: &str,
        role: SystemRole,
    ) {
        let hash = crate::auth::password::hash_password(password).unwrap();
        state
            .auth_db
            .create_user(id, username, &format!("{username}@test.com"), &hash, role)
            .unwrap();
    }

    async fn sparql_update(app: axum::Router, token: &str, update: &str) -> StatusCode {
        app.oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/sparql")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/sparql-update")
                .body(Body::from(update.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    // ─── Finding 15–17: security response headers ─────────────────────────────

    #[tokio::test]
    async fn security_headers_present() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let h = resp.headers();
        assert_eq!(
            h.get("x-frame-options").map(|v| v.to_str().unwrap()),
            Some("DENY")
        );
        assert_eq!(
            h.get("x-content-type-options").map(|v| v.to_str().unwrap()),
            Some("nosniff")
        );
        assert!(
            h.get("referrer-policy").is_some(),
            "Referrer-Policy must be set"
        );
        let csp = h.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(
            csp.contains("frame-ancestors 'none'"),
            "CSP must forbid framing: {csp}"
        );
        assert!(
            !csp.contains("script-src 'unsafe-inline'"),
            "script-src must not allow unsafe-inline"
        );
    }

    // ─── Finding 16: CORS only reflects a configured origin ───────────────────

    #[tokio::test]
    async fn cors_rejects_unconfigured_origin() {
        let app = build_router(test_state(), "https://app.example.com", vec![]);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .header(header::ORIGIN, "https://evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let acao = resp
            .headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap().to_string());
        assert_ne!(
            acao.as_deref(),
            Some("https://evil.example.com"),
            "an unconfigured origin must never be reflected in Access-Control-Allow-Origin"
        );
    }

    // ─── `CORS_ORIGINS=*` mirrors any origin so any-host browser clients connect ──

    #[tokio::test]
    async fn cors_wildcard_mirrors_any_origin_with_credentials() {
        // A browser client (e.g. the OTL viewer on http://localhost:5190) preflights a
        // credentialed request against a store launched with CORS_ORIGINS=*. The store
        // must echo that exact origin (never the literal '*') and allow credentials, or
        // the browser blocks the response. It must also reflect the requested headers —
        // including ones outside the fixed allow-list — so any-origin mode is actually
        // usable for clients we don't control.
        let app = build_router(test_state(), "*", vec![]);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/auth/me")
                    .header(header::ORIGIN, "http://localhost:5190")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "authorization,x-otl-custom",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let headers = resp.headers();
        assert_eq!(
            headers
                .get("access-control-allow-origin")
                .and_then(|v| v.to_str().ok()),
            Some("http://localhost:5190"),
            "wildcard CORS must mirror the caller's Origin, not the literal '*'"
        );
        assert_eq!(
            headers
                .get("access-control-allow-credentials")
                .and_then(|v| v.to_str().ok()),
            Some("true"),
            "credentialed cross-origin fetches require Access-Control-Allow-Credentials: true"
        );
        // The non-standard `x-otl-custom` header is not in the fixed allow-list, so the
        // preflight only succeeds if mirror mode reflects the requested headers.
        let allowed = headers
            .get("access-control-allow-headers")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(
            allowed.contains("x-otl-custom"),
            "wildcard CORS must mirror requested headers, got: {allowed:?}"
        );
    }

    // ─── Mirror mode's safety rests on session cookies staying SameSite=Strict ────

    #[tokio::test]
    async fn auth_session_cookies_are_samesite_strict() {
        // The CORS_ORIGINS=* mirror mode reflects any origin WITH credentials. That is
        // only safe because the browser withholds the session cookies on cross-site
        // requests — which holds solely while those cookies are SameSite=Strict. If a
        // future change downgrades them to Lax/None this test fails, instead of silently
        // turning mirror mode into a credentialed-CORS CSRF / account-takeover hole.
        let state = test_state();
        create_user_with_password(
            &state,
            "u1",
            "alice",
            "correct-horse-battery",
            SystemRole::User,
        );
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"alice","password":"correct-horse-battery"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "valid login should succeed");

        let cookies: Vec<String> = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(str::to_string)
            .collect();
        let session_cookies: Vec<&String> = cookies
            .iter()
            .filter(|c| c.starts_with("access_token=") || c.starts_with("refresh_token="))
            .collect();
        assert_eq!(
            session_cookies.len(),
            2,
            "login must set both access_token and refresh_token cookies, got: {cookies:?}"
        );
        for c in session_cookies {
            assert!(
                c.contains("SameSite=Strict"),
                "session cookie must be SameSite=Strict (mirror-mode CORS safety depends on \
                 it); got: {c}"
            );
        }
    }

    // ─── Finding 1: variable-graph UPDATE is admin-only ───────────────────────

    #[tokio::test]
    async fn update_variable_graph_target_requires_admin() {
        let state = test_state();
        // Non-admin with a write grant on ONE specific graph.
        state
            .auth_db
            .create_user("u1", "writer", "w@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_user("adm", "admin", "a@t.com", "h", SystemRole::Admin)
            .unwrap();
        state
            .auth_db
            .grant_graph_permission("g1", "urn:mine", "user", "u1", "write", "adm")
            .unwrap();
        let user = token("u1", "writer", "user");

        // A variable-graph rewrite touches every graph → must be refused for non-admins.
        let evil = "DELETE { GRAPH ?g { ?s ?p ?o } } INSERT { GRAPH ?g { ?s <urn:pwned> 1 } } WHERE { GRAPH ?g { ?s ?p ?o } }";
        let status = sparql_update(test_app(state), &user, evil).await;
        assert!(
            status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
            "variable-graph UPDATE by a non-admin must be denied, got {status}"
        );
    }

    // ─── Finding 2: UPDATE WHERE/USING read side is access-scoped ──────────────

    #[tokio::test]
    async fn update_cross_graph_read_denied() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "writer", "w@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_user("adm", "admin", "a@t.com", "h", SystemRole::Admin)
            .unwrap();
        // Writer can WRITE urn:mine but has no READ grant on urn:secret.
        state
            .auth_db
            .grant_graph_permission("g1", "urn:mine", "user", "u1", "write", "adm")
            .unwrap();
        state
            .store
            .update("INSERT DATA { GRAPH <urn:secret> { <s:x> <p:y> \"top-secret\" } }")
            .unwrap();
        let user = token("u1", "writer", "user");

        // Copy data out of a graph the writer cannot read into one they can.
        let exfil =
            "INSERT { GRAPH <urn:mine> { ?s ?p ?o } } WHERE { GRAPH <urn:secret> { ?s ?p ?o } }";
        let status = sparql_update(test_app(state), &user, exfil).await;
        assert!(
            status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
            "reading an unauthorized graph in an UPDATE WHERE must be denied, got {status}"
        );
    }

    // ─── Finding 30: browse_suggest rejects backslash in prefix ───────────────

    #[tokio::test]
    async fn browse_suggest_rejects_backslash_prefix() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/suggest?field=subject&prefix=x%5C") // prefix = "x\"
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "a backslash in the suggest prefix must be rejected (not corrupt the SPARQL)"
        );
    }

    // ─── Finding 10: login gives a uniform response (no enumeration) ──────────

    #[tokio::test]
    async fn login_uniform_response_unknown_vs_deactivated() {
        let state = test_state();
        create_user_with_password(
            &state,
            "u1",
            "realuser",
            "correct-horse-battery",
            SystemRole::User,
        );
        state.auth_db.set_user_active("u1", false).unwrap(); // deactivate

        let login = |username: &str, password: &str| {
            Request::builder()
                .method(Method::POST)
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"username":"{username}","password":"{password}"}}"#
                )))
                .unwrap()
        };

        // Unknown user.
        let r1 = test_app(state.clone())
            .oneshot(login("ghost", "whatever"))
            .await
            .unwrap();
        let s1 = r1.status();
        let b1 = body_text(r1.into_body()).await;

        // Existing-but-deactivated user with the CORRECT password.
        let r2 = test_app(state)
            .oneshot(login("realuser", "correct-horse-battery"))
            .await
            .unwrap();
        let s2 = r2.status();
        let b2 = body_text(r2.into_body()).await;

        assert_eq!(s1, StatusCode::UNAUTHORIZED);
        assert_eq!(s2, StatusCode::UNAUTHORIZED);
        assert_eq!(
            b1, b2,
            "unknown-user and deactivated-user responses must be identical (no enumeration)"
        );
        assert!(
            !b2.to_lowercase().contains("deactivat"),
            "the deactivated reason must not leak in the response body"
        );
    }

    // ─── Finding 5: legacy delete_user enforces the role hierarchy ────────────

    #[tokio::test]
    async fn legacy_delete_user_cannot_delete_higher_role() {
        let state = test_state();
        state
            .auth_db
            .create_user("adm", "admin", "a@t.com", "h", SystemRole::Admin)
            .unwrap();
        state
            .auth_db
            .create_user("root", "root", "r@t.com", "h", SystemRole::SuperAdmin)
            .unwrap();
        let admin = token("adm", "admin", "admin");

        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/users/root")
                    .header(header::AUTHORIZATION, format!("Bearer {admin}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "an admin must not be able to delete a super_admin"
        );
        assert!(
            state.auth_db.get_user_by_id("root").unwrap().is_some(),
            "the super_admin account must still exist"
        );
    }

    // ─── Finding 12: the last super_admin cannot be demoted ───────────────────

    #[tokio::test]
    async fn cannot_demote_last_super_admin() {
        let state = test_state();
        state
            .auth_db
            .create_user("root", "root", "r@t.com", "h", SystemRole::SuperAdmin)
            .unwrap();
        let root = token("root", "root", "super_admin");

        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/admin/users/root")
                    .header(header::AUTHORIZATION, format!("Bearer {root}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"role":"user"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CONFLICT,
            "demoting the last active super_admin must be refused"
        );
        assert_eq!(
            state.auth_db.get_user_by_id("root").unwrap().unwrap().role,
            SystemRole::SuperAdmin
        );
    }

    #[test]
    fn count_active_super_admins_counts_only_active() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("r1", "root1", "r1@t.com", "h", SystemRole::SuperAdmin)
            .unwrap();
        db.create_user("r2", "root2", "r2@t.com", "h", SystemRole::SuperAdmin)
            .unwrap();
        db.create_user("u1", "user1", "u1@t.com", "h", SystemRole::User)
            .unwrap();
        assert_eq!(db.count_active_super_admins().unwrap(), 2);
        db.set_user_active("r2", false).unwrap();
        assert_eq!(db.count_active_super_admins().unwrap(), 1);
    }

    // ─── Finding 4: cyclic SHACL shapes terminate (no stack-overflow crash) ───

    #[tokio::test]
    async fn shacl_cyclic_node_reference_terminates() {
        let state = test_state();
        // Two shapes that reference each other via sh:node, plus one target instance.
        state
            .store
            .update(
                "PREFIX sh: <http://www.w3.org/ns/shacl#> \
             INSERT DATA { GRAPH <urn:shapes> { \
               <urn:A> a sh:NodeShape ; sh:targetClass <urn:T> ; sh:node <urn:B> . \
               <urn:B> a sh:NodeShape ; sh:node <urn:A> . \
             } }",
            )
            .unwrap();
        state
            .store
            .update("INSERT DATA { GRAPH <urn:data> { <urn:x> a <urn:T> } }")
            .unwrap();

        // Run on a blocking thread with a wall-clock bound. If the recursion guard
        // were missing this would overflow the stack and abort the process; with it,
        // validation returns promptly.
        let store = state.store.clone();
        let handle = tokio::task::spawn_blocking(move || {
            crate::shacl::validate(&store, "urn:shapes", &["urn:data".to_string()])
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), handle).await;
        assert!(
            result.is_ok(),
            "cyclic SHACL validation must terminate, not hang/crash"
        );
        assert!(
            result.unwrap().unwrap().is_ok(),
            "validation should return a report, not error out"
        );
    }

    // ─── Token scope (M-8 generalized): read-scoped tokens can't mutate ───────

    /// Mint a read-only API token for `user_id` and return the raw `ots_` value.
    fn read_scoped_token(state: &AppState, user_id: &str) -> String {
        let raw = crate::auth::jwt::generate_api_token();
        let hash = crate::auth::jwt::hash_token(&raw);
        let prefix = &raw[..raw.len().min(11)];
        state
            .auth_db
            .create_api_token(
                "tok-read",
                user_id,
                "test",
                &hash,
                prefix,
                &[ApiScope::Read],
                None,
            )
            .unwrap();
        raw
    }

    #[tokio::test]
    async fn read_scoped_token_cannot_mutate_but_can_read() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "writer", "w@t.com", "h", SystemRole::User)
            .unwrap();
        let tok = read_scoped_token(&state, "u1");

        // A mutating request (create dataset) must be refused for a read-only token.
        let post = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/datasets")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"X","visibility":"private"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            post.status(),
            StatusCode::FORBIDDEN,
            "a read-scoped API token must not be able to mutate (create a dataset)"
        );

        // A read request with the same token still works.
        let get = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            get.status(),
            StatusCode::OK,
            "a read-scoped token must still be able to read"
        );
    }

    // ─── Per-account login lockout ────────────────────────────────────────────

    #[test]
    fn login_lockout_db_logic() {
        let db = AuthDb::in_memory().unwrap();
        for _ in 0..8 {
            db.record_login_failure("victim").unwrap();
        }
        assert!(
            db.is_login_locked("victim").unwrap(),
            "repeated failures must lock the account"
        );
        db.clear_login_attempts("victim").unwrap();
        assert!(
            !db.is_login_locked("victim").unwrap(),
            "a successful login clears the lock"
        );
    }

    #[tokio::test]
    async fn login_blocked_when_account_locked() {
        let state = test_state();
        create_user_with_password(
            &state,
            "u1",
            "victim",
            "correct-horse-battery",
            SystemRole::User,
        );
        // Pre-lock via the DB so this test isn't shaped by the per-IP rate limiter.
        for _ in 0..8 {
            state.auth_db.record_login_failure("victim").unwrap();
        }
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"username":"victim","password":"correct-horse-battery"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "a locked account must be refused even with the correct password"
        );
    }

    // ─── reasoning_materialize requires graph-write on the target ─────────────

    #[tokio::test]
    async fn reasoning_materialize_requires_graph_write() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "user", "u@t.com", "h", SystemRole::User)
            .unwrap();
        // JWT session (write_access=true) but no graph-write grant on the entailment graph.
        let tok = token("u1", "user", "user");
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/reasoning/materialize")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"regime":"rdfs"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::UNAUTHORIZED || resp.status() == StatusCode::FORBIDDEN,
            "a non-admin without a graph-write grant must not materialize into the entailment graph, got {}",
            resp.status()
        );
    }

    // ─── update_me uniqueness → 409 (not a 500 that leaks SQLite text) ────────

    #[tokio::test]
    async fn update_me_duplicate_email_returns_409() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "alice", "alice@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_user("u2", "bob", "bob@t.com", "h", SystemRole::User)
            .unwrap();
        let tok = token("u1", "alice", "user");
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/auth/me")
                    .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"email":"bob@t.com"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        // PUT /me no longer changes the email at all (POST /api/auth/change-email
        // owns that flow, with password confirmation + mailbox-control checks);
        // the original concern stands: no 500 leaking the DB UNIQUE constraint.
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "PUT /me must refuse email changes cleanly (never a 500 leaking the DB constraint)"
        );
    }

    // ─── SHACL Studio pipelines must enforce per-graph write ACL ──────────────
    // A pipeline's inference / derived writes must require graph-write on the
    // targets, mirroring the SPARQL UPDATE path. Without this a low-priv user
    // could materialise/overwrite triples in another tenant's graph (and the
    // scheduler would run it with ambient authority).

    async fn create_pipeline(app: axum::Router, token: &str, body: &str) -> StatusCode {
        app.oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/shacl/pipelines")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    #[tokio::test]
    async fn pipeline_inferred_target_requires_graph_write() {
        let state = test_state();
        state
            .auth_db
            .create_user("u1", "user", "u@t.com", "h", SystemRole::User)
            .unwrap();
        let tok = token("u1", "user", "user"); // JWT session: write_access, but no graph grants

        // run_inference materialises in place + writes a chosen target graph the user can't write.
        let attack = r#"{"name":"evil","visibility":"private","run_inference":true,
            "inferred_target":"new_graph","inferred_target_graph":"urn:victim:secret",
            "graph_iris":["urn:victim:data"]}"#;
        let status = create_pipeline(test_app(state.clone()), &tok, attack).await;
        assert_eq!(status, StatusCode::FORBIDDEN,
            "a non-admin must not create an inference/write pipeline targeting a graph they cannot write");

        // Positive control: a read-only pipeline (no inference, no explicit write target) is fine,
        // so the gate doesn't break the ordinary validate-only case.
        let benign = r#"{"name":"ok","visibility":"private","graph_iris":["urn:my:data"]}"#;
        let status = create_pipeline(test_app(state), &tok, benign).await;
        assert_eq!(
            status,
            StatusCode::CREATED,
            "a read-only pipeline with no write surface must still be allowed"
        );
    }

    // ─── HIGH: bulk-import cross-tenant write/IDOR (per-graph write boundary) ──
    // POST /api/import/bulk gates only on `can_write_dataset(dataset_id)`, but the
    // target graph IRIs are caller-supplied (`default_target_graph`, `targets`,
    // and quad-format embedded graph names). Without a per-graph boundary a
    // principal who owns dataset A can name dataset B's graph (or a `urn:system:*`
    // graph) as the target and, with `replace=true`, overwrite or wipe it. A
    // dataset-scoped import may therefore only write graphs registered to the
    // dataset or under its canonical `{base}/dataset/{id}/` namespace.

    /// Build a `multipart/form-data` body. Each part: (name, content-type,
    /// optional filename, bytes).
    fn multipart_body(boundary: &str, parts: &[(&str, &str, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, content_type, filename, bytes) in parts {
            out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match filename {
                Some(f) => out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\n"
                    )
                    .as_bytes(),
                ),
                None => out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n").as_bytes(),
                ),
            }
            out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
            out.extend_from_slice(bytes);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        out
    }

    async fn bulk_import(
        app: axum::Router,
        token: &str,
        boundary: &str,
        body: Vec<u8>,
    ) -> StatusCode {
        app.oneshot(
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
        .unwrap()
        .status()
    }

    /// A graph owned by dataset B (a second tenant). `base_url` in the test state
    /// is `http://localhost:7878`, so this IRI is NOT under dataset A's namespace.
    const B_GRAPH: &str = "http://localhost:7878/dataset/dsB/secret";

    /// Two non-admin tenants: alice owns dataset A, bob owns dataset B. Both own
    /// `OwnerType::User` datasets, so each can write their own but is a platform
    /// non-admin (so the per-graph boundary applies).
    fn two_tenant_state() -> AppState {
        let state = test_state();
        state
            .auth_db
            .create_user("alice", "alice", "alice@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_user("bob", "bob", "bob@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "dsA",
                "Alice DS",
                None,
                OwnerType::User,
                "alice",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "dsB",
                "Bob DS",
                None,
                OwnerType::User,
                "bob",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
    }

    /// Register and seed a data-bearing graph for dataset B.
    fn seed_b_graph(state: &AppState) {
        state.auth_db.add_dataset_graph("dsB", B_GRAPH).unwrap();
        state
            .store
            .update(&format!(
                "INSERT DATA {{ GRAPH <{B_GRAPH}> {{ <urn:b:s> <urn:b:p> \"bob-secret\" }} }}"
            ))
            .unwrap();
        assert_eq!(state.store.count_graph(Some(B_GRAPH)).unwrap(), 1);
    }

    #[tokio::test]
    async fn bulk_import_cross_tenant_target_graph_rejected() {
        let state = two_tenant_state();
        seed_b_graph(&state);
        let alice = token("alice", "alice", "user");

        // alice owns dataset A (passes can_write_dataset) but names dataset B's
        // graph as the replace target.
        let boundary = "BNDxt1";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"dsA"),
                (
                    "default_target_graph",
                    "text/plain",
                    None,
                    B_GRAPH.as_bytes(),
                ),
                ("replace", "text/plain", None, b"true"),
                (
                    "file",
                    "text/turtle",
                    Some("a.ttl"),
                    b"<urn:x:s> <urn:x:p> <urn:x:o> .",
                ),
            ],
        );
        let status = bulk_import(test_app(state.clone()), &alice, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "owner of dataset A must not replace a graph owned by dataset B"
        );
        // Critically: the rejected import wiped nothing — B's data is intact.
        assert_eq!(
            state.store.count_graph(Some(B_GRAPH)).unwrap(),
            1,
            "B's graph must be untouched after the rejected import"
        );
    }

    #[tokio::test]
    async fn bulk_import_cross_tenant_quad_embedded_graph_rejected() {
        let state = two_tenant_state();
        seed_b_graph(&state);
        let alice = token("alice", "alice", "user");

        // N-Quads carry their own graph name; with merge off it is preserved, so a
        // target-graph-only check would miss it. The boundary runs on the fully
        // resolved graph set, so the embedded graph is still caught.
        let nquads = format!("<urn:x:s> <urn:x:p> <urn:x:o> <{B_GRAPH}> .");
        let boundary = "BNDxt2";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"dsA"),
                ("replace", "text/plain", None, b"true"),
                (
                    "file",
                    "application/n-quads",
                    Some("a.nq"),
                    nquads.as_bytes(),
                ),
            ],
        );
        let status = bulk_import(test_app(state.clone()), &alice, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "a quad-format file must not smuggle another tenant's graph past the boundary"
        );
        assert_eq!(state.store.count_graph(Some(B_GRAPH)).unwrap(), 1);
    }

    #[tokio::test]
    async fn bulk_import_quad_remap_into_namespace_allowed() {
        let state = two_tenant_state();
        let alice = token("alice", "alice", "user");

        // A quad file whose embedded graph is a foreign IRI (not under dsA's
        // namespace) would normally be rejected. With `graph_remap` re-homing it
        // under dsA's own namespace, the write lands there and passes the
        // boundary — this is the happy path the wizard now drives.
        let foreign = "http://foreign.example/g";
        let target = "http://localhost:7878/dataset/dsA/data";
        let nquads = format!("<urn:x:s> <urn:x:p> <urn:x:o> <{foreign}> .");
        let meta = format!(
            r#"{{"dataset_id":"dsA","graph_remap":{{"a.nq":{{"{foreign}":"{target}"}}}}}}"#
        );
        let boundary = "BNDxt7";
        let body = multipart_body(
            boundary,
            &[
                ("meta", "application/json", None, meta.as_bytes()),
                (
                    "file",
                    "application/n-quads",
                    Some("a.nq"),
                    nquads.as_bytes(),
                ),
            ],
        );
        let status = bulk_import(test_app(state.clone()), &alice, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a quad import remapped under the dataset's own namespace must be allowed"
        );
        // Data landed in the remapped (namespaced) graph, not the foreign one.
        assert_eq!(state.store.count_graph(Some(target)).unwrap(), 1);
        assert_eq!(state.store.count_graph(Some(foreign)).unwrap(), 0);
        // …and the remapped graph was registered to dataset A by the handler.
        assert!(
            state
                .auth_db
                .list_dataset_graphs("dsA")
                .unwrap()
                .iter()
                .any(|g| g == target),
            "the remapped graph must be registered to dataset A after import"
        );
    }

    #[tokio::test]
    async fn bulk_import_quad_default_graph_routed_into_namespace() {
        let state = two_tenant_state();
        let alice = token("alice", "alice", "user");

        // A plain N-Quads file whose statement has NO graph label parses into the
        // RDF default graph. Before the fix it bypassed the per-graph boundary
        // entirely and was written to the store's shared global default graph
        // (cross-tenant space). Now a non-admin dataset-scoped import routes it to
        // the dataset's own namespaced default graph, where the boundary admits it
        // and it stays inside the tenant.
        let ns_default = "http://localhost:7878/dataset/dsA/default";
        let nquads = "<urn:x:s> <urn:x:p> <urn:x:o> .";
        let boundary = "BNDxtdg1";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"dsA"),
                (
                    "file",
                    "application/n-quads",
                    Some("a.nq"),
                    nquads.as_bytes(),
                ),
            ],
        );
        let status = bulk_import(test_app(state.clone()), &alice, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a dataset-scoped default-graph quad import is allowed once routed into the namespace"
        );
        // Landed in the dataset's namespaced default graph…
        assert_eq!(state.store.count_graph(Some(ns_default)).unwrap(), 1);
        // …and NEVER in the shared global default graph.
        assert_eq!(
            state.store.count_graph(None).unwrap(),
            0,
            "default-graph triples must never reach the shared global default graph"
        );
        // …and the routed graph was registered to dataset A by the handler.
        assert!(
            state
                .auth_db
                .list_dataset_graphs("dsA")
                .unwrap()
                .iter()
                .any(|g| g == ns_default),
            "the namespaced default graph must be registered to dataset A after import"
        );
    }

    #[tokio::test]
    async fn bulk_import_trig_default_graph_routed_into_namespace() {
        let state = two_tenant_state();
        let alice = token("alice", "alice", "user");

        // TriG triples written outside any `GRAPH {}` block are default-graph triples
        // too; they take the same unnamed-graph arm and must likewise be routed into
        // the dataset namespace rather than the shared global default graph.
        let ns_default = "http://localhost:7878/dataset/dsA/default";
        let trig = "<urn:x:s> <urn:x:p> <urn:x:o> .";
        let boundary = "BNDxtdg2";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"dsA"),
                ("file", "application/trig", Some("a.trig"), trig.as_bytes()),
            ],
        );
        let status = bulk_import(test_app(state.clone()), &alice, boundary, body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(state.store.count_graph(Some(ns_default)).unwrap(), 1);
        assert_eq!(
            state.store.count_graph(None).unwrap(),
            0,
            "TriG default-graph triples must never reach the shared global default graph"
        );
    }

    #[tokio::test]
    async fn bulk_import_quad_remap_to_foreign_graph_rejected() {
        let state = two_tenant_state();
        seed_b_graph(&state);
        let alice = token("alice", "alice", "user");

        // The remap is NOT an escape hatch: pointing it at dataset B's graph is
        // still rejected on the final (remapped) destination, because the
        // boundary runs on the resolved write set after the remap is applied.
        let foreign = "http://foreign.example/g";
        let nquads = format!("<urn:x:s> <urn:x:p> <urn:x:o> <{foreign}> .");
        let meta = format!(
            r#"{{"dataset_id":"dsA","replace":true,"graph_remap":{{"a.nq":{{"{foreign}":"{B_GRAPH}"}}}}}}"#
        );
        let boundary = "BNDxt8";
        let body = multipart_body(
            boundary,
            &[
                ("meta", "application/json", None, meta.as_bytes()),
                (
                    "file",
                    "application/n-quads",
                    Some("a.nq"),
                    nquads.as_bytes(),
                ),
            ],
        );
        let status = bulk_import(test_app(state.clone()), &alice, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "remapping a quad's graph onto another tenant's graph must still be refused"
        );
        assert_eq!(
            state.store.count_graph(Some(B_GRAPH)).unwrap(),
            1,
            "B's graph must be untouched after the rejected remap"
        );
    }

    #[tokio::test]
    async fn bulk_import_system_graph_target_rejected() {
        let state = two_tenant_state();
        let alice = token("alice", "alice", "user");

        // `urn:system:*` graphs are never registered to a dataset and never under a
        // dataset namespace, so a non-admin import can never target one — not even
        // the caller's own dataset metadata graph.
        let boundary = "BNDxt3";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"dsA"),
                (
                    "default_target_graph",
                    "text/plain",
                    None,
                    b"urn:system:metadata:dataset:dsA",
                ),
                ("replace", "text/plain", None, b"true"),
                (
                    "file",
                    "text/turtle",
                    Some("a.ttl"),
                    b"<urn:x:s> <urn:x:p> <urn:x:o> .",
                ),
            ],
        );
        let status = bulk_import(test_app(state), &alice, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "a non-admin must not target a urn:system:* graph via bulk import"
        );
    }

    #[tokio::test]
    async fn bulk_import_into_dataset_namespace_allowed() {
        let state = two_tenant_state();
        let alice = token("alice", "alice", "user");

        // A brand-new graph under dataset A's OWN namespace is allowed even though
        // it is not yet registered (the normal first-import workflow); the boundary
        // admits it structurally and the handler registers it afterwards.
        let target = "http://localhost:7878/dataset/dsA/data";
        let boundary = "BNDxt4";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"dsA"),
                (
                    "default_target_graph",
                    "text/plain",
                    None,
                    target.as_bytes(),
                ),
                (
                    "file",
                    "text/turtle",
                    Some("a.ttl"),
                    b"<urn:x:s> <urn:x:p> <urn:x:o> .",
                ),
            ],
        );
        let status = bulk_import(test_app(state.clone()), &alice, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "an import into the dataset's own IRI namespace must be allowed"
        );
        assert_eq!(state.store.count_graph(Some(target)).unwrap(), 1);
        assert!(
            state
                .auth_db
                .list_dataset_graphs("dsA")
                .unwrap()
                .iter()
                .any(|g| g == target),
            "the namespaced graph must be registered to dataset A after import"
        );
    }

    #[tokio::test]
    async fn bulk_import_into_preregistered_graph_allowed() {
        let state = two_tenant_state();
        let alice = token("alice", "alice", "user");

        // A graph already registered to dataset A is writable even with an IRI
        // outside the dataset namespace (e.g. a legacy or externally-named graph).
        let target = "http://legacy.example/g";
        state.auth_db.add_dataset_graph("dsA", target).unwrap();
        let boundary = "BNDxt5";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"dsA"),
                (
                    "default_target_graph",
                    "text/plain",
                    None,
                    target.as_bytes(),
                ),
                (
                    "file",
                    "text/turtle",
                    Some("a.ttl"),
                    b"<urn:x:s> <urn:x:p> <urn:x:o> .",
                ),
            ],
        );
        let status = bulk_import(test_app(state.clone()), &alice, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "an import into a graph already registered to the dataset must be allowed"
        );
        assert_eq!(state.store.count_graph(Some(target)).unwrap(), 1);
    }

    #[tokio::test]
    async fn bulk_import_register_then_overwrite_bypass_rejected() {
        let state = two_tenant_state();
        seed_b_graph(&state);
        let alice = token("alice", "alice", "user");

        // The bypass: `POST /api/datasets/{id}/graphs` only checks dataset-write,
        // so alice attaches dataset B's graph to her own dataset A. It is now
        // "registered to A" — but the write boundary must still refuse it because
        // it remains owned by dataset B.
        state.auth_db.add_dataset_graph("dsA", B_GRAPH).unwrap();

        let boundary = "BNDxt6";
        let body = multipart_body(
            boundary,
            &[
                ("dataset_id", "text/plain", None, b"dsA"),
                (
                    "default_target_graph",
                    "text/plain",
                    None,
                    B_GRAPH.as_bytes(),
                ),
                ("replace", "text/plain", None, b"true"),
                (
                    "file",
                    "text/turtle",
                    Some("a.ttl"),
                    b"<urn:x:s> <urn:x:p> <urn:x:o> .",
                ),
            ],
        );
        let status = bulk_import(test_app(state.clone()), &alice, boundary, body).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "registering another tenant's graph to your dataset must not unlock writing it"
        );
        assert_eq!(
            state.store.count_graph(Some(B_GRAPH)).unwrap(),
            1,
            "B's graph must be untouched after the rejected bypass attempt"
        );
    }

    // ─── LOW: dataset access-management requires manage (owner/admin), not editor ─
    // grant_access / revoke_access / list_access previously gated on
    // `can_write_dataset`, so a mere editor could grant access to arbitrary users
    // and enumerate the access list. They must require `can_manage_dataset`,
    // matching the role-based grant endpoints.

    #[tokio::test]
    async fn security_dataset_access_management_requires_manage_not_editor() {
        use crate::auth::models::ResourceRole;
        let state = test_state();
        // alice owns dsA; bob holds an EDITOR grant (write, not manage); carol is the grantee.
        state
            .auth_db
            .create_user("alice", "alice", "alice@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_user("bob", "bob", "bob@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_user("carol", "carol", "carol@t.com", "h", SystemRole::User)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "dsA",
                "Alice DS",
                None,
                OwnerType::User,
                "alice",
                Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .set_resource_grant(
                "dataset",
                "dsA",
                "user",
                "bob",
                ResourceRole::Editor,
                "alice",
            )
            .unwrap();

        // Precondition: bob can write dsA but cannot manage it.
        let ds = state.auth_db.get_dataset("dsA").unwrap().unwrap();
        assert!(state.auth_db.can_write_dataset("bob", &ds).unwrap());
        assert!(!state.auth_db.can_manage_dataset("bob", &ds).unwrap());

        let bob = token("bob", "bob", "user");
        let alice = token("alice", "alice", "user");

        let grant_req = |tok: &str| {
            Request::builder()
                .method(Method::POST)
                .uri("/api/datasets/dsA/access")
                .header(header::AUTHORIZATION, format!("Bearer {tok}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"user_id":"carol"}"#))
                .unwrap()
        };

        // Editor bob must NOT be able to grant access…
        let r = test_app(state.clone())
            .oneshot(grant_req(&bob))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::FORBIDDEN,
            "a dataset editor must not be able to grant access"
        );

        // …nor enumerate the access list.
        let r = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/datasets/dsA/access")
                    .header(header::AUTHORIZATION, format!("Bearer {bob}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::FORBIDDEN,
            "a dataset editor must not be able to list the access grants"
        );

        // Positive control: the owner (manage) can still grant access, and it takes effect.
        let r = test_app(state.clone())
            .oneshot(grant_req(&alice))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::CREATED,
            "the dataset owner must still be able to grant access"
        );
        assert!(
            state
                .auth_db
                .list_dataset_access_users("dsA")
                .unwrap()
                .iter()
                .any(|u| u.id == "carol"),
            "carol must have access after the owner's grant"
        );
    }

    // ─── LOW/MEDIUM: cross-tenant authorization denials are audit-logged ──────
    // A denied per-dataset access (403) must leave an audit trail attributed to
    // the caller, so cross-tenant probe attempts are forensically visible. The
    // denial-audit pass lives in the require_auth/optional_auth middleware.

    #[tokio::test]
    async fn security_denied_cross_tenant_access_is_audit_logged() {
        let state = two_tenant_state(); // alice owns dsA (private), bob owns dsB (private)
        let bob = token("bob", "bob", "user");

        // bob holds no role on alice's dataset → managing its access is denied (403).
        let resp = test_app(state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/datasets/dsA/access")
                    .header(header::AUTHORIZATION, format!("Bearer {bob}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"user_id":"bob"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "bob must not be able to manage alice's dataset"
        );

        // The denial is recorded in the append-only audit log, attributed to bob.
        let events = state
            .audit
            .list(100, 0, Some("permission_denied"), Some("bob"), None)
            .unwrap();
        assert!(
            events.iter().any(|e| {
                e.action.as_deref() == Some("POST")
                    && e.resource_id.as_deref() == Some("/api/datasets/dsA/access")
            }),
            "a permission_denied audit event for bob's cross-tenant probe must be recorded; \
             got {} event(s): {:?}",
            events.len(),
            events
        );
    }
}
