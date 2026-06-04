//! Backup subsystem.
//!
//! Produces gzipped N-Quads dumps of the RDF store and online SQLite backups
//! of the auth database, both written to a configurable backup directory.
//! Each backup writes a JSON manifest with SHA-256 checksums so it can be
//! verified later without trusting filesystem mtime.
//!
//! Optional `age` X25519 encryption is applied to both files when the
//! `backup-encrypt` feature is enabled and `BACKUP_ENCRYPT=true`.

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

/// Initialize backup encryption with automatic key generation.
/// If BACKUP_ENCRYPT_KEY_PATH doesn't exist, generates a new age X25519 keypair
/// and writes the public key to the path with 0o600 permissions.
pub fn init_backup_encryption(key_path: &Path) -> anyhow::Result<Option<PathBuf>> {
    #[cfg(feature = "backup-encrypt")]
    {
        if key_path.exists() {
            // Key already exists, just return the path
            let key_content = fs::read_to_string(key_path)
                .with_context(|| format!("read existing age recipient key {}", key_path.display()))?;
            key_content.trim().parse::<age::x25519::Recipient>()
                .map_err(|e: &str| anyhow::anyhow!("invalid age recipient in {}: {}", key_path.display(), e))?;
            tracing::info!("Using existing backup encryption key at {}", key_path.display());
            Ok(Some(key_path.to_path_buf()))
        } else {
            // Generate new keypair
            let identity = age::x25519::Identity::generate();
            let recipient = identity.to_public();
            let key_str = recipient.to_string();

            // Write public key with secure permissions
            fs::create_dir_all(key_path.parent().unwrap_or_else(|| Path::new(".")))?;
            let mut file = File::create(key_path)?;
            file.write_all(key_str.as_bytes())?;
            file.write_all(b"\n")?;
            drop(file);

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(key_path, fs::Permissions::from_mode(0o600))?;
            }

            let display_key = if key_str.len() > 16 {
                format!("{}...", &key_str[..16])
            } else {
                key_str.clone()
            };
            tracing::info!("Generated new backup encryption key at {} (recipient: {})", key_path.display(), display_key);
            tracing::warn!("⚠️  Store the private key securely outside this directory: age-keygen");

            Ok(Some(key_path.to_path_buf()))
        }
    }
    #[cfg(not(feature = "backup-encrypt"))]
    {
        let _ = key_path;
        tracing::error!("BACKUP_ENCRYPT=true but binary was compiled without `backup-encrypt` feature");
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
        fs::create_dir_all(&backup_dir).with_context(|| format!("create backup dir {}", backup_dir.display()))?;
        Ok(Self { backup_dir, sqlite_path, store, audit, retention, encrypt, encrypt_key_path })
    }

    pub fn backup_dir(&self) -> &Path { &self.backup_dir }

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
        let rdf_name = if self.encrypt { "rdf.nq.gz.age" } else { "rdf.nq.gz" };
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
        let sqlite_name = if self.encrypt { "auth.sqlite.age" } else { "auth.sqlite" };
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
        write_secure(&dir.join("manifest.json"), serde_json::to_string_pretty(&manifest)?.as_bytes())?;

        self.prune()?;
        Ok(manifest)
    }

    /// Recompute SHA-256 of a backup's files and compare to its manifest.
    pub fn verify(&self, id: &str) -> anyhow::Result<bool> {
        validate_backup_id(id)?;
        let dir = self.backup_dir.join(id);
        let manifest: BackupManifest = serde_json::from_slice(&fs::read(dir.join("manifest.json"))?)?;
        let rdf_ok = sha256_hex(&fs::read(dir.join(&manifest.rdf_path))?) == manifest.rdf_sha256;
        let sqlite_ok = sha256_hex(&fs::read(dir.join(&manifest.sqlite_path))?) == manifest.sqlite_sha256;
        let ok = rdf_ok && sqlite_ok;
        self.audit.log(
            AuditEventBuilder::new(AuditEventType::BackupVerified, if ok { AuditOutcome::Success } else { AuditOutcome::Failure })
                .resource("backup", id)
                .details(serde_json::json!({ "rdf_ok": rdf_ok, "sqlite_ok": sqlite_ok })),
        );
        Ok(ok)
    }

    pub fn list(&self) -> anyhow::Result<Vec<BackupManifest>> {
        let mut out = Vec::new();
        if !self.backup_dir.exists() { return Ok(out); }
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
        if all.len() <= self.retention { return Ok(()); }
        let to_remove = all.split_off(self.retention);
        for m in to_remove {
            let _ = fs::remove_dir_all(self.backup_dir.join(&m.id));
        }
        Ok(())
    }

    fn maybe_encrypt(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        if !self.encrypt { return Ok(data.to_vec()); }
        #[cfg(feature = "backup-encrypt")]
        {
            let key_path = self.encrypt_key_path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("BACKUP_ENCRYPT=true but BACKUP_ENCRYPT_KEY_PATH is unset"))?;
            let pubkey_str = fs::read_to_string(key_path)
                .with_context(|| format!("read age recipient key {}", key_path.display()))?;
            let recipient: age::x25519::Recipient = pubkey_str.trim().parse()
                .map_err(|e: &str| anyhow::anyhow!("invalid age recipient: {}", e))?;
            let encryptor = age::Encryptor::with_recipients(vec![Box::new(recipient)])
                .ok_or_else(|| anyhow::anyhow!("age encryptor init failed"))?;
            let mut out = Vec::new();
            let mut writer = encryptor.wrap_output(&mut out)?;
            writer.write_all(data)?;
            writer.finish()?;
            Ok(out)
        }
        #[cfg(not(feature = "backup-encrypt"))]
        {
            let _ = data;
            anyhow::bail!("BACKUP_ENCRYPT=true but binary was built without `backup-encrypt` feature");
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
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

/// `POST /api/admin/backup` — trigger a backup synchronously. super_admin only.
pub async fn admin_create_backup(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(mgr): axum::extract::State<Option<Arc<BackupManager>>>,
) -> Result<axum::Json<BackupManifest>, (axum::http::StatusCode, String)> {
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let mgr = mgr.ok_or((axum::http::StatusCode::SERVICE_UNAVAILABLE, "Backup not configured".into()))?;
    let mgr2 = mgr.clone();
    let m = tokio::task::spawn_blocking(move || mgr2.run_once())
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(axum::Json(m))
}

/// `GET /api/admin/backup` — list manifests. super_admin only.
pub async fn admin_list_backups(
    axum::Extension(current_user): axum::Extension<crate::auth::middleware::AuthenticatedUser>,
    axum::extract::State(mgr): axum::extract::State<Option<Arc<BackupManager>>>,
) -> Result<axum::Json<Vec<BackupManifest>>, (axum::http::StatusCode, String)> {
    if current_user.role != crate::auth::models::SystemRole::SuperAdmin {
        return Err((axum::http::StatusCode::FORBIDDEN, "super_admin only".into()));
    }
    let mgr = mgr.ok_or((axum::http::StatusCode::SERVICE_UNAVAILABLE, "Backup not configured".into()))?;
    let list = mgr.list().map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
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
    let mgr = mgr.ok_or((axum::http::StatusCode::SERVICE_UNAVAILABLE, "Backup not configured".into()))?;
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
        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        anyhow::bail!("invalid backup id");
    }
    Ok(())
}

/// Upload a backup directory's files to S3 under `backups/<id>/...`. No-op
/// if `BACKUP_S3_ENABLED` is not "true" or the ObjectStore is not configured.
pub async fn maybe_upload_to_s3(
    obj: &crate::storage::ObjectStore,
    mgr: &BackupManager,
    id: &str,
) -> anyhow::Result<()> {
    if std::env::var("BACKUP_S3_ENABLED").unwrap_or_default() != "true" { return Ok(()); }
    if !obj.is_configured() { return Ok(()); }
    validate_backup_id(id)?;
    let dir = mgr.backup_dir().join(id);
    let manifest: BackupManifest = serde_json::from_slice(&fs::read(dir.join("manifest.json"))?)?;
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

/// Spawn a Tokio task that runs the backup every `interval_hours` hours.
pub fn spawn_scheduler(
    mgr: Arc<BackupManager>,
    interval_hours: u64,
    object_store: Option<Arc<crate::storage::ObjectStore>>,
) {
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(interval_hours.max(1) * 3600);
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
                        }
                    }
                }
                Ok(Err(e)) => tracing::warn!("backup: failed: {}", e),
                Err(e) => tracing::warn!("backup: task join error: {}", e),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::audit::AuditLogger;
    use crate::auth::db::AuthDb;
    use crate::store::TripleStore;
    use oxigraph::io::RdfFormat;

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

        let mgr = BackupManager::new(
            backup_dir,
            sqlite_path,
            store,
            audit,
            7,
            false,
            None,
        )
        .unwrap();

        let manifest = mgr.run_once().unwrap();
        assert_eq!(manifest.rdf_quad_count, 2);
        assert!(!manifest.encrypted);

        // Checksums in the manifest match the files on disk.
        assert!(mgr.verify(&manifest.id).unwrap(), "fresh backup should verify");

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
}
