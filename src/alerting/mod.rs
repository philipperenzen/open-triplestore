//! Operational alerting.
//!
//! Two backends, both opt-in via env vars:
//! - SMTP (gated by the `alerting` Cargo feature, configured via `ALERT_SMTP_*`)
//! - HTTP webhook (always available, configured via `ALERT_WEBHOOK_URL`)
//!
//! Every successful dispatch is recorded in the audit log as `alert_sent`.
//! Failures are logged at WARN but never propagated; alerting must never
//! break the calling code path.

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
    pub smtp_from: Option<String>,
    pub smtp_to: Vec<String>,
    pub webhook_url: Option<String>,
}

impl AlertConfig {
    pub fn from_env() -> Self {
        Self {
            smtp_host: std::env::var("ALERT_SMTP_HOST").ok(),
            smtp_port: std::env::var("ALERT_SMTP_PORT").ok().and_then(|s| s.parse().ok()),
            smtp_user: std::env::var("ALERT_SMTP_USER").ok(),
            smtp_pass: std::env::var("ALERT_SMTP_PASS").ok(),
            smtp_from: std::env::var("ALERT_SMTP_FROM").ok(),
            smtp_to: std::env::var("ALERT_SMTP_TO")
                .ok()
                .map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
                .unwrap_or_default(),
            webhook_url: std::env::var("ALERT_WEBHOOK_URL").ok(),
        }
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
        if !self.cfg.is_enabled() { return; }

        let mut delivered = Vec::new();
        if let Some(url) = &self.cfg.webhook_url {
            match self.http.post(url).json(&alert).send().await {
                Ok(r) if r.status().is_success() => delivered.push("webhook"),
                Ok(r) => tracing::warn!("alerting: webhook returned {}", r.status()),
                Err(e) => tracing::warn!("alerting: webhook failed: {}", e),
            }
        }

        #[cfg(feature = "alerting")]
        if let (Some(host), Some(from), false) =
            (self.cfg.smtp_host.as_ref(), self.cfg.smtp_from.as_ref(), self.cfg.smtp_to.is_empty())
        {
            if let Err(e) = self.send_email(host, from, &alert).await {
                tracing::warn!("alerting: SMTP failed: {}", e);
            } else {
                delivered.push("email");
            }
        }

        if !delivered.is_empty() {
            self.audit.log(
                AuditEventBuilder::new(AuditEventType::AlertSent, AuditOutcome::Success)
                    .details(serde_json::json!({
                        "kind": alert.kind,
                        "severity": alert.severity,
                        "channels": delivered,
                    })),
            );
        }
    }

    /// Send a plain-text email to explicit recipients — used for targeted
    /// notifications (e.g. "your saved query broke") rather than the fixed
    /// ops `smtp_to` list. Best-effort; returns whether anything was delivered.
    /// Requires the `alerting` feature and SMTP (`ALERT_SMTP_*`) configuration.
    #[cfg(feature = "alerting")]
    pub async fn send_direct(&self, to: &[String], subject: &str, body: &str) -> bool {
        use lettre::{message::header::ContentType, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
        let (Some(host), Some(from)) = (self.cfg.smtp_host.as_ref(), self.cfg.smtp_from.as_ref()) else {
            return false;
        };
        if to.is_empty() {
            return false;
        }
        let port = self.cfg.smtp_port.unwrap_or(587);
        let builder = match AsyncSmtpTransport::<Tokio1Executor>::relay(host) {
            Ok(b) => b.port(port),
            Err(e) => {
                tracing::warn!("notify: SMTP relay setup failed: {e}");
                return false;
            }
        };
        let builder = if let (Some(u), Some(p)) = (self.cfg.smtp_user.as_ref(), self.cfg.smtp_pass.as_ref()) {
            builder.credentials(lettre::transport::smtp::authentication::Credentials::new(u.clone(), p.clone()))
        } else {
            builder
        };
        let mailer = builder.build();
        let mut delivered = false;
        for addr in to {
            let from_mbox = match from.parse() {
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
            let msg = Message::builder()
                .from(from_mbox)
                .to(to_mbox)
                .subject(subject)
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

    #[cfg(feature = "alerting")]
    async fn send_email(&self, host: &str, from: &str, alert: &Alert) -> anyhow::Result<()> {
        use lettre::{message::header::ContentType, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
        let port = self.cfg.smtp_port.unwrap_or(587);
        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::relay(host)?.port(port);
        if let (Some(u), Some(p)) = (self.cfg.smtp_user.as_ref(), self.cfg.smtp_pass.as_ref()) {
            builder = builder.credentials(lettre::transport::smtp::authentication::Credentials::new(u.clone(), p.clone()));
        }
        let mailer = builder.build();
        let body = format!(
            "[{:?}] {}\n\n{}\n\nContext:\n{}",
            alert.severity, alert.kind, alert.message,
            serde_json::to_string_pretty(&alert.context).unwrap_or_default(),
        );
        for to in &self.cfg.smtp_to {
            let email = Message::builder()
                .from(from.parse()?)
                .to(to.parse()?)
                .subject(format!("[triplestore][{:?}] {}", alert.severity, alert.kind))
                .header(ContentType::TEXT_PLAIN)
                .body(body.clone())?;
            mailer.send(email).await?;
        }
        Ok(())
    }
}
