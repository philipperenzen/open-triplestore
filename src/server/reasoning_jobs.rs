//! Background reasoning runs (`?async=true` on `POST /api/reasoning/materialize`
//! and `POST /api/reasoning/check`).
//!
//! A job records who started it and, once finished, the HTTP status and JSON
//! body the synchronous call would have answered with — so a 422
//! inconsistency or a 504 timeout reads the same either way. Jobs live in
//! process memory: a restart forgets them, and finished jobs are dropped an
//! hour after they finish (or sooner, past [`MAX_JOBS`]).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use serde_json::{json, Value};

/// Jobs kept at most; the oldest finished ones go first.
const MAX_JOBS: usize = 1000;
/// How long a finished job stays readable.
const KEEP_FINISHED: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub owner: String,
    pub kind: &'static str,
    pub status: &'static str,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub http_status: Option<u16>,
    pub result: Option<Value>,
    finished: Option<Instant>,
}

impl Job {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "kind": self.kind,
            "status": self.status,
            "created_at": self.created_at,
            "started_at": self.started_at,
            "finished_at": self.finished_at,
            "http_status": self.http_status,
            "result": self.result,
        })
    }
}

fn jobs() -> &'static Mutex<HashMap<String, Job>> {
    static JOBS: OnceLock<Mutex<HashMap<String, Job>>> = OnceLock::new();
    JOBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Register a queued job for `owner`; its id.
pub fn create(owner: &str, kind: &'static str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    let mut map = jobs().lock().unwrap_or_else(|p| p.into_inner());
    map.retain(|_, j| j.finished.is_none_or(|f| f.elapsed() < KEEP_FINISHED));
    if map.len() >= MAX_JOBS {
        let mut finished: Vec<(Instant, String)> = map
            .values()
            .filter_map(|j| j.finished.map(|f| (f, j.id.clone())))
            .collect();
        finished.sort();
        for (_, old) in finished.into_iter().take(map.len() + 1 - MAX_JOBS) {
            map.remove(&old);
        }
    }
    map.insert(
        id.clone(),
        Job {
            id: id.clone(),
            owner: owner.to_string(),
            kind,
            status: "queued",
            created_at: now(),
            started_at: None,
            finished_at: None,
            http_status: None,
            result: None,
            finished: None,
        },
    );
    id
}

pub fn start(id: &str) {
    if let Some(j) = jobs().lock().unwrap_or_else(|p| p.into_inner()).get_mut(id) {
        j.status = "running";
        j.started_at = Some(now());
    }
}

/// Record the response the synchronous call would have sent.
pub fn finish(id: &str, status: StatusCode, body: Value) {
    if let Some(j) = jobs().lock().unwrap_or_else(|p| p.into_inner()).get_mut(id) {
        j.status = if status.is_success() {
            "succeeded"
        } else {
            "failed"
        };
        j.http_status = Some(status.as_u16());
        j.result = Some(body);
        j.finished_at = Some(now());
        j.finished = Some(Instant::now());
    }
}

/// The job, if `user` may see it (its owner, or an admin).
pub fn get(id: &str, user: &str, is_admin: bool) -> Option<Job> {
    let map = jobs().lock().unwrap_or_else(|p| p.into_inner());
    map.get(id).filter(|j| is_admin || j.owner == user).cloned()
}

/// The 202 answer for a job just queued.
pub fn accepted(id: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    let location = format!("/api/reasoning/jobs/{id}");
    (
        StatusCode::ACCEPTED,
        [(axum::http::header::LOCATION, location.clone())],
        axum::Json(json!({ "job_id": id, "status": "queued", "location": location })),
    )
        .into_response()
}
