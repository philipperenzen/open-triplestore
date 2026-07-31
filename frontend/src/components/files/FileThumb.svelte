<script>
  // Lazy thumbnail / kind icon for one asset tile or row.
  //
  // Images load their bytes only when the tile scrolls into view
  // (IntersectionObserver), preferring the server-generated 256px sibling
  // (`<id>-thumb.png`, feature asset-thumbnail) and falling back to the full
  // image when it is reasonably small. Everything else renders its kind icon.
  // Object URLs are revoked on destroy; the immutable-ETag cache makes
  // re-fetches after remount effectively free.
  import { onDestroy, onMount } from 'svelte';
  import { fetchAssetContent } from '../../lib/api.js';
  import { fileKind } from '../../lib/files';
  import {
    File, FileText, FileCode, FileSpreadsheet, FileArchive,
    Image as ImageIcon, Film, Music, Boxes, Map as MapIcon, Scan, Layers,
  } from 'lucide-svelte';

  export let datasetId;
  export let asset;
  /** Optional smaller sibling asset to fetch instead of the full image. */
  export let thumbAsset = null;
  /** Icon size in px (image thumbs fill their container). */
  export let iconSize = 22;

  const FULL_IMAGE_THUMB_LIMIT = 6 * 1024 * 1024;

  const ICONS = {
    image: ImageIcon, video: Film, audio: Music, model3d: Boxes,
    document: FileText, text: FileCode, spreadsheet: FileSpreadsheet,
    archive: FileArchive, geodata: MapIcon, pointcloud: Scan, cad: Layers,
    generic: File,
  };

  let el;
  let objectUrl = null;
  let failed = false;
  let started = false;

  $: kind = fileKind(asset?.content_type, asset?.filename || '');
  $: icon = ICONS[kind] || File;
  $: source = kind === 'image'
    ? (thumbAsset || (asset && asset.size_bytes <= FULL_IMAGE_THUMB_LIMIT ? asset : null))
    : null;

  async function load() {
    if (started || !source || !datasetId) return;
    started = true;
    try {
      const res = await fetchAssetContent(datasetId, source.id);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const blob = await res.blob();
      objectUrl = URL.createObjectURL(blob);
    } catch {
      failed = true;
    }
  }

  onMount(() => {
    if (!source) return;
    if (typeof IntersectionObserver === 'undefined') { load(); return; }
    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) {
        io.disconnect();
        load();
      }
    }, { rootMargin: '160px' });
    io.observe(el);
    return () => io.disconnect();
  });

  onDestroy(() => {
    if (objectUrl) URL.revokeObjectURL(objectUrl);
  });
</script>

<div class="thumb kind-{kind}" bind:this={el}>
  {#if objectUrl && !failed}
    <img src={objectUrl} alt="" loading="lazy" />
  {:else}
    <svelte:component this={icon} size={iconSize} />
  {/if}
</div>

<style>
  .thumb {
    width: 100%;
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    border-radius: inherit;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }

  /* Kind tints (icon color only; tiles set their own backgrounds). */
  .kind-image { color: #0ea5e9; }
  .kind-video { color: #8b5cf6; }
  .kind-audio { color: #d946ef; }
  .kind-model3d { color: #f59e0b; }
  .kind-cad { color: #ea580c; }
  .kind-pointcloud { color: #10b981; }
  .kind-geodata { color: #059669; }
  .kind-document { color: #2563eb; }
  .kind-text { color: #64748b; }
  .kind-spreadsheet { color: #16a34a; }
  .kind-archive { color: #a16207; }
  .kind-generic { color: #94a3b8; }
</style>
