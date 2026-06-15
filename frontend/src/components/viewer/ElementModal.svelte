<script>
  // Element inspector modal for the dataset explorer. Draggable by its header,
  // expandable to fullscreen. Three tabs:
  //   Properties — the element's RDF (browse API + RdfTerm) and BIM/IFC facts
  //   Structure  — the BOT/IFC decomposition tree: the "part of" context leads,
  //                then the navigable sub-elements
  //   3D         — interactive model viewer (orbit: rotate / pan / zoom); when
  //                an element ships several formats, a chip switches between them
  import { createEventDispatcher } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { X, Maximize2, Minimize2, Boxes, ChevronRight, MapPin, CornerLeftUp } from 'lucide-svelte';
  import { browseResource } from '../../lib/api.js';
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import { safeExternalUrl } from '../../lib/safeUrl';
  import { Link } from '../../lib/router/index.js';
  import { modelRefOf, modelRefsOf, FORMAT_LABELS } from '../../lib/viewer/detect';
  import { preview } from '../../lib/viewer/preview';
  import RdfTerm from '../RdfTerm.svelte';
  import Model3D from './Model3D.svelte';

  /** The focused element (viewer-feed shape). */
  export let element = null;
  /** All feed elements — used to derive the substructure tree. */
  export let elements = [];
  export let datasetId = '';
  /** Whether the hosting page shows a map — enables the "Show on map" action. */
  export let hasMap = false;

  const dispatch = createEventDispatcher();

  let tab = 'properties';
  let full = false;
  let pos = { x: 0, y: 0 };
  let dragging = null;
  let data = null;
  let loading = false;
  let error = '';

  $: children = element ? elements.filter((e) => e.parent === element.id) : [];
  // The Structure tab only earns its place when there's containment to show — a
  // parent ("part of") or sub-elements. Hide it for a standalone element.
  $: hasStructure = children.length > 0 || !!element?.parent;
  // Don't strand the user on a now-hidden Structure tab (element switch).
  $: if (tab === 'structure' && !hasStructure) tab = 'properties';
  // parent id → number of children, in one pass (the structure tree reads a
  // count per row; filtering elements per row would be O(N²)).
  $: childCount = elements.reduce(
    (m, e) => (e.parent ? m.set(e.parent, (m.get(e.parent) || 0) + 1) : m),
    new Map()
  );

  // All linked 3D representations (glTF / CityJSON / CityGML / STL). The 3D tab
  // lets the user switch; the preferred format is the default.
  $: modelOptions = element ? modelRefsOf(element) : [];
  let chosenFormat = null;
  $: if (element?.id) chosenFormat = null; // element switch resets the choice
  $: modelRef = modelOptions.find((o) => o.format === chosenFormat) ?? modelOptions[0] ?? null;

  // Leading header icon: a 3D element reads as a model, a located one as a pin.
  $: parentEl = element?.parent ? elements.find((e) => e.id === element.parent) : null;

  async function load(iri) {
    if (!iri) return;
    loading = true;
    error = '';
    data = null;
    try {
      data = await browseResource(iri, { dataset_id: datasetId });
    } catch (e) {
      error = e?.message || 'failed';
    } finally {
      loading = false;
    }
  }

  function startDrag(e) {
    if (full || e.target.closest('button, a')) return;
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
  function onKeydown(e) {
    if (e.key !== 'Escape') return;
    // The preview overlay can be stacked on top (RdfTerm chips in the
    // Properties tab open it) and owns Escape while visible — both the
    // defaultPrevented mark and the store check make this robust regardless
    // of svelte:window listener order.
    if (e.defaultPrevented || $preview) return;
    dispatch('close');
  }

  $: load(element?.id);
  // Reset transient panel state when closed: the component stays mounted, so
  // the next element would otherwise open at the previous drag offset / size.
  $: if (!element) {
    pos = { x: 0, y: 0 };
    full = false;
  }
  // When the element loses its model, fall back from the 3D tab.
  $: if (tab === '3d' && !modelRef) tab = 'properties';

  // "Show on map" target: this element when it's located, else the nearest
  // located ancestor (a beam flies to its building/site anchor). Null hides the
  // action (no map on the page, or nothing in the chain has geometry).
  $: mapTargetId = (() => {
    if (!hasMap || !element) return null;
    let cur = element;
    const seen = new Set();
    while (cur && !cur.wkt4326 && cur.parent && !seen.has(cur.id)) {
      seen.add(cur.id);
      cur = elements.find((e) => e.id === cur.parent) || null;
    }
    return cur?.wkt4326 ? cur.id : null;
  })();
</script>

<svelte:window on:keydown={onKeydown} />

{#if element}
  <div
    class="element-modal"
    class:full
    style:transform={full ? '' : `translate(${pos.x}px, ${pos.y}px)`}
    role="dialog"
    aria-label={element.label || element.id}
  >
    <!-- Drag handle: pointer-only affordance; all controls inside stay
         keyboard-accessible and Escape closes the panel. -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <header on:pointerdown={startDrag}>
      {#if modelRef}
        <span class="head-icon model" aria-hidden="true"><Boxes size={16} /></span>
      {:else if element.wkt4326}
        <span class="head-icon geo" aria-hidden="true"><MapPin size={16} /></span>
      {/if}
      <div class="head-text">
        <h3>{element.label || shortenIRI(element.id)}</h3>
        <div class="types">
          {#each element.types || [] as t}
            <span class="type-chip" title={t}>{shortenIRI(t)}</span>
          {/each}
        </div>
      </div>
      <div class="actions">
        <button on:click={() => (full = !full)} title={$i18nT('viewer.resize')} aria-label={$i18nT('viewer.resize')}>
          {#if full}<Minimize2 size={15} />{:else}<Maximize2 size={15} />{/if}
        </button>
        <button on:click={() => dispatch('close')} title={$i18nT('viewer.close')} aria-label={$i18nT('viewer.close')}>
          <X size={15} />
        </button>
      </div>
    </header>

    <nav class="tabs">
      <button class:active={tab === 'properties'} on:click={() => (tab = 'properties')}>
        {$i18nT('viewer.properties')}
      </button>
      {#if hasStructure}
        <button class:active={tab === 'structure'} on:click={() => (tab = 'structure')}>
          {$i18nT('viewer.structure')}
          {#if children.length}<span class="count">{children.length}</span>{/if}
        </button>
      {/if}
      {#if modelRef}
        <button class:active={tab === '3d'} on:click={() => (tab = '3d')}>
          {$i18nT('viewer.model3d')}
        </button>
      {/if}
      <span class="spacer"></span>
      {#if mapTargetId}
        <!-- span wrapper: `.tabs > button` styles direct children as tabs -->
        <span class="nav-action">
          <button class="btn btn-sm btn-ghost" on:click={() => dispatch('showonmap', { id: mapTargetId })}>
            <MapPin size={13} /> {$i18nT('viewer.showOnMap')}
          </button>
        </span>
      {/if}
      <Link to={`/resource?iri=${encodeURIComponent(element.id)}`} class="btn btn-sm">
        {$i18nT('pages.datasetViewer.openResource')}
      </Link>
    </nav>

    <div class="body">
      {#if tab === 'properties'}
        {#if element.ifc_guid || element.ifc_url || element.gltf_url || (element.files || []).length}
          <section class="bim">
            <h4>{$i18nT('viewer.bimFiles')}</h4>
            {#if element.ifc_guid}
              <div class="bim-row">
                <span class="k">IFC GlobalId</span>
                <code>{element.ifc_guid}</code>
              </div>
            {/if}
            {#each element.files || [] as [format, url]}
              <div class="bim-row">
                <span class="k">{format}</span>
                <a href={safeExternalUrl(url)} target="_blank" rel="noreferrer" title={url}>{shortenIRI(url)}</a>
              </div>
            {/each}
          </section>
        {/if}
        {#if loading}
          <div class="state">
            <span class="spinner" aria-hidden="true"></span>
          </div>
        {:else if error}
          <p class="state error">{error}</p>
        {:else if data}
          <dl class="props">
            {#each data.outgoing || [] as row}
              <div class="prop-row">
                <dt title={row.p?.value}>{shortenIRI(row.p?.value || '')}</dt>
                <dd><RdfTerm term={row.o} /></dd>
              </div>
            {/each}
          </dl>
        {/if}
      {:else if tab === 'structure'}
        <!-- The containment context ("part of") leads, so you see where you are
             before drilling down into the contained parts. -->
        {#if element.parent}
          <button class="crumb" on:click={() => dispatch('navigate', { id: element.parent })}>
            <CornerLeftUp size={14} />
            <span class="crumb-k">{$i18nT('viewer.parent')}</span>
            <span class="crumb-v">{parentEl?.label || shortenIRI(element.parent)}</span>
          </button>
        {/if}
        {#if children.length === 0}
          <p class="state">{$i18nT('viewer.noChildren')}</p>
        {:else}
          <ul class="tree">
            {#each children as child}
              <li>
                <button class="tree-row" on:click={() => dispatch('navigate', { id: child.id })}>
                  <ChevronRight size={13} />
                  <span class="label">{child.label || shortenIRI(child.id)}</span>
                  {#if modelRefOf(child)}<span class="badge"><Boxes size={11} /> 3D</span>{/if}
                  {#if child.wkt4326}<span class="badge geo">geo</span>{/if}
                  {#if childCount.get(child.id)}
                    <span class="sub-count">{childCount.get(child.id)} ▸</span>
                  {/if}
                </button>
              </li>
            {/each}
          </ul>
        {/if}
      {:else if tab === '3d' && modelRef}
        <div class="model-wrap">
          {#if modelOptions.length > 1}
            <div class="fmt-picker" role="group" aria-label={$i18nT('viewer.modelFormat')}>
              {#each modelOptions as opt (opt.format)}
                <button
                  class="fmt-chip"
                  class:active={opt.format === modelRef.format}
                  on:click={() => (chosenFormat = opt.format)}
                >{FORMAT_LABELS[opt.format]}</button>
              {/each}
            </div>
          {/if}
          <Model3D
            refs={[{ id: element.id, label: element.label || '', url: modelRef.url, format: modelRef.format }]}
            height="100%"
          />
          <p class="orbit-hint">{$i18nT('viewer.orbitHint')}</p>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .element-modal {
    position: fixed;
    z-index: 1100;
    left: 50%;
    top: 50%;
    margin-left: min(-360px, calc(-45vw));
    margin-top: -290px;
    width: min(720px, 90vw);
    height: min(580px, 86vh);
    display: flex;
    flex-direction: column;
    background: var(--bg-elevated, #fff);
    border: 1px solid var(--border, #e2e8f0);
    border-radius: var(--radius-lg, 18px);
    box-shadow: var(--shadow-lg, 0 28px 60px rgba(15, 32, 39, 0.18));
    overflow: hidden;
    backdrop-filter: blur(12px);
  }
  @media (prefers-reduced-motion: no-preference) {
    /* `scale`/opacity only — leaves the inline translate (drag offset) intact. */
    .element-modal {
      animation: modal-pop 170ms cubic-bezier(0.2, 0.8, 0.2, 1);
    }
  }
  @keyframes modal-pop {
    from {
      opacity: 0;
      scale: 0.97;
    }
    to {
      opacity: 1;
      scale: 1;
    }
  }
  .element-modal.full {
    inset: 3vh 3vw;
    margin: 0;
    width: auto;
    height: auto;
    transform: none;
  }

  header {
    display: flex;
    align-items: flex-start;
    gap: 0.65rem;
    padding: 14px 14px 10px 16px;
    cursor: move;
    user-select: none;
    background: linear-gradient(180deg, var(--bg-subtle, #f8fbfd), transparent);
  }
  .head-icon {
    flex-shrink: 0;
    width: 30px;
    height: 30px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    margin-top: 1px;
  }
  .head-icon.model {
    background: rgba(232, 89, 12, 0.13);
    color: #e8590c;
  }
  .head-icon.geo {
    background: var(--bg-accent-soft, #e6f7f5);
    color: var(--brand-600, #2f7a8c);
  }
  .head-text {
    min-width: 0;
    flex: 1;
  }
  h3 {
    margin: 0;
    font-size: 1.04rem;
    line-height: 1.25;
    color: var(--ink-900, #0f2027);
    overflow-wrap: anywhere;
  }
  .types {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    margin-top: 6px;
  }
  .type-chip {
    font-size: 0.7rem;
    font-weight: 600;
    padding: 1px 9px;
    border-radius: 99px;
    background: var(--bg-accent-soft, #e6f7f5);
    color: var(--brand-700, #1f5f6d);
  }
  .actions {
    display: flex;
    gap: 2px;
    flex-shrink: 0;
  }
  .actions button {
    border: 0;
    background: transparent;
    padding: 6px;
    border-radius: var(--radius-sm, 8px);
    cursor: pointer;
    color: var(--muted, #64748b);
    display: grid;
    place-items: center;
    transition: background 0.12s, color 0.12s;
  }
  .actions button:hover {
    background: var(--bg-hover, rgba(15, 32, 39, 0.06));
    color: var(--ink-900, #0f2027);
  }

  .tabs {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 0 12px;
    border-bottom: 1px solid var(--line-soft, rgba(15, 32, 39, 0.08));
  }
  .tabs > button {
    border: 0;
    background: transparent;
    padding: 9px 12px;
    font-size: 0.84rem;
    color: var(--muted, #64748b);
    border-bottom: 2px solid transparent;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 6px;
    transition: color 0.12s;
  }
  .tabs > button:hover {
    color: var(--ink-800, #1e293b);
  }
  .tabs > button.active {
    color: var(--brand-600, #2f7a8c);
    border-bottom-color: var(--brand-500, #3a95a6);
    font-weight: 600;
  }
  .count {
    font-size: 0.68rem;
    font-weight: 600;
    background: var(--bg-soft, #f1f5f9);
    border-radius: 99px;
    padding: 0 7px;
    color: var(--ink-700, #334155);
  }
  .spacer {
    flex: 1;
  }
  .nav-action {
    display: inline-flex;
    margin-right: 6px;
  }
  .nav-action button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 14px 16px 16px;
  }

  /* BIM / geometry files card */
  .bim {
    border: 1px solid var(--line-soft, rgba(15, 32, 39, 0.08));
    border-radius: var(--radius-md, 14px);
    padding: 11px 13px;
    margin-bottom: 12px;
    background: var(--bg-subtle, #f8fbfd);
  }
  .bim h4 {
    margin: 0 0 7px;
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted, #64748b);
  }
  .bim-row {
    display: flex;
    gap: 0.7rem;
    font-size: 0.84rem;
    padding: 3px 0;
    align-items: baseline;
  }
  .bim-row .k {
    min-width: 110px;
    color: var(--muted, #64748b);
    font-size: 0.78rem;
  }
  .bim-row code {
    font-size: 0.78rem;
    color: var(--ink-900, #0f2027);
    overflow-wrap: anywhere;
  }

  /* Properties — a definition grid: predicate left, value right. */
  .props {
    margin: 0;
    display: flex;
    flex-direction: column;
  }
  .prop-row {
    display: grid;
    grid-template-columns: minmax(110px, 34%) 1fr;
    gap: 0.5rem 0.9rem;
    padding: 7px 8px;
    border-radius: var(--radius-sm, 8px);
    border-top: 1px solid var(--line-soft, rgba(15, 32, 39, 0.07));
  }
  .prop-row:first-child {
    border-top: 0;
  }
  .prop-row:hover {
    background: var(--bg-hover, rgba(15, 32, 39, 0.035));
  }
  .prop-row dt {
    font-size: 0.78rem;
    font-weight: 600;
    color: var(--muted, #64748b);
    overflow-wrap: anywhere;
  }
  .prop-row dd {
    margin: 0;
    font-size: 0.85rem;
    color: var(--ink-900, #0f2027);
    overflow-wrap: anywhere;
    min-width: 0;
  }

  /* Structure */
  .crumb {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    text-align: left;
    margin-bottom: 10px;
    padding: 8px 11px;
    border: 1px solid var(--line-soft, rgba(15, 32, 39, 0.08));
    border-radius: var(--radius-md, 12px);
    background: var(--bg-subtle, #f8fbfd);
    color: var(--ink-800, #1e293b);
    cursor: pointer;
    transition: border-color 0.12s, background 0.12s;
  }
  .crumb:hover {
    border-color: var(--brand-300, #7ed6d0);
    background: var(--bg-accent-soft, #e6f7f5);
  }
  .crumb :global(svg) {
    color: var(--brand-600, #2f7a8c);
    flex-shrink: 0;
  }
  .crumb-k {
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted, #64748b);
  }
  .crumb-v {
    font-size: 0.84rem;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tree {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .tree-row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 6px;
    text-align: left;
    border: 0;
    background: transparent;
    padding: 8px;
    border-radius: var(--radius-sm, 8px);
    cursor: pointer;
    font-size: 0.87rem;
    color: var(--ink-900, #0f2027);
    transition: background 0.12s;
  }
  .tree-row:hover {
    background: var(--bg-hover, rgba(15, 32, 39, 0.04));
  }
  .tree-row > :global(svg:first-child) {
    color: var(--muted, #94a3b8);
    flex-shrink: 0;
  }
  .tree-row .label {
    flex: 0 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: 0.64rem;
    font-weight: 600;
    padding: 1px 7px;
    border-radius: 99px;
    background: rgba(232, 89, 12, 0.13);
    color: #e8590c;
  }
  .badge.geo {
    background: var(--bg-accent-soft, #e6f7f5);
    color: var(--brand-600, #2f7a8c);
  }
  .sub-count {
    margin-left: auto;
    font-size: 0.72rem;
    color: var(--muted, #64748b);
  }

  /* 3D */
  .model-wrap {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .fmt-picker {
    display: flex;
    gap: 5px;
    flex-wrap: wrap;
  }
  .fmt-chip {
    border: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg, #fff);
    color: var(--muted, #64748b);
    font-size: 0.72rem;
    font-weight: 600;
    padding: 3px 11px;
    border-radius: 999px;
    cursor: pointer;
    transition: color 0.12s, border-color 0.12s, background 0.12s;
  }
  .fmt-chip:hover {
    color: var(--ink-900, #0f2027);
    border-color: var(--brand-300, #7ed6d0);
  }
  .fmt-chip.active {
    background: var(--bg-accent-soft, #e6f7f5);
    border-color: var(--brand-400, #5bb8be);
    color: var(--brand-700, #1f5f6d);
  }
  .model-wrap :global(.model-3d) {
    flex: 1;
  }
  .orbit-hint {
    text-align: center;
    margin: 0;
    font-size: 0.74rem;
    color: var(--muted, #64748b);
  }

  /* Shared empty / loading / error states */
  .state {
    color: var(--muted, #64748b);
    font-size: 0.86rem;
    padding: 6px 2px;
  }
  .state.error {
    color: var(--danger-500, #dc2626);
  }
  .spinner {
    display: block;
    width: 22px;
    height: 22px;
    margin: 14px auto;
    border-radius: 50%;
    border: 2.5px solid var(--line-soft, rgba(15, 32, 39, 0.12));
    border-top-color: var(--brand-500, #3a95a6);
    animation: spin 0.7s linear infinite;
  }
  @media (prefers-reduced-motion: reduce) {
    .spinner {
      animation-duration: 1.8s;
    }
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
