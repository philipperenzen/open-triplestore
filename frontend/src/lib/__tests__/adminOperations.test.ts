/**
 * The Operations page against the three status bodies it reads, with the
 * API mocked: the words a leader, a broken follower and a store with capture
 * off should see, and the exits table's shape.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { user, authInitialized } from '../stores';

const api = vi.hoisted(() => ({
  replicationStatus: vi.fn(),
  adminTelemetry: vi.fn(),
  adminChangesStatus: vi.fn(),
}));
vi.mock('../api.js', () => api);

import AdminOperations from '../../pages/AdminOperations.svelte';

const LEADER = {
  role: 'leader', configured_role: 'leader', mode: 'warm', scope: 'all', leader_url: null,
  node_id: 'leader-1', read_only: false, epoch: null, applied_seq: 0, leader_newest_seq: null,
  lag_rows: null, last_sync_at: null, last_error: null, applied_rows: 0, refetched_graphs: 0,
  resyncs: 0, interval_secs: 60, healthy: true,
};

const FOLLOWER_WITHOUT_TOKEN = {
  ...LEADER, role: 'follower', configured_role: 'follower', mode: 'hot', node_id: 'replica-1',
  leader_url: 'http://leader:7878', read_only: true, interval_secs: 0.5, healthy: false,
  last_sync_at: '2026-09-18T10:00:00Z', last_error: 'http://leader:7878/api/replication/manifest: HTTP 401',
};

const TELEMETRY = {
  uptime_secs: 3725,
  queries: {
    total: 1234, window: 154,
    by_served: { cache_hit: 900, fast_count: 10, shards: 100, columnar: 200, full_copy: 4, engine: 20 },
    aggregate_text: 110,
    analytical: { count: 30, share: 0.2, p50_us: 41, p95_us: 18300, p99_us: 91000, max_us: 402113, by_served: {} },
    other: { count: 124, share: 0.8, p50_us: 37, p95_us: 2210, p99_us: 14400, max_us: 88000, by_served: {} },
  },
  validations: {
    total: 3, window: 3, by_path: { gate: 2, dataset: 1 }, by_source: { live: 3 },
    with_run_index: 1, p50_ms: 14, p95_ms: 2210, max_ms: 7150, max_quads: 180000,
  },
  writes: {
    total: 12,
    gaps: [
      { label: 'lt_100ms', upper_ms: 100, count: 9 },
      { label: 'ge_100ms', upper_ms: null, count: 3 },
    ],
  },
};

const CAPTURING = {
  enabled: true, epoch: 'df30ae76-5ec7-446d-aa82-a1a7697c453a', next_seq: 491, rows: 490,
  committed: 439, pending: 0, unknown: 51, oldest_seq: 1, newest_seq: 490,
  cursors: [{ name: 'replica-1', seq: 480, owner: 'admin', expires_at: '2026-10-18T10:00:00Z', updated_at: '2026-09-18T10:00:00Z' }],
  max_scan: 250000, max_payload: 250000, retention_days: 90, size_bytes: 34205696,
};

const OFF = { ...CAPTURING, enabled: false, rows: 0, committed: 0, unknown: 0, oldest_seq: null, newest_seq: null, cursors: [], size_bytes: 0 };

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  user.set({ id: 'u1', username: 'admin', role: 'super_admin' } as never);
  authInitialized.set(true);
});

function mount(replication: unknown, telemetry: unknown, changes: unknown) {
  api.replicationStatus.mockResolvedValue(replication);
  api.adminTelemetry.mockResolvedValue(telemetry);
  api.adminChangesStatus.mockResolvedValue(changes);
  return render(AdminOperations);
}

describe('AdminOperations', () => {
  it('names a leader, lists every exit in try order, and shows its follower’s cursor', async () => {
    const { findByText, container, unmount } = mount(LEADER, TELEMETRY, CAPTURING);
    await findByText('Leader');
    expect(api.replicationStatus).toHaveBeenCalledTimes(1);
    expect(api.adminTelemetry).toHaveBeenCalledTimes(1);
    expect(api.adminChangesStatus).toHaveBeenCalledTimes(1);

    const exits = [...container.querySelectorAll('td code')].map((c) => c.textContent);
    expect(exits.slice(0, 6)).toEqual(['cache_hit', 'fast_count', 'shards', 'columnar', 'full_copy', 'engine']);
    expect(container.textContent).toContain('the cached result is returned');

    // Capturing, with the cursor table.
    expect(container.textContent).toContain('Capturing');
    expect(container.textContent).toContain('replica-1');
    // Retention floor is the lowest cursor.
    expect(container.textContent).toContain('lowest one (480)');
    // 33.4 MB on disk; the uptime in the queries header.
    expect(container.textContent).toContain('32.6 MB');
    expect(container.textContent).toContain('Up 1h 02m');
    unmount();
  });

  it('shows a follower that cannot reach its leader as an error, with the message', async () => {
    const { findByText, container, unmount } = mount(FOLLOWER_WITHOUT_TOKEN, TELEMETRY, OFF);
    await findByText('Error');
    expect(container.textContent).toContain('HTTP 401');
    expect(container.textContent).toContain('The last catch-up failed');
    expect(container.textContent).toContain('read-only');
    expect(container.textContent).toContain('long-polls the leader');
    unmount();
  });

  it('explains why capture is off and how to turn it on', async () => {
    const { findByText, container, unmount } = mount({ ...LEADER, role: 'none', configured_role: 'none' }, TELEMETRY, OFF);
    await findByText('Not replicating');
    expect(container.textContent).toContain('Off');
    expect(container.textContent).toContain('×2.5–4');
    expect(container.textContent).toContain('OTS_CHANGE_CAPTURE=on');
    expect(container.textContent).toContain('No consumer has bookmarked this log yet.');
    unmount();
  });

  it('keeps the other cards when one call fails', async () => {
    api.replicationStatus.mockResolvedValue(LEADER);
    api.adminTelemetry.mockRejectedValue(new Error('Forbidden'));
    api.adminChangesStatus.mockResolvedValue(CAPTURING);
    const { findByText, container, unmount } = render(AdminOperations);
    await findByText('Leader');
    expect(container.textContent).toContain('Forbidden');
    expect(container.textContent).toContain('Capturing');
    unmount();
  });

  it('refreshes every five seconds, not while paused, and at once on resume', async () => {
    vi.useFakeTimers();
    try {
      const { getByRole, unmount } = mount(LEADER, TELEMETRY, CAPTURING);
      await vi.advanceTimersByTimeAsync(0);
      expect(api.replicationStatus).toHaveBeenCalledTimes(1);

      await vi.advanceTimersByTimeAsync(5_000);
      expect(api.replicationStatus).toHaveBeenCalledTimes(2);

      await fireEvent.click(getByRole('button', { name: /Pause/ }));
      await vi.advanceTimersByTimeAsync(10_000);
      expect(api.replicationStatus).toHaveBeenCalledTimes(2);

      await fireEvent.click(getByRole('button', { name: /Resume/ }));
      await vi.advanceTimersByTimeAsync(0);
      expect(api.replicationStatus).toHaveBeenCalledTimes(3);

      unmount();
      await vi.advanceTimersByTimeAsync(10_000);
      expect(api.replicationStatus).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
    }
  });

  it('sends a non-admin away', async () => {
    user.set({ id: 'u2', username: 'bob', role: 'user' } as never);
    const { container, unmount } = mount(LEADER, TELEMETRY, CAPTURING);
    await new Promise((r) => setTimeout(r, 0));
    expect(api.replicationStatus).not.toHaveBeenCalled();
    expect(container.querySelector('.card')).toBeNull();
    unmount();
  });
});
