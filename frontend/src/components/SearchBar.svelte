<script>
  import { onDestroy } from 'svelte';
  import { t } from 'svelte-i18n';
  import { navigate } from '../lib/router/index.js';
  import { listDatasets } from '../lib/api.js';
  import { filterByQuery } from '../lib/searchMatch';
  import { Search, ArrowRight, ChevronRight, Database, BookOpen, Building2, Rows3, Terminal, Upload } from 'lucide-svelte';

  export let onclose = () => {};
  export let selectedDataset = null;

  let query = '';
  let inputEl;
  let showSuggestions = false;
  let selectedSuggestionIdx = -1;
  let recentSearches = JSON.parse(localStorage.getItem('recentSearches') || '[]');

  const quickActions = [
    { label: () => $t('search.browseTriples'), path: '/browse', icon: Rows3 },
    { label: () => $t('search.openSparql'), path: '/sparql', icon: Terminal },
    { label: () => $t('search.importData'), path: '/import', icon: Upload },
  ];

  const navShortcuts = [
    { label: () => $t('nav.datasets'), path: '/datasets', icon: Database, meta: () => $t('nav.datasetsMeta') },
    { label: () => $t('nav.organisations'), path: '/organisations', icon: Building2, meta: () => $t('nav.organisationsMeta') },
    { label: () => $t('components.searchBar.modelRegistry'), path: '/models', icon: BookOpen, meta: () => $t('components.searchBar.modelRegistryMeta') },
  ];

  // Dataset suggestions come from the API, which scopes the list to what this
  // caller may actually see (public datasets only, for a guest).
  //
  // `/api/datasets` takes no query parameter, so the debounce guards ONE fetch
  // rather than one request per keystroke: we wait for a pause in typing, load
  // the accessible datasets once, and filter them in memory from then on. The
  // load is deferred until the user actually types, so merely opening the
  // palette costs no request — and App.svelte mounts this component fresh each
  // time the palette opens, so the list cannot go stale within a session.
  const SUGGEST_DEBOUNCE_MS = 200;
  const MAX_RECENT_SUGGESTIONS = 3;
  const MAX_DATASET_SUGGESTIONS = 3;
  const MAX_SUGGESTIONS = 5;

  let datasets = [];
  let datasetsRequested = false;
  let debounceTimer = null;

  /**
   * Load the caller's accessible datasets once.
   *
   * Fails soft: a rejected request — or a guest with nothing public to see —
   * leaves `datasets` empty and the palette running on recent searches alone,
   * rather than surfacing an error in a transient overlay.
   */
  function loadDatasets() {
    if (datasetsRequested) return;
    datasetsRequested = true;
    listDatasets()
      .then((rows) => { datasets = Array.isArray(rows) ? rows : []; })
      .catch(() => { datasets = []; });
  }

  function scheduleDatasetLoad() {
    if (datasetsRequested) return;
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(loadDatasets, SUGGEST_DEBOUNCE_MS);
  }

  onDestroy(() => clearTimeout(debounceTimer));

  /** Drop repeats, so a recent search that is also a dataset name shows once. */
  function dedupeByValue(items) {
    const seen = new Set();
    const out = [];
    for (const item of items) {
      if (!item.value || seen.has(item.value)) continue;
      seen.add(item.value);
      out.push(item);
    }
    return out;
  }

  $: filteredNav = query.trim().length > 0
    ? navShortcuts.filter(n => n.label().toLowerCase().includes(query.toLowerCase()))
    : navShortcuts;

  // Matched through the shared `filterByQuery`, so the palette folds case and
  // accents like every other filter box — a hand-rolled `includes()` only
  // matched the old lower-case mock names.
  $: datasetSuggestions = query.trim().length > 0
    ? filterByQuery(datasets, query, (d) => [d.name, d.id])
        .map((d) => d.name || d.id)
        .filter(Boolean)
    : [];

  // Recent searches are matched against the query too. They used to be listed
  // unconditionally, which was unremarkable next to three fixed mock names but
  // is not next to real ones: unrelated recents would take the first three of
  // five slots and push out the datasets the user is actually looking for.
  $: recentSuggestions = query.trim().length > 0
    ? filterByQuery(recentSearches, query, (term) => [term])
    : [];

  $: suggestions = query.trim().length > 0
    ? dedupeByValue([
        ...recentSuggestions
          .slice(0, MAX_RECENT_SUGGESTIONS)
          .map((value) => ({ value, kind: 'recent' })),
        ...datasetSuggestions
          .slice(0, MAX_DATASET_SUGGESTIONS)
          .map((value) => ({ value, kind: 'dataset' })),
      ]).slice(0, MAX_SUGGESTIONS)
    : [];

  function submit() {
    // A keyboard-highlighted suggestion takes precedence over the typed text;
    // adopt it as the query and fall through to the normal navigation logic.
    if (selectedSuggestionIdx >= 0 && suggestions[selectedSuggestionIdx]) {
      query = suggestions[selectedSuggestionIdx].value;
      selectedSuggestionIdx = -1;
    }

    const value = query.trim();
    if (!value) return;
    addRecent(value);
    if (value.startsWith('http://') || value.startsWith('https://') || value.startsWith('urn:')) {
      navigate(`/resource?iri=${encodeURIComponent(value)}`);
    } else {
      navigate(`/browse?subject=${encodeURIComponent(value)}`);
    }
    query = '';
    showSuggestions = false;
    selectedSuggestionIdx = -1;
    onclose();
  }

  function addRecent(term) {
    recentSearches = [term, ...recentSearches.filter(s => s !== term)].slice(0, 8);
    localStorage.setItem('recentSearches', JSON.stringify(recentSearches));
  }

  function useSuggestion(term) {
    query = term;
    showSuggestions = false;
    submit();
  }

  function goAction(path) {
    navigate(path);
    onclose();
  }

  function handleKeydown(e) {
    if (e.key === 'Escape') {
      showSuggestions = false;
      onclose();
      return;
    }

    if (e.key === 'ArrowDown') {
      e.preventDefault();
      selectedSuggestionIdx = Math.min(selectedSuggestionIdx + 1, suggestions.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      selectedSuggestionIdx = Math.max(selectedSuggestionIdx - 1, -1);
    } else if (e.key === 'Enter' && showSuggestions) {
      e.preventDefault();
      submit();
    }
  }

  function handleInput() {
    showSuggestions = query.trim().length > 0;
    selectedSuggestionIdx = -1;
    if (showSuggestions) scheduleDatasetLoad();
  }

  export function focus() {
    inputEl?.focus();
  }
</script>

<div class="flex flex-col gap-5">
  <form on:submit|preventDefault={submit} class="relative flex flex-col">
    <div class="relative flex items-center gap-2">
      <div class="flex-1 relative">
        <Search class="absolute left-4 top-1/2 -translate-y-1/2 text-ink-400 shrink-0 pointer-events-none" size={18} />
        <input
          id="global-search"
          bind:this={inputEl}
          bind:value={query}
          on:keydown={handleKeydown}
          on:input={handleInput}
          on:focus={() => { showSuggestions = query.trim().length > 0; if (showSuggestions) scheduleDatasetLoad(); }}
          on:blur={() => { setTimeout(() => { showSuggestions = false; }, 150); }}
          placeholder={$t('search.placeholder')}
          class="w-full pl-14 pr-4 py-3 bg-white border border-[var(--line-soft)] rounded-2xl text-base focus:border-transparent focus:shadow-sm focus:shadow-[var(--brand-500)]/30 focus:bg-white transition-all"
          aria-label={$t('search.placeholder')}
        />
      </div>
      <button type="submit" class="btn btn-sm flex items-center gap-2 whitespace-nowrap shrink-0">
        <ArrowRight size={14} class="shrink-0" />
        {$t('search.open')}
      </button>
    </div>

    {#if selectedDataset}
      <div class="flex items-center gap-2 mt-2 px-1">
        <span class="text-xs text-ink-500 font-medium">{$t('components.searchBar.datasetLabel')}</span>
        <span class="inline-flex items-center gap-1.5 px-2.5 py-1 rounded-lg bg-brand-100/40 text-brand-700 border border-brand-200/50 text-xs font-medium">
          {selectedDataset}
        </span>
      </div>
    {/if}

    <!-- In flow, not absolutely positioned: the sections below are hidden while
         suggestions are open (see the `{#if !showSuggestions}` block), so there
         is nothing to overlay — and an overlay here was clipped away entirely,
         because the modal collapses to the height of the input and
         `.search-modal` in App.svelte hides its overflow to keep its corners
         rounded. Letting the panel take up space grows the modal instead. -->
    {#if showSuggestions && suggestions.length > 0}
      <div class="mt-2 bg-white border border-[var(--line-soft)] rounded-xl shadow-lg overflow-hidden">
        <div class="max-h-64 overflow-y-auto">
          {#each suggestions as suggestion, idx}
            <button
              type="button"
              data-suggestion-kind={suggestion.kind}
              on:click={() => useSuggestion(suggestion.value)}
              on:mouseenter={() => { selectedSuggestionIdx = idx; }}
              class="w-full text-left px-4 py-2.5 flex items-center justify-between hover:bg-brand-50 transition-colors border-b border-line-soft/30 text-sm"
              class:bg-brand-50={selectedSuggestionIdx === idx}
            >
              <span class="flex items-center gap-2.5">
                <!-- Now that the names are real, the icon says where one came
                     from: a dataset on this instance, or something you typed. -->
                <svelte:component
                  this={suggestion.kind === 'dataset' ? Database : Search}
                  size={14}
                  class="text-ink-400 shrink-0"
                />
                <span class="text-ink-900">{suggestion.value}</span>
              </span>
              <ChevronRight size={14} class="text-ink-300 shrink-0" />
            </button>
          {/each}
        </div>
      </div>
    {/if}
  </form>

  {#if !showSuggestions}
    <!-- Navigate to -->
    <div>
      <div class="search-section-label">{$t('components.searchBar.navigateTo')}</div>
      <div class="grid grid-cols-3 gap-2">
        {#each filteredNav as item}
          <button
            class="nav-shortcut"
            on:click={() => goAction(item.path)}
          >
            <span class="nav-shortcut-icon"><svelte:component this={item.icon} size={16} class="shrink-0" /></span>
            <span class="nav-shortcut-text">
              <span class="font-medium text-[var(--ink-900)]">{item.label()}</span>
              <span class="text-[var(--ink-400)] text-[0.7rem] leading-tight">{item.meta()}</span>
            </span>
          </button>
        {/each}
      </div>
    </div>

    {#if recentSearches.length > 0}
      <div>
        <div class="search-section-label">{$t('search.recentSearches')}</div>
        <div class="flex flex-wrap gap-1.5">
          {#each recentSearches.slice(0, 4) as term}
            <button
              class="inline-flex items-center gap-2 px-2.5 py-1.5 rounded-lg bg-white/60 border border-[var(--line-soft)] text-[var(--ink-700)] hover:bg-white hover:border-[var(--brand-300)] transition-all text-xs cursor-pointer"
              on:click={() => useSuggestion(term)}
            >
              <Search size={11} class="shrink-0 text-[var(--ink-400)]" />
              <span class="max-w-[200px] truncate">{term}</span>
            </button>
          {/each}
        </div>
      </div>
    {/if}

    <div>
      <div class="search-section-label">{$t('search.quickActions')}</div>
      <div class="flex flex-wrap gap-1.5">
        {#each quickActions as action}
          <button
            class="inline-flex items-center gap-2 px-3 py-1.5 rounded-lg bg-[var(--bg-accent-soft)] text-[var(--brand-600)] hover:bg-[var(--brand-300)]/30 font-medium transition-all text-xs cursor-pointer"
            on:click={() => goAction(action.path)}
          >
            <svelte:component this={action.icon} size={12} class="shrink-0" />
            {action.label()}
          </button>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .search-section-label {
    font-size: 0.7rem;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--ink-400, #94a3b8);
    margin-bottom: 0.5rem;
  }

  .nav-shortcut {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    padding: 0.625rem 0.75rem;
    border-radius: 0.875rem;
    border: 1px solid var(--line-soft, #e2e8f0);
    background: white;
    cursor: pointer;
    transition: all 0.15s;
    text-align: left;
    min-width: 0;
  }
  .nav-shortcut:hover {
    border-color: var(--brand-300, #a5b4fc);
    background: var(--bg-accent-soft, #f0f4ff);
    transform: translateY(-1px);
    box-shadow: 0 2px 8px rgba(99,102,241,0.12);
  }

  .nav-shortcut-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 2rem;
    height: 2rem;
    border-radius: 0.625rem;
    background: var(--bg-accent-soft, #f0f4ff);
    color: var(--brand-600, #4f46e5);
    flex-shrink: 0;
  }
  .nav-shortcut:hover .nav-shortcut-icon {
    background: var(--brand-100, #e0e7ff);
  }

  .nav-shortcut-text {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    min-width: 0;
    font-size: 0.8rem;
    line-height: 1.25;
    overflow: hidden;
  }
  .nav-shortcut-text > span {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  :global(:is([data-theme="dark"], .dark)) .nav-shortcut { background: var(--bg-strong); }
</style>
