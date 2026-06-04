<script>
  import { onMount } from 'svelte';
  import { navigate } from '../lib/router/index.js';
  import { ArrowLeft, Loader2, Plus, Minus, RefreshCw } from 'lucide-svelte';
  import { getVocabularyDiff } from '../lib/api.js';
  import { t as i18nT } from 'svelte-i18n';

  export let id;

  const params = new URLSearchParams(window.location.search);
  let fromVer = params.get('from') || '';
  let toVer = params.get('to') || '';

  let diff = null;
  let loading = false;
  let error = '';

  onMount(() => {
    if (fromVer && toVer) load();
  });

  async function load() {
    loading = true;
    error = '';
    diff = null;
    try {
      diff = await getVocabularyDiff(id, fromVer, toVer);
    } catch (e) {
      error = e.message;
    }
    loading = false;
  }

  function shortenIri(iri) {
    const raw = iri.replace(/^<|>$/g, '');
    const lastHash = raw.lastIndexOf('#');
    const lastSlash = raw.lastIndexOf('/');
    const cut = Math.max(lastHash, lastSlash);
    if (cut > 0 && cut < raw.length - 1) return raw.slice(cut + 1);
    return raw;
  }

  function fmtTerm(term) {
    if (!term) return '';
    if (term.startsWith('<') && term.endsWith('>')) return shortenIri(term);
    return term;
  }
</script>

<div class="space-y-6">
  <div class="flex items-center gap-3">
    <button class="btn btn-ghost btn-sm" on:click={() => navigate(`/vocabularies/${id}`)}>
      <ArrowLeft size={16} /> {$i18nT('system.back')}
    </button>
    <h2 class="text-xl font-semibold m-0">{$i18nT('pages.vocabularyDiff.heading')} <code class="text-base font-mono">v{fromVer}</code> → <code class="text-base font-mono">v{toVer}</code></h2>
  </div>

  <div class="flex flex-wrap items-end gap-3">
    <div>
      <label class="label" for="diff-from">{$i18nT('pages.vocabularyDiff.fromVersion')}</label>
      <input id="diff-from" type="text" class="input w-32" bind:value={fromVer} placeholder="1.0.0" />
    </div>
    <div>
      <label class="label" for="diff-to">{$i18nT('pages.vocabularyDiff.toVersion')}</label>
      <input id="diff-to" type="text" class="input w-32" bind:value={toVer} placeholder="1.1.0" />
    </div>
    <button class="btn btn-primary btn-sm" on:click={load} disabled={!fromVer || !toVer || loading}>
      {#if loading}<Loader2 size={14} class="animate-spin" />{:else}<RefreshCw size={14} />{/if}
      {$i18nT('pages.vocabularyDiff.compare')}
    </button>
  </div>

  {#if loading}
    <div class="flex items-center justify-center py-16 text-[var(--ink-400)]">
      <Loader2 size={24} class="animate-spin mr-2" /> {$i18nT('pages.vocabularyDiff.computing')}
    </div>
  {:else if error}
    <div class="p-4 rounded-xl bg-red-50 border border-red-200 text-red-700 text-sm">{error}</div>
  {:else if diff}
    <div class="flex gap-4 flex-wrap">
      <div class="px-4 py-2 rounded-xl bg-green-50 border border-green-200">
        <span class="text-green-700 font-semibold text-lg">{diff.summary.added}</span>
        <span class="text-green-600 text-sm ml-1">{$i18nT('pages.vocabularyDiff.added')}</span>
      </div>
      <div class="px-4 py-2 rounded-xl bg-red-50 border border-red-200">
        <span class="text-red-700 font-semibold text-lg">{diff.summary.removed}</span>
        <span class="text-red-600 text-sm ml-1">{$i18nT('pages.vocabularyDiff.removed')}</span>
      </div>
      <div class="px-4 py-2 rounded-xl bg-amber-50 border border-amber-200">
        <span class="text-amber-700 font-semibold text-lg">{diff.summary.changed}</span>
        <span class="text-amber-600 text-sm ml-1">{$i18nT('pages.vocabularyDiff.changed')}</span>
      </div>
    </div>

    {#if diff.summary.added === 0 && diff.summary.removed === 0 && diff.summary.changed === 0}
      <p class="text-[var(--ink-400)] text-sm text-center py-8">{$i18nT('pages.vocabularyDiff.identical')}</p>
    {/if}

    {#if diff.added.length > 0}
      <section>
        <h3 class="text-base font-semibold text-green-700 mb-2 flex items-center gap-1.5">
          <Plus size={16} /> {$i18nT('pages.vocabularyDiff.addedSection', { values: { count: diff.added.length } })}
        </h3>
        <div class="diff-table added">
          <div class="diff-header"><span>{$i18nT('pages.vocabularyDiff.subject')}</span><span>{$i18nT('pages.vocabularyDiff.predicate')}</span><span>{$i18nT('pages.vocabularyDiff.object')}</span></div>
          {#each diff.added as t}
            <div class="diff-row">
              <span class="truncate" title={t.s}>{fmtTerm(t.s)}</span>
              <span class="truncate" title={t.p}>{fmtTerm(t.p)}</span>
              <span class="truncate" title={t.o}>{fmtTerm(t.o)}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if diff.removed.length > 0}
      <section>
        <h3 class="text-base font-semibold text-red-700 mb-2 flex items-center gap-1.5">
          <Minus size={16} /> {$i18nT('pages.vocabularyDiff.removedSection', { values: { count: diff.removed.length } })}
        </h3>
        <div class="diff-table removed">
          <div class="diff-header"><span>{$i18nT('pages.vocabularyDiff.subject')}</span><span>{$i18nT('pages.vocabularyDiff.predicate')}</span><span>{$i18nT('pages.vocabularyDiff.object')}</span></div>
          {#each diff.removed as t}
            <div class="diff-row">
              <span class="truncate" title={t.s}>{fmtTerm(t.s)}</span>
              <span class="truncate" title={t.p}>{fmtTerm(t.p)}</span>
              <span class="truncate" title={t.o}>{fmtTerm(t.o)}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if diff.changed.length > 0}
      <section>
        <h3 class="text-base font-semibold text-amber-700 mb-2">{$i18nT('pages.vocabularyDiff.changedSection', { values: { count: diff.changed.length } })}</h3>
        <div class="space-y-2">
          {#each diff.changed as c}
            <div class="rounded-xl border border-amber-200 overflow-hidden">
              <div class="bg-amber-50 px-3 py-1.5 text-xs text-amber-700">
                <span class="font-mono">{fmtTerm(c.s)}</span> · <span class="font-mono">{fmtTerm(c.p)}</span>
              </div>
              <div class="grid grid-cols-2 divide-x divide-amber-100">
                <div class="px-3 py-2 bg-red-50 text-xs text-red-700 font-mono break-all">− {fmtTerm(c.before)}</div>
                <div class="px-3 py-2 bg-green-50 text-xs text-green-700 font-mono break-all">+ {fmtTerm(c.after)}</div>
              </div>
            </div>
          {/each}
        </div>
      </section>
    {/if}
  {/if}
</div>

<style>
  .btn { display: inline-flex; align-items: center; gap: 0.375rem; padding: 0.5rem 1rem; border-radius: 0.75rem; font-size: 0.875rem; font-weight: 500; cursor: pointer; border: none; transition: all 0.15s; }
  .btn-primary { background: var(--brand-500, #6366f1); color: white; }
  .btn-primary:hover:not(:disabled) { background: var(--brand-600, #4f46e5); }
  .btn-ghost { background: transparent; color: var(--ink-600, #475569); }
  .btn-ghost:hover { background: var(--bg-soft, #f1f5f9); }
  .btn-sm { padding: 0.375rem 0.75rem; font-size: 0.8125rem; }
  .btn:disabled { opacity: 0.6; cursor: not-allowed; }
  .label { display: block; font-size: 0.8125rem; font-weight: 500; margin-bottom: 0.25rem; color: var(--ink-600); }
  .input { padding: 0.4rem 0.6rem; border: 1px solid var(--line-soft); border-radius: 0.6rem; font-size: 0.875rem; }
  .diff-table { border: 1px solid; border-radius: 0.75rem; overflow: hidden; font-size: 0.8125rem; }
  .diff-table.added { border-color: #bbf7d0; }
  .diff-table.removed { border-color: #fecaca; }
  .diff-header { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 0.5rem; padding: 0.5rem 0.75rem; font-weight: 600; font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.05em; background: #f8fafc; color: var(--ink-400); border-bottom: 1px solid #e2e8f0; }
  .diff-row { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 0.5rem; padding: 0.4rem 0.75rem; font-family: monospace; border-bottom: 1px solid; }
  .diff-table.added .diff-row { background: #f0fdf4; border-color: #dcfce7; color: #15803d; }
  .diff-table.added .diff-row:last-child { border-bottom: none; }
  .diff-table.removed .diff-row { background: #fff7f7; border-color: #fee2e2; color: #b91c1c; }
  .diff-table.removed .diff-row:last-child { border-bottom: none; }

  :global(:is([data-theme="dark"], .dark)) .diff-table.added { border-color: rgba(16,185,129,0.4); }
  :global(:is([data-theme="dark"], .dark)) .diff-table.removed { border-color: rgba(239,68,68,0.4); }
  :global(:is([data-theme="dark"], .dark)) .diff-header { background: var(--bg-soft); border-bottom-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .diff-table.added .diff-row { background: rgba(16,185,129,0.1); border-color: rgba(16,185,129,0.25); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .diff-table.removed .diff-row { background: rgba(239,68,68,0.1); border-color: rgba(239,68,68,0.25); color: #fca5a5; }
</style>
