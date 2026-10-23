//! Scheduled sync runs (#530). A sync job's schedule is `off` (the default:
//! it runs when someone runs it), `auto` — every `ldes:pollingInterval` the
//! stream declares, which the LDES vocabulary defines as the number of
//! seconds a client should keep between synchronisation runs — or a fixed
//! interval; never more often than [`MIN_INTERVAL_SECS`] either way.
//!
//! * Where: on the node that writes. A follower, or a cluster member that
//!   does not lead right now, holds a copy of every job (the identity
//!   database is shipped to it) but its store is read-only, so it starts
//!   none; the member that leads next picks them up from `next_run_at`.
//! * As whom: the job's owner (`created_by`), checked at every run the way a
//!   request is checked: still an active user, allowed to write the dataset,
//!   and past the graph gate. A run the owner may no longer have pauses the
//!   job with `last_error` saying why; setting its schedule again resumes it.
//! * How many: at most `OTS_LDES_SYNC_CONCURRENCY` runs at once, on the
//!   blocking pool, and never two of one job — a request to run a job waits
//!   for its scheduled run, and the scheduler skips a job that is running.
//! * After a failure: the wait doubles with each failure in a row, up to
//!   [`MAX_BACKOFF_SECS`] (or the interval, when that is longer); a success
//!   resets it. Every run, whoever started it, records its outcome with
//!   [`store::record_sync_outcome`] and plans the next one.

use std::collections::HashSet;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

use super::client::{JobQuery, SyncJobView, Trigger};
use super::store::{self, Schedule, SyncJob};
use crate::auth::db::AuthDb;
use crate::auth::middleware::AuthenticatedUser;
use crate::server::AppState;

/// The shortest wait between two runs of a job.
pub const MIN_INTERVAL_SECS: u64 = 60;

/// The longest fixed interval a schedule takes (a leap year).
pub const MAX_INTERVAL_SECS: u64 = 366 * 24 * 3600;

/// The longest back-off after failures, unless the interval is longer.
pub const MAX_BACKOFF_SECS: u64 = 6 * 3600;

/// How often the server looks for due jobs.
pub const TICK: std::time::Duration = std::time::Duration::from_secs(10);

/// Where the scheduler reads the time: the system clock in the server, a
/// clock in the test's hand otherwise.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// The seconds between runs on `schedule`, given the `ldes:pollingInterval`
/// the stream declared at its last successful run; `None` when it is off. A
/// stream that declares none is read every [`MIN_INTERVAL_SECS`].
pub fn interval_secs(schedule: Schedule, polling_interval: Option<u64>) -> Option<u64> {
    let secs = match schedule {
        Schedule::Off => return None,
        Schedule::Auto => polling_interval.unwrap_or(MIN_INTERVAL_SECS),
        Schedule::Every(s) => s,
    };
    Some(secs.clamp(MIN_INTERVAL_SECS, MAX_INTERVAL_SECS))
}

/// The wait after a run that ended `failures` failures in a row: the
/// interval, doubled for each.
pub fn wait_secs(interval: u64, failures: u32) -> u64 {
    if failures == 0 {
        return interval;
    }
    let cap = MAX_BACKOFF_SECS.max(interval);
    interval.saturating_mul(1u64 << failures.min(32)).min(cap)
}

/// The `ldes:pollingInterval` the job's last successful run reported.
fn polling_interval(job: &SyncJob) -> Option<u64> {
    job.last_report.as_ref()?.get("polling_interval")?.as_u64()
}

fn later(t: DateTime<Utc>, secs: u64) -> DateTime<Utc> {
    t + chrono::Duration::seconds(secs.min(MAX_INTERVAL_SECS) as i64)
}

/// When a job just put on a schedule is first due: an interval after its last
/// run, or now when that is past.
fn first_due(job: &SyncJob, interval: u64, now: DateTime<Utc>) -> DateTime<Utc> {
    job.last_run_at
        .as_deref()
        .or(job.synced_at.as_deref())
        .and_then(store::instant)
        .map(|t| later(t, interval))
        .filter(|t| *t > now)
        .unwrap_or(now)
}

fn is_due(job: &SyncJob, now: DateTime<Utc>) -> bool {
    job.next_run_at
        .as_deref()
        .and_then(store::instant)
        .is_none_or(|t| t <= now)
}

// ─── One run of a job at a time ─────────────────────────────────────────────

type JobKey = (String, String);

fn running() -> &'static (Mutex<HashSet<JobKey>>, Condvar) {
    static RUNNING: OnceLock<(Mutex<HashSet<JobKey>>, Condvar)> = OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// A run of a job in progress; dropping it lets the next run of that job
/// start.
pub(crate) struct Running(JobKey);

impl Running {
    /// The job, unless a run of it is going.
    fn try_claim(dataset_id: &str, url: &str) -> Option<Self> {
        let key = (dataset_id.to_string(), url.to_string());
        let mut set = running().0.lock().unwrap_or_else(|p| p.into_inner());
        set.insert(key.clone()).then_some(Running(key))
    }

    /// The job, once a run of it that is going has ended. Blocking.
    pub(crate) fn claim(dataset_id: &str, url: &str) -> Self {
        let key = (dataset_id.to_string(), url.to_string());
        let (lock, ended) = running();
        let mut set = lock.lock().unwrap_or_else(|p| p.into_inner());
        while set.contains(&key) {
            set = ended.wait(set).unwrap_or_else(|p| p.into_inner());
        }
        set.insert(key.clone());
        Running(key)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let (lock, ended) = running();
        lock.lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.0);
        ended.notify_all();
    }
}

fn permits() -> &'static Arc<Semaphore> {
    static PERMITS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    PERMITS.get_or_init(|| Arc::new(Semaphore::new(crate::config::LDES_SYNC_CONCURRENCY.get())))
}

// ─── The scheduler ──────────────────────────────────────────────────────────

/// The background task: every `every`, start the jobs that are due by
/// `clock`.
pub fn spawn(state: AppState, every: std::time::Duration, clock: Clock) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            // Started, not awaited: a long run does not hold up the others,
            // and a job still running at the next tick is skipped.
            drop(start_due(&state, &clock).await);
        }
    })
}

/// One pass of the scheduler at `now`: start the jobs that are due and wait
/// for them. How many it started.
#[allow(dead_code)] // the server's scheduler does not wait for its runs; tests do
pub async fn run_due(state: &AppState, now: DateTime<Utc>) -> usize {
    let clock: Clock = Arc::new(move || now);
    let started = start_due(state, &clock).await;
    let n = started.len();
    for run in started {
        let _ = run.await;
    }
    n
}

/// Start the jobs due by `clock`, as many as there are free permits, on the
/// blocking pool. The rest stay due for the next tick.
async fn start_due(state: &AppState, clock: &Clock) -> Vec<JoinHandle<()>> {
    // A follower, or a cluster member that does not lead: its store refuses
    // writes, and the node that writes runs the jobs.
    if state.store.replication().read_only() {
        return Vec::new();
    }
    let now = clock();
    let db = state.auth_db.clone();
    let jobs = match tokio::task::spawn_blocking(move || store::scheduled_sync_jobs(&db)).await {
        Ok(Ok(jobs)) => jobs,
        Ok(Err(e)) => {
            tracing::warn!("ldes schedule: listing the scheduled jobs failed: {e}");
            return Vec::new();
        }
        Err(_) => return Vec::new(),
    };
    let mut started = Vec::new();
    for job in jobs.into_iter().filter(|j| is_due(j, now)) {
        let Ok(permit) = permits().clone().try_acquire_owned() else {
            break;
        };
        let Some(claim) = Running::try_claim(&job.dataset_id, &job.source_url) else {
            continue;
        };
        let (st, clock) = (state.clone(), clock.clone());
        started.push(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _claim = claim;
            run_scheduled(&st, &job, &clock);
        }));
    }
    started
}

/// Run a due job as its owner, or pause it when the owner may no longer
/// have it run. Blocking.
fn run_scheduled(state: &AppState, job: &SyncJob, clock: &Clock) {
    // Read again: it may have been changed or forgotten since it was listed.
    let job = match store::sync_job(&state.auth_db, &job.dataset_id, &job.source_url) {
        Ok(Some(j)) if Schedule::of(&j) != Schedule::Off && j.paused_at.is_none() => j,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!("ldes schedule: reading the job failed: {e}");
            return;
        }
    };
    let (ds, url) = (job.dataset_id.as_str(), job.source_url.as_str());
    let (Some(graph), Some(owner)) = (job.graph_iri.as_deref(), job.created_by.as_deref()) else {
        pause(
            state,
            &job,
            "the job has no recorded target graph or owner",
            clock(),
        );
        return;
    };
    match authority(state, ds, graph, owner) {
        Ok(Ok(())) => {}
        Ok(Err(why)) => {
            pause(state, &job, &why, clock());
            return;
        }
        // Not a verdict on the owner: a failed run, tried again later.
        Err(e) => {
            let e = format!("the owner's access could not be checked: {e}");
            if let Err(e) =
                store::record_sync_outcome(&state.auth_db, ds, url, graph, owner, Err(&e))
            {
                tracing::warn!("ldes schedule: recording the run's outcome failed: {e}");
            }
            after_run(&state.auth_db, ds, url, false, clock());
            return;
        }
    }
    let r = super::client::run_job(state, url, ds, graph, owner, Trigger::Schedule);
    if let Err(e) = &r {
        tracing::warn!("ldes schedule: the sync of <{url}> into {ds} failed: {e}");
    }
    after_run(&state.auth_db, ds, url, r.is_ok(), clock());
}

/// Whether `owner` may still have the job run — checked as `POST
/// /api/ldes/sync` checks its caller: `Ok(Err(why))` when not. The graph is
/// (re-)registered to the dataset when the gate lets it through.
fn authority(
    state: &AppState,
    dataset_id: &str,
    graph: &str,
    owner: &str,
) -> anyhow::Result<Result<(), String>> {
    let db = &state.auth_db;
    let Some(user) = db.get_user_by_id(owner)? else {
        return Ok(Err(format!("its owner ({owner}) no longer exists")));
    };
    if !user.is_active {
        return Ok(Err(format!("its owner {} is deactivated", user.username)));
    }
    let Some(dataset) = db.get_dataset(dataset_id)? else {
        return Ok(Err(format!("dataset '{dataset_id}' no longer exists")));
    };
    let principal = AuthenticatedUser::acting_for(db, &user);
    if !(principal.write_access || principal.is_admin())
        || !db.can_write_dataset(owner, &dataset)?
    {
        return Ok(Err(format!(
            "its owner {} may no longer write dataset '{dataset_id}'",
            user.username
        )));
    }
    match crate::auth::dataset_graph::gate_dataset_graph_target(
        &state.store,
        db,
        &state.base_url,
        dataset_id,
        graph,
        &principal,
    ) {
        Ok(claim) => {
            let _ =
                crate::auth::dataset_graph::register_claimed_graph(db, dataset_id, graph, claim);
            Ok(Ok(()))
        }
        Err(m) => Ok(Err(m)),
    }
}

fn pause(state: &AppState, job: &SyncJob, why: &str, at: DateTime<Utc>) {
    let reason = format!("schedule paused: {why}; set the schedule again to resume it");
    tracing::warn!(
        "ldes schedule: the sync of <{}> into {}: {reason}",
        job.source_url,
        job.dataset_id
    );
    if let Err(e) = store::pause_sync_schedule(
        &state.auth_db,
        &job.dataset_id,
        &job.source_url,
        &reason,
        at,
    ) {
        tracing::warn!("ldes schedule: pausing the job failed: {e}");
    }
}

/// Plan a scheduled job's next run after one ended at `now`, whoever started
/// it: an interval later after a success, the back-off after a failure.
/// Nothing for a job that is off or paused.
pub(crate) fn after_run(db: &AuthDb, dataset_id: &str, url: &str, ok: bool, now: DateTime<Utc>) {
    let job = match store::sync_job(db, dataset_id, url) {
        Ok(Some(j)) if j.paused_at.is_none() => j,
        Ok(_) => return,
        Err(e) => {
            tracing::warn!("ldes schedule: reading the job failed: {e}");
            return;
        }
    };
    // After the outcome is recorded: a success's report carries the
    // stream's current polling interval.
    let Some(interval) = interval_secs(Schedule::of(&job), polling_interval(&job)) else {
        return;
    };
    let failures = if ok {
        0
    } else {
        job.consecutive_failures.saturating_add(1)
    };
    let next = later(now, wait_secs(interval, failures));
    if let Err(e) = store::set_sync_next_run(db, dataset_id, url, failures, next) {
        tracing::warn!("ldes schedule: planning the next run failed: {e}");
    }
}

// ─── The admin route ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ScheduleBody {
    /// `off`, `auto` or `interval`.
    pub schedule: String,
    /// The seconds between runs of an `interval` schedule.
    pub interval_secs: Option<u64>,
}

fn parse_schedule(body: &ScheduleBody) -> Result<Schedule, String> {
    match body.schedule.as_str() {
        "off" => Ok(Schedule::Off),
        "auto" => Ok(Schedule::Auto),
        "interval" => match body.interval_secs {
            Some(s) if (MIN_INTERVAL_SECS..=MAX_INTERVAL_SECS).contains(&s) => {
                Ok(Schedule::Every(s))
            }
            Some(s) => Err(format!(
                "interval_secs: {s} is not between {MIN_INTERVAL_SECS} and {MAX_INTERVAL_SECS} \
                 seconds"
            )),
            None => Err(format!(
                "interval_secs: an interval schedule needs one, at least {MIN_INTERVAL_SECS} \
                 seconds"
            )),
        },
        other => Err(format!(
            "schedule: `{other}` is not one of off, auto and interval"
        )),
    }
}

/// PUT /api/admin/ldes/syncs/schedule?dataset_id=…&url=… — when a sync job
/// runs on its own. Setting it, to anything, also resumes a paused job and
/// forgets its failures. Answers the job as the list shows it. Admin only
/// (the admin router's guard).
pub async fn admin_set_schedule(
    State(state): State<AppState>,
    Query(q): Query<JobQuery>,
    Json(body): Json<ScheduleBody>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let schedule = parse_schedule(&body).map_err(|m| (StatusCode::BAD_REQUEST, m))?;
    let e500 = |e: anyhow::Error| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    let not_found = || (StatusCode::NOT_FOUND, "No such sync job".to_string());
    let job = store::sync_job(&state.auth_db, &q.dataset_id, &q.url)
        .map_err(e500)?
        .ok_or_else(not_found)?;
    if schedule != Schedule::Off && (job.graph_iri.is_none() || job.created_by.is_none()) {
        return Err((
            StatusCode::CONFLICT,
            "This job predates recorded target graphs and owners: run it once with POST \
             /api/ldes/sync before scheduling it"
                .to_string(),
        ));
    }
    let next = interval_secs(schedule, polling_interval(&job))
        .map(|interval| first_due(&job, interval, Utc::now()));
    store::set_sync_schedule(&state.auth_db, &q.dataset_id, &q.url, schedule, next)
        .map_err(e500)?;
    let job = store::sync_job(&state.auth_db, &q.dataset_id, &q.url)
        .map_err(e500)?
        .ok_or_else(not_found)?;
    Ok(Json(SyncJobView::of(&state, job)))
}
