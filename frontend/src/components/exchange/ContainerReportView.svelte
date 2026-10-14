<script lang="ts">
  // A container profile's validation report: conforms or not, the counts,
  // and every finding (violations first) with the file or index node it is
  // about. Used for an import's report, a strict refusal and validate-only.
  import { t } from 'svelte-i18n';
  import { CheckCircle2, CircleAlert } from 'lucide-svelte';
  import { sortFindings, ONTOLOGY_MISSING, type ContainerReport } from '../../lib/exchange';
  import './exchange.css';

  export let report: ContainerReport;

  $: findings = sortFindings(report?.findings ?? []);
  $: ontologyMissing = findings.some((f) => f.code === ONTOLOGY_MISSING);
</script>

<div class="xc-report" data-testid="container-report">
  <div class="xc-row">
    {#if report.conforms}
      <span class="xc-badge xc-ok"><CheckCircle2 size={12} /> {$t('components.exchange.container.conforms')}</span>
    {:else}
      <span class="xc-badge xc-bad"><CircleAlert size={12} /> {$t('components.exchange.container.notConforms')}</span>
    {/if}
    <span class="xc-muted">
      {$t('components.exchange.container.counts', { values: { violations: report.violations, warnings: report.warnings } })}
    </span>
  </div>
  {#if ontologyMissing}
    <p class="xc-muted">{$t('components.exchange.container.ontologyMissingFinding')}</p>
  {/if}
  {#if findings.length}
    <div class="xc-table-wrap">
      <table class="xc-table">
        <thead>
          <tr>
            <th>{$t('components.exchange.container.severity')}</th>
            <th>{$t('components.exchange.container.code')}</th>
            <th>{$t('components.exchange.container.message')}</th>
            <th>{$t('components.exchange.container.where')}</th>
          </tr>
        </thead>
        <tbody>
          {#each findings as f, i (i)}
            <tr>
              <td>
                <span class="xc-badge" class:xc-bad={f.severity === 'violation'} class:xc-warn={f.severity === 'warning'} class:xc-info={f.severity === 'info'}>
                  {$t(`components.exchange.container.sev.${f.severity}`)}
                </span>
              </td>
              <td><code>{f.code}</code></td>
              <td>{f.message}</td>
              <td>
                {#if f.file}<code>{f.file}</code>{/if}
                {#if f.focus}<code title={f.focus}>{f.focus}</code>{/if}
                {#if f.path}<code>{f.path}</code>{/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>
