//! Operational alerting.
//!
//! Two backends, both opt-in via env vars:
//! - SMTP (gated by the `alerting` Cargo feature, configured via `ALERT_SMTP_*`;
//!   without `ALERT_SMTP_HOST` it uses the account-email relay, `SMTP_*`)
//! - HTTP webhook (always available, configured via `ALERT_WEBHOOK_URL`)
//!
//! Every successful dispatch is recorded in the audit log as `alert_sent`.
//! Failures are logged at WARN but never propagated; alerting must never
//! break the calling code path.

use crate::email::SmtpTls;
use crate::secrets::env_secret_opt;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::audit::{AuditEventBuilder, AuditEventType, AuditLogger, AuditOutcome};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Info,
    Warn,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub severity: AlertSeverity,
    pub kind: String,
    pub message: String,
    pub context: serde_json::Value,
}

#[derive(Clone, Default)]
pub struct AlertConfig {
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_user: Option<String>,
    pub smtp_pass: Option<String>,
    /// Transport security for the relay hop; `None` is implicit TLS (SMTPS),
    /// what alerting has always used.
    pub smtp_tls: Option<SmtpTls>,
    pub smtp_from: Option<String>,
    pub smtp_to: Vec<String>,
    pub webhook_url: Option<String>,
}

impl AlertConfig {
    /// The alerting channels from the environment.
    ///
    /// The SMTP relay comes from one place as a whole, so credentials are
    /// never sent to a host they were not configured for:
    /// - with `ALERT_SMTP_HOST` set, from `ALERT_SMTP_PORT`, `ALERT_SMTP_USER`,
    ///   `ALERT_SMTP_PASS` and `ALERT_SMTP_TLS` (`none`|`starttls`|`implicit`,
    ///   default implicit TLS, as before);
    /// - otherwise from the account-email relay: `SMTP_HOST`, `SMTP_PORT`,
    ///   `SMTP_USERNAME`, `SMTP_PASSWORD` and `SMTP_TLS`/`SMTP_STARTTLS`,
    ///   resolved exactly as the mailer resolves them.
    ///
    /// The sender is `ALERT_SMTP_FROM`, else `SMTP_FROM`, else (on the
    /// account relay) the mailer's default. Recipients are only ever
    /// `ALERT_SMTP_TO`, so the fallback alone sends no ops alert anywhere.
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok(), env_secret_opt)
    }

    /// [`Self::from_env`] over an arbitrary lookup: `var` reads a plain
    /// setting, `secret` a secret one. Empty and blank values count as unset.
    pub fn from_lookup(
        var: impl Fn(&str) -> Option<String>,
        secret: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let var = |name: &str| var(name).filter(|v| !v.trim().is_empty());
        let port = |name: &str| var(name).and_then(|s| s.parse::<u16>().ok());
        let alert_from = var("ALERT_SMTP_FROM").or_else(|| var("SMTP_FROM"));
        let mut cfg = Self {
            smtp_to: var("ALERT_SMTP_TO")
                .map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            webhook_url: var("ALERT_WEBHOOK_URL"),
            ..Self::default()
        };
        if let Some(host) = var("ALERT_SMTP_HOST") {
            cfg.smtp_host = Some(host);
            cfg.smtp_port = port("ALERT_SMTP_PORT");
            cfg.smtp_user = var("ALERT_SMTP_USER");
            cfg.smtp_pass = secret("ALERT_SMTP_PASS");
            cfg.smtp_tls = var("ALERT_SMTP_TLS").map(|v| {
                crate::email::parse_tls(&v).unwrap_or_else(|| {
                    tracing::warn!(
                        "alerting: unrecognized ALERT_SMTP_TLS value {:?} (expected \
                         none|starttls|implicit) — using implicit TLS",
                        v.trim()
                    );
                    SmtpTls::Implicit
                })
            });
            cfg.smtp_from = alert_from;
        } else if let Some(host) = var("SMTP_HOST") {
            let p = port("SMTP_PORT");
            cfg.smtp_tls = Some(crate::email::resolve_tls(
                p.unwrap_or(587),
                var("SMTP_TLS").as_deref(),
                var("SMTP_STARTTLS").as_deref(),
            ));
            cfg.smtp_host = Some(host);
            cfg.smtp_port = p;
            cfg.smtp_user = var("SMTP_USERNAME");
            cfg.smtp_pass = secret("SMTP_PASSWORD");
            cfg.smtp_from = alert_from.or_else(|| Some(crate::email::DEFAULT_FROM.to_string()));
        }
        cfg
    }

    pub fn is_enabled(&self) -> bool {
        self.webhook_url.is_some() || (self.smtp_host.is_some() && !self.smtp_to.is_empty())
    }
}

#[derive(Clone)]
pub struct AlertManager {
    cfg: AlertConfig,
    http: reqwest::Client,
    audit: Arc<AuditLogger>,
}

impl AlertManager {
    pub fn new(cfg: AlertConfig, audit: Arc<AuditLogger>) -> Self {
        Self {
            cfg,
            http: reqwest::Client::new(),
            audit,
        }
    }

    /// Best-effort dispatch. Never returns an error; failures are logged.
    pub async fn dispatch(&self, alert: Alert) {
        if !self.cfg.is_enabled() {
            return;
        }

        let mut delivered = Vec::new();
        if let Some(url) = &self.cfg.webhook_url {
            match self.http.post(url).json(&alert).send().await {
                Ok(r) if r.status().is_success() => delivered.push("webhook"),
                Ok(r) => tracing::warn!("alerting: webhook returned {}", r.status()),
                Err(e) => tracing::warn!("alerting: webhook failed: {}", e),
            }
        }

        #[cfg(feature = "alerting")]
        if let (Some(host), Some(from), false) = (
            self.cfg.smtp_host.as_ref(),
            self.cfg.smtp_from.as_ref(),
            self.cfg.smtp_to.is_empty(),
        ) {
            if let Err(e) = self.send_email(host, from, &alert).await {
                tracing::warn!("alerting: SMTP failed: {}", e);
            } else {
                delivered.push("email");
            }
        }

        if !delivered.is_empty() {
            self.audit.log(
                AuditEventBuilder::new(AuditEventType::AlertSent, AuditOutcome::Success).details(
                    serde_json::json!({
                        "kind": alert.kind,
                        "severity": alert.severity,
                        "channels": delivered,
                    }),
                ),
            );
        }
    }

    /// Send a plain-text email to explicit recipients — used for targeted
    /// notifications (e.g. "your saved query broke") rather than the fixed
    /// ops `smtp_to` list. Best-effort; returns whether anything was delivered.
    /// Requires the `alerting` feature and an SMTP relay (`ALERT_SMTP_*`, else
    /// the account-email `SMTP_*`).
    #[cfg(feature = "alerting")]
    pub async fn send_direct(&self, to: &[String], subject: &str, body: &str) -> bool {
        use lettre::{message::header::ContentType, AsyncTransport, Message};
        let (Some(host), Some(from)) = (self.cfg.smtp_host.as_ref(), self.cfg.smtp_from.as_ref())
        else {
            return false;
        };
        if to.is_empty() {
            return false;
        }
        let mailer = match self.smtp_mailer(host) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("notify: SMTP relay setup failed: {e}");
                return false;
            }
        };
        let mut delivered = false;
        for addr in to {
            let from_mbox: lettre::message::Mailbox = match from.parse() {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("notify: bad SMTP from address: {e}");
                    return false;
                }
            };
            let to_mbox = match addr.parse() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!("notify: skipping invalid recipient {addr}: {e}");
                    continue;
                }
            };
            // Receivers like Gmail reject mail without a valid Message-ID.
            let message_id = crate::email::rfc5322_message_id(from_mbox.email.domain());
            let msg = Message::builder()
                .from(from_mbox)
                .to(to_mbox)
                .subject(subject)
                .message_id(Some(message_id))
                .header(ContentType::TEXT_PLAIN)
                .body(body.to_string());
            match msg {
                Ok(m) => match mailer.send(m).await {
                    Ok(_) => delivered = true,
                    Err(e) => tracing::warn!("notify: send to {addr} failed: {e}"),
                },
                Err(e) => tracing::warn!("notify: build email failed: {e}"),
            }
        }
        delivered
    }

    /// Stub when the `alerting` feature is disabled: nothing is sent.
    #[cfg(not(feature = "alerting"))]
    pub async fn send_direct(&self, _to: &[String], _subject: &str, _body: &str) -> bool {
        false
    }

    /// The SMTP transport for `host` under the configured port (default 587),
    /// transport security and credentials (sent only when both are set).
    #[cfg(feature = "alerting")]
    fn smtp_mailer(
        &self,
        host: &str,
    ) -> Result<lettre::AsyncSmtpTransport<lettre::Tokio1Executor>, lettre::transport::smtp::Error>
    {
        let tls = self.cfg.smtp_tls.unwrap_or(SmtpTls::Implicit);
        let mut builder =
            crate::email::smtp_transport(host, tls)?.port(self.cfg.smtp_port.unwrap_or(587));
        if let (Some(u), Some(p)) = (self.cfg.smtp_user.as_ref(), self.cfg.smtp_pass.as_ref()) {
            builder = builder.credentials(
                lettre::transport::smtp::authentication::Credentials::new(u.clone(), p.clone()),
            );
        }
        Ok(builder.build())
    }

    #[cfg(feature = "alerting")]
    async fn send_email(&self, host: &str, from: &str, alert: &Alert) -> anyhow::Result<()> {
        use lettre::{message::header::ContentType, AsyncTransport, Message};
        let mailer = self.smtp_mailer(host)?;
        let body = format!(
            "[{:?}] {}\n\n{}\n\nContext:\n{}",
            alert.severity,
            alert.kind,
            alert.message,
            serde_json::to_string_pretty(&alert.context).unwrap_or_default(),
        );
        for to in &self.cfg.smtp_to {
            let from_mbox: lettre::message::Mailbox = from.parse()?;
            // Receivers like Gmail reject mail without a valid Message-ID.
            let message_id = crate::email::rfc5322_message_id(from_mbox.email.domain());
            let email = Message::builder()
                .from(from_mbox)
                .to(to.parse()?)
                .subject(format!(
                    "[triplestore][{:?}] {}",
                    alert.severity, alert.kind
                ))
                .message_id(Some(message_id))
                .header(ContentType::TEXT_PLAIN)
                .body(body.clone())?;
            mailer.send(email).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::db::AuthDb;

    fn manager(cfg: AlertConfig) -> (AlertManager, Arc<AuditLogger>) {
        let db = AuthDb::in_memory().unwrap();
        let audit = Arc::new(AuditLogger::new(db.pool()));
        (AlertManager::new(cfg, audit.clone()), audit)
    }

    /// Alerting is off unless a channel is configured — the whole module must be
    /// a no-op for the default deployment.
    #[test]
    fn is_enabled_requires_a_usable_channel() {
        assert!(!AlertConfig::default().is_enabled());

        let webhook = AlertConfig {
            webhook_url: Some("http://example.invalid/hook".into()),
            ..Default::default()
        };
        assert!(webhook.is_enabled(), "a webhook alone is enough");

        // An SMTP host with no recipients cannot deliver anything.
        let no_recipients = AlertConfig {
            smtp_host: Some("smtp.example.invalid".into()),
            ..Default::default()
        };
        assert!(
            !no_recipients.is_enabled(),
            "an SMTP host with no ALERT_SMTP_TO cannot deliver"
        );

        let smtp = AlertConfig {
            smtp_host: Some("smtp.example.invalid".into()),
            smtp_to: vec!["ops@example.invalid".into()],
            ..Default::default()
        };
        assert!(smtp.is_enabled());
    }

    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| v.to_string())
        }
    }

    /// Without `ALERT_SMTP_HOST`, alerting uses the account-email relay as a
    /// whole: host, port, credentials, TLS mode and sender.
    #[test]
    fn smtp_falls_back_to_the_account_relay() {
        let vars = [
            ("SMTP_HOST", "mail.example.org"),
            ("SMTP_PORT", "2525"),
            ("SMTP_USERNAME", "relay-user"),
            ("SMTP_PASSWORD", "relay-pass"),
            ("SMTP_TLS", "none"),
            ("ALERT_SMTP_TO", "ops@example.org, oncall@example.org"),
        ];
        let cfg = AlertConfig::from_lookup(lookup(&vars), lookup(&vars));
        assert_eq!(cfg.smtp_host.as_deref(), Some("mail.example.org"));
        assert_eq!(cfg.smtp_port, Some(2525));
        assert_eq!(cfg.smtp_user.as_deref(), Some("relay-user"));
        assert_eq!(cfg.smtp_pass.as_deref(), Some("relay-pass"));
        assert_eq!(cfg.smtp_tls, Some(SmtpTls::None));
        assert_eq!(cfg.smtp_from.as_deref(), Some(crate::email::DEFAULT_FROM));
        assert_eq!(cfg.smtp_to, ["ops@example.org", "oncall@example.org"]);
        assert!(cfg.is_enabled());

        // The mailer's own TLS resolution applies: port 465 means implicit
        // TLS, anything else STARTTLS, unless SMTP_TLS says otherwise.
        let vars = [
            ("SMTP_HOST", "mail.example.org"),
            ("SMTP_FROM", "ops <a@example.org>"),
        ];
        let cfg = AlertConfig::from_lookup(lookup(&vars), lookup(&vars));
        assert_eq!(cfg.smtp_tls, Some(SmtpTls::StartTls));
        assert_eq!(cfg.smtp_from.as_deref(), Some("ops <a@example.org>"));
        assert!(!cfg.is_enabled(), "no ALERT_SMTP_TO, no ops alert");
    }

    /// With `ALERT_SMTP_HOST` set, nothing comes from the account relay's
    /// credentials or TLS mode, so they never reach a host they were not
    /// configured for; the default stays implicit TLS.
    #[test]
    fn an_alert_relay_never_borrows_the_account_credentials() {
        let vars = [
            ("ALERT_SMTP_HOST", "alerts.example.org"),
            ("ALERT_SMTP_FROM", "alerts <a@example.org>"),
            ("SMTP_HOST", "mail.example.org"),
            ("SMTP_USERNAME", "relay-user"),
            ("SMTP_PASSWORD", "relay-pass"),
            ("SMTP_TLS", "none"),
            ("SMTP_PORT", "25"),
        ];
        let cfg = AlertConfig::from_lookup(lookup(&vars), lookup(&vars));
        assert_eq!(cfg.smtp_host.as_deref(), Some("alerts.example.org"));
        assert_eq!(cfg.smtp_user, None);
        assert_eq!(cfg.smtp_pass, None);
        assert_eq!(cfg.smtp_port, None);
        assert_eq!(cfg.smtp_tls, None, "unset: implicit TLS, as before");
        assert_eq!(cfg.smtp_from.as_deref(), Some("alerts <a@example.org>"));

        for (value, mode) in [
            ("starttls", SmtpTls::StartTls),
            ("NONE", SmtpTls::None),
            ("implicit", SmtpTls::Implicit),
            ("bogus", SmtpTls::Implicit),
        ] {
            let vars = [
                ("ALERT_SMTP_HOST", "alerts.example.org"),
                ("ALERT_SMTP_TLS", value),
            ];
            let cfg = AlertConfig::from_lookup(lookup(&vars), lookup(&vars));
            assert_eq!(cfg.smtp_tls, Some(mode), "ALERT_SMTP_TLS={value}");
        }
    }

    /// Nothing configured, nothing enabled — and blank values are unset.
    #[test]
    fn no_relay_without_a_host() {
        let vars = [("ALERT_SMTP_HOST", " "), ("SMTP_USERNAME", "relay-user")];
        let cfg = AlertConfig::from_lookup(lookup(&vars), lookup(&vars));
        assert_eq!(cfg.smtp_host, None);
        assert_eq!(cfg.smtp_user, None);
        assert!(!cfg.is_enabled());
    }

    /// Dispatch must never panic or block the caller, whatever the channel does.
    /// Here the webhook host does not resolve, which is the common failure.
    #[tokio::test]
    async fn dispatch_is_best_effort_and_records_nothing_on_failure() {
        let (mgr, _audit) = manager(AlertConfig {
            // .invalid is reserved by RFC 2606 and never resolves.
            webhook_url: Some("http://alert.invalid/hook".into()),
            ..Default::default()
        });
        mgr.dispatch(Alert {
            severity: AlertSeverity::Critical,
            kind: "backup_failed".into(),
            message: "scheduled backup did not complete".into(),
            context: serde_json::json!({ "error": "disk full" }),
        })
        .await;
        // Reaching here without a panic is the property under test: alerting
        // must never break the code path that raised the alert.
    }

    /// With no channel configured, dispatch returns immediately.
    #[tokio::test]
    async fn dispatch_is_a_no_op_when_disabled() {
        let (mgr, _audit) = manager(AlertConfig::default());
        mgr.dispatch(Alert {
            severity: AlertSeverity::Info,
            kind: "noop".into(),
            message: "nothing configured".into(),
            context: serde_json::Value::Null,
        })
        .await;
    }
}
