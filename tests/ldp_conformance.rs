//! LDP 1.0 conformance tests.
//!
//! Covers store-level container operations (container.rs) and HTTP handler
//! behaviour (handler.rs) for Basic, Direct, and Indirect Containers, Non-RDF
//! Sources, PATCH, Prefer header, and Constrained-By Link header.

#![cfg(feature = "ldp")]

use open_triplestore::ldp::container;
use open_triplestore::ldp::container::ContainerType;
use open_triplestore::store::TripleStore;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn fresh() -> TripleStore {
    TripleStore::in_memory().unwrap()
}

fn ask(store: &TripleStore, sparql: &str) -> bool {
    matches!(
        store.query(sparql).unwrap(),
        oxigraph::sparql::QueryResults::Boolean(true)
    )
}

// ═══════════════════════════════════════════════════════════
// ETag
// ═══════════════════════════════════════════════════════════

#[test]
fn etag_deterministic() {
    let e1 = container::compute_etag(b"hello");
    let e2 = container::compute_etag(b"hello");
    assert_eq!(e1, e2);
    assert!(e1.starts_with('"'));
    assert!(e1.ends_with('"'));
}

#[test]
fn etag_different_for_different_content() {
    assert_ne!(container::compute_etag(b"a"), container::compute_etag(b"b"));
}

// ═══════════════════════════════════════════════════════════
// Basic Container — store level
// ═══════════════════════════════════════════════════════════

#[test]
fn basic_container_lifecycle() {
    let store = fresh();
    container::ensure_container(&store, "http://ex.org/c/").unwrap();
    container::add_member(&store, "http://ex.org/c/", "http://ex.org/c/item1").unwrap();
    container::add_member(&store, "http://ex.org/c/", "http://ex.org/c/item2").unwrap();
    let members = container::list_members(&store, "http://ex.org/c/", 0, 100).unwrap();
    assert_eq!(members.len(), 2);
    container::remove_member(&store, "http://ex.org/c/", "http://ex.org/c/item1").unwrap();
    let members = container::list_members(&store, "http://ex.org/c/", 0, 100).unwrap();
    assert_eq!(members.len(), 1);
}

#[test]
fn get_container_type_basic() {
    let store = fresh();
    container::ensure_container(&store, "http://ex.org/bc/").unwrap();
    assert_eq!(
        container::get_container_type(&store, "http://ex.org/bc/"),
        ContainerType::Basic
    );
}

#[test]
fn count_members_empty() {
    let store = fresh();
    container::ensure_container(&store, "http://ex.org/empty/").unwrap();
    assert_eq!(
        container::count_members(&store, "http://ex.org/empty/").unwrap(),
        0
    );
}

#[test]
fn list_members_pagination() {
    let store = fresh();
    container::ensure_container(&store, "http://ex.org/pg/").unwrap();
    for i in 1..=5 {
        container::add_member(
            &store,
            "http://ex.org/pg/",
            &format!("http://ex.org/pg/i{i}"),
        )
        .unwrap();
    }
    assert_eq!(
        container::list_members(&store, "http://ex.org/pg/", 0, 2)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        container::list_members(&store, "http://ex.org/pg/", 2, 2)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        container::list_members(&store, "http://ex.org/pg/", 4, 2)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn resource_exists_check() {
    let store = fresh();
    assert!(!container::resource_exists(&store, "http://ex.org/missing"));
    container::ensure_container(&store, "http://ex.org/missing").unwrap();
    assert!(container::resource_exists(&store, "http://ex.org/missing"));
}

// ═══════════════════════════════════════════════════════════
// Direct Container — store level
// ═══════════════════════════════════════════════════════════

#[test]
fn get_container_type_direct() {
    let store = fresh();
    container::ensure_direct_container(
        &store,
        "http://ex.org/dc/",
        "http://ex.org/res",
        container::LDP_MEMBER,
        None,
    )
    .unwrap();
    assert_eq!(
        container::get_container_type(&store, "http://ex.org/dc/"),
        ContainerType::Direct
    );
}

#[test]
fn get_membership_info_direct() {
    let store = fresh();
    container::ensure_direct_container(
        &store,
        "http://ex.org/dc/",
        "http://ex.org/res",
        "http://ex.org/hasItem",
        None,
    )
    .unwrap();
    let info = container::get_membership_info(&store, "http://ex.org/dc/")
        .unwrap()
        .unwrap();
    assert_eq!(info.membership_resource, "http://ex.org/res");
    assert_eq!(info.has_member_relation, "http://ex.org/hasItem");
    assert!(info.inserted_content_relation.is_none());
}

#[test]
fn get_membership_info_basic_returns_none() {
    let store = fresh();
    container::ensure_container(&store, "http://ex.org/bc/").unwrap();
    assert!(container::get_membership_info(&store, "http://ex.org/bc/")
        .unwrap()
        .is_none());
}

#[test]
fn add_remove_direct_membership_triple() {
    let store = fresh();
    container::add_direct_membership_triple(
        &store,
        "http://ex.org/res",
        "http://ex.org/has",
        "http://ex.org/item1",
    )
    .unwrap();
    assert!(ask(
        &store,
        "ASK { <http://ex.org/res> <http://ex.org/has> <http://ex.org/item1> }"
    ));
    container::remove_direct_membership_triple(
        &store,
        "http://ex.org/res",
        "http://ex.org/has",
        "http://ex.org/item1",
    )
    .unwrap();
    assert!(!ask(
        &store,
        "ASK { <http://ex.org/res> <http://ex.org/has> <http://ex.org/item1> }"
    ));
}

#[test]
fn list_membership_triples_direct() {
    let store = fresh();
    container::ensure_direct_container(
        &store,
        "http://ex.org/dc2/",
        "http://ex.org/mr",
        "http://ex.org/hasMember",
        None,
    )
    .unwrap();
    container::add_direct_membership_triple(
        &store,
        "http://ex.org/mr",
        "http://ex.org/hasMember",
        "http://ex.org/a",
    )
    .unwrap();
    container::add_direct_membership_triple(
        &store,
        "http://ex.org/mr",
        "http://ex.org/hasMember",
        "http://ex.org/b",
    )
    .unwrap();
    let triples = container::list_membership_triples(&store, "http://ex.org/dc2/").unwrap();
    assert_eq!(triples.len(), 2);
    assert!(triples
        .iter()
        .all(|(s, p, _)| s == "http://ex.org/mr" && p == "http://ex.org/hasMember"));
}

// ═══════════════════════════════════════════════════════════
// Indirect Container — store level
// ═══════════════════════════════════════════════════════════

#[test]
fn get_container_type_indirect() {
    let store = fresh();
    container::ensure_indirect_container(
        &store,
        "http://ex.org/ic/",
        "http://ex.org/res",
        container::LDP_MEMBER,
        "http://ex.org/via",
    )
    .unwrap();
    assert_eq!(
        container::get_container_type(&store, "http://ex.org/ic/"),
        ContainerType::Indirect
    );
}

#[test]
fn indirect_container_inserted_content_relation() {
    let store = fresh();
    container::ensure_indirect_container(
        &store,
        "http://ex.org/ic2/",
        "http://ex.org/col",
        "http://ex.org/hasBook",
        "http://ex.org/bookIRI",
    )
    .unwrap();
    let info = container::get_membership_info(&store, "http://ex.org/ic2/")
        .unwrap()
        .unwrap();
    assert_eq!(
        info.inserted_content_relation.as_deref(),
        Some("http://ex.org/bookIRI")
    );
}

// ═══════════════════════════════════════════════════════════
// Non-RDF Source — store level
// ═══════════════════════════════════════════════════════════

#[test]
fn get_container_type_non_rdf() {
    let store = fresh();
    container::store_binary_resource(&store, "http://ex.org/img", "image/png", b"\x89PNG").unwrap();
    assert_eq!(
        container::get_container_type(&store, "http://ex.org/img"),
        ContainerType::NonRdfSource
    );
}

#[test]
fn store_binary_resource_round_trip() {
    let store = fresh();
    let data = b"\x89PNG\r\nfake binary content";
    container::store_binary_resource(&store, "http://ex.org/img", "image/png", data).unwrap();
    let (ct, retrieved) = container::get_binary_resource(&store, "http://ex.org/img")
        .unwrap()
        .unwrap();
    assert_eq!(ct, "image/png");
    assert_eq!(retrieved, data);
}

#[test]
fn binary_content_type_preserved() {
    let store = fresh();
    container::store_binary_resource(&store, "http://ex.org/vid", "video/mp4", b"x").unwrap();
    let (ct, _) = container::get_binary_resource(&store, "http://ex.org/vid")
        .unwrap()
        .unwrap();
    assert_eq!(ct, "video/mp4");
}

#[test]
fn get_binary_resource_missing_returns_none() {
    let store = fresh();
    assert!(
        container::get_binary_resource(&store, "http://ex.org/missing")
            .unwrap()
            .is_none()
    );
}

#[test]
fn is_non_rdf_source_flag() {
    let store = fresh();
    assert!(!container::is_non_rdf_source(&store, "http://ex.org/x"));
    container::store_binary_resource(&store, "http://ex.org/x", "text/plain", b"hi").unwrap();
    assert!(container::is_non_rdf_source(&store, "http://ex.org/x"));
}

#[test]
fn binary_typed_as_non_rdf_source() {
    let store = fresh();
    container::store_binary_resource(&store, "http://ex.org/bin", "application/pdf", b"%PDF")
        .unwrap();
    assert!(ask(
        &store,
        &format!(
            "ASK {{ <http://ex.org/bin> <{}> <{}> }}",
            container::RDF_TYPE,
            container::LDP_NON_RDF_SOURCE
        )
    ));
}

// ═══════════════════════════════════════════════════════════
// HTTP handler integration tests
// ═══════════════════════════════════════════════════════════

mod http_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use open_triplestore::ldp::ldp_routes;
    use open_triplestore::server::AppState;

    fn make_router() -> (axum::Router, TripleStore) {
        let store = TripleStore::in_memory().unwrap();
        // Clone the store reference for the state — TripleStore is Arc-backed
        let state = AppState::test_default_with_store(store.clone());
        let router = ldp_routes().with_state(state);
        (router, store)
    }

    async fn body_string(body: Body) -> String {
        let bytes = body.collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).to_string()
    }

    // ── OPTIONS ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn options_includes_patch_in_allow() {
        let (router, store) = make_router();
        container::ensure_container(&store, "http://localhost/ldp/col/").unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/ldp/col/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let allow = resp
            .headers()
            .get("allow")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            allow.contains("PATCH"),
            "Allow should include PATCH, got: {allow}"
        );
    }

    #[tokio::test]
    async fn options_has_accept_patch_header() {
        let (router, store) = make_router();
        container::ensure_container(&store, "http://localhost/ldp/col2/").unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/ldp/col2/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let ap = resp
            .headers()
            .get("accept-patch")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ap.contains("application/sparql-update"),
            "Accept-Patch should include sparql-update, got: {ap}"
        );
    }

    // ── GET — link headers ────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_basic_container_link_headers() {
        let (router, store) = make_router();
        container::ensure_container(&store, "http://localhost/ldp/bc/").unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/ldp/bc/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let link = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            link.contains("BasicContainer"),
            "Link should mention BasicContainer, got: {link}"
        );
    }

    #[tokio::test]
    async fn get_direct_container_link_headers() {
        let (router, store) = make_router();
        container::ensure_direct_container(
            &store,
            "http://localhost/ldp/dc/",
            "http://localhost/ldp/dc/res",
            container::LDP_MEMBER,
            None,
        )
        .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/ldp/dc/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let link = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            link.contains("DirectContainer"),
            "Link should mention DirectContainer, got: {link}"
        );
    }

    #[tokio::test]
    async fn get_constrained_by_link_header() {
        let (router, store) = make_router();
        container::ensure_container(&store, "http://localhost/ldp/cb/").unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/ldp/cb/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let link = resp
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            link.contains("constrainedBy"),
            "Link should include constrainedBy, got: {link}"
        );
    }

    // ── GET — Prefer header ───────────────────────────────────────────────────

    #[tokio::test]
    async fn get_prefer_representation_includes_contains() {
        let (router, store) = make_router();
        container::ensure_container(&store, "http://localhost/ldp/pref/").unwrap();
        container::add_member(
            &store,
            "http://localhost/ldp/pref/",
            "http://localhost/ldp/pref/item1",
        )
        .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/ldp/pref/")
                    .header("prefer", "return=representation")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(resp.into_body()).await;
        assert!(
            body.contains("item1") || body.contains("ldp#contains"),
            "Body should include member with return=representation, got: {body}"
        );
    }

    #[tokio::test]
    async fn get_prefer_minimal_omits_contains() {
        let (router, store) = make_router();
        container::ensure_container(&store, "http://localhost/ldp/min/").unwrap();
        container::add_member(
            &store,
            "http://localhost/ldp/min/",
            "http://localhost/ldp/min/item1",
        )
        .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/ldp/min/")
                    .header("prefer", "return=minimal")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = body_string(resp.into_body()).await;
        assert!(
            !body.contains("ldp/min/item1"),
            "Body should NOT include member IRI with return=minimal, got: {body}"
        );
    }

    #[tokio::test]
    async fn get_preference_applied_header_present() {
        let (router, store) = make_router();
        container::ensure_container(&store, "http://localhost/ldp/pa/").unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/ldp/pa/")
                    .header("prefer", "return=minimal")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.headers().contains_key("preference-applied"),
            "Response should have Preference-Applied header"
        );
    }

    // ── GET — Non-RDF Source ──────────────────────────────────────────────────

    #[tokio::test]
    async fn get_non_rdf_source_binary() {
        let (router, store) = make_router();
        let data = b"\x89PNG fake";
        container::store_binary_resource(&store, "http://localhost/ldp/img.png", "image/png", data)
            .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/ldp/img.png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(ct, "image/png");
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body_bytes[..], data);
    }

    // ── POST ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn post_turtle_creates_member() {
        let (router, store) = make_router();
        container::ensure_container(&store, "http://localhost/ldp/ttl/").unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ldp/ttl/")
                    .header("content-type", "text/turtle")
                    .header("slug", "new-item")
                    .body(Body::from(
                        "<http://localhost/ldp/ttl/new-item> <http://example.org/p> \"v\" .\n",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            location.contains("new-item"),
            "Location should reference slug, got: {location}"
        );
    }

    #[tokio::test]
    async fn post_to_direct_container_adds_membership_triple() {
        let (router, store) = make_router();
        container::ensure_direct_container(
            &store,
            "http://localhost/ldp/dc2/",
            "http://localhost/ldp/dc2/",
            container::LDP_MEMBER,
            None,
        )
        .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/ldp/dc2/")
                    .header("content-type", "text/turtle")
                    .header("slug", "m1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        assert!(
            ask(
                &store,
                &format!(
                    "ASK {{ <http://localhost/ldp/dc2/> <{}> <http://localhost/ldp/dc2/m1> }}",
                    container::LDP_MEMBER
                )
            ),
            "Direct Container membership triple should be present"
        );
    }

    // ── PATCH ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn patch_applies_sparql_update() {
        let (router, store) = make_router();
        store.load_str(
            "<http://localhost/ldp/patch-me> <http://example.org/val> \"old\" .\n\
             <http://localhost/ldp/patch-me> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://www.w3.org/ns/ldp#RDFSource> .",
            oxigraph::io::RdfFormat::NTriples,
            None,
        ).unwrap();

        let update = "DELETE { <http://localhost/ldp/patch-me> <http://example.org/val> \"old\" } \
                      INSERT { <http://localhost/ldp/patch-me> <http://example.org/val> \"new\" } WHERE {}";
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/ldp/patch-me")
                    .header("content-type", "application/sparql-update")
                    .body(Body::from(update))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(ask(
            &store,
            "ASK { <http://localhost/ldp/patch-me> <http://example.org/val> \"new\" }"
        ));
    }

    #[tokio::test]
    async fn patch_wrong_content_type_415() {
        let (router, store) = make_router();
        store
            .load_str(
                "<http://localhost/ldp/r1> <http://example.org/p> \"v\" .",
                oxigraph::io::RdfFormat::NTriples,
                None,
            )
            .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/ldp/r1")
                    .header("content-type", "text/turtle")
                    .body(Body::from(
                        "<http://localhost/ldp/r1> <http://example.org/p> \"v2\" .",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn patch_missing_resource_404() {
        let (router, _store) = make_router();
        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/ldp/does-not-exist")
                    .header("content-type", "application/sparql-update")
                    .body(Body::from(
                        "INSERT DATA { <http://ex.org/x> <http://ex.org/p> <http://ex.org/y> }",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn patch_if_match_mismatch_412() {
        let (router, store) = make_router();
        store
            .load_str(
                "<http://localhost/ldp/etag-test> <http://example.org/p> \"v\" .",
                oxigraph::io::RdfFormat::NTriples,
                None,
            )
            .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/ldp/etag-test")
                    .header("content-type", "application/sparql-update")
                    .header("if-match", "\"wrong-etag\"")
                    .body(Body::from(
                        "INSERT DATA { <http://ex.org/x> <http://ex.org/p> <http://ex.org/y> }",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
    }

    // ── PUT ───────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn put_if_match_wrong_etag_412() {
        let (router, store) = make_router();
        store
            .load_str(
                "<http://localhost/ldp/put-etag> <http://example.org/p> \"v\" .",
                oxigraph::io::RdfFormat::NTriples,
                None,
            )
            .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/ldp/put-etag")
                    .header("content-type", "text/turtle")
                    .header("if-match", "\"wrong\"")
                    .body(Body::from(
                        "<http://localhost/ldp/put-etag> <http://example.org/p> \"v2\" .\n",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PRECONDITION_FAILED);
    }

    #[tokio::test]
    async fn put_if_match_correct_etag_204() {
        let (router, store) = make_router();
        let nt = "<http://localhost/ldp/put-ok> <http://example.org/p> \"v\" .\n";
        store
            .load_str(nt, oxigraph::io::RdfFormat::NTriples, None)
            .unwrap();
        let body_bytes =
            container::describe_resource(&store, "http://localhost/ldp/put-ok").unwrap();
        let etag = container::compute_etag(&body_bytes);

        let resp = router
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/ldp/put-ok")
                    .header("content-type", "text/turtle")
                    .header("if-match", &etag)
                    .body(Body::from(
                        "<http://localhost/ldp/put-ok> <http://example.org/p> \"v2\" .\n",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // ── HEAD ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn head_returns_etag() {
        let (router, store) = make_router();
        store
            .load_str(
                "<http://localhost/ldp/head-me> <http://example.org/p> \"v\" .",
                oxigraph::io::RdfFormat::NTriples,
                None,
            )
            .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .method("HEAD")
                    .uri("/ldp/head-me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers().contains_key("etag"),
            "HEAD response should have ETag"
        );
    }

    // ── DELETE ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_direct_member_removes_membership_triple() {
        let (router, store) = make_router();
        container::ensure_direct_container(
            &store,
            "http://localhost/ldp/del-dc/",
            "http://localhost/ldp/del-dc/",
            container::LDP_MEMBER,
            None,
        )
        .unwrap();
        container::add_member(
            &store,
            "http://localhost/ldp/del-dc/",
            "http://localhost/ldp/del-dc/item",
        )
        .unwrap();
        container::add_direct_membership_triple(
            &store,
            "http://localhost/ldp/del-dc/",
            container::LDP_MEMBER,
            "http://localhost/ldp/del-dc/item",
        )
        .unwrap();

        let resp = router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/ldp/del-dc/item")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        assert!(!ask(
            &store,
            &format!(
                "ASK {{ <http://localhost/ldp/del-dc/> <{}> <http://localhost/ldp/del-dc/item> }}",
                container::LDP_MEMBER
            )
        ));
    }
}
