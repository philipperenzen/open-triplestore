import { describe, it, expect } from 'vitest';
import { createSseParser } from '../sse.js';

function collect() {
  const events = [];
  const parser = createSseParser((name, data) => events.push([name, data]));
  return { events, parser };
}

describe('createSseParser', () => {
  it('parses named events with JSON payloads', () => {
    const { events, parser } = collect();
    parser.push('event: delta\ndata: {"text":"Hi"}\n\n');
    parser.push('event: done\ndata: {"answer":"Hi"}\n\n');
    parser.finish();
    expect(events).toEqual([
      ['delta', '{"text":"Hi"}'],
      ['done', '{"answer":"Hi"}'],
    ]);
  });

  it('reassembles events split anywhere across chunks', () => {
    const { events, parser } = collect();
    const wire = 'event: delta\ndata: {"text":"Hello world"}\n\n';
    for (const ch of wire) parser.push(ch); // one char at a time
    parser.finish();
    expect(events).toEqual([['delta', '{"text":"Hello world"}']]);
  });

  it('handles CRLF lines, data without space, and comment keep-alives', () => {
    const { events, parser } = collect();
    parser.push(': ping\r\n\r\nevent: delta\r\ndata:{"text":"a"}\r\n\r\n');
    parser.finish();
    expect(events).toEqual([['delta', '{"text":"a"}']]);
  });

  it('joins multi data-line events with newlines (SSE spec)', () => {
    const { events, parser } = collect();
    parser.push('event: x\ndata: line1\ndata: line2\n\n');
    parser.finish();
    expect(events).toEqual([['x', 'line1\nline2']]);
  });

  it('defaults the event name to "message" and resets it between events', () => {
    const { events, parser } = collect();
    parser.push('event: delta\ndata: a\n\ndata: b\n\n');
    parser.finish();
    expect(events).toEqual([
      ['delta', 'a'],
      ['message', 'b'],
    ]);
  });

  it('delivers a final unterminated event on finish', () => {
    const { events, parser } = collect();
    parser.push('event: done\ndata: {"answer":"tail"}'); // connection closed here
    parser.finish();
    expect(events).toEqual([['done', '{"answer":"tail"}']]);
  });

  it('ignores blank keep-alive frames without data', () => {
    const { events, parser } = collect();
    parser.push('\n\n: keep\n\n\n');
    parser.finish();
    expect(events).toEqual([]);
  });
});
