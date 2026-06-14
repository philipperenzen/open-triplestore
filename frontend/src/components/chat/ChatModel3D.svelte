<script>
  // 3D-model widget for ```model3d blocks: one or more URL-referenced models in
  // the shared orbit viewer (viewer/Model3D), laid out on the same √n ground
  // grid the dataset viewer uses (lib/viewer/geometry.ts modelRefs). Model3D
  // pulls the heavy three.js chunk, so it is imported on demand — like
  // ResourceDetail/PreviewOverlay do — and chat answers without 3D never pay
  // for it. URLs were already gated in chatRich.parseModel3dSpec (scheme
  // allowlist + extension-detected format).
  import { t } from 'svelte-i18n';
  import { Boxes } from 'lucide-svelte';

  const model3d = () => import('../viewer/Model3D.svelte');

  /** @type {Array<{id: string, label: string, url: string, format: string}>} */
  export let models = [];

  // Grid slots, same maths as geometry.ts modelRefs(): √n columns, 3 m apart.
  const SPACING = 3;
  $: cols = Math.max(1, Math.ceil(Math.sqrt(models.length)));
  $: refs = models.map((m, i) => ({
    ...m,
    slot: [(i % cols) * SPACING, Math.floor(i / cols) * SPACING],
  }));
  $: labelled = models.filter((m) => m.label);
</script>

<div class="model-block">
  <div class="head">
    <span class="title"><Boxes size={13} /> {$t('components.chat.model3dTitle')}</span>
    <span class="hint">{$t('components.chat.model3dHint')}</span>
  </div>
  <div class="stage">
    {#await model3d()}
      <div class="placeholder">{$t('components.chat.model3dLoading')}</div>
    {:then mod}
      <svelte:component this={mod.default} {refs} height="100%" />
    {:catch}
      <div class="placeholder failed">{$t('components.chat.model3dFailed')}</div>
    {/await}
  </div>
  {#if labelled.length}
    <ul class="legend">
      {#each labelled as m}
        <li title={m.url}>{m.label}</li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .model-block {
    margin: 0 0 0.55rem; border: 1px solid var(--line-soft); border-radius: 10px;
    background: var(--bg-strong); padding: 0.55rem 0.65rem;
  }
  .head {
    display: flex; align-items: baseline; justify-content: space-between;
    gap: 0.6rem; margin: 0 0 0.4rem; flex-wrap: wrap;
  }
  .title {
    display: inline-flex; align-items: center; gap: 0.3rem;
    font-size: 0.8rem; font-weight: 700; color: var(--ink-700);
  }
  .title :global(svg) { color: #e8590c; flex-shrink: 0; }
  .hint { font-size: 0.7rem; color: var(--ink-400); }
  .stage { height: 300px; }
  .placeholder {
    height: 100%; display: flex; align-items: center; justify-content: center;
    border: 1px dashed var(--line-strong); border-radius: 10px;
    font-size: 0.78rem; color: var(--ink-400); background: var(--bg-soft, #eef2f6);
  }
  .placeholder.failed { font-style: italic; }
  .legend {
    list-style: none; display: flex; flex-wrap: wrap; gap: 0.2rem 0.7rem;
    margin: 0.35rem 0 0; padding: 0; font-size: 0.74rem; color: var(--ink-600);
  }
</style>
