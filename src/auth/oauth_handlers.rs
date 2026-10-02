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
    acs_url, begin_saml_flow, complete_saml_flow, generate_sp_metadata, sp_entity_id,
    take_pending_request,
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

/// POST /api/admin/oauth/providers
pub async fn admin_create_provider(
    State(state): State<AppState>,
    Extension(_user): Extension<AuthenticatedUser>,
    Json(mut body): Json<OauthProviderCreate>,
) -> impl IntoResponse {
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

/// The `saml_state` cookie. The IdP returns the browser to the ACS with a
/// cross-site POST, which a `SameSite=Lax` cookie does not accompany, so over
/// HTTPS it is `SameSite=None; Secure`. Without secure cookies (local dev) it
/// stays `Lax`, which still works when the IdP is same-site (e.g. `localhost`).
fn saml_state_cookie(value: &str, max_age: u32, secure: bool) -> String {
    let same_site = if secure { "None; Secure" } else { "Lax" };
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
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{{\"error\":\"{e}\"}}"),
            )
                .into_response()
        }
    };

    match begin_saml_flow(&provider, &state.base_url) {
        Ok((url, relay_state)) => (
            [(
                header::SET_COOKIE,
                saml_state_cookie(&relay_state, 600, state.secure_cookies),
            )],
            Redirect::temporary(&url),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("SAML login error for '{}': {e}", slug);
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

/// GET /api/auth/saml/:slug/metadata — SAML SP metadata XML
pub async fn saml_metadata(State(state): State<AppState>, Path(slug): Path<String>) -> Response {
    let provider = match state.auth_db.get_oauth_provider_by_slug(&slug) {
        Ok(Some(p)) if p.provider_type == "saml" => p,
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

    match generate_sp_metadata(
        &provider,
        &sp_entity_id(&state.base_url, &slug),
        &acs_url(&state.base_url, &slug),
    ) {
        Ok(xml) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/samlmetadata+xml")],
            xml,
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("{{\"error\":\"{e}\"}}"),
        )
            .into_response(),
    }
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
    let provider = match state.auth_db.get_oauth_provider_by_slug(&slug) {
        Ok(Some(p)) if p.is_active && p.provider_type == "saml" => p,
        Ok(_) => {
            audit_sso_login_failure(
                &state,
                &headers,
                client_ip,
                "saml",
                &slug,
                "provider_not_found",
            );
            return (StatusCode::NOT_FOUND, "{\"error\":\"Provider not found\"}").into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("{{\"error\":\"{e}\"}}"),
            )
                .into_response()
        }
    };

    // Login-CSRF defence: the RelayState must be the one this browser was given
    // when it started the sign-in (see saml_login). Otherwise an attacker could
    // deliver their own signed response to a victim's browser and sign the
    // victim in as the attacker.
    let relay_state = form.relay_state.as_deref().unwrap_or_default();
    if relay_state.is_empty()
        || cookie_value(&headers, SAML_STATE_COOKIE).as_deref() != Some(relay_state)
    {
        audit_sso_login_failure(
            &state,
            &headers,
            client_ip,
            "saml",
            &slug,
            "invalid_state_binding",
        );
        return (
            StatusCode::BAD_REQUEST,
            "{\"error\":\"Invalid or missing state binding\"}",
        )
            .into_response();
    }
    // The AuthnRequest this response must answer. Consumed here, so a response
    // can be presented once; IdP-initiated responses have no request to answer.
    let Some(request_id) = take_pending_request(relay_state, &slug) else {
        audit_sso_login_failure(
            &state,
            &headers,
            client_ip,
            "saml",
            &slug,
            "unknown_request",
        );
        return (
            StatusCode::BAD_REQUEST,
            "{\"error\":\"Unknown or expired sign-in request\"}",
        )
            .into_response();
    };

    match complete_saml_flow(
        &form.saml_response,
        &request_id,
        &provider,
        &state.base_url,
        &state.auth_db,
        &state.jwt_config,
    )
    .await
    {
        Ok((access, refresh)) => {
            audit_sso_login_success(&state, &headers, client_ip, "saml", &slug, &access);
            (
                [(
                    header::SET_COOKIE,
                    saml_state_cookie("", 0, state.secure_cookies),
                )],
                Redirect::to(&sso_landing_url(&state.base_url, &access, &refresh)),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("SAML ACS error for '{}': {e}", slug);
            audit_sso_login_failure(
                &state,
                &headers,
                client_ip,
                "saml",
                &slug,
                "assertion_rejected",
            );
            (StatusCode::UNAUTHORIZED, format!("{{\"error\":\"{e}\"}}")).into_response()
        }
    }
}
