<script>
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import { prefixForNamespace, lookupNamespacePrefix } from '../../lib/ontology/prefixService.js';
  import { Shapes, Tag, Library, Database, ChevronDown, ChevronRight, X, PanelLeftClose, PanelLeftOpen } from 'lucide-svelte';

  // { classes:[{iri,count}], properties:[{iri,count}], graphs:[{iri,count,role,roleLabel}] }
  export let facets = { classes: [], properties: [], graphs: [] };
  export let loading = false;
  export let uiMode = 'simple';
  export let chips = [];            // active chips, to mark facets as selected
  export let collapsed = false;

  const dispatch = createEventDispatcher();

  let railSearch = '';
  let open = { classes: true, properties: true, vocabularies: false, graphs: true };
  function toggle(k) { open = { ...open, [k]: !open[k] }; }

  function nsOf(iri) {
    const h = iri.lastIndexOf('#'); const s = iri.lastIndexOf('/');
    return iri.slice(0, Math.max(h, s) + 1) || iri;
  }

  // Per-namespace colour, same hash scheme as the predicate column, so a
  // vocabulary's colour matches the predicates that belong to it.
  function strHue(str) {
    let h = 0;
    for (let i = 0; i < str.length; i++) h = (h * 31 + str.charCodeAt(i)) & 0xffffffff;
    return Math.abs(h) % 360;
  }
  const nsColor = (ns) => `hsl(${strHue(ns)},52%,38%)`;
  const nsBg = (ns) => `hsl(${strHue(ns)},65%,94%)`;

  // Extra prefix bases (beyond the prefix-lookup service) for namespaces common
  // to this project. Matched by prefix of the namespace IRI.
  const PREFIX_BASES = [
    { prefix: 'ex',  bases: ['https://example.org/', 'http://example.org/'] },
    { prefix: 'ots', bases: ['https://opentriplestore.org/', 'http://opentriplestore.org/'] },
  ];
  function basePrefix(ns) {
    for (const { prefix, bases } of PREFIX_BASES) {
      if (bases.some((b) => ns.startsWith(b))) return prefix;
    }
    return null;
  }
  // Short label from a namespace's last non-empty path/fragment segment.
  function deriveLabel(ns) {
    const seg = ns.replace(/[#/]+$/, '').split(/[#/]/).filter(Boolean).pop();
    return seg || ns;
  }
  // Best prefix for a namespace (no trailing colon): the prefix-lookup service,
  // then our extra bases, then a derived short label.
  function rawPrefix(ns) {
    return prefixForNamespace(ns) || basePrefix(ns) || deriveLabel(ns);
  }

  // Vocabularies = namespaces the present classes+properties belong to, with the
  // number of distinct terms each contributes — a friendly "which vocabulary" lens.
  $: vocabularies = (() => {
    const m = new Map(); // ns -> { ns, terms, count }
    for (const c of [...(facets.classes || []), ...(facets.properties || [])]) {
      if (!c.iri) continue;
      const ns = nsOf(c.iri);
      const cur = m.get(ns) || { ns, terms: 0, count: 0 };
      cur.terms += 1; cur.count += c.count || 0;
      m.set(ns, cur);
    }
    return [...m.values()].sort((a, b) => b.terms - a.terms);
  })();

  // Prefix labels (no trailing colon). When two DIFFERENT namespaces resolve to
  // the same prefix, disambiguate with 1/2/3…. Unknown namespaces are warmed from
  // prefix.cc in the background and re-rendered when they resolve.
  let _prefixTick = 0;
  $: vocabLabels = (() => {
    void _prefixTick;
    const raw = vocabularies.map((v) => ({ ns: v.ns, p: rawPrefix(v.ns) }));
    const groups = {};
    for (const r of raw) (groups[r.p] ||= []).push(r.ns);
    const out = {};
    for (const r of raw) {
      const arr = groups[r.p];
      out[r.ns] = arr.length > 1 ? `${r.p}${arr.indexOf(r.ns) + 1}` : r.p;
    }
    return out;
  })();
  $: {
    void _prefixTick;
    for (const v of vocabularies) {
      if (!prefixForNamespace(v.ns) && !basePrefix(v.ns)) {
        lookupNamespacePrefix(v.ns).then((p) => { if (p) _prefixTick += 1; });
      }
    }
  }

  function match(label, iri) {
    if (!railSearch) return true;
    const q = railSearch.toLowerCase();
    return (label || '').toLowerCase().includes(q) || (iri || '').toLowerCase().includes(q);
  }
  $: fClasses = (facets.classes || []).filter((c) => match(shortenIRI(c.iri), c.iri));
  $: fProps = (facets.properties || []).filter((c) => match(shortenIRI(c.iri), c.iri));
  $: fVocab = vocabularies.filter((v) => match(vocabLabels[v.ns], v.ns));
  $: fGraphs = (facets.graphs || []).filter((g) => match(shortenIRI(g.iri), g.iri));

  // Active-state lookups against current chips.
  $: classSel = new Set(chips.filter((c) => c.field === 'object' && c.mode === 'exact').map((c) => c.value));
  $: propSel = new Set(chips.filter((c) => c.field === 'predicate' && c.mode === 'exact').map((c) => c.value));
  $: graphSel = new Set(chips.filter((c) => c.field === 'graph' && c.mode === 'exact').map((c) => c.value));
  $: vocabSel = new Set(chips.filter((c) => c.field === 'vocabulary').map((c) => c.value));

  const pickClass = (iri) => dispatch('addchips', [{ field: 'object', value: iri, mode: 'exact' }]);
  const pickProp = (iri) => dispatch('addchips', [{ field: 'predicate', value: iri, mode: 'exact' }]);
  const pickVocab = (ns) => dispatch('addchips', [{ field: 'vocabulary', value: ns, mode: 'exact' }]);
  const pickGraph = (iri) => dispatch('addchips', [{ field: 'graph', value: iri, mode: 'exact' }]);

  const roleClass = (r) => `role-${(r || 'other')}`;
</script>

{#if collapsed}
  <button class="rail-reopen" on:click={() => (collapsed = false)} title={$t('components.facetRail.showFacets')}>
    <PanelLeftOpen size={16} />
  </button>
{:else}
  <aside class="rail">
    <div class="rail-head">
      <span class="rail-title">{uiMode === 'simple' ? $t('components.facetRail.whatsInHere') : $t('components.facetRail.facetsInScope')}</span>
      <button class="rail-collapse" on:click={() => (collapsed = true)} title={$t('components.facetRail.hideFacets')}><PanelLeftClose size={15} /></button>
    </div>

    <div class="rail-search-wrap">
      <input class="rail-search" placeholder={$t('components.facetRail.filterFacets')} bind:value={railSearch} />
      {#if railSearch}<button class="rail-search-x" on:click={() => (railSearch = '')}><X size={12} /></button>{/if}
    </div>

    {#if loading}
      <p class="rail-msg">{$t('components.facetRail.scanningScope')}</p>
    {:else if (facets.classes?.length || 0) + (facets.properties?.length || 0) + (facets.graphs?.length || 0) === 0}
      <p class="rail-msg">{$t('components.facetRail.noDataInScope')}</p>
    {:else}
      <div class="rail-body">
        <!-- Classes -->
        <section class="fsec">
          <button class="fsec-head" on:click={() => toggle('classes')}>
            {#if open.classes}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
            <Shapes size={13} /> <span class="fsec-name">{$t('components.facetRail.classes')}</span>
            <span class="fsec-count">{fClasses.length}</span>
          </button>
          {#if open.classes}
            <ul class="fsec-list">
              {#each fClasses as c}
                <li><button class="fitem" class:fitem-on={classSel.has(c.iri)} title={c.iri} on:click={() => pickClass(c.iri)}>
                  <span class="fitem-name">{shortenIRI(c.iri)}</span><span class="fitem-count">{c.count.toLocaleString()}</span>
                </button></li>
              {/each}
              {#if fClasses.length === 0}<li class="fitem-empty">{$t('components.facetRail.noMatches')}</li>{/if}
            </ul>
          {/if}
        </section>

        <!-- Properties -->
        <section class="fsec">
          <button class="fsec-head" on:click={() => toggle('properties')}>
            {#if open.properties}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
            <Tag size={13} /> <span class="fsec-name">{$t('components.facetRail.properties')}</span>
            <span class="fsec-count">{fProps.length}</span>
          </button>
          {#if open.properties}
            <ul class="fsec-list">
              {#each fProps as c}
                <li><button class="fitem" class:fitem-on={propSel.has(c.iri)} title={c.iri} on:click={() => pickProp(c.iri)}>
                  <span class="fitem-name">{shortenIRI(c.iri)}</span><span class="fitem-count">{c.count.toLocaleString()}</span>
                </button></li>
              {/each}
              {#if fProps.length === 0}<li class="fitem-empty">{$t('components.facetRail.noMatches')}</li>{/if}
            </ul>
          {/if}
        </section>

        <!-- Vocabularies -->
        <section class="fsec">
          <button class="fsec-head" on:click={() => toggle('vocabularies')}>
            {#if open.vocabularies}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
            <Library size={13} /> <span class="fsec-name">{$t('components.facetRail.vocabularies')}</span>
            <span class="fsec-count">{fVocab.length}</span>
          </button>
          {#if open.vocabularies}
            <ul class="fsec-list">
              {#each fVocab as v}
                <li><button class="fitem" class:fitem-on={vocabSel.has(v.ns)} title={v.ns} on:click={() => pickVocab(v.ns)}>
                  <span class="vocab-pill" style="color:{nsColor(v.ns)};background:{nsBg(v.ns)}">{vocabLabels[v.ns]}</span>
                  <span class="fitem-count">{$t('components.facetRail.termsCount', { values: { count: v.terms } })}</span>
                </button></li>
              {/each}
              {#if fVocab.length === 0}<li class="fitem-empty">{$t('components.facetRail.noMatches')}</li>{/if}
            </ul>
          {/if}
        </section>

        <!-- Graphs -->
        <section class="fsec">
          <button class="fsec-head" on:click={() => toggle('graphs')}>
            {#if open.graphs}<ChevronDown size={13} />{:else}<ChevronRight size={13} />{/if}
            <Database size={13} /> <span class="fsec-name">{$t('components.facetRail.graphs')}</span>
            <span class="fsec-count">{fGraphs.length}</span>
          </button>
          {#if open.graphs}
            <ul class="fsec-list">
              {#each fGraphs as g}
                <li><button class="fitem" class:fitem-on={graphSel.has(g.iri)} title={g.iri} on:click={() => pickGraph(g.iri)}>
                  <span class="fitem-name">{shortenIRI(g.iri)}</span>
                  {#if g.roleLabel}<span class="role-tag {roleClass(g.role)}">{g.roleLabel}</span>{/if}
                  <span class="fitem-count">{g.count.toLocaleString()}</span>
                </button></li>
              {/each}
              {#if fGraphs.length === 0}<li class="fitem-empty">{$t('components.facetRail.noMatches')}</li>{/if}
            </ul>
          {/if}
        </section>
      </div>
    {/if}
  </aside>
{/if}

<style>
  .rail {
    width: 248px; flex-shrink: 0; display: flex; flex-direction: column;
    border-right: 1px solid #e2e8f0; background: #fafbff; min-height: 0;
  }
  .rail-head {
    display: flex; align-items: center; justify-content: space-between;
    padding: 0.5rem 0.6rem 0.35rem; border-bottom: 1px solid #eef2f7;
  }
  .rail-title { font-size: 0.74rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.04em; color: #475569; }
  .rail-collapse, .rail-search-x { background: none; border: none; cursor: pointer; color: #94a3b8; display: inline-flex; padding: 2px; }
  .rail-collapse:hover { color: #475569; }

  .rail-reopen {
    align-self: flex-start; margin: 0.4rem; background: #eef2ff; border: 1px solid #c7d2fe;
    color: #4f46e5; border-radius: 7px; padding: 0.3rem; cursor: pointer; display: inline-flex;
  }
  .rail-reopen:hover { background: #e0e7ff; }

  .rail-search-wrap { position: relative; padding: 0.4rem 0.5rem; }
  .rail-search { width: 100%; box-sizing: border-box; font-size: 0.76rem; padding: 0.3rem 1.5rem 0.3rem 0.5rem; border: 1px solid #cbd5e1; border-radius: 6px; }
  .rail-search:focus { outline: none; border-color: #6366f1; }
  .rail-search-x { position: absolute; right: 0.7rem; top: 50%; transform: translateY(-50%); }

  .rail-msg { font-size: 0.78rem; color: #94a3b8; padding: 0.6rem; text-align: center; }
  .rail-body { overflow-y: auto; min-height: 0; flex: 1; padding-bottom: 0.5rem; }

  .fsec { border-bottom: 1px solid #f1f5f9; }
  .fsec-head {
    display: flex; align-items: center; gap: 0.3rem; width: 100%;
    padding: 0.4rem 0.55rem; background: transparent; border: none; cursor: pointer;
    font-size: 0.78rem; font-weight: 600; color: #334155;
  }
  .fsec-head:hover { background: #f1f5f9; }
  .fsec-name { flex: 1; text-align: left; }
  .fsec-count { font-size: 0.68rem; color: #64748b; background: #e2e8f0; border-radius: 9px; padding: 0 6px; font-weight: 600; }

  .fsec-list { list-style: none; margin: 0; padding: 0 0 0.25rem; }
  .fitem {
    display: flex; align-items: center; gap: 0.3rem; width: 100%;
    padding: 0.22rem 0.55rem 0.22rem 1.5rem; background: transparent; border: none;
    cursor: pointer; font-size: 0.75rem; color: #475569; text-align: left;
  }
  .fitem:hover { background: #eef2ff; color: #1e293b; }
  .fitem-on { background: #e0e7ff; color: #3730a3; font-weight: 600; }
  .fitem-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .fitem-count { font-size: 0.68rem; color: #94a3b8; flex-shrink: 0; }
  /* Coloured vocabulary prefix pill — colour matches the predicate column. */
  .vocab-pill {
    flex: 0 1 auto; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font-weight: 700; font-size: 0.72rem; padding: 0 7px; border-radius: 10px;
  }
  .fitem-empty { font-size: 0.72rem; color: #cbd5e1; padding: 0.2rem 0.55rem 0.2rem 1.5rem; }

  .role-tag { font-size: 0.6rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em; padding: 0 4px; border-radius: 4px; flex-shrink: 0; }
  .role-instances { background: #dbeafe; color: #1d4ed8; }
  .role-model { background: #dcfce7; color: #15803d; }
  .role-vocabulary { background: #fef3c7; color: #b45309; }
  .role-shapes { background: #fae8ff; color: #a21caf; }
  .role-entailment { background: #e0e7ff; color: #4338ca; }
  .role-other { background: #e2e8f0; color: #64748b; }

  /* ── Dark mode ──────────────────────────────────────────────────────────
     The facet rail ("What's in here" sidebar) is hardcoded light above; these
     overrides retint the shell, search, sections and items for dark surfaces. */
  :global(html.dark) .rail {
    border-right-color: #1e293b;
    background: #0f172a;
  }
  :global(html.dark) .rail-head { border-bottom-color: #1e293b; }
  :global(html.dark) .rail-title { color: #94a3b8; }
  :global(html.dark) .rail-collapse,
  :global(html.dark) .rail-search-x { color: #64748b; }
  :global(html.dark) .rail-collapse:hover { color: #cbd5e1; }

  :global(html.dark) .rail-reopen {
    background: #1e293b; border-color: #334155; color: #a5b4fc;
  }
  :global(html.dark) .rail-reopen:hover { background: #283549; }

  :global(html.dark) .rail-search {
    background: #1e293b; border-color: #334155; color: #e2e8f0;
  }
  :global(html.dark) .rail-search::placeholder { color: #64748b; }
  :global(html.dark) .rail-search:focus { border-color: #6366f1; }

  :global(html.dark) .rail-msg { color: #64748b; }

  :global(html.dark) .fsec { border-bottom-color: #1e293b; }
  :global(html.dark) .fsec-head { color: #cbd5e1; }
  :global(html.dark) .fsec-head:hover { background: #1e293b; }
  :global(html.dark) .fsec-count { color: #94a3b8; background: #283549; }

  :global(html.dark) .fitem { color: #cbd5e1; }
  :global(html.dark) .fitem:hover { background: #1e293b; color: #f1f5f9; }
  :global(html.dark) .fitem-on { background: #312e81; color: #c7d2fe; }
  :global(html.dark) .fitem-count { color: #64748b; }
  :global(html.dark) .fitem-empty { color: #475569; }

  :global(html.dark) .role-instances { background: #1e3a5f; color: #93c5fd; }
  :global(html.dark) .role-model { background: #14432a; color: #86efac; }
  :global(html.dark) .role-vocabulary { background: #43320f; color: #fcd34d; }
  :global(html.dark) .role-shapes { background: #3f1d44; color: #f0abfc; }
  :global(html.dark) .role-entailment { background: #2a2a5c; color: #a5b4fc; }
  :global(html.dark) .role-other { background: #283549; color: #94a3b8; }
</style>
