<script lang="ts">
  // Export a shape graph to buildingSMART IDS 1.0 and say honestly what came
  // out: how many specifications, whether they were written back unchanged
  // from the IDS an import recorded (a lossless re-export), and everything
  // the format could not carry. The document is saved only on request.
  import { createEventDispatcher, onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Loader2, Download, CheckCircle2, AlertTriangle, Info } from 'lucide-svelte';
  import { getShapeGraphTurtle, exportSpec } from '../../lib/api';
  import { downloadBlob } from '../../lib/exchange';
  import ExchangeDialog from './ExchangeDialog.svelte';
  import './exchange.css';

  export let shapeGraph: { id: string; name: string };

  const dispatch = createEventDispatcher<{ close: void }>();

  type Exported = { document: string; specification_count: number; verbatim_specifications: number; losses: string[] };
  let loading = true;
  let result: Exported | null = null;
  let error = '';

  onMount(async () => {
    try {
      const turtle = await getShapeGraphTurtle(shapeGraph.id);
      result = await exportSpec('ids', turtle, shapeGraph.name);
    } catch (e) {
      error = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
  });

  $: lossless = !!result && result.losses.length === 0 && result.verbatim_specifications === result.specification_count;
  $: partlyVerbatim = !!result && result.verbatim_specifications > 0 && !lossless;

  function save() {
    if (!result) return;
    const base = (shapeGraph.name || 'shapes').replace(/[^\w.-]+/g, '_').replace(/^_+|_+$/g, '') || 'shapes';
    downloadBlob(new Blob([result.document], { type: 'application/xml' }), `${base}.ids`);
  }
</script>

<ExchangeDialog title={$t('components.exchange.idsExport.title', { values: { name: shapeGraph.name } })} on:close={() => dispatch('close')}>
  {#if loading}
    <p class="xc-muted"><Loader2 size={12} class="animate-spin" /> {$t('components.exchange.idsExport.exporting')}</p>
  {:else if error}
    <p class="xc-error" role="alert">{error}</p>
  {:else if result}
    <p class="xc-intro">
      {$t('components.exchange.idsExport.count', { values: { n: result.specification_count } })}
    </p>
    {#if lossless}
      <div class="xc-callout xc-ok" role="status" data-testid="ids-lossless">
        <CheckCircle2 size={15} />
        <div>{$t('components.exchange.idsExport.lossless')}</div>
      </div>
    {:else if partlyVerbatim}
      <div class="xc-callout xc-info" role="status">
        <Info size={15} />
        <div>{$t('components.exchange.idsExport.partlyVerbatim', { values: { verbatim: result.verbatim_specifications, total: result.specification_count } })}</div>
      </div>
    {:else}
      <div class="xc-callout xc-info" role="status">
        <Info size={15} />
        <div>{$t('components.exchange.idsExport.translated')}</div>
      </div>
    {/if}
    {#if result.losses.length}
      <div class="xc-callout xc-warn" role="note" data-testid="ids-losses">
        <AlertTriangle size={15} />
        <div>
          <strong>{$t('components.exchange.idsExport.losses', { values: { n: result.losses.length } })}</strong>
          <ul class="xc-list">{#each result.losses as l}<li>{l}</li>{/each}</ul>
        </div>
      </div>
    {/if}
  {/if}
  <svelte:fragment slot="footer">
    <button class="btn btn-ghost btn-sm" on:click={() => dispatch('close')}>{$t('system.close')}</button>
    <button class="btn btn-sm" on:click={save} disabled={!result}>
      <Download size={13} /> {$t('components.exchange.idsExport.download')}
    </button>
  </svelte:fragment>
</ExchangeDialog>
