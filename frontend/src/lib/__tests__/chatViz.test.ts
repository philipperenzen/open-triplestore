// Result-shape inference behind Spark's visual result widgets: which visuals a
// result's rows can support, and that every builder draws only what the rows
// returned.
import { describe, it, expect } from 'vitest';
import {
  inferViews,
  resolveView,
  buildChart,
  buildStats,
  mapElementsFromTable,
  graphFromTable,
  graphFromBindings,
  tableFromBindings,
  tableFromTripleBindings,
  shaclFromTable,
  shaclFromReport,
  summarizeShacl,
  temporalValue,
  severityName,
  MAX_BARS,
  type VizTable,
} from '../chatViz';

const SH = 'http://www.w3.org/ns/shacl#';
const RDF_TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';

const perGraph: VizTable = {
  columns: ['g', 'n'],
  rows: [
    ['https://ex.org/graph/a', '120'],
    ['https://ex.org/graph/b', '4500'],
    ['https://ex.org/graph/c', '37'],
  ],
};

const perYear: VizTable = {
  columns: ['year', 'count'],
  rows: [
    ['2021', '5'],
    ['2019', '9'],
    ['2020', '2'],
  ],
};

const places: VizTable = {
  columns: ['el', 'label', 'wkt'],
  rows: [
    ['https://ex.org/b1', 'Bridge', 'POINT(4.9 52.37)'],
    ['https://ex.org/b2', 'Tower', '<http://www.opengis.net/def/crs/OGC/1.3/CRS84> POINT(5.1 52.1)'],
    ['https://ex.org/b3', 'Projected', '<http://www.opengis.net/def/crs/EPSG/0/28992> POINT(121000 487000)'],
  ],
};

const triples: VizTable = {
  columns: ['subject', 'predicate', 'object'],
  rows: [
    ['<https://ex.org/a>', '<http://www.w3.org/2000/01/rdf-schema#label>', 'Alpha'],
    ['<https://ex.org/a>', '<https://ex.org/knows>', 'https://ex.org/b'],
    ['<https://ex.org/b>', `<${RDF_TYPE}>`, 'https://ex.org/Person'],
  ],
};

describe('temporalValue', () => {
  it('reads ISO dates, dateTimes and year-months', () => {
    expect(temporalValue('2024-03-01')).toBe(Date.UTC(2024, 2, 1));
    expect(temporalValue('2024-03')).toBe(Date.UTC(2024, 2, 1));
    expect(temporalValue('2024-03-01T10:00:00Z')).toBe(Date.UTC(2024, 2, 1, 10));
  });
  it('treats a bare year as time only in a date-ish column', () => {
    expect(temporalValue('2020', 'year')).toBe(Date.UTC(2020, 0, 1));
    expect(temporalValue('2020', 'bouwjaar')).toBe(Date.UTC(2020, 0, 1));
    expect(temporalValue('2020', 'count')).toBeNull();
  });
  it('rejects non-dates', () => {
    expect(temporalValue('Bridge')).toBeNull();
    expect(temporalValue('')).toBeNull();
  });
});

describe('inferViews', () => {
  it('offers a bar chart for label + aggregate rows', () => {
    expect(inferViews(perGraph)).toEqual({ views: ['bar', 'table'], best: 'bar' });
  });
  it('prefers a timeline when the label column is time', () => {
    const { views, best } = inferViews(perYear);
    expect(best).toBe('line');
    expect(views).toContain('bar');
  });
  it('offers a map for WGS84 WKT rows', () => {
    expect(inferViews(places).best).toBe('map');
  });
  it('offers a node-link graph for triples', () => {
    expect(inferViews(triples)).toEqual({ views: ['graph', 'table'], best: 'graph' });
  });
  it('offers KPI tiles for a single aggregate row', () => {
    expect(inferViews({ columns: ['count', 'oldest'], rows: [['42', '1890']] }).best).toBe('stat');
  });
  it('falls back to the table when nothing fits', () => {
    expect(inferViews({ columns: ['s', 'label'], rows: [['https://ex.org/a', 'A'], ['https://ex.org/b', 'B']] })).toEqual({ views: ['table'], best: 'table' });
    expect(inferViews({ columns: ['result'], rows: [['true']] }).best).toBe('table');
    expect(inferViews({ columns: [], rows: [] }).best).toBe('table');
    expect(inferViews(null).best).toBe('table');
  });
  it('needs two rows before charting', () => {
    expect(inferViews({ columns: ['g', 'n'], rows: [['a', '1']] }).views).not.toContain('bar');
  });
});

describe('resolveView', () => {
  it('auto picks the best fit', () => {
    expect(resolveView(perGraph, 'auto')).toMatchObject({ view: 'bar', fellBack: null });
  });
  it('honours a request the rows support, aliases included', () => {
    expect(resolveView(perYear, 'bar').view).toBe('bar');
    expect(resolveView(perYear, 'chart').view).toBe('line');
    expect(resolveView(perGraph, 'chart').view).toBe('bar');
  });
  it('shows the table, and says so, when the request does not fit', () => {
    expect(resolveView(perGraph, 'map')).toMatchObject({ view: 'table', fellBack: 'map' });
    expect(resolveView(perGraph, 'table')).toMatchObject({ view: 'table', fellBack: null });
  });
  it('treats an unknown request as auto', () => {
    expect(resolveView(perGraph, 'sunburst')).toMatchObject({ view: 'bar', fellBack: null });
  });
});

describe('buildChart', () => {
  it('sorts bars by value and labels the axes with the column names', () => {
    const c = buildChart(perGraph, 'bar')!;
    expect(c.spec.series[0].data.map((d) => d.value)).toEqual([4500, 120, 37]);
    expect(c.spec.xLabel).toBe('g');
    expect(c.spec.yLabel).toBe('n');
  });
  it('caps bars at the top N and reports how many were cut', () => {
    const rows = Array.from({ length: 40 }, (_, i) => [`item${i}`, String(i)]);
    const c = buildChart({ columns: ['item', 'n'], rows }, 'bar')!;
    expect(c.shown).toBe(MAX_BARS);
    expect(c.total).toBe(40);
    expect(c.spec.series[0].data[0]).toEqual({ label: 'item39', value: 39 });
  });
  it('keeps a timeline chronological, not sorted by value', () => {
    const c = buildChart(perYear, 'line')!;
    expect(c.spec.labels).toEqual(['2019', '2020', '2021']);
    expect(c.spec.series[0].data.map((d) => d.value)).toEqual([9, 2, 5]);
  });
  it('splits a timeline into series by a small categorical column', () => {
    const c = buildChart(
      {
        columns: ['month', 'type', 'n'],
        rows: [
          ['2024-02', 'Bridge', '3'],
          ['2024-01', 'Tunnel', '1'],
          ['2024-01', 'Bridge', '2'],
        ],
      },
      'line',
    )!;
    expect(c.spec.labels).toEqual(['2024-01', '2024-02']);
    expect(c.spec.series.map((s) => s.name).sort()).toEqual(['Bridge', 'Tunnel']);
    // The tunnel series has no February point — it is not filled in.
    expect(c.spec.series.find((s) => s.name === 'Tunnel')!.data).toEqual([{ label: '2024-01', value: 1 }]);
  });
  it('skips rows with an empty or non-numeric value instead of charting 0', () => {
    const c = buildChart({ columns: ['g', 'n'], rows: [['a', '3'], ['b', ''], ['c', '7']] }, 'bar')!;
    expect(c.spec.series[0].data.map((d) => d.label)).toEqual(['c', 'a']);
  });
  it('honours x/y hints that name real columns', () => {
    const t: VizTable = { columns: ['g', 'triples', 'subjects'], rows: [['a', '10', '2'], ['b', '5', '9']] };
    expect(buildChart(t, 'bar', { y: 'triples' })!.spec.yLabel).toBe('triples');
    expect(buildChart(t, 'bar', { y: '?nope' })!.spec.yLabel).toBe('subjects');
  });
});

describe('buildStats', () => {
  it('returns the single row exactly as returned', () => {
    expect(buildStats({ columns: ['count', 'oldest', 'name'], rows: [['42', '1890', 'Bridge']] })).toEqual([
      { label: 'count', value: '42', numeric: true },
      { label: 'oldest', value: '1890', numeric: true },
      { label: 'name', value: 'Bridge', numeric: false },
    ]);
  });
  it('returns nothing for multi-row results', () => {
    expect(buildStats(perGraph)).toEqual([]);
  });
});

describe('mapElementsFromTable', () => {
  it('maps WGS84 WKT rows and skips projected ones', () => {
    const els = mapElementsFromTable(places);
    expect(els.map((e) => e.id)).toEqual(['https://ex.org/b1', 'https://ex.org/b2']);
    expect(els[0]).toMatchObject({ label: 'Bridge', wkt4326: 'POINT(4.9 52.37)' });
  });
  it('builds points from a lat/lon column pair', () => {
    const els = mapElementsFromTable({ columns: ['stop', 'lat', 'lon'], rows: [['https://ex.org/s', '52.1', '5.2'], ['https://ex.org/t', '', '5.0']] });
    expect(els).toHaveLength(1);
    expect(els[0].wkt4326).toBe('POINT(5.2 52.1)');
  });
});

describe('graph elements', () => {
  it('builds nodes and edges from a triple table, keeping literals as leaves', () => {
    const { nodes, edges } = graphFromTable(triples);
    const ids = nodes.map((n) => n.data.id);
    expect(ids).toContain('https://ex.org/a');
    expect(ids).toContain('https://ex.org/b');
    expect(nodes.find((n) => n.data.nodeType === 'literal')?.data.label).toContain('Alpha');
    expect(edges).toHaveLength(3);
    expect(nodes.find((n) => n.data.id === 'https://ex.org/b')?.data.rdfType).toBeTruthy();
  });
  it('builds from typed bindings and round-trips them into a table', () => {
    const bindings = [
      { s: { type: 'uri', value: 'https://ex.org/a' }, p: { type: 'uri', value: 'https://ex.org/p' }, o: { type: 'bnode', value: 'b0' } },
    ];
    expect(graphFromBindings(bindings).nodes.map((n) => n.data.nodeType)).toEqual(['uri', 'bnode']);
    expect(tableFromTripleBindings(bindings).rows).toEqual([['https://ex.org/a', 'https://ex.org/p', '_:b0']]);
  });
  it('converts SELECT bindings to string rows, unbound cells empty', () => {
    expect(tableFromBindings(['a', 'b'], [{ a: { type: 'literal', value: '1' } }])).toEqual({ columns: ['a', 'b'], rows: [['1', '']] });
  });
});

describe('SHACL findings', () => {
  const reportTriples: VizTable = {
    columns: ['subject', 'predicate', 'object'],
    rows: [
      ['_:r', `<${RDF_TYPE}>`, `${SH}ValidationReport`],
      ['_:r', `<${SH}conforms>`, 'false'],
      ['_:v1', `<${SH}focusNode>`, 'https://ex.org/b1'],
      ['_:v1', `<${SH}resultSeverity>`, `${SH}Violation`],
      ['_:v1', `<${SH}resultPath>`, 'https://ex.org/height'],
      ['_:v1', `<${SH}resultMessage>`, 'Missing height'],
      ['_:v2', `<${SH}focusNode>`, 'https://ex.org/b2'],
      ['_:v2', `<${SH}resultSeverity>`, `${SH}Warning`],
      ['_:v2', `<${SH}resultPath>`, 'https://ex.org/height'],
    ],
  };

  it('reads a report queried as triples', () => {
    const f = shaclFromTable(reportTriples)!;
    expect(f.conforms).toBe(false);
    expect(f.issues).toHaveLength(2);
    expect(f.issues[0]).toMatchObject({ focus: 'https://ex.org/b1', severity: 'Violation', message: 'Missing height' });
    expect(inferViews(reportTriples).best).toBe('shacl');
  });
  it('reads a conforming report with no results', () => {
    const f = shaclFromTable({
      columns: ['s', 'p', 'o'],
      rows: [['_:r', `<${RDF_TYPE}>`, `${SH}ValidationReport`], ['_:r', `<${SH}conforms>`, 'true']],
    })!;
    expect(f).toEqual({ conforms: true, issues: [] });
  });
  it('reads a SELECT over validation results by column name', () => {
    const f = shaclFromTable({
      columns: ['focusNode', 'severity', 'message'],
      rows: [['https://ex.org/x', 'sh:Violation', 'Too long']],
    })!;
    expect(f.conforms).toBeNull();
    expect(f.issues[0]).toMatchObject({ severity: 'Violation', message: 'Too long' });
  });
  it('does not mistake ordinary data for a report', () => {
    expect(shaclFromTable(triples)).toBeNull();
    expect(shaclFromTable(perGraph)).toBeNull();
  });
  it('reads the platform JSON report', () => {
    const f = shaclFromReport({ conforms: false, results_count: 1, results: [{ severity: 'Violation', focus_node: 'https://ex.org/a', path: '<https://ex.org/p>', message: 'm' }] })!;
    expect(f.issues[0]).toMatchObject({ focus: 'https://ex.org/a', severity: 'Violation' });
    expect(shaclFromReport(null)).toBeNull();
  });
  it('summarises counts with violations first', () => {
    const s = summarizeShacl(shaclFromTable(reportTriples)!);
    expect(s.total).toBe(2);
    expect(s.bySeverity).toEqual([{ key: 'Violation', count: 1 }, { key: 'Warning', count: 1 }]);
    expect(s.byPath).toEqual([{ key: 'https://ex.org/height', count: 2 }]);
    expect(s.samples[0].severity).toBe('Violation');
  });
  it('names severities by their local name', () => {
    expect(severityName(`<${SH}Info>`)).toBe('Info');
    expect(severityName('sh:Warning')).toBe('Warning');
  });
});
