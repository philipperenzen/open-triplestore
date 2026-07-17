//! Resilient store open: auto-recover from RocksDB corruption so the server (and
//! `docker compose up`) comes back instead of crash-looping.
//!
//! An abrupt SIGKILL mid-write can leave the Oxigraph/RocksDB store as
//! `Corruption: SST file is ahead of WALs in CF default`, and oxigraph 0.4 exposes
//! no repair/WAL-recovery API — so reopening just errors and the container restart
//! loops. This wrapper detects that on open and recovers **without destroying
//! data**: it quarantines the corrupt RocksDB files (renamed aside under the data
//! dir, never deleted), reopens a fresh store, and best-effort restores the newest
//! unencrypted backup; the normal startup seeds then repopulate demo content.
//!
//! Opt out with `STORE_AUTO_RECOVER=false` — then a corrupt store fails fast with a
//! remediation message instead of being rebuilt.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use oxigraph::io::RdfFormat;
use tracing::{info, warn};

use crate::store::{StoreError, TripleStore};

/// Open the persistent store, recovering from on-disk corruption when enabled.
///
/// `data_dir` is the RocksDB directory (which also holds `auth.db`, `jwt_secret`,
/// `prefix_cache.json`, `backups/`). `backup_dir` is where timestamped backups
/// live (`{backup_dir}/backup-*/rdf.nq.gz`).
pub fn open_store_with_recovery(data_dir: &Path, backup_dir: &Path) -> anyhow::Result<TripleStore> {
    match TripleStore::open(data_dir) {
        Ok(store) => Ok(store),
        Err(e) if is_corruption_error(&e) => {
            if !auto_recover_enabled() {
                anyhow::bail!(
                    "the triple store at {} is corrupt ({e}). Auto-recovery is disabled \
                     (STORE_AUTO_RECOVER=false). To recover manually: stop the service, move the \
                     RocksDB files in that directory aside (or restore a backup from {}), then \
                     restart. Set STORE_AUTO_RECOVER=true (the default) to let the server \
                     quarantine the corrupt files and rebuild automatically.",
                    data_dir.display(),
                    backup_dir.display(),
                );
            }
            warn!("triple store at {} is corrupt: {e}", data_dir.display());
            warn!(
                "STORE_AUTO_RECOVER is on — quarantining the corrupt store and rebuilding \
                 (the corrupt files are preserved, never deleted)"
            );
            let quarantine = quarantine_store_files(data_dir)?;
            warn!("moved corrupt store files to {}", quarantine.display());

            let store = TripleStore::open(data_dir).map_err(|e2| {
                anyhow::anyhow!("reopening a fresh store after quarantine failed: {e2}")
            })?;

            match restore_latest_backup(&store, backup_dir) {
                Ok(Some(path)) => warn!(
                    "restored RDF from the newest backup {} (auth DB untouched; demo seeds fill gaps)",
                    path.display()
                ),
                Ok(None) => {
                    // Distinguish "genuinely no backup" from "only encrypted backups,
                    // which this node cannot auto-decrypt" — the age private key is held
                    // by the operator and never on disk (see backup::init_backup_encryption).
                    // The latter is NOT a benign empty start: restorable data exists and the
                    // operator must act, so surface it loudly with the quarantine path instead
                    // of the reassuring "starting fresh" line.
                    if encrypted_backup_exists(backup_dir) {
                        tracing::error!(
                            "store was quarantined to {} and rebuilt EMPTY: encrypted backups \
                             (rdf.nq.gz.age) exist in {} but cannot be auto-restored (the age \
                             private key is held off-box). Restore manually — decrypt the newest \
                             backup with your age identity and load it, or recover the quarantined \
                             RocksDB files. Set STORE_AUTO_RECOVER=false to fail fast instead.",
                            quarantine.display(),
                            backup_dir.display(),
                        );
                    } else {
                        warn!(
                            "no restorable (unencrypted) backup in {}; starting fresh — startup \
                             seeds will repopulate the bundled demo data",
                            backup_dir.display()
                        );
                    }
                }
                Err(e3) => warn!(
                    "backup restore failed ({e3}); starting fresh — startup seeds will repopulate \
                     the bundled demo data"
                ),
            }
            Ok(store)
        }
        Err(e) => Err(e.into()),
    }
}

/// Whether a store-open error looks like recoverable on-disk corruption.
pub fn is_corruption_error(e: &StoreError) -> bool {
    message_indicates_corruption(&e.to_string())
}

/// Pure string check, factored out so it's unit-testable without constructing an
/// oxigraph `StorageError` (which has no public constructor).
fn message_indicates_corruption(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("corruption") || m.contains("sst file is ahead of wals")
}

fn auto_recover_enabled() -> bool {
    !matches!(
        std::env::var("STORE_AUTO_RECOVER")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "false" | "0" | "no" | "off"
    )
}

/// Names/patterns of the RocksDB files that make up the triple store. Used to
/// quarantine ONLY the store, leaving `auth.db*`, `jwt_secret`,
/// `prefix_cache.json` and `backups/` (which share `data_dir`) untouched.
fn is_rocksdb_file(name: &str) -> bool {
    const EXACT: &[&str] = &["CURRENT", "IDENTITY", "LOCK", "LOG"];
    EXACT.contains(&name)
        || name.starts_with("LOG.old.")
        || name.starts_with("MANIFEST-")
        || name.starts_with("OPTIONS-")
        || name.ends_with(".sst")
        || name.ends_with(".log")
        || name.ends_with(".ldb")
        || name.ends_with(".dbtmp")
}

fn unix_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Move the RocksDB store files into `{data_dir}/quarantine-store-{ts}/`,
/// preserving them for manual recovery. Returns the quarantine directory.
fn quarantine_store_files(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let dest = data_dir.join(format!("quarantine-store-{}", unix_ts()));
    std::fs::create_dir_all(&dest)?;
    let mut moved = 0usize;
    for entry in std::fs::read_dir(data_dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if entry.path().is_file() && is_rocksdb_file(&name) {
            if let Err(e) = std::fs::rename(entry.path(), dest.join(&*name)) {
                warn!("could not quarantine store file {name}: {e}");
            } else {
                moved += 1;
            }
        }
    }
    info!("quarantined {moved} store file(s) into {}", dest.display());
    Ok(dest)
}

/// Restore from the newest unencrypted `rdf.nq.gz` under `backup_dir`. Returns the
/// path restored, or `None` if no suitable backup exists.
fn restore_latest_backup(
    store: &TripleStore,
    backup_dir: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    if !backup_dir.exists() {
        return Ok(None);
    }
    let mut candidates: Vec<(SystemTime, PathBuf)> = Vec::new();
    collect_rdf_backups(backup_dir, &mut candidates)?;
    candidates.sort_by_key(|(t, _)| *t);
    let Some((_, path)) = candidates.pop() else {
        return Ok(None);
    };
    let file = std::fs::File::open(&path)?;
    let gz = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
    let reader = std::io::BufReader::new(gz);
    store
        .load_reader(reader, RdfFormat::NQuads, None)
        .map_err(|e| anyhow::anyhow!("loading backup {}: {e}", path.display()))?;
    Ok(Some(path))
}

/// Whether any encrypted RDF backup (`rdf.nq.gz.age`) exists under `dir`. Lets
/// recovery tell "genuinely no backup" apart from "backups exist but are encrypted
/// and can't be auto-restored" (the age private key is never on disk), so it can
/// warn loudly instead of silently coming up empty.
fn encrypted_backup_exists(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if encrypted_backup_exists(&path) {
                return true;
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some("rdf.nq.gz.age") {
            return true;
        }
    }
    false
}

/// Recursively find `rdf.nq.gz` files (the unencrypted RDF dumps) under `dir`.
fn collect_rdf_backups(dir: &Path, out: &mut Vec<(SystemTime, PathBuf)>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rdf_backups(&path, out)?;
        } else if path.file_name().and_then(|n| n.to_str()) == Some("rdf.nq.gz") {
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(UNIX_EPOCH);
            out.push((mtime, path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        encrypted_backup_exists, is_rocksdb_file, message_indicates_corruption,
        restore_latest_backup,
    };
    use crate::store::TripleStore;

    #[test]
    fn matches_the_sst_ahead_of_wals_corruption() {
        for msg in [
            "Storage error: Corruption: SST file is ahead of WALs in CF default",
            "Corruption: block checksum mismatch",
            "storage error: CORRUPTION detected",
        ] {
            assert!(message_indicates_corruption(msg), "{msg:?} should match");
        }
        for msg in [
            "IO error: No space left on device",
            "SPARQL syntax error: unexpected token",
            "Graph not found: urn:x",
        ] {
            assert!(
                !message_indicates_corruption(msg),
                "{msg:?} should not match"
            );
        }
    }

    #[test]
    fn classifies_rocksdb_files_but_spares_sibling_state() {
        for f in [
            "CURRENT",
            "IDENTITY",
            "LOCK",
            "LOG",
            "LOG.old.1700000000",
            "MANIFEST-000007",
            "OPTIONS-000123",
            "000045.sst",
            "000003.log",
            "000009.ldb",
            "000010.dbtmp",
        ] {
            assert!(is_rocksdb_file(f), "{f} should be a RocksDB file");
        }
        for f in [
            "auth.db",
            "auth.db-wal",
            "auth.db-shm",
            "jwt_secret",
            "prefix_cache.json",
            "backups",
            "quarantine-store-1700000000",
            "tantivy",
        ] {
            assert!(!is_rocksdb_file(f), "{f} must be spared");
        }
    }

    #[test]
    fn encrypted_backup_exists_distinguishes_age_dumps() {
        // Only an encrypted dump present (nested under a backup-* dir).
        let enc = tempfile::tempdir().unwrap();
        let eb = enc.path().join("backup-1");
        std::fs::create_dir_all(&eb).unwrap();
        std::fs::write(eb.join("rdf.nq.gz.age"), b"ciphertext").unwrap();
        assert!(encrypted_backup_exists(enc.path()));

        // Only an unencrypted dump present — must NOT be flagged as encrypted.
        let plain = tempfile::tempdir().unwrap();
        let pb = plain.path().join("backup-1");
        std::fs::create_dir_all(&pb).unwrap();
        std::fs::write(pb.join("rdf.nq.gz"), b"plain").unwrap();
        assert!(!encrypted_backup_exists(plain.path()));

        // Missing directory.
        assert!(!encrypted_backup_exists(
            &plain.path().join("does-not-exist")
        ));
    }

    #[test]
    fn restores_newest_unencrypted_backup_but_not_encrypted() {
        use oxigraph::sparql::QueryResults;
        use std::io::Write;

        // A gzipped single-quad backup is restored into the store.
        let dir = tempfile::tempdir().unwrap();
        let b = dir.path().join("backup-1");
        std::fs::create_dir_all(&b).unwrap();
        let nq = b"<http://ex/s> <http://ex/p> <http://ex/o> <http://ex/g> .\n";
        let file = std::fs::File::create(b.join("rdf.nq.gz")).unwrap();
        let mut gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        gz.write_all(nq).unwrap();
        gz.finish().unwrap();

        let store = TripleStore::in_memory().unwrap();
        assert!(restore_latest_backup(&store, dir.path()).unwrap().is_some());
        let present = matches!(
            store
                .query("ASK { GRAPH <http://ex/g> { <http://ex/s> <http://ex/p> <http://ex/o> } }")
                .unwrap(),
            QueryResults::Boolean(true)
        );
        assert!(present, "restored triple should be queryable");

        // With ONLY an encrypted dump, restore is a no-op (the node can't decrypt it),
        // but it is detectable so recovery can warn loudly instead of silently emptying.
        let enc = tempfile::tempdir().unwrap();
        let eb = enc.path().join("backup-1");
        std::fs::create_dir_all(&eb).unwrap();
        std::fs::write(eb.join("rdf.nq.gz.age"), b"ciphertext").unwrap();
        let store2 = TripleStore::in_memory().unwrap();
        assert!(restore_latest_backup(&store2, enc.path())
            .unwrap()
            .is_none());
        assert!(encrypted_backup_exists(enc.path()));
    }
}
