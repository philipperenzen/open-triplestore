//! Security regression tests for the federated-login (OIDC / SAML) paths.
//!
//! Covers:
//!  * S6 — successful/failed SSO logins must emit audit events. The OIDC/SAML
//!    HTTP callbacks cannot be driven end-to-end without a live IdP (code
//!    exchange / signed-assertion verification), so we test the exact mechanism
//!    the `saml_acs` / `oidc_callback` handlers rely on: an `AuditLogger`
//!    `LoginSuccess` / `LoginFailure` row carrying the SSO provider details, with
//!    the actor recovered from the just-issued access token (see
//!    `oauth_handlers::audit_sso_login_success` / `audit_sso_login_failure`).
//!  * S13 — the returning-user branch of `provision_or_link_user` must keep
//!    returning the existing linked user (and refresh its identity record)
//!    instead of swallowing/aborting on the housekeeping upsert.
//!  * The login page only offers providers a sign-in can start from: the
//!    synthetic `env-oidc` row that `OIDC_ISSUER` creates (no client_id) stays
//!    off it, through admin edits, while bearer tokens from that IdP keep
//!    verifying and JIT-provisioning against the row.
//!  * SP-initiated SAML (`saml` feature): the login route sends an AuthnRequest
//!    to the IdP, and the ACS accepts only a signed response that answers that
//!    request, presented once, from the browser that started it.

mod common;
use common::*;

use std::sync::Arc;

use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
};
use open_triplestore::auth::{
    audit::{AuditEventBuilder, AuditEventType, AuditLogger, AuditOutcome},
    db::AuthDb,
    jwt::{issue_access_token, verify_token, JwtConfig},
    models::{OauthProviderCreate, SystemRole},
    oauth::provision_or_link_user,
    oidc_provider::ProviderKeys,
    oidc_rs::{ensure_env_provider, AuthExt, OidcVerifier, ENV_OIDC_PROVIDER_SLUG},
};
use tower::ServiceExt as _;

/// A provider row of the given type; `client_id` is what an admin typed (or
/// left out).
fn provider_row(slug: &str, provider_type: &str, client_id: Option<&str>) -> OauthProviderCreate {
    OauthProviderCreate {
        name: format!("Provider {slug}"),
        slug: slug.to_string(),
        provider_type: provider_type.to_string(),
        client_id: client_id.map(str::to_string),
        client_secret: None,
        client_secret_enc: None,
        discovery_url: Some("https://idp.example.com/.well-known/openid-configuration".to_string()),
        tenant_id: None,
        entity_id: None,
        sso_url: None,
        idp_certificate: None,
        scopes: None,
        role_claim_map: None,
        auto_provision: true,
        default_role: Some("user".to_string()),
        is_active: true,
    }
}

/// Build a minimal OIDC provider row and return its id.
fn make_oidc_provider(db: &Arc<AuthDb>, slug: &str, auto_provision: bool) -> String {
    let create = OauthProviderCreate {
        auto_provision,
        ..provider_row(slug, "oidc", Some("client-123"))
    };
    db.create_oauth_provider(&create).unwrap().id
}

/// S13: a returning SSO user (an identity link already exists for this
/// provider+subject) is found and returned, and the identity record's
/// `last_login_at` is refreshed rather than the call silently failing.
#[test]
fn returning_user_is_relinked_and_returned() {
    let db = Arc::new(AuthDb::in_memory().unwrap());
    let provider_id = make_oidc_provider(&db, "acme", true);
    let provider = db.get_oauth_provider_by_id(&provider_id).unwrap().unwrap();

    // Existing local user with an established identity link.
    let user = db
        .create_user(
            "u-return",
            "returning",
            "returning@example.com",
            "oauth:acme:ext-sub-1",
            SystemRole::User,
        )
        .unwrap();
    db.upsert_oauth_identity("id-1", &user.id, &provider_id, "ext-sub-1", None)
        .unwrap();

    // The returning-user branch must return the SAME user without error.
    let resolved = provision_or_link_user(
        "ext-sub-1",
        Some("returning@example.com"),
        true,
        "Returning User",
        SystemRole::User,
        &provider,
        &db,
    )
    .unwrap();
    assert_eq!(
        resolved.id, user.id,
        "must resolve to the linked local user"
    );

    // The housekeeping upsert ran: last_login_at is populated and no duplicate
    // identity was created.
    let identities = db.list_oauth_identities_for_user(&user.id).unwrap();
    assert_eq!(identities.len(), 1, "must not create a duplicate identity");
    assert!(
        identities[0].last_login_at.is_some(),
        "returning-user login must refresh last_login_at"
    );
}

/// S6: the audit mechanism used by the SSO callbacks records a `LoginSuccess`
/// row carrying the SSO provider details, with the actor recovered from the
/// freshly issued access token (id / username / role).
#[test]
fn sso_login_success_audit_event_is_recorded() {
    let db = Arc::new(AuthDb::in_memory().unwrap());
    let logger = AuditLogger::new(db.pool());
    let jwt = JwtConfig::new(JWT_SECRET.to_string(), 30, 30);

    // Token issued by the flow for the provisioned user.
    let access = issue_access_token(&jwt, "u-sso", "ssouser", "user").unwrap();

    // Mirror oauth_handlers::audit_sso_login_success.
    let mut b = AuditEventBuilder::new(AuditEventType::LoginSuccess, AuditOutcome::Success)
        .details(serde_json::json!({
            "auth_method": "sso",
            "provider_type": "oidc",
            "provider_slug": "acme",
        }));
    let claims = verify_token(&jwt, &access).unwrap();
    b = b.actor(claims.sub, claims.username, claims.role);
    logger.log(b);

    let events = logger
        .list(
            10,
            0,
            Some(AuditEventType::LoginSuccess.as_str()),
            None,
            None,
        )
        .unwrap();
    assert_eq!(events.len(), 1, "exactly one LoginSuccess must be recorded");
    let ev = &events[0];
    assert_eq!(ev.event_type, "login_success");
    assert_eq!(ev.actor_id.as_deref(), Some("u-sso"));
    assert_eq!(ev.actor_username.as_deref(), Some("ssouser"));
    let details = ev.details.as_ref().expect("details present");
    assert_eq!(details["auth_method"], "sso");
    assert_eq!(details["provider_type"], "oidc");
    assert_eq!(details["provider_slug"], "acme");
}

/// S6 (failure path): a rejected SSO assertion/callback records a `LoginFailure`
/// row with the provider details and a redacted reason, and no actor.
#[test]
fn sso_login_failure_audit_event_is_recorded() {
    let db = Arc::new(AuthDb::in_memory().unwrap());
    let logger = AuditLogger::new(db.pool());

    // Mirror oauth_handlers::audit_sso_login_failure.
    let b = AuditEventBuilder::new(AuditEventType::LoginFailure, AuditOutcome::Failure).details(
        serde_json::json!({
            "auth_method": "sso",
            "provider_type": "saml",
            "provider_slug": "corp",
            "reason": "assertion_rejected",
        }),
    );
    logger.log(b);

    let events = logger
        .list(
            10,
            0,
            Some(AuditEventType::LoginFailure.as_str()),
            None,
            None,
        )
        .unwrap();
    assert_eq!(events.len(), 1, "exactly one LoginFailure must be recorded");
    let ev = &events[0];
    assert_eq!(ev.event_type, "login_failure");
    assert_eq!(ev.outcome, "failure");
    assert!(ev.actor_id.is_none(), "failed SSO login has no known actor");
    let details = ev.details.as_ref().expect("details present");
    assert_eq!(details["provider_type"], "saml");
    assert_eq!(details["reason"], "assertion_rejected");
}

// ─── Login page vs. the resource-server provider row ──────────────────────────

/// The slugs the public login page renders as SSO buttons, sorted.
async fn login_page_slugs(app: &axum::Router) -> Vec<String> {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/oauth/providers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let mut slugs: Vec<String> = body_json(resp.into_body())
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["slug"].as_str().unwrap().to_string())
        .collect();
    slugs.sort();
    slugs
}

async fn authorize_status(app: &axum::Router, slug: &str) -> StatusCode {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/auth/oauth/{slug}/authorize"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
}

/// An OIDC row without a client_id cannot start a browser login (the flow bails
/// on the missing client_id), so the login page must not render it as a button:
/// that covers the synthetic `env-oidc` row `OIDC_ISSUER` creates and an OIDC
/// row an admin saved without one. SAML rows carry no client_id by design; one
/// with an SSO URL is listed when the build supports SAML.
#[tokio::test]
async fn providers_without_a_client_id_are_not_offered_on_the_login_page() {
    let state = test_state();
    let db = &state.auth_db;
    make_oidc_provider(db, "acme", true);
    db.create_oauth_provider(&OauthProviderCreate {
        sso_url: Some("https://idp.example.com/saml/sso".to_string()),
        ..provider_row("corp-saml", "saml", None)
    })
    .unwrap();
    // Nowhere to send an AuthnRequest.
    db.create_oauth_provider(&provider_row("no-sso-url", "saml", None))
        .unwrap();
    db.create_oauth_provider(&provider_row("no-client", "oidc", None))
        .unwrap();
    db.create_oauth_provider(&provider_row("blank-client", "oidc", Some("  ")))
        .unwrap();
    ensure_env_provider(db, "https://idp.example.org", "user").unwrap();
    let app = test_app(state);

    let expected: &[&str] = if cfg!(feature = "saml") {
        &["acme", "corp-saml"]
    } else {
        &["acme"]
    };
    assert_eq!(login_page_slugs(&app).await, expected);
    // Hitting the authorize URL directly finds no login to start either.
    for slug in [ENV_OIDC_PROVIDER_SLUG, "no-client", "blank-client"] {
        assert_eq!(
            authorize_status(&app, slug).await,
            StatusCode::NOT_FOUND,
            "{slug}"
        );
    }
}

/// End to end in resource-server mode: an admin edits the `env-oidc` row
/// through the identity-provider API (renames it, saves an empty client_id,
/// later switches it off). It never shows up on the login page, and bearer
/// tokens from the configured issuer keep verifying and JIT-provisioning one
/// local user against that row.
#[tokio::test]
async fn edited_env_oidc_row_stays_off_the_login_page_and_keeps_serving_bearer_tokens() {
    // The IdP: another instance whose built-in OIDC provider signs the tokens.
    let mut idp = test_state();
    idp.oidc_provider = Some(Arc::new(
        ProviderKeys::load_or_generate(&idp.auth_db, "test-secret").unwrap(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    idp.base_url = Arc::new(issuer.clone());
    let keys = idp.oidc_provider.clone().unwrap();
    let idp_app = test_app(idp);
    std::thread::spawn(move || {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                let l = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(l, idp_app).await.unwrap();
            });
    });

    // The resource server, configured as OIDC_ISSUER=<idp> OIDC_AUDIENCE=ots-api.
    let (mut rs, admin_token) = admin_state();
    let mut ext = AuthExt::disabled();
    ext.oidc = Some(OidcVerifier::new(issuer.clone(), Some("ots-api".into())));
    rs.auth_ext = Arc::new(ext);
    let env = ensure_env_provider(&rs.auth_db, &issuer, "user").unwrap();
    let app = test_app(rs.clone());

    let edit = |name: &str, is_active: bool| {
        let body = serde_json::json!({
            "name": name,
            "slug": ENV_OIDC_PROVIDER_SLUG,
            "provider_type": "oidc",
            "client_id": "",
            "discovery_url": env.discovery_url,
            "scopes": "openid email profile",
            "auto_provision": true,
            "default_role": "user",
            "is_active": is_active,
        });
        Request::builder()
            .method(Method::PUT)
            .uri(format!("/api/admin/oauth/providers/{}", env.id))
            .header(header::AUTHORIZATION, format!("Bearer {admin_token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let me = |sub: &str| {
        let now = chrono::Utc::now().timestamp();
        let token = keys
            .sign_claims(&serde_json::json!({
                "iss": issuer,
                "sub": sub,
                "aud": "ots-api",
                "iat": now,
                "nbf": now - 5,
                "exp": now + 60,
                "preferred_username": "rs-user",
                "email": "rs-user@example.org",
            }))
            .unwrap();
        Request::builder()
            .uri("/api/auth/me")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    };

    let resp = app
        .clone()
        .oneshot(edit("Company IdP", true))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(login_page_slugs(&app).await.is_empty());

    let resp = app.clone().oneshot(me("ext-sub-1")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let first = body_json(resp.into_body()).await;
    assert_eq!(first["username"], "rs-user");

    // Switched off: still not on the login page, and the same subject still
    // resolves to the same JIT-provisioned user through the row.
    let resp = app
        .clone()
        .oneshot(edit("Company IdP", false))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(login_page_slugs(&app).await.is_empty());
    let resp = app.clone().oneshot(me("ext-sub-1")).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp.into_body()).await["id"], first["id"]);

    // The edits kept the one row the resource-server path looks up by slug.
    let row = ensure_env_provider(&rs.auth_db, &issuer, "user").unwrap();
    assert_eq!(row.id, env.id);
    assert_eq!(row.name, "Company IdP");
}

// ─── SP-initiated SAML ────────────────────────────────────────────────────────

#[cfg(feature = "saml")]
mod saml_sp_initiated {
    use super::*;
    use base64::Engine as _;
    use samael::crypto::{CertificateDer, Crypto, CryptoProvider};
    use samael::idp::response_builder::{build_response_template, ResponseAttribute};
    use samael::idp::sp_extractor::RequiredAttribute;
    use samael::idp::{CertificateParams, IdentityProvider, KeyType, Rsa};
    use samael::traits::ToXml;
    use std::io::Read as _;

    const IDP_ENTITY: &str = "https://idp.example.org/saml";
    const IDP_SSO: &str = "https://idp.example.org/saml/sso";
    const SLUG: &str = "corp";
    const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

    /// An IdP with its own signing key, standing in for the real one.
    struct TestIdp {
        key: IdentityProvider,
        cert: CertificateDer,
    }

    impl TestIdp {
        fn new() -> Self {
            let key = IdentityProvider::generate_new(KeyType::Rsa(Rsa::Rsa2048)).unwrap();
            let cert = key
                .create_certificate(&CertificateParams {
                    common_name: "idp.example.org",
                    issuer_name: "idp.example.org",
                    days_until_expiration: 30,
                })
                .unwrap();
            Self { key, cert }
        }

        /// A signed, base64-encoded response for `name_id` (with an `email`
        /// attribute) answering the AuthnRequest `in_response_to`.
        fn response(&self, base_url: &str, in_response_to: &str, name_id: &str) -> String {
            let audience = format!("{base_url}/api/auth/saml/{SLUG}/metadata");
            let acs = format!("{base_url}/api/auth/saml/{SLUG}/acs");
            let email = format!("{name_id}@example.org");
            let attributes = [ResponseAttribute {
                required_attribute: RequiredAttribute {
                    name: "email".to_string(),
                    format: None,
                },
                value: &email,
            }];
            let mut response = build_response_template(
                &self.cert,
                name_id,
                &audience,
                IDP_ENTITY,
                &acs,
                in_response_to,
                &attributes,
            );
            // samael's template leaves the bearer confirmation without
            // NotOnOrAfter, which a real IdP sets and the SP side requires.
            let confirmation = &mut response
                .assertion
                .as_mut()
                .unwrap()
                .subject
                .as_mut()
                .unwrap()
                .subject_confirmations
                .as_mut()
                .unwrap()[0];
            confirmation
                .subject_confirmation_data
                .as_mut()
                .unwrap()
                .not_on_or_after = Some(chrono::Utc::now() + chrono::Duration::minutes(5));
            let xml = response.to_string().unwrap();
            let signed =
                Crypto::sign_xml(&xml, &self.key.export_private_key_der().unwrap()).unwrap();
            B64.encode(signed)
        }
    }

    /// A store with one SAML provider trusting `idp`; returns the app and its
    /// base URL.
    fn store_trusting(idp: &TestIdp) -> (axum::Router, String) {
        let state = test_state();
        state
            .auth_db
            .create_oauth_provider(&OauthProviderCreate {
                entity_id: Some(IDP_ENTITY.to_string()),
                sso_url: Some(IDP_SSO.to_string()),
                idp_certificate: Some(B64.encode(idp.cert.der_data())),
                discovery_url: None,
                ..provider_row(SLUG, "saml", None)
            })
            .unwrap();
        let base_url = state.base_url.to_string();
        (test_app(state), base_url)
    }

    /// Start a sign-in the way the login button does. Returns the RelayState
    /// (also set as the `saml_state` cookie) and the AuthnRequest's ID.
    async fn start(app: &axum::Router, base_url: &str) -> (String, String) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/auth/saml/{SLUG}/login"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        let cookie = resp.headers()[header::SET_COOKIE].to_str().unwrap();
        let cookie_relay = cookie
            .strip_prefix("saml_state=")
            .and_then(|c| c.split(';').next())
            .unwrap()
            .to_string();
        let location = url::Url::parse(resp.headers()[header::LOCATION].to_str().unwrap()).unwrap();
        assert!(location.as_str().starts_with(IDP_SSO), "{location}");
        let query: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
        assert_eq!(query["RelayState"], cookie_relay);

        // HTTP-Redirect binding: base64 of a raw-DEFLATEd AuthnRequest.
        let mut request = String::new();
        flate2::read::DeflateDecoder::new(B64.decode(&query["SAMLRequest"]).unwrap().as_slice())
            .read_to_string(&mut request)
            .unwrap();
        assert!(request.contains(&format!("{base_url}/api/auth/saml/{SLUG}/acs")));
        assert!(request.contains(&format!("{base_url}/api/auth/saml/{SLUG}/metadata")));
        let id = request
            .split(" ID=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap()
            .to_string();
        (cookie_relay, id)
    }

    async fn post_acs(
        app: &axum::Router,
        saml_response: &str,
        relay_state: &str,
        cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("SAMLResponse", saml_response)
            .append_pair("RelayState", relay_state)
            .finish();
        let mut req = Request::builder()
            .method(Method::POST)
            .uri(format!("/api/auth/saml/{SLUG}/acs"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, format!("saml_state={c}"));
        }
        app.clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    /// The whole browser round trip: login button → IdP → ACS → the SPA's
    /// callback page, signed in as the asserted user. The response cannot be
    /// presented a second time.
    #[tokio::test]
    async fn sp_initiated_sign_in_lands_on_the_callback_page_signed_in() {
        let idp = TestIdp::new();
        let (app, base_url) = store_trusting(&idp);

        let (relay, request_id) = start(&app, &base_url).await;
        let response = idp.response(&base_url, &request_id, "alice");
        let resp = post_acs(&app, &response, &relay, Some(&relay)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers()[header::LOCATION].to_str().unwrap();
        let fragment = location
            .strip_prefix(&format!("{base_url}/oauth/callback#"))
            .unwrap_or_else(|| panic!("unexpected landing {location}"));
        let tokens: std::collections::HashMap<_, _> =
            url::form_urlencoded::parse(fragment.as_bytes())
                .into_owned()
                .collect();
        let cleared = resp.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cleared.starts_with("saml_state=;") && cleared.contains("Max-Age=0"));

        let me = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", tokens["access_token"]),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(me.status(), StatusCode::OK);
        assert_eq!(
            body_json(me.into_body()).await["email"],
            "alice@example.org"
        );

        // Replay: the request it answered has been used up.
        let resp = post_acs(&app, &response, &relay, Some(&relay)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// The ACS refuses a response that is not bound to a request this browser
    /// started: missing or foreign `saml_state` cookie (login CSRF), an
    /// unknown RelayState (IdP-initiated or forged), a response answering a
    /// different request, and a response signed by a key the provider does not
    /// trust.
    #[tokio::test]
    async fn acs_refuses_responses_not_bound_to_this_browsers_request() {
        let idp = TestIdp::new();
        let (app, base_url) = store_trusting(&idp);

        let (relay, request_id) = start(&app, &base_url).await;
        let good = idp.response(&base_url, &request_id, "alice");
        // No cookie, or another browser's cookie.
        assert_eq!(
            post_acs(&app, &good, &relay, None).await.status(),
            StatusCode::BAD_REQUEST
        );
        let (other_relay, _) = start(&app, &base_url).await;
        assert_eq!(
            post_acs(&app, &good, &relay, Some(&other_relay))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        // A RelayState this store never issued.
        assert_eq!(
            post_acs(&app, &good, "made-up", Some("made-up"))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );

        // A validly signed response to some other request.
        let (relay, _) = start(&app, &base_url).await;
        let other = idp.response(&base_url, "_another-request", "alice");
        assert_eq!(
            post_acs(&app, &other, &relay, Some(&relay)).await.status(),
            StatusCode::UNAUTHORIZED
        );

        // The right request, signed by an IdP key the provider does not trust.
        let (relay, request_id) = start(&app, &base_url).await;
        let forged = TestIdp::new().response(&base_url, &request_id, "alice");
        assert_eq!(
            post_acs(&app, &forged, &relay, Some(&relay)).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
