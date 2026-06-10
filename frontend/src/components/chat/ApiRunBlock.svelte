<script>
  // A runnable API call in a chat answer (`GET /api/...`). For API-service run
  // URLs it behaves like the API services page: loads the service definition
  // (name + parameters), offers editable parameters, runs through the same
  // saved-query client, and renders the negotiated result (SPARQL JSON table,
  // CSV, RDF, JSON, …). Any other same-origin GET /api path runs as a plain
  // authenticated read. Only GETs ever run — chat content cannot trigger writes.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { navigate } from '../../lib/router/index.js';
  import { runSavedQuery, getSavedQuery } from '../../lib/api.js';
  import { describeApiService, parseCsv, normalizeSparqlResult } from '../../lib/chatRich.js';
  import { prettyJson, prettyXml, highlightJson, highlightXml, highlightRdf } from '../../lib/resultHighlight.js';
  import { copyToClipboard } from '../../lib/clipboard.js';
  import SparqlResultView from './SparqlResultView.svelte';
  import CsvPreview from './CsvPreview.svelte';
  import { Globe, Play, Loader2, Copy, Check, Download, ExternalLink, ChevronDown, ChevronRight } from 'lucide-svelte';

  export let method = 'GET';
  export let path = '';
  /** Run immediately (used when the user clicked an inline `GET /api/...`). */
  export let autorun = false;

  $: svc = describeApiService(path);

  let service = null; // saved-query definition, when this is a service run URL
  let paramValues = {};
  let showParams = false;
  let running = false;
  let outcome = null; // { kind, ... } per classify()
  let raw = '';
  let contentType = '';
  let versionServed = null;
  let error = null;
  let elapsed = null;
  let copied = false;

  onMount(() => {
    if (svc) {
      paramValues = { ...svc.params };
      getSavedQuery(svc.scope, svc.ownerId, svc.slug)
        .then((q) => {
          service = q;
          for (const p of q?.parameters || []) {
            if (paramValues[p.name] === undefined && p.default != null) paramValues[p.name] = String(p.default);
          }
          showParams = (q?.parameters || []).length > 0;
        })
        .catch(() => {}); // not fatal — the raw URL can still be run
    }
    if (autorun) run();
  });

  function classify(text, ct) {
    const c = (ct || '').toLowerCase();
    if (c.includes('sparql-results+json')) {
      try { return { kind: 'sparql', result: normalizeSparqlResult(JSON.parse(text)) }; }
      catch { return { kind: 'text', text }; }
    }
    if (c.includes('json')) return { kind: 'json', text: prettyJson(text) };
    if (c.includes('csv')) { const tab = parseCsv(text); return { kind: 'csv', ...tab }; }
    if (c.includes('tab-separated')) {
      const lines = text.split('\n').filter((l) => l.trim());
      const cells = lines.map((l) => l.split('\t'));
      return { kind: 'csv', columns: cells[0] || [], rows: cells.slice(1) };
    }
    if (c.includes('turtle') || c.includes('n-triples') || c.includes('n-quads') || c.includes('trig')) {
      return { kind: 'rdf', text };
    }
    if (c.includes('xml')) return { kind: 'xml', text: prettyXml(text) };
    return { kind: 'text', text };
  }

  /** The effective URL (path + current parameter values) for display/copy. */
  function buildPath(s, values) {
    if (!s) return path;
    const qs = new URLSearchParams();
    for (const [k, v] of Object.entries(values)) if (v !== '' && v != null) qs.set(k, v);
    const base = `/api/${s.scope}/${encodeURIComponent(s.ownerId)}/api-services/${encodeURIComponent(s.slug)}/run`;
    return qs.toString() ? `${base}?${qs}` : base;
  }
  $: effectivePath = buildPath(svc, paramValues);

  async function run() {
    if (running) return;
    running = true;
    error = null;
    outcome = null;
    const t0 = performance.now();
    try {
      if (svc) {
        const r = await runSavedQuery(svc.scope, svc.ownerId, svc.slug, paramValues);
        raw = r.raw;
        contentType = r.contentType;
        versionServed = r.versionServed;
      } else {
        const res = await fetch(path, {
          headers: { Accept: 'application/sparql-results+json, application/json;q=0.9, */*;q=0.8' },
          credentials: 'include',
        });
        raw = await res.text();
        contentType = res.headers.get('content-type') || '';
        if (!res.ok) throw new Error(raw || `${res.status} ${res.statusText}`);
      }
      outcome = classify(raw, contentType);
    } catch (e) {
      error = e?.message || String(e);
    } finally {
      elapsed = Math.round(performance.now() - t0);
      running = false;
    }
  }

  async function copyUrl() {
    await copyToClipboard(`${location.origin}${effectivePath}`);
    copied = true;
    setTimeout(() => { copied = false; }, 1500);
  }

  function download() {
    const c = contentType.toLowerCase();
    const ext = c.includes('sparql-results+json') || c.includes('json') ? 'json'
      : c.includes('csv') ? 'csv'
      : c.includes('tab-separated') ? 'tsv'
      : c.includes('turtle') ? 'ttl'
      : c.includes('n-triples') ? 'nt'
      : c.includes('xml') ? 'xml' : 'txt';
    const a = document.createElement('a');
    a.href = URL.createObjectURL(new Blob([raw], { type: contentType || 'text/plain' }));
    a.download = `${svc?.slug || 'result'}.${ext}`;
    a.click();
    URL.revokeObjectURL(a.href);
  }

  const SCOPE_ROUTES = { datasets: 'datasets', organisations: 'organisations', groups: 'groups' };
  function openServices() {
    if (svc) navigate(`/${SCOPE_ROUTES[svc.scope]}/${encodeURIComponent(svc.ownerId)}/api-services`);
  }

  $: textShown = outcome && ['json', 'rdf', 'xml', 'text'].includes(outcome.kind)
    ? outcome.text.slice(0, 20000)
    : '';
  $: textTruncated = outcome && textShown.length < (outcome.text?.length || 0);
</script>

<div class="block">
  <div class="head">
    <span class="label" title={`${method} ${path}`}>
      <Globe size={12} />
      {#if service?.name}{service.name}{:else if svc}{svc.slug}{:else}{$t('components.chat.apiTitle')}{/if}
    </span>
    <span class="actions">
      {#if elapsed != null && !running}<span class="elapsed">{elapsed} ms</span>{/if}
      {#if svc}
        <button class="act" on:click={openServices} title={$t('components.chat.openServices')} aria-label={$t('components.chat.openServices')}>
          <ExternalLink size={12} />
        </button>
      {/if}
      <button class="act" on:click={copyUrl} title={$t('components.chat.copyUrl')} aria-label={$t('components.chat.copyUrl')}>
        {#if copied}<Check size={12} />{:else}<Copy size={12} />{/if}
      </button>
      <button class="act run" on:click={run} disabled={running}>
        {#if running}<Loader2 size={12} class="spin" /> {$t('components.chat.running')}{:else}<Play size={12} /> {$t('components.chat.run')}{/if}
      </button>
    </span>
  </div>

  <div class="url"><span class="method">{method}</span> <span class="path">{effectivePath}</span></div>

  {#if service?.parameters?.length}
    <button class="params-toggle" on:click={() => { showParams = !showParams; }}>
      {#if showParams}<ChevronDown size={12} />{:else}<ChevronRight size={12} />{/if}
      {$t('components.chat.parameters')} ({service.parameters.length})
    </button>
    {#if showParams}
      <div class="params">
        {#each service.parameters as p (p.name)}
          <label class="param">
            <span class="param-name">{p.name}</span>
            <input type="text" bind:value={paramValues[p.name]} placeholder={p.default != null ? String(p.default) : ''} />
          </label>
        {/each}
      </div>
    {/if}
  {/if}

  {#if error}
    <div class="body"><SparqlResultView result={null} {error} /></div>
  {:else if outcome}
    <div class="body">
      {#if outcome.kind === 'sparql'}
        <SparqlResultView result={outcome.result} />
      {:else if outcome.kind === 'csv'}
        <CsvPreview columns={outcome.columns} rows={outcome.rows} raw={raw} filename={svc?.slug || 'result'} framed={false} />
      {:else if outcome.kind === 'json'}
        <!-- The resultHighlight.js highlighters HTML-escape every source character
             and only add <span class="tok-*"> wrappers, so {@html} is safe here. -->
        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
        <pre class="text-result"><code>{@html highlightJson(textShown)}</code></pre>
      {:else if outcome.kind === 'rdf'}
        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
        <pre class="text-result"><code>{@html highlightRdf(textShown)}</code></pre>
      {:else if outcome.kind === 'xml'}
        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
        <pre class="text-result"><code>{@html highlightXml(textShown)}</code></pre>
      {:else}
        <pre class="text-result"><code>{textShown}</code></pre>
      {/if}
      <p class="meta">
        {#if textTruncated}{$t('components.chat.showingFirst', { values: { count: 20000 } })} · {/if}
        {#if versionServed}{$t('components.chat.servedVersion', { values: { version: versionServed } })} · {/if}
        <button class="link" on:click={download}><Download size={11} /> {$t('components.chat.download')}</button>
      </p>
    </div>
  {/if}
</div>

<style>
  .block {
    margin: 0 0 0.55rem; border: 1px solid var(--line-soft); border-radius: 10px;
    background: var(--bg-soft); overflow: hidden;
  }
  .head {
    display: flex; align-items: center; justify-content: space-between; gap: 0.5rem;
    padding: 0.3rem 0.55rem; border-bottom: 1px solid var(--line-soft);
  }
  .label {
    display: inline-flex; align-items: center; gap: 0.35rem; min-width: 0;
    font-size: 0.74rem; font-weight: 700; color: var(--ink-700);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .actions { display: inline-flex; align-items: center; gap: 0.3rem; flex-shrink: 0; }
  .elapsed { font-size: 0.68rem; color: var(--ink-400); }
  .act {
    display: inline-flex; align-items: center; gap: 0.25rem; cursor: pointer;
    font-size: 0.7rem; font-weight: 600; padding: 2px 7px; border-radius: 6px;
    background: var(--bg-strong); border: 1px solid var(--line-soft); color: var(--ink-600);
  }
  .act:hover:not(:disabled) { background: var(--bg-elevated); border-color: var(--line-strong); }
  .act:disabled { opacity: 0.6; cursor: default; }
  .act.run { background: #ecfdf5; border-color: #a7f3d0; color: #047857; }
  .act.run:hover:not(:disabled) { background: #d1fae5; }
  .url {
    padding: 0.4rem 0.55rem; font-family: 'SF Mono', ui-monospace, monospace; font-size: 0.74rem;
    color: var(--ink-700); word-break: break-all;
  }
  .method {
    font-weight: 700; color: #047857; background: #ecfdf5; border: 1px solid #a7f3d0;
    border-radius: 5px; padding: 0 5px; font-size: 0.68rem; margin-right: 2px;
  }
  .params-toggle {
    display: inline-flex; align-items: center; gap: 0.25rem; margin: 0 0.55rem 0.4rem;
    background: none; border: none; cursor: pointer; color: #4f46e5; font-size: 0.72rem; font-weight: 600; padding: 0;
  }
  .params { display: flex; flex-wrap: wrap; gap: 0.45rem; padding: 0 0.55rem 0.5rem; }
  .param { display: flex; align-items: center; gap: 0.3rem; font-size: 0.72rem; }
  .param-name { font-family: 'SF Mono', ui-monospace, monospace; color: var(--ink-500); }
  .param input {
    width: 9rem; font-size: 0.74rem; padding: 2px 6px; border-radius: 6px;
    border: 1px solid var(--line-strong); background: var(--bg-strong); color: var(--ink-800);
  }
  .body { padding: 0 0.55rem 0.55rem; }
  .text-result {
    margin: 0.45rem 0 0; padding: 0.55rem 0.7rem; background: #1e1e2e; color: #cdd6f4;
    border-radius: 8px; font-size: 0.74rem; line-height: 1.5; overflow: auto; max-height: 280px;
    font-family: 'SF Mono', ui-monospace, monospace;
  }
  .text-result code { background: none; padding: 0; }
  .meta { display: flex; align-items: center; gap: 0.3rem; margin: 0.3rem 0 0; font-size: 0.72rem; color: var(--ink-400); }
  .link {
    display: inline-flex; align-items: center; gap: 0.2rem; background: none; border: none;
    cursor: pointer; color: #4f46e5; font-size: 0.72rem; padding: 0;
  }
  .link:hover { text-decoration: underline; }

  :global(:is([data-theme="dark"], .dark)) .act.run { background: rgba(16,185,129,0.15); border-color: rgba(16,185,129,0.3); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .act.run:hover:not(:disabled) { background: rgba(16,185,129,0.25); }
  :global(:is([data-theme="dark"], .dark)) .method { background: rgba(16,185,129,0.15); border-color: rgba(16,185,129,0.3); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .params-toggle, :global(:is([data-theme="dark"], .dark)) .link { color: #a5b4fc; }
</style>
