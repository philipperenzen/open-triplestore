<script>
  import { t } from 'svelte-i18n';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { navigate } from '../lib/router/index.js';
  import { Activity, Loader2, RefreshCw } from 'lucide-svelte';
  import Select from '../components/Select.svelte';
  import { adminLlmRequests, adminLlmStats } from '../lib/api.js';

  const LIMIT = 100;

  let stats = null;
  let requests = [];
  let loading = false;
  let loadingMore = false;
  let hasMore = false;
  let statusFilter = '';
  let endpointFilter = '';

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
    else refresh();
  }

  function filterParams() {
    const p = { limit: LIMIT };
    if (statusFilter) p.status = statusFilter;
    if (endpointFilter) p.endpoint = endpointFilter;
    return p;
  }

  async function loadRequests() {
    loading = true;
    try {
      const res = await adminLlmRequests({ ...filterParams(), offset: 0 });
      requests = res.requests || [];
      hasMore = requests.length >= LIMIT;
    } catch (e) {
      alert(e.message);
    }
    loading = false;
  }

  async function loadMore() {
    loadingMore = true;
    try {
      const res = await adminLlmRequests({ ...filterParams(), offset: requests.length });
      const page = res.requests || [];
      requests = [...requests, ...page];
      hasMore = page.length >= LIMIT;
    } catch (e) {
      alert(e.message);
    }
    loadingMore = false;
  }

  async function loadStats() {
    try { stats = await adminLlmStats(); } catch { /* non-fatal */ }
  }

  function refresh() {
    loadStats();
    loadRequests();
  }

  /** "843 ms" below a second, "2.1 s" above; em dash when absent. */
  function fmtMs(v) {
    if (v === null || v === undefined) return '—';
    if (v >= 1000) return `${(v / 1000).toFixed(1)} s`;
    return `${Math.round(v)} ms`;
  }

  function statusClass(s) {
    if (s === 'ok') return 'badge-ok';
    if (s === 'blocked') return 'badge-blocked';
    return 'badge-error';
  }

  $: byStatus = stats?.last_24h?.by_status || {};
  $: total24h = Object.values(byStatus).reduce((a, b) => a + b, 0);
</script>

<div class="admin-llm">
  <div class="header-row">
    <div>
      <h2><Activity size={20} /> {$t('pages.adminLlm.title')}</h2>
      <p class="subtitle">{$t('pages.adminLlm.detail')}</p>
    </div>
  </div>

  {#if stats}
    <div class="stats-row">
      <div class="stat-card">
        <span class="stat-value">{total24h}</span>
        <span class="stat-label">{$t('pages.adminLlm.requests24h')}</span>
      </div>
      <div class="stat-card">
        <span class="stat-value stat-blocked">{byStatus.blocked ?? 0}</span>
        <span class="stat-label">{$t('pages.adminLlm.blocked24h')}</span>
      </div>
      <div class="stat-card">
        <span class="stat-value stat-error">{byStatus.error ?? 0}</span>
        <span class="stat-label">{$t('pages.adminLlm.errors24h')}</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{fmtMs(stats.last_24h?.avg_duration_ms)}</span>
        <span class="stat-label">{$t('pages.adminLlm.avgDuration')}</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{fmtMs(stats.last_24h?.avg_ttft_ms)}</span>
        <span class="stat-label">{$t('pages.adminLlm.avgTtft')}</span>
      </div>
    </div>

    {#if stats.top_users_7d?.length}
      <div class="top-users">
        <span class="top-users-label">{$t('pages.adminLlm.topUsers')}</span>
        {#each stats.top_users_7d as u}
          <span class="user-chip">{u.user}<span class="chip-count">{u.requests}</span></span>
        {/each}
      </div>
    {/if}
  {/if}

  <div class="filter-row">
    <div class="filter-select">
      <label for="llm-filter-status">{$t('pages.adminLlm.colStatus')}</label>
      <Select id="llm-filter-status" size="sm" bind:value={statusFilter} on:change={loadRequests} options={[
        { value: '', label: $t('pages.adminLlm.filterAll') },
        { value: 'ok', label: 'ok' },
        { value: 'error', label: 'error' },
        { value: 'blocked', label: 'blocked' },
      ]} />
    </div>
    <div class="filter-select">
      <label for="llm-filter-endpoint">{$t('pages.adminLlm.colEndpoint')}</label>
      <Select id="llm-filter-endpoint" size="sm" bind:value={endpointFilter} on:change={loadRequests} options={[
        { value: '', label: $t('pages.adminLlm.filterAll') },
        { value: 'chat', label: 'chat' },
        { value: 'chat_stream', label: 'chat_stream' },
        { value: 'sparql', label: 'sparql' },
        { value: 'shacl', label: 'shacl' },
      ]} />
    </div>
    <button class="btn btn-sm" on:click={refresh} disabled={loading}>
      <RefreshCw size={14} /> {$t('pages.adminLlm.refresh')}
    </button>
  </div>

  {#if loading}
    <div class="loading"><Loader2 size={24} class="animate-spin" /> {$t('system.loading')}</div>
  {:else if requests.length === 0}
    <div class="empty-state">
      <Activity size={32} />
      <p>{$t('pages.adminLlm.empty')}</p>
    </div>
  {:else}
    <table class="data-table">
      <thead>
        <tr>
          <th>{$t('pages.adminLlm.colWhen')}</th>
          <th>{$t('pages.adminLlm.colUser')}</th>
          <th>{$t('pages.adminLlm.colEndpoint')}</th>
          <th>{$t('pages.adminLlm.colModel')}</th>
          <th>{$t('pages.adminLlm.colStatus')}</th>
          <th>{$t('pages.adminLlm.colDuration')}</th>
          <th>{$t('pages.adminLlm.colTtft')}</th>
          <th>{$t('pages.adminLlm.colRounds')}</th>
          <th>{$t('pages.adminLlm.colQuestion')}</th>
          <th>{$t('pages.adminLlm.colFlag')}</th>
        </tr>
      </thead>
      <tbody>
        {#each requests as r (r.id)}
          <tr>
            <td class="nowrap" title={r.timestamp}>{new Date(r.timestamp).toLocaleString()}</td>
            <td class="nowrap">
              {#if r.username}
                <strong>{r.username}</strong>
              {:else if r.user_id}
                {r.user_id}
              {:else}
                <em class="anon">{$t('pages.adminLlm.anonymous')}</em>
              {/if}
            </td>
            <td><code>{r.endpoint}</code></td>
            <td class="nowrap">{r.model || '—'}</td>
            <td><span class="status-badge {statusClass(r.status)}">{r.status}</span></td>
            <td class="nowrap">{fmtMs(r.duration_ms)}</td>
            <td class="nowrap">{fmtMs(r.ttft_ms)}</td>
            <td>{r.query_rounds ?? '—'}</td>
            <td class="question-cell">
              <span class="question" title={r.question_preview}>{r.question_preview || ''}</span>
              {#if r.error}
                <span class="row-error" title={r.error}>{r.error}</span>
              {/if}
            </td>
            <td>
              {#if r.guard_flag}
                <code class="flag-chip">{r.guard_flag}</code>
              {:else}
                —
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>

    {#if hasMore}
      <div class="load-more-row">
        <button class="btn btn-sm" on:click={loadMore} disabled={loadingMore}>
          {#if loadingMore}<Loader2 size={14} class="animate-spin" />{/if}
          {$t('pages.adminLlm.loadMore')}
        </button>
      </div>
    {/if}
  {/if}
</div>

<style>
  .admin-llm { max-width: 1200px; }
  .header-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
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

  .stats-row {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 0.75rem;
    margin-bottom: 1rem;
  }
  .stat-card {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.85rem 1rem;
    background: white;
    border: 1px solid var(--line-soft);
    border-radius: 12px;
  }
  .stat-value { font-size: 1.35rem; font-weight: 700; line-height: 1.1; }
  .stat-blocked { color: #92400e; }
  .stat-error { color: #c62828; }
  .stat-label {
    font-size: 0.72rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-500);
  }

  .top-users {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-bottom: 1rem;
  }
  .top-users-label {
    font-size: 0.78rem;
    font-weight: 600;
    color: var(--ink-500);
    margin-right: 0.25rem;
  }
  .user-chip {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.18rem 0.55rem;
    border-radius: 20px;
    font-size: 0.78rem;
    font-weight: 600;
    background: #f5f5f5;
    color: #616161;
  }
  .chip-count {
    padding: 0.05rem 0.4rem;
    border-radius: 20px;
    font-size: 0.7rem;
    background: #e3f2fd;
    color: #1565c0;
  }

  .filter-row {
    display: flex;
    align-items: flex-end;
    gap: 0.75rem;
    margin-bottom: 1rem;
    flex-wrap: wrap;
  }
  .filter-select {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    min-width: 150px;
  }
  .filter-select label {
    font-size: 0.72rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-500);
  }

  .loading {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 2rem;
    justify-content: center;
    color: var(--ink-500);
  }
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    padding: 3rem 1rem;
    color: var(--ink-500);
    text-align: center;
  }
  .empty-state p { margin: 0; }

  .data-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.84rem;
    background: white;
    border-radius: 12px;
    overflow: hidden;
    border: 1px solid var(--line-soft);
  }
  .data-table th, .data-table td {
    padding: 0.55rem 0.7rem;
    text-align: left;
    border-bottom: 1px solid var(--line-soft);
    vertical-align: top;
  }
  .data-table th {
    font-weight: 600;
    color: var(--ink-500);
    font-size: 0.74rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    background: var(--bg-accent, #f8f9fa);
    white-space: nowrap;
  }
  .data-table tr:last-child td { border-bottom: none; }
  .nowrap { white-space: nowrap; }
  .anon { color: var(--ink-400, #9ca3af); }

  .status-badge {
    display: inline-block;
    padding: 0.2rem 0.55rem;
    border-radius: 20px;
    font-size: 0.74rem;
    font-weight: 600;
  }
  .badge-ok { background: #d4edda; color: #155724; }
  .badge-error { background: #fee2e2; color: #991b1b; }
  .badge-blocked { background: #fef3c7; color: #92400e; }

  .question-cell { max-width: 280px; }
  .question {
    display: block;
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .row-error {
    display: block;
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-top: 0.15rem;
    font-size: 0.74rem;
    color: #c62828;
  }
  .flag-chip {
    display: inline-block;
    padding: 0.12rem 0.4rem;
    border-radius: 6px;
    font-size: 0.72rem;
    font-family: monospace;
    background: #f5f5f5;
    color: #616161;
  }

  .load-more-row {
    display: flex;
    justify-content: center;
    margin-top: 1rem;
  }

  @media (max-width: 640px) {
    .data-table { font-size: 0.76rem; }
    .data-table th, .data-table td { padding: 0.45rem 0.5rem; }
  }

  :global(:is([data-theme="dark"], .dark)) .stat-card { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .stat-blocked { color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .stat-error { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .data-table { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .badge-ok { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .badge-error { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .badge-blocked { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .user-chip { background: rgba(255,255,255,0.08); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .chip-count { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .flag-chip { background: rgba(255,255,255,0.08); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .row-error { color: #fca5a5; }
</style>
