/**
 * ShapesEditor on large shape graphs: one parse per change, the source editor
 * only when asked for, and a dirty flag that does not wait for the Turtle.
 *
 * The editor used to parse the same text twice on the main thread (once for
 * the toolbar count, once in the builder) and to create CodeMirror with the
 * whole document up front, which on a 20 MB graph kept the page from ever
 * becoming usable.
 */
import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { render, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { generateLargeShapesGraph } from './fixtures/largeShapesGraph';

// Count real parses: the mock passes every call through to the parser.
const parseSpy = vi.hoisted(() => ({ fn: null as null | ((...a: unknown[]) => unknown) }));
vi.mock('../shaclModel.ts', async (importOriginal) => {
  const mod = await importOriginal<typeof import('../shaclModel.ts')>();
  const fn = vi.fn(mod.parseShapesGraph);
  parseSpy.fn = fn;
  return { ...mod, parseShapesGraph: fn };
});

const getShapeGraphTurtle = vi.fn();
const putShapeGraphTurtle = vi.fn();
vi.mock('../api.js', () => ({
  getShapes: vi.fn(),
  putShapes: vi.fn(),
  inferDataset: vi.fn(),
  getModelContext: vi.fn(),
  getShapeGraphTurtle: (...a: unknown[]) => getShapeGraphTurtle(...a),
  putShapeGraphTurtle: (...a: unknown[]) => putShapeGraphTurtle(...a),
}));
vi.mock('../../components/SparqlEditorCM.svelte', async () => ({
  default: (await import('./fixtures/EditorProbe.svelte')).default,
}));
vi.mock('../../components/AiAssistPanel.svelte', async () => ({
  default: (await import('./fixtures/NullComponent.svelte')).default,
}));

const ShapesEditor = (await import('../../components/ShapesEditor.svelte')).default;
const parseCalls = () => (parseSpy.fn as unknown as { mock: { calls: unknown[][] } }).mock.calls;
const parsesOf = (ttl: string) => parseCalls().filter((c) => c[0] === ttl).length;

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  (parseSpy.fn as unknown as { mockClear: () => void }).mockClear();
  getShapeGraphTurtle.mockReset();
  putShapeGraphTurtle.mockReset();
  putShapeGraphTurtle.mockResolvedValue({ version: 2 });
});

const SMALL = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .

ex:BridgeShape a sh:NodeShape ;
  sh:targetClass ex:Bridge ;
  sh:property [ sh:path ex:name ] .
`;

const sectionLabel = (c: HTMLElement) => c.querySelector('.section-label')?.textContent?.replace(/\s+/g, ' ').trim();
const sourceButton = (c: HTMLElement) => [...c.querySelectorAll<HTMLButtonElement>('.vt')].find((b) => b.textContent?.includes('Source'))!;

describe('ShapesEditor — one parse per change', () => {
  it('parses the loaded text once and takes the count from the builder', async () => {
    getShapeGraphTurtle.mockResolvedValue(SMALL);
    const { container } = render(ShapesEditor, { shapeGraphId: 'sg-1' });
    await waitFor(() => expect(container.querySelector('article.shape')).not.toBeNull());
    expect(parsesOf(SMALL)).toBe(1);
    expect(sectionLabel(container)).toBe('Shapes (1)');

    // An edit in the builder writes new text, which is not parsed again: the
    // builder already holds the model it came from.
    const before = parseCalls().length;
    const input = container.querySelector<HTMLInputElement>('.name-input')!;
    input.value = 'http://example.org/OtherShape';
    await fireEvent.change(input);
    expect(parseCalls().length).toBe(before);
    expect(container.querySelector('.dirty-chip')).not.toBeNull();
  });

  it('reports invalid Turtle from the same parse', async () => {
    const bad = '@prefix ex: <http://example.org/> .\nex:a ex:b';
    getShapeGraphTurtle.mockResolvedValue(bad);
    const { container } = render(ShapesEditor, { shapeGraphId: 'sg-1' });
    await waitFor(() => expect(container.querySelector('.parse-bad')).not.toBeNull());
    expect(parsesOf(bad)).toBe(1);
  });
});

describe('ShapesEditor — source view', () => {
  it('creates the source editor only once the source view is opened', async () => {
    getShapeGraphTurtle.mockResolvedValue(SMALL);
    const { container, queryByTestId } = render(ShapesEditor, { shapeGraphId: 'sg-1' });
    await waitFor(() => expect(container.querySelector('article.shape')).not.toBeNull());
    expect(queryByTestId('editor-probe')).toBeNull();
    await fireEvent.click(sourceButton(container));
    expect(queryByTestId('editor-probe')?.dataset.large).toBe('false');
  });

  it('asks before opening a very large document, then opens it in large mode', async () => {
    const big = generateLargeShapesGraph(1500).turtle; // about 8.5 MB
    expect(big.length).toBeGreaterThan(8 * 1024 * 1024);
    getShapeGraphTurtle.mockResolvedValue(big);
    const { container, queryByTestId, getByText } = render(ShapesEditor, { shapeGraphId: 'sg-1' });
    await waitFor(() => expect(container.querySelector('article.shape')).not.toBeNull(), { timeout: 30000 });
    expect(sectionLabel(container)).toBe('Shapes (1500)');
    expect(parsesOf(big)).toBe(1);

    await fireEvent.click(sourceButton(container));
    expect(queryByTestId('editor-probe')).toBeNull();
    expect(container.querySelector('.source-notice')?.textContent).toContain('MB');
    await fireEvent.click(getByText('Open source anyway'));
    expect(queryByTestId('editor-probe')?.dataset.large).toBe('true');
  }, 60000);
});

describe('ShapesEditor — deferred builder writes', () => {
  it('shows unsaved changes at once and saves the edit even before its pause is over', async () => {
    const big = generateLargeShapesGraph(200).turtle; // over the half-megabyte line
    getShapeGraphTurtle.mockResolvedValue(big);
    const { container } = render(ShapesEditor, { shapeGraphId: 'sg-1' });
    await waitFor(() => expect(container.querySelector('article.shape')).not.toBeNull(), { timeout: 30000 });

    const input = container.querySelector<HTMLInputElement>('.name-input')!;
    input.value = 'http://example.org/shape/Renamed';
    await fireEvent.change(input);
    expect(container.querySelector('.dirty-chip')).not.toBeNull();

    // Save straight away: the pending edit has to be in what is sent.
    const save = container.querySelector<HTMLButtonElement>('.save-btn')!;
    await fireEvent.click(save);
    await waitFor(() => expect(putShapeGraphTurtle).toHaveBeenCalledTimes(1));
    const sent = putShapeGraphTurtle.mock.calls[0][1] as string;
    expect(sent).toContain('http://example.org/shape/Renamed');
    await waitFor(() => expect(container.querySelector('.dirty-chip')).toBeNull());
  }, 60000);
});
