<script>
  // Free-text input with a styled suggestion popup. Drop-in replacement for an
  // <input list="…"> + <datalist>, but the suggestion list is our own markup
  // (fixed-position popup) instead of the stock browser dropdown.
  //
  // Usage:
  //   <Combobox bind:value={x} suggestions={['ex:Foo','ex:Bar']} placeholder="ex:Class" />
  // Suggestions may be strings or { value, label, hint? }.
  import { createEventDispatcher, onDestroy, tick } from 'svelte';
  import { Search } from 'lucide-svelte';

  export let value = '';
  /** Array of strings or { value, label?, hint? }. */
  export let suggestions = [];
  export let placeholder = '';
  export let disabled = false;
  export let id = undefined;
  export let name = undefined;
  export let ariaLabel = undefined;
  export let title = undefined;
  /** Filter suggestions by substring of the typed value. */
  export let filter = true;
  /** Max suggestions to render. */
  export let max = 50;
  let klass = '';
  export { klass as class };

  const dispatch = createEventDispatcher();

  let inputEl;
  let listEl;
  let open = false;
  let activeIdx = -1;
  let pos = { left: 0, top: 0, width: 0, placement: 'bottom' };

  $: norm = (suggestions || []).map((s) =>
    s !== null && typeof s === 'object' ? { value: s.value, label: s.label ?? String(s.value), hint: s.hint } : { value: s, label: String(s), hint: undefined },
  );
  $: q = (value ?? '').toString().toLowerCase();
  $: matches = (filter && q
    ? norm.filter((s) => s.label.toLowerCase().includes(q) || String(s.value).toLowerCase().includes(q))
    : norm
  ).slice(0, max);

  function reposition() {
    if (!inputEl) return;
    const r = inputEl.getBoundingClientRect();
    const vh = window.innerHeight;
    const below = vh - r.bottom;
    const estH = Math.min(260, matches.length * 32 + 10);
    const placement = below < estH && r.top > below ? 'top' : 'bottom';
    pos = { left: r.left, top: placement === 'bottom' ? r.bottom + 4 : r.top - 4, width: r.width, placement };
  }

  async function openMenu() {
    if (disabled || !matches.length) return;
    open = true;
    activeIdx = -1;
    reposition();
    await tick();
    window.addEventListener('scroll', reposition, true);
    window.addEventListener('resize', reposition);
  }

  function closeMenu() {
    open = false;
    activeIdx = -1;
    window.removeEventListener('scroll', reposition, true);
    window.removeEventListener('resize', reposition);
  }

  function onInput(e) {
    value = e.currentTarget.value;
    dispatch('input', value);
    if (matches.length) { open = true; reposition(); } else { open = false; }
    activeIdx = -1;
  }

  function pick(opt) {
    value = opt.value;
    dispatch('input', value);
    dispatch('change', value);
    closeMenu();
    inputEl?.focus();
  }

  function onKeydown(e) {
    if (disabled) return;
    if (!open && (e.key === 'ArrowDown') && matches.length) { openMenu(); return; }
    if (!open) { if (e.key === 'Enter') dispatch('change', value); return; }
    switch (e.key) {
      case 'ArrowDown': e.preventDefault(); activeIdx = Math.min(activeIdx + 1, matches.length - 1); scrollActiveIntoView(); break;
      case 'ArrowUp': e.preventDefault(); activeIdx = Math.max(activeIdx - 1, -1); scrollActiveIntoView(); break;
      case 'Enter':
        if (activeIdx >= 0 && matches[activeIdx]) { e.preventDefault(); pick(matches[activeIdx]); }
        else { closeMenu(); dispatch('change', value); }
        break;
      case 'Escape': e.preventDefault(); closeMenu(); break;
      case 'Tab': closeMenu(); break;
    }
  }

  function scrollActiveIntoView() {
    if (!listEl) return;
    listEl.querySelector(`[data-idx="${activeIdx}"]`)?.scrollIntoView({ block: 'nearest' });
  }

  function onBlur(_e) {
    // Fire change like a native input/datalist does on commit.
    dispatch('change', value);
  }

  function onWindowPointer(e) {
    if (!open) return;
    if (inputEl?.contains(e.target) || listEl?.contains(e.target)) return;
    closeMenu();
  }

  // Render the popup at <body> so a transformed/filtered ancestor can't become
  // the containing block for position:fixed (which would offset coordinates).
  function portal(node) {
    if (typeof document !== 'undefined') document.body.appendChild(node);
    return { destroy() { node.parentNode?.removeChild(node); } };
  }

  onDestroy(() => {
    window.removeEventListener('scroll', reposition, true);
    window.removeEventListener('resize', reposition);
  });
</script>

<svelte:window on:pointerdown={onWindowPointer} />

<input
  {id}
  {name}
  {placeholder}
  {disabled}
  {title}
  bind:this={inputEl}
  class="cb-input {klass}"
  value={value ?? ''}
  autocomplete="off"
  aria-label={ariaLabel}
  aria-autocomplete="list"
  aria-expanded={open}
  on:input={onInput}
  on:focus={() => { dispatch('focus'); if (matches.length) openMenu(); }}
  on:blur={onBlur}
  on:keydown={onKeydown}
/>

{#if open && matches.length}
  <ul
    bind:this={listEl}
    use:portal
    class="cb-popup"
    role="listbox"
    style="left:{pos.left}px; {pos.placement === 'bottom' ? `top:${pos.top}px` : `bottom:${window.innerHeight - pos.top}px`}; min-width:{pos.width}px;"
  >
    {#each matches as opt, i (opt.value)}
      <li
        class="cb-option"
        class:active={i === activeIdx}
        data-idx={i}
        role="option"
        aria-selected={i === activeIdx}
        on:mouseenter={() => (activeIdx = i)}
        on:mousedown|preventDefault={() => pick(opt)}
      >
        <Search size={12} class="cb-icon" />
        <span class="cb-label">{opt.label}</span>
        {#if opt.hint}<span class="cb-hint">{opt.hint}</span>{/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  .cb-input {
    width: 100%;
    padding: 0.4rem 0.55rem;
    font-size: 0.86rem;
    font-family: inherit;
    color: var(--ink-900, #1e293b);
    background: var(--surface, #fff);
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 8px;
    transition: border-color 0.12s, box-shadow 0.12s;
  }
  .cb-input:hover:not(:disabled) { border-color: var(--brand-400, #60a5fa); }
  .cb-input:focus { outline: none; border-color: var(--brand-500, #3b82f6); box-shadow: 0 0 0 3px color-mix(in srgb, var(--brand-500, #3b82f6) 22%, transparent); }
  .cb-input:disabled { opacity: 0.55; cursor: not-allowed; }

  .cb-popup {
    position: fixed;
    z-index: 99999;
    margin: 0;
    padding: 5px;
    list-style: none;
    max-height: 260px;
    overflow-y: auto;
    background: #fff;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 10px;
    box-shadow: 0 6px 28px rgba(0,0,0,0.15), 0 1px 4px rgba(0,0,0,0.08);
    font-size: 0.84rem;
  }
  .cb-option {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.38rem 0.5rem;
    border-radius: 6px;
    cursor: pointer;
    color: var(--ink-900, #1e293b);
    white-space: nowrap;
  }
  .cb-option.active { background: var(--bg-accent-soft, #eff6ff); }
  .cb-label { overflow: hidden; text-overflow: ellipsis; }
  .cb-hint { margin-left: auto; color: var(--ink-400, #94a3b8); font-size: 0.74rem; }
  :global(.cb-popup .cb-icon) { color: var(--ink-400, #94a3b8); flex-shrink: 0; }

  :global(:is([data-theme="dark"], .dark)) .cb-input { background: var(--bg-soft); color: var(--ink-900); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .cb-popup { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .cb-option { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .cb-option.active { background: rgba(255,255,255,0.06); }
</style>
