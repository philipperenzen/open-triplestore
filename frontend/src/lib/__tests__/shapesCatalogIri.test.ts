/**
 * ShapesCatalog IRI rendering.
 *
 * The catalog carried its own `shortIRI()` that kept only the last path segment,
 * so every shape, target class and path lost its namespace: two graphs both
 * ending in `/shapes` were indistinguishable, and `sh:NodeShape` read as plain
 * "NodeShape". It also offered no way back to the full IRI on the chips.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/svelte';
import ShapesCatalog from '../../components/ShapesCatalog.svelte';

const GRAPH = 'https://data.example/shapes/building';

const summary = {
  graphs: [
    { graph: GRAPH, node_count: 1, property_count: 1, total: 2, registered: false },
  ],
};
const shapes = {
  shapes: [
    {
      shape: 'https://data.example/shapes/building#WallShape',
      kind: 'node',
      target_classes: ['http://www.w3.org/ns/shacl#NodeShape'],
      path: 'http://xmlns.com/foaf/0.1/name',
    },
  ],
};

vi.mock('../api.js', () => ({
  listShapesCatalog: (graph?: string) => Promise.resolve(graph ? shapes : summary),
  listShapeGraphs: () => Promise.resolve([]),
  createShapeGraph: vi.fn(),
  importShapesIntoGraph: vi.fn(),
  registerShapeGraph: vi.fn(),
  getShapeGraphTurtle: vi.fn(),
}));

/** Render, then open the single source graph so its shapes load. */
async function openGraph() {
  const { container } = render(ShapesCatalog, {});
  await vi.waitFor(() => expect(container.querySelector('.grp-iri')).toBeTruthy());
  await fireEvent.click(container.querySelector('.expander')!);
  await vi.waitFor(() => expect(container.querySelector('.shape-row')).toBeTruthy());
  return container;
}

describe('ShapesCatalog', () => {
  it('shows the source graph as a CURIE, with the full IRI in its tooltip', async () => {
    const container = await openGraph();
    const iri = container.querySelector('.grp-iri')!;
    expect(iri.textContent?.trim()).toBe('shapes:building');
    expect(iri.getAttribute('title')).toBe(GRAPH);
  });

  it('keeps the namespace on the shape name', async () => {
    const container = await openGraph();
    const name = container.querySelector('.shape-name')!;
    expect(name.textContent?.trim()).toBe('building:WallShape');
    expect(name.getAttribute('title')).toBe('https://data.example/shapes/building#WallShape');
  });

  it('shows target-class and path chips as CURIEs and keeps the full IRI reachable', async () => {
    const container = await openGraph();
    const target = container.querySelector('.chip-target')!;
    expect(target.textContent?.trim()).toBe('sh:NodeShape');
    expect(target.getAttribute('title')).toBe('http://www.w3.org/ns/shacl#NodeShape');

    const path = container.querySelector('.chip-path')!;
    expect(path.textContent?.trim()).toBe('foaf:name');
    expect(path.getAttribute('title')).toBe('http://xmlns.com/foaf/0.1/name');
  });
});
