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
use std::sync::OnceLock;
use std::time::Duration;

use axum::{
    extract::State,
    routing::{get, post},
    Extension, Json, Router,
};
use oxigraph::model::Term;
use oxigraph::sparql::QueryResults;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::middleware::AuthenticatedUser;
use crate::saved_queries::store::SavedQueryStore;

use super::error::AppError;
use super::routes::{resolve_prefixes, scope_query_to_authorized};
use super::AppState;

const SYSTEM_PROMPT: &str =
    "You are a SPARQL generation assistant. Translate the natural-language question into a single, \
complete, valid SPARQL query.\n\
- Declare EVERY prefix you use with a `PREFIX` line at the top of the query (for example \
`PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>`). Never use a prefix you have not declared.\n\
- Prefer the prefixes and vocabulary the user provides.\n\
- If the user gives a \"Current query\", EDIT and extend that query to satisfy the request, keeping \
the parts that are still correct, instead of starting from scratch.\n\
- Reply with ONLY the SPARQL query — no explanation and no markdown code fences.";

/// Output-token budget for a generated SPARQL query (prefix block + body). Large
/// enough that a query with several PREFIX lines is never cut off mid-statement —
/// a truncated query is invalid and would only force the repair round-trip.
const SPARQL_MAX_TOKENS: u32 = 1024;

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
pub(crate) fn api_key() -> Option<String> {
    env_nonempty("LLM_API_KEY")
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Shared HTTP client for every call to the LLM gateway. Connection pooling +
/// keep-alive means repeat calls (every chat round, health probe, feedback post)
/// reuse an established TCP/TLS connection instead of paying a fresh handshake —
/// for a TLS gateway that alone shaves ~100–300 ms off time-to-first-token, and a
/// multi-round chat turn pays it once instead of per round.
pub(crate) fn llm_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .tcp_keepalive(Duration::from_secs(60))
            .build()
            .expect("default reqwest client must build")
    })
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
    let url = format!(
        "{}/v1/chat/completions",
        gateway_base().trim_end_matches('/')
    );
    let mut rb = llm_client().post(&url).json(&payload);
    if let Some(key) = api_key() {
        rb = rb.bearer_auth(key);
    }
    let resp = rb
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("LLM endpoint unreachable at {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "LLM endpoint returned {}",
            resp.status()
        )));
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
    let url = format!(
        "{}/v1/chat/completions",
        gateway_base().trim_end_matches('/')
    );
    let mut rb = llm_client()
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
        return Err(AppError::Internal(format!(
            "LLM endpoint returned {}",
            resp.status()
        )));
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
        .route(
            "/api/llm/chat/stream",
            post(super::llm_stream::llm_chat_stream),
        )
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
        .map(|c| {
            format!(
                "\n\n# MODEL CONTEXT (real classes + properties in scope)\n{}",
                c
            )
        })
        .unwrap_or_default();

    let (system, user_msg, want_turtle) = match task.as_str() {
        "draft" => {
            let desc = req.description.as_deref().ok_or_else(|| {
                AppError::BadRequest("description is required for task=draft".into())
            })?;
            (
                SHACL_DRAFT_SYSTEM,
                format!("Draft SHACL Turtle for this requirement:\n\n{desc}{context_block}"),
                true,
            )
        }
        "explain" => {
            let ttl = req.turtle.as_deref().ok_or_else(|| {
                AppError::BadRequest("turtle is required for task=explain".into())
            })?;
            (
                SHACL_EXPLAIN_SYSTEM,
                format!("Explain these SHACL shapes:\n\n```turtle\n{ttl}\n```"),
                false,
            )
        }
        "improve" => {
            let ttl = req.turtle.as_deref().ok_or_else(|| {
                AppError::BadRequest("turtle is required for task=improve".into())
            })?;
            let desc = req.description.as_deref().unwrap_or("");
            (SHACL_IMPROVE_SYSTEM, format!("Review these SHACL shapes and suggest improvements.{}\n\n```turtle\n{ttl}\n```{context_block}",
                if desc.is_empty() { String::new() } else { format!(" Focus on: {desc}") }), false)
        }
        _ => {
            return Err(AppError::BadRequest(
                "task must be one of: draft, explain, improve".into(),
            ))
        }
    };

    let answer = chat_completion(&model, system, &user_msg, 1200).await?;
    Ok(Json(if want_turtle {
        ShaclAssistResponse {
            model,
            task,
            turtle: Some(answer),
            explanation: None,
        }
    } else {
        ShaclAssistResponse {
            model,
            task,
            turtle: None,
            explanation: Some(answer),
        }
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
    let client = llm_client();
    for path in ["/v1/models", "/health"] {
        let mut rb = client
            .get(format!("{base}{path}"))
            .timeout(Duration::from_secs(3));
        if let Some(key) = api_key() {
            rb = rb.bearer_auth(key);
        }
        if let Ok(resp) = rb.send().await {
            if resp.status().is_success() {
                let detail = resp.json::<Value>().await.ok();
                return Json(LlmHealth {
                    gateway,
                    reachable: true,
                    detail,
                });
            }
        }
    }
    Json(LlmHealth {
        gateway,
        reachable: false,
        detail: None,
    })
}

#[derive(Deserialize)]
pub struct NlSparqlRequest {
    pub question: String,
    /// Optional ontology / prefix context to ground the generation (classes, predicates, prefixes).
    #[serde(default)]
    pub schema_hint: Option<String>,
    /// The query currently in the editor. When present the model edits/extends it in
    /// place (a refinement) rather than always generating a brand-new query.
    #[serde(default)]
    pub current_query: Option<String>,
    /// Override the model; defaults to the configured model (see `LLM_SPARQL_MODEL` / `LLM_MODEL`).
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct NlSparqlResponse {
    pub sparql: String,
    pub model: String,
}

/// POST /api/llm/sparql  { question, schema_hint?, current_query?, model? } -> { sparql, model }
async fn nl_to_sparql(
    State(state): State<AppState>,
    Json(req): Json<NlSparqlRequest>,
) -> Result<Json<NlSparqlResponse>, AppError> {
    if req.question.trim().is_empty() {
        return Err(AppError::BadRequest("question is required".to_string()));
    }
    let model = req.model.clone().unwrap_or_else(sparql_model);
    let user_content = build_sparql_prompt(&req);

    // Generate, then make the query actually runnable: inject any prefixes the model
    // forgot to declare (resolved from the prefix registry), then verify it parses.
    // If it doesn't, give the model ONE chance to repair its own output before we
    // hand it back — so the editor receives a checked, complete query, not a fragment.
    let raw = chat_completion(&model, SYSTEM_PROMPT, &user_content, SPARQL_MAX_TOKENS).await?;
    let mut sparql = finalize_sparql(&state, raw).await;

    if let Err(err) = validate_sparql(&sparql) {
        let repair = format!(
            "This SPARQL query is not valid ({err}):\n\n{sparql}\n\n\
             Return a corrected, complete query. Declare every PREFIX you use. Reply with ONLY the SPARQL.",
        );
        if let Ok(fixed) = chat_completion(&model, SYSTEM_PROMPT, &repair, SPARQL_MAX_TOKENS).await
        {
            let fixed = finalize_sparql(&state, fixed).await;
            // Keep the repair only if it now parses; otherwise return the first attempt
            // so the user still has something concrete to edit.
            if validate_sparql(&fixed).is_ok() {
                sparql = fixed;
            }
        }
    }

    Ok(Json(NlSparqlResponse { sparql, model }))
}

/// Assemble the NL→SPARQL user prompt from the question plus any ontology hint and
/// the query currently in the editor (so the model can refine it in place).
fn build_sparql_prompt(req: &NlSparqlRequest) -> String {
    let mut s = req.question.trim().to_string();
    if let Some(h) = req
        .schema_hint
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
    {
        s.push_str("\n\nOntology / prefixes:\n");
        s.push_str(h);
    }
    if let Some(q) = req
        .current_query
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        s.push_str(
            "\n\nCurrent query (edit this if the request refines it, otherwise replace it):\n",
        );
        s.push_str(q);
    }
    s
}

/// Inject any prefixes used-but-not-declared (resolved from the prefix registry /
/// prefix.cc), so a `PREFIX` line the model forgot never makes the query fail. A
/// no-op when every prefix is already declared.
async fn finalize_sparql(state: &AppState, sparql: String) -> String {
    let sparql = sparql.trim().to_string();
    // Bind the result so the `&sparql` borrow ends before `sparql` is moved into
    // unwrap_or (and so clippy sees the idiomatic unwrap_or, not a manual match).
    let resolved = resolve_prefixes(state, &sparql).await;
    resolved.unwrap_or(sparql)
}

/// Parse-check a query string with the same grammar the engine uses, returning the
/// parser's message on failure. Undeclared prefixes fail here — which is exactly why
/// [`finalize_sparql`] runs first.
fn validate_sparql(sparql: &str) -> Result<(), String> {
    spargebra::Query::parse(sparql, None)
        .map(|_| ())
        .map_err(|e| e.to_string())
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
    let mut rb = llm_client().post(&url).json(&signal);
    if let Some(key) = api_key() {
        rb = rb.bearer_auth(key);
    }
    let resp = rb
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("LLM endpoint unreachable at {url}: {e}")))?;
    let ok = resp.status().is_success();
    let body: Value = resp
        .json()
        .await
        .unwrap_or_else(|_| json!({"accepted": ok}));
    Ok(Json(body))
}

// ─── Knowledge-graph chat ──────────────────────────────────────────────────────
//
// A grounded assistant for the platform. Each turn we hand the model a snapshot of
// what *this caller* may see — the datasets they can access (with DCAT topics), the
// API services runnable against them, and the named graphs in scope — so questions
// like "how many datasets about X are there?" or "is there an API service for this?"
// are answered from real platform state, not hallucinated. For questions that need
// the actual triples, the model emits a `SPARQL:` line; we run it through the exact
// same `scope_query_to_authorized` read boundary as any user query (it can never
// read a graph the caller is not authorized to see), then feed the rows back. The
// model may iterate (a few bounded rounds, with error feedback for self-repair)
// before writing the final answer. Answers are markdown plus a small set of fenced
// "widget" blocks (chart/map/card/api/csv) that the chat UI renders interactively.

pub(crate) const CHAT_SYSTEM_PROMPT: &str = "You are Spark, the linked-data expert of the Open Triplestore platform, \
a knowledge-graph database. Help the user explore and understand linked data: which datasets exist and what \
they cover, which API services can answer a question, what the graphs actually contain, and how RDF, SPARQL, \
named graphs, vocabularies and SHACL work. Be precise with linked-data terminology, prefer labels over bare \
IRIs in prose, and say briefly how you obtained an answer (which graph or service it came from).\n\n\
Use the PLATFORM CONTEXT below as your source of truth about what exists on this platform. It lists only \
the datasets, API services and named graphs THIS user is allowed to see — never claim something exists \
that is not listed.\n\n\
# RETRIEVING DATA\n\
If answering needs the actual contents of the graphs (counts, specific values, relationships, geometries), \
reply with EXACTLY one line: `SPARQL:` followed by a single valid SPARQL query against the listed named \
graphs, and nothing else. The system runs it read-only under the user's permissions and gives you the \
result rows; you may then reply with another `SPARQL:` line if you still need different data, otherwise \
write the final answer. Result cells may be truncated (they then end with …).\n\n\
# PRESENTING DATA\n\
Final answers are markdown, and these fenced blocks render as live interactive widgets — use them whenever \
they make the answer clearer:\n\
- ```sparql — a query card with a Run button the user can execute themselves and open in the SPARQL \
workspace. Use it whenever you show a query.\n\
- ```api — a runnable API call; first line is `GET <path>`, for example:\n\
```api\nGET /api/datasets/<dataset-id>/api-services/<slug>/run?param=value\n```\n\
Use one whenever you mention an API service (inline code like `GET /api/...` becomes clickable too).\n\
- ```chart — a JSON spec rendered as a chart: \
{\"type\":\"bar\",\"title\":\"…\",\"yLabel\":\"…\",\"data\":[{\"label\":\"A\",\"value\":12.5}]} with type bar, line or pie; \
multi-series: {\"type\":\"line\",\"series\":[{\"name\":\"2024\",\"data\":[{\"label\":\"Jan\",\"value\":3}]}]}. \
Only chart numbers you actually retrieved — never invent values. Keep it under 40 points.\n\
- ```map — a JSON spec rendered as an interactive map: \
{\"features\":[{\"label\":\"Waalbrug\",\"wkt\":\"POINT(5.8645 51.8519)\",\"iri\":\"http://…\"}]}. \
WKT must be WGS84 with longitude before latitude. Prefer points or centroids; skip geometries whose WKT \
was truncated.\n\
- ```card — an entity info card: {\"title\":\"…\",\"subtitle\":\"…\",\"iri\":\"http://…\",\"image\":\"https://…\",\
\"facts\":[{\"label\":\"Type\",\"value\":\"Bridge\"}]}. Ideal for \"tell me about X\" answers.\n\
- ```csv — CSV text rendered as a table with a download button.\n\
- ```turtle / ```json / ```xml — syntax-highlighted data snippets (not runnable). Small markdown tables \
also render well.\n\n\
Pick at most a couple of widgets per answer, chosen for the question: trends or comparisons → chart, \
locations → map, a single entity → card, raw listings → markdown table or csv, \"how do I get this \
myself\" → sparql or api block. Answer in the user's language.";

/// Cap how much platform state we serialise into the prompt so a large instance
/// stays within the model's context window.
const MAX_DATASETS_IN_CONTEXT: usize = 60;
const MAX_SERVICES_IN_CONTEXT: usize = 40;
const MAX_GRAPHS_IN_CONTEXT: usize = 40;
/// Cap rows returned from a chat-issued SPARQL query (both to the model and the UI).
const MAX_CHAT_QUERY_ROWS: usize = 50;
/// How many `SPARQL:` rounds the model may use within one user turn. Feeding rows
/// (or the error, for self-repair) back after each round lets it e.g. count first
/// and then fetch geometry for a map, while keeping latency and tokens bounded.
pub(crate) const MAX_CHAT_QUERY_ROUNDS: usize = 3;
/// Per-cell character budgets when rendering result rows into the follow-up prompt.
/// WKT geometry cells get a larger budget so small geometries survive verbatim into
/// a ```map widget; anything longer is truncated with '…' and the system prompt
/// tells the model to skip truncated WKT.
const CHAT_CELL_MAX_CHARS: usize = 80;
const CHAT_WKT_CELL_MAX_CHARS: usize = 600;
/// Output-token budget per chat turn. Rich answers (markdown + widget JSON specs)
/// need headroom; short answers still stop early.
pub(crate) const CHAT_MAX_TOKENS: u32 = 3072;

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

/// One SPARQL round the chat ran (or attempted) while answering a turn.
#[derive(Serialize)]
pub struct ChatQueryRun {
    pub sparql: String,
    /// False when the query failed to run; `error` then says why.
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<String>>>,
    /// True when the result set was capped at [`MAX_CHAT_QUERY_ROWS`].
    pub truncated: bool,
}

#[derive(Serialize)]
pub struct ChatResponse {
    /// The assistant's natural-language answer (markdown + widget blocks).
    pub answer: String,
    pub model: String,
    /// True when at least one SPARQL query was generated and successfully run.
    pub ran_query: bool,
    /// The SPARQL that was run (or attempted), when the model chose to query.
    /// Mirrors the last successful round (or the last attempt) for older clients;
    /// `queries` carries the full trail.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sparql: Option<String>,
    /// Tabular results of the query, for the UI to render alongside the answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub columns: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<Vec<String>>>,
    /// True when the result set was capped at [`MAX_CHAT_QUERY_ROWS`].
    pub truncated: bool,
    /// Every query round of this turn, in order — successes and failures — so the
    /// UI can show the full retrieval trail.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<ChatQueryRun>,
}

/// POST /api/llm/chat — grounded knowledge-graph chat.
async fn llm_chat(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<ChatResponse>, AppError> {
    let user = user.as_deref();
    if req.messages.is_empty() || req.messages.iter().all(|m| m.content.trim().is_empty()) {
        return Err(AppError::BadRequest(
            "at least one message is required".to_string(),
        ));
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
        let role = if m.role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        msgs.push(json!({"role": role, "content": m.content}));
    }

    // Retrieval loop: the model either answers in prose or replies `SPARQL: <query>`.
    // Each query runs under the caller's read scope; its rows — or its error, so the
    // model can self-repair — go back into the conversation for the next round.
    let mut runs: Vec<ChatQueryRun> = Vec::new();
    let mut reply = chat_completion_messages(&model, msgs.clone(), CHAT_MAX_TOKENS).await?;
    for round in 1..=MAX_CHAT_QUERY_ROUNDS {
        let Some(query) = extract_sparql_directive(&reply) else {
            break;
        };
        msgs.push(json!({"role": "assistant", "content": format!("SPARQL:\n{query}")}));
        let remaining = MAX_CHAT_QUERY_ROUNDS - round;
        let follow_up = match run_chat_query(&state, &query, &graphs).await {
            Ok(qr) => {
                let table = render_rows_for_llm(&qr);
                runs.push(ChatQueryRun {
                    sparql: query,
                    ok: true,
                    error: None,
                    columns: Some(qr.columns),
                    rows: Some(qr.rows),
                    truncated: qr.truncated,
                });
                follow_up_after_rows(&table, remaining)
            }
            Err(e) => {
                let emsg = e.message();
                runs.push(ChatQueryRun {
                    sparql: query,
                    ok: false,
                    error: Some(emsg.clone()),
                    columns: None,
                    rows: None,
                    truncated: false,
                });
                follow_up_after_error(&emsg, remaining)
            }
        };
        msgs.push(json!({"role": "user", "content": follow_up}));
        reply = chat_completion_messages(&model, msgs.clone(), CHAT_MAX_TOKENS)
            .await
            .unwrap_or_else(|_| fallback_answer(&runs));
    }
    // A stubborn model may still emit a directive after its last allowed round —
    // never show that to the user; fall back to the data we did retrieve.
    if extract_sparql_directive(&reply).is_some() {
        reply = fallback_answer(&runs);
    }

    // Legacy single-query fields mirror the last successful round (or the last
    // attempt, so the UI can still offer "open in workspace" after a failure).
    let last = runs.iter().rev().find(|r| r.ok).or_else(|| runs.last());
    let ran_query = last.map(|r| r.ok).unwrap_or(false);
    let sparql = last.map(|r| r.sparql.clone());
    let columns = last.and_then(|r| r.columns.clone());
    let rows = last.and_then(|r| r.rows.clone());
    let truncated = last.map(|r| r.truncated).unwrap_or(false);
    Ok(Json(ChatResponse {
        answer: reply,
        model,
        ran_query,
        sparql,
        columns,
        rows,
        truncated,
        queries: runs,
    }))
}

/// The follow-up user message after a successful query round: rows in, and either
/// an invitation to keep querying or the instruction to write the final answer.
/// Shared verbatim by the buffered and streaming chat loops.
pub(crate) fn follow_up_after_rows(table: &str, remaining: usize) -> String {
    if remaining > 0 {
        format!(
            "Query results:\n{table}\nIf you still need different data, reply with \
             `SPARQL:` and one query ({remaining} more allowed this turn). Otherwise \
             write the final answer to my previous question in clear natural language, \
             using the presentation widgets (chart/map/card/api/csv/markdown table) \
             where they help."
        )
    } else {
        format!(
            "Query results:\n{table}\nWrite the final answer to my previous question \
             in clear natural language, using the presentation widgets where they \
             help. Do not output another SPARQL: line."
        )
    }
}

/// The follow-up user message after a failed query round: the error for
/// self-repair, bounded by how many rounds remain.
pub(crate) fn follow_up_after_error(emsg: &str, remaining: usize) -> String {
    if remaining > 0 {
        format!(
            "That query failed to run: {emsg}\nReply with `SPARQL:` and a corrected \
             query ({remaining} more allowed this turn), or answer without querying — \
             you may include the corrected query as a ```sparql block for the user to \
             run themselves."
        )
    } else {
        format!(
            "That query failed to run: {emsg}\nAnswer my previous question as well as \
             you can without another query; include a corrected query as a ```sparql \
             block if useful. Do not output another SPARQL: line."
        )
    }
}

/// Last-resort answer when the model keeps demanding more queries than allowed (or
/// the gateway dies mid-turn): surface what we did retrieve instead of leaking a
/// raw `SPARQL:` directive to the user.
pub(crate) fn fallback_answer(runs: &[ChatQueryRun]) -> String {
    if let Some(ok) = runs.iter().rev().find(|r| r.ok) {
        let mut s = String::from("Here is what the query returned:\n\n");
        if let (Some(cols), Some(rows)) = (&ok.columns, &ok.rows) {
            s.push_str(&format!("| {} |\n", cols.join(" | ")));
            s.push_str(&format!(
                "|{}|\n",
                cols.iter().map(|_| " --- ").collect::<Vec<_>>().join("|")
            ));
            for row in rows.iter().take(15) {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| truncate(c, 80).replace('|', "\\|"))
                    .collect();
                s.push_str(&format!("| {} |\n", cells.join(" | ")));
            }
            if rows.is_empty() {
                s.push_str("\n*(no rows)*\n");
            } else if rows.len() > 15 || ok.truncated {
                s.push_str("\n*(more rows not shown)*\n");
            }
        }
        s
    } else if let Some(last) = runs.last() {
        format!(
            "I tried to answer by querying the knowledge graph, but the query did not run ({}). \
             You can refine it here:\n\n```sparql\n{}\n```",
            last.error.as_deref().unwrap_or("unknown error"),
            last.sparql
        )
    } else {
        "I could not produce an answer this turn — please try rephrasing the question.".to_string()
    }
}

/// The named graphs `user` may read — the same scope the normal SPARQL endpoint
/// applies. Mirrors `execute_query`: accessible-dataset graphs, plus named-graph
/// ACL grants, plus (for admins) every registered graph.
pub(crate) fn chat_accessible_graphs(
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
            if let Ok(acl) = state
                .auth_db
                .get_graph_acl_readable_iris(&u.user_id, u.role.as_str())
            {
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
pub(crate) fn build_platform_context(
    state: &AppState,
    user_id: Option<&str>,
    graphs: &HashSet<String>,
) -> String {
    let mut ctx = String::new();

    let datasets = state
        .auth_db
        .list_accessible_datasets(user_id)
        .unwrap_or_default();
    ctx.push_str(&format!("## Datasets ({} accessible)\n", datasets.len()));
    for d in datasets.iter().take(MAX_DATASETS_IN_CONTEXT) {
        ctx.push_str(&format!(
            "- \"{}\" (id {}, {:?})",
            d.name, d.id, d.visibility
        ));
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
        ctx.push_str(&format!(
            "- …and {} more.\n",
            datasets.len() - MAX_DATASETS_IN_CONTEXT
        ));
    }

    // API services across the accessible datasets.
    let store = SavedQueryStore::new(state.auth_db.pool());
    let mut services: Vec<String> = Vec::new();
    for d in &datasets {
        if services.len() >= MAX_SERVICES_IN_CONTEXT {
            break;
        }
        let Ok(queries) = store.list_active_dataset_queries(&d.id) else {
            continue;
        };
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
            ctx.push_str(&format!(
                "- …and {} more graphs.\n",
                graphs.len() - MAX_GRAPHS_IN_CONTEXT
            ));
        }
    }

    ctx
}

/// Tabular result of a chat-issued query.
pub(crate) struct ChatQueryResult {
    pub(crate) columns: Vec<String>,
    pub(crate) rows: Vec<Vec<String>>,
    pub(crate) truncated: bool,
}

/// Run a model-generated query under the caller's read scope and collect a capped
/// table. The query is re-scoped with [`scope_query_to_authorized`] (the read
/// boundary) exactly like a user-typed query, so it cannot read outside `graphs`.
pub(crate) async fn run_chat_query(
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
                let columns: Vec<String> = solutions
                    .variables()
                    .iter()
                    .map(|v| v.as_str().to_string())
                    .collect();
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
                Ok(ChatQueryResult {
                    columns,
                    rows,
                    truncated,
                })
            }
            QueryResults::Graph(triples) => {
                let columns = vec![
                    "subject".to_string(),
                    "predicate".to_string(),
                    "object".to_string(),
                ];
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
                Ok(ChatQueryResult {
                    columns,
                    rows,
                    truncated,
                })
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
pub(crate) fn render_rows_for_llm(qr: &ChatQueryResult) -> String {
    let mut s = String::new();
    s.push_str(&qr.columns.join(" | "));
    s.push('\n');
    for row in &qr.rows {
        let cells: Vec<String> = row.iter().map(|c| truncate(c, cell_budget(c))).collect();
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

/// Prompt budget for one result cell: WKT geometries get a larger budget than
/// ordinary values so small ones survive verbatim into a ```map widget.
fn cell_budget(cell: &str) -> usize {
    if looks_like_wkt(cell) {
        CHAT_WKT_CELL_MAX_CHARS
    } else {
        CHAT_CELL_MAX_CHARS
    }
}

/// Does this value look like a WKT geometry literal, optionally carrying a
/// GeoSPARQL `<crs-iri>` prefix?
fn looks_like_wkt(s: &str) -> bool {
    let t = s.trim_start();
    let t = match t.strip_prefix('<') {
        Some(rest) => rest
            .split_once('>')
            .map(|(_, after)| after.trim_start())
            .unwrap_or(t),
        None => t,
    };
    const KINDS: [&str; 7] = [
        "MULTIPOINT",
        "MULTILINESTRING",
        "MULTIPOLYGON",
        "GEOMETRYCOLLECTION",
        "POINT",
        "LINESTRING",
        "POLYGON",
    ];
    KINDS
        .iter()
        .any(|k| t.get(..k.len()).is_some_and(|p| p.eq_ignore_ascii_case(k)))
}

/// If the model asked to run a query, return the query text. We look for a
/// `SPARQL:` marker (case-insensitive, byte-safe), strip any code fence, and only
/// accept it when it actually contains a query form — otherwise the reply is prose.
pub(crate) fn extract_sparql_directive(reply: &str) -> Option<String> {
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
pub(crate) fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
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
    use super::{
        extract_sparql_directive, fallback_answer, find_ci, looks_like_wkt, strip_code_fence,
        truncate, validate_sparql, ChatQueryRun, CHAT_CELL_MAX_CHARS, CHAT_WKT_CELL_MAX_CHARS,
    };

    #[test]
    fn wkt_cells_are_recognised_for_the_larger_budget() {
        assert!(looks_like_wkt("POINT(5.8645 51.8519)"));
        assert!(looks_like_wkt("point(5.8645 51.8519)"));
        assert!(looks_like_wkt(
            "<http://www.opengis.net/def/crs/EPSG/0/4326> POLYGON((0 0, 1 0, 1 1, 0 0))"
        ));
        assert!(looks_like_wkt("  MULTIPOLYGON(((0 0,1 0,1 1,0 0)))"));
        assert!(!looks_like_wkt("Waalbrug"));
        assert!(!looks_like_wkt("http://example.org/bridge/1"));
        // Multi-byte content must not panic the prefix check.
        assert!(!looks_like_wkt("héllo wörld"));
        assert_eq!(super::cell_budget("POINT(1 2)"), CHAT_WKT_CELL_MAX_CHARS);
        assert_eq!(super::cell_budget("plain value"), CHAT_CELL_MAX_CHARS);
    }

    #[test]
    fn fallback_answer_prefers_last_successful_run() {
        let runs = vec![
            ChatQueryRun {
                sparql: "SELECT ?broken".into(),
                ok: false,
                error: Some("parse error".into()),
                columns: None,
                rows: None,
                truncated: false,
            },
            ChatQueryRun {
                sparql: "SELECT ?name ?count WHERE {}".into(),
                ok: true,
                error: None,
                columns: Some(vec!["name".into(), "count".into()]),
                rows: Some(vec![vec!["Waalbrug".into(), "3".into()]]),
                truncated: false,
            },
        ];
        let s = fallback_answer(&runs);
        assert!(s.contains("| name | count |"), "markdown header: {s}");
        assert!(s.contains("| Waalbrug | 3 |"), "row: {s}");
        assert!(
            !s.to_uppercase().contains("SPARQL:"),
            "no directive leaks: {s}"
        );
    }

    #[test]
    fn fallback_answer_surfaces_failed_query_for_the_user() {
        let runs = vec![ChatQueryRun {
            sparql: "SELECT ?s WHERE { ?s ?p }".into(),
            ok: false,
            error: Some("parse error".into()),
            columns: None,
            rows: None,
            truncated: false,
        }];
        let s = fallback_answer(&runs);
        assert!(s.contains("parse error"));
        assert!(s.contains("```sparql"), "offers the query to refine: {s}");
    }

    #[test]
    fn validate_sparql_accepts_valid_and_rejects_invalid() {
        assert!(validate_sparql(
            "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> \
             SELECT ?s WHERE { ?s rdfs:label ?l }"
        )
        .is_ok());
        assert!(validate_sparql("this is not sparql").is_err());
        // An undeclared prefix must fail to parse — this is exactly why the server
        // injects forgotten prefixes (finalize_sparql) before validating.
        assert!(validate_sparql("SELECT ?s WHERE { ?s foaf:name ?n }").is_err());
    }

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
