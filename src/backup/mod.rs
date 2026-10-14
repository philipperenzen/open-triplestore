//! Backup subsystem.
//!
//! Produces gzipped N-Quads dumps of the RDF store and online SQLite backups
//! of the auth database, both written to a configurable backup directory.
//! Each backup writes a JSON manifest with SHA-256 checksums so it can be
//! verified later without trusting filesystem mtime.
//!
//! Optional `age` X25519 encryption is applied to both files when the
//! `backup-encrypt` feature is enabled and `OTS_BACKUP_ENCRYPTION_ENABLED=true`.

use anyhow::Context;
use chrono::Utc;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::auth::audit::{AuditEventBuilder, AuditEventType, AuditLogger, AuditOutcome};
use crate::store::TripleStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub id: String,
    pub created_at: String,
    pub software_version: String,
    pub rdf_path: String,
    pub rdf_sha256: String,
    pub rdf_quad_count: usize,
    pub sqlite_path: String,
    pub sqlite_sha256: String,
    pub encrypted: bool,
}

/// Resolve the age recipient used to encrypt backups.
///
/// The recipient must be supplied by the operator at `OTS_BACKUP_ENCRYPTION_KEY_PATH`.
/// This deliberately does NOT generate one: it used to, by calling
/// `Identity::generate()`, writing only `to_public()` and letting the identity
/// drop at the end of the scope. The private half was never printed or
/// persisted, so every backup written under that recipient was encrypted to a
/// key that existed nowhere — unrecoverable the moment it was needed, with the
/// failure invisible until a restore was attempted. Refusing to start is the
/// only honest behaviour: an operator who wants encryption must run
/// `age-keygen` and keep the identity somewhere the server cannot lose it.
pub fn init_backup_encryption(key_path: &Path) -> anyhow::Result<Option<PathBuf>> {
    #[cfg(feature = "backup-encrypt")]
    {
        if !key_path.exists() {
            anyhow::bail!(
                "OTS_BACKUP_ENCRYPTION_ENABLED=true but no age recipient exists at {}. Generate a keypair with \
                 `age-keygen -o backup-identity.txt`, store that identity file somewhere safe and \
                 OUTSIDE the data directory, then write its public line (age1…) to {}. Set \
                 OTS_BACKUP_DECRYPTION_IDENTITY_PATH to the identity file to allow automated restore.",
                key_path.display(),
                key_path.display()
            );
        }
        let key_content = fs::read_to_string(key_path)
            .with_context(|| format!("read existing age recipient key {}", key_path.display()))?;
        key_content
            .trim()
            .parse::<age::x25519::Recipient>()
            .map_err(|e: &str| {
                anyhow::anyhow!("invalid age recipient in {}: {}", key_path.display(), e)
            })?;
        tracing::info!(
            "Using backup encryption recipient at {}",
            key_path.display()
        );
        Ok(Some(key_path.to_path_buf()))
    }
    #[cfg(not(feature = "backup-encrypt"))]
    {
        let _ = key_path;
        tracing::error!(
            "OTS_BACKUP_ENCRYPTION_ENABLED=true but binary was compiled without `backup-encrypt` feature"
        );
        anyhow::bail!("backup encryption not available: recompile with --features backup-encrypt")
    }
}

#[derive(Clone)]
pub struct BackupManager {
    backup_dir: PathBuf,
    sqlite_path: PathBuf,
    store: TripleStore,
    audit: Arc<AuditLogger>,
    retention: usize,
    encrypt: bool,
    #[allow(dead_code)] // only read when `backup-encrypt` feature enabled
    encrypt_key_path: Option<PathBuf>,
}

impl BackupManager {
    pub fn new(
        backup_dir: PathBuf,
        sqlite_path: PathBuf,
        store: TripleStore,
        audit: Arc<AuditLogger>,
        retention: usize,
        encrypt: bool,
        encrypt_key_path: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        fs::create_dir_all(&backup_dir)
            .with_context(|| format!("create backup dir {}", backup_dir.display()))?;
        Ok(Self {
            backup_dir,
            sqlite_path,
            store,
            audit,
            retention,
            encrypt,
            encrypt_key_path,
        })
    }

    pub fn backup_dir(&self) -> &Path {
        &self.backup_dir
    }

    pub fn retention(&self) -> usize {
        self.retention
    }

    pub fn encrypts(&self) -> bool {
        self.encrypt
    }

    /// Run one backup cycle: dump RDF + SQLite, write manifest, prune old
    /// snapshots, log to audit. Returns the manifest of the new backup.
    pub fn run_once(&self) -> anyhow::Result<BackupManifest> {
        match self.run_once_inner() {
            Ok(m) => {
                self.audit.log(
                    AuditEventBuilder::new(AuditEventType::BackupCreated, AuditOutcome::Success)
                        .resource("backup", &m.id)
                        .details(serde_json::to_value(&m).unwrap_or(serde_json::Value::Null)),
                );
                Ok(m)
            }
            Err(e) => {
                self.audit.log(
                    AuditEventBuilder::new(AuditEventType::BackupFailed, AuditOutcome::Failure)
                        .details(serde_json::json!({ "error": e.to_string() })),
                );
                Err(e)
            }
        }
    }

    fn run_once_inner(&self) -> anyhow::Result<BackupManifest> {
        let id = format!("backup-{}", Utc::now().format("%Y%m%dT%H%M%SZ"));
        let dir = self.backup_dir.join(&id);
        fs::create_dir_all(&dir)?;
        // 0o700 on the per-backup directory.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }

        // ── RDF dump (gzipped N-Quads) ────────────────────────────────────
        let rdf_name = if self.encrypt {
            "rdf.nq.gz.age"
        } else {
            "rdf.nq.gz"
        };
        let rdf_path = dir.join(rdf_name);
        let mut rdf_buf: Vec<u8> = Vec::new();
        let quad_count = {
            let mut gz = GzEncoder::new(&mut rdf_buf, Compression::default());
            let n = self.store.dump_all_nquads(&mut gz)?;
            gz.finish()?;
            n
        };
        let rdf_bytes = self.maybe_encrypt(&rdf_buf)?;
        write_secure(&rdf_path, &rdf_bytes)?;
        let rdf_sha256 = sha256_hex(&rdf_bytes);

        // ── SQLite online backup ──────────────────────────────────────────
        let sqlite_name = if self.encrypt {
            "auth.sqlite.age"
        } else {
            "auth.sqlite"
        };
        let sqlite_out = dir.join(sqlite_name);
        let raw_sqlite_tmp = dir.join("auth.sqlite.raw");
        sqlite_online_backup(&self.sqlite_path, &raw_sqlite_tmp)?;
        let raw = fs::read(&raw_sqlite_tmp)?;
        let sqlite_bytes = self.maybe_encrypt(&raw)?;
        write_secure(&sqlite_out, &sqlite_bytes)?;
        fs::remove_file(&raw_sqlite_tmp).ok();
        let sqlite_sha256 = sha256_hex(&sqlite_bytes);

        let manifest = BackupManifest {
            id: id.clone(),
            created_at: Utc::now().to_rfc3339(),
            software_version: env!("CARGO_PKG_VERSION").to_string(),
            rdf_path: rdf_path.file_name().unwrap().to_string_lossy().into(),
            rdf_sha256,
            rdf_quad_count: quad_count,
            sqlite_path: sqlite_out.file_name().unwrap().to_string_lossy().into(),
            sqlite_sha256,
            encrypted: self.encrypt,
        };
        write_secure(
            &dir.join("manifest.json"),
            serde_json::to_string_pretty(&manifest)?.as_bytes(),
        )?;

        self.prune()?;
        Ok(manifest)
    }

    /// Recompute SHA-256 of a backup's files and compare to its manifest.
    pub fn verify(&self, id: &str) -> anyhow::Result<bool> {
        validate_backup_id(id)?;
        let dir = self.backup_dir.join(id);
        let manifest = read_manifest(&dir, id)?;
        let rdf_ok = sha256_hex(&fs::read(dir.join(&manifest.rdf_path))?) == manifest.rdf_sha256;
        let sqlite_ok =
            sha256_hex(&fs::read(dir.join(&manifest.sqlite_path))?) == manifest.sqlite_sha256;
        let ok = rdf_ok && sqlite_ok;
        self.audit.log(
            AuditEventBuilder::new(
                AuditEventType::BackupVerified,
                if ok {
                    AuditOutcome::Success
                } else {
                    AuditOutcome::Failure
                },
            )
            .resource("backup", id)
            .details(serde_json::json!({ "rdf_ok": rdf_ok, "sqlite_ok": sqlite_ok })),
        );
        Ok(ok)
    }

    pub fn list(&self) -> anyhow::Result<Vec<BackupManifest>> {
        let mut out = Vec::new();
        if !self.backup_dir.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&self.backup_dir)? {
            let e = entry?;
            let m_path = e.path().join("manifest.json");
            if m_path.exists() {
                if let Ok(bytes) = fs::read(&m_path) {
                    if let Ok(m) = serde_json::from_slice::<BackupManifest>(&bytes) {
                        out.push(m);
                    }
                }
            }
        }
        out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(out)
    }

    fn prune(&self) -> anyhow::Result<()> {
        let mut all = self.list()?;
        if all.len() <= self.retention {
            return Ok(());
        }
        let to_remove = all.split_off(self.retention);
        // A backup staged for restore survives retention until it is applied
        // or cancelled: pruning it would turn the restart into a failure.
        let pending = pending_restore(&self.backup_dir).map(|p| p.id);
        for m in to_remove {
            if pending.as_deref() == Some(m.id.as_str()) {
                continue;
            }
            let _ = fs::remove_dir_all(self.backup_dir.join(&m.id));
        }
        Ok(())
    }

    fn maybe_encrypt(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        if !self.encrypt {
            return Ok(data.to_vec());
        }
        #[cfg(feature = "backup-encrypt")]
        {
            let key_path = self.encrypt_key_path.as_ref().ok_or_else(|| {
                anyhow::anyhow!("OTS_BACKUP_ENCRYPTION_ENABLED=true but OTS_BACKUP_ENCRYPTION_KEY_PATH is unset")
            })?;
            let pubkey_str = fs::read_to_string(key_path)
                .with_context(|| format!("read age recipient key {}", key_path.display()))?;
            let recipient: age::x25519::Recipient = pubkey_str
                .trim()
                .parse()
                .map_err(|e: &str| anyhow::anyhow!("invalid age recipient: {}", e))?;
            // age 0.12 takes an iterator of `&dyn Recipient` (was a boxed Vec) and
            // reports recipient problems as `Err` rather than `None`.
            let encryptor =
                age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                    .map_err(|e| anyhow::anyhow!("age encryptor init failed: {e}"))?;
            let mut out = Vec::new();
            let mut writer = encryptor.wrap_output(&mut out)?;
            writer.write_all(data)?;
            writer.finish()?;
            Ok(out)
        }
        #[cfg(not(feature = "backup-encrypt"))]
        {
            let _ = data;
            anyhow::bail!(
                "OTS_BACKUP_ENCRYPTION_ENABLED=true but binary was built without `backup-encrypt` feature"
            );
        }
    }
}

/// Decrypt both halves of an age-encrypted backup with the operator's identity
/// from `OTS_BACKUP_DECRYPTION_IDENTITY_PATH`.
fn decrypt_backup_pair(
    id: &str,
    rdf_raw: &[u8],
    sqlite_raw: &[u8],
    identity_path: Option<&Path>,
) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
    #[cfg(feature = "backup-encrypt")]
    {
        use std::io::Read;

        let identity_path = identity_path.ok_or_else(|| {
            anyhow::anyhow!(
                "backup {id} is age-encrypted; set OTS_BACKUP_DECRYPTION_IDENTITY_PATH to the age \
                 identity file (the private half from `age-keygen`) to restore it. The server \
                 stores only the recipient, never the identity."
            )
        })?;
        let identity_path = identity_path.display();
        let identity_text = fs::read_to_string(identity_path.to_string())
            .with_context(|| format!("read age identity {identity_path}"))?;
        // An age-keygen file carries `# public key:` comment lines around the
        // AGE-SECRET-KEY line; take the first parseable identity.
        let identity: age::x25519::Identity = identity_text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .find_map(|l| l.parse().ok())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no AGE-SECRET-KEY found in {identity_path}; expected an \
                                 `age-keygen` identity file"
                )
            })?;

        let decrypt = |data: &[u8], what: &str| -> anyhow::Result<Vec<u8>> {
            let decryptor = age::Decryptor::new(data)
                .with_context(|| format!("read age header of the {what} of backup {id}"))?;
            let mut reader = decryptor
                .decrypt(std::iter::once(&identity as &dyn age::Identity))
                .with_context(|| {
                    format!(
                        "decrypt the {what} of backup {id} — the identity in {identity_path} does \
                         not match the recipient it was encrypted to"
                    )
                })?;
            let mut out = Vec::new();
            reader.read_to_end(&mut out)?;
            Ok(out)
        };

        Ok((
            decrypt(rdf_raw, "RDF dump")?,
            decrypt(sqlite_raw, "SQLite DB")?,
        ))
    }
    #[cfg(not(feature = "backup-encrypt"))]
    {
        let _ = (rdf_raw, sqlite_raw, identity_path);
        anyhow::bail!(
            "backup {id} is age-encrypted but this binary was built without the \
             `backup-encrypt` feature; rebuild with --features backup-encrypt to restore it"
        )
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex::encode(h.finalize())
}

fn write_secure(path: &Path, data: &[u8]) -> anyhow::Result<()> {
    let mut f = File::create(path)?;
    f.write_all(data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Online SQLite backup using `rusqlite::backup::Backup`. Safe under WAL.
fn sqlite_online_backup(src: &Path, dst: &Path) -> anyhow::Result<()> {
    use rusqlite::Connection;
    let src_conn = Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut dst_conn = Connection::open(dst)?;
    let backup = rusqlite::backup::Backup::new(&src_conn, &mut dst_conn)?;
    backup.run_to_completion(100, std::time::Duration::from_millis(50), None)?;
    Ok(())
}

// ─── Admin HTTP handlers ────────────────────────────────────────────────────

/// `POST /api/admin/backup` — trigger a backup synchronously, and copy it to
/// S3 when `OTS_BACKUP_S3_ENABLED=true` (as the scheduled backups are). The upload
/// outcome is in `s3` (`null` when S3 is off); a failed upload does not fail
/// the backup, which is complete on disk. super_admin only.
pub async fn admin_create_backup(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(state): axum::extract::State<crate::server::AppState>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let mgr = state.backup.clone().ok_or((
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Backup not configured".into(),
    ))?;
    let mgr2 = mgr.clone();
    let m = tokio::task::spawn_blocking(move || mgr2.run_once())
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let s3 = if s3_enabled() && state.object_store.is_configured() {
        let r = maybe_upload_to_s3(&state.object_store, &mgr, &m.id).await;
        Some(match r {
            Ok(()) => serde_json::json!({ "uploaded": true }),
            Err(e) => {
                tracing::warn!("backup: S3 upload failed: {}", e);
                serde_json::json!({ "uploaded": false, "error": e.to_string() })
            }
        })
    } else {
        None
    };
    let mut body = serde_json::to_value(&m)
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    body["s3"] = s3.unwrap_or(serde_json::Value::Null);
    Ok(axum::Json(body))
}

/// `GET /api/admin/backup` — list manifests. super_admin only.
pub async fn admin_list_backups(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(mgr): axum::extract::State<Option<Arc<BackupManager>>>,
) -> Result<axum::Json<Vec<BackupManifest>>, (axum::http::StatusCode, String)> {
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let mgr = mgr.ok_or((
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Backup not configured".into(),
    ))?;
    let list = mgr
        .list()
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(axum::Json(list))
}

/// `POST /api/admin/backup/:id/verify` — recompute checksums. super_admin only.
pub async fn admin_verify_backup(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(mgr): axum::extract::State<Option<Arc<BackupManager>>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let mgr = mgr.ok_or((
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Backup not configured".into(),
    ))?;
    let mgr2 = mgr.clone();
    let id2 = id.clone();
    let ok = tokio::task::spawn_blocking(move || mgr2.verify(&id2))
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(axum::Json(serde_json::json!({ "id": id, "verified": ok })))
}

/// Reject a backup `id` that could escape the backup directory (path traversal).
/// IDs are server-minted (`backup-<timestamp>`); this guards the admin routes that
/// take an `id` path param before it is `join`ed onto the backup dir.
fn validate_backup_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        anyhow::bail!("invalid backup id");
    }
    Ok(())
}

/// Reject a manifest file-name (`rdf_path` / `sqlite_path`) that could escape the
/// per-backup directory. Manifests are written by us, but they live on disk and
/// could be tampered with, so these names are untrusted when read back: require a
/// bare file name with no path separators, `..`, or absolute-path prefix before
/// it is `join`ed onto the backup dir.
fn validate_backup_file_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || Path::new(name).is_absolute()
    {
        anyhow::bail!("invalid backup file name");
    }
    Ok(())
}

/// Read the manifest of the backup in `dir`, refusing file names that would
/// leave that directory: `dir.join` of an absolute path *replaces* `dir`, and
/// `..` climbs out of it, so an edited manifest could otherwise point verify,
/// restore or the S3 upload at any file on the host.
fn read_manifest(dir: &Path, id: &str) -> anyhow::Result<BackupManifest> {
    let manifest: BackupManifest = serde_json::from_slice(
        &fs::read(dir.join("manifest.json"))
            .with_context(|| format!("read manifest for backup {id}"))?,
    )?;
    validate_backup_file_name(&manifest.rdf_path)?;
    validate_backup_file_name(&manifest.sqlite_path)?;
    Ok(manifest)
}

/// Restore a backup into `store` and the SQLite identity DB at `target_sqlite`,
/// **replacing** current data. SHA-256 checksums are verified before anything is
/// touched, so a corrupt backup aborts without destroying the live store.
///
/// Offline operation for the `--restore` CLI path: the server must not be serving
/// and the identity DB must be closed (its file is atomically replaced).
///
/// An age-encrypted backup is decrypted in-process when `identity_path` names
/// the age identity file (the private half produced by `age-keygen`); the
/// `--restore` CLI passes `OTS_BACKUP_DECRYPTION_IDENTITY_PATH`. The server never
/// stores that identity — only the recipient it encrypts to — so restoring an
/// encrypted backup is an explicit operator action with the key they kept.
///
/// The path is a parameter rather than an env lookup so this stays a pure
/// function: reading it here would force every test to mutate the process
/// environment, which races other tests in the same binary.
pub fn restore_backup(
    backup_dir: &Path,
    id: &str,
    store: &TripleStore,
    target_sqlite: &Path,
    identity_path: Option<&Path>,
) -> anyhow::Result<BackupManifest> {
    validate_backup_id(id)?;
    let dir = backup_dir.join(id);
    let manifest = read_manifest(&dir, id)?;

    // Verify integrity BEFORE replacing any live data.
    let rdf_raw = fs::read(dir.join(&manifest.rdf_path))?;
    let sqlite_raw = fs::read(dir.join(&manifest.sqlite_path))?;
    if sha256_hex(&rdf_raw) != manifest.rdf_sha256
        || sha256_hex(&sqlite_raw) != manifest.sqlite_sha256
    {
        anyhow::bail!("backup {id} failed checksum verification; refusing to restore");
    }
    // Decrypt before anything is replaced: a missing or wrong identity must fail
    // while the live store is still intact.
    let (rdf_raw, sqlite_raw) = if manifest.encrypted {
        decrypt_backup_pair(id, &rdf_raw, &sqlite_raw, identity_path)?
    } else {
        (rdf_raw, sqlite_raw)
    };

    // ── RDF: clear the store, then stream the gzipped N-Quads dump back in. ──
    store
        .update("DROP ALL")
        .map_err(|e| anyhow::anyhow!("clear store before restore: {e}"))?;
    let reader = std::io::BufReader::new(flate2::read::GzDecoder::new(&rdf_raw[..]));
    store
        .load_reader(reader, oxigraph::io::RdfFormat::NQuads, None)
        .map_err(|e| anyhow::anyhow!("load N-Quads dump: {e}"))?;

    // ── SQLite: atomically replace the identity DB (drop stale WAL/SHM). ──
    let tmp = target_sqlite.with_extension("restore-tmp");
    write_secure(&tmp, &sqlite_raw)?;
    for ext in ["-wal", "-shm"] {
        let mut p = target_sqlite.as_os_str().to_os_string();
        p.push(ext);
        let _ = fs::remove_file(PathBuf::from(p));
    }
    fs::rename(&tmp, target_sqlite)
        .with_context(|| format!("replace identity DB at {}", target_sqlite.display()))?;

    // A restore replaces the whole store: the one mutation that must appear in
    // the trail, since every earlier entry describes data that no longer exists.
    crate::commit_log::record(
        store,
        "urn:ots:backup",
        crate::commit_log::CommitKind::Backup,
        format!(
            "Restored backup {} (created {}, {} quads)",
            manifest.id, manifest.created_at, manifest.rdf_quad_count
        ),
        None,
        Some("urn:ots:store".to_string()),
        Vec::new(),
        manifest.rdf_quad_count,
        0,
        None,
    );

    Ok(manifest)
}

/// Upload a backup directory's files to S3 under `backups/<id>/...`. No-op
/// unless `OTS_BACKUP_S3_ENABLED` is on, or if the ObjectStore is not configured.
pub async fn maybe_upload_to_s3(
    obj: &crate::storage::ObjectStore,
    mgr: &BackupManager,
    id: &str,
) -> anyhow::Result<()> {
    if !s3_enabled() || !obj.is_configured() {
        return Ok(());
    }
    let r = upload_to_s3(obj, mgr, id).await;
    *last_s3_upload().lock().unwrap_or_else(|p| p.into_inner()) = Some(S3Upload {
        id: id.to_string(),
        at: Utc::now().to_rfc3339(),
        ok: r.is_ok(),
        error: r.as_ref().err().map(|e| e.to_string()),
    });
    r
}

/// `OTS_BACKUP_S3_ENABLED`: copy each backup to the object store.
pub fn s3_enabled() -> bool {
    crate::config::BACKUP_S3_ENABLED.get()
}

/// The latest S3 copy this process attempted, for the admin status.
#[derive(Debug, Clone, Serialize)]
pub struct S3Upload {
    pub id: String,
    pub at: String,
    pub ok: bool,
    pub error: Option<String>,
}

fn last_s3_upload() -> &'static std::sync::Mutex<Option<S3Upload>> {
    static LAST: std::sync::OnceLock<std::sync::Mutex<Option<S3Upload>>> =
        std::sync::OnceLock::new();
    LAST.get_or_init(|| std::sync::Mutex::new(None))
}

async fn upload_to_s3(
    obj: &crate::storage::ObjectStore,
    mgr: &BackupManager,
    id: &str,
) -> anyhow::Result<()> {
    validate_backup_id(id)?;
    let dir = mgr.backup_dir().join(id);
    let manifest = read_manifest(&dir, id)?;
    for (name, ct) in [
        (manifest.rdf_path.as_str(), "application/octet-stream"),
        (manifest.sqlite_path.as_str(), "application/octet-stream"),
        ("manifest.json", "application/json"),
    ] {
        let bytes = fs::read(dir.join(name))?;
        let key = format!("backups/{}/{}", id, name);
        obj.upload(&key, bytes::Bytes::from(bytes), ct).await?;
    }
    Ok(())
}

// ─── Staged restore ─────────────────────────────────────────────────────────
//
// A restore replaces the identity DB file, which a serving process holds open,
// so it cannot run while the server serves. The admin UI therefore *stages*
// one: the checksums are verified now, a marker names the backup, and the next
// start applies it before anything is opened (as `--restore` does), then
// records how it went for the UI to show.

const RESTORE_PENDING: &str = "restore-pending.json";
const RESTORE_LAST: &str = "restore-last.json";

/// A restore staged for the next start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRestore {
    pub id: String,
    pub requested_by: Option<String>,
    pub requested_at: String,
}

/// How the last staged restore went.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreOutcome {
    pub id: String,
    pub requested_by: Option<String>,
    pub requested_at: String,
    pub finished_at: String,
    pub ok: bool,
    pub error: Option<String>,
    pub quad_count: Option<usize>,
}

pub fn pending_restore(backup_dir: &Path) -> Option<PendingRestore> {
    let bytes = fs::read(backup_dir.join(RESTORE_PENDING)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn last_restore(backup_dir: &Path) -> Option<RestoreOutcome> {
    let bytes = fs::read(backup_dir.join(RESTORE_LAST)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Stage `id` for restore at the next start, after verifying its checksums
/// (and, for an encrypted backup, that a decryption identity is configured):
/// a backup that cannot be restored is refused now, not at the restart.
pub fn stage_restore(
    mgr: &BackupManager,
    id: &str,
    requested_by: Option<&str>,
) -> anyhow::Result<PendingRestore> {
    validate_backup_id(id)?;
    let manifest = read_manifest(&mgr.backup_dir.join(id), id)?;
    if !mgr.verify(id)? {
        anyhow::bail!("backup {id} failed checksum verification; it cannot be restored");
    }
    if manifest.encrypted
        && crate::config::BACKUP_DECRYPTION_IDENTITY_PATH
            .get()
            .is_none()
    {
        anyhow::bail!(
            "backup {id} is age-encrypted and OTS_BACKUP_DECRYPTION_IDENTITY_PATH is not set; \
             the restart could not decrypt it"
        );
    }
    let pending = PendingRestore {
        id: id.to_string(),
        requested_by: requested_by.map(str::to_string),
        requested_at: Utc::now().to_rfc3339(),
    };
    write_secure(
        &mgr.backup_dir.join(RESTORE_PENDING),
        serde_json::to_string_pretty(&pending)?.as_bytes(),
    )?;
    mgr.audit.log(
        AuditEventBuilder::new(
            AuditEventType::BackupRestoreRequested,
            AuditOutcome::Success,
        )
        .resource("backup", id)
        .details(serde_json::json!({ "requested_by": requested_by })),
    );
    Ok(pending)
}

/// Withdraw a staged restore. `false` when none was staged.
pub fn cancel_restore(mgr: &BackupManager, by: Option<&str>) -> anyhow::Result<bool> {
    let Some(pending) = pending_restore(&mgr.backup_dir) else {
        return Ok(false);
    };
    fs::remove_file(mgr.backup_dir.join(RESTORE_PENDING))?;
    mgr.audit.log(
        AuditEventBuilder::new(
            AuditEventType::BackupRestoreCancelled,
            AuditOutcome::Success,
        )
        .resource("backup", &pending.id)
        .details(serde_json::json!({ "cancelled_by": by })),
    );
    Ok(true)
}

/// Apply a staged restore, if there is one. Called at startup before the
/// identity DB and the store are opened. The marker is removed whatever the
/// outcome — a failing restore must not stop every later start — and the
/// outcome is written for the admin UI. A failure leaves the live data as it
/// was: [`restore_backup`] verifies and decrypts before it replaces anything.
pub fn apply_pending_restore(
    backup_dir: &Path,
    data_dir: &Path,
    target_sqlite: &Path,
    identity_path: Option<&Path>,
) -> Option<RestoreOutcome> {
    let pending = pending_restore(backup_dir)?;
    let _ = fs::remove_file(backup_dir.join(RESTORE_PENDING));
    tracing::info!("Applying the staged restore of backup {}…", pending.id);
    let result = TripleStore::open(data_dir)
        .map_err(|e| anyhow::anyhow!("open the store: {e}"))
        .and_then(|store| {
            restore_backup(
                backup_dir,
                &pending.id,
                &store,
                target_sqlite,
                identity_path,
            )
        });
    let outcome = RestoreOutcome {
        id: pending.id.clone(),
        requested_by: pending.requested_by,
        requested_at: pending.requested_at,
        finished_at: Utc::now().to_rfc3339(),
        ok: result.is_ok(),
        error: result.as_ref().err().map(|e| format!("{e:#}")),
        quad_count: result.as_ref().ok().map(|m| m.rdf_quad_count),
    };
    match &result {
        Ok(m) => tracing::info!("Restored backup {} ({} quads)", m.id, m.rdf_quad_count),
        Err(e) => tracing::error!("The staged restore of backup {} failed: {e:#}", pending.id),
    }
    if let Ok(json) = serde_json::to_string_pretty(&outcome) {
        let _ = write_secure(&backup_dir.join(RESTORE_LAST), json.as_bytes());
    }
    Some(outcome)
}

/// `GET /api/admin/backup/status` — whether backups are configured, how they
/// are kept (retention, encryption, schedule), the S3 copy, and any staged
/// or last restore. super_admin only. Answers 200 with `configured: false`
/// when the backup subsystem is off, so the UI can say so.
pub async fn admin_backup_status(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(state): axum::extract::State<crate::server::AppState>,
) -> Result<axum::Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let s3 = serde_json::json!({
        "enabled": s3_enabled(),
        "object_store_configured": state.object_store.is_configured(),
        "last_upload": last_s3_upload().lock().unwrap_or_else(|p| p.into_inner()).clone(),
    });
    let Some(mgr) = state.backup.clone() else {
        return Ok(axum::Json(serde_json::json!({
            "configured": false,
            "s3": s3,
        })));
    };
    let dir = mgr.backup_dir().to_path_buf();
    let (count, latest) = tokio::task::spawn_blocking({
        let mgr = mgr.clone();
        move || {
            let list = mgr.list().unwrap_or_default();
            (list.len(), list.first().cloned())
        }
    })
    .await
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(axum::Json(serde_json::json!({
        "configured": true,
        "dir": dir.display().to_string(),
        "retention": mgr.retention(),
        "encrypted": mgr.encrypts(),
        "decrypt_identity_configured": crate::config::BACKUP_DECRYPTION_IDENTITY_PATH.get().is_some(),
        "schedule_secs": crate::config::BACKUP_SCHEDULE.get().as_secs(),
        "count": count,
        "latest": latest,
        "s3": s3,
        "pending_restore": pending_restore(&dir),
        "last_restore": last_restore(&dir),
    })))
}

/// `POST /api/admin/backup/:id/restore` — stage a restore of backup `id` for
/// the next start (see [`stage_restore`]). Replaces any restore staged
/// before. super_admin only.
pub async fn admin_stage_restore(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(mgr): axum::extract::State<Option<Arc<BackupManager>>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::Json<PendingRestore>, (axum::http::StatusCode, String)> {
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let mgr = mgr.ok_or((
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Backup not configured".into(),
    ))?;
    let user = current_user.user_id.clone();
    let pending = tokio::task::spawn_blocking(move || stage_restore(&mgr, &id, Some(&user)))
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (axum::http::StatusCode::UNPROCESSABLE_ENTITY, e.to_string()))?;
    Ok(axum::Json(pending))
}

/// `DELETE /api/admin/backup/restore` — withdraw the staged restore.
/// super_admin only.
pub async fn admin_cancel_restore(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(mgr): axum::extract::State<Option<Arc<BackupManager>>>,
) -> Result<axum::http::StatusCode, (axum::http::StatusCode, String)> {
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let mgr = mgr.ok_or((
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Backup not configured".into(),
    ))?;
    match cancel_restore(&mgr, Some(&current_user.user_id)) {
        Ok(true) => Ok(axum::http::StatusCode::NO_CONTENT),
        Ok(false) => Err((
            axum::http::StatusCode::NOT_FOUND,
            "No restore is staged".into(),
        )),
        Err(e) => Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// Spawn a Tokio task that runs the backup every `interval` (`OTS_BACKUP_SCHEDULE_SECS`).
pub fn spawn_scheduler(
    mgr: Arc<BackupManager>,
    interval: std::time::Duration,
    object_store: Option<Arc<crate::storage::ObjectStore>>,
) {
    tokio::spawn(async move {
        // A silently failing backup is the worst kind: nothing looks wrong
        // until a restore is needed. Every failure below therefore raises an
        // operational alert as well as logging — `AlertConfig::from_env` is a
        // no-op when neither OTS_ALERT_WEBHOOK_URL nor OTS_ALERT_SMTP_* is configured,
        // so this costs nothing when alerting is off.
        let alerts = crate::alerting::AlertManager::new(
            crate::alerting::AlertConfig::from_env(),
            mgr.audit.clone(),
        );
        // The registry refuses an interval under a minute.
        let interval = interval.max(std::time::Duration::from_secs(60));
        loop {
            tokio::time::sleep(interval).await;
            let mgr_inner = mgr.clone();
            // Run the synchronous backup off the runtime thread.
            let res = tokio::task::spawn_blocking(move || mgr_inner.run_once()).await;
            match res {
                Ok(Ok(m)) => {
                    tracing::info!("backup: created {}", m.id);
                    if let Some(ref obj) = object_store {
                        if let Err(e) = maybe_upload_to_s3(obj, &mgr, &m.id).await {
                            tracing::warn!("backup: S3 upload failed: {}", e);
                            alerts
                                .dispatch(backup_alert("backup_upload_failed", &e.to_string()))
                                .await;
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!("backup: failed: {}", e);
                    alerts
                        .dispatch(backup_alert("backup_failed", &e.to_string()))
                        .await;
                }
                Err(e) => {
                    tracing::warn!("backup: task join error: {}", e);
                    alerts
                        .dispatch(backup_alert("backup_failed", &e.to_string()))
                        .await;
                }
            }
        }
    });
}

fn backup_alert(kind: &str, error: &str) -> crate::alerting::Alert {
    crate::alerting::Alert {
        severity: crate::alerting::AlertSeverity::Critical,
        kind: kind.to_string(),
        message: format!("Scheduled backup did not complete: {error}"),
        context: serde_json::json!({ "error": error }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::audit::AuditLogger;
    use crate::auth::db::AuthDb;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

    /// Restore round-trip: take a backup, mutate the store, restore — the store
    /// must return to the backup snapshot (checksums are verified first).
    /// Quads outside the commit log: a restore records itself in the trail of
    /// the restored store, and that entry must not count as restored data.
    fn data_quads(store: &TripleStore) -> usize {
        store
            .store()
            .quads_for_pattern(None, None, None, None)
            .filter_map(Result::ok)
            .filter(|q| match &q.graph_name {
                oxigraph::model::GraphName::NamedNode(n) => {
                    n.as_str() != crate::commit_log::COMMIT_GRAPH
                }
                _ => true,
            })
            .count()
    }

    #[test]
    fn restore_round_trips_unencrypted_backup() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        let sqlite_path = dir.path().join("auth.sqlite");
        let auth_db = Arc::new(AuthDb::open(&sqlite_path).unwrap());
        let audit = Arc::new(AuditLogger::new(auth_db.pool()));

        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                "@prefix ex: <http://example.org/> . ex:a ex:b ex:c . ex:d ex:e ex:f .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();

        // TripleStore is Arc-backed, so the manager shares the same store.
        let mgr = BackupManager::new(
            backup_dir.clone(),
            sqlite_path.clone(),
            store.clone(),
            audit,
            7,
            false,
            None,
        )
        .unwrap();
        let manifest = mgr.run_once().unwrap();
        assert_eq!(manifest.rdf_quad_count, 2);

        // Mutate the store after the backup was taken.
        store
            .update("INSERT DATA { <http://example.org/x> <http://example.org/y> <http://example.org/z> }")
            .unwrap();
        assert_eq!(data_quads(&store), 3);

        // Restoring returns the store to the 2-quad snapshot.
        let restored =
            restore_backup(&backup_dir, &manifest.id, &store, &sqlite_path, None).unwrap();
        assert_eq!(restored.id, manifest.id);
        // The restore itself is in the trail of the restored store.
        let trail = crate::commit_log::list_commits(
            &store,
            &crate::commit_log::CommitScope::Subject("urn:ots:store".to_string()),
            &crate::commit_log::CommitQuery::default(),
        );
        assert_eq!(trail.len(), 1, "a backup restore is a recorded mutation");
        assert_eq!(trail[0].kind, crate::commit_log::CommitKind::Backup);
        assert_eq!(trail[0].added, manifest.rdf_quad_count);
        assert_eq!(
            data_quads(&store),
            2,
            "store must match the restored backup snapshot"
        );
    }

    /// A restore staged from the admin UI: verified when staged, kept from
    /// retention while it waits, applied at the next start before anything
    /// opens the store, and its outcome recorded. Cancelling withdraws it.
    #[test]
    fn a_staged_restore_survives_retention_and_applies_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        let data_dir = dir.path().join("data");
        let sqlite_path = dir.path().join("auth.sqlite");
        let auth_db = Arc::new(AuthDb::open(&sqlite_path).unwrap());
        let audit = Arc::new(AuditLogger::new(auth_db.pool()));
        let first = {
            let store = TripleStore::open(&data_dir).unwrap();
            store
                .load_str(
                    "@prefix ex: <http://example.org/> . ex:a ex:b ex:c . ex:d ex:e ex:f .",
                    RdfFormat::Turtle,
                    None,
                )
                .unwrap();
            // Retention 1: the next backup would prune this one.
            let mgr = BackupManager::new(
                backup_dir.clone(),
                sqlite_path.clone(),
                store.clone(),
                audit.clone(),
                1,
                false,
                None,
            )
            .unwrap();
            let first = mgr.run_once().unwrap();
            store
                .update("INSERT DATA { <http://example.org/x> <http://example.org/y> <http://example.org/z> }")
                .unwrap();

            // Cancel: nothing staged afterwards, and a second cancel says so.
            stage_restore(&mgr, &first.id, Some("adm")).unwrap();
            assert!(cancel_restore(&mgr, Some("adm")).unwrap());
            assert!(pending_restore(&backup_dir).is_none());
            assert!(!cancel_restore(&mgr, Some("adm")).unwrap());

            let pending = stage_restore(&mgr, &first.id, Some("adm")).unwrap();
            assert_eq!(pending.requested_by.as_deref(), Some("adm"));
            assert_eq!(pending_restore(&backup_dir).unwrap().id, first.id);

            // A newer backup does not prune the staged one.
            std::thread::sleep(std::time::Duration::from_millis(1100));
            let second = mgr.run_once().unwrap();
            assert_ne!(second.id, first.id);
            assert!(
                backup_dir.join(&first.id).join("manifest.json").exists(),
                "the staged backup survives retention"
            );
            first
        };

        // The next start: the store is closed again, the restore applies.
        let outcome =
            apply_pending_restore(&backup_dir, &data_dir, &sqlite_path, None).expect("applied");
        assert!(outcome.ok, "{:?}", outcome.error);
        assert_eq!(outcome.id, first.id);
        assert_eq!(outcome.quad_count, Some(2));
        assert!(pending_restore(&backup_dir).is_none(), "applied once");
        assert!(last_restore(&backup_dir).is_some_and(|o| o.ok && o.id == first.id));
        let store = TripleStore::open(&data_dir).unwrap();
        assert_eq!(data_quads(&store), 2, "the store is the staged snapshot");
        drop(store);
        assert!(
            apply_pending_restore(&backup_dir, &data_dir, &sqlite_path, None).is_none(),
            "nothing staged, nothing applied"
        );
    }

    /// A backup whose files no longer match their checksums cannot be staged.
    #[test]
    fn a_corrupt_backup_cannot_be_staged() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        let sqlite_path = dir.path().join("auth.sqlite");
        let auth_db = Arc::new(AuthDb::open(&sqlite_path).unwrap());
        let audit = Arc::new(AuditLogger::new(auth_db.pool()));
        let store = TripleStore::in_memory().unwrap();
        let mgr = BackupManager::new(
            backup_dir.clone(),
            sqlite_path,
            store,
            audit,
            7,
            false,
            None,
        )
        .unwrap();
        let m = mgr.run_once().unwrap();
        fs::write(backup_dir.join(&m.id).join(&m.rdf_path), b"tampered").unwrap();
        let err = stage_restore(&mgr, &m.id, None).unwrap_err().to_string();
        assert!(err.contains("checksum"), "{err}");
        assert!(pending_restore(&backup_dir).is_none());
        assert!(stage_restore(&mgr, "../etc", None).is_err());
    }

    /// An encrypted backup must be recoverable with the operator's identity.
    ///
    /// It was not: `init_backup_encryption` generated a keypair, wrote only
    /// `to_public()` and dropped the identity, so backups were encrypted to a
    /// key that existed nowhere — and `restore_backup` refused encrypted
    /// archives outright. Both halves are fixed here: the recipient must be
    /// supplied, and restore decrypts with `OTS_BACKUP_DECRYPTION_IDENTITY_PATH`.
    #[cfg(feature = "backup-encrypt")]
    #[test]
    fn encrypted_backup_round_trips_with_the_operator_identity() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        let sqlite_path = dir.path().join("auth.sqlite");
        let auth_db = Arc::new(AuthDb::open(&sqlite_path).unwrap());
        let audit = Arc::new(AuditLogger::new(auth_db.pool()));

        use age::secrecy::ExposeSecret;

        // The operator's keypair: identity kept, recipient handed to the server.
        let identity = age::x25519::Identity::generate();
        let identity_path = dir.path().join("identity.txt");
        fs::write(
            &identity_path,
            format!(
                "# created by the test\n# public key: {}\n{}\n",
                identity.to_public(),
                identity.to_string().expose_secret()
            ),
        )
        .unwrap();
        let recipient_path = dir.path().join("backup_key.age");
        fs::write(&recipient_path, format!("{}\n", identity.to_public())).unwrap();

        let key_path = init_backup_encryption(&recipient_path)
            .unwrap()
            .expect("an existing recipient must be accepted");

        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                "@prefix ex: <http://example.org/> . ex:a ex:b ex:c . ex:d ex:e ex:f .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();

        let mgr = BackupManager::new(
            backup_dir.clone(),
            sqlite_path.clone(),
            store.clone(),
            audit,
            7,
            true,
            Some(key_path),
        )
        .unwrap();
        let manifest = mgr.run_once().unwrap();
        assert!(manifest.encrypted, "backup must be encrypted");

        store
            .update("INSERT DATA { <http://example.org/x> <http://example.org/y> <http://example.org/z> }")
            .unwrap();
        assert_eq!(data_quads(&store), 3);

        let restored = restore_backup(
            &backup_dir,
            &manifest.id,
            &store,
            &sqlite_path,
            Some(identity_path.as_path()),
        )
        .unwrap();

        assert_eq!(restored.id, manifest.id);
        assert_eq!(
            data_quads(&store),
            2,
            "an encrypted backup must restore to its snapshot"
        );
    }

    /// Encryption must not silently invent a key nobody holds: with no
    /// recipient file, initialisation fails instead of generating one whose
    /// private half is discarded.
    #[cfg(feature = "backup-encrypt")]
    #[test]
    fn missing_recipient_is_an_error_not_a_generated_key() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent.age");
        let err = init_backup_encryption(&missing)
            .expect_err("a missing recipient must be refused, not generated");
        assert!(
            err.to_string().contains("age-keygen"),
            "the error must tell the operator how to create one: {err}"
        );
        assert!(
            !missing.exists(),
            "no key file may be created as a side effect"
        );
    }

    /// End-to-end (unencrypted) backup: dump RDF + SQLite, write a manifest with
    /// checksums, then verify those checksums round-trip and detect tampering.
    #[test]
    fn backup_run_verify_and_tamper_detection() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        let sqlite_path = dir.path().join("auth.sqlite");

        // A real on-disk SQLite DB for the online backup to read from.
        let auth_db = Arc::new(AuthDb::open(&sqlite_path).unwrap());
        let audit = Arc::new(AuditLogger::new(auth_db.pool()));

        let store = TripleStore::in_memory().unwrap();
        store
            .load_str(
                "@prefix ex: <http://example.org/> . ex:a ex:b ex:c . ex:a ex:d ex:e .",
                RdfFormat::Turtle,
                None,
            )
            .unwrap();

        let mgr =
            BackupManager::new(backup_dir, sqlite_path, store, audit, 7, false, None).unwrap();

        let manifest = mgr.run_once().unwrap();
        assert_eq!(manifest.rdf_quad_count, 2);
        assert!(!manifest.encrypted);

        // Checksums in the manifest match the files on disk.
        assert!(
            mgr.verify(&manifest.id).unwrap(),
            "fresh backup should verify"
        );

        // It shows up in the listing.
        assert!(mgr.list().unwrap().iter().any(|m| m.id == manifest.id));

        // Corrupting a backed-up file is detected by verify().
        let rdf_file = mgr.backup_dir().join(&manifest.id).join(&manifest.rdf_path);
        fs::write(&rdf_file, b"corrupted").unwrap();
        assert!(
            !mgr.verify(&manifest.id).unwrap(),
            "tampered backup must fail verification"
        );
    }

    /// A manifest whose file names leave the backup directory is refused by
    /// verify (and by restore and the S3 upload, which read it the same way)
    /// instead of hashing, restoring or uploading a file elsewhere on the host.
    #[test]
    fn manifest_paths_outside_the_backup_dir_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let backup_dir = dir.path().join("backups");
        let sqlite_path = dir.path().join("auth.sqlite");
        let auth_db = Arc::new(AuthDb::open(&sqlite_path).unwrap());
        let audit = Arc::new(AuditLogger::new(auth_db.pool()));
        let mgr = BackupManager::new(
            backup_dir.clone(),
            sqlite_path,
            TripleStore::in_memory().unwrap(),
            audit,
            7,
            false,
            None,
        )
        .unwrap();
        let manifest = mgr.run_once().unwrap();
        let manifest_file = backup_dir.join(&manifest.id).join("manifest.json");

        // A file outside the backup directory, with its true checksum in the
        // manifest, so only the path check can stop it.
        let outside = dir.path().join("secret.txt");
        fs::write(&outside, b"not part of any backup").unwrap();
        let outside_sha = sha256_hex(&fs::read(&outside).unwrap());
        for evil in [
            outside.to_string_lossy().into_owned(),
            "../../secret.txt".to_string(),
        ] {
            let mut m = manifest.clone();
            m.rdf_path = evil.clone();
            m.rdf_sha256 = outside_sha.clone();
            fs::write(&manifest_file, serde_json::to_vec(&m).unwrap()).unwrap();
            let err = mgr
                .verify(&manifest.id)
                .expect_err(&format!("rdf_path {evil:?} must be refused"));
            assert!(
                err.to_string().contains("invalid backup file name"),
                "{err}"
            );
        }
    }

    /// Retention pruning keeps only the newest `retention` snapshots.
    #[test]
    fn prune_enforces_retention() {
        let dir = tempfile::tempdir().unwrap();
        let sqlite_path = dir.path().join("auth.sqlite");
        let auth_db = Arc::new(AuthDb::open(&sqlite_path).unwrap());
        let audit = Arc::new(AuditLogger::new(auth_db.pool()));
        let store = TripleStore::in_memory().unwrap();

        let mgr = BackupManager::new(
            dir.path().join("backups"),
            sqlite_path,
            store,
            audit,
            2, // retain only 2
            false,
            None,
        )
        .unwrap();

        // run_once embeds a second-resolution timestamp in the id; sleep to keep ids unique.
        for _ in 0..3 {
            mgr.run_once().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }

        assert_eq!(mgr.list().unwrap().len(), 2, "retention should cap at 2");
    }

    /// Encrypted backups are decrypted out-of-band (there is no in-process
    /// restore path for `.age` files), so nothing else in the suite proves the
    /// write side actually produces a readable age file. This pins that, and
    /// with it the `age` API surface — which has changed shape across releases.
    #[cfg(feature = "backup-encrypt")]
    #[test]
    fn maybe_encrypt_round_trips_through_age() {
        use std::io::Read;

        let dir = tempfile::tempdir().unwrap();
        let sqlite_path = dir.path().join("auth.sqlite");
        let auth_db = Arc::new(AuthDb::open(&sqlite_path).unwrap());
        let audit = Arc::new(AuditLogger::new(auth_db.pool()));

        // The key file holds the public half, exactly as init_backup_encryption writes it.
        let identity = age::x25519::Identity::generate();
        let key_path = dir.path().join("backup-recipient.txt");
        fs::write(&key_path, identity.to_public().to_string()).unwrap();

        let mgr = BackupManager::new(
            dir.path().join("backups"),
            sqlite_path,
            TripleStore::in_memory().unwrap(),
            audit,
            7,
            true,
            Some(key_path),
        )
        .unwrap();

        let plaintext = b"@prefix ex: <http://example.org/> . ex:a ex:b ex:c .";
        let ciphertext = mgr.maybe_encrypt(plaintext).unwrap();
        assert!(
            ciphertext.starts_with(b"age-encryption.org/"),
            "output should carry the age file header"
        );

        let mut decrypted = Vec::new();
        age::Decryptor::new_buffered(&ciphertext[..])
            .unwrap()
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .unwrap()
            .read_to_end(&mut decrypted)
            .unwrap();
        assert_eq!(decrypted, plaintext, "round-trip must return the plaintext");
    }
}
