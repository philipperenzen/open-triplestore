<script lang="ts">
  // The answer of an OWL 2 DL check (POST /api/reasoning/check).
  import { t } from 'svelte-i18n';
  import type { CheckResult } from '../../lib/api';
  import './reasoning.css';

  export let result: CheckResult;

  $: tone = result.result === 'unknown' ? 'warn' : result.result === 'true' ? 'ok' : 'bad';
</script>

<div class="rz-card" data-testid="check-result">
  <p class="rz-msg rz-msg-{tone}">
    <strong>{$t(`components.reasoning.checkAnswer.${result.task}.${result.result}`)}</strong>
  </p>
  <dl class="rz-dl">
    <dt>{$t('components.reasoning.checkTask')}</dt><dd>{$t(`components.reasoning.task.${result.task}`)}</dd>
    {#if result.backend}
      <dt>{$t('components.reasoning.backendRan')}</dt>
      <dd>
        <code>{result.backend}</code>{#if result.backend_version} {result.backend_version}{/if}
        {#if result.complete === false}<span class="rz-badge rz-warn">{$t('components.reasoning.incomplete')}</span>
        {:else if result.complete === true}<span class="rz-badge rz-ok">{$t('components.reasoning.complete')}</span>{/if}
      </dd>
    {/if}
    {#if typeof result.rule === 'string'}
      <dt>{$t('components.reasoning.inconsistencyRule')}</dt><dd><code>{result.rule}</code> {result.detail ?? ''}</dd>
    {:else if result.detail}
      <dt>{$t('components.reasoning.detail')}</dt><dd>{result.detail}</dd>
    {/if}
    <dt>{$t('components.reasoning.sourcesLabel')}</dt>
    <dd>{result.sources ? result.sources.length : $t('components.reasoning.premiseOrDefault')}</dd>
  </dl>
  {#if result.violations?.length}
    <div class="rz-table-wrap">
      <table class="rz-table">
        <thead><tr><th>{$t('components.reasoning.violationRule')}</th><th>{$t('components.reasoning.detail')}</th></tr></thead>
        <tbody>{#each result.violations as v}<tr><td><code>{v.rule}</code></td><td>{v.detail}</td></tr>{/each}</tbody>
      </table>
    </div>
  {/if}
  {#if result.warnings?.length}
    <ul class="rz-list rz-msg rz-msg-warn">{#each result.warnings as w}<li>{w}</li>{/each}</ul>
  {/if}
</div>
