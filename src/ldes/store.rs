//! SQLite persistence for LDES: stream configuration, the member log, the
//! sealed fragment bounds, the retention policy and its sweep, and sync
//! bookmarks. Lives in the identity DB next to the dataset registry.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, OptionalExtension};

use crate::auth::db::AuthDb;

#[derive(Debug, Clone, serde::Serialize)]
pub struct StreamConfig {
    pub enabled: bool,
    pub page_size: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Member {
    pub id: i64,
    pub dataset_id: String,
    pub entity_iri: String,
    pub graph_iri: String,
    pub created_at: String,
    pub deleted: bool,
    /// The entity's description at that moment, as N-Triples (empty for a
    /// tombstone).
    pub ntriples: String,
}

pub fn stream(db: &AuthDb, dataset_id: &str) -> anyhow::Result<Option<StreamConfig>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            "SELECT enabled, page_size, created_at FROM ldes_streams WHERE dataset_id = ?1",
            params![dataset_id],
            |r| {
                Ok(StreamConfig {
                    enabled: r.get::<_, i64>(0)? != 0,
                    page_size: r.get::<_, i64>(1)?.max(1) as u64,
                    created_at: r.get(2)?,
                })
            },
        )
        .optional()?)
}

pub fn set_stream(
    db: &AuthDb,
    dataset_id: &str,
    enabled: bool,
    page_size: u64,
) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    conn.execute(
        "INSERT INTO ldes_streams (dataset_id, enabled, page_size, created_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(dataset_id) DO UPDATE SET enabled = excluded.enabled, page_size = excluded.page_size",
        params![dataset_id, enabled as i64, page_size.max(1) as i64, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// `(graph, dataset)` pairs for the given graphs that belong to a dataset with
/// an enabled stream. Graphs marked private are not tracked: a stream is read
/// by every viewer of the dataset, so private data never becomes a member.
/// The common case — no streams at all — is one indexed query returning
/// nothing.
pub fn tracked(db: &AuthDb, graphs: &[String]) -> anyhow::Result<Vec<(String, String)>> {
    if graphs.is_empty() {
        return Ok(Vec::new());
    }
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(
        "SELECT g.graph_iri, g.dataset_id FROM dataset_graphs g \
         JOIN ldes_streams s ON s.dataset_id = g.dataset_id AND s.enabled = 1 \
         WHERE g.graph_iri = ?1 AND g.private = 0",
    )?;
    let mut out = Vec::new();
    for g in graphs {
        let rows = stmt.query_map(params![g], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub fn insert_member(
    db: &AuthDb,
    dataset_id: &str,
    entity_iri: &str,
    graph_iri: &str,
    created_at: &str,
    deleted: bool,
    ntriples: &str,
) -> anyhow::Result<i64> {
    let conn = db.pool().get()?;
    conn.execute(
        "INSERT INTO ldes_members (dataset_id, entity_iri, graph_iri, created_at, deleted, ntriples) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![dataset_id, entity_iri, graph_iri, created_at, deleted as i64, ntriples],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn member_count(db: &AuthDb, dataset_id: &str) -> anyhow::Result<u64> {
    let conn = db.pool().get()?;
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM ldes_members WHERE dataset_id = ?1",
        params![dataset_id],
        |r| r.get::<_, i64>(0),
    )? as u64)
}

fn row_to_member(r: &rusqlite::Row<'_>) -> rusqlite::Result<Member> {
    Ok(Member {
        id: r.get(0)?,
        dataset_id: r.get(1)?,
        entity_iri: r.get(2)?,
        graph_iri: r.get(3)?,
        created_at: r.get(4)?,
        deleted: r.get::<_, i64>(5)? != 0,
        ntriples: r.get(6)?,
    })
}

const MEMBER_COLUMNS: &str = "id, dataset_id, entity_iri, graph_iri, created_at, deleted, ntriples";

/// Members of page `page` (1-based) of the *unsealed tail* — the members with
/// an id above `after_id`, in insertion order, `page_size` per page. With no
/// sealed nodes (`after_id = 0`) this is the whole stream paged from the
/// start, which is how every stream was served before sealing existed.
pub fn tail_page(
    db: &AuthDb,
    dataset_id: &str,
    after_id: i64,
    page: u64,
    page_size: u64,
) -> anyhow::Result<Vec<Member>> {
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {MEMBER_COLUMNS} FROM ldes_members \
         WHERE dataset_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3 OFFSET ?4"
    ))?;
    let offset = page.saturating_sub(1).saturating_mul(page_size) as i64;
    let rows = stmt.query_map(
        params![dataset_id, after_id, page_size as i64, offset],
        row_to_member,
    )?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// The members of a sealed node: whatever is left of the id range it froze.
pub fn members_between(
    db: &AuthDb,
    dataset_id: &str,
    first_id: i64,
    last_id: i64,
) -> anyhow::Result<Vec<Member>> {
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {MEMBER_COLUMNS} FROM ldes_members \
         WHERE dataset_id = ?1 AND id BETWEEN ?2 AND ?3 ORDER BY id"
    ))?;
    let rows = stmt.query_map(params![dataset_id, first_id, last_id], row_to_member)?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// Members with an id above `after_id` (the unsealed tail's size).
pub fn count_after(db: &AuthDb, dataset_id: &str, after_id: i64) -> anyhow::Result<u64> {
    let conn = db.pool().get()?;
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM ldes_members WHERE dataset_id = ?1 AND id > ?2",
        params![dataset_id, after_id],
        |r| r.get::<_, i64>(0),
    )? as u64)
}

// ─── Sealed fragments ───────────────────────────────────────────────────────

/// A frozen fragment: the members with ids in `first_id..=last_id` are node
/// `node`, forever. `members` is how many of them still exist (retention
/// only ever removes rows inside a sealed range, never moves one).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SealedNode {
    pub node: u64,
    pub first_id: i64,
    pub last_id: i64,
    /// `dct:created` of the first member *after* this node at sealing time:
    /// the `tree:value` of the relation out of it, stable no matter what is
    /// pruned later.
    pub next_created_at: String,
    pub sealed_at: String,
    pub members: u64,
}

/// The sealed nodes of a stream in node order, with their surviving member
/// counts.
pub fn sealed_nodes(db: &AuthDb, dataset_id: &str) -> anyhow::Result<Vec<SealedNode>> {
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(
        "SELECT n.node, n.first_id, n.last_id, n.next_created_at, n.sealed_at, \
                (SELECT COUNT(*) FROM ldes_members m \
                  WHERE m.dataset_id = n.dataset_id AND m.id BETWEEN n.first_id AND n.last_id) \
         FROM ldes_nodes n WHERE n.dataset_id = ?1 ORDER BY n.node",
    )?;
    let rows = stmt.query_map(params![dataset_id], |r| {
        Ok(SealedNode {
            node: r.get::<_, i64>(0)?.max(0) as u64,
            first_id: r.get(1)?,
            last_id: r.get(2)?,
            next_created_at: r.get(3)?,
            sealed_at: r.get(4)?,
            members: r.get::<_, i64>(5)?.max(0) as u64,
        })
    })?;
    rows.map(|r| r.map_err(Into::into)).collect()
}

/// Seal every full page beyond the last sealed one. A page is sealed only
/// once the page after it has a member — so the relation out of it always
/// has a `tree:value`, and a full last page stays the mutable tail until
/// the next member arrives, exactly as it was served before sealing existed.
/// Idempotent; returns the number of nodes sealed.
pub fn seal_full_pages(db: &AuthDb, dataset_id: &str, page_size: u64) -> anyhow::Result<usize> {
    let page_size = page_size.max(1) as usize;
    let conn = db.pool().get()?;
    let mut sealed = 0;
    loop {
        let (last_node, last_id): (i64, i64) = conn
            .query_row(
                "SELECT node, last_id FROM ldes_nodes WHERE dataset_id = ?1 ORDER BY node DESC LIMIT 1",
                params![dataset_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((0, 0));
        let mut stmt = conn.prepare(
            "SELECT id, created_at FROM ldes_members WHERE dataset_id = ?1 AND id > ?2 \
             ORDER BY id LIMIT ?3",
        )?;
        let window: Vec<(i64, String)> = stmt
            .query_map(params![dataset_id, last_id, (page_size + 1) as i64], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        if window.len() <= page_size {
            return Ok(sealed);
        }
        conn.execute(
            "INSERT INTO ldes_nodes (dataset_id, node, first_id, last_id, next_created_at, sealed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                dataset_id,
                last_node + 1,
                window[0].0,
                window[page_size - 1].0,
                window[page_size].1,
                Utc::now().to_rfc3339()
            ],
        )?;
        sealed += 1;
    }
}

// ─── Retention ──────────────────────────────────────────────────────────────

/// A retention policy in the LDES 1.0 §4.4 vocabulary. Durations are
/// `xsd:duration` lexical forms, `starting_from` an `xsd:dateTime`. An empty
/// policy (every field `None`) means "keep every member".
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetentionPolicy {
    /// `ldes:fullLogDuration` — from now back this far, every member is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_log_duration: Option<String>,
    /// `ldes:versionAmount` — the newest N versions of each entity are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_amount: Option<u64>,
    /// `ldes:versionDuration` — those N versions are kept only this long
    /// (default: for ever).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_duration: Option<String>,
    /// `ldes:versionDeleteDuration` — deletions (tombstones) are kept this
    /// long, after which the entity's history is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_delete_duration: Option<String>,
    /// `ldes:startingFrom` — members created before this instant are not
    /// kept at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starting_from: Option<String>,
}

impl RetentionPolicy {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Every field well-formed, every duration positive, the amount > 0
    /// (the vocabulary: "MUST be greater than 0").
    pub fn validate(&self) -> Result<(), String> {
        for (name, d) in [
            ("full_log_duration", &self.full_log_duration),
            ("version_duration", &self.version_duration),
            ("version_delete_duration", &self.version_delete_duration),
        ] {
            if let Some(d) = d {
                parse_xsd_duration(d).map_err(|e| format!("{name}: {e}"))?;
            }
        }
        if self.version_amount == Some(0) {
            return Err("version_amount must be greater than 0".into());
        }
        if self.version_duration.is_some() && self.version_amount.is_none() {
            return Err("version_duration is only meaningful with version_amount".into());
        }
        if let Some(t) = &self.starting_from {
            DateTime::parse_from_rfc3339(t)
                .map_err(|e| format!("starting_from: {e} (an xsd:dateTime with a timezone)"))?;
        }
        Ok(())
    }
}

/// The `PnYnMnDTnHnMnS` subset of `xsd:duration`, as a fixed span: a year is
/// taken as 365 days and a month as 30 days — retention windows are not
/// calendar arithmetic, and the declared lexical form is what the stream
/// publishes verbatim. Negative and zero durations are rejected.
pub fn parse_xsd_duration(s: &str) -> Result<Duration, String> {
    let rest = s
        .strip_prefix('P')
        .ok_or_else(|| format!("`{s}` is not an xsd:duration (expected PnYnMnDTnHnMnS)"))?;
    if s.starts_with('-') || rest.is_empty() {
        return Err(format!("`{s}` is not a positive xsd:duration"));
    }
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) if !t.is_empty() => (d, t),
        Some(_) => return Err(format!("`{s}`: nothing follows T")),
        None => (rest, ""),
    };
    let mut secs: f64 = 0.0;
    let mut take = |part: &str, allowed: &[(char, f64)]| -> Result<(), String> {
        let mut num = String::new();
        let mut last_idx = usize::MAX;
        for ch in part.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                num.push(ch);
                continue;
            }
            let idx = allowed
                .iter()
                .position(|(c, _)| *c == ch)
                .ok_or_else(|| format!("`{s}`: unexpected designator `{ch}`"))?;
            if last_idx != usize::MAX && idx <= last_idx {
                return Err(format!("`{s}`: designators out of order"));
            }
            last_idx = idx;
            let n: f64 = num
                .parse()
                .map_err(|_| format!("`{s}`: `{num}` is not a number"))?;
            if ch != 'S' && num.contains('.') {
                return Err(format!("`{s}`: only seconds may be fractional"));
            }
            secs += n * allowed[idx].1;
            num.clear();
        }
        if !num.is_empty() {
            return Err(format!("`{s}`: a number without a designator"));
        }
        Ok(())
    };
    take(
        date,
        &[
            ('Y', 365.0 * 86_400.0),
            ('M', 30.0 * 86_400.0),
            ('W', 7.0 * 86_400.0),
            ('D', 86_400.0),
        ],
    )?;
    take(time, &[('H', 3_600.0), ('M', 60.0), ('S', 1.0)])?;
    if secs <= 0.0 {
        return Err(format!("`{s}` is not a positive duration"));
    }
    Duration::try_milliseconds((secs * 1000.0).round() as i64)
        .ok_or_else(|| format!("`{s}` is out of range"))
}

pub fn retention(db: &AuthDb, dataset_id: &str) -> anyhow::Result<Option<RetentionPolicy>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            "SELECT full_log_duration, version_amount, version_duration, version_delete_duration, starting_from \
             FROM ldes_retention WHERE dataset_id = ?1",
            params![dataset_id],
            |r| {
                Ok(RetentionPolicy {
                    full_log_duration: r.get(0)?,
                    version_amount: r.get::<_, Option<i64>>(1)?.map(|n| n.max(0) as u64),
                    version_duration: r.get(2)?,
                    version_delete_duration: r.get(3)?,
                    starting_from: r.get(4)?,
                })
            },
        )
        .optional()?
        .filter(|p| !p.is_empty()))
}

/// Store the policy; an empty policy clears it.
pub fn set_retention(
    db: &AuthDb,
    dataset_id: &str,
    policy: &RetentionPolicy,
) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    if policy.is_empty() {
        conn.execute(
            "DELETE FROM ldes_retention WHERE dataset_id = ?1",
            params![dataset_id],
        )?;
        return Ok(());
    }
    conn.execute(
        "INSERT INTO ldes_retention (dataset_id, full_log_duration, version_amount, version_duration, \
                                     version_delete_duration, starting_from, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(dataset_id) DO UPDATE SET \
           full_log_duration = excluded.full_log_duration, \
           version_amount = excluded.version_amount, \
           version_duration = excluded.version_duration, \
           version_delete_duration = excluded.version_delete_duration, \
           starting_from = excluded.starting_from, \
           updated_at = excluded.updated_at",
        params![
            dataset_id,
            policy.full_log_duration,
            policy.version_amount.map(|n| n as i64),
            policy.version_duration,
            policy.version_delete_duration,
            policy.starting_from,
            Utc::now().to_rfc3339()
        ],
    )?;
    Ok(())
}

/// How much longer than declared a member is kept, so that a consumer's own
/// clock skew ("the consumer MUST take into account a safe buffer", LDES
/// §4.4) never finds the server stricter than its policy.
pub const SAFETY_BUFFER: Duration = Duration::minutes(5);

/// Delete the members `policy` no longer retains as of `now`, keeping every
/// member `buffer` longer than declared. A member is retained when any rule
/// keeps it: it is within the full-log window; it is one of the newest
/// `version_amount` versions of its entity (and within `version_duration`);
/// or it is a tombstone within `version_delete_duration`. A member created
/// before `starting_from` is never retained. Returns the number deleted.
///
/// Rows are deleted in place — sealed nodes keep their id ranges, so a page
/// only ever loses members and never changes which node a member is on.
pub fn prune(
    db: &AuthDb,
    dataset_id: &str,
    policy: &RetentionPolicy,
    now: DateTime<Utc>,
    buffer: Duration,
) -> anyhow::Result<usize> {
    if policy.is_empty() {
        return Ok(0);
    }
    policy.validate().map_err(|e| anyhow::anyhow!(e))?;
    let cut = |d: &Option<String>| -> Option<String> {
        d.as_deref()
            .and_then(|d| parse_xsd_duration(d).ok())
            .map(|d| (now - d - buffer).to_rfc3339())
    };
    let cut_full = cut(&policy.full_log_duration);
    let cut_version = cut(&policy.version_duration);
    let cut_delete = cut(&policy.version_delete_duration);
    let starting_from = policy
        .starting_from
        .as_deref()
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| (t.with_timezone(&Utc) - buffer).to_rfc3339());
    // No window at all: nothing bounds the log, so nothing is pruned except
    // what precedes starting_from.
    let unbounded = policy.full_log_duration.is_none() && policy.version_amount.is_none();
    // When deletions have their own window, tombstones fall under it alone;
    // otherwise a tombstone is simply the newest version of its entity.
    let versions_keep = if policy.version_delete_duration.is_some() {
        "(deleted = 0 AND :va IS NOT NULL AND rn <= :va \
           AND (:cut_version IS NULL OR julianday(created_at) >= julianday(:cut_version)))"
    } else {
        "(:va IS NOT NULL AND rn <= :va \
           AND (:cut_version IS NULL OR julianday(created_at) >= julianday(:cut_version)))"
    };
    let sql = format!(
        "DELETE FROM ldes_members WHERE dataset_id = :ds AND id IN ( \
           SELECT id FROM ( \
             SELECT id, created_at, deleted, \
                    ROW_NUMBER() OVER (PARTITION BY entity_iri ORDER BY id DESC) AS rn \
             FROM ldes_members WHERE dataset_id = :ds \
           ) WHERE \
             (:sf IS NOT NULL AND julianday(created_at) < julianday(:sf)) \
             OR NOT ( \
               :unbounded = 1 \
               OR (:cut_full IS NOT NULL AND julianday(created_at) >= julianday(:cut_full)) \
               OR {versions_keep} \
               OR (deleted = 1 AND :cut_delete IS NOT NULL \
                   AND julianday(created_at) >= julianday(:cut_delete)) \
             ) \
         )"
    );
    let conn = db.pool().get()?;
    let deleted = conn.execute(
        &sql,
        rusqlite::named_params! {
            ":ds": dataset_id,
            ":sf": starting_from,
            ":unbounded": unbounded as i64,
            ":cut_full": cut_full,
            ":va": policy.version_amount.map(|n| n as i64),
            ":cut_version": cut_version,
            ":cut_delete": cut_delete,
        },
    )?;
    Ok(deleted)
}

/// Sweep at most this often per dataset from the write path (a prune scans
/// the dataset's member log); `OTS_LDES_SWEEP_INTERVAL_SECS` overrides.
fn sweep_interval_secs() -> u64 {
    std::env::var("OTS_LDES_SWEEP_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(60)
}

fn last_sweeps() -> &'static Mutex<HashMap<String, Instant>> {
    static LAST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Apply the dataset's retention policy now, unless it ran less than a minute
/// ago (`force` ignores the debounce). Returns the number of members deleted;
/// failures are logged, never propagated — retention lag is always safe (the
/// server keeps more than it declares), the reverse would not be.
pub fn sweep(db: &AuthDb, dataset_id: &str, force: bool) -> usize {
    let policy = match retention(db, dataset_id) {
        Ok(Some(p)) => p,
        Ok(None) => return 0,
        Err(e) => {
            tracing::warn!("ldes: retention lookup failed for {dataset_id}: {e}");
            return 0;
        }
    };
    if !force {
        let mut last = last_sweeps().lock().unwrap_or_else(|p| p.into_inner());
        if last
            .get(dataset_id)
            .is_some_and(|t| t.elapsed().as_secs() < sweep_interval_secs())
        {
            return 0;
        }
        last.insert(dataset_id.to_string(), Instant::now());
    }
    match prune(db, dataset_id, &policy, Utc::now(), SAFETY_BUFFER) {
        Ok(n) => {
            if n > 0 {
                tracing::info!("ldes: retention removed {n} member(s) from {dataset_id}");
            }
            n
        }
        Err(e) => {
            tracing::warn!("ldes: retention sweep failed for {dataset_id}: {e}");
            0
        }
    }
}

// ─── Sync bookmarks ─────────────────────────────────────────────────────────

/// The newest member timestamp applied from `source_url` into `dataset_id`.
pub fn sync_bookmark(
    db: &AuthDb,
    dataset_id: &str,
    source_url: &str,
) -> anyhow::Result<Option<String>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            "SELECT last_timestamp FROM ldes_sync_state WHERE dataset_id = ?1 AND source_url = ?2",
            params![dataset_id, source_url],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

pub fn set_sync_bookmark(
    db: &AuthDb,
    dataset_id: &str,
    source_url: &str,
    last_timestamp: Option<&str>,
    applied: u64,
) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    conn.execute(
        "INSERT INTO ldes_sync_state (dataset_id, source_url, last_timestamp, members_applied, synced_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(dataset_id, source_url) DO UPDATE SET \
           last_timestamp = COALESCE(excluded.last_timestamp, ldes_sync_state.last_timestamp), \
           members_applied = ldes_sync_state.members_applied + excluded.members_applied, \
           synced_at = excluded.synced_at",
        params![dataset_id, source_url, last_timestamp, applied as i64, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xsd_duration_subset_parses_and_rejects() {
        assert_eq!(parse_xsd_duration("P1D").unwrap(), Duration::days(1));
        assert_eq!(
            parse_xsd_duration("PT1H30M").unwrap(),
            Duration::minutes(90)
        );
        assert_eq!(parse_xsd_duration("P1Y").unwrap(), Duration::days(365));
        assert_eq!(parse_xsd_duration("P2M").unwrap(), Duration::days(60));
        assert_eq!(parse_xsd_duration("P1W").unwrap(), Duration::days(7));
        assert_eq!(
            parse_xsd_duration("P1DT0.5S").unwrap(),
            Duration::days(1) + Duration::milliseconds(500)
        );
        for bad in [
            "", "P", "PT", "1D", "-P1D", "P0D", "PT1.5H", "P1H", "PD", "P1D1Y", "P1DT",
        ] {
            assert!(parse_xsd_duration(bad).is_err(), "{bad} is rejected");
        }
    }

    #[test]
    fn a_policy_validates_its_fields() {
        let ok = RetentionPolicy {
            full_log_duration: Some("P30D".into()),
            version_amount: Some(2),
            version_duration: Some("P1Y".into()),
            version_delete_duration: Some("P7D".into()),
            starting_from: Some("2026-01-01T00:00:00Z".into()),
        };
        assert_eq!(ok.validate(), Ok(()));
        assert!(RetentionPolicy::default().is_empty());
        assert!(RetentionPolicy {
            version_amount: Some(0),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(RetentionPolicy {
            version_duration: Some("P1D".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(RetentionPolicy {
            starting_from: Some("yesterday".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(RetentionPolicy {
            full_log_duration: Some("30 days".into()),
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
