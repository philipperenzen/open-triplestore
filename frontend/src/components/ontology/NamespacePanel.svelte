<script lang="ts">
  import { t } from 'svelte-i18n';
  import type { SchemaModel } from '../../lib/ontology/schema-model';
  import { shortenIRI } from '../../lib/rdf-utils';

  export let model: SchemaModel | null = null;
  export let onOpen: (_iri: string) => void = () => {};

  $: rows = model
    ? [...model.namespaces.values()].sort((a, b) => b.count - a.count)
    : [];
  $: imports = model ? model.imports : [];

  let selectedNs = '';

  function entriesIn(ns: string): { iri: string; kind: string; label: string }[] {
    if (!model) return [];
    const out: { iri: string; kind: string; label: string }[] = [];
    for (const c of model.classes.values()) {
      if (c.iri.startsWith(ns)) out.push({ iri: c.iri, kind: 'class', label: c.label });
    }
    for (const p of model.properties.values()) {
      if (p.iri.startsWith(ns)) out.push({ iri: p.iri, kind: p.kind, label: p.label });
    }
    for (const k of model.concepts.values()) {
      if (k.iri.startsWith(ns)) out.push({ iri: k.iri, kind: 'concept', label: k.prefLabel });
    }
    for (const sh of model.shapes.values()) {
      if (sh.iri.startsWith(ns)) out.push({ iri: sh.iri, kind: 'shape', label: sh.label });
    }
    return out.sort((a, b) => a.iri.localeCompare(b.iri));
  }

  $: nsEntries = selectedNs ? entriesIn(selectedNs) : [];
</script>

<div class="ns">
  <div class="ns-table-wrap">
    {#if imports.length}
      <div class="imports">
        <strong>owl:imports:</strong>
        {#each imports as i}<a class="ich" href={i} target="_blank" rel="noopener">{shortenIRI(i)}</a>{/each}
      </div>
    {/if}
    {#if rows.length === 0}
      <div class="empty">{$t('components.namespacePanel.noNamespaces')}</div>
    {:else}
      <table class="ns-table">
        <thead><tr>
          <th>{$t('components.namespacePanel.colNamespace')}</th><th>{$t('components.namespacePanel.colPrefix')}</th><th>{$t('components.namespacePanel.colVocab')}</th><th>{$t('components.namespacePanel.colEntities')}</th><th>{$t('components.namespacePanel.colImported')}</th>
        </tr></thead>
        <tbody>
          {#each rows as r (r.ns)}
            <tr class:active={r.ns === selectedNs} on:click={() => (selectedNs = r.ns)}>
              <td><code class="ns-iri">{r.ns}</code></td>
              <td>{r.prefix ? `${r.prefix}:` : '—'}</td>
              <td><span class="kpill kp-{r.kind}">{r.kind}</span></td>
              <td>{r.count}</td>
              <td>{r.isImported ? $t('components.namespacePanel.yes') : '—'}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </div>

  <div class="ns-detail">
    {#if !selectedNs}
      <div class="empty">{$t('components.namespacePanel.selectNamespace')}</div>
    {:else}
      <div class="head"><code>{selectedNs}</code></div>
      <div class="ent-list">
        {#each nsEntries as e}
          <button class="ent" on:click={() => onOpen(e.iri)} title={e.iri}>
            <span class="kpill kp-{e.kind}">{e.kind}</span>
            <span class="ent-label">{e.label || shortenIRI(e.iri)}</span>
          </button>
        {/each}
        {#if nsEntries.length === 0}<div class="empty">{$t('components.namespacePanel.noEntries')}</div>{/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .ns { display: grid; grid-template-columns: 1.4fr 1fr; gap: 0.75rem; min-height: 320px; }
  .ns-table-wrap, .ns-detail { border: 1px solid #e2e8f0; border-radius: 10px; padding: 0.4rem; overflow: auto; max-height: 60vh; background: #fff; }
  .empty { color: #94a3b8; padding: 1rem; text-align: center; }
  .imports { font-size: 0.8rem; color: #475569; padding: 0.3rem 0.4rem; background: #f8fafc; border-radius: 6px; margin-bottom: 0.4rem; }
  .ich { display: inline-block; margin: 0 0.2rem; color: #1565c0; font-family: monospace; font-size: 0.78rem; text-decoration: none; }
  .ich:hover { text-decoration: underline; }
  .ns-table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
  .ns-table th { text-align: left; background: #f8fafc; padding: 0.35rem 0.55rem; border-bottom: 1px solid #e2e8f0;
    font-weight: 600; color: #475569; position: sticky; top: 0; }
  .ns-table td { padding: 0.32rem 0.55rem; border-bottom: 1px solid #f1f5f9; vertical-align: top; }
  .ns-table tr { cursor: pointer; }
  .ns-table tr:hover { background: #f8fafc; }
  .ns-table tr.active { background: #eef5ff; }
  .ns-iri { font-family: monospace; font-size: 0.78rem; color: #0f172a; word-break: break-all; }
  .kpill { font-size: 0.65rem; padding: 1px 6px; border-radius: 999px; background: #f1f5f9; color: #475569; font-weight: 700; text-transform: uppercase; }
  .kp-class { background: #dbeafe; color: #1d4ed8; }
  .kp-object { background: #dcfce7; color: #15803d; }
  .kp-datatype { background: #fef3c7; color: #92400e; }
  .kp-annotation { background: #fde9f3; color: #9d174d; }
  .kp-concept { background: #ede9fe; color: #6d28d9; }
  .kp-shape { background: #ffedd5; color: #9a3412; }
  .head { font-family: monospace; font-size: 0.85rem; padding: 0.3rem 0.4rem; background: #f8fafc; border-radius: 6px; word-break: break-all; }
  .ent-list { display: flex; flex-direction: column; gap: 0.2rem; margin-top: 0.4rem; }
  .ent { background: transparent; border: 1px solid transparent; border-radius: 6px; padding: 0.25rem 0.45rem;
    display: flex; gap: 0.5rem; align-items: center; cursor: pointer; text-align: left; }
  .ent:hover { background: #f1f5f9; border-color: #e2e8f0; }
  .ent-label { color: #1e293b; font-size: 0.82rem; }

  :global(:is([data-theme="dark"], .dark)) .ns-table-wrap,
  :global(:is([data-theme="dark"], .dark)) .ns-detail { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .imports { background: var(--bg-soft); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .ich { color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .ns-table th { background: var(--bg-soft); border-bottom-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .ns-table td { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .ns-table tr:hover { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .ns-table tr.active { background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .ns-iri { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .kpill { background: rgba(255,255,255,0.06); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .kp-class { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .kp-object { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .kp-datatype { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .kp-annotation { background: rgba(236,72,153,0.2); color: #f9a8d4; }
  :global(:is([data-theme="dark"], .dark)) .kp-concept { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .kp-shape { background: rgba(249,115,22,0.2); color: #fdba74; }
  :global(:is([data-theme="dark"], .dark)) .head { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .ent:hover { background: rgba(255,255,255,0.06); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .ent-label { color: var(--ink-900); }
</style>
