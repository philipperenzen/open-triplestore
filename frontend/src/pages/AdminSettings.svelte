<script>
  // Admin settings: the runtime settings an administrator can change without a
  // restart (self-registration, guest capabilities, embed framing, AI budgets,
  // OIDC token policies), and the read-only effective configuration with the
  // source of every value. Overrides are stored server-side and audit-logged;
  // resetting one hands the decision back to the environment.
  import { t } from 'svelte-i18n';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { navigate, Link } from '../lib/router/index.js';
  import { SlidersHorizontal, Loader2, CircleAlert, RefreshCw, Save, RotateCcw, Search } from 'lucide-svelte';
  import { adminListSettings, adminSetSetting, adminGetConfig } from '../lib/api.js';
  import { controlFor, groupSettings, isDirty, filterConfig } from '../lib/adminSettings.ts';
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

  $: groups = groupSettings(settings);
  $: shown = config ? filterConfig(config.settings, query, changedOnly) : [];

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

  function sourceLabel(source) {
    return $t(`pages.adminSettings.source.${source}`);
  }

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
    <section class="card">
      <h3>{$t('pages.adminSettings.runtimeTitle')}</h3>
      <p class="hint">{$t('pages.adminSettings.runtimeHint')}</p>
      <p class="hint"><Link to="/admin/security">{$t('pages.adminSettings.guestHint')}</Link></p>
      {#each groups as g (g.group)}
        <h4 class="group">{g.group}</h4>
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

    {#if config}
      <section class="card">
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
            <tbody>
              {#each shown as c (c.name)}
                <tr>
                  <td><code title={c.doc}>{c.name}</code></td>
                  <td class="value">{c.value ?? '—'}</td>
                  <td><span class="badge badge-{c.source}">{sourceLabel(c.source)}</span></td>
                  <td class="value">{c.default ?? '—'}</td>
                </tr>
              {/each}
            </tbody>
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
  .card { background: white; border: 1px solid var(--line-soft); border-radius: 12px; padding: 1rem 1.1rem; margin-bottom: 1rem; }
  h3 { margin: 0 0 0.25rem; font-size: 1rem; }
  .hint { margin: 0 0 0.75rem; font-size: 0.84rem; color: var(--ink-500); line-height: 1.45; }
  .group { margin: 1rem 0 0.4rem; font-size: 0.78rem; text-transform: uppercase; letter-spacing: 0.04em; color: var(--ink-500); }
  .setting-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.6rem; }
  .setting { border: 1px solid var(--line-soft); border-radius: 10px; padding: 0.65rem 0.8rem; }
  .setting-head { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; }
  .setting-name { font-size: 0.82rem; overflow-wrap: anywhere; }
  .doc { margin: 0.3rem 0 0.5rem; font-size: 0.84rem; color: var(--ink-700); line-height: 1.45; }
  .setting-edit { display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem; }
  .setting-edit input[type='text'], .setting-edit select { width: auto; min-width: min(18rem, 100%); }
  .switch { display: inline-flex; align-items: center; gap: 0.4rem; font-size: 0.86rem; cursor: pointer; }
  .configured { margin: 0.4rem 0 0; font-size: 0.8rem; color: var(--ink-500); }
  .badge { font-size: 0.72rem; padding: 0.1rem 0.45rem; border-radius: 999px; background: var(--bg-muted, #f1f5f9); color: var(--ink-700); }
  .badge-admin { background: #ede9fe; color: #5b21b6; }
  .badge-env, .badge-file, .badge-cli { background: #e0f2fe; color: #075985; }
  .badge-profile { background: #fef3c7; color: #92400e; }
  .card-error { display: flex; align-items: center; gap: 0.4rem; margin: 0 0 0.75rem; padding: 0.5rem 0.7rem; border-radius: 8px; font-size: 0.84rem; background: #fee2e2; color: #991b1b; overflow-wrap: anywhere; }
  .loading { display: flex; align-items: center; justify-content: center; gap: 0.5rem; padding: 2rem; color: var(--ink-500); }
  .config-filters { display: flex; align-items: center; flex-wrap: wrap; gap: 0.75rem; margin-bottom: 0.6rem; }
  .search { display: inline-flex; align-items: center; gap: 0.35rem; }
  .search input { width: min(22rem, 70vw); }
  .changed-only { display: inline-flex; align-items: center; gap: 0.35rem; font-size: 0.84rem; }
  .table-wrap { overflow-x: auto; }
  .config-table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
  .config-table th, .config-table td { text-align: left; padding: 0.35rem 0.5rem; border-bottom: 1px solid var(--line-soft); vertical-align: top; }
  .config-table .value { overflow-wrap: anywhere; max-width: 28rem; }
</style>
