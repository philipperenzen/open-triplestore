<script>
  import { createEventDispatcher } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { detectValueKind, shortenIri, parseWktGeometry, geometryCoords, datatypeLabel } from '../../lib/ontology/valueType.js';
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import { ExternalLink, Play, Copy, Check, Image as ImageIcon, ChevronRight, ChevronDown, Braces, MapPin } from 'lucide-svelte';
  import RdfTerm from '../RdfTerm.svelte';
  import { sanitizeHtml } from '../../lib/ontology/sanitizeHtml.js';

  /** SPARQL-JSON-style binding: { type, value, datatype?, 'xml:lang'? } */
  export let term;
  export let predicate = '';
  export let compact = false;
  /** Map of blank-node id → [{p, o}] so blank node values can expand inline.
   * @type {Record<string, Array<{p?: any, o?: any}>> | null} */
  export let bnodes = null;
  /** Recursion depth guard for nested blank nodes. */
  export let depth = 0;

  const MAX_DEPTH = 6;
  const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
  const dispatch = createEventDispatcher();

  let expanded = false;
  let copied = false;
  // Auto-expand only the first level of blank nodes, and never in compact mode.
  let bnodeOpen = depth < 1 && !compact;

  $: detection = detectValueKind(
    term && { type: term.type, value: term.value, datatype: term.datatype, lang: term['xml:lang'] || term.lang },
    predicate,
  );

  $: bnodeRows = (term?.type === 'bnode' && bnodes && Array.isArray(bnodes[term.value]) && bnodes[term.value].length)
    ? bnodes[term.value]
    : null;
  $: bnodeTypeLabel = (() => {
    const t = bnodeRows?.find(r => r.p?.value === RDF_TYPE && (r.o?.type === 'uri' || r.o?.type === 'iri'));
    return t ? shortenIRI(t.o.value) : '';
  })();

  $: geom = detection.kind === 'geo' && detection.format === 'wkt' ? parseWktGeometry(term?.value || '') : null;
  $: geoCoords = geom ? geometryCoords(geom) : [];
  $: geoRep = geoCoords.length ? geoCoords[0] : null; // [lng, lat]

  $: dtLabel = detection.datatype ? datatypeLabel(/** @type {string} */ (detection.datatype)) : '';
  $: dateFormatted = detection.kind === 'date' ? formatDate(term?.value) : '';
  $: numberFormatted = detection.kind === 'number' ? formatNum(term?.value) : '';

  function copy() {
    navigator.clipboard?.writeText(String(term?.value ?? ''));
    copied = true;
    setTimeout(() => (copied = false), 1200);
  }

  function runSparql() {
    dispatch('run-sparql', { query: term?.value || '' });
  }

  function formatDate(v) {
    const s = String(v ?? '');
    // Partial dates (gYear, gYearMonth, gMonthDay…) — show verbatim; `new Date`
    // would over-interpret them into a misleading full timestamp.
    if (/^-?\d{4}$/.test(s) || /^-?\d{4}-\d{2}$/.test(s) || s.startsWith('--')) return s;
    if (/^\d{2}:\d{2}/.test(s)) return s; // time only
    try {
      const d = new Date(s);
      if (isNaN(d.getTime())) return s;
      if (/^\d{4}-\d{2}-\d{2}$/.test(s)) return d.toLocaleDateString();
      return d.toLocaleString();
    } catch { return s; }
  }
  function formatNum(v) {
    const n = Number(v);
    return Number.isFinite(n) ? n.toLocaleString() : String(v);
  }

  // rdf:HTML literals are attacker-controllable, so they are sanitized with
  // DOMPurify (see lib/ontology/sanitizeHtml.js) before being injected via {@html}.
  const sanitize = sanitizeHtml;
</script>

{#if !term}
  <span class="muted">—</span>
{:else if term.type === 'bnode'}
  {#if depth >= MAX_DEPTH || !bnodeRows}
    <span class="chip bnode-chip" title={$i18nT('components.valueRenderer.blankNodeTitle', { values: { id: term.value } })}>
      <Braces size={11} /> {bnodeRows ? $i18nT('components.valueRenderer.propCount', { values: { count: bnodeRows.length } }) : $i18nT('components.valueRenderer.node')}
    </span>
  {:else}
    <div class="bnode">
      <button class="bnode-toggle" type="button" on:click={() => (bnodeOpen = !bnodeOpen)}>
        {#if bnodeOpen}<ChevronDown size={12} />{:else}<ChevronRight size={12} />{/if}
        <Braces size={11} />
        <span class="bnode-count">{bnodeRows.length === 1 ? $i18nT('components.valueRenderer.propertyCountSingular', { values: { count: bnodeRows.length } }) : $i18nT('components.valueRenderer.propertyCountPlural', { values: { count: bnodeRows.length } })}</span>
        {#if bnodeTypeLabel}<span class="bnode-type">{bnodeTypeLabel}</span>{/if}
      </button>
      {#if bnodeOpen}
        <div class="bnode-body">
          {#each bnodeRows as r}
            <div class="bnode-row">
              <span class="bnode-pred" title={r.p?.value}>{shortenIRI(r.p?.value || '')}</span>
              <span class="bnode-val">
                <svelte:self term={r.o} predicate={r.p?.value || ''} {bnodes} depth={depth + 1} {compact} on:run-sparql />
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
{:else if detection.kind === 'image'}
  <div class="img-wrap">
    <a href={term.value} target="_blank" rel="noopener">
      <img src={term.value} alt="" loading="lazy" on:error={(e) => { /** @type {HTMLImageElement} */ (e.currentTarget).style.display='none'; }} />
    </a>
    <a class="img-link" href={term.value} target="_blank" rel="noopener" title={term.value}>
      <ImageIcon size={11} /> {shortenIri(term.value)}
    </a>
  </div>
{:else if detection.kind === 'geo' && geom}
  <span class="geo">
    {#if geom.kind === 'point' && geoRep}
      <span class="chip geo-chip" title={term.value}>
        <MapPin size={11} /> {geoRep[1].toFixed(4)}, {geoRep[0].toFixed(4)}
      </span>
      <a href="https://www.openstreetmap.org/?mlat={geoRep[1]}&mlon={geoRep[0]}&zoom=14"
         target="_blank" rel="noopener" class="ext-link" title={$i18nT('components.valueRenderer.openInOpenStreetMap')}>
        <ExternalLink size={11} />
      </a>
    {:else}
      <span class="chip geo-chip" title={term.value}>
        <MapPin size={11} /> {geom.kind} · {$i18nT('components.valueRenderer.pointCount', { values: { count: geoCoords.length } })}
      </span>
      {#if geoRep}
        <a href="https://www.openstreetmap.org/?mlat={geoRep[1]}&mlon={geoRep[0]}&zoom=10"
           target="_blank" rel="noopener" class="ext-link" title={$i18nT('components.valueRenderer.openNearGeometry')}>
          <ExternalLink size={11} />
        </a>
      {/if}
    {/if}
  </span>
{:else if detection.kind === 'geo'}
  <span class="chip geo-chip" title={term.value}>
    <MapPin size={11} /> {detection.format === 'gml' ? $i18nT('components.valueRenderer.gmlGeometry') : $i18nT('components.valueRenderer.geometry')}
  </span>
{:else if detection.kind === 'sparql'}
  <div class="sparql">
    <div class="sparql-header">
      <span class="chip">SPARQL</span>
      <button class="btn-icon" on:click={runSparql} title={$i18nT('components.valueRenderer.openInSparqlEditor')}>
        <Play size={12} /> {$i18nT('components.valueRenderer.run')}
      </button>
      <button class="btn-icon" on:click={copy} title={$i18nT('system.copy')}>
        {#if copied}<Check size={12} />{:else}<Copy size={12} />{/if}
      </button>
    </div>
    <pre class="code">{term.value}</pre>
  </div>
{:else if detection.kind === 'html'}
  <!-- value is passed through sanitize() (strips <script> and on* handlers) before rendering -->
  <!-- eslint-disable-next-line svelte/no-at-html-tags -->
  <div class="html">{@html sanitize(term.value)}</div>
{:else if detection.kind === 'bool'}
  <span class="bool" class:t={term.value === 'true'}>{term.value === 'true' ? $i18nT('components.valueRenderer.boolTrue') : $i18nT('components.valueRenderer.boolFalse')}</span>
{:else if detection.kind === 'date'}
  <span title={term.value}>{dateFormatted}</span>{#if dtLabel}<span class="dt-chip" title={String(detection.datatype ?? '')}>{dtLabel}</span>{/if}
{:else if detection.kind === 'duration'}
  <span class="duration" title={term.value}>⏱ {term.value}</span>{#if dtLabel && dtLabel !== 'xsd:duration'}<span class="dt-chip" title={String(detection.datatype ?? '')}>{dtLabel}</span>{/if}
{:else if detection.kind === 'number'}
  <span class="number">{numberFormatted}</span>{#if dtLabel && dtLabel !== 'xsd:integer' && dtLabel !== 'xsd:decimal' && dtLabel !== 'xsd:double'}<span class="dt-chip" title={String(detection.datatype ?? '')}>{dtLabel}</span>{/if}
{:else if detection.kind === 'binary'}
  <span class="binary">
    <span class="chip">{dtLabel || $i18nT('components.valueRenderer.binary')}</span>
    <code class="binary-preview" title={term.value}>{(term.value || '').slice(0, 24)}{(term.value || '').length > 24 ? '…' : ''}</code>
  </span>
{:else if detection.kind === 'color'}
  <span class="color-swatch">
    <span class="swatch" style="background: {term.value}"></span>
    <code>{term.value}</code>
  </span>
{:else if detection.kind === 'url' || detection.kind === 'iri'}
  <RdfTerm term={{ type: term.type === 'literal' ? 'uri' : term.type, value: term.value }} />
{:else if detection.kind === 'lang'}
  <span class="lang">
    <span class="lang-badge">{detection.lang}</span>
    <span class="text">{term.value}</span>
  </span>
{:else if detection.kind === 'text' && detection.long && !expanded && !compact}
  <div class="long">
    <div class="long-preview">{term.value.slice(0, 200)}…</div>
    <button class="btn-link" on:click={() => expanded = true}>{$i18nT('components.valueRenderer.showMore')}</button>
  </div>
{:else}
  <span class="plain">{term.value}</span>{#if dtLabel}<span class="dt-chip" title={String(detection.datatype ?? '')}>{dtLabel}</span>{/if}
  {#if (term.value || '').length > 30}
    <button class="btn-icon inline" on:click={copy} title={$i18nT('system.copy')}>
      {#if copied}<Check size={11} />{:else}<Copy size={11} />{/if}
    </button>
  {/if}
{/if}

<style>
  .muted { color: #9ca3af; }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 1px 6px;
    border-radius: 10px;
    font-size: 10px;
    background: #e0e7ff;
    color: #3730a3;
    font-weight: 600;
  }
  .geo-chip { background: #d1fae5; color: #065f46; }
  .img-wrap {
    display: inline-flex;
    flex-direction: column;
    gap: 3px;
    max-width: 160px;
  }
  .img-wrap img {
    max-width: 160px;
    max-height: 120px;
    object-fit: contain;
    border: 1px solid #e5e7eb;
    border-radius: 4px;
    background: #f9fafb;
  }
  .img-link { font-size: 10px; color: #6b7280; display: inline-flex; align-items: center; gap: 3px; }
  .geo { display: inline-flex; align-items: center; gap: 4px; }
  .ext-link { color: #6b7280; }
  .sparql {
    display: inline-block;
    border: 1px solid #e5e7eb;
    border-radius: 4px;
    background: #fafafa;
    max-width: 420px;
    width: 100%;
  }
  .sparql-header {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 6px;
    border-bottom: 1px solid #e5e7eb;
    background: #f3f4f6;
  }
  .sparql pre.code {
    margin: 0;
    padding: 6px 8px;
    font-size: 11px;
    font-family: 'SF Mono', 'Fira Code', monospace;
    white-space: pre-wrap;
    max-height: 140px;
    overflow: auto;
    color: #1f2937;
  }
  .bool { font-weight: 600; color: #b91c1c; }
  .bool.t { color: #166534; }
  .number { font-variant-numeric: tabular-nums; color: #1e40af; }
  .duration { font-variant-numeric: tabular-nums; color: #6d28d9; }
  .binary { display: inline-flex; align-items: center; gap: 5px; }
  .binary-preview { font-size: 11px; color: #6b7280; font-family: 'SF Mono', 'Fira Code', monospace; }
  .color-swatch { display: inline-flex; align-items: center; gap: 4px; }
  .swatch { display: inline-block; width: 14px; height: 14px; border-radius: 3px; border: 1px solid #d1d5db; }
  .lang-badge {
    font-size: 9px;
    padding: 1px 4px;
    background: #ede9fe;
    color: #5b21b6;
    border-radius: 8px;
    margin-right: 4px;
    font-weight: 600;
    text-transform: uppercase;
  }
  .long-preview { white-space: pre-wrap; }
  .btn-link {
    background: none;
    border: none;
    color: #2563eb;
    cursor: pointer;
    font-size: 11px;
    padding: 2px 0;
  }
  .btn-icon {
    background: transparent;
    border: 1px solid transparent;
    color: #4b5563;
    padding: 2px 4px;
    cursor: pointer;
    border-radius: 3px;
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-size: 11px;
  }
  .btn-icon:hover { background: #e5e7eb; }
  .btn-icon.inline { margin-left: 4px; }
  .html { max-width: 100%; overflow: auto; }

  /* Datatype badge — subtle, surfaces the literal's declared type. */
  .dt-chip {
    margin-left: 5px;
    font-size: 9px;
    padding: 0 5px;
    border-radius: 7px;
    background: #f1f5f9;
    color: #64748b;
    font-family: 'SF Mono', 'Fira Code', monospace;
    vertical-align: middle;
    white-space: nowrap;
  }

  /* Blank node — inline expandable nested properties. */
  .bnode-chip { background: #f3f4f6; color: #6b7280; }
  .bnode { display: inline-flex; flex-direction: column; gap: 3px; max-width: 100%; }
  .bnode-toggle {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    align-self: flex-start;
    padding: 1px 8px 1px 4px;
    border: 1px solid #e2e8f0;
    border-radius: 10px;
    background: #f8fafc;
    color: #475569;
    cursor: pointer;
    font-size: 11px;
  }
  .bnode-toggle:hover { background: #eef2f7; border-color: #cbd5e1; }
  .bnode-count { font-weight: 600; }
  .bnode-type {
    font-family: 'SF Mono', 'Fira Code', monospace;
    font-size: 10px;
    color: #6a5acd;
    background: #f1effb;
    border-radius: 6px;
    padding: 0 5px;
  }
  .bnode-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-left: 7px;
    padding-left: 10px;
    border-left: 2px solid #e6e9f0;
  }
  .bnode-row { display: flex; align-items: baseline; gap: 8px; flex-wrap: wrap; }
  .bnode-pred {
    color: #6a5acd;
    font-size: 0.78rem;
    font-weight: 500;
    white-space: nowrap;
  }
  .bnode-val { min-width: 0; }

  /* ── Dark mode ──────────────────────────────────────────────────────────
     This renderer is hardcoded light above; retint chips, code, bnodes and
     value badges so resource-detail values stay legible on dark surfaces. */
  :global(html.dark) .muted { color: #64748b; }
  :global(html.dark) .chip { background: #312e81; color: #c7d2fe; }
  :global(html.dark) .geo-chip { background: #064e3b; color: #6ee7b7; }
  :global(html.dark) .img-wrap img { border-color: #334155; background: #0f172a; }
  :global(html.dark) .img-link { color: #94a3b8; }
  :global(html.dark) .ext-link { color: #94a3b8; }
  :global(html.dark) .sparql { border-color: #334155; background: #111827; }
  :global(html.dark) .sparql-header { border-bottom-color: #334155; background: #1e293b; }
  :global(html.dark) .sparql pre.code { color: #e2e8f0; }
  :global(html.dark) .bool { color: #fca5a5; }
  :global(html.dark) .bool.t { color: #86efac; }
  :global(html.dark) .number { color: #93c5fd; }
  :global(html.dark) .duration { color: #c4b5fd; }
  :global(html.dark) .binary-preview { color: #94a3b8; }
  :global(html.dark) .swatch { border-color: #475569; }
  :global(html.dark) .lang-badge { background: #3b2f63; color: #c4b5fd; }
  :global(html.dark) .btn-link { color: #7db4f0; }
  :global(html.dark) .btn-icon { color: #cbd5e1; }
  :global(html.dark) .btn-icon:hover { background: #1e293b; }
  :global(html.dark) .dt-chip { background: #1e293b; color: #94a3b8; }
  :global(html.dark) .bnode-chip { background: #1e293b; color: #94a3b8; }
  :global(html.dark) .bnode-toggle {
    border-color: #334155; background: #1e293b; color: #cbd5e1;
  }
  :global(html.dark) .bnode-toggle:hover { background: #283549; border-color: #475569; }
  :global(html.dark) .bnode-type { color: #c4b5fd; background: #3b2f63; }
  :global(html.dark) .bnode-body { border-left-color: #334155; }
  :global(html.dark) .bnode-pred { color: #c4b5fd; }
</style>
