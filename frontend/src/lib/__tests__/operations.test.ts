import { describe, it, expect } from 'vitest';
import {
  SERVED_ORDER,
  servedRows,
  replicationState,
  gapRows,
  lowestCursor,
  fmtMicros,
  fmtMillis,
  fmtBytes,
  fmtUptime,
  relativeTime,
  pct,
  shortId,
} from '../operations';

describe('servedRows', () => {
  it('lists every exit in try order, at zero when the server has no count', () => {
    const rows = servedRows({ engine: 3 });
    expect(rows.map((r) => r.key)).toEqual([...SERVED_ORDER]);
    expect(rows.find((r) => r.key === 'engine')).toEqual({ key: 'engine', count: 3, share: 1 });
    expect(rows.find((r) => r.key === 'cache_hit')).toEqual({ key: 'cache_hit', count: 0, share: 0 });
  });

  it('shares sum to one over the total of all exits', () => {
    const rows = servedRows({ cache_hit: 60, shards: 20, columnar: 20 });
    const sum = rows.reduce((a, r) => a + r.share, 0);
    expect(sum).toBeCloseTo(1, 10);
    expect(rows.find((r) => r.key === 'cache_hit')?.share).toBeCloseTo(0.6, 10);
  });

  it('keeps an exit this build does not know, after the known ones', () => {
    const rows = servedRows({ engine: 1, future_exit: 4 });
    expect(rows[rows.length - 1]).toEqual({ key: 'future_exit', count: 4, share: 0.8 });
  });

  it('is all zeros with no traffic and no body', () => {
    expect(servedRows(null).every((r) => r.count === 0 && r.share === 0)).toBe(true);
    expect(servedRows({}).length).toBe(SERVED_ORDER.length);
  });
});

describe('replicationState', () => {
  it('is standalone without a body or with role none', () => {
    expect(replicationState(null)).toBe('standalone');
    expect(replicationState({ role: 'none', healthy: true })).toBe('standalone');
  });

  it('is leader for a leader, whatever its health flag says', () => {
    expect(replicationState({ role: 'leader', healthy: true })).toBe('leader');
  });

  it('separates in sync from catching up by the rows behind', () => {
    const base = { role: 'follower', healthy: true, last_sync_at: '2026-09-18T10:00:00Z' };
    expect(replicationState({ ...base, lag_rows: 0 })).toBe('in_sync');
    expect(replicationState({ ...base, lag_rows: 12 })).toBe('catching_up');
    // The leader has not been asked yet, so the lag is unknown: not "in sync".
    expect(replicationState({ ...base, lag_rows: null })).toBe('catching_up');
  });

  it('reads an unhealthy follower as error, stale, or a first catch-up', () => {
    const base = { role: 'follower', healthy: false, lag_rows: null };
    expect(replicationState({ ...base, last_error: '401 Unauthorized' })).toBe('error');
    expect(replicationState({ ...base, last_error: null, last_sync_at: '2026-09-18T09:00:00Z' })).toBe('stale');
    // Nothing applied, nothing failed: the bootstrap is still running.
    expect(replicationState({ ...base, last_error: null, last_sync_at: null })).toBe('catching_up');
  });
});

describe('gapRows', () => {
  const gaps = [
    { label: 'lt_100ms', upper_ms: 100, count: 70 },
    { label: 'lt_500ms', upper_ms: 500, count: 20 },
    { label: 'lt_5s', upper_ms: 5000, count: 5 },
    { label: 'ge_5s', upper_ms: null, count: 5 },
  ];

  it('turns bucket bounds into readable ranges', () => {
    expect(gapRows(gaps).map((r) => r.range)).toEqual(['< 100 ms', '100 ms – 500 ms', '500 ms – 5 s', '≥ 5 s']);
  });

  it('keeps labels and counts and computes shares of the total', () => {
    const rows = gapRows(gaps);
    expect(rows[0]).toMatchObject({ label: 'lt_100ms', count: 70, share: 0.7 });
    expect(rows[3].share).toBeCloseTo(0.05, 10);
  });

  it('handles a single open-ended bucket and no buckets', () => {
    expect(gapRows([{ label: 'all', upper_ms: null, count: 0 }])[0]).toMatchObject({ range: 'all', share: 0 });
    expect(gapRows(null)).toEqual([]);
  });
});

describe('lowestCursor', () => {
  it('is the smallest sequence number, or null without cursors', () => {
    expect(lowestCursor([{ seq: 40 }, { seq: 12 }, { seq: 99 }])).toBe(12);
    expect(lowestCursor([])).toBeNull();
    expect(lowestCursor(undefined)).toBeNull();
  });
});

describe('formatting', () => {
  it('fmtMicros picks the unit by magnitude', () => {
    expect(fmtMicros(41)).toBe('41 µs');
    expect(fmtMicros(18_300)).toBe('18 ms');
    expect(fmtMicros(2_210)).toBe('2.2 ms');
    expect(fmtMicros(1_200_000)).toBe('1.2 s');
    expect(fmtMicros(null)).toBe('—');
    expect(fmtMicros(undefined)).toBe('—');
  });

  it('fmtMillis drops a needless decimal', () => {
    expect(fmtMillis(100)).toBe('100 ms');
    expect(fmtMillis(1500)).toBe('1.5 s');
    expect(fmtMillis(5000)).toBe('5 s');
    expect(fmtMillis(120_000)).toBe('2 min');
  });

  it('fmtBytes uses powers of 1024', () => {
    expect(fmtBytes(512)).toBe('512 B');
    expect(fmtBytes(12_697)).toBe('12.4 KB');
    expect(fmtBytes(3.1 * 1024 * 1024)).toBe('3.1 MB');
    expect(fmtBytes(null)).toBe('—');
  });

  it('fmtUptime shows the two largest units', () => {
    expect(fmtUptime(45)).toBe('45 s');
    expect(fmtUptime(4 * 60 + 9)).toBe('4m 09s');
    expect(fmtUptime(2 * 3600 + 5 * 60)).toBe('2h 05m');
    expect(fmtUptime(3 * 86400 + 4 * 3600 + 59 * 60)).toBe('3d 4h');
    expect(fmtUptime(-3)).toBe('0 s');
  });

  it('relativeTime speaks the viewer’s language from an injected now', () => {
    const now = Date.parse('2026-09-18T10:00:00Z');
    expect(relativeTime('2026-09-18T09:59:48Z', now, 'en')).toBe('12 seconds ago');
    expect(relativeTime('2026-09-18T09:30:00Z', now, 'en')).toBe('30 minutes ago');
    expect(relativeTime('2026-09-18T07:00:00Z', now, 'en')).toBe('3 hours ago');
    expect(relativeTime('2026-09-16T10:00:00Z', now, 'en')).toBe('2 days ago');
    expect(relativeTime('2026-09-18T09:59:48Z', now, 'nl')).toBe('12 seconden geleden');
    expect(relativeTime(null, now)).toBe('—');
    expect(relativeTime('not a date', now)).toBe('—');
  });

  it('relativeTime falls back to English for a locale Intl does not know', () => {
    const now = Date.parse('2026-09-18T10:00:00Z');
    expect(relativeTime('2026-09-18T09:59:00Z', now, 'x-not-a-locale')).toBe('1 minute ago');
  });

  it('pct rounds above ten and keeps a decimal below', () => {
    expect(pct(0.234)).toBe('23 %');
    expect(pct(0.034)).toBe('3.4 %');
    expect(pct(0)).toBe('0 %');
    expect(pct(1)).toBe('100 %');
    expect(pct(null)).toBe('—');
  });

  it('shortId trims long ids for a label and leaves short ones alone', () => {
    expect(shortId('9d1c0f3a7b2e4d5c6f7a8b9c')).toBe('9d1c0f3a…');
    expect(shortId('replica-1')).toBe('replica-1');
    expect(shortId(null)).toBe('—');
  });
});
