<script>
  // Wraps an inline trigger (a predicate chip, an RDF term…) and shows a floating
  // TermDefinitionCard on hover or click. Only opens when the IRI resolves to a
  // bundled vocabulary term, so non-vocabulary terms stay quiet.
  import { tick } from 'svelte';
  import { clickOutside } from '../../lib/actions/clickOutside.js';
  import { warmVocab, lookupTerm, lookupTermSync } from '../../lib/ontology/termDictionary.js';
  import TermDefinitionCard from './TermDefinitionCard.svelte';

  export let iri = '';
  /** @type {'rich' | 'compact'} */
  export let variant = 'rich';
  /** @type {'click' | 'hover'} */
  export let trigger = 'click';

  let open = false;
  let x = 0;
  let y = 0;
  let triggerEl;
  let panelEl;
  let openTimer;
  let closeTimer;
  let known; // undefined until resolved, then true/false

  let lastIri = '';
  $: if (iri !== lastIri) { lastIri = iri; known = undefined; open = false; }

  async function ensureKnown() {
    if (known !== undefined) return known;
    const sync = lookupTermSync(iri);
    if (sync !== undefined) { known = !!sync; return known; }
    warmVocab(iri);
    known = !!(await lookupTerm(iri));
    return known;
  }

  async function place() {
    await tick();
    if (!triggerEl) return;
    const r = triggerEl.getBoundingClientRect();
    const pw = variant === 'compact' ? 300 : 360;
    const ph = panelEl?.offsetHeight || 240;
    x = Math.min(Math.max(8, r.left), window.innerWidth - pw - 8);
    y = r.bottom + 6;
    if (y + ph > window.innerHeight - 8) y = Math.max(8, r.top - ph - 6);
  }

  async function doOpen() {
    if (!(await ensureKnown())) return; // never open an empty popover
    open = true;
    await place();
  }
  function close() { open = false; }

  function onEnter() {
    if (trigger !== 'hover') return;
    clearTimeout(closeTimer);
    openTimer = setTimeout(doOpen, 150);
  }
  function onLeave() {
    if (trigger !== 'hover') return;
    clearTimeout(openTimer);
    closeTimer = setTimeout(close, 200);
  }
  function onClick(e) {
    if (trigger !== 'click') return;
    e.stopPropagation();
    if (open) close(); else doOpen();
  }
  function onKey(e) {
    if (trigger === 'click' && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); onClick(e); }
    if (e.key === 'Escape') close();
  }
  function onPanelEnter() { clearTimeout(closeTimer); }
  function onPanelLeave() { if (trigger === 'hover') closeTimer = setTimeout(close, 200); }
</script>

<!-- The trigger uses the ARIA button pattern (role=button + tabindex + keydown)
     when clickable; the linter can't narrow the conditional role, so the
     noninteractive-tabindex check is suppressed deliberately. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<span
  class="tp-trigger"
  class:tp-clickable={trigger === 'click'}
  bind:this={triggerEl}
  on:mouseenter={onEnter}
  on:mouseleave={onLeave}
  on:focus={onEnter}
  on:blur={onLeave}
  on:click={onClick}
  on:keydown={onKey}
  role={trigger === 'click' ? 'button' : undefined}
  tabindex={trigger === 'click' ? 0 : undefined}
><slot /></span>

{#if open}
  <!-- Floating popover; mouse handlers only keep it open while bridging from the
       trigger (hover variant). Its content is reachable via the trigger. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="tp-panel tp-{variant}"
    style="left:{x}px; top:{y}px"
    bind:this={panelEl}
    on:mouseenter={onPanelEnter}
    on:mouseleave={onPanelLeave}
    use:clickOutside={close}
  >
    <TermDefinitionCard {iri} {variant} />
  </div>
{/if}

<style>
  .tp-trigger { display: inline; }
  .tp-clickable { cursor: pointer; }
  .tp-panel {
    position: fixed; z-index: 1000; width: 360px; max-width: calc(100vw - 16px);
    max-height: 70vh; overflow: auto; background: #fff; border: 1px solid #e2e8f0;
    border-radius: 10px; box-shadow: 0 8px 30px rgba(15, 23, 42, 0.18); padding: 0.7rem 0.8rem;
  }
  .tp-compact { width: 300px; }
  :global(html.dark) .tp-panel { background: #0f172a; border-color: #1e293b; box-shadow: 0 8px 30px rgba(0, 0, 0, 0.5); }
</style>
