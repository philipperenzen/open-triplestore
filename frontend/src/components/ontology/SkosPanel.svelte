<script lang="ts">
  import type { ConceptEntry, SchemaModel } from '../../lib/ontology/schema-model';
  import { shortenIRI } from '../../lib/rdf-utils';
  import { ChevronRight, ChevronDown } from 'lucide-svelte';
  import { t } from 'svelte-i18n';

  export let concepts: ConceptEntry[] = [];
  export let model: SchemaModel | null = null;
  export let onOpen: (_iri: string) => void = () => {};

  let selectedIri = '';
  let expanded = new Set<string>();

  $: byIri = new Map(concepts.map(c => [c.iri, c]));
  $: selected = selectedIri ? byIri.get(selectedIri) || null : null;
  $: schemes = model ? [...model.schemes.values()] : [];
  $: orphans = concepts.filter(c =>
    c.scheme.length === 0 && c.topConceptOf.length === 0 && c.broader.length === 0
  );

  function toggle(iri: string) {
    const next = new Set(expanded);
    if (next.has(iri)) next.delete(iri); else next.add(iri);
    expanded = next;
  }
  function lbl(c: ConceptEntry | undefined, iri: string): string {
    if (!c) return shortenIRI(iri);
    return c.prefLabel || shortenIRI(iri);
  }
</script>

<div class="sk">
  <div class="sk-list">
    {#if schemes.length === 0 && concepts.length === 0}
      <div class="empty">{$t('components.skosPanel.emptyNoConcepts')}</div>
    {/if}
    {#each schemes as sch (sch.iri)}
      <div class="scheme">
        <div class="scheme-head">
          <button class="scheme-btn" on:click={() => onOpen(sch.iri)} title={sch.iri}>
            <span class="scheme-label">{sch.label || shortenIRI(sch.iri)}</span>
            <span class="scheme-count">{$t('components.skosPanel.topCount', { values: { count: sch.topConcepts.length } })}</span>
          </button>
        </div>
        <ul class="tree">
          {#each sch.topConcepts as ti}
            {@const tc = byIri.get(ti)}
            {#if tc}{@render ctree(tc, 0)}{/if}
          {/each}
        </ul>
      </div>
    {/each}
    {#if orphans.length}
      <div class="scheme">
        <div class="scheme-head"><span class="scheme-label">{$t('components.skosPanel.conceptsNoScheme')}</span></div>
        <ul class="tree">
          {#each orphans as oc (oc.iri)}{@render ctree(oc, 0)}{/each}
        </ul>
      </div>
    {/if}
  </div>
  <div class="sk-detail">
    {#if !selected}
      <div class="empty">{$t('components.skosPanel.selectConcept')}</div>
    {:else}
      <div class="head">
        <div class="title" title={selected.iri}>{selected.prefLabel || shortenIRI(selected.iri)}</div>
        <button class="open-btn" on:click={() => onOpen(selected.iri)}>{$t('components.skosPanel.open')}</button>
      </div>
      <div class="ax-grid">
        {#if selected.altLabels.length}
          <section class="ax"><h4>altLabel</h4>
            <ul>{#each selected.altLabels as l}<li>{l}</li>{/each}</ul>
          </section>
        {/if}
        {#if selected.hiddenLabels.length}
          <section class="ax"><h4>hiddenLabel</h4>
            <ul>{#each selected.hiddenLabels as l}<li>{l}</li>{/each}</ul>
          </section>
        {/if}
        {#if selected.notation.length}
          <section class="ax"><h4>notation</h4>
            <ul>{#each selected.notation as l}<li class="dl">{l}</li>{/each}</ul>
          </section>
        {/if}
        {#if selected.scheme.length}
          <section class="ax"><h4>inScheme</h4>
            <ul>{#each selected.scheme as s}
              <li><button class="link" on:click={() => onOpen(s)}>{shortenIRI(s)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.broader.length}
          <section class="ax"><h4>broader</h4>
            <ul>{#each selected.broader as b}
              <li><button class="link" on:click={() => (selectedIri = b)}>{lbl(byIri.get(b), b)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.narrower.length}
          <section class="ax"><h4>narrower</h4>
            <ul>{#each selected.narrower as n}
              <li><button class="link" on:click={() => (selectedIri = n)}>{lbl(byIri.get(n), n)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.broaderTransitive.length}
          <section class="ax"><h4>broaderTransitive</h4>
            <ul>{#each selected.broaderTransitive as b}
              <li><button class="link" on:click={() => (selectedIri = b)}>{lbl(byIri.get(b), b)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.related.length}
          <section class="ax"><h4>related</h4>
            <ul>{#each selected.related as r}
              <li><button class="link" on:click={() => (selectedIri = r)}>{lbl(byIri.get(r), r)}</button></li>
            {/each}</ul>
          </section>
        {/if}
      </div>
    {/if}
  </div>
</div>

{#snippet ctree(c, depth)}
  {@const exp = expanded.has(c.iri)}
  <li class="cnode">
    <div class="crow" class:active={c.iri === selectedIri}>
      {#if c.narrower.length > 0}
        <button class="t-toggle" on:click={() => toggle(c.iri)}>
          {#if exp}<ChevronDown size={11} />{:else}<ChevronRight size={11} />{/if}
        </button>
      {:else}
        <span class="t-toggle spacer"></span>
      {/if}
      <button class="cbtn" on:click={() => (selectedIri = c.iri)} title={c.iri}>
        <span>{c.prefLabel || shortenIRI(c.iri)}</span>
        {#if c.notation.length}<span class="not-pill">{c.notation[0]}</span>{/if}
      </button>
    </div>
    {#if exp && c.narrower.length > 0}
      <ul class="tree nested">
        {#each c.narrower as ni}
          {@const child = byIri.get(ni)}
          {#if child}{@render ctree(child, depth + 1)}{/if}
        {/each}
      </ul>
    {/if}
  </li>
{/snippet}

<style>
  .sk { display: grid; grid-template-columns: 1fr 1.2fr; gap: 0.75rem; min-height: 320px; }
  .sk-list, .sk-detail { border: 1px solid #e2e8f0; border-radius: 10px; padding: 0.4rem; overflow: auto; max-height: 60vh; background: #fff; }
  .empty { color: #94a3b8; padding: 1rem; text-align: center; }
  .scheme { margin-bottom: 0.5rem; }
  .scheme-head { padding: 0.3rem 0.45rem; background: #f8fafc; border-radius: 6px; margin-bottom: 0.2rem;
    display: flex; align-items: center; }
  .scheme-btn { background: none; border: none; cursor: pointer; display: flex; gap: 0.4rem; align-items: center; padding: 0; }
  .scheme-label { font-weight: 600; color: #1e293b; font-size: 0.85rem; }
  .scheme-count { color: #64748b; font-size: 0.7rem; }
  .tree { list-style: none; margin: 0; padding: 0; }
  .tree.nested { padding-left: 0.85rem; border-left: 1px dashed #e2e8f0; margin-left: 0.4rem; }
  .cnode { margin: 0.08rem 0; }
  .crow { display: flex; align-items: center; gap: 0.2rem; border-radius: 6px; }
  .crow.active { background: #eef5ff; }
  .t-toggle { width: 16px; height: 16px; display: inline-flex; align-items: center; justify-content: center;
    border: none; background: transparent; cursor: pointer; color: #94a3b8; padding: 0; }
  .t-toggle.spacer { cursor: default; }
  .cbtn { flex: 1; text-align: left; padding: 0.16rem 0.4rem; background: transparent; border: none; cursor: pointer;
    display: flex; gap: 0.4rem; align-items: center; font-size: 0.82rem; }
  .cbtn:hover { background: #f1f5f9; border-radius: 4px; }
  .not-pill { background: #f1f5f9; color: #475569; font-size: 0.65rem; padding: 0 5px; border-radius: 999px; font-family: monospace; }

  .head { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; margin-bottom: 0.3rem; }
  .title { font-weight: 600; font-size: 0.95rem; color: #1e293b; word-break: break-all; }
  .open-btn { background: #eef5ff; color: #1565c0; border: 1px solid #bbdefb; padding: 0.18rem 0.55rem;
    border-radius: 6px; font-size: 0.72rem; cursor: pointer; }
  .ax-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem 0.75rem; }
  .ax { border: 1px solid #f1f5f9; border-radius: 8px; padding: 0.4rem 0.55rem; background: #fbfdff; }
  .ax h4 { font-size: 0.78rem; font-weight: 700; color: #475569; margin: 0 0 0.25rem 0; }
  .ax ul { list-style: none; margin: 0; padding: 0; }
  .ax li { font-size: 0.8rem; color: #1e293b; padding: 0.1rem 0; }
  .dl { font-family: 'JetBrains Mono', monospace; font-size: 0.78rem; color: #0f172a; }
  .link { background: none; border: none; color: #2563eb; cursor: pointer; padding: 0; font: inherit; }
  .link:hover { text-decoration: underline; }

  :global(:is([data-theme="dark"], .dark)) .sk-list,
  :global(:is([data-theme="dark"], .dark)) .sk-detail { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .scheme-head { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .scheme-label { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .scheme-count { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .tree.nested { border-left-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .crow.active { background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .cbtn:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .not-pill { background: rgba(255,255,255,0.06); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .title { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .open-btn { background: rgba(59,130,246,0.15); color: #93c5fd; border-color: rgba(59,130,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .ax { background: var(--bg-soft); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .ax h4 { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .ax li { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .dl { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .link { color: #93c5fd; }
</style>
