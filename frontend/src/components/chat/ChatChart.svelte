<script>
  // Dependency-free SVG chart for ```chart widget blocks (bar / line / pie).
  // Specs are validated/normalized by chatRich.parseChartSpec(): one or more
  // series of {label, value}. Intentionally small — for anything beyond a quick
  // visual the answer should hand the user a query or CSV instead.
  import { t } from 'svelte-i18n';

  /** @type {{type: 'bar'|'line'|'pie', title?: string, xLabel?: string, yLabel?: string, series: Array<{name: string, data: Array<{label: string, value: number}>}>}} */
  export let spec;

  const PALETTE = ['#6d4ad9', '#0ea5e9', '#16a34a', '#f59e0b', '#dc2626', '#0d9488', '#c026d3', '#64748b'];
  const color = (i) => PALETTE[i % PALETTE.length];

  const PAD_L = 48;
  const PAD_R = 14;
  const PAD_T = 12;
  const PAD_B = 42;
  const PLOT_H = 170;

  $: series = spec?.series || [];
  // X categories: first-seen label order across all series.
  $: labels = [...new Set(series.flatMap((s) => s.data.map((d) => d.label)))];
  $: allValues = series.flatMap((s) => s.data.map((d) => d.value));
  $: yMax = niceCeil(Math.max(0, ...allValues));
  $: yMin = Math.min(0, niceFloor(Math.min(0, ...allValues)));
  $: ySpan = yMax - yMin || 1;
  $: ticks = [0, 0.25, 0.5, 0.75, 1].map((f) => yMin + f * ySpan);

  // Bar geometry: width grows with data; the container scrolls horizontally.
  $: groupW = Math.max(26, Math.min(110, series.length * 16 + 14));
  $: plotW = Math.max(260, labels.length * groupW);
  $: width = PAD_L + plotW + PAD_R;
  $: height = PAD_T + PLOT_H + PAD_B;

  $: y = (v) => PAD_T + PLOT_H - ((v - yMin) / ySpan) * PLOT_H;
  $: groupX = (i) => PAD_L + i * (plotW / Math.max(1, labels.length));
  $: lineX = (i) => PAD_L + (labels.length === 1 ? plotW / 2 : (i * plotW) / (labels.length - 1));

  function niceCeil(v) {
    if (v <= 0) return 1;
    const exp = Math.floor(Math.log10(v));
    const f = v / 10 ** exp;
    const nf = f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10;
    return nf * 10 ** exp;
  }
  function niceFloor(v) {
    return v >= 0 ? 0 : -niceCeil(-v);
  }
  function fmt(n) {
    if (!Number.isFinite(n)) return '';
    if (Number.isInteger(n)) return n.toLocaleString();
    return Math.abs(n) >= 100 ? Math.round(n).toLocaleString() : n.toFixed(2).replace(/\.?0+$/, '');
  }
  function short(s, max = 14) {
    s = String(s ?? '');
    return s.length > max ? `${s.slice(0, max - 1)}…` : s;
  }
  function valueAt(s, label) {
    return s.data.find((d) => d.label === label);
  }

  // Pie: positive slices of the first series.
  $: pieData = (series[0]?.data || []).filter((d) => d.value > 0);
  $: pieTotal = pieData.reduce((a, d) => a + d.value, 0);
  function pieSlices(data, total) {
    const cx = 95;
    const cy = 95;
    const r = 78;
    const r0 = 38;
    let a = -Math.PI / 2;
    return data.map((d, i) => {
      const frac = total ? d.value / total : 0;
      const a2 = a + frac * 2 * Math.PI;
      const large = a2 - a > Math.PI ? 1 : 0;
      // Donut slice: outer arc then inner arc back.
      const p = [
        `M ${cx + r * Math.cos(a)} ${cy + r * Math.sin(a)}`,
        `A ${r} ${r} 0 ${large} 1 ${cx + r * Math.cos(a2)} ${cy + r * Math.sin(a2)}`,
        `L ${cx + r0 * Math.cos(a2)} ${cy + r0 * Math.sin(a2)}`,
        `A ${r0} ${r0} 0 ${large} 0 ${cx + r0 * Math.cos(a)} ${cy + r0 * Math.sin(a)}`,
        'Z',
      ].join(' ');
      const out = { d: p, color: color(i), label: d.label, value: d.value, pct: total ? Math.round((d.value / total) * 100) : 0 };
      a = a2;
      return out;
    });
  }

  $: slices = pieSlices(pieData, pieTotal);

  $: legendSeries = series.filter((s) => s.name);
</script>

<div class="chart">
  {#if spec.title}<p class="title">{spec.title}</p>{/if}

  {#if spec.type === 'pie'}
    <div class="pie-row">
      <svg viewBox="0 0 190 190" width="170" height="170" role="img" aria-label={spec.title || $t('components.chat.chartAlt')}>
        {#each slices as s}
          <path d={s.d} fill={s.color}><title>{s.label}: {fmt(s.value)} ({s.pct}%)</title></path>
        {/each}
      </svg>
      <ul class="pie-legend">
        {#each slices as s}
          <li><span class="dot" style="background:{s.color}"></span><span class="pl-label" title={s.label}>{short(s.label, 24)}</span><span class="pl-val">{fmt(s.value)} · {s.pct}%</span></li>
        {/each}
      </ul>
    </div>
  {:else}
    <div class="plot-scroll">
      <svg viewBox="0 0 {width} {height}" width={width} height={height} role="img" aria-label={spec.title || $t('components.chat.chartAlt')}>
        {#each ticks as tk}
          <line x1={PAD_L} x2={width - PAD_R} y1={y(tk)} y2={y(tk)} class="grid" />
          <text x={PAD_L - 6} y={y(tk) + 3} class="tick" text-anchor="end">{fmt(tk)}</text>
        {/each}
        {#if yMin < 0}
          <line x1={PAD_L} x2={width - PAD_R} y1={y(0)} y2={y(0)} class="zero" />
        {/if}

        {#if spec.type === 'bar'}
          {#each labels as label, i}
            {#each series as s, j}
              {@const d = valueAt(s, label)}
              {#if d}
                {@const barW = (groupW - 10) / series.length}
                {@const x = groupX(i) + 5 + j * barW}
                <rect
                  x={x}
                  y={Math.min(y(d.value), y(0))}
                  width={Math.max(2, barW - 2)}
                  height={Math.max(1, Math.abs(y(d.value) - y(0)))}
                  rx="2"
                  fill={color(j)}
                ><title>{s.name ? `${s.name} · ` : ''}{label}: {fmt(d.value)}</title></rect>
              {/if}
            {/each}
          {/each}
        {:else}
          {#each series as s, j}
            {@const pts = labels
              .map((label, i) => ({ d: valueAt(s, label), i }))
              .filter((p) => p.d)}
            <polyline
              points={pts.map((p) => `${lineX(p.i)},${y(p.d.value)}`).join(' ')}
              fill="none"
              stroke={color(j)}
              stroke-width="2"
            />
            {#each pts as p}
              <circle cx={lineX(p.i)} cy={y(p.d.value)} r="3" fill={color(j)}>
                <title>{s.name ? `${s.name} · ` : ''}{p.d.label}: {fmt(p.d.value)}</title>
              </circle>
            {/each}
          {/each}
        {/if}

        {#each labels as label, i}
          {@const cx = spec.type === 'bar' ? groupX(i) + groupW / 2 - 2 : lineX(i)}
          <text x={cx} y={PAD_T + PLOT_H + 14} class="xlab" text-anchor={label.length > 8 ? 'end' : 'middle'} transform={label.length > 8 ? `rotate(-30 ${cx} ${PAD_T + PLOT_H + 14})` : ''}>
            {short(label)}<title>{label}</title>
          </text>
        {/each}
        {#if spec.xLabel}<text x={PAD_L + plotW / 2} y={height - 4} class="axis-label" text-anchor="middle">{spec.xLabel}</text>{/if}
        {#if spec.yLabel}<text x={12} y={PAD_T + PLOT_H / 2} class="axis-label" text-anchor="middle" transform="rotate(-90 12 {PAD_T + PLOT_H / 2})">{spec.yLabel}</text>{/if}
      </svg>
    </div>
    {#if legendSeries.length}
      <ul class="legend">
        {#each series as s, j}
          <li><span class="dot" style="background:{color(j)}"></span>{s.name || `#${j + 1}`}</li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>

<style>
  .chart {
    margin: 0 0 0.55rem; border: 1px solid var(--line-soft); border-radius: 10px;
    background: var(--bg-strong); padding: 0.55rem 0.65rem 0.45rem;
  }
  .title { margin: 0 0 0.3rem; font-size: 0.8rem; font-weight: 700; color: var(--ink-700); }
  .plot-scroll { overflow-x: auto; }
  .grid { stroke: var(--line-soft, #e5e7eb); stroke-width: 1; }
  .zero { stroke: var(--ink-400, #94a3b8); stroke-width: 1; }
  .tick { font-size: 9px; fill: var(--ink-400, #94a3b8); }
  .xlab { font-size: 9.5px; fill: var(--ink-500, #64748b); }
  .axis-label { font-size: 9.5px; font-weight: 600; fill: var(--ink-500, #64748b); }
  .legend, .pie-legend {
    list-style: none; display: flex; flex-wrap: wrap; gap: 0.25rem 0.8rem;
    margin: 0.3rem 0 0; padding: 0; font-size: 0.72rem; color: var(--ink-600);
  }
  .pie-legend { flex-direction: column; gap: 0.25rem; min-width: 0; }
  .pie-legend li { display: flex; align-items: center; gap: 0.35rem; min-width: 0; }
  .legend li { display: inline-flex; align-items: center; gap: 0.3rem; }
  .dot { width: 9px; height: 9px; border-radius: 3px; flex-shrink: 0; display: inline-block; }
  .pl-label { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .pl-val { margin-left: auto; color: var(--ink-400); white-space: nowrap; }
  .pie-row { display: flex; align-items: center; gap: 1rem; flex-wrap: wrap; }
</style>
