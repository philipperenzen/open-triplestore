<script>
  // The one floating resource-preview card (see lib/resourcePreview.js).
  // Mounted once in App.svelte; renders whatever `hoverCard` holds. Pointer
  // events stay off so the card can never trap the hover that opened it —
  // interaction happens on the anchor (click = open the resource page).
  import { t } from 'svelte-i18n';
  import { hoverCard } from '../lib/resourcePreview.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { Box, ExternalLink } from 'lucide-svelte';

  const CARD_W = 320;
  const MARGIN = 8;

  // Clamp into the viewport: prefer below the anchor, flip above when the
  // bottom edge would clip (the card's height is bounded by its max sizes).
  $: pos = (() => {
    if (!$hoverCard) return null;
    const vw = typeof window !== 'undefined' ? window.innerWidth : 1280;
    const vh = typeof window !== 'undefined' ? window.innerHeight : 800;
    const x = Math.min(Math.max($hoverCard.x, MARGIN), vw - CARD_W - MARGIN);
    const below = $hoverCard.y + 6;
    const flip = below > vh - 180;
    return { x, y: flip ? undefined : below, bottom: flip ? vh - $hoverCard.y + 26 : undefined };
  })();
</script>

{#if $hoverCard && pos}
  <div
    class="resource-hover-card"
    style:left="{pos.x}px"
    style:top={pos.y !== undefined ? `${pos.y}px` : null}
    style:bottom={pos.bottom !== undefined ? `${pos.bottom}px` : null}
    role="tooltip"
    aria-live="polite"
  >
    {#if $hoverCard.state === 'loading'}
      <div class="row muted">
        <span class="spinner" aria-hidden="true"></span>
        {$t('components.resourceCard.loading')}
      </div>
    {:else if $hoverCard.preview?.known}
      <div class="title">
        <Box size={13} aria-hidden="true" />
        <strong>{$hoverCard.preview.label || shortenIRI($hoverCard.iri)}</strong>
      </div>
      {#if $hoverCard.preview.types.length}
        <div class="types">
          {#each $hoverCard.preview.types as ty (ty)}
            <span class="type-chip">{ty}</span>
          {/each}
        </div>
      {/if}
      {#if $hoverCard.preview.description}
        <p class="desc">{$hoverCard.preview.description}</p>
      {/if}
      <div class="iri" title={$hoverCard.iri}>{shortenIRI($hoverCard.iri)}</div>
      <div class="row muted">
        {$t('components.resourceCard.facts', {
          values: { count: $hoverCard.preview.facts, more: $hoverCard.preview.more ? '+' : '' },
        })}
        · {$t('components.resourceCard.clickToOpen')}
      </div>
    {:else}
      <div class="title">
        <ExternalLink size={13} aria-hidden="true" />
        <strong>{shortenIRI($hoverCard.iri)}</strong>
      </div>
      <div class="row muted">{$t('components.resourceCard.notInStore')}</div>
    {/if}
  </div>
{/if}

<style>
  .resource-hover-card {
    position: fixed;
    z-index: 3000;
    width: 320px;
    max-height: 260px;
    overflow: hidden;
    padding: 0.6rem 0.7rem;
    border-radius: 10px;
    background: var(--panel, #fff);
    border: 1px solid var(--line-soft, #e2e8f0);
    box-shadow: 0 8px 24px rgba(15, 23, 42, 0.14);
    font-size: 0.78rem;
    line-height: 1.45;
    pointer-events: none;
    color: var(--ink-700, #334155);
  }
  .title { display: flex; align-items: center; gap: 0.4rem; margin-bottom: 0.25rem; }
  .title strong { font-size: 0.82rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .types { display: flex; flex-wrap: wrap; gap: 0.25rem; margin: 0.15rem 0 0.3rem; }
  .type-chip {
    padding: 0.05rem 0.45rem; border-radius: 999px; font-size: 0.68rem;
    background: var(--accent-soft, #eef2ff); color: var(--accent, #4338ca);
    border: 1px solid var(--line-soft, #e2e8f0);
  }
  .desc {
    margin: 0 0 0.3rem;
    display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .iri {
    font-family: 'SF Mono', ui-monospace, monospace; font-size: 0.68rem;
    color: var(--ink-400, #94a3b8);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    margin-bottom: 0.2rem;
  }
  .row { display: flex; align-items: center; gap: 0.35rem; }
  .muted { color: var(--ink-400, #94a3b8); font-size: 0.7rem; }
  .spinner {
    width: 10px; height: 10px; border-radius: 50%;
    border: 2px solid var(--line-soft, #e2e8f0); border-top-color: var(--accent, #4338ca);
    animation: rhc-spin 0.7s linear infinite;
  }
  @keyframes rhc-spin { to { transform: rotate(360deg); } }

  :global(:is([data-theme='dark'], .dark)) .resource-hover-card {
    background: var(--panel, #1e293b);
    border-color: var(--line-soft, #334155);
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.45);
    color: var(--ink-200, #e2e8f0);
  }
  :global(:is([data-theme='dark'], .dark)) .type-chip {
    background: rgba(99, 102, 241, 0.15); color: #a5b4fc; border-color: rgba(99, 102, 241, 0.3);
  }
</style>
