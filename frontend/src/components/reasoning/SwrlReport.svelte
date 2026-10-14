<script lang="ts">
  // What POST /api/swrl/execute did: the fixed point, why it stopped, and —
  // run with a regime — the rounds of rules-then-regime and the regime's own
  // triples; per rule the SPARQL it ran.
  import { t } from 'svelte-i18n';
  import type { SwrlExecuteReport } from '../../lib/api';
  import './reasoning.css';

  export let report: SwrlExecuteReport;
</script>

<div class="rz-card" data-testid="swrl-report">
  <dl class="rz-dl">
    <dt>{$t('components.swrl.rulesRun')}</dt><dd>{report.rules_count}</dd>
    <dt>{$t('components.swrl.triplesInferred')}</dt><dd><strong>{report.triples_inferred}</strong></dd>
    <dt>{$t('components.swrl.iterations')}</dt><dd>{report.iterations}</dd>
    <dt>{$t('components.swrl.converged')}</dt>
    <dd>
      {#if report.converged}<span class="rz-badge rz-ok">{$t('components.swrl.yes')}</span>
      {:else}<span class="rz-badge rz-warn">{$t('components.swrl.no')}</span>{/if}
      <span class="rz-muted">{$t(`components.swrl.stop.${report.stop_reason}`)}</span>
    </dd>
    {#if report.regime}
      <dt>{$t('components.reasoning.regimeLabel')}</dt><dd>{report.regime}</dd>
      <dt>{$t('components.swrl.rounds')}</dt><dd data-testid="swrl-rounds">{report.rounds ?? '—'}</dd>
      <dt>{$t('components.swrl.regimeTriples')}</dt><dd data-testid="swrl-regime-triples">{report.regime_triples ?? '—'}</dd>
    {/if}
    {#if report.target_graph}<dt>{$t('components.reasoning.targetGraph')}</dt><dd><code>{report.target_graph}</code></dd>{/if}
    <dt>{$t('components.reasoning.sourcesLabel')}</dt>
    <dd>{report.sources ? report.sources.length : $t('components.reasoning.defaultGraph')}</dd>
  </dl>
  {#if report.rule_results?.length}
    <details class="rz-details">
      <summary>{$t('components.swrl.perRule', { values: { count: report.rule_results.length } })}</summary>
      {#each report.rule_results as r}
        <div class="rz-set">
          <div class="rz-set-head">
            <span class="rz-set-title">{r.rule_name}</span>
            {#if r.success}<span class="rz-badge rz-ok">{$t('components.swrl.ok')}</span>{:else}<span class="rz-badge rz-bad">{$t('components.swrl.failed')}</span>{/if}
          </div>
          {#if r.error}<p class="rz-msg rz-msg-bad">{r.error}</p>{/if}
          <pre class="rz-pre">{r.sparql}</pre>
        </div>
      {/each}
    </details>
  {/if}
</div>
