<script>
  // One query result, shown as whichever visual its rows support: a tab per
  // applicable view (lib/chatViz.inferViews) over the same rows — bar or line
  // chart, MapLibre map, cytoscape node-link graph, SHACL summary, KPI tiles —
  // with the table always available. `preferred` is the starting view: a
  // ```view block's "as" (or auto), or `table` where the result used to be a
  // plain table. A preferred view the rows cannot support starts on the table
  // and says so: the widget never draws something the query did not return.
  import { t } from 'svelte-i18n';
  import {
    resolveView,
    buildChart,
    buildStats,
    mapElementsFromTable,
    graphFromTable,
    shaclFromTable,
  } from '../../lib/chatViz';
  import ChatChart from './ChatChart.svelte';
  import ChatGraph from './ChatGraph.svelte';
  import ChatResultMap from './ChatResultMap.svelte';
  import ChatShaclReport from './ChatShaclReport.svelte';
  import ChatStats from './ChatStats.svelte';
  import CsvPreview from './CsvPreview.svelte';

  /** @type {{columns: string[], rows: string[][]}} */
  export let table = { columns: [], rows: [] };
  /** Starting view: 'auto', 'table', or a VizKind (aliases resolved by chatViz). */
  export let preferred = 'auto';
  export let title = '';
  /** Column hints for charts (a ```view block's "x"/"y"). */
  export let x = '';
  export let y = '';
  /** True when the query hit its row cap — counts are then of the rows shown. */
  export let truncated = false;
  /** Typed node-link elements (Run blocks parse real N-Triples); else built from the table. */
  export let graph = null;

  $: hints = { x, y };
  $: resolved = resolveView(table, preferred, hints);
  // The user's tab choice wins until the result itself changes — keyed on the
  // rows array, since callers may pass a fresh `table` wrapper every render.
  let chosen = null;
  let lastRows = null;
  $: if (table?.rows !== lastRows) {
    lastRows = table?.rows;
    chosen = null;
  }
  $: view = chosen && resolved.views.includes(chosen) ? chosen : resolved.view;

  $: chart = view === 'bar' || view === 'line' ? buildChart(table, view, { ...hints, title }) : null;
  $: tiles = view === 'stat' ? buildStats(table) : [];
  $: mapElements = view === 'map' ? mapElementsFromTable(table) : [];
  $: graphEls = view === 'graph' ? graph || graphFromTable(table) : null;
  $: shacl = view === 'shacl' ? shaclFromTable(table) : null;
</script>

<div class="result-visual">
  {#if resolved.views.length > 1}
    <div class="tabs" role="tablist" aria-label={$t('components.chat.views.label')}>
      {#each resolved.views as v}
        <button
          type="button"
          role="tab"
          class="tab"
          class:active={v === view}
          aria-selected={v === view}
          on:click={() => { chosen = v; }}
        >{$t(`components.chat.views.${v}`)}</button>
      {/each}
    </div>
  {/if}
  {#if resolved.fellBack && !chosen}
    <p class="fallback" role="note">
      {$t('components.chat.views.fellBack', { values: { view: $t(`components.chat.views.${resolved.fellBack}`) } })}
    </p>
  {/if}

  {#if view === 'bar' || view === 'line'}
    {#if chart}
      <ChatChart spec={chart.spec} />
      {#if chart.shown < chart.total || truncated}
        <p class="note">
          {$t('components.chat.views.chartShown', { values: { shown: chart.shown, total: chart.total } })}
          {#if truncated}{$t('components.chat.views.capped')}{/if}
        </p>
      {/if}
    {/if}
  {:else if view === 'stat'}
    <ChatStats {tiles} />
  {:else if view === 'map'}
    <ChatResultMap elements={mapElements} total={table.rows.length} />
  {:else if view === 'graph'}
    <ChatGraph nodes={graphEls.nodes} edges={graphEls.edges} total={table.rows.length} />
  {:else if view === 'shacl'}
    <ChatShaclReport findings={shacl} {truncated} />
  {:else}
    <slot name="table">
      {#if title}<p class="title">{title}</p>{/if}
      <CsvPreview columns={table.columns} rows={table.rows} framed={false} downloadable={false} />
    </slot>
  {/if}
</div>

<style>
  .result-visual { margin: 0 0 0.35rem; }
  .tabs { display: flex; flex-wrap: wrap; gap: 0.25rem; margin: 0.35rem 0 0.4rem; }
  .tab {
    cursor: pointer; font-size: 0.7rem; font-weight: 600; padding: 2px 9px; border-radius: 999px;
    border: 1px solid var(--line-soft); background: var(--bg-strong); color: var(--ink-500);
  }
  .tab:hover { border-color: var(--line-strong); color: var(--ink-700); }
  .tab.active { background: var(--brand-50, #eff6ff); border-color: var(--brand-300, #93c5fd); color: var(--brand-700, #1d4ed8); }
  .fallback, .note { margin: 0.2rem 0 0.35rem; font-size: 0.72rem; color: var(--ink-400); }
  .fallback { font-style: italic; }
  .title { margin: 0 0 0.3rem; font-size: 0.8rem; font-weight: 700; color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .tab.active { background: rgba(59,130,246,0.15); border-color: rgba(59,130,246,0.4); color: #93c5fd; }
</style>
