//! HTTP client for an OWL 2 DL reasoner sidecar (`OTS_DL_BACKEND=sidecar`).
//!
//! Wire protocol, version 1 (documented in `docs/owl2-dl.md`):
//!
//! * `POST {OTS_REASONER_URL}/v1/reason` — body
//!   `{"data": "<N-Triples>", "timeout_ms": n}`; answer
//!   `{"consistent": true|false|null, "inconsistency"?, "in_profile": bool,
//!   "violations": [{rule, detail}], "unsatisfiable": [IRI], "inferred":
//!   "<N-Triples>", "complete": bool, "backend": {"name", "version"},
//!   "warnings": [..]}`.
//! * `POST {OTS_REASONER_URL}/v1/check` — body `{"task": "consistency" |
//!   "entailment" | "satisfiability", "data", "conclusion"?, "class"?,
//!   "timeout_ms"}`; answer `{"result": "true"|"false"|"unknown", "detail"?,
//!   "in_profile", "violations", "backend", "warnings"}`.
//!
//! `Authorization: Bearer $OTS_REASONER_TOKEN` when a token is configured. A
//! sidecar that cannot be reached, or answers 503, is `Unavailable`; no answer
//! within the time limit (or a 504) is `Timeout`; `in_profile: false` (or a
//! 422 carrying `violations`) is `NotInProfile`.

use std::time::Duration;

use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::Triple;
use serde::Deserialize;

use super::common::{ProfileViolation, ReasoningError};
use super::dl_backend::{CheckOutcome, CheckTask, DlBackend, DlInput, DlOutcome, Tri};
use super::dl_config::DlConfig;

const NAME: &str = "sidecar";

pub struct SidecarBackend {
    url: Option<String>,
    token: Option<String>,
    timeout: Duration,
}

#[derive(Debug, Deserialize, Default)]
struct BackendInfo {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReasonAnswer {
    consistent: Option<bool>,
    #[serde(default)]
    inconsistency: Option<String>,
    #[serde(default = "yes")]
    in_profile: bool,
    #[serde(default)]
    violations: Vec<ProfileViolation>,
    #[serde(default)]
    unsatisfiable: Vec<String>,
    #[serde(default)]
    inferred: String,
    #[serde(default)]
    backend: BackendInfo,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CheckAnswer {
    result: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default = "yes")]
    in_profile: bool,
    #[serde(default)]
    violations: Vec<ProfileViolation>,
    #[serde(default)]
    backend: BackendInfo,
    #[serde(default)]
    warnings: Vec<String>,
}

fn yes() -> bool {
    true
}

fn ntriples(ts: &[Triple]) -> String {
    let mut s = String::with_capacity(ts.len() * 96);
    for t in ts {
        s.push_str(&t.to_string());
        s.push_str(" .\n");
    }
    s
}

fn version(b: &BackendInfo) -> Option<String> {
    match (&b.name, &b.version) {
        (Some(n), Some(v)) => Some(format!("{n} {v}")),
        (Some(n), None) => Some(n.clone()),
        (None, Some(v)) => Some(v.clone()),
        (None, None) => None,
    }
}

impl SidecarBackend {
    pub fn new(cfg: &DlConfig) -> Self {
        SidecarBackend {
            url: cfg.sidecar_url.clone(),
            token: cfg.sidecar_token.clone(),
            timeout: cfg.timeout,
        }
    }

    /// POST `body` to `path` and decode the JSON answer.
    fn post<T: for<'de> Deserialize<'de> + Send + 'static>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> Result<T, ReasoningError> {
        let base = self.url.clone().ok_or_else(|| {
            ReasoningError::Unavailable("OTS_DL_BACKEND=sidecar needs OTS_REASONER_URL".into())
        })?;
        let url = format!("{}{path}", base.trim_end_matches('/'));
        let token = self.token.clone();
        // The sidecar computes under `timeout_ms`; the client waits a little
        // longer (a quarter, at most 5 s) so the sidecar's own answer can
        // arrive first.
        let wait = self.timeout + (self.timeout / 4).min(Duration::from_secs(5));
        let seconds = self.timeout.as_secs();
        let fut = async move {
            let client = reqwest::Client::builder()
                .timeout(wait)
                .build()
                .map_err(|e| ReasoningError::Backend {
                    backend: NAME.into(),
                    detail: e.to_string(),
                })?;
            let mut req = client.post(&url).json(&body);
            if let Some(t) = token {
                req = req.bearer_auth(t);
            }
            let resp = req.send().await.map_err(|e| {
                if e.is_timeout() {
                    ReasoningError::Timeout {
                        backend: NAME.into(),
                        seconds,
                    }
                } else {
                    ReasoningError::Unavailable(format!("reasoner sidecar at {url}: {e}"))
                }
            })?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| {
                if e.is_timeout() {
                    ReasoningError::Timeout {
                        backend: NAME.into(),
                        seconds,
                    }
                } else {
                    ReasoningError::Backend {
                        backend: NAME.into(),
                        detail: e.to_string(),
                    }
                }
            })?;
            match status.as_u16() {
                200 => serde_json::from_str::<T>(&text).map_err(|e| ReasoningError::Backend {
                    backend: NAME.into(),
                    detail: format!("unreadable answer: {e}"),
                }),
                503 => Err(ReasoningError::Unavailable(format!(
                    "reasoner sidecar answered 503: {}",
                    text.chars().take(300).collect::<String>()
                ))),
                504 => Err(ReasoningError::Timeout {
                    backend: NAME.into(),
                    seconds,
                }),
                422 => {
                    #[derive(Deserialize)]
                    struct V {
                        #[serde(default)]
                        violations: Vec<ProfileViolation>,
                    }
                    match serde_json::from_str::<V>(&text) {
                        Ok(v) if !v.violations.is_empty() => Err(ReasoningError::NotInProfile {
                            violations: v.violations,
                        }),
                        _ => Err(ReasoningError::Backend {
                            backend: NAME.into(),
                            detail: format!("422: {}", text.chars().take(300).collect::<String>()),
                        }),
                    }
                }
                s => Err(ReasoningError::Backend {
                    backend: NAME.into(),
                    detail: format!("HTTP {s}: {}", text.chars().take(300).collect::<String>()),
                }),
            }
        };
        // A private runtime on its own thread: the caller is synchronous
        // reasoning code that may or may not sit inside a Tokio context.
        std::thread::scope(|s| {
            s.spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| ReasoningError::Backend {
                        backend: NAME.into(),
                        detail: e.to_string(),
                    })?
                    .block_on(fut)
            })
            .join()
            .unwrap_or_else(|_| {
                Err(ReasoningError::Backend {
                    backend: NAME.into(),
                    detail: "client thread panicked".into(),
                })
            })
        })
    }
}

impl DlBackend for SidecarBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn reason(&self, input: &DlInput) -> Result<DlOutcome, ReasoningError> {
        let a: ReasonAnswer = self.post(
            "/v1/reason",
            serde_json::json!({
                "data": ntriples(&input.triples),
                "timeout_ms": self.timeout.as_millis() as u64,
            }),
        )?;
        if !a.in_profile {
            return Err(ReasoningError::NotInProfile {
                violations: a.violations,
            });
        }
        let inferred: Vec<Triple> = RdfParser::from_format(RdfFormat::NTriples)
            .for_slice(a.inferred.as_bytes())
            .map(|q| q.map(Triple::from))
            .collect::<Result<_, _>>()
            .map_err(|e| ReasoningError::Backend {
                backend: NAME.into(),
                detail: format!("unreadable inferred triples: {e}"),
            })?;
        Ok(DlOutcome {
            consistent: a.consistent,
            inconsistency: a.inconsistency,
            unsatisfiable: a.unsatisfiable,
            inferred,
            version: version(&a.backend),
            warnings: a.warnings,
            incomplete: false,
        })
    }

    fn check(&self, input: &DlInput, task: &CheckTask) -> Result<CheckOutcome, ReasoningError> {
        let mut body = serde_json::json!({
            "task": task.name(),
            "data": ntriples(&input.triples),
            "timeout_ms": self.timeout.as_millis() as u64,
        });
        match task {
            CheckTask::Entailment { conclusion } => {
                body["conclusion"] = ntriples(conclusion).into()
            }
            CheckTask::Satisfiability { class } => body["class"] = class.clone().into(),
            _ => {}
        }
        let a: CheckAnswer = self.post("/v1/check", body)?;
        if !a.in_profile {
            return Err(ReasoningError::NotInProfile {
                violations: a.violations,
            });
        }
        let result = match a.result.as_str() {
            "true" => Tri::True,
            "false" => Tri::False,
            "unknown" => Tri::Unknown,
            other => {
                return Err(ReasoningError::Backend {
                    backend: NAME.into(),
                    detail: format!("result {other:?} is not true, false or unknown"),
                })
            }
        };
        let mut o = CheckOutcome::of(result);
        if matches!(task, CheckTask::Consistency) && result == Tri::False {
            o.inconsistency = Some((
                "external-reasoner".into(),
                a.detail.clone().unwrap_or_else(|| {
                    "the reasoner sidecar found the ontology inconsistent".into()
                }),
            ));
        }
        o.detail = a.detail;
        o.version = version(&a.backend);
        o.warnings = a.warnings;
        Ok(o)
    }
}
