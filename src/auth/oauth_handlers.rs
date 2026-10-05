//! HTTP handlers for OAuth/SSO provider management and the OIDC/SAML flows.

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Extension, Json,
};
use serde::{Deserialize, Serialize};

use super::audit::{self, AuditEventBuilder, AuditEventType, AuditOutcome};
use super::middleware::AuthenticatedUser;
use super::models::OauthProviderCreate;
use super::oauth::{begin_oidc_flow, complete_oidc_flow, OAuthSessions};
use super::saml::{
    base_url_is_https, begin_saml_flow, complete_saml_flow, generate_sp_metadata, handle_slo,
    take_pending_request, verify_saml_response, SloBinding, SloOutcome,
};
use super::secret::store_configured_secret;
use crate::server::client_ip::ClientIp;
use crate::server::AppState;

// ─── Public provider listing (for login UI) ────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PublicProvider {
    pub slug: String,
    pub name: String,
    pub provider_type: String,
}

/// GET /api/auth/oauth/providers
/// Returns the SSO providers a sign-in can start from (no secrets) for the
/// login UI. Active entries without a client ID, such as `env-oidc`, are left
/// out: they still serve IdP bearer tokens, which never consult this list.
pub async fn list_active_providers(State(state): State<AppState>) -> impl IntoResponse {
    match state.auth_db.list_oauth_providers(true) {
        Ok(providers) => {
            let public: Vec<PublicProvider> = providers
                .into_iter()
                .filter(|p| p.offers_login())
                .map(|p| PublicProvider {
                    slug: p.slug,
                    name: p.name,
                    provider_type: p.provider_type,
                })
                .collect();
            Json(public).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

// ─── Admin: provider CRUD ─────────────────────────────────────────────────────

/// GET /api/admin/oauth/providers
pub async fn admin_list_providers(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
) -> impl IntoResponse {
    match state.auth_db.list_oauth_providers(false) {
        Ok(providers) => {
            // Redact secrets before responding
            let safe: Vec<_> = providers
                .into_iter()
                .map(|mut p| {
                    p.client_secret_enc = None;
                    p.idp_certificate = None;
                    p
                })
                .collect();
            Json(safe).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// Map a secret-storage failure to a response. A refused raw value or a bad
/// reference is the caller's problem; a cipher failure is ours. The message
/// is the error's own text, which never contains the secret.
fn secret_error(e: &anyhow::Error) -> axum::response::Response {
    use crate::secrets::SecretError;
    let status = match e.downcast_ref::<SecretError>() {
        Some(_) => StatusCode::BAD_REQUEST,
        None => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(serde_json::json!({ "error": e.to_string() }))).into_response()
}

/// Refuse SAML settings or certificates the sign-in could not use.
// A `Response` error keeps the handlers short; these run once per request.
#[allow(clippy::result_large_err)]
fn check_saml_fields(body: &OauthProviderCreate) -> Result<(), Response> {
    if body.provider_type != "saml" {
        return Ok(());
    }
    super::saml::validate_provider_fields(
        body.idp_certificate.as_deref(),
        body.saml_config.as_ref(),
    )
    .map_err(|msg| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response()
    })
}

/// POST /api/admin/oauth/providers
pub async fn admin_create_provider(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Json(mut body): Json<OauthProviderCreate>,
) -> impl IntoResponse {
    if let Err(r) = check_saml_fields(&body) {
        return r;
    }
    // A secret reference is stored as itself; a raw value is refused in the
    // production posture and encrypted (deprecated) elsewhere.
    if let Some(supplied) = body.client_secret.take() {
        match store_configured_secret("client_secret", &supplied, &state.jwt_config.secret) {
            Ok(stored) => body.client_secret_enc = Some(stored),
            Err(e) => return secret_error(&e),
        }
    }
    match state.auth_db.create_oauth_provider(&body) {
        Ok(mut p) => {
            p.client_secret_enc = None;
            (StatusCode::CREATED, Json(p)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// GET /api/admin/oauth/providers/:id
pub async fn admin_get_provider(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.auth_db.get_oauth_provider_by_id(&id) {
        Ok(Some(mut p)) => {
            p.client_secret_enc = None;
            p.idp_certificate = None;
            Json(p).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, "{\"error\":\"Provider not found\"}").into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// PUT /api/admin/oauth/providers/:id
pub async fn admin_update_provider(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    Json(mut body): Json<OauthProviderCreate>,
) -> impl IntoResponse {
    if let Err(r) = check_saml_fields(&body) {
        return r;
    }
    // Only re-store when a new secret was supplied.
    if let Some(supplied) = body.client_secret.take() {
        match store_configured_secret("client_secret", &supplied, &state.jwt_config.secret) {
            Ok(stored) => body.client_secret_enc = Some(stored),
            Err(e) => return secret_error(&e),
        }
    } else if body.client_secret_enc.is_none() {
        // Preserve existing encrypted secret
        if let Ok(Some(existing)) = state.auth_db.get_oauth_provider_by_id(&id) {
            body.client_secret_enc = existing.client_secret_enc;
        }
    }
    // The IdP certificate is redacted from every read, so a client editing the
    // provider cannot send it back. Absent means "keep the stored one".
    if body.idp_certificate.is_none() {
        if let Ok(Some(existing)) = state.auth_db.get_oauth_provider_by_id(&id) {
            body.idp_certificate = existing.idp_certificate;
        }
    }
    match state.auth_db.update_oauth_provider(&id, &body) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

/// DELETE /api/admin/oauth/providers/:id
pub async fn admin_delete_provider(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.auth_db.delete_oauth_provider(&id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
}

// ─── SSO audit helpers ─────────────────────────────────────────────────────────

/// Record a successful SSO login. Mirrors the password `login` handler's
/// `LoginSuccess` event (see `src/auth/handlers.rs`), enriched with the SSO
/// provider type + slug. The actor (user id / username / role) is recovered by
/// verifying the just-issued access token against our own signing key, so the
/// audit row attributes the login to the provisioned/linked local user without
/// the flow having to thread the `User` back out.
fn audit_sso_login_success(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    client_ip: ClientIp,
    provider_type: &str,
    slug: &str,
    access_token: &str,
) {
    let mut b = AuditEventBuilder::new(AuditEventType::LoginSuccess, AuditOutcome::Success)
        .details(serde_json::json!({
            "auth_method": "sso",
            "provider_type": provider_type,
            "provider_slug": slug,
        }));
    if let Ok(claims) = crate::auth::jwt::verify_token(&state.jwt_config, access_token) {
        b = b.actor(claims.sub, claims.username, claims.role);
    }
    b.ip_address = client_ip.as_string();
    b.user_agent = audit::user_agent(headers);
    b.request_id = audit::request_id_from_headers(headers);
    state.audit.log(b);
}

/// Record a failed SSO login (callback/assertion rejected). Mirrors the password
/// `login` handler's `LoginFailure` event; the actor is unknown at this point, so
/// only the provider type/slug and a redacted reason are recorded.
fn audit_sso_login_failure(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    client_ip: ClientIp,
    provider_type: &str,
    slug: &str,
    reason: &str,
) {
    let mut b = AuditEventBuilder::new(AuditEventType::LoginFailure, AuditOutcome::Failure)
        .details(serde_json::json!({
            "auth_method": "sso",
            "provider_type": provider_type,
            "provider_slug": slug,
            "reason": reason,
        }));
    b.ip_address = client_ip.as_string();
    b.user_agent = audit::user_agent(headers);
    b.request_id = audit::request_id_from_headers(headers);
    state.audit.log(b);
}

// ─── OIDC flow ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OidcCallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// GET /api/auth/oauth/:slug/authorize
/// Initiates the OIDC login flow by redirecting to the IdP.
pub async fn oidc_authorize(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    axum::extract::Extension(sessions): axum::extract::Extension<OAuthSessions>,
) -> Response {
    let provider = match state.auth_db.get_oauth_provider_by_slug(&slug) {
        Ok(Some(p)) if p.provider_type == "oidc" && p.offers_login() => p,
        Ok(_) => {
            return (StatusCode::NOT_FOUND, "{\"error\":\"Provider not found\"}").into_response()
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{{\"error\":\"{e}\"}}"),
            )
                .into_response()
        }
    };

    let redirect_uri = format!("{}/api/auth/oauth/{}/callback", state.base_url, slug);

    match begin_oidc_flow(
        &provider,
        &sessions,
        &redirect_uri,
        &state.jwt_config.secret,
    )
    .await
    {
        Ok((auth_url, state_key)) => {
            // Login-CSRF defense: bind the flow's `state` to THIS browser via a
            // short-lived cookie that the callback must echo. SameSite=Lax so it is
            // still sent on the top-level GET navigation back from the IdP.
            let secure = if state.secure_cookies { "; Secure" } else { "" };
            let cookie = format!(
                "oauth_state={state_key}; HttpOnly; SameSite=Lax; Path=/api/auth/oauth; Max-Age=600{secure}"
            );
            (
                [(axum::http::header::SET_COOKIE, cookie)],
                Redirect::temporary(&auth_url),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("OIDC flow error for '{}': {e}", slug);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{{\"error\":\"{e}\"}}"),
            )
                .into_response()
        }
    }
}

/// GET /api/auth/oauth/:slug/callback
/// Handles the OIDC authorization code callback.
pub async fn oidc_callback(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    axum::extract::Extension(sessions): axum::extract::Extension<OAuthSessions>,
    client_ip: ClientIp,
    headers: axum::http::HeaderMap,
    Query(params): Query<OidcCallbackParams>,
) -> Response {
    if let Some(err) = params.error {
        let desc = params.error_description.as_deref().unwrap_or("");
        audit_sso_login_failure(
            &state,
            &headers,
            client_ip,
            "oidc",
            &slug,
            &format!("idp_error:{err}"),
        );
        return (
            StatusCode::BAD_REQUEST,
            format!("{{\"error\":\"{err}\",\"error_description\":\"{desc}\"}}"),
        )
            .into_response();
    }

    let code = match params.code {
        Some(c) => c,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "{\"error\":\"Missing code parameter\"}",
            )
                .into_response()
        }
    };
    let state_key = match params.state {
        Some(s) => s,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "{\"error\":\"Missing state parameter\"}",
            )
                .into_response()
        }
    };

    // Login-CSRF defense: the IdP-returned `state` must match the binding cookie
    // set on this browser when the flow began (see oidc_authorize). This stops an
    // attacker from delivering a pre-captured (code, state) pair to a victim to log
    // them into the attacker's identity.
    let cookie_state = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|c| {
            c.split(';')
                .find_map(|p| p.trim().strip_prefix("oauth_state=").map(str::to_string))
        });
    if cookie_state.as_deref() != Some(state_key.as_str()) {
        audit_sso_login_failure(
            &state,
            &headers,
            client_ip,
            "oidc",
            &slug,
            "invalid_state_binding",
        );
        return (
            StatusCode::BAD_REQUEST,
            "{\"error\":\"Invalid or missing state binding\"}",
        )
            .into_response();
    }

    let redirect_uri = format!("{}/api/auth/oauth/{}/callback", state.base_url, slug);

    match complete_oidc_flow(
        &code,
        &state_key,
        &sessions,
        &state.auth_db,
        &state.jwt_config,
        &redirect_uri,
        &state.base_url,
    )
    .await
    {
        Ok((access, refresh)) => {
            audit_sso_login_success(&state, &headers, client_ip, "oidc", &slug, &access);
            // M-3: redirect to the SPA with tokens in the URL fragment (never server-logged).
            Redirect::to(&sso_landing_url(&state.base_url, &access, &refresh)).into_response()
        }
        Err(e) => {
            tracing::error!("OIDC callback error for '{}': {e}", slug);
            audit_sso_login_failure(
                &state,
                &headers,
                client_ip,
                "oidc",
                &slug,
                "code_exchange_failed",
            );
            (StatusCode::UNAUTHORIZED, format!("{{\"error\":\"{e}\"}}")).into_response()
        }
    }
}

/// Where a completed SSO sign-in sends the browser: the SPA's
/// `/oauth/callback` page (`OAuthCallback.svelte`), which reads the tokens from
/// the fragment and immediately removes them from the URL bar. A fragment is
/// never sent to a server, so the tokens stay out of access logs and `Referer`.
fn sso_landing_url(base_url: &str, access: &str, refresh: &str) -> String {
    format!(
        "{}/oauth/callback#access_token={access}&refresh_token={refresh}",
        base_url.trim_end_matches('/')
    )
}

// ─── SAML flow ────────────────────────────────────────────────────────────────

/// Cookie binding an SP-initiated SAML sign-in to the browser that started it.
const SAML_STATE_COOKIE: &str = "saml_state";

/// The generic body every refused SAML message gets. The reason goes to the
/// audit log and the server log, never to the client.
const SAML_FAILED: &str = "{\"error\":\"SAML sign-in failed\"}";

/// The `saml_state` cookie. The IdP returns the browser to the ACS with a
/// cross-site POST, which only a `SameSite=None; Secure` cookie accompanies, so
/// it is that whenever `BASE_URL` is https — whatever `SECURE_COOKIES` says.
/// On a loopback http `BASE_URL` (local development; [`super::saml::check_transport`]
/// refuses any other http one) it stays `Lax`, which works when the IdP is
/// same-site.
fn saml_state_cookie(value: &str, max_age: u32, https: bool) -> String {
    let same_site = if https { "None; Secure" } else { "Lax" };
    format!(
        "{SAML_STATE_COOKIE}={value}; HttpOnly; SameSite={same_site}; Path=/api/auth/saml; \
         Max-Age={max_age}"
    )
}

fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|c| c.split(';'))
        .find_map(|p| {
            p.trim()
                .strip_prefix(name)
                .and_then(|r| r.strip_prefix('='))
                .map(str::to_string)
        })
}

/// The active SAML provider `slug`, or the response to send instead.
// A `Response` error keeps the handlers short; these run once per request.
#[allow(clippy::result_large_err)]
fn active_saml_provider(
    state: &AppState,
    slug: &str,
) -> Result<super::models::OauthProvider, Response> {
    match state.auth_db.get_oauth_provider_by_slug(slug) {
        Ok(Some(p)) if p.provider_type == "saml" && p.is_active => Ok(p),
        Ok(_) => Err((StatusCode::NOT_FOUND, "{\"error\":\"Provider not found\"}").into_response()),
        Err(e) => {
            tracing::error!("SAML provider lookup for '{slug}' failed: {e}");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "{\"error\":\"Provider lookup failed\"}",
            )
                .into_response())
        }
    }
}

/// Run CPU-bound SAML work (RSA, libxml, key generation) off the async workers.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| anyhow::anyhow!("SAML task failed: {e}"))?
}

/// Audit text for a refused message: the error chain, bounded.
fn audit_detail(e: &anyhow::Error) -> String {
    let mut s = format!("{e:#}");
    if s.len() > 500 {
        let mut cut = 500;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
    s
}

/// GET /api/auth/saml/:slug/login — start an SP-initiated SAML sign-in.
/// Redirects to the IdP's SSO URL with an AuthnRequest and binds the flow to
/// this browser with the `saml_state` cookie.
pub async fn saml_login(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    let provider = match state.auth_db.get_oauth_provider_by_slug(&slug) {
        Ok(Some(p)) if p.provider_type == "saml" && p.offers_login() => p,
        Ok(_) => {
            return (StatusCode::NOT_FOUND, "{\"error\":\"Provider not found\"}").into_response()
        }
        Err(e) => {
            tracing::error!("SAML provider lookup for '{slug}' failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "{\"error\":\"Provider lookup failed\"}",
            )
                .into_response();
        }
    };
    let https = base_url_is_https(&state.base_url);
    let (db, base, secret) = (
        state.auth_db.clone(),
        state.base_url.to_string(),
        state.jwt_config.secret.clone(),
    );
    match blocking(move || begin_saml_flow(&db, &provider, &base, &secret)).await {
        Ok((url, relay_state)) => (
            [(
                header::SET_COOKIE,
                saml_state_cookie(&relay_state, 600, https),
            )],
            Redirect::temporary(&url),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("SAML login error for '{}': {e:#}", slug);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "{\"error\":\"Could not start the SAML sign-in\"}",
            )
                .into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SamlAcsForm {
    #[serde(rename = "SAMLResponse")]
    pub saml_response: String,
    #[serde(rename = "RelayState")]
    pub relay_state: Option<String>,
}

fn metadata_response(xml: anyhow::Result<String>, slug: &str) -> Response {
    match xml {
        Ok(xml) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/samlmetadata+xml")],
            xml,
        )
            .into_response(),
        Err(e) => {
            tracing::error!("SAML metadata for '{slug}' failed: {e:#}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "{\"error\":\"Could not build the SAML metadata\"}",
            )
                .into_response()
        }
    }
}

/// GET /api/auth/saml/:slug/metadata — SAML SP metadata XML of an active
/// provider. Admins can fetch it before activation from
/// `/api/admin/oauth/providers/:id/saml/metadata`.
pub async fn saml_metadata(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    let provider = match active_saml_provider(&state, &slug) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let (db, base, secret) = (
        state.auth_db.clone(),
        state.base_url.to_string(),
        state.jwt_config.secret.clone(),
    );
    let xml = blocking(move || generate_sp_metadata(&db, &provider, &base, &secret)).await;
    metadata_response(xml, &slug)
}

/// POST /api/auth/saml/:slug/acs — SAML Assertion Consumer Service
pub async fn saml_acs(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    client_ip: ClientIp,
    headers: axum::http::HeaderMap,
    // `axum::Form` consumes the request body and so must be the LAST extractor.
    axum::Form(form): axum::Form<SamlAcsForm>,
) -> Response {
    let provider = match active_saml_provider(&state, &slug) {
        Ok(p) => p,
        Err(r) => {
            audit_sso_login_failure(
                &state,
                &headers,
                client_ip,
                "saml",
                &slug,
                "provider_not_found",
            );
            return r;
        }
    };

    // Login-CSRF defence: the RelayState must be the one this browser was given
    // when it started the sign-in (see saml_login). Otherwise an attacker could
    // deliver their own signed response to a victim's browser and sign the
    // victim in as the attacker. The pending request is consumed here, so a
    // response can be presented once.
    let relay_state = form.relay_state.as_deref().unwrap_or_default();
    let bound = !relay_state.is_empty()
        && cookie_value(&headers, SAML_STATE_COOKIE).as_deref() == Some(relay_state);
    let request_id = if bound {
        take_pending_request(relay_state, &slug)
    } else {
        None
    };
    // Without a request of this browser to answer, only an IdP-initiated
    // response can be accepted, and only when the provider allows them.
    if request_id.is_none() && !provider.saml_config.allow_idp_initiated {
        let reason = if bound {
            "unknown_request"
        } else {
            "invalid_state_binding"
        };
        audit_sso_login_failure(&state, &headers, client_ip, "saml", &slug, reason);
        return (
            StatusCode::BAD_REQUEST,
            "{\"error\":\"Invalid or missing state binding\"}",
        )
            .into_response();
    }

    let (db, base, secret, p) = (
        state.auth_db.clone(),
        state.base_url.to_string(),
        state.jwt_config.secret.clone(),
        provider.clone(),
    );
    let response_b64 = form.saml_response;
    let jwt = state.jwt_config.clone();
    let result = blocking(move || {
        let claims = verify_saml_response(
            &db,
            &p,
            &base,
            &secret,
            &response_b64,
            request_id.as_deref(),
        )?;
        complete_saml_flow(&claims, &p, &db, &jwt)
    })
    .await;

    match result {
        Ok((access, refresh)) => {
            audit_sso_login_success(&state, &headers, client_ip, "saml", &slug, &access);
            (
                [(
                    header::SET_COOKIE,
                    saml_state_cookie("", 0, base_url_is_https(&state.base_url)),
                )],
                Redirect::to(&sso_landing_url(&state.base_url, &access, &refresh)),
            )
                .into_response()
        }
        Err(e) => {
            tracing::warn!("SAML ACS refused a response for '{}': {e:#}", slug);
            let mut b = AuditEventBuilder::new(AuditEventType::LoginFailure, AuditOutcome::Failure)
                .details(serde_json::json!({
                    "auth_method": "sso",
                    "provider_type": "saml",
                    "provider_slug": slug,
                    "reason": "assertion_rejected",
                    "detail": audit_detail(&e),
                }));
            b.ip_address = client_ip.as_string();
            b.user_agent = audit::user_agent(&headers);
            b.request_id = audit::request_id_from_headers(&headers);
            state.audit.log(b);
            (StatusCode::UNAUTHORIZED, SAML_FAILED).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SamlSloForm {
    #[serde(rename = "SAMLRequest")]
    pub saml_request: Option<String>,
    #[serde(rename = "SAMLResponse")]
    pub saml_response: Option<String>,
    #[serde(rename = "RelayState")]
    pub relay_state: Option<String>,
}

/// GET /api/auth/saml/:slug/slo — Single Logout, HTTP-Redirect binding.
pub async fn saml_slo_redirect(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    client_ip: ClientIp,
    headers: axum::http::HeaderMap,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
) -> Response {
    let query = query.unwrap_or_default();
    let is_response = query.split('&').any(|p| p.starts_with("SAMLResponse="));
    slo(
        state,
        slug,
        client_ip,
        headers,
        is_response,
        move |db, p, base, secret| handle_slo(db, p, base, secret, SloBinding::Redirect(&query)),
    )
    .await
}

/// POST /api/auth/saml/:slug/slo — Single Logout, HTTP-POST binding.
pub async fn saml_slo_post(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    client_ip: ClientIp,
    headers: axum::http::HeaderMap,
    axum::Form(form): axum::Form<SamlSloForm>,
) -> Response {
    let is_response = form.saml_request.is_none() && form.saml_response.is_some();
    slo(
        state,
        slug,
        client_ip,
        headers,
        is_response,
        move |db, p, base, secret| {
            let message = form
                .saml_request
                .as_deref()
                .or(form.saml_response.as_deref())
                .ok_or_else(|| anyhow::anyhow!("no SAMLRequest or SAMLResponse"))?;
            handle_slo(
                db,
                p,
                base,
                secret,
                SloBinding::Post {
                    message_b64: message,
                    relay_state: form.relay_state.as_deref(),
                },
            )
        },
    )
    .await
}

async fn slo(
    state: AppState,
    slug: String,
    client_ip: ClientIp,
    headers: axum::http::HeaderMap,
    is_response: bool,
    run: impl FnOnce(
            &super::db::AuthDb,
            &super::models::OauthProvider,
            &str,
            &str,
        ) -> anyhow::Result<SloOutcome>
        + Send
        + 'static,
) -> Response {
    let provider = match active_saml_provider(&state, &slug) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let (db, base, secret, p) = (
        state.auth_db.clone(),
        state.base_url.to_string(),
        state.jwt_config.secret.clone(),
        provider.clone(),
    );
    let home = format!("{}/", state.base_url.trim_end_matches('/'));
    let audit = |details: serde_json::Value, outcome: AuditOutcome| {
        let mut b = AuditEventBuilder::new(AuditEventType::Logout, outcome).details(details);
        b.ip_address = client_ip.as_string();
        b.user_agent = audit::user_agent(&headers);
        b.request_id = audit::request_id_from_headers(&headers);
        state.audit.log(b);
    };
    match blocking(move || run(&db, &p, &base, &secret)).await {
        Ok(SloOutcome::LoggedOut {
            revoked,
            user_ids,
            response_url,
        }) => {
            audit(
                serde_json::json!({
                    "auth_method": "saml_slo",
                    "initiated_by": "idp",
                    "provider_slug": slug,
                    "revoked_sessions": revoked,
                    "user_ids": user_ids,
                }),
                AuditOutcome::Success,
            );
            Redirect::to(response_url.as_deref().unwrap_or(&home)).into_response()
        }
        Ok(SloOutcome::Answered { success }) => {
            if !success {
                tracing::warn!("SAML IdP '{slug}' reported a partial or failed logout");
            }
            Redirect::to(&home).into_response()
        }
        Err(e) if is_response => {
            // Our side of the session ended before the browser left for the
            // IdP; a bad answer changes nothing, so just land the browser.
            tracing::warn!("SAML LogoutResponse from '{slug}' refused: {e:#}");
            Redirect::to(&home).into_response()
        }
        Err(e) => {
            tracing::warn!("SAML LogoutRequest from '{slug}' refused: {e:#}");
            audit(
                serde_json::json!({
                    "auth_method": "saml_slo",
                    "initiated_by": "idp",
                    "provider_slug": slug,
                    "reason": "logout_request_rejected",
                    "detail": audit_detail(&e),
                }),
                AuditOutcome::Failure,
            );
            (
                StatusCode::BAD_REQUEST,
                "{\"error\":\"SAML logout request refused\"}",
            )
                .into_response()
        }
    }
}

// ─── SAML admin ───────────────────────────────────────────────────────────────

fn json_error(status: StatusCode, msg: impl std::fmt::Display) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": msg.to_string() })),
    )
        .into_response()
}

// A `Response` error keeps the handlers short; these run once per request.
#[allow(clippy::result_large_err)]
fn saml_provider_by_id(
    state: &AppState,
    id: &str,
) -> Result<super::models::OauthProvider, Response> {
    match state.auth_db.get_oauth_provider_by_id(id) {
        Ok(Some(p)) if p.provider_type == "saml" => Ok(p),
        Ok(_) => Err(json_error(StatusCode::NOT_FOUND, "SAML provider not found")),
        Err(e) => Err(json_error(StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct SamlMetadataImport {
    /// https URL to fetch the IdP metadata from (no redirects).
    pub url: Option<String>,
    /// Pasted IdP metadata XML.
    pub xml: Option<String>,
}

/// POST /api/admin/oauth/saml-metadata — read IdP metadata (fetched from an
/// https URL or pasted) and return the fields to fill in: entity ID, SSO and
/// SLO URLs and every signing certificate.
pub async fn admin_read_saml_metadata(
    State(_state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Json(body): Json<SamlMetadataImport>,
) -> Response {
    let result = match (body.url.as_deref().map(str::trim), body.xml) {
        (Some(url), None) if !url.is_empty() => super::saml::fetch_idp_metadata(url).await,
        (None, Some(xml)) => super::saml::read_idp_metadata(&xml),
        _ => return json_error(StatusCode::BAD_REQUEST, "send either `url` or `xml`"),
    };
    match result {
        Ok(md) => Json(md).into_response(),
        Err(e) => json_error(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

/// GET /api/admin/oauth/providers/:id/saml — our entity ID, endpoints and SP
/// keys, and the IdP certificates, for the admin UI.
pub async fn admin_saml_overview(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Response {
    let provider = match saml_provider_by_id(&state, &id) {
        Ok(p) => p,
        Err(r) => return r,
    };
    match super::saml::sp_overview(&state.auth_db, &provider, &state.base_url) {
        Ok(v) => Json(v).into_response(),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// GET /api/admin/oauth/providers/:id/saml/metadata — the SP metadata, also
/// for a provider that is not active yet.
pub async fn admin_saml_metadata(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
) -> Response {
    let provider = match saml_provider_by_id(&state, &id) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let slug = provider.slug.clone();
    let (db, base, secret) = (
        state.auth_db.clone(),
        state.base_url.to_string(),
        state.jwt_config.secret.clone(),
    );
    let xml = blocking(move || generate_sp_metadata(&db, &provider, &base, &secret)).await;
    metadata_response(xml, &slug)
}

#[derive(Debug, Default, Deserialize)]
pub struct SamlKeyCreate {
    /// The private key: a secret reference (`env:`, `file:`, `vault:`) or,
    /// outside the production posture, a PEM key. Omit both fields to have the
    /// store generate a key pair.
    pub private_key: Option<String>,
    /// The key's X.509 certificate (PEM).
    pub certificate: Option<String>,
}

/// POST /api/admin/oauth/providers/:id/saml/keys — add an SP key (generated,
/// or imported). The first key becomes current; later ones are published in
/// the metadata and decrypt, and sign once activated.
pub async fn admin_create_saml_key(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Path(id): Path<String>,
    body: Option<Json<SamlKeyCreate>>,
) -> Response {
    let provider = match saml_provider_by_id(&state, &id) {
        Ok(p) => p,
        Err(r) => return r,
    };
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let (db, base, secret) = (
        state.auth_db.clone(),
        state.base_url.to_string(),
        state.jwt_config.secret.clone(),
    );
    let result = match (body.private_key, body.certificate) {
        (None, None) => {
            blocking(move || super::saml::generate_sp_key(&db, &provider, &base, &secret)).await
        }
        (Some(key), Some(cert)) => {
            blocking(move || super::saml::import_sp_key(&db, &provider, &key, &cert, &secret)).await
        }
        _ => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "send both `private_key` and `certificate`, or neither to generate a key",
            )
        }
    };
    match result {
        Ok(key) => (StatusCode::CREATED, Json(key)).into_response(),
        Err(e) if e.downcast_ref::<crate::secrets::SecretError>().is_some() => secret_error(&e),
        Err(e) => json_error(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

/// POST /api/admin/oauth/providers/:id/saml/keys/:kid/activate — make a key
/// the one that signs (the step after the IdP has picked up the new metadata).
pub async fn admin_activate_saml_key(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Path((id, kid)): Path<(String, String)>,
) -> Response {
    if let Err(r) = saml_provider_by_id(&state, &id) {
        return r;
    }
    match state.auth_db.activate_saml_sp_key(&id, &kid) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => json_error(StatusCode::NOT_FOUND, "SP key not found"),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// DELETE /api/admin/oauth/providers/:id/saml/keys/:kid — retire a key that
/// no longer signs. The current key cannot be deleted.
pub async fn admin_delete_saml_key(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Path((id, kid)): Path<(String, String)>,
) -> Response {
    if let Err(r) = saml_provider_by_id(&state, &id) {
        return r;
    }
    match state.auth_db.delete_saml_sp_key(&id, &kid) {
        Ok(Some(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Some(false)) => json_error(
            StatusCode::CONFLICT,
            "the current key signs; activate another key before deleting this one",
        ),
        Ok(None) => json_error(StatusCode::NOT_FOUND, "SP key not found"),
        Err(e) => json_error(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}
