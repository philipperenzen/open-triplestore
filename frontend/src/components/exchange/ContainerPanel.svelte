<script lang="ts">
  // ISO 21597-1 ICDD containers for one dataset: import an archive (with the
  // strict toggle and the validator's report), validate one without storing
  // anything, and export the dataset as a container with the export's own
  // validation — the X-Container-* headers a plain download would hide.
  // Warns up front when the operator has not set OTS_ICDD_ONTOLOGY_DIR, the
  // reason an export is not Part 1-conformant.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Package, Loader2, Upload, ShieldCheck, Download, AlertTriangle, CheckCircle2, CircleAlert } from 'lucide-svelte';
  import { listContainerProfiles, importContainer, validateContainer, exportContainer } from '../../lib/api';
  import {
    parseContainerHeaders, downloadBlob, pctOf, fmtBytes, ONTOLOGY_MISSING, type ContainerReport,
  } from '../../lib/exchange';
  import { toastError, toastSuccess } from '../../lib/toast';
  import ContainerReportView from './ContainerReportView.svelte';
  import './exchange.css';

  export let datasetId: string;
  export let canWrite = false;
  /** Import and validate need a sign-in; anyone who can see the dataset may export it. */
  export let authenticated = false;

  const dispatch = createEventDispatcher<{ changed: void }>();

  type Ontology = { env: string; configured: boolean; complete: boolean; missing: string[] };
  let ontology: Ontology | null = null;
  let profilesError = '';

  async function loadProfiles() {
    try {
      const res = await listContainerProfiles();
      const icdd = (res?.profiles ?? []).find((p: { id: string }) => p.id === 'icdd');
      ontology = icdd?.ontology_resources ?? null;
    } catch (e) {
      profilesError = (e as Error)?.message || String(e);
    }
  }
  $: if (authenticated) loadProfiles();

  let file: File | null = null;
  let fileInput: HTMLInputElement;
  let strict = false;
  let busy: '' | 'import' | 'validate' | 'export' = '';
  let progress = 0;
  let uploaded = false;

  let report: ContainerReport | null = null;
  let reportKind: '' | 'import' | 'refused' | 'validate' = '';
  let imported: Record<string, any> | null = null;
  let actionError = '';

  type ExportResult = ReturnType<typeof parseContainerHeaders> & { filename: string; size: number };
  let exported: ExportResult | null = null;

  function onFile(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0] ?? null;
    file = f;
    report = null;
    imported = null;
    actionError = '';
  }

  function onProgress(f: number) {
    progress = f;
    if (f >= 1) uploaded = true;
  }

  async function doImport() {
    if (!file) return;
    busy = 'import';
    progress = 0;
    uploaded = false;
    actionError = '';
    report = null;
    imported = null;
    try {
      const res = await importContainer(datasetId, file, { strict }, onProgress);
      imported = res;
      report = res?.validation ?? null;
      reportKind = 'import';
      toastSuccess($t('components.exchange.container.imported', {
        values: { documents: res?.documents?.length ?? 0, graphs: res?.graphs?.length ?? 0 },
      }));
      dispatch('changed');
    } catch (e) {
      const err = e as Error & { status?: number; body?: any };
      if (err.status === 422 && err.body?.validation) {
        report = err.body.validation;
        reportKind = 'refused';
      }
      actionError = err.message || String(e);
    } finally {
      busy = '';
    }
  }

  async function doValidate() {
    if (!file) return;
    busy = 'validate';
    progress = 0;
    uploaded = false;
    actionError = '';
    report = null;
    imported = null;
    try {
      report = await validateContainer(file, onProgress);
      reportKind = 'validate';
    } catch (e) {
      actionError = (e as Error)?.message || String(e);
    } finally {
      busy = '';
    }
  }

  async function doExport() {
    busy = 'export';
    exported = null;
    try {
      const res = await exportContainer(datasetId);
      exported = { ...parseContainerHeaders(res.headers), filename: res.filename, size: res.blob.size };
      downloadBlob(res.blob, res.filename);
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    } finally {
      busy = '';
    }
  }

  $: exportOntologyMissing = !!exported?.findings.includes(ONTOLOGY_MISSING);
</script>

<section class="xc-sub" data-testid="container-panel">
  <div class="xc-head">
    <Package size={15} />
    <h4>{$t('components.exchange.container.heading')}</h4>
  </div>
  <p class="xc-intro">{$t('components.exchange.container.intro')}</p>

  {#if ontology && !ontology.complete}
    <div class="xc-callout xc-warn" role="note" data-testid="ontology-warning">
      <AlertTriangle size={15} />
      <div>
        {#if ontology.configured}
          {$t('components.exchange.container.ontologyIncomplete', { values: { env: ontology.env, files: ontology.missing.join(', ') } })}
        {:else}
          {$t('components.exchange.container.ontologyUnset', { values: { env: ontology.env } })}
        {/if}
      </div>
    </div>
  {/if}
  {#if profilesError}<p class="xc-muted">{profilesError}</p>{/if}

  {#if authenticated}
  <div class="xc-form">
    <label class="xc-field">
      <span>{$t('components.exchange.container.file')}</span>
      <input
        class="xc-file"
        type="file"
        accept=".icdd,.zip,application/zip"
        bind:this={fileInput}
        on:change={onFile}
        disabled={!!busy}
        data-testid="container-file"
      />
      {#if file}<small>{file.name} · {fmtBytes(file.size)}</small>{/if}
    </label>
    {#if canWrite}
      <label class="xc-check">
        <input type="checkbox" bind:checked={strict} disabled={!!busy} />
        {$t('components.exchange.container.strict')}
      </label>
      <p class="xc-muted">{$t('components.exchange.container.strictHint')}</p>
    {/if}
    <div class="xc-row">
      {#if canWrite}
        <button class="btn btn-sm" on:click={doImport} disabled={!file || !!busy}>
          {#if busy === 'import'}<Loader2 size={13} class="animate-spin" />{:else}<Upload size={13} />{/if}
          {$t('components.exchange.container.import')}
        </button>
      {/if}
      <button class="btn btn-sm btn-ghost" on:click={doValidate} disabled={!file || !!busy}>
        {#if busy === 'validate'}<Loader2 size={13} class="animate-spin" />{:else}<ShieldCheck size={13} />{/if}
        {$t('components.exchange.container.validateOnly')}
      </button>
    </div>
    {#if busy === 'import' || busy === 'validate'}
      <div class="xc-progress" class:xc-indeterminate={uploaded} role="progressbar" aria-valuenow={pctOf(progress)} aria-valuemin={0} aria-valuemax={100}>
        <div style="width: {pctOf(progress)}%"></div>
      </div>
      <p class="xc-muted">
        {uploaded ? $t('components.exchange.processing') : $t('components.exchange.uploading', { values: { pct: pctOf(progress) } })}
      </p>
    {/if}
  </div>

  {/if}

  {#if actionError}<p class="xc-error" role="alert">{actionError}</p>{/if}

  {#if imported}
    <div class="xc-callout xc-ok" role="status">
      <CheckCircle2 size={15} />
      <div>
        {$t('components.exchange.container.importedSummary', {
          values: {
            title: imported.title || imported.container,
            documents: imported.documents?.length ?? 0,
            graphs: imported.graphs?.length ?? 0,
          },
        })}
        {#if imported.warnings?.length}
          <ul class="xc-list">{#each imported.warnings as w}<li>{w}</li>{/each}</ul>
        {/if}
      </div>
    </div>
  {/if}

  {#if report}
    <h5 class="xc-label">
      {reportKind === 'refused'
        ? $t('components.exchange.container.reportRefused')
        : reportKind === 'validate'
          ? $t('components.exchange.container.reportValidate')
          : $t('components.exchange.container.reportImport')}
    </h5>
    <ContainerReportView {report} />
  {/if}

  <div class="xc-sub">
    <div class="xc-row">
      <button class="btn btn-sm btn-ghost" on:click={doExport} disabled={!!busy} data-testid="container-export">
        {#if busy === 'export'}<Loader2 size={13} class="animate-spin" />{:else}<Download size={13} />{/if}
        {$t('components.exchange.container.export')}
      </button>
      <span class="xc-muted">{$t('components.exchange.container.exportHint')}</span>
    </div>
    {#if exported}
      <div class="xc-callout" class:xc-ok={exported.conforms === true} class:xc-warn={exported.conforms !== true} role="status" data-testid="container-export-result">
        {#if exported.conforms === true}<CheckCircle2 size={15} />{:else}<CircleAlert size={15} />{/if}
        <div>
          <strong>{exported.filename}</strong> · {fmtBytes(exported.size)}<br />
          {exported.conforms === true
            ? $t('components.exchange.container.exportConforms')
            : $t('components.exchange.container.exportNotConforms')}
          {#if exported.violations !== null}
            · {$t('components.exchange.container.counts', { values: { violations: exported.violations, warnings: exported.warnings ?? 0 } })}
          {/if}
          {#if exported.findings.length}
            <div class="xc-row">
              {#each exported.findings as code}<code class="xc-code">{code}</code>{/each}
            </div>
          {/if}
          {#if exportOntologyMissing}
            <p class="xc-muted">{$t('components.exchange.container.exportOntologyMissing')}</p>
          {/if}
        </div>
      </div>
    {/if}
  </div>
</section>
