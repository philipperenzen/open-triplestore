<script>
  // Element inspector modal for the dataset explorer. Draggable by its header,
  // expandable to fullscreen. Three tabs:
  //   Properties — the element's RDF (browse API + RdfTerm) and BIM/IFC facts
  //   Structure  — the BOT/IFC decomposition tree (sub-elements), each row
  //                navigable so every substructure can be inspected/visualised
  //   3D         — interactive model viewer (orbit: rotate / pan / zoom)
  import { createEventDispatcher } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { X, Maximize2, Minimize2, Boxes, ChevronRight, MapPin, Footprints } from 'lucide-svelte';
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

  // Collapsible sections (accordion) — MULTIPLE can be open at once, so you can
  // read Properties, the Structure tree and the 3D model together. Persisted
  // across element navigation so the user's chosen layout sticks.
  let openSections = new Set(['properties']);
  function toggleSection(key) {
    const next = new Set(openSections);
    next.has(key) ? next.delete(key) : next.add(key);
    openSections = next;
  }
  let full = false;
  let pos = { x: offset * 30, y: offset * 30 };
  let dragging = null;
  let data = null;
  let loading = false;
  let error = '';

  $: children = element ? elements.filter((e) => e.parent === element.id) : [];
  // The Structure tab is only useful when there's containment to show — a parent
  // ("part of") or sub-elements. Hide it for a standalone element with neither.
  $: hasStructure = (children.length > 0 || !!element?.parent);
  // GlobalIds of all descendant leaf elements (BFS over the BOT parent links), so
  // a spatial container (storey / building / space) — which owns no geometry of
  // its own — can isolate its whole subtree in the IFC loader instead of falling
  // back to the entire building. Empty for a leaf element (it has no children),
  // so leaf picks keep isolating their single atom via the URL #GlobalId.
  $: descendantGuids = (() => {
    if (!element) return [];
    const out = [];
    const seen = new Set([element.id]);
    const stack = [element.id];
    while (stack.length) {
      const id = stack.pop();
      for (const e of elements) {
        if (e.parent === id && !seen.has(e.id)) {
          seen.add(e.id);
          if (e.ifc_guid) out.push(e.ifc_guid);
          stack.push(e.id);
        }
      }
    }
    return out;
  })();
  // All linked 3D representations of this element (glTF / CityJSON / STL /
  // IFC …). The user can switch between them in the 3D tab; the preferred
  // format is the default.
  $: modelOptions = element ? modelRefsOf(element) : [];
  let chosenFormat = null;
  // Reset per-element view state on an element switch.
  let loadRequested = false;
  let structExpanded = new Set();
  $: if (element?.id) {
    chosenFormat = null;
    loadRequested = false;
    structExpanded = new Set();
  }
  $: modelRef = modelOptions.find((o) => o.format === chosenFormat) ?? modelOptions[0] ?? null;
  // A first-person walkthrough is offered for an IFC *container* (Site / Building
  // / Storey — it has contained elements): walking through a single leaf wall is
  // pointless, so a bare element with no substructure doesn't get the action.
  $: canWalk = children.length > 0 && modelOptions.some((o) => o.format === 'ifc');
  // A "lite" panel (3D viewer capped) auto-loads its model in the background the
  // moment the user opens its 3D section — no manual "Load" button to click.
  $: if (openSections.has('3d') && lite && modelRef && !loadRequested) {
    loadRequested = true;
    dispatch('loadmodel');
  }

  // ── Inline structure tree ───────────────────────────────────────────────────
  // The Structure tab shows the element's DIRECT children expanded (n+1); each
  // child with its own children gets a caret that expands them INLINE (n+2…)
  // rather than opening a new window. Clicking a child's name still opens it.
  $: byIdMap = new Map(elements.map((e) => [e.id, e]));
  $: childIdsMap = (() => {
    const m = new Map();
    for (const e of elements) {
      if (!e.parent) continue;
      const arr = m.get(e.parent);
      if (arr) arr.push(e.id);
      else m.set(e.parent, [e.id]);
    }
    const lbl = (id) => byIdMap.get(id)?.label || id;
    for (const ids of m.values()) ids.sort((a, b) => lbl(a).localeCompare(lbl(b)));
    return m;
  })();
  // Flatten the focused element's descendants to the rows currently visible.
  $: structRows = (() => {
    if (!element) return [];
    const rows = [];
    const walk = (pid, depth) => {
      for (const cid of childIdsMap.get(pid) || []) {
        const el = byIdMap.get(cid);
        if (!el) continue;
        const count = (childIdsMap.get(cid) || []).length;
        const open = structExpanded.has(cid);
        rows.push({ el, depth, count, open });
        if (count && open) walk(cid, depth + 1);
      }
    };
    walk(element.id, 0);
    return rows;
  })();
  function toggleStruct(id) {
    const next = new Set(structExpanded);
    next.has(id) ? next.delete(id) : next.add(id);
    structExpanded = next;
  }

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

    <!-- Whole-element actions, always visible above the collapsible sections. -->
    <div class="modal-toolbar">
      {#if mapTargetId}
        <button class="btn btn-sm" on:click={() => dispatch('showonmap', { id: element.id })}>
          <MapPin size={13} /> {$i18nT('viewer.showOnMap')}
        </button>
      {/if}
      {#if canWalk}
        <button class="btn btn-sm walk-btn" on:click={() => dispatch('walkthrough', { id: element.id })}>
          <Footprints size={13} /> {$i18nT('viewer.exploreInside')}
        </button>
      {/if}
      <Link to={`/resource?iri=${encodeURIComponent(element.id)}`} class="btn btn-sm">
        {$i18nT('pages.datasetViewer.openResource')}
      </Link>
    </div>

    <!-- Collapsible sections — multiple can be open at once. -->
    <div class="body sections">
      <section class="acc">
        <button
          class="acc-head"
          class:open={openSections.has('properties')}
          on:click={() => toggleSection('properties')}
          aria-expanded={openSections.has('properties')}
        >
          <ChevronRight class="acc-caret" size={14} />
          <span class="acc-title">{$i18nT('viewer.properties')}</span>
        </button>
        {#if openSections.has('properties')}
          <div class="acc-content">
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
          </div>
        {/if}
      </section>

      {#if hasStructure}
        <section class="acc">
          <button
            class="acc-head"
            class:open={openSections.has('structure')}
            on:click={() => toggleSection('structure')}
            aria-expanded={openSections.has('structure')}
          >
            <ChevronRight class="acc-caret" size={14} />
            <span class="acc-title">{$i18nT('viewer.structure')}</span>
            {#if children.length}<span class="count">{children.length}</span>{/if}
          </button>
          {#if openSections.has('structure')}
            <div class="acc-content">
              <!-- The containment context (what this is "part of") leads, so you
                   see where you are before drilling into the contained parts. -->
              {#if element.parent}
                <button class="tree-row parent" on:click={() => dispatch('navigate', { id: element.parent })}>
                  ↑ {$i18nT('viewer.parent')}:
                  {elements.find((e) => e.id === element.parent)?.label || shortenIRI(element.parent)}
                </button>
              {/if}
              {#if structRows.length === 0}
                <p class="hint">{$i18nT('viewer.noChildren')}</p>
              {:else}
                <!-- Inline tree: the caret expands a child's own children HERE;
                     the name opens that child in its own panel. -->
                <ul class="tree">
                  {#each structRows as r (r.el.id)}
                    <li class="struct-row" style:--d={r.depth}>
                      {#if r.count}
                        <button
                          class="twist"
                          class:open={r.open}
                          on:click={() => toggleStruct(r.el.id)}
                          title={r.open ? $i18nT('viewer.collapse') : $i18nT('viewer.expand')}
                          aria-label={r.open ? $i18nT('viewer.collapse') : $i18nT('viewer.expand')}
                        ><ChevronRight size={13} /></button>
                      {:else}
                        <span class="twist-spacer"></span>
                      {/if}
                      <button class="struct-main" on:click={() => dispatch('navigate', { id: r.el.id })} title={r.el.id}>
                        <span class="label">{r.el.label || shortenIRI(r.el.id)}</span>
                        {#if modelRefOf(r.el)}<span class="badge"><Boxes size={11} /> 3D</span>{/if}
                        {#if r.el.wkt4326}<span class="badge geo">geo</span>{/if}
                        {#if r.count}<span class="sub-count">{r.count}</span>{/if}
                      </button>
                    </li>
                  {/each}
                </ul>
                <p class="hint struct-hint">{$i18nT('viewer.structHint')}</p>
              {/if}
            </div>
          {/if}
        </section>
      {/if}

      {#if modelRef}
        <section class="acc">
          <button
            class="acc-head"
            class:open={openSections.has('3d')}
            on:click={() => toggleSection('3d')}
            aria-expanded={openSections.has('3d')}
          >
            <ChevronRight class="acc-caret" size={14} />
            <span class="acc-title">{$i18nT('viewer.model3d')}</span>
          </button>
          {#if openSections.has('3d')}
            <div class="acc-content model">
              {#if lite}
                <!-- Capped 3D viewer: auto-load in the background (no button). -->
                <div class="model-locked">
                  <span class="m3d-spin"></span>
                  <p class="hint center">{$i18nT('viewer.loadingModels')}</p>
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
                    refs={[{ id: element.id, label: element.label || '', url: modelRef.url, format: modelRef.format, upAxis: modelRef.upAxis, guids: modelRef.format === 'ifc' ? descendantGuids : undefined }]}
                    on:select={(e) => {
                      // Picking an IFC mesh selects that atom (beam, slab, …):
                      // resolve its GlobalId to the feed element and open it.
                      const guid = e.detail?.guid;
                      const hit = guid && elements.find((el) => el.ifc_guid === guid);
                      if (hit && hit.id !== element.id) dispatch('navigate', { id: hit.id });
                    }}
                    height="100%"
                  />
                  <p class="hint center">{$i18nT('viewer.orbitHint')}</p>
                </div>
              {/if}
            </div>
          {/if}
        </section>
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
  .element-modal {
    animation: emFade 180ms var(--ease-out, ease) both;
  }
  /* Opacity-only entrance — must NOT animate transform (the inline drag transform
     owns it). */
  @keyframes emFade {
    from { opacity: 0; }
    to { opacity: 1; }
  }
  @media (prefers-reduced-motion: reduce) {
    .element-modal { animation: none; }
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
    align-items: center;
    justify-content: space-between;
    gap: 0.6rem;
    padding: 10px 12px 10px 12px;
    cursor: grab;
    user-select: none;
    background: var(--bg-subtle, #fafcfe);
    border-bottom: 1px solid var(--line-soft, #eef1f4);
  }
  header:active {
    cursor: grabbing;
  }
  /* Drag-handle affordance (a grip of dots) — there was no visual cue the panel
     was draggable. Hidden in full-screen, where dragging is disabled. */
  header::before {
    content: '';
    flex: none;
    align-self: center;
    width: 9px;
    height: 18px;
    background-image: radial-gradient(currentColor 1px, transparent 1.4px);
    background-size: 4px 4px;
    color: var(--muted, #94a3b8);
    opacity: 0.55;
  }
  .element-modal.full header {
    cursor: default;
  }
  .element-modal.full header::before {
    display: none;
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
  .modal-toolbar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--line-soft, #eef1f4);
  }
  .modal-toolbar :global(.btn) {
    display: inline-flex;
    align-items: center;
    gap: 5px;
  }
  .count {
    font-size: 0.68rem;
    background: var(--bg-soft, #f1f5f9);
    border-radius: 99px;
    padding: 0 7px;
    color: var(--ink-700, #334155);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0;
  }
  /* Accordion: each section toggles independently, so several can be open. */
  .acc + .acc,
  .acc {
    border-top: 1px solid var(--line-soft, #eef1f4);
  }
  .acc:first-child {
    border-top: 0;
  }
  .acc-head {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 7px;
    border: 0;
    background: transparent;
    padding: 9px 14px;
    cursor: pointer;
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--ink-700, #334155);
    text-align: left;
  }
  .acc-head:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.03));
  }
  .acc-head.open {
    color: var(--brand-600, #1d6fb8);
  }
  .acc-head :global(.acc-caret) {
    flex: none;
    color: var(--muted, #94a3b8);
    transition: transform 0.14s ease;
  }
  .acc-head.open :global(.acc-caret) {
    transform: rotate(90deg);
    color: var(--brand-500, #2f88d8);
  }
  .acc-title {
    flex: 1;
  }
  .acc-head:focus-visible {
    outline: none;
    box-shadow: inset 0 0 0 2px var(--brand-400, #5aa9e0);
  }
  @media (prefers-reduced-motion: reduce) {
    .acc-head :global(.acc-caret) {
      transition: none;
    }
  }
  .acc-content {
    padding: 2px 16px 14px;
  }
  /* The 3D section needs an explicit height now that it isn't the sole content. */
  .acc-content.model {
    height: min(340px, 48vh);
    padding: 8px 12px 12px;
  }
  .element-modal.full .acc-content.model {
    height: 62vh;
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
  /* Inline structure tree: a caret (expand here) + a name (open in a panel),
     each its own control so it's clear both are clickable. */
  .struct-row {
    display: flex;
    align-items: center;
    border-radius: var(--radius-sm, 8px);
    padding-left: calc(var(--d, 0) * 14px);
  }
  .struct-row .twist {
    flex: none;
    width: 24px;
    align-self: stretch;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 0;
    background: transparent;
    color: var(--muted, #64748b);
    cursor: pointer;
    padding: 0;
    border-radius: var(--radius-sm, 6px);
  }
  .struct-row .twist:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.07));
    color: var(--ink-900, #0f172a);
  }
  .struct-row .twist :global(svg) {
    transition: transform 0.12s ease;
  }
  .struct-row .twist.open :global(svg) {
    transform: rotate(90deg);
  }
  @media (prefers-reduced-motion: reduce) {
    .struct-row .twist :global(svg) {
      transition: none;
    }
  }
  .twist-spacer {
    flex: none;
    width: 24px;
  }
  .struct-main {
    flex: 1;
    min-width: 0;
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
  .struct-main:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.04));
  }
  .struct-main .label {
    flex: 0 1 auto;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .struct-row .twist:focus-visible,
  .struct-main:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--brand-400, #5aa9e0);
  }
  .struct-hint {
    margin: 10px 4px 0;
    font-size: 0.72rem;
    opacity: 0.85;
  }
  /* Spinner shown while a capped 3D viewer auto-loads its model. */
  .m3d-spin {
    width: 24px;
    height: 24px;
    border: 3px solid color-mix(in srgb, var(--brand-500, #2f88d8) 30%, transparent);
    border-top-color: var(--brand-500, #2f88d8);
    border-radius: 50%;
    animation: m3d-spin 0.8s linear infinite;
  }
  @keyframes m3d-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .m3d-spin {
      animation: none;
    }
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

  /* Visible keyboard focus on every interactive control (the global ring is
     box-shadow only and gets clipped by some of these containers). */
  .actions button:focus-visible,
  .fmt-chip:focus-visible,
  .tree-row:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--brand-400, #5aa9e0);
  }
</style>
