<script lang="ts">
  // Apply an RDF Patch to a dataset (POST /api/datasets/:id/patch), or apply
  // it and record it in the dataset's patch log (POST …/log, which needs an
  // `H id` and an `H prev` naming the log's latest entry — added here when
  // missing). Triples (no graph term) go to the graph picked here. A SHACL
  // write gate that refuses the patch answers 422 with its report.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { FileDiff, Loader2, Upload } from 'lucide-svelte';
  import Select from '../Select.svelte';
  import { applyDatasetPatch, appendDatasetPatchLog, type PatchApplied } from '../../lib/api';
  import { withLogHeaders } from '../../lib/reasoning';
  import './reasoning.css';

  export let datasetId: string;
  export let canWrite = false;
  /** The dataset's graphs (for the `?graph=` of triples). */
  export let graphs: string[] = [];
  /** The log's latest entry id, for `H prev`. */
  export let latestId: string | null = null;

  const dispatch = createEventDispatcher<{ applied: PatchApplied }>();
  const EXAMPLE = 'TX .\nA <https://example.org/s> <https://example.org/p> "o" <GRAPH> .\nTC .';

  let patch = '';
  let graph = '';
  let record = true;
  let running = false;
  let result: PatchApplied | null = null;
  let error = '';
  let gateReport: unknown = null;
  let fileInput: HTMLInputElement;

  $: graphOptions = [{ value: '', label: $t('components.patch.noDefaultGraph') }, ...graphs.map((g) => ({ value: g, label: g }))];
  $: example = EXAMPLE.replace('GRAPH', graphs[0] ?? 'https://example.org/graph');

  async function readFile(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0];
    if (f) patch = await f.text();
  }

  function uuid(): string {
    return typeof crypto !== 'undefined' && 'randomUUID' in crypto
      ? `urn:uuid:${crypto.randomUUID()}`
      : `urn:uuid:${Date.now().toString(16)}-${Math.random().toString(16).slice(2)}`;
  }

  async function apply() {
    running = true;
    error = '';
    result = null;
    gateReport = null;
    try {
      result = record
        ? await appendDatasetPatchLog(datasetId, withLogHeaders(patch, latestId, uuid), graph || null)
        : await applyDatasetPatch(datasetId, patch, graph || null);
      dispatch('applied', result);
    } catch (e) {
      const err = e as { status?: number; message?: string; body?: Record<string, unknown> };
      error = err?.message || String(e);
      if (err?.status === 422 && err.body) gateReport = err.body.report ?? err.body;
    } finally {
      running = false;
    }
  }
</script>

<div class="card rz-card" id="apply">
  <div class="rz-head">
    <div class="rz-head-left"><FileDiff size={15} /><h3>{$t('components.patch.applyHeading')}</h3></div>
    {#if canWrite}<button class="btn btn-sm btn-ghost" on:click={() => (patch = example)} disabled={running}>{$t('components.patch.example')}</button>{/if}
  </div>
  <p class="rz-intro">{$t('components.patch.applyIntro')}</p>
  {#if canWrite}
    <div class="rz-field">
      <label for="patch-text">{$t('components.patch.patch')}</label>
      <textarea id="patch-text" class="rz-code" style="min-height: 12rem" bind:value={patch} placeholder={example} disabled={running}></textarea>
    </div>
    <div class="rz-row">
      <input type="file" accept=".rdfp,.rdf-patch,.txt,application/rdf-patch,text/plain" bind:this={fileInput} on:change={readFile} hidden />
      <button class="btn btn-sm btn-ghost" on:click={() => fileInput.click()} disabled={running}><Upload size={13} /> {$t('components.patch.loadFile')}</button>
    </div>
    <div class="rz-grid">
      <div class="rz-field">
        <span class="rz-label">{$t('components.patch.defaultGraph')}</span>
        <Select bind:value={graph} options={graphOptions} ariaLabel={$t('components.patch.defaultGraph')} disabled={running} />
        <span class="rz-muted">{$t('components.patch.defaultGraphHint')}</span>
      </div>
    </div>
    <label class="rz-check"><input type="checkbox" bind:checked={record} disabled={running} /> {$t('components.patch.record')}</label>
    <span class="rz-muted">{record ? $t('components.patch.recordHint') : $t('components.patch.noRecordHint')}</span>
    <div class="rz-row">
      <button class="btn btn-sm" on:click={apply} disabled={running || !patch.trim()}>
        {#if running}<Loader2 size={13} class="animate-spin" />{/if}
        {$t('components.patch.apply')}
      </button>
    </div>
    {#if error}
      <div class="rz-msg rz-msg-bad" role="alert">
        {error}
        {#if gateReport}<pre class="rz-pre">{JSON.stringify(gateReport, null, 2)}</pre>{/if}
      </div>
    {/if}
    {#if result}
      <div class="rz-msg rz-msg-ok" data-testid="patch-result">
        {result.applied ? $t('components.patch.applied', { values: { added: result.added, removed: result.removed } }) : $t('components.patch.nothing')}
        {#if result.aborted} · {$t('components.patch.aborted')}{/if}
        {#if result.version != null} · {$t('components.patch.logVersion', { values: { version: result.version } })}{/if}
      </div>
    {/if}
  {:else}
    <p class="rz-muted">{$t('components.reasoning.writeNeeded')}</p>
  {/if}
</div>
