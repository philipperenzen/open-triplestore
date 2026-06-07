<script lang="ts">
  // Prefix / vocabulary finder.
  //
  // A search box over three prefix sources — curated built-ins, the prefix.cc
  // community registry, and on-platform registered vocabularies — surfaced with
  // their full namespace IRI, a source badge and (when known) a short "about"
  // description. Each result has an Add action; already-declared prefixes are
  // shown as "Added".
  //
  // Self-contained: parents listen for `add` events and decide how to insert the
  // PREFIX declaration. Does not touch the editor itself.
  import { onMount, onDestroy, createEventDispatcher } from 'svelte';
  import { Search, Loader2, Plus, Check, ExternalLink, X } from 'lucide-svelte';
  import { searchPrefixes } from '../lib/ontology/prefixService.js';
  import type { PrefixCandidate, PrefixSource } from '../lib/ontology/prefixService.js';
  import { listPlatformPrefixes } from '../lib/api.js';
  import type { PlatformPrefix } from '../lib/api.js';
  import { kindOf } from '../lib/ontology/vocabularies.js';
  import { safeExternalUrl } from '../lib/safeUrl.js';
  import { t } from 'svelte-i18n';

  /** Prefix labels already declared in the target — shown as "Added"/disabled. */
  export let existing: Set<string> | string[] = [];
  export let placeholder = '';
  export let autofocus = false;
  /** Cap on rendered results. */
  export let limit = 30;

  const dispatch = createEventDispatcher<{ add: { prefix: string; namespace: string } }>();

  // Normalise `existing` to a lookup set of lowercased labels.
  $: existingSet = new Set(
    (Array.isArray(existing) ? existing : [...existing]).map((p) => String(p).toLowerCase()),
  );
  function isAdded(prefix: string): boolean {
    return existingSet.has(prefix.toLowerCase());
  }

  let query = '';
  let results: PrefixCandidate[] = [];
  let loading = false;
  let searched = false;
  let platform: PlatformPrefix[] = [];

  let inputEl: HTMLInputElement | null = null;
  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  let reqId = 0;

  onMount(async () => {
    if (autofocus && inputEl) inputEl.focus();
    // Best-effort: registered vocabularies. Failure (e.g. anonymous) is fine.
    try {
      platform = await listPlatformPrefixes();
    } catch {
      platform = [];
    }
    // Show a useful default list (built-ins + platform) before the user types.
    runSearch();
  });

  onDestroy(() => {
    if (debounceTimer) clearTimeout(debounceTimer);
  });

  async function runSearch() {
    const id = ++reqId;
    loading = true;
    try {
      const res = await searchPrefixes(query, platform, { limit });
      // Ignore out-of-order responses (a later keystroke already superseded us).
      if (id === reqId) {
        results = res;
        searched = true;
      }
    } catch {
      if (id === reqId) results = [];
    } finally {
      if (id === reqId) loading = false;
    }
  }

  function onInput() {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(runSearch, 180);
  }

  function clear() {
    query = '';
    runSearch();
    inputEl?.focus();
  }

  function add(c: PrefixCandidate) {
    if (isAdded(c.prefix)) return;
    dispatch('add', { prefix: c.prefix, namespace: c.namespace });
  }

  // Reactive so the source badges re-translate when the locale changes.
  $: SOURCE_LABEL = {
    builtin: $t('components.prefixSearch.sourceBuiltin'),
    'prefix.cc': 'prefix.cc',
    platform: $t('components.prefixSearch.sourcePlatform'),
  } as Record<PrefixSource, string>;

  // The vocabulary "kind" to show next to the description. `tr` (the $t store) is
  // passed in so the template call re-runs on locale change.
  function kindLabel(c: PrefixCandidate, tr: (_id: string) => string): string {
    if (c.source === 'platform') {
      return (c as PlatformPrefix).kind === 'vocabulary'
        ? tr('components.prefixSearch.kindVocabulary')
        : tr('components.prefixSearch.kindDataModel');
    }
    const k = kindOf(c.namespace);
    return k === 'custom' ? '' : k;
  }
</script>

<div class="ps">
  <div class="ps-search">
    <Search size={14} class="ps-search-icon" />
    <input
      bind:this={inputEl}
      type="text"
      class="ps-input"
      placeholder={placeholder || $t('components.prefixSearch.placeholder')}
      bind:value={query}
      on:input={onInput}
      aria-label={$t('components.prefixSearch.ariaSearch')}
    />
    {#if loading}
      <Loader2 size={14} class="animate-spin ps-spin" />
    {:else if query}
      <button class="ps-clear" on:click={clear} title={$t('components.prefixSearch.clear')} aria-label={$t('components.prefixSearch.clearSearch')}>
        <X size={13} />
      </button>
    {/if}
  </div>

  <div class="ps-results" role="list">
    {#if loading && results.length === 0}
      <div class="ps-state"><Loader2 size={16} class="animate-spin" /> {$t('components.prefixSearch.searching')}</div>
    {:else if results.length === 0}
      <div class="ps-state">
        {#if searched && query}
          {$t('components.prefixSearch.noMatch', { values: { query } })}
        {:else}
          {$t('components.prefixSearch.hint')}
        {/if}
      </div>
    {:else}
      {#each results as c (c.source + ':' + c.prefix)}
        {@const added = isAdded(c.prefix)}
        {@const kind = kindLabel(c, $t)}
        {@const home = safeExternalUrl(c.homepage)}
        <div class="ps-row" role="listitem">
          <div class="ps-main">
            <div class="ps-head">
              <code class="ps-prefix">{c.prefix}:</code>
              <span class="ps-badge ps-src-{c.source.replace('.', '-')}">{SOURCE_LABEL[c.source]}</span>
              {#if kind}<span class="ps-kind">{kind}</span>{/if}
              {#if home}
                <a class="ps-home" href={home} target="_blank" rel="noopener noreferrer" title={$t('components.prefixSearch.openHomepage')}>
                  <ExternalLink size={11} />
                </a>
              {/if}
            </div>
            <code class="ps-ns" title={c.namespace}>{c.namespace}</code>
            {#if c.title || c.description}
              <p class="ps-about">
                {#if c.title}<span class="ps-title">{c.title}.</span>{/if}
                {#if c.description}{c.description}{/if}
              </p>
            {/if}
          </div>
          <button
            class="ps-add"
            class:ps-added={added}
            disabled={added}
            on:click={() => add(c)}
            title={added ? $t('components.prefixSearch.alreadyDeclared') : $t('components.prefixSearch.addPrefix', { values: { prefix: c.prefix } })}
          >
            {#if added}
              <Check size={13} /> {$t('components.prefixSearch.added')}
            {:else}
              <Plus size={13} /> {$t('components.prefixSearch.add')}
            {/if}
          </button>
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .ps {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 10px;
    background: #fff;
    overflow: hidden;
  }

  /* Search box */
  .ps-search {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.4rem 0.6rem;
    border-bottom: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg-soft, #f8fafc);
    color: var(--ink-400, #94a3b8);
  }
  .ps-input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    font-size: 0.85rem;
    color: var(--ink-800, #1e293b);
  }
  .ps-clear {
    border: none;
    background: transparent;
    cursor: pointer;
    color: var(--ink-400, #94a3b8);
    display: inline-flex;
    align-items: center;
    padding: 2px;
    border-radius: 4px;
  }
  .ps-clear:hover { color: var(--ink-700, #334155); background: rgba(0, 0, 0, 0.05); }
  .ps-spin { color: var(--ink-400, #94a3b8); }

  /* Results */
  .ps-results {
    overflow-y: auto;
    max-height: 360px;
  }
  .ps-state {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.45rem;
    padding: 1.6rem 1rem;
    color: var(--ink-400, #94a3b8);
    font-size: 0.82rem;
    text-align: center;
  }

  .ps-row {
    display: flex;
    align-items: flex-start;
    gap: 0.6rem;
    padding: 0.55rem 0.7rem;
    border-bottom: 1px solid var(--line-soft, #f1f5f9);
  }
  .ps-row:last-child { border-bottom: none; }
  .ps-row:hover { background: var(--bg-soft, #f8fafc); }

  .ps-main { flex: 1; min-width: 0; }
  .ps-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .ps-prefix {
    font-family: 'SF Mono', ui-monospace, monospace;
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--ink-900, #0f172a);
  }
  .ps-badge {
    font-size: 0.62rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.02em;
    padding: 1px 6px;
    border-radius: 999px;
    background: #f1f5f9;
    color: #475569;
    white-space: nowrap;
  }
  .ps-src-builtin { background: #dbeafe; color: #1d4ed8; }
  .ps-src-prefix-cc { background: #ede9fe; color: #6d28d9; }
  .ps-src-platform { background: #dcfce7; color: #15803d; }
  .ps-kind {
    font-size: 0.65rem;
    font-weight: 600;
    color: var(--ink-400, #94a3b8);
    text-transform: lowercase;
  }
  .ps-home {
    display: inline-flex;
    align-items: center;
    color: var(--ink-400, #94a3b8);
    line-height: 1;
  }
  .ps-home:hover { color: var(--brand-600, #4f46e5); }

  .ps-ns {
    display: block;
    margin-top: 2px;
    font-family: 'SF Mono', ui-monospace, monospace;
    font-size: 0.7rem;
    color: var(--ink-500, #64748b);
    word-break: break-all;
  }
  .ps-about {
    margin: 0.25rem 0 0;
    font-size: 0.74rem;
    line-height: 1.35;
    color: var(--ink-600, #475569);
  }
  .ps-title { font-weight: 600; color: var(--ink-700, #334155); }

  /* Add button */
  .ps-add {
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    padding: 0.28rem 0.6rem;
    border: 1px solid var(--brand-500, #6366f1);
    border-radius: 6px;
    background: var(--brand-500, #6366f1);
    color: #fff;
    font-size: 0.72rem;
    font-weight: 600;
    cursor: pointer;
    white-space: nowrap;
    transition: background 0.12s, border-color 0.12s;
  }
  .ps-add:hover { background: var(--brand-600, #4f46e5); border-color: var(--brand-600, #4f46e5); }
  .ps-add.ps-added {
    background: transparent;
    color: var(--ink-400, #94a3b8);
    border-color: var(--line-soft, #e2e8f0);
    cursor: default;
  }

  /* Dark theme */
  :global(:is([data-theme='dark'], .dark)) .ps { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme='dark'], .dark)) .ps-search { background: var(--bg-soft); border-bottom-color: var(--line-strong); }
  :global(:is([data-theme='dark'], .dark)) .ps-input { color: var(--ink-900); }
  :global(:is([data-theme='dark'], .dark)) .ps-clear:hover { background: rgba(255, 255, 255, 0.08); color: var(--ink-900); }
  :global(:is([data-theme='dark'], .dark)) .ps-row { border-bottom-color: var(--line-soft); }
  :global(:is([data-theme='dark'], .dark)) .ps-row:hover { background: rgba(255, 255, 255, 0.04); }
  :global(:is([data-theme='dark'], .dark)) .ps-prefix { color: var(--ink-900); }
  :global(:is([data-theme='dark'], .dark)) .ps-badge { background: rgba(255, 255, 255, 0.06); color: var(--ink-400); }
  :global(:is([data-theme='dark'], .dark)) .ps-src-builtin { background: rgba(59, 130, 246, 0.2); color: #93c5fd; }
  :global(:is([data-theme='dark'], .dark)) .ps-src-prefix-cc { background: rgba(139, 92, 246, 0.2); color: #c4b5fd; }
  :global(:is([data-theme='dark'], .dark)) .ps-src-platform { background: rgba(16, 185, 129, 0.18); color: #6ee7b7; }
  :global(:is([data-theme='dark'], .dark)) .ps-ns { color: var(--ink-400); }
  :global(:is([data-theme='dark'], .dark)) .ps-about { color: var(--ink-500); }
  :global(:is([data-theme='dark'], .dark)) .ps-title { color: var(--ink-700); }
  :global(:is([data-theme='dark'], .dark)) .ps-add.ps-added { border-color: var(--line-strong); color: var(--ink-400); }
</style>
