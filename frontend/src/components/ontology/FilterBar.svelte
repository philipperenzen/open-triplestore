<script lang="ts">
  import { Search, X, Regex, Filter } from 'lucide-svelte';
  import { t as i18nT } from 'svelte-i18n';
  import FilterModal from './FilterModal.svelte';
  import type { FilterState } from '../../lib/ontology/filters';

  export let state: FilterState;
  export let onChange: (_s: FilterState) => void = () => {};

  let modalOpen = false;
  let regexMode = false;

  function setText(t: string)  { onChange({ ...state, text: t }); }
  function setRegex(r: string) { onChange({ ...state, iriRegex: r }); }
  function clearInput() {
    if (regexMode) setRegex(''); else setText('');
  }

  $: regexValid = (() => { if (!state.iriRegex) return true; try { new RegExp(state.iriRegex); return true; } catch { return false; } })();
  $: activeChipCount = state.vocabs.size + state.kinds.size + state.usage.size;
  $: currentValue = regexMode ? state.iriRegex : state.text;
  $: showClear = currentValue.length > 0;
</script>

<div class="fb">
  <div class="search-wrap" class:regex={regexMode} class:invalid={regexMode && !regexValid}>
    <Search size={13} class="search-icon" />
    <input
      class="search-input"
      class:mono={regexMode}
      type="text"
      placeholder={regexMode ? $i18nT('components.filterBar.iriRegexPlaceholder') : $i18nT('components.filterBar.searchPlaceholder')}
      value={currentValue}
      on:input={(e) => regexMode ? setRegex(e.currentTarget.value) : setText(e.currentTarget.value)}
      title={regexMode && !regexValid ? $i18nT('components.filterBar.invalidRegex') : ''}
    />
    {#if showClear}
      <button class="icon-btn" on:click={clearInput} title={$i18nT('system.clear')} aria-label={$i18nT('components.filterBar.clearInputAria')}>
        <X size={12} />
      </button>
    {/if}
    <button
      class="icon-btn regex-toggle"
      class:active={regexMode}
      on:click={() => regexMode = !regexMode}
      title={regexMode ? $i18nT('components.filterBar.switchToPlainText') : $i18nT('components.filterBar.switchToIriRegex')}
      aria-label={$i18nT('components.filterBar.toggleRegexAria')}
      aria-pressed={regexMode}
    >
      <Regex size={13} />
    </button>
  </div>

  <button
    class="filters-btn"
    class:has-active={activeChipCount > 0}
    on:click={() => modalOpen = true}
    aria-haspopup="dialog"
  >
    <Filter size={13} /> {$i18nT('components.filterBar.filters')}
    {#if activeChipCount > 0}<span class="badge">{activeChipCount}</span>{/if}
  </button>
</div>

{#if modalOpen}
  <FilterModal {state} {onChange} onClose={() => modalOpen = false} />
{/if}

<style>
  .fb {
    display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap;
    padding: 0.45rem 0.6rem;
    border: 1px solid #e2e8f0; background: #f8fafc; border-radius: 10px;
  }

  .search-wrap {
    flex: 1; min-width: 220px;
    display: flex; align-items: center; gap: 0.35rem;
    padding: 0.25rem 0.5rem;
    background: #fff; border: 1px solid #cbd5e1; border-radius: 8px;
    color: #94a3b8;
  }
  .search-wrap:focus-within { border-color: #1565c0; box-shadow: 0 0 0 2px rgba(21,101,192,0.12); }
  .search-wrap.regex { background: #fafaff; }
  .search-wrap.invalid { border-color: #dc2626; background: #fef2f2; }

  .search-input {
    flex: 1; border: none; outline: none; background: transparent;
    font-size: 0.82rem; color: #1e293b; padding: 0.15rem 0;
  }
  .search-input.mono { font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }

  .icon-btn {
    border: none; background: transparent; cursor: pointer;
    padding: 3px; border-radius: 5px; display: inline-flex; align-items: center;
    color: #94a3b8;
  }
  .icon-btn:hover { background: #f1f5f9; color: #1e293b; }
  .regex-toggle.active { background: #1565c0; color: #fff; }
  .regex-toggle.active:hover { background: #0d4a92; color: #fff; }

  .filters-btn {
    display: inline-flex; align-items: center; gap: 0.35rem;
    padding: 0.35rem 0.7rem; border-radius: 8px;
    border: 1px solid #cbd5e1; background: #fff;
    font-size: 0.8rem; color: #334155; cursor: pointer;
    white-space: nowrap;
  }
  .filters-btn:hover { background: #f1f5f9; }
  .filters-btn.has-active { background: #eef5ff; border-color: #93c5fd; color: #1565c0; }
  .badge {
    background: #1565c0; color: #fff; font-size: 0.7rem; font-weight: 700;
    border-radius: 999px; padding: 0 0.4rem; min-width: 18px; text-align: center;
  }

  :global(:is([data-theme="dark"], .dark)) .fb { background: var(--bg-soft); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .search-wrap { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .search-wrap.regex { background: rgba(139,92,246,0.08); }
  :global(:is([data-theme="dark"], .dark)) .search-wrap.invalid { background: rgba(220,38,38,0.12); border-color: rgba(239,68,68,0.5); }
  :global(:is([data-theme="dark"], .dark)) .search-input { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn:hover { background: rgba(255,255,255,0.06); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .filters-btn { background: var(--bg-strong); border-color: var(--line-strong); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .filters-btn:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .filters-btn.has-active { background: rgba(59,130,246,0.15); border-color: rgba(59,130,246,0.4); color: #93c5fd; }
</style>
