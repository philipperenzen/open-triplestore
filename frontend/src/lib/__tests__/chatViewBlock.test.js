// The ```view widget (bound to a query run of this turn, never carrying data of
// its own) and the row-bound ```chart's time-axis ordering.
import { describe, it, expect } from 'vitest';
import { parseChatBlocks, parseViewSpec, parseChartSpec } from '../chatRich.js';

const runs = [
  { sparql: 'q1', ok: true, columns: ['g', 'n'], rows: [['a', '1'], ['b', '2']], truncated: false },
  { sparql: 'q2', ok: false, error: 'boom' },
  { sparql: 'q3', ok: true, columns: ['el', 'wkt'], rows: [['https://ex.org/x', 'POINT(4 52)']], truncated: true },
];

describe('parseViewSpec', () => {
  it('binds to the last run with rows by default', () => {
    const r = parseViewSpec('{"source":"query"}', runs);
    expect(r.view).toMatchObject({ as: 'auto', queryIndex: 2, truncated: true });
    expect(r.view.table.columns).toEqual(['el', 'wkt']);
  });
  it('binds to the Nth round when asked', () => {
    const r = parseViewSpec('{"source":"query","query":1,"as":"bar","title":"Per graph","y":"n"}', runs);
    expect(r.view).toMatchObject({ as: 'bar', title: 'Per graph', y: 'n', queryIndex: 0 });
  });
  it('refuses a round that produced no rows', () => {
    expect(parseViewSpec('{"query":2}', runs)).toEqual({ error: 'query 2 produced no rows this turn' });
  });
  it('refuses when no query ran this turn', () => {
    expect(parseViewSpec('{"source":"query"}', [])).toHaveProperty('error');
    expect(parseViewSpec('{"source":"query"}', null)).toHaveProperty('error');
  });
  it('turns an unknown "as" into auto and tolerates lenient JSON', () => {
    expect(parseViewSpec('{"as":"sunburst",}', runs).view.as).toBe('auto');
  });
  it('rejects non-objects', () => {
    expect(parseViewSpec('nope', runs)).toEqual({ error: 'invalid JSON' });
    expect(parseViewSpec('[1]', runs)).toEqual({ error: 'not an object' });
  });
});

describe('parseChatBlocks ```view', () => {
  it('produces a view segment from the turn’s runs', () => {
    const segs = parseChatBlocks('Here:\n```view\n{"source":"query","as":"map"}\n```', { queryRuns: runs });
    expect(segs.map((s) => s.kind)).toEqual(['md', 'view']);
    expect(segs[1].view.as).toBe('map');
  });
  it('shows a broken card when there are no rows to bind', () => {
    const segs = parseChatBlocks('```view\n{"source":"query"}\n```', { queryRuns: [] });
    expect(segs[0]).toMatchObject({ kind: 'broken', label: 'view' });
  });
  it('is never sniffed from an untagged fence', () => {
    const segs = parseChatBlocks('```\n{"source":"query"}\n```', { queryRuns: runs });
    expect(segs.map((s) => s.kind)).toEqual(['md']);
  });
});

describe('row-bound ```chart on a time axis', () => {
  it('keeps chronological order instead of sorting by value', () => {
    const r = parseChartSpec('{"type":"line","source":"query","x":"year","y":"n"}', {
      columns: ['year', 'n'],
      rows: [['2021', '5'], ['2019', '9'], ['2020', '2']],
    });
    expect(r.spec.series[0].data.map((d) => d.label)).toEqual(['2019', '2020', '2021']);
  });
  it('still sorts categories by value', () => {
    const r = parseChartSpec('{"source":"query"}', { columns: ['g', 'n'], rows: [['a', '1'], ['b', '3']] });
    expect(r.spec.series[0].data.map((d) => d.label)).toEqual(['b', 'a']);
  });
  it('skips an empty value instead of charting 0', () => {
    const r = parseChartSpec('{"source":"query"}', { columns: ['g', 'n'], rows: [['a', '1'], ['b', '']] });
    expect(r.spec.series[0].data.map((d) => d.label)).toEqual(['a']);
  });
});
