<script>
  // Tabular preview for ```csv widget blocks and CSV/TSV API results: first rows
  // as a real table, full content downloadable.
  import { t } from 'svelte-i18n';
  import { Download, Table2 } from 'lucide-svelte';

  export let columns = [];
  export let rows = [];
  export let raw = '';
  export let filename = 'data';
  /** Standalone widget (header strip + border) vs embedded in another card. */
  export let framed = true;

  const MAX_SHOWN = 100;
  $: shown = rows.slice(0, MAX_SHOWN);

  function download() {
    const a = document.createElement('a');
    a.href = URL.createObjectURL(new Blob([raw], { type: 'text/csv' }));
    a.download = `${filename}.csv`;
    a.click();
    URL.revokeObjectURL(a.href);
  }
</script>

<div class="csv" class:framed>
  {#if framed}
    <div class="head">
      <span class="label"><Table2 size={12} /> CSV</span>
      <button class="link" on:click={download}><Download size={11} /> {$t('components.chat.download')}</button>
    </div>
  {/if}
  <div class="table-wrap">
    <table>
      <thead>
        <tr>{#each columns as c}<th>{c}</th>{/each}</tr>
      </thead>
      <tbody>
        {#each shown as r}
          <tr>{#each r as cell}<td title={cell}>{cell}</td>{/each}</tr>
        {/each}
      </tbody>
    </table>
    {#if !rows.length}<p class="empty">{$t('components.chat.noRows')}</p>{/if}
  </div>
  <p class="note">
    {$t('components.chat.rowCount', { values: { count: rows.length } })}
    {#if rows.length > MAX_SHOWN}· {$t('components.chat.showingFirst', { values: { count: MAX_SHOWN } })}{/if}
    {#if !framed}
      · <button class="link" on:click={download}><Download size={11} /> {$t('components.chat.download')}</button>
    {/if}
  </p>
</div>

<style>
  .csv.framed {
    margin: 0 0 0.55rem; border: 1px solid var(--line-soft); border-radius: 10px;
    background: var(--bg-soft); overflow: hidden;
  }
  .csv.framed .table-wrap { margin: 0; border: none; border-radius: 0; }
  .csv.framed .note { padding: 0 0.55rem 0.45rem; }
  .head {
    display: flex; align-items: center; justify-content: space-between;
    padding: 0.3rem 0.55rem; border-bottom: 1px solid var(--line-soft);
  }
  .label {
    display: inline-flex; align-items: center; gap: 0.35rem;
    font-size: 0.7rem; font-weight: 700; letter-spacing: 0.4px; text-transform: uppercase;
    color: var(--ink-500);
  }
  .table-wrap {
    margin-top: 0.45rem; max-height: 280px; overflow: auto;
    border: 1px solid var(--line-soft); border-radius: 8px; background: var(--bg-strong);
  }
  table { border-collapse: collapse; width: 100%; font-size: 0.76rem; }
  th, td {
    text-align: left; padding: 4px 8px; border-bottom: 1px solid var(--line-soft);
    max-width: 300px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  th { background: var(--bg-soft); position: sticky; top: 0; font-weight: 600; color: var(--ink-600); }
  .empty { margin: 0; padding: 0.8rem; font-size: 0.78rem; color: var(--ink-400); text-align: center; }
  .note { display: flex; align-items: center; gap: 0.3rem; margin: 0.3rem 0 0; font-size: 0.72rem; color: var(--ink-400); }
  .link {
    display: inline-flex; align-items: center; gap: 0.2rem; background: none; border: none;
    cursor: pointer; color: #4f46e5; font-size: 0.72rem; padding: 0;
  }
  .link:hover { text-decoration: underline; }
  :global(:is([data-theme="dark"], .dark)) .link { color: #a5b4fc; }
</style>
