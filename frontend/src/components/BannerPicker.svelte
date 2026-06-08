<!--
  BannerPicker — choose a built-in default banner (animated or gradient), upload
  your own image, or remove the current banner. Used inside the dataset and
  organisation metadata editors. The parent owns persistence; this only emits
  intent:
    • selectPreset { preset }  — a built-in preset id was chosen
    • upload       { file }    — a file was picked to upload
    • clear        ()          — revert to the default (no custom banner)
-->
<script>
  import { createEventDispatcher } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { ImagePlus, Loader2, Check } from 'lucide-svelte';
  import { BANNER_PRESETS, parseBanner } from '../lib/banners';

  /** Live `banner_key` from the parent (object key, `preset:<id>`, true, or null). */
  export let bannerKey = null;
  /** URL of the uploaded banner image (with cache-bust), for the "uploaded" chip. */
  export let imageUrl = '';
  /** True while an upload/preset change is in flight. */
  export let uploading = false;

  const dispatch = createEventDispatcher();
  let fileInputEl;

  $: parsed = parseBanner(bannerKey);
  $: activePreset = parsed.type === 'preset' ? parsed.presetId : null;
  $: isUpload = parsed.type === 'upload';
  $: hasSelection = parsed.type !== 'none';

  function pick(e) {
    const f = e.target.files?.[0];
    if (f) dispatch('upload', { file: f });
    if (fileInputEl) fileInputEl.value = '';
  }
</script>

<div class="banner-picker">
  <div class="bp-grid">
    {#each BANNER_PRESETS as p (p.id)}
      <button
        type="button"
        class="bp-swatch"
        class:active={activePreset === p.id}
        style="background:{p.gradient}"
        on:click={() => dispatch('selectPreset', { preset: p.id })}
        title={p.name}
        aria-label={p.name}
        aria-pressed={activePreset === p.id}
      >
        {#if p.kind === 'animated'}
          <span class="bp-dot" style="background:rgb({p.color})" aria-hidden="true"></span>
        {/if}
        {#if activePreset === p.id}<span class="bp-check"><Check size={12} /></span>{/if}
        <span class="bp-name">{p.name}</span>
      </button>
    {/each}

    <label class="bp-swatch bp-upload" class:active={isUpload} title={$i18nT('components.bannerPicker.uploadOwn')}>
      {#if uploading}<Loader2 size={16} class="animate-spin" />{:else}<ImagePlus size={16} />{/if}
      {#if isUpload && !uploading}<span class="bp-check"><Check size={12} /></span>{/if}
      <span class="bp-name">{$i18nT('components.bannerPicker.uploadOwn')}</span>
      <input bind:this={fileInputEl} type="file" accept="image/*" on:change={pick} style="display:none" />
    </label>
  </div>

  <div class="bp-foot">
    {#if isUpload && imageUrl}
      <span class="bp-current"><img src={imageUrl} alt="" /> {$i18nT('components.bannerPicker.uploaded')}</span>
    {/if}
    {#if hasSelection}
      <button type="button" class="bp-clear" on:click={() => dispatch('clear')}>
        {$i18nT('components.bannerPicker.removeBanner')}
      </button>
    {/if}
  </div>
</div>

<style>
  .banner-picker { display: flex; flex-direction: column; gap: 0.5rem; }
  .bp-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(84px, 1fr));
    gap: 0.5rem;
  }
  .bp-swatch {
    position: relative;
    height: 52px;
    border-radius: 8px;
    border: 1px solid var(--line-soft, #e5e7eb);
    cursor: pointer;
    overflow: hidden;
    padding: 0;
    color: #fff;
    display: flex;
    align-items: flex-end;
    justify-content: flex-start;
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.06);
  }
  .bp-swatch.active {
    outline: 2px solid var(--brand-500, #3a95a6);
    outline-offset: 1px;
    border-color: transparent;
  }
  .bp-name {
    position: relative;
    z-index: 1;
    font-size: 0.66rem;
    font-weight: 600;
    padding: 0.15rem 0.35rem;
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.5);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }
  .bp-dot {
    position: absolute;
    top: 6px;
    left: 6px;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.35);
  }
  .bp-check {
    position: absolute;
    top: 4px;
    right: 4px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--brand-500, #3a95a6);
    color: #fff;
    z-index: 2;
  }
  .bp-upload {
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.15rem;
    background: var(--bg-subtle, #f3f4f6);
    color: var(--ink-500, #6b7280);
  }
  .bp-upload .bp-name { text-shadow: none; color: var(--ink-600, #475569); }
  .bp-foot { display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap; min-height: 22px; }
  .bp-current {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.78rem;
    color: var(--ink-500, #6b7280);
  }
  .bp-current img {
    width: 34px;
    height: 20px;
    object-fit: cover;
    border-radius: 4px;
    border: 1px solid var(--line-soft, #e5e7eb);
  }
  .bp-clear {
    background: transparent;
    border: none;
    cursor: pointer;
    font-size: 0.78rem;
    color: var(--brand-600, #2563eb);
    padding: 0;
    margin-left: auto;
    text-decoration: underline;
  }
  .bp-clear:hover { color: var(--danger-500, #dc2626); }
  :global(:is([data-theme="dark"], .dark)) .bp-upload { background: var(--bg-soft); }
</style>
