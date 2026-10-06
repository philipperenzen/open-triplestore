<script lang="ts">
  // The outcome of an IDS check, one row per specification: passed, or which
  // entities fail a requirement, or that the model has no applicable entity
  // although the specification requires one. The rows come from
  // summarizeIdsRun over the SHACL report of the pipeline run.
  import { t } from 'svelte-i18n';
  import { CheckCircle2, CircleAlert, ChevronDown, ChevronRight } from 'lucide-svelte';
  import { summarizeIdsRun } from '../../lib/exchange';
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import './exchange.css';

  /** `{shape, label}` of each shape in the IDS shape graph. */
  export let specs: { shape: string; label?: string | null }[] = [];
  /** A pipeline run: `{ran_at, conforms, report: {results}}`. */
  export let run: { ran_at?: string; conforms?: boolean; report?: { results?: any[] } | null } | null = null;

  $: rows = summarizeIdsRun(specs, run?.report?.results ?? []);
  $: failed = rows.filter((r) => !r.passed).length;
  let open = new Set<string>();
  function toggle(shape: string) {
    open.has(shape) ? open.delete(shape) : open.add(shape);
    open = open;
  }
  const MAX_ENTITIES = 50;
</script>

{#if run}
  <div class="xc-ids-results" data-testid="ids-results">
    <div class="xc-row">
      {#if failed === 0}
        <span class="xc-badge xc-ok"><CheckCircle2 size={12} /> {$t('components.exchange.ids.allPassed')}</span>
      {:else}
        <span class="xc-badge xc-bad"><CircleAlert size={12} /> {$t('components.exchange.ids.someFailed', { values: { failed, total: rows.length } })}</span>
      {/if}
      {#if run.ran_at}<span class="xc-muted">{new Date(run.ran_at).toLocaleString()}</span>{/if}
    </div>
    {#if rows.length === 0}
      <p class="xc-muted">{$t('components.exchange.ids.noSpecs')}</p>
    {:else}
      <div class="xc-table-wrap">
        <table class="xc-table">
          <thead>
            <tr>
              <th>{$t('components.exchange.ids.specification')}</th>
              <th>{$t('components.exchange.ids.status')}</th>
              <th>{$t('components.exchange.ids.failingEntities')}</th>
            </tr>
          </thead>
          <tbody>
            {#each rows as r (r.shape)}
              <tr>
                <td>
                  {#if !r.passed}
                    <button class="icon-btn" on:click={() => toggle(r.shape)} aria-expanded={open.has(r.shape)} aria-label={$t('components.exchange.ids.details')}>
                      {#if open.has(r.shape)}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
                    </button>
                  {/if}
                  {r.name}
                </td>
                <td>
                  {#if r.passed}
                    <span class="xc-badge xc-ok">{$t('components.exchange.ids.pass')}</span>
                  {:else if r.missing && r.failing.length === 0}
                    <span class="xc-badge xc-bad">{$t('components.exchange.ids.noApplicable')}</span>
                  {:else}
                    <span class="xc-badge xc-bad">{$t('components.exchange.ids.fail')}</span>
                  {/if}
                </td>
                <td>{r.failing.length}</td>
              </tr>
              {#if open.has(r.shape)}
                <tr>
                  <td colspan="3">
                    {#if r.messages.length}
                      <ul class="xc-list">{#each r.messages as m}<li>{m}</li>{/each}</ul>
                    {/if}
                    {#if r.failing.length}
                      <div class="xc-row">
                        {#each r.failing.slice(0, MAX_ENTITIES) as e}<code class="xc-code" title={e}>{shortenIRI(e)}</code>{/each}
                      </div>
                      {#if r.failing.length > MAX_ENTITIES}
                        <p class="xc-muted">{$t('components.exchange.ids.more', { values: { n: r.failing.length - MAX_ENTITIES } })}</p>
                      {/if}
                    {/if}
                  </td>
                </tr>
              {/if}
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
{/if}
