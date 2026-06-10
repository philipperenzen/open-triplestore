<script context="module">
  import { copyToClipboard } from '../../lib/clipboard.js';

  /**
   * Copy `text` to the clipboard and flip a "copied" flag for 1.5 s.
   * @param {string} text
   * @param {(copied: boolean) => void} set receives true immediately, false after the reset
   */
  export async function copyWithReset(text, set) {
    await copyToClipboard(text);
    set(true);
    setTimeout(() => set(false), 1500);
  }
</script>

<script>
  // Shared shell for runnable cards in chat answers (SPARQL / API): the bordered
  // block, the header row (label left, actions right), and the base styling for
  // `.act` action buttons and the `.elapsed` badge slotted into the header.
  // The only per-card styling difference is the Run-button accent palette,
  // selected by `accent` and wired through CSS custom properties so the
  // dark-theme variants stay with the palette definition.

  /** Run-button accent: 'indigo' (SPARQL cards) or 'emerald' (API cards). */
  export let accent = 'indigo';
</script>

<div class="block {accent}">
  <div class="head">
    <slot name="label" />
    <slot name="actions" />
  </div>
  <slot />
</div>

<style>
  .block {
    margin: 0 0 0.55rem; border: 1px solid var(--line-soft); border-radius: 10px;
    background: var(--bg-soft); overflow: hidden;
  }
  .head {
    display: flex; align-items: center; justify-content: space-between; gap: 0.5rem;
    padding: 0.3rem 0.55rem; border-bottom: 1px solid var(--line-soft);
  }
  .head :global(.elapsed) { font-size: 0.68rem; color: var(--ink-400); }
  .head :global(.act) {
    display: inline-flex; align-items: center; gap: 0.25rem; cursor: pointer;
    font-size: 0.7rem; font-weight: 600; padding: 2px 7px; border-radius: 6px;
    background: var(--bg-strong); border: 1px solid var(--line-soft); color: var(--ink-600);
  }
  .head :global(.act:hover:not(:disabled)) { background: var(--bg-elevated); border-color: var(--line-strong); }
  .head :global(.act:disabled) { opacity: 0.6; cursor: default; }
  .head :global(.act.run) { background: var(--run-bg); border-color: var(--run-border); color: var(--run-ink); }
  .head :global(.act.run:hover:not(:disabled)) { background: var(--run-bg-hover); }

  .block.indigo { --run-bg: #eef2ff; --run-border: #c7d2fe; --run-ink: #4338ca; --run-bg-hover: #e0e7ff; }
  .block.emerald { --run-bg: #ecfdf5; --run-border: #a7f3d0; --run-ink: #047857; --run-bg-hover: #d1fae5; }
  :global(:is([data-theme="dark"], .dark)) .block.indigo {
    --run-bg: rgba(99,102,241,0.2); --run-border: rgba(99,102,241,0.3); --run-ink: #a5b4fc; --run-bg-hover: rgba(99,102,241,0.28);
  }
  :global(:is([data-theme="dark"], .dark)) .block.emerald {
    --run-bg: rgba(16,185,129,0.15); --run-border: rgba(16,185,129,0.3); --run-ink: #6ee7b7; --run-bg-hover: rgba(16,185,129,0.25);
  }
</style>
