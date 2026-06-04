<script>
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import { Shapes, Target, Type, Link } from 'lucide-svelte';
  import { t } from 'svelte-i18n';

  /** shape object: { iri, targetClass, targetNode, properties: [...] } */
  export let shape;
</script>

<div class="shape-card">
  <div class="shape-head">
    <Shapes size={13} />
    <code class="iri" title={shape.iri}>{shortenIRI(shape.iri)}</code>
  </div>

  {#if shape.targetClass?.length || shape.targetNode?.length}
    <div class="targets">
      {#each shape.targetClass as tc (tc)}
        <span class="chip target-class" title={tc}><Target size={10} /> {shortenIRI(tc)}</span>
      {/each}
      {#each shape.targetNode as tn (tn)}
        <span class="chip target-node" title={tn}>🎯 {shortenIRI(tn)}</span>
      {/each}
    </div>
  {/if}

  {#if shape.properties?.length}
    <table class="props">
      <thead>
        <tr>
          <th>{$t('components.shaclShapePanel.colPath')}</th>
          <th>{$t('components.shaclShapePanel.colCardinality')}</th>
          <th>{$t('components.shaclShapePanel.colType')}</th>
          <th>{$t('components.shaclShapePanel.colExtra')}</th>
        </tr>
      </thead>
      <tbody>
        {#each shape.properties as p}
          <tr>
            <td class="path">
              <code title={p.path}>{p.name || shortenIRI(p.path)}</code>
            </td>
            <td class="card">
              <span class="cardinality">
                [{p.minCount ?? 0}..{p.maxCount ?? '∗'}]
              </span>
            </td>
            <td class="type">
              {#if p.datatype}
                <span class="badge dt" title={p.datatype}><Type size={10} /> {shortenIRI(p.datatype)}</span>
              {/if}
              {#if p.class}
                <span class="badge cls" title={p.class}><Link size={10} /> {shortenIRI(p.class)}</span>
              {/if}
              {#if p.nodeKind}
                <span class="badge nk">{shortenIRI(p.nodeKind)}</span>
              {/if}
            </td>
            <td class="extra">
              {#if p.pattern}<code class="pattern" title="sh:pattern">/{p.pattern}/</code>{/if}
              {#if p.in?.length}<span class="badge enum" title="sh:in">∈ {$t('components.shaclShapePanel.valuesCount', { values: { count: p.in.length } })}</span>{/if}
              {#if p.severity}<span class="badge sev">{shortenIRI(p.severity)}</span>{/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {:else}
    <div class="empty">{$t('components.shaclShapePanel.noPropertyShapes')}</div>
  {/if}
</div>

<style>
  .shape-card {
    border: 1px solid #e5e7eb;
    border-radius: 6px;
    padding: 10px;
    background: #fff;
    margin-bottom: 10px;
  }
  .shape-head {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
    margin-bottom: 6px;
    color: #831843;
  }
  .iri { font-size: 12px; }
  .targets { display: flex; flex-wrap: wrap; gap: 4px; margin-bottom: 8px; }
  .chip {
    font-size: 10px;
    padding: 1px 6px;
    border-radius: 10px;
    display: inline-flex;
    align-items: center;
    gap: 3px;
  }
  .target-class { background: #dbeafe; color: #1e3a8a; }
  .target-node { background: #ede9fe; color: #5b21b6; }
  table.props {
    width: 100%;
    border-collapse: collapse;
    font-size: 11px;
  }
  table.props th, table.props td {
    text-align: left;
    padding: 4px 6px;
    border-bottom: 1px solid #f3f4f6;
    vertical-align: top;
  }
  table.props th { color: #6b7280; font-weight: 600; font-size: 10px; text-transform: uppercase; }
  .cardinality { font-family: monospace; color: #111827; }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-size: 10px;
    padding: 1px 5px;
    border-radius: 3px;
    margin-right: 3px;
  }
  .badge.dt { background: #ecfccb; color: #365314; }
  .badge.cls { background: #dbeafe; color: #1e3a8a; }
  .badge.nk { background: #f3f4f6; color: #374151; }
  .badge.enum { background: #fef3c7; color: #78350f; }
  .badge.sev { background: #fee2e2; color: #7f1d1d; }
  .pattern { font-size: 10px; color: #7c2d12; }
  .empty { font-size: 11px; color: #9ca3af; font-style: italic; }

  :global(:is([data-theme="dark"], .dark)) .shape-card { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .shape-head { color: #f9a8d4; }
  :global(:is([data-theme="dark"], .dark)) .target-class { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .target-node { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) table.props th { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) table.props th,
  :global(:is([data-theme="dark"], .dark)) table.props td { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .cardinality { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .badge.dt { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .badge.cls { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .badge.nk { background: rgba(255,255,255,0.06); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .badge.enum { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .badge.sev { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .pattern { color: #fdba74; }
</style>
