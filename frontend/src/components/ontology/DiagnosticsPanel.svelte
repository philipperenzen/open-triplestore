<script>
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { AlertCircle, AlertTriangle, Info, Filter } from 'lucide-svelte';
  import { shortenIRI } from '../../lib/rdf-utils.js';

  /** issues: Array<{ code, severity, focus, message, predicate?, object? }> */
  export let issues = [];

  const dispatch = createEventDispatcher();
  let filterSeverity = 'all';
  let filterText = '';

  $: errorCount   = issues.filter(i => i.severity === 'error' || i.severity === 'Violation').length;
  $: warningCount = issues.filter(i => i.severity === 'warning' || i.severity === 'Warning').length;
  $: infoCount    = issues.filter(i => i.severity === 'info' || i.severity === 'Info').length;

  $: filtered = issues.filter(i => {
    if (filterSeverity !== 'all') {
      const sev = String(i.severity).toLowerCase();
      if (!sev.startsWith(filterSeverity)) return false;
    }
    if (filterText.trim()) {
      const q = filterText.toLowerCase();
      return (i.message || '').toLowerCase().includes(q) ||
             (i.focus || '').toLowerCase().includes(q) ||
             (i.code || '').toLowerCase().includes(q);
    }
    return true;
  });

  function sevClass(sev) {
    const s = String(sev).toLowerCase();
    if (s.startsWith('error') || s === 'violation') return 'err';
    if (s.startsWith('warn')) return 'warn';
    return 'info';
  }

  // Group by rule code
  $: grouped = (() => {
    const m = new Map();
    for (const i of filtered) {
      if (!m.has(i.code)) m.set(i.code, []);
      m.get(i.code).push(i);
    }
    return [...m.entries()].sort((a, b) => b[1].length - a[1].length);
  })();
</script>

<div class="diag">
  <div class="summary">
    <button class="pill err" class:active={filterSeverity === 'error' || filterSeverity === 'violation'}
            on:click={() => filterSeverity = filterSeverity === 'error' ? 'all' : 'error'}>
      <AlertCircle size={12} /> {$t('components.diagnosticsPanel.errorsCount', { values: { count: errorCount } })}
    </button>
    <button class="pill warn" class:active={filterSeverity === 'warn'}
            on:click={() => filterSeverity = filterSeverity === 'warn' ? 'all' : 'warn'}>
      <AlertTriangle size={12} /> {$t('components.diagnosticsPanel.warningsCount', { values: { count: warningCount } })}
    </button>
    <button class="pill info" class:active={filterSeverity === 'info'}
            on:click={() => filterSeverity = filterSeverity === 'info' ? 'all' : 'info'}>
      <Info size={12} /> {$t('components.diagnosticsPanel.infoCount', { values: { count: infoCount } })}
    </button>
    <div class="filter">
      <Filter size={12} />
      <input placeholder={$t('components.diagnosticsPanel.filterPlaceholder')} bind:value={filterText} />
    </div>
  </div>

  {#if filtered.length === 0}
    <div class="ok">
      ✓ {$t('components.diagnosticsPanel.noIssues')}
    </div>
  {:else}
    <div class="groups">
      {#each grouped as [code, list]}
        <section class="group">
          <header>
            <code>{code}</code>
            <span class="count">{list.length}</span>
          </header>
          <ul>
            {#each list as i}
              <li class={sevClass(i.severity)}>
                <span class="ico">
                  {#if sevClass(i.severity) === 'err'}<AlertCircle size={12} />
                  {:else if sevClass(i.severity) === 'warn'}<AlertTriangle size={12} />
                  {:else}<Info size={12} />{/if}
                </span>
                <div class="body">
                  <div class="msg">{i.message}</div>
                  {#if i.focus}
                    <button class="focus" on:click={() => dispatch('navigate', { iri: i.focus })}
                            title={i.focus}>
                      {shortenIRI(i.focus)}
                    </button>
                  {/if}
                </div>
              </li>
            {/each}
          </ul>
        </section>
      {/each}
    </div>
  {/if}
</div>

<style>
  .diag { display: flex; flex-direction: column; height: 100%; }
  .summary {
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 10px 12px;
    border-bottom: 1px solid #e5e7eb;
    background: #fafafa;
    flex-wrap: wrap;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    padding: 3px 10px;
    border-radius: 12px;
    border: 1px solid transparent;
    cursor: pointer;
    font-weight: 600;
  }
  .pill.err { background: #fee2e2; color: #991b1b; }
  .pill.warn { background: #fef3c7; color: #92400e; }
  .pill.info { background: #e0f2fe; color: #075985; }
  .pill.active { outline: 2px solid currentColor; }
  .filter { margin-left: auto; display: flex; align-items: center; gap: 4px; background: #fff; border: 1px solid #d1d5db; border-radius: 4px; padding: 2px 6px; }
  .filter input { border: none; outline: none; font-size: 12px; width: 200px; background: transparent; }
  .ok { padding: 20px; text-align: center; color: #16a34a; font-weight: 600; }
  .groups { overflow: auto; padding: 10px; flex: 1; }
  .group { margin-bottom: 12px; }
  .group header {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 4px;
  }
  .group header code {
    font-size: 11px;
    background: #f3f4f6;
    padding: 2px 6px;
    border-radius: 3px;
    color: #1f2937;
  }
  .count {
    font-size: 10px;
    background: #e5e7eb;
    color: #374151;
    padding: 1px 6px;
    border-radius: 8px;
    font-weight: 600;
  }
  ul { list-style: none; margin: 0; padding: 0; }
  li {
    display: flex;
    gap: 6px;
    padding: 6px 8px;
    border-radius: 4px;
    font-size: 12px;
    margin-bottom: 3px;
    align-items: start;
  }
  li.err { background: #fef2f2; color: #7f1d1d; }
  li.warn { background: #fffbeb; color: #78350f; }
  li.info { background: #eff6ff; color: #1e40af; }
  .ico { margin-top: 2px; }
  .msg { line-height: 1.35; }
  .focus {
    background: transparent;
    border: none;
    color: inherit;
    opacity: 0.8;
    font-size: 10px;
    padding: 2px 0;
    cursor: pointer;
    text-decoration: underline dotted;
    font-family: monospace;
  }

  :global(:is([data-theme="dark"], .dark)) .summary { background: var(--bg-soft); border-bottom-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .pill.err { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .pill.warn { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .pill.info { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .filter { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .ok { color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .group header code { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .count { background: rgba(255,255,255,0.08); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) li.err { background: rgba(239,68,68,0.12); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) li.warn { background: rgba(245,158,11,0.12); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) li.info { background: rgba(59,130,246,0.12); color: #93c5fd; }
</style>
