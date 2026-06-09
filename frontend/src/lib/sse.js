// Minimal incremental parser for a Server-Sent-Events byte stream, for endpoints
// EventSource can't reach (POST bodies + Authorization headers — our streaming
// chat). Feed it decoded text chunks as they arrive; it invokes `onEvent(name,
// data)` for each complete event. Handles the full framing: CRLF or LF line
// ends, `data:` with or without the leading space, multi-`data:`-line events
// (joined with \n per spec), `:` comment keep-alives, and chunk boundaries that
// split lines anywhere.

/**
 * @param {(name: string, data: string) => void} onEvent
 * @returns {{push(chunk: string): void, finish(): void}}
 */
export function createSseParser(onEvent) {
  let buf = '';
  let eventName = '';
  let dataLines = [];

  const dispatch = () => {
    if (dataLines.length) {
      onEvent(eventName || 'message', dataLines.join('\n'));
    }
    eventName = '';
    dataLines = [];
  };

  const takeLine = (line) => {
    if (line === '') {
      dispatch();
    } else if (line.startsWith(':')) {
      // comment / keep-alive
    } else if (line.startsWith('event:')) {
      eventName = line.slice('event:'.length).trim();
    } else if (line.startsWith('data:')) {
      const d = line.slice('data:'.length);
      dataLines.push(d.startsWith(' ') ? d.slice(1) : d);
    }
    // other fields (id:, retry:) are irrelevant here
  };

  return {
    push(chunk) {
      buf += chunk;
      let nl;
      while ((nl = buf.indexOf('\n')) !== -1) {
        let line = buf.slice(0, nl);
        buf = buf.slice(nl + 1);
        if (line.endsWith('\r')) line = line.slice(0, -1);
        takeLine(line);
      }
    },
    // The server may close without a trailing blank line — deliver what's pending.
    finish() {
      if (buf !== '') {
        takeLine(buf.endsWith('\r') ? buf.slice(0, -1) : buf);
        buf = '';
      }
      dispatch();
    },
  };
}
