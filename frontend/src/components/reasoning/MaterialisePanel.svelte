<script lang="ts">
  // Materialise a regime over the dataset now (POST /api/reasoning/materialize
  // with `dataset`), into the dataset's own graph for the regime unless a
  // target is given; `eq_ref` for OWL 2 RL; in the background as a job when
  // asked (always offered for owl2-dl, which can take minutes).
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Loader2, Play } from 'lucide-svelte';
  import Select from '../Select.svelte';
  import MaterialiseReport from './MaterialiseReport.svelte';
  import RunFailure from './RunFailure.svelte';
  import { materializeEntailment, type MaterializeRegime, type MaterializeReport } from '../../lib/api';
  import { toastInfo } from '../../lib/toast';
  import './reasoning.css';

  export let datasetId: string;
  export let canWrite = false;
  /** The server's DL backend; null: owl2-dl is unavailable. */
  export let dlBackend: string | null = null;

  const dispatch = createEventDispatcher<{ queued: { jobId: string } }>();
  const REGIMES: MaterializeRegime[] = ['rdfs', 'owl2-rl', 'owl2-el', 'owl2-ql', 'owl2-dl'];

  let regime: string = 'owl2-rl';
  let eqRef = false;
  let runAsync = false;
  let target = '';
  let running = false;
  let report: MaterializeReport | null = null;
  let failure: { status: number | null; message: string; body: Record<string, unknown> | null } | null = null;

  $: options = REGIMES.map((r) => ({ value: r, label: $t(`components.reasoning.regime.${r}`), disabled: r === 'owl2-dl' && !dlBackend }));
  $: defaultTarget = `urn:entailment:${regime}:${datasetId}`;
  $: if (regime === 'owl2-dl') runAsync = true;

  async function run() {
    running = true;
    report = null;
    failure = null;
    try {
      const body: Parameters<typeof materializeEntailment>[0] = { regime: regime as MaterializeRegime, dataset: datasetId };
      if (regime === 'owl2-rl' && eqRef) body.eq_ref = true;
      if (target.trim()) body.target_graph = target.trim();
      const res = await materializeEntailment(body, runAsync);
      if ('job_id' in res) {
        toastInfo($t('components.reasoning.queued'));
        dispatch('queued', { jobId: res.job_id });
      } else {
        report = res;
      }
    } catch (e) {
      const err = e as { status?: number; message?: string; body?: Record<string, unknown> };
      failure = { status: err?.status ?? null, message: err?.message || String(e), body: (err?.body as Record<string, unknown>) ?? null };
    } finally {
      running = false;
    }
  }
</script>

<div class="card rz-card" id="materialise">
  <div class="rz-head">
    <div class="rz-head-left"><Play size={15} /><h3>{$t('components.reasoning.materialiseHeading')}</h3></div>
  </div>
  <p class="rz-intro">{$t('components.reasoning.materialiseIntro')}</p>
  {#if canWrite}
    <div class="rz-grid">
      <div class="rz-field">
        <span class="rz-label">{$t('components.reasoning.regimeLabel')}</span>
        <Select bind:value={regime} {options} ariaLabel={$t('components.reasoning.materialiseRegime')} disabled={running} />
      </div>
      <div class="rz-field">
        <label for="mat-target">{$t('components.reasoning.targetGraph')}</label>
        <input id="mat-target" bind:value={target} placeholder={defaultTarget} disabled={running} />
      </div>
    </div>
    <div class="rz-row">
      {#if regime === 'owl2-rl'}
        <label class="rz-check"><input type="checkbox" bind:checked={eqRef} disabled={running} /> {$t('components.reasoning.eqRef')}</label>
      {/if}
      <label class="rz-check"><input type="checkbox" bind:checked={runAsync} disabled={running || regime === 'owl2-dl'} /> {$t('components.reasoning.runAsync')}</label>
    </div>
    <div class="rz-row">
      <button class="btn btn-sm" on:click={run} disabled={running || (regime === 'owl2-dl' && !dlBackend)}>
        {#if running}<Loader2 size={13} class="animate-spin" />{:else}<Play size={13} />{/if}
        {$t('components.reasoning.materialise')}
      </button>
    </div>
    {#if failure}<RunFailure {...failure} />{/if}
    {#if report}<MaterialiseReport {report} />{/if}
  {:else}
    <p class="rz-muted">{$t('components.reasoning.writeNeeded')}</p>
  {/if}
</div>
