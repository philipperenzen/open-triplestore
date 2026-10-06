<script lang="ts">
  // Run SWRL rules once (POST /api/swrl/execute), over a dataset's reasoning
  // sources or the default graph, optionally jointly with an entailment
  // regime to one fixed point. Every rule is checked before any runs: a
  // refusal names the rule, atom or built-in, and nothing is written.
  import { t } from 'svelte-i18n';
  import { Loader2, Zap } from 'lucide-svelte';
  import Select from '../Select.svelte';
  import SwrlRuleEditor from './SwrlRuleEditor.svelte';
  import SwrlReport from './SwrlReport.svelte';
  import { executeSwrl, type MaterializeRegime, type SwrlExecuteReport, type SwrlFormat } from '../../lib/api';
  import { parsePrefixLines } from '../../lib/reasoning';
  import './reasoning.css';

  /** The dataset whose reasoning sources rule bodies read; null: the default graph. */
  export let datasetId: string | null = null;
  export let canWrite = true;
  /** Where derived triples go by default (shown as the placeholder). */
  export let inferenceGraph = '';

  let format: SwrlFormat = 'swrlapi';
  let rules = '';
  let prefixText = '';
  let rdfFormat = 'turtle';
  let baseIri = '';
  let regime = '';
  let target = '';
  let maxIterations = 100;
  let running = false;
  let report: SwrlExecuteReport | null = null;
  let error = '';

  $: regimeOptions = [
    { value: '', label: $t('components.swrl.noRegime') },
    ...(['rdfs', 'owl2-rl', 'owl2-el', 'owl2-ql', 'owl2-dl'] as const).map((r) => ({ value: r, label: $t(`components.reasoning.regime.${r}`) })),
  ];

  async function run() {
    const { prefixes, bad } = parsePrefixLines(prefixText);
    if (format === 'swrlapi' && bad.length) {
      error = $t('components.swrl.badPrefixLines', { values: { lines: bad.join(', ') } });
      return;
    }
    running = true;
    error = '';
    report = null;
    try {
      report = await executeSwrl({
        rules,
        format,
        prefixes: format === 'swrlapi' ? prefixes : undefined,
        rdf_format: format === 'rdf' ? rdfFormat : undefined,
        base_iri: format === 'rdf' && baseIri.trim() ? baseIri.trim() : undefined,
        dataset: datasetId ?? undefined,
        regime: (regime || undefined) as MaterializeRegime | undefined,
        target_graph: target.trim() || undefined,
        max_iterations: maxIterations,
      });
    } catch (e) {
      error = (e as Error)?.message || String(e);
    } finally {
      running = false;
    }
  }
</script>

<div class="card rz-card" id="swrl-run">
  <div class="rz-head">
    <div class="rz-head-left"><Zap size={15} /><h3>{$t('components.swrl.runHeading')}</h3></div>
  </div>
  <p class="rz-intro">{datasetId ? $t('components.swrl.runIntroDataset') : $t('components.swrl.runIntro')}</p>
  {#if canWrite}
    <SwrlRuleEditor bind:format bind:rules bind:prefixText bind:rdfFormat bind:baseIri disabled={running} idPrefix="run" />
    <div class="rz-grid">
      <div class="rz-field">
        <span class="rz-label">{$t('components.swrl.regimeLabel')}</span>
        <Select bind:value={regime} options={regimeOptions} ariaLabel={$t('components.swrl.regimeLabel')} disabled={running} />
      </div>
      <div class="rz-field">
        <label for="swrl-target">{$t('components.reasoning.targetGraph')}</label>
        <input id="swrl-target" bind:value={target} placeholder={inferenceGraph || $t('components.reasoning.defaultGraph')} disabled={running} />
      </div>
      <div class="rz-field">
        <label for="swrl-iter">{$t('components.swrl.maxIterations')}</label>
        <input id="swrl-iter" type="number" min="1" max="1000" bind:value={maxIterations} disabled={running} />
      </div>
    </div>
    {#if regime && !datasetId}
      <p class="rz-muted">{$t('components.swrl.regimeNeedsScope')}</p>
    {/if}
    <div class="rz-row">
      <button class="btn btn-sm" on:click={run} disabled={running || !rules.trim()}>
        {#if running}<Loader2 size={13} class="animate-spin" />{:else}<Zap size={13} />{/if}
        {$t('components.swrl.execute')}
      </button>
    </div>
    {#if error}<p class="rz-msg rz-msg-bad" role="alert" data-testid="swrl-error">{error}</p>{/if}
    {#if report}<SwrlReport {report} />{/if}
  {:else}
    <p class="rz-muted">{$t('components.reasoning.writeNeeded')}</p>
  {/if}
</div>
