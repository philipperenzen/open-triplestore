<script>
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import cytoscape from 'cytoscape';
  import coseBilkent from 'cytoscape-cose-bilkent';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { literalBadge, datatypeLabel } from '../lib/ontology/valueType.js';
  import { isDark } from '../lib/theme.js';
  import Combobox from './Combobox.svelte';
  import ValueRenderer from './ontology/ValueRenderer.svelte';
  import RdfTerm from './RdfTerm.svelte';
  import GeoPreview from './GeoPreview.svelte';
  import EdgeInfoPanel from './EdgeInfoPanel.svelte';
  import { X, Copy, Check, ArrowUpRight, MapPin, ArrowRight, ArrowDownLeft } from 'lucide-svelte';
  import { copyToClipboard } from '../lib/clipboard.js';

  export let nodes = [];
  export let edges = [];
  export let layout = 'cose-bilkent';
  export let height = '500px';
  // Set of node IDs to highlight; null/undefined means no filter (show all normally)
  export let highlightIds = null;
  // Show a full-canvas loading overlay (initial data fetch)
  export let loading = false;
  // Show a slim top banner without unmounting the canvas (load-more / incremental)
  export let loadingMore = false;
  // Set of IRIs that have been expanded (shows − badge on hover)
  /** @type {Set<string> | null} */
  export let expandedNodes = null;
  // IRI of the node currently being fetched (shows pulsing border + badge)
  export let expandingNode = null;
  // Set of IRIs where all connections have been fetched (hides + badge)
  /** @type {Set<string> | null} */
  export let exhaustedNodes = null;
  // When true, a single tap opens the node inspector panel (properties + the data
  // reached *through* blank nodes). Consumers that used to navigate on click can
  // leave this on and handle the panel's `nodeOpen` event instead.
  export let inspector = true;

  const dispatch = createEventDispatcher();

  // Dark-mode flag (reactive) — drives canvas label + minimap colours so the
  // graph isn't a bright panel inside the dark app shell.
  let dark = false;
  $: dark = $isDark;

  let container;
  let wrapperEl;
  let cy;
  let minimapCanvas;
  let minimapCtx;
  let minimapDimTimer;
  let minimapDimmed = false;
  // True while a Cytoscape layout is animating. During this time the viewport
  // changes (fit-to-graph) continuously, causing the minimap scale to fluctuate.
  // We suppress viewport-driven minimap renders until layoutstop.
  let layoutRunning = true; // assume true on init; set false by first layoutstop
  let tooltip = { visible: false, x: 0, y: 0, label: '', type: '', iri: '', value: '', datatype: '', language: '' };
  // Node inspector panels — several can be open at once (one per node, deduped
  // by node id), each draggable and z-stacked. The count is capped by viewport
  // width so panels never bury the graph on small screens. See openInspector().
  /** @type {Array<{id:string, model:any, pos:{x:number,y:number}, z:number, seq:number, incomingLimit:number}>} */
  let inspPanels = [];
  let inspZTop = 25;   // stacking order base (panel CSS z-index starts here)
  let inspSeq = 0;     // cascade counter so new panels don't open exactly on top
  let inspDrag = null; // { id, dx, dy } while dragging a panel by its header
  // Edge / predicate info panel state — see buildEdgeInspector(). Single
  // instance (one edge panel at a time); node panels stay open alongside it.
  let inspectedEdge = null;   // built model for the tapped edge, or null when closed
  let inspectedEdgeId = null; // its edge id, so we can rebuild / clear on graph changes
  let copiedInspector = '';   // `${panelId}` that just copied (checkmark flash)
  let inspectorAutoExpanded = new Set(); // blank-node ids we've asked the host to load
  let spinnerPos = null; // { x, y } rendered px — drives HTML spinner overlay
  let pinnedNodes = new Set();
  // Tracks the last node tap so we can detect double-taps ourselves: Cytoscape
  // core emits no 'dbltap' event, so a `cy.on('dbltap', …)` listener never fires.
  let lastNodeTap = { id: null, t: 0 };
  let settingsOpen = false;
  let internalLayout = layout;
  let internalSearch = '';
  // Search box starts minimised (just an icon) and expands on hover/focus.
  let searchExpanded = false;
  // Autocomplete options drawn ONLY from nodes currently in the graph.
  $: nodeSuggestions = [...new Set(
    (nodes || [])
      .map((n) => n?.data?.label || n?.data?.fullIri)
      .filter(Boolean)
  )].slice(0, 200);
  function expandSearch() {
    searchExpanded = true;
  }
  function maybeCollapseSearch() {
    if (!internalSearch) searchExpanded = false;
  }

  // ─── User-adjustable settings ────────────────────────────────────────────────
  let settings = {
    nodeSizeScale: 1,    // 0.5 – 2.0
    textSize: 11,        // px, 8 – 18
    edgeLength: 120,     // px, 40 – 400 (idealEdgeLength for force layouts)
    edgeWidth: 1.5,      // px, 0.5 – 5
  };

  function applySettings() {
    if (!cy) return;
    cy.batch(() => {
      cy.nodes().style({
        'width':     (ele) => nodeSize(ele),
        'height':    (ele) => nodeSize(ele),
        'font-size': `${settings.textSize}px`,
        'min-zoomed-font-size': Math.max(5, settings.textSize * 0.6),
      });
      cy.edges().style({
        'font-size': `${Math.max(8, settings.textSize - 1)}px`,
        'min-zoomed-font-size': Math.max(4, (settings.textSize - 1) * 0.55),
        'width': settings.edgeWidth,
      });
    });
  }

  // Register cose-bilkent layout
  try { cytoscape.use(coseBilkent); } catch { /* already registered */ }

  // ─── Node badge SVGs ────────────────────────────────────────────────────────

  // Composite badge: action badge at bottom-right, pin at top-right (when pinned)
  function nodeBadge(ele) {
    // URIs are always expandable (addressable by their IRI). A blank node is
    // expandable only when it has a parent edge in the graph to anchor a path query
    // on — an orphan blank node (no incoming edge) can't be referenced in SPARQL.
    const nodeType = ele.data('nodeType');
    const isExpandable = nodeType === 'uri' || (nodeType === 'bnode' && ele.indegree(false) > 0);
    const isPinned = !!ele.data('pinned');
    const isExpanded = !!ele.data('expanded');
    const isExpanding = !!ele.data('expanding');
    const isExhausted = !!ele.data('exhausted');
    const isHovered = !!ele.data('hovered');
    // (Literal nodes show their datatype/language as a separate fixed-size pill —
    // see litPillUri + the node[isLiteral] style — so it stays crisp and readable
    // instead of being stretched along with the node.)
    if (!isExpandable && !isPinned) return 'none';

    const parts = [];
    if (isExpandable) {
      if (isExpanding) {
        // Spinner is an HTML overlay with real CSS @keyframes — SVG background-image cannot animate.
      } else if (isHovered) {
        if (isExpanded) {
          // Hover: dark-blue − badge (collapse)
          parts.push(
            `<circle cx="33" cy="33" r="5.5" fill="#1d4ed8" stroke="white" stroke-width="1.5"/>`,
            `<line x1="30" y1="33" x2="36" y2="33" stroke="white" stroke-width="2" stroke-linecap="round"/>`,
          );
        } else if (!isExhausted) {
          // Hover + not exhausted: blue + badge (expand available)
          parts.push(
            `<circle cx="33" cy="33" r="5.5" fill="#3b82f6" stroke="white" stroke-width="1.5"/>`,
            `<line x1="33" y1="30" x2="33" y2="36" stroke="white" stroke-width="2" stroke-linecap="round"/>`,
            `<line x1="30" y1="33" x2="36" y2="33" stroke="white" stroke-width="2" stroke-linecap="round"/>`,
          );
        }
        // if exhausted and not expanded: no badge (all connections shown)
      }
    }
    if (isPinned) {
      // Top-right: amber pin badge — always visible
      parts.push(
        `<circle cx="33" cy="7" r="5.5" fill="#f59e0b" stroke="white" stroke-width="1.5"/>`,
        `<line x1="33" y1="4" x2="33" y2="10.5" stroke="white" stroke-width="1.8" stroke-linecap="round"/>`,
        `<line x1="30.5" y1="6.5" x2="33" y2="4" stroke="white" stroke-width="1.5" stroke-linecap="round"/>`,
        `<line x1="35.5" y1="6.5" x2="33" y2="4" stroke="white" stroke-width="1.5" stroke-linecap="round"/>`,
      );
    }
    if (parts.length === 0) return 'none';
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40" viewBox="0 0 40 40">${parts.join('')}</svg>`;
    return `data:image/svg+xml,${encodeURIComponent(svg)}`;
  }

  // ─── Literal datatype pill ────────────────────────────────────────────────────
  // A readable, colour-coded tag naming a literal's kind (str / num / date / geo /
  // @en …). Rendered as a fixed-size corner badge on literal nodes so — unlike the
  // old 5px dot stretched with the node — you can recognise the datatype at a glance.
  function litBadgeFor(ele) {
    if (ele.data('nodeType') !== 'literal') return null;
    return literalBadge(ele.data('datatype'), ele.data('language'));
  }
  function litPillWidth(text) {
    return Math.round(Math.max(20, (text || '').length * 6.5 + 13));
  }
  function litPillUri(text, color) {
    const w = litPillWidth(text);
    const h = 17;
    const safe = String(text).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    const svg =
      `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
      `<rect x="0.9" y="0.9" width="${w - 1.8}" height="${h - 1.8}" rx="${(h - 1.8) / 2}" ` +
        `fill="${color}" stroke="#ffffff" stroke-width="1.6"/>` +
      `<text x="${w / 2}" y="${h / 2 + 1}" text-anchor="middle" ` +
        `font-family="IBM Plex Sans, system-ui, sans-serif" font-size="10" font-weight="700" fill="#ffffff">` +
        `${safe}</text>` +
      `</svg>`;
    return `data:image/svg+xml,${encodeURIComponent(svg)}`;
  }

  // ─── Colour system ──────────────────────────────────────────────────────────

  // Ontology-aware type palette (fill, stroke) — matches graph viewer conventions
  const TYPE_PALETTE = {
    'owl:Class':                 { fill: '#ede9fe', stroke: '#7c3aed' },
    'rdfs:Class':                { fill: '#ede9fe', stroke: '#7c3aed' },
    'owl:ObjectProperty':        { fill: '#fce7f3', stroke: '#be185d' },
    'owl:DatatypeProperty':      { fill: '#fce7f3', stroke: '#be185d' },
    'rdf:Property':              { fill: '#fce7f3', stroke: '#be185d' },
    'skos:Concept':              { fill: '#dbeafe', stroke: '#2563eb' },
    'skos:ConceptScheme':        { fill: '#eff6ff', stroke: '#3b82f6' },
    'dcat:Dataset':              { fill: '#fef3c7', stroke: '#d97706' },
    'dcat:Distribution':         { fill: '#fef9c3', stroke: '#ca8a04' },
    'foaf:Person':               { fill: '#dcfce7', stroke: '#16a34a' },
    'foaf:Organization':         { fill: '#ffedd5', stroke: '#ea580c' },
    'schema:Person':             { fill: '#dcfce7', stroke: '#16a34a' },
    'schema:Organization':       { fill: '#ffedd5', stroke: '#ea580c' },
    'prov:Entity':               { fill: '#f0fdf4', stroke: '#22c55e' },
    'prov:Activity':             { fill: '#ecfeff', stroke: '#0891b2' },
    'prov:Agent':                { fill: '#f0f9ff', stroke: '#0284c7' },
    'owl:NamedIndividual':       { fill: '#e0f2fe', stroke: '#0369a1' },
    'sh:NodeShape':              { fill: '#fdf4ff', stroke: '#a21caf' },
    'sh:PropertyShape':          { fill: '#fdf4ff', stroke: '#c026d3' },
  };

  // Degree-scaled node size — reads data.degree which is kept in sync by updateGraph
  function nodeSize(ele) {
    const deg = ele.data('degree') || 1;
    return Math.round((30 + Math.sqrt(deg) * 7) * settings.nodeSizeScale);
  }

  // Hash a string to a hue (0-360)
  function strHue(str) {
    let h = 0;
    for (let i = 0; i < str.length; i++) h = (h * 31 + str.charCodeAt(i)) & 0xffffffff;
    return Math.abs(h) % 360;
  }

  // Extract namespace from IRI (up to last # or /)
  function namespace(iri) {
    if (!iri) return '';
    const h = iri.lastIndexOf('#');
    const s = iri.lastIndexOf('/');
    const idx = Math.max(h, s);
    return idx > 0 ? iri.slice(0, idx + 1) : iri;
  }

  function nodeColors(ele) {
    const rdfType = ele.data('rdfType');
    if (rdfType && TYPE_PALETTE[rdfType]) return TYPE_PALETTE[rdfType];
    const nodeType = ele.data('nodeType');
    if (nodeType === 'literal') return { fill: '#f0fdf4', stroke: '#16a34a' };
    if (nodeType === 'bnode')   return { fill: '#fafafa', stroke: '#a1a1aa' };
    // Hash-based for URIs
    const iri = ele.data('fullIri') || ele.id();
    const hue = strHue(namespace(iri));
    return { fill: `hsl(${hue},60%,92%)`, stroke: `hsl(${hue},55%,42%)` };
  }

  function edgeColor(ele) {
    const predIri = ele.data('predicate') || '';
    const hue = strHue(namespace(predIri));
    return `hsl(${hue},50%,48%)`;
  }

  // ─── Layout options ──────────────────────────────────────────────────────────

  // Per-edge ideal length: longer edges between bigger / more-connected nodes so
  // dense hubs push their neighbours out and stay readable instead of collapsing
  // into a hairball. length = base + k·(sizeA+sizeB) + m·(degA+degB), clamped so
  // the user's edge-length slider still anchors the scale.
  //   base  ← the slider value (settings.edgeLength)
  //   k     ← how much the two endpoints' rendered sizes stretch the edge
  //   m     ← how much the two endpoints' degrees stretch the edge
  // Used as a function for the built-in 'cose' layout (which supports a function
  // idealEdgeLength); cose-bilkent only takes a number, so for that layout we
  // feed a single value scaled by the graph's average endpoint weight instead.
  function endpointWeight(node) {
    if (!node || !node.data) return { size: 30, deg: 1 };
    const deg = (typeof node.degree === 'function' ? node.degree(false) : node.data('degree')) || 1;
    return { size: nodeSize(node), deg };
  }
  function edgeIdealLength(edge) {
    const base = settings.edgeLength;
    const a = endpointWeight(edge.source());
    const b = endpointWeight(edge.target());
    const k = 0.9;   // size contribution
    const m = 4;     // degree contribution
    const len = base + k * (a.size + b.size) + m * (a.deg + b.deg);
    // Keep it sane: never shorter than the slider, never absurdly long.
    return Math.round(Math.min(base * 6 + 600, Math.max(base, len)));
  }

  // cose-bilkent takes a *number* for idealEdgeLength, so derive one that scales
  // with the average node size + degree across the current graph — this spreads
  // dense graphs proportionally even though we can't vary length per edge there.
  function scaledGlobalEdgeLength() {
    const base = settings.edgeLength;
    if (!cy || cy.nodes().length === 0) return base;
    const ns = cy.nodes();
    let sizeSum = 0, degSum = 0;
    ns.forEach((n) => { sizeSum += nodeSize(n); degSum += (n.degree(false) || 1); });
    const avgSize = sizeSum / ns.length;
    const avgDeg = degSum / ns.length;
    // Two endpoints per edge → roughly 2× the per-node averages.
    const len = base + 0.9 * (2 * avgSize) + 4 * (2 * avgDeg);
    return Math.round(Math.min(base * 4 + 400, Math.max(base, len)));
  }

  function getLayoutOptions(name) {
    const eLen = settings.edgeLength;
    // STATIC: no entrance animation — nodes appear in their final positions
    // immediately (the user reported an off-putting left→zoom-in reveal). This
    // also means no animated fit/zoom runs as part of layout on initial load.
    const base = { padding: 50, animate: false, fit: true };
    switch (name) {
      case 'cose-bilkent':
        // Higher repulsion + label-aware sizing keeps every node clearly
        // separated; the scaled global edge length spreads dense hubs. We also
        // pass the per-edge function (harmless if cose-bilkent ignores it).
        return {
          name: 'cose-bilkent', ...base,
          nodeRepulsion: 18000,
          idealEdgeLength: scaledGlobalEdgeLength(),
          edgeElasticity: 0.45,
          nodeDimensionsIncludeLabels: true,
          gravity: 0.25,
          gravityRange: 3.8,
          tile: true,
        };
      case 'cose':
        // Built-in cose DOES support idealEdgeLength as a function of the edge,
        // so this branch gives true per-edge lengths driven by both endpoints.
        return {
          name: 'cose', ...base,
          nodeRepulsion: 400000,
          idealEdgeLength: (edge) => edgeIdealLength(edge),
          nodeOverlap: 24,
          nodeDimensionsIncludeLabels: true,
          gravity: 40,
          componentSpacing: 120,
        };
      case 'breadthfirst':
        return { name: 'breadthfirst', ...base, directed: false, spacingFactor: 1.8 };
      case 'grid':
        return { name: 'grid', ...base, rows: Math.ceil(Math.sqrt(nodes.length)) };
      case 'circle':
        return { name: 'circle', ...base };
      case 'concentric':
        return { name: 'concentric', ...base, minNodeSpacing: eLen * 0.33 };
      default:
        return {
          name: 'cose', ...base,
          nodeRepulsion: 400000,
          idealEdgeLength: (edge) => edgeIdealLength(edge),
          nodeOverlap: 24,
          nodeDimensionsIncludeLabels: true,
          gravity: 40,
          componentSpacing: 120,
        };
    }
  }

  // ─── Minimap ─────────────────────────────────────────────────────────────────

  function renderMinimap() {
    if (!minimapCtx || !cy) return;
    const W = 180, H = 120;
    minimapCtx.clearRect(0, 0, W, H);
    minimapCtx.fillStyle = dark ? '#0b1220' : '#f8fafc';
    minimapCtx.fillRect(0, 0, W, H);

    const ext = cy.extent();
    if (!ext || ext.w <= 0 || ext.h <= 0) return;
    const scaleX = W / ext.w;
    const scaleY = H / ext.h;
    const scale = Math.min(scaleX, scaleY) * 0.85;

    const offX = (W - ext.w * scale) / 2 - ext.x1 * scale;
    const offY = (H - ext.h * scale) / 2 - ext.y1 * scale;

    // Draw edges
    minimapCtx.strokeStyle = dark ? 'rgba(148,163,184,0.4)' : '#c7d2fe';
    minimapCtx.lineWidth = 0.7;
    cy.edges().forEach(edge => {
      const src = edge.source().position();
      const tgt = edge.target().position();
      minimapCtx.beginPath();
      minimapCtx.moveTo(src.x * scale + offX, src.y * scale + offY);
      minimapCtx.lineTo(tgt.x * scale + offX, tgt.y * scale + offY);
      minimapCtx.stroke();
    });

    // Draw nodes
    cy.nodes().forEach(node => {
      const pos = node.position();
      const x = pos.x * scale + offX;
      const y = pos.y * scale + offY;
      const r = Math.max(2, Math.sqrt(node.data('degree') || 1) * 1.5 + 2);
      const col = nodeColors(node);
      minimapCtx.beginPath();
      minimapCtx.arc(x, y, r, 0, Math.PI * 2);
      minimapCtx.fillStyle = col.stroke;
      minimapCtx.fill();
    });

    // Draw viewport rectangle — clamp to canvas bounds so it stays visible when
    // the user is zoomed out far enough that the viewport exceeds the minimap size.
    const pan = cy.pan();
    const zoom = cy.zoom();
    const contW = cy.width();
    const contH = cy.height();
    const vx1 = (-pan.x / zoom) * scale + offX;
    const vy1 = (-pan.y / zoom) * scale + offY;
    const vw = (contW / zoom) * scale;
    const vh = (contH / zoom) * scale;

    const rx = Math.max(0, vx1);
    const ry = Math.max(0, vy1);
    const rx2 = Math.min(W, vx1 + vw);
    const ry2 = Math.min(H, vy1 + vh);
    if (rx2 > rx && ry2 > ry) {
      minimapCtx.strokeStyle = '#3b82f6';
      minimapCtx.lineWidth = 1.5;
      minimapCtx.strokeRect(rx, ry, rx2 - rx, ry2 - ry);
      minimapCtx.fillStyle = 'rgba(59,130,246,0.08)';
      minimapCtx.fillRect(rx, ry, rx2 - rx, ry2 - ry);
    }
  }

  let minimapDebounce;
  let layoutDebounce;

  // Debounced layout runner — batches rapid additions into one layout pass
  function scheduleLayout() {
    clearTimeout(layoutDebounce);
    layoutDebounce = setTimeout(() => {
      if (!cy) return;
      const nodeCount = cy.nodes().length;
      // Auto-switch to faster layout for large graphs
      const effectiveLayout = nodeCount > 300 ? 'breadthfirst' : internalLayout;
      cy.layout(getLayoutOptions(effectiveLayout)).run();
    }, 120);
  }

  function scheduleMinimapRender() {
    clearTimeout(minimapDebounce);
    minimapDebounce = setTimeout(renderMinimap, 60);
    // Reset dim timer
    minimapDimmed = false;
    clearTimeout(minimapDimTimer);
    minimapDimTimer = setTimeout(() => { minimapDimmed = true; }, 3000);
  }

  // ─── Mount ───────────────────────────────────────────────────────────────────

  onMount(() => {
    cy = cytoscape({
      container,
      pixelRatio: window.devicePixelRatio || 2,
      elements: { nodes, edges },
      style: /** @type {any} */ (buildStyle()),
      layout: getLayoutOptions(layout),
      minZoom: 0.05,
      maxZoom: 12,
    });

    // ── Events ──────────────────────────────────────────────────────────────

    cy.on('tap', 'node', (evt) => {
      const data = evt.target.data();
      const now = Date.now();
      // Manual double-tap detection (Cytoscape has no native 'dbltap'): two taps
      // on the same node within 300ms expand it; otherwise it's a single select.
      if (lastNodeTap.id === data.id && now - lastNodeTap.t < 300) {
        lastNodeTap = { id: null, t: 0 };
        if (data.fullIri || data.nodeType === 'bnode') dispatch('nodeExpand', data);
        return;
      }
      lastNodeTap = { id: data.id, t: now };
      dispatch('nodeClick', data);
      if (inspector) openInspector(evt.target);
    });

    cy.on('tap', 'edge', (evt) => {
      dispatch('edgeClick', evt.target.data());
      if (inspector) openEdgeInspector(evt.target);
    });

    // Tap on empty canvas closes whichever panel is open (node taps set
    // evt.target to the node, edge taps to the edge).
    cy.on('tap', (evt) => { if (evt.target === cy) { closeInspectorOnCanvasTap(); closeEdgeInspector(); } });

    // Context menu (right-click)
    cy.on('cxttap', 'node', (evt) => {
      evt.originalEvent.preventDefault();
      dispatch('nodeContextMenu', {
        data: evt.target.data(),
        x: evt.originalEvent.clientX,
        y: evt.originalEvent.clientY,
      });
    });

    cy.on('cxttap', (evt) => {
      if (evt.target === cy) {
        evt.originalEvent.preventDefault();
        dispatch('canvasContextMenu', {
          x: evt.originalEvent.clientX,
          y: evt.originalEvent.clientY,
        });
      }
    });

    // Hover dimming
    cy.on('mouseover', 'node', (evt) => {
      const node = evt.target;
      const hood = node.closedNeighborhood();
      cy.batch(() => {
        cy.elements().not(hood).style({ opacity: 0.2 });
        hood.style({ opacity: 1 });
        node.data('hovered', true);
      });
      const { x1, y1 } = node.renderedBoundingBox();
      // A node shows the "+" badge (and so can be expanded by double-click) when it
      // is a URI, or a blank node with a parent edge to anchor on — and isn't already
      // expanded/exhausted. Surface that as a tooltip hint so the affordance is discoverable.
      const ntype = node.data('nodeType') || 'uri';
      const expandable = ntype === 'uri' || (ntype === 'bnode' && node.indegree(false) > 0);
      tooltip = {
        visible: true,
        x: x1 + node.renderedWidth() / 2,
        y: y1 - 8,
        label: node.data('label') || node.id(),
        type: ntype,
        iri: node.data('fullIri') || '',
        value: node.data('isLiteral') ? String(node.data('literalValue') ?? '') : '',
        datatype: node.data('datatype') || '',
        language: node.data('language') || '',
        expandHint: expandable && !node.data('expanded') && !node.data('exhausted'),
      };
    });

    cy.on('mouseout', 'node', (evt) => {
      evt.target.data('hovered', false);
      cy.elements().style({ opacity: 1 });
      tooltip = { ...tooltip, visible: false };
    });

    // Keyboard shortcuts on the wrapper
    wrapperEl?.addEventListener('keydown', handleKeydown);

    // Prevent browser native context menu on the canvas so cxttap fires cleanly
    container.addEventListener('contextmenu', (e) => e.preventDefault());
    wrapperEl?.addEventListener('contextmenu', (e) => e.preventDefault());

    // Viewport changes (zoom / pan): update minimap immediately and reset the dim timer.
    // Skip during layout animation — the continuous fit-to-graph viewport changes
    // during animation make the minimap appear to zoom in/out.
    cy.on('viewport', () => {
      if (layoutRunning) return;
      renderMinimap();
      minimapDimmed = false;
      clearTimeout(minimapDimTimer);
      minimapDimTimer = setTimeout(() => { minimapDimmed = true; }, 3000);
    });

    // Track layout state so the viewport handler above can suppress renders.
    cy.on('layoutstart', () => { layoutRunning = true; });

    // Graph structure / position changes: debounced (can be expensive).
    // Deliberately NOT using 'render' here — it fires every animation frame and
    // would continually cancel the debounce timer, preventing the minimap from updating.
    cy.on('add remove', scheduleMinimapRender);
    cy.on('dragfree', 'node', scheduleMinimapRender);
    cy.on('layoutstop', () => { layoutRunning = false; applySettings(); scheduleMinimapRender(); });

    // Setup minimap canvas
    minimapCtx = minimapCanvas?.getContext('2d');
    scheduleMinimapRender();

    // Minimap click → pan the main canvas to the clicked graph position
    minimapCanvas?.addEventListener('click', (e) => {
      if (!cy) return;
      const rect = minimapCanvas.getBoundingClientRect();
      const mx = (e.clientX - rect.left) * (minimapCanvas.width / rect.width);
      const my = (e.clientY - rect.top) * (minimapCanvas.height / rect.height);
      const W = minimapCanvas.width, H = minimapCanvas.height;
      const ext = cy.extent();
      if (!ext || ext.w <= 0 || ext.h <= 0) return;
      const scaleX = W / ext.w;
      const scaleY = H / ext.h;
      const scale = Math.min(scaleX, scaleY) * 0.85;
      const offX = (W - ext.w * scale) / 2 - ext.x1 * scale;
      const offY = (H - ext.h * scale) / 2 - ext.y1 * scale;
      const gx = (mx - offX) / scale;
      const gy = (my - offY) / scale;
      cy.animate({
        pan: { x: cy.width() / 2 - gx * cy.zoom(), y: cy.height() / 2 - gy * cy.zoom() }
      }, { duration: 200 });
    });
  });

  onDestroy(() => {
    if (cy) cy.destroy();
    clearTimeout(minimapDebounce);
    clearTimeout(minimapDimTimer);
    clearTimeout(layoutDebounce);
    wrapperEl?.removeEventListener('keydown', handleKeydown);
  });

  // ─── Style builder ───────────────────────────────────────────────────────────

  function buildStyle() {
    return [
      {
        selector: 'node',
        style: {
          'background-color': (ele) => nodeColors(ele).fill,
          'border-color': (ele) => nodeColors(ele).stroke,
          'border-width': 2,
          'label': 'data(label)',
          'color': dark ? '#e2e8f0' : '#1e293b',
          'font-size': `${settings.textSize}px`,
          'font-weight': '600',
          'font-family': 'IBM Plex Sans, system-ui, sans-serif',
          'text-valign': 'bottom',
          'text-halign': 'center',
          'text-margin-y': 5,
          'width': (ele) => nodeSize(ele),
          'height': (ele) => nodeSize(ele),
          'text-wrap': 'ellipsis',
          'text-max-width': 90,
          'text-outline-color': dark ? '#0b1220' : '#ffffff',
          'text-outline-width': 2,
          'min-zoomed-font-size': Math.max(5, settings.textSize * 0.6),
          'shadow-blur': 8,
          'shadow-color': 'rgba(0,0,0,0.12)',
          'shadow-offset-x': 0,
          'shadow-offset-y': 2,
          'shadow-opacity': 1,
          'background-image': (ele) => nodeBadge(ele),
          'background-width': '100%',
          'background-height': '100%',
          'background-clip': 'none',
          'background-image-containment': 'over',
          'bounds-expansion': 7,
        }
      },
      {
        selector: 'node[isLiteral]',
        style: {
          'shape': 'round-rectangle',
          'width': 'label',
          'height': 'label',
          'padding': '9px',
          'font-size': '10px',
          'text-valign': 'center',
          'text-halign': 'center',
          // Datatype/language tag as a fixed-size pill on the bottom-left corner,
          // overriding the (expand/pin) composite badge that literals never need.
          'background-image': (ele) => { const lb = litBadgeFor(ele); return lb ? litPillUri(lb.text, lb.color) : 'none'; },
          'background-width': (ele) => { const lb = litBadgeFor(ele); return lb ? `${litPillWidth(lb.text)}px` : '0px'; },
          'background-height': '17px',
          'background-position-x': '1px',
          'background-position-y': '101%',
          'background-clip': 'none',
          'background-image-containment': 'over',
          'bounds-expansion': 10,
        }
      },
      {
        selector: 'node[?pinned]',
        style: {
          'border-style': 'dashed',
          'border-width': 3,
        }
      },
      {
        selector: 'node:selected',
        style: {
          'border-color': '#3b82f6',
          'border-width': 4,
          'shadow-color': 'rgba(59,130,246,0.35)',
          'shadow-blur': 14,
          'overlay-opacity': 0,
        }
      },
      {
        selector: 'edge',
        style: {
          'width': settings.edgeWidth,
          'line-color': (ele) => edgeColor(ele),
          'target-arrow-color': (ele) => edgeColor(ele),
          'target-arrow-shape': 'triangle',
          'curve-style': 'bezier',
          'label': 'data(label)',
          // Predicate labels: slightly larger + bolder + higher-contrast colour
          // and a more opaque pill so they stay legible at the default zoom.
          'font-size': `${Math.max(8, settings.textSize - 1)}px`,
          'font-weight': '700',
          'font-family': 'IBM Plex Sans, system-ui, sans-serif',
          'color': dark ? '#e2e8f0' : '#1e293b',
          'text-rotation': 'autorotate',
          'text-margin-y': -7,
          'text-outline-color': dark ? '#0b1220' : '#ffffff',
          'text-outline-width': 2.5,
          'min-zoomed-font-size': Math.max(4, (settings.textSize - 1) * 0.55),
          'text-background-color': dark ? 'rgba(15,23,42,0.92)' : 'rgba(255,255,255,0.92)',
          'text-background-opacity': 1,
          'text-background-shape': 'round-rectangle',
          'text-background-padding': '3px',
        }
      },
      {
        selector: 'edge:selected',
        style: {
          'line-color': '#3b82f6',
          'target-arrow-color': '#3b82f6',
          'width': 2.5,
        }
      },
      {
        selector: 'node[?expanded]',
        style: {
          'border-color': '#1d4ed8',
          'border-width': 3,
        }
      },
      {
        selector: 'node[?expanding]',
        style: {
          'border-width': 3,
        }
      },
      {
        selector: 'node:active',
        style: { 'overlay-opacity': 0 }
      },
      {
        selector: 'edge:active',
        style: { 'overlay-opacity': 0 }
      },
    ];
  }

  // ─── Reactive graph update ───────────────────────────────────────────────────

  $: if (cy && (nodes || edges)) {
    updateGraph(nodes, edges);
  }

  // Re-skin the live canvas + minimap when the theme flips (dark is the dep).
  $: reskinCanvas(cy, dark);
  function reskinCanvas(cyInst, _dark) {
    if (!cyInst) return;
    cyInst.style(buildStyle());
    applySettings();
    renderMinimap();
  }

  // Sync expandedNodes prop → node data so nodeBadge re-renders
  $: if (cy && expandedNodes !== undefined) {
    cy.batch(() => {
      cy.nodes().forEach(n => {
        const iri = n.data('fullIri') || (n.data('nodeType') === 'bnode' ? n.id() : null);
        const shouldBeExpanded = iri && expandedNodes && expandedNodes.has(iri) ? true : undefined;
        if (!!n.data('expanded') !== !!shouldBeExpanded) n.data('expanded', shouldBeExpanded);
      });
    });
    cy.style().update();
  }

  // Sync expandingNode prop → node data so nodeBadge/border re-renders
  $: if (cy && expandingNode !== undefined) {
    cy.batch(() => {
      cy.nodes().forEach(n => {
        const iri = n.data('fullIri') || (n.data('nodeType') === 'bnode' ? n.id() : null);
        const shouldBeExpanding = iri && iri === expandingNode ? true : undefined;
        if (!!n.data('expanding') !== !!shouldBeExpanding) n.data('expanding', shouldBeExpanding);
      });
    });
    cy.style().update();
    updateSpinnerPos();
  }

  // Sync exhaustedNodes prop → node data so badge hides + on exhausted nodes
  $: if (cy && exhaustedNodes !== undefined) {
    cy.batch(() => {
      cy.nodes().forEach(n => {
        const iri = n.data('fullIri') || (n.data('nodeType') === 'bnode' ? n.id() : null);
        const shouldBeExhausted = iri && exhaustedNodes && exhaustedNodes.has(iri) ? true : undefined;
        if (!!n.data('exhausted') !== !!shouldBeExhausted) n.data('exhausted', shouldBeExhausted);
      });
    });
  }

  // ─── Spinner overlay position ─────────────────────────────────────────────

  // Find the expanding node's rendered pixel position so the HTML spinner sits on it.
  function updateSpinnerPos() {
    if (!cy || !expandingNode) { spinnerPos = null; return; }
    const node = cy.nodes().filter(n => (n.data('fullIri') || n.id()) === expandingNode).first();
    if (!node || node.length === 0) { spinnerPos = null; return; }
    const rp = node.renderedPosition();
    spinnerPos = { x: rp.x, y: rp.y };
  }

  // ─── Node inspector ───────────────────────────────────────────────────────
  // Builds a description of a tapped node from the LIVE in-graph neighbourhood and
  // — crucially — follows blank nodes so values that hide behind them (a geometry
  // behind geo:hasGeometry, a file behind a dcat:Distribution, an address, …) are
  // surfaced inline instead of dead-ending at an anonymous node. Rendering is
  // delegated to ValueRenderer (the same component the resource pages use), so
  // geometries, images, files and nested blank nodes look consistent everywhere.

  const RDF_TYPE_IRI = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
  const GEO_WKT_DT = 'http://www.opengis.net/ont/geosparql#wktLiteral';
  const WKT_RE = /^\s*(?:<[^>]*>\s*)?(?:POINT|MULTIPOINT|LINESTRING|MULTILINESTRING|POLYGON|MULTIPOLYGON|GEOMETRYCOLLECTION|TRIANGLE|TIN|POLYHEDRALSURFACE|CIRCULARSTRING|CURVEPOLYGON)\s*(?:Z|M|ZM)?\s*(?:\(|EMPTY)/i;
  const WGS84_LAT = 'http://www.w3.org/2003/01/geo/wgs84_pos#lat';
  const WGS84_LONG = 'http://www.w3.org/2003/01/geo/wgs84_pos#long';
  const SCHEMA_LAT = 'http://schema.org/latitude';
  const SCHEMA_LONG = 'http://schema.org/longitude';

  // A cytoscape node → SPARQL-JSON-style term (the shape ValueRenderer/RdfTerm want).
  function cyNodeToTerm(n) {
    const nt = n.data('nodeType');
    if (nt === 'literal') {
      const term = { type: 'literal', value: String(n.data('literalValue') ?? n.data('label') ?? '') };
      if (n.data('datatype')) term.datatype = n.data('datatype');
      if (n.data('language')) term['xml:lang'] = n.data('language');
      return term;
    }
    if (nt === 'bnode') return { type: 'bnode', value: n.id() };
    return { type: 'uri', value: n.data('fullIri') || n.id() };
  }

  // Walk the in-graph blank-node closure reachable from `node`, building the
  // { bnodeId: [{p, o}] } map ValueRenderer uses to expand blank nodes inline.
  // Depth-bounded, mirroring ValueRenderer's own MAX_DEPTH.
  function collectBnodeClosure(node, map = {}, seen = new Set(), depth = 0) {
    if (depth > 6) return map;
    node.outgoers('edge').forEach((e) => {
      const tgt = e.target();
      if (tgt.data('nodeType') !== 'bnode') return;
      const id = tgt.id();
      if (seen.has(id)) return;
      seen.add(id);
      map[id] = tgt.outgoers('edge').map((ce) => ({
        p: { type: 'uri', value: ce.data('predicate') },
        o: cyNodeToTerm(ce.target()),
      }));
      collectBnodeClosure(tgt, map, seen, depth + 1);
    });
    return map;
  }

  const isWktTerm = (term) =>
    term?.type === 'literal' && (term.datatype === GEO_WKT_DT || WKT_RE.test(term.value || ''));

  // Every geometry reachable from the node — direct WKT literals, WKT carried by a
  // blank node (geo:hasGeometry [ geo:asWKT … ]) and lat/long pairs — so a map can
  // be shown even when the geometry hides behind a blank node.
  function collectGeometries(props, bnodes) {
    const rows = props.map((p) => ({ p: p.predicate, o: p.o }));
    for (const list of Object.values(bnodes)) for (const r of (list || [])) rows.push({ p: r.p?.value, o: r.o });
    const out = [];
    const seen = new Set();
    for (const r of rows) if (isWktTerm(r.o) && !seen.has(r.o.value)) { seen.add(r.o.value); out.push(r.o.value); }
    const findVal = (preds) => { for (const r of rows) if (preds.includes(r.p)) return r.o?.value; return undefined; };
    const lat = findVal([WGS84_LAT, SCHEMA_LAT]);
    const lon = findVal([WGS84_LONG, SCHEMA_LONG]);
    if (lat != null && lon != null && !isNaN(parseFloat(lat)) && !isNaN(parseFloat(lon))) {
      const wkt = `POINT(${parseFloat(lon)} ${parseFloat(lat)})`;
      if (!seen.has(wkt)) out.push(wkt);
    }
    return out;
  }

  function buildInspector(node) {
    if (!node || node.length === 0) return null;
    const nodeType = node.data('nodeType') || 'uri';
    const props = node.outgoers('edge').map((e) => ({
      predicate: e.data('predicate') || '',
      predLabel: e.data('label') || shortenIRI(e.data('predicate') || ''),
      o: cyNodeToTerm(e.target()),
    })).filter((p) => p.predicate !== RDF_TYPE_IRI)
      .sort((a, b) => a.predLabel.localeCompare(b.predLabel));
    const incoming = node.incomers('edge').map((e) => ({
      predicate: e.data('predicate') || '',
      predLabel: e.data('label') || shortenIRI(e.data('predicate') || ''),
      s: cyNodeToTerm(e.source()),
    })).sort((a, b) => a.predLabel.localeCompare(b.predLabel));
    const bnodes = collectBnodeClosure(node);
    return {
      id: node.id(),
      nodeType,
      label: node.data('label') || node.id(),
      iri: node.data('fullIri') || '',
      value: nodeType === 'literal' ? String(node.data('literalValue') ?? node.data('label') ?? '') : '',
      datatype: node.data('datatype') || '',
      language: node.data('language') || '',
      rdfType: node.data('rdfType') || '',
      props,
      incoming,
      bnodes,
      geometries: collectGeometries(props, bnodes),
    };
  }

  // How many panels fit comfortably: panels are 320px wide; leave room for the
  // graph itself. 1 on narrow screens, up to 4 on wide ones.
  function maxInspPanels() {
    const w = typeof window === 'undefined' ? 1280 : window.innerWidth;
    return Math.max(1, Math.min(4, Math.floor(w / 380)));
  }

  function openInspector(node) {
    if (!inspector || !node) return;
    // An edge panel occupies the same default spot — close it; node panels the
    // user has arranged stay open (multi-panel comparison is the whole point).
    closeEdgeInspector();
    const id = node.id();
    const existing = inspPanels.find((p) => p.id === id);
    if (existing) {
      // Same node tapped again: refresh its content and bring it to front.
      existing.model = buildInspector(node);
      focusInspPanel(id);
      return;
    }
    const cascade = (inspSeq % 5) * 28;
    inspPanels = [
      ...inspPanels,
      { id, model: buildInspector(node), pos: { x: cascade, y: cascade }, z: ++inspZTop, seq: inspSeq++, incomingLimit: 25 },
    ];
    const cap = maxInspPanels();
    if (inspPanels.length > cap) inspPanels = inspPanels.slice(inspPanels.length - cap);
    autoExpandBlankChildren(node);
  }
  // When the inspected node — or a blank node it points at — has no contents in
  // the graph yet (e.g. a geometry behind geo:hasGeometry, or a freshly-tapped
  // blank node), ask the host to load it once. refreshInspector() then rebuilds
  // the panel as those neighbours arrive, so data hidden behind a blank node
  // appears without a manual expand.
  function autoExpandBlankChildren(node) {
    const requestExpand = (n) => {
      if (n.data('nodeType') !== 'bnode' || n.outgoers('edge').length > 0) return;
      const id = n.id();
      if (inspectorAutoExpanded.has(id)) return;
      inspectorAutoExpanded.add(id);
      dispatch('nodeExpand', n.data());
    };
    requestExpand(node); // the tapped node itself, if it's an unexpanded blank node
    node.outgoers('edge').forEach((e) => requestExpand(e.target())); // blank nodes it points at
  }
  // Rebuild after the graph changes (e.g. a blank node was expanded) so newly
  // arrived neighbours — like a geometry — appear in every open panel
  // automatically; panels whose node was removed close themselves.
  function refreshInspector() {
    if (!cy || !inspPanels.length) return;
    inspPanels = inspPanels.flatMap((p) => {
      const n = cy.getElementById(p.id);
      return n && n.length ? [{ ...p, model: buildInspector(n) }] : [];
    });
  }
  function focusInspPanel(id) {
    inspPanels = inspPanels.map((p) => (p.id === id ? { ...p, z: ++inspZTop } : p));
  }
  function closeInspPanel(id) {
    inspPanels = inspPanels.filter((p) => p.id !== id);
    if (!inspPanels.length && cy) cy.$('node:selected').unselect();
  }
  // Background tap: dismiss a lone panel (the familiar gesture), but never wipe
  // out a multi-panel arrangement the user deliberately built.
  function closeInspectorOnCanvasTap() {
    if (inspPanels.length === 1) {
      inspPanels = [];
      if (cy) cy.$(':selected').unselect();
    }
  }
  // Drag a panel by its header (pointer-only; controls stay accessible).
  function startInspDrag(e, id) {
    if (e.target.closest('button, a')) return;
    const p = inspPanels.find((x) => x.id === id);
    if (!p) return;
    inspDrag = { id, dx: e.clientX - p.pos.x, dy: e.clientY - p.pos.y };
    window.addEventListener('pointermove', onInspDrag);
    window.addEventListener('pointerup', stopInspDrag, { once: true });
  }
  function onInspDrag(e) {
    if (!inspDrag) return;
    const { id, dx, dy } = inspDrag;
    inspPanels = inspPanels.map((p) =>
      p.id === id ? { ...p, pos: { x: e.clientX - dx, y: e.clientY - dy } } : p
    );
  }
  function stopInspDrag() {
    inspDrag = null;
    window.removeEventListener('pointermove', onInspDrag);
  }
  function inspectorOpen(model) {
    if (model?.iri) {
      dispatch('nodeOpen', { id: model.id, fullIri: model.iri, nodeType: model.nodeType, label: model.label });
    }
  }
  function bumpIncomingLimit(id) {
    inspPanels = inspPanels.map((p) => (p.id === id ? { ...p, incomingLimit: p.incomingLimit + 25 } : p));
  }
  async function copyInspectorText(s, panelId = '') {
    if (await copyToClipboard(s)) {
      copiedInspector = panelId || 'x';
      setTimeout(() => (copiedInspector = ''), 1200);
    }
  }

  // ─── Edge / predicate inspector ─────────────────────────────────────────────
  // Builds a model describing the *predicate* (relationship) behind a tapped
  // edge: its IRI + short form, plus the two endpoints' labels/types so the
  // EdgeInfoPanel can infer domain/range and resolve the vocabulary. The panel
  // itself does the vocab/description resolution via the read-only ontology libs.

  function edgeEndpoint(node) {
    if (!node || node.length === 0) return { label: '', iri: null, rdfType: null, nodeType: 'uri' };
    return {
      label: node.data('label') || node.id(),
      iri: node.data('fullIri') || null,
      rdfType: node.data('rdfType') || null,
      nodeType: node.data('nodeType') || 'uri',
    };
  }

  function buildEdgeInspector(edge) {
    if (!edge || edge.length === 0) return null;
    return {
      id: edge.id(),
      predicate: edge.data('predicate') || '',
      label: edge.data('label') || shortenIRI(edge.data('predicate') || ''),
      source: edgeEndpoint(edge.source()),
      target: edgeEndpoint(edge.target()),
    };
  }

  function openEdgeInspector(edge) {
    if (!inspector || !edge) return;
    // Node panels stay open (they're movable windows the user arranged); only
    // shift the selection highlight from nodes to the just-tapped edge.
    if (cy) cy.$('node:selected').unselect();
    inspectedEdgeId = edge.id();
    inspectedEdge = buildEdgeInspector(edge);
  }

  // Rebuild after graph changes so the open edge panel stays in sync (e.g. an
  // endpoint gained an rdf:type), or close it if the edge is gone.
  function refreshEdgeInspector() {
    if (!inspectedEdgeId || !cy) return;
    const e = cy.getElementById(inspectedEdgeId);
    if (e && e.length) inspectedEdge = buildEdgeInspector(e);
    else { inspectedEdge = null; inspectedEdgeId = null; }
  }

  function closeEdgeInspector() {
    inspectedEdge = null;
    inspectedEdgeId = null;
    if (cy) cy.$('edge:selected').unselect();
  }

  // The EdgeInfoPanel's "Open predicate" → reuse the node-open dispatch so the
  // host (TripleBrowser) can navigate to the predicate's resource page just like
  // it does for nodes, without needing a new event contract.
  function openPredicateResource(detail) {
    if (detail?.iri) {
      dispatch('nodeOpen', { id: detail.iri, fullIri: detail.iri, nodeType: 'uri', label: detail.label || detail.iri });
    }
  }

  // Compute effective highlight set: external prop takes priority, then internal search
  $: internalSearchIds = internalSearch.trim().length > 1
    ? new Set(nodes
        .filter(n => {
          const q = internalSearch.toLowerCase();
          return (n.data.label || '').toLowerCase().includes(q) ||
                 (n.data.fullIri || '').toLowerCase().includes(q);
        })
        .map(n => n.data.id))
    : null;
  $: effectiveHighlightIds = highlightIds ?? internalSearchIds;

  // Highlight matching nodes when effectiveHighlightIds changes
  $: if (cy) {
    if (effectiveHighlightIds && effectiveHighlightIds.size > 0) {
      cy.batch(() => {
        cy.nodes().forEach(n => {
          const match = effectiveHighlightIds.has(n.id());
          n.style({ opacity: match ? 1 : 0.15 });
        });
        cy.edges().style({ opacity: 0.1 });
      });
    } else {
      cy.batch(() => {
        cy.nodes().style({ opacity: 1 });
        cy.edges().style({ opacity: 1 });
      });
    }
  }

  // Fan new nodes around their existing connected neighbors so they don’t scatter randomly.
  function placeIncrementalNodes(addedNodeIds) {
    const addedSet = new Set(addedNodeIds);
    // Group incoming nodes by the anchor point = average position of their pre-existing neighbors
    const byAnchor = new Map(); // anchorKey → { ax, ay, nodes[] }
    cy.nodes().filter(n => addedSet.has(n.id())).forEach(newNode => {
      const existingNeighbors = newNode.neighborhood().nodes().filter(n => !addedSet.has(n.id()));
      if (existingNeighbors.length === 0) return; // orphan — stays at (0,0)
      let ax = 0, ay = 0;
      existingNeighbors.forEach(a => { ax += a.position('x'); ay += a.position('y'); });
      ax /= existingNeighbors.length; ay /= existingNeighbors.length;
      // Key by the most-connected existing neighbor to group siblings together
      const primary = existingNeighbors.reduce((best, a) =>
        (a.degree(false) > best.degree(false) ? a : best), existingNeighbors[0]);
      const key = primary.id();
      if (!byAnchor.has(key)) byAnchor.set(key, { ax, ay, nodes: [] });
      byAnchor.get(key).nodes.push(newNode);
    });
    cy.batch(() => {
      for (const { ax, ay, nodes } of byAnchor.values()) {
        const count = nodes.length;
        const radius = Math.max(110, count * 24);
        nodes.forEach((n, i) => {
          const angle = (2 * Math.PI * i / count) - Math.PI / 2;
          n.position({ x: ax + Math.cos(angle) * radius, y: ay + Math.sin(angle) * radius });
        });
      }
    });
  }

  function updateGraph(newNodes, newEdges) {
    if (!cy) return;
    const newNodeIds = new Set(newNodes.map(n => n.data.id));
    const newEdgeIds = new Set(newEdges.map(e => e.data.id));
    const existingNodeIds = new Set(cy.nodes().map(n => n.id()));
    const existingEdgeIds = new Set(cy.edges().map(e => e.id()));
    const isIncremental = existingNodeIds.size > 0;

    // Remove nodes/edges no longer present in props (e.g. after collapse)
    let removedAny = false;
    const edgesToRemove = cy.edges().filter(e => !newEdgeIds.has(e.id()));
    const nodesToRemove = cy.nodes().filter(n => !newNodeIds.has(n.id()));
    if (edgesToRemove.length || nodesToRemove.length) {
      edgesToRemove.remove();
      nodesToRemove.remove();
      removedAny = true;
    }

    // Add nodes/edges that are new
    const addedNodeIds = new Set();
    const toAdd = [];
    for (const n of newNodes) {
      if (!existingNodeIds.has(n.data.id)) { toAdd.push(n); addedNodeIds.add(n.data.id); }
    }
    for (const e of newEdges) {
      if (!existingEdgeIds.has(e.data.id)) toAdd.push(e);
    }

    if (toAdd.length > 0) {
      cy.add(toAdd);
      // Keep data.degree in sync with the live graph so nodeSize() returns correct values
      cy.nodes().forEach(n => n.data('degree', n.degree(false)));
      applySettings(); // re-applies sizes and font styles with updated degrees
      if (isIncremental && addedNodeIds.size > 0) {
        // Incremental expansion: place new nodes near their connected existing nodes
        placeIncrementalNodes(addedNodeIds);
      } else {
        // Initial load: run the full layout
        scheduleLayout();
      }
    } else if (removedAny) {
      cy.nodes().forEach(n => n.data('degree', n.degree(false)));
      applySettings();
      scheduleLayout();
    }
    scheduleMinimapRender();
    refreshInspector();
    refreshEdgeInspector();
  }

  // ─── Keyboard ────────────────────────────────────────────────────────────────

  function handleKeydown(e) {
    if (!cy) return;
    if (e.key === '+' || e.key === '=') { e.preventDefault(); zoomIn(); }
    else if (e.key === '-') { e.preventDefault(); zoomOut(); }
    else if (e.key === 'f' || e.key === 'F') { e.preventDefault(); fitAll(); }
    else if (e.key === 'r' || e.key === 'R') { e.preventDefault(); scheduleLayout(); }
    else if (e.key === 'Escape') { cy.$(':selected').unselect(); }
    else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      const sel = cy.$('node:selected');
      if (sel.length) {
        sel.connectedEdges().remove();
        sel.remove();
        scheduleMinimapRender();
      }
    }
    else if (e.key === 'p' || e.key === 'P') {
      const sel = cy.$('node:selected');
      sel.forEach(n => {
        if (n.data('pinned')) {
          n.unlock();
          n.data('pinned', false);
          pinnedNodes.delete(n.id());
        } else {
          n.lock();
          n.data('pinned', true);
          pinnedNodes.add(n.id());
        }
      });
      cy.forceRender();
    }
  }

  // ─── Public API ──────────────────────────────────────────────────────────────

  export function fitAll() {
    if (cy) { cy.fit(undefined, 40); scheduleMinimapRender(); }
  }

  export function zoomIn() {
    if (cy) cy.zoom({ level: cy.zoom() * 1.3, renderedPosition: { x: cy.width() / 2, y: cy.height() / 2 } });
  }

  export function zoomOut() {
    if (cy) cy.zoom({ level: cy.zoom() * 0.77, renderedPosition: { x: cy.width() / 2, y: cy.height() / 2 } });
  }

  export function resetGraph() {
    if (!cy) return;
    cy.elements().remove();
    cy.add([...nodes, ...edges]);
    cy.layout(getLayoutOptions(layout)).run();
    pinnedNodes.clear();
  }

  export function exportPng(scale = 2) {
    if (!cy) return;
    const png = cy.png({ full: true, scale, bg: '#ffffff' });
    const a = document.createElement('a');
    a.href = png;
    a.download = 'graph.png';
    a.click();
  }

  /** Export a high-res PNG at 4× scale for print/presentation quality. */
  export function exportPngHiRes() { exportPng(4); }

  /**
   * Export an SVG by re-rendering the current viewport onto a Canvas then
   * embedding it in an SVG foreign-object. Falls back to PNG when unavailable.
   * Cytoscape has no native SVG serializer without a plugin, so we export the
   * scene as a data-URI PNG embedded in an SVG wrapper (maintains scalability
   * at the canvas level).
   */
  export function exportSvg() {
    if (!cy) return;
    const bb = cy.elements().boundingBox();
    const pad = 20;
    const w = Math.ceil(bb.w + pad * 2);
    const h = Math.ceil(bb.h + pad * 2);
    // Render to PNG at 2× then wrap in SVG with correct intrinsic dimensions
    const pngDataUrl = cy.png({ full: true, scale: 2, bg: '#ffffff' });
    const svgStr = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"
     width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">
  <image href="${pngDataUrl}" x="0" y="0" width="${w}" height="${h}" />
</svg>`;
    const blob = new Blob([svgStr], { type: 'image/svg+xml' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'graph.svg';
    a.click();
    URL.revokeObjectURL(url);
  }

  export function runLayout(layoutName) {
    if (cy) { cy.layout(getLayoutOptions(layoutName)).run(); }
  }

  export function clearGraph() {
    if (cy) { cy.elements().remove(); pinnedNodes.clear(); scheduleMinimapRender(); }
  }

  export function unpinNode(nodeId) {
    if (!cy) return;
    const n = cy.getElementById(nodeId);
    if (n) { n.unlock(); n.data('pinned', false); pinnedNodes.delete(nodeId); cy.forceRender(); }
  }

  export function removeNode(nodeId) {
    if (!cy) return;
    const n = cy.getElementById(nodeId);
    if (n) { n.connectedEdges().remove(); n.remove(); scheduleMinimapRender(); }
  }

  export function highlightNodes(nodeIds) {
    if (!cy) return;
    if (!nodeIds || nodeIds.length === 0) {
      cy.elements().style({ opacity: 1 });
      return;
    }
    const idSet = new Set(nodeIds);
    cy.batch(() => {
      cy.nodes().forEach(n => {
        const match = idSet.has(n.id());
        n.style({ opacity: match ? 1 : 0.15, 'border-width': match ? 4 : 2 });
      });
      cy.edges().style({ opacity: 0.15 });
    });
    // Pan to first match
    const first = cy.getElementById(nodeIds[0]);
    if (first.length) cy.animate({ center: { eles: first }, zoom: Math.max(cy.zoom(), 1) }, { duration: 350 });
  }

  export function pinSelected() {
    if (!cy) return;
    const sel = cy.$('node:selected');
    if (!sel.length) return;
    sel.forEach(n => {
      if (n.data('pinned')) {
        n.unlock(); n.data('pinned', false); pinnedNodes.delete(n.id());
      } else {
        n.lock(); n.data('pinned', true); pinnedNodes.add(n.id());
      }
    });
    cy.forceRender();
  }

  export function hasSelectedPinned() {
    if (!cy) return false;
    return cy.$('node:selected').some(n => n.data('pinned'));
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div
  class="graph-wrapper"
  style="height: {height}"
  bind:this={wrapperEl}
  tabindex="0"
>
  <div bind:this={container} class="cy-container"></div>

  <!-- Full-canvas loading overlay (initial load) -->
  {#if loading}
    <div class="graph-loading-overlay">
      <div class="graph-spinner-ring"></div>
      <span class="graph-loading-text">{$t('system.loading')}</span>
    </div>
  {/if}

  <!-- Slim top banner for incremental load-more -->
  {#if loadingMore}
    <div class="graph-loading-more-banner">
      <span class="loading-more-dot"></span>
      {$t('pages.graphViz.loadingMoreTriples')}
    </div>
  {/if}

  <!-- Built-in node search (top-left) — minimised to an icon, expands on hover/click.
       Autocomplete is sourced only from nodes currently in the graph. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="graph-search-box"
    class:collapsed={!searchExpanded && !internalSearch}
    on:click|stopPropagation
    on:keydown|stopPropagation
    on:mouseenter={expandSearch}
    on:mouseleave={maybeCollapseSearch}
  >
    <button class="graph-search-icon-btn" on:click={expandSearch} aria-label={$t('pages.graphViz.searchNodes')} title={$t('pages.graphViz.searchNodesInGraphTitle')}>
      <svg class="graph-search-icon" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35"/></svg>
    </button>
    {#if searchExpanded || internalSearch}
      <Combobox
        class="graph-search-input"
        suggestions={nodeSuggestions.map(s => s)}
        placeholder={$t('pages.graphViz.searchNodesPlaceholder')}
        bind:value={internalSearch}
        on:blur={maybeCollapseSearch}
        ariaLabel={$t('pages.graphViz.searchNodes')}
      />
      {#if internalSearch}
        <button class="graph-search-clear" on:click={() => { internalSearch = ''; searchExpanded = false; }} aria-label={$t('pages.graphViz.clearSearch')}>×</button>
      {/if}
    {/if}
  </div>

  <!-- Floating tooltip — absolute inside wrapper so fixed-transform issues don't apply -->
  {#if tooltip.visible}
    <div class="node-tooltip" style="left:{tooltip.x}px; top:{tooltip.y}px">
      <span class="tt-type tt-type-{tooltip.type}">
        {tooltip.type === 'literal' ? $t('pages.graphViz.typeLiteral')
          : tooltip.type === 'bnode' ? $t('pages.graphViz.typeBnode')
          : tooltip.type === 'uri' ? $t('pages.graphViz.typeUri')
          : tooltip.type}
      </span>
      {#if tooltip.type === 'literal'}
        <div class="tt-value">{tooltip.value}</div>
        {#if tooltip.language}
          <span class="tt-meta">@{tooltip.language}</span>
        {:else if tooltip.datatype}
          <span class="tt-meta" title={tooltip.datatype}>{datatypeLabel(tooltip.datatype)}</span>
        {/if}
      {:else}
        <span class="tt-label">{tooltip.label}</span>
        {#if tooltip.iri}
          <span class="tt-iri" title={tooltip.iri}>{shortenIRI(tooltip.iri)}</span>
        {/if}
      {/if}
      {#if tooltip.type === 'literal' || tooltip.type === 'bnode' || tooltip.type === 'uri'}
        <span class="tt-explain">
          {tooltip.type === 'literal' ? $t('pages.graphViz.typeLiteralExplain')
            : tooltip.type === 'bnode' ? $t('pages.graphViz.typeBnodeExplain')
            : $t('pages.graphViz.typeUriExplain')}
        </span>
      {/if}
      {#if tooltip.expandHint}
        <span class="tt-expand">{$t('pages.graphViz.dblclickExpand')}</span>
      {/if}
    </div>
  {/if}

  <!-- Spinner overlay for the node currently being expanded (HTML so CSS animation works) -->
  {#if spinnerPos}
    <div class="node-spinner" style="left:{spinnerPos.x}px; top:{spinnerPos.y}px" aria-label={$t('system.loading')}>
      <svg width="30" height="30" viewBox="0 0 30 30" fill="none">
        <circle cx="15" cy="15" r="12" stroke="#e2e8f0" stroke-width="3"/>
        <path d="M15 3 A12 12 0 0 1 27 15" stroke="#f59e0b" stroke-width="3" stroke-linecap="round"/>
      </svg>
    </div>
  {/if}

  <!-- Zoom controls (top-right) -->
  <div class="graph-controls">
    <button class="ctrl-btn" title={$t('pages.graphViz.fitAllTitle')} on:click={fitAll} aria-label={$t('pages.graphViz.fitAll')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7"/></svg>
    </button>
    <button class="ctrl-btn" title={$t('pages.graphViz.prettifyTitle')} on:click={scheduleLayout} aria-label={$t('pages.graphViz.prettifyLayout')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M3 2v6h6"/><path d="M3 13a9 9 0 1 0 3-7.7L3 8"/></svg>
    </button>
    <button class="ctrl-btn" title={$t('pages.graphViz.zoomInTitle')} on:click={zoomIn} aria-label={$t('pages.graphViz.zoomIn')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35M11 8v6M8 11h6"/></svg>
    </button>
    <button class="ctrl-btn" title={$t('pages.graphViz.zoomOutTitle')} on:click={zoomOut} aria-label={$t('pages.graphViz.zoomOut')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="11" cy="11" r="8"/><path d="m21 21-4.35-4.35M8 11h6"/></svg>
    </button>
    <div class="ctrl-divider"></div>
    <button class="ctrl-btn" title={$t('pages.graphViz.pinToggleTitle')} on:click={pinSelected} aria-label={$t('pages.graphViz.pinSelected')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="12" y1="17" x2="12" y2="22"/><path d="M5 17h14v-1.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1v4.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24Z"/></svg>
    </button>
    <button class="ctrl-btn" title={$t('pages.graphViz.exportPng')} on:click={() => exportPng()} aria-label={$t('pages.graphViz.exportPng')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><polyline points="7 10 12 15 17 10"/><line x1="12" y1="15" x2="12" y2="3"/></svg>
    </button>
    <div class="ctrl-divider"></div>
    <button class="ctrl-btn" class:ctrl-active={settingsOpen} title={$t('pages.graphViz.displaySettings')} on:click={() => settingsOpen = !settingsOpen} aria-label={$t('pages.graphViz.settings')}>
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>
    </button>
  </div>

  <!-- Settings panel -->
  {#if settingsOpen}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="settings-panel" on:click|stopPropagation on:keydown|stopPropagation>
      <div class="settings-header">
        <span class="settings-title">{$t('pages.graphViz.displaySettings')}</span>
        <button class="settings-close" on:click={() => settingsOpen = false} aria-label={$t('pages.graphViz.closeSettings')}>
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
        </button>
      </div>

      <label class="setting-row">
        <span class="setting-label">{$t('pages.graphViz.nodeSize')}</span>
        <span class="setting-val">{settings.nodeSizeScale.toFixed(1)}×</span>
        <input id="graph-node-size" type="range" min="0.4" max="2.5" step="0.1"
          bind:value={settings.nodeSizeScale}
          on:input={applySettings}
          class="setting-slider"
        />
      </label>

      <label class="setting-row">
        <span class="setting-label">{$t('pages.graphViz.textSize')}</span>
        <span class="setting-val">{settings.textSize}px</span>
        <input id="graph-text-size" type="range" min="7" max="20" step="1"
          bind:value={settings.textSize}
          on:input={applySettings}
          class="setting-slider"
        />
      </label>

      <label class="setting-row">
        <span class="setting-label">{$t('pages.graphViz.edgeLength')}</span>
        <span class="setting-val">{settings.edgeLength}px</span>
        <input id="graph-edge-length" type="range" min="30" max="400" step="10"
          bind:value={settings.edgeLength}
          on:input={() => { if (cy) cy.layout(getLayoutOptions(internalLayout)).run(); }}
          class="setting-slider"
        />
      </label>

      <label class="setting-row">
        <span class="setting-label">{$t('pages.graphViz.edgeWidth')}</span>
        <span class="setting-val">{settings.edgeWidth.toFixed(1)}px</span>
        <input id="graph-edge-width" type="range" min="0.5" max="6" step="0.5"
          bind:value={settings.edgeWidth}
          on:input={applySettings}
          class="setting-slider"
        />
      </label>

      <div class="setting-row setting-row-layout">
        <span class="setting-label">{$t('pages.graphViz.layout')}</span>
        <div class="layout-pills-row">
          {#each ['cose-bilkent', 'breadthfirst', 'grid', 'circle', 'concentric'] as l}
            <button
              class="layout-pill-btn"
              class:layout-pill-active={internalLayout === l}
              on:click={() => { internalLayout = l; if (cy) cy.layout(getLayoutOptions(l)).run(); }}
            >{l === 'cose-bilkent' ? $t('pages.graphViz.layoutForce') : l === 'breadthfirst' ? $t('pages.graphViz.layoutTree') : l === 'grid' ? $t('pages.graphViz.layoutGrid') : l === 'circle' ? $t('pages.graphViz.layoutCircle') : $t('pages.graphViz.layoutConcentric')}</button>
          {/each}
        </div>
      </div>

      <button class="setting-reset" on:click={() => {
        settings = { nodeSizeScale: 1, textSize: 11, edgeLength: 120, edgeWidth: 1.5 };
        applySettings();
      }}>{$t('pages.graphViz.resetDefaults')}</button>
    </div>
  {/if}

  <!-- Minimap (bottom-right) -->
  <div class="minimap-wrap" class:minimap-dimmed={minimapDimmed}>
    <canvas bind:this={minimapCanvas} width="180" height="120" class="minimap-canvas"></canvas>
    <div class="minimap-label">{$t('pages.graphViz.overview')}</div>
  </div>

  <!-- Legend (bottom-left) -->
  <div class="graph-legend">
    <span class="legend-dot" style="background:#7c3aed" title={$t('pages.graphViz.legendClassTitle')}></span><span class="legend-txt">{$t('pages.graphViz.legendClass')}</span>
    <span class="legend-dot" style="background:#2563eb" title={$t('pages.graphViz.legendConceptTitle')}></span><span class="legend-txt">{$t('pages.graphViz.legendConcept')}</span>
    <span class="legend-dot" style="background:#0369a1" title={$t('pages.graphViz.legendResourceTitle')}></span><span class="legend-txt">{$t('pages.graphViz.legendResource')}</span>
    <span class="legend-dot" style="background:#16a34a" title={$t('pages.graphViz.legendLiteralTitle')}></span><span class="legend-txt">{$t('pages.graphViz.legendLiteral')}</span>
    <span class="legend-dot" style="background:#a1a1aa" title={$t('pages.graphViz.legendBNodeTitle')}></span><span class="legend-txt">{$t('pages.graphViz.legendBNode')}</span>
  </div>

  <!-- Node inspectors — one per inspected node, all draggable and z-stacked.
       Each surfaces its node's properties and, via blank-node traversal, the
       geometry / files / data that would otherwise hide behind an anonymous
       node. Clicking a panel brings it to the front. -->
  {#if inspector}
    {#each inspPanels as p (p.id)}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div
        class="inspector-panel"
        style:transform={`translate(${p.pos.x}px, ${p.pos.y}px)`}
        style:z-index={p.z}
        on:pointerdown|capture={() => focusInspPanel(p.id)}
        on:click|stopPropagation
        on:keydown|stopPropagation
      >
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="insp-header insp-draggable" on:pointerdown={(e) => startInspDrag(e, p.id)}>
          <span class="insp-type insp-type-{p.model.nodeType}">
            {p.model.nodeType === 'literal' ? $t('pages.graphViz.typeLiteral')
              : p.model.nodeType === 'bnode' ? $t('pages.graphViz.typeBnode')
              : $t('pages.graphViz.typeUri')}
          </span>
          <button class="insp-close" on:click={() => closeInspPanel(p.id)} aria-label={$t('pages.graphViz.inspectorClose')} title={$t('pages.graphViz.inspectorClose')}>
            <X size={14} />
          </button>
        </div>

        <div class="insp-body">
          {#if p.model.nodeType === 'literal'}
            <div class="insp-literal">{p.model.value}</div>
          {:else}
            <h4 class="insp-title">{p.model.label}</h4>
          {/if}

          <p class="insp-explain">
            {p.model.nodeType === 'literal' ? $t('pages.graphViz.typeLiteralExplain')
              : p.model.nodeType === 'bnode' ? $t('pages.graphViz.typeBnodeExplain')
              : $t('pages.graphViz.typeUriExplain')}
          </p>

          {#if p.model.nodeType === 'literal'}
            <div class="insp-meta-row">
              {#if p.model.language}
                <span class="insp-chip" title={$t('pages.graphViz.inspectorLanguage')}>@{p.model.language}</span>
              {/if}
              {#if p.model.datatype}
                <span class="insp-chip mono" title={p.model.datatype}>{datatypeLabel(p.model.datatype)}</span>
              {/if}
              <button class="insp-mini-btn" on:click={() => copyInspectorText(p.model.value, p.id)} title={$t('pages.graphViz.inspectorCopyValue')}>
                {#if copiedInspector === p.id}<Check size={12} />{:else}<Copy size={12} />{/if}
              </button>
            </div>
          {:else if p.model.iri}
            <div class="insp-iri-row">
              <code class="insp-iri" title={p.model.iri}>{p.model.iri}</code>
              <button class="insp-mini-btn" on:click={() => copyInspectorText(p.model.iri, p.id)} title={$t('pages.graphViz.inspectorCopyIri')}>
                {#if copiedInspector === p.id}<Check size={12} />{:else}<Copy size={12} />{/if}
              </button>
            </div>
            <button class="insp-open-btn" on:click={() => inspectorOpen(p.model)}>
              <ArrowUpRight size={13} /> {$t('pages.graphViz.inspectorOpenResource')}
            </button>
          {/if}

          {#if p.model.rdfType}
            <div class="insp-meta-row"><span class="insp-chip type">{p.model.rdfType}</span></div>
          {/if}

          <!-- Connected data: geometry surfaced even when it sits behind a blank node -->
          {#if p.model.geometries.length > 0}
            <div class="insp-section">
              <div class="insp-section-head"><MapPin size={12} /> {$t('pages.graphViz.inspectorConnectedData')}</div>
              <GeoPreview wkts={p.model.geometries} height="150px" />
            </div>
          {/if}

          <!-- Outgoing properties — blank-node values expand inline via ValueRenderer -->
          {#if p.model.nodeType !== 'literal'}
            <div class="insp-section">
              <div class="insp-section-head">
                <ArrowRight size={12} /> {$t('pages.graphViz.inspectorProperties')}
                {#if p.model.props.length}<span class="insp-count">{p.model.props.length}</span>{/if}
              </div>
              {#if p.model.props.length === 0}
                <p class="insp-empty">{$t('pages.graphViz.inspectorNoProps')}</p>
                <p class="insp-hint">{$t('pages.graphViz.inspectorExpandHint')}</p>
              {:else}
                <div class="insp-props">
                  {#each p.model.props as prop}
                    <div class="insp-prop">
                      <span class="insp-pred" title={prop.predicate}>{prop.predLabel}</span>
                      <span class="insp-val">
                        <ValueRenderer term={prop.o} predicate={prop.predicate} bnodes={p.model.bnodes} on:run-sparql={(e) => dispatch('runSparql', e.detail)} />
                      </span>
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}

          <!-- Incoming links -->
          {#if p.model.incoming.length > 0}
            <div class="insp-section">
              <div class="insp-section-head">
                <ArrowDownLeft size={12} /> {$t('pages.graphViz.inspectorLinkedFrom')}
                <span class="insp-count">{p.model.incoming.length}</span>
              </div>
              <div class="insp-props">
                {#each p.model.incoming.slice(0, p.incomingLimit) as inc}
                  <div class="insp-prop insp-prop-in">
                    <span class="insp-val"><RdfTerm term={inc.s} /></span>
                    <span class="insp-pred insp-pred-in" title={inc.predicate}>{inc.predLabel}</span>
                  </div>
                {/each}
              </div>
              {#if p.model.incoming.length > p.incomingLimit}
                <button class="insp-more" on:click={() => bumpIncomingLimit(p.id)}>
                  {$t('pages.graphViz.inspectorMore', { values: { count: p.model.incoming.length - p.incomingLimit } })}
                </button>
              {/if}
            </div>
          {/if}
        </div>
      </div>
    {/each}
  {/if}

  <!-- Predicate / edge info panel — opens on edge click. Mirrors the node
       inspector's position/style and is mutually exclusive with it. Explains the
       predicate (vocabulary, description, inferred domain/range). -->
  {#if inspector && inspectedEdge}
    <EdgeInfoPanel
      edge={inspectedEdge}
      on:close={closeEdgeInspector}
      on:openPredicate={(e) => openPredicateResource(e.detail)}
    />
  {/if}
</div>

<style>
  .graph-wrapper {
    position: relative;
    border: 1px solid #e2e8f0;
    border-radius: 8px;
    overflow: hidden;
    background:
      radial-gradient(circle at 1px 1px, #e2e8f0 1px, transparent 0) 0 0 / 24px 24px,
      #f8fafc;
    outline: none;
  }

  .cy-container {
    width: 100%;
    height: 100%;
    background: transparent;
  }

  /* ── Controls ── */
  .graph-controls {
    position: absolute;
    top: 10px;
    right: 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    z-index: 10;
  }

  .ctrl-btn {
    width: 30px;
    height: 30px;
    border: 1px solid #e2e8f0;
    border-radius: 7px;
    background: rgba(255,255,255,0.96);
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    box-shadow: 0 1px 4px rgba(0,0,0,0.08);
    color: #475569;
    padding: 0;
    transition: background 0.15s, border-color 0.15s, color 0.15s;
  }

  .ctrl-btn:hover {
    background: #eff6ff;
    border-color: #3b82f6;
    color: #2563eb;
  }

  .ctrl-divider {
    height: 1px;
    background: #e2e8f0;
    margin: 2px 4px;
  }

  /* ── Minimap ── */
  .minimap-wrap {
    position: absolute;
    bottom: 36px;
    right: 10px;
    width: 182px;
    background: rgba(248,250,252,0.95);
    border: 1px solid #e2e8f0;
    border-radius: 6px;
    overflow: hidden;
    box-shadow: 0 2px 8px rgba(0,0,0,0.1);
    z-index: 10;
    transition: opacity 0.5s;
  }

  .minimap-wrap.minimap-dimmed {
    opacity: 0.35;
  }

  .minimap-wrap:hover {
    opacity: 1 !important;
  }

  .minimap-canvas {
    display: block;
    width: 180px;
    height: 120px;
    cursor: crosshair;
  }

  .minimap-label {
    position: absolute;
    top: 3px;
    left: 5px;
    font-size: 9px;
    font-weight: 600;
    color: #94a3b8;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    pointer-events: none;
  }

  /* ── Legend ── */
  .graph-legend {
    position: absolute;
    bottom: 8px;
    left: 10px;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    background: rgba(255,255,255,0.92);
    padding: 4px 8px;
    border-radius: 6px;
    border: 1px solid #e2e8f0;
    font-size: 10px;
    z-index: 10;
    box-shadow: 0 1px 3px rgba(0,0,0,0.06);
  }

  .legend-dot {
    display: inline-block;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    flex-shrink: 0;
  }

  .legend-txt {
    color: #475569;
    font-weight: 500;
    margin-right: 4px;
  }

  /* ── Tooltip ── */
  .node-tooltip {
    position: absolute;
    z-index: 100;
    background: #1e293b;
    color: #f1f5f9;
    border-radius: 7px;
    padding: 6px 10px;
    font-size: 11px;
    pointer-events: none;
    max-width: 300px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.25);
    transform: translate(-50%, -100%);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .tt-type {
    display: inline-block;
    border-radius: 3px;
    padding: 1px 5px;
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    align-self: flex-start;
  }

  .tt-type-uri     { background: #3b82f6; color: #fff; }
  .tt-type-literal { background: #16a34a; color: #fff; }
  .tt-type-bnode   { background: #71717a; color: #fff; }
  .tt-type-class   { background: #7c3aed; color: #fff; }
  .tt-type-instance{ background: #0369a1; color: #fff; }

  .tt-label {
    font-weight: 600;
    word-break: break-word;
    line-height: 1.3;
  }

  .tt-iri {
    font-family: 'IBM Plex Mono', monospace;
    font-size: 9px;
    color: #94a3b8;
    word-break: break-all;
  }

  /* Full literal value — bounded + scrollable so long/multi-line text stays readable. */
  .tt-value {
    font-weight: 600;
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.35;
    max-height: 9rem;
    overflow-y: auto;
  }

  .tt-meta {
    align-self: flex-start;
    font-family: 'IBM Plex Mono', monospace;
    font-size: 9px;
    color: #cbd5e1;
    background: rgba(255, 255, 255, 0.08);
    border-radius: 3px;
    padding: 1px 5px;
  }

  /* ── Spinner overlay ── */
  .node-spinner {
    position: absolute;
    pointer-events: none;
    z-index: 20;
    animation: node-spin 0.75s linear infinite;
  }

  @keyframes node-spin {
    from { transform: translate(-50%, -50%) rotate(0deg); }
    to   { transform: translate(-50%, -50%) rotate(360deg); }
  }

  /* ── Settings panel ── */
  .ctrl-active {
    background: #eff6ff !important;
    border-color: #3b82f6 !important;
    color: #2563eb !important;
  }

  .settings-panel {
    position: absolute;
    top: 10px;
    right: 50px; /* just left of the controls column */
    width: 230px;
    background: rgba(255,255,255,0.97);
    border: 1px solid #e2e8f0;
    border-radius: 10px;
    box-shadow: 0 4px 20px rgba(0,0,0,0.12);
    padding: 0;
    z-index: 20;
    overflow: hidden;
  }

  .settings-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 9px 12px 8px;
    border-bottom: 1px solid #f1f5f9;
    background: #f8fafc;
  }

  .settings-title {
    font-size: 0.75rem;
    font-weight: 700;
    color: #1e293b;
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .settings-close {
    width: 22px; height: 22px; padding: 0;
    border: none; background: transparent; cursor: pointer;
    color: #94a3b8; border-radius: 5px;
    display: flex; align-items: center; justify-content: center;
    transition: background 0.12s, color 0.12s;
  }
  .settings-close:hover { background: #fee2e2; color: #dc2626; }

  .setting-row {
    display: grid;
    grid-template-columns: 1fr auto;
    grid-template-rows: auto auto;
    column-gap: 6px;
    row-gap: 2px;
    padding: 10px 12px 8px;
    border-bottom: 1px solid #f8fafc;
    cursor: default;
  }

  .setting-label {
    font-size: 0.75rem;
    font-weight: 600;
    color: #475569;
    grid-column: 1;
    grid-row: 1;
    align-self: center;
  }

  .setting-val {
    font-size: 0.72rem;
    font-weight: 700;
    color: #3b82f6;
    font-variant-numeric: tabular-nums;
    grid-column: 2;
    grid-row: 1;
    text-align: right;
    align-self: center;
  }

  .setting-slider {
    grid-column: 1 / -1;
    grid-row: 2;
    -webkit-appearance: none;
    appearance: none;
    width: 100%;
    height: 4px;
    border-radius: 2px;
    background: #e2e8f0;
    outline: none;
    margin-top: 4px;
    cursor: pointer;
  }

  .setting-slider::-webkit-slider-thumb {
    -webkit-appearance: none;
    appearance: none;
    width: 14px; height: 14px;
    border-radius: 50%;
    background: #3b82f6;
    border: 2px solid #fff;
    box-shadow: 0 1px 4px rgba(59,130,246,0.35);
    cursor: pointer;
    transition: background 0.12s, transform 0.1s;
  }
  .setting-slider::-webkit-slider-thumb:hover { background: #2563eb; transform: scale(1.15); }
  .setting-slider::-moz-range-thumb {
    width: 14px; height: 14px;
    border-radius: 50%;
    background: #3b82f6;
    border: 2px solid #fff;
    cursor: pointer;
  }

  .setting-reset {
    display: block;
    width: calc(100% - 24px);
    margin: 8px 12px 10px;
    padding: 5px 0;
    font-size: 0.72rem;
    font-weight: 600;
    color: #64748b;
    background: #f1f5f9;
    border: 1px solid #e2e8f0;
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.12s, color 0.12s;
  }
  .setting-reset:hover { background: #e0e7ef; color: #334155; }

  /* ── Layout selector in settings ── */
  .setting-row-layout {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 12px 8px;
    border-bottom: 1px solid #f8fafc;
  }

  .layout-pills-row {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .layout-pill-btn {
    padding: 3px 8px;
    font-size: 0.7rem;
    font-weight: 600;
    border: 1px solid #e2e8f0;
    border-radius: 5px;
    background: #f8fafc;
    color: #475569;
    cursor: pointer;
    transition: background 0.12s, border-color 0.12s, color 0.12s;
    text-transform: capitalize;
  }

  .layout-pill-btn:hover {
    background: #eff6ff;
    border-color: #93c5fd;
    color: #1d4ed8;
  }

  .layout-pill-active {
    background: #eff6ff !important;
    border-color: #3b82f6 !important;
    color: #1d4ed8 !important;
  }

  /* ── Built-in search box ── */
  .graph-search-box {
    position: absolute;
    top: 10px;
    left: 10px;
    display: flex;
    align-items: center;
    gap: 5px;
    background: rgba(255,255,255,0.96);
    border: 1px solid #e2e8f0;
    border-radius: 8px;
    padding: 5px 8px;
    box-shadow: 0 1px 4px rgba(0,0,0,0.08);
    z-index: 10;
    width: 200px;
    transition: width 0.18s ease, padding 0.18s ease;
  }

  /* Minimised: just the search icon button. */
  .graph-search-box.collapsed {
    width: 30px;
    padding: 4px;
    cursor: pointer;
  }

  .graph-search-box:focus-within {
    border-color: #3b82f6;
    width: 240px;
  }

  .graph-search-icon-btn {
    display: inline-flex; align-items: center; justify-content: center;
    border: none; background: transparent; padding: 0; cursor: pointer;
    color: #64748b; flex-shrink: 0;
  }
  .graph-search-icon-btn:hover { color: #2563eb; }

  .graph-search-icon {
    color: inherit;
    flex-shrink: 0;
  }

  .graph-search-input {
    flex: 1;
    border: none;
    background: transparent;
    outline: none;
    font-size: 0.75rem;
    color: #1e293b;
    font-family: inherit;
    min-width: 0;
  }

  .graph-search-input::placeholder {
    color: #94a3b8;
  }

  .graph-search-clear {
    border: none;
    background: transparent;
    color: #94a3b8;
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0 1px;
    border-radius: 3px;
    transition: color 0.12s;
  }

  .graph-search-clear:hover { color: #475569; }

  /* ── Loading overlay ── */
  .graph-loading-overlay {
    position: absolute;
    inset: 0;
    background: rgba(248, 250, 252, 0.88);
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    z-index: 30;
    border-radius: inherit;
  }

  .graph-spinner-ring {
    width: 36px;
    height: 36px;
    border: 3px solid #e2e8f0;
    border-top-color: #3b82f6;
    border-radius: 50%;
    animation: spin-ring 0.8s linear infinite;
  }

  @keyframes spin-ring {
    to { transform: rotate(360deg); }
  }

  .graph-loading-text {
    font-size: 0.8rem;
    font-weight: 600;
    color: #64748b;
  }

  /* ── Load-more banner ── */
  .graph-loading-more-banner {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 28px;
    background: rgba(239, 246, 255, 0.95);
    border-bottom: 1px solid #bfdbfe;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    font-size: 0.73rem;
    font-weight: 600;
    color: #1d4ed8;
    z-index: 20;
  }

  .loading-more-dot {
    width: 8px;
    height: 8px;
    border: 2px solid #3b82f6;
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin-ring 0.7s linear infinite;
  }

  /* ── Dark theme: wrapper + all floating chrome hardcode light surfaces ── */
  :global(html.dark) .graph-wrapper {
    border-color: rgba(255, 255, 255, 0.1);
    background:
      radial-gradient(circle at 1px 1px, rgba(255, 255, 255, 0.07) 1px, transparent 0) 0 0 / 24px 24px,
      #0b1220;
  }
  :global(html.dark) .ctrl-btn {
    background: rgba(30, 41, 59, 0.92); border-color: rgba(255, 255, 255, 0.12); color: #cbd5e1;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.4);
  }
  :global(html.dark) .ctrl-btn:hover { background: rgba(59, 130, 246, 0.18); border-color: #3b82f6; color: #93c5fd; }
  :global(html.dark) .ctrl-divider { background: rgba(255, 255, 255, 0.12); }
  :global(html.dark) .ctrl-active { background: rgba(59, 130, 246, 0.18) !important; border-color: #3b82f6 !important; color: #93c5fd !important; }
  :global(html.dark) .minimap-wrap { background: rgba(15, 23, 42, 0.92); border-color: rgba(255, 255, 255, 0.12); box-shadow: 0 2px 8px rgba(0, 0, 0, 0.45); }
  :global(html.dark) .minimap-label { color: #94a3b8; }
  :global(html.dark) .graph-legend { background: rgba(15, 23, 42, 0.92); border-color: rgba(255, 255, 255, 0.12); box-shadow: 0 1px 3px rgba(0, 0, 0, 0.4); }
  :global(html.dark) .legend-txt { color: #cbd5e1; }
  :global(html.dark) .settings-panel { background: rgba(15, 23, 42, 0.97); border-color: rgba(255, 255, 255, 0.12); box-shadow: 0 4px 20px rgba(0, 0, 0, 0.5); }
  :global(html.dark) .settings-header { background: rgba(255, 255, 255, 0.03); border-bottom-color: rgba(255, 255, 255, 0.08); }
  :global(html.dark) .settings-title { color: #e2e8f0; }
  :global(html.dark) .setting-row { border-bottom-color: rgba(255, 255, 255, 0.06); }
  :global(html.dark) .setting-label { color: #cbd5e1; }
  :global(html.dark) .setting-slider { background: rgba(255, 255, 255, 0.14); }
  :global(html.dark) .setting-reset { background: rgba(255, 255, 255, 0.06); border-color: rgba(255, 255, 255, 0.12); color: #cbd5e1; }
  :global(html.dark) .setting-reset:hover { background: rgba(255, 255, 255, 0.1); color: #e2e8f0; }
  :global(html.dark) .setting-row-layout { border-bottom-color: rgba(255, 255, 255, 0.06); }
  :global(html.dark) .layout-pill-btn { background: rgba(255, 255, 255, 0.04); border-color: rgba(255, 255, 255, 0.12); color: #cbd5e1; }
  :global(html.dark) .layout-pill-btn:hover { background: rgba(59, 130, 246, 0.15); border-color: #3b82f6; color: #93c5fd; }
  :global(html.dark) .layout-pill-active { background: rgba(59, 130, 246, 0.2) !important; border-color: #3b82f6 !important; color: #93c5fd !important; }
  :global(html.dark) .graph-search-box { background: rgba(30, 41, 59, 0.95); border-color: rgba(255, 255, 255, 0.12); box-shadow: 0 1px 4px rgba(0, 0, 0, 0.4); }
  :global(html.dark) .graph-search-input { color: #e2e8f0; }
  :global(html.dark) .graph-search-icon-btn { color: #94a3b8; }
  :global(html.dark) .graph-loading-overlay { background: rgba(11, 18, 32, 0.85); }
  :global(html.dark) .graph-spinner-ring { border-color: rgba(255, 255, 255, 0.14); border-top-color: #3b82f6; }
  :global(html.dark) .graph-loading-more-banner { background: rgba(30, 58, 138, 0.3); border-bottom-color: rgba(59, 130, 246, 0.4); color: #bfdbfe; }

  /* ── Tooltip: plain-language explanation of the node kind ── */
  .tt-explain {
    font-size: 10px;
    line-height: 1.35;
    color: #cbd5e1;
    border-top: 1px solid rgba(255, 255, 255, 0.12);
    padding-top: 4px;
    margin-top: 2px;
    max-width: 260px;
  }
  .tt-expand {
    font-size: 10px;
    font-weight: 600;
    color: #93c5fd;
    margin-top: 3px;
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .tt-expand::before { content: '＋'; font-weight: 700; }

  /* ── Node inspector panel ── */
  .inspector-panel {
    position: absolute;
    top: 52px;
    left: 10px;
    width: 320px;
    max-width: calc(100% - 80px);
    max-height: calc(100% - 110px);
    display: flex;
    flex-direction: column;
    background: rgba(255, 255, 255, 0.98);
    border: 1px solid #e2e8f0;
    border-radius: 10px;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.16);
    z-index: 25;
    overflow: hidden;
  }

  .insp-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 8px 8px 8px 12px;
    background: #f8fafc;
    border-bottom: 1px solid #eef2f7;
    flex-shrink: 0;
  }
  .insp-draggable { cursor: move; user-select: none; touch-action: none; }

  .insp-type {
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    padding: 2px 7px;
    border-radius: 4px;
    color: #fff;
  }
  .insp-type-uri { background: #3b82f6; }
  .insp-type-literal { background: #16a34a; }
  .insp-type-bnode { background: #71717a; }

  .insp-close {
    width: 24px; height: 24px; padding: 0;
    border: none; background: transparent; cursor: pointer;
    color: #94a3b8; border-radius: 6px;
    display: flex; align-items: center; justify-content: center;
    transition: background 0.12s, color 0.12s;
  }
  .insp-close:hover { background: #fee2e2; color: #dc2626; }

  .insp-body {
    padding: 10px 12px 12px;
    overflow-y: auto;
  }

  .insp-title {
    margin: 0;
    font-size: 0.92rem;
    font-weight: 700;
    color: #1e293b;
    word-break: break-word;
    line-height: 1.3;
  }

  .insp-literal {
    font-size: 0.85rem;
    font-weight: 600;
    color: #14532d;
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 8rem;
    overflow-y: auto;
    background: #f0fdf4;
    border: 1px solid #dcfce7;
    border-radius: 6px;
    padding: 6px 8px;
  }

  .insp-explain {
    margin: 6px 0 0;
    font-size: 0.72rem;
    line-height: 1.45;
    color: #64748b;
  }

  .insp-iri-row { display: flex; align-items: center; gap: 4px; margin-top: 8px; }
  .insp-iri {
    flex: 1; min-width: 0;
    font-family: 'IBM Plex Mono', monospace;
    font-size: 0.68rem;
    color: #475569;
    background: #f1f5f9;
    border-radius: 5px;
    padding: 4px 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .insp-mini-btn {
    flex-shrink: 0;
    width: 24px; height: 24px; padding: 0;
    border: 1px solid #e2e8f0;
    background: #fff;
    color: #64748b;
    border-radius: 5px;
    cursor: pointer;
    display: inline-flex; align-items: center; justify-content: center;
    transition: background 0.12s, border-color 0.12s, color 0.12s;
  }
  .insp-mini-btn:hover { background: #eff6ff; border-color: #93c5fd; color: #2563eb; }

  .insp-open-btn {
    display: inline-flex; align-items: center; gap: 5px;
    margin-top: 8px;
    padding: 5px 10px;
    font-size: 0.74rem; font-weight: 600;
    color: #1d4ed8;
    background: #eff6ff;
    border: 1px solid #bfdbfe;
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.12s, border-color 0.12s;
  }
  .insp-open-btn:hover { background: #dbeafe; border-color: #60a5fa; }

  .insp-meta-row { display: flex; flex-wrap: wrap; align-items: center; gap: 5px; margin-top: 8px; }
  .insp-chip {
    font-size: 0.68rem; font-weight: 600;
    padding: 1px 7px; border-radius: 8px;
    background: #e2e8f0; color: #475569;
  }
  .insp-chip.mono { font-family: 'IBM Plex Mono', monospace; }
  .insp-chip.type { background: #ede9fe; color: #6d28d9; }

  .insp-section { margin-top: 13px; }
  .insp-section-head {
    display: flex; align-items: center; gap: 5px;
    font-size: 0.7rem; font-weight: 700;
    text-transform: uppercase; letter-spacing: 0.4px;
    color: #94a3b8;
    margin-bottom: 7px;
  }
  .insp-count {
    font-size: 0.64rem; font-weight: 700;
    color: #64748b; background: #f1f5f9;
    border-radius: 7px; padding: 0 5px;
  }

  .insp-props { display: flex; flex-direction: column; gap: 8px; }
  .insp-prop {
    display: flex; flex-direction: column; gap: 3px;
    padding-bottom: 8px;
    border-bottom: 1px solid #f1f5f9;
  }
  .insp-prop:last-child { border-bottom: none; padding-bottom: 0; }
  .insp-prop-in { flex-direction: row; align-items: baseline; flex-wrap: wrap; gap: 6px; }
  .insp-pred {
    font-size: 0.72rem; font-weight: 600;
    color: #6a5acd; word-break: break-word;
  }
  .insp-pred-in { color: #0369a1; font-weight: 500; }
  .insp-val { font-size: 0.8rem; color: #1e293b; min-width: 0; }
  .insp-empty { font-size: 0.74rem; color: #94a3b8; margin: 0; }
  .insp-hint { font-size: 0.7rem; color: #b4bdca; margin: 5px 0 0; }
  .insp-more {
    margin-top: 7px;
    font-size: 0.72rem; font-weight: 600;
    color: #2563eb; background: none; border: none;
    cursor: pointer; padding: 2px 0;
  }
  .insp-more:hover { text-decoration: underline; }

  /* Inspector — dark */
  :global(html.dark) .inspector-panel { background: rgba(15, 23, 42, 0.98); border-color: rgba(255, 255, 255, 0.12); box-shadow: 0 6px 24px rgba(0, 0, 0, 0.55); }
  :global(html.dark) .insp-header { background: rgba(255, 255, 255, 0.03); border-bottom-color: rgba(255, 255, 255, 0.08); }
  :global(html.dark) .insp-close:hover { background: rgba(220, 38, 38, 0.2); color: #fca5a5; }
  :global(html.dark) .insp-title { color: #e2e8f0; }
  :global(html.dark) .insp-literal { background: rgba(22, 163, 74, 0.12); border-color: rgba(34, 197, 94, 0.3); color: #86efac; }
  :global(html.dark) .insp-explain { color: #94a3b8; }
  :global(html.dark) .insp-iri { background: #1e293b; color: #cbd5e1; }
  :global(html.dark) .insp-mini-btn { background: #1e293b; border-color: rgba(255, 255, 255, 0.12); color: #cbd5e1; }
  :global(html.dark) .insp-mini-btn:hover { background: rgba(59, 130, 246, 0.18); border-color: #3b82f6; color: #93c5fd; }
  :global(html.dark) .insp-open-btn { background: rgba(59, 130, 246, 0.16); border-color: rgba(59, 130, 246, 0.4); color: #93c5fd; }
  :global(html.dark) .insp-open-btn:hover { background: rgba(59, 130, 246, 0.26); }
  :global(html.dark) .insp-chip { background: #1e293b; color: #cbd5e1; }
  :global(html.dark) .insp-chip.type { background: #3b2f63; color: #c4b5fd; }
  :global(html.dark) .insp-count { background: #1e293b; color: #94a3b8; }
  :global(html.dark) .insp-section-head { color: #64748b; }
  :global(html.dark) .insp-prop { border-bottom-color: rgba(255, 255, 255, 0.06); }
  :global(html.dark) .insp-pred { color: #c4b5fd; }
  :global(html.dark) .insp-pred-in { color: #7dd3fc; }
  :global(html.dark) .insp-val { color: #e2e8f0; }
  :global(html.dark) .insp-empty { color: #64748b; }
  :global(html.dark) .insp-hint { color: #475569; }
</style>
