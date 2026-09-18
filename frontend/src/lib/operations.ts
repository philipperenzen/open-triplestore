/**
 * Pure helpers behind the admin Operations page. They turn the three status
 * bodies — `GET /api/replication/status`, `GET /api/admin/telemetry` and
 * `GET /api/admin/changes/status` — into rows and words the page can show,
 * and know nothing about the DOM or fetch, so they are tested as functions.
 */

/** The exits a query can take, in the order the store tries them. */
export const SERVED_ORDER = ['cache_hit', 'fast_count', 'shards', 'columnar', 'full_copy', 'engine'] as const;
export type ServedKey = (typeof SERVED_ORDER)[number];

export interface ServedRow {
  key: string;
  count: number;
  /** `count / total`; 0 when there has been no traffic. */
  share: number;
}

/**
 * One row per exit, in try order, each with its share of the total. Every
 * known exit is present even at zero so the table keeps its shape; a key this
 * build does not know (a newer server) is appended rather than dropped.
 */
export function servedRows(byServed?: Record<string, number> | null): ServedRow[] {
  const src = byServed ?? {};
  const total = Object.values(src).reduce((a, b) => a + (Number(b) || 0), 0);
  const keys: string[] = [...SERVED_ORDER];
  for (const k of Object.keys(src)) if (!keys.includes(k)) keys.push(k);
  return keys.map((key) => {
    const count = Number(src[key]) || 0;
    return { key, count, share: total ? count / total : 0 };
  });
}

/** Replication in one word, for the badge and the sentence under it. */
export type ReplicationState = 'standalone' | 'leader' | 'in_sync' | 'catching_up' | 'stale' | 'error';

export interface ReplicationLike {
  role?: string;
  healthy?: boolean;
  lag_rows?: number | null;
  last_error?: string | null;
  last_sync_at?: string | null;
}

/**
 * - `standalone`: not replicating (role `none`).
 * - `leader`: serving followers.
 * - `in_sync`: a healthy follower with no rows outstanding.
 * - `catching_up`: healthy with rows outstanding, or a follower still on its
 *   first catch-up (nothing applied yet, nothing failed yet).
 * - `stale`: the last successful catch-up is older than three intervals.
 * - `error`: unhealthy and the last attempt failed — the message says why.
 */
export function replicationState(s?: ReplicationLike | null): ReplicationState {
  if (!s || !s.role || s.role === 'none') return 'standalone';
  if (s.role === 'leader') return 'leader';
  if (!s.healthy) {
    if (s.last_error) return 'error';
    return s.last_sync_at ? 'stale' : 'catching_up';
  }
  return s.lag_rows === 0 ? 'in_sync' : 'catching_up';
}

export interface GapBucket {
  label: string;
  /** Exclusive upper bound in ms; `null` for the open-ended last bucket. */
  upper_ms: number | null;
  count: number;
}

export interface GapRow {
  label: string;
  /** "< 100 ms", "100 ms – 500 ms", "≥ 5 s". */
  range: string;
  count: number;
  share: number;
}

/** The inter-write gap histogram as rows with a readable range each. */
export function gapRows(gaps?: GapBucket[] | null): GapRow[] {
  const src = gaps ?? [];
  const total = src.reduce((a, g) => a + (Number(g.count) || 0), 0);
  let lower: number | null = null;
  return src.map((g) => {
    const upper = g.upper_ms ?? null;
    let range: string;
    if (upper === null) range = lower === null ? 'all' : `≥ ${fmtMillis(lower)}`;
    else if (lower === null) range = `< ${fmtMillis(upper)}`;
    else range = `${fmtMillis(lower)} – ${fmtMillis(upper)}`;
    lower = upper;
    const count = Number(g.count) || 0;
    return { label: g.label, range, count, share: total ? count / total : 0 };
  });
}

/** The lowest live cursor — retention on a leader never sweeps above it. */
export function lowestCursor(cursors?: { seq: number }[] | null): number | null {
  const seqs = (cursors ?? []).map((c) => Number(c.seq)).filter((n) => Number.isFinite(n));
  return seqs.length ? Math.min(...seqs) : null;
}

function missing(v: unknown): boolean {
  return v === null || v === undefined || typeof v !== 'number' || !Number.isFinite(v);
}

/** Microseconds as "41 µs", "18.3 ms" or "1.2 s"; an em dash when absent. */
export function fmtMicros(us?: number | null): string {
  if (missing(us)) return '—';
  const v = us as number;
  if (v >= 1_000_000) return `${(v / 1_000_000).toFixed(v >= 10_000_000 ? 0 : 1)} s`;
  if (v >= 1000) return `${(v / 1000).toFixed(v >= 10_000 ? 0 : 1)} ms`;
  return `${Math.round(v)} µs`;
}

/** Milliseconds as "100 ms", "1.5 s" or "2 min". */
export function fmtMillis(ms?: number | null): string {
  if (missing(ms)) return '—';
  const v = ms as number;
  if (v >= 60_000) return `${(v / 60_000).toFixed(v % 60_000 ? 1 : 0)} min`;
  if (v >= 1000) return `${(v / 1000).toFixed(v % 1000 ? 1 : 0)} s`;
  return `${Math.round(v)} ms`;
}

/** Bytes as "512 B", "12.4 KB", "3.1 MB" (powers of 1024). */
export function fmtBytes(b?: number | null): string {
  if (missing(b)) return '—';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = b as number;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${Math.round(v)} B` : `${v.toFixed(1)} ${units[i]}`;
}

/** Seconds of uptime as "3d 4h", "2h 05m", "4m 09s" or "45 s". */
export function fmtUptime(secs?: number | null): string {
  if (missing(secs)) return '—';
  const s = Math.max(0, Math.floor(secs as number));
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${String(m).padStart(2, '0')}m`;
  if (m > 0) return `${m}m ${String(s % 60).padStart(2, '0')}s`;
  return `${s} s`;
}

/**
 * "12 seconds ago" in the viewer's language (`Intl.RelativeTimeFormat`);
 * an em dash when there is no time. `now` is injectable for tests and for a
 * page that re-renders on a tick without refetching.
 */
export function relativeTime(iso?: string | null, now: number = Date.now(), locale = 'en'): string {
  if (!iso) return '—';
  const t = Date.parse(iso);
  if (!Number.isFinite(t)) return '—';
  const diff = Math.round((t - now) / 1000); // negative = in the past
  const abs = Math.abs(diff);
  let rtf: Intl.RelativeTimeFormat;
  try {
    rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'always' });
  } catch {
    rtf = new Intl.RelativeTimeFormat('en', { numeric: 'always' });
  }
  if (abs < 60) return rtf.format(diff, 'second');
  if (abs < 3600) return rtf.format(Math.round(diff / 60), 'minute');
  if (abs < 86400) return rtf.format(Math.round(diff / 3600), 'hour');
  return rtf.format(Math.round(diff / 86400), 'day');
}

/** A share (0–1) as "23 %", with one decimal below ten ("3.4 %"). */
export function pct(share?: number | null): string {
  if (missing(share)) return '—';
  const v = (share as number) * 100;
  return `${v >= 10 || v === 0 ? Math.round(v) : v.toFixed(1)} %`;
}

/** The first eight characters of an epoch or id, for a label; whole when short. */
export function shortId(id?: string | null): string {
  if (!id) return '—';
  return id.length > 12 ? `${id.slice(0, 8)}…` : id;
}
