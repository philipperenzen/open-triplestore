<script>
  // Custom listbox replacement for native <select>. Styled trigger + a
  // fixed-position popup so it never inherits the stock browser option list and
  // never clips inside toolbars / scroll containers.
  //
  // Usage (drop-in for `<select bind:value>` with array options):
  //   <Select bind:value={x} options={[{value:'a', label:'A'}, ...]} placeholder="Pick…" />
  // Options may also be plain strings, and may carry `disabled` or `group`.
  import { createEventDispatcher, onDestroy, tick } from 'svelte';
  import { ChevronDown, Check } from 'lucide-svelte';
  import { t } from 'svelte-i18n';

  export let value = undefined;
  /** Array of strings or { value, label, disabled?, group? }. */
  export let options = [];
  /** Shown when nothing matches `value`; also used as a blank-state label. */
  export let placeholder = '';
  export let disabled = false;
  export let id = undefined;
  export let name = undefined;
  export let ariaLabel = undefined;
  export let title = undefined;
  /** 'sm' | 'md' */
  export let size = 'md';
  /** Extra class(es) on the trigger button. */
  let klass = '';
  export { klass as class };

  const dispatch = createEventDispatcher();

  let triggerEl;
  let listEl;
  let open = false;
  let activeIdx = -1;
  let pos = { left: 0, top: 0, width: 0, placement: 'bottom' };
  let typeahead = '';
  let typeaheadTimer;

  $: norm = (options || []).map((o) =>
    o !== null && typeof o === 'object' ? { value: o.value, label: o.label ?? String(o.value), disabled: !!o.disabled, group: o.group } : { value: o, label: String(o), disabled: false, group: undefined },
  );
  $: selected = norm.find((o) => o.value === value);
  $: displayLabel = selected ? selected.label : '';

  function reposition() {
    if (!triggerEl) return;
    const r = triggerEl.getBoundingClientRect();
    const vh = window.innerHeight;
    const below = vh - r.bottom;
    const estH = Math.min(280, norm.length * 34 + 10);
    const placement = below < estH && r.top > below ? 'top' : 'bottom';
    pos = {
      left: r.left,
      top: placement === 'bottom' ? r.bottom + 4 : r.top - 4,
      width: r.width,
      placement,
    };
  }

  async function openMenu() {
    if (disabled) return;
    open = true;
    reposition();
    activeIdx = norm.findIndex((o) => o.value === value);
    if (activeIdx < 0) activeIdx = norm.findIndex((o) => !o.disabled);
    await tick();
    scrollActiveIntoView();
    window.addEventListener('scroll', reposition, true);
    window.addEventListener('resize', reposition);
  }

  function closeMenu() {
    open = false;
    activeIdx = -1;
    window.removeEventListener('scroll', reposition, true);
    window.removeEventListener('resize', reposition);
  }

  function toggle() {
    open ? closeMenu() : openMenu();
  }

  function choose(opt) {
    if (!opt || opt.disabled) return;
    if (opt.value !== value) {
      value = opt.value;
      dispatch('change', opt.value);
    }
    closeMenu();
    triggerEl?.focus();
  }

  function moveActive(delta) {
    if (!norm.length) return;
    let i = activeIdx;
    for (let n = 0; n < norm.length; n++) {
      i = (i + delta + norm.length) % norm.length;
      if (!norm[i].disabled) { activeIdx = i; break; }
    }
    scrollActiveIntoView();
  }

  function scrollActiveIntoView() {
    if (!listEl) return;
    const el = listEl.querySelector(`[data-idx="${activeIdx}"]`);
    el?.scrollIntoView({ block: 'nearest' });
  }

  function onTypeahead(key) {
    clearTimeout(typeaheadTimer);
    typeahead += key.toLowerCase();
    typeaheadTimer = setTimeout(() => (typeahead = ''), 600);
    const match = norm.findIndex((o) => !o.disabled && o.label.toLowerCase().startsWith(typeahead));
    if (match >= 0) { activeIdx = match; scrollActiveIntoView(); }
  }

  function onKeydown(e) {
    if (disabled) return;
    if (!open) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp' || e.key === 'Enter' || e.key === ' ') {
        e.preventDefault();
        openMenu();
      }
      return;
    }
    switch (e.key) {
      case 'ArrowDown': e.preventDefault(); moveActive(1); break;
      case 'ArrowUp': e.preventDefault(); moveActive(-1); break;
      case 'Home': e.preventDefault(); activeIdx = norm.findIndex((o) => !o.disabled); scrollActiveIntoView(); break;
      case 'End': e.preventDefault(); for (let i = norm.length - 1; i >= 0; i--) { if (!norm[i].disabled) { activeIdx = i; break; } } scrollActiveIntoView(); break;
      case 'Enter':
      case ' ': e.preventDefault(); if (activeIdx >= 0) choose(norm[activeIdx]); break;
      case 'Escape': e.preventDefault(); closeMenu(); triggerEl?.focus(); break;
      case 'Tab': closeMenu(); break;
      default:
        if (e.key.length === 1 && !e.metaKey && !e.ctrlKey && !e.altKey) onTypeahead(e.key);
    }
  }

  function onWindowPointer(e) {
    if (!open) return;
    if (triggerEl?.contains(e.target) || listEl?.contains(e.target)) return;
    closeMenu();
  }

  // Render the popup at <body> so a transformed/filtered ancestor can't become
  // the containing block for position:fixed (which would offset coordinates).
  function portal(node) {
    if (typeof document !== 'undefined') document.body.appendChild(node);
    return { destroy() { node.parentNode?.removeChild(node); } };
  }

  function showGroupHeader(i) {
    const g = norm[i].group;
    if (!g) return false;
    return i === 0 || norm[i - 1].group !== g;
  }

  onDestroy(() => {
    window.removeEventListener('scroll', reposition, true);
    window.removeEventListener('resize', reposition);
  });
</script>

<svelte:window on:pointerdown={onWindowPointer} />

<button
  type="button"
  {id}
  bind:this={triggerEl}
  class="sel-trigger sel-{size} {klass}"
  class:open
  class:placeholder={!selected}
  {disabled}
  {title}
  aria-haspopup="listbox"
  aria-expanded={open}
  aria-label={ariaLabel}
  on:click={toggle}
  on:keydown={onKeydown}
>
  <span class="sel-value">{displayLabel || placeholder}</span>
  <ChevronDown size={size === 'sm' ? 13 : 15} class="sel-chevron" />
</button>

{#if name}<input type="hidden" {name} value={value ?? ''} />{/if}

{#if open}
  <ul
    bind:this={listEl}
    use:portal
    class="sel-popup"
    class:place-top={pos.placement === 'top'}
    role="listbox"
    tabindex="-1"
    style="left:{pos.left}px; {pos.placement === 'bottom' ? `top:${pos.top}px` : `bottom:${window.innerHeight - pos.top}px`}; min-width:{pos.width}px;"
  >
    {#each norm as opt, i (i)}
      {#if showGroupHeader(i)}<li class="sel-group" role="presentation">{opt.group}</li>{/if}
      <!-- svelte-ignore a11y_click_events_have_key_events -->
      <li
        class="sel-option"
        class:active={i === activeIdx}
        class:selected={opt.value === value}
        class:disabled={opt.disabled}
        data-idx={i}
        role="option"
        aria-selected={opt.value === value}
        on:mouseenter={() => { if (!opt.disabled) activeIdx = i; }}
        on:click={() => choose(opt)}
      >
        <span class="sel-option-label">{opt.label}</span>
        {#if opt.value === value}<Check size={14} class="sel-check" />{/if}
      </li>
    {/each}
    {#if !norm.length}<li class="sel-empty">{$t('components.select.noOptions')}</li>{/if}
  </ul>
{/if}

<style>
  .sel-trigger {
    display: inline-flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.4rem;
    width: 100%;
    padding: 0.4rem 0.55rem;
    font-size: 0.86rem;
    font-family: inherit;
    color: var(--ink-900, #1e293b);
    background: var(--surface, #fff);
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 8px;
    cursor: pointer;
    text-align: left;
    transition: border-color 0.12s, box-shadow 0.12s, background 0.12s;
  }
  .sel-trigger.sel-sm { padding: 0.25rem 0.45rem; font-size: 0.8rem; border-radius: 7px; }
  .sel-trigger:hover:not(:disabled) { border-color: var(--brand-400, #60a5fa); }
  .sel-trigger:focus-visible,
  .sel-trigger.open {
    outline: none;
    border-color: var(--brand-500, #3b82f6);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--brand-500, #3b82f6) 22%, transparent);
  }
  .sel-trigger:disabled { opacity: 0.55; cursor: not-allowed; }
  .sel-trigger.placeholder .sel-value { color: var(--ink-400, #94a3b8); }
  .sel-value { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  :global(.sel-trigger .sel-chevron) { color: var(--ink-400, #94a3b8); flex-shrink: 0; transition: transform 0.15s; }
  .sel-trigger.open :global(.sel-chevron) { transform: rotate(180deg); }

  .sel-popup {
    position: fixed;
    z-index: 99999;
    margin: 0;
    padding: 5px;
    list-style: none;
    max-height: 280px;
    overflow-y: auto;
    background: #fff;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 10px;
    box-shadow: 0 6px 28px rgba(0,0,0,0.15), 0 1px 4px rgba(0,0,0,0.08);
    font-size: 0.86rem;
  }

  .sel-option {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.4rem 0.55rem;
    border-radius: 6px;
    cursor: pointer;
    color: var(--ink-900, #1e293b);
    white-space: nowrap;
  }
  .sel-option-label { overflow: hidden; text-overflow: ellipsis; }
  .sel-option.active { background: var(--bg-accent-soft, #eff6ff); }
  .sel-option.selected { font-weight: 600; color: var(--brand-700, #1d4ed8); }
  .sel-option.disabled { opacity: 0.45; cursor: not-allowed; }
  :global(.sel-popup .sel-check) { color: var(--brand-600, #2563eb); flex-shrink: 0; }

  .sel-group {
    padding: 0.4rem 0.55rem 0.2rem;
    font-size: 0.68rem;
    font-weight: 700;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--ink-400, #94a3b8);
  }
  .sel-empty { padding: 0.5rem 0.55rem; color: var(--ink-400, #94a3b8); }

  :global(:is([data-theme="dark"], .dark)) .sel-trigger { background: var(--bg-soft); color: var(--ink-900); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .sel-popup { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .sel-option { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .sel-option.active { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .sel-option.selected { color: var(--brand-400); }
</style>
