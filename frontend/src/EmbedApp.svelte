<script>
  // Chrome-less embeddable viewer, mounted instead of the full App when the
  // path starts with /embed (see main.ts). Meant to be iframed by external
  // sites/webapps:
  //
  //   /embed/map/<dataset-id>       MapLibre map + to-scale 3D models
  //   /embed/3d/<dataset-id>        orbitable 3D models of the dataset
  //   /embed/cesium/<dataset-id>    CesiumJS 3D-Tiles globe
  //   /embed/model?src=<url>        one model file by URL (glTF/IFC/CityJSON/STL)
  //
  // Query params (all optional): element=<IRI> selects + focuses one element,
  // basemap=streets|satellite, theme=light|dark (not persisted — the host page
  // decides), format=<gltf|ifc|…> for /embed/model when the src URL carries no
  // file extension.
  //
  // Element picks are forwarded to the host page via postMessage:
  //   { source: 'open-triplestore', type: 'select', dataset, id, guid }
  // so an embedding webapp can react to clicks (see docs/embedding.md).
  //
  // No auth UI: anonymous access works for public datasets; a private dataset
  // shows a sign-in hint linking to the full app.
  import { t as i18nT } from 'svelte-i18n';
  import { getViewerFeed } from './lib/api.js';
  import { isDark } from './lib/theme.js';
  import { modelRefs } from './lib/viewer/geometry';
  import { modelFormatFromUrl } from './lib/viewer/detect';
  import ViewerMap from './components/viewer/ViewerMap.svelte';
  import Model3D from './components/viewer/Model3D.svelte';
  import CesiumViewer from './components/viewer/CesiumViewer.svelte';

  // ── URL → view state (parsed once; embeds are single-view pages) ───────────
  const segs = window.location.pathname.split('/').filter(Boolean); // ['embed', kind, id?]
  const kind = segs[1] || '';
  const datasetId = decodeURIComponent(segs[2] || '');
  const params = new URLSearchParams(window.location.search);
  const elementParam = params.get('element') || '';
  const basemapParam = params.get('basemap') === 'satellite' ? 'satellite' : 'streets';

  // Theme override — applied directly (NOT via setTheme, which would persist
  // the host page's choice into the user's own app preference).
  const themeParam = params.get('theme');
  if (themeParam === 'dark' || themeParam === 'light') {
    const dark = themeParam === 'dark';
    document.documentElement.classList.toggle('dark', dark);
    document.documentElement.setAttribute('data-theme', themeParam);
    isDark.set(dark);
  }

  const KNOWN = ['map', '3d', 'cesium', 'model'];
  const valid = KNOWN.includes(kind) && (kind === 'model' ? !!params.get('src') : !!datasetId);

  let elements = [];
  let loading = kind !== 'model';
  let error = '';
  let unauthorized = false;
  let selected = elementParam;
  let mapComponent;

  $: hasGeo = elements.some((e) => e.wkt4326);
  $: refs = modelRefs(elements);

  // /embed/model — a single file by URL, no dataset feed involved.
  const srcParam = params.get('src') || '';
  const srcFormat = params.get('format') || modelFormatFromUrl(srcParam) || '';
  const modelRefsSingle =
    kind === 'model' && srcParam && srcFormat
      ? [{ id: srcParam, label: srcParam.split('/').pop() || 'model', url: srcParam, format: srcFormat, slot: [0, 0], upAxis: params.get('up') || null }]
      : [];

  async function load() {
    if (!valid || kind === 'model') return;
    try {
      // Fast located subset first so the map paints immediately…
      if (kind === 'map') {
        try {
          const fast = await getViewerFeed(datasetId, null, { located: true });
          elements = fast?.elements || [];
          loading = false;
        } catch {
          /* the full feed below is the source of truth */
        }
      }
      // …then the full feed (models + structure) swaps in behind it.
      const full = await getViewerFeed(datasetId);
      if (full?.elements) elements = full.elements;
      loading = false;
      if (elementParam) {
        selected = elementParam;
        // `force`: an ?element= deep link is an explicit framing request, so it
        // must move the camera even when the element happens to already be in
        // view at the dataset's default extent.
        setTimeout(() => mapComponent?.focusElement?.(elementParam, { force: true }), 400);
      }
    } catch (e) {
      loading = false;
      if (!elements.length) {
        unauthorized = /401|403/.test(String(e?.message || ''));
        error = e?.message || 'failed';
      }
    }
  }

  function postSelect(detail) {
    if (window.parent === window) return;
    try {
      window.parent.postMessage(
        { source: 'open-triplestore', type: 'select', dataset: datasetId, id: detail.id || null, guid: detail.guid || null },
        '*'
      );
    } catch {
      /* host page gone — nothing to do */
    }
  }

  function onSelect(event) {
    const { id, guid } = event.detail || {};
    // An IFC mesh pick carries a GlobalId — prefer that element, like the full viewer.
    const byGuid = guid ? elements.find((e) => e.ifc_guid === guid) : null;
    selected = byGuid?.id || id || selected;
    postSelect({ id: selected, guid });
  }

  $: selectedLabel = (() => {
    const el = elements.find((e) => e.id === selected);
    return el ? el.label || el.id.split(/[/#]/).pop() : '';
  })();

  const appHref = kind === 'model' ? '/' : `/datasets/${encodeURIComponent(datasetId)}/viewer`;

  if (kind !== 'model') load();
</script>

<div class="embed-shell">
  {#if !valid}
    <div class="embed-msg">
      <strong>Open Triplestore embed</strong>
      <p>
        Use <code>/embed/map/&lt;dataset&gt;</code>, <code>/embed/3d/&lt;dataset&gt;</code>,
        <code>/embed/cesium/&lt;dataset&gt;</code> or <code>/embed/model?src=&lt;url&gt;</code>.
      </p>
    </div>
  {:else if error && !elements.length}
    <div class="embed-msg">
      <strong>{unauthorized ? $i18nT('embed.signInNeeded') : $i18nT('embed.loadFailed')}</strong>
      <p><a href={appHref} target="_blank" rel="noopener">{$i18nT('embed.openInApp')}</a></p>
    </div>
  {:else if kind === 'map'}
    {#if !loading && !hasGeo && refs.length}
      <!-- No located elements — same fallback as the full explorer: pure 3D. -->
      <Model3D {refs} {selected} height="100%" on:select={onSelect} />
    {:else}
      <ViewerMap bind:this={mapComponent} {elements} {selected} basemap={basemapParam} height="100%" on:select={onSelect} />
    {/if}
  {:else if kind === 'cesium'}
    <CesiumViewer {datasetId} {selected} embedded expand={false} height="100%" on:select={onSelect} />
  {:else if kind === '3d'}
    {#if loading}
      <div class="embed-msg"><span class="embed-spin"></span></div>
    {:else}
      <Model3D refs={refs} {selected} height="100%" on:select={onSelect} />
    {/if}
  {:else if kind === 'model'}
    {#if modelRefsSingle.length}
      <Model3D refs={modelRefsSingle} height="100%" />
    {:else}
      <div class="embed-msg"><strong>{$i18nT('embed.badModelSrc')}</strong></div>
    {/if}
  {/if}

  <!-- Attribution / deep link — the one piece of chrome an embed keeps. -->
  <div class="embed-bar">
    {#if selectedLabel}
      <span class="embed-sel" title={selected}>{selectedLabel}</span>
    {/if}
    <a class="embed-brand" href={appHref} target="_blank" rel="noopener" title="Open Triplestore">
      <svg viewBox="0 0 64 64" fill="none" aria-hidden="true" width="13" height="13">
        <circle cx="32" cy="32" r="19" stroke="#56b6bd" stroke-width="6" />
        <circle cx="32" cy="51" r="8" fill="#2F7A8C" />
        <circle cx="48.45" cy="22.5" r="8" fill="#2F7A8C" />
        <circle cx="15.55" cy="22.5" r="8" fill="#2F7A8C" />
      </svg>
      Open Triplestore
    </a>
  </div>
</div>

<style>
  .embed-shell {
    position: fixed;
    inset: 0;
    display: flex;
    background: var(--bg, #fff);
  }
  .embed-shell > :global(.viewer-map-wrap),
  .embed-shell > :global(.model-3d) {
    flex: 1;
    min-height: 0;
  }
  .embed-msg {
    margin: auto;
    text-align: center;
    color: var(--ink-700, #334155);
    font: 400 0.9rem/1.5 system-ui, sans-serif;
    padding: 20px;
  }
  .embed-msg code {
    background: var(--bg-soft, #f1f5f9);
    padding: 1px 5px;
    border-radius: 5px;
    font-size: 0.8rem;
  }
  .embed-msg a {
    color: var(--brand-600, #2563a8);
  }
  .embed-spin {
    display: inline-block;
    width: 26px;
    height: 26px;
    border: 3px solid rgba(100, 116, 139, 0.25);
    border-top-color: #2f88d8;
    border-radius: 50%;
    animation: embed-sp 0.8s linear infinite;
  }
  @keyframes embed-sp {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .embed-spin {
      animation: none;
    }
  }
  .embed-bar {
    position: fixed;
    left: 8px;
    bottom: 8px;
    z-index: 30;
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: calc(100vw - 16px);
  }
  .embed-sel {
    padding: 3px 10px;
    border-radius: 999px;
    background: rgba(232, 89, 12, 0.92);
    color: #fff;
    font: 600 0.72rem/1.4 system-ui, sans-serif;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 45vw;
  }
  .embed-brand {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 9px;
    border-radius: 999px;
    background: rgba(15, 23, 42, 0.78);
    color: #eef4fb;
    font: 600 0.7rem/1.4 system-ui, sans-serif;
    text-decoration: none;
    backdrop-filter: blur(6px);
  }
  .embed-brand:hover {
    background: rgba(15, 23, 42, 0.92);
  }
</style>
