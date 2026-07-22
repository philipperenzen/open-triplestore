<script context="module">
  // Per-tab reading state (which accordions are open, which sub-trees are
  // expanded). It lives outside the component because minimising a window
  // unmounts its body — the user's layout must survive a restore.
  const VIEW_MAX = 120;
  const VIEW = new Map(); // tab key → { sections, expanded, format, requested }
</script>

<script>
  // Element inspector window for the dataset explorer. Draggable by its header,
  // minimisable to the dock, expandable to fullscreen, and — since links inside
  // a window open as TABS rather than as new windows — a tab group of subjects.
  // Each tab shows:
  //   Properties — the subject's RDF (browse API + RdfTerm) and BIM/IFC facts
  //   Structure  — the BOT/IFC decomposition tree (sub-elements), each row
  //                navigable so every substructure can be inspected/visualised
  //   3D         — interactive model viewer (orbit: rotate / pan / zoom)
  import { createEventDispatcher, onDestroy, setContext, tick } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { X, Maximize2, Minimize2, Minus, Boxes, ChevronRight, MapPin } from 'lucide-svelte';
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import { safeExternalUrl } from '../../lib/safeUrl';
  import { Link } from '../../lib/router/index.js';
  import { modelRefOf, modelRefsOf, FORMAT_LABELS } from '../../lib/viewer/detect';
  import { resourceCache } from '../../lib/viewer/resourceCache';
  import { OPEN_RESOURCE_CONTEXT } from '../../lib/viewer/windows';
  import RdfTerm from '../RdfTerm.svelte';
  import Model3D from './Model3D.svelte';
  import InspectorTabs from './InspectorTabs.svelte';

  /** The subject of the ACTIVE tab (viewer-feed shape; synthetic for a plain
   *  resource that isn't part of the dataset's element feed). */
  export let element = null;
  /** All feed elements — used to derive the substructure tree. */
  export let elements = [];
  export let datasetId = '';
  /** Window identity + tab group, owned by the parent (see lib/viewer/windows). */
  export let wid = '';
  export let tabs = [];
  export let activeKey = '';
  /** The other open windows, for the tab menu's "move to window" fallback. */
  export let targets = [];
  /** Drag offset from the CSS-centred origin; the parent stores it so it
   *  survives a minimise/restore cycle. */
  export let pos = { x: 0, y: 0 };
  /** Fullscreen state — also parent-owned for the same reason. */
  export let full = false;
  /** Stacking order — the parent bumps this to bring a window to the front. */
  export let z = 1100;
  /** Info-only mode: the 3D model isn't mounted (the parent caps how many heavy
   *  3D viewers run at once); the 3D section auto-requests a slot instead. */
  export let lite = false;
  /** Whether the hosting page shows a map — enables the "Show on map" action. */
  export let hasMap = false;

  const dispatch = createEventDispatcher();

  // An RDF link inside this window becomes a TAB of this window instead of a
  // full-page navigation. RdfTerm picks this up through the context ONLY when an
  // ancestor provided it, so every other surface keeps routing to /resource.
  setContext(OPEN_RESOURCE_CONTEXT, (req) => {
    const iri = req?.iri;
    if (!iri) return false;
    dispatch('opentab', { kind: 'resource', id: iri, label: shortenIRI(iri) });
    return true;
  });

  let tabsComponent;
  /** Move DOM focus to the active tab — used after a restore from the dock. */
  export function focusActive() {
    tabsComponent?.focusActive();
  }

  // Collapsible sections (accordion) — MULTIPLE can be open at once, so you can
  // read Properties, the Structure tree and the 3D model together. Persisted per
  // tab so switching back restores the reader's layout.
  let openSections = new Set(['properties']);
  let structExpanded = new Set();
  let chosenFormat = null;
  let loadRequested = false;
  let viewKey = '';

  function toggleSection(key) {
    const next = new Set(openSections);
    next.has(key) ? next.delete(key) : next.add(key);
    openSections = next;
  }
  function toggleStruct(id) {
    const next = new Set(structExpanded);
    next.has(id) ? next.delete(id) : next.add(id);
    structExpanded = next;
  }

  function stashView() {
    if (!viewKey) return;
    VIEW.delete(viewKey);
    VIEW.set(viewKey, {
      sections: [...openSections],
      expanded: [...structExpanded],
      format: chosenFormat,
      requested: loadRequested,
    });
    while (VIEW.size > VIEW_MAX) VIEW.delete(VIEW.keys().next().value);
  }
  function switchView(key) {
    stashView();
    viewKey = key;
    const v = VIEW.get(key);
    openSections = new Set(v?.sections ?? ['properties']);
    structExpanded = new Set(v?.expanded ?? []);
    chosenFormat = v?.format ?? null;
    loadRequested = v?.requested ?? false;
  }
  $: if (activeKey && activeKey !== viewKey) switchView(activeKey);
  onDestroy(() => {
    stashView();
    // A window that is minimised or closed stops waiting on its read; the shared
    // cache only cancels the request when no other window still wants it.
    inflightAbort?.abort();
  });

  let dragPos = null; // local override while dragging (avoids a state round-trip per frame)
  let dragging = null;
  let data = null;
  let loading = false;
  let error = '';
  let reqSeq = 0;
  let loadedKey = '';

  $: shownPos = dragPos ?? pos;
  // Release the local override once the parent's stored position matches.
  $: if (dragPos && pos.x === dragPos.x && pos.y === dragPos.y) dragPos = null;
  $: activeIndex = Math.max(0, tabs.findIndex((t) => t.key === activeKey));
  $: activeTab = tabs.find((t) => t.key === activeKey) ?? null;

  $: children = element ? elements.filter((e) => e.parent === element.id) : [];
  // The Structure section is only useful when there's containment to show — a
  // parent ("part of") or sub-elements. Hidden for a subject with neither
  // (which is also the case for an off-dataset resource tab).
  $: hasStructure = children.length > 0 || !!element?.parent;
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
  // IFC …). The user can switch between them in the 3D section; the preferred
  // format is the default.
  $: modelOptions = element ? modelRefsOf(element) : [];
  $: modelRef = modelOptions.find((o) => o.format === chosenFormat) ?? modelOptions[0] ?? null;
  // A "lite" window (3D viewer capped) auto-loads its model in the background the
  // moment the user opens its 3D section — no manual "Load" button to click.
  // The request is one-shot on purpose: another window can later take the slot
  // back, and an automatic re-request would make the two windows fight over it
  // forever. When that happens the section offers an explicit "load" instead.
  let modelPending = false;
  // Holding a slot counts as "already asked", however the slot was obtained: a
  // window granted one at open time never went through the branch below, so
  // without this an eviction (a third window claiming the budget) would make it
  // silently re-request — the two evicted windows then leap-frogged in front of
  // the one the user was reading, each remounting its viewer on the way.
  $: if (!lite) loadRequested = true;
  $: if (openSections.has('3d') && lite && modelRef && !loadRequested) {
    loadRequested = true;
    modelPending = true;
    dispatch('loadmodel');
    tick().then(() => (modelPending = false));
  }
  $: modelRevoked = lite && loadRequested && !modelPending;
  // Passed straight to <Model3D refs={…}>: the array literal is re-derived on
  // every reactive pass, but Model3D compares the CONTENT (lib/viewer/
  // refsSignature.ts), so a re-render or a pointerdown no longer tears the scene
  // down and re-frames the camera. Nothing to memoise here.
  $: model3dRefs = modelRef
    ? [
        {
          id: element.id,
          label: element.label || '',
          url: modelRef.url,
          format: modelRef.format,
          upAxis: modelRef.upAxis,
          guids: modelRef.format === 'ifc' ? descendantGuids : undefined,
        },
      ]
    : [];

  // ── Inline structure tree ───────────────────────────────────────────────────
  // The Structure section shows the element's DIRECT children expanded (n+1);
  // each child with its own children gets a caret that expands them INLINE
  // (n+2…). Clicking a child's name opens it as a tab of THIS window.
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

  // In-window navigation. Shift-click (or the tab menu) is the escape hatch that
  // opens a subject in a window of its own instead.
  function openSubject(e, id, label) {
    const detail = { kind: 'element', id, label };
    dispatch(e?.shiftKey ? 'navigate' : 'opentab', detail);
  }

  // The fetch is keyed on the TAB, not on the element object: a re-render (for
  // instance the pointerdown that raises this window) must never re-issue it.
  // The shared resourceCache does the rest — a tab you already read resolves
  // from cache within the same task (so the table never blanks), two windows on
  // the same subject share ONE request, and abort + the sequence token together
  // guarantee a superseded answer can't land on the visible tab.
  let inflightAbort = null;
  async function load(iri, scope) {
    if (!iri) return;
    error = '';
    inflightAbort?.abort();
    const controller = new AbortController();
    inflightAbort = controller;
    const seq = ++reqSeq;
    data = null;
    loading = true;
    try {
      const res = await resourceCache.get(iri, scope, { signal: controller.signal });
      if (seq !== reqSeq) return; // superseded by a newer tab — never overwrite
      data = res;
    } catch (e) {
      if (seq !== reqSeq || e?.name === 'AbortError') return;
      error = e?.message || 'failed';
    } finally {
      if (seq === reqSeq) loading = false;
    }
  }
  // Scope follows the TAB KIND. An element tab is by definition part of this
  // dataset, so scoping the lookup to it is both correct and cheaper. A resource
  // tab is an arbitrary IRI reached from a link — overwhelmingly a vocabulary
  // term (rdf:type objects, geo:/dcat:/owl: predicates) that lives in the shared
  // vocabulary graphs, NOT in the dataset. Forcing the dataset scope on those
  // returned an empty answer, which is exactly the "the link does nothing"
  // complaint; the broad accessible set is what /resource itself resolves against.
  $: if (activeKey && element?.id && activeKey !== loadedKey) {
    loadedKey = activeKey;
    load(element.id, activeTab?.kind === 'resource' ? {} : { dataset_id: datasetId });
  }

  // Both halves of the browse payload are shown: `outgoing` (this subject's own
  // statements) and `incoming` (what links here). The incoming list is capped —
  // see the template — because the endpoint returns it unbounded.
  const INCOMING_SHOWN = 50;
  $: outgoingRows = data?.outgoing || [];
  $: incomingRows = (data?.incoming || []).slice(0, INCOMING_SHOWN);
  $: incomingMore = Math.max(0, (data?.incoming || []).length - incomingRows.length);

  function startDrag(e) {
    if (full || e.target.closest('button, a, [role="tab"]')) return;
    dragging = { x: e.clientX - shownPos.x, y: e.clientY - shownPos.y };
    dragPos = shownPos;
    window.addEventListener('pointermove', onDrag);
    window.addEventListener('pointerup', stopDrag, { once: true });
  }
  function onDrag(e) {
    if (!dragging) return;
    dragPos = { x: e.clientX - dragging.x, y: e.clientY - dragging.y };
  }
  function stopDrag() {
    dragging = null;
    window.removeEventListener('pointermove', onDrag);
    if (dragPos) dispatch('move', { ...dragPos }); // hand the final position to the parent
  }

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

{#if element}
  <!-- `focusin` raises the window alongside the pointer path: windows render in
       insertion order but stack by rank, so tabbing into a window that sits
       behind another one would otherwise put focus on controls the user cannot
       see (WCAG 2.2 SC 2.4.11). focusWindow() no-ops for the top window, so the
       extra handler costs nothing. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="element-modal"
    class:full
    style:transform={full ? '' : `translate(${shownPos.x}px, ${shownPos.y}px)`}
    style:z-index={z}
    on:pointerdown|capture={() => dispatch('focus')}
    on:focusin={() => dispatch('focus')}
    role="dialog"
    aria-modal="false"
    tabindex="-1"
    aria-label={element.label || element.id}
  >
    <!-- Drag handle: pointer-only affordance; all controls inside stay
         keyboard-accessible and Escape closes the window. -->
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
        <button
          on:click={() => dispatch('minimize')}
          title={$i18nT('viewer.minimizeWindow')}
          aria-label={$i18nT('viewer.minimizeWindow')}
        >
          <Minus size={15} />
        </button>
        <button on:click={() => dispatch('togglefull')} title={$i18nT('viewer.resize')} aria-label={$i18nT('viewer.resize')}>
          {#if full}<Minimize2 size={15} />{:else}<Maximize2 size={15} />{/if}
        </button>
        <button on:click={() => dispatch('close')} title={$i18nT('viewer.closeWindow')} aria-label={$i18nT('viewer.closeWindow')}>
          <X size={15} />
        </button>
      </div>
    </header>

    <!-- Tab strip — always rendered, even for a single tab: it is also the drop
         surface a tab dragged from another window lands on. -->
    <InspectorTabs
      bind:this={tabsComponent}
      {wid}
      {tabs}
      {activeKey}
      {targets}
      on:select={(e) => dispatch('tabselect', e.detail)}
      on:close={(e) => dispatch('tabclose', e.detail)}
      on:move={(e) => dispatch('tabmove', e.detail)}
      on:detach={(e) => dispatch('tabdetach', e.detail)}
    />

    <!-- Whole-element actions, always visible above the collapsible sections. -->
    <div class="modal-toolbar">
      {#if mapTargetId}
        <button class="btn btn-sm" on:click={() => dispatch('showonmap', { id: element.id })}>
          <MapPin size={13} /> {$i18nT('viewer.showOnMap')}
        </button>
      {/if}
      <!-- The deliberate "leave the viewer" escape hatch: everything else in
           this window now stays in this window. -->
      <Link to={`/resource?iri=${encodeURIComponent(element.id)}`} class="btn btn-sm">
        {$i18nT('pages.datasetViewer.openResource')}
      </Link>
    </div>

    <!-- Collapsible sections — multiple can be open at once. -->
    <div
      class="body sections"
      id={`tabpanel-${wid}`}
      role="tabpanel"
      aria-labelledby={`tab-${wid}-${activeIndex}`}
    >
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
            <!-- Keep the rows mounted while a revalidation is in flight: the old
                 "blank then refetch" pass destroyed the very link the user was
                 pressing, which is why in-modal links appeared dead. -->
            {#if loading && !data}
              <p class="hint">…</p>
            {:else if error}
              <p class="hint error">{error}</p>
            {:else if data}
              {#if outgoingRows.length}
                <table class="props">
                  <tbody>
                    {#each outgoingRows as row}
                      <tr>
                        <td class="pred" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</td>
                        <td><RdfTerm term={row.o} /></td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              {/if}
              <!-- "Linked from" — the other half of the browse payload. Without
                   it a term whose only statements point AT it (a class, a
                   vocabulary property) rendered as an empty table, which read
                   as a broken link. -->
              {#if incomingRows.length}
                <h4 class="sub-head">{$i18nT('viewer.linkedFrom')}</h4>
                <table class="props">
                  <tbody>
                    {#each incomingRows as row}
                      <tr>
                        <td class="pred" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</td>
                        <td><RdfTerm term={row.s} /></td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
                <!-- The endpoint does not cap `incoming`, and a hub term (a
                     class every element is typed with) can carry thousands of
                     them — far too many for a panel this size. -->
                {#if incomingMore > 0}
                  <p class="hint">{$i18nT('viewer.moreLinks', { values: { count: incomingMore } })}</p>
                {/if}
              {/if}
              {#if !outgoingRows.length && !incomingRows.length}
                <p class="hint">{$i18nT('viewer.noStatements')}</p>
              {/if}
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
                <button
                  class="tree-row parent"
                  title={$i18nT('pages.datasetViewer.openInNewWindow')}
                  on:click={(e) =>
                    openSubject(
                      e,
                      element.parent,
                      elements.find((x) => x.id === element.parent)?.label || shortenIRI(element.parent)
                    )}
                >
                  ↑ {$i18nT('viewer.parent')}:
                  {elements.find((e) => e.id === element.parent)?.label || shortenIRI(element.parent)}
                </button>
              {/if}
              {#if structRows.length === 0}
                <p class="hint">{$i18nT('viewer.noChildren')}</p>
              {:else}
                <!-- Inline tree: the caret expands a child's own children HERE;
                     the name opens that child as a tab of this window. -->
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
                      <button
                        class="struct-main"
                        on:click={(e) => openSubject(e, r.el.id, r.el.label || shortenIRI(r.el.id))}
                        title={r.el.id}
                      >
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
              {#if modelRevoked}
                <!-- The 3D budget went to another window while this one waited;
                     asking again is a deliberate click, never automatic. -->
                <div class="model-locked">
                  <p class="hint center">{$i18nT('viewer.modelLimited')}</p>
                  <button class="btn btn-sm" on:click={() => dispatch('loadmodel')}>
                    <Boxes size={13} /> {$i18nT('viewer.load3d')}
                  </button>
                </div>
              {:else if lite}
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
                    refs={model3dRefs}
                    on:select={(e) => {
                      // Picking an IFC mesh selects that atom (beam, slab, …):
                      // resolve its GlobalId to the feed element and open it as
                      // a tab here.
                      const guid = e.detail?.guid;
                      const hit = guid && elements.find((el) => el.ifc_guid === guid);
                      if (hit && hit.id !== element.id) {
                        dispatch('opentab', {
                          kind: 'element',
                          id: hit.id,
                          label: hit.label || shortenIRI(hit.id),
                        });
                      }
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
  /* "Linked from" heading between the two statement tables. */
  .sub-head {
    margin: 14px 0 2px;
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
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
  /* Inline structure tree: a caret (expand here) + a name (open as a tab),
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
