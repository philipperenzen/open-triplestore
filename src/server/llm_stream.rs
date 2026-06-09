//! Streaming knowledge-graph chat (`POST /api/llm/chat/stream`).
//!
//! Same grounded retrieval loop as [`super::llm_sparql::llm_chat`] — same system
//! prompt, same `scope_query_to_authorized` read boundary, same round budget —
//! but the gateway is asked for `stream: true` and answer tokens are forwarded to
//! the browser the moment they arrive, as Server-Sent Events. Time-to-first-token
//! becomes the gateway's own TTFT instead of the full multi-round turn.
//!
//! The wrinkle is the retrieval protocol: a round's reply may be a `SPARQL:`
//! directive, which must *never* render as answer text. We can't know which kind
//! of reply we're in until tokens arrive, so [`DirectiveScanner`] holds tokens
//! back just long enough to decide: nothing is emitted until a short initial
//! window passes without the marker, a marker-sized tail is always withheld so a
//! split `SPA…RQL:` can't leak, and from the moment the marker is seen everything
//! is suppressed. In the rare case prose was already emitted before a late marker
//! (`Sure, let me check. SPARQL: …`), a `round_discard` event tells the client to
//! drop that round's text. The closing `done` event carries the complete
//! [`ChatResponse`], so the streamed state is always reconciled to exactly what
//! the buffered endpoint would have returned.
//!
//! Event protocol (all payloads JSON):
//! - `meta`          `{model}` — sent immediately, before the first gateway byte
//! - `round`         `{round}` — a completion round started
//! - `delta`         `{text}` — answer text to append
//! - `round_discard` `{}` — drop all text streamed this turn (it was a directive)
//! - `query`         `{sparql}` — a retrieval round: this query is about to run
//! - `query_result`  [`ChatQueryRun`] — that run finished (rows or error)
//! - `done`          [`ChatResponse`] — authoritative final state
//! - `error`         `{message}` — terminal failure before anything was produced

use std::collections::HashSet;
use std::convert::Infallible;
use std::time::Duration;

use axum::{
    extract::State,
    http::{header::HeaderName, HeaderValue},
    response::sse::{Event, KeepAlive, Sse},
    response::IntoResponse,
    Extension, Json,
};
use futures::Stream;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::auth::middleware::AuthenticatedUser;

use super::error::AppError;
use super::llm_sparql::{
    api_key, build_platform_context, chat_accessible_graphs, default_model,
    extract_sparql_directive, fallback_answer, find_ci, follow_up_after_error,
    follow_up_after_rows, gateway_base, llm_client, render_rows_for_llm, run_chat_query,
    ChatQueryRun, ChatRequest, ChatResponse, CHAT_MAX_TOKENS, CHAT_SYSTEM_PROMPT,
    MAX_CHAT_QUERY_ROUNDS,
};
use super::AppState;

/// Whole-request budget for one streamed completion round. `reqwest`'s timeout
/// covers the entire body read, so this must comfortably exceed the longest
/// expected generation — it only exists to reap a gateway that wedges mid-stream.
const STREAM_ROUND_TIMEOUT: Duration = Duration::from_secs(300);

// ─── OpenAI SSE wire parsing ───────────────────────────────────────────────────

/// Incremental parser for an OpenAI-style `text/event-stream` completion body.
/// Feed raw bytes as they arrive; out come the `choices[0].delta.content`
/// fragments. Handles chunk boundaries that split lines (or UTF-8 sequences —
/// only complete `\n`-terminated lines are decoded), `data:` with or without the
/// space, multi-`data:`-line events, `:` comment lines, and the `[DONE]` sentinel.
pub(crate) struct GatewayStreamParser {
    /// Unconsumed bytes — everything after the last complete line.
    buf: Vec<u8>,
    /// `data:` payload lines of the event currently being assembled.
    data_lines: Vec<String>,
    done: bool,
}

impl GatewayStreamParser {
    pub(crate) fn new() -> Self {
        Self {
            buf: Vec::new(),
            data_lines: Vec::new(),
            done: false,
        }
    }

    /// Feed one network chunk; returns the content fragments it completed.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        // Process every complete line; a UTF-8 char never contains b'\n', so
        // splitting on it can't cut a character in half.
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            self.take_line(line, &mut out);
        }
        out
    }

    /// End of stream: dispatch a final event some servers leave unterminated.
    pub(crate) fn finish(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buf)).to_string();
            self.take_line(line.trim_end_matches(['\n', '\r']), &mut out);
        }
        self.dispatch(&mut out);
        out
    }

    pub(crate) fn is_done(&self) -> bool {
        self.done
    }

    fn take_line(&mut self, line: &str, out: &mut Vec<String>) {
        if self.done {
            return;
        }
        if line.is_empty() {
            self.dispatch(out);
        } else if let Some(data) = line.strip_prefix("data:") {
            self.data_lines
                .push(data.strip_prefix(' ').unwrap_or(data).to_string());
        }
        // `:` comments (keep-alives) and other fields (event:, id:, retry:) are ignored.
    }

    /// Assemble the pending `data:` lines into one event payload and extract the
    /// delta text, if any.
    fn dispatch(&mut self, out: &mut Vec<String>) {
        if self.data_lines.is_empty() {
            return;
        }
        let payload = std::mem::take(&mut self.data_lines).join("\n");
        let payload = payload.trim();
        if payload == "[DONE]" {
            self.done = true;
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(payload) else {
            return; // malformed chunk — skip rather than abort the stream
        };
        if let Some(text) = v["choices"][0]["delta"]["content"].as_str() {
            if !text.is_empty() {
                out.push(text.to_string());
            }
        }
    }
}

// ─── Directive hold-back ───────────────────────────────────────────────────────

/// The retrieval-directive marker (see `extract_sparql_directive`).
const MARKER: &str = "SPARQL:";
/// Don't start emitting until this many visible chars arrive without a marker —
/// a directive reply announces itself almost immediately, so this keeps even a
/// misbehaving model's `SPARQL:` from flashing in the UI. ~6 tokens of latency.
const HOLD_INITIAL_CHARS: usize = 24;
/// Never emit the trailing marker-length bytes mid-stream, so a marker split
/// across network chunks (`SPA` + `RQL:`) can never partially leak.
const HOLD_TAIL_BYTES: usize = MARKER.len();

/// Decides, token by token, whether a round's reply is answer prose (forward it)
/// or a `SPARQL:` retrieval directive (suppress it). Pure state machine — the
/// caller forwards whatever [`push`](Self::push)/[`flush`](Self::flush) return.
pub(crate) struct DirectiveScanner {
    buf: String,
    /// Byte offset of `buf` already handed out.
    emitted: usize,
    /// Byte position of the marker, once seen. From then on nothing is emitted.
    marker_at: Option<usize>,
}

impl DirectiveScanner {
    pub(crate) fn new() -> Self {
        Self {
            buf: String::new(),
            emitted: 0,
            marker_at: None,
        }
    }

    /// Add a fragment; returns text now safe to show the user, if any.
    pub(crate) fn push(&mut self, fragment: &str) -> Option<String> {
        self.buf.push_str(fragment);
        if self.marker_at.is_none() {
            self.marker_at = find_ci(&self.buf, MARKER);
        }
        if self.marker_at.is_some() {
            return None; // suppress everything from the moment a marker exists
        }
        if self.buf.trim_start().chars().count() < HOLD_INITIAL_CHARS {
            return None;
        }
        let limit = floor_char_boundary(&self.buf, self.buf.len() - HOLD_TAIL_BYTES);
        self.emit_to(limit)
    }

    /// End of round for a reply that is NOT a directive: release everything held
    /// back (the tail, or the whole short reply). The caller decides — only after
    /// `extract_sparql_directive` on the full text says this is prose.
    pub(crate) fn flush(&mut self) -> Option<String> {
        self.emit_to(self.buf.len())
    }

    /// Was anything shown to the user this round? (Drives `round_discard`.)
    pub(crate) fn emitted_any(&self) -> bool {
        self.emitted > 0
    }

    /// The complete reply accumulated so far.
    pub(crate) fn text(&self) -> &str {
        &self.buf
    }

    fn emit_to(&mut self, limit: usize) -> Option<String> {
        if limit <= self.emitted {
            return None;
        }
        let out = self.buf[self.emitted..limit].to_string();
        self.emitted = limit;
        Some(out)
    }
}

/// Largest char boundary ≤ `i` (stable substitute for `str::floor_char_boundary`).
fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

// ─── Gateway streaming call ────────────────────────────────────────────────────

/// One streamed chat completion. Each content fragment is handed to
/// `on_fragment` as it arrives; returns the full reply text. When the gateway
/// ignores `stream: true` and answers with plain JSON (some proxies do), the
/// whole reply arrives as a single fragment — streaming degrades, nothing breaks.
/// `on_fragment` returning `false` aborts the read (client went away).
pub(crate) async fn stream_chat_completion(
    base: &str,
    model: &str,
    messages: &[Value],
    max_tokens: u32,
    mut on_fragment: impl FnMut(&str) -> bool,
) -> Result<String, AppError> {
    let payload = json!({
        "model": model,
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "messages": messages,
        "stream": true,
    });
    let url = format!("{}/v1/chat/completions", base.trim_end_matches('/'));
    let mut rb = llm_client()
        .post(&url)
        .json(&payload)
        .timeout(STREAM_ROUND_TIMEOUT);
    if let Some(key) = api_key() {
        rb = rb.bearer_auth(key);
    }
    let mut resp = rb
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("LLM endpoint unreachable at {url}: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::Internal(format!(
            "LLM endpoint returned {}",
            resp.status()
        )));
    }

    let is_sse = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false);
    if !is_sse {
        let body: Value = resp
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("invalid LLM response: {e}")))?;
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string();
        on_fragment(&content);
        return Ok(content);
    }

    let mut parser = GatewayStreamParser::new();
    let mut full = String::new();
    loop {
        let chunk = resp
            .chunk()
            .await
            .map_err(|e| AppError::Internal(format!("LLM stream failed: {e}")))?;
        let Some(chunk) = chunk else { break };
        for frag in parser.push(&chunk) {
            full.push_str(&frag);
            if !on_fragment(&frag) {
                return Err(AppError::Internal("client disconnected".to_string()));
            }
        }
        if parser.is_done() {
            break;
        }
    }
    for frag in parser.finish() {
        full.push_str(&frag);
        on_fragment(&frag);
    }
    Ok(full.trim().to_string())
}

// ─── The streaming chat endpoint ───────────────────────────────────────────────

/// POST /api/llm/chat/stream — grounded knowledge-graph chat, streamed as SSE.
/// Validation, authorization scope and platform context are identical to
/// `llm_chat`; they run before the stream opens so request errors are still
/// ordinary HTTP errors, not events.
pub async fn llm_chat_stream(
    State(state): State<AppState>,
    user: Option<Extension<AuthenticatedUser>>,
    Json(req): Json<ChatRequest>,
) -> Result<impl IntoResponse, AppError> {
    let user = user.as_deref();
    if req.messages.is_empty() || req.messages.iter().all(|m| m.content.trim().is_empty()) {
        return Err(AppError::BadRequest(
            "at least one message is required".to_string(),
        ));
    }
    let model = req.model.clone().unwrap_or_else(default_model);
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

    let (tx, rx) = mpsc::unbounded_channel::<Event>();
    tokio::spawn(run_chat_stream(state, model, msgs, graphs, tx));
    let stream = sse_receiver_stream(rx);
    Ok((
        // Tell buffering reverse proxies (nginx & friends) to pass events through.
        [(
            HeaderName::from_static("x-accel-buffering"),
            HeaderValue::from_static("no"),
        )],
        Sse::new(stream).keep_alive(KeepAlive::default()),
    ))
}

/// Adapt the worker channel into the `Stream` axum's `Sse` consumes.
fn sse_receiver_stream(
    rx: mpsc::UnboundedReceiver<Event>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (Ok(ev), rx))
    })
}

/// A finished completion round: the full reply text and whether any of it was
/// already shown to the user (drives `round_discard` when it turns out to be a
/// directive).
struct StreamedRound {
    text: String,
    emitted_any: bool,
}

/// The retrieval loop of `llm_chat`, restated around streamed rounds. Event
/// sends ignore failure — a send only fails when the client is gone, and the
/// between-round `is_closed` checks stop the work soon after.
async fn run_chat_stream(
    state: AppState,
    model: String,
    mut msgs: Vec<Value>,
    graphs: HashSet<String>,
    tx: mpsc::UnboundedSender<Event>,
) {
    let send = |name: &str, data: String| {
        let _ = tx.send(Event::default().event(name).data(data));
    };
    send("meta", json!({"model": model}).to_string());

    let mut runs: Vec<ChatQueryRun> = Vec::new();
    send("round", json!({"round": 1}).to_string());
    let mut reply = match stream_round(&model, &msgs, &tx).await {
        Ok(r) => r,
        Err(e) => {
            // Mirror llm_chat: a dead gateway on the FIRST round is a request error.
            send("error", json!({"message": e.message()}).to_string());
            return;
        }
    };

    for round in 1..=MAX_CHAT_QUERY_ROUNDS {
        let Some(query) = extract_sparql_directive(&reply.text) else {
            break;
        };
        if reply.emitted_any {
            send("round_discard", "{}".to_string());
        }
        send("query", json!({"sparql": query}).to_string());
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
        if let Some(run) = runs.last() {
            if let Ok(v) = serde_json::to_string(run) {
                send("query_result", v);
            }
        }
        if tx.is_closed() {
            return; // client went away — don't spend more gateway tokens
        }
        msgs.push(json!({"role": "user", "content": follow_up}));
        send("round", json!({"round": round + 1}).to_string());
        reply = match stream_round(&model, &msgs, &tx).await {
            Ok(r) => r,
            Err(_) => {
                if tx.is_closed() {
                    return;
                }
                // Mirror llm_chat: the gateway died mid-turn after we already
                // retrieved data — surface what we have instead of nothing.
                let fb = fallback_answer(&runs);
                send("delta", json!({"text": fb}).to_string());
                StreamedRound {
                    text: fb,
                    emitted_any: true,
                }
            }
        };
    }

    // A stubborn model may still emit a directive after its last allowed round —
    // never show that to the user; fall back to the data we did retrieve.
    if extract_sparql_directive(&reply.text).is_some() {
        if reply.emitted_any {
            send("round_discard", "{}".to_string());
        }
        let fb = fallback_answer(&runs);
        send("delta", json!({"text": fb}).to_string());
        reply = StreamedRound {
            text: fb,
            emitted_any: true,
        };
    }

    // Authoritative final payload — identical shape to the buffered endpoint.
    let last = runs.iter().rev().find(|r| r.ok).or_else(|| runs.last());
    let resp = ChatResponse {
        answer: reply.text,
        model,
        ran_query: last.map(|r| r.ok).unwrap_or(false),
        sparql: last.map(|r| r.sparql.clone()),
        columns: last.and_then(|r| r.columns.clone()),
        rows: last.and_then(|r| r.rows.clone()),
        truncated: last.map(|r| r.truncated).unwrap_or(false),
        queries: runs,
    };
    if let Ok(v) = serde_json::to_string(&resp) {
        send("done", v);
    }
}

/// Run one completion round, forwarding answer prose as `delta` events through a
/// [`DirectiveScanner`]. Text held back by the scanner is only released once the
/// full reply proves to be prose, so a directive never renders.
async fn stream_round(
    model: &str,
    msgs: &[Value],
    tx: &mpsc::UnboundedSender<Event>,
) -> Result<StreamedRound, AppError> {
    let mut scanner = DirectiveScanner::new();
    let full = {
        let scanner = &mut scanner;
        stream_chat_completion(&gateway_base(), model, msgs, CHAT_MAX_TOKENS, |frag| {
            if let Some(text) = scanner.push(frag) {
                let _ = tx.send(
                    Event::default()
                        .event("delta")
                        .data(json!({"text": text}).to_string()),
                );
            }
            !tx.is_closed()
        })
        .await?
    };
    if extract_sparql_directive(scanner.text()).is_none() {
        if let Some(rest) = scanner.flush() {
            let _ = tx.send(
                Event::default()
                    .event("delta")
                    .data(json!({"text": rest}).to_string()),
            );
        }
    }
    Ok(StreamedRound {
        text: full,
        emitted_any: scanner.emitted_any(),
    })
}

#[cfg(test)]
mod tests {
    use super::{floor_char_boundary, DirectiveScanner, GatewayStreamParser};

    fn chunk(s: &str) -> String {
        format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":{}}}}}]}}\n\n",
            serde_json::to_string(s).unwrap()
        )
    }

    #[test]
    fn parser_extracts_deltas_across_split_chunks() {
        let mut p = GatewayStreamParser::new();
        let wire = format!("{}{}data: [DONE]\n\n", chunk("Hel"), chunk("lo"));
        let bytes = wire.as_bytes();
        let mut out = Vec::new();
        // Feed one byte at a time — every line and JSON boundary gets split.
        for b in bytes {
            out.extend(p.push(&[*b]));
        }
        out.extend(p.finish());
        assert_eq!(out.join(""), "Hello");
        assert!(p.is_done());
    }

    #[test]
    fn parser_survives_utf8_split_inside_a_line() {
        let mut p = GatewayStreamParser::new();
        let wire = chunk("héllo ✓");
        let bytes = wire.as_bytes();
        let mid = bytes.len() / 2;
        let mut out = p.push(&bytes[..mid]);
        out.extend(p.push(&bytes[mid..]));
        out.extend(p.finish());
        assert_eq!(out.join(""), "héllo ✓");
    }

    #[test]
    fn parser_handles_no_space_data_comments_and_role_chunks() {
        let mut p = GatewayStreamParser::new();
        let wire = ": keep-alive\n\n\
                    data:{\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n\
                    data:{\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n";
        let mut out = p.push(wire.as_bytes());
        out.extend(p.finish());
        assert_eq!(out.join(""), "hi");
        assert!(!p.is_done());
    }

    #[test]
    fn parser_flushes_unterminated_final_event() {
        let mut p = GatewayStreamParser::new();
        // No trailing blank line and no [DONE] — connection just closed.
        let wire = "data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}";
        let mut out = p.push(wire.as_bytes());
        out.extend(p.finish());
        assert_eq!(out.join(""), "tail");
    }

    #[test]
    fn parser_skips_malformed_chunks_without_aborting() {
        let mut p = GatewayStreamParser::new();
        let wire = format!("data: {{not json\n\n{}", chunk("ok"));
        let mut out = p.push(wire.as_bytes());
        out.extend(p.finish());
        assert_eq!(out.join(""), "ok");
    }

    /// Drive a scanner with fragments; returns (emitted live, flushed at end).
    fn run_scanner(frags: &[&str]) -> (String, DirectiveScanner) {
        let mut sc = DirectiveScanner::new();
        let mut live = String::new();
        for f in frags {
            if let Some(t) = sc.push(f) {
                live.push_str(&t);
            }
        }
        (live, sc)
    }

    #[test]
    fn scanner_streams_prose_and_flushes_the_tail() {
        let msg = "There are 3 datasets about water quality on this platform.";
        let frags: Vec<&str> = msg.split_inclusive(' ').collect();
        let (live, mut sc) = run_scanner(&frags);
        assert!(!live.is_empty(), "prose must stream before the round ends");
        let mut all = live.clone();
        if let Some(rest) = sc.flush() {
            all.push_str(&rest);
        }
        assert_eq!(all, msg);
        assert!(sc.emitted_any());
    }

    #[test]
    fn scanner_suppresses_directive_even_split_across_chunks() {
        let (live, sc) = run_scanner(&["SPA", "RQL: SELECT * WHERE { ?s ?p ?o }"]);
        assert_eq!(live, "", "no part of a directive may reach the user");
        assert!(!sc.emitted_any());
        assert_eq!(sc.text(), "SPARQL: SELECT * WHERE { ?s ?p ?o }");
    }

    #[test]
    fn scanner_suppresses_lowercase_marker_within_hold_window() {
        let (live, sc) = run_scanner(&["sparql:", " ASK { ?s ?p ?o }"]);
        assert_eq!(live, "");
        assert!(!sc.emitted_any());
    }

    #[test]
    fn scanner_stops_at_late_marker_after_preamble_streamed() {
        let msg = "Sure, let me look that up in the graph. SPARQL: SELECT ?s WHERE { ?s ?p ?o }";
        let frags: Vec<&str> = msg.split_inclusive(' ').collect();
        let (live, sc) = run_scanner(&frags);
        assert!(sc.emitted_any(), "preamble streamed before the marker arrived");
        assert!(
            !live.to_uppercase().contains("SPARQL:"),
            "marker itself must never be emitted: {live}"
        );
        assert!(!live.contains("SELECT"));
    }

    #[test]
    fn scanner_holds_short_replies_until_flush() {
        let (live, mut sc) = run_scanner(&["Yes."]);
        assert_eq!(live, "", "below the hold window nothing streams yet");
        assert_eq!(sc.flush().as_deref(), Some("Yes."));
    }

    #[test]
    fn scanner_flush_releases_everything_for_prose_with_marker_like_text() {
        // Caller flushes when extract_sparql_directive says it's NOT a directive
        // (marker without a query form) — the user must still get the whole text.
        let (live, mut sc) = run_scanner(&["SPARQL: is the query language of RDF."]);
        assert_eq!(live, "");
        let mut all = String::new();
        if let Some(rest) = sc.flush() {
            all.push_str(&rest);
        }
        assert_eq!(all, "SPARQL: is the query language of RDF.");
    }

    #[test]
    fn scanner_never_splits_multibyte_chars() {
        // Tail holdback lands mid-char unless floored to a boundary.
        let msg = "Längenmaße und Flächen größer als üblich — Berge, Täler, Seen.";
        let frags: Vec<&str> = msg.split_inclusive(' ').collect();
        let (live, mut sc) = run_scanner(&frags);
        let mut all = live;
        if let Some(rest) = sc.flush() {
            all.push_str(&rest);
        }
        assert_eq!(all, msg);
    }

    #[test]
    fn floor_char_boundary_is_safe() {
        let s = "aé"; // 'é' is two bytes starting at index 1
        assert_eq!(floor_char_boundary(s, 2), 1);
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 99), s.len());
        assert_eq!(floor_char_boundary("", 0), 0);
    }
}
