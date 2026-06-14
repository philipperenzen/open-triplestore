<script>
  // Renders a *file* resource (a samples/asset path or an http(s) URL ending in
  // a known file extension) rather than an RDF resource. Used by the resource
  // detail page when the requested IRI is actually a file — clicking e.g.
  // "/samples/schependomlaan-3dbag.city.json" lands here instead of erroring on
  // the "IRI must be absolute" browseResource path.
  import { onMount } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { Download, ExternalLink, FileText, FileJson, Image as ImageIcon, Boxes, File as FileIcon, FileType2 } from 'lucide-svelte';
  import { fileResourceKind, FORMAT_LABELS } from '../../lib/viewer/detect';
  import { safeExternalUrl, safeImageUrl } from '../../lib/safeUrl.js';

  /** The file URL — a site-relative path or an http(s) URL. */
  export let url = '';
  export let height = '320px';

  // Model3D pulls the heavy three.js chunk; load it only for 3D files.
  const model3d = () => import('./Model3D.svelte');

  // How many bytes of a text/JSON file to show inline before truncating.
  const MAX_TEXT_BYTES = 64 * 1024;

  $: detected = fileResourceKind(url);
  $: fileName = (() => {
    const path = String(url || '').split(/[?#]/)[0];
    const seg = path.split('/').filter(Boolean).pop() || path;
    return decodeURIComponent(seg);
  })();
  $: typeLabel = (() => {
    if (!detected) return '';
    if (detected.kind === 'model3d' && detected.format) {
      return FORMAT_LABELS[detected.format] || $i18nT('fileViewer.kind3d');
    }
    return $i18nT(`fileViewer.kind${detected.kind.charAt(0).toUpperCase()}${detected.kind.slice(1)}`);
  })();

  // Safe href / src — site-relative paths inherit the page's http(s) scheme and
  // pass through unchanged; unsafe schemes (javascript:, data:…) become undefined.
  $: safeHref = safeExternalUrl(url);
  $: safeImg = safeImageUrl(url);

  // Resolve a site-relative path to an absolute URL for the 3D viewer / fetch
  // (Model3D and fetch both want a fully-qualified or same-origin URL).
  function resolveUrl(u) {
    try {
      return new URL(u, typeof window !== 'undefined' ? window.location.origin : 'http://localhost').href;
    } catch {
      return u;
    }
  }

  // ── Inline text / JSON preview ───────────────────────────────────────────
  let textState = 'idle'; // idle | loading | ready | error
  let textBody = '';
  let textTruncated = false;
  let textError = '';

  async function loadText() {
    if (!detected || (detected.kind !== 'text' && detected.kind !== 'json')) return;
    const forUrl = url;
    textState = 'loading';
    textBody = '';
    textTruncated = false;
    textError = '';
    try {
      const res = await fetch(resolveUrl(url));
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      let body = await res.text();
      if (forUrl !== url) return; // navigated away while loading
      if (body.length > MAX_TEXT_BYTES) {
        body = body.slice(0, MAX_TEXT_BYTES);
        textTruncated = true;
      }
      if (detected.kind === 'json') {
        try {
          body = JSON.stringify(JSON.parse(body), null, 2);
        } catch {
          // Not valid JSON (or truncated mid-document) — show the raw text.
        }
      }
      textBody = body;
      textState = 'ready';
    } catch (e) {
      if (forUrl !== url) return;
      textError = e?.message || String(e);
      textState = 'error';
    }
  }

  // Re-fetch whenever the file (or its kind) changes — the resource page reuses
  // this component across navigations.
  $: if (detected && (detected.kind === 'text' || detected.kind === 'json')) {
    // touch `url` so Svelte tracks it as a dependency of this reactive block
    void url;
    loadText();
  }

  onMount(() => {});
</script>

<div class="file-viewer">
  <div class="fv-head">
    <div class="fv-id">
      {#if detected?.kind === 'model3d'}<Boxes size={16} />
      {:else if detected?.kind === 'image'}<ImageIcon size={16} />
      {:else if detected?.kind === 'pdf'}<FileType2 size={16} />
      {:else if detected?.kind === 'json'}<FileJson size={16} />
      {:else if detected?.kind === 'text'}<FileText size={16} />
      {:else}<FileIcon size={16} />{/if}
      <span class="fv-name" title={fileName}>{fileName}</span>
      {#if typeLabel}<span class="fv-type">{typeLabel}</span>{/if}
    </div>
    <div class="fv-actions">
      {#if safeHref}
        <a class="fv-btn" href={safeHref} target="_blank" rel="noopener" title={$i18nT('fileViewer.openInNewTab')}>
          <ExternalLink size={13} /> {$i18nT('fileViewer.openInNewTab')}
        </a>
        <a class="fv-btn" href={safeHref} download={fileName} title={$i18nT('fileViewer.download')}>
          <Download size={13} /> {$i18nT('fileViewer.download')}
        </a>
      {/if}
    </div>
  </div>

  <div class="fv-body">
    {#if !detected}
      <p class="fv-muted">{$i18nT('fileViewer.unknownFile')}</p>
    {:else if detected.kind === 'model3d'}
      {#await model3d() then mod}
        <svelte:component
          this={mod.default}
          refs={[{ id: resolveUrl(url), url: resolveUrl(url), format: detected.format, upAxis: null }]}
          {height}
        />
      {:catch}
        <p class="fv-muted">{$i18nT('fileViewer.previewFailed')}</p>
      {/await}
    {:else if detected.kind === 'image'}
      {#if safeImg}
        <div class="fv-image-wrap">
          <img src={safeImg} alt={fileName} loading="lazy" />
        </div>
      {:else}
        <p class="fv-muted">{$i18nT('fileViewer.unsafeUrl')}</p>
      {/if}
    {:else if detected.kind === 'pdf'}
      {#if safeHref}
        <iframe class="fv-pdf" src={safeHref} title={fileName} style="height:{height}"></iframe>
      {:else}
        <p class="fv-muted">{$i18nT('fileViewer.unsafeUrl')}</p>
      {/if}
    {:else if detected.kind === 'text' || detected.kind === 'json'}
      {#if textState === 'loading'}
        <p class="fv-muted">{$i18nT('fileViewer.loading')}</p>
      {:else if textState === 'error'}
        <p class="fv-error">{$i18nT('fileViewer.fetchError', { values: { error: textError } })}</p>
      {:else if textState === 'ready'}
        <pre class="fv-pre" class:json={detected.kind === 'json'}>{textBody}</pre>
        {#if textTruncated}
          <p class="fv-muted fv-trunc">{$i18nT('fileViewer.truncated')}</p>
        {/if}
      {/if}
    {:else}
      <div class="fv-binary">
        <FileIcon size={28} />
        <div>
          <strong>{fileName}</strong>
          <p class="fv-muted">{$i18nT('fileViewer.binaryDesc')}</p>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .file-viewer { display: flex; flex-direction: column; gap: 0.75rem; }
  .fv-head { display: flex; align-items: center; justify-content: space-between; gap: 0.75rem; flex-wrap: wrap; }
  .fv-id { display: inline-flex; align-items: center; gap: 0.45rem; min-width: 0; color: var(--ink-800, #2b3445); }
  .fv-name { font-family: monospace; font-size: 0.85rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%; }
  .fv-type { flex-shrink: 0; font-size: 0.7rem; padding: 1px 8px; border-radius: 999px; background: #eef5ff; color: #1565c0; border: 1px solid #cfe1fb; }

  .fv-actions { display: inline-flex; gap: 0.4rem; flex-shrink: 0; flex-wrap: wrap; }
  .fv-btn { display: inline-flex; align-items: center; gap: 0.3rem; padding: 0.3rem 0.65rem; border-radius: 10px; border: 1px solid var(--line-soft, #e5e9ee); background: #fff; color: #4a90d9; font-size: 0.78rem; text-decoration: none; transition: background .12s, border-color .12s; }
  .fv-btn:hover { background: #e8f2fc; border-color: var(--brand-300, #90caf9); }

  .fv-body { min-height: 40px; }
  .fv-muted { color: #888; font-size: 0.85rem; margin: 0; }
  .fv-error { color: #b3261e; font-size: 0.85rem; margin: 0; }
  .fv-trunc { margin-top: 0.4rem; }

  .fv-image-wrap { display: flex; justify-content: center; padding: 0.5rem; background: var(--bg-soft, #f9fafb); border: 1px solid var(--line-soft, #e5e7eb); border-radius: 10px; }
  .fv-image-wrap img { max-width: 100%; max-height: 480px; object-fit: contain; }

  .fv-pdf { width: 100%; border: 1px solid var(--line-soft, #e5e7eb); border-radius: 10px; background: #fff; }

  .fv-pre { margin: 0; padding: 0.85rem 1rem; max-height: 480px; overflow: auto; background: var(--bg-soft, #f7f9fc); border: 1px solid var(--line-soft, #e5e9ee); border-radius: 10px; font-family: monospace; font-size: 0.78rem; line-height: 1.5; white-space: pre; color: var(--ink-900, #1f2937); }

  .fv-binary { display: flex; align-items: center; gap: 0.85rem; padding: 1rem 1.1rem; background: var(--bg-soft, #f7f9fc); border: 1px solid var(--line-soft, #e5e9ee); border-radius: 12px; color: var(--ink-700, #555); }
  .fv-binary strong { display: block; margin-bottom: 0.2rem; word-break: break-all; }

  :global(:is([data-theme="dark"], .dark)) .fv-id { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .fv-type { background: rgba(59,130,246,0.16); color: #93c5fd; border-color: rgba(59,130,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .fv-btn { background: var(--bg-strong); color: #60a5fa; border-color: var(--line-strong, rgba(255,255,255,0.12)); }
  :global(:is([data-theme="dark"], .dark)) .fv-btn:hover { background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .fv-muted { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .fv-image-wrap,
  :global(:is([data-theme="dark"], .dark)) .fv-pre,
  :global(:is([data-theme="dark"], .dark)) .fv-binary { background: rgba(255,255,255,0.04); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .fv-pre { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .fv-binary { color: var(--ink-600); }
</style>
