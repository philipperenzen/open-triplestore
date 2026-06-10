<script>
  // Entity info card for ```card widget blocks: title (linked to the in-app
  // resource page when an IRI is given), optional image, and a small fact list.
  // Image/IRI URLs pass the shared scheme allowlist (safeUrl) — a model cannot
  // smuggle javascript: or data: URLs into the card.
  import { navigate } from '../../lib/router/index.js';
  import { safeImageUrl } from '../../lib/safeUrl.js';
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import { IdCard } from 'lucide-svelte';

  /** @type {{title: string, subtitle?: string, iri?: string, image?: string, facts: Array<{label: string, value: string, iri?: string}>}} */
  export let card;

  $: image = safeImageUrl(card.image);

  const isIri = (v) => /^https?:\/\/\S+$/i.test(v || '');
  function open(iri) {
    navigate(`/resource?iri=${encodeURIComponent(iri)}`);
  }
</script>

<div class="card">
  <div class="main">
    <p class="title">
      <IdCard size={14} />
      {#if card.iri}
        <button class="title-link" title={card.iri} on:click={() => open(card.iri)}>{card.title}</button>
      {:else}
        <span>{card.title}</span>
      {/if}
    </p>
    {#if card.subtitle}<p class="subtitle">{card.subtitle}</p>{/if}
    {#if card.iri}<p class="iri" title={card.iri}>{shortenIRI(card.iri)}</p>{/if}
    {#if card.facts.length}
      <dl class="facts">
        {#each card.facts as f}
          <dt>{f.label}</dt>
          <dd>
            {#if f.iri && isIri(f.iri)}
              <button class="link" title={f.iri} on:click={() => open(f.iri)}>{f.value || shortenIRI(f.iri)}</button>
            {:else if isIri(f.value)}
              <button class="link" title={f.value} on:click={() => open(f.value)}>{shortenIRI(f.value)}</button>
            {:else}
              {f.value}
            {/if}
          </dd>
        {/each}
      </dl>
    {/if}
  </div>
  {#if image}
    <img class="thumb" src={image} alt={card.title} loading="lazy" on:error={() => { image = null; }} />
  {/if}
</div>

<style>
  .card {
    display: flex; gap: 0.75rem; align-items: flex-start;
    margin: 0 0 0.55rem; padding: 0.6rem 0.7rem;
    border: 1px solid var(--line-soft); border-radius: 10px;
    background: var(--bg-strong);
    box-shadow: 0 1px 3px rgba(15,32,39,0.05);
  }
  .main { flex: 1; min-width: 0; }
  .title {
    display: flex; align-items: center; gap: 0.35rem;
    margin: 0; font-size: 0.92rem; font-weight: 700; color: var(--ink-800);
  }
  .title :global(svg) { color: #6d4ad9; flex-shrink: 0; }
  .title-link {
    background: none; border: none; padding: 0; cursor: pointer;
    font: inherit; color: inherit; text-align: left;
  }
  .title-link:hover { color: #4f46e5; text-decoration: underline; }
  .subtitle { margin: 0.15rem 0 0; font-size: 0.8rem; color: var(--ink-600); }
  .iri {
    margin: 0.15rem 0 0; font-family: 'SF Mono', ui-monospace, monospace;
    font-size: 0.7rem; color: var(--ink-400);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .facts {
    display: grid; grid-template-columns: max-content 1fr; gap: 0.15rem 0.8rem;
    margin: 0.5rem 0 0;
  }
  dt { font-size: 0.74rem; font-weight: 600; color: var(--ink-500); }
  dd { margin: 0; font-size: 0.78rem; color: var(--ink-800); word-break: break-word; }
  .link {
    background: none; border: none; cursor: pointer; padding: 0; font-size: 0.78rem;
    color: #4f46e5; text-decoration: underline; text-decoration-color: rgba(79,70,229,0.35);
  }
  .link:hover { text-decoration-color: currentColor; }
  .thumb {
    width: 116px; max-height: 116px; object-fit: cover;
    border-radius: 8px; border: 1px solid var(--line-soft); flex-shrink: 0;
  }
  :global(:is([data-theme="dark"], .dark)) .title :global(svg) { color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .link, :global(:is([data-theme="dark"], .dark)) .title-link:hover { color: #a5b4fc; }
</style>
