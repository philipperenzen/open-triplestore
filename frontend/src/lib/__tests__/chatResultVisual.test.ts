// Spark's visual result widgets: ResultVisual's view tabs and table fallback,
// the chart / KPI / SHACL summary renderers, and the lazily-loaded graph and
// map views (GraphCanvas / ViewerMap stubbed — cytoscape and maplibre do not
// run in jsdom; the stub reports what would have been drawn).
import { describe, it, expect, beforeAll, vi } from 'vitest';
import { render, fireEvent, screen } from '@testing-library/svelte';
import { tick } from 'svelte';
import { init, addMessages, locale } from 'svelte-i18n';
import en from '../i18n/en.json';
import nl from '../i18n/nl.json';

vi.mock('../../components/GraphCanvas.svelte', async () => import('./fixtures/PropsProbe.svelte'));
vi.mock('../../components/viewer/ViewerMap.svelte', async () => import('./fixtures/PropsProbe.svelte'));

import ResultVisual from '../../components/chat/ResultVisual.svelte';
import ChatShaclReport from '../../components/chat/ChatShaclReport.svelte';
import ChatStats from '../../components/chat/ChatStats.svelte';
import ChatGraph from '../../components/chat/ChatGraph.svelte';
import ChatResultMap from '../../components/chat/ChatResultMap.svelte';
import { graphFromTable } from '../chatViz';

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  addMessages('nl', nl as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const SH = 'http://www.w3.org/ns/shacl#';
const perGraph = { columns: ['g', 'n'], rows: [['graph-a', '120'], ['graph-b', '4500']] };
const places = { columns: ['el', 'label', 'wkt'], rows: [['https://ex.org/b1', 'Bridge', 'POINT(4.9 52.37)']] };
const triples = {
  columns: ['subject', 'predicate', 'object'],
  rows: [
    ['https://ex.org/a', 'https://ex.org/knows', 'https://ex.org/b'],
    ['https://ex.org/a', 'http://www.w3.org/2000/01/rdf-schema#label', 'Alpha'],
  ],
};

async function settle() {
  // Let the dynamic import + {#await} resolve.
  for (let i = 0; i < 5; i++) {
    await new Promise((r) => setTimeout(r, 0));
    await tick();
  }
}

describe('ResultVisual', () => {
  it('starts on the best visual and offers a tab per supported view', () => {
    const { container } = render(ResultVisual, { table: perGraph, preferred: 'auto' });
    const tabs = [...container.querySelectorAll('[role="tab"]')].map((b) => b.textContent?.trim());
    expect(tabs).toEqual([en.components.chat.views.bar, en.components.chat.views.table]);
    expect(container.querySelector('svg rect')).toBeTruthy();
  });

  it('starts on the table where the result used to be a table, and switches on click', async () => {
    const { container } = render(ResultVisual, { table: perGraph, preferred: 'table' });
    expect(container.querySelector('svg')).toBeNull();
    expect(container.textContent).toContain('graph-b');
    await fireEvent.click(screen.getByRole('tab', { name: en.components.chat.views.bar }));
    expect(container.querySelectorAll('svg rect')).toHaveLength(2);
  });

  it('shows the table with a note when the requested view does not fit the rows', () => {
    const { container } = render(ResultVisual, { table: perGraph, preferred: 'map' });
    const note = container.querySelector('[role="note"]');
    expect(note?.textContent).toContain(en.components.chat.views.map);
    expect(container.querySelector('svg')).toBeNull();
    expect(container.textContent).toContain('graph-a');
  });

  it('draws no tabs for a result only a table fits', () => {
    const { container } = render(ResultVisual, { table: { columns: ['label'], rows: [['A'], ['B']] } });
    expect(container.querySelector('[role="tablist"]')).toBeNull();
  });

  it('hands the map exactly the rows with WGS84 geometry', async () => {
    render(ResultVisual, { table: places, preferred: 'auto' });
    const probe = await screen.findByTestId('probe', {}, { timeout: 5000 });
    expect(JSON.parse(probe.dataset.ids || '[]')).toEqual(['https://ex.org/b1']);
  });

  it('hands the graph the triples as nodes and edges', async () => {
    render(ResultVisual, { table: triples, preferred: 'auto' });
    const probe = await screen.findByTestId('probe', {}, { timeout: 5000 });
    const summary = JSON.parse(probe.dataset.summary || '{}');
    expect(summary).toMatchObject({ nodes: 3, edges: 2 });
  });

  it('renders KPI tiles for a single aggregate row', () => {
    const { container } = render(ResultVisual, { table: { columns: ['count'], rows: [['12345']] } });
    expect(container.querySelector('[data-testid="chat-stats"]')?.textContent).toContain((12345).toLocaleString());
  });

  it('says how many rows a capped chart shows', () => {
    const rows = Array.from({ length: 20 }, (_, i) => [`g${i}`, String(i)]);
    const { container } = render(ResultVisual, { table: { columns: ['g', 'n'], rows }, preferred: 'bar' });
    expect(container.textContent).toContain('Showing 15 of 20 rows.');
  });
});

describe('ChatShaclReport', () => {
  const findings = {
    conforms: false,
    issues: [
      { focus: 'https://ex.org/b1', path: '<https://ex.org/height>', severity: 'Violation', message: 'Missing height', shape: '' },
      { focus: 'https://ex.org/b2', path: '<https://ex.org/height>', severity: 'Warning', message: '', shape: '' },
    ],
  };

  it('states the verdict and counts per severity', () => {
    const { container } = render(ChatShaclReport, { findings });
    expect(container.textContent).toContain(en.components.chat.shacl.doesNotConform);
    expect(container.textContent).toContain('2 validation results');
    const sev = [...container.querySelectorAll('.sev')].map((e) => e.textContent?.replace(/\s+/g, ' ').trim());
    expect(sev).toEqual([`1 ${en.components.chat.shacl.violation}`, `1 ${en.components.chat.shacl.warning}`]);
    expect(container.textContent).toContain('Missing height');
    expect(container.textContent).toContain(en.components.chat.shacl.noMessage);
  });

  it('reports a conforming run', () => {
    const { container } = render(ChatShaclReport, { findings: { conforms: true, issues: [] } });
    expect(container.textContent).toContain(en.components.chat.shacl.conforms);
    expect(container.querySelector('.shacl.ok')).toBeTruthy();
  });

  it('renders in Dutch', async () => {
    locale.set('nl');
    await tick();
    const { container } = render(ChatShaclReport, { findings });
    expect(container.textContent).toContain(nl.components.chat.shacl.doesNotConform);
    expect(container.textContent).toContain('2 validatieresultaten');
    locale.set('en');
    await tick();
  });

  it('is what ResultVisual shows for a report queried as triples', () => {
    const { container } = render(ResultVisual, {
      table: {
        columns: ['s', 'p', 'o'],
        rows: [
          ['_:r', `${SH}conforms`, 'false'],
          ['_:v', `${SH}focusNode`, 'https://ex.org/x'],
          ['_:v', `${SH}resultSeverity`, `${SH}Violation`],
        ],
      },
    });
    expect(container.querySelector('[data-testid="chat-shacl"]')).toBeTruthy();
  });
});

describe('ChatStats', () => {
  it('shows text cells verbatim', () => {
    const { container } = render(ChatStats, { tiles: [{ label: 'oldest', value: 'Stone bridge', numeric: false }] });
    expect(container.textContent).toContain('Stone bridge');
    expect(container.textContent).toContain('oldest');
  });
});

describe('ChatGraph / ChatResultMap', () => {
  it('notes when the graph was drawn from a capped result', async () => {
    const g = graphFromTable(triples);
    const { container } = render(ChatGraph, { nodes: g.nodes, edges: g.edges, total: 50 });
    await settle();
    expect(container.textContent).toContain('3 nodes · 2 edges');
    expect(container.textContent).toContain('of 50');
  });

  it('says so when there is nothing to draw', () => {
    const { container } = render(ChatGraph, { nodes: [], edges: [] });
    expect(container.textContent).toContain(en.components.chat.graphEmpty);
  });

  it('counts the rows the map could not place', async () => {
    const { container } = render(ChatResultMap, { elements: [{ id: 'https://ex.org/b1', wkt4326: 'POINT(4 52)' }], total: 3 });
    await settle();
    expect(container.textContent).toContain('2 rows without WGS84 geometry not shown');
  });
});
