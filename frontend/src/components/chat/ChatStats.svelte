<script>
  // KPI tiles for a single-row aggregate result (`SELECT (COUNT(…) AS ?n)
  // (MIN(?year) AS ?oldest) …`): one tile per returned cell, the value exactly
  // as the store returned it (numbers only get thousands separators).
  /** @type {import('../../lib/chatViz').StatTile[]} */
  export let tiles = [];

  function fmt(tile) {
    if (!tile.numeric) return tile.value;
    const n = Number(tile.value);
    return Number.isInteger(n) ? n.toLocaleString() : tile.value;
  }
</script>

<ul class="stats" data-testid="chat-stats">
  {#each tiles as tile}
    <li class="tile">
      <span class="value" class:text={!tile.numeric} title={tile.value}>{fmt(tile)}</span>
      <span class="label">{tile.label}</span>
    </li>
  {/each}
</ul>

<style>
  .stats {
    list-style: none; margin: 0 0 0.55rem; padding: 0;
    display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 0.45rem;
  }
  .tile {
    display: flex; flex-direction: column; gap: 0.15rem; min-width: 0;
    padding: 0.55rem 0.7rem; border-radius: 10px;
    border: 1px solid var(--line-soft); background: var(--bg-strong);
  }
  .value {
    font-size: 1.25rem; font-weight: 700; color: var(--ink-800, #1e293b);
    font-variant-numeric: tabular-nums; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .value.text { font-size: 0.9rem; }
  .label { font-size: 0.7rem; color: var(--ink-400); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
