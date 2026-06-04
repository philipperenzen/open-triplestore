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
    use crate::auth::models::{ApiScope, SystemRole};
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
            base_url: Arc::new("http://localhost:7878".to_string()),
            oauth_sessions: crate::auth::oauth::new_session_store(),
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
    fn create_user_with_password(state: &AppState, id: &str, username: &str, password: &str, role: SystemRole) {
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
            .oneshot(Request::builder().method(Method::GET).uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let h = resp.headers();
        assert_eq!(h.get("x-frame-options").map(|v| v.to_str().unwrap()), Some("DENY"));
        assert_eq!(h.get("x-content-type-options").map(|v| v.to_str().unwrap()), Some("nosniff"));
        assert!(h.get("referrer-policy").is_some(), "Referrer-Policy must be set");
        let csp = h.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'"), "CSP must forbid framing: {csp}");
        assert!(!csp.contains("script-src 'unsafe-inline'"), "script-src must not allow unsafe-inline");
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
        assert_ne!(acao.as_deref(), Some("https://evil.example.com"),
            "an unconfigured origin must never be reflected in Access-Control-Allow-Origin");
    }

    // ─── Finding 1: variable-graph UPDATE is admin-only ───────────────────────

    #[tokio::test]
    async fn update_variable_graph_target_requires_admin() {
        let state = test_state();
        // Non-admin with a write grant on ONE specific graph.
        state.auth_db.create_user("u1", "writer", "w@t.com", "h", SystemRole::User).unwrap();
        state.auth_db.create_user("adm", "admin", "a@t.com", "h", SystemRole::Admin).unwrap();
        state.auth_db
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
        state.auth_db.create_user("u1", "writer", "w@t.com", "h", SystemRole::User).unwrap();
        state.auth_db.create_user("adm", "admin", "a@t.com", "h", SystemRole::Admin).unwrap();
        // Writer can WRITE urn:mine but has no READ grant on urn:secret.
        state.auth_db
            .grant_graph_permission("g1", "urn:mine", "user", "u1", "write", "adm")
            .unwrap();
        state.store
            .update("INSERT DATA { GRAPH <urn:secret> { <s:x> <p:y> \"top-secret\" } }")
            .unwrap();
        let user = token("u1", "writer", "user");

        // Copy data out of a graph the writer cannot read into one they can.
        let exfil = "INSERT { GRAPH <urn:mine> { ?s ?p ?o } } WHERE { GRAPH <urn:secret> { ?s ?p ?o } }";
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
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST,
            "a backslash in the suggest prefix must be rejected (not corrupt the SPARQL)");
    }

    // ─── Finding 10: login gives a uniform response (no enumeration) ──────────

    #[tokio::test]
    async fn login_uniform_response_unknown_vs_deactivated() {
        let state = test_state();
        create_user_with_password(&state, "u1", "realuser", "correct-horse-battery", SystemRole::User);
        state.auth_db.set_user_active("u1", false).unwrap(); // deactivate

        let login = |username: &str, password: &str| {
            Request::builder()
                .method(Method::POST)
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"username":"{username}","password":"{password}"}}"#)))
                .unwrap()
        };

        // Unknown user.
        let r1 = test_app(state.clone()).oneshot(login("ghost", "whatever")).await.unwrap();
        let s1 = r1.status();
        let b1 = body_text(r1.into_body()).await;

        // Existing-but-deactivated user with the CORRECT password.
        let r2 = test_app(state).oneshot(login("realuser", "correct-horse-battery")).await.unwrap();
        let s2 = r2.status();
        let b2 = body_text(r2.into_body()).await;

        assert_eq!(s1, StatusCode::UNAUTHORIZED);
        assert_eq!(s2, StatusCode::UNAUTHORIZED);
        assert_eq!(b1, b2, "unknown-user and deactivated-user responses must be identical (no enumeration)");
        assert!(!b2.to_lowercase().contains("deactivat"),
            "the deactivated reason must not leak in the response body");
    }

    // ─── Finding 5: legacy delete_user enforces the role hierarchy ────────────

    #[tokio::test]
    async fn legacy_delete_user_cannot_delete_higher_role() {
        let state = test_state();
        state.auth_db.create_user("adm", "admin", "a@t.com", "h", SystemRole::Admin).unwrap();
        state.auth_db.create_user("root", "root", "r@t.com", "h", SystemRole::SuperAdmin).unwrap();
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
        assert_eq!(resp.status(), StatusCode::FORBIDDEN,
            "an admin must not be able to delete a super_admin");
        assert!(state.auth_db.get_user_by_id("root").unwrap().is_some(),
            "the super_admin account must still exist");
    }

    // ─── Finding 12: the last super_admin cannot be demoted ───────────────────

    #[tokio::test]
    async fn cannot_demote_last_super_admin() {
        let state = test_state();
        state.auth_db.create_user("root", "root", "r@t.com", "h", SystemRole::SuperAdmin).unwrap();
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
        assert_eq!(resp.status(), StatusCode::CONFLICT,
            "demoting the last active super_admin must be refused");
        assert_eq!(state.auth_db.get_user_by_id("root").unwrap().unwrap().role, SystemRole::SuperAdmin);
    }

    #[test]
    fn count_active_super_admins_counts_only_active() {
        let db = AuthDb::in_memory().unwrap();
        db.create_user("r1", "root1", "r1@t.com", "h", SystemRole::SuperAdmin).unwrap();
        db.create_user("r2", "root2", "r2@t.com", "h", SystemRole::SuperAdmin).unwrap();
        db.create_user("u1", "user1", "u1@t.com", "h", SystemRole::User).unwrap();
        assert_eq!(db.count_active_super_admins().unwrap(), 2);
        db.set_user_active("r2", false).unwrap();
        assert_eq!(db.count_active_super_admins().unwrap(), 1);
    }

    // ─── Finding 4: cyclic SHACL shapes terminate (no stack-overflow crash) ───

    #[tokio::test]
    async fn shacl_cyclic_node_reference_terminates() {
        let state = test_state();
        // Two shapes that reference each other via sh:node, plus one target instance.
        state.store.update(
            "PREFIX sh: <http://www.w3.org/ns/shacl#> \
             INSERT DATA { GRAPH <urn:shapes> { \
               <urn:A> a sh:NodeShape ; sh:targetClass <urn:T> ; sh:node <urn:B> . \
               <urn:B> a sh:NodeShape ; sh:node <urn:A> . \
             } }",
        ).unwrap();
        state.store.update(
            "INSERT DATA { GRAPH <urn:data> { <urn:x> a <urn:T> } }",
        ).unwrap();

        // Run on a blocking thread with a wall-clock bound. If the recursion guard
        // were missing this would overflow the stack and abort the process; with it,
        // validation returns promptly.
        let store = state.store.clone();
        let handle = tokio::task::spawn_blocking(move || {
            crate::shacl::validate(&store, "urn:shapes", &["urn:data".to_string()])
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), handle).await;
        assert!(result.is_ok(), "cyclic SHACL validation must terminate, not hang/crash");
        assert!(result.unwrap().unwrap().is_ok(), "validation should return a report, not error out");
    }

    // ─── Token scope (M-8 generalized): read-scoped tokens can't mutate ───────

    /// Mint a read-only API token for `user_id` and return the raw `ots_` value.
    fn read_scoped_token(state: &AppState, user_id: &str) -> String {
        let raw = crate::auth::jwt::generate_api_token();
        let hash = crate::auth::jwt::hash_token(&raw);
        let prefix = &raw[..raw.len().min(11)];
        state
            .auth_db
            .create_api_token("tok-read", user_id, "test", &hash, prefix, &[ApiScope::Read], None)
            .unwrap();
        raw
    }

    #[tokio::test]
    async fn read_scoped_token_cannot_mutate_but_can_read() {
        let state = test_state();
        state.auth_db.create_user("u1", "writer", "w@t.com", "h", SystemRole::User).unwrap();
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
        assert_eq!(post.status(), StatusCode::FORBIDDEN,
            "a read-scoped API token must not be able to mutate (create a dataset)");

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
        assert_eq!(get.status(), StatusCode::OK, "a read-scoped token must still be able to read");
    }

    // ─── Per-account login lockout ────────────────────────────────────────────

    #[test]
    fn login_lockout_db_logic() {
        let db = AuthDb::in_memory().unwrap();
        for _ in 0..8 {
            db.record_login_failure("victim").unwrap();
        }
        assert!(db.is_login_locked("victim").unwrap(), "repeated failures must lock the account");
        db.clear_login_attempts("victim").unwrap();
        assert!(!db.is_login_locked("victim").unwrap(), "a successful login clears the lock");
    }

    #[tokio::test]
    async fn login_blocked_when_account_locked() {
        let state = test_state();
        create_user_with_password(&state, "u1", "victim", "correct-horse-battery", SystemRole::User);
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
                    .body(Body::from(r#"{"username":"victim","password":"correct-horse-battery"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED,
            "a locked account must be refused even with the correct password");
    }

    // ─── reasoning_materialize requires graph-write on the target ─────────────

    #[tokio::test]
    async fn reasoning_materialize_requires_graph_write() {
        let state = test_state();
        state.auth_db.create_user("u1", "user", "u@t.com", "h", SystemRole::User).unwrap();
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
        state.auth_db.create_user("u1", "alice", "alice@t.com", "h", SystemRole::User).unwrap();
        state.auth_db.create_user("u2", "bob", "bob@t.com", "h", SystemRole::User).unwrap();
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
        assert_eq!(resp.status(), StatusCode::CONFLICT,
            "updating to another user's email must return 409, not a 500 leaking the DB constraint");
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
        state.auth_db.create_user("u1", "user", "u@t.com", "h", SystemRole::User).unwrap();
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
        assert_eq!(status, StatusCode::CREATED,
            "a read-only pipeline with no write surface must still be allowed");
    }
}
