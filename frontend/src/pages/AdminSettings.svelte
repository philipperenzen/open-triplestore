<script>
  // Workspace settings: everything configured for this workspace as a whole
  // (personal settings live at /settings). Two tabs. "Live settings" are the
  // runtime settings an administrator can change without a restart
  // (self-registration, guest capabilities, embed framing, AI budgets, OIDC
  // token policies); overrides are stored server-side and audit-logged, and
  // resetting one hands the decision back to the environment. "Configuration"
  // is the read-only effective configuration, grouped, with the source of
  // every value.
  import { t } from 'svelte-i18n';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { navigate, Link } from '../lib/router/index.js';
  import { SlidersHorizontal, Loader2, CircleAlert, RefreshCw, Save, RotateCcw, Search, Zap, ServerCog, Lock } from 'lucide-svelte';
  import { adminListSettings, adminSetSetting, adminGetConfig } from '../lib/api.js';
  import { controlFor, groupSettings, isDirty, filterConfig, configSummary, tabFromHash } from '../lib/adminSettings.ts';
  import { toastSuccess } from '../lib/toast.ts';

  let settings = [];
  let config = null;
  let loading = false;
  let loadError = '';
  /** Unsaved edits per setting name. */
  let drafts = {};
  let saving = null;
  let query = '';
  let changedOnly = false;

  // The active tab lives in the URL hash, so it survives a reload and can be
  // linked to (/admin/settings#config).
  let tab = tabFromHash(location.hash);
  function selectTab(id) {
    tab = id;
    history.replaceState(null, '', `#${id}`);
  }

  $: groups = groupSettings(settings);
  $: shown = config ? filterConfig(config.settings, query, changedOnly) : [];
  $: shownGroups = groupSettings(shown);
  $: summary = configSummary(settings, config?.settings ?? []);

  async function load() {
    loading = true;
    loadError = '';
    try {
      const [s, c] = await Promise.all([adminListSettings(), adminGetConfig()]);
      settings = s.settings || [];
      config = c;
      drafts = Object.fromEntries(settings.map((x) => [x.name, x.value ?? '']));
    } catch (e) {
      loadError = e?.message || String(e);
    }
    loading = false;
  }

  async function apply(s, value) {
    saving = s.name;
    loadError = '';
    try {
      const { setting } = await adminSetSetting(s.name, value);
      settings = settings.map((x) => (x.name === s.name ? setting : x));
      drafts[s.name] = setting?.value ?? '';
      toastSuccess($t(value === null ? 'pages.adminSettings.resetDone' : 'pages.adminSettings.saved', { values: { name: s.name } }));
      config = await adminGetConfig();
    } catch (e) {
      loadError = e?.message || String(e);
    }
    saving = null;
  }

  function toggle(s) {
    apply(s, s.value === 'true' ? 'false' : 'true');
  }

  /** The configuration narrowed to what is not at its default: the settings
   *  the summary counts, so no text filter on top. */
  function showChanged() {
    query = '';
    changedOnly = true;
    selectTab('config');
  }

  // Reactive so the labels follow a language switch.
  $: sourceLabel = (source) => $t(`pages.adminSettings.source.${source}`);
  // A group the UI has no name for shows as the server sends it.
  $: groupLabel = (group) => $t(`pages.adminSettings.groups.${group}`, { default: group });

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
    else load();
  }
</script>

<div class="admin-settings">
  <div class="header-row">
    <div class="header-text">
      <h2><SlidersHorizontal size={20} /> {$t('pages.adminSettings.title')}</h2>
      <p class="subtitle">{$t('pages.adminSettings.detail')}</p>
      <p class="subtitle personal"><Link to="/settings">{$t('pages.adminSettings.personalHint')}</Link></p>
    </div>
    <button class="btn btn-sm btn-ghost" on:click={load} disabled={loading}>
      <RefreshCw size={14} class={loading ? 'animate-spin' : ''} /> {$t('pages.adminSettings.refresh')}
    </button>
  </div>

  {#if loadError}
    <p class="card-error" role="alert"><CircleAlert size={14} /> {loadError}</p>
  {/if}

  {#if loading && settings.length === 0}
    <div class="loading"><Loader2 size={18} class="animate-spin" /> {$t('pages.adminSettings.loading')}</div>
  {:else}
    {#if config}
      <div class="summary">
        <div class="stat-pill">
          <span class="stat-value">{config.profile}</span>
          <span class="stat-label">{$t('pages.adminSettings.summaryProfile')}</span>
        </div>
        <div class="stat-pill">
          <span class="stat-value">{summary.total}</span>
          <span class="stat-label">{$t('pages.adminSettings.summaryTotal')}</span>
        </div>
        <button type="button" class="stat-pill stat-button" on:click={showChanged} title={$t('pages.adminSettings.summaryChangedTitle')}>
          <span class="stat-value">{summary.changed}</span>
          <span class="stat-label">{$t('pages.adminSettings.summaryChanged')}</span>
        </button>
        <div class="stat-pill">
          <span class="stat-value">{summary.overrides}</span>
          <span class="stat-label">{$t('pages.adminSettings.summaryOverrides')}</span>
        </div>
      </div>
    {/if}

    <div class="settings-tabs" role="tablist" aria-label={$t('pages.adminSettings.title')}>
      <button type="button" role="tab" class="settings-tab" class:active={tab === 'live'} aria-selected={tab === 'live'} on:click={() => selectTab('live')}>
        <Zap size={15} /> {$t('pages.adminSettings.tabLive')} <span class="tab-count">{settings.length}</span>
      </button>
      <button type="button" role="tab" class="settings-tab" class:active={tab === 'config'} aria-selected={tab === 'config'} on:click={() => selectTab('config')}>
        <ServerCog size={15} /> {$t('pages.adminSettings.tabConfig')} <span class="tab-count">{summary.total}</span>
      </button>
    </div>

    {#if tab === 'live'}
      <section class="card" role="tabpanel">
        <h3>{$t('pages.adminSettings.runtimeTitle')}</h3>
        <p class="hint">{$t('pages.adminSettings.runtimeHint')}</p>
        <p class="hint"><Link to="/admin/security">{$t('pages.adminSettings.guestHint')}</Link></p>
        {#each groups as g (g.group)}
          <h4 class="group">{groupLabel(g.group)}</h4>
          <ul class="setting-list">
            {#each g.settings as s (s.name)}
              {@const control = controlFor(s)}
              <li class="setting">
                <div class="setting-head">
                  <code class="setting-name">{s.name}</code>
                  <span class="badge badge-{s.source}" title={$t('pages.adminSettings.sourceTitle')}>{sourceLabel(s.source)}</span>
                </div>
                <p class="doc">{s.doc}</p>
                <div class="setting-edit">
                  {#if control === 'toggle'}
                    <label class="switch">
                      <input type="checkbox" checked={s.value === 'true'} disabled={saving === s.name} on:change={() => toggle(s)} />
                      <span>{s.value === 'true' ? $t('pages.adminSettings.on') : $t('pages.adminSettings.off')}</span>
                    </label>
                  {:else if control === 'select'}
                    <select bind:value={drafts[s.name]} aria-label={s.name} disabled={saving === s.name}>
                      {#each s.choices as c}
                        <option value={c}>{c}</option>
                      {/each}
                    </select>
                  {:else}
                    <input type="text" bind:value={drafts[s.name]} aria-label={s.name} disabled={saving === s.name} placeholder={s.configured ?? ''} />
                  {/if}
                  {#if control !== 'toggle'}
                    <button class="btn btn-sm" on:click={() => apply(s, drafts[s.name].trim())} disabled={saving === s.name || !isDirty(s, drafts[s.name])}>
                      {#if saving === s.name}<Loader2 size={14} class="animate-spin" />{:else}<Save size={14} />{/if}
                      {$t('pages.adminSettings.save')}
                    </button>
                  {/if}
                  {#if s.overridden}
                    <button class="btn btn-sm btn-ghost" on:click={() => apply(s, null)} disabled={saving === s.name}
                      title={$t('pages.adminSettings.resetTitle')}>
                      <RotateCcw size={14} /> {$t('pages.adminSettings.reset')}
                    </button>
                  {/if}
                </div>
                {#if s.overridden}
                  <p class="configured">
                    {$t('pages.adminSettings.configured', { values: { value: s.configured ?? '—', source: sourceLabel(s.configured_source) } })}
                  </p>
                {/if}
              </li>
            {/each}
          </ul>
        {/each}
      </section>
    {:else if config}
      <section class="card" role="tabpanel">
        <h3>{$t('pages.adminSettings.configTitle')}</h3>
        <p class="hint">
          {$t('pages.adminSettings.configHint', { values: { profile: config.profile } })}
        </p>
        <div class="config-filters">
          <label class="search">
            <Search size={14} />
            <input type="search" bind:value={query} placeholder={$t('pages.adminSettings.search')} aria-label={$t('pages.adminSettings.search')} />
          </label>
          <label class="changed-only">
            <input type="checkbox" bind:checked={changedOnly} />
            {$t('pages.adminSettings.changedOnly')}
          </label>
        </div>
        <div class="table-wrap">
          <table class="config-table">
            <thead>
              <tr>
                <th>{$t('pages.adminSettings.colName')}</th>
                <th>{$t('pages.adminSettings.colValue')}</th>
                <th>{$t('pages.adminSettings.colSource')}</th>
                <th>{$t('pages.adminSettings.colDefault')}</th>
              </tr>
            </thead>
            {#each shownGroups as g (g.group)}
              <tbody>
                <tr class="group-row">
                  <th colspan="4" scope="rowgroup">{groupLabel(g.group)} <span class="group-count">{g.settings.length}</span></th>
                </tr>
                {#each g.settings as c (c.name)}
                  <tr class:changed={c.source !== 'default'}>
                    <td class="name-cell">
                      <div class="name-line">
                        <code>{c.name}</code>
                        {#if c.secret}
                          <span class="secret" title={$t('pages.adminSettings.secretTitle')} aria-label={$t('pages.adminSettings.secretTitle')}><Lock size={12} /></span>
                        {/if}
                        {#if c.runtime}
                          <button type="button" class="chip-live" on:click={() => selectTab('live')} title={$t('pages.adminSettings.liveTitle')}>
                            <Zap size={11} /> {$t('pages.adminSettings.live')}
                          </button>
                        {/if}
                      </div>
                      {#if c.doc}<p class="row-doc">{c.doc}</p>{/if}
                    </td>
                    <td class="value">{c.value ?? '—'}</td>
                    <td><span class="badge badge-{c.source}">{sourceLabel(c.source)}</span></td>
                    <td class="value">{c.default ?? '—'}</td>
                  </tr>
                {/each}
              </tbody>
            {:else}
              <tbody>
                <tr><td colspan="4" class="no-match">{$t('pages.adminSettings.noMatch')}</td></tr>
              </tbody>
            {/each}
          </table>
        </div>
      </section>
    {/if}
  {/if}
</div>

<style>
  .header-row { display: flex; align-items: flex-start; justify-content: space-between; gap: 1rem; margin-bottom: 1rem; }
  .header-text { min-width: 0; }
  h2 { display: flex; align-items: center; gap: 0.5rem; margin: 0; }
  .subtitle { margin: 0.35rem 0 0; font-size: 0.86rem; color: var(--ink-500); line-height: 1.45; }
  .personal { font-size: 0.8rem; }
  .personal :global(a) { color: var(--brand-600); }
  /* The surface comes from the global .card, which follows the theme. */
  .card { padding: 1rem 1.1rem; margin-bottom: 1rem; }
  h3 { margin: 0 0 0.25rem; font-size: 1rem; }
  .hint { margin: 0 0 0.75rem; font-size: 0.84rem; color: var(--ink-500); line-height: 1.45; }

  /* ── Summary ──────────────────────────────────────────────────────────── */
  .summary { display: flex; flex-wrap: wrap; gap: 0.6rem; margin-bottom: 1rem; }
  .summary .stat-pill { min-width: 7.5rem; }
  .summary .stat-value { text-transform: capitalize; }
  .stat-button { font: inherit; text-align: left; cursor: pointer; }
  .stat-button:hover { border-color: var(--brand-400); }

  /* ── Tabs ─────────────────────────────────────────────────────────────── */
  .settings-tabs {
    display: flex;
    gap: 0.3rem;
    margin-bottom: 0.9rem;
    border-bottom: 1px solid var(--line-soft);
    overflow-x: auto;
    scrollbar-width: none;
  }
  .settings-tabs::-webkit-scrollbar { display: none; }
  .settings-tab {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    flex-shrink: 0;
    padding: 0.5rem 0.85rem;
    border: none;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
    background: transparent;
    font: inherit;
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--ink-500);
    cursor: pointer;
    transition: color 0.12s, border-color 0.12s, background 0.12s;
  }
  .settings-tab:hover { color: var(--ink-800); background: var(--bg-soft); }
  .settings-tab.active { color: var(--brand-600); border-bottom-color: var(--brand-500); }
  .tab-count {
    font-size: 0.7rem;
    font-weight: 600;
    padding: 0 0.4rem;
    border-radius: 999px;
    background: var(--bg-soft);
    color: var(--ink-500);
  }

  /* ── Live settings ────────────────────────────────────────────────────── */
  .group { margin: 1rem 0 0.4rem; font-size: 0.78rem; text-transform: uppercase; letter-spacing: 0.04em; color: var(--ink-500); }
  .setting-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.6rem; }
  .setting { border: 1px solid var(--line-soft); border-radius: 10px; padding: 0.65rem 0.8rem; background: var(--bg-float); }
  .setting-head { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; }
  .setting-name { font-size: 0.82rem; overflow-wrap: anywhere; }
  .doc { margin: 0.3rem 0 0.5rem; font-size: 0.84rem; color: var(--ink-700); line-height: 1.45; }
  .setting-edit { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; }
  .setting-edit input[type='text'], .setting-edit select { width: auto; min-width: min(18rem, 100%); }
  .switch { display: inline-flex; align-items: center; gap: 0.4rem; font-size: 0.86rem; cursor: pointer; }
  .configured { margin: 0.4rem 0 0; font-size: 0.8rem; color: var(--ink-500); }

  /* ── Source badges ────────────────────────────────────────────────────── */
  .badge { font-size: 0.72rem; padding: 0.1rem 0.45rem; border-radius: 999px; background: var(--bg-soft); color: var(--ink-700); white-space: nowrap; }
  .badge-admin { background: var(--violet-100); color: #5b21b6; }
  .badge-env, .badge-file, .badge-cli { background: #e0f2fe; color: #075985; }
  .badge-profile { background: var(--warning-100); color: #92400e; }

  .card-error { display: flex; align-items: center; gap: 0.4rem; margin: 0 0 0.75rem; padding: 0.5rem 0.7rem; border-radius: 8px; font-size: 0.84rem; background: var(--danger-100); color: #991b1b; overflow-wrap: anywhere; }
  .loading { display: flex; align-items: center; justify-content: center; gap: 0.5rem; padding: 2rem; color: var(--ink-500); }

  /* ── Configuration ────────────────────────────────────────────────────── */
  .config-filters { display: flex; align-items: center; flex-wrap: wrap; gap: 0.75rem; margin-bottom: 0.6rem; }
  .search { display: inline-flex; align-items: center; gap: 0.35rem; color: var(--ink-500); }
  .search input { width: min(22rem, 70vw); }
  .changed-only { display: inline-flex; align-items: center; gap: 0.35rem; font-size: 0.84rem; }
  .table-wrap { overflow-x: auto; }
  .config-table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
  .config-table th, .config-table td { text-align: left; padding: 0.4rem 0.55rem; border-bottom: 1px solid var(--line-soft); vertical-align: top; }
  .config-table .value { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; font-size: 0.78rem; overflow-wrap: anywhere; max-width: 24rem; }
  .config-table tr.group-row th {
    padding-top: 0.9rem;
    font-size: 0.74rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--ink-600);
    background: var(--bg-soft);
  }
  .group-count { margin-left: 0.3rem; font-weight: 600; color: var(--ink-400); }
  .config-table tr.changed td:first-child { box-shadow: inset 2px 0 0 var(--brand-500); }
  .name-cell { min-width: 14rem; }
  .name-line { display: flex; align-items: center; flex-wrap: wrap; gap: 0.35rem; }
  .name-line code { overflow-wrap: anywhere; }
  .secret { display: inline-flex; color: var(--ink-500); }
  .chip-live {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    padding: 0 0.4rem;
    border: 1px solid var(--brand-200);
    border-radius: 999px;
    background: var(--brand-100);
    color: var(--brand-700);
    font: inherit;
    font-size: 0.68rem;
    font-weight: 600;
    cursor: pointer;
  }
  .row-doc { margin: 0.2rem 0 0; font-size: 0.76rem; color: var(--ink-500); line-height: 1.4; max-width: 36rem; }
  .no-match { color: var(--ink-500); text-align: center; padding: 1rem; }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .settings-tab:hover { background: rgba(255, 255, 255, 0.05); }
  :global(:is([data-theme="dark"], .dark)) .tab-count { background: rgba(255, 255, 255, 0.06); }
  :global(:is([data-theme="dark"], .dark)) .badge { background: rgba(255, 255, 255, 0.06); color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .badge-admin { background: rgba(139, 92, 246, 0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .badge-env,
  :global(:is([data-theme="dark"], .dark)) .badge-file,
  :global(:is([data-theme="dark"], .dark)) .badge-cli { background: rgba(14, 165, 233, 0.2); color: #7dd3fc; }
  :global(:is([data-theme="dark"], .dark)) .badge-profile { background: rgba(245, 158, 11, 0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .card-error { background: rgba(239, 68, 68, 0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .config-table tr.group-row th { background: rgba(255, 255, 255, 0.04); }
</style>
