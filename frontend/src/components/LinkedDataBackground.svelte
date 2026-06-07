<!--
  Ambient "linked data" background: nodes drift slowly and edges fade in/out as
  nodes pass within linking distance — a pleasant connect/disconnect effect.
  Fills its (positioned) parent as an absolute, non-interactive canvas. Honours
  prefers-reduced-motion by rendering a single static frame.
-->
<script>
  import { onMount, onDestroy } from 'svelte';

  /** RGB triplet (no rgb() wrapper) for nodes and edges. */
  export let color = '255, 255, 255';
  /** Nodes per px² of the canvas (clamped by min/maxNodes). */
  export let density = 0.00010;
  export let minNodes = 10;
  export let maxNodes = 70;
  /** Distance (px) under which two nodes are linked by a fading edge. */
  export let linkDist = 140;
  /** Per-frame drift speed (px). */
  export let speed = 0.18;
  /** Base opacity multiplier for the whole layer. */
  export let intensity = 1;

  let canvas;
  let ctx = null;
  let raf = 0;
  let ro = null;
  let nodes = [];
  let w = 0;
  let h = 0;
  let reduced = false;

  function resize() {
    if (!canvas) return;
    const rect = canvas.getBoundingClientRect();
    w = Math.max(1, Math.round(rect.width));
    h = Math.max(1, Math.round(rect.height));
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    ctx = canvas.getContext('2d');
    if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    seed();
    if (reduced) draw(false);
  }

  function seed() {
    const target = Math.min(maxNodes, Math.max(minNodes, Math.round(w * h * density)));
    nodes = Array.from({ length: target }, () => ({
      x: Math.random() * w,
      y: Math.random() * h,
      vx: (Math.random() - 0.5) * speed,
      vy: (Math.random() - 0.5) * speed,
      r: 1.4 + Math.random() * 2.2,
      // Slow opacity pulse so individual nodes gently breathe.
      ph: Math.random() * Math.PI * 2,
    }));
  }

  function draw(move) {
    if (!ctx) return;
    ctx.clearRect(0, 0, w, h);

    if (move) {
      for (const n of nodes) {
        n.x += n.vx;
        n.y += n.vy;
        if (n.x <= 0 || n.x >= w) n.vx *= -1;
        if (n.y <= 0 || n.y >= h) n.vy *= -1;
        n.x = Math.max(0, Math.min(w, n.x));
        n.y = Math.max(0, Math.min(h, n.y));
        n.ph += 0.012;
      }
    }

    // Edges — opacity scales with closeness, so links fade in/out as nodes drift.
    for (let i = 0; i < nodes.length; i++) {
      const a = nodes[i];
      for (let j = i + 1; j < nodes.length; j++) {
        const b = nodes[j];
        const dx = a.x - b.x;
        const dy = a.y - b.y;
        const d = Math.hypot(dx, dy);
        if (d < linkDist) {
          const o = (1 - d / linkDist) * 0.45 * intensity;
          ctx.strokeStyle = `rgba(${color}, ${o})`;
          ctx.lineWidth = 1;
          ctx.beginPath();
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
          ctx.stroke();
        }
      }
    }

    // Nodes.
    for (const n of nodes) {
      const o = (0.55 + 0.35 * Math.sin(n.ph)) * intensity;
      ctx.fillStyle = `rgba(${color}, ${o})`;
      ctx.beginPath();
      ctx.arc(n.x, n.y, n.r, 0, Math.PI * 2);
      ctx.fill();
    }
  }

  function loop() {
    draw(true);
    raf = requestAnimationFrame(loop);
  }

  onMount(() => {
    reduced =
      typeof window !== 'undefined' &&
      window.matchMedia &&
      window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    resize();
    if (typeof ResizeObserver !== 'undefined') {
      ro = new ResizeObserver(() => resize());
      ro.observe(canvas);
    }
    if (!reduced) loop();
  });

  onDestroy(() => {
    if (raf) cancelAnimationFrame(raf);
    if (ro) ro.disconnect();
  });
</script>

<canvas bind:this={canvas} class="ld-bg" aria-hidden="true"></canvas>

<style>
  .ld-bg {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
    pointer-events: none;
    z-index: 0;
  }
</style>
