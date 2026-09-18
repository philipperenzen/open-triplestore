<script>
  import { onDestroy } from 'svelte';
  import { t, locale } from 'svelte-i18n';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { navigate, Link } from '../lib/router/index.js';
  import { Gauge, Loader2, RefreshCw, Pause, Play, Radio, ScrollText, CircleAlert, CheckCircle2 } from 'lucide-svelte';
  import { replicationStatus, adminTelemetry, adminChangesStatus } from '../lib/api.js';
  import {
    SERVED_ORDER, servedRows, replicationState, gapRows, lowestCursor,
    fmtMicros, fmtMillis, fmtBytes, fmtUptime, relativeTime, pct, shortId,
  } from '../lib/operations.js';

  /** The three bodies are cheap to serve — counters and a few small reads. */
  const REFRESH_MS = 5000;

  let replication = null;
  let telemetry = null;
  let changes = null;
  let errors = { replication: null, telemetry: null, changes: null };
  let loading = false;
  let paused = false;
  let updatedAt = null;
  let now = Date.now();
  let timer = null;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
    else {
      refresh();
      start();
    }
  }

  async function refresh() {
    if (loading) return;
    loading = true;
    const [r, tm, c] = await Promise.allSettled([replicationStatus(), adminTelemetry(), adminChangesStatus()]);
    const take = (res, key) => {
      if (res.status === 'fulfilled') {
        errors[key] = null;
        return res.value;
      }
      errors[key] = res.reason?.message || String(res.reason);
      return null;
    };
    replication = take(r, 'replication') ?? replication;
    telemetry = take(tm, 'telemetry') ?? telemetry;
    changes = take(c, 'changes') ?? changes;
    errors = errors;
    updatedAt = Date.now();
    now = updatedAt;
    loading = false;
  }

  function start() {
    stop();
    timer = setInterval(() => {
      now = Date.now();
      if (!paused && !document.hidden) refresh();
    }, REFRESH_MS);
  }
  function stop() {
    if (timer) clearInterval(timer);
    timer = null;
  }
  function togglePause() {
    paused = !paused;
    if (!paused) refresh();
  }
  onDestroy(stop);

  const KNOWN_EXITS = new Set(SERVED_ORDER);
  const n = (v) => (v === null || v === undefined ? '—' : Number(v).toLocaleString());
  const ago = (iso) => relativeTime(iso, now, lang);

  $: lang = $locale || 'en';
  $: state = replicationState(replication);
  $: served = servedRows(telemetry?.queries?.by_served);
  $: gaps = gapRows(telemetry?.writes?.gaps);
  $: floor = lowestCursor(changes?.cursors);
  $: latencyKinds = telemetry
    ? [
        ['analytical', telemetry.queries?.analytical],
        ['other', telemetry.queries?.other],
      ]
    : [];
  $: isFollower = replication?.role === 'follower';
  $: modeHint = replication?.mode ? $t(`pages.adminOperations.replication.modeHint.${replication.mode}`, { default: '' }) : '';
</script>

<div class="admin-ops">
  <div class="header-row">
    <div>
      <h2><Gauge size={20} /> {$t('pages.adminOperations.title')}</h2>
      <p class="subtitle">{$t('pages.adminOperations.detail')}</p>
    </div>
    <div class="controls">
      {#if updatedAt}
        <span class="updated" title={$t('pages.adminOperations.autoRefresh')}>
          {$t('pages.adminOperations.updated', { values: { when: ago(new Date(updatedAt).toISOString()) } })}
        </span>
      {/if}
      <button class="btn btn-sm" on:click={togglePause} aria-pressed={paused}>
        {#if paused}<Play size={14} /> {$t('pages.adminOperations.resume')}{:else}<Pause size={14} /> {$t('pages.adminOperations.pause')}{/if}
      </button>
      <button class="btn btn-sm" on:click={refresh} disabled={loading}>
        {#if loading}<Loader2 size={14} class="animate-spin" />{:else}<RefreshCw size={14} />{/if}
        {$t('pages.adminOperations.refresh')}
      </button>
    </div>
  </div>

  {#if !replication && !telemetry && !changes && !errors.replication && !errors.telemetry && !errors.changes}
    <div class="loading"><Loader2 size={24} class="animate-spin" /> {$t('system.loading')}</div>
  {/if}

  <!-- ── Replication ─────────────────────────────────────────────────── -->
  {#if replication || errors.replication}
    <section class="card">
      <header class="card-head">
        <h3><Radio size={16} /> {$t('pages.adminOperations.replication.title')}</h3>
        {#if replication}
          <span class="state-badge state-{state}">{$t(`pages.adminOperations.replication.state.${state}`)}</span>
        {/if}
      </header>
      {#if errors.replication}
        <p class="card-error"><CircleAlert size={14} /> {errors.replication}</p>
      {:else}
        <p class="explain">{$t(`pages.adminOperations.replication.explain.${state}`)}</p>
        {#if replication.last_error}
          <p class="card-error"><CircleAlert size={14} /> {replication.last_error}</p>
        {/if}
        <dl class="facts">
          <div>
            <dt>{$t('pages.adminOperations.replication.role')}</dt>
            <dd>
              <code>{replication.role}</code>
              {#if replication.configured_role && replication.configured_role !== replication.role}
                <span class="muted">({$t('pages.adminOperations.replication.configuredRole', { values: { role: replication.configured_role } })})</span>
              {/if}
              {#if replication.read_only}<span class="chip">{$t('pages.adminOperations.replication.readOnly')}</span>{/if}
            </dd>
          </div>
          {#if state !== 'standalone'}
            <div>
              <dt>{$t('pages.adminOperations.replication.mode')}</dt>
              <dd><code>{replication.mode}</code>{#if modeHint} <span class="muted">— {modeHint}</span>{/if}</dd>
            </div>
            <div><dt>{$t('pages.adminOperations.replication.scope')}</dt><dd><code>{replication.scope}</code></dd></div>
            <div><dt>{$t('pages.adminOperations.replication.node')}</dt><dd><code>{replication.node_id}</code></dd></div>
          {/if}
          {#if isFollower}
            <div><dt>{$t('pages.adminOperations.replication.leaderUrl')}</dt><dd><code>{replication.leader_url || '—'}</code></dd></div>
            <div><dt>{$t('pages.adminOperations.replication.epoch')}</dt><dd><code title={replication.epoch || ''}>{shortId(replication.epoch)}</code></dd></div>
            <div>
              <dt>{$t('pages.adminOperations.replication.applied')}</dt>
              <dd>{n(replication.applied_seq)} / {n(replication.leader_newest_seq)}</dd>
            </div>
            <div>
              <dt>{$t('pages.adminOperations.replication.lag')}</dt>
              <dd class:emph={replication.lag_rows > 0}>{n(replication.lag_rows)}</dd>
            </div>
            <div>
              <dt>{$t('pages.adminOperations.replication.lastSync')}</dt>
              <dd title={replication.last_sync_at || ''}>{ago(replication.last_sync_at)}</dd>
            </div>
            <div>
              <dt>{$t('pages.adminOperations.replication.interval')}</dt>
              <dd>{fmtMillis(replication.interval_secs * 1000)}</dd>
            </div>
            <div><dt>{$t('pages.adminOperations.replication.appliedRows')}</dt><dd>{n(replication.applied_rows)}</dd></div>
            <div><dt>{$t('pages.adminOperations.replication.refetched')}</dt><dd>{n(replication.refetched_graphs)}</dd></div>
            <div><dt>{$t('pages.adminOperations.replication.resyncs')}</dt><dd>{n(replication.resyncs)}</dd></div>
            {#if replication.identity}
              <div>
                <dt>{$t('pages.adminOperations.replication.identity')}</dt>
                <dd>
                  {#if replication.identity.applied_version !== null && replication.identity.applied_version !== undefined}
                    {$t('pages.adminOperations.replication.identityApplied', { values: { version: replication.identity.applied_version, when: ago(replication.identity.applied_at) } })}
                  {:else}
                    —
                  {/if}
                  {#if replication.identity.last_error}<span class="inline-error">{replication.identity.last_error}</span>{/if}
                </dd>
              </div>
            {/if}
          {/if}
        </dl>

        {#if replication.sync}
          <h4>{$t('pages.adminOperations.replication.sync.title')}</h4>
          <dl class="facts">
            <div>
              <dt>{$t('pages.adminOperations.replication.sync.followers')}</dt>
              <dd>
                {$t('pages.adminOperations.replication.sync.acked', { values: { acked: replication.sync.acked?.length ?? 0, followers: replication.sync.followers?.length ?? 0, required: replication.sync.required } })}
                <span class="muted">— {(replication.sync.followers || []).join(', ')}</span>
              </dd>
            </div>
            <div><dt>{$t('pages.adminOperations.replication.sync.lastConfirmed')}</dt><dd>{n(replication.sync.last_confirmed_seq)}</dd></div>
            <div>
              <dt>{$t('pages.adminOperations.replication.sync.waits')}</dt>
              <dd>{n(replication.sync.waits)} <span class="muted">/ {$t('pages.adminOperations.replication.sync.degraded', { values: { n: n(replication.sync.degraded_waits) } })}</span></dd>
            </div>
            {#if replication.sync.degraded_since}
              <div>
                <dt>{$t('pages.adminOperations.replication.sync.degradedSince')}</dt>
                <dd class="emph" title={replication.sync.degraded_since}>{ago(replication.sync.degraded_since)}</dd>
              </div>
            {/if}
          </dl>
        {/if}

        {#if replication.cluster}
          <h4>{$t('pages.adminOperations.replication.cluster.title')}</h4>
          <dl class="facts">
            <div><dt>{$t('pages.adminOperations.replication.cluster.leader')}</dt><dd><code>{replication.cluster.leader ?? '—'}</code> <span class="muted">{$t('pages.adminOperations.replication.cluster.term', { values: { term: replication.cluster.term } })}</span></dd></div>
            <div><dt>{$t('pages.adminOperations.replication.cluster.state')}</dt><dd><code>{replication.cluster.state}</code></dd></div>
            <div>
              <dt>{$t('pages.adminOperations.replication.cluster.members')}</dt>
              <dd>
                {#each Object.entries(replication.cluster.members || {}) as [id, st]}
                  <span class="chip"><code>{id}</code> {st}</span>
                {/each}
              </dd>
            </div>
          </dl>
        {/if}

        <p class="docs-line">
          <Link to="/docs/operations">{$t('pages.adminOperations.replication.docs')}</Link>
        </p>
      {/if}
    </section>
  {/if}

  <!-- ── Queries ─────────────────────────────────────────────────────── -->
  {#if telemetry || errors.telemetry}
    <section class="card">
      <header class="card-head">
        <h3><Gauge size={16} /> {$t('pages.adminOperations.queries.title')}</h3>
        {#if telemetry}
          <span class="muted">{$t('pages.adminOperations.uptime', { values: { uptime: fmtUptime(telemetry.uptime_secs) } })}</span>
        {/if}
      </header>
      {#if errors.telemetry}
        <p class="card-error"><CircleAlert size={14} /> {errors.telemetry}</p>
      {:else}
        <div class="stats-row">
          <div class="stat-card">
            <span class="stat-value">{n(telemetry.queries?.total)}</span>
            <span class="stat-label">{$t('pages.adminOperations.queries.total')}</span>
          </div>
          <div class="stat-card">
            <span class="stat-value">{n(telemetry.queries?.window)}</span>
            <span class="stat-label">{$t('pages.adminOperations.queries.timed')}</span>
          </div>
          <div class="stat-card">
            <span class="stat-value">{pct(telemetry.queries?.analytical?.share)}</span>
            <span class="stat-label">{$t('pages.adminOperations.queries.analyticalShare')}</span>
          </div>
          <div class="stat-card">
            <span class="stat-value">{fmtMicros(telemetry.queries?.other?.p95_us)}</span>
            <span class="stat-label">{$t('pages.adminOperations.queries.p95')}</span>
          </div>
        </div>

        <h4>{$t('pages.adminOperations.queries.exitsTitle')}</h4>
        <p class="explain">{$t('pages.adminOperations.queries.exitsExplain')}</p>
        <table class="data-table">
          <thead>
            <tr>
              <th>{$t('pages.adminOperations.queries.colExit')}</th>
              <th>{$t('pages.adminOperations.queries.colMeaning')}</th>
              <th class="num">{$t('pages.adminOperations.queries.colCount')}</th>
              <th class="share-col">{$t('pages.adminOperations.queries.colShare')}</th>
            </tr>
          </thead>
          <tbody>
            {#each served as row (row.key)}
              <tr>
                <td class="nowrap"><code>{row.key}</code></td>
                <td class="meaning">{KNOWN_EXITS.has(row.key) ? $t(`pages.adminOperations.queries.exit.${row.key}`) : ''}</td>
                <td class="num">{n(row.count)}</td>
                <td class="share-col">
                  <span class="bar" style={`--w:${Math.round(row.share * 100)}%`}></span>
                  <span class="share-text">{pct(row.share)}</span>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>

        <h4>{$t('pages.adminOperations.queries.latencyTitle')}</h4>
        <p class="explain">{$t('pages.adminOperations.queries.latencyExplain')}</p>
        <table class="data-table">
          <thead>
            <tr>
              <th>{$t('pages.adminOperations.queries.colKind')}</th>
              <th class="num">{$t('pages.adminOperations.queries.colSampled')}</th>
              <th class="num">{$t('pages.adminOperations.queries.colShare')}</th>
              <th class="num">p50</th>
              <th class="num">p95</th>
              <th class="num">p99</th>
              <th class="num">{$t('pages.adminOperations.queries.colMax')}</th>
            </tr>
          </thead>
          <tbody>
            {#each latencyKinds as [kind, l] (kind)}
              <tr>
                <td>{$t(`pages.adminOperations.queries.${kind}`)}</td>
                <td class="num">{n(l?.count)}</td>
                <td class="num">{pct(l?.share)}</td>
                <td class="num">{fmtMicros(l?.p50_us)}</td>
                <td class="num">{fmtMicros(l?.p95_us)}</td>
                <td class="num">{fmtMicros(l?.p99_us)}</td>
                <td class="num">{fmtMicros(l?.max_us)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </section>

    <!-- ── Validations and writes ────────────────────────────────────── -->
    {#if telemetry}
      <div class="two-up">
        <section class="card">
          <header class="card-head">
            <h3><CheckCircle2 size={16} /> {$t('pages.adminOperations.validations.title')}</h3>
          </header>
          <p class="explain">{$t('pages.adminOperations.validations.explain')}</p>
          <div class="stats-row">
            <div class="stat-card">
              <span class="stat-value">{n(telemetry.validations?.total)}</span>
              <span class="stat-label">{$t('pages.adminOperations.validations.total')}</span>
            </div>
            <div class="stat-card">
              <span class="stat-value">{fmtMillis(telemetry.validations?.p50_ms)}</span>
              <span class="stat-label">p50</span>
            </div>
            <div class="stat-card">
              <span class="stat-value">{fmtMillis(telemetry.validations?.p95_ms)}</span>
              <span class="stat-label">p95</span>
            </div>
            <div class="stat-card">
              <span class="stat-value">{fmtMillis(telemetry.validations?.max_ms)}</span>
              <span class="stat-label">{$t('pages.adminOperations.validations.max')}</span>
            </div>
          </div>
          <dl class="facts">
            <div>
              <dt>{$t('pages.adminOperations.validations.byPath')}</dt>
              <dd>
                {#each Object.entries(telemetry.validations?.by_path || {}) as [k, v]}
                  <span class="chip"><code>{k}</code> {n(v)}</span>
                {:else}—{/each}
              </dd>
            </div>
            <div>
              <dt>{$t('pages.adminOperations.validations.bySource')}</dt>
              <dd>
                {#each Object.entries(telemetry.validations?.by_source || {}) as [k, v]}
                  <span class="chip"><code>{k}</code> {n(v)}</span>
                {:else}—{/each}
              </dd>
            </div>
            <div><dt>{$t('pages.adminOperations.validations.withRunIndex')}</dt><dd>{n(telemetry.validations?.with_run_index)}</dd></div>
            <div><dt>{$t('pages.adminOperations.validations.maxQuads')}</dt><dd>{n(telemetry.validations?.max_quads)}</dd></div>
          </dl>
        </section>

        <section class="card">
          <header class="card-head">
            <h3><RefreshCw size={16} /> {$t('pages.adminOperations.writes.title')}</h3>
            <span class="muted">{$t('pages.adminOperations.writes.total', { values: { n: n(telemetry.writes?.total) } })}</span>
          </header>
          <p class="explain">{$t('pages.adminOperations.writes.explain')}</p>
          {#if gaps.length}
            <table class="data-table">
              <thead>
                <tr>
                  <th>{$t('pages.adminOperations.writes.colGap')}</th>
                  <th class="num">{$t('pages.adminOperations.writes.colCount')}</th>
                  <th class="share-col">{$t('pages.adminOperations.writes.colShare')}</th>
                </tr>
              </thead>
              <tbody>
                {#each gaps as g (g.label)}
                  <tr>
                    <td class="nowrap">{g.range}</td>
                    <td class="num">{n(g.count)}</td>
                    <td class="share-col">
                      <span class="bar" style={`--w:${Math.round(g.share * 100)}%`}></span>
                      <span class="share-text">{pct(g.share)}</span>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {:else}
            <p class="muted">{$t('pages.adminOperations.writes.none')}</p>
          {/if}
        </section>
      </div>
    {/if}
  {/if}

  <!-- ── Change log ──────────────────────────────────────────────────── -->
  {#if changes || errors.changes}
    <section class="card">
      <header class="card-head">
        <h3><ScrollText size={16} /> {$t('pages.adminOperations.changes.title')}</h3>
        {#if changes}
          <span class="state-badge {changes.enabled ? 'state-in_sync' : 'state-standalone'}">
            {changes.enabled ? $t('pages.adminOperations.changes.on') : $t('pages.adminOperations.changes.off')}
          </span>
        {/if}
      </header>
      {#if errors.changes}
        <p class="card-error"><CircleAlert size={14} /> {errors.changes}</p>
      {:else}
        <p class="explain">{changes.enabled ? $t('pages.adminOperations.changes.onExplain') : $t('pages.adminOperations.changes.offExplain')}</p>
        <dl class="facts">
          <div><dt>{$t('pages.adminOperations.changes.epoch')}</dt><dd><code title={changes.epoch}>{shortId(changes.epoch)}</code></dd></div>
          <div><dt>{$t('pages.adminOperations.changes.nextSeq')}</dt><dd>{n(changes.next_seq)}</dd></div>
          <div>
            <dt>{$t('pages.adminOperations.changes.rows')}</dt>
            <dd>
              {n(changes.rows)}
              <span class="muted">— {$t('pages.adminOperations.changes.byState', { values: { committed: n(changes.committed), pending: n(changes.pending), unknown: n(changes.unknown) } })}</span>
            </dd>
          </div>
          <div>
            <dt>{$t('pages.adminOperations.changes.range')}</dt>
            <dd>{changes.oldest_seq === null || changes.oldest_seq === undefined ? '—' : `${n(changes.oldest_seq)} – ${n(changes.newest_seq)}`}</dd>
          </div>
          <div><dt>{$t('pages.adminOperations.changes.size')}</dt><dd>{fmtBytes(changes.size_bytes)}</dd></div>
          <div>
            <dt>{$t('pages.adminOperations.changes.retention')}</dt>
            <dd>{$t('pages.adminOperations.changes.retentionDays', { values: { days: n(changes.retention_days) } })}</dd>
          </div>
          <div>
            <dt>{$t('pages.adminOperations.changes.caps')}</dt>
            <dd>{$t('pages.adminOperations.changes.capsValue', { values: { scan: n(changes.max_scan), payload: fmtBytes(changes.max_payload) } })}</dd>
          </div>
        </dl>

        <h4>{$t('pages.adminOperations.changes.cursorsTitle')}</h4>
        <p class="explain">
          {$t('pages.adminOperations.changes.cursorsExplain', { values: { floor: floor === null ? '—' : n(floor) } })}
        </p>
        {#if changes.cursors?.length}
          <table class="data-table">
            <thead>
              <tr>
                <th>{$t('pages.adminOperations.changes.colName')}</th>
                <th class="num">{$t('pages.adminOperations.changes.colSeq')}</th>
                <th class="num">{$t('pages.adminOperations.changes.colBehind')}</th>
                <th>{$t('pages.adminOperations.changes.colOwner')}</th>
                <th>{$t('pages.adminOperations.changes.colUpdated')}</th>
                <th>{$t('pages.adminOperations.changes.colExpires')}</th>
              </tr>
            </thead>
            <tbody>
              {#each changes.cursors as c (c.name)}
                <tr>
                  <td class="nowrap"><code>{c.name}</code></td>
                  <td class="num">{n(c.seq)}</td>
                  <td class="num">{changes.newest_seq === null || changes.newest_seq === undefined ? '—' : n(Math.max(0, changes.newest_seq - c.seq))}</td>
                  <td class="nowrap">{c.owner || '—'}</td>
                  <td class="nowrap" title={c.updated_at}>{ago(c.updated_at)}</td>
                  <td class="nowrap" title={c.expires_at}>{ago(c.expires_at)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        {:else}
          <p class="muted">{$t('pages.adminOperations.changes.cursorsNone')}</p>
        {/if}
        <p class="docs-line">
          <Link to="/docs/versioning">{$t('pages.adminOperations.changes.docs')}</Link>
        </p>
      {/if}
    </section>
  {/if}
</div>

<style>
  .admin-ops { max-width: 1200px; }
  .header-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    flex-wrap: wrap;
    margin-bottom: 1rem;
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0;
  }
  .subtitle {
    margin: 0.35rem 0 0;
    color: var(--ink-500);
    font-size: 0.88rem;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .updated {
    font-size: 0.78rem;
    color: var(--ink-500);
  }

  .card {
    background: white;
    border: 1px solid var(--line-soft);
    border-radius: 12px;
    padding: 1rem 1.1rem;
    margin-bottom: 1rem;
  }
  .card-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    flex-wrap: wrap;
    margin-bottom: 0.5rem;
  }
  h3 {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    margin: 0;
    font-size: 1.02rem;
  }
  h4 {
    margin: 1.1rem 0 0.25rem;
    font-size: 0.8rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-500);
  }
  .explain {
    margin: 0 0 0.75rem;
    font-size: 0.86rem;
    color: var(--ink-600, #4b5563);
    line-height: 1.45;
  }
  .muted { color: var(--ink-500); font-size: 0.82rem; }
  .emph { font-weight: 700; color: #92400e; }
  .docs-line { margin: 0.9rem 0 0; font-size: 0.84rem; }
  .card-error {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin: 0 0 0.75rem;
    padding: 0.5rem 0.7rem;
    border-radius: 8px;
    font-size: 0.84rem;
    background: #fee2e2;
    color: #991b1b;
    word-break: break-word;
  }
  .inline-error { display: block; font-size: 0.78rem; color: #c62828; }

  .state-badge {
    display: inline-block;
    padding: 0.2rem 0.6rem;
    border-radius: 20px;
    font-size: 0.74rem;
    font-weight: 600;
    white-space: nowrap;
  }
  .state-standalone { background: #f5f5f5; color: #616161; }
  .state-leader { background: #e3f2fd; color: #1565c0; }
  .state-in_sync { background: #d4edda; color: #155724; }
  .state-catching_up { background: #fef3c7; color: #92400e; }
  .state-stale { background: #fef3c7; color: #92400e; }
  .state-error { background: #fee2e2; color: #991b1b; }

  .facts {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 0.45rem 1.25rem;
    margin: 0;
  }
  .facts > div {
    display: grid;
    grid-template-columns: minmax(120px, 42%) 1fr;
    gap: 0.5rem;
    align-items: baseline;
    font-size: 0.86rem;
    padding: 0.2rem 0;
    border-bottom: 1px dotted var(--line-soft);
  }
  .facts dt { margin: 0; color: var(--ink-500); }
  .facts dd { margin: 0; overflow-wrap: anywhere; }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    margin: 0 0.3rem 0.2rem 0;
    padding: 0.1rem 0.5rem;
    border-radius: 20px;
    font-size: 0.76rem;
    background: #f5f5f5;
    color: #616161;
  }

  .stats-row {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    gap: 0.75rem;
    margin-bottom: 0.5rem;
  }
  .stat-card {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.75rem 0.9rem;
    background: var(--bg-accent, #f8f9fa);
    border: 1px solid var(--line-soft);
    border-radius: 10px;
  }
  .stat-value { font-size: 1.25rem; font-weight: 700; line-height: 1.1; }
  .stat-label {
    font-size: 0.7rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-500);
  }

  .two-up {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(340px, 1fr));
    gap: 1rem;
  }
  .two-up .card { margin-bottom: 1rem; }

  .data-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.84rem;
  }
  .data-table th, .data-table td {
    padding: 0.45rem 0.6rem;
    text-align: left;
    border-bottom: 1px solid var(--line-soft);
    vertical-align: top;
  }
  .data-table th {
    font-weight: 600;
    color: var(--ink-500);
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    white-space: nowrap;
  }
  .data-table tr:last-child td { border-bottom: none; }
  .nowrap { white-space: nowrap; }
  .num { text-align: right; white-space: nowrap; font-variant-numeric: tabular-nums; }
  .meaning { color: var(--ink-600, #4b5563); }
  .share-col { min-width: 140px; }
  .bar {
    display: inline-block;
    vertical-align: middle;
    width: var(--w, 0%);
    max-width: 90px;
    height: 8px;
    border-radius: 4px;
    background: #1565c0;
    opacity: 0.75;
    margin-right: 0.4rem;
  }
  .share-text { font-variant-numeric: tabular-nums; }

  .loading {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 2rem;
    justify-content: center;
    color: var(--ink-500);
  }

  @media (max-width: 640px) {
    .data-table { font-size: 0.76rem; }
    .data-table th, .data-table td { padding: 0.4rem 0.45rem; }
    .facts > div { grid-template-columns: 1fr; gap: 0.1rem; }
  }

  :global(:is([data-theme="dark"], .dark)) .card { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .stat-card { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .explain,
  :global(:is([data-theme="dark"], .dark)) .meaning { color: var(--ink-400, #9ca3af); }
  :global(:is([data-theme="dark"], .dark)) .emph { color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .card-error { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .inline-error { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .chip { background: rgba(255,255,255,0.08); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .state-standalone { background: rgba(255,255,255,0.08); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .state-leader { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .state-in_sync { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .state-catching_up,
  :global(:is([data-theme="dark"], .dark)) .state-stale { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .state-error { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .bar { background: #60a5fa; }
</style>
