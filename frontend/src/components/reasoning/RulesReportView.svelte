<script lang="ts">
  // How the last run of a dataset's stored rules went (RulesReport).
  import { t } from 'svelte-i18n';
  import type { RulesReport } from '../../lib/api';
  import './reasoning.css';

  export let report: RulesReport;
</script>

<div class="rz-card" data-testid="rules-report">
  <dl class="rz-dl">
    <dt>{$t('components.swrl.rulesRun')}</dt><dd>{report.rules}</dd>
    <dt>{$t('components.swrl.triplesInferred')}</dt><dd><strong>{report.triples_inferred}</strong></dd>
    <dt>{$t('components.swrl.rounds')}</dt><dd>{report.rounds}</dd>
    <dt>{$t('components.swrl.converged')}</dt>
    <dd>
      {#if report.converged}<span class="rz-badge rz-ok">{$t('components.swrl.yes')}</span>
      {:else}<span class="rz-badge rz-warn">{$t('components.swrl.no')}</span>{/if}
    </dd>
    <dt>{$t('components.reasoning.targetGraph')}</dt><dd><code>{report.target_graph}</code></dd>
    <dt>{$t('components.swrl.ranAt')}</dt><dd>{new Date(report.ran_at).toLocaleString()}</dd>
  </dl>
  {#if report.error}<p class="rz-msg rz-msg-bad" role="alert">{report.error}</p>{/if}
</div>
