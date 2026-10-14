<script lang="ts">
  // Check a building against an IDS: run an IDS-imported shape graph over the
  // IDS projection an IFC import writes to `{building}/ids`. The run is a
  // SHACL Studio pipeline scoped to exactly that graph and shape graph — made
  // on first use, reused after — so its runs are kept and the last one shows
  // when the panel opens.
  import { t } from 'svelte-i18n';
  import { ClipboardCheck, Loader2, Play } from 'lucide-svelte';
  import {
    listShapeGraphs, listPipelines, createPipeline, runPipeline, listPipelineRuns, getPipelineRun, listShapesCatalog,
  } from '../../lib/api';
  import { idsProjectionGraphs, buildingOf, isIdsShapeGraph, findIdsPipeline } from '../../lib/exchange';
  import { Link } from '../../lib/router/index.js';
  import { toastError } from '../../lib/toast';
  import IdsResults from './IdsResults.svelte';
  import './exchange.css';

  export let datasetId: string;
  export let graphs: (string | { graph_iri: string })[] = [];
  export let datasetName = '';

  type ShapeGraph = { id: string; name: string; graph_iri: string; tags?: string[] };
  type Pipeline = { id: string; dataset_ids?: string[]; graph_iris?: string[]; shape_graph_ids?: string[] };

  let shapeGraphs: ShapeGraph[] = [];
  let loading = true;
  let loadError = '';
  let sgId = '';
  let graph = '';
  let running = false;
  let run: any = null;
  let specs: { shape: string; label?: string | null }[] = [];
  let pipelineId = '';

  $: projections = idsProjectionGraphs(graphs);
  $: if (!graph && projections.length) graph = projections[0];

  async function loadShapeGraphs() {
    loading = true;
    try {
      const all: ShapeGraph[] = await listShapeGraphs();
      shapeGraphs = (all ?? []).filter(isIdsShapeGraph);
      if (!sgId && shapeGraphs.length) sgId = shapeGraphs[0].id;
    } catch (e) {
      loadError = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
  }
  loadShapeGraphs();

  async function loadSpecs(sg: ShapeGraph | undefined) {
    specs = [];
    if (!sg) return;
    try {
      const res = await listShapesCatalog(sg.graph_iri);
      specs = (res?.shapes ?? [])
        .filter((s: { kind?: string }) => !s.kind || s.kind === 'node')
        .map((s: { shape: string; label?: string | null }) => ({ shape: s.shape, label: s.label }));
    } catch { /* names fall back to the shape IRIs */ }
  }

  /** The last kept run of the pipeline for this pair, if there is one. */
  async function loadLast(sg: string, g: string) {
    run = null;
    pipelineId = '';
    if (!sg || !g) return;
    try {
      const p = findIdsPipeline<Pipeline>(await listPipelines(), datasetId, g, sg);
      if (!p) return;
      pipelineId = p.id;
      const runs = await listPipelineRuns(p.id, 1);
      const last = Array.isArray(runs) ? runs[0] : runs?.runs?.[0];
      if (last?.id) run = await getPipelineRun(p.id, last.id);
    } catch { /* no history to show */ }
  }

  $: selected = shapeGraphs.find((s) => s.id === sgId);
  $: loadSpecs(selected);
  $: loadLast(sgId, graph);

  async function check() {
    if (!selected || !graph) return;
    running = true;
    try {
      let p = findIdsPipeline<Pipeline>(await listPipelines(), datasetId, graph, selected.id);
      if (!p) {
        p = await createPipeline({
          name: $t('components.exchange.ids.pipelineName', {
            values: { spec: selected.name, building: datasetName || datasetId },
          }),
          description: $t('components.exchange.ids.pipelineDescription', { values: { graph } }),
          visibility: 'private',
          dataset_ids: [datasetId],
          graph_iris: [graph],
          shape_graph_ids: [selected.id],
        });
      }
      pipelineId = p!.id;
      run = await runPipeline(p!.id);
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    } finally {
      running = false;
    }
  }
</script>

<section class="xc-sub" data-testid="ids-check-panel">
  <div class="xc-head">
    <ClipboardCheck size={15} />
    <h4>{$t('components.exchange.ids.checkHeading')}</h4>
  </div>
  <p class="xc-intro">{$t('components.exchange.ids.checkIntro')}</p>

  {#if loading}
    <p class="xc-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
  {:else if loadError}
    <p class="xc-error">{loadError}</p>
  {:else if projections.length === 0}
    <p class="xc-muted">{$t('components.exchange.ids.noProjection')}</p>
  {:else if shapeGraphs.length === 0}
    <p class="xc-muted">
      {$t('components.exchange.ids.noIdsShapes')}
      <Link to="/shacl/shapes">{$t('components.exchange.ids.openLibrary')}</Link>
    </p>
  {:else}
    <div class="xc-grid">
      <label class="xc-field">
        <span>{$t('components.exchange.ids.specLabel')}</span>
        <select bind:value={sgId} disabled={running} data-testid="ids-shape-graph">
          {#each shapeGraphs as sg (sg.id)}<option value={sg.id}>{sg.name}</option>{/each}
        </select>
      </label>
      <label class="xc-field">
        <span>{$t('components.exchange.ids.buildingLabel')}</span>
        <select bind:value={graph} disabled={running} data-testid="ids-graph">
          {#each projections as g (g)}<option value={g}>{buildingOf(g)}</option>{/each}
        </select>
      </label>
    </div>
    <div class="xc-row" style="margin-top: 0.5rem">
      <button class="btn btn-sm" on:click={check} disabled={running || !selected || !graph}>
        {#if running}<Loader2 size={13} class="animate-spin" />{:else}<Play size={13} />{/if}
        {$t('components.exchange.ids.run')}
      </button>
      {#if pipelineId}
        <Link to={`/shacl/pipelines/${pipelineId}`} class="btn btn-sm btn-ghost">{$t('components.exchange.ids.openPipeline')}</Link>
      {/if}
    </div>
    <IdsResults {specs} {run} />
  {/if}
</section>
