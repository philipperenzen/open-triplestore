<script>
  import { t } from 'svelte-i18n';
  import { navigate } from '../lib/router/index.js';
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
    { label: () => $t('components.searchBar.vocabularyRegistry'), path: '/vocabularies', icon: BookOpen, meta: () => $t('components.searchBar.vocabularyRegistryMeta') },
  ];

  // Mock dataset suggestions (can be replaced with actual API call)
  const mockDatasets = ['public-data', 'linked-open-data', 'geo-data'];

  $: filteredNav = query.trim().length > 0
    ? navShortcuts.filter(n => n.label().toLowerCase().includes(query.toLowerCase()))
    : navShortcuts;

  $: suggestions = query.trim().length > 0
    ? [
        ...recentSearches.slice(0, 3),
        ...mockDatasets.filter(d => d.includes(query.toLowerCase())).slice(0, 2)
      ].slice(0, 5)
    : [];

  function submit() {
    // A keyboard-highlighted suggestion takes precedence over the typed text;
    // adopt it as the query and fall through to the normal navigation logic.
    if (selectedSuggestionIdx >= 0 && suggestions[selectedSuggestionIdx]) {
      query = suggestions[selectedSuggestionIdx];
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
          on:focus={() => { showSuggestions = query.trim().length > 0; }}
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

    {#if showSuggestions && suggestions.length > 0}
      <div class="absolute top-full left-0 right-0 mt-2 bg-white border border-[var(--line-soft)] rounded-xl shadow-lg z-50 overflow-hidden">
        <div class="max-h-64 overflow-y-auto">
          {#each suggestions as suggestion, idx}
            <button
              type="button"
              on:click={() => useSuggestion(suggestion)}
              on:mouseenter={() => { selectedSuggestionIdx = idx; }}
              class="w-full text-left px-4 py-2.5 flex items-center justify-between hover:bg-brand-50 transition-colors border-b border-line-soft/30 text-sm"
              class:bg-brand-50={selectedSuggestionIdx === idx}
            >
              <span class="flex items-center gap-2.5">
                <Search size={14} class="text-ink-400 shrink-0" />
                <span class="text-ink-900">{suggestion}</span>
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
