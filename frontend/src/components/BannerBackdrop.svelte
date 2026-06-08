<!--
  BannerBackdrop — the shared background layer for a page hero/cover. Given a
  stored `banner_key`, it renders one of:
    • an uploaded image (object-fit cover), or
    • a built-in preset: a themed gradient, optionally with the animated
      linked-data canvas on top (the same motif as the landing hero), or
    • when nothing is set, a stable per-page default animated preset.
  Fills its positioned parent as an absolute, non-interactive layer (z-index 0);
  the parent layers its glass header above at z-index 1.
-->
<script>
  import LinkedDataBackground from './LinkedDataBackground.svelte';
  import { parseBanner, getPreset, defaultPresetFor } from '../lib/banners';

  /** Raw `banner_key` from the dataset/org record (object key, `preset:<id>`, or null). */
  export let bannerKey = null;
  /** Resolved URL for an uploaded banner image (include any cache-bust query). */
  export let imageUrl = '';
  /** Seed string (e.g. record id) used to pick a stable default preset when unset. */
  export let seed = '';

  $: parsed = parseBanner(bannerKey);
  $: preset =
    parsed.type === 'preset'
      ? getPreset(parsed.presetId)
      : parsed.type === 'none'
        ? defaultPresetFor(seed)
        : null; // uploaded image → no gradient/animation overlay
  $: animated = !!preset && preset.kind === 'animated';

  function hideBroken(e) {
    /** @type {HTMLElement} */ (e.currentTarget).style.display = 'none';
  }
</script>

<div
  class="banner-backdrop"
  style={preset ? `--bd-gradient:${preset.gradient}` : ''}
  aria-hidden="true"
>
  {#if parsed.type === 'upload'}
    <img class="bd-img" src={imageUrl} alt="" on:error={hideBroken} />
  {/if}
  {#if animated}
    <LinkedDataBackground color={preset.color} intensity={preset.intensity} />
  {/if}
</div>

<style>
  .banner-backdrop {
    position: absolute;
    inset: 0;
    z-index: 0;
    overflow: hidden;
    background: var(--bd-gradient, linear-gradient(135deg, #0f2a33 0%, #1e5663 55%, #2f7a8c 100%));
  }
  .bd-img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
</style>
