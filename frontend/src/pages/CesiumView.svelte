<script>
  // Page hosting the CesiumJS 3D-Tiles viewer for a single dataset. Mirrors the
  // DatasetViewer page chrome (back link + title) but hands the canvas entirely
  // to Cesium, whose native depth/terrain handling avoids the MapLibre+three.js
  // 3D failure modes the 2D explorer works around.
  import { Link } from '../lib/router/index.js';
  import { ChevronLeft, Boxes } from 'lucide-svelte';
  import CesiumViewer from '../components/viewer/CesiumViewer.svelte';

  export let id = '';
</script>

<div class="page cesium-page">
  <div class="page-head">
    <Link to={`/datasets/${id}/viewer`} class="btn btn-sm">
      <ChevronLeft size={16} />
      Back to explorer
    </Link>
    <h1><Boxes size={18} /> 3D Tiles viewer</h1>
  </div>

  <section class="canvas">
    <CesiumViewer datasetId={id} height="100%" />
  </section>
</div>

<style>
  .cesium-page {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 90px);
  }
  .page-head {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.6rem;
  }
  .page-head h1 {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 1.2rem;
    color: var(--ink-900, #0f172a);
  }
  .canvas {
    flex: 1;
    min-height: 0;
    border: 1px solid var(--border, #e2e8f0);
    border-radius: var(--radius-lg, 12px);
    background: var(--bg-elevated, #fff);
    overflow: hidden;
  }
</style>
