use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::info;

// `send_direct` (targeted saved-query notifications) and `dispatch` (the ops
// webhook/SMTP fan-out, raised on scheduled-backup failure) are both used by the
// server binary. The allow covers the remaining constructors, which the library
// surface and tests exercise.
#[allow(dead_code)]
mod alerting;
mod assets;
mod auth;
mod backup;
mod catalog;
mod commit_log;
mod config;
mod conformance;
mod containers;
mod data_models;
mod dataset_changes;
mod dataset_rules;
mod dataset_settings;
mod dataset_versions;
mod dcat;
mod docs;
mod email;
mod entailment;
mod federation;
mod feedback;
mod geo;
mod ifc;
mod imports;
mod jsonld;
mod kind_detector;
mod ldes;
#[cfg(feature = "ldp")]
mod ldp;
mod netutil;
mod ogcapi;
mod plugins;
mod prefixes;
mod property_states;
mod provenance;
mod rdf_patch;
mod rdf_patch_log;
mod reasoning;
mod remote;
// The binary serves the repair routes; the rest of the module's public
// surface (the report's stable view, the per-store quad cap override) is
// the library's, for its integration tests.
#[allow(dead_code)]
mod repair;
mod rml;
mod saved_queries;
mod secrets;
mod seed_bundles;
mod server;
mod shacl;
mod shacl_studio;
// The server serves SHACL-C through `parse_request` and the store-level
// serializers; `parse`, `Document::base` and the graph-level entry points are
// the library surface the conformance suites use.
#[allow(dead_code, unused_imports)]
mod shaclc;
#[cfg(feature = "shex")]
mod shex;
mod sources;
mod sparql;
mod spec_import;
mod statement_provenance;
mod storage;
mod store;
mod svc_registry;
#[cfg(feature = "swrl")]
mod swrl;
#[cfg(feature = "text-search")]
mod text_search;
#[cfg(feature = "geometry3d")]
mod tiles3d;
mod vocab_search;

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

    /// Promote an existing user to super_admin and exit
    #[arg(long)]
    promote_super_admin: Option<String>,

    /// Restore the store + identity DB from a backup id (in BACKUP_DIR, default
    /// {data-dir}/backups), REPLACING current data, then exit. For an encrypted
    /// backup, set BACKUP_DECRYPT_IDENTITY_PATH to your `age-keygen` identity
    /// file — the server stores only the recipient, never the identity.
    #[arg(long, value_name = "BACKUP_ID")]
    restore: Option<String>,
    // Every other flag is a setting from the configuration registry
    // (`src/config/registry.rs`), added by [`settings_args`] and validated
    // with the environment by `Config::load`.
}

/// One flag per registry setting that has one. A flag wins over the
/// environment; its value goes through the same parser.
fn settings_args(mut cmd: clap::Command) -> clap::Command {
    for spec in config::SPECS {
        let Some(long) = spec.cli else { continue };
        let mut arg = clap::Arg::new(spec.name)
            .long(long)
            .value_name("VALUE")
            .help(format!("{} [env: {}]", spec.doc, spec.name));
        if let Some(c) = spec.short {
            arg = arg.short(c);
        }
        if matches!(spec.kind, config::Kind::Bool) {
            // `--secure-cookies` alone means true; `--secure-cookies=false` works too.
            arg = arg.num_args(0..=1).default_missing_value("true");
        }
        cmd = cmd.arg(arg);
    }
    cmd
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = settings_args(<Cli as clap::CommandFactory>::command()).get_matches();
    let cli =
        <Cli as clap::FromArgMatches>::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    // Every setting, from the flags and the environment, validated together:
    // any problem — a number that does not parse, an unknown enum value, a
    // boolean spelled some other way, a retired name — stops the start, and
    // all of them are reported at once.
    let cli_settings: std::collections::HashMap<String, String> = config::SPECS
        .iter()
        .filter(|s| s.cli.is_some())
        .filter_map(|s| {
            matches
                .get_one::<String>(s.name)
                .map(|v| (s.name.to_string(), v.clone()))
        })
        .collect();
    let settings = match config::Config::load(&|name| std::env::var(name).ok(), &cli_settings) {
        Ok(c) => Arc::new(c),
        Err(errors) => {
            eprintln!(
                "Refusing to start: {} configuration error{}:",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" }
            );
            for e in &errors {
                eprintln!("  {e}");
            }
            std::process::exit(2);
        }
    };
    config::install(settings.clone());
    // Where backups live, read once: the restore path, store auto-recovery and
    // the backup subsystem all use this value (an empty BACKUP_DIR means unset).
    let backup_dir = server::default_backup_dir(&cli.data_dir);

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cli.log_level)),
        )
        .init();

    print_banner();
    for name in config::unknown_names(std::env::vars().map(|(k, _)| k)) {
        tracing::warn!("{name} is set but is not a setting this version reads (a typo?)");
    }

    info!("Starting open-triplestore v{}", env!("CARGO_PKG_VERSION"));

    // Create data directory if it doesn't exist
    std::fs::create_dir_all(&cli.data_dir)?;

    // Handle --restore: rebuild the store + identity DB from a backup, then exit.
    // Runs before the identity DB is opened so its SQLite file can be replaced.
    if let Some(ref id) = cli.restore {
        let target_sqlite = config::AUTH_DB_PATH
            .get()
            .unwrap_or_else(|| cli.data_dir.join("auth.db"));
        // Only needed for an age-encrypted backup; the server never holds this
        // identity, so restoring one is an explicit operator action.
        let identity = config::BACKUP_DECRYPT_IDENTITY_PATH.get();
        info!("Restoring backup {id} from {}…", backup_dir.display());
        let store = store::TripleStore::open(&cli.data_dir)?;
        let manifest =
            backup::restore_backup(&backup_dir, id, &store, &target_sqlite, identity.as_deref())?;
        info!(
            "Restored backup {} ({} quads). Restart without --restore to run the server.",
            manifest.id, manifest.rdf_quad_count
        );
        return Ok(());
    }

    // A restore an admin staged in the UI (POST /api/admin/backup/:id/restore)
    // runs now, like --restore, before the identity DB and the store open.
    // Not for --promote-super-admin, which must work beside a live server.
    if cli.promote_super_admin.is_none() {
        let target_sqlite = config::AUTH_DB_PATH
            .get()
            .unwrap_or_else(|| cli.data_dir.join("auth.db"));
        let identity = config::BACKUP_DECRYPT_IDENTITY_PATH.get();
        backup::apply_pending_restore(
            &backup_dir,
            &cli.data_dir,
            &target_sqlite,
            identity.as_deref(),
        );
    }

    // Initialize the auth database (SQLite) — needed by --promote-super-admin and the server.
    // Opened before the RocksDB store so admin operations can run while the server is live.
    let db_path = config::AUTH_DB_PATH
        .get()
        .unwrap_or_else(|| cli.data_dir.join("auth.db"));
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
    let store = store::recovery::open_store_with_recovery(&cli.data_dir, &backup_dir)?;
    info!("Store opened at {:?}", cli.data_dir);

    // Load initial data if specified
    if let Some(ref load_path) = cli.load {
        info!("Loading data from {:?}", load_path);
        store.load_file(load_path)?;
        info!("Data loaded successfully");
    }

    // Initialize the prefix registry: bundled prefix.cc/LOV snapshot +
    // persisted lookup cache.  Live prefix.cc is only contacted when the
    // operator opts in with PREFIX_CC_FALLBACK=true.
    let prefix_cc_fallback = config::PREFIX_CC_FALLBACK.get();
    let prefix_registry = Arc::new(prefixes::PrefixRegistry::open(
        cli.data_dir.join("prefix_cache.json"),
        prefix_cc_fallback,
    )?);
    info!(
        "Prefix registry ready ({} bundled prefixes, cache at {:?}, prefix.cc fallback {})",
        prefix_registry.dataset_len(),
        cli.data_dir.join("prefix_cache.json"),
        if prefix_cc_fallback { "on" } else { "off" }
    );

    // Initialize JWT config — prefer explicit secret, then file-backed persistent secret,
    // then a newly-generated secret written to disk so tokens survive restarts.
    let jwt_secret_path = cli.data_dir.join("jwt_secret");
    // Track where the secret came from so a weak-secret failure can name the exact input to fix.
    // An auto-generated secret is 64 random chars and can never be weak, so reaching the weak
    // check below always means the value was explicitly configured (env/CLI or the on-disk file).
    // A configured secret goes through the secrets module: a reference
    // (env:/file:/vault:) resolves, a raw value is refused in the production
    // posture and warned about once in development — as S3_SECRET_KEY is.
    let (jwt_secret, jwt_secret_from_file) = if let Some(s) = config::JWT_SECRET.get() {
        (auth::jwt::configured_jwt_secret(&s)?, false)
    } else {
        match std::fs::read_to_string(&jwt_secret_path) {
            Ok(s) if !s.trim().is_empty() => {
                info!("Loaded JWT secret from {:?}", jwt_secret_path);
                (s.trim().to_string(), true)
            }
            _ => {
                use rand::RngExt;
                let secret: String = rand::rng()
                    .sample_iter(&rand::distr::Alphanumeric)
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
    let jwt_config = Arc::new(auth::jwt::JwtConfig::with_expiry(
        jwt_secret,
        config::ACCESS_TOKEN_EXPIRY.get(),
        config::REFRESH_TOKEN_EXPIRY.get(),
    ));

    // The service-registry bearer, likewise: a reference resolves here, a raw
    // value is refused under OTS_ENV=production.
    let registry_token = match config::LD_REGISTRY_TOKEN.get() {
        None => String::new(),
        Some(v) => secrets::resolve_configured("LD_REGISTRY_TOKEN", &v)
            .map_err(|e| anyhow::anyhow!("LD_REGISTRY_TOKEN: {e}"))?
            .expose()
            .to_string(),
    };

    // Initialize asset storage — S3/MinIO if configured, local filesystem otherwise
    let object_store = if let Some(endpoint) = config::S3_ENDPOINT.get() {
        let access_key = config::S3_ACCESS_KEY.get().unwrap_or_default();
        let bucket = config::S3_BUCKET.get();
        // A secret reference (env:/file:/vault:) is resolved here; a raw value
        // is refused in the production posture and warned about elsewhere.
        let secret_key = match config::S3_SECRET_KEY.get() {
            Some(v) => secrets::resolve_configured("S3_SECRET_KEY", &v)
                .map_err(|e| anyhow::anyhow!("S3_SECRET_KEY: {e}"))?
                .expose()
                .to_string(),
            None => String::new(),
        };
        let obj_store = storage::ObjectStore::new(
            &endpoint,
            &bucket,
            &access_key,
            &secret_key,
            &config::S3_REGION.get(),
        )
        .await?;
        info!(
            "Asset storage: S3 (endpoint={}, bucket={})",
            endpoint, bucket
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
        let tantivy_dir = config::TEXT_SEARCH_DIR
            .get()
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
    if !config::LD_DISCOVERY.get() {
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
        settings,
        &addr,
        registry_token,
        cli.data_dir.clone(),
        db_path.clone(),
        backup_dir,
        #[cfg(feature = "text-search")]
        text_index,
        #[cfg(feature = "vocab-search")]
        Some(Arc::new(vocab_search::index::VocabSearchEngine::new(
            cli.data_dir.clone(),
        ))),
    )
    .await?;

    Ok(())
}
