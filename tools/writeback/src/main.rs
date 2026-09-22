//! `ots-writeback` — the external writeback worker.
//!
//! ```text
//! ots-writeback --store http://localhost:7878 --token env:OTS_WRITEBACK_TOKEN \
//!   --dataset products --mapping products-map \
//!   --target sqlite:/var/lib/shop/shop.db --state /var/lib/writeback/products.json
//! ```
//!
//! It follows the dataset's LDES stream from where it left off (the state
//! file remembers the last member applied), turns each member into an
//! upsert or a delete under the mapping read backwards, and applies each
//! fragment as one transaction. `--once` processes what is there and exits;
//! `--dry-run` prints the SQL instead of running it; `--from-file` reads a
//! fragment (or a whole stream page) from disk instead of the store, with
//! the mapping from `--mapping-file`.
//!
//! Secrets never appear on the command line: the store token and a
//! PostgreSQL DSN are given as `env:NAME` or `file:/path` references. The
//! target account is the worker's own, writing one — the store's datasource
//! accounts are read-only by contract and are not reused here.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use clap::Parser;
use serde::{Deserialize, Serialize};

use ots_writeback::ldes::{self, Format, Fragment};
use ots_writeback::rml::TableRule;
use ots_writeback::sql::{self, Change, Target};

#[derive(Parser, Debug)]
#[command(name = "ots-writeback", version, about)]
struct Args {
    /// The store's base URL.
    #[arg(long, env = "OTS_WRITEBACK_STORE")]
    store: Option<String>,
    /// The API token, as a secret reference: `env:NAME` or `file:/path`.
    #[arg(
        long,
        env = "OTS_WRITEBACK_TOKEN_REF",
        default_value = "env:OTS_WRITEBACK_TOKEN"
    )]
    token: String,
    /// The dataset whose LDES stream to follow.
    #[arg(long)]
    dataset: Option<String>,
    /// The mapping to read backwards (its id in the store).
    #[arg(long)]
    mapping: Option<String>,
    /// A fragment or stream page on disk, instead of the store.
    #[arg(long)]
    from_file: Option<PathBuf>,
    /// The mapping as a Turtle file, with `--from-file`.
    #[arg(long)]
    mapping_file: Option<PathBuf>,
    /// The target: `sqlite:/path/to.db`, or a PostgreSQL DSN as a reference
    /// (`env:NAME`, `file:/path`) — never inline.
    #[arg(long, env = "OTS_WRITEBACK_TARGET")]
    target: String,
    /// Where the cursor (the last member applied) is kept.
    #[arg(long)]
    state: Option<PathBuf>,
    /// Print the SQL and apply nothing.
    #[arg(long)]
    dry_run: bool,
    /// Process what is there and exit, instead of polling.
    #[arg(long)]
    once: bool,
    /// Seconds between polls of the stream's tail.
    #[arg(long, default_value_t = 30)]
    interval: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// The highest member number applied.
    last_member: i64,
    /// The node the stream was last read at.
    node: Option<String>,
}

impl State {
    fn load(path: &PathBuf) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).context("reading the state file"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).context("reading the state file"),
        }
    }

    fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

/// `env:NAME` or `file:/path`, resolved now. Anything else is refused, so a
/// secret cannot be typed onto a command line by mistake.
fn resolve_ref(reference: &str, what: &str) -> anyhow::Result<String> {
    if let Some(name) = reference.strip_prefix("env:") {
        return std::env::var(name)
            .with_context(|| format!("{what}: environment variable {name} is not set"));
    }
    if let Some(path) = reference.strip_prefix("file:") {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("{what}: reading {path}"))?;
        return Ok(text.trim_end().to_string());
    }
    bail!("{what} must be a secret reference (env:NAME or file:/path), not a value");
}

fn open_target<'a>(
    spec: &str,
    dry_run: bool,
    stdout: &'a mut std::io::Stdout,
) -> anyhow::Result<Box<dyn Target + 'a>> {
    if dry_run {
        return Ok(Box::new(sql::DryRun(stdout)));
    }
    if let Some(path) = spec.strip_prefix("sqlite:") {
        let conn = rusqlite::Connection::open(path).with_context(|| format!("opening {path}"))?;
        return Ok(Box::new(sql::Sqlite(conn)));
    }
    let dsn = resolve_ref(spec, "the target DSN")?;
    if !dsn.starts_with("postgres://")
        && !dsn.starts_with("postgresql://")
        && !dsn.contains("host=")
    {
        bail!("the target DSN is not a PostgreSQL connection string");
    }
    let client =
        postgres::Client::connect(&dsn, postgres::NoTls).context("connecting to PostgreSQL")?;
    Ok(Box::new(sql::Postgres(client)))
}

struct Store {
    base: String,
    token: String,
    http: reqwest::blocking::Client,
}

impl Store {
    fn get(&self, url: &str, accept: &str) -> anyhow::Result<(String, String)> {
        let resp = self
            .http
            .get(url)
            .bearer_auth(&self.token)
            .header("Accept", accept)
            .send()
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let text = resp.text()?;
        if !status.is_success() {
            bail!(
                "GET {url} answered {status}: {}",
                text.chars().take(200).collect::<String>()
            );
        }
        Ok((content_type, text))
    }

    fn mapping_rml(&self, mapping: &str) -> anyhow::Result<String> {
        Ok(self
            .get(
                &format!("{}/api/mappings/{mapping}/rml", self.base),
                "text/turtle",
            )?
            .1)
    }

    fn fragment(&self, url: &str) -> anyhow::Result<Fragment> {
        let (ct, text) = self.get(url, "application/n-triples")?;
        ldes::parse_fragment(&text, Format::of_content_type(&ct)).map_err(|e| anyhow!(e))
    }
}

/// Apply one fragment's members past the cursor; returns how many.
fn apply_fragment(
    fragment: &Fragment,
    rules: &[TableRule],
    state: &mut State,
    target: &mut dyn Target,
) -> anyhow::Result<usize> {
    let fresh: Vec<_> = fragment
        .members
        .iter()
        .filter(|m| m.member_id > state.last_member)
        .cloned()
        .collect();
    if fresh.is_empty() {
        return Ok(0);
    }
    let plan = ots_writeback::plan(&fresh, rules);
    for entity in &plan.skipped {
        tracing::debug!(entity, "no rule recognises this entity; skipped");
    }
    let by_table = plan.changes.iter().fold(
        std::collections::BTreeMap::<String, usize>::new(),
        |mut acc, c| {
            *acc.entry(c.table().to_string()).or_default() += 1;
            acc
        },
    );
    let changes: Vec<Change> = plan.changes;
    target
        .apply(&changes)
        .map_err(|e| anyhow!("applying to the target: {e}"))?;
    tracing::info!(
        members = fresh.len(),
        changes = changes.len(),
        ?by_table,
        "fragment applied"
    );
    state.last_member = fresh
        .iter()
        .map(|m| m.member_id)
        .max()
        .unwrap_or(state.last_member);
    Ok(fresh.len())
}

fn main() -> anyhow::Result<()> {
    // Logs on stderr: stdout is the dry run's SQL and nothing else.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    let mut stdout = std::io::stdout();

    // The rules: from the store's copy of the mapping, or from a file.
    let (rules, warnings) = match (&args.from_file, &args.mapping_file) {
        (Some(_), Some(path)) => {
            let turtle = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            ots_writeback::rml::invert(&turtle).map_err(|e| anyhow!(e))?
        }
        (Some(_), None) => bail!("--from-file needs --mapping-file"),
        (None, _) => {
            let base = args
                .store
                .clone()
                .ok_or_else(|| anyhow!("--store is required"))?;
            let mapping = args
                .mapping
                .clone()
                .ok_or_else(|| anyhow!("--mapping is required"))?;
            let store = Store {
                base: base.trim_end_matches('/').to_string(),
                token: resolve_ref(&args.token, "the store token")?,
                http: reqwest::blocking::Client::builder()
                    .timeout(Duration::from_secs(60))
                    .build()?,
            };
            let turtle = store.mapping_rml(&mapping)?;
            ots_writeback::rml::invert(&turtle).map_err(|e| anyhow!(e))?
        }
    };
    for w in &warnings {
        tracing::warn!("{w}");
    }
    if rules.is_empty() {
        bail!("the mapping has no triples map that can be written back");
    }
    for r in &rules {
        tracing::info!(
            table = %r.table,
            key = %r.key_column,
            columns = r.columns.len(),
            "rule from {}",
            r.triples_map
        );
    }

    let state_path = args.state.clone();
    let mut state = match &state_path {
        Some(p) => State::load(p)?,
        None => State::default(),
    };
    let mut target = open_target(&args.target, args.dry_run, &mut stdout)?;

    if let Some(path) = &args.from_file {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let format = if path.extension().is_some_and(|e| e == "nt") {
            Format::NTriples
        } else {
            Format::Turtle
        };
        let fragment = ldes::parse_fragment(&text, format).map_err(|e| anyhow!(e))?;
        let n = apply_fragment(&fragment, &rules, &mut state, target.as_mut())?;
        if let Some(p) = &state_path {
            state.save(p)?;
        }
        tracing::info!(members = n, "done");
        return Ok(());
    }

    let base = args
        .store
        .clone()
        .unwrap()
        .trim_end_matches('/')
        .to_string();
    let dataset = args
        .dataset
        .clone()
        .ok_or_else(|| anyhow!("--dataset is required"))?;
    let store = Store {
        base: base.clone(),
        token: resolve_ref(&args.token, "the store token")?,
        http: reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?,
    };
    let stream_url = format!("{base}/api/datasets/{dataset}/ldes");
    loop {
        // Resume at the node we last read (its tail may have grown), else
        // at the stream's first node.
        let mut node = match &state.node {
            Some(n) => n.clone(),
            None => {
                let description = store.fragment(&stream_url)?;
                description
                    .view
                    .ok_or_else(|| anyhow!("the stream at {stream_url} names no view"))?
            }
        };
        let mut applied = 0usize;
        loop {
            let fragment = store.fragment(&node)?;
            applied += apply_fragment(&fragment, &rules, &mut state, target.as_mut())?;
            state.node = Some(node.clone());
            if let Some(p) = &state_path {
                state.save(p)?;
            }
            match fragment.next {
                Some(next) if next != node => node = next,
                _ => break,
            }
        }
        if args.once {
            tracing::info!(members = applied, "done");
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(args.interval.max(1)));
    }
}
