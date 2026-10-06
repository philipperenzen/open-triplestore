<script lang="ts">
  // OWL 2 DL checks (POST /api/reasoning/check): consistency, profile,
  // class satisfiability or entailment, over the dataset (its readable
  // conformance layer) or a Turtle premise; in the background on request.
  // The profile check needs no DL backend, the others do.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Loader2, ShieldQuestion } from 'lucide-svelte';
  import Select from '../Select.svelte';
  import CheckResultView from './CheckResultView.svelte';
  import RunFailure from './RunFailure.svelte';
  import { reasoningCheck, type CheckResult, type CheckTask } from '../../lib/api';
  import { toastInfo } from '../../lib/toast';
  import './reasoning.css';

  /** The dataset to check; null: a Turtle premise only (or the default graph). */
  export let datasetId: string | null = null;
  export let dlBackend: string | null = null;

  const dispatch = createEventDispatcher<{ queued: { jobId: string } }>();
  const TASKS: CheckTask[] = ['consistency', 'profile', 'satisfiability', 'entailment'];
  const PREMISE_PLACEHOLDER = '@prefix ex: <https://example.org/> .\nex:A owl:disjointWith ex:B .';

  let task: string = 'consistency';
  let usePremise = !datasetId;
  let premise = '';
  let conclusion = '';
  let klass = '';
  let runAsync = false;
  let running = false;
  let result: CheckResult | null = null;
  let failure: { status: number | null; message: string; body: Record<string, unknown> | null } | null = null;

  $: needsBackend = task !== 'profile';
  $: options = TASKS.map((x) => ({ value: x, label: $t(`components.reasoning.task.${x}`), disabled: x !== 'profile' && !dlBackend }));
  // The backend may be learnt after mount (the page loads it), so the choice
  // is not forced here: a task that needs one is disabled until it is known.

  async function run() {
    running = true;
    result = null;
    failure = null;
    try {
      const body: Parameters<typeof reasoningCheck>[0] = { task: task as CheckTask };
      if (usePremise) body.premise = premise;
      else if (datasetId) body.dataset = datasetId;
      if (task === 'entailment') body.conclusion = conclusion;
      if (task === 'satisfiability') body.class = klass.trim();
      const res = await reasoningCheck(body, runAsync);
      if ('job_id' in res) {
        toastInfo($t('components.reasoning.queued'));
        dispatch('queued', { jobId: res.job_id });
      } else {
        result = res;
      }
    } catch (e) {
      const err = e as { status?: number; message?: string; body?: Record<string, unknown> };
      failure = { status: err?.status ?? null, message: err?.message || String(e), body: (err?.body as Record<string, unknown>) ?? null };
    } finally {
      running = false;
    }
  }
</script>

<div class="card rz-card" id="dl-check">
  <div class="rz-head">
    <div class="rz-head-left"><ShieldQuestion size={15} /><h3>{$t('components.reasoning.checkHeading')}</h3></div>
  </div>
  <p class="rz-intro">{$t('components.reasoning.checkIntro')}</p>
  {#if !dlBackend}
    <p class="rz-msg rz-msg-warn">{$t('components.reasoning.checkNoBackend')}</p>
  {/if}
  <div class="rz-grid">
    <div class="rz-field">
      <span class="rz-label">{$t('components.reasoning.checkTask')}</span>
      <Select bind:value={task} {options} ariaLabel={$t('components.reasoning.checkTask')} disabled={running} />
    </div>
    {#if task === 'satisfiability'}
      <div class="rz-field">
        <label for="check-class">{$t('components.reasoning.checkClass')}</label>
        <input id="check-class" bind:value={klass} placeholder="https://example.org/Class" disabled={running} />
      </div>
    {/if}
  </div>
  {#if datasetId}
    <div class="rz-row">
      <label class="rz-check"><input type="radio" bind:group={usePremise} value={false} disabled={running} /> {$t('components.reasoning.premiseDataset')}</label>
      <label class="rz-check"><input type="radio" bind:group={usePremise} value={true} disabled={running} /> {$t('components.reasoning.premiseTurtle')}</label>
    </div>
  {/if}
  {#if usePremise}
    <div class="rz-field">
      <label for="check-premise">{$t('components.reasoning.premise')}</label>
      <textarea id="check-premise" class="rz-code" bind:value={premise} placeholder={PREMISE_PLACEHOLDER} disabled={running}></textarea>
    </div>
  {/if}
  {#if task === 'entailment'}
    <div class="rz-field">
      <label for="check-conclusion">{$t('components.reasoning.conclusion')}</label>
      <textarea id="check-conclusion" class="rz-code" bind:value={conclusion} disabled={running}></textarea>
    </div>
  {/if}
  <div class="rz-row">
    <label class="rz-check"><input type="checkbox" bind:checked={runAsync} disabled={running} /> {$t('components.reasoning.runAsync')}</label>
    <button class="btn btn-sm" on:click={run} disabled={running || (needsBackend && !dlBackend) || (task === 'satisfiability' && !klass.trim()) || (task === 'entailment' && !conclusion.trim()) || (usePremise && !premise.trim())}>
      {#if running}<Loader2 size={13} class="animate-spin" />{/if}
      {$t('components.reasoning.runCheck')}
    </button>
  </div>
  {#if failure}<RunFailure {...failure} />{/if}
  {#if result}<CheckResultView {result} />{/if}
</div>
