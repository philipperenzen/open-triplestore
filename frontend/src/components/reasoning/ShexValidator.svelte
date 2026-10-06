<script lang="ts">
  // ShEx 2.1 validation (docs/shex.md): a schema in ShExC or ShExJ, which
  // nodes to check against which shapes — every shape on the nodes that use
  // its predicates, a ShapeMap, or node/shape pairs — and the verdicts.
  // Over a dataset's readable graphs, or (no dataset) everything the caller
  // may read.
  import { t } from 'svelte-i18n';
  import { CheckCircle2, Loader2, Plus, ShieldCheck, Trash2, XCircle } from 'lucide-svelte';
  import Select from '../Select.svelte';
  import { validateShex, type ShexReport } from '../../lib/api';
  import { shexVerdict } from '../../lib/reasoning';
  import './reasoning.css';

  export let datasetId: string | null = null;

  const EXAMPLE = `PREFIX ex: <https://example.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>

ex:PersonShape {
  ex:name xsd:string ;
  ex:age xsd:integer ?
}`;

  let schema = '';
  let schemaFormat = '';
  let base = '';
  let mapMode: 'discover' | 'text' | 'pairs' = 'discover';
  let mapText = '';
  let pairs: { node: string; shape: string }[] = [{ node: '', shape: '' }];
  let running = false;
  let error = '';
  let report: ShexReport | null = null;
  let filter: 'all' | 'bad' | 'ok' = 'all';
  let search = '';

  $: formatOptions = [
    { value: '', label: $t('components.shex.formatAuto') },
    { value: 'shexc', label: 'ShExC' },
    { value: 'shexj', label: 'ShExJ' },
  ];

  $: rows = (report?.results ?? []).map((r) => ({ ...r, ...shexVerdict(r.status) }));
  $: failing = rows.filter((r) => !r.ok).length;
  $: shown = rows.filter((r) =>
    (filter === 'all' || (filter === 'bad' ? !r.ok : r.ok)) &&
    (!search.trim() || `${r.focus_node} ${r.shape} ${r.reason}`.toLowerCase().includes(search.trim().toLowerCase())),
  );

  async function run() {
    running = true;
    error = '';
    report = null;
    try {
      const body: Parameters<typeof validateShex>[1] = { schema };
      if (schemaFormat) body.schema_format = schemaFormat as 'shexc' | 'shexj';
      if (base.trim()) body.base = base.trim();
      if (mapMode === 'text' && mapText.trim()) body.shape_map = mapText;
      if (mapMode === 'pairs') {
        const p = pairs.filter((x) => x.node.trim() && x.shape.trim()).map((x) => ({ node: x.node.trim(), shape: x.shape.trim() }));
        if (p.length) body.shape_map = p;
      }
      report = await validateShex(datasetId, body);
      filter = 'all';
    } catch (e) {
      error = (e as Error)?.message || String(e);
    } finally {
      running = false;
    }
  }

  function addPair() { pairs = [...pairs, { node: '', shape: '' }]; }
  function removePair(i: number) { pairs = pairs.filter((_, j) => j !== i); if (!pairs.length) addPair(); }
</script>

<div class="card rz-card" id="shex">
  <div class="rz-head">
    <div class="rz-head-left"><ShieldCheck size={15} /><h3>{$t('components.shex.heading')}</h3></div>
    <button class="btn btn-sm btn-ghost" on:click={() => (schema = EXAMPLE)} disabled={running}>{$t('components.shex.example')}</button>
  </div>
  <p class="rz-intro">{datasetId ? $t('components.shex.introDataset') : $t('components.shex.intro')}</p>

  <div class="rz-field">
    <label for="shex-schema">{$t('components.shex.schema')}</label>
    <textarea id="shex-schema" class="rz-code" style="min-height: 12rem" bind:value={schema} placeholder={EXAMPLE} disabled={running}></textarea>
  </div>
  <div class="rz-grid">
    <div class="rz-field">
      <span class="rz-label">{$t('components.shex.format')}</span>
      <Select bind:value={schemaFormat} options={formatOptions} ariaLabel={$t('components.shex.format')} disabled={running} />
    </div>
    <div class="rz-field">
      <label for="shex-base">{$t('components.shex.base')}</label>
      <input id="shex-base" bind:value={base} placeholder="https://example.org/" disabled={running} />
    </div>
  </div>

  <div class="rz-field">
    <span class="rz-label">{$t('components.shex.shapeMap')}</span>
    <div class="rz-tabs" role="group" aria-label={$t('components.shex.shapeMap')}>
      {#each ['discover', 'text', 'pairs'] as m}
        <button type="button" class="rz-tab" aria-pressed={mapMode === m} on:click={() => (mapMode = m as typeof mapMode)}>{$t(`components.shex.mapMode.${m}`)}</button>
      {/each}
    </div>
    {#if mapMode === 'discover'}
      <span class="rz-muted">{$t('components.shex.discoverHint')}</span>
    {:else if mapMode === 'text'}
      <textarea class="rz-code" style="min-height: 4rem" bind:value={mapText} aria-label={$t('components.shex.mapMode.text')} placeholder={'<https://example.org/alice>@<https://example.org/PersonShape>, {FOCUS a <https://example.org/Person>}@START'} disabled={running}></textarea>
      <span class="rz-muted">{$t('components.shex.textHint')}</span>
    {:else}
      <div class="rz-table-wrap">
        <table class="rz-table">
          <thead><tr><th>{$t('components.shex.node')}</th><th>{$t('components.shex.shape')}</th><th></th></tr></thead>
          <tbody>
            {#each pairs as p, i}
              <tr>
                <td><input bind:value={p.node} placeholder="https://example.org/alice" aria-label={$t('components.shex.nodeN', { values: { n: i + 1 } })} disabled={running} /></td>
                <td><input bind:value={p.shape} placeholder="https://example.org/PersonShape" aria-label={$t('components.shex.shapeN', { values: { n: i + 1 } })} disabled={running} /></td>
                <td><button class="tbl-btn danger" on:click={() => removePair(i)} aria-label={$t('components.shex.removePair')}><Trash2 size={13} /></button></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      <div><button class="btn btn-sm btn-ghost" on:click={addPair} disabled={running}><Plus size={13} /> {$t('components.shex.addPair')}</button></div>
    {/if}
  </div>

  <div class="rz-row">
    <button class="btn btn-sm" on:click={run} disabled={running || !schema.trim()}>
      {#if running}<Loader2 size={13} class="animate-spin" />{/if}
      {$t('components.shex.validate')}
    </button>
  </div>
  {#if error}<p class="rz-msg rz-msg-bad" role="alert">{error}</p>{/if}

  {#if report}
    <div class="rz-card" data-testid="shex-report">
      {#if !rows.length}
        <p class="rz-msg rz-msg-warn">{$t('components.shex.noResults')}</p>
      {:else if report.conforms}
        <p class="rz-msg rz-msg-ok"><CheckCircle2 size={14} /> {$t('components.shex.conforms', { values: { count: rows.length } })}</p>
      {:else}
        <p class="rz-msg rz-msg-bad"><XCircle size={14} /> {$t('components.shex.fails', { values: { failing, count: rows.length } })}</p>
      {/if}
      {#if rows.length}
        <div class="rz-row">
          <div class="rz-tabs" role="group" aria-label={$t('components.shex.filter')}>
            {#each ['all', 'bad', 'ok'] as f}
              <button type="button" class="rz-tab" aria-pressed={filter === f} on:click={() => (filter = f as typeof filter)}>{$t(`components.shex.filterMode.${f}`)}</button>
            {/each}
          </div>
          <input style="max-width: 18rem" bind:value={search} placeholder={$t('components.shex.search')} aria-label={$t('components.shex.search')} />
        </div>
        <div class="rz-table-wrap">
          <table class="rz-table">
            <thead><tr><th>{$t('components.shex.node')}</th><th>{$t('components.shex.shape')}</th><th>{$t('components.shex.verdict')}</th></tr></thead>
            <tbody>
              {#each shown as r}
                <tr data-testid="shex-row">
                  <td><code>{r.focus_node}</code></td>
                  <td><code>{r.shape}</code></td>
                  <td>
                    {#if r.ok}<span class="rz-badge rz-ok">{$t('components.shex.conformant')}</span>
                    {:else}<span class="rz-badge rz-bad">{$t('components.shex.nonConformant')}</span> <span class="rz-del">{r.reason}</span>{/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>
  {/if}
</div>
