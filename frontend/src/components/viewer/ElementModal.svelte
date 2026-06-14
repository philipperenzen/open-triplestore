<script>
  // Element inspector modal for the dataset explorer. Draggable by its header,
  // expandable to fullscreen. Three tabs:
  //   Properties — the element's RDF (browse API + RdfTerm) and BIM/IFC facts
  //   Structure  — the BOT/IFC decomposition tree (sub-elements), each row
  //                navigable so every substructure can be inspected/visualised
  //   3D         — interactive model viewer (orbit: rotate / pan / zoom)
  import { createEventDispatcher } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { X, Maximize2, Minimize2, Boxes, ChevronRight, MapPin } from 'lucide-svelte';
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
  /** Cascade index so stacked panels don't open exactly on top of each other. */
  export let offset = 0;
  /** Stacking order — the parent bumps this to bring a panel to the front. */
  export let z = 1100;
  /** Info-only mode: the 3D model isn't mounted (the parent caps how many heavy
   *  3D viewers run at once); the 3D tab offers a "load" button instead. */
  export let lite = false;
  /** Whether the hosting page shows a map — enables the "Show on map" action. */
  export let hasMap = false;

  const dispatch = createEventDispatcher();

  let tab = 'properties';
  let full = false;
  let pos = { x: offset * 30, y: offset * 30 };
  let dragging = null;
  let data = null;
  let loading = false;
  let error = '';

  $: children = element ? elements.filter((e) => e.parent === element.id) : [];
  // All linked 3D representations of this element (glTF / CityJSON / STL /
  // IFC …). The user can switch between them in the 3D tab; the preferred
  // format is the default.
  $: modelOptions = element ? modelRefsOf(element) : [];
  let chosenFormat = null;
  $: if (element?.id) chosenFormat = null; // element switch resets the choice
  $: modelRef = modelOptions.find((o) => o.format === chosenFormat) ?? modelOptions[0] ?? null;
  // parent id → number of children, in one pass (the structure tree reads a
  // count per row; filtering elements per row would be O(N²)).
  $: childCount = elements.reduce(
    (m, e) => (e.parent ? m.set(e.parent, (m.get(e.parent) || 0) + 1) : m),
    new Map()
  );

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
  // When the element loses its model, fall back from the 3D tab.
  $: if (tab === '3d' && !modelRef) tab = 'properties';

  // "Show on map" target: this element when it is located, else the nearest
  // located ancestor (a beam flies to its building/site anchor). Null hides
  // the action (no map on the page, or nothing in the chain has geometry).
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
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="element-modal"
    class:full
    style:transform={full ? '' : `translate(${pos.x}px, ${pos.y}px)`}
    style:z-index={z}
    on:pointerdown|capture={() => dispatch('focus')}
    role="dialog"
    tabindex="-1"
    aria-label={element.label || element.id}
  >
    <!-- Drag handle: pointer-only affordance; all controls inside stay
         keyboard-accessible and Escape closes the panel. -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <header on:pointerdown={startDrag}>
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
      <button class:active={tab === 'structure'} on:click={() => (tab = 'structure')}>
        {$i18nT('viewer.structure')}
        {#if children.length}<span class="count">{children.length}</span>{/if}
      </button>
      {#if modelRef}
        <button class:active={tab === '3d'} on:click={() => (tab = '3d')}>
          {$i18nT('viewer.model3d')}
        </button>
      {/if}
      <span class="spacer"></span>
      {#if mapTargetId}
        <!-- span wrapper: `.tabs > button` styles direct children as tabs -->
        <span class="map-action">
          <button class="btn btn-sm" on:click={() => dispatch('showonmap', { id: element.id })}>
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
          <section class="bim card-flat">
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
          <p class="hint">…</p>
        {:else if error}
          <p class="hint error">{error}</p>
        {:else if data}
          <table class="props">
            <tbody>
              {#each data.outgoing || [] as row}
                <tr>
                  <td class="pred" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</td>
                  <td><RdfTerm term={row.o} /></td>
                </tr>
              {/each}
            </tbody>
          </table>
        {/if}
      {:else if tab === 'structure'}
        {#if children.length === 0}
          <p class="hint">{$i18nT('viewer.noChildren')}</p>
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
        {#if element.parent}
          <button class="tree-row parent" on:click={() => dispatch('navigate', { id: element.parent })}>
            ↑ {$i18nT('viewer.parent')}:
            {elements.find((e) => e.id === element.parent)?.label || shortenIRI(element.parent)}
          </button>
        {/if}
      {:else if tab === '3d' && modelRef}
        {#if lite}
          <div class="model-locked">
            <Boxes size={26} />
            <p class="hint center">{$i18nT('viewer.modelLimited')}</p>
            <button class="btn btn-sm" on:click={() => dispatch('loadmodel')}>{$i18nT('viewer.load3d')}</button>
          </div>
        {:else}
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
              refs={[{ id: element.id, label: element.label || '', url: modelRef.url, format: modelRef.format, upAxis: modelRef.upAxis }]}
              on:select={(e) => {
                // Picking an IFC mesh selects that atom (beam, slab, …): resolve
                // its GlobalId to the feed element and open that panel.
                const guid = e.detail?.guid;
                const hit = guid && elements.find((el) => el.ifc_guid === guid);
                if (hit && hit.id !== element.id) dispatch('navigate', { id: hit.id });
              }}
              height="100%"
            />
            <p class="hint center">{$i18nT('viewer.orbitHint')}</p>
          </div>
        {/if}
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
    border-radius: var(--radius-lg, 14px);
    box-shadow: var(--shadow-lg, 0 18px 50px rgba(0, 0, 0, 0.25));
    overflow: hidden;
    backdrop-filter: blur(10px);
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
    justify-content: space-between;
    gap: 0.6rem;
    padding: 12px 12px 8px 16px;
    cursor: move;
    user-select: none;
  }
  .head-text {
    min-width: 0;
  }
  h3 {
    margin: 0;
    font-size: 1.02rem;
    color: var(--ink-900, #0f172a);
    overflow-wrap: anywhere;
  }
  .types {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    margin-top: 4px;
  }
  .type-chip {
    font-size: 0.7rem;
    padding: 1px 8px;
    border-radius: 99px;
    background: var(--bg-accent-soft, #eef4fa);
    color: var(--ink-700, #334155);
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
  .tabs {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 0 12px;
    border-bottom: 1px solid var(--line-soft, #eef1f4);
  }
  .tabs > button {
    border: 0;
    background: transparent;
    padding: 8px 12px;
    font-size: 0.84rem;
    color: var(--muted, #64748b);
    border-bottom: 2px solid transparent;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .tabs > button.active {
    color: var(--brand-600, #1d6fb8);
    border-bottom-color: var(--brand-500, #2f88d8);
    font-weight: 600;
  }
  .count {
    font-size: 0.68rem;
    background: var(--bg-soft, #f1f5f9);
    border-radius: 99px;
    padding: 0 7px;
    color: var(--ink-700, #334155);
  }
  .spacer {
    flex: 1;
  }
  .map-action {
    display: inline-flex;
    margin-right: 6px;
  }
  .map-action button {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 12px 16px;
  }
  .bim {
    border: 1px solid var(--line-soft, #eef1f4);
    border-radius: var(--radius-md, 10px);
    padding: 10px 12px;
    margin-bottom: 10px;
    background: var(--bg-subtle, #fafcfe);
  }
  .bim h4 {
    margin: 0 0 6px;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted, #64748b);
  }
  .bim-row {
    display: flex;
    gap: 0.7rem;
    font-size: 0.84rem;
    padding: 2px 0;
    align-items: baseline;
  }
  .bim-row .k {
    min-width: 110px;
    color: var(--muted, #64748b);
    font-size: 0.78rem;
  }
  .bim-row code {
    font-size: 0.78rem;
    color: var(--ink-900, #0f172a);
  }
  .props {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }
  .props td {
    padding: 4px 6px;
    vertical-align: top;
    border-top: 1px solid var(--line-soft, #eef1f4);
  }
  .props .pred {
    white-space: nowrap;
    color: var(--muted, #64748b);
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
    padding: 7px 8px;
    border-radius: var(--radius-sm, 8px);
    cursor: pointer;
    font-size: 0.87rem;
    color: var(--ink-900, #0f172a);
  }
  .tree-row:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.04));
  }
  .tree-row .label {
    flex: 0 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tree-row.parent {
    margin-top: 8px;
    color: var(--muted, #64748b);
    font-size: 0.8rem;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: 0.64rem;
    padding: 1px 7px;
    border-radius: 99px;
    background: rgba(232, 89, 12, 0.13);
    color: #e8590c;
  }
  .badge.geo {
    background: rgba(59, 130, 196, 0.14);
    color: var(--brand-600, #1d6fb8);
  }
  .sub-count {
    margin-left: auto;
    font-size: 0.72rem;
    color: var(--muted, #64748b);
  }
  .model-wrap {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .fmt-picker {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .fmt-chip {
    border: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg, #fff);
    color: var(--muted, #64748b);
    font-size: 0.72rem;
    font-weight: 600;
    padding: 2px 10px;
    border-radius: 999px;
    cursor: pointer;
  }
  .fmt-chip:hover {
    color: var(--ink-900, #0f172a);
  }
  .fmt-chip.active {
    background: var(--bg-accent-soft, #e7f0fb);
    border-color: var(--brand-500, #2f88d8);
    color: var(--brand-600, #1d6fb8);
  }
  .model-wrap :global(.model-3d) {
    flex: 1;
  }
  .model-locked {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.6rem;
    color: var(--muted, #64748b);
    text-align: center;
  }
  .hint {
    color: var(--muted, #64748b);
    font-size: 0.86rem;
  }
  .hint.center {
    text-align: center;
    margin: 0;
    font-size: 0.74rem;
  }
  .hint.error {
    color: var(--danger-500, #c0392b);
  }
</style>
