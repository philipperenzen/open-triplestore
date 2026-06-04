<script lang="ts">
  import type { ClassEntry } from '../../lib/ontology/schema-model';
  import { renderClassExpr, renderRestriction } from '../../lib/ontology/dl-render';
  import { shortenIRI } from '../../lib/rdf-utils';
  import { ChevronRight, ChevronDown, Layers } from 'lucide-svelte';
  import { t } from 'svelte-i18n';

  export let classes: ClassEntry[] = [];
  export let onOpen: (_iri: string) => void = () => {};

  let selectedIri = '';
  let expanded = new Set<string>();

  $: byIri = new Map(classes.map(c => [c.iri, c]));
  $: roots = classes.filter(c =>
    c.parents.length === 0 || !c.parents.some(p => byIri.has(p))
  );
  $: selected = selectedIri ? byIri.get(selectedIri) || null : null;

  function toggle(iri: string) {
    const next = new Set(expanded);
    if (next.has(iri)) next.delete(iri); else next.add(iri);
    expanded = next;
  }

  function hasAxioms(c: ClassEntry): boolean {
    return c.equivalents.length > 0 || c.disjoints.length > 0
        || (c.unionOf?.length ?? 0) > 0 || (c.intersectionOf?.length ?? 0) > 0
        || !!c.complementOf || (c.oneOf?.length ?? 0) > 0
        || (c.hasKey?.length ?? 0) > 0 || c.restrictions.length > 0;
  }
</script>

<div class="cap">
  <div class="cap-list">
    <ul class="tree">
      {#each roots as r (r.iri)}
        {@render node(r, 0)}
      {/each}
    </ul>
  </div>
  <div class="cap-detail">
    {#if !selected}
      <div class="empty"><Layers size={14} /> {$t('components.classAxiomPanel.emptyState')}</div>
    {:else}
      <div class="head">
        <div class="title" title={selected.iri}>{selected.label || shortenIRI(selected.iri)}</div>
        <button class="open-btn" on:click={() => onOpen(selected.iri)}>{$t('components.classAxiomPanel.open')}</button>
      </div>
      {#if selected.comment}<p class="cmt">{selected.comment}</p>{/if}

      <div class="ax-grid">
        {#if selected.parents.length}
          <section class="ax">
            <h4>⊑ subClassOf</h4>
            <ul>
              {#each selected.parents as p}
                <li><button class="link" on:click={() => onOpen(p)}>{shortenIRI(p)}</button></li>
              {/each}
            </ul>
          </section>
        {/if}
        {#if selected.children.length}
          <section class="ax">
            <h4>⊒ {$t('components.classAxiomPanel.subclasses')}</h4>
            <ul>
              {#each selected.children as c}
                <li><button class="link" on:click={() => onOpen(c)}>{shortenIRI(c)}</button></li>
              {/each}
            </ul>
          </section>
        {/if}
        {#if selected.equivalents.length}
          <section class="ax">
            <h4>≡ equivalentClass</h4>
            <ul>
              {#each selected.equivalents as e}
                <li class="dl">{renderClassExpr(e)}</li>
              {/each}
            </ul>
          </section>
        {/if}
        {#if selected.disjoints.length}
          <section class="ax">
            <h4>⊥ disjointWith</h4>
            <ul>
              {#each selected.disjoints as d}
                <li><button class="link" on:click={() => onOpen(d)}>{shortenIRI(d)}</button></li>
              {/each}
            </ul>
          </section>
        {/if}
        {#if selected.unionOf}
          <section class="ax"><h4>⊔ unionOf</h4>
            <ul>{#each selected.unionOf as p}<li class="dl">{renderClassExpr(p)}</li>{/each}</ul>
          </section>
        {/if}
        {#if selected.intersectionOf}
          <section class="ax"><h4>⊓ intersectionOf</h4>
            <ul>{#each selected.intersectionOf as p}<li class="dl">{renderClassExpr(p)}</li>{/each}</ul>
          </section>
        {/if}
        {#if selected.complementOf}
          <section class="ax"><h4>¬ complementOf</h4>
            <ul><li class="dl">{renderClassExpr(selected.complementOf)}</li></ul>
          </section>
        {/if}
        {#if selected.oneOf}
          <section class="ax"><h4>{'{ } oneOf'}</h4>
            <ul>{#each selected.oneOf as m}
              <li><button class="link" on:click={() => onOpen(m)}>{shortenIRI(m)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.hasKey}
          <section class="ax"><h4>🔑 hasKey</h4>
            <ul>{#each selected.hasKey as k}
              <li class="dl">({k.map(x => shortenIRI(x)).join(', ')})</li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.restrictions.length}
          <section class="ax"><h4>{$t('components.classAxiomPanel.restrictions')}</h4>
            <ul>{#each selected.restrictions as r}
              <li class="dl">{renderRestriction(r)}</li>
            {/each}</ul>
          </section>
        {/if}
        <section class="ax meta">
          <h4>{$t('components.classAxiomPanel.stats')}</h4>
          <div class="meta-line">{$t('components.classAxiomPanel.instances')}: <strong>{selected.instanceCount}</strong></div>
          <div class="meta-line">{$t('components.classAxiomPanel.shaclTarget')}: <strong>{selected.hasShape ? $t('components.classAxiomPanel.yes') : $t('components.classAxiomPanel.no')}</strong></div>
        </section>
      </div>
    {/if}
  </div>
</div>

{#snippet node(c, depth)}
  {@const exp = expanded.has(c.iri)}
  <li class="node">
    <div class="row" class:active={c.iri === selectedIri}>
      {#if c.children.length > 0}
        <button class="t-toggle" on:click={() => toggle(c.iri)}>
          {#if exp}<ChevronDown size={11} />{:else}<ChevronRight size={11} />{/if}
        </button>
      {:else}
        <span class="t-toggle spacer"></span>
      {/if}
      <button class="cls-btn" on:click={() => (selectedIri = c.iri)} title={c.iri}>
        <span class="cls-label">{c.label || shortenIRI(c.iri)}</span>
        {#if hasAxioms(c)}<span class="ax-pill" title={$t('components.classAxiomPanel.hasAxioms')}>ax</span>{/if}
        {#if c.instanceCount > 0}<span class="inst-pill">{c.instanceCount}</span>{/if}
      </button>
    </div>
    {#if exp && c.children.length > 0}
      <ul class="tree nested">
        {#each c.children as childIri}
          {@const child = byIri.get(childIri)}
          {#if child}{@render node(child, depth + 1)}{/if}
        {/each}
      </ul>
    {/if}
  </li>
{/snippet}

<style>
  .cap { display: grid; grid-template-columns: 1fr 1.4fr; gap: 0.75rem; min-height: 320px; }
  .cap-list { border: 1px solid #e2e8f0; border-radius: 10px; padding: 0.4rem; overflow: auto; max-height: 60vh; }
  .cap-detail { border: 1px solid #e2e8f0; border-radius: 10px; padding: 0.6rem 0.7rem; overflow: auto; max-height: 60vh; background: #fff; }
  .tree { list-style: none; margin: 0; padding: 0; }
  .tree.nested { padding-left: 0.85rem; border-left: 1px dashed #e2e8f0; margin-left: 0.4rem; }
  .node { margin: 0.1rem 0; }
  .row { display: flex; align-items: center; gap: 0.2rem; border-radius: 6px; }
  .row.active { background: #eef5ff; }
  .t-toggle { width: 16px; height: 16px; display: inline-flex; align-items: center; justify-content: center;
    border: none; background: transparent; cursor: pointer; color: #94a3b8; padding: 0; }
  .t-toggle.spacer { cursor: default; }
  .cls-btn { flex: 1; text-align: left; padding: 0.18rem 0.4rem; background: transparent; border: none;
    cursor: pointer; display: flex; align-items: center; gap: 0.4rem; font-size: 0.82rem; }
  .cls-btn:hover { background: #f1f5f9; border-radius: 4px; }
  .cls-label { font-weight: 500; color: #1e293b; }
  .ax-pill { background: #fef3c7; color: #92400e; font-size: 0.65rem; padding: 0 5px; border-radius: 999px; font-weight: 700; }
  .inst-pill { background: #ede9fe; color: #6d28d9; font-size: 0.65rem; padding: 0 5px; border-radius: 999px; font-weight: 700; }

  .empty { color: #94a3b8; text-align: center; padding: 1rem; display: inline-flex; gap: 0.4rem; align-items: center; }

  .head { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; margin-bottom: 0.3rem; }
  .title { font-weight: 600; font-size: 0.95rem; color: #1e293b; word-break: break-all; }
  .open-btn { background: #eef5ff; color: #1565c0; border: 1px solid #bbdefb; padding: 0.18rem 0.55rem;
    border-radius: 6px; font-size: 0.72rem; cursor: pointer; }
  .cmt { color: #64748b; font-size: 0.8rem; margin: 0 0 0.4rem 0; }
  .ax-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0.5rem 0.75rem; }
  .ax { border: 1px solid #f1f5f9; border-radius: 8px; padding: 0.4rem 0.55rem; background: #fbfdff; }
  .ax h4 { font-size: 0.78rem; font-weight: 700; color: #475569; margin: 0 0 0.25rem 0; }
  .ax ul { list-style: none; margin: 0; padding: 0; }
  .ax li { font-size: 0.8rem; color: #1e293b; padding: 0.1rem 0; }
  .ax.meta { grid-column: 1 / -1; }
  .meta-line { font-size: 0.78rem; color: #475569; }
  .dl { font-family: 'JetBrains Mono', monospace; font-size: 0.78rem; color: #0f172a; }
  .link { background: none; border: none; color: #2563eb; cursor: pointer; padding: 0; font: inherit; }
  .link:hover { text-decoration: underline; }

  :global(:is([data-theme="dark"], .dark)) .cap-list,
  :global(:is([data-theme="dark"], .dark)) .cap-detail { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .cap-detail { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .tree.nested { border-left-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .row.active { background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .cls-btn:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .cls-label { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .ax-pill { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .inst-pill { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .title { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .open-btn { background: rgba(59,130,246,0.15); color: #93c5fd; border-color: rgba(59,130,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .ax { background: var(--bg-soft); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .ax h4 { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .ax li { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .meta-line { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .dl { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .link { color: #93c5fd; }
</style>
