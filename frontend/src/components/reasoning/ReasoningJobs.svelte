<script lang="ts">
  // Background reasoning runs (`?async=true`, GET /api/reasoning/jobs): the
  // caller's jobs (an admin's: everyone's), for one dataset or all, with their
  // status; polled while any is queued or running. A finished job shows the
  // answer the synchronous call would have given.
  import { onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { ListChecks, Loader2, RefreshCw } from 'lucide-svelte';
  import MaterialiseReport from './MaterialiseReport.svelte';
  import CheckResultView from './CheckResultView.svelte';
  import RunFailure from './RunFailure.svelte';
  import { listReasoningJobs, type CheckResult, type MaterializeReport, type ReasoningJob } from '../../lib/api';
  import { statusTone } from '../../lib/reasoning';
  import './reasoning.css';

  export let datasetId: string | null = null;
  /** Bump to reload (a job was just queued). */
  export let refreshKey = 0;
  export let pollMs = 2000;

  let jobs: ReasoningJob[] = [];
  let loading = true;
  let error = '';
  let open: string | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  async function load() {
    try {
      jobs = (await listReasoningJobs(datasetId ?? undefined)).jobs;
      error = '';
    } catch (e) {
      error = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
    if (timer) clearTimeout(timer);
    timer = jobs.some((j) => j.status === 'queued' || j.status === 'running') ? setTimeout(load, pollMs) : null;
  }

  $: refreshKey, datasetId, load();
  onDestroy(() => { if (timer) clearTimeout(timer); });

  function fmt(ts: string | null) {
    if (!ts) return '—';
    const d = new Date(ts);
    return Number.isNaN(d.getTime()) ? ts : d.toLocaleTimeString();
  }
  const asMaterialize = (r: Record<string, unknown> | null) => r as unknown as MaterializeReport;
  const asCheck = (r: Record<string, unknown> | null) => r as unknown as CheckResult;
  const failureOf = (j: ReasoningJob) => ({
    status: j.http_status,
    message: String((j.result as Record<string, unknown> | null)?.error ?? (j.result as Record<string, unknown> | null)?.message ?? ''),
    body: j.result,
  });
</script>

<div class="card rz-card" id="jobs">
  <div class="rz-head">
    <div class="rz-head-left"><ListChecks size={15} /><h3>{$t('components.reasoning.jobsHeading')}</h3></div>
    <button class="btn btn-sm btn-ghost" on:click={load} title={$t('components.reasoning.refresh')} aria-label={$t('components.reasoning.refreshJobs')}><RefreshCw size={13} /></button>
  </div>
  <p class="rz-intro">{$t('components.reasoning.jobsIntro')}</p>
  {#if loading}
    <p class="rz-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
  {:else if error}
    <p class="rz-msg rz-msg-bad">{error}</p>
  {:else if !jobs.length}
    <p class="rz-muted">{$t('components.reasoning.noJobs')}</p>
  {:else}
    <div class="rz-table-wrap">
      <table class="rz-table">
        <thead>
          <tr>
            <th>{$t('components.reasoning.jobKind')}</th>
            <th>{$t('components.reasoning.status')}</th>
            {#if !datasetId}<th>{$t('components.reasoning.jobDataset')}</th>{/if}
            <th>{$t('components.reasoning.jobCreated')}</th>
            <th>{$t('components.reasoning.jobFinished')}</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {#each jobs as j (j.id)}
            <tr data-testid="job-row">
              <td>{$t(`components.reasoning.jobKinds.${j.kind}`)}</td>
              <td>
                <span class="rz-badge rz-{statusTone(j.status)}">
                  {#if j.status === 'queued' || j.status === 'running'}<Loader2 size={11} class="animate-spin" />{/if}
                  {$t(`components.reasoning.jobStatus.${j.status}`)}
                </span>
                {#if j.http_status && j.http_status !== 200}<code>{j.http_status}</code>{/if}
              </td>
              {#if !datasetId}<td>{j.dataset ?? '—'}</td>{/if}
              <td>{fmt(j.created_at)}</td>
              <td>{fmt(j.finished_at)}</td>
              <td>
                {#if j.result}
                  <button class="btn btn-sm btn-ghost" on:click={() => (open = open === j.id ? null : j.id)}>
                    {open === j.id ? $t('components.reasoning.hideResult') : $t('components.reasoning.showResult')}
                  </button>
                {/if}
              </td>
            </tr>
            {#if open === j.id && j.result}
              <tr>
                <td colspan={datasetId ? 5 : 6}>
                  {#if j.status === 'failed'}
                    <RunFailure {...failureOf(j)} />
                  {:else if j.kind === 'materialize'}
                    <MaterialiseReport report={asMaterialize(j.result)} />
                  {:else}
                    <CheckResultView result={asCheck(j.result)} />
                  {/if}
                </td>
              </tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>
