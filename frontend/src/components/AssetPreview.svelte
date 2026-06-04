<script>
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { fetchAssetContent, assetMetadata } from '../lib/api.js';
  import { X, Download, ZoomIn, ZoomOut, FileText, Image as ImageIcon, Code2, ExternalLink } from 'lucide-svelte';

  /**
   * Asset preview modal.
   * Props:
   *   asset     - { id, filename, content_type|mime_type, size_bytes|size, public? }
   *   datasetId - for fetchAssetContent
   * Events:
   *   close
   */

  /** @type {{ id?: string, filename?: string, content_type?: string, mime_type?: string, size_bytes?: number, size?: number, public?: boolean } | null} */
  export let asset = null;
  export let datasetId = null;

  const dispatch = createEventDispatcher();

  let content = null;       // text content
  let objectUrl = null;     // for images / PDFs
  let loadingContent = false;
  let contentError = null;
  let imgZoom = 1;
  let backdropEl;
  let meta = null;          // typed asset metadata from the triplestore

  // Build a compact one-line summary of the typed metadata, e.g.
  // "1920×1080 · 2.3 MB · 1,240,000 points · sha256 1a2b3c…".
  function fmtDuration(iso) {
    if (!iso) return null;
    const m = iso.match(/PT([\d.]+)S/); if (!m) return iso;
    const s = Math.round(Number(m[1]));
    return s >= 60 ? `${Math.floor(s/60)}m ${s%60}s` : `${s}s`;
  }
  $: metaSummary = (() => {
    if (!meta) return '';
    const p = [];
    if (meta.width && meta.height) p.push(`${meta.width}×${meta.height}${meta.panorama ? ' 360°' : ''}`);
    if (meta.pages) p.push($t('components.assetPreview.metaPages', { values: { count: meta.pages } }));
    const d = fmtDuration(meta.duration); if (d) p.push(d);
    if (meta.point_count) p.push($t('components.assetPreview.metaPoints', { values: { count: meta.point_count.toLocaleString() } }));
    if (meta.entry_count) p.push($t('components.assetPreview.metaFiles', { values: { count: meta.entry_count } }));
    if (meta.sheet_count) p.push($t('components.assetPreview.metaSheets', { values: { count: meta.sheet_count } }));
    if (meta.row_count) p.push($t('components.assetPreview.metaRows', { values: { count: meta.row_count.toLocaleString() } }));
    if (meta.sha256) p.push(`sha256 ${meta.sha256.slice(0, 8)}…`);
    return p.join(' · ');
  })();

  // Normalise asset fields regardless of API naming variant
  $: mime = asset?.mime_type || asset?.content_type || '';
  $: sizeBytes = asset?.size ?? asset?.size_bytes ?? null;

  // Determine category from MIME or filename extension
  function fileCategory(mime, filename) {
    const m = (mime || '').toLowerCase().split(';')[0].trim();
    const n = (filename || '').toLowerCase();
    const ext = n.split('.').pop();
    if (m.startsWith('image/') || ['png','jpg','jpeg','gif','svg','webp','bmp','ico'].includes(ext))
      return 'image';
    if (m === 'application/pdf' || ext === 'pdf')
      return 'pdf';
    if (m.startsWith('audio/') || ['mp3','ogg','wav','flac','m4a','aac'].includes(ext))
      return 'audio';
    if (m.startsWith('video/') || ['mp4','webm','ogv','mov'].includes(ext))
      return 'video';
    if (/text\/turtle|application\/n-triples|text\/n3/.test(m) || ['ttl','n3','nt','trig','nq'].includes(ext))
      return 'rdf';
    if (m === 'application/json' || m === 'application/ld+json' || ['json','jsonld'].includes(ext))
      return 'code';
    if (['xml','rdf','xsd'].includes(ext) || m.includes('xml'))
      return 'code';
    if (m.startsWith('text/') || ['txt','md','csv','tsv','yaml','yml','sparql','rq','sh','toml'].includes(ext))
      return 'text';
    return 'binary';
  }

  $: category = fileCategory(mime, asset?.filename);
  $: isText   = ['text','code','rdf','csv'].includes(category);
  $: isBinary = ['image','pdf','audio','video'].includes(category);

  // Reload content when asset changes
  $: if (asset && datasetId) { loadContent(); }

  function revokeUrl() {
    if (objectUrl) { URL.revokeObjectURL(objectUrl); objectUrl = null; }
  }

  // Fetch the triplestore's typed metadata (best-effort; failure just hides the summary).
  async function loadMeta() {
    meta = null;
    if (!asset || !datasetId) return;
    try {
      meta = await assetMetadata(datasetId, asset.id);
    } catch {
      meta = null;
    }
  }
  $: if (asset && datasetId) { loadMeta(); }

  async function loadContent() {
    if (!asset) return;
    loadingContent = true;
    contentError = null;
    content = null;
    revokeUrl();

    try {
      const res = await fetchAssetContent(datasetId, asset.id);
      if (!res.ok) {
        if (res.status === 401 || res.status === 403)
          throw new Error($t('components.assetPreview.accessDenied'));
        throw new Error(`HTTP ${res.status}`);
      }

      if (isText) {
        content = await res.text();
      } else if (isBinary) {
        const blob = await res.blob();
        objectUrl = URL.createObjectURL(blob);
      }
    } catch (e) {
      contentError = e.message || $t('components.assetPreview.loadFailed');
    } finally {
      loadingContent = false;
    }
  }

  function close() {
    revokeUrl();
    dispatch('close');
  }

  function onBackdrop(e) {
    if (e.target === backdropEl) close();
  }

  function onKeydown(e) {
    if (e.key === 'Escape') close();
  }

  function zoomImg(delta) {
    imgZoom = Math.max(0.2, Math.min(4, imgZoom + delta));
  }

  async function downloadAsset() {
    if (!datasetId || !asset) return;
    try {
      const res = await fetchAssetContent(datasetId, asset.id);
      if (!res.ok) return;
      const blob = await res.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = asset.filename || 'asset';
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 5000);
    } catch {}
  }

  onMount(() => {
    window.addEventListener('keydown', onKeydown);
  });

  onDestroy(() => {
    window.removeEventListener('keydown', onKeydown);
    revokeUrl();
  });

  function formatBytes(b) {
    if (!b) return '';
    if (b < 1024) return `${b} B`;
    if (b < 1024 * 1024) return `${(b/1024).toFixed(1)} KB`;
    return `${(b/1024/1024).toFixed(1)} MB`;
  }

  $: lines = content ? content.split('\n') : [];
</script>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<div class="preview-backdrop" bind:this={backdropEl} on:click={onBackdrop} role="dialog" aria-modal="true" tabindex="-1">
  <div class="preview-modal">

    <!-- Header -->
    <div class="preview-header">
      <div class="preview-title-area">
        {#if category === 'image'}
          <ImageIcon size={16} class="file-icon" />
        {:else if category === 'rdf' || category === 'code'}
          <Code2 size={16} class="file-icon" />
        {:else}
          <FileText size={16} class="file-icon" />
        {/if}
        <span class="preview-filename">{asset?.filename || $t('components.assetPreview.fileFallback')}</span>
        {#if sizeBytes}
          <span class="preview-size">{formatBytes(sizeBytes)}</span>
        {/if}
        {#if mime}
          <span class="preview-mime">{mime}</span>
        {/if}
        {#if metaSummary}
          <span class="preview-typed" title={$t('components.assetPreview.typedMetadataTooltip')}>{metaSummary}</span>
        {/if}
      </div>
      <div class="preview-actions">
        {#if category === 'image'}
          <button class="hdr-btn" on:click={() => zoomImg(-0.25)} title={$t('components.assetPreview.zoomOut')}><ZoomOut size={15}/></button>
          <span class="zoom-label">{Math.round(imgZoom * 100)}%</span>
          <button class="hdr-btn" on:click={() => zoomImg(0.25)} title={$t('components.assetPreview.zoomIn')}><ZoomIn size={15}/></button>
        {/if}
        {#if objectUrl}
          <a href={objectUrl} target="_blank" rel="noopener noreferrer" class="hdr-btn" title={$t('components.assetPreview.openInNewTab')}>
            <ExternalLink size={15}/>
          </a>
        {/if}
        <button class="hdr-btn" on:click={downloadAsset} title={$t('components.assetPreview.download')}><Download size={15}/></button>
        <button class="hdr-btn hdr-close" on:click={close} title={$t('components.assetPreview.closeEsc')}><X size={16}/></button>
      </div>
    </div>

    <!-- Body -->
    <div class="preview-body">

      {#if loadingContent}
        <div class="preview-loading">
          <div class="loading-spinner"></div>
          {$t('components.assetPreview.loadingPreview')}
        </div>

      {:else if contentError}
        <div class="preview-error">
          <p>{contentError}</p>
          <button class="btn btn-sm" on:click={downloadAsset}><Download size={13}/> {$t('components.assetPreview.downloadInstead')}</button>
        </div>

      <!-- Image preview -->
      {:else if category === 'image' && objectUrl}
        <div class="img-container">
          <img
            src={objectUrl}
            alt={asset?.filename}
            style="transform: scale({imgZoom}); transform-origin: center center;"
            class="preview-img"
          />
        </div>

      <!-- PDF preview -->
      {:else if category === 'pdf' && objectUrl}
        <embed src={objectUrl} type="application/pdf" class="pdf-frame" />

      <!-- Audio -->
      {:else if category === 'audio' && objectUrl}
        <div class="preview-audio-wrap">
          <audio controls class="preview-audio" src={objectUrl}></audio>
        </div>

      <!-- Video -->
      {:else if category === 'video' && objectUrl}
        <!-- svelte-ignore a11y-media-has-caption -->
        <video controls class="preview-video" src={objectUrl}></video>

      <!-- Text / code / RDF / CSV preview -->
      {:else if isText && content !== null}
        <div class="code-view" class:rdf-view={category === 'rdf'}>
          <table class="code-table" aria-label={$t('components.assetPreview.fileContent')}>
            <tbody>
              {#each lines as line, i}
                <tr>
                  <td class="line-num">{i + 1}</td>
                  <td class="line-code">{line}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>

      <!-- Binary fallback -->
      {:else if !loadingContent}
        <div class="preview-unavailable">
          <FileText size={48} />
          <p>{$t('components.assetPreview.previewUnavailable')}</p>
          <button class="btn btn-sm" on:click={downloadAsset}><Download size={14}/> {$t('components.assetPreview.downloadFile')}</button>
        </div>
      {/if}
    </div>

  </div>
</div>

<style>
  .preview-backdrop {
    position: fixed; inset: 0;
    background: rgba(15, 23, 42, 0.55);
    display: flex; align-items: center; justify-content: center;
    z-index: 50000;
    backdrop-filter: blur(2px);
  }

  .preview-modal {
    background: #fff;
    border-radius: 14px;
    width: min(90vw, 980px);
    height: min(88vh, 700px);
    display: flex; flex-direction: column;
    overflow: hidden;
    box-shadow: 0 20px 60px rgba(0,0,0,0.22);
    animation: modalIn 0.18s ease;
  }

  @keyframes modalIn {
    from { opacity: 0; transform: scale(0.96) translateY(8px); }
    to   { opacity: 1; transform: scale(1) translateY(0); }
  }

  /* ── Header ── */
  .preview-header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 12px 16px;
    border-bottom: 1px solid #f1f5f9;
    background: #f8fafc;
    flex-shrink: 0; gap: 1rem;
  }

  .preview-title-area {
    display: flex; align-items: center; gap: 8px;
    min-width: 0; flex: 1;
  }

  .preview-title-area :global(.file-icon) { color: #3b82f6; flex-shrink: 0; }

  .preview-filename {
    font-weight: 700; font-size: 0.9rem; color: #1e293b;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }

  .preview-size {
    font-size: 0.72rem; color: #94a3b8; background: #f1f5f9;
    padding: 1px 6px; border-radius: 4px; flex-shrink: 0;
  }

  .preview-mime {
    font-size: 0.7rem; color: #64748b; background: #e0f2fe;
    padding: 1px 6px; border-radius: 4px; flex-shrink: 0;
    font-family: monospace;
  }

  .preview-typed {
    font-size: 0.7rem; color: #0369a1; background: #f0f9ff;
    padding: 1px 6px; border-radius: 4px;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    min-width: 0;
  }

  .preview-actions {
    display: flex; align-items: center; gap: 4px; flex-shrink: 0;
  }

  .zoom-label {
    font-size: 0.75rem; color: #64748b; font-variant-numeric: tabular-nums;
    min-width: 36px; text-align: center;
  }

  .hdr-btn {
    width: 30px; height: 30px; border: 1px solid #e2e8f0;
    border-radius: 7px; background: #fff; cursor: pointer;
    display: flex; align-items: center; justify-content: center;
    color: #475569; padding: 0; text-decoration: none;
    transition: background 0.12s, color 0.12s, border-color 0.12s;
  }
  .hdr-btn:hover { background: #eff6ff; color: #2563eb; border-color: #bfdbfe; }
  .hdr-close:hover { background: #fee2e2; color: #dc2626; border-color: #fca5a5; }

  /* ── Body ── */
  .preview-body {
    flex: 1; overflow: hidden; position: relative;
  }

  /* Image */
  .img-container {
    width: 100%; height: 100%;
    display: flex; align-items: center; justify-content: center;
    overflow: auto;
    background:
      linear-gradient(45deg, #e2e8f0 25%, transparent 25%) 0 0 / 16px 16px,
      linear-gradient(-45deg, #e2e8f0 25%, transparent 25%) 0 8px / 16px 16px,
      linear-gradient(45deg, transparent 75%, #e2e8f0 75%) 8px -8px / 16px 16px,
      linear-gradient(-45deg, transparent 75%, #e2e8f0 75%) -8px 0 / 16px 16px,
      #f8fafc;
    padding: 24px;
  }

  .preview-img {
    max-width: 100%; max-height: 100%;
    object-fit: contain;
    border-radius: 4px;
    transition: transform 0.15s;
    display: block;
  }

  /* PDF */
  .pdf-frame {
    width: 100%; height: 100%;
    border: none; display: block;
  }

  /* Audio / Video */
  .preview-audio-wrap {
    display: flex; align-items: center; justify-content: center;
    height: 100%; padding: 2rem;
  }
  .preview-audio { width: 100%; max-width: 600px; }
  .preview-video { width: 100%; height: 100%; background: #000; display: block; }

  /* Code / text */
  .code-view {
    width: 100%; height: 100%;
    overflow: auto;
    background: #1e293b;
    color: #e2e8f0;
  }

  .rdf-view {
    background: #0f172a;
  }

  .code-table {
    border-collapse: collapse;
    width: 100%;
    font-family: 'IBM Plex Mono', 'Fira Mono', monospace;
    font-size: 12.5px;
    line-height: 1.55;
  }

  .line-num {
    width: 48px; min-width: 48px;
    padding: 0 12px 0 8px;
    color: #475569; text-align: right;
    user-select: none;
    border-right: 1px solid #334155;
    background: #1a2332;
    vertical-align: top;
    font-size: 11px;
    white-space: nowrap;
  }

  .line-code {
    padding: 0 16px;
    white-space: pre;
    color: #e2e8f0;
    vertical-align: top;
  }

  /* Alternate row highlight */
  .code-table tr:hover .line-code { background: rgba(59,130,246,0.08); }
  .code-table tr:hover .line-num { background: #1f2d3d; }

  /* Loading / error / unavailable */
  .preview-loading,
  .preview-error,
  .preview-unavailable {
    display: flex; flex-direction: column;
    align-items: center; justify-content: center;
    height: 100%; gap: 1rem;
    color: #94a3b8; font-size: 0.9rem; text-align: center;
    padding: 2rem;
  }

  .preview-error { color: #dc2626; }

  .preview-unavailable :global(svg) { opacity: 0.3; }

  .loading-spinner {
    width: 28px; height: 28px;
    border: 3px solid #e2e8f0;
    border-top-color: #3b82f6;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin { to { transform: rotate(360deg); } }

  :global(:is([data-theme="dark"], .dark)) .preview-modal { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .preview-header { background: var(--bg-soft); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .preview-filename { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .preview-size { background: rgba(255,255,255,0.08); }
  :global(:is([data-theme="dark"], .dark)) .preview-mime { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .preview-typed { background: rgba(14,165,233,0.16); color: #7dd3fc; }
  :global(:is([data-theme="dark"], .dark)) .hdr-btn { background: var(--bg-soft); border-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .hdr-btn:hover { background: rgba(59,130,246,0.15); color: #93c5fd; border-color: rgba(59,130,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .hdr-close:hover { background: rgba(239,68,68,0.18); color: #fca5a5; border-color: rgba(239,68,68,0.35); }
  :global(:is([data-theme="dark"], .dark)) .preview-error { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .loading-spinner { border-color: var(--line-strong); border-top-color: #3b82f6; }
</style>
