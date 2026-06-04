//! Natural-language → SPARQL via any OpenAI-compatible LLM endpoint ("lookup triples with LLM").
//!
//! This endpoint only *generates* a query. The client then runs the returned SPARQL through the
//! normal dataset SPARQL endpoint, so it passes the exact same `scope_query_to_authorized`
//! boundary as any user-typed query — the model never reads data directly and cannot widen what
//! the caller is authorized to see. Keeping generation and (scoped) execution separate is the
//! security-critical design choice here.
//!
//! **Bring your own LLM.** Point `LLM_GATEWAY_URL` at any server that speaks the OpenAI
//! `/v1/chat/completions` API — OpenAI, OpenRouter, Azure OpenAI, Ollama, LM Studio, vLLM,
//! llama.cpp, or a self-hosted gateway. Choose the model with `LLM_MODEL`, and set
//! `LLM_API_KEY` if the endpoint requires a bearer token. Nothing here is tied to a specific
//! provider or model. When no endpoint is reachable the UI hides the AI features.

use std::collections::HashSet;
use std::time::Duration;

use axum::{extract::State, routing::{get, post}, Extension, Json, Router};
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::middleware::AuthenticatedUser;
use crate::saved_queries::store::SavedQueryStore;

use super::error::AppError;
use super::routes::{resolve_prefixes, scope_query_to_authorized};
use super::AppState;

const SYSTEM_PROMPT: &str = "You are a SPARQL generation assistant. Translate the natural-language \
question into a single valid SPARQL query using the provided ontology prefixes. Reply with ONLY \
the SPARQL query.";

/// Base URL of the OpenAI-compatible LLM endpoint (`LLM_GATEWAY_URL`). Defaults to a
/// local server on :8000; if nothing runs there, the AI features show as unavailable.
pub(crate) fn gateway_base() -> String {
    std::env::var("LLM_GATEWAY_URL").unwrap_or_else(|_| "http://127.0.0.1:8000".to_string())
}

/// Model name sent on every completion. Configure with `LLM_MODEL` (an OpenAI model
/// id, an Ollama tag, a vLLM-served name, …). The per-task overrides `LLM_SPARQL_MODEL`
/// and `LLM_SHACL_MODEL` fall back to this.
pub(crate) fn default_model() -> String {
    env_nonempty("LLM_MODEL").unwrap_or_else(|| "default".to_string())
}

/// Model for NL→SPARQL generation and saved-query repair.
pub(crate) fn sparql_model() -> String {
    env_nonempty("LLM_SPARQL_MODEL").unwrap_or_else(default_model)
}

/// Model for the SHACL Studio assistant.
fn shacl_model() -> String {
    env_nonempty("LLM_SHACL_MODEL").unwrap_or_else(default_model)
}

/// Optional bearer token for the endpoint (`LLM_API_KEY`). Required by hosted APIs
/// (OpenAI, OpenRouter, …); leave unset for local servers (Ollama, LM Studio).
fn api_key() -> Option<String> {
    env_nonempty("LLM_API_KEY")
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Send a single-turn chat completion to the gateway and return the assistant's
/// reply with any markdown code fence stripped. Shared by NL→SPARQL generation
/// and saved-query repair so both speak to the gateway identically.
pub(crate) async fn chat_completion(
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Result<String, AppError> {
    let payload = json!({
        "model": model,
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    });
    let url = format!("{}/v1/chat/completions", gateway_base().trim_end_matches('/'));
    let mut rb = reqwest::Client::new().post(&url).json(&payload);
    if let Some(key) = api_key() {
        rb = rb.bearer_auth(key);
    }
    let resp = rb
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("LLM endpoint unreachable at {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!("LLM endpoint returned {}", resp.status())));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("invalid LLM response: {e}")))?;
    let content = body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim();
    Ok(strip_code_fence(content))
}

/// Send a full multi-turn conversation to the gateway and return the assistant's
/// raw reply (trimmed, no code-fence stripping — the chat answer is prose, and any
/// embedded SPARQL is extracted/sanitised separately). Used by the chat endpoint.
pub(crate) async fn chat_completion_messages(
    model: &str,
    messages: Vec<Value>,
    max_tokens: u32,
) -> Result<String, AppError> {
    let payload = json!({
        "model": model,
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "messages": messages,
    });
    let url = format!("{}/v1/chat/completions", gateway_base().trim_end_matches('/'));
    let mut rb = reqwest::Client::new()
        .post(&url)
        .json(&payload)
        .timeout(Duration::from_secs(60));
    if let Some(key) = api_key() {
        rb = rb.bearer_auth(key);
    }
    let resp = rb
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("LLM endpoint unreachable at {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!("LLM endpoint returned {}", resp.status())));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("invalid LLM response: {e}")))?;
    Ok(body["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string())
}

pub fn llm_routes() -> Router<AppState> {
    Router::new()
        .route("/api/llm/sparql", post(nl_to_sparql))
        .route("/api/llm/chat", post(llm_chat))
        .route("/api/llm/feedback", post(forward_feedback))
        .route("/api/llm/health", get(llm_health))
        .route("/api/llm/shacl", post(shacl_assist))
}

/// Request body for `/api/llm/shacl` — the SHACL Studio AI assistant.
#[derive(Deserialize)]
pub struct ShaclAssistRequest {
    /// "draft" — generate Turtle shapes from a natural-language description.
    /// "explain" — describe an existing shape graph in plain language.
    /// "improve" — suggest refinements to an existing shape graph.
    pub task: String,
    /// User's natural-language description (for draft / improve).
    #[serde(default)]
    pub description: Option<String>,
    /// Existing shapes Turtle (for explain / improve).
    #[serde(default)]
    pub turtle: Option<String>,
    /// Optional model context (classes + properties) so the assistant uses real
    /// IRIs from the user's data, not `ex:someProperty` placeholders.
    #[serde(default)]
    pub model_context: Option<Value>,
    /// Optional model override (defaults to the configured model — see `LLM_SHACL_MODEL` / `LLM_MODEL`).
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct ShaclAssistResponse {
    pub model: String,
    pub task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turtle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

const SHACL_DRAFT_SYSTEM: &str = "You are a SHACL shapes author. Generate ONLY valid SHACL Turtle — no prose, no markdown fences. Use the `sh:` prefix and standard prefixes (rdf, rdfs, xsd). Prefer `sh:targetClass` to bind shapes to a class. Each property shape MUST include `sh:path`, a sensible cardinality (`sh:minCount` / `sh:maxCount`) when implied, and a human `sh:message`. When a model context is provided, reuse its real class and property IRIs exactly — do NOT invent vocabulary.";

const SHACL_EXPLAIN_SYSTEM: &str = "You are a SHACL expert. Given a shapes Turtle document, explain in clear, non-technical prose what each shape validates, what would fail, and why. Use short bullet points per shape. Do not output Turtle.";

const SHACL_IMPROVE_SYSTEM: &str = "You are a SHACL expert reviewing a shapes Turtle document. Suggest concrete, prioritised improvements: missing constraints, missing `sh:message`, over- or under-constrained cardinality, missing `sh:datatype` / `sh:class`, severity that doesn't match the rule's intent, redundancy, and naming conventions. Output a short markdown list. Do not rewrite the Turtle.";

/// POST /api/llm/shacl — SHACL Studio's AI assistant.
async fn shacl_assist(
    Json(req): Json<ShaclAssistRequest>,
) -> Result<Json<ShaclAssistResponse>, AppError> {
    let task = req.task.trim().to_lowercase();
    let model = req.model.clone().unwrap_or_else(shacl_model);

    let context_block = req
        .model_context
        .as_ref()
        .map(|c| format!("\n\n# MODEL CONTEXT (real classes + properties in scope)\n{}", c))
        .unwrap_or_default();

    let (system, user_msg, want_turtle) = match task.as_str() {
        "draft" => {
            let desc = req
                .description
                .as_deref()
                .ok_or_else(|| AppError::BadRequest("description is required for task=draft".into()))?;
            (SHACL_DRAFT_SYSTEM, format!("Draft SHACL Turtle for this requirement:\n\n{desc}{context_block}"), true)
        }
        "explain" => {
            let ttl = req
                .turtle
                .as_deref()
                .ok_or_else(|| AppError::BadRequest("turtle is required for task=explain".into()))?;
            (SHACL_EXPLAIN_SYSTEM, format!("Explain these SHACL shapes:\n\n```turtle\n{ttl}\n```"), false)
        }
        "improve" => {
            let ttl = req
                .turtle
                .as_deref()
                .ok_or_else(|| AppError::BadRequest("turtle is required for task=improve".into()))?;
            let desc = req.description.as_deref().unwrap_or("");
            (SHACL_IMPROVE_SYSTEM, format!("Review these SHACL shapes and suggest improvements.{}\n\n```turtle\n{ttl}\n```{context_block}",
                if desc.is_empty() { String::new() } else { format!(" Focus on: {desc}") }), false)
        }
        _ => return Err(AppError::BadRequest("task must be one of: draft, explain, improve".into())),
    };

    let answer = chat_completion(&model, system, &user_msg, 1200).await?;
    Ok(Json(if want_turtle {
        ShaclAssistResponse { model, task, turtle: Some(answer), explanation: None }
    } else {
        ShaclAssistResponse { model, task, turtle: None, explanation: Some(answer) }
    }))
}

#[derive(Serialize)]
pub struct LlmHealth {
    /// The LLM endpoint this instance is configured to use (`LLM_GATEWAY_URL`).
    gateway: String,
    /// Whether that endpoint answered within the timeout.
    reachable: bool,
    /// The endpoint's payload when reachable (e.g. the `/v1/models` list, or a
    /// gateway's own `/health` detail).
    detail: Option<Value>,
}

/// GET /api/llm/health — is an LLM endpoint reachable from this server?
/// Lets the UI show AI availability alongside its other service health. Probes the
/// OpenAI-standard `/v1/models` first (works for OpenAI, Ollama, LM Studio, vLLM, …),
/// then falls back to a gateway `/health` for servers that expose one.
async fn llm_health(State(_state): State<AppState>) -> Json<LlmHealth> {
    let gateway = gateway_base();
    let base = gateway.trim_end_matches('/');
    let client = reqwest::Client::new();
    for path in ["/v1/models", "/health"] {
        let mut rb = client.get(format!("{base}{path}")).timeout(Duration::from_secs(3));
        if let Some(key) = api_key() {
            rb = rb.bearer_auth(key);
        }
        if let Ok(resp) = rb.send().await {
            if resp.status().is_success() {
                let detail = resp.json::<Value>().await.ok();
                return Json(LlmHealth { gateway, reachable: true, detail });
            }
        }
    }
    Json(LlmHealth { gateway, reachable: false, detail: None })
}

#[derive(Deserialize)]
pub struct NlSparqlRequest {
    pub question: String,
    /// Optional ontology / prefix context to ground the generation (classes, predicates, prefixes).
    #[serde(default)]
    pub schema_hint: Option<String>,
    /// Override the model; defaults to the configured model (see `LLM_SPARQL_MODEL` / `LLM_MODEL`).
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct NlSparqlResponse {
    pub sparql: String,
    pub model: String,
}

/// POST /api/llm/sparql  { question, schema_hint?, model? } -> { sparql, model }
async fn nl_to_sparql(
    State(_state): State<AppState>,
    Json(req): Json<NlSparqlRequest>,
) -> Result<Json<NlSparqlResponse>, AppError> {
    if req.question.trim().is_empty() {
        return Err(AppError::BadRequest("question is required".to_string()));
    }
    let model = req.model.unwrap_or_else(sparql_model);

    let user_content = match req.schema_hint.as_deref() {
        Some(h) if !h.trim().is_empty() => {
            format!("{}\n\nOntology / prefixes:\n{}", req.question.trim(), h.trim())
        }
        _ => req.question.trim().to_string(),
    };

    let sparql = chat_completion(&model, SYSTEM_PROMPT, &user_content, 512).await?;
    Ok(Json(NlSparqlResponse { sparql, model }))
}

/// POST /api/llm/feedback  <TrainingExample> -> endpoint `/v1/signals`
///
/// Optional training-signal feedback loop: forwards accept/edit/reject signals to an
/// endpoint that implements `/v1/signals` (e.g. a fine-tuning pipeline). Endpoints
/// without that route simply reject it and the UI ignores the result — the core AI
/// features work regardless. Proxied so the browser only talks to its own origin.
async fn forward_feedback(
    State(_state): State<AppState>,
    Json(signal): Json<Value>,
) -> Result<Json<Value>, AppError> {
    let url = format!("{}/v1/signals", gateway_base().trim_end_matches('/'));
    let mut rb = reqwest::Client::new().post(&url).json(&signal);
    if let Some(key) = api_key() {
        rb = rb.bearer_auth(key);
    }
    let resp = rb
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("LLM endpoint unreachable at {url}: {e}")))?;
    let ok = resp.status().is_success();
    let body: Value = resp.json().await.unwrap_or_else(|_| json!({"accepted": ok}));
    Ok(Json(body))
}

// ─── Knowledge-graph chat ──────────────────────────────────────────────────────
//
// A grounded assistant for the platform. Each turn we hand the model a snapshot of
// what *this caller* may see — the datasets they can access (with DCAT topics), the
// API services runnable against them, and the named graphs in scope — so questions
// like "how many datasets about X are there?" or "is there an API service for this?"
// are answered from real platform state, not hallucinated. For questions that need
// the actual triples, the model emits a single `SPARQL:` line; we run it through the
// exact same `scope_query_to_authorized` read boundary as any user query (it can
// never read a graph the caller is not authorized to see), then feed the rows back
// for a natural-language answer. At most one query per turn keeps weak models robust.

const CHAT_SYSTEM_PROMPT: &str = "You are the Linked Data assistant for the Open Triplestore platform, \
a knowledge-graph database. Help the user explore linked data: answer questions about which datasets \
exist and what topics they cover, point them at API services that can answer a question, explain RDF/SPARQL \
concepts, and answer data questions about the knowledge graphs.\n\n\
Use the PLATFORM CONTEXT below as your source of truth about what exists on this platform. It lists only \
the datasets, API services and named graphs THIS user is allowed to see — never claim something exists \
that is not listed.\n\n\
If answering needs the actual contents of the graphs (counts, specific values, relationships), reply with \
EXACTLY one line: `SPARQL:` followed by a single valid SPARQL query against the listed named graphs, and \
nothing else. The system will run it and give you the results to summarise. Otherwise, answer directly and \
concisely in natural language (markdown allowed). When you mention an API service, give its run URL.";

/// Cap how much platform state we serialise into the prompt so a large instance
/// stays within the model's context window.
const MAX_DATASETS_IN_CONTEXT: usize = 60;
const MAX_SERVICES_IN_CONTEXT: usize = 40;
const MAX_GRAPHS_IN_CONTEXT: usize = 40;
/// Cap rows returned from a chat-issued SPARQL query (both to the model and the UI).
const MAX_CHAT_QUERY_ROWS: usize = 50;

#[derive(Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct ChatResponse {
    /// The assistant's natural-language answer (markdown).
    pub answer: String,
    pub model: String,
    /// True when a SPARQL query was generated and successfully run to answer.
    pub ran_query: bool,
    /// The SPARQL that was run (or attempted), when the model chose to query.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sparql: Option<String>,
    /// Tabular results of the query, for the UI to render alongside the answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<String>>>,
    /// True when the result set was capped at [`MAX_CHAT_QUERY_ROWS`].
    pub truncated: bool,
}

/// POST /api/llm/chat — grounded knowledge-graph chat.
async fn llm_chat(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, AppError> {
    let user = user.as_deref();
    if req.messages.is_empty() || req.messages.iter().all(|m| m.content.trim().is_empty()) {
        return Err(AppError::BadRequest("at least one message is required".to_string()));
    }
    let model = req.model.clone().unwrap_or_else(default_model);

    // The set of graphs this caller may read — the security scope for any query.
    let graphs = chat_accessible_graphs(&state, user)?;
    let user_id = user.map(|u| u.user_id.as_str());
    let context = build_platform_context(&state, user_id, &graphs);

    let mut msgs: Vec<Value> = Vec::with_capacity(req.messages.len() + 1);
    msgs.push(json!({
        "role": "system",
        "content": format!("{CHAT_SYSTEM_PROMPT}\n\n# PLATFORM CONTEXT\n{context}"),
    }));
    for m in &req.messages {
        let role = if m.role == "assistant" { "assistant" } else { "user" };
        msgs.push(json!({"role": role, "content": m.content}));
    }

    // Planning turn: answer directly, or emit `SPARQL:` to fetch data.
    let first = chat_completion_messages(&model, msgs.clone(), 700).await?;

    let Some(query) = extract_sparql_directive(&first) else {
        return Ok(Json(ChatResponse {
            answer: first,
            model,
            ran_query: false,
            sparql: None,
            columns: None,
            rows: None,
            truncated: false,
        }));
    };

    // The model wants data. Run the query under the caller's read scope, then ask
    // it to turn the rows into prose.
    match run_chat_query(&state, &query, &graphs).await {
        Ok(qr) => {
            let table = render_rows_for_llm(&qr);
            msgs.push(json!({"role": "assistant", "content": format!("SPARQL:\n{query}")}));
            msgs.push(json!({
                "role": "user",
                "content": format!(
                    "Query results:\n{table}\nUsing ONLY these results, answer my previous question \
                     in clear natural language. Do not output SPARQL."
                ),
            }));
            let answer = chat_completion_messages(&model, msgs, 700)
                .await
                .unwrap_or_else(|_| format!("Here are the results of the query:\n\n{table}"));
            Ok(Json(ChatResponse {
                answer,
                model,
                ran_query: true,
                sparql: Some(query),
                columns: Some(qr.columns),
                rows: Some(qr.rows),
                truncated: qr.truncated,
            }))
        }
        Err(e) => Ok(Json(ChatResponse {
            answer: format!(
                "I tried to answer by querying the knowledge graph, but the query did not run ({}). \
                 You can refine it in the SPARQL workspace:",
                e.message()
            ),
            model,
            ran_query: false,
            sparql: Some(query),
            columns: None,
            rows: None,
            truncated: false,
        })),
    }
}

/// The named graphs `user` may read — the same scope the normal SPARQL endpoint
/// applies. Mirrors `execute_query`: accessible-dataset graphs, plus named-graph
/// ACL grants, plus (for admins) every registered graph.
fn chat_accessible_graphs(
    state: &AppState,
    user: Option<&AuthenticatedUser>,
) -> Result<HashSet<String>, AppError> {
    let user_id = user.map(|u| u.user_id.as_str());
    let cached = state
        .auth_db
        .get_accessible_graph_iris_cached(user_id)
        .map_err(|e| AppError::Internal(e.to_string()))?;
    let mut set = cached.0.clone();
    if user.map(|u| u.is_admin()).unwrap_or(false) {
        for iri in &cached.1 {
            set.insert(iri.clone());
        }
    }
    match user {
        Some(u) => {
            if let Ok(acl) = state.auth_db.get_graph_acl_readable_iris(&u.user_id, u.role.as_str()) {
                set.extend(acl);
            }
        }
        None => {
            if let Ok(acl) = state.auth_db.get_graph_acl_readable_iris("", "public") {
                set.extend(acl);
            }
        }
    }
    Ok(set)
}

/// Serialise the platform state visible to `user_id` into the prompt: accessible
/// datasets (name, visibility, description, DCAT topics), the API services runnable
/// against them, and the named graphs in scope.
fn build_platform_context(
    state: &AppState,
    user_id: Option<&str>,
    graphs: &HashSet<String>,
) -> String {
    let mut ctx = String::new();

    let datasets = state.auth_db.list_accessible_datasets(user_id).unwrap_or_default();
    ctx.push_str(&format!("## Datasets ({} accessible)\n", datasets.len()));
    for d in datasets.iter().take(MAX_DATASETS_IN_CONTEXT) {
        ctx.push_str(&format!("- \"{}\" (id {}, {:?})", d.name, d.id, d.visibility));
        if let Some(desc) = d.description.as_deref().filter(|s| !s.trim().is_empty()) {
            ctx.push_str(&format!(" — {}", truncate(desc, 160)));
        }
        let topics: Vec<&str> = [d.themes.as_deref(), d.keywords.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.trim().is_empty())
            .collect();
        if !topics.is_empty() {
            ctx.push_str(&format!(" [topics: {}]", truncate(&topics.join(", "), 120)));
        }
        ctx.push('\n');
    }
    if datasets.len() > MAX_DATASETS_IN_CONTEXT {
        ctx.push_str(&format!("- …and {} more.\n", datasets.len() - MAX_DATASETS_IN_CONTEXT));
    }

    // API services across the accessible datasets.
    let store = SavedQueryStore::new(state.auth_db.pool());
    let mut services: Vec<String> = Vec::new();
    for d in &datasets {
        if services.len() >= MAX_SERVICES_IN_CONTEXT {
            break;
        }
        let Ok(queries) = store.list_active_dataset_queries(&d.id) else { continue };
        for q in queries {
            if services.len() >= MAX_SERVICES_IN_CONTEXT {
                break;
            }
            let params = if q.parameters.is_empty() {
                String::new()
            } else {
                let names: Vec<&str> = q.parameters.iter().map(|p| p.name.as_str()).collect();
                format!(" — parameters: {}", names.join(", "))
            };
            let mut line = format!(
                "- \"{}\" on dataset \"{}\": GET /api/datasets/{}/api-services/{}/run",
                q.name, d.name, d.id, q.slug
            );
            if let Some(desc) = q.description.as_deref().filter(|s| !s.trim().is_empty()) {
                line.push_str(&format!(" — {}", truncate(desc, 140)));
            }
            line.push_str(&params);
            services.push(line);
        }
    }
    if services.is_empty() {
        ctx.push_str("\n## API Services\n(none accessible)\n");
    } else {
        ctx.push_str("\n## API Services (saved SPARQL queries runnable as HTTP APIs)\n");
        for s in &services {
            ctx.push_str(s);
            ctx.push('\n');
        }
    }

    if !graphs.is_empty() {
        ctx.push_str("\n## Named graphs in scope (use these IRIs in GRAPH/FROM clauses)\n");
        for g in graphs.iter().take(MAX_GRAPHS_IN_CONTEXT) {
            ctx.push_str(&format!("- <{g}>\n"));
        }
        if graphs.len() > MAX_GRAPHS_IN_CONTEXT {
            ctx.push_str(&format!("- …and {} more graphs.\n", graphs.len() - MAX_GRAPHS_IN_CONTEXT));
        }
    }

    ctx
}

/// Tabular result of a chat-issued query.
struct ChatQueryResult {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    truncated: bool,
}

/// Run a model-generated query under the caller's read scope and collect a capped
/// table. The query is re-scoped with [`scope_query_to_authorized`] (the read
/// boundary) exactly like a user-typed query, so it cannot read outside `graphs`.
async fn run_chat_query(
    state: &AppState,
    query: &str,
    graphs: &HashSet<String>,
) -> Result<ChatQueryResult, AppError> {
    let scoped = scope_query_to_authorized(query, graphs);
    let resolved = resolve_prefixes(state, &scoped).await;
    let final_query = resolved.unwrap_or(scoped);
    let store = state.store.clone();

    tokio::task::spawn_blocking(move || {
        let results = store
            .query(&final_query)
            .map_err(|e| AppError::BadRequest(format!("query failed: {e}")))?;
        match results {
            QueryResults::Boolean(b) => Ok(ChatQueryResult {
                columns: vec!["result".to_string()],
                rows: vec![vec![b.to_string()]],
                truncated: false,
            }),
            QueryResults::Solutions(solutions) => {
                let columns: Vec<String> =
                    solutions.variables().iter().map(|v| v.as_str().to_string()).collect();
                let mut rows = Vec::new();
                let mut truncated = false;
                for sol in solutions {
                    if rows.len() >= MAX_CHAT_QUERY_ROWS {
                        truncated = true;
                        break;
                    }
                    let sol = sol.map_err(|e| AppError::Internal(e.to_string()))?;
                    rows.push(
                        columns
                            .iter()
                            .map(|c| sol.get(c.as_str()).map(term_to_short).unwrap_or_default())
                            .collect(),
                    );
                }
                Ok(ChatQueryResult { columns, rows, truncated })
            }
            QueryResults::Graph(triples) => {
                let columns =
                    vec!["subject".to_string(), "predicate".to_string(), "object".to_string()];
                let mut rows = Vec::new();
                let mut truncated = false;
                for t in triples {
                    if rows.len() >= MAX_CHAT_QUERY_ROWS {
                        truncated = true;
                        break;
                    }
                    let t = t.map_err(|e| AppError::Internal(e.to_string()))?;
                    rows.push(vec![
                        t.subject.to_string(),
                        t.predicate.to_string(),
                        term_to_short(&t.object),
                    ]);
                }
                Ok(ChatQueryResult { columns, rows, truncated })
            }
        }
    })
    .await
    .map_err(|e| AppError::Internal(format!("query task panicked: {e}")))?
}

/// A short, human-readable rendering of an RDF term for tables and prompts.
fn term_to_short(term: &Term) -> String {
    match term {
        Term::NamedNode(n) => n.as_str().to_string(),
        Term::BlankNode(b) => format!("_:{}", b.as_str()),
        Term::Literal(l) => l.value().to_string(),
        other => other.to_string(),
    }
}

/// Render a query result as a compact pipe-delimited table for the follow-up prompt.
fn render_rows_for_llm(qr: &ChatQueryResult) -> String {
    let mut s = String::new();
    s.push_str(&qr.columns.join(" | "));
    s.push('\n');
    for row in &qr.rows {
        let cells: Vec<String> = row.iter().map(|c| truncate(c, 80)).collect();
        s.push_str(&cells.join(" | "));
        s.push('\n');
    }
    if qr.rows.is_empty() {
        s.push_str("(no rows)\n");
    } else if qr.truncated {
        s.push_str(&format!("(showing first {MAX_CHAT_QUERY_ROWS} rows)\n"));
    }
    s
}

/// If the model asked to run a query, return the query text. We look for a
/// `SPARQL:` marker (case-insensitive, byte-safe), strip any code fence, and only
/// accept it when it actually contains a query form — otherwise the reply is prose.
fn extract_sparql_directive(reply: &str) -> Option<String> {
    let pos = find_ci(reply, "SPARQL:")?;
    let after = reply[pos + "SPARQL:".len()..].trim();
    let query = strip_code_fence(after);
    let is_query = ["SELECT", "ASK", "CONSTRUCT", "DESCRIBE"]
        .iter()
        .any(|kw| find_ci(&query, kw).is_some());
    is_query.then_some(query)
}

/// Case-insensitive (ASCII) byte-index search — safe for slicing `haystack`,
/// unlike `to_uppercase().find()` which can shift indices for some Unicode.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let (hb, nb) = (haystack.as_bytes(), needle.as_bytes());
    if nb.is_empty() || hb.len() < nb.len() {
        return None;
    }
    (0..=hb.len() - nb.len()).find(|&i| hb[i..i + nb.len()].eq_ignore_ascii_case(nb))
}

/// Truncate to `max` chars (char-boundary safe), appending `…` when shortened.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Strip a leading ```/```sparql fence (and trailing ```), which small models often add.
fn strip_code_fence(s: &str) -> String {
    let t = s.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t.to_string();
    };
    let rest = rest.strip_prefix("sparql").unwrap_or(rest);
    let rest = rest.trim_start_matches('\n');
    match rest.rfind("```") {
        Some(end) => rest[..end].trim().to_string(),
        None => rest.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_sparql_directive, find_ci, strip_code_fence, truncate};

    #[test]
    fn extracts_sparql_directive_with_fence() {
        let q = extract_sparql_directive("SPARQL:\n```sparql\nSELECT * WHERE { ?s ?p ?o }\n```")
            .expect("should detect a query");
        assert_eq!(q, "SELECT * WHERE { ?s ?p ?o }");
    }

    #[test]
    fn extracts_directive_case_insensitively_inline() {
        let q = extract_sparql_directive("Sure, let me check. sparql: ASK { ?s ?p ?o }")
            .expect("marker is case-insensitive");
        assert_eq!(q, "ASK { ?s ?p ?o }");
    }

    #[test]
    fn prose_answer_is_not_treated_as_a_query() {
        assert_eq!(
            extract_sparql_directive("There are 3 datasets about water quality."),
            None
        );
    }

    #[test]
    fn find_ci_is_byte_safe() {
        assert_eq!(find_ci("aaSPARQL:", "sparql:"), Some(2));
        assert_eq!(find_ci("no marker", "sparql:"), None);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hello…");
        // Multi-byte chars must not be split mid-byte.
        assert_eq!(truncate("héllo wörld", 4), "héll…");
    }

    #[test]
    fn strips_sparql_fence() {
        assert_eq!(
            strip_code_fence("```sparql\nSELECT * WHERE { ?s ?p ?o }\n```"),
            "SELECT * WHERE { ?s ?p ?o }"
        );
    }

    #[test]
    fn passes_through_plain_query() {
        assert_eq!(strip_code_fence("SELECT ?x WHERE {}"), "SELECT ?x WHERE {}");
    }

    #[test]
    fn strips_bare_fence_without_lang() {
        assert_eq!(strip_code_fence("```\nASK {}\n```"), "ASK {}");
    }
}
