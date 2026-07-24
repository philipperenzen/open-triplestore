//! Transactional email for account lifecycle flows (address verification,
//! password reset, username reminders, email-change confirmation).
//!
//! Configured from the environment. When `SMTP_HOST` is unset the mailer falls
//! back to a log-only backend: every message is written to the server log at
//! INFO level (including any action link), so development and test
//! deployments can exercise the full flows without an SMTP relay.
//!
//! Environment variables:
//! - `SMTP_HOST` — SMTP relay host. Unset → log-only backend.
//! - `SMTP_PORT` — relay port (default 587).
//! - `SMTP_USERNAME` / `SMTP_PASSWORD` — optional credentials.
//! - `SMTP_TLS` — transport security: `none` (plaintext — only for a relay on a
//!   trusted private network, e.g. the bundled compose `mail` service),
//!   `starttls`, or `implicit` (TLS-wrapped/SMTPS). Default: implicit TLS on
//!   port 465, STARTTLS otherwise.
//! - `SMTP_STARTTLS` — legacy switch: `1`/`true` forces STARTTLS, `0`/`false`
//!   forces implicit TLS. Ignored when `SMTP_TLS` is set.
//! - `SMTP_FROM` — From mailbox (default `Open Triplestore <no-reply@localhost>`).
//! - `PUBLIC_BASE_URL` — base URL minted into email links (default: the
//!   server's linked-data base URL).
//!
//! Note: the ops alerting module (`alerting` feature) has its own independent
//! `ALERT_SMTP_*` configuration; this mailer is for user-facing account email.

use std::sync::Arc;

/// How long (seconds) an email send may take before it is abandoned.
const SEND_TIMEOUT_SECS: u64 = 30;

/// SMTP transport security for the relay hop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SmtpTls {
    /// Plaintext SMTP — only sane towards a relay on a trusted private network
    /// (e.g. the bundled compose `mail` service; nothing there is published on
    /// a host interface).
    None,
    /// Plain connection upgraded via STARTTLS (typical on port 587).
    StartTls,
    /// TLS-wrapped from the first byte (SMTPS, typical on port 465).
    Implicit,
}

/// Resolve the transport security mode: explicit `SMTP_TLS` wins, then the
/// legacy `SMTP_STARTTLS` flag, then the port convention (465 ⇒ implicit TLS,
/// anything else ⇒ STARTTLS).
fn resolve_tls(port: u16, tls: Option<&str>, starttls: Option<&str>) -> SmtpTls {
    if let Some(v) = tls {
        match v.trim().to_ascii_lowercase().as_str() {
            "none" | "off" | "plain" => return SmtpTls::None,
            "starttls" => return SmtpTls::StartTls,
            "implicit" | "tls" | "smtps" => return SmtpTls::Implicit,
            other => tracing::warn!(
                "email: unrecognized SMTP_TLS value {other:?} (expected none|starttls|implicit) — falling back"
            ),
        }
    }
    match starttls {
        Some("1") | Some("true") | Some("TRUE") => SmtpTls::StartTls,
        Some("0") | Some("false") | Some("FALSE") => SmtpTls::Implicit,
        _ => {
            if port == 465 {
                SmtpTls::Implicit
            } else {
                SmtpTls::StartTls
            }
        }
    }
}

enum Backend {
    Smtp {
        host: String,
        port: u16,
        username: Option<String>,
        password: Option<String>,
        tls: SmtpTls,
    },
    /// No SMTP configured — log every message instead of delivering it.
    Log,
}

pub struct Mailer {
    backend: Backend,
    from: String,
    /// Public base URL used to mint action links (no trailing slash).
    link_base: String,
}

/// A fresh RFC 5322 `Message-ID` (`<uuid@domain>`) for an outgoing message.
/// Large receivers (Gmail: 550 5.7.1) reject mail without a valid Message-ID
/// outright, and SMTP relays only repair the header for clients they consider
/// local — which a sibling container is not — so the app must stamp its own.
pub(crate) fn rfc5322_message_id(from_domain: &str) -> String {
    let domain = match from_domain.trim() {
        "" => "localhost",
        d => d,
    };
    format!("<{}@{}>", uuid::Uuid::new_v4(), domain)
}

fn env_opt(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

impl Mailer {
    /// Build the mailer from `SMTP_*` env vars. `default_link_base` is used for
    /// action links unless `PUBLIC_BASE_URL` overrides it.
    pub fn from_env(default_link_base: &str) -> Self {
        let link_base = env_opt("PUBLIC_BASE_URL")
            .unwrap_or_else(|| default_link_base.to_string())
            .trim_end_matches('/')
            .to_string();
        let from = env_opt("SMTP_FROM")
            .unwrap_or_else(|| "Open Triplestore <no-reply@localhost>".to_string());

        let backend = match env_opt("SMTP_HOST") {
            Some(host) => {
                let port: u16 = env_opt("SMTP_PORT")
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(587);
                let tls = resolve_tls(
                    port,
                    env_opt("SMTP_TLS").as_deref(),
                    env_opt("SMTP_STARTTLS").as_deref(),
                );
                Backend::Smtp {
                    host,
                    port,
                    username: env_opt("SMTP_USERNAME"),
                    password: env_opt("SMTP_PASSWORD"),
                    tls,
                }
            }
            None => Backend::Log,
        };

        Self {
            backend,
            from,
            link_base,
        }
    }

    /// Log-only mailer for tests.
    pub fn log_only(link_base: &str) -> Self {
        Self {
            backend: Backend::Log,
            from: "Open Triplestore <no-reply@localhost>".to_string(),
            link_base: link_base.trim_end_matches('/').to_string(),
        }
    }

    /// True when a real SMTP relay is configured (messages actually leave the
    /// box). Surfaced to the frontend so it can word flows accordingly.
    pub fn smtp_configured(&self) -> bool {
        matches!(self.backend, Backend::Smtp { .. })
    }

    /// Public base URL for action links (no trailing slash).
    pub fn link_base(&self) -> &str {
        &self.link_base
    }

    /// Build one RFC 5322-complete plain-text message (From/To/Subject/Date/
    /// Message-ID). Errors are descriptive strings for the caller to log.
    fn build_message(
        &self,
        to: &str,
        subject: &str,
        body: &str,
    ) -> Result<lettre::Message, String> {
        use lettre::message::{header::ContentType, Mailbox};
        let from_mbox: Mailbox = self
            .from
            .parse()
            .map_err(|e| format!("bad SMTP_FROM address: {e}"))?;
        let to_mbox: Mailbox = to.parse().map_err(|e| format!("invalid recipient: {e}"))?;
        let message_id = rfc5322_message_id(from_mbox.email.domain());
        lettre::Message::builder()
            .from(from_mbox)
            .to(to_mbox)
            .subject(subject)
            .message_id(Some(message_id))
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|e| format!("build message failed: {e}"))
    }

    /// Deliver one plain-text message. Best-effort: failures are logged, never
    /// propagated (account flows must not 500 because a relay hiccupped).
    pub async fn send(&self, to: &str, subject: &str, body: &str) -> bool {
        match &self.backend {
            Backend::Log => {
                tracing::info!(
                    target: "ots::email",
                    "email (log backend, SMTP not configured)\nTo: {to}\nSubject: {subject}\n\n{body}"
                );
                true
            }
            Backend::Smtp {
                host,
                port,
                username,
                password,
                tls,
            } => {
                use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
                let builder = match tls {
                    SmtpTls::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host),
                    SmtpTls::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(host),
                    SmtpTls::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                        host,
                    )),
                };
                let builder = match builder {
                    Ok(b) => b.port(*port),
                    Err(e) => {
                        tracing::warn!("email: SMTP relay setup failed: {e}");
                        return false;
                    }
                };
                let builder = if let (Some(u), Some(p)) = (username, password) {
                    builder.credentials(lettre::transport::smtp::authentication::Credentials::new(
                        u.clone(),
                        p.clone(),
                    ))
                } else {
                    builder
                };
                let msg = match self.build_message(to, subject, body) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::warn!("email: {e}");
                        return false;
                    }
                };
                let transport = builder.build();
                match tokio::time::timeout(
                    std::time::Duration::from_secs(SEND_TIMEOUT_SECS),
                    transport.send(msg),
                )
                .await
                {
                    Ok(Ok(_)) => true,
                    Ok(Err(e)) => {
                        tracing::warn!("email: send failed: {e}");
                        false
                    }
                    Err(_) => {
                        tracing::warn!("email: send timed out after {SEND_TIMEOUT_SECS}s");
                        false
                    }
                }
            }
        }
    }

    /// Fire-and-forget delivery on a background task. Handlers use this so
    /// response latency never depends on (or reveals) whether a message was
    /// actually sent — important for the enumeration-safe recovery endpoints.
    pub fn send_background(self: &Arc<Self>, to: String, subject: String, body: String) {
        let mailer = self.clone();
        tokio::spawn(async move {
            mailer.send(&to, &subject, &body).await;
        });
    }

    // ─── Message templates ───────────────────────────────────────────────────

    pub fn send_verification_email(self: &Arc<Self>, to: &str, username: &str, token: &str) {
        let link = format!("{}/verify-email?token={token}", self.link_base);
        self.send_background(
            to.to_string(),
            "Verify your email address".to_string(),
            format!(
                "Hi {username},\n\n\
                 Welcome to Open Triplestore. Please confirm this email address by opening:\n\n\
                 {link}\n\n\
                 The link is valid for 24 hours. If you did not create this account, you can ignore this message.\n"
            ),
        );
    }

    pub fn send_password_reset_email(self: &Arc<Self>, to: &str, username: &str, token: &str) {
        let link = format!("{}/reset-password?token={token}", self.link_base);
        self.send_background(
            to.to_string(),
            "Reset your password".to_string(),
            format!(
                "Hi {username},\n\n\
                 A password reset was requested for your Open Triplestore account. To choose a new password, open:\n\n\
                 {link}\n\n\
                 The link is valid for 1 hour and can be used once. If you did not request this, you can ignore this message — your password is unchanged.\n"
            ),
        );
    }

    pub fn send_username_reminder_email(self: &Arc<Self>, to: &str, usernames: &[String]) {
        let listing = usernames
            .iter()
            .map(|u| format!("  - {u}"))
            .collect::<Vec<_>>()
            .join("\n");
        let link = format!("{}/login", self.link_base);
        self.send_background(
            to.to_string(),
            "Your username".to_string(),
            format!(
                "Hi,\n\n\
                 A username reminder was requested for this email address. The account associated with it:\n\n\
                 {listing}\n\n\
                 Sign in at {link}\n\n\
                 If you did not request this, you can ignore this message.\n"
            ),
        );
    }

    pub fn send_change_email_confirmation(self: &Arc<Self>, to: &str, username: &str, token: &str) {
        let link = format!("{}/verify-email?token={token}", self.link_base);
        self.send_background(
            to.to_string(),
            "Confirm your new email address".to_string(),
            format!(
                "Hi {username},\n\n\
                 A request was made to change your Open Triplestore account email to this address. To confirm the change, open:\n\n\
                 {link}\n\n\
                 The link is valid for 24 hours. If you did not request this, you can ignore this message — your account email is unchanged.\n"
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smtp_tls_explicit_mode_wins() {
        assert_eq!(resolve_tls(587, Some("none"), None), SmtpTls::None);
        assert_eq!(resolve_tls(465, Some("NONE"), Some("1")), SmtpTls::None);
        assert_eq!(resolve_tls(25, Some(" starttls "), None), SmtpTls::StartTls);
        assert_eq!(resolve_tls(587, Some("implicit"), None), SmtpTls::Implicit);
        assert_eq!(
            resolve_tls(587, Some("smtps"), Some("1")),
            SmtpTls::Implicit
        );
    }

    #[test]
    fn smtp_tls_legacy_starttls_flag() {
        assert_eq!(resolve_tls(587, None, Some("0")), SmtpTls::Implicit);
        assert_eq!(resolve_tls(465, None, Some("1")), SmtpTls::StartTls);
        assert_eq!(resolve_tls(587, None, Some("false")), SmtpTls::Implicit);
    }

    #[test]
    fn message_id_shape_and_uniqueness() {
        let a = rfc5322_message_id("example.org");
        let b = rfc5322_message_id("example.org");
        assert!(a.starts_with('<') && a.ends_with("@example.org>"), "{a}");
        assert_ne!(a, b, "each message must get a fresh id");
        assert!(rfc5322_message_id("  ").ends_with("@localhost>"));
    }

    #[test]
    fn built_message_has_message_id_and_date() {
        let mailer = Mailer::log_only("http://localhost");
        let msg = mailer
            .build_message("user@example.com", "Subject", "Body")
            .expect("message builds");
        let raw = String::from_utf8(msg.formatted()).expect("utf8");
        // Gmail rejects mail without a valid Message-ID (550 5.7.1); the id
        // domain follows the From mailbox (default no-reply@localhost).
        assert!(
            raw.contains("Message-ID: <"),
            "missing Message-ID in:\n{raw}"
        );
        assert!(raw.contains("@localhost>"), "id domain should follow From");
        assert!(raw.contains("Date: "), "missing Date in:\n{raw}");
    }

    #[test]
    fn smtp_tls_port_default() {
        assert_eq!(resolve_tls(465, None, None), SmtpTls::Implicit);
        assert_eq!(resolve_tls(587, None, None), SmtpTls::StartTls);
        assert_eq!(resolve_tls(25, None, None), SmtpTls::StartTls);
        // Unrecognized SMTP_TLS falls through to the legacy flag / port default.
        assert_eq!(resolve_tls(465, Some("bogus"), None), SmtpTls::Implicit);
        assert_eq!(
            resolve_tls(587, Some("bogus"), Some("0")),
            SmtpTls::Implicit
        );
    }
}
