use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

// Only `AlertManager::send_direct` is used by the server binary; the rest of the
// alerting API is exercised by the library surface/tests.
#[allow(dead_code)]
mod alerting;
mod assets;
mod auth;
mod backup;
mod catalog;
mod commit_log;
mod data_models;
mod dataset_versions;
mod dcat;
mod docs;
mod email;
mod geo;
mod ifc;
mod imports;
mod kind_detector;
#[cfg(feature = "ldp")]
mod ldp;
mod netutil;
mod ogcapi;
mod plugins;
mod prefixes;
mod reasoning;
mod rml;
mod saved_queries;
mod seed_bundles;
mod server;
mod shacl;
mod shacl_studio;
mod shaclc;
#[cfg(feature = "shex")]
mod shex;
mod sparql;
mod storage;
mod store;
mod svc_registry;
#[cfg(feature = "swrl")]
mod swrl;
#[cfg(feature = "text-search")]
mod text_search;
#[cfg(feature = "geometry3d")]
mod tiles3d;

#[derive(Parser, Debug)]
#[command(name = "open-triplestore")]
#[command(about = "A modern RDF triple store with SPARQL 1.1/1.2 and GeoSPARQL support")]
#[command(version)]
struct Cli {
    /// Storage directory for persistent data
    #[arg(short, long, default_value = "./data")]
    data_dir: PathBuf,

    /// HTTP port to listen on
    #[arg(short, long, default_value_t = 7878)]
    port: u16,

    /// Bind address
    #[arg(short, long, default_value = "0.0.0.0")]
    bind: String,

    /// Load an RDF file on startup (Turtle, N-Triples, RDF/XML, N-Quads, TriG)
    #[arg(long)]
    load: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,

    /// JWT signing secret (random if not provided)
    #[arg(long, env = "JWT_SECRET")]
    jwt_secret: Option<String>,

    /// Path to the SQLite identity database
    #[arg(long, env = "AUTH_DB_PATH")]
    db_path: Option<PathBuf>,

    /// S3-compatible endpoint URL (e.g. http://localhost:9000 for MinIO)
    #[arg(long, env = "S3_ENDPOINT")]
    s3_endpoint: Option<String>,

    /// S3 bucket name
    #[arg(long, env = "S3_BUCKET", default_value = "triplestore-assets")]
    s3_bucket: String,

    /// S3 access key
    #[arg(long, env = "S3_ACCESS_KEY")]
    s3_access_key: Option<String>,

    /// S3 secret key
    #[arg(long, env = "S3_SECRET_KEY")]
    s3_secret_key: Option<String>,

    /// S3 region
    #[arg(long, env = "S3_REGION", default_value = "us-east-1")]
    s3_region: String,

    /// Access token expiry in minutes
    #[arg(long, env = "ACCESS_TOKEN_EXPIRY_MINUTES", default_value_t = 30)]
    access_token_expiry_minutes: u64,

    /// Refresh token expiry in days
    #[arg(long, env = "REFRESH_TOKEN_EXPIRY_DAYS", default_value_t = 30)]
    refresh_token_expiry_days: u64,

    /// Promote an existing user to super_admin and exit
    #[arg(long)]
    promote_super_admin: Option<String>,

    /// Restore the store + identity DB from a backup id (in BACKUP_DIR, default
    /// {data-dir}/backups), REPLACING current data, then exit. Encrypted backups
    /// must be decrypted manually first.
    #[arg(long, value_name = "BACKUP_ID")]
    restore: Option<String>,

    /// Allowed CORS origins, comma-separated (e.g. "https://app.example.com,https://admin.example.com").
    /// If empty, same-origin only.
    #[arg(long, env = "CORS_ORIGINS", default_value = "")]
    cors_origins: String,

    /// Base URL for linked data IRIs (e.g. https://example.com)
    #[arg(long, env = "BASE_URL", default_value = "http://localhost:7878")]
    base_url: String,

    /// Enable cross-app service discovery (opt-in). When set, self-register with the registry at
    /// LD_REGISTRY_URL so siblings can resolve this store, and let the web UI resolve siblings too.
    /// Off by default — a missing registry is fail-soft either way; this just makes it explicit.
    #[arg(long, env = "LD_DISCOVERY", default_value = "false", value_parser = parse_lenient_bool)]
    discovery: bool,

    /// service-registry base URL, used only when --discovery/LD_DISCOVERY is set. Fail-soft:
    /// if it's unreachable the triplestore runs exactly as before.
    #[arg(long, env = "LD_REGISTRY_URL", default_value = "http://localhost:8500")]
    registry_url: String,

    /// Bearer token for the service registry (only needed when it binds a non-loopback host).
    #[arg(long, env = "LD_REGISTRY_TOKEN", default_value = "")]
    registry_token: String,

    /// Comma-separated list of trusted reverse-proxy CIDRs whose X-Forwarded-For headers
    /// are honoured for rate limiting (H-2). Empty = no proxy trust; direct TCP IP is used.
    /// Example: "10.0.0.0/8,172.16.0.0/12"
    #[arg(long, env = "TRUSTED_PROXY_CIDRS", default_value = "")]
    trusted_proxy_cidrs: String,

    /// SPARQL query and update execution timeout in seconds (M-1 / W4-21).
    #[arg(long, env = "SPARQL_QUERY_TIMEOUT_SECS", default_value_t = 30)]
    query_timeout_secs: u64,

    /// Write-path execution timeout in seconds for Graph Store PUT/POST/DELETE and
    /// data-model/dataset DELETE/PATCH. Separate from (and larger than) the query
    /// timeout so a stuck write fails fast without starving reads, while ordinary
    /// writes have generous headroom. Bulk import (`/api/import/bulk`) is
    /// intentionally uncapped — large IFC/CityJSON loads legitimately run for minutes.
    #[arg(long, env = "WRITE_TIMEOUT_SECS", default_value_t = 120)]
    write_timeout_secs: u64,

    /// Issue auth cookies with the `Secure` attribute (HTTPS-only transport).
    /// Enable in production behind TLS; leave off for plain-HTTP local development.
    #[arg(long, env = "SECURE_COOKIES", default_value_t = false)]
    secure_cookies: bool,

    /// Serve the bundled web UI (frontend SPA) at `/`. On by default; set to
    /// `false` (or pass `--serve-frontend false`) for a headless, API-only server
    /// — SPARQL, Graph Store and the REST API are unaffected either way.
    #[arg(long, env = "SERVE_FRONTEND", default_value_t = true, action = clap::ArgAction::Set)]
    serve_frontend: bool,

    /// Directory for the Tantivy full-text index (default: {data_dir}/tantivy)
    #[cfg(feature = "text-search")]
    #[arg(long, env = "TEXT_SEARCH_DIR")]
    text_search_dir: Option<PathBuf>,

    /// Directory of seed bundles to load at boot (each a subdirectory with a
    /// `manifest.toml` + RDF payload files — see docs/plugins.md). Idempotent,
    /// fail-soft, and per-bundle opt-out-able; unset by default (no extra
    /// bundles loaded). This is how a downstream operator adds org-owned
    /// datasets without patching source: mount a directory here (e.g. a Docker
    /// volume) instead of forking.
    #[arg(long, env = "SEED_DIR")]
    seed_dir: Option<PathBuf>,

    /// Opt-in fallback to the next free port when `--port`/`-p` is already in
    /// use, instead of refusing to start. The advertised base URL and service-
    /// registry self-registration are rewritten to the port actually bound.
    /// Off by default — existing deployments that rely on "refuse to start on
    /// a busy port" see no behavior change.
    #[arg(long, env = "PORT_FALLBACK", default_value_t = false, value_parser = parse_lenient_bool)]
    port_fallback: bool,
}

/// Best-effort terminal width (columns); 80 when stdout is not a tty.
fn term_width() -> u16 {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        // SAFETY: ws is fully populated by the ioctl before it is read.
        unsafe {
            let mut ws: libc::winsize = std::mem::zeroed();
            if libc::ioctl(std::io::stdout().as_raw_fd(), libc::TIOCGWINSZ, &mut ws) == 0
                && ws.ws_col > 0
            {
                return ws.ws_col;
            }
        }
    }
    80
}

/// Print the brand banner on startup: the "O" is a ring that crosses three
/// triple-nodes arranged as an upside-down triangle (joined by straight
/// edges), then the block-letter "Open" / "Triplestore" wordmark. Falls back
/// to a compact mark on narrow terminals. Teal/dim ANSI colouring is skipped
/// when stdout is not a terminal (e.g. piped logs).
fn print_banner() {
    use std::io::IsTerminal;
    let color = std::io::stdout().is_terminal();
    let (teal, dim, reset) = if color {
        ("\x1b[38;5;80m", "\x1b[38;5;66m", "\x1b[0m")
    } else {
        ("", "", "")
    };
    let v = env!("CARGO_PKG_VERSION");
    const FULL: &[&str] = &[
        "",
        "       ▄▄▄▄▄▄▄▄▄▄▄",
        " ▄██▄█▀▀         ▀▀█▄██▄",
        " ████───────────────████",
        " ▀██▀               ▀██▀",
        " █▀ ╲               ╱ ▀█    ████▄ ▄█▀█▄ ████▄",
        " █    ╲           ╱    █    ██ ██ ██▄█▀ ██ ██",
        " █▄    ╲         ╱    ▄█    ████▀ ▀█▄▄▄ ██ ██",
        "  █     ╲       ╱     █     ██",
        "  ▀█▄    ╲ ▄██▄╱    ▄█▀     ▀▀",
        "    ▀█▄▄   ████  ▄▄█▀",
        "       ▀▀▀▀▀██▀▀▀▀",
        "      ▄▄▄▄▄▄▄▄▄              ▄▄",
        "      ▀▀▀███▀▀▀    ▀▀        ██              ██",
        "         ███ ████▄ ██  ████▄ ██ ▄█▀█▄ ▄█▀▀▀ ▀██▀▀ ▄███▄ ████▄ ▄█▀█▄",
        "         ███ ██ ▀▀ ██  ██ ██ ██ ██▄█▀ ▀███▄  ██   ██ ██ ██ ▀▀ ██▄█▀",
        "         ███ ██    ██▄ ████▀ ██ ▀█▄▄▄ ▄▄▄█▀  ██   ▀███▀ ██    ▀█▄▄▄",
        "                       ██",
        "                       ▀▀",
    ];
    const COMPACT: &[&str] = &[
        "",
        "      ▄▄▄▄▄▄▄▄▄",
        " ▄██▄█▀       ▀█▄██▄",
        " ████───────────████",
        " ▀██▀           ▀██▀   OpenTriplestore",
        " █  ╲           ╱  █   RDF triple store",
        " █   ╲         ╱   █",
        " █    ╲       ╱    █",
        " ▀█    ╲     ╱    █▀",
        "  ▀█▄   ╲▄██▄   ▄█▀",
        "    ▀█▄  ████ ▄█▀",
        "      ▀▀▀▀██▀▀▀",
    ];
    let wide = term_width() >= 68;
    for &line in if wide { FULL } else { COMPACT } {
        println!("{teal}{line}{reset}");
    }
    let tagline = if wide {
        format!("  A modern RDF triple store · v{v} · SPARQL 1.1/1.2 · GeoSPARQL")
    } else {
        format!("  v{v} · SPARQL 1.1/1.2 · GeoSPARQL")
    };
    println!("{dim}{tagline}{reset}");
    println!();
}

/// Parse a lenient boolean for opt-in flags: accepts 1/true/yes/on and 0/false/no/off
/// (case-insensitive), so both `LD_DISCOVERY=1` and `LD_DISCOVERY=true` work.
fn parse_lenient_bool(s: &str) -> Result<bool, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "" | "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!(
            "expected a boolean (true/false/1/0/yes/no/on/off), got {other:?}"
        )),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cli.log_level)),
        )
        .init();

    print_banner();

    info!("Starting open-triplestore v{}", env!("CARGO_PKG_VERSION"));

    // Create data directory if it doesn't exist
    std::fs::create_dir_all(&cli.data_dir)?;

    // Handle --restore: rebuild the store + identity DB from a backup, then exit.
    // Runs before the identity DB is opened so its SQLite file can be replaced.
    if let Some(ref id) = cli.restore {
        let backup_dir = std::env::var("BACKUP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| cli.data_dir.join("backups"));
        let target_sqlite = cli
            .db_path
            .clone()
            .unwrap_or_else(|| cli.data_dir.join("auth.db"));
        info!("Restoring backup {id} from {}…", backup_dir.display());
        let store = store::TripleStore::open(&cli.data_dir)?;
        let manifest = backup::restore_backup(&backup_dir, id, &store, &target_sqlite)?;
        info!(
            "Restored backup {} ({} quads). Restart without --restore to run the server.",
            manifest.id, manifest.rdf_quad_count
        );
        return Ok(());
    }

    // Initialize the auth database (SQLite) — needed by --promote-super-admin and the server.
    // Opened before the RocksDB store so admin operations can run while the server is live.
    let db_path = cli.db_path.unwrap_or_else(|| cli.data_dir.join("auth.db"));
    let auth_db = Arc::new(auth::db::AuthDb::open(&db_path)?);

    // Handle --promote-super-admin: only needs the auth DB, exits immediately.
    // RocksDB is intentionally not opened here so this works while the server is running.
    if let Some(ref username) = cli.promote_super_admin {
        let user = auth_db
            .get_user_by_username(username)?
            .ok_or_else(|| anyhow::anyhow!("User '{}' not found", username))?;
        auth_db.update_user_role(&user.id, auth::models::SystemRole::SuperAdmin)?;
        info!("Promoted user '{}' to super_admin", username);
        return Ok(());
    }

    info!("Auth database at {:?}", db_path);

    // Initialize the store, auto-recovering from RocksDB corruption (e.g. an
    // unclean shutdown that left "SST file is ahead of WALs") so the service comes
    // back instead of crash-looping. See store::recovery.
    let backup_dir = std::env::var("BACKUP_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| cli.data_dir.join("backups"));
    let store = store::recovery::open_store_with_recovery(&cli.data_dir, &backup_dir)?;
    info!("Store opened at {:?}", cli.data_dir);

    // Load initial data if specified
    if let Some(ref load_path) = cli.load {
        info!("Loading data from {:?}", load_path);
        store.load_file(load_path)?;
        info!("Data loaded successfully");
    }

    // Initialize the prefix registry (loads local cache if it exists).
    let prefix_registry = Arc::new(prefixes::PrefixRegistry::open(
        cli.data_dir.join("prefix_cache.json"),
    )?);
    info!(
        "Prefix registry ready (cache at {:?})",
        cli.data_dir.join("prefix_cache.json")
    );

    // Initialize JWT config — prefer explicit secret, then file-backed persistent secret,
    // then a newly-generated secret written to disk so tokens survive restarts.
    let jwt_secret_path = cli.data_dir.join("jwt_secret");
    // Track where the secret came from so a weak-secret failure can name the exact input to fix.
    // An auto-generated secret is 64 random chars and can never be weak, so reaching the weak
    // check below always means the value was explicitly configured (env/CLI or the on-disk file).
    let (jwt_secret, jwt_secret_from_file) = if let Some(s) = cli.jwt_secret {
        (s, false)
    } else {
        match std::fs::read_to_string(&jwt_secret_path) {
            Ok(s) if !s.trim().is_empty() => {
                info!("Loaded JWT secret from {:?}", jwt_secret_path);
                (s.trim().to_string(), true)
            }
            _ => {
                use rand::Rng;
                let secret: String = rand::thread_rng()
                    .sample_iter(&rand::distributions::Alphanumeric)
                    .take(64)
                    .map(char::from)
                    .collect();
                if let Err(e) = std::fs::write(&jwt_secret_path, &secret) {
                    tracing::warn!(
                        "JWT secret auto-generated but could not be saved to {:?}: {}. \
                        Tokens will be invalidated on restart. Set JWT_SECRET env var for production.",
                        jwt_secret_path, e
                    );
                } else {
                    // Restrict permissions so the key is not world-readable (L-3)
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let perms = std::fs::Permissions::from_mode(0o600);
                        if let Err(e) = std::fs::set_permissions(&jwt_secret_path, perms) {
                            tracing::warn!("Could not set 0o600 on jwt_secret file: {}", e);
                        }
                    }
                    info!("Generated JWT secret and saved to {:?}", jwt_secret_path);
                }
                (secret, false)
            }
        }
    };
    // Fail closed on a well-known default/placeholder JWT secret in every mode (not only under
    // --secure-cookies): a public signing key makes every session token forgeable by anyone who can
    // read this source. The message names the specific input and exactly how to fix it.
    if auth::jwt::is_weak_jwt_secret(&jwt_secret) {
        let how_to_fix = if jwt_secret_from_file {
            format!(
                "It was loaded from the file {path}. Replace it with a strong, unique value — e.g. \
                 `openssl rand -hex 32 > {path}` — or delete the file to have one generated \
                 automatically on the next start.",
                path = jwt_secret_path.display(),
            )
        } else {
            "It came from the JWT_SECRET environment variable (or --jwt-secret). Set it to a strong, \
             unique value — e.g. `export JWT_SECRET=$(openssl rand -hex 32)` — or unset it to have \
             one generated automatically."
                .to_string()
        };
        anyhow::bail!(
            "Refusing to start: the configured JWT signing secret is a well-known default/placeholder \
             value, so every session token would be forgeable. {how_to_fix}"
        );
    }
    let jwt_config = Arc::new(auth::jwt::JwtConfig::new(
        jwt_secret,
        cli.access_token_expiry_minutes,
        cli.refresh_token_expiry_days,
    ));

    // Initialize asset storage — S3/MinIO if configured, local filesystem otherwise
    let object_store = if let Some(endpoint) = cli.s3_endpoint {
        let access_key = cli.s3_access_key.unwrap_or_default();
        let secret_key = cli.s3_secret_key.unwrap_or_default();
        let obj_store = storage::ObjectStore::new(
            &endpoint,
            &cli.s3_bucket,
            &access_key,
            &secret_key,
            &cli.s3_region,
        )
        .await?;
        info!(
            "Asset storage: S3 (endpoint={}, bucket={})",
            endpoint, cli.s3_bucket
        );
        Arc::new(obj_store)
    } else {
        let assets_dir = cli.data_dir.join("assets");
        let obj_store = storage::ObjectStore::local(assets_dir.clone())?;
        info!("Asset storage: local filesystem ({:?})", assets_dir);
        Arc::new(obj_store)
    };

    // Initialize the full-text search index (text-search feature)
    #[cfg(feature = "text-search")]
    let text_index = {
        use std::sync::Arc;
        let tantivy_dir = cli
            .text_search_dir
            .unwrap_or_else(|| cli.data_dir.join("tantivy"));
        match text_search::TextIndex::open(&tantivy_dir) {
            Ok(idx) => {
                info!("Text search index opened at {:?}", tantivy_dir);
                Some(Arc::new(idx))
            }
            Err(e) => {
                tracing::warn!(
                    "Text search index could not be opened: {}. Continuing without it.",
                    e
                );
                None
            }
        }
    };

    // Parse trusted proxy CIDRs (H-2)
    let trusted_cidrs: Vec<ipnet::IpNet> = cli
        .trusted_proxy_cidrs
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<ipnet::IpNet>().ok())
        .collect();
    if !trusted_cidrs.is_empty() {
        info!("Trusting XFF headers from {} CIDR(s)", trusted_cidrs.len());
    }

    // Start the HTTP server
    let addr = format!("{}:{}", cli.bind, cli.port);
    info!("SPARQL endpoint available at http://{}/sparql", addr);
    info!("Graph Store Protocol at http://{}/store", addr);
    info!("API at http://{}/api", addr);
    info!("Service description at http://{}/", addr);

    // Cross-app service discovery is opt-in (LD_DISCOVERY); self-registration is
    // handled inside `server::run`, AFTER the listener is bound, so that when
    // `--port-fallback` moves the bind off the requested port the registry sees
    // the base URL rewritten to the port actually in use, not the stale one.
    if !cli.discovery {
        info!(
            "service discovery disabled (set LD_DISCOVERY=true to self-register with the registry)"
        );
    }

    server::run(
        store,
        prefix_registry,
        auth_db,
        jwt_config,
        object_store,
        &cli.base_url,
        &addr,
        &cli.cors_origins,
        trusted_cidrs,
        cli.query_timeout_secs,
        cli.write_timeout_secs,
        cli.secure_cookies,
        cli.serve_frontend,
        cli.seed_dir,
        cli.port_fallback,
        cli.discovery,
        cli.registry_url,
        cli.registry_token,
        #[cfg(feature = "text-search")]
        text_index,
    )
    .await?;

    Ok(())
}
