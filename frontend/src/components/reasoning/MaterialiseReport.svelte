<script lang="ts">
  // What a materialisation did: `POST /api/reasoning/materialize`'s report.
  import { t } from 'svelte-i18n';
  import IgnoredAxioms from './IgnoredAxioms.svelte';
  import type { MaterializeReport } from '../../lib/api';
  import './reasoning.css';

  export let report: MaterializeReport;
</script>

<div class="rz-card" data-testid="materialise-report">
  <dl class="rz-dl">
    <dt>{$t('components.reasoning.regimeLabel')}</dt><dd>{report.regime}</dd>
    <dt>{$t('components.reasoning.triplesAdded')}</dt><dd><strong>{report.triples_added}</strong></dd>
    <dt>{$t('components.reasoning.iterations')}</dt><dd>{report.iterations}</dd>
    <dt>{$t('components.reasoning.elapsed')}</dt><dd>{report.elapsed_ms} ms</dd>
    <dt>{$t('components.reasoning.targetGraph')}</dt><dd><code>{report.target_graph}</code></dd>
    <dt>{$t('components.reasoning.consistency')}</dt>
    <dd>
      {#if report.consistent === true}<span class="rz-badge rz-ok">{$t('components.reasoning.consistent')}</span>
      {:else}<span class="rz-muted">{$t('components.reasoning.notChecked')}</span>{/if}
    </dd>
    {#if report.backend}
      <dt>{$t('components.reasoning.backendRan')}</dt>
      <dd>
        <code>{report.backend}</code>{#if report.backend_version} {report.backend_version}{/if}
        {#if report.complete === false}<span class="rz-badge rz-warn">{$t('components.reasoning.incomplete')}</span>
        {:else if report.complete === true}<span class="rz-badge rz-ok">{$t('components.reasoning.complete')}</span>{/if}
      </dd>
    {/if}
    {#if report.identity}
      <dt>{$t('components.reasoning.identityApplied')}</dt><dd>{$t(`components.identityPolicy.option.${report.identity}`)}</dd>
    {/if}
    <dt>{$t('components.reasoning.sourcesLabel')}</dt>
    <dd>{report.sources ? report.sources.length : $t('components.reasoning.defaultGraph')}</dd>
  </dl>
  {#if report.warnings?.length}
    <ul class="rz-list rz-msg rz-msg-warn">{#each report.warnings as w}<li>{w}</li>{/each}</ul>
  {/if}
  <IgnoredAxioms {report} />
</div>
