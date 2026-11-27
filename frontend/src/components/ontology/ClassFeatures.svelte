<script lang="ts">
  // Attributes, relations and shape constraints of the selected class. A
  // property is listed once, in the table its kind belongs to, with the facts
  // from rdfs:domain/range and from SHACL merged into that one row; the badges
  // say where each row comes from.
  import type { SchemaModel } from '../../lib/ontology/schema-model';
  import type { ClassDetails, FeatureRow } from '../../lib/ontology/class-details';
  import { renderPath } from '../../lib/shaclModel';
  import { shortenIRI } from '../../lib/rdf-utils';
  import { t } from 'svelte-i18n';

  export let model: SchemaModel;
  export let details: ClassDetails;
  export let classIri: string;
  export let onSelectClass: (_iri: string) => void = () => {};
  export let onOpenProperty: (_iri: string) => void = () => {};
  export let onOpenShape: (_iri: string) => void = () => {};
  export let onOpen: (_iri: string) => void = () => {};

  // Inherited groups stay open while the whole pane fits on a screen or so.
  const OPEN_INHERITED_UP_TO = 12;

  type Group = { origin: string; rows: FeatureRow[] };
  function groups(rows: FeatureRow[]): Group[] {
    const out: Group[] = [];
    for (const r of rows) {
      const last = out[out.length - 1];
      if (last && last.origin === r.origin) last.rows.push(r);
      else out.push({ origin: r.origin, rows: [r] });
    }
    return out;
  }

  $: attrGroups = groups(details.attributes);
  $: outGroups = groups(details.outgoing);
  $: inheritedCount = [...details.attributes, ...details.outgoing].filter(r => r.origin !== classIri).length;
  $: openInherited = inheritedCount <= OPEN_INHERITED_UP_TO;

  const curie = (iri: string) => shortenIRI(iri);
  const classLabel = (iri: string) => model.classes.get(iri)?.label || curie(iri);
  const shapeLabel = (iri: string) => model.shapes.get(iri)?.label || curie(iri);

  function cardinality(r: { minCount?: string; maxCount?: string }): string {
    if (r.minCount == null && r.maxCount == null) return '';
    return `${r.minCount ?? '0'}..${r.maxCount ?? '*'}`;
  }

  function openProp(r: { iri: string; propKind: unknown }) {
    // Only declared properties have a row in the properties tab.
    if (r.propKind) onOpenProperty(r.iri); else onOpen(r.iri);
  }
</script>

{#snippet classRef(iri: string)}
  {#if model.classes.has(iri)}
    <button type="button" class="link" title={iri}
      aria-label={$t('components.classAxiomPanel.selectClass', { values: { cls: classLabel(iri) } })}
      on:click={() => onSelectClass(iri)}>{classLabel(iri)}</button>
  {:else}
    <code class="dt" title={iri}>{curie(iri)}</code>
  {/if}
{/snippet}

{#snippet propRef(r: { iri: string; label: string; propKind: unknown; name?: string })}
  <button type="button" class="link prop" title={r.propKind ? $t('components.classAxiomPanel.openInProperties') : r.iri}
    on:click={() => openProp(r)}>{r.label || r.name || curie(r.iri)}</button>
  <code class="curie">{curie(r.iri)}</code>
{/snippet}

{#snippet sources(r: FeatureRow)}
  {#if r.declared}
    <span class="badge declared" title={$t('components.classAxiomPanel.badgeDeclaredTitle')}>{$t('components.classAxiomPanel.badgeDeclared')}</span>
  {/if}
  {#if r.viaUnion}
    <span class="badge union" title={$t('components.classAxiomPanel.badgeUnionTitle')}>{$t('components.classAxiomPanel.badgeUnion')}</span>
  {/if}
  {#each r.shapes as s}
    <button type="button" class="badge shape" title={$t('components.classAxiomPanel.openInShacl')} on:click={() => onOpenShape(s)}>
      {$t('components.classAxiomPanel.badgeShape')}: {shapeLabel(s)}
    </button>
  {/each}
{/snippet}

{#snippet facets(r: FeatureRow)}
  {#each r.targets as tgt}<span class="tgt">{@render classRef(tgt)}</span>{/each}
  {#if r.nodeKind}<code class="dt">{curie(r.nodeKind)}</code>{/if}
  {#if r.inCount != null}<span class="facet">{$t('components.classAxiomPanel.inValues', { values: { count: r.inCount } })}</span>{/if}
  {#if r.pattern}<code class="facet pattern" title={r.pattern}>/{r.pattern}/</code>{/if}
  {#if !r.targets.length && !r.nodeKind && r.inCount == null && !r.pattern}<span class="muted">—</span>{/if}
{/snippet}

{#snippet cardCell(r: FeatureRow)}
  <span class="mono">{cardinality(r)}</span>
  {#if r.functional}<span class="badge fn" title={$t('components.classAxiomPanel.functionalTitle')}>{$t('components.classAxiomPanel.functional')}</span>{/if}
{/snippet}

{#snippet featureTable(rows: FeatureRow[], relation: boolean)}
  <table class="cf-table">
    <thead><tr>
      <th scope="col">{relation ? $t('components.classAxiomPanel.colRelation') : $t('components.classAxiomPanel.colAttribute')}</th>
      <th scope="col">{relation ? $t('components.classAxiomPanel.colTarget') : $t('components.classAxiomPanel.colType')}</th>
      <th scope="col">{$t('components.classAxiomPanel.colCardinality')}</th>
      <th scope="col">{$t('components.classAxiomPanel.colSource')}</th>
    </tr></thead>
    <tbody>
      {#each rows as r (r.iri)}
        <tr data-prop={r.iri}>
          <td>{@render propRef(r)}</td>
          <td>{#if relation}→ {/if}{@render facets(r)}</td>
          <td>{@render cardCell(r)}</td>
          <td>{@render sources(r)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
{/snippet}

{#snippet grouped(gs: Group[], relation: boolean)}
  {#each gs as g (g.origin)}
    {#if g.origin === classIri}
      {@render featureTable(g.rows, relation)}
    {:else}
      <details class="inherited" open={openInherited}>
        <summary>
          {$t('components.classAxiomPanel.inheritedFrom', { values: { cls: classLabel(g.origin), count: g.rows.length } })}
        </summary>
        {@render featureTable(g.rows, relation)}
      </details>
    {/if}
  {/each}
{/snippet}

{#if details.attributes.length}
  <section class="cf">
    <h4>{$t('components.classAxiomPanel.attributes')} <span class="n">{details.attributes.length}</span></h4>
    {@render grouped(attrGroups, false)}
  </section>
{/if}

{#if details.outgoing.length || details.incoming.length}
  <section class="cf">
    <h4>{$t('components.classAxiomPanel.relations')} <span class="n">{details.outgoing.length + details.incoming.length}</span></h4>
    {#if details.outgoing.length}
      <h5>{$t('components.classAxiomPanel.outgoing')}</h5>
      {@render grouped(outGroups, true)}
    {/if}
    {#if details.incoming.length}
      <h5>{$t('components.classAxiomPanel.incoming')}</h5>
      <table class="cf-table">
        <thead><tr>
          <th scope="col">{$t('components.classAxiomPanel.colSourceClass')}</th>
          <th scope="col">{$t('components.classAxiomPanel.colRelation')}</th>
          <th scope="col">{$t('components.classAxiomPanel.colSource')}</th>
        </tr></thead>
        <tbody>
          {#each details.incoming as r (r.iri)}
            <tr data-prop={r.iri}>
              <td>
                {#each r.sources as s}<span class="tgt">{@render classRef(s)}</span>{/each}
                {#if !r.sources.length}<span class="muted">{$t('components.classAxiomPanel.anyClass')}</span>{/if}
              </td>
              <td>← {@render propRef(r)}</td>
              <td>
                {#if r.declared}<span class="badge declared" title={$t('components.classAxiomPanel.badgeDeclaredTitle')}>{$t('components.classAxiomPanel.badgeDeclared')}</span>{/if}
                {#each r.shapes as s}
                  <button type="button" class="badge shape" title={$t('components.classAxiomPanel.openInShacl')} on:click={() => onOpenShape(s)}>
                    {$t('components.classAxiomPanel.badgeShape')}: {shapeLabel(s)}
                  </button>
                {/each}
                {#if r.origin !== classIri}
                  <span class="badge inh">{$t('components.classAxiomPanel.inheritedBadge', { values: { cls: classLabel(r.origin) } })}</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </section>
{/if}

{#if details.shapes.length}
  <section class="cf">
    <h4>{$t('components.classAxiomPanel.shapes')} <span class="n">{details.shapes.length}</span></h4>
    <ul class="cf-list">
      {#each details.shapes as s (s.iri)}
        <li>
          <button type="button" class="link" title={$t('components.classAxiomPanel.openInShacl')} on:click={() => onOpenShape(s.iri)}>{s.label || curie(s.iri)}</button>
          <span class="muted">
            {s.implicit ? $t('components.classAxiomPanel.shapeImplicit') : $t('components.classAxiomPanel.shapeTargets', { values: { cls: classLabel(s.origin) } })}
            · {$t('components.classAxiomPanel.shapePropertyCount', { values: { count: s.propertyCount } })}
          </span>
          {#if s.origin !== classIri}<span class="badge inh">{$t('components.classAxiomPanel.inheritedBadge', { values: { cls: classLabel(s.origin) } })}</span>{/if}
        </li>
      {/each}
    </ul>
    {#if details.pathConstraints.length}
      <h5>{$t('components.classAxiomPanel.pathConstraints')}</h5>
      <table class="cf-table">
        <thead><tr>
          <th scope="col">{$t('components.classAxiomPanel.colPath')}</th>
          <th scope="col">{$t('components.classAxiomPanel.colType')}</th>
          <th scope="col">{$t('components.classAxiomPanel.colCardinality')}</th>
          <th scope="col">{$t('components.classAxiomPanel.colSource')}</th>
        </tr></thead>
        <tbody>
          {#each details.pathConstraints as pc}
            <tr>
              <td><code>{renderPath(pc.prop.pathExpr ?? pc.prop.path, curie)}</code>{#if pc.prop.name}<span class="muted"> — {pc.prop.name}</span>{/if}</td>
              <td>
                {#if pc.prop.datatype}<code class="dt">{curie(pc.prop.datatype)}</code>{/if}
                {#if pc.prop.cls}<span class="tgt">{@render classRef(pc.prop.cls)}</span>{/if}
                {#if pc.prop.nodeKind}<code class="dt">{curie(pc.prop.nodeKind)}</code>{/if}
                {#if pc.prop.inCount != null}<span class="facet">{$t('components.classAxiomPanel.inValues', { values: { count: pc.prop.inCount } })}</span>{/if}
                {#if pc.prop.pattern}<code class="facet pattern">/{pc.prop.pattern}/</code>{/if}
              </td>
              <td class="mono">{cardinality(pc.prop)}</td>
              <td>
                <button type="button" class="badge shape" title={$t('components.classAxiomPanel.openInShacl')} on:click={() => onOpenShape(pc.shape)}>
                  {$t('components.classAxiomPanel.badgeShape')}: {shapeLabel(pc.shape)}
                </button>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  </section>
{/if}

<style>
  .cf { border: 1px solid #f1f5f9; border-radius: 8px; padding: 0.4rem 0.55rem; background: #fbfdff; margin-top: 0.5rem; }
  .cf h4 { font-size: 0.78rem; font-weight: 700; color: #475569; margin: 0 0 0.25rem 0; }
  .cf h5 { font-size: 0.72rem; font-weight: 700; color: #64748b; margin: 0.35rem 0 0.15rem; text-transform: uppercase; letter-spacing: 0.03em; }
  .n { background: #e2e8f0; color: #475569; border-radius: 999px; padding: 0 0.4rem; font-size: 0.66rem; }
  .cf-table { width: 100%; border-collapse: collapse; font-size: 0.78rem; }
  .cf-table th { text-align: left; font-weight: 600; color: #64748b; padding: 0.2rem 0.35rem; border-bottom: 1px solid #e2e8f0; }
  .cf-table td { padding: 0.22rem 0.35rem; border-bottom: 1px solid #f1f5f9; vertical-align: top; color: #1e293b; }
  .cf-list { list-style: none; margin: 0; padding: 0; }
  .cf-list li { font-size: 0.8rem; padding: 0.1rem 0; }
  .inherited { margin: 0.25rem 0; }
  .inherited summary { cursor: pointer; font-size: 0.74rem; color: #6d28d9; font-weight: 600; padding: 0.1rem 0; }
  .link { background: none; border: none; color: #2563eb; cursor: pointer; padding: 0; font: inherit; text-align: left; }
  .link:hover { text-decoration: underline; }
  .link.prop { font-weight: 500; }
  .curie { display: block; font-size: 0.68rem; color: #94a3b8; }
  .dt { font-size: 0.72rem; color: #0f766e; }
  .tgt + .tgt::before { content: ', '; color: #94a3b8; }
  .facet { font-size: 0.7rem; color: #475569; margin-left: 0.25rem; }
  .pattern { word-break: break-all; }
  .mono { font-family: 'JetBrains Mono', monospace; font-size: 0.74rem; }
  .muted { color: #94a3b8; font-size: 0.74rem; }
  .badge { display: inline-block; font-size: 0.64rem; font-weight: 600; padding: 0 6px; border-radius: 999px;
    margin: 1px 2px 1px 0; border: 1px solid transparent; font-family: inherit; line-height: 1.5; }
  .badge.declared { background: #dcfce7; color: #15803d; }
  .badge.union { background: #f1f5f9; color: #475569; }
  .badge.shape { background: #ede9fe; color: #6d28d9; border-color: #ddd6fe; cursor: pointer; }
  .badge.shape:hover { text-decoration: underline; }
  .badge.fn { background: #ecfeff; color: #0e7490; }
  .badge.inh { background: #fef3c7; color: #92400e; }

  :global(:is([data-theme="dark"], .dark)) .cf { background: var(--bg-soft); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .cf h4,
  :global(:is([data-theme="dark"], .dark)) .cf h5,
  :global(:is([data-theme="dark"], .dark)) .cf-table th { color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .cf-table td { color: var(--ink-900); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .cf-table th { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .n { background: rgba(255,255,255,0.1); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .link { color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .dt { color: #5eead4; }
  :global(:is([data-theme="dark"], .dark)) .inherited summary { color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .badge.declared { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .badge.union { background: rgba(255,255,255,0.06); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .badge.shape { background: rgba(139,92,246,0.2); color: #c4b5fd; border-color: rgba(139,92,246,0.35); }
  :global(:is([data-theme="dark"], .dark)) .badge.fn { background: rgba(34,211,238,0.18); color: #67e8f9; }
  :global(:is([data-theme="dark"], .dark)) .badge.inh { background: rgba(245,158,11,0.18); color: #fcd34d; }
</style>
