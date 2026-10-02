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
    /// Bearer token sent to the sidecar (`OTS_REASONER_TOKEN`).
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
    /// Read the environment. An unknown `OTS_DL_BACKEND` is logged and leaves
    /// the regime unconfigured (503), rather than silently picking one.
    pub fn from_env() -> Self {
        let mut c = DlConfig::default();
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(b) = var("OTS_DL_BACKEND") {
            match DlBackendKind::parse(&b) {
                Some(kind) => c.backend = Some(kind),
                None => tracing::error!(
                    "OTS_DL_BACKEND={b} is not one of native, konclude, sidecar; \
                     the owl2-dl regime stays unavailable"
                ),
            }
        }
        if var("OTS_EXTERNAL_REASONER").is_some() || var("OTS_EXTERNAL_REASONER_BIN").is_some() {
            tracing::warn!(
                "OTS_EXTERNAL_REASONER / OTS_EXTERNAL_REASONER_BIN were removed; \
                 set OTS_DL_BACKEND=konclude and OTS_KONCLUDE_BIN instead"
            );
        }
        if let Some(b) = var("OTS_KONCLUDE_BIN") {
            c.konclude_bin = b.trim().to_string();
        }
        c.sidecar_url = var("OTS_REASONER_URL").map(|u| u.trim().trim_end_matches('/').to_string());
        c.sidecar_token = var("OTS_REASONER_TOKEN").map(|t| t.trim().to_string());
        if let Some(s) = var("OTS_REASONER_TIMEOUT_SECS").and_then(|v| v.trim().parse().ok()) {
            c.timeout = Duration::from_secs(s);
        }
        if let Some(n) = var("OTS_REASONER_MAX_TRIPLES").and_then(|v| v.trim().parse().ok()) {
            c.max_triples = n;
        }
        if let Some(ms) = var("OTS_DL_DEBOUNCE_MS").and_then(|v| v.trim().parse().ok()) {
            c.debounce = Duration::from_millis(ms);
        }
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
