<script>
  // Dataset geo data explorer. The map is the canvas: zoomed out every located
  // element is a dot; zooming in, elements with a 3D model become to-scale
  // model markers. Clicking a feature (or a list row) opens the element modal —
  // properties, BOT/IFC substructure (all navigable) and an interactive 3D
  // viewer. Datasets without any located element fall back to a pure 3D
  // explorer over their models. Light/dark follows the app theme.
  import { onDestroy, tick } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { Link } from '../lib/router/index.js';
  import { getViewerFeed, listDatasetGraphs } from '../lib/api.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { copyToClipboard } from '../lib/clipboard.ts';
  import { ChevronLeft, ChevronRight, Search, Boxes, MapPin, X, Download, FileDown, Footprints, Code2, Check } from 'lucide-svelte';
  import { modelRefOf, modelRefsOf } from '../lib/viewer/detect';
  import { modelRefs } from '../lib/viewer/geometry';
  import { preview } from '../lib/viewer/preview';
  import { resourceCache } from '../lib/viewer/resourceCache';
  import { Z_INSPECTOR_BASE, reportInspectorTopZ } from '../lib/viewer/zLayers';
  import {
    activeTabOf,
    closeAll as closeAllWindows,
    closeTab,
    closeWindow,
    createState,
    detachTabToNewWindow,
    findTab,
    focusWindow,
    minimizeWindow,
    moveTabToWindow,
    moveWindow,
    openInNewWindow,
    openTabInWindow,
    requestModel,
    restoreWindow,
    setActiveTab,
    tabKey,
    toggleFull,
  } from '../lib/viewer/windows';
  import ViewerMap from '../components/viewer/ViewerMap.svelte';
  import Model3D from '../components/viewer/Model3D.svelte';
  import CesiumViewer from '../components/viewer/CesiumViewer.svelte';
  import ElementModal from '../components/viewer/ElementModal.svelte';
  import Walkthrough from '../components/viewer/Walkthrough.svelte';

  export let id = '';

  let elements = [];
  let graphs = [];
  let loading = true;
  let error = '';
  let selected = '';
  let query = '';
  let mapComponent;
  let dlOpen = false;
  // Central canvas render mode. 'map' = MapLibre + three.js (the full-feature
  // explorer: located dots/lines/areas, to-scale glTF/IFC/CityJSON/STL models,
  // and click → the ElementModal inspector with the BOT/IFC decomposition tree).
  // 'cesium' = the CesiumJS 3D-Tiles globe view (a toggle, not a separate page,
  // so it sacrifices none of those features — a pick still opens the inspector).
  let canvasMode = 'map';

  // Download sources: the original IFC file (fragment-less) and every dataset
  // graph in the user's preferred RDF serialization.
  const LD_FORMATS = [
    { key: 'turtle', label: 'Turtle' },
    { key: 'jsonld', label: 'JSON-LD' },
    { key: 'rdfxml', label: 'RDF/XML' },
    { key: 'ntriples', label: 'N-Triples' },
  ];
  $: ifcUrl = (elements.find((e) => e.ifc_url)?.ifc_url || '').split('#')[0];
  $: mapAttribution = elements.some((e) =>
    (e.files || []).some(([, url]) => /3dbag/i.test(url || ''))
  )
    ? '© 3DBAG by tudelft3d and 3DGI (CC BY 4.0)'
    : '';

  // Several movable inspector WINDOWS can be open at once, each holding a group
  // of tabs: a pick in this list or on the map opens a new window, while a link
  // followed INSIDE a window opens a tab of that window (see lib/viewer/windows
  // for the whole state machine — caps, stacking, eviction, the 3D budget).
  // Only MODEL_CAP live windows mount the heavy 3D viewer; the rest open
  // "info-only" and claim a slot on demand. Minimised windows unmount their body
  // (freeing the WebGL context, and no longer counting against the cap) and
  // collapse into the dock at the bottom, which restores them again.
  let wstate = createState();
  $: windows = wstate.windows;
  /** Inspector components by window id, so a restore can return focus to it. */
  let modalRefs = {};

  // Rendered stacking. `w.z` from the state module is a monotonic ORDER counter,
  // not a CSS value: every focus bumps it, so a long session would walk it past
  // the preview overlay (1200), then the dock (1300) and finally the walkthrough
  // (1400), burying each in turn. Rank the windows instead, so the band they
  // actually render in is bounded by MAX_WINDOWS — see lib/viewer/zLayers.ts for
  // the invariant (windows < preview < dock < walkthrough).
  $: zByWid = (() => {
    const m = new Map();
    [...windows]
      .sort((a, b) => a.z - b.z)
      .forEach((w, i) => m.set(w.wid, Z_INSPECTOR_BASE + i));
    return m;
  })();
  // Tell the preview overlay where the top of that band currently is, so it can
  // sit just above it instead of relying on a hard-coded literal.
  $: reportInspectorTopZ(windows.length ? Z_INSPECTOR_BASE + windows.length - 1 : 0);

  // Labels resolve against the live feed first, so a window opened during the
  // fast (located-only) phase picks up its real label once the full feed lands.
  //
  // These two are reactive ASSIGNMENTS, not plain declarations, on purpose: a
  // function declaration carries no dirty bit, so `element={resolveTab(w)}` in
  // the template only ever re-ran when `windows` changed — never when the feed
  // did, which is exactly the upgrade they exist for.
  $: windowLabel = (w) => {
    const tab = activeTabOf(w);
    if (!tab) return '';
    return byId.get(tab.id)?.label || tab.label || shortenIRI(tab.id);
  };
  // Feed elements resolved by IRI keep their structure tree and 3D section, so a
  // plain RDF link that happens to point at a dataset element opens as a full
  // element tab; anything else opens as a resource tab (properties only).
  function tabFor({ kind, id, label }) {
    const el = byId.get(id);
    if (el) {
      return { key: tabKey('element', el.id), kind: 'element', id: el.id, label: el.label || shortenIRI(el.id) };
    }
    const k = kind === 'element' ? 'element' : 'resource';
    return { key: tabKey(k, id), kind: k, id, label: label || shortenIRI(id) };
  }
  /** The subject a window's active tab shows, for the inspector's `element` prop. */
  const syntheticEls = new Map(); // stable identities so the child doesn't re-derive
  $: resolveTab = (w) => {
    const tab = activeTabOf(w);
    if (!tab) return null;
    const el = byId.get(tab.id);
    if (el) return el;
    let synth = syntheticEls.get(tab.key);
    if (!synth || synth.label !== tab.label) {
      synth = { id: tab.id, label: tab.label, types: [] };
      syntheticEls.set(tab.key, synth);
    }
    return synth;
  };
  // One derived view per window, so the subject and the label are re-resolved
  // whenever EITHER the windows or the feed change.
  $: windowViews = windows.map((w) => ({ w, element: resolveTab(w), label: windowLabel(w) }));

  $: filtered = query
    ? elements.filter((e) =>
        (e.label || e.id).toLowerCase().includes(query.toLowerCase())
      )
    : elements;
  $: located = filtered.filter((e) => e.wkt4326);
  $: unlocated = filtered.filter((e) => !e.wkt4326);
  $: hasGeo = elements.some((e) => e.wkt4326);
  $: fallbackRefs = modelRefs(elements);
  // 3D-Tiles-able content: any loadable 3D model, or volumetric WKT geometry
  // (POLYHEDRALSURFACE / TIN / SOLID / Z) — gates the Cesium viewer entry point.
  $: has3dContent =
    elements.some((e) => modelRefOf(e)) ||
    elements.some((e) => /POLYHEDRALSURFACE|\bTIN\b|\bSOLID\b| Z[ (]/i.test(e.wkt4326 || ''));

  const hasModel = (el) => !!modelRefOf(el);
  const nodeLabel = (el) => el?.label || shortenIRI(el?.id || '');

  // ── Structural tree ─────────────────────────────────────────────────────────
  // Arrange the side list by the BOT/IFC spatial structure — Site → Building →
  // Storey → Space → Element — so the parts of one building stay together and a
  // 3000-element model is navigable by drilling in. Datasets with no parent
  // links (a flat set of geo features) keep the located / unlocated split, and
  // an active search always shows a flat result list.
  $: byId = new Map(elements.map((e) => [e.id, e]));
  $: childrenOf = (() => {
    const m = new Map();
    for (const e of elements) {
      if (e.parent && byId.has(e.parent)) {
        if (!m.has(e.parent)) m.set(e.parent, []);
        m.get(e.parent).push(e.id);
      }
    }
    const lbl = (cid) => byId.get(cid)?.label || cid;
    for (const kids of m.values()) kids.sort((a, b) => lbl(a).localeCompare(lbl(b)));
    return m;
  })();
  $: roots = elements
    .filter((e) => !e.parent || !byId.has(e.parent))
    .sort((a, b) => nodeLabel(a).localeCompare(nodeLabel(b)));
  $: hasStructure = childrenOf.size > 0;

  let expanded = new Set();
  let treeInit = false;
  // Open the top two structural levels on first load (Site → Building →
  // Storeys), leaving deeper containers collapsed.
  $: if (elements.length && !treeInit && childrenOf) {
    treeInit = true;
    const next = new Set();
    const seed = (eid, depth) => {
      const kids = childrenOf.get(eid) || [];
      if (kids.length && depth < 2) {
        next.add(eid);
        kids.forEach((k) => seed(k, depth + 1));
      }
    };
    roots.forEach((r) => seed(r.id, 0));
    expanded = next;
  }
  function toggle(eid) {
    const next = new Set(expanded);
    next.has(eid) ? next.delete(eid) : next.add(eid);
    expanded = next;
  }
  function expandAll() {
    expanded = new Set(childrenOf.keys());
  }
  function collapseAll() {
    expanded = new Set();
  }
  // Flatten the tree to the rows currently visible (depth for indentation,
  // child count for the disclosure caret).
  $: treeRows = (() => {
    const rows = [];
    const walk = (eid, depth) => {
      const el = byId.get(eid);
      if (!el) return;
      const kids = childrenOf.get(eid) || [];
      rows.push({ el, depth, count: kids.length, open: expanded.has(eid) });
      if (kids.length && expanded.has(eid)) kids.forEach((k) => walk(k, depth + 1));
    };
    roots.forEach((r) => walk(r.id, 0));
    return rows;
  })();

  // Two-phase load so the map paints fast on big BIM datasets: phase 1 fetches
  // only the located elements (the map's render set — seconds, not the whole
  // multi-thousand-element building); phase 2 fetches the full feed (the
  // structure tree + every sub-element) in the background and swaps it in.
  let fullLoaded = false;
  async function load() {
    loading = true;
    error = '';
    // Phase 1 — fast located subset → the map renders almost immediately.
    try {
      const fast = await getViewerFeed(id, null, { located: true });
      elements = fast?.elements || [];
    } catch {
      /* fall through — the full feed below is the source of truth */
    }
    loading = false; // shell + map are interactive now

    // Graph list (download menu) — cheap, fetch alongside without blocking.
    listDatasetGraphs(id)
      .then((g) => (graphs = g || []))
      .catch(() => (graphs = [])); // downloads simply hide when unavailable

    // Phase 2 — the full feed (tree + sub-elements) in the background.
    try {
      const full = await getViewerFeed(id);
      if (full?.elements) {
        treeInit = false; // re-seed the auto-expansion over the full hierarchy
        elements = full.elements;
      }
    } catch (e) {
      if (!elements.length) error = e?.message || 'failed';
    } finally {
      fullLoaded = true;
    }
  }

  // Every window operation goes through apply(): the state module returns the
  // SAME object for a no-op (e.g. focusing the window that is already on top),
  // and skipping the assignment then skips a re-render — which is what used to
  // make a mere pointerdown inside a panel refetch its RDF and rebuild its scene.
  function apply(next) {
    if (next === wstate) return;
    wstate = next;
    syncSelection();
  }
  // The map highlight follows the top-most visible window's active ELEMENT tab;
  // a resource tab (an IRI outside the feed) leaves the current highlight alone,
  // and closing everything clears it.
  function syncSelection() {
    if (!wstate.windows.length) {
      selected = '';
      return;
    }
    const top = wstate.windows.filter((w) => !w.minimized).sort((a, b) => b.z - a.z)[0];
    const tab = activeTabOf(top);
    if (tab?.kind === 'element') selected = tab.id;
  }

  // A pick in the side list / on the map opens a NEW window…
  //
  // It NEVER moves the camera. Selecting used to fly the map in and slam the
  // pitch over, which threw away whatever viewpoint the user had set up — the
  // "camera collapse". Framing is an explicit action now: the modal's "Show on
  // map" button and the map's own "zoom to selection" control.
  function open(elId) {
    selected = elId;
    const el = byId.get(elId);
    if (!el) return;
    const before = wstate.windows;
    apply(openInNewWindow(wstate, tabFor({ kind: 'element', id: elId }), { wantsModel: hasModel(el) }));
    rememberOpener(before);
  }
  // …while a link followed inside a window opens a TAB of that window.
  // Focus follows the new tab: activating a link destroys the very control the
  // user pressed, and without this focus falls back to <body> — the next Tab
  // then restarts at the top of the page and nothing announces the new subject.
  async function openTab(wid, detail) {
    const tab = tabFor(detail);
    apply(openTabInWindow(wstate, wid, tab));
    await tick();
    // The subject may already be open elsewhere — that window is revealed
    // instead of the tab being copied, so focus follows it there.
    const host = findTab(wstate, tab.key);
    if (host) modalRefs[host.wid]?.focusActive();
  }
  // Shift-click escape hatch: the same subject in a window of its own.
  function openWindowFor(detail) {
    const el = byId.get(detail.id);
    const before = wstate.windows;
    apply(openInNewWindow(wstate, tabFor(detail), { wantsModel: !!el && hasModel(el) }));
    rememberOpener(before);
  }

  // The control each window was opened from, so closing it can hand focus back
  // (WCAG 2.4.3) instead of dropping it on <body>.
  const openerByWid = new Map();
  function rememberOpener(before) {
    // Drop openers of windows that are gone (an eviction at the cap never runs
    // closeWin): each entry pins a DOM node, so a long session would retain
    // detached subtrees.
    const live = new Set(wstate.windows.map((w) => w.wid));
    for (const wid of [...openerByWid.keys()]) if (!live.has(wid)) openerByWid.delete(wid);
    const prev = new Set(before.map((w) => w.wid));
    const fresh = wstate.windows.find((w) => !prev.has(w.wid));
    if (fresh) openerByWid.set(fresh.wid, document.activeElement);
  }

  const focusWin = (wid) => apply(focusWindow(wstate, wid));
  async function closeWin(wid) {
    const opener = openerByWid.get(wid);
    openerByWid.delete(wid);
    apply(closeWindow(wstate, wid));
    await tick();
    if (opener?.isConnected) {
      opener.focus();
      return;
    }
    const top = wstate.windows.filter((w) => !w.minimized).sort((a, b) => b.z - a.z)[0];
    if (top) modalRefs[top.wid]?.focusActive();
  }
  const closeAll = () => apply(closeAllWindows(wstate));
  const moveWin = (wid, pos) => apply(moveWindow(wstate, wid, pos));
  const toggleWinFull = (wid) => apply(toggleFull(wstate, wid));
  const minimizeWin = (wid) => apply(minimizeWindow(wstate, wid));
  async function selectTab(wid, key) {
    apply(setActiveTab(wstate, wid, key));
    await tick();
    modalRefs[wid]?.focusActive();
  }
  // Closing a tab destroys its × button; focus moves to the tab that takes its
  // place, or — when that was the window's last tab — back to the opener.
  async function closeTabIn(wid, key) {
    apply(closeTab(wstate, wid, key));
    await tick();
    if (wstate.windows.some((w) => w.wid === wid)) modalRefs[wid]?.focusActive();
    else {
      const opener = openerByWid.get(wid);
      openerByWid.delete(wid);
      if (opener?.isConnected) opener.focus();
    }
  }
  const moveTab = (wid, { key, toWid }) => apply(moveTabToWindow(wstate, wid, key, toWid));
  const detachTab = (wid, { key, pos }) => apply(detachTabToNewWindow(wstate, wid, key, pos));
  // Promote an info-only window to a live 3D viewer; the state module revokes the
  // lowest-stacked live viewer when the cap is reached, so the user can always
  // inspect the 3D they asked for.
  const loadModelFor = (wid) => apply(requestModel(wstate, wid));

  // Restoring returns keyboard focus to the window's active tab — the dock chip
  // that was just clicked disappears from under the pointer otherwise.
  async function restoreWin(wid) {
    apply(restoreWindow(wstate, wid));
    await tick();
    modalRefs[wid]?.focusActive();
  }

  // ONE Escape handler for the whole viewer: each inspector used to register its
  // own, and since none of them marked the event handled, a single press closed
  // every open panel at once.
  function onKey(e) {
    if (e.key !== 'Escape') return;
    // Precedence follows the stacking bands (lib/viewer/zLayers.ts): the
    // immersive walkthrough sits on top of everything and owns Escape (the
    // browser also uses it to leave pointer lock), then the preview overlay,
    // then the windows. A tab drag cancels itself on Escape and marks the event
    // handled, which `defaultPrevented` covers.
    if (e.defaultPrevented || walkthrough || $preview) return;
    // Escape in the search box clears the search — it must not also close a window.
    const tag = e.target?.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || e.target?.isContentEditable) return;
    const top = wstate.windows.filter((w) => !w.minimized).sort((a, b) => b.z - a.z)[0];
    if (!top) return;
    e.preventDefault();
    closeWin(top.wid);
  }

  // The browse cache is shared by every window — drop it with the page so a long
  // session can't pin memory (and so a re-entry re-reads live data). The z report
  // has to be retracted too, or the preview overlay would keep making room for
  // windows that no longer exist.
  onDestroy(() => {
    resourceCache.clear();
    reportInspectorTopZ(0);
  });

  // Fly the map to a located element (windows stay open; the flown-to element
  // gets the selection highlight so it's findable among the rooftops). When the
  // canvas is in 3D-Tiles (Cesium) mode the MapLibre component isn't mounted, so
  // switch back to the map first and focus once it has initialised.
  // `force` because this is the user asking, in so many words, to be taken
  // there: focusElement leaves the camera alone for a target that is already
  // comfortably framed, which would make the button look broken.
  function showOnMap(elId) {
    selected = elId;
    if (canvasMode === 'cesium') {
      canvasMode = 'map';
      setTimeout(() => mapComponent?.focusElement(elId, { force: true }), 650);
    } else {
      mapComponent?.focusElement(elId, { force: true });
    }
  }

  function onMapSelect(event) {
    const { id: elId, guid } = event.detail || {};
    // An IFC mesh pick carries the element's GlobalId — resolve it to the feed
    // element so clicking a beam opens *that beam's* panel.
    if (guid) {
      const byGuid = elements.find((e) => e.ifc_guid === guid);
      if (byGuid) {
        open(byGuid.id);
        return;
      }
    }
    if (elId) open(elId);
  }

  // ── First-person walkthrough ────────────────────────────────────────────────
  // ViewerMap suggests an IFC building to walk through once you're zoomed in on
  // it; clicking the prompt opens the whole-building model in a first-person view.
  let walkSuggest = null; // { id, label } | null
  let walkthrough = null; // { url, format, upAxis, label } | null
  function onWalkSuggest(e) {
    walkSuggest = e.detail || null;
  }
  function openWalkthrough() {
    const el = walkSuggest && elements.find((x) => x.id === walkSuggest.id);
    const ref = el && modelRefOf(el);
    if (!ref) return;
    walkthrough = {
      url: ref.url.split('#')[0], // the whole building, not one element fragment
      format: ref.format,
      upAxis: ref.upAxis,
      label: walkSuggest.label,
    };
  }
  // "Explore inside" from an element inspector — open the walkthrough for the
  // element's whole IFC building (a container: Site / Building / Storey).
  function walkthroughFor(elId) {
    const el = elements.find((x) => x.id === elId);
    const ref = el && modelRefsOf(el).find((r) => r.format === 'ifc');
    if (!ref) return;
    walkthrough = {
      url: ref.url.split('#')[0],
      format: 'ifc',
      upAxis: ref.upAxis,
      label: el.label || shortenIRI(el.id),
    };
  }
  // Leaving the walkthrough to read an element's data keeps the trip resumable:
  // the walkthrough component remembers its camera pose per model URL, and the
  // chip below re-opens the same model — together they put you back exactly
  // where you stood in the house.
  let lastWalkthrough = null;
  function walkInspect(e) {
    lastWalkthrough = walkthrough;
    walkthrough = null; // leave the immersive view to show the full RDF panel
    onMapSelect(e);
  }

  // ── Embed snippet ───────────────────────────────────────────────────────────
  // Copyable <iframe> code for the current canvas mode; the /embed/* pages are
  // the chrome-less variants of this explorer (see docs/embedding.md).
  let embedOpen = false;
  let embedCopied = false;
  $: embedUrl = `${window.location.origin}/embed/${canvasMode === 'cesium' ? 'cesium' : 'map'}/${encodeURIComponent(id)}`;
  $: embedSnippet = `<iframe src="${embedUrl}" width="100%" height="480" style="border:0;border-radius:12px" loading="lazy" allowfullscreen></iframe>`;
  async function copyEmbed() {
    embedCopied = await copyToClipboard(embedSnippet);
    if (embedCopied) setTimeout(() => (embedCopied = false), 1600);
  }

  load();
</script>

<div class="page explorer-page">
  <div class="page-head">
    <Link to={`/datasets/${id}`} class="btn btn-sm">
      <ChevronLeft size={16} />
      {$i18nT('pages.datasetViewer.back')}
    </Link>
    <h1>{$i18nT('pages.datasetViewer.title')}</h1>
    {#if !loading && elements.length}
      <span class="count-chip">{elements.length} {$i18nT('pages.datasetViewer.elements').toLowerCase()}</span>
    {/if}
    {#if !loading && has3dContent}
      <!-- Render-mode toggle: the full MapLibre+three.js explorer (default) vs.
           the CesiumJS 3D-Tiles globe. A toggle, not a separate page — the side
           list + ElementModal inspector stay in both. -->
      <div class="canvas-mode" role="group" aria-label={$i18nT('pages.datasetViewer.cesium3dDesc')}>
        <button class:active={canvasMode === 'map'} on:click={() => (canvasMode = 'map')} title={$i18nT('pages.datasetViewer.mapMode')}>
          <MapPin size={13} /> {$i18nT('pages.datasetViewer.mapMode')}
        </button>
        <button class:active={canvasMode === 'cesium'} on:click={() => (canvasMode = 'cesium')} title={$i18nT('pages.datasetViewer.cesium3dDesc')}>
          <Boxes size={13} /> {$i18nT('pages.datasetViewer.cesium3d')}
        </button>
      </div>
    {/if}
    {#if !loading && elements.length}
      <div class="dl-wrap embed-wrap">
        <button class="btn btn-sm" on:click={() => (embedOpen = !embedOpen)} aria-expanded={embedOpen}>
          <Code2 size={14} /> {$i18nT('embed.embedTitle')}
        </button>
        {#if embedOpen}
          <div class="dl-menu embed-menu">
            <div class="dl-head">{$i18nT('embed.embedDesc')}</div>
            <code class="embed-code">{embedSnippet}</code>
            <button class="btn btn-sm embed-copy" on:click={copyEmbed}>
              {#if embedCopied}<Check size={13} /> {$i18nT('embed.copied')}{:else}{$i18nT('embed.copy')}{/if}
            </button>
          </div>
        {/if}
      </div>
    {/if}
    {#if !loading && (ifcUrl || graphs.length)}
      <div class="dl-wrap">
        <button class="btn btn-sm" on:click={() => (dlOpen = !dlOpen)} aria-expanded={dlOpen}>
          <Download size={14} /> {$i18nT('viewer.download')}
        </button>
        {#if dlOpen}
          <div class="dl-menu">
            {#if ifcUrl}
              <a class="dl-item dl-ifc" href={ifcUrl} download>
                <FileDown size={13} /> {$i18nT('viewer.downloadIfc')}
              </a>
            {/if}
            {#if graphs.length}
              <div class="dl-head">{$i18nT('viewer.downloadLd')}</div>
              {#each graphs as g (g.graph_iri)}
                <div class="dl-graph">
                  <span class="dl-gname" title={g.graph_iri}>{shortenIRI(g.graph_iri)}</span>
                  <span class="dl-fmts">
                    {#each LD_FORMATS as f}
                      <a href={`/store?graph=${encodeURIComponent(g.graph_iri)}&format=${f.key}`}>{f.label}</a>
                    {/each}
                  </span>
                </div>
              {/each}
            {/if}
          </div>
        {/if}
      </div>
    {/if}
  </div>

  {#if error}
    <p class="hint error">{error}</p>
  {:else}
    <!-- The shell + map render immediately; the side list shows a loading state
         while the feed streams in, so the map is interactive in ~1s instead of
         blocking on the whole-building feed. -->
    <div class="explorer">
      <aside class="side card-flat">
        <label class="search">
          <Search size={14} />
          <input
            type="search"
            placeholder={$i18nT('viewer.search')}
            bind:value={query}
            aria-label={$i18nT('viewer.search')}
          />
        </label>
        {#if hasStructure && !query}
          <div class="tree-tools">
            <button class="link-btn" on:click={expandAll}>{$i18nT('viewer.expandAll')}</button>
            <span class="dot-sep">·</span>
            <button class="link-btn" on:click={collapseAll}>{$i18nT('viewer.collapseAll')}</button>
          </div>
        {/if}
        <div class="list-scroll">
          {#if loading && elements.length === 0}
            <div class="side-loading" role="status">
              <span class="ls-spin"></span>{$i18nT('viewer.loadingElements')}
            </div>
          {:else if !loading && elements.length === 0}
            <p class="empty-row">{$i18nT('pages.datasetViewer.empty')}</p>
          {:else if query}
            <!-- Search: flat result list across the whole dataset. -->
            <ul>
              {#each filtered as el (el.id)}
                <li>
                  <button class="row" class:active={el.id === selected} on:click={() => open(el.id)} title={el.id}>
                    <span class="label">{nodeLabel(el)}</span>
                    {#if el.wkt4326}<span class="loc-i"><MapPin size={11} /></span>{/if}
                    {#if hasModel(el)}<span class="badge">3D</span>{/if}
                  </button>
                </li>
              {/each}
              {#if !filtered.length}<li class="empty-row">{$i18nT('viewer.noMatches')}</li>{/if}
            </ul>
          {:else if hasStructure}
            <!-- Structural tree: Site → Building → Storey → Space → Element. -->
            <ul class="tree">
              {#each treeRows as r (r.el.id)}
                <li
                  class="tree-row"
                  class:active={r.el.id === selected}
                  style:--depth={r.depth}
                >
                  {#if r.count}
                    <button
                      class="twist"
                      class:open={r.open}
                      on:click={() => toggle(r.el.id)}
                      aria-label={r.open ? $i18nT('viewer.collapse') : $i18nT('viewer.expand')}
                    ><ChevronRight size={13} /></button>
                  {:else}
                    <span class="twist-spacer"></span>
                  {/if}
                  <button class="row row-main" on:click={() => open(r.el.id)} title={r.el.id}>
                    <span class="label">{nodeLabel(r.el)}</span>
                    {#if r.count}<span class="cnt">{r.count}</span>{/if}
                    {#if r.el.wkt4326}<span class="loc-i"><MapPin size={11} /></span>{/if}
                    {#if hasModel(r.el)}<span class="badge">3D</span>{/if}
                  </button>
                </li>
              {/each}
            </ul>
          {:else}
            <!-- Flat dataset (no spatial structure): located / unlocated split. -->
            {#if located.length}
              <div class="group-label"><MapPin size={12} /> {$i18nT('viewer.located')}</div>
              <ul>
                {#each located as el (el.id)}
                  <li>
                    <button class="row" class:active={el.id === selected} on:click={() => open(el.id)} title={el.id}>
                      <span class="label">{nodeLabel(el)}</span>
                      {#if hasModel(el)}<span class="badge">3D</span>{/if}
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
            {#if unlocated.length}
              <div class="group-label"><Boxes size={12} /> {$i18nT('viewer.noLocation')}</div>
              <ul>
                {#each unlocated as el (el.id)}
                  <li>
                    <button class="row" class:active={el.id === selected} on:click={() => open(el.id)} title={el.id}>
                      <span class="label">{nodeLabel(el)}</span>
                      {#if hasModel(el)}<span class="badge">3D</span>{/if}
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        </div>
        {#if !fullLoaded}
          <div class="loading-structure" role="status">
            <span class="ls-spin"></span>{$i18nT('viewer.loadingStructure')}
          </div>
        {/if}
        <p class="side-hint">{$i18nT('viewer.zoomHint')}</p>
      </aside>

      <section class="canvas card-flat">
        {#if canvasMode === 'cesium'}
          <!-- 3D Tiles globe. Embedded: a pick dispatches `select`, opening the
               same ElementModal inspector the map view uses (no lost features). -->
          <CesiumViewer datasetId={id} {selected} embedded on:select={onMapSelect} height="100%" />
        {:else if hasGeo || loading}
          <!-- Mount the map immediately (even before the feed resolves) so the
               basemap is interactive in ~1s; features stream in when the feed lands. -->
          <ViewerMap
            bind:this={mapComponent}
            {elements}
            {selected}
            extraAttribution={mapAttribution}
            on:select={onMapSelect}
            on:walksuggest={onWalkSuggest}
            height="100%"
          />
          {#if walkSuggest && !walkthrough}
            <button class="walk-suggest" on:click={openWalkthrough}>
              <Footprints size={16} /> {$i18nT('viewer.walkThroughBuilding')}
            </button>
          {/if}
        {:else}
          <Model3D refs={fallbackRefs} {selected} on:select={onMapSelect} height="100%" />
        {/if}
      </section>
    </div>
  {/if}
</div>

<svelte:window on:keydown={onKey} />

{#each windowViews as { w, element } (w.wid)}
  <!-- A minimised window keeps its tabs (in wstate) but unmounts its body, so
       its WebGL context and every heavy child are released until it's restored. -->
  {#if !w.minimized}
    <ElementModal
      bind:this={modalRefs[w.wid]}
      {element}
      {elements}
      datasetId={id}
      wid={w.wid}
      tabs={w.tabs}
      activeKey={w.activeKey}
      targets={windowViews.filter((x) => x.w.wid !== w.wid).map((x) => ({ wid: x.w.wid, label: x.label }))}
      pos={w.pos}
      full={w.full}
      z={zByWid.get(w.wid)}
      lite={!w.loadModel}
      hasMap={hasGeo}
      on:focus={() => focusWin(w.wid)}
      on:close={() => closeWin(w.wid)}
      on:minimize={() => minimizeWin(w.wid)}
      on:togglefull={() => toggleWinFull(w.wid)}
      on:move={(e) => moveWin(w.wid, e.detail)}
      on:opentab={(e) => openTab(w.wid, e.detail)}
      on:navigate={(e) => openWindowFor(e.detail)}
      on:tabselect={(e) => selectTab(w.wid, e.detail.key)}
      on:tabclose={(e) => closeTabIn(w.wid, e.detail.key)}
      on:tabmove={(e) => moveTab(w.wid, e.detail)}
      on:tabdetach={(e) => detachTab(w.wid, e.detail)}
      on:loadmodel={() => loadModelFor(w.wid)}
      on:showonmap={(e) => showOnMap(e.detail.id)}
      on:walkthrough={(e) => walkthroughFor(e.detail.id)}
    />
  {/if}
{/each}

{#if windows.length}
  <div class="panel-dock">
    <span class="dock-label" title={$i18nT('pages.datasetViewer.windowsHint')}>
      {windows.length}
      {$i18nT('viewer.openWindows')}
    </span>
    {#each windowViews as { w, label } (w.wid)}
      <span class="dock-chip" class:has3d={w.loadModel && !w.minimized} class:minimized={w.minimized}>
        <button
          class="dock-focus"
          on:click={() => (w.minimized ? restoreWin(w.wid) : focusWin(w.wid))}
          title={w.tabs.map((t) => t.label).join(' · ')}
          aria-label={`${label} — ${$i18nT('viewer.tabCount', { values: { count: w.tabs.length } })}${w.minimized ? ` — ${$i18nT('viewer.restoreWindow')}` : ''}`}
        >
          {#if w.loadModel && !w.minimized}<Boxes size={11} />{/if}
          <span class="dock-name">{label}</span>
          {#if w.tabs.length > 1}<span class="dock-tabs">+{w.tabs.length - 1}</span>{/if}
        </button>
        <button class="dock-close" on:click={() => closeWin(w.wid)} aria-label={$i18nT('viewer.closeWindow')}>
          <X size={12} />
        </button>
      </span>
    {/each}
    {#if windows.length > 1}
      <button class="dock-closeall" on:click={closeAll}>{$i18nT('viewer.closeAll')}</button>
    {/if}
  </div>
{/if}

{#if walkthrough}
  <Walkthrough
    url={walkthrough.url}
    format={walkthrough.format}
    upAxis={walkthrough.upAxis}
    label={walkthrough.label}
    {elements}
    on:close={() => (walkthrough = null)}
    on:inspect={walkInspect}
  />
{:else if lastWalkthrough}
  <button
    class="walk-resume"
    on:click={() => {
      walkthrough = lastWalkthrough;
      lastWalkthrough = null;
    }}
  >
    <Footprints size={15} /> {$i18nT('viewer.resumeWalkthrough')}{lastWalkthrough.label ? ` · ${lastWalkthrough.label}` : ''}
  </button>
{/if}

<style>
  .explorer-page {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 90px);
  }
  .page-head {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.6rem;
  }
  .page-head h1 {
    margin: 0;
    font-size: 1.2rem;
    color: var(--ink-900, #0f172a);
  }
  .count-chip {
    font-size: 0.74rem;
    padding: 2px 10px;
    border-radius: 99px;
    background: var(--bg-soft, #f1f5f9);
    color: var(--muted, #64748b);
  }
  .explorer {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: 290px 1fr;
    gap: 0.6rem;
  }
  .side,
  .canvas {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border: 1px solid var(--border, #e2e8f0);
    border-radius: var(--radius-lg, 12px);
    background: var(--bg-elevated, #fff);
    overflow: hidden;
  }
  .canvas {
    position: relative; /* anchor the floating "walk through" prompt */
  }
  .walk-suggest {
    position: absolute;
    left: 50%;
    bottom: 18px;
    transform: translateX(-50%);
    z-index: 6;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 9px 17px;
    border: 0;
    border-radius: 999px;
    background: var(--brand-600, #2563a8);
    color: #fff;
    font-size: 0.84rem;
    font-weight: 600;
    cursor: pointer;
    box-shadow: 0 6px 22px rgba(37, 99, 168, 0.5);
    /* One-shot entrance instead of an endless box-shadow pulse, which read as
       distracting and ignored motion preferences. */
    animation: rowIn var(--dur-base, 220ms) var(--ease-out, ease) both;
  }
  .walk-suggest:hover {
    background: #e8590c;
  }
  /* Floating "pick up where you left off" chip after leaving the walkthrough
     to inspect an element. Fixed: it must survive scrolling the RDF panel. */
  .walk-resume {
    position: fixed;
    right: 22px;
    bottom: 22px;
    z-index: 1350; /* above windows + dock, below an active walkthrough */
    display: inline-flex;
    align-items: center;
    gap: 7px;
    padding: 9px 17px;
    border: 0;
    border-radius: 999px;
    background: var(--brand-600, #2563a8);
    color: #fff;
    font-size: 0.84rem;
    font-weight: 600;
    cursor: pointer;
    box-shadow: 0 6px 22px rgba(37, 99, 168, 0.5);
    animation: rowIn var(--dur-base, 220ms) var(--ease-out, ease) both;
  }
  .walk-resume:hover {
    background: #e8590c;
  }
  @media (prefers-reduced-motion: reduce) {
    .walk-suggest,
    .walk-resume,
    .panel-dock,
    .dl-menu {
      animation: none;
    }
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 10px;
    padding: 6px 10px;
    border: 1px solid var(--line-soft, #e6eaef);
    border-radius: var(--radius-md, 9px);
    background: var(--bg, #fff);
    color: var(--muted, #64748b);
  }
  .search input {
    flex: 1;
    border: 0;
    outline: 0;
    background: transparent;
    font-size: 0.85rem;
    color: var(--ink-900, #0f172a);
  }
  .list-scroll {
    flex: 1;
    overflow: auto;
    padding: 0 8px 8px;
  }
  .group-label {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted, #64748b);
    padding: 8px 6px 4px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  li > button {
    width: 100%;
    text-align: left;
    border: 0;
    background: transparent;
    padding: 6px 8px;
    border-radius: var(--radius-sm, 7px);
    cursor: pointer;
    font-size: 0.86rem;
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: var(--ink-900, #0f172a);
  }
  li > button:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.04));
  }
  li > button.active {
    background: var(--bg-accent-soft, #e7f0fb);
    font-weight: 600;
  }
  li .label {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    font-size: 0.62rem;
    padding: 0 6px;
    border-radius: 99px;
    background: rgba(232, 89, 12, 0.13);
    color: #e8590c;
  }

  /* Structural tree (Site → Building → Storey → Space → Element) */
  .tree-tools {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 12px 0;
  }
  .link-btn {
    border: 0;
    background: transparent;
    color: var(--brand-600, #2563a8);
    cursor: pointer;
    padding: 2px 0;
    font-size: 0.7rem;
  }
  .link-btn:hover {
    text-decoration: underline;
  }
  .dot-sep {
    color: var(--muted, #94a3b8);
    font-size: 0.7rem;
  }
  .tree-row {
    display: flex;
    align-items: center;
    border-radius: var(--radius-sm, 7px);
    padding-left: calc(var(--depth, 0) * 13px);
  }
  .tree-row:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.04));
  }
  .tree-row.active {
    background: var(--bg-accent-soft, #e7f0fb);
  }
  .tree-row.active .label {
    font-weight: 600;
  }
  .tree-row .twist {
    flex: none;
    width: 22px;
    align-self: stretch;
    display: flex;
    align-items: center;
    justify-content: center;
    border: 0;
    background: transparent;
    color: var(--muted, #64748b);
    cursor: pointer;
    padding: 0;
  }
  .tree-row .twist :global(svg) {
    transition: transform 0.12s ease;
  }
  .tree-row .twist.open :global(svg) {
    transform: rotate(90deg);
  }
  .twist-spacer {
    flex: none;
    width: 22px;
  }
  .tree-row .row-main {
    flex: 1;
    min-width: 0;
    width: auto;
    background: transparent;
  }
  .tree-row .row-main:hover {
    background: transparent;
  }
  .cnt {
    flex: none;
    font-size: 0.64rem;
    padding: 0 6px;
    border-radius: 99px;
    background: var(--bg-soft, #eef2f6);
    color: var(--muted, #64748b);
  }
  .loc-i {
    flex: none;
    display: inline-flex;
    color: var(--brand-500, #2f88d8);
  }
  .empty-row {
    padding: 10px 8px;
    color: var(--muted, #64748b);
    font-size: 0.8rem;
  }
  .side-hint {
    margin: 0;
    padding: 8px 12px;
    border-top: 1px solid var(--line-soft, #eef1f4);
    font-size: 0.72rem;
    color: var(--muted, #64748b);
  }
  /* Side-list loading state while the first (located) feed is in flight. */
  .side-loading {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 14px 12px;
    font-size: 0.8rem;
    color: var(--muted, #64748b);
  }
  /* Background full-feed (structure tree) loading cue — the map is already live. */
  .loading-structure {
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 7px 12px;
    border-top: 1px solid var(--line-soft, #eef1f4);
    font-size: 0.72rem;
    color: var(--muted, #64748b);
  }
  .ls-spin {
    width: 11px;
    height: 11px;
    flex: none;
    border: 2px solid color-mix(in srgb, var(--brand-500, #2f88d8) 35%, transparent);
    border-top-color: var(--brand-500, #2f88d8);
    border-radius: 50%;
    animation: ls-spin 0.8s linear infinite;
  }
  @keyframes ls-spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ls-spin {
      animation: none;
    }
  }
  .hint {
    color: var(--muted, #64748b);
  }
  .hint.error {
    color: var(--danger-500, #c0392b);
  }

  /* Canvas render-mode toggle (Map ↔ 3D Tiles) */
  .canvas-mode {
    display: inline-flex;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 999px;
    overflow: hidden;
    background: var(--bg-soft, #f1f5f9);
  }
  .canvas-mode button {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 0;
    background: transparent;
    padding: 4px 11px;
    font-size: 0.76rem;
    color: var(--muted, #64748b);
    cursor: pointer;
  }
  .canvas-mode button + button {
    border-left: 1px solid var(--line-soft, #e2e8f0);
  }
  .canvas-mode button.active {
    background: var(--bg-accent-soft, #e7f0fb);
    color: var(--brand-600, #2563a8);
    font-weight: 600;
  }
  /* Visible keyboard focus on the bespoke viewer controls. */
  .canvas-mode button:focus-visible,
  .dock-focus:focus-visible,
  .dock-close:focus-visible,
  .dock-closeall:focus-visible,
  .twist:focus-visible,
  .link-btn:focus-visible,
  .walk-suggest:focus-visible,
  .row:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--brand-400, #5aa9e0);
    border-radius: var(--radius-sm, 7px);
  }

  /* Download menu (IFC + linked-data formats) */
  .dl-wrap { position: relative; margin-left: auto; }
  /* When the Embed button is present it owns the auto push; Download follows it. */
  .embed-wrap ~ .dl-wrap { margin-left: 0; }
  .embed-menu { min-width: 340px; }
  .embed-code {
    display: block;
    margin: 6px 8px;
    padding: 8px 10px;
    border-radius: 8px;
    background: var(--bg-soft, #f1f5f9);
    color: var(--ink-700, #334155);
    font-size: 0.68rem;
    line-height: 1.5;
    word-break: break-all;
    user-select: all;
  }
  .embed-copy { margin: 0 8px 6px; }
  .dl-menu {
    position: absolute;
    right: 0;
    top: calc(100% + 6px);
    z-index: 50;
    min-width: 300px;
    padding: 8px;
    border-radius: 12px;
    background: var(--bg-elevated, #fff);
    border: 1px solid var(--line-soft, #e2e8f0);
    box-shadow: var(--shadow-md, 0 12px 30px rgba(0, 0, 0, 0.16));
    animation: rowIn var(--dur-base, 220ms) var(--ease-out, ease) both;
  }
  .dl-item {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 7px 8px;
    border-radius: 8px;
    font-size: 0.84rem;
    color: var(--ink-900, #0f172a);
    text-decoration: none;
  }
  .dl-item:hover { background: var(--bg-hover, rgba(0, 0, 0, 0.05)); }
  .dl-head {
    font-size: 0.66rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted, #64748b);
    font-weight: 700;
    padding: 8px 8px 2px;
  }
  .dl-graph {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 10px;
    padding: 5px 8px;
    font-size: 0.8rem;
  }
  .dl-gname {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--ink-700, #334155);
    min-width: 0;
  }
  .dl-fmts { display: inline-flex; gap: 8px; flex: none; }
  .dl-fmts a { font-size: 0.74rem; color: var(--brand-600, #1d6fb8); text-decoration: none; }
  .dl-fmts a:hover { text-decoration: underline; }

  /* Dock — manages the open inspector windows (focus / restore / close). */
  .panel-dock {
    position: fixed;
    left: 50%;
    bottom: 14px;
    transform: translateX(-50%);
    z-index: 1300;
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: min(92vw, 900px);
    flex-wrap: wrap;
    padding: 6px 8px;
    border-radius: 999px;
    background: var(--bg-elevated, rgba(255, 255, 255, 0.92));
    border: 1px solid var(--line-soft, #e2e8f0);
    box-shadow: var(--shadow-md, 0 12px 30px rgba(0, 0, 0, 0.16));
    backdrop-filter: blur(10px);
    animation: rowIn var(--dur-base, 220ms) var(--ease-out, ease) both;
  }
  .dock-label {
    font-size: 0.7rem;
    color: var(--muted, #64748b);
    padding: 0 4px;
    white-space: nowrap;
  }
  .dock-chip {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    border-radius: 999px;
    background: var(--bg-soft, #f1f5f9);
    border: 1px solid transparent;
    max-width: 180px;
  }
  .dock-chip.has3d {
    border-color: rgba(232, 89, 12, 0.4);
  }
  /* A minimised window is still open — dashed and dimmed says "parked here",
     and clicking the chip brings it back with its tabs intact. */
  .dock-chip.minimized {
    border-style: dashed;
    border-color: var(--line-soft, #cbd5e1);
    opacity: 0.75;
  }
  /* "+N" = how many more tabs this window holds beyond the active one. */
  .dock-tabs {
    flex: none;
    font-size: 0.66rem;
    padding: 0 6px;
    border-radius: 99px;
    background: var(--bg-accent-soft, #e7f0fb);
    color: var(--brand-600, #1d6fb8);
  }
  .dock-focus {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    border: 0;
    background: transparent;
    padding: 4px 4px 4px 10px;
    cursor: pointer;
    font-size: 0.76rem;
    color: var(--ink-900, #0f172a);
    min-width: 0;
  }
  .dock-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dock-close {
    border: 0;
    background: transparent;
    padding: 4px 8px 4px 2px;
    cursor: pointer;
    color: var(--muted, #64748b);
    display: grid;
    place-items: center;
  }
  .dock-close:hover {
    color: var(--danger-500, #c0392b);
  }
  .dock-closeall {
    border: 0;
    background: transparent;
    cursor: pointer;
    font-size: 0.72rem;
    color: var(--muted, #64748b);
    padding: 0 8px;
    white-space: nowrap;
  }
  .dock-closeall:hover {
    color: var(--ink-900, #0f172a);
  }
  @media (max-width: 900px) {
    .explorer {
      grid-template-columns: 1fr;
      grid-template-rows: 220px 1fr;
    }
  }
</style>
