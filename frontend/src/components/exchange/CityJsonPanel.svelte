<script lang="ts">
  // CityJSON (3D BAG and the like) into a dataset graph, over
  // POST /api/datasets/:id/ingest/cityjson: pick a file, preview the
  // conversion (counts plus the first N-Triples, nothing written), then
  // import with upload progress and see what it produced.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Building2, Loader2, Eye, Upload, CheckCircle2 } from 'lucide-svelte';
  import { ingestCityJson } from '../../lib/api';
  import { fmtBytes, pctOf } from '../../lib/exchange';
  import { Link } from '../../lib/router/index.js';
  import { toastSuccess } from '../../lib/toast';
  import './exchange.css';

  export let datasetId: string;

  const dispatch = createEventDispatcher<{ changed: void }>();

  type Stats = { objects: number; geometries: number; triples: number; crs: string | null };
  type Result = { graph: string | null; asset_id: string | null; asset_url: string | null; stats: Stats; sample?: string; preview: boolean };

  let file: File | null = null;
  let targetGraph = '';
  let publicAsset = false;
  let busy: '' | 'preview' | 'import' = '';
  let progress = 0;
  let uploaded = false;
  let result: Result | null = null;
  let error = '';

  function onFile(e: Event) {
    file = (e.target as HTMLInputElement).files?.[0] ?? null;
    result = null;
    error = '';
  }

  async function run(preview: boolean) {
    if (!file) return;
    busy = preview ? 'preview' : 'import';
    progress = 0;
    uploaded = false;
    error = '';
    result = null;
    try {
      result = await ingestCityJson(
        datasetId,
        file,
        { targetGraph: targetGraph.trim() || undefined, publicAsset, preview },
        (f) => { progress = f; if (f >= 1) uploaded = true; },
      );
      if (!preview) {
        toastSuccess($t('components.exchange.cityjson.imported', { values: { triples: result?.stats?.triples ?? 0 } }));
        dispatch('changed');
      }
    } catch (e) {
      error = (e as Error)?.message || String(e);
    } finally {
      busy = '';
    }
  }
</script>

<section class="xc-sub" data-testid="cityjson-panel">
  <div class="xc-head">
    <Building2 size={15} />
    <h4>{$t('components.exchange.cityjson.heading')}</h4>
  </div>
  <p class="xc-intro">{$t('components.exchange.cityjson.intro')}</p>

  <div class="xc-form">
    <label class="xc-field">
      <span>{$t('components.exchange.cityjson.file')}</span>
      <input class="xc-file" type="file" accept=".json,.city.json,.cityjson,application/json" on:change={onFile} disabled={!!busy} data-testid="cityjson-file" />
      {#if file}<small>{file.name} · {fmtBytes(file.size)}</small>{/if}
    </label>
    <label class="xc-field">
      <span>{$t('components.exchange.cityjson.targetGraph')}</span>
      <input type="text" bind:value={targetGraph} placeholder={$t('components.exchange.cityjson.targetGraphPlaceholder')} disabled={!!busy} />
      <small>{$t('components.exchange.cityjson.targetGraphHint')}</small>
    </label>
    <label class="xc-check">
      <input type="checkbox" bind:checked={publicAsset} disabled={!!busy} />
      {$t('components.exchange.cityjson.publicAsset')}
    </label>
    <div class="xc-row">
      <button class="btn btn-sm btn-ghost" on:click={() => run(true)} disabled={!file || !!busy}>
        {#if busy === 'preview'}<Loader2 size={13} class="animate-spin" />{:else}<Eye size={13} />{/if}
        {$t('components.exchange.cityjson.preview')}
      </button>
      <button class="btn btn-sm" on:click={() => run(false)} disabled={!file || !!busy}>
        {#if busy === 'import'}<Loader2 size={13} class="animate-spin" />{:else}<Upload size={13} />{/if}
        {$t('components.exchange.cityjson.import')}
      </button>
    </div>
    {#if busy}
      <div class="xc-progress" class:xc-indeterminate={uploaded} role="progressbar" aria-valuenow={pctOf(progress)} aria-valuemin={0} aria-valuemax={100}>
        <div style="width: {pctOf(progress)}%"></div>
      </div>
      <p class="xc-muted">
        {uploaded ? $t('components.exchange.cityjson.converting') : $t('components.exchange.uploading', { values: { pct: pctOf(progress) } })}
      </p>
    {/if}
  </div>

  {#if error}<p class="xc-error" role="alert">{error}</p>{/if}

  {#if result}
    <div class="xc-callout" class:xc-ok={!result.preview} class:xc-info={result.preview} role="status" data-testid="cityjson-result">
      <CheckCircle2 size={15} />
      <div>
        <strong>{result.preview ? $t('components.exchange.cityjson.previewDone') : $t('components.exchange.cityjson.importDone')}</strong>
        <dl class="xc-facts">
          <div><dt>{$t('components.exchange.cityjson.objects')}</dt><dd>{result.stats.objects.toLocaleString()}</dd></div>
          <div><dt>{$t('components.exchange.cityjson.geometries')}</dt><dd>{result.stats.geometries.toLocaleString()}</dd></div>
          <div><dt>{$t('components.exchange.cityjson.triples')}</dt><dd>{result.stats.triples.toLocaleString()}</dd></div>
          <div><dt>{$t('components.exchange.cityjson.crs')}</dt><dd>{result.stats.crs || '—'}</dd></div>
        </dl>
        {#if result.graph}
          <p class="xc-muted">
            {$t('components.exchange.cityjson.graph')} <code class="xc-code">{result.graph}</code>
            · <Link to={`/browse?graph=${encodeURIComponent(result.graph)}`}>{$t('components.exchange.cityjson.browse')}</Link>
            · <Link to={`/datasets/${datasetId}/viewer`}>{$t('components.exchange.cityjson.open3d')}</Link>
          </p>
        {/if}
        {#if result.preview && result.sample}
          <pre class="xc-pre">{result.sample}</pre>
        {/if}
      </div>
    </div>
  {/if}
</section>
