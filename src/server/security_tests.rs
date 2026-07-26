/// Security integration tests for the HTTP API.
///
/// Each test builds a fully in-memory AppState (no disk I/O) and drives the
/// router via `tower::ServiceExt::oneshot`, so no real network port is bound.
///
/// Coverage:
///  - Service description hides private graph IRIs and triple counts
///  - SPARQL UPDATE requires authentication
///  - Graph Store writes (PUT/POST/DELETE) require authentication
///  - SPARQL SELECT is scoped to accessible graphs only
///  - Browse APIs filter to accessible graphs
///  - IRI injection in browse params is rejected with 400
///  - CSP header is present on all responses
///  - Internal error details are not leaked to clients
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt as _;
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    use tower::ServiceExt as _;

    use crate::auth::db::AuthDb;
    use crate::auth::jwt::{issue_access_token, JwtConfig};
    use crate::auth::models::{OwnerType, SystemRole, Visibility};
    use crate::prefixes::PrefixRegistry;
    use crate::server::{build_router, AppState};
    use crate::storage::ObjectStore;
    use crate::store::TripleStore;

    // ─── Shared test infrastructure ───────────────────────────────────────────

    const TEST_JWT_SECRET: &str = "test_secret_must_be_32_chars_abcd";

    fn test_state() -> AppState {
        let auth_db = Arc::new(AuthDb::in_memory().unwrap());
        let audit = Arc::new(crate::auth::audit::AuditLogger::new(auth_db.pool()));
        let oidc_provider =
            crate::auth::oidc_provider::ProviderKeys::load_or_generate(&auth_db, TEST_JWT_SECRET)
                .ok()
                .map(Arc::new);
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
            oidc_provider,
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

    fn test_app(state: AppState) -> axum::Router {
        build_router(state, "", vec![])
    }

    async fn body_text(body: Body) -> String {
        let bytes = body.collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// Issue a JWT access token directly (bypasses the login endpoint).
    fn admin_token(state: &AppState) -> String {
        state
            .auth_db
            .create_user(
                "admin0",
                "admin",
                "admin@test.com",
                "hash",
                SystemRole::SuperAdmin,
            )
            .unwrap();
        issue_access_token(
            &JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30),
            "admin0",
            "admin",
            "super_admin",
        )
        .unwrap()
    }

    fn url_encode(s: &str) -> String {
        utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
    }

    // ─── Service Description ──────────────────────────────────────────────────

    /// Unauthenticated GET / must not reveal private graph IRIs or real triple counts.
    #[tokio::test]
    async fn test_service_description_hides_private_graph() {
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
            .add_dataset_graph("d1", "https://private.example.com/graph")
            .unwrap();
        state
            .store
            .update(
                "INSERT DATA { GRAPH <https://private.example.com/graph> { <s:s> <p:p> <o:o> } }",
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            !body.contains("private.example.com"),
            "Private graph IRI must not appear in unauthenticated service description:\n{body}"
        );
        assert!(
            body.contains("void:triples 0"),
            "Unauthenticated service description must report 0 triples:\n{body}"
        );
    }

    /// Authenticated admin GET / must show all graphs and real triple counts.
    #[tokio::test]
    async fn test_service_description_admin_sees_all() {
        let state = test_state();
        let token = admin_token(&state);
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
            .add_dataset_graph("d1", "https://private.example.com/graph")
            .unwrap();
        state
            .store
            .update(
                "INSERT DATA { GRAPH <https://private.example.com/graph> { <s:s> <p:p> <o:o> } }",
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("private.example.com"),
            "Admin must see private graphs in service description:\n{body}"
        );
        // The named graph holds exactly one triple, which must surface as a
        // per-graph void:triples count (the default graph is legitimately empty).
        assert!(
            body.contains("void:triples 1"),
            "Admin service description must report the real per-graph triple count:\n{body}"
        );
    }

    /// Public graphs must appear in the unauthenticated service description.
    #[tokio::test]
    async fn test_service_description_shows_public_graph() {
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
            .add_dataset_graph("d1", "https://public.example.com/graph")
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("public.example.com"),
            "Public graph must appear in unauthenticated service description:\n{body}"
        );
    }

    // ─── SPARQL UPDATE requires auth ──────────────────────────────────────────

    #[tokio::test]
    async fn test_sparql_update_direct_requires_auth() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sparql")
                    .header(header::CONTENT_TYPE, "application/sparql-update")
                    .body(Body::from("INSERT DATA { <s:a> <p:b> <o:c> }"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "SPARQL UPDATE without auth must be 401"
        );
    }

    #[tokio::test]
    async fn test_sparql_update_via_form_requires_auth() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/sparql")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "update=INSERT+DATA+%7B+%3Cs%3Aa%3E+%3Cp%3Ab%3E+%3Co%3Ac%3E+%7D",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "SPARQL UPDATE via form without auth must be 401"
        );
    }

    // ─── Graph Store write methods require auth ───────────────────────────────

    #[tokio::test]
    async fn test_graph_store_put_requires_auth() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/store?graph=https%3A%2F%2Fexample.org%2Ftest")
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .body(Body::from("<s:a> <p:b> <o:c> ."))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Graph Store PUT without auth must be 401"
        );
    }

    #[tokio::test]
    async fn test_graph_store_post_requires_auth() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/store?graph=https%3A%2F%2Fexample.org%2Ftest")
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .body(Body::from("<s:a> <p:b> <o:c> ."))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Graph Store POST without auth must be 401"
        );
    }

    #[tokio::test]
    async fn test_graph_store_delete_requires_auth() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/store?graph=https%3A%2F%2Fexample.org%2Ftest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Graph Store DELETE without auth must be 401"
        );
    }

    // ─── SPARQL read scoping ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_sparql_query_cannot_read_private_graph() {
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
            .add_dataset_graph("d1", "https://secret.example.com/graph")
            .unwrap();
        state.store
            .update("INSERT DATA { GRAPH <https://secret.example.com/graph> { <s:secret> <p:val> \"top secret\" } }")
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
            !body.contains("top secret"),
            "Unauthenticated SPARQL must not return data from private graphs:\n{body}"
        );
        assert!(
            !body.contains("secret.example.com"),
            "Private graph IRI must not appear in unauthenticated SPARQL results:\n{body}"
        );
    }

    #[tokio::test]
    async fn test_sparql_query_can_read_public_graph() {
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
            .add_dataset_graph("d1", "https://public.example.com/graph")
            .unwrap();
        state.store
            .update("INSERT DATA { GRAPH <https://public.example.com/graph> { <s:pub> <p:val> \"open data\" } }")
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
            body.contains("open data"),
            "Unauthenticated SPARQL must be able to read public graph data:\n{body}"
        );
    }

    // ─── Browse API scoping ───────────────────────────────────────────────────

    #[tokio::test]
    async fn test_browse_graphs_hides_private() {
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
            .add_dataset_graph("d1", "https://private.example.com/graph")
            .unwrap();
        state
            .store
            .update(
                "INSERT DATA { GRAPH <https://private.example.com/graph> { <s:s> <p:p> <o:o> } }",
            )
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
            !body.contains("private.example.com"),
            "Browse graphs must not expose private graph IRIs without auth:\n{body}"
        );
    }

    #[tokio::test]
    async fn test_browse_stats_hides_private_count() {
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
            .add_dataset_graph("d1", "https://private.example.com/graph")
            .unwrap();
        state
            .store
            .update(
                "INSERT DATA { GRAPH <https://private.example.com/graph> { <s:s> <p:p> <o:o> } }",
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
        let body = body_text(resp.into_body()).await;
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["total_triples"], 0,
            "Unauthenticated browse/stats must report 0 triples when all graphs are private:\n{body}");
        assert_eq!(
            json["named_graphs"], 0,
            "Unauthenticated browse/stats must report 0 named graphs when all are private:\n{body}"
        );
    }

    // ─── IRI injection ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_browse_triples_rejects_iri_injection() {
        // The subject param contains '>' which would break out of SPARQL angle-bracket literal
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
    async fn test_browse_resource_rejects_injection() {
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

    // ─── Security headers ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_csp_header_on_all_responses() {
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

    // ─── Error message hardening ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_internal_error_message_not_leaked() {
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
            "Invalid SPARQL must return a 4xx, got {}",
            resp.status()
        );
        let body = body_text(resp.into_body()).await;
        assert!(
            !body.to_lowercase().contains("oxigraph"),
            "Internal library name must not appear in error response:\n{body}"
        );
    }

    // ─── Endpoint ACL ─────────────────────────────────────────────────────────

    /// A deny rule targeting a specific user should block that user's request.
    #[tokio::test]
    async fn test_endpoint_acl_deny_blocks_user() {
        let state = test_state();
        // Create a regular user
        state
            .auth_db
            .create_user(
                "u1",
                "bob",
                "bob@test.com",
                "hash",
                crate::auth::models::SystemRole::User,
            )
            .unwrap();
        let token = issue_access_token(
            &JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30),
            "u1",
            "bob",
            "user",
        )
        .unwrap();

        // Create an admin to insert the rule
        state
            .auth_db
            .create_user(
                "adm",
                "admin2",
                "adm@test.com",
                "hash",
                crate::auth::models::SystemRole::Admin,
            )
            .unwrap();

        // Insert a deny rule for user u1 on /api/browse/*
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

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/graphs")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "Endpoint ACL deny rule must block the targeted user"
        );
    }

    /// An allow rule for a user should not affect other users (no rule → default allow).
    #[tokio::test]
    async fn test_endpoint_acl_allow_no_effect_on_others() {
        let state = test_state();
        state
            .auth_db
            .create_user(
                "u1",
                "alice2",
                "alice2@test.com",
                "hash",
                crate::auth::models::SystemRole::User,
            )
            .unwrap();
        state
            .auth_db
            .create_user(
                "u2",
                "carol",
                "carol@test.com",
                "hash",
                crate::auth::models::SystemRole::User,
            )
            .unwrap();
        state
            .auth_db
            .create_user(
                "adm",
                "admin3",
                "adm3@test.com",
                "hash",
                crate::auth::models::SystemRole::Admin,
            )
            .unwrap();

        let carol_token = issue_access_token(
            &JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30),
            "u2",
            "carol",
            "user",
        )
        .unwrap();

        // Deny rule only for u1 — carol is unaffected
        state
            .auth_db
            .create_endpoint_acl_rule(
                "rule2",
                "user",
                "u1",
                "/api/browse/**",
                "*",
                "deny",
                10,
                "adm",
            )
            .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/browse/stats")
                    .header(header::AUTHORIZATION, format!("Bearer {carol_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "Other users must not be affected by deny rules targeting a different user"
        );
    }

    // ─── Graph ACL ────────────────────────────────────────────────────────────

    /// A user with a graph_acl read grant can see data in a private dataset's graph.
    #[tokio::test]
    async fn test_graph_acl_grants_read_access() {
        let state = test_state();
        state
            .auth_db
            .create_user(
                "u1",
                "dave",
                "dave@test.com",
                "hash",
                crate::auth::models::SystemRole::User,
            )
            .unwrap();
        state
            .auth_db
            .create_user(
                "adm",
                "admin4",
                "adm4@test.com",
                "hash",
                crate::auth::models::SystemRole::Admin,
            )
            .unwrap();
        state
            .auth_db
            .create_organisation("o1", "Acme4", "acme4", None, None)
            .unwrap();
        state
            .auth_db
            .create_dataset(
                "d1",
                "Private",
                None,
                crate::auth::models::OwnerType::Organisation,
                "o1",
                crate::auth::models::Visibility::Private,
                None,
            )
            .unwrap();
        state
            .auth_db
            .add_dataset_graph("d1", "https://secret.example.com/restricted")
            .unwrap();
        state.store
            .update("INSERT DATA { GRAPH <https://secret.example.com/restricted> { <s:x> <p:y> \"graph-acl-value\" } }")
            .unwrap();

        // Grant read access to u1 via graph_acl
        state
            .auth_db
            .grant_graph_permission(
                "gacl1",
                "https://secret.example.com/restricted",
                "user",
                "u1",
                "read",
                "adm",
            )
            .unwrap();

        let user_token = issue_access_token(
            &JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30),
            "u1",
            "dave",
            "user",
        )
        .unwrap();

        let query = "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }";
        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/sparql?query={}", url_encode(query)))
                    .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                    .header(header::ACCEPT, "application/sparql-results+json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert!(
            body.contains("graph-acl-value"),
            "User with graph_acl read grant must see the data:\n{body}"
        );
    }

    /// Graph Store write is denied when the user has only read access via graph_acl.
    #[tokio::test]
    async fn test_graph_acl_read_only_blocks_write() {
        let state = test_state();
        state
            .auth_db
            .create_user(
                "u1",
                "eve",
                "eve@test.com",
                "hash",
                crate::auth::models::SystemRole::User,
            )
            .unwrap();
        state
            .auth_db
            .create_user(
                "adm",
                "admin5",
                "adm5@test.com",
                "hash",
                crate::auth::models::SystemRole::Admin,
            )
            .unwrap();

        // Grant only read — not write
        state
            .auth_db
            .grant_graph_permission(
                "gacl2",
                "https://target.example.com/graph",
                "user",
                "u1",
                "read",
                "adm",
            )
            .unwrap();

        let user_token = issue_access_token(
            &JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30),
            "u1",
            "eve",
            "user",
        )
        .unwrap();

        let resp = test_app(state)
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/store?graph=https%3A%2F%2Ftarget.example.com%2Fgraph")
                    .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
                    .header(header::CONTENT_TYPE, "text/turtle")
                    .body(Body::from("<s:a> <p:b> <o:c> ."))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "Read-only graph_acl grant must not allow Graph Store writes"
        );
    }

    // ─── OAuth provider listing (public) ──────────────────────────────────────

    /// GET /api/auth/oauth/providers returns an empty list when none are configured.
    #[tokio::test]
    async fn test_oauth_providers_empty_when_none_configured() {
        let resp = test_app(test_state())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/auth/oauth/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp.into_body()).await;
        assert_eq!(
            body.trim(),
            "[]",
            "No SSO providers configured → empty list"
        );
    }

    // ─── ACL admin endpoints require admin auth ────────────────────────────────

    #[tokio::test]
    async fn test_acl_endpoints_require_admin() {
        let state = test_state();
        state
            .auth_db
            .create_user(
                "u1",
                "frank",
                "frank@test.com",
                "hash",
                crate::auth::models::SystemRole::User,
            )
            .unwrap();
        let user_token = issue_access_token(
            &JwtConfig::new(TEST_JWT_SECRET.to_string(), 30, 30),
            "u1",
            "frank",
            "user",
        )
        .unwrap();

        for path in [
            "/api/admin/acl/endpoints",
            "/api/admin/acl/graphs",
            "/api/admin/acl/triples",
        ] {
            let resp = test_app(state.clone())
                .oneshot(
                    Request::builder()
                        .method(Method::GET)
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {user_token}"))
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
