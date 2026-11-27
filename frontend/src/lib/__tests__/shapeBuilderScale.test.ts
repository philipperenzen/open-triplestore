/**
 * ShapeBuilder on large shape graphs.
 *
 * Opening a shape graph of about 20 MB (thousands of node shapes, every blank
 * node a labelled subject, long lists, SPARQL targets) hung the editor page for
 * good: the text was parsed twice on the main thread, every shape card and
 * property row was rendered at once, and every edit re-serialised the whole
 * document. These tests pin the windowed list, the filter, the deferred write
 * and the unchanged behaviour of small graphs.
 */
import { describe, it, expect, beforeAll, vi } from 'vitest';
import { render, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { parseShapesGraph, serializeShapesGraph, type NodeShape } from '../shaclModel.ts';
import { generateLargeShapesGraph } from './fixtures/largeShapesGraph';
import ShapeBuilder from '../../components/ShapeBuilder.svelte';

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

// About 11 MB: well past the off-thread and windowing thresholds.
const BIG = generateLargeShapesGraph(2000);
const BIG_MODEL = parseShapesGraph(BIG.turtle);

const cards = (c: HTMLElement) => c.querySelectorAll('article.shape');
const nameInputs = (c: HTMLElement) =>
  [...c.querySelectorAll<HTMLInputElement>('article.shape .name-input')].map((i) => i.value);

async function renderBig(props: Record<string, unknown> = {}) {
  const r = render(ShapeBuilder, { turtle: BIG.turtle, ...props });
  await waitFor(() => expect(cards(r.container).length).toBeGreaterThan(0), { timeout: 30000 });
  return r;
}

// The filter applies after a short pause; wait for the list to follow.
async function filterFor(container: HTMLElement, text: string, expectIri: string) {
  const input = container.querySelector<HTMLInputElement>('.filter-input')!;
  input.value = text;
  await fireEvent.input(input);
  await waitFor(() => expect(nameInputs(container)).toContain(expectIri));
  expect(container.querySelector('.list-count')?.textContent).toContain('matching');
}

// What a round trip must keep of a shape: blank-node labels are rewritten on
// serialise, so retained quads are compared by count and shape.
function projection(s: NodeShape) {
  const quad = (q: { p: string; o: { termType: string; value: string } }) =>
    `${q.p} ${q.o.termType === 'BlankNode' ? '_' : q.o.value}`;
  return {
    iri: s.iri,
    declared: s.declared,
    targets: s.targets,
    closed: s.closed,
    ignoredProperties: s.ignoredProperties,
    logic: Object.keys(s.logic || {}),
    extra: (s.extraQuads || []).map(quad).sort(),
    properties: s.properties.map((p) => ({ iri: p.iri, path: p.path, c: p.c, name: p.name, message: p.message })),
  };
}

describe('generated large shape graph', () => {
  it('has the structure of the real case', () => {
    expect(BIG.turtle.length).toBeGreaterThan(10_000_000);
    expect(BIG.turtle).not.toMatch(/\bsh:|\[\s/); // `shacl:` prefix, no inline blank nodes
    expect(BIG.turtle).toContain('shacl:target _:b');
    expect(BIG.turtle).not.toContain('targetClass');
    expect(BIG_MODEL.canRoundTrip).toBe(true);
    expect(BIG_MODEL.shapes.length).toBe(BIG.nodeShapes);
  });
});

describe('ShapeBuilder — large graph', () => {
  it('paints a loading state before the parse, then renders one page of shapes', async () => {
    const onParsed = vi.fn();
    const { container } = render(ShapeBuilder, { turtle: BIG.turtle, onParsed });
    // Synchronously after mount nothing has been parsed yet: the loading
    // state is what the browser gets to paint.
    expect(container.textContent).toContain('Reading');
    expect(cards(container).length).toBe(0);
    expect(onParsed).toHaveBeenLastCalledWith({ count: null, parseError: null });

    await waitFor(() => expect(cards(container).length).toBeGreaterThan(0), { timeout: 30000 });
    expect(cards(container).length).toBeLessThanOrEqual(25);
    expect(container.querySelector('.list-count')?.textContent).toContain(`of ${BIG.nodeShapes} shapes`);
    expect(onParsed).toHaveBeenLastCalledWith({ count: BIG.nodeShapes, parseError: null });
  });

  it('"Show more" adds the next page', async () => {
    const { container, getByText } = await renderBig();
    await fireEvent.click(getByText(/Show 25 more/));
    expect(cards(container).length).toBe(50);
  });

  it('the filter finds a shape that is not on the first page, by IRI and by property path', async () => {
    const { container } = await renderBig();
    const target = BIG.shapeIris[1500];
    expect(nameInputs(container)).not.toContain(target);

    await filterFor(container, 'Shape01500', target);
    expect(nameInputs(container)).toEqual([target]);

    // A property path unique to one shape far down the list.
    const counts = new Map<string, number>();
    for (const s of BIG_MODEL.shapes) for (const p of s.properties) if (p.path) counts.set(p.path, (counts.get(p.path) || 0) + 1);
    const owner = BIG_MODEL.shapes.slice(1000).find((s) => s.properties.some((p) => counts.get(p.path) === 1))!;
    const path = owner.properties.find((p) => counts.get(p.path) === 1)!.path;
    await filterFor(container, path.slice(path.lastIndexOf('/') + 1), owner.iri);
    expect(nameInputs(container).length).toBe(1);
  });

  it('an edit to a filtered shape writes a document that still holds every other shape', async () => {
    const onChange = vi.fn();
    const onEdit = vi.fn();
    const { container } = await renderBig({ onChange, onEdit });
    await filterFor(container, 'Shape01500', BIG.shapeIris[1500]);

    const input = container.querySelector<HTMLInputElement>('article.shape .name-input')!;
    input.value = 'http://example.org/shape/Renamed';
    await fireEvent.change(input);

    // The edit is reported at once; the Turtle follows after a pause.
    expect(onEdit).toHaveBeenCalled();
    expect(onChange).not.toHaveBeenCalled();
    await waitFor(() => expect(onChange).toHaveBeenCalledTimes(1), { timeout: 5000 });

    const out = parseShapesGraph(onChange.mock.calls[0][0] as string);
    expect(out.parseError).toBeUndefined();
    expect(out.shapes.length).toBe(BIG.nodeShapes);
    const iris = new Set(out.shapes.map((s) => s.iri));
    expect(iris.has('http://example.org/shape/Renamed')).toBe(true);
    expect(iris.has(BIG.shapeIris[1500])).toBe(false);

    const untouched = BIG.shapeIris[3];
    const before = BIG_MODEL.shapes.find((s) => s.iri === untouched)!;
    const after = out.shapes.find((s) => s.iri === untouched)!;
    expect(projection(after)).toEqual(projection(before));
  }, 60000);

  it('flush() writes a pending edit straight away', async () => {
    const onChange = vi.fn();
    const { container, component } = await renderBig({ onChange });
    const box = container.querySelector<HTMLInputElement>('article.shape .mini-toggle input')!;
    box.checked = !box.checked;
    await fireEvent.change(box);
    expect(onChange).not.toHaveBeenCalled();
    (component as unknown as { flush: () => void }).flush();
    expect(onChange).toHaveBeenCalledTimes(1);
  }, 60000);

  it('"Add shape" shows and focuses the new shape although it is past the page', async () => {
    const onParsed = vi.fn();
    const { container } = await renderBig({ onParsed });
    const add = container.querySelector<HTMLButtonElement>('button.add-shape')!;
    await fireEvent.click(add);
    await waitFor(() => expect(document.activeElement?.classList.contains('name-input')).toBe(true));
    const focused = document.activeElement as HTMLInputElement;
    expect(focused.closest('article.shape')).not.toBeNull();
    expect(container.contains(focused)).toBe(true);
    expect(focused.value).toMatch(/Shape$/);
    expect(cards(container).length).toBe(26);
    expect(onParsed).toHaveBeenLastCalledWith({ count: BIG.nodeShapes + 1, parseError: null });
  }, 60000);

  it('caps a long property list behind "Show all"', async () => {
    // Over 100 shapes keeps the list windowed even though the text is small.
    const props = Array.from({ length: 40 }, (_, i) => `  sh:property [ sh:path ex:p${i} ]`).join(' ;\n');
    const many = Array.from({ length: 110 }, (_, i) => `ex:S${String(i).padStart(3, '0')} a sh:NodeShape ; sh:targetClass ex:C${i} .`).join('\n');
    const ttl = `@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.org/> .\n\nex:A a sh:NodeShape ;\n${props} .\n${many}\n`;
    const { container, getByText } = render(ShapeBuilder, { turtle: ttl });
    const first = container.querySelector('article.shape')!;
    expect(first.querySelectorAll('.prop').length).toBe(20);
    await fireEvent.click(getByText('Show all 40 properties'));
    expect(first.querySelectorAll('.prop').length).toBe(40);
  });
});

describe('ShapeBuilder — small graph unchanged', () => {
  const SMALL = generateLargeShapesGraph(30);

  it('renders every shape, with no filter or paging, and writes each edit at once', async () => {
    expect(SMALL.turtle.length).toBeLessThan(512 * 1024);
    const onChange = vi.fn();
    const { container } = render(ShapeBuilder, { turtle: SMALL.turtle, onChange });
    // Parsed inline: rendered on the first pass, no waiting.
    expect(cards(container).length).toBe(30);
    expect(container.querySelector('.filter-input')).toBeNull();
    expect(container.querySelector('.list-count')).toBeNull();
    expect(container.textContent).not.toContain('Show all');

    const input = container.querySelector<HTMLInputElement>('article.shape .name-input')!;
    input.value = 'http://example.org/shape/Other';
    await fireEvent.change(input);
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(parseShapesGraph(onChange.mock.calls[0][0] as string).shapes.length).toBe(30);
  });
});

describe('parseShapesGraph — duplicate triples', () => {
  it('drops repeated triples, also in buckets of many values', () => {
    const vals = Array.from({ length: 12 }, (_, i) => `ex:n${i}`);
    const ttl = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
ex:S a sh:NodeShape ; sh:targetNode ${vals.join(' , ')} , ex:n3 , ex:n11 .
ex:S sh:targetNode ex:n0 .
ex:S sh:name "S" , "S" .
ex:S ex:note "a"@en , "a"@nl , "a"@en .
`;
    const g = parseShapesGraph(ttl);
    expect(g.shapes[0].targets.map((t) => t.value)).toEqual(vals.map((v) => v.replace('ex:', 'http://example.org/')));
    expect(g.shapes[0].name).toBe('S');
    expect((g.shapes[0].extraQuads || []).length).toBe(2);
    const again = parseShapesGraph(serializeShapesGraph(g));
    expect(again.shapes[0].targets.length).toBe(12);
  });
});
