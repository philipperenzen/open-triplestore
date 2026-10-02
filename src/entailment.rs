//! Selectable entailment regimes per dataset, with a materialisation toggle.
//!
//! A dataset picks a regime (`rdfs`, `owl2-rl`, `owl2-el`, `owl2-ql`,
//! `owl2-dl`) and a mode:
//!
//! * `materialize` — after every write to one of the dataset's graphs, the
//!   regime is re-run over the dataset's conformance layer (its instance,
//!   model, vocabulary, domain-value and linkset graphs) into the dataset's
//!   own entailment graph `urn:entailment:<regime>:<dataset>`, so tenants
//!   never share inferred triples and consequences of deleted data never
//!   linger;
//! * `off` — the entailment graph is cleared and no longer maintained.
//!
//! Queries opt in with `?entailment_dataset=<id>` (plus `?entailment=<regime>`
//! to pick a regime other than the configured one): the dataset's entailment
//! graph joins the query's default graph, exactly as the global
//! `?entailment=` graphs do.
//!
//! SWRL rules stored with a dataset always run (`swrl` feature): every
//! `swrl:Imp` in the dataset's `entailment`- and `model`-role graphs and in
//! the model version it conforms to is read in the SWRL RDF syntax and run
//! over the same reasoning sources as the regime. With a regime in
//! `materialize` mode the rules and the regime run to one joint fixed point
//! in the regime's graph; without one, the rules run on their own into
//! `urn:entailment:swrl:<dataset>`. Either way they re-run after every write
//! to one of the dataset's graphs, and `GET …/entailment` reports how the
//! last run went.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::auth::db::AuthDb;
use crate::auth::middleware::AuthenticatedUser;
use crate::auth::models::{Dataset, GraphKind, OwnerType, Role};
use crate::reasoning::identity::IdentityPolicy;
use crate::server::AppState;

pub const REGIMES: &[&str] = &["rdfs", "owl2-rl", "owl2-el", "owl2-ql", "owl2-dl"];

#[derive(Debug, Clone, Serialize)]
pub struct EntailmentConfig {
    pub dataset_id: String,
    pub regime: String,
    /// `materialize` | `off`
    pub mode: String,
    pub graph: String,
    pub updated_at: String,
    pub last_run_at: Option<String>,
    pub last_triples: Option<i64>,
}

pub fn dataset_entailment_graph(regime: &str, dataset_id: &str) -> String {
    format!("urn:entailment:{regime}:{dataset_id}")
}

pub fn config(db: &AuthDb, dataset_id: &str) -> anyhow::Result<Option<EntailmentConfig>> {
    let conn = db.pool().get()?;
    Ok(conn
        .query_row(
            "SELECT regime, mode, updated_at, last_run_at, last_triples FROM dataset_entailment WHERE dataset_id = ?1",
            params![dataset_id],
            |r| {
                let regime: String = r.get(0)?;
                Ok(EntailmentConfig {
                    dataset_id: dataset_id.to_string(),
                    graph: dataset_entailment_graph(&regime, dataset_id),
                    regime,
                    mode: r.get(1)?,
                    updated_at: r.get(2)?,
                    last_run_at: r.get(3)?,
                    last_triples: r.get(4)?,
                })
            },
        )
        .optional()?)
}

fn set_config(db: &AuthDb, dataset_id: &str, regime: &str, mode: &str) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    conn.execute(
        "INSERT INTO dataset_entailment (dataset_id, regime, mode, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(dataset_id) DO UPDATE SET regime = excluded.regime, mode = excluded.mode, updated_at = excluded.updated_at",
        params![dataset_id, regime, mode, chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

fn record_run(db: &AuthDb, dataset_id: &str, triples: i64) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    conn.execute(
        "UPDATE dataset_entailment SET last_run_at = ?2, last_triples = ?3 WHERE dataset_id = ?1",
        params![dataset_id, chrono::Utc::now().to_rfc3339(), triples],
    )?;
    Ok(())
}

/// Datasets in `materialize` mode that own any of `graphs`.
fn materialized_datasets_for(
    db: &AuthDb,
    graphs: &[String],
) -> anyhow::Result<Vec<(String, String)>> {
    if graphs.is_empty() {
        return Ok(Vec::new());
    }
    let conn = db.pool().get()?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT e.dataset_id, e.regime FROM dataset_entailment e \
         JOIN dataset_graphs g ON g.dataset_id = e.dataset_id \
         WHERE e.mode = 'materialize' AND g.graph_iri = ?1",
    )?;
    let mut out: Vec<(String, String)> = Vec::new();
    for g in graphs {
        // A dataset's own entailment graph is never a trigger.
        if g.starts_with("urn:entailment:") {
            continue;
        }
        for row in stmt.query_map(params![g], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let row = row?;
            if !out.contains(&row) {
                out.push(row);
            }
        }
    }
    Ok(out)
}

/// The store write generation each dataset's entailment graph was last
/// brought in sync at (a successful run). Process-local: it decides whether
/// an additive write may *extend* the graph instead of rebuilding it, and it
/// skips a re-run when nothing was written since the last one.
fn last_run_generations() -> &'static std::sync::Mutex<std::collections::HashMap<String, u64>> {
    static MAP: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, u64>>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn last_run_generation(dataset_id: &str) -> Option<u64> {
    last_run_generations()
        .lock()
        .ok()
        .and_then(|m| m.get(dataset_id).copied())
}

/// The graph a dataset's stored SWRL rules write to when the dataset has no
/// regime in `materialize` mode.
pub fn dataset_rules_graph(dataset_id: &str) -> String {
    format!("urn:entailment:swrl:{dataset_id}")
}

/// The dataset's inference graph: its regime's entailment graph in
/// `materialize` mode, else the graph its stored rules write to.
pub fn inference_graph(db: &AuthDb, dataset_id: &str) -> String {
    match config(db, dataset_id) {
        Ok(Some(c)) if c.mode == "materialize" => c.graph,
        _ => dataset_rules_graph(dataset_id),
    }
}

/// The most rounds of rules-then-regime a joint fixed point may take.
#[cfg(feature = "swrl")]
const MAX_JOINT_ROUNDS: usize = 32;

/// The most iterations one pass of a dataset's stored rules may take.
#[cfg(feature = "swrl")]
const RULE_MAX_ITERATIONS: usize = 1000;

/// What the last run of a dataset's stored SWRL rules did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RulesReport {
    /// The graphs the rules were read from.
    pub rule_graphs: Vec<String>,
    /// How many rules ran.
    pub rules: usize,
    /// The graph they wrote to.
    pub target_graph: String,
    /// Triples the rules derived (the regime's own are counted separately).
    pub triples_inferred: usize,
    /// Rounds of rules-then-regime it took.
    pub rounds: usize,
    /// Whether the joint fixed point was reached.
    pub converged: bool,
    /// Why the rules did not run, or stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub ran_at: String,
}

fn last_rules_reports() -> &'static std::sync::Mutex<std::collections::HashMap<String, RulesReport>>
{
    static MAP: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, RulesReport>>,
    > = std::sync::OnceLock::new();
    MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn record_rules(dataset_id: &str, report: Option<&RulesReport>) {
    if let Ok(mut m) = last_rules_reports().lock() {
        match report {
            Some(r) => {
                m.insert(dataset_id.to_string(), r.clone());
            }
            None => {
                m.remove(dataset_id);
            }
        }
    }
}

/// The last rules report recorded for `dataset_id` in this process.
pub fn last_rules_report(dataset_id: &str) -> Option<RulesReport> {
    last_rules_reports()
        .lock()
        .ok()
        .and_then(|m| m.get(dataset_id).cloned())
}

/// The graphs whose `swrl:Imp` rules run with `ds`: its `entailment`- and
/// `model`-role graphs and the graphs of the model version it conforms to.
pub fn rule_graphs(state: &AppState, ds: &Dataset) -> Vec<String> {
    let layer = crate::conformance::resolve(state, ds);
    let mut graphs: Vec<String> = layer
        .graphs
        .iter()
        .filter(|g| matches!(g.role, Some(GraphKind::Entailment | GraphKind::Model)))
        .map(|g| g.graph_iri.clone())
        .collect();
    if let Some(model) = layer.conforms_to_model {
        graphs.push(model.graph_iri);
        graphs.extend(model.sub_graphs);
    }
    graphs.sort();
    graphs.dedup();
    graphs
}

/// Whether `ds` stores any SWRL rule. Probes the dataset's own rule graphs
/// first and resolves its model version only when it declares one, as this
/// runs after every write.
#[cfg(feature = "swrl")]
fn holds_rules(state: &AppState, ds: &Dataset) -> bool {
    let own: Vec<String> = state
        .auth_db
        .list_dataset_graph_entries(&ds.id)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| matches!(e.graph_role, Some(GraphKind::Entailment | GraphKind::Model)))
        .map(|e| e.graph_iri)
        .collect();
    if crate::swrl::rdf::graphs_hold_rules(state.store.store(), &own) {
        return true;
    }
    ds.conforms_to_model
        .as_deref()
        .is_some_and(|m| !m.is_empty())
        && crate::swrl::rdf::graphs_hold_rules(state.store.store(), &rule_graphs(state, ds))
}

#[cfg(not(feature = "swrl"))]
fn holds_rules(_state: &AppState, _ds: &Dataset) -> bool {
    false
}

/// Run the stored rules of `ds` over `sources` into `target`; with a
/// `regime`, alternate rules and regime until neither derives anything new.
/// `None` when the dataset stores no rules. The caller has already run the
/// regime to its own fixed point.
#[cfg(feature = "swrl")]
fn run_stored_rules(
    state: &AppState,
    ds: &Dataset,
    sources: &[String],
    target: &str,
    regime: Option<(&str, IdentityPolicy)>,
) -> Option<RulesReport> {
    if !holds_rules(state, ds) {
        return None;
    }
    let graphs = rule_graphs(state, ds);
    let mut report = RulesReport {
        rule_graphs: graphs.clone(),
        target_graph: target.to_string(),
        ran_at: chrono::Utc::now().to_rfc3339(),
        ..RulesReport::default()
    };
    let rules = match crate::swrl::rdf::rules_in_graphs(state.store.store(), &graphs) {
        Ok(rules) => rules,
        Err(e) => {
            report.error = Some(e);
            return Some(report);
        }
    };
    report.rules = rules.len();
    let compiled = match crate::swrl::compile_rules(&rules, Some(target), Some(sources)) {
        Ok(c) => c,
        Err(e) => {
            report.error = Some(e);
            return Some(report);
        }
    };
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(state.write_timeout_secs);
    for round in 1..=MAX_JOINT_ROUNDS {
        report.rounds = round;
        let run = match crate::swrl::execute_compiled(
            &state.store,
            &compiled,
            RULE_MAX_ITERATIONS,
            Some(deadline),
        ) {
            Ok(run) => run,
            Err(e) => {
                report.error = Some(e);
                return Some(report);
            }
        };
        report.triples_inferred += run.triples_inferred;
        if !run.converged {
            report.error = Some(format!(
                "the rules stopped before their fixed point ({:?})",
                run.stop_reason
            ));
            return Some(report);
        }
        // The regime was at its fixed point before this pass: if the rules
        // added nothing, so is the pair.
        if run.triples_inferred == 0 {
            report.converged = true;
            return Some(report);
        }
        let Some((regime, policy)) = regime else {
            report.converged = true;
            return Some(report);
        };
        match crate::server::routes::run_regime(
            state,
            regime,
            Some(sources.to_vec()),
            target,
            policy,
        ) {
            Ok(Some(r)) if r.triples_added == 0 => {
                report.converged = true;
                return Some(report);
            }
            Ok(_) => {}
            Err(e) => {
                report.error = Some(format!("{e:?}"));
                return Some(report);
            }
        }
    }
    report.error = Some(format!(
        "no joint fixed point with {} after {MAX_JOINT_ROUNDS} rounds",
        regime.map(|(r, _)| r).unwrap_or("the regime")
    ));
    Some(report)
}

#[cfg(not(feature = "swrl"))]
fn run_stored_rules(
    _state: &AppState,
    _ds: &Dataset,
    _sources: &[String],
    _target: &str,
    _regime: Option<(&str, IdentityPolicy)>,
) -> Option<RulesReport> {
    None
}

/// Run the stored rules of a dataset that has no regime in `materialize`
/// mode into its rules graph, cleared first unless `extend`.
pub fn run_rules_for_dataset(
    state: &AppState,
    dataset_id: &str,
    extend: bool,
) -> Result<Option<RulesReport>, String> {
    let ds = state
        .auth_db
        .get_dataset(dataset_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("dataset {dataset_id} not found"))?;
    let (sources, _) = reasoning_sources(state, &ds);
    let target = dataset_rules_graph(dataset_id);
    if !extend {
        state
            .store
            .update(&format!("CLEAR SILENT GRAPH <{target}>"))
            .map_err(|e| format!("clearing <{target}>: {e}"))?;
    }
    let report = run_stored_rules(state, &ds, &sources, &target, None);
    record_rules(dataset_id, report.as_ref());
    if let Ok(mut m) = last_run_generations().lock() {
        m.insert(dataset_id.to_string(), state.store.write_generation());
    }
    Ok(report)
}

/// Re-materialise `regime` for `dataset_id` into its entailment graph —
/// cleared and rebuilt from scratch. Returns the number of triples in the
/// entailment graph afterwards.
pub fn run_for_dataset(state: &AppState, dataset_id: &str, regime: &str) -> Result<i64, String> {
    run_for_dataset_with(state, dataset_id, regime, false)
}

/// As [`run_for_dataset`]; with `extend` the entailment graph is NOT cleared
/// first: the regime's rules are monotone, so after a write that only added
/// quads the fixed point re-run on top of the existing consequences is the
/// same graph a rebuild would produce, minus the rebuild.
pub fn run_for_dataset_with(
    state: &AppState,
    dataset_id: &str,
    regime: &str,
    extend: bool,
) -> Result<i64, String> {
    let ds = state
        .auth_db
        .get_dataset(dataset_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("dataset {dataset_id} not found"))?;
    let (sources, identity) = reasoning_sources(state, &ds);
    let target = dataset_entailment_graph(regime, dataset_id);
    if !extend {
        // The regime's graph is the inference graph now; stored rules write
        // there, not to the rules-only graph.
        for g in [target.clone(), dataset_rules_graph(dataset_id)] {
            state
                .store
                .update(&format!("CLEAR SILENT GRAPH <{g}>"))
                .map_err(|e| format!("clearing <{g}>: {e}"))?;
        }
    }
    crate::server::routes::run_regime(
        state,
        regime,
        Some(sources.clone()),
        &target,
        identity.policy,
    )
    .map_err(|e| format!("{e:?}"))?;
    // Stored rules join the regime to one joint fixed point.
    let rules = run_stored_rules(
        state,
        &ds,
        &sources,
        &target,
        Some((regime, identity.policy)),
    );
    record_rules(dataset_id, rules.as_ref());
    let n = state.store.graph_count_cached(Some(&target)).unwrap_or(0) as i64;
    let _ = record_run(&state.auth_db, dataset_id, n);
    if let Ok(mut m) = last_run_generations().lock() {
        m.insert(dataset_id.to_string(), state.store.write_generation());
    }
    Ok(n)
}

/// After a write to `graphs`: re-materialise every dataset in `materialize`
/// mode that owns one of them. Synchronous — call it from a blocking context
/// (the write paths already sit in one for the LDES capture). Best-effort;
/// failures are logged.
pub fn after_write(state: &AppState, graphs: &[String]) {
    after_write_kind(state, graphs, false)
}

/// After a write that only ADDED quads to `graphs` (a Graph Store `POST`):
/// the affected entailment graphs are extended in place — the rules re-run to
/// their fixed point on top of the existing consequences — instead of being
/// cleared and rebuilt. A dataset without a successful run on record since
/// the process started is rebuilt.
pub fn after_additive_write(state: &AppState, graphs: &[String]) {
    after_write_kind(state, graphs, true)
}

/// Datasets that own any of `graphs`, are not in `skip`, and store SWRL
/// rules: their rules re-run after the write even without a regime.
fn rule_datasets_for(state: &AppState, graphs: &[String], skip: &[String]) -> Vec<String> {
    let owners = (|| -> anyhow::Result<Vec<String>> {
        let conn = state.auth_db.pool().get()?;
        let mut stmt =
            conn.prepare("SELECT DISTINCT dataset_id FROM dataset_graphs WHERE graph_iri = ?1")?;
        let mut out: Vec<String> = Vec::new();
        for g in graphs {
            if g.starts_with("urn:entailment:") {
                continue;
            }
            for id in stmt.query_map(params![g], |r| r.get::<_, String>(0))? {
                let id = id?;
                if !out.contains(&id) && !skip.contains(&id) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    })();
    let owners = match owners {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("entailment: rule-dataset lookup failed: {e}");
            return Vec::new();
        }
    };
    owners
        .into_iter()
        .filter(
            |id| matches!(state.auth_db.get_dataset(id), Ok(Some(ds)) if holds_rules(state, &ds)),
        )
        .collect()
}

fn after_write_kind(state: &AppState, graphs: &[String], additive: bool) {
    let targets = match materialized_datasets_for(&state.auth_db, graphs) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("entailment: lookup failed: {e}");
            return;
        }
    };
    let regime_ids: Vec<String> = targets.iter().map(|(ds, _)| ds.clone()).collect();
    let mut targets: Vec<(String, Option<String>)> = targets
        .into_iter()
        .map(|(ds, regime)| (ds, Some(regime)))
        .collect();
    targets.extend(
        rule_datasets_for(state, graphs, &regime_ids)
            .into_iter()
            .map(|ds| (ds, None)),
    );
    let generation = state.store.write_generation();
    for (ds, regime) in targets {
        let last = last_run_generation(&ds);
        if last == Some(generation) {
            tracing::debug!("entailment: {ds} already in sync at generation {generation}");
            continue;
        }
        let extend = additive && last.is_some();
        let Some(regime) = regime else {
            match run_rules_for_dataset(state, &ds, extend) {
                Ok(r) => tracing::debug!(
                    "entailment: stored rules for {ds}: {} triples",
                    r.map(|r| r.triples_inferred).unwrap_or(0)
                ),
                Err(e) => tracing::warn!("entailment: stored rules for {ds} failed: {e}"),
            }
            continue;
        };
        match run_for_dataset_with(state, &ds, &regime, extend) {
            Ok(n) => tracing::debug!(
                "entailment: {} {regime} for {ds}: {n} triples",
                if extend {
                    "extended"
                } else {
                    "re-materialised"
                }
            ),
            Err(e) => tracing::warn!("entailment: re-materialising {regime} for {ds} failed: {e}"),
        }
    }
}

// ── HTTP ────────────────────────────────────────────────────────────────────

type ApiErr = (StatusCode, String);

fn e500<E: std::fmt::Display>(e: E) -> ApiErr {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn visible(
    state: &AppState,
    uid: Option<&str>,
    id: &str,
) -> Result<crate::auth::models::Dataset, ApiErr> {
    let ds = state
        .auth_db
        .get_dataset(id)
        .map_err(e500)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Dataset not found".to_string()))?;
    if !state.auth_db.can_access_dataset(uid, &ds).map_err(e500)? {
        return Err((StatusCode::NOT_FOUND, "Dataset not found".to_string()));
    }
    Ok(ds)
}

/// GET /api/datasets/:id/entailment
pub async fn get_entailment(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let uid = user.as_ref().map(|Extension(u)| u.user_id.as_str());
    let ds = visible(&state, uid, &dataset_id)?;
    let mut v = match config(&state.auth_db, &dataset_id).map_err(e500)? {
        Some(c) => serde_json::to_value(c).unwrap(),
        None => serde_json::json!({
            "dataset_id": dataset_id,
            "regime": null,
            "mode": "off",
            "regimes": REGIMES,
        }),
    };
    // The identity policy in force and the graphs a run would read under it.
    let (sources, identity) = reasoning_sources(&state, &ds);
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "identity".into(),
            serde_json::json!(identity.policy.as_str()),
        );
        obj.insert("identity_source".into(), serde_json::json!(identity.source));
        obj.insert(
            "identity_options".into(),
            serde_json::json!(IdentityPolicy::ALL),
        );
        obj.insert("reasoning_sources".into(), serde_json::json!(sources));
        obj.insert(
            "inference_graph".into(),
            serde_json::json!(inference_graph(&state.auth_db, &dataset_id)),
        );
        obj.insert("rules".into(), rules_json(&state, &ds));
    }
    Ok(Json(v))
}

/// The stored rules of `ds` for `GET …/entailment`: where they are read
/// from, how many there are (or why they cannot be read), and the last run.
#[cfg(feature = "swrl")]
fn rules_json(state: &AppState, ds: &Dataset) -> serde_json::Value {
    let graphs = rule_graphs(state, ds);
    let (count, error) = match crate::swrl::rdf::rules_in_graphs(state.store.store(), &graphs) {
        Ok(rules) => (rules.len(), None),
        Err(e) => (0, Some(e)),
    };
    serde_json::json!({
        "graphs": graphs,
        "count": count,
        "error": error,
        "last_run": last_rules_report(&ds.id),
    })
}

#[cfg(not(feature = "swrl"))]
fn rules_json(_state: &AppState, _ds: &Dataset) -> serde_json::Value {
    serde_json::Value::Null
}

#[derive(Debug, Deserialize)]
pub struct EntailmentBody {
    pub regime: String,
    /// `materialize` (default) | `off`
    #[serde(default)]
    pub mode: Option<String>,
    /// Identity policy for this dataset: `sameas-off` | `sameas-narrow` |
    /// `sameas-full`, or `inherit` to drop the dataset's own setting and use
    /// the organisation's (or the built-in default). Omitted: unchanged.
    #[serde(default)]
    pub identity: Option<String>,
}

/// PUT /api/datasets/:id/entailment — select a regime and mode; in
/// `materialize` mode the regime runs immediately.
pub async fn put_entailment(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<EntailmentBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = visible(&state, Some(&user.user_id), &dataset_id)?;
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, &ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    let regime = body.regime.trim().to_ascii_lowercase();
    if !REGIMES.contains(&regime.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("unknown regime `{regime}`; one of {}", REGIMES.join(", ")),
        ));
    }
    let mode = body
        .mode
        .as_deref()
        .unwrap_or("materialize")
        .trim()
        .to_ascii_lowercase();
    if mode != "materialize" && mode != "off" {
        return Err((
            StatusCode::BAD_REQUEST,
            "mode must be `materialize` or `off`".to_string(),
        ));
    }
    if let Some(raw) = body.identity.as_deref() {
        if raw.trim().eq_ignore_ascii_case("inherit") {
            clear_identity_setting(&state.auth_db, "dataset", &dataset_id).map_err(e500)?;
        } else {
            let p = IdentityPolicy::parse(raw).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!(
                        "unknown identity policy `{raw}`; one of {} or `inherit`",
                        IdentityPolicy::ALL.join(", ")
                    ),
                )
            })?;
            set_identity_setting(&state.auth_db, "dataset", &dataset_id, p).map_err(e500)?;
        }
    }
    set_config(&state.auth_db, &dataset_id, &regime, &mode).map_err(e500)?;
    let graph = dataset_entailment_graph(&regime, &dataset_id);
    let st = state.clone();
    let id = dataset_id.clone();
    let r = regime.clone();
    let m = mode.clone();
    let g = graph.clone();
    let (triples, rules) =
        tokio::task::spawn_blocking(move || -> Result<(i64, Option<RulesReport>), String> {
            if m == "off" {
                st.store
                    .update(&format!("CLEAR SILENT GRAPH <{g}>"))
                    .map_err(|e| e.to_string())?;
                let _ = record_run(&st.auth_db, &id, 0);
                // Stored rules still run, into the rules-only graph.
                let rules = run_rules_for_dataset(&st, &id, false)?;
                return Ok((0, rules));
            }
            let n = run_for_dataset(&st, &id, &r)?;
            Ok((n, last_rules_report(&id)))
        })
        .await
        .map_err(e500)?
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    let identity = effective_identity(&state.auth_db, &ds);
    Ok(Json(serde_json::json!({
        "dataset_id": dataset_id,
        "regime": regime,
        "mode": mode,
        "graph": graph,
        "triples": triples,
        "rules": rules,
        "inference_graph": inference_graph(&state.auth_db, &dataset_id),
        "identity": identity.policy.as_str(),
        "identity_source": identity.source,
    })))
}

// ── Identity policy (owl:sameAs) ────────────────────────────────────────────
//
// Set per organisation (inherited by the datasets it owns) or per dataset
// (overrides the organisation's); a dataset with neither uses the built-in
// default. Stored in its own small table next to `dataset_entailment` — the
// identity database's schema (src/auth) is not touched.

/// The policy in force for a dataset and where it comes from.
#[derive(Debug, Clone, Serialize)]
pub struct EffectiveIdentity {
    pub policy: IdentityPolicy,
    /// `dataset` | `organisation` | `default`
    pub source: &'static str,
}

fn ensure_identity_table(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS identity_policy (
            scope TEXT NOT NULL,
            id TEXT NOT NULL,
            policy TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (scope, id)
        )",
    )
}

/// The setting stored for `scope` (`dataset` | `organisation`) and `id`, if any.
pub fn identity_setting(
    db: &AuthDb,
    scope: &str,
    id: &str,
) -> anyhow::Result<Option<IdentityPolicy>> {
    let conn = db.pool().get()?;
    ensure_identity_table(&conn)?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT policy FROM identity_policy WHERE scope = ?1 AND id = ?2",
            params![scope, id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw.as_deref().and_then(IdentityPolicy::parse))
}

pub fn set_identity_setting(
    db: &AuthDb,
    scope: &str,
    id: &str,
    policy: IdentityPolicy,
) -> anyhow::Result<()> {
    let conn = db.pool().get()?;
    ensure_identity_table(&conn)?;
    conn.execute(
        "INSERT INTO identity_policy (scope, id, policy, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(scope, id) DO UPDATE SET policy = excluded.policy, updated_at = excluded.updated_at",
        params![scope, id, policy.as_str(), chrono::Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Remove the setting; `Ok(true)` when one existed.
pub fn clear_identity_setting(db: &AuthDb, scope: &str, id: &str) -> anyhow::Result<bool> {
    let conn = db.pool().get()?;
    ensure_identity_table(&conn)?;
    let n = conn.execute(
        "DELETE FROM identity_policy WHERE scope = ?1 AND id = ?2",
        params![scope, id],
    )?;
    Ok(n > 0)
}

/// Dataset setting, else the owning organisation's, else the built-in default.
pub fn effective_identity(db: &AuthDb, ds: &Dataset) -> EffectiveIdentity {
    if let Ok(Some(policy)) = identity_setting(db, "dataset", &ds.id) {
        return EffectiveIdentity {
            policy,
            source: "dataset",
        };
    }
    if matches!(ds.owner_type, OwnerType::Organisation) {
        if let Ok(Some(policy)) = identity_setting(db, "organisation", &ds.owner_id) {
            return EffectiveIdentity {
                policy,
                source: "organisation",
            };
        }
    }
    EffectiveIdentity {
        policy: IdentityPolicy::default(),
        source: "default",
    }
}

/// The graphs a reasoner reads for `ds` under its identity policy: the
/// conformance layer's reasoning sources (`GET …/conformance`) minus the
/// `linkset`-role graphs, unless the policy is `sameas-full`. The conformance
/// endpoint keeps listing every candidate; this is the effective set.
pub fn reasoning_sources(state: &AppState, ds: &Dataset) -> (Vec<String>, EffectiveIdentity) {
    let identity = effective_identity(&state.auth_db, ds);
    let mut sources = crate::conformance::resolve(state, ds).reasoning_sources;
    if !identity.policy.includes_linksets() {
        let linksets: Vec<String> = state
            .auth_db
            .list_dataset_graph_entries(&ds.id)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| matches!(e.graph_role, Some(GraphKind::Linkset)))
            .map(|e| e.graph_iri)
            .collect();
        sources.retain(|g| !linksets.contains(g));
    }
    (sources, identity)
}

fn identity_json(
    scope: &str,
    id: &str,
    effective: &EffectiveIdentity,
    setting: Option<IdentityPolicy>,
) -> serde_json::Value {
    serde_json::json!({
        "scope": scope,
        "id": id,
        "policy": effective.policy.as_str(),
        "source": effective.source,
        "setting": setting.map(|p| p.as_str()),
        "description": effective.policy.description(),
        // Typed correspondences are never identity, whatever the policy.
        "never_identity": crate::reasoning::identity::CORRESPONDENCE_PREDICATES,
        "options": IdentityPolicy::ALL.iter().map(|s| serde_json::json!({
            "policy": s,
            "description": IdentityPolicy::parse(s).map(|p| p.description()).unwrap_or(""),
        })).collect::<Vec<_>>(),
    })
}

#[derive(Debug, Deserialize)]
pub struct IdentityBody {
    /// `sameas-off` | `sameas-narrow` | `sameas-full`
    pub policy: String,
}

fn parse_policy(raw: &str) -> Result<IdentityPolicy, ApiErr> {
    IdentityPolicy::parse(raw).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "unknown identity policy `{raw}`; one of {}",
                IdentityPolicy::ALL.join(", ")
            ),
        )
    })
}

/// Re-materialise `dataset_id` if it is in `materialize` mode, so a policy
/// change takes effect at once. Best-effort; failures are logged.
fn rematerialize_if_configured(state: &AppState, dataset_id: &str) {
    match config(&state.auth_db, dataset_id) {
        Ok(Some(c)) if c.mode == "materialize" => {
            if let Err(e) = run_for_dataset(state, dataset_id, &c.regime) {
                tracing::warn!("identity policy: re-materialising {dataset_id} failed: {e}");
            }
        }
        // Stored rules read the same sources, so they re-run too.
        _ => {
            if matches!(state.auth_db.get_dataset(dataset_id), Ok(Some(ds)) if holds_rules(state, &ds))
            {
                if let Err(e) = run_rules_for_dataset(state, dataset_id, false) {
                    tracing::warn!("identity policy: re-running rules of {dataset_id} failed: {e}");
                }
            }
        }
    }
}

/// Re-materialise every dataset in `materialize` mode that inherits its
/// identity policy from organisation `org_id` (no dataset-level setting).
fn rematerialize_org_datasets(state: &AppState, org_id: &str) {
    let rows = (|| -> anyhow::Result<Vec<(String, String)>> {
        let conn = state.auth_db.pool().get()?;
        let mut stmt = conn.prepare(
            "SELECT dataset_id, regime FROM dataset_entailment WHERE mode = 'materialize'",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })()
    .unwrap_or_default();
    for (ds_id, regime) in rows {
        let Ok(Some(ds)) = state.auth_db.get_dataset(&ds_id) else {
            continue;
        };
        if !matches!(ds.owner_type, OwnerType::Organisation) || ds.owner_id != org_id {
            continue;
        }
        if matches!(
            identity_setting(&state.auth_db, "dataset", &ds_id),
            Ok(Some(_))
        ) {
            continue;
        }
        if let Err(e) = run_for_dataset(state, &ds_id, &regime) {
            tracing::warn!(
                "identity policy: re-materialising {ds_id} for organisation {org_id} failed: {e}"
            );
        }
    }
}

/// GET /api/datasets/:id/identity — the policy in force, its source, the
/// dataset's own setting (if any) and the options with their descriptions.
pub async fn get_dataset_identity(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = visible(&state, Some(&user.user_id), &dataset_id)?;
    let effective = effective_identity(&state.auth_db, &ds);
    let setting = identity_setting(&state.auth_db, "dataset", &dataset_id).map_err(e500)?;
    Ok(Json(identity_json(
        "dataset",
        &dataset_id,
        &effective,
        setting,
    )))
}

fn require_dataset_write(
    state: &AppState,
    user: &AuthenticatedUser,
    ds: &Dataset,
) -> Result<(), ApiErr> {
    if !state
        .auth_db
        .can_write_dataset(&user.user_id, ds)
        .map_err(e500)?
    {
        return Err((StatusCode::FORBIDDEN, "Write access required".to_string()));
    }
    Ok(())
}

/// PUT /api/datasets/:id/identity — set the dataset's own policy; the dataset
/// is re-materialised at once when it is in `materialize` mode.
pub async fn put_dataset_identity(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
    Json(body): Json<IdentityBody>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = visible(&state, Some(&user.user_id), &dataset_id)?;
    require_dataset_write(&state, &user, &ds)?;
    let policy = parse_policy(&body.policy)?;
    set_identity_setting(&state.auth_db, "dataset", &dataset_id, policy).map_err(e500)?;
    let st = state.clone();
    let id = dataset_id.clone();
    tokio::task::spawn_blocking(move || rematerialize_if_configured(&st, &id))
        .await
        .map_err(e500)?;
    let effective = effective_identity(&state.auth_db, &ds);
    Ok(Json(identity_json(
        "dataset",
        &dataset_id,
        &effective,
        Some(policy),
    )))
}

/// DELETE /api/datasets/:id/identity — drop the dataset's own setting (back
/// to the organisation's or the default).
pub async fn delete_dataset_identity(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(dataset_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    let ds = visible(&state, Some(&user.user_id), &dataset_id)?;
    require_dataset_write(&state, &user, &ds)?;
    clear_identity_setting(&state.auth_db, "dataset", &dataset_id).map_err(e500)?;
    let st = state.clone();
    let id = dataset_id.clone();
    tokio::task::spawn_blocking(move || rematerialize_if_configured(&st, &id))
        .await
        .map_err(e500)?;
    let effective = effective_identity(&state.auth_db, &ds);
    Ok(Json(identity_json(
        "dataset",
        &dataset_id,
        &effective,
        None,
    )))
}

/// Organisation admins (and system admins) manage the organisation's policy;
/// members may read it.
fn org_access(
    state: &AppState,
    user: &AuthenticatedUser,
    org_id: &str,
    manage: bool,
) -> Result<(), ApiErr> {
    let org = state.auth_db.get_organisation(org_id).map_err(e500)?;
    if org.is_none() {
        return Err((StatusCode::NOT_FOUND, "Organisation not found".to_string()));
    }
    if user.is_admin() {
        return Ok(());
    }
    let role = state
        .auth_db
        .get_org_membership(&user.user_id, org_id)
        .map_err(e500)?;
    match (manage, role) {
        (false, Some(_)) => Ok(()),
        (true, Some(Role::Admin)) => Ok(()),
        (false, None) => Err((StatusCode::NOT_FOUND, "Organisation not found".to_string())),
        (true, _) => Err((
            StatusCode::FORBIDDEN,
            "Organisation admin role required".to_string(),
        )),
    }
}

fn org_effective(
    db: &AuthDb,
    org_id: &str,
) -> anyhow::Result<(EffectiveIdentity, Option<IdentityPolicy>)> {
    let setting = identity_setting(db, "organisation", org_id)?;
    Ok((
        match setting {
            Some(policy) => EffectiveIdentity {
                policy,
                source: "organisation",
            },
            None => EffectiveIdentity {
                policy: IdentityPolicy::default(),
                source: "default",
            },
        },
        setting,
    ))
}

/// GET /api/organisations/:id/identity
pub async fn get_org_identity(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    org_access(&state, &user, &org_id, false)?;
    let (effective, setting) = org_effective(&state.auth_db, &org_id).map_err(e500)?;
    Ok(Json(identity_json(
        "organisation",
        &org_id,
        &effective,
        setting,
    )))
}

/// PUT /api/organisations/:id/identity — set the policy every dataset the
/// organisation owns inherits (unless the dataset has its own); inheriting
/// datasets in `materialize` mode are re-materialised.
pub async fn put_org_identity(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(org_id): Path<String>,
    Json(body): Json<IdentityBody>,
) -> Result<impl IntoResponse, ApiErr> {
    org_access(&state, &user, &org_id, true)?;
    let policy = parse_policy(&body.policy)?;
    set_identity_setting(&state.auth_db, "organisation", &org_id, policy).map_err(e500)?;
    let st = state.clone();
    let id = org_id.clone();
    tokio::task::spawn_blocking(move || rematerialize_org_datasets(&st, &id))
        .await
        .map_err(e500)?;
    let (effective, setting) = org_effective(&state.auth_db, &org_id).map_err(e500)?;
    Ok(Json(identity_json(
        "organisation",
        &org_id,
        &effective,
        setting,
    )))
}

/// DELETE /api/organisations/:id/identity — drop the organisation's setting.
pub async fn delete_org_identity(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, ApiErr> {
    org_access(&state, &user, &org_id, true)?;
    clear_identity_setting(&state.auth_db, "organisation", &org_id).map_err(e500)?;
    let st = state.clone();
    let id = org_id.clone();
    tokio::task::spawn_blocking(move || rematerialize_org_datasets(&st, &id))
        .await
        .map_err(e500)?;
    let (effective, setting) = org_effective(&state.auth_db, &org_id).map_err(e500)?;
    Ok(Json(identity_json(
        "organisation",
        &org_id,
        &effective,
        setting,
    )))
}
