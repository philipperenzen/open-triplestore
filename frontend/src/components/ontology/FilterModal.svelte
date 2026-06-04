<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { X } from 'lucide-svelte';
  import type { FilterState, EntityKind, UsageFlag } from '../../lib/ontology/filters';
  import type { VocabKind } from '../../lib/ontology/vocabularies';

  export let state: FilterState;
  export let onChange: (_s: FilterState) => void = () => {};
  export let onClose: () => void = () => {};

  const VOCAB_OPTIONS: { kind: VocabKind; label: string }[] = [
    { kind: 'owl', label: 'owl' },
    { kind: 'rdfs', label: 'rdfs' },
    { kind: 'rdf', label: 'rdf' },
    { kind: 'sh', label: 'sh' },
    { kind: 'skos', label: 'skos' },
    { kind: 'dcterms', label: 'dcterms' },
    { kind: 'foaf', label: 'foaf' },
    { kind: 'schema', label: 'schema' },
    { kind: 'void', label: 'void' },
    { kind: 'dcat', label: 'dcat' },
    { kind: 'prov', label: 'prov' },
    { kind: 'custom', label: 'custom' },
  ];

  const KIND_OPTIONS: { kind: EntityKind; label: string }[] = [
    { kind: 'class', label: 'class' },
    { kind: 'objectProperty', label: 'object' },
    { kind: 'datatypeProperty', label: 'datatype' },
    { kind: 'annotationProperty', label: 'annotation' },
    { kind: 'rdfProperty', label: 'rdf:Property' },
    { kind: 'concept', label: 'concept' },
    { kind: 'shape', label: 'NodeShape' },
  ];
  const KIND_LABEL_KEYS: Record<string, string> = {
    class: 'components.filterModal.kind.class',
    object: 'components.filterModal.kind.object',
    datatype: 'components.filterModal.kind.datatype',
    annotation: 'components.filterModal.kind.annotation',
    concept: 'components.filterModal.kind.concept',
  };

  const USAGE_OPTIONS: { flag: UsageFlag; labelKey: string; titleKey: string }[] = [
    { flag: 'hasInstances', labelKey: 'components.filterModal.usage.hasInstances.label', titleKey: 'components.filterModal.usage.hasInstances.title' },
    { flag: 'hasSubclasses', labelKey: 'components.filterModal.usage.hasSubclasses.label', titleKey: 'components.filterModal.usage.hasSubclasses.title' },
    { flag: 'isShapeTarget', labelKey: 'components.filterModal.usage.isShapeTarget.label', titleKey: 'components.filterModal.usage.isShapeTarget.title' },
    { flag: 'hasAxioms', labelKey: 'components.filterModal.usage.hasAxioms.label', titleKey: 'components.filterModal.usage.hasAxioms.title' },
    { flag: 'leaf', labelKey: 'components.filterModal.usage.leaf.label', titleKey: 'components.filterModal.usage.leaf.title' },
  ];

  function toggle<T>(set: Set<T>, value: T): Set<T> {
    const next = new Set(set);
    if (next.has(value)) next.delete(value); else next.add(value);
    return next;
  }
  function toggleVocab(v: VocabKind) { onChange({ ...state, vocabs: toggle(state.vocabs, v) }); }
  function toggleKind(k: EntityKind)  { onChange({ ...state, kinds:  toggle(state.kinds,  k) }); }
  function toggleUsage(u: UsageFlag)  { onChange({ ...state, usage:  toggle(state.usage,  u) }); }

  function clearAll() {
    onChange({ ...state, vocabs: new Set(), kinds: new Set(), usage: new Set() });
  }

  $: activeCount = state.vocabs.size + state.kinds.size + state.usage.size;

  function onKeydown(e: KeyboardEvent) { if (e.key === 'Escape') onClose(); }
  onMount(() => window.addEventListener('keydown', onKeydown));
  onDestroy(() => window.removeEventListener('keydown', onKeydown));
</script>

<!-- svelte-ignore a11y-click-events-have-key-events -->
<!-- svelte-ignore a11y-no-static-element-interactions -->
<div class="fm-backdrop" on:click={onClose}>
  <div class="fm-box" on:click|stopPropagation role="dialog" aria-modal="true" aria-label={$t('components.filterModal.heading')} tabindex="-1">
    <div class="fm-head">
      <h3 class="fm-title">{$t('components.filterModal.heading')}{activeCount ? ` (${activeCount})` : ''}</h3>
      <button class="fm-close" on:click={onClose} aria-label={$t('system.close')}><X size={16} /></button>
    </div>

    <div class="fm-body">
      <section class="fm-section">
        <div class="fm-section-label">{$t('components.filterModal.vocabulary')}</div>
        <div class="fm-chips">
          {#each VOCAB_OPTIONS as v}
            <button class="chip" class:active={state.vocabs.has(v.kind)} on:click={() => toggleVocab(v.kind)}>
              {v.label}
            </button>
          {/each}
        </div>
      </section>

      <section class="fm-section">
        <div class="fm-section-label">{$t('components.filterModal.kindLabel')}</div>
        <div class="fm-chips">
          {#each KIND_OPTIONS as k}
            <button class="chip" class:active={state.kinds.has(k.kind)} on:click={() => toggleKind(k.kind)}>
              {KIND_LABEL_KEYS[k.label] ? $t(KIND_LABEL_KEYS[k.label]) : k.label}
            </button>
          {/each}
        </div>
      </section>

      <section class="fm-section">
        <div class="fm-section-label">{$t('components.filterModal.usageLabel')}</div>
        <div class="fm-chips">
          {#each USAGE_OPTIONS as u}
            <button class="chip" class:active={state.usage.has(u.flag)} title={$t(u.titleKey)} on:click={() => toggleUsage(u.flag)}>
              {$t(u.labelKey)}
            </button>
          {/each}
        </div>
      </section>
    </div>

    <div class="fm-foot">
      <button class="btn-ghost" on:click={clearAll} disabled={activeCount === 0}>{$t('components.filterModal.clearAll')}</button>
      <button class="btn-primary" on:click={onClose}>{$t('components.filterModal.done')}</button>
    </div>
  </div>
</div>

<style>
  .fm-backdrop {
    position: fixed; inset: 0; background: rgba(15, 23, 42, 0.45);
    display: flex; align-items: center; justify-content: center;
    z-index: 1000; padding: 1rem;
  }
  .fm-box {
    background: #fff; border-radius: 12px; box-shadow: 0 20px 60px rgba(0,0,0,0.25);
    width: 100%; max-width: 560px; max-height: 88vh;
    display: flex; flex-direction: column; overflow: hidden;
  }
  .fm-head {
    display: flex; align-items: center; justify-content: space-between;
    padding: 0.75rem 1rem; border-bottom: 1px solid #e2e8f0; background: #f8fafc;
  }
  .fm-title { margin: 0; font-size: 0.95rem; font-weight: 600; color: #1e293b; }
  .fm-close {
    border: none; background: transparent; cursor: pointer; color: #64748b;
    padding: 4px; border-radius: 6px; display: inline-flex;
  }
  .fm-close:hover { background: #e2e8f0; color: #1e293b; }

  .fm-body { padding: 0.9rem 1rem; overflow-y: auto; display: flex; flex-direction: column; gap: 0.9rem; }
  .fm-section-label { font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.04em; color: #64748b; margin-bottom: 0.4rem; }
  .fm-chips { display: flex; flex-wrap: wrap; gap: 0.3rem; }

  .chip {
    padding: 3px 10px; border-radius: 999px; border: 1px solid #cbd5e1;
    background: #fff; font-size: 0.78rem; color: #475569; cursor: pointer;
  }
  .chip:hover { background: #eef5ff; }
  .chip.active { background: #1565c0; color: #fff; border-color: #1565c0; }

  .fm-foot {
    display: flex; align-items: center; justify-content: space-between;
    padding: 0.65rem 1rem; border-top: 1px solid #e2e8f0; background: #f8fafc; gap: 0.5rem;
  }
  .btn-ghost {
    padding: 0.4rem 0.85rem; border-radius: 8px; border: 1px solid #cbd5e1; background: #fff;
    font-size: 0.82rem; color: #475569; cursor: pointer;
  }
  .btn-ghost:hover:not(:disabled) { background: #f1f5f9; }
  .btn-ghost:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn-primary {
    padding: 0.4rem 1rem; border-radius: 8px; border: none; background: #1565c0;
    color: #fff; font-size: 0.82rem; font-weight: 600; cursor: pointer;
  }
  .btn-primary:hover { background: #0d4a92; }

  :global(:is([data-theme="dark"], .dark)) .fm-box { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .fm-head { background: var(--bg-soft); border-bottom-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .fm-title { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .fm-close:hover { background: rgba(255,255,255,0.08); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .chip { background: var(--bg-soft); border-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .chip:hover { background: rgba(59,130,246,0.15); }
  :global(:is([data-theme="dark"], .dark)) .fm-foot { background: var(--bg-soft); border-top-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .btn-ghost { background: var(--bg-soft); border-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .btn-ghost:hover:not(:disabled) { background: rgba(255,255,255,0.06); }
</style>
