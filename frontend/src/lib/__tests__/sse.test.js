import { describe, it, expect } from 'vitest';
import { parseSseLine, sseJsonEvents } from '../sse.js';

function streamOf(chunks) {
  const enc = new TextEncoder();
  return new ReadableStream({
    start(controller) {
      for (const c of chunks) controller.enqueue(enc.encode(c));
      controller.close();
    },
  });
}

async function collect(chunks) {
  const out = [];
  for await (const ev of sseJsonEvents(streamOf(chunks))) out.push(ev);
  return out;
}

describe('parseSseLine', () => {
  it('extracts JSON payloads with and without a space after data:', () => {
    expect(parseSseLine('data: {"a":1}')).toEqual({ a: 1 });
    expect(parseSseLine('data:{"a":1}')).toEqual({ a: 1 });
  });

  it('ignores non-data fields, comments, sentinels and broken JSON', () => {
    expect(parseSseLine('event: message')).toBeUndefined();
    expect(parseSseLine('id: 4')).toBeUndefined();
    expect(parseSseLine(': keep-alive')).toBeUndefined();
    expect(parseSseLine('data: [DONE]')).toBeUndefined();
    expect(parseSseLine('data: {broken')).toBeUndefined();
    expect(parseSseLine('')).toBeUndefined();
  });
});

describe('sseJsonEvents', () => {
  it('yields each data event in order', async () => {
    const events = await collect([
      'data: {"type":"status","round":0}\n\n',
      'data: {"type":"delta","text":"Hi"}\n\n',
    ]);
    expect(events).toEqual([
      { type: 'status', round: 0 },
      { type: 'delta', text: 'Hi' },
    ]);
  });

  it('reassembles events split across network chunks', async () => {
    const events = await collect([
      'data: {"type":"del',
      'ta","text":"to',
      'gether"}\ndata: {"type":"done"}\n',
    ]);
    expect(events).toEqual([
      { type: 'delta', text: 'together' },
      { type: 'done' },
    ]);
  });

  it('handles CRLF line endings and keep-alive comments', async () => {
    const events = await collect([
      ': ping\r\n\r\ndata: {"ok":true}\r\n\r\n',
    ]);
    expect(events).toEqual([{ ok: true }]);
  });

  it('yields a final event even when the stream ends without a newline', async () => {
    const events = await collect(['data: {"tail":1}']);
    expect(events).toEqual([{ tail: 1 }]);
  });

  it('decodes multi-byte characters split across chunk boundaries', async () => {
    const enc = new TextEncoder();
    const bytes = enc.encode('data: {"text":"héllo…"}\n');
    // Split in the middle of the é byte pair.
    const cut = 16;
    const stream = new ReadableStream({
      start(controller) {
        controller.enqueue(bytes.slice(0, cut));
        controller.enqueue(bytes.slice(cut));
        controller.close();
      },
    });
    const out = [];
    for await (const ev of sseJsonEvents(stream)) out.push(ev);
    expect(out).toEqual([{ text: 'héllo…' }]);
  });
});
