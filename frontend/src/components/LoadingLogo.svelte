<script>
  // Default app loader: the brand "O" — a knowledge-graph ring with three
  // nodes. The nodes hop around the ring in three 120° steps (returning to
  // their original positions), then the whole mark spins a full turn, then
  // the cycle repeats.
  export let size = 96;
  export let label = 'Loading…';
</script>

<div class="loader" role="status" aria-live="polite">
  <svg
    class="mark"
    width={size}
    height={size}
    viewBox="0 0 64 64"
    fill="none"
    aria-hidden="true"
  >
    <defs>
      <linearGradient id="llRing" gradientUnits="userSpaceOnUse" x1="13" y1="11" x2="51" y2="55">
        <stop offset="0%" stop-color="#cdf6f1" />
        <stop offset="48%" stop-color="#7ED6D0" />
        <stop offset="100%" stop-color="#56b6bd" />
      </linearGradient>
      <radialGradient id="llNode" cx="0.34" cy="0.28" r="0.85">
        <stop offset="0%" stop-color="#d4f7f2" />
        <stop offset="45%" stop-color="#6fcdc9" />
        <stop offset="100%" stop-color="#2F7A8C" />
      </radialGradient>
    </defs>

    <!-- The whole mark spins during the second half of the cycle. -->
    <g class="spin">
      <circle cx="32" cy="32" r="19" stroke="url(#llRing)" stroke-width="4.6" />

      <!-- Edges + nodes hop together around the ring during the first half. -->
      <g class="hop">
        <g stroke="#dbf7f3" stroke-width="1.9" stroke-opacity="0.5" stroke-linecap="round">
          <line x1="32" y1="51" x2="48.45" y2="22.5" />
          <line x1="48.45" y1="22.5" x2="15.55" y2="22.5" />
          <line x1="15.55" y1="22.5" x2="32" y2="51" />
        </g>
        <g stroke="#eafdfb" stroke-width="2.1">
          <circle cx="32" cy="51" r="6.4" fill="url(#llNode)" />
          <circle cx="48.45" cy="22.5" r="6.4" fill="url(#llNode)" />
          <circle cx="15.55" cy="22.5" r="6.4" fill="url(#llNode)" />
        </g>
      </g>
    </g>
  </svg>
  <span class="sr-only">{label}</span>
</div>

<style>
  .loader {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 100%;
  }

  .mark {
    filter: drop-shadow(0 4px 14px rgba(13, 42, 50, 0.35));
  }

  /* SVG groups rotate about the mark's centre (32,32) in the 64x64 viewBox. */
  .spin,
  .hop {
    transform-box: view-box;
    transform-origin: 32px 32px;
  }

  /* First half of the cycle: three discrete 120° hops that bring each node
     to the next node's position, landing back at the start by 48%. The mark
     itself is still here. */
  .hop {
    animation: hop 4.2s infinite;
  }

  /* Second half: the entire mark turns one full revolution. It is held still
     (rotate 0) while the nodes are hopping. */
  .spin {
    animation: spin 4.2s infinite;
  }

  @keyframes hop {
    0%, 8%    { transform: rotate(0deg); }
    16%, 26%  { transform: rotate(120deg); }
    34%, 44%  { transform: rotate(240deg); }
    50%, 100% { transform: rotate(360deg); }
  }

  @keyframes spin {
    0%, 50%   { transform: rotate(0deg); }
    100%      { transform: rotate(360deg); }
  }

  @media (prefers-reduced-motion: reduce) {
    .hop { animation: none; }
    .spin {
      animation: spin 1.6s linear infinite;
    }
    @keyframes spin {
      from { transform: rotate(0deg); }
      to   { transform: rotate(360deg); }
    }
  }

  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
    border: 0;
  }
</style>
