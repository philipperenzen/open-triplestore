<script>
  // Singleton floating preview panel (mounted once in App.svelte): shows a 3D
  // model or a map for whatever RDF term requested it via lib/viewer/preview —
  // so the triple table, graph explorer and resource panels all get 3D/geo
  // previews without owning a viewer. Draggable by its header, two sizes
  // (large / fullscreen). Theme follows the app's CSS variables.
  import { t as i18nT } from 'svelte-i18n';
  import { X, Maximize2, Minimize2 } from 'lucide-svelte';
  import { preview, closePreview } from '../../lib/viewer/preview';
  import { inspectorTopZ, previewZ } from '../../lib/viewer/zLayers';

  // Model3D (three.js) and GeoPreview (leaflet) both pull heavy vendor chunks —
  // load each only when its preview is actually opened. This overlay is mounted
  // globally in App.svelte, so a static import would drag both into the main
  // bundle that loads on every page.
  const model3d = () => import('./Model3D.svelte');
  const geoPreview = () => import('../GeoPreview.svelte');

  let full = false;
  let pos = { x: 0, y: 0 };
  let dragging = null;

  function startDrag(e) {
    if (full || e.target.closest('button')) return;
    dragging = { x: e.clientX - pos.x, y: e.clientY - pos.y };
    window.addEventListener('pointermove', onDrag);
    window.addEventListener('pointerup', stopDrag, { once: true });
  }
  function onDrag(e) {
    if (!dragging) return;
    pos = { x: e.clientX - dragging.x, y: e.clientY - dragging.y };
  }
  function stopDrag() {
    dragging = null;
    window.removeEventListener('pointermove', onDrag);
  }
  function close() {
    full = false;
    pos = { x: 0, y: 0 };
    closePreview();
  }
  function onKeydown(e) {
    if (e.key !== 'Escape' || !$preview) return;
    // Topmost panel owns Escape: mark it consumed so a panel underneath
    // (e.g. the dataset explorer's ElementModal) doesn't also close.
    e.preventDefault();
    close();
  }
</script>

<!-- Capture phase, deliberately. This overlay renders above the viewer's
     inspector windows, so it must be the one that answers Escape. Bubble-phase
     window listeners fire in registration order, and this component is mounted
     once in App.svelte — any window opened later would otherwise get Escape
     first. Capturing keeps the existing contract intact: we only mark the event
     defaultPrevented, so handlers underneath still see it and can bow out
     (DatasetViewer's single Escape handler checks `e.defaultPrevented`). -->
<svelte:window on:keydown|capture={onKeydown} />

{#if $preview}
  <!-- z-index comes from lib/viewer/zLayers.ts rather than the stylesheet: the
       viewer raises its focused inspector window with a running counter, and a
       fixed literal here is how the overlay ended up buried underneath one. -->
  <div
    class="preview-overlay"
    class:full
    style:z-index={previewZ($inspectorTopZ)}
    style:transform={full ? '' : `translate(${pos.x}px, ${pos.y}px)`}
    role="dialog"
    aria-label={$preview.title}
  >
    <!-- Drag handle: pointer-only affordance; all controls inside stay
         keyboard-accessible and Escape closes the panel. -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <header on:pointerdown={startDrag}>
      <span class="title" title={$preview.title}>{$preview.title}</span>
      <div class="actions">
        <button on:click={() => (full = !full)} title={$i18nT('viewer.resize')} aria-label={$i18nT('viewer.resize')}>
          {#if full}<Minimize2 size={14} />{:else}<Maximize2 size={14} />{/if}
        </button>
        <button on:click={close} title={$i18nT('viewer.close')} aria-label={$i18nT('viewer.close')}>
          <X size={14} />
        </button>
      </div>
    </header>
    <div class="body">
      {#if $preview.kind === 'model'}
        {#await model3d()}
          <p class="loading">…</p>
        {:then mod}
          <svelte:component
            this={mod.default}
            refs={[{ id: $preview.url, label: $preview.title, url: $preview.url, format: $preview.format }]}
            height="100%"
          />
        {/await}
      {:else}
        {#await geoPreview()}
          <p class="loading">…</p>
        {:then mod}
          <svelte:component this={mod.default} wkts={$preview.wkts} height="100%" />
        {/await}
      {/if}
    </div>
  </div>
{/if}

<style>
  .preview-overlay {
    position: fixed;
    /* Resting value only — the inline style above (Z_PREVIEW … Z_DOCK - 1) is
       the authority. Kept so the panel still stacks sanely if that ever fails. */
    z-index: 1200;
    right: 32px;
    bottom: 32px;
    width: min(560px, calc(100vw - 48px));
    height: min(440px, calc(100vh - 96px));
    display: flex;
    flex-direction: column;
    background: var(--bg-elevated, #fff);
    border: 1px solid var(--border, #e2e8f0);
    border-radius: var(--radius-lg, 14px);
    box-shadow: var(--shadow-lg, 0 18px 50px rgba(0, 0, 0, 0.25));
    overflow: hidden;
    backdrop-filter: blur(8px);
  }
  .preview-overlay.full {
    inset: 4vh 4vw;
    width: auto;
    height: auto;
    transform: none;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 7px 8px 7px 12px;
    border-bottom: 1px solid var(--line-soft, #eef1f4);
    cursor: move;
    user-select: none;
  }
  .title {
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--ink-900, #0f172a);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .actions {
    display: flex;
    gap: 2px;
  }
  .actions button {
    border: 0;
    background: transparent;
    padding: 5px;
    border-radius: var(--radius-sm, 6px);
    cursor: pointer;
    color: var(--muted, #64748b);
    display: grid;
    place-items: center;
  }
  .actions button:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.05));
    color: var(--ink-900, #0f172a);
  }
  .actions button:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 2px var(--brand-400, #5aa9e0);
  }
  .body {
    flex: 1;
    min-height: 0;
  }
  .loading {
    margin: 0;
    height: 100%;
    display: grid;
    place-items: center;
    color: var(--muted, #64748b);
  }
</style>
