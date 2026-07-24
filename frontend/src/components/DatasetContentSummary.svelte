<script>
  import { t } from 'svelte-i18n';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { Layers, Boxes, Puzzle, Shapes, Tag, GitBranch } from 'lucide-svelte';

  /**
   * A lightweight, always-on "what's in this dataset" card, driven by the cheap
   * facet-derived summary from datasetContentKind(). Replaces the old 30-60s
   * content-kind scan for the informational case; the actionable mismatch warning
   * lives in ContentKindWarning.
   * @type {{ summary?: any, loading?: boolean }}
   */
  let { summary = null, loading = false } = $props();

  // Verdict → a friendly label + icon for the "kind" chip.
  const KIND = {
    model:      { icon: Layers,    label: 'components.datasetContentSummary.kindModel',      cls: 'k-model' },
    vocabulary: { icon: Tag,       label: 'components.datasetContentSummary.kindVocabulary', cls: 'k-vocab' },
    shapes:     { icon: Shapes,    label: 'components.datasetContentSummary.kindShapes',     cls: 'k-shapes' },
    entailment: { icon: GitBranch, label: 'components.datasetContentSummary.kindEntailment', cls: 'k-entail' },
    instances:  { icon: Boxes,     label: 'components.datasetContentSummary.kindInstances',  cls: 'k-inst' },
    mixed:      { icon: Puzzle,    label: 'components.datasetContentSummary.kindMixed',       cls: 'k-mixed' },
  };
  let kind = $derived(summary ? KIND[summary.verdict] : null);
  let plus = $derived(summary?.capped ? '+' : '');
</script>

{#if loading}
  <div class="dcs-card" aria-hidden="true">
    <span class="skel dcs-skel-badge"></span>
    <span class="skel dcs-skel-line"></span>
  </div>
{:else if summary && summary.verdict !== 'empty'}
  <div class="dcs-card">
    {#if kind}
      {@const KindIcon = kind.icon}
      <span class="dcs-kind {kind.cls}"><KindIcon size={13} /> {$t(kind.label)}</span>
    {/if}
    <div class="dcs-metrics">
      <span class="dcs-metric"><b>{summary.instanceTypeCount.toLocaleString()}{plus}</b> {$t('components.datasetContentSummary.types')}</span>
      <span class="dcs-dot">·</span>
      <span class="dcs-metric"><b>{summary.predicateCount.toLocaleString()}{plus}</b> {$t('components.datasetContentSummary.properties')}</span>
      <span class="dcs-dot">·</span>
      <span class="dcs-metric"><b>{summary.instanceCount.toLocaleString()}{plus}</b> {$t('components.datasetContentSummary.instances')}</span>
    </div>
    {#if summary.sampleTypes?.length}
      <div class="dcs-types">
        <span class="dcs-types-label">{$t('components.datasetContentSummary.topTypes')}</span>
        {#each summary.sampleTypes as s}
          <span class="dcs-type" title={s.cls}>{shortenIRI(s.cls)} <span class="dcs-type-n">{s.count.toLocaleString()}</span></span>
        {/each}
      </div>
    {/if}
  </div>
{/if}

<style>
  .dcs-card {
    display: flex; align-items: center; flex-wrap: wrap; gap: 0.5rem 0.75rem;
    padding: 0.55rem 0.8rem; border: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg-soft, #f8fafc); border-radius: 12px; font-size: 0.82rem;
    color: var(--ink-700, #334155);
  }
  .dcs-kind {
    display: inline-flex; align-items: center; gap: 0.3rem; font-weight: 700;
    font-size: 0.74rem; padding: 2px 9px; border-radius: 999px;
  }
  .k-model  { background: #ede9fe; color: #6d28d9; }
  .k-vocab  { background: #fef3c7; color: #b45309; }
  .k-shapes { background: #fae8ff; color: #a21caf; }
  .k-entail { background: #e0e7ff; color: #4338ca; }
  .k-inst   { background: #dbeafe; color: #1d4ed8; }
  .k-mixed  { background: #e2e8f0; color: #475569; }

  .dcs-metrics { display: inline-flex; align-items: center; gap: 0.4rem; }
  .dcs-metric b { color: var(--ink-900, #0f172a); font-variant-numeric: tabular-nums; }
  .dcs-dot { color: var(--ink-300, #cbd5e1); }

  .dcs-types { display: inline-flex; align-items: center; flex-wrap: wrap; gap: 0.3rem; margin-left: auto; }
  .dcs-types-label { color: var(--ink-400, #94a3b8); font-size: 0.74rem; }
  .dcs-type {
    display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.74rem;
    padding: 1px 7px; border-radius: 999px; background: var(--surface, #fff);
    border: 1px solid var(--line-soft, #e2e8f0); color: var(--ink-700, #334155);
  }
  .dcs-type-n { color: var(--ink-400, #94a3b8); font-variant-numeric: tabular-nums; }

  .dcs-skel-badge { display: inline-block; width: 5rem; height: 1.1rem; border-radius: 999px; }
  .dcs-skel-line { display: inline-block; width: 14rem; height: 0.9rem; border-radius: 6px; }

  :global(:is([data-theme="dark"], .dark)) .dcs-card { background: var(--bg-soft); border-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .dcs-type { background: var(--bg-strong); border-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .k-model  { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .k-vocab  { background: rgba(245,158,11,0.2); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .k-shapes { background: rgba(217,70,239,0.18); color: #f0abfc; }
  :global(:is([data-theme="dark"], .dark)) .k-entail { background: rgba(99,102,241,0.2); color: #a5b4fc; }
  :global(:is([data-theme="dark"], .dark)) .k-inst   { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .k-mixed  { background: rgba(148,163,184,0.2); color: #cbd5e1; }
</style>
