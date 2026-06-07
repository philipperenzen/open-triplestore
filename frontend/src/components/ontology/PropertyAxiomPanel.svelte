<script lang="ts">
  import type { PropertyEntry } from '../../lib/ontology/schema-model';
  import { renderChain } from '../../lib/ontology/dl-render';
  import { shortenIRI } from '../../lib/rdf-utils';
  import { t } from 'svelte-i18n';
  import TermDefinitionCard from './TermDefinitionCard.svelte';

  export let properties: PropertyEntry[] = [];
  export let onOpen: (_iri: string) => void = () => {};

  let selectedIri = '';
  $: byIri = new Map(properties.map(p => [p.iri, p]));
  $: selected = selectedIri ? byIri.get(selectedIri) || null : null;
</script>

<div class="pap">
  <div class="pap-list">
    <table class="pp-table">
      <thead><tr>
        <th>{$t('components.propertyAxiomPanel.colProperty')}</th><th>{$t('components.propertyAxiomPanel.colKind')}</th><th>{$t('components.propertyAxiomPanel.colCharacteristics')}</th>
      </tr></thead>
      <tbody>
        {#each properties as p (p.iri)}
          <tr class:active={p.iri === selectedIri} on:click={() => (selectedIri = p.iri)}>
            <td><strong>{p.label || shortenIRI(p.iri)}</strong>
              <div class="muted">{shortenIRI(p.iri)}</div>
            </td>
            <td><span class="kind kind-{p.kind}">{p.kind}</span></td>
            <td>
              {#each [...p.characteristics] as ch}
                <span class="ch-pill" title={ch}>{ch}</span>
              {/each}
              {#if p.characteristics.size === 0}<span class="muted small">—</span>{/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <div class="pap-detail">
    {#if !selected}
      <div class="empty">{$t('components.propertyAxiomPanel.emptyState')}</div>
    {:else}
      <div class="head">
        <div class="title" title={selected.iri}>{selected.label || shortenIRI(selected.iri)}</div>
        <button class="open-btn" on:click={() => onOpen(selected.iri)}>{$t('components.propertyAxiomPanel.open')}</button>
      </div>
      {#if selected.comment}<p class="cmt">{selected.comment}</p>{/if}

      <div class="ax-grid">
        <section class="ax">
          <h4>{$t('components.propertyAxiomPanel.colKind')}</h4>
          <div><span class="kind kind-{selected.kind}">{selected.kind}</span></div>
        </section>
        <section class="ax">
          <h4>{$t('components.propertyAxiomPanel.colCharacteristics')}</h4>
          {#if selected.characteristics.size}
            <div class="row">{#each [...selected.characteristics] as ch}<span class="ch-pill">{ch}</span>{/each}</div>
          {:else}<div class="muted small">{$t('components.propertyAxiomPanel.none')}</div>{/if}
        </section>
        {#if selected.domain.length}
          <section class="ax">
            <h4>rdfs:domain</h4>
            <ul>{#each selected.domain as d}
              <li><button class="link" on:click={() => onOpen(d)}>{shortenIRI(d)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.range.length}
          <section class="ax">
            <h4>rdfs:range</h4>
            <ul>{#each selected.range as r}
              <li><button class="link" on:click={() => onOpen(r)}>{shortenIRI(r)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.superProperties.length}
          <section class="ax">
            <h4>⊑ subPropertyOf</h4>
            <ul>{#each selected.superProperties as sp}
              <li><button class="link" on:click={() => onOpen(sp)}>{shortenIRI(sp)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.equivalentProperty.length}
          <section class="ax">
            <h4>≡ equivalentProperty</h4>
            <ul>{#each selected.equivalentProperty as e}
              <li><button class="link" on:click={() => onOpen(e)}>{shortenIRI(e)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.inverseOf.length}
          <section class="ax">
            <h4>⁻¹ inverseOf</h4>
            <ul>{#each selected.inverseOf as i}
              <li><button class="link" on:click={() => onOpen(i)}>{shortenIRI(i)}</button></li>
            {/each}</ul>
          </section>
        {/if}
        {#if selected.chains.length}
          <section class="ax">
            <h4>∘ propertyChainAxiom</h4>
            <ul>{#each selected.chains as c}
              <li class="dl">{renderChain(c)}</li>
            {/each}</ul>
          </section>
        {/if}
      </div>
      <TermDefinitionCard iri={selected.iri} variant="rich" hideEmpty {onOpen} />
    {/if}
  </div>
</div>

<style>
  .pap { display: grid; grid-template-columns: 1.2fr 1fr; gap: 0.75rem; min-height: 320px; }
  .pap-list, .pap-detail { border: 1px solid #e2e8f0; border-radius: 10px; overflow: auto; max-height: 60vh; }
  .pap-detail { padding: 0.6rem 0.7rem; background: #fff; }
  .pp-table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
  .pp-table th { text-align: left; background: #f8fafc; padding: 0.35rem 0.55rem; border-bottom: 1px solid #e2e8f0;
    font-weight: 600; color: #475569; position: sticky; top: 0; }
  .pp-table td { padding: 0.32rem 0.55rem; border-bottom: 1px solid #f1f5f9; vertical-align: top; }
  .pp-table tr { cursor: pointer; }
  .pp-table tr:hover { background: #f8fafc; }
  .pp-table tr.active { background: #eef5ff; }
  .muted { color: #94a3b8; font-size: 0.7rem; font-family: monospace; }
  .small { font-size: 0.75rem; }
  .kind { font-size: 0.7rem; font-weight: 700; text-transform: uppercase; padding: 1px 6px; border-radius: 999px;
    background: #f1f5f9; color: #475569; }
  .kind-object { background: #dbeafe; color: #1d4ed8; }
  .kind-datatype { background: #dcfce7; color: #15803d; }
  .kind-annotation { background: #fef3c7; color: #92400e; }
  .ch-pill { display: inline-block; font-size: 0.68rem; padding: 1px 6px; border-radius: 999px;
    background: #ecfeff; color: #0e7490; border: 1px solid #a5f3fc; margin: 1px 2px 1px 0; }
  .empty { color: #94a3b8; padding: 1rem; text-align: center; }
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
  .row { display: flex; flex-wrap: wrap; gap: 0.25rem; }
  .dl { font-family: 'JetBrains Mono', monospace; font-size: 0.78rem; color: #0f172a; }
  .link { background: none; border: none; color: #2563eb; cursor: pointer; padding: 0; font: inherit; }
  .link:hover { text-decoration: underline; }

  :global(:is([data-theme="dark"], .dark)) .pap-list,
  :global(:is([data-theme="dark"], .dark)) .pap-detail { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .pap-detail { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .pp-table th { background: var(--bg-soft); border-bottom-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .pp-table td { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .pp-table tr:hover { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .pp-table tr.active { background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .kind { background: rgba(255,255,255,0.06); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .kind-object { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .kind-datatype { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .kind-annotation { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .ch-pill { background: rgba(34,211,238,0.18); color: #67e8f9; border-color: rgba(34,211,238,0.3); }
  :global(:is([data-theme="dark"], .dark)) .title { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .open-btn { background: rgba(59,130,246,0.15); color: #93c5fd; border-color: rgba(59,130,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .ax { background: var(--bg-soft); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .ax h4 { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .ax li { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .dl { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .link { color: #93c5fd; }
</style>
