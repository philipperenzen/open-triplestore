<script lang="ts">
  // A dataset's entailment regime, mode and identity policy over
  // GET/PUT /api/datasets/:id/entailment (docs/reasoning.md), with the last
  // run: its status, consistency, the DL backend that ran and the stored
  // rules. An `owl2-dl` run goes to the background; while it is queued or
  // running this view polls until it settles.
  import { createEventDispatcher, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Brain, Loader2, RefreshCw } from 'lucide-svelte';
  import Select from '../Select.svelte';
  import { getEntailment, setEntailment, type EntailmentRegime, type EntailmentState, type IdentityPolicyName } from '../../lib/api';
  import { statusTone } from '../../lib/reasoning';
  import { toastError, toastSuccess } from '../../lib/toast';
  import './reasoning.css';

  export let datasetId: string;
  export let canWrite = false;
  /** Poll interval while a background run is queued or running (ms). */
  export let pollMs = 2000;
  /** Bump to reload the state (the stored rules changed) without resetting the form. */
  export let refreshKey = 0;

  const dispatch = createEventDispatcher<{ loaded: EntailmentState }>();
  const REGIMES: EntailmentRegime[] = ['rdfs', 'owl2-rl', 'owl2-el', 'owl2-ql', 'owl2-dl', 'skos'];

  let state: EntailmentState | null = null;
  let loading = true;
  let loadError = '';
  let saving = false;
  let runError: { message: string; violations?: { rule: string; detail: string }[] } | null = null;
  let regime: string = 'owl2-rl';
  let mode: string = 'materialize';
  let identity: string = 'inherit';
  let eqRef = false;
  let timer: ReturnType<typeof setTimeout> | null = null;

  async function load(fill = true, target = datasetId) {
    try {
      const s = await getEntailment(target);
      state = s;
      loadError = '';
      if (fill) {
        regime = s.regime ?? 'owl2-rl';
        mode = s.regime ? s.mode : 'materialize';
        identity = s.identity_source === 'dataset' ? s.identity : 'inherit';
        eqRef = !!s.eq_ref;
      }
      dispatch('loaded', s);
      schedulePoll();
    } catch (e) {
      loadError = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
  }

  function schedulePoll() {
    if (timer) clearTimeout(timer);
    timer = null;
    if (state && (state.status === 'queued' || state.status === 'running')) {
      timer = setTimeout(() => load(false), pollMs);
    }
  }

  onDestroy(() => { if (timer) clearTimeout(timer); });

  $: load(true, datasetId);
  $: if (refreshKey) load(false);

  $: dlUnavailable = !!state && !state.dl_backend;
  $: regimeOptions = REGIMES.map((r) => ({
    value: r,
    label: $t(`components.reasoning.regime.${r}`),
    disabled: r === 'owl2-dl' && dlUnavailable,
  }));
  $: modeOptions = [
    { value: 'materialize', label: $t('components.reasoning.mode.materialize') },
    { value: 'off', label: $t('components.reasoning.mode.off') },
  ];
  $: identityOptions = [
    { value: 'inherit', label: $t('components.reasoning.identityInherit') },
    ...(state?.identity_options ?? ['sameas-off', 'sameas-narrow', 'sameas-full']).map((p) => ({
      value: p, label: $t(`components.identityPolicy.option.${p}`),
    })),
  ];
  $: eqRefSupported = !!state && 'eq_ref' in state;
  $: eqRefApplies = regime === 'owl2-rl' || regime === 'skos';
  $: busy = state?.status === 'queued' || state?.status === 'running';

  async function save() {
    saving = true;
    runError = null;
    try {
      const body: Parameters<typeof setEntailment>[1] = {
        regime: regime as EntailmentRegime,
        mode: mode as 'materialize' | 'off',
        identity: identity as IdentityPolicyName | 'inherit',
      };
      if (eqRefSupported) body.eq_ref = eqRefApplies && eqRef;
      await setEntailment(datasetId, body);
      toastSuccess($t('components.reasoning.saved'));
    } catch (e) {
      const err = e as { message?: string; body?: { violations?: { rule: string; detail: string }[] } };
      runError = { message: err?.message || String(e), violations: err?.body?.violations };
      if ((e as { status?: number })?.status !== 422) toastError(runError.message);
    } finally {
      saving = false;
      await load(false);
    }
  }

  function fmt(ts: string | null | undefined) {
    if (!ts) return '—';
    const d = new Date(ts);
    return Number.isNaN(d.getTime()) ? ts : d.toLocaleString();
  }
</script>

<div class="card rz-card" id="entailment-settings">
  <div class="rz-head">
    <div class="rz-head-left">
      <Brain size={15} />
      <h3>{$t('components.reasoning.settingsHeading')}</h3>
    </div>
    <button class="btn btn-sm btn-ghost" on:click={() => load(false)} title={$t('components.reasoning.refresh')} aria-label={$t('components.reasoning.refresh')}>
      <RefreshCw size={13} />
    </button>
  </div>
  <p class="rz-intro">{$t('components.reasoning.settingsIntro')}</p>

  {#if loading && !state}
    <p class="rz-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
  {:else if loadError}
    <p class="rz-msg rz-msg-bad">{loadError}</p>
  {:else if state}
    <dl class="rz-dl" aria-label={$t('components.reasoning.inForce')}>
      <dt>{$t('components.reasoning.regimeLabel')}</dt>
      <dd>
        {#if state.regime}
          <strong>{$t(`components.reasoning.regime.${state.regime}`)}</strong>
          <span class="rz-badge">{$t(`components.reasoning.mode.${state.mode}`)}</span>
        {:else}
          {$t('components.reasoning.noRegime')}
        {/if}
      </dd>
      {#if state.regime}
        <dt>{$t('components.reasoning.status')}</dt>
        <dd>
          <span class="rz-badge rz-{statusTone(state.status)}" data-testid="entailment-status">
            {#if busy}<Loader2 size={11} class="animate-spin" />{/if}
            {state.status ? $t(`components.reasoning.runStatus.${state.status}`) : '—'}
          </span>
          <span class="rz-muted">{$t('components.reasoning.lastRun', { values: { when: fmt(state.last_run_at) } })}</span>
        </dd>
        <dt>{$t('components.reasoning.entailedTriples')}</dt>
        <dd>{state.last_triples ?? '—'} <code>{state.graph}</code></dd>
        <dt>{$t('components.reasoning.consistency')}</dt>
        <dd>
          {#if state.consistent === true}<span class="rz-badge rz-ok">{$t('components.reasoning.consistent')}</span>
          {:else if state.consistent === false}<span class="rz-badge rz-bad">{$t('components.reasoning.inconsistent')}</span>
          {:else}<span class="rz-muted">{$t('components.reasoning.notChecked')}</span>{/if}
        </dd>
        {#if state.inconsistency}
          <dt>{$t('components.reasoning.inconsistencyRule')}</dt>
          <dd><code>{state.inconsistency.rule}</code> — {state.inconsistency.detail}</dd>
        {/if}
        {#if state.error}
          <dt>{$t('components.reasoning.error')}</dt>
          <dd class="rz-del">{state.error}</dd>
        {/if}
        {#if state.backend}
          <dt>{$t('components.reasoning.backendRan')}</dt>
          <dd>
            <code>{state.backend}</code>
            {#if state.complete === false}<span class="rz-badge rz-warn">{$t('components.reasoning.incomplete')}</span>
            {:else if state.complete === true}<span class="rz-badge rz-ok">{$t('components.reasoning.complete')}</span>{/if}
          </dd>
        {/if}
      {/if}
      <dt>{$t('components.reasoning.dlBackend')}</dt>
      <dd data-testid="dl-backend">
        {#if state.dl_backend}<code>{state.dl_backend}</code>{:else}<span class="rz-muted">{$t('components.reasoning.dlBackendNone')}</span>{/if}
      </dd>
      <dt>{$t('components.reasoning.identity')}</dt>
      <dd>
        {$t(`components.identityPolicy.option.${state.identity}`)}
        <span class="rz-muted">{$t(`components.identityPolicy.source.${state.identity_source}`)}</span>
      </dd>
      <dt>{$t('components.reasoning.inferenceGraph')}</dt>
      <dd><code>{state.inference_graph}</code></dd>
      {#if state.rules}
        <dt>{$t('components.reasoning.storedRules')}</dt>
        <dd>
          {$t('components.reasoning.storedRulesCount', { values: { count: state.rules.count } })}
          {#if state.rules.error}<span class="rz-del">{state.rules.error}</span>{/if}
          {#if state.rules.last_run}
            <span class="rz-muted">{$t('components.reasoning.rulesLastRun', { values: { triples: state.rules.last_run.triples_inferred, rounds: state.rules.last_run.rounds } })}</span>
          {/if}
        </dd>
      {/if}
    </dl>

    {#if state.reasoning_sources?.length}
      <details class="rz-details">
        <summary>{$t('components.reasoning.sources', { values: { count: state.reasoning_sources.length } })}</summary>
        <ul class="rz-list">{#each state.reasoning_sources as g}<li><code>{g}</code></li>{/each}</ul>
      </details>
    {/if}

    {#if canWrite}
      <div class="rz-grid">
        <div class="rz-field">
          <span class="rz-label" id="ent-regime-label">{$t('components.reasoning.regimeLabel')}</span>
          <Select bind:value={regime} options={regimeOptions} ariaLabel={$t('components.reasoning.regimeLabel')} disabled={saving} />
        </div>
        <div class="rz-field">
          <span class="rz-label">{$t('components.reasoning.modeLabel')}</span>
          <Select bind:value={mode} options={modeOptions} ariaLabel={$t('components.reasoning.modeLabel')} disabled={saving} />
        </div>
        <div class="rz-field">
          <span class="rz-label">{$t('components.reasoning.identity')}</span>
          <Select bind:value={identity} options={identityOptions} ariaLabel={$t('components.reasoning.identity')} disabled={saving} />
        </div>
      </div>
      {#if regime === 'owl2-dl' && dlUnavailable}
        <p class="rz-msg rz-msg-warn">{$t('components.reasoning.dlUnavailableHint')}</p>
      {/if}
      {#if regime === 'skos'}
        <p class="rz-muted">{$t('components.reasoning.skosHint')}</p>
      {/if}
      {#if eqRefSupported && eqRefApplies}
        <label class="rz-check"><input type="checkbox" bind:checked={eqRef} disabled={saving} /> {$t('components.reasoning.eqRef')}</label>
      {/if}
      <div class="rz-row">
        <button class="btn btn-sm" on:click={save} disabled={saving || (regime === 'owl2-dl' && dlUnavailable)}>
          {#if saving}<Loader2 size={13} class="animate-spin" />{/if}
          {mode === 'materialize' ? $t('components.reasoning.saveAndRun') : $t('system.save')}
        </button>
        <span class="rz-muted">{$t('components.reasoning.saveHint')}</span>
      </div>
      {#if runError}
        <div class="rz-msg rz-msg-bad" role="alert">
          {runError.message}
          {#if runError.violations?.length}
            <ul class="rz-list">{#each runError.violations as v}<li><code>{v.rule}</code> {v.detail}</li>{/each}</ul>
          {/if}
        </div>
      {/if}
    {:else}
      <p class="rz-muted">{$t('components.reasoning.readOnly')}</p>
    {/if}
  {/if}
</div>
