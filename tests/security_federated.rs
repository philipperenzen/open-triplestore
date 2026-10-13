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
//!  * SAML 2.0 (`saml` feature), against a fake IdP in this process: sign-in
//!    started here or at the IdP, signature, audience, recipient, destination
//!    and expiry checks, SHA-1 and DOCTYPE refusal, encrypted assertions, signed
//!    AuthnRequests, SP key rollover, IdP metadata import and Single Logout in
//!    both directions.

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
        saml_config: None,
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

// ─── SAML 2.0 against a fake IdP ──────────────────────────────────────────────

#[cfg(feature = "saml")]
mod saml {
    use super::*;
    use base64::Engine as _;
    use chrono::{Duration, SecondsFormat, Utc};
    use open_triplestore::auth::models::{OauthProvider, SamlConfig};
    use open_triplestore::server::AppState;
    use openssl::hash::MessageDigest;
    use openssl::pkey::{PKey, Private, Public};
    use samael::crypto::{Crypto, CryptoProvider};
    use samael::idp::{CertificateParams, IdentityProvider, KeyType, Rsa};
    use std::collections::HashMap;
    use std::io::{Read as _, Write as _};

    const IDP_ENTITY: &str = "https://idp.example.org/saml";
    const IDP_SSO: &str = "https://idp.example.org/saml/sso";
    const IDP_SLO: &str = "https://idp.example.org/saml/slo";
    const SLUG: &str = "corp";
    const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;
    const NS_P: &str = "urn:oasis:names:tc:SAML:2.0:protocol";
    const NS_A: &str = "urn:oasis:names:tc:SAML:2.0:assertion";
    const PERSISTENT: &str = "urn:oasis:names:tc:SAML:2.0:nameid-format:persistent";
    const TRANSIENT: &str = "urn:oasis:names:tc:SAML:2.0:nameid-format:transient";
    const SHA256: (&str, &str) = (
        "http://www.w3.org/2001/04/xmldsig-more#rsa-sha256",
        "http://www.w3.org/2001/04/xmlenc#sha256",
    );
    const SHA1: (&str, &str) = (
        "http://www.w3.org/2000/09/xmldsig#rsa-sha1",
        "http://www.w3.org/2000/09/xmldsig#sha1",
    );

    fn strip_decl(xml: &str) -> String {
        let xml = xml.trim_start();
        match xml.strip_prefix("<?xml") {
            Some(rest) => rest.split_once("?>").unwrap().1.trim_start().to_string(),
            None => xml.to_string(),
        }
    }

    fn ts(d: chrono::DateTime<Utc>) -> String {
        d.to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    fn sig_template(id: &str, alg: (&str, &str)) -> String {
        format!(
            r##"<ds:Signature xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><ds:SignedInfo><ds:CanonicalizationMethod Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/><ds:SignatureMethod Algorithm="{}"/><ds:Reference URI="#{id}"><ds:Transforms><ds:Transform Algorithm="http://www.w3.org/2000/09/xmldsig#enveloped-signature"/><ds:Transform Algorithm="http://www.w3.org/2001/10/xml-exc-c14n#"/></ds:Transforms><ds:DigestMethod Algorithm="{}"/><ds:DigestValue></ds:DigestValue></ds:Reference></ds:SignedInfo><ds:SignatureValue></ds:SignatureValue></ds:Signature>"##,
            alg.0, alg.1
        )
    }

    fn deflate_b64(xml: &str) -> String {
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(xml.as_bytes()).unwrap();
        B64.encode(enc.finish().unwrap())
    }

    fn inflate_b64(b64: &str) -> String {
        let mut out = String::new();
        flate2::read::DeflateDecoder::new(B64.decode(b64).unwrap().as_slice())
            .read_to_string(&mut out)
            .unwrap();
        out
    }

    fn enc(v: &str) -> String {
        url::form_urlencoded::byte_serialize(v.as_bytes()).collect()
    }

    /// The query of a redirect URL: raw pairs and decoded values.
    fn query_of(location: &str) -> (Vec<(String, String)>, HashMap<String, String>) {
        let q = location.split_once('?').unwrap().1;
        let raw = q
            .split('&')
            .map(|p| {
                let (k, v) = p.split_once('=').unwrap();
                (k.to_string(), v.to_string())
            })
            .collect();
        let decoded = url::form_urlencoded::parse(q.as_bytes())
            .into_owned()
            .collect();
        (raw, decoded)
    }

    /// Verify an HTTP-Redirect signature (RSA-SHA256) made by the store's SP key.
    fn redirect_signature_verifies(location: &str, param: &str, cert_der: &[u8]) -> bool {
        let (raw, decoded) = query_of(location);
        let get = |k: &str| raw.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let mut signed = format!("{param}={}", get(param).unwrap());
        if let Some(rs) = get("RelayState") {
            signed.push_str(&format!("&RelayState={rs}"));
        }
        signed.push_str(&format!("&SigAlg={}", get("SigAlg").unwrap()));
        assert_eq!(decoded["SigAlg"], SHA256.0);
        let key: PKey<Public> = openssl::x509::X509::from_der(cert_der)
            .unwrap()
            .public_key()
            .unwrap();
        let mut v = openssl::sign::Verifier::new(MessageDigest::sha256(), &key).unwrap();
        v.update(signed.as_bytes()).unwrap();
        v.verify(&B64.decode(&decoded["Signature"]).unwrap())
            .unwrap()
    }

    /// An IdP with its own signing key, standing in for the real one.
    struct TestIdp {
        key_der: Vec<u8>,
        pkey: PKey<Private>,
        cert: Vec<u8>,
    }

    /// How the fake IdP builds one response.
    #[derive(Clone)]
    struct Answer {
        in_response_to: Option<String>,
        name_id: String,
        name_id_format: String,
        audience: String,
        recipient: String,
        destination: String,
        issuer: String,
        expires_in: Duration,
        attributes: Vec<(String, Vec<String>)>,
        session_index: String,
        sign_assertion: bool,
        sign_response: bool,
        alg: (&'static str, &'static str),
        encrypt_to: Option<(Vec<u8>, Enc)>,
        doctype: bool,
    }

    #[derive(Clone, Copy)]
    enum Enc {
        /// AES-256-GCM, RSA-OAEP (MGF1-SHA1, SHA-1 digest).
        Gcm,
        /// AES-128-CBC, xmlenc11 RSA-OAEP with a SHA-256 digest.
        Cbc,
    }

    impl TestIdp {
        fn new() -> Self {
            let idp = IdentityProvider::generate_new(KeyType::Rsa(Rsa::Rsa2048)).unwrap();
            let cert = idp
                .create_certificate(&CertificateParams {
                    common_name: "idp.example.org",
                    issuer_name: "idp.example.org",
                    days_until_expiration: 30,
                })
                .unwrap();
            let key_der = idp.export_private_key_der().unwrap();
            let pkey =
                PKey::from_rsa(openssl::rsa::Rsa::private_key_from_der(&key_der).unwrap()).unwrap();
            Self {
                key_der,
                pkey,
                cert: cert.der_data().to_vec(),
            }
        }

        fn cert_pem(&self) -> String {
            let b64 = B64.encode(&self.cert);
            format!("-----BEGIN CERTIFICATE-----\n{b64}\n-----END CERTIFICATE-----\n")
        }

        fn sign(&self, xml: &str) -> String {
            strip_decl(&Crypto::sign_xml(xml, &self.key_der).unwrap())
        }

        /// The default answer for `name_id` to `in_response_to` at `store`.
        fn answer(&self, store: &Store, in_response_to: Option<&str>, name_id: &str) -> Answer {
            Answer {
                in_response_to: in_response_to.map(str::to_string),
                name_id: name_id.to_string(),
                name_id_format: PERSISTENT.to_string(),
                audience: store.entity_id(),
                recipient: store.acs(),
                destination: store.acs(),
                issuer: IDP_ENTITY.to_string(),
                expires_in: Duration::minutes(5),
                attributes: vec![(
                    "urn:oid:0.9.2342.19200300.100.1.3".to_string(),
                    vec![format!("{name_id}@example.org")],
                )],
                session_index: format!("_s-{name_id}"),
                sign_assertion: true,
                sign_response: false,
                alg: SHA256,
                encrypt_to: None,
                doctype: false,
            }
        }

        /// A base64 SAMLResponse built from `a`.
        fn respond(&self, a: &Answer) -> String {
            let now = Utc::now();
            let exp = ts(now + a.expires_in);
            let (now_s, nb) = (ts(now), ts(now - Duration::minutes(1)));
            let aid = format!("_a{}", uuid::Uuid::new_v4().simple());
            let rid = format!("_r{}", uuid::Uuid::new_v4().simple());
            let irt = a
                .in_response_to
                .as_deref()
                .map(|v| format!(" InResponseTo=\"{v}\""))
                .unwrap_or_default();
            let attrs: String = a
                .attributes
                .iter()
                .map(|(n, vs)| {
                    let values: String = vs
                        .iter()
                        .map(|v| format!("<saml:AttributeValue>{v}</saml:AttributeValue>"))
                        .collect();
                    format!("<saml:Attribute Name=\"{n}\">{values}</saml:Attribute>")
                })
                .collect();
            let statement = if attrs.is_empty() {
                String::new()
            } else {
                format!("<saml:AttributeStatement>{attrs}</saml:AttributeStatement>")
            };
            let asig = if a.sign_assertion {
                sig_template(&aid, a.alg)
            } else {
                String::new()
            };
            let mut assertion = format!(
                r#"<saml:Assertion xmlns:saml="{NS_A}" ID="{aid}" Version="2.0" IssueInstant="{now_s}"><saml:Issuer>{issuer}</saml:Issuer>{asig}<saml:Subject><saml:NameID Format="{fmt}">{name_id}</saml:NameID><saml:SubjectConfirmation Method="urn:oasis:names:tc:SAML:2.0:cm:bearer"><saml:SubjectConfirmationData{irt} NotOnOrAfter="{exp}" Recipient="{recipient}"/></saml:SubjectConfirmation></saml:Subject><saml:Conditions NotBefore="{nb}" NotOnOrAfter="{exp}"><saml:AudienceRestriction><saml:Audience>{aud}</saml:Audience></saml:AudienceRestriction></saml:Conditions><saml:AuthnStatement AuthnInstant="{now_s}" SessionIndex="{si}"><saml:AuthnContext><saml:AuthnContextClassRef>urn:oasis:names:tc:SAML:2.0:ac:classes:PasswordProtectedTransport</saml:AuthnContextClassRef></saml:AuthnContext></saml:AuthnStatement>{statement}</saml:Assertion>"#,
                issuer = a.issuer,
                fmt = a.name_id_format,
                name_id = a.name_id,
                recipient = a.recipient,
                aud = a.audience,
                si = a.session_index,
            );
            if a.sign_assertion {
                assertion = self.sign(&assertion);
            }
            let body = match &a.encrypt_to {
                Some((cert, kind)) => encrypt(&assertion, cert, *kind),
                None => assertion,
            };
            let rsig = if a.sign_response {
                sig_template(&rid, a.alg)
            } else {
                String::new()
            };
            let mut response = format!(
                r#"<samlp:Response xmlns:samlp="{NS_P}" xmlns:saml="{NS_A}" ID="{rid}" Version="2.0" IssueInstant="{now_s}" Destination="{dest}"{irt}><saml:Issuer>{issuer}</saml:Issuer>{rsig}<samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status>{body}</samlp:Response>"#,
                dest = a.destination,
                issuer = a.issuer,
            );
            if a.sign_response {
                response = self.sign(&response);
            }
            if a.doctype {
                response = format!("<!DOCTYPE samlp:Response [<!ENTITY x \"x\">]>{response}");
            }
            B64.encode(response)
        }

        /// A redirect-binding query carrying `xml` as `param`, signed with
        /// this IdP's key (RSA-SHA256).
        fn signed_query(&self, param: &str, xml: &str, relay: Option<&str>) -> String {
            let mut q = format!("{param}={}", enc(&deflate_b64(xml)));
            if let Some(r) = relay {
                q.push_str(&format!("&RelayState={}", enc(r)));
            }
            q.push_str(&format!("&SigAlg={}", enc(SHA256.0)));
            let mut signer =
                openssl::sign::Signer::new(MessageDigest::sha256(), &self.pkey).unwrap();
            signer.update(q.as_bytes()).unwrap();
            let sig = signer.sign_to_vec().unwrap();
            q.push_str(&format!("&Signature={}", enc(&B64.encode(sig))));
            q
        }

        fn logout_request(
            &self,
            store: &Store,
            name_id: &str,
            session_index: Option<&str>,
        ) -> (String, String) {
            let id = format!("_l{}", uuid::Uuid::new_v4().simple());
            let si = session_index
                .map(|s| format!("<samlp:SessionIndex>{s}</samlp:SessionIndex>"))
                .unwrap_or_default();
            let xml = format!(
                r#"<samlp:LogoutRequest xmlns:samlp="{NS_P}" xmlns:saml="{NS_A}" ID="{id}" Version="2.0" IssueInstant="{}" Destination="{}"><saml:Issuer>{IDP_ENTITY}</saml:Issuer><saml:NameID Format="{PERSISTENT}">{name_id}</saml:NameID>{si}</samlp:LogoutRequest>"#,
                ts(Utc::now()),
                store.slo()
            );
            (id, xml)
        }
    }

    /// Encrypt a (signed) assertion to the SP certificate `cert_der`.
    fn encrypt(assertion: &str, cert_der: &[u8], kind: Enc) -> String {
        use openssl::symm::{encrypt_aead, Cipher, Crypter, Mode};
        let public = openssl::x509::X509::from_der(cert_der)
            .unwrap()
            .public_key()
            .unwrap();
        let (data_alg, data, key_method, wrapped) = match kind {
            Enc::Gcm => {
                let mut key = [0u8; 32];
                let mut iv = [0u8; 12];
                openssl::rand::rand_bytes(&mut key).unwrap();
                openssl::rand::rand_bytes(&mut iv).unwrap();
                let mut tag = [0u8; 16];
                let ct = encrypt_aead(
                    Cipher::aes_256_gcm(),
                    &key,
                    Some(&iv),
                    &[],
                    assertion.as_bytes(),
                    &mut tag,
                )
                .unwrap();
                let data = [iv.as_slice(), &ct, &tag].concat();
                let rsa = public.rsa().unwrap();
                let mut ek = vec![0u8; rsa.size() as usize];
                let n = rsa
                    .public_encrypt(&key, &mut ek, openssl::rsa::Padding::PKCS1_OAEP)
                    .unwrap();
                ek.truncate(n);
                (
                    "http://www.w3.org/2009/xmlenc11#aes256-gcm",
                    data,
                    r#"<xenc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#rsa-oaep-mgf1p"><ds:DigestMethod Algorithm="http://www.w3.org/2000/09/xmldsig#sha1"/></xenc:EncryptionMethod>"#,
                    ek,
                )
            }
            Enc::Cbc => {
                let mut key = [0u8; 16];
                let mut iv = [0u8; 16];
                openssl::rand::rand_bytes(&mut key).unwrap();
                openssl::rand::rand_bytes(&mut iv).unwrap();
                let mut plain = assertion.as_bytes().to_vec();
                let pad = 16 - plain.len() % 16;
                plain.extend(std::iter::repeat_n(0xAAu8, pad - 1));
                plain.push(pad as u8);
                let mut c =
                    Crypter::new(Cipher::aes_128_cbc(), Mode::Encrypt, &key, Some(&iv)).unwrap();
                c.pad(false);
                let mut out = vec![0u8; plain.len() + 16];
                let mut n = c.update(&plain, &mut out).unwrap();
                n += c.finalize(&mut out[n..]).unwrap();
                out.truncate(n);
                let data = [iv.as_slice(), &out].concat();
                let mut ctx = openssl::pkey_ctx::PkeyCtx::new(&public).unwrap();
                ctx.encrypt_init().unwrap();
                ctx.set_rsa_padding(openssl::rsa::Padding::PKCS1_OAEP)
                    .unwrap();
                ctx.set_rsa_oaep_md(openssl::md::Md::sha256()).unwrap();
                ctx.set_rsa_mgf1_md(openssl::md::Md::sha1()).unwrap();
                let mut ek = Vec::new();
                ctx.encrypt_to_vec(&key, &mut ek).unwrap();
                (
                    "http://www.w3.org/2001/04/xmlenc#aes128-cbc",
                    data,
                    r#"<xenc:EncryptionMethod Algorithm="http://www.w3.org/2009/xmlenc11#rsa-oaep"><ds:DigestMethod Algorithm="http://www.w3.org/2001/04/xmlenc#sha256"/></xenc:EncryptionMethod>"#,
                    ek,
                )
            }
        };
        format!(
            r#"<saml:EncryptedAssertion><xenc:EncryptedData xmlns:xenc="http://www.w3.org/2001/04/xmlenc#" Type="http://www.w3.org/2001/04/xmlenc#Element"><xenc:EncryptionMethod Algorithm="{data_alg}"/><ds:KeyInfo xmlns:ds="http://www.w3.org/2000/09/xmldsig#"><xenc:EncryptedKey>{key_method}<xenc:CipherData><xenc:CipherValue>{}</xenc:CipherValue></xenc:CipherData></xenc:EncryptedKey></ds:KeyInfo><xenc:CipherData><xenc:CipherValue>{}</xenc:CipherValue></xenc:CipherData></xenc:EncryptedData></saml:EncryptedAssertion>"#,
            B64.encode(wrapped),
            B64.encode(data)
        )
    }

    /// A store with one SAML provider trusting `idp`.
    struct Store {
        state: AppState,
        base: String,
        admin: String,
    }

    impl Store {
        fn new(idp: &TestIdp, config: SamlConfig) -> Self {
            Self::with_base(idp, config, None)
        }

        fn with_base(idp: &TestIdp, config: SamlConfig, base: Option<&str>) -> Self {
            // The refusal reasons go to the log; show them with a failing test.
            let _ = tracing_subscriber::fmt().with_test_writer().try_init();
            let (mut state, admin) = admin_state();
            if let Some(b) = base {
                state.base_url = Arc::new(b.to_string());
            }
            state
                .auth_db
                .create_oauth_provider(&OauthProviderCreate {
                    entity_id: Some(IDP_ENTITY.to_string()),
                    sso_url: Some(IDP_SSO.to_string()),
                    idp_certificate: Some(idp.cert_pem()),
                    discovery_url: None,
                    role_claim_map: Some(
                        r#"{"staff-admins":"admin","root":"super_admin","pub":"publisher"}"#
                            .to_string(),
                    ),
                    saml_config: Some(config),
                    ..provider_row(SLUG, "saml", None)
                })
                .unwrap();
            Self {
                base: state.base_url.to_string(),
                state,
                admin,
            }
        }

        /// A fresh router over the same state for every request: the SSO
        /// routes allow a burst of 8 requests per client and router.
        async fn send(&self, req: Request<Body>) -> axum::response::Response {
            test_app(self.state.clone()).oneshot(req).await.unwrap()
        }

        fn provider(&self) -> OauthProvider {
            self.state
                .auth_db
                .get_oauth_provider_by_slug(SLUG)
                .unwrap()
                .unwrap()
        }

        fn entity_id(&self) -> String {
            format!("{}/api/auth/saml/{SLUG}/metadata", self.base)
        }
        fn acs(&self) -> String {
            format!("{}/api/auth/saml/{SLUG}/acs", self.base)
        }
        fn slo(&self) -> String {
            format!("{}/api/auth/saml/{SLUG}/slo", self.base)
        }

        async fn get(&self, uri: &str) -> axum::response::Response {
            self.send(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
        }

        async fn admin_json(
            &self,
            method: Method,
            uri: &str,
            body: serde_json::Value,
        ) -> axum::response::Response {
            self.send(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", self.admin))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
        }

        async fn metadata(&self) -> String {
            let resp = self.get(&format!("/api/auth/saml/{SLUG}/metadata")).await;
            assert_eq!(resp.status(), StatusCode::OK);
            body_text(resp.into_body()).await
        }

        /// The certificates the metadata publishes for `usage`, in order.
        async fn published(&self, usage: &str) -> Vec<Vec<u8>> {
            let md = self.metadata().await;
            md.split(&format!("<md:KeyDescriptor use=\"{usage}\">"))
                .skip(1)
                .map(|part| {
                    let c = part
                        .split("<ds:X509Certificate>")
                        .nth(1)
                        .unwrap()
                        .split("</ds:X509Certificate>")
                        .next()
                        .unwrap();
                    B64.decode(c).unwrap()
                })
                .collect()
        }

        /// Start a sign-in the way the login button does. Returns the
        /// RelayState (also the `saml_state` cookie), the AuthnRequest's ID and
        /// XML, and the redirect URL.
        async fn start(&self) -> (String, String, String, String) {
            let resp = self.get(&format!("/api/auth/saml/{SLUG}/login")).await;
            assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
            let cookie = resp.headers()[header::SET_COOKIE].to_str().unwrap();
            let relay = cookie
                .strip_prefix("saml_state=")
                .and_then(|c| c.split(';').next())
                .unwrap()
                .to_string();
            let location = resp.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .to_string();
            assert!(location.starts_with(IDP_SSO), "{location}");
            let (_, q) = query_of(&location);
            assert_eq!(q["RelayState"], relay);
            let request = inflate_b64(&q["SAMLRequest"]);
            let id = request
                .split(" ID=\"")
                .nth(1)
                .and_then(|r| r.split('"').next())
                .unwrap()
                .to_string();
            (relay, id, request, location)
        }

        async fn post_acs(
            &self,
            saml_response: &str,
            relay: &str,
            cookie: Option<&str>,
        ) -> axum::response::Response {
            let body = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("SAMLResponse", saml_response)
                .append_pair("RelayState", relay)
                .finish();
            let mut req = Request::builder()
                .method(Method::POST)
                .uri(format!("/api/auth/saml/{SLUG}/acs"))
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            if let Some(c) = cookie {
                req = req.header(header::COOKIE, format!("saml_state={c}"));
            }
            self.send(req.body(Body::from(body)).unwrap()).await
        }

        /// Tokens from a 303 to the SPA's callback page.
        fn tokens(&self, resp: &axum::response::Response) -> HashMap<String, String> {
            assert_eq!(resp.status(), StatusCode::SEE_OTHER);
            let location = resp.headers()[header::LOCATION].to_str().unwrap();
            let fragment = location
                .strip_prefix(&format!("{}/oauth/callback#", self.base))
                .unwrap_or_else(|| panic!("unexpected landing {location}"));
            url::form_urlencoded::parse(fragment.as_bytes())
                .into_owned()
                .collect()
        }

        /// A full SP-initiated sign-in with `edit` applied to the answer.
        async fn sign_in_with(
            &self,
            idp: &TestIdp,
            name_id: &str,
            edit: impl FnOnce(&mut Answer),
        ) -> axum::response::Response {
            let (relay, id, _, _) = self.start().await;
            let mut answer = idp.answer(self, Some(&id), name_id);
            edit(&mut answer);
            self.post_acs(&idp.respond(&answer), &relay, Some(&relay))
                .await
        }

        async fn sign_in(&self, idp: &TestIdp, name_id: &str) -> HashMap<String, String> {
            let resp = self.sign_in_with(idp, name_id, |_| {}).await;
            self.tokens(&resp)
        }

        /// Rotate `refresh_token`: the new refresh token, or `None` when the
        /// session is over.
        async fn refresh(&self, refresh_token: &str) -> Option<String> {
            let resp = self
                .send(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/auth/refresh")
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({ "refresh_token": refresh_token }).to_string(),
                        ))
                        .unwrap(),
                )
                .await;
            if resp.status() != StatusCode::OK {
                return None;
            }
            body_json(resp.into_body()).await["refresh_token"]
                .as_str()
                .map(str::to_string)
        }

        fn claims(&self, access: &str) -> open_triplestore::auth::jwt::Claims {
            verify_token(&self.state.jwt_config, access).unwrap()
        }
    }

    // ── Sign-in started here ─────────────────────────────────────────────────

    /// The whole browser round trip: login button → IdP → ACS → the SPA's
    /// callback page, signed in as the asserted user, with groups mapped to a
    /// role and the publisher grant. The request asks for a persistent NameID,
    /// and the response cannot be presented a second time.
    #[tokio::test]
    async fn sp_initiated_sign_in_maps_attributes_and_cannot_be_replayed() {
        let idp = TestIdp::new();
        let store = Store::new(&idp, SamlConfig::default());

        let (relay, id, request, _) = store.start().await;
        assert!(request.contains(&store.acs()) && request.contains(&store.entity_id()));
        assert!(
            request.contains(&format!("Format=\"{PERSISTENT}\"")),
            "AuthnRequest asks for a persistent NameID: {request}"
        );
        assert!(
            id.starts_with('_') && id.len() == 33,
            "128-bit request ID: {id}"
        );

        let mut answer = idp.answer(&store, Some(&id), "alice");
        answer.attributes.push((
            "http://schemas.microsoft.com/ws/2008/06/identity/claims/groups".to_string(),
            vec!["staff-admins".to_string(), "pub".to_string()],
        ));
        answer.attributes.push((
            "urn:oid:2.16.840.1.113730.3.1.241".to_string(),
            vec!["Alice Example".to_string()],
        ));
        let response = idp.respond(&answer);
        let resp = store.post_acs(&response, &relay, Some(&relay)).await;
        let cleared = resp.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .to_string();
        assert!(cleared.starts_with("saml_state=;") && cleared.contains("Max-Age=0"));
        let tokens = store.tokens(&resp);
        let claims = store.claims(&tokens["access_token"]);
        assert_eq!(claims.role, "admin");
        let user = store
            .state
            .auth_db
            .get_user_by_id(&claims.sub)
            .unwrap()
            .unwrap();
        assert_eq!(user.email, "alice@example.org");
        assert!(user.can_publish, "the `pub` group grants publishing");

        // Replay: the request it answered has been used up.
        let resp = store.post_acs(&response, &relay, Some(&relay)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // A later sign-in finds the same account (persistent NameID).
        let again = store.sign_in(&idp, "alice").await;
        assert_eq!(store.claims(&again["access_token"]).sub, claims.sub);
    }

    /// The ACS refuses a response that is not bound to a request this browser
    /// started: missing or foreign `saml_state` cookie (login CSRF), an
    /// unknown RelayState (IdP-initiated or forged), and a response answering a
    /// different request.
    #[tokio::test]
    async fn acs_refuses_responses_not_bound_to_this_browsers_request() {
        let idp = TestIdp::new();
        let store = Store::new(&idp, SamlConfig::default());

        let (relay, id, _, _) = store.start().await;
        let good = idp.respond(&idp.answer(&store, Some(&id), "alice"));
        assert_eq!(
            store.post_acs(&good, &relay, None).await.status(),
            StatusCode::BAD_REQUEST
        );
        let (other_relay, _, _, _) = store.start().await;
        assert_eq!(
            store
                .post_acs(&good, &relay, Some(&other_relay))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            store
                .post_acs(&good, "made-up", Some("made-up"))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );

        // A validly signed response to some other request.
        let resp = store
            .sign_in_with(&idp, "alice", |a| {
                a.in_response_to = Some("_another".into())
            })
            .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            body_json(resp.into_body()).await["error"],
            "SAML sign-in failed"
        );
    }

    /// Every check on the response itself: signature present, by a trusted key,
    /// with SHA-256 or stronger; audience, destination and recipient are ours;
    /// not expired; no DOCTYPE; and a transient NameID is refused.
    #[tokio::test]
    async fn acs_refuses_unsigned_forged_weak_misaddressed_expired_and_doctype_responses() {
        let idp = TestIdp::new();
        let store = Store::new(&idp, SamlConfig::default());
        let stranger = TestIdp::new();

        type Edit = fn(&mut Answer);
        let cases: [(&str, Edit); 9] = [
            ("unsigned", |a: &mut Answer| a.sign_assertion = false),
            ("sha1", |a: &mut Answer| a.alg = SHA1),
            ("wrong audience", |a: &mut Answer| {
                a.audience = "https://other.example.org/sp".into()
            }),
            ("wrong destination", |a: &mut Answer| {
                a.destination = "https://other.example.org/acs".into();
                a.sign_response = true;
            }),
            ("wrong recipient", |a: &mut Answer| {
                a.recipient = "https://other.example.org/acs".into()
            }),
            ("expired", |a: &mut Answer| {
                a.expires_in = Duration::minutes(-30)
            }),
            ("wrong issuer", |a: &mut Answer| {
                a.issuer = "https://evil.example.org".into()
            }),
            ("doctype", |a: &mut Answer| a.doctype = true),
            ("transient", |a: &mut Answer| {
                a.name_id_format = TRANSIENT.into()
            }),
        ];
        for (name, edit) in cases {
            let resp = store.sign_in_with(&idp, "alice", edit).await;
            assert_eq!(
                resp.status(),
                StatusCode::UNAUTHORIZED,
                "{name} must be refused"
            );
        }
        // Signed by a key the provider does not trust.
        let (relay, id, _, _) = store.start().await;
        let forged = stranger.respond(&stranger.answer(&store, Some(&id), "alice"));
        assert_eq!(
            store.post_acs(&forged, &relay, Some(&relay)).await.status(),
            StatusCode::UNAUTHORIZED
        );
        // None of them created an account.
        assert!(store
            .state
            .auth_db
            .get_user_by_email("alice@example.org")
            .unwrap()
            .is_none());
    }

    /// Sign-in through SAML never grants super_admin, whatever the mapping says.
    #[tokio::test]
    async fn a_super_admin_mapping_is_capped_at_admin() {
        let idp = TestIdp::new();
        let store = Store::new(&idp, SamlConfig::default());
        let resp = store
            .sign_in_with(&idp, "root-user", |a| {
                a.attributes.push(("groups".into(), vec!["root".into()]))
            })
            .await;
        let tokens = store.tokens(&resp);
        assert_eq!(store.claims(&tokens["access_token"]).role, "admin");
    }

    /// A transient NameID is fine when an attribute identifies the account.
    #[tokio::test]
    async fn a_subject_attribute_identifies_accounts_behind_a_transient_name_id() {
        let idp = TestIdp::new();
        let store = Store::new(
            &idp,
            SamlConfig {
                name_id_format: Some(TRANSIENT.into()),
                subject_attribute: Some("urn:oid:1.3.6.1.4.1.5923.1.1.1.6".into()),
                ..Default::default()
            },
        );
        let mut subs = Vec::new();
        for n in ["t-1", "t-2"] {
            let resp = store
                .sign_in_with(&idp, n, |a| {
                    a.name_id_format = TRANSIENT.into();
                    a.attributes.push((
                        "urn:oid:1.3.6.1.4.1.5923.1.1.1.6".into(),
                        vec!["bob@example.org".into()],
                    ));
                    a.attributes[0].1 = vec!["bob@example.org".into()];
                })
                .await;
            subs.push(store.claims(&store.tokens(&resp)["access_token"]).sub);
        }
        assert_eq!(subs[0], subs[1], "two transient NameIDs, one account");
    }

    // ── Sign-in started at the IdP ───────────────────────────────────────────

    /// IdP-initiated responses are refused by default. When the provider
    /// allows them, one is accepted once (replay cache), and one that claims
    /// to answer a request is refused.
    #[tokio::test]
    async fn idp_initiated_sign_in_is_off_by_default_and_single_use_when_on() {
        let idp = TestIdp::new();
        let store = Store::new(&idp, SamlConfig::default());
        let unsolicited = idp.respond(&idp.answer(&store, None, "carol"));
        assert_eq!(
            store.post_acs(&unsolicited, "", None).await.status(),
            StatusCode::BAD_REQUEST
        );

        let store = Store::new(
            &idp,
            SamlConfig {
                allow_idp_initiated: true,
                ..Default::default()
            },
        );
        let unsolicited = idp.respond(&idp.answer(&store, None, "carol"));
        let resp = store.post_acs(&unsolicited, "", None).await;
        store.tokens(&resp);
        let resp = store.post_acs(&unsolicited, "", None).await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "replayed assertion"
        );

        let claims_request = idp.respond(&idp.answer(&store, Some("_captured"), "carol"));
        assert_eq!(
            store.post_acs(&claims_request, "", None).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    // ── Transport ────────────────────────────────────────────────────────────

    /// Over https the binding cookie is `SameSite=None; Secure` (the IdP posts
    /// back cross-site), whatever SECURE_COOKIES says; a plain-http BASE_URL
    /// other than localhost refuses to start a sign-in.
    #[tokio::test]
    async fn saml_needs_https_outside_localhost() {
        let idp = TestIdp::new();
        let store = Store::with_base(
            &idp,
            SamlConfig::default(),
            Some("https://data.example.org"),
        );
        let resp = store.get(&format!("/api/auth/saml/{SLUG}/login")).await;
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        let cookie = resp.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(cookie.contains("SameSite=None; Secure"), "{cookie}");

        let store = Store::with_base(&idp, SamlConfig::default(), Some("http://data.example.org"));
        let resp = store.get(&format!("/api/auth/saml/{SLUG}/login")).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// A provider without an IdP signing certificate never starts a sign-in:
    /// samael would accept unsigned responses.
    #[tokio::test]
    async fn a_provider_without_an_idp_certificate_does_not_start_a_sign_in() {
        let idp = TestIdp::new();
        let store = Store::new(&idp, SamlConfig::default());
        let mut p = store.provider();
        p.idp_certificate = None;
        let body = OauthProviderCreate {
            name: p.name,
            slug: p.slug,
            provider_type: p.provider_type,
            client_id: None,
            client_secret: None,
            client_secret_enc: None,
            discovery_url: None,
            tenant_id: None,
            entity_id: p.entity_id,
            sso_url: p.sso_url,
            idp_certificate: None,
            scopes: None,
            role_claim_map: None,
            auto_provision: true,
            default_role: Some("user".to_string()),
            is_active: true,
            saml_config: None,
        };
        store
            .state
            .auth_db
            .update_oauth_provider(&p.id, &body)
            .unwrap();
        let resp = store.get(&format!("/api/auth/saml/{SLUG}/login")).await;
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    // ── Encryption, signed requests, keys ────────────────────────────────────

    /// The metadata publishes our key for encryption with `use="encryption"`
    /// (not samael's mislabelled `signing`), and an assertion encrypted to it
    /// is accepted — AES-GCM and AES-CBC, signed inside or with the whole
    /// response signed. An assertion encrypted to another key is refused, and
    /// `require_encrypted_assertions` refuses a plain one.
    #[tokio::test]
    async fn encrypted_assertions_decrypt_with_the_published_key() {
        let idp = TestIdp::new();
        let store = Store::new(&idp, SamlConfig::default());
        let md = store.metadata().await;
        assert!(md.contains(&format!("entityID=\"{}\"", store.entity_id())));
        assert!(md.contains(&format!("Location=\"{}\"", store.slo())));
        assert!(
            !md.contains("localhost:8080"),
            "no samael default endpoints"
        );
        assert!(md.contains(&format!("<md:NameIDFormat>{PERSISTENT}</md:NameIDFormat>")));
        let enc_cert = store.published("encryption").await.remove(0);
        assert_eq!(store.published("signing").await[0], enc_cert);

        for (name, kind, sign_response, sign_assertion) in [
            ("gcm", Enc::Gcm, false, true),
            ("cbc", Enc::Cbc, false, true),
            ("signed response", Enc::Gcm, true, false),
        ] {
            let resp = store
                .sign_in_with(&idp, "dave", |a| {
                    a.encrypt_to = Some((enc_cert.clone(), kind));
                    a.sign_response = sign_response;
                    a.sign_assertion = sign_assertion;
                })
                .await;
            assert_eq!(resp.status(), StatusCode::SEE_OTHER, "{name}");
        }
        // Encrypted, but neither the response nor the assertion is signed.
        let resp = store
            .sign_in_with(&idp, "dave", |a| {
                a.encrypt_to = Some((enc_cert.clone(), Enc::Gcm));
                a.sign_assertion = false;
            })
            .await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "unsigned encrypted"
        );

        // Encrypted to a key this store does not hold.
        let resp = store
            .sign_in_with(&idp, "dave", |a| {
                a.encrypt_to = Some((idp.cert.clone(), Enc::Gcm))
            })
            .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let strict = Store::new(
            &idp,
            SamlConfig {
                require_encrypted_assertions: true,
                ..Default::default()
            },
        );
        let resp = strict.sign_in_with(&idp, "dave", |_| {}).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// With `sign_authn_requests`, the AuthnRequest redirect carries an
    /// RSA-SHA256 signature by the published signing key, and the metadata says
    /// so. Key rollover: a second key is published (and decrypts) before it
    /// signs; once activated it signs; the signing key cannot be deleted, the
    /// retired one can.
    #[tokio::test]
    async fn signed_authn_requests_and_sp_key_rollover() {
        let idp = TestIdp::new();
        let store = Store::new(
            &idp,
            SamlConfig {
                sign_authn_requests: true,
                ..Default::default()
            },
        );
        assert!(store
            .metadata()
            .await
            .contains("AuthnRequestsSigned=\"true\""));
        let first = store.published("signing").await.remove(0);
        let (_, _, _, location) = store.start().await;
        assert!(redirect_signature_verifies(
            &location,
            "SAMLRequest",
            &first
        ));

        let id = store.provider().id;
        let resp = store
            .admin_json(
                Method::POST,
                &format!("/api/admin/oauth/providers/{id}/saml/keys"),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let new_kid = body_json(resp.into_body()).await["kid"]
            .as_str()
            .unwrap()
            .to_string();
        let published = store.published("encryption").await;
        assert_eq!(published.len(), 2);
        let second = published[1].clone();
        assert_eq!(published[0], first, "the current key stays first");

        // An IdP that already picked up the new key encrypts to it.
        let resp = store
            .sign_in_with(&idp, "erin", |a| {
                a.encrypt_to = Some((second.clone(), Enc::Gcm))
            })
            .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let (_, _, _, location) = store.start().await;
        assert!(redirect_signature_verifies(
            &location,
            "SAMLRequest",
            &first
        ));

        let resp = store
            .admin_json(
                Method::POST,
                &format!("/api/admin/oauth/providers/{id}/saml/keys/{new_kid}/activate"),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let (_, _, _, location) = store.start().await;
        assert!(redirect_signature_verifies(
            &location,
            "SAMLRequest",
            &second
        ));

        let resp = store
            .admin_json(
                Method::DELETE,
                &format!("/api/admin/oauth/providers/{id}/saml/keys/{new_kid}"),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::CONFLICT, "the signing key stays");
        let overview = body_json(
            store
                .admin_json(
                    Method::GET,
                    &format!("/api/admin/oauth/providers/{id}/saml"),
                    serde_json::json!({}),
                )
                .await
                .into_body(),
        )
        .await;
        let old_kid = overview["sp_keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["is_current"] == false)
            .unwrap()["kid"]
            .as_str()
            .unwrap()
            .to_string();
        let resp = store
            .admin_json(
                Method::DELETE,
                &format!("/api/admin/oauth/providers/{id}/saml/keys/{old_kid}"),
                serde_json::json!({}),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert_eq!(store.published("encryption").await, vec![second]);
    }

    // ── Admin: metadata import and validation ────────────────────────────────

    /// Pasted IdP metadata yields the entity ID, SSO and SLO URLs and every
    /// signing certificate (two, for a rollover); the provider then accepts
    /// responses signed with either key. Unusable settings are refused on save.
    #[tokio::test]
    async fn idp_metadata_import_trusts_every_signing_certificate() {
        let old = TestIdp::new();
        let new = TestIdp::new();
        let store = Store::new(&old, SamlConfig::default());
        let metadata = format!(
            r#"<?xml version="1.0"?><md:EntityDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata" xmlns:ds="http://www.w3.org/2000/09/xmldsig#" entityID="{IDP_ENTITY}"><md:IDPSSODescriptor WantAuthnRequestsSigned="true" protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol"><md:KeyDescriptor use="signing"><ds:KeyInfo><ds:X509Data><ds:X509Certificate>{}</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor><md:KeyDescriptor><ds:KeyInfo><ds:X509Data><ds:X509Certificate>{}</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor><md:KeyDescriptor use="encryption"><ds:KeyInfo><ds:X509Data><ds:X509Certificate>{}</ds:X509Certificate></ds:X509Data></ds:KeyInfo></md:KeyDescriptor><md:SingleLogoutService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect" Location="{IDP_SLO}"/><md:SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-POST" Location="https://idp.example.org/post"/><md:SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect" Location="{IDP_SSO}"/></md:IDPSSODescriptor></md:EntityDescriptor>"#,
            B64.encode(&old.cert),
            B64.encode(&new.cert),
            B64.encode(&TestIdp::new().cert),
        );
        let resp = store
            .admin_json(
                Method::POST,
                "/api/admin/oauth/saml-metadata",
                serde_json::json!({ "xml": metadata }),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let md = body_json(resp.into_body()).await;
        assert_eq!(md["entity_id"], IDP_ENTITY);
        assert_eq!(md["sso_url"], IDP_SSO);
        assert_eq!(md["slo_url"], IDP_SLO);
        assert_eq!(md["want_authn_requests_signed"], true);
        assert_eq!(
            md["certificates"].as_array().unwrap().len(),
            2,
            "signing keys only"
        );

        // Save what the import returned, as the admin form does.
        let id = store.provider().id;
        let mut body = serde_json::json!({
            "name": "Corp", "slug": SLUG, "provider_type": "saml",
            "entity_id": md["entity_id"], "sso_url": md["sso_url"],
            "idp_certificate": md["certificates_pem"],
            "auto_provision": true, "is_active": true, "default_role": "user",
            "saml_config": { "idp_slo_url": md["slo_url"] },
        });
        let resp = store
            .admin_json(
                Method::PUT,
                &format!("/api/admin/oauth/providers/{id}"),
                body.clone(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        for idp in [&old, &new] {
            let resp = store.sign_in_with(idp, "frank", |_| {}).await;
            assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        }

        // Refused on save: a certificate that is not one, a transient NameID
        // without a subject attribute, an http SLO URL, a skew over 10 minutes.
        for (field, value) in [
            ("idp_certificate", serde_json::json!("not a certificate")),
            (
                "saml_config",
                serde_json::json!({ "name_id_format": TRANSIENT }),
            ),
            (
                "saml_config",
                serde_json::json!({ "idp_slo_url": "http://idp.example.org/slo" }),
            ),
            (
                "saml_config",
                serde_json::json!({ "clock_skew_seconds": 3600 }),
            ),
        ] {
            let mut bad = body.clone();
            bad[field] = value;
            let resp = store
                .admin_json(
                    Method::PUT,
                    &format!("/api/admin/oauth/providers/{id}"),
                    bad,
                )
                .await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "{field}");
        }
        // An aggregate with two IdPs is refused rather than guessed at.
        body = serde_json::json!({ "xml": format!(
            r#"<md:EntitiesDescriptor xmlns:md="urn:oasis:names:tc:SAML:2.0:metadata">{m}{m}</md:EntitiesDescriptor>"#,
            m = strip_decl(&metadata)
        )});
        let resp = store
            .admin_json(Method::POST, "/api/admin/oauth/saml-metadata", body)
            .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Single Logout ────────────────────────────────────────────────────────

    fn slo_config() -> SamlConfig {
        SamlConfig {
            idp_slo_url: Some(IDP_SLO.to_string()),
            ..Default::default()
        }
    }

    /// Signing out here revokes the SAML session's refresh-token family and
    /// sends the browser to the IdP with a signed LogoutRequest naming the
    /// NameID and SessionIndex; the IdP's signed LogoutResponse lands it home.
    #[tokio::test]
    async fn sp_initiated_logout_revokes_the_session_and_tells_the_idp() {
        let idp = TestIdp::new();
        let store = Store::new(&idp, slo_config());
        let tokens = store.sign_in(&idp, "gina").await;

        let resp = store
            .send(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/auth/logout")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "refresh_token": tokens["refresh_token"] }).to_string(),
                    ))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let url = body_json(resp.into_body()).await["saml_logout_url"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(url.starts_with(IDP_SLO));
        let signing = store.published("signing").await.remove(0);
        assert!(redirect_signature_verifies(&url, "SAMLRequest", &signing));
        let (_, q) = query_of(&url);
        let request = inflate_b64(&q["SAMLRequest"]);
        assert!(request.contains(">gina</saml:NameID>"), "{request}");
        assert!(request.contains("<samlp:SessionIndex>_s-gina</samlp:SessionIndex>"));
        assert!(store.refresh(&tokens["refresh_token"]).await.is_none());

        // The IdP answers.
        let request_id = request
            .split(" ID=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let answer = format!(
            r#"<samlp:LogoutResponse xmlns:samlp="{NS_P}" xmlns:saml="{NS_A}" ID="_lr1" Version="2.0" IssueInstant="{}" Destination="{}" InResponseTo="{request_id}"><saml:Issuer>{IDP_ENTITY}</saml:Issuer><samlp:Status><samlp:StatusCode Value="urn:oasis:names:tc:SAML:2.0:status:Success"/></samlp:Status></samlp:LogoutResponse>"#,
            ts(Utc::now()),
            store.slo()
        );
        let resp = store
            .get(&format!(
                "/api/auth/saml/{SLUG}/slo?{}",
                idp.signed_query("SAMLResponse", &answer, None)
            ))
            .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers()[header::LOCATION], format!("{}/", store.base));
    }

    /// A signed LogoutRequest from the IdP revokes the named session only
    /// (by SessionIndex), answers with a signed LogoutResponse, and cannot be
    /// replayed. Unsigned, forged or misissued requests are refused and revoke
    /// nothing. The HTTP-POST binding works the same way.
    #[tokio::test]
    async fn idp_initiated_logout_revokes_the_named_session_only() {
        let idp = TestIdp::new();
        let store = Store::new(&idp, slo_config());
        let first = store.sign_in(&idp, "hank").await;
        let second = store.tokens(
            &store
                .sign_in_with(&idp, "hank", |a| a.session_index = "_s-other".into())
                .await,
        );

        // Refused: unsigned, signed by a stranger, issued by someone else.
        let (_, xml) = idp.logout_request(&store, "hank", Some("_s-hank"));
        let unsigned = format!("SAMLRequest={}", enc(&deflate_b64(&xml)));
        let forged = TestIdp::new().signed_query("SAMLRequest", &xml, None);
        let (_, other_issuer) = idp.logout_request(&store, "hank", Some("_s-hank"));
        let other_issuer = idp.signed_query(
            "SAMLRequest",
            &other_issuer.replace(IDP_ENTITY, "https://evil.example.org"),
            None,
        );
        for query in [unsigned, forged, other_issuer] {
            let resp = store
                .get(&format!("/api/auth/saml/{SLUG}/slo?{query}"))
                .await;
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }
        let first_rt = store
            .refresh(&first["refresh_token"])
            .await
            .expect("still signed in");

        let (id, xml) = idp.logout_request(&store, "hank", Some("_s-hank"));
        let query = idp.signed_query("SAMLRequest", &xml, Some("idp-relay"));
        let resp = store
            .get(&format!("/api/auth/saml/{SLUG}/slo?{query}"))
            .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_string();
        assert!(location.starts_with(IDP_SLO));
        let signing = store.published("signing").await.remove(0);
        assert!(redirect_signature_verifies(
            &location,
            "SAMLResponse",
            &signing
        ));
        let (_, q) = query_of(&location);
        assert_eq!(q["RelayState"], "idp-relay");
        let answer = inflate_b64(&q["SAMLResponse"]);
        assert!(answer.contains(&format!("InResponseTo=\"{id}\"")));
        assert!(answer.contains("status:Success"));

        // The first session is gone, the second lives; the request is spent.
        assert!(store.refresh(&first_rt).await.is_none());
        let second_rt = store
            .refresh(&second["refresh_token"])
            .await
            .expect("the other session lives");
        let resp = store
            .get(&format!("/api/auth/saml/{SLUG}/slo?{query}"))
            .await;
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "replayed LogoutRequest"
        );

        // HTTP-POST binding, no SessionIndex: every session of the subject.
        let (_, xml) = idp.logout_request(&store, "hank", None);
        let id = xml
            .split(" ID=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap()
            .to_string();
        let signed = idp.sign(&xml.replacen(
            &format!("<saml:Issuer>{IDP_ENTITY}</saml:Issuer>"),
            &format!(
                "<saml:Issuer>{IDP_ENTITY}</saml:Issuer>{}",
                sig_template(&id, SHA256)
            ),
            1,
        ));
        let body = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("SAMLRequest", &B64.encode(signed))
            .finish();
        let resp = store
            .send(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/auth/saml/{SLUG}/slo"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert!(store.refresh(&second_rt).await.is_none());
    }
}
