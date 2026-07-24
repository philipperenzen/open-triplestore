<script>
  // Tab strip of a floating inspector window. Tabs behave like browser tabs:
  // links opened inside a window land here instead of spawning another window,
  // a tab can be dragged onto another window's strip to join that group, and
  // dropping it on empty space tears it off into a window of its own. The drag
  // is pointer-only, so every grouping move also has a menu item behind the ⋯
  // button — the gesture is never the only way to do something.
  import { createEventDispatcher, onDestroy, tick } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { X, MoreHorizontal, Link2 } from 'lucide-svelte';
  import { dropPos } from '../../lib/viewer/windows';
  import { Z_DOCK } from '../../lib/viewer/zLayers';

  /** Band for this strip's transient popups: above the dock (a menu opened near
   *  the bottom of the screen overlaps it) but below the walkthrough. */
  const POPUP_Z = Z_DOCK + 1;

  /**
   * Re-parent a node to <body>.
   *
   * The tab menu and the drag ghost are positioned in VIEWPORT coordinates
   * (getBoundingClientRect / pointer events), but their host window carries an
   * inline `transform` for its drag offset — and a transformed element becomes
   * the containing block for its `position: fixed` descendants AND clips them
   * with its `overflow: hidden`. Left inside, the menu opened at an offset and a
   * ghost dragged toward another window vanished at the window's edge.
   */
  function portal(node) {
    document.body.appendChild(node);
    return {
      destroy() {
        node.parentNode?.removeChild(node);
      },
    };
  }

  /** Id of the window this strip belongs to (the drag drop-target hook). */
  export let wid = '';
  /** WindowTab[] — { key, kind, id, label }. */
  export let tabs = [];
  export let activeKey = '';
  /** The other open windows, for the "move to window" menu: [{ wid, label }]. */
  export let targets = [];

  const dispatch = createEventDispatcher();

  // Pointer travel (px) before a press turns into a drag, so a plain click on a
  // tab still selects it.
  const DRAG_THRESHOLD = 6;

  let btnEls = [];
  let menuBtnEls = [];
  let drag = null; // { key, label, startX, startY, x, y, overWid, moved, el, pointerId }
  let menuKey = null;
  let menuPos = { x: 0, y: 0 };
  let menuEl = null;
  let menuOpener = null; // control the menu was opened from, for the focus return
  let suppressClick = false;
  let dropHint = '';

  $: menuTab = tabs.find((t) => t.key === menuKey) ?? null;

  /** Move DOM focus to the active tab (used after a restore from the dock). */
  export function focusActive() {
    const i = tabs.findIndex((t) => t.key === activeKey);
    btnEls[i < 0 ? 0 : i]?.focus();
  }

  function select(key) {
    if (key !== activeKey) dispatch('select', { key });
  }

  function onTabClick(key) {
    // The click that trails a completed drag must not also re-select the tab.
    if (suppressClick) {
      suppressClick = false;
      return;
    }
    select(key);
  }

  // ── Drag to group / detach ──────────────────────────────────────────────────
  // Pointer capture keeps the gesture alive once the pointer leaves this window,
  // but it also RETARGETS the events to the captured element — so the drop
  // target is resolved with elementFromPoint, never with e.target.
  function stripWidAt(x, y) {
    const strip = document.elementFromPoint(x, y)?.closest?.('[data-tabstrip]');
    return strip?.getAttribute('data-wid') || null;
  }

  function onPointerDown(e, tab) {
    if (e.button !== 0) return;
    closeMenu();
    // Clear the guard when a NEW press starts, not when a click consumes it: a
    // drag that moved or detached the tab takes its DOM node out of this strip,
    // so the trailing click never reaches onTabClick and the flag used to stay
    // latched — swallowing some later, unrelated tab click.
    suppressClick = false;
    drag = {
      key: tab.key,
      label: tab.label,
      startX: e.clientX,
      startY: e.clientY,
      x: e.clientX,
      y: e.clientY,
      overWid: null,
      moved: false,
      el: e.currentTarget,
      pointerId: e.pointerId,
    };
    window.addEventListener('pointermove', onPointerMove);
    window.addEventListener('pointerup', onPointerUp, { once: true });
    window.addEventListener('pointercancel', cancelDrag, { once: true });
    // Capture phase: Escape belongs to the drag while one is running, before the
    // page-level handler that closes the top-most window sees it.
    window.addEventListener('keydown', onDragKey, true);
  }

  function onPointerMove(e) {
    if (!drag) return;
    if (!drag.moved) {
      if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < DRAG_THRESHOLD) return;
      drag.el?.setPointerCapture?.(drag.pointerId);
    }
    const overWid = stripWidAt(e.clientX, e.clientY);
    drag = { ...drag, x: e.clientX, y: e.clientY, overWid, moved: true };
    dropHint =
      overWid && overWid !== wid ? $i18nT('viewer.dropToGroup') : $i18nT('viewer.dropToDetach');
  }

  function onPointerUp(e) {
    const d = drag;
    stopDrag();
    if (!d?.moved) return;
    suppressClick = true;
    const overWid = stripWidAt(e.clientX, e.clientY);
    if (overWid === wid) return; // dropped back on its own strip — nothing to do
    if (overWid) {
      dispatch('move', { key: d.key, toWid: overWid });
    } else {
      // Anchor the new window so the grab point lands on its header.
      dispatch('detach', {
        key: d.key,
        pos: dropPos(
          { x: e.clientX - 40, y: e.clientY - 12 },
          { width: window.innerWidth, height: window.innerHeight }
        ),
      });
    }
  }

  function onDragKey(e) {
    if (e.key !== 'Escape' || !drag) return;
    e.preventDefault();
    e.stopPropagation();
    cancelDrag();
  }

  function cancelDrag() {
    if (drag?.moved) suppressClick = true;
    stopDrag();
  }

  function stopDrag() {
    if (drag?.el?.hasPointerCapture?.(drag.pointerId)) {
      drag.el.releasePointerCapture(drag.pointerId);
    }
    window.removeEventListener('pointermove', onPointerMove);
    window.removeEventListener('pointerup', onPointerUp);
    window.removeEventListener('pointercancel', cancelDrag);
    window.removeEventListener('keydown', onDragKey, true);
    drag = null;
    dropHint = '';
  }

  // ── Keyboard model (WAI-ARIA tabs, automatic activation) ────────────────────
  // The handler lives on the TABS, not on the strip: the strip itself is never
  // focusable (a tablist must not be), and every key press a user can make here
  // arrives at the roving-tabindex tab or bubbles up from its buttons.
  function onTabKey(e, tab, index) {
    // Activation keys belong to whatever is focused: the ⋯ and × buttons live
    // INSIDE the tab, and swallowing their Space/Enter would make them dead.
    const onTabItself = e.target === e.currentTarget;
    if (onTabItself && (e.key === 'Enter' || e.key === ' ')) {
      e.preventDefault();
      select(tab.key);
      return;
    }
    if (menuKey && e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeMenu(true);
      return;
    }
    // The ⋯ and × buttons sit inside the tab, where ARIA's "children
    // presentational" rule hides them from assistive tech and their own tab
    // stops would break the roving tabindex — so they are taken out of the tab
    // order and their actions are reachable from the tab itself instead:
    // Delete closes, Shift+F10 / the context-menu key opens the menu (both
    // announced through `aria-describedby`).
    if (onTabItself && (e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10'))) {
      e.preventDefault();
      e.stopPropagation();
      toggleMenu(tab.key, menuBtnEls[index] ?? e.currentTarget);
      return;
    }
    // An open menu owns the arrow keys (it is portalled away from this strip, so
    // a stray key here must not switch tabs underneath it).
    if (menuKey) return;
    const i = tabs.findIndex((t) => t.key === activeKey);
    let n = -1;
    if (e.key === 'ArrowRight') n = (i + 1 + tabs.length) % tabs.length;
    else if (e.key === 'ArrowLeft') n = (i - 1 + tabs.length) % tabs.length;
    else if (e.key === 'Home') n = 0;
    else if (e.key === 'End') n = tabs.length - 1;
    else if (e.key === 'Delete' && onTabItself) {
      e.preventDefault();
      e.stopPropagation();
      dispatch('close', { key: activeKey });
      return;
    } else return;
    e.preventDefault();
    e.stopPropagation();
    select(tabs[n].key);
    tick().then(() => btnEls[n]?.focus());
  }

  // ── Menu fallback ───────────────────────────────────────────────────────────
  // This menu is the ONLY non-pointer way to group or detach a tab, so it
  // follows the WAI-ARIA menu-button pattern properly: opening moves focus to
  // the first item, the arrows rove inside it, and Escape (or picking an item)
  // hands focus back to the control it was opened from.
  function toggleMenu(key, anchor) {
    if (menuKey === key) {
      closeMenu(true);
      return;
    }
    const r = anchor?.getBoundingClientRect?.();
    menuPos = { x: Math.max(8, (r?.left ?? 0) - 8), y: (r?.bottom ?? 0) + 4 };
    menuKey = key;
    menuOpener = anchor ?? null;
    window.addEventListener('pointerdown', onOutsideMenu, true);
    tick().then(() => focusMenuItem(0));
  }
  function onOutsideMenu(e) {
    if (!e.target?.closest?.('.tab-menu, .tab-menu-wrap')) closeMenu();
  }
  /** `restoreFocus` returns focus to the ⋯ button (Escape, re-toggle, item
   *  picked); a menu abandoned by Tab or by a click elsewhere must NOT pull the
   *  focus back to where the user just left. */
  function closeMenu(restoreFocus = false) {
    const opener = menuOpener;
    const open = !!menuKey;
    menuKey = null;
    menuOpener = null;
    window.removeEventListener('pointerdown', onOutsideMenu, true);
    if (!open || !restoreFocus) return;
    tick().then(() => {
      if (opener?.isConnected) opener.focus();
      else focusActive(); // the tab moved away with the action — stay in the strip
    });
  }
  function menuItems() {
    return menuEl ? [...menuEl.querySelectorAll('[role="menuitem"]')] : [];
  }
  function focusMenuItem(i) {
    const items = menuItems();
    if (!items.length) return;
    items[(i + items.length) % items.length].focus();
  }
  function onMenuKey(e) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      closeMenu(true);
      return;
    }
    if (e.key === 'Tab') {
      closeMenu(); // let focus leave naturally; an open menu behind it is useless
      return;
    }
    const items = menuItems();
    const i = items.indexOf(document.activeElement);
    if (e.key === 'ArrowDown') focusMenuItem(i + 1);
    else if (e.key === 'ArrowUp') focusMenuItem(i - 1);
    else if (e.key === 'Home') focusMenuItem(0);
    else if (e.key === 'End') focusMenuItem(items.length - 1);
    else return;
    e.preventDefault();
    e.stopPropagation();
  }
  function onMenuFocusOut(e) {
    const to = e.relatedTarget;
    // Focus landing back on the ⋯ button is the toggle-closed gesture; leave it
    // to that button's own click, or the menu would close and instantly reopen.
    if (to && (menuEl?.contains(to) || to === menuOpener)) return;
    closeMenu();
  }
  function menuDetach(key) {
    closeMenu(true);
    dispatch('detach', { key }); // no pos — the parent cascades it
  }
  function menuMove(key, toWid) {
    closeMenu(true);
    dispatch('move', { key, toWid });
  }

  // A window can disappear mid-gesture (evicted at the cap, or emptied by the
  // very drop being made), so the global listeners are torn down here too.
  onDestroy(() => {
    stopDrag();
    closeMenu();
  });
</script>

<div
  class="tab-strip"
  class:drop-target={!!drag && drag.overWid === wid}
  role="tablist"
  aria-label={$i18nT('viewer.tabs')}
  data-tabstrip
  data-wid={wid}
>
  {#each tabs as tab, i (tab.key)}
    <div
      class="tab"
      class:active={tab.key === activeKey}
      class:dragging={drag?.moved && drag.key === tab.key}
      role="tab"
      id={`tab-${wid}-${i}`}
      aria-selected={tab.key === activeKey}
      aria-controls={`tabpanel-${wid}`}
      aria-describedby={`tabhint-${wid}`}
      aria-keyshortcuts="Delete Shift+F10"
      tabindex={tab.key === activeKey ? 0 : -1}
      title={tab.id}
      bind:this={btnEls[i]}
      on:pointerdown={(e) => onPointerDown(e, tab)}
      on:click={() => onTabClick(tab.key)}
      on:keydown={(e) => onTabKey(e, tab, i)}
    >
      {#if tab.kind === 'resource'}<span class="tab-kind"><Link2 size={11} /></span>{/if}
      <span class="tab-label">{tab.label}</span>
      <!-- Both icon buttons are OUT of the tab sequence (`tabindex="-1"`): ARIA
           makes a tab's children presentational, so as tab stops they would be
           unlabelled and would break the roving tabindex. Keyboard users reach
           the same two actions from the tab itself — see `onTabKey`. -->
      <span class="tab-menu-wrap">
        <button
          class="tab-btn"
          tabindex="-1"
          aria-haspopup="menu"
          aria-expanded={menuKey === tab.key}
          aria-label={$i18nT('viewer.tabMenu')}
          title={$i18nT('viewer.tabMenu')}
          bind:this={menuBtnEls[i]}
          on:click|stopPropagation={(e) => toggleMenu(tab.key, e.currentTarget)}
          on:pointerdown={(e) => e.stopPropagation()}
        ><MoreHorizontal size={12} /></button>
      </span>
      <button
        class="tab-btn tab-close"
        tabindex="-1"
        aria-label={$i18nT('viewer.closeTab')}
        title={$i18nT('viewer.closeTab')}
        on:click|stopPropagation={() => dispatch('close', { key: tab.key })}
        on:pointerdown={(e) => e.stopPropagation()}
      ><X size={12} /></button>
    </div>
  {/each}
</div>
<span class="sr-only" id={`tabhint-${wid}`}>{$i18nT('viewer.tabKeyHint')}</span>

<!-- The menu is a viewport-positioned sibling, not a child of the strip: the
     strip scrolls horizontally, and any dropdown inside it would be clipped.
     `portal` takes it out of the window entirely — see the action's comment. -->
{#if menuTab}
  <div
    class="tab-menu"
    role="menu"
    tabindex="-1"
    aria-label={$i18nT('viewer.tabMenu')}
    use:portal
    bind:this={menuEl}
    style:left={`${menuPos.x}px`}
    style:top={`${menuPos.y}px`}
    style:z-index={POPUP_Z}
    on:keydown={onMenuKey}
    on:focusout={onMenuFocusOut}
  >
    <button role="menuitem" tabindex="-1" on:click|stopPropagation={() => menuDetach(menuTab.key)}>
      {$i18nT('viewer.moveToNewWindow')}
    </button>
    {#if targets.length}
      <div class="menu-head">{$i18nT('viewer.moveToWindow')}</div>
      {#each targets as target (target.wid)}
        <button
          role="menuitem"
          tabindex="-1"
          on:click|stopPropagation={() => menuMove(menuTab.key, target.wid)}
        >
          {target.label}
        </button>
      {/each}
    {/if}
  </div>
{/if}

{#if drag?.moved}
  <!-- Drag ghost: pointer-events must stay off or it would win every hit test. -->
  <div
    class="tab-ghost"
    use:portal
    style:left={`${drag.x + 10}px`}
    style:top={`${drag.y + 8}px`}
    style:z-index={POPUP_Z}
  >
    {drag.label}
  </div>
{/if}
<span class="sr-only" role="status" aria-live="polite">{dropHint}</span>

<style>
  .tab-strip {
    display: flex;
    align-items: stretch;
    gap: 3px;
    padding: 5px 8px 0;
    overflow-x: auto;
    background: var(--bg-subtle, #fafcfe);
    border-bottom: 1px solid var(--line-soft, #eef1f4);
    scrollbar-width: thin;
  }
  /* Highlight the strip a dragged tab is currently over. */
  .tab-strip.drop-target {
    box-shadow: inset 0 0 0 2px var(--brand-400, #5aa9e0);
  }
  .tab {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    max-width: 190px;
    min-width: 0;
    padding: 4px 4px 4px 10px;
    border: 1px solid transparent;
    border-bottom: 0;
    border-radius: 8px 8px 0 0;
    background: transparent;
    color: var(--muted, #64748b);
    font-size: 0.78rem;
    cursor: pointer;
    user-select: none;
    /* The strip is a horizontal scroller AND a drag surface — without this a
       touch drag would scroll the strip instead of moving the tab. */
    touch-action: none;
  }
  .tab:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.04));
    color: var(--ink-900, #0f172a);
  }
  .tab.active {
    background: var(--bg-elevated, #fff);
    border-color: var(--line-soft, #e2e8f0);
    color: var(--ink-900, #0f172a);
    font-weight: 600;
  }
  .tab.dragging {
    opacity: 0.45;
  }
  .tab:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--brand-400, #5aa9e0);
  }
  .tab-label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tab-kind {
    display: inline-flex;
    color: var(--brand-500, #2f88d8);
    flex: none;
  }
  .tab-btn {
    border: 0;
    background: transparent;
    /* #64748b, not #94a3b8: these are icon-only controls on a near-white strip,
       where the lighter fallback lands at 2.5:1 — under WCAG 1.4.11's 3:1. */
    color: var(--muted, #64748b);
    padding: 2px;
    border-radius: 5px;
    cursor: pointer;
    display: grid;
    place-items: center;
    flex: none;
  }
  .tab-btn:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.08));
    color: var(--ink-900, #0f172a);
  }
  .tab-close:hover {
    color: var(--danger-500, #c0392b);
  }
  .tab-btn:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--brand-400, #5aa9e0);
  }
  .tab-menu-wrap {
    position: relative;
    display: inline-flex;
    flex: none;
  }
  .tab-menu {
    position: fixed;
    /* Resting value only — the inline style (see POPUP_Z) is the authority. */
    z-index: 1301;
    min-width: 190px;
    max-width: 260px;
    padding: 4px;
    border-radius: 10px;
    background: var(--bg-elevated, #fff);
    border: 1px solid var(--line-soft, #e2e8f0);
    box-shadow: var(--shadow-md, 0 12px 30px rgba(0, 0, 0, 0.16));
    text-align: left;
  }
  .tab-menu button {
    display: block;
    width: 100%;
    text-align: left;
    border: 0;
    background: transparent;
    padding: 6px 8px;
    border-radius: 6px;
    font-size: 0.78rem;
    color: var(--ink-900, #0f172a);
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tab-menu button:hover {
    background: var(--bg-hover, rgba(0, 0, 0, 0.05));
  }
  .tab-menu button:focus-visible {
    outline: none;
    box-shadow: 0 0 0 2px var(--brand-400, #5aa9e0);
  }
  .menu-head {
    font-size: 0.64rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted, #64748b);
    padding: 6px 8px 2px;
  }
  .tab-ghost {
    position: fixed;
    /* Resting value only — the inline style (see POPUP_Z) is the authority. */
    z-index: 1301;
    pointer-events: none;
    max-width: 200px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: 4px 10px;
    border-radius: 8px;
    font-size: 0.78rem;
    background: var(--bg-elevated, #fff);
    border: 1px solid var(--brand-400, #5aa9e0);
    color: var(--ink-900, #0f172a);
    box-shadow: var(--shadow-md, 0 12px 30px rgba(0, 0, 0, 0.16));
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
    white-space: nowrap;
  }
</style>
