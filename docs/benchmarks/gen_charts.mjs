// Minimal dependency-free SVG chart generator for the performance report.
//
// Usage: node gen_charts.mjs <charts.json> <out-dir>
// charts.json: [ { file, title, subtitle, unit, lower_is_better, series:[name,...]?,
//                  bars:[ {label, values:[..]} | {label, value} ] } ]
//
// Emits one .svg per chart entry. Horizontal bars (good for long labels); grouped
// when each bar carries multiple values (e.g. Open Triplestore vs Fuseki).

import { readFileSync, writeFileSync } from 'node:fs';

const PALETTE = ['#2563eb', '#ea580c', '#16a34a', '#9333ea', '#0891b2'];
const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

function fmt(v, unit) {
  if (v == null || Number.isNaN(v)) return 'n/a';
  if (unit === 'x') return v.toFixed(2) + '×';
  if (unit === '%') return v.toFixed(0) + '%';
  // time/throughput: keep 3 significant figures
  const abs = Math.abs(v);
  let s;
  if (abs >= 100) s = v.toFixed(0);
  else if (abs >= 10) s = v.toFixed(1);
  else s = v.toFixed(2);
  return unit ? `${s} ${unit}` : s;
}

function chart(spec) {
  const series = spec.series && spec.series.length ? spec.series : [spec.title];
  const bars = spec.bars.map((b) => ({
    label: b.label,
    values: b.values != null ? b.values : [b.value],
  }));
  const groups = series.length;

  // Largest value across all bars/series for scaling.
  const maxVal = Math.max(...bars.flatMap((b) => b.values.map((v) => (v == null ? 0 : v))), 1e-9);

  const W = 760;
  const padL = 168, padR = 92, padT = spec.subtitle ? 58 : 40, padB = 34;
  const barH = 17, gGap = 4, bGap = 16;
  const rowH = groups * barH + (groups - 1) * gGap + bGap;
  const plotW = W - padL - padR;
  const H = padT + bars.length * rowH + padB;

  const out = [];
  out.push(`<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}" font-family="ui-sans-serif,system-ui,Segoe UI,Roboto,Arial" font-size="12">`);
  out.push(`<rect width="${W}" height="${H}" fill="#ffffff"/>`);
  out.push(`<text x="16" y="24" font-size="15" font-weight="700" fill="#0f172a">${esc(spec.title)}</text>`);
  if (spec.subtitle) out.push(`<text x="16" y="42" font-size="11" fill="#64748b">${esc(spec.subtitle)}</text>`);

  // Vertical gridlines (axis).
  const ticks = 4;
  for (let t = 0; t <= ticks; t++) {
    const x = padL + (plotW * t) / ticks;
    out.push(`<line x1="${x.toFixed(1)}" y1="${padT - 6}" x2="${x.toFixed(1)}" y2="${H - padB}" stroke="#eef2f7"/>`);
    out.push(`<text x="${x.toFixed(1)}" y="${H - padB + 14}" font-size="9" fill="#94a3b8" text-anchor="middle">${fmt((maxVal * t) / ticks, spec.unit)}</text>`);
  }

  bars.forEach((b, i) => {
    const y0 = padT + i * rowH;
    out.push(`<text x="${padL - 8}" y="${y0 + (groups * barH + (groups - 1) * gGap) / 2 + 4}" text-anchor="end" fill="#334155" font-size="11">${esc(b.label)}</text>`);
    b.values.forEach((v, g) => {
      const y = y0 + g * (barH + gGap);
      const w = v == null ? 0 : Math.max(1, (v / maxVal) * plotW);
      const color = PALETTE[g % PALETTE.length];
      out.push(`<rect x="${padL}" y="${y}" width="${w.toFixed(1)}" height="${barH}" rx="2" fill="${color}"/>`);
      out.push(`<text x="${(padL + w + 5).toFixed(1)}" y="${y + barH - 4}" font-size="10" fill="#475569">${fmt(v, spec.unit)}</text>`);
    });
  });

  // Legend (grouped charts only).
  if (groups > 1) {
    let lx = padL;
    const ly = H - 6;
    series.forEach((name, g) => {
      out.push(`<rect x="${lx}" y="${ly - 9}" width="10" height="10" rx="2" fill="${PALETTE[g % PALETTE.length]}"/>`);
      out.push(`<text x="${lx + 14}" y="${ly}" font-size="10" fill="#475569">${esc(name)}</text>`);
      lx += 16 + name.length * 6.2 + 14;
    });
  }
  const note = spec.lower_is_better ? 'lower is better' : (spec.higher_is_better ? 'higher is better' : '');
  if (note) out.push(`<text x="${W - 12}" y="20" font-size="10" fill="#94a3b8" text-anchor="end">${note}</text>`);

  out.push('</svg>');
  return out.join('\n');
}

const [, , dataPath, outDir] = process.argv;
const specs = JSON.parse(readFileSync(dataPath, 'utf8'));
for (const spec of specs) {
  const svg = chart(spec);
  writeFileSync(`${outDir}/${spec.file}`, svg);
  console.log(`wrote ${spec.file} (${spec.bars.length} bars × ${(spec.series || [1]).length})`);
}
