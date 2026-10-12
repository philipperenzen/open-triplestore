//! Configuration of the OWL 2 DL backend (`OTS_DL_BACKEND` and friends).
//!
//! The `owl2-dl` regime has no default backend: unless one is configured it
//! answers 503, because the native rules are not a complete OWL 2 DL reasoner
//! and must be chosen knowingly (`OTS_DL_BACKEND=native`).

use std::path::PathBuf;
use std::time::Duration;

/// The backend that serves `owl2-dl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DlBackendKind {
    /// OWL 2 RL plus the DL-syntax rules, in process. Sound, not complete.
    Native,
    /// The Konclude binary, driven through OWLlink and SPARQL files.
    Konclude,
    /// An HTTP reasoner sidecar speaking the protocol in `docs/owl2-dl.md`.
    Sidecar,
}

impl DlBackendKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "native" => Some(DlBackendKind::Native),
            "konclude" => Some(DlBackendKind::Konclude),
            "sidecar" => Some(DlBackendKind::Sidecar),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            DlBackendKind::Native => "native",
            DlBackendKind::Konclude => "konclude",
            DlBackendKind::Sidecar => "sidecar",
        }
    }
}

pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
pub const DEFAULT_MAX_TRIPLES: usize = 1_000_000;
pub const DEFAULT_DEBOUNCE_MS: u64 = 2_000;

#[derive(Debug, Clone)]
pub struct DlConfig {
    /// `None`: no backend — `owl2-dl` answers 503.
    pub backend: Option<DlBackendKind>,
    /// Konclude binary (`OTS_KONCLUDE_BIN`, default `Konclude` on `PATH`).
    pub konclude_bin: String,
    /// Sidecar base URL (`OTS_REASONER_URL`).
    pub sidecar_url: Option<String>,
    /// Bearer token sent to the sidecar (`OTS_REASONER_TOKEN`, a secret
    /// reference or, outside production, a raw value).
    pub sidecar_token: Option<String>,
    /// Time limit per backend call (`OTS_REASONER_TIMEOUT_SECS`).
    pub timeout: Duration,
    /// Largest input handed to an external backend (`OTS_REASONER_MAX_TRIPLES`).
    pub max_triples: usize,
    /// Quiet period after the last write before a dataset's DL run starts
    /// (`OTS_DL_DEBOUNCE_MS`).
    pub debounce: Duration,
    /// Where Konclude's input and output files go (system temp dir).
    pub work_dir: PathBuf,
}

impl Default for DlConfig {
    fn default() -> Self {
        DlConfig {
            backend: None,
            konclude_bin: "Konclude".to_string(),
            sidecar_url: None,
            sidecar_token: None,
            timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            max_triples: DEFAULT_MAX_TRIPLES,
            debounce: Duration::from_millis(DEFAULT_DEBOUNCE_MS),
            work_dir: std::env::temp_dir(),
        }
    }
}

impl DlConfig {
    /// Read the configuration (validated at startup: an unknown
    /// `OTS_DL_BACKEND` stops the start).
    pub fn from_env() -> Self {
        use crate::config as cfg;
        let mut c = DlConfig {
            backend: cfg::DL_BACKEND.get().and_then(|b| DlBackendKind::parse(&b)),
            konclude_bin: cfg::KONCLUDE_BIN.get(),
            sidecar_url: cfg::REASONER_URL
                .get()
                .map(|u| u.trim_end_matches('/').to_string()),
            timeout: cfg::REASONER_TIMEOUT.get(),
            max_triples: cfg::REASONER_MAX_TRIPLES.get(),
            debounce: cfg::DL_DEBOUNCE.get(),
            ..DlConfig::default()
        };
        // A secret reference (`env:`, `file:`, `vault:`) like `LLM_API_KEY`: a
        // raw token is accepted with a warning outside the production posture
        // and refused in it; one that does not resolve leaves the sidecar
        // without a token (it then refuses the call) rather than sending the
        // reference text.
        c.sidecar_token = crate::secrets::env_secret_opt("OTS_REASONER_TOKEN")
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        if c.backend == Some(DlBackendKind::Sidecar) && c.sidecar_url.is_none() {
            tracing::error!("OTS_DL_BACKEND=sidecar needs OTS_REASONER_URL");
        }
        c
    }

    /// The configured backend, for tests and embedding.
    pub fn with_backend(mut self, kind: DlBackendKind) -> Self {
        self.backend = Some(kind);
        self
    }
}
