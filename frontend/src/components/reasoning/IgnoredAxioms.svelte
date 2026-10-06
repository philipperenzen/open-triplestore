<script lang="ts">
  // The axioms a regime left out because they are outside its profile: the
  // EL / QL "ignored axioms" report of a materialisation.
  import { t } from 'svelte-i18n';
  import { normaliseIgnored, ignoredSample } from '../../lib/reasoning';
  import './reasoning.css';

  /** A materialisation report (or anything with `ignored` / `ignored_axioms`). */
  export let report: Parameters<typeof normaliseIgnored>[0];

  $: rows = normaliseIgnored(report);
  $: sample = ignoredSample(report);
  $: total = rows.reduce((n, r) => n + (r.count ?? 0), 0);
</script>

{#if rows.length}
  <div class="rz-card" data-testid="ignored-axioms">
    <p class="rz-msg rz-msg-warn">{$t('components.reasoning.ignoredIntro', { values: { count: total } })}</p>
    <div class="rz-table-wrap">
      <table class="rz-table">
        <thead>
          <tr>
            <th>{$t('components.reasoning.ignoredConstruct')}</th>
            <th class="rz-num">{$t('components.reasoning.ignoredCount')}</th>
            <th>{$t('components.reasoning.ignoredExample')}</th>
          </tr>
        </thead>
        <tbody>
          {#each rows as r}
            <tr>
              <td>{r.construct}</td>
              <td class="rz-num">{r.count}</td>
              <td>{#if r.example}<code>{r.example}</code>{:else}—{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if sample.length > 1}
      <details class="rz-details">
        <summary>{$t('components.reasoning.ignoredSample', { values: { count: sample.length } })}</summary>
        <ul class="rz-list">{#each sample as a}<li><code>{a}</code></li>{/each}</ul>
      </details>
    {/if}
  </div>
{/if}
