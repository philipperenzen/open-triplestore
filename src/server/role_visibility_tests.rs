//! Authorization matrix tests: **every principal kind × every data kind × both
//! visibilities**, asserted at the HTTP layer (router driven via `oneshot`).
//!
//! These exist because a regression shipped where logged-out users could not see
//! *public* triples and the landing-page total count read zero — the previous
//! suite only checked the "all private ⇒ 0" direction, never the "public ⇒ the
//! real count is visible to anonymous" direction. The matrix below pins both.
//!
//! Principals:
//!   * anonymous            — no token
//!   * outsider             — authenticated `user`, member of nothing
//!   * priv_viewer          — `Viewer` of the org that owns the private dataset
//!   * priv_admin           — `Admin`  of that org (can read its private data)
//!   * super_admin          — sees everything in the store
//!
//! Data kinds / display surfaces, each scoped by the caller's access:
//!   GET /api/browse/stats   (total_triples + named_graphs — the landing metrics)
//!   GET /api/browse/graphs  (per-graph counts)
//!   GET /api/browse/triples (triple rows)
//!   GET /sparql             (SELECT COUNT scoping)
//!   GET /store              (Graph Store read)
//!   GET /api/datasets       (dataset listing)
//!   GET /                   (service description void:triples)

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use http_body_util::BodyExt as _;
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    use serde_json::Value;
    use tower::ServiceExt as _;

    use crate::auth::db::AuthDb;
    use crate::auth::jwt::{issue_access_token, JwtConfig};
    use crate::auth::models::{OwnerType, Role, SystemRole, Visibility};
    use crate::prefixes::PrefixRegistry;
    use crate::server::{build_router, AppState};
    use crate::storage::ObjectStore;
    use crate::store::TripleStore;

    const SECRET: &str = "test_secret_must_be_32_chars_abcd";
    const PUB_GRAPH: &str = "https://ex.test/public";
    const PRIV_GRAPH: &str = "https://ex.test/private";
    const PUB_TRIPLES: u64 = 3;
    const PRIV_TRIPLES: u64 = 2;

    fn test_state() -> AppState {
        let auth_db = Arc::new(AuthDb::in_memory().unwrap());
        let audit = Arc::new(crate::auth::audit::AuditLogger::new(auth_db.pool()));
        AppState {
            store: TripleStore::in_memory().unwrap(),
            prefix_registry: Arc::new(PrefixRegistry::empty()),
            auth_db,
            audit,
            backup: None,
            jwt_config: Arc::new(JwtConfig::new(SECRET.to_string(), 30, 30)),
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

    fn token(user_id: &str, role: &str) -> String {
        issue_access_token(
            &JwtConfig::new(SECRET.to_string(), 30, 30),
            user_id,
            user_id,
            role,
        )
        .unwrap()
    }

    fn enc(s: &str) -> String {
        utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
    }

    /// One fixture used by every matrix test: a PUBLIC dataset with 3 triples and a
    /// PRIVATE dataset (owned by `priv-org`) with 2 triples, plus the four
    /// authenticated principals. Returns a router; data lives behind the shared
    /// Arc-backed store so it's visible to every cloned `oneshot`.
    fn fixture() -> axum::Router {
        let state = test_state();
        let db = &state.auth_db;

        db.create_user("super0", "super0", "super0@t", "h", SystemRole::SuperAdmin)
            .unwrap();
        db.create_user("outsider", "outsider", "out@t", "h", SystemRole::User)
            .unwrap();
        db.create_user("pviewer", "pviewer", "pv@t", "h", SystemRole::User)
            .unwrap();
        db.create_user("padmin", "padmin", "pa@t", "h", SystemRole::User)
            .unwrap();

        // Public org/dataset/graph (3 triples).
        db.create_organisation("pub-org", "PubOrg", "pub-org", None, None)
            .unwrap();
        db.create_dataset(
            "pub-ds",
            "Public DS",
            None,
            OwnerType::Organisation,
            "pub-org",
            Visibility::Public,
            None,
        )
        .unwrap();
        db.add_dataset_graph("pub-ds", PUB_GRAPH).unwrap();
        state
            .store
            .update(&format!(
                "INSERT DATA {{ GRAPH <{PUB_GRAPH}> {{ \
                 <s:a> <p:1> <o:a> . <s:b> <p:1> <o:b> . <s:c> <p:1> <o:c> . }} }}"
            ))
            .unwrap();

        // Private org/dataset/graph (2 triples). padmin=org Admin (sees private),
        // pviewer=org Viewer (must NOT see private — see scope_membership_role).
        db.create_organisation("priv-org", "PrivOrg", "priv-org", None, None)
            .unwrap();
        db.create_dataset(
            "priv-ds",
            "Private DS",
            None,
            OwnerType::Organisation,
            "priv-org",
            Visibility::Private,
            None,
        )
        .unwrap();
        db.add_dataset_graph("priv-ds", PRIV_GRAPH).unwrap();
        state
            .store
            .update(&format!(
                "INSERT DATA {{ GRAPH <{PRIV_GRAPH}> {{ \
                 <s:x> <p:1> <o:x> . <s:y> <p:1> <o:y> . }} }}"
            ))
            .unwrap();
        db.add_org_member("padmin", "priv-org", Role::Admin)
            .unwrap();
        db.add_org_member("pviewer", "priv-org", Role::Viewer)
            .unwrap();

        build_router(state, "", vec![])
    }

    async fn get(app: &axum::Router, uri: &str, bearer: Option<&str>) -> (StatusCode, String) {
        let mut b = Request::builder().method(Method::GET).uri(uri);
        if let Some(t) = bearer {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let resp = app
            .clone()
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    fn json(body: &str) -> Value {
        serde_json::from_str(body).unwrap_or_else(|e| panic!("not JSON ({e}):\n{body}"))
    }

    /// Each principal as (label, bearer token or None for anonymous).
    fn principals() -> Vec<(&'static str, Option<String>)> {
        vec![
            ("anonymous", None),
            ("outsider", Some(token("outsider", "user"))),
            ("priv_viewer", Some(token("pviewer", "user"))),
            ("priv_admin", Some(token("padmin", "user"))),
            ("super_admin", Some(token("super0", "super_admin"))),
        ]
    }

    /// True for principals that may read the private dataset.
    fn sees_private(label: &str) -> bool {
        matches!(label, "priv_admin" | "super_admin")
    }

    fn expected_triples(label: &str) -> u64 {
        if sees_private(label) {
            PUB_TRIPLES + PRIV_TRIPLES
        } else {
            PUB_TRIPLES
        }
    }

    fn expected_graphs(label: &str) -> u64 {
        if sees_private(label) {
            2
        } else {
            1
        }
    }

    // ─── browse/stats — the landing-page metrics ──────────────────────────────

    /// The headline regression guard: an ANONYMOUS caller must see the real public
    /// triple count, never zero, whenever public data exists.
    #[tokio::test]
    async fn anonymous_browse_stats_reports_public_triples_not_zero() {
        let app = fixture();
        let (status, body) = get(&app, "/api/browse/stats", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let j = json(&body);
        assert_eq!(
            j["total_triples"], PUB_TRIPLES,
            "anonymous browse/stats must count public triples, not 0:\n{body}"
        );
        assert_eq!(
            j["named_graphs"], 1,
            "anonymous browse/stats must report the public named graph:\n{body}"
        );
    }

    #[tokio::test]
    async fn browse_stats_scopes_to_each_role() {
        let app = fixture();
        for (label, tok) in principals() {
            let (status, body) = get(&app, "/api/browse/stats", tok.as_deref()).await;
            assert_eq!(status, StatusCode::OK, "{label}: {body}");
            let j = json(&body);
            assert_eq!(
                j["total_triples"],
                expected_triples(label),
                "{label}: wrong total_triples\n{body}"
            );
            assert_eq!(
                j["named_graphs"],
                expected_graphs(label),
                "{label}: wrong named_graphs\n{body}"
            );
        }
    }

    // ─── browse/graphs — per-graph counts ─────────────────────────────────────

    #[tokio::test]
    async fn browse_graphs_lists_only_authorized_graphs_with_counts() {
        let app = fixture();
        for (label, tok) in principals() {
            let (status, body) = get(&app, "/api/browse/graphs", tok.as_deref()).await;
            assert_eq!(status, StatusCode::OK, "{label}: {body}");
            assert!(
                body.contains(PUB_GRAPH),
                "{label}: public graph must be listed\n{body}"
            );
            // The public graph's count must be present and correct (caught the
            // "graph listed but count 0" half of the bug).
            let j = json(&body);
            let pub_entry = j
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["iri"] == PUB_GRAPH)
                .unwrap_or_else(|| panic!("{label}: public graph missing\n{body}"));
            assert_eq!(
                pub_entry["count"], PUB_TRIPLES,
                "{label}: public graph count must be {PUB_TRIPLES}\n{body}"
            );
            if sees_private(label) {
                assert!(
                    body.contains(PRIV_GRAPH),
                    "{label}: must see private graph\n{body}"
                );
            } else {
                assert!(
                    !body.contains(PRIV_GRAPH),
                    "{label}: must NOT see private graph\n{body}"
                );
            }
        }
    }

    // ─── browse/triples — triple rows ─────────────────────────────────────────

    #[tokio::test]
    async fn browse_triples_returns_authorized_rows_only() {
        let app = fixture();
        for (label, tok) in principals() {
            let (status, body) = get(&app, "/api/browse/triples?limit=100", tok.as_deref()).await;
            assert_eq!(status, StatusCode::OK, "{label}: {body}");
            assert!(
                body.contains("s:a"),
                "{label}: public subject must appear\n{body}"
            );
            if sees_private(label) {
                assert!(
                    body.contains("s:x"),
                    "{label}: must see private subject\n{body}"
                );
            } else {
                assert!(
                    !body.contains("s:x"),
                    "{label}: must NOT see private subject\n{body}"
                );
            }
        }
    }

    // ─── /sparql — query scoping ──────────────────────────────────────────────

    #[tokio::test]
    async fn sparql_select_count_is_scoped_per_role() {
        let app = fixture();
        let query = enc("SELECT (COUNT(*) AS ?c) WHERE { ?s ?p ?o }");
        for (label, tok) in principals() {
            let uri = format!("/sparql?query={query}");
            let (status, body) = {
                // Ask for JSON results explicitly.
                let mut b = Request::builder()
                    .method(Method::GET)
                    .uri(&uri)
                    .header(header::ACCEPT, "application/sparql-results+json");
                if let Some(t) = &tok {
                    b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
                }
                let resp = app
                    .clone()
                    .oneshot(b.body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                let st = resp.status();
                let bytes = resp.into_body().collect().await.unwrap().to_bytes();
                (st, String::from_utf8_lossy(&bytes).into_owned())
            };
            assert_eq!(status, StatusCode::OK, "{label}: {body}");
            let j = json(&body);
            let count: u64 = j["results"]["bindings"][0]["c"]["value"]
                .as_str()
                .unwrap_or("?")
                .parse()
                .unwrap_or_else(|_| panic!("{label}: no count in\n{body}"));
            assert_eq!(
                count,
                expected_triples(label),
                "{label}: SPARQL count must be scoped\n{body}"
            );
        }
    }

    // ─── /store — Graph Store read ────────────────────────────────────────────

    #[tokio::test]
    async fn graph_store_read_enforces_graph_visibility() {
        let app = fixture();
        for (label, tok) in principals() {
            // Public graph: everyone gets the data.
            let (status, body) = get(
                &app,
                &format!("/store?graph={}", enc(PUB_GRAPH)),
                tok.as_deref(),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{label}: public GSP read\n{body}");
            assert!(
                body.contains("s:a"),
                "{label}: public GSP must return data\n{body}"
            );

            // Private graph: only the authorized principals.
            let (pstatus, pbody) = get(
                &app,
                &format!("/store?graph={}", enc(PRIV_GRAPH)),
                tok.as_deref(),
            )
            .await;
            if sees_private(label) {
                assert_eq!(pstatus, StatusCode::OK, "{label}: private GSP\n{pbody}");
                assert!(pbody.contains("s:x"), "{label}: private GSP data\n{pbody}");
            } else {
                assert!(
                    pstatus == StatusCode::UNAUTHORIZED || pstatus == StatusCode::FORBIDDEN,
                    "{label}: private GSP read must be denied, got {pstatus}\n{pbody}"
                );
            }
        }
    }

    // ─── /api/datasets — dataset listing ──────────────────────────────────────

    #[tokio::test]
    async fn dataset_listing_is_filtered_by_access() {
        let app = fixture();
        for (label, tok) in principals() {
            let (status, body) = get(&app, "/api/datasets", tok.as_deref()).await;
            assert_eq!(status, StatusCode::OK, "{label}: {body}");
            let ids: Vec<String> = json(&body)
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d["id"].as_str().unwrap_or("").to_string())
                .collect();
            assert!(
                ids.iter().any(|id| id == "pub-ds"),
                "{label}: public dataset must be listed: {ids:?}"
            );
            assert_eq!(
                ids.iter().any(|id| id == "priv-ds"),
                sees_private(label),
                "{label}: private dataset visibility wrong: {ids:?}"
            );
        }
    }

    // ─── GET / — service description ──────────────────────────────────────────

    #[tokio::test]
    async fn service_description_hides_private_graph_from_unauthorized() {
        let app = fixture();
        for (label, tok) in principals() {
            let (status, body) = get(&app, "/", tok.as_deref()).await;
            assert_eq!(status, StatusCode::OK, "{label}: {body}");
            assert!(
                body.contains(PUB_GRAPH),
                "{label}: public graph must appear in service description\n{body}"
            );
            if !sees_private(label) {
                assert!(
                    !body.contains(PRIV_GRAPH),
                    "{label}: private graph must be hidden\n{body}"
                );
            }
        }
    }

    // ─── Integration: the bundled demo seed must be visible to anonymous ──────

    /// Drives the real demo seeder, then asserts an anonymous caller sees the public
    /// demo triples and a non-zero landing count. This is the end-to-end guard for
    /// the original report ("logged out ⇒ no public triples, count zero"): if the
    /// seed registers public graphs but fails to populate them, this fails.
    #[tokio::test]
    async fn demo_seed_is_visible_to_anonymous() {
        let state = test_state();
        // The seeder needs an admin to own the demo content.
        state
            .auth_db
            .create_user(
                "seed-admin",
                "seed-admin",
                "sa@t",
                "h",
                SystemRole::SuperAdmin,
            )
            .unwrap();

        // Unit tests must never reach the network: an empty SEED_IFC_URL makes the
        // seeder skip the Schependomlaan IFC download (the static demo graphs still
        // seed). Run on the blocking pool exactly like production boot does — the
        // seeder uses block_on internally, which panics on an async worker.
        std::env::set_var("SEED_IFC_URL", "");
        let seed_state = state.clone();
        tokio::task::spawn_blocking(move || {
            crate::saved_queries::seed::seed_open_triplestore(&seed_state)
        })
        .await
        .unwrap();

        // If the demo seed was disabled (SEED_STANDARDS_DEMO=off), there is nothing
        // to assert — don't manufacture a false failure.
        let datasets = state.auth_db.list_datasets().unwrap();
        if datasets.is_empty() {
            return;
        }
        assert!(
            datasets.iter().all(|d| d.visibility == Visibility::Public),
            "all bundled demo datasets are expected to be public"
        );

        let app = build_router(state, "", vec![]);
        let (status, body) = get(&app, "/api/browse/stats", None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let j = json(&body);
        let total = j["total_triples"].as_u64().unwrap_or(0);
        let graphs = j["named_graphs"].as_u64().unwrap_or(0);
        assert!(
            total > 0,
            "anonymous landing count over the seeded public demo must be > 0, got {total}\n{body}"
        );
        assert!(
            graphs >= 1,
            "anonymous must see at least one seeded public graph, got {graphs}\n{body}"
        );
    }
}
