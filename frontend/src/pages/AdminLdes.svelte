<script lang="ts">
  // Admin: LDES sync jobs — remote event streams mirrored into dataset
  // graphs. Lists every (dataset, source) sync with its latest outcome, adds
  // one (which runs it at once), re-runs one, schedules one, and forgets one
  // (its bookmark too; the mirrored graph stays). A failed run keeps the
  // previous bookmark, so "run now" resumes from where the last good run
  // stopped. A scheduled job shows its next run, or that it is paused (its
  // owner may no longer write; last_error says why) until saved again.
  import { t, locale } from 'svelte-i18n';
  import { Rss, Loader2, RefreshCw, Play, Trash2, Plus, CircleAlert, ChevronDown, ChevronRight, Clock } from 'lucide-svelte';
  import { isAdmin, authInitialized } from '../lib/stores.js';
  import { navigate, Link } from '../lib/router/index.js';
  import {
    adminListLdesSyncs, adminForgetLdesSync, adminSetLdesSyncSchedule, runLdesSync, listDatasets,
    type LdesSyncJob, type LdesSchedule,
  } from '../lib/api';
  import { askConfirm } from '../lib/confirm';
  import { toastError, toastSuccess, toastWarn } from '../lib/toast';
  import '../components/exchange/exchange.css';

  let jobs: LdesSyncJob[] = [];
  let loading = false;
  let error = '';
  let running: Record<string, boolean> = {};
  let expanded = new Set<string>();

  // The schedule form of each opened job: what is being edited, whether it
  // is being saved, and why it was refused.
  const MIN_INTERVAL = 60;
  let drafts: Record<string, { schedule: LdesSchedule; interval: number | string }> = {};
  let saving: Record<string, boolean> = {};
  let scheduleError: Record<string, string> = {};

  let datasets: { id: string; name: string }[] = [];
  let adding = false;
  let url = '';
  let datasetId = '';
  let graphIri = '';
  let addError = '';
  let submitting = false;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
    else load();
  }

  $: lang = $locale || 'en';
  const when = (iso: string | null | undefined) => (iso ? new Date(iso).toLocaleString(lang) : '—');
  const key = (j: { dataset_id: string; source_url: string }) => `${j.dataset_id}\u0000${j.source_url}`;

  async function load() {
    loading = true;
    error = '';
    try {
      jobs = await adminListLdesSyncs();
    } catch (e) {
      error = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
  }

  async function openAdd() {
    adding = true;
    addError = '';
    if (!datasets.length) {
      try {
        const res = await listDatasets();
        datasets = (Array.isArray(res) ? res : res?.datasets ?? []).map((d: { id: string; name: string }) => ({ id: d.id, name: d.name }));
      } catch (e) {
        addError = (e as Error)?.message || String(e);
      }
    }
  }

  function report(r: any) {
    const warnings: string[] = r?.warnings ?? [];
    const msg = $t('pages.adminLdes.ranSummary', {
      values: { updated: r?.entities_updated ?? 0, deleted: r?.entities_deleted ?? 0, nodes: r?.nodes_visited ?? 0 },
    });
    if (warnings.length) toastWarn(`${msg} ${warnings.join(' ')}`);
    else toastSuccess(msg);
  }

  async function add() {
    addError = '';
    if (!/^https?:\/\//i.test(url.trim())) { addError = $t('pages.adminLdes.errors.url'); return; }
    if (!datasetId) { addError = $t('pages.adminLdes.errors.dataset'); return; }
    if (!/^[a-z][a-z0-9+.-]*:\S+$/i.test(graphIri.trim())) { addError = $t('pages.adminLdes.errors.graph'); return; }
    submitting = true;
    try {
      const r = await runLdesSync({ url: url.trim(), dataset_id: datasetId, graph_iri: graphIri.trim() });
      report(r);
      adding = false;
      url = '';
      graphIri = '';
    } catch (e) {
      addError = (e as Error)?.message || String(e);
    } finally {
      submitting = false;
      await load();
    }
  }

  async function runNow(j: LdesSyncJob) {
    if (!j.graph_iri) return;
    running = { ...running, [key(j)]: true };
    try {
      report(await runLdesSync({ url: j.source_url, dataset_id: j.dataset_id, graph_iri: j.graph_iri }));
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    } finally {
      running = { ...running, [key(j)]: false };
      await load();
    }
  }

  async function forget(j: LdesSyncJob) {
    const ok = await askConfirm({
      title: $t('pages.adminLdes.forgetTitle'),
      message: $t('pages.adminLdes.forgetMessage', { values: { url: j.source_url, dataset: j.dataset_name || j.dataset_id } }),
      confirmLabel: $t('pages.adminLdes.forget'),
    });
    if (!ok) return;
    try {
      await adminForgetLdesSync(j.dataset_id, j.source_url);
      toastSuccess($t('pages.adminLdes.forgotten'));
      await load();
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    }
  }

  function toggle(j: LdesSyncJob) {
    const k = key(j);
    if (expanded.has(k)) {
      expanded.delete(k);
    } else {
      expanded.add(k);
      drafts = { ...drafts, [k]: { schedule: j.schedule ?? 'off', interval: j.interval_secs ?? 900 } };
    }
    expanded = expanded;
  }

  // 90 → "90 s", 900 → "15 min", 7200 → "2 h".
  function span(secs: number): string {
    if (secs % 3600 === 0) return $t('pages.adminLdes.unitHours', { values: { n: secs / 3600 } });
    if (secs % 60 === 0) return $t('pages.adminLdes.unitMinutes', { values: { n: secs / 60 } });
    return $t('pages.adminLdes.unitSeconds', { values: { n: secs } });
  }

  // What the job's schedule does, as the list says it.
  function scheduleLabel(j: LdesSyncJob): string {
    if (j.schedule === 'interval' && j.interval_secs) {
      return $t('pages.adminLdes.every', { values: { interval: span(j.interval_secs) } });
    }
    if (j.schedule === 'auto') {
      const declared = j.last_report?.polling_interval;
      const secs = Math.max(MIN_INTERVAL, typeof declared === 'number' ? declared : MIN_INTERVAL);
      return $t('pages.adminLdes.autoEvery', { values: { interval: span(secs) } });
    }
    return $t('pages.adminLdes.scheduleOff');
  }

  async function saveSchedule(j: LdesSyncJob) {
    const k = key(j);
    const d = drafts[k];
    if (!d) return;
    scheduleError = { ...scheduleError, [k]: '' };
    const body: { schedule: LdesSchedule; interval_secs?: number } = { schedule: d.schedule };
    if (d.schedule === 'interval') {
      const secs = Number(d.interval);
      if (!Number.isInteger(secs) || secs < MIN_INTERVAL) {
        scheduleError = { ...scheduleError, [k]: $t('pages.adminLdes.errors.interval') };
        return;
      }
      body.interval_secs = secs;
    }
    saving = { ...saving, [k]: true };
    try {
      await adminSetLdesSyncSchedule(j.dataset_id, j.source_url, body);
      toastSuccess($t('pages.adminLdes.scheduleSaved'));
      await load();
    } catch (e) {
      scheduleError = { ...scheduleError, [k]: (e as Error)?.message || String(e) };
    } finally {
      saving = { ...saving, [k]: false };
    }
  }
</script>

<div class="admin-ldes">
  <div class="header-row">
    <div>
      <h2><Rss size={20} /> {$t('pages.adminLdes.title')}</h2>
      <p class="subtitle">{$t('pages.adminLdes.detail')}</p>
    </div>
    <div class="xc-row">
      <button class="btn btn-sm btn-ghost" on:click={load} disabled={loading}>
        {#if loading}<Loader2 size={13} class="animate-spin" />{:else}<RefreshCw size={13} />{/if}
        {$t('pages.adminMaintenance.refresh')}
      </button>
      <button class="btn btn-sm" on:click={openAdd} disabled={adding}><Plus size={13} /> {$t('pages.adminLdes.add')}</button>
    </div>
  </div>

  {#if adding}
    <section class="card" data-testid="ldes-add">
      <div class="xc-head"><h3>{$t('pages.adminLdes.addTitle')}</h3></div>
      <p class="xc-intro">{$t('pages.adminLdes.addIntro')}</p>
      <form class="xc-form" on:submit|preventDefault={add}>
        <label class="xc-field">
          <span>{$t('pages.adminLdes.url')}</span>
          <input type="url" bind:value={url} placeholder="https://example.org/api/datasets/roads/ldes" disabled={submitting} />
          <small>{$t('pages.adminLdes.urlHint')}</small>
        </label>
        <div class="xc-grid">
          <label class="xc-field">
            <span>{$t('pages.adminLdes.dataset')}</span>
            <select bind:value={datasetId} disabled={submitting} data-testid="ldes-dataset">
              <option value="">{$t('pages.adminLdes.pickDataset')}</option>
              {#each datasets as d (d.id)}<option value={d.id}>{d.name}</option>{/each}
            </select>
          </label>
          <label class="xc-field">
            <span>{$t('pages.adminLdes.graph')}</span>
            <input type="text" bind:value={graphIri} placeholder="https://example.org/mirror/instances" disabled={submitting} />
            <small>{$t('pages.adminLdes.graphHint')}</small>
          </label>
        </div>
        {#if addError}<p class="xc-error" role="alert">{addError}</p>{/if}
        <div class="xc-row">
          <button type="submit" class="btn btn-sm" disabled={submitting}>
            {#if submitting}<Loader2 size={13} class="animate-spin" />{:else}<Play size={13} />{/if}
            {$t('pages.adminLdes.addRun')}
          </button>
          <button type="button" class="btn btn-sm btn-ghost" on:click={() => (adding = false)} disabled={submitting}>{$t('system.cancel')}</button>
        </div>
      </form>
    </section>
  {/if}

  <section class="card" data-testid="ldes-jobs">
    {#if error}
      <p class="xc-error"><CircleAlert size={14} /> {error}</p>
    {:else if loading && jobs.length === 0}
      <p class="xc-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
    {:else if jobs.length === 0}
      <p class="xc-muted">{$t('pages.adminLdes.none')}</p>
    {:else}
      <div class="xc-table-wrap">
        <table class="xc-table">
          <thead>
            <tr>
              <th></th>
              <th>{$t('pages.adminLdes.source')}</th>
              <th>{$t('pages.adminLdes.target')}</th>
              <th>{$t('pages.adminLdes.status')}</th>
              <th>{$t('pages.adminLdes.lastRun')}</th>
              <th>{$t('pages.adminLdes.schedule')}</th>
              <th>{$t('pages.adminLdes.applied')}</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {#each jobs as j (key(j))}
              <tr>
                <td>
                  <button class="icon-btn" on:click={() => toggle(j)} aria-expanded={expanded.has(key(j))} aria-label={$t('pages.adminLdes.details')}>
                    {#if expanded.has(key(j))}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
                  </button>
                </td>
                <td><code>{j.source_url}</code></td>
                <td>
                  <Link to={`/datasets/${j.dataset_id}`}>{j.dataset_name || j.dataset_id}</Link>
                  {#if j.graph_iri}<br /><code>{j.graph_iri}</code>{/if}
                </td>
                <td>
                  {#if j.last_status === 'ok'}
                    <span class="xc-badge xc-ok">{$t('pages.adminLdes.ok')}</span>
                  {:else if j.last_status === 'error'}
                    <span class="xc-badge xc-bad">{$t('pages.adminLdes.failed')}</span>
                  {:else}
                    <span class="xc-badge">{$t('pages.adminLdes.unknown')}</span>
                  {/if}
                </td>
                <td>{when(j.last_run_at ?? j.synced_at)}</td>
                <td data-testid="ldes-schedule">
                  {#if j.paused_at}
                    <span class="xc-badge xc-bad">{$t('pages.adminLdes.paused')}</span>
                  {:else}
                    {scheduleLabel(j)}
                    {#if j.next_run_at}
                      <br /><small class="xc-muted">{$t('pages.adminLdes.nextRun', { values: { when: when(j.next_run_at) } })}</small>
                    {/if}
                  {/if}
                </td>
                <td>{j.members_applied.toLocaleString()}</td>
                <td class="row-actions">
                  <button class="btn btn-xs btn-ghost" on:click={() => runNow(j)} disabled={!j.graph_iri || running[key(j)]} title={j.graph_iri ? '' : $t('pages.adminLdes.noGraph')}>
                    {#if running[key(j)]}<Loader2 size={12} class="animate-spin" />{:else}<Play size={12} />{/if}
                    {$t('pages.adminLdes.runNow')}
                  </button>
                  <button class="btn btn-xs btn-ghost" on:click={() => forget(j)}><Trash2 size={12} /> {$t('pages.adminLdes.forget')}</button>
                </td>
              </tr>
              {#if j.last_status === 'error' && j.last_error}
                <tr class="err-row"><td></td><td colspan="7"><span class="xc-error">{j.last_error}</span></td></tr>
              {/if}
              {#if expanded.has(key(j))}
                <tr>
                  <td></td>
                  <td colspan="7">
                    <dl class="xc-facts">
                      <div><dt>{$t('pages.adminLdes.bookmark')}</dt><dd>{j.last_timestamp || '—'}</dd></div>
                      <div><dt>{$t('pages.adminLdes.lastSuccess')}</dt><dd>{when(j.synced_at)}</dd></div>
                      <div><dt>{$t('pages.adminLdes.nextRunLabel')}</dt><dd>{j.paused_at ? $t('pages.adminLdes.paused') : when(j.next_run_at)}</dd></div>
                      {#if j.consecutive_failures}
                        <div><dt>{$t('pages.adminLdes.failures')}</dt><dd>{j.consecutive_failures}</dd></div>
                      {/if}
                      {#if j.last_report}
                        <div><dt>{$t('pages.adminLdes.nodes')}</dt><dd>{j.last_report.nodes_visited ?? 0}</dd></div>
                        <div><dt>{$t('pages.adminLdes.members')}</dt><dd>{j.last_report.members_seen ?? 0}</dd></div>
                        <div><dt>{$t('pages.adminLdes.updatedDeleted')}</dt><dd>{j.last_report.entities_updated ?? 0} / {j.last_report.entities_deleted ?? 0}</dd></div>
                        <div><dt>{$t('pages.adminLdes.pollingInterval')}</dt><dd>{j.last_report.polling_interval ?? '—'}</dd></div>
                      {/if}
                    </dl>
                    {#if j.last_report?.warnings?.length}
                      <ul class="xc-list">{#each j.last_report.warnings as w}<li>{w}</li>{/each}</ul>
                    {/if}
                    {#if drafts[key(j)]}
                      <form class="xc-form sched-form" data-testid="ldes-schedule-form" on:submit|preventDefault={() => saveSchedule(j)}>
                        <div class="xc-grid">
                          <label class="xc-field">
                            <span>{$t('pages.adminLdes.schedule')}</span>
                            <select bind:value={drafts[key(j)].schedule} disabled={!j.graph_iri || saving[key(j)]}>
                              <option value="off">{$t('pages.adminLdes.scheduleOff')}</option>
                              <option value="auto">{$t('pages.adminLdes.scheduleAuto')}</option>
                              <option value="interval">{$t('pages.adminLdes.scheduleInterval')}</option>
                            </select>
                          </label>
                          {#if drafts[key(j)].schedule === 'interval'}
                            <label class="xc-field">
                              <span>{$t('pages.adminLdes.intervalSecs')}</span>
                              <input type="number" min={MIN_INTERVAL} step="1" bind:value={drafts[key(j)].interval} disabled={saving[key(j)]} />
                            </label>
                          {/if}
                        </div>
                        <small>{j.graph_iri ? $t('pages.adminLdes.scheduleHint') : $t('pages.adminLdes.noGraph')}</small>
                        {#if scheduleError[key(j)]}<p class="xc-error" role="alert">{scheduleError[key(j)]}</p>{/if}
                        <div class="xc-row">
                          <button type="submit" class="btn btn-xs" disabled={!j.graph_iri || saving[key(j)]}>
                            {#if saving[key(j)]}<Loader2 size={12} class="animate-spin" />{:else}<Clock size={12} />{/if}
                            {$t('pages.adminLdes.saveSchedule')}
                          </button>
                        </div>
                      </form>
                    {/if}
                  </td>
                </tr>
              {/if}
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </section>
</div>

<style>
  .admin-ldes { display: grid; gap: 1rem; max-width: 76rem; }
  .header-row { display: flex; justify-content: space-between; align-items: flex-start; gap: 1rem; flex-wrap: wrap; }
  .header-row h2 { display: flex; align-items: center; gap: 0.5rem; margin: 0; }
  .subtitle { margin: 0.25rem 0 0; color: var(--ink-500); font-size: 0.9rem; }
  .row-actions { white-space: nowrap; text-align: right; }
  .err-row td { border-top: 0; padding-top: 0; }
  .sched-form { margin-top: 0.75rem; }
</style>
