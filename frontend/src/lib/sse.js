// Minimal Server-Sent-Events reader for fetch() response bodies.
//
// EventSource can't send Authorization headers or a POST body, so the chat
// streams through fetch + ReadableStream instead. The server emits one JSON
// document per `data:` line; this yields each parsed document in order.

/**
 * Async-iterate the JSON payloads of an SSE response body.
 * Ignores comments, `event:`/`id:` fields, blank lines, unparsable payloads,
 * and the OpenAI-style `[DONE]` sentinel.
 *
 * @param {ReadableStream<Uint8Array>} body fetch response body
 */
export async function* sseJsonEvents(body) {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buf = '';
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      let nl;
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl).replace(/\r$/, '');
        buf = buf.slice(nl + 1);
        const ev = parseSseLine(line);
        if (ev !== undefined) yield ev;
      }
    }
    // A final unterminated line (stream closed without trailing newline).
    const ev = parseSseLine(buf.replace(/\r$/, ''));
    if (ev !== undefined) yield ev;
  } finally {
    reader.releaseLock();
  }
}

/**
 * Parse one SSE line into its JSON payload, or `undefined` when the line
 * carries no event data (other fields, comments, sentinels, broken JSON).
 */
export function parseSseLine(line) {
  if (!line.startsWith('data:')) return undefined;
  const payload = line.slice(5).trim();
  if (!payload || payload === '[DONE]') return undefined;
  try {
    return JSON.parse(payload);
  } catch {
    return undefined;
  }
}
