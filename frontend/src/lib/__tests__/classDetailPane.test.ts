// Selecting a class in the model viewer's hierarchy shows its attributes,
// relations, shape constraints and instances. The instances held in data the
// caller can read are looked up lazily over /sparql, once per selection, and a
// slow answer for an earlier selection must never overwrite the current one.
import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { render, fireEvent, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { init, addMessages } from 'svelte-i18n';
import { Parser, Store } from 'n3';
import en from '../i18n/en.json';

vi.mock('../api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api')>()),
  sparqlQuery: vi.fn(),
}));

import { sparqlQuery } from '../api';
import OntologyModelViewer from '../../components/OntologyModelViewer.svelte';

const EX = 'http://example.org/ns#';
const mockedQuery = vi.mocked(sparqlQuery);

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  mockedQuery.mockReset();
});

function store(): Store {
  return new Store(new Parser().parse(`
    @prefix ex:   <${EX}> .
    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
    @prefix owl:  <http://www.w3.org/2002/07/owl#> .
    @prefix sh:   <http://www.w3.org/ns/shacl#> .
    @prefix xsd:  <http://www.w3.org/2001/XMLSchema#> .
    ex:Bridge a owl:Class ; rdfs:label "Bridge"@en .
    ex:River a owl:Class ; rdfs:label "River"@en .
    ex:span a owl:DatatypeProperty ; rdfs:label "span"@en ; rdfs:domain ex:Bridge ; rdfs:range xsd:decimal .
    ex:crosses a owl:ObjectProperty ; rdfs:label "crosses"@en ; rdfs:domain ex:Bridge ; rdfs:range ex:River .
    ex:BridgeShape a sh:NodeShape ; sh:targetClass ex:Bridge ;
      sh:property [ sh:path ex:code ; sh:datatype xsd:string ; sh:minCount 1 ] .
    ex:b1 a ex:Bridge ; rdfs:label "First bridge"@en .
  `));
}

type Binding = Record<string, { value: string }>;
const answer = (n: number, subjects: [string, string][] = []) => ({
  results: {
    bindings: subjects.length
      ? subjects.map(([s, label]): Binding => ({ n: { value: String(n) }, s: { value: s }, label: { value: label } }))
      : [{ n: { value: String(n) } }],
  },
});

/** A promise plus the function that settles it, to control response order. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

async function flush() {
  for (let i = 0; i < 4; i++) { await Promise.resolve(); await tick(); }
}

async function mount() {
  const r = render(OntologyModelViewer, { graphIri: '', subGraphs: [], preloadedStore: store() });
  await tick();
  return r;
}

async function selectInTree(container: HTMLElement, label: string) {
  const btn = [...container.querySelectorAll<HTMLButtonElement>('.cls-btn')]
    .find((b) => b.querySelector('.cls-label')?.textContent === label)!;
  await fireEvent.click(btn);
  await flush();
}

const detail = (container: HTMLElement) => container.querySelector<HTMLElement>('.cap-detail')!;
const section = (container: HTMLElement, heading: string) =>
  [...detail(container).querySelectorAll<HTMLElement>('section')]
    .find((s) => s.querySelector('h4')?.textContent?.trim().startsWith(heading)) ?? null;

describe('class detail pane', () => {
  it('shows attributes, relations, shapes and model instances for the selected class', async () => {
    mockedQuery.mockResolvedValue(answer(0));
    const { container } = await mount();
    await selectInTree(container, 'Bridge');

    const attrs = section(container, 'Attributes')!;
    const attrRows = [...attrs.querySelectorAll('tbody tr')].map((tr) => tr.getAttribute('data-prop'));
    // Declared ex:span and shape-only ex:code, each once.
    expect(attrRows.sort()).toEqual([EX + 'code', EX + 'span']);
    expect(within(attrs).getByText(/shape: /)).toBeTruthy();

    const rels = section(container, 'Relations')!;
    expect(rels.querySelector(`tr[data-prop="${EX}crosses"]`)).toBeTruthy();
    expect(section(container, 'Constraints from shapes')).toBeTruthy();

    const inst = section(container, 'Instances')!;
    expect(within(inst).getByText('First bridge')).toBeTruthy();
    expect(within(inst).getByText('No instances in data you can access.')).toBeTruthy();
  });

  it('omits empty sections but always shows the instances section', async () => {
    mockedQuery.mockResolvedValue(answer(0));
    const { container } = await mount();
    await selectInTree(container, 'River');

    expect(section(container, 'Attributes')).toBeNull();
    expect(section(container, 'Constraints from shapes')).toBeNull();
    // River is the target of ex:crosses, so it has an incoming relation.
    expect(section(container, 'Relations')!.textContent).toContain('Incoming');
    const inst = section(container, 'Instances')!;
    expect(within(inst).getByText('No instances in this model.')).toBeTruthy();
  });

  it('selects a related class in the tree when it is clicked', async () => {
    mockedQuery.mockResolvedValue(answer(0));
    const { container } = await mount();
    await selectInTree(container, 'Bridge');

    const rels = section(container, 'Relations')!;
    await fireEvent.click(within(rels).getByRole('button', { name: 'Select class River' }));
    await flush();

    expect(detail(container).querySelector('.title')?.textContent).toBe('River');
    expect(container.querySelector('.row.active .cls-label')?.textContent).toBe('River');
  });

  it('opens a declared property in the properties tab and a shape in the SHACL tab', async () => {
    mockedQuery.mockResolvedValue(answer(0));
    const { container } = await mount();
    await selectInTree(container, 'Bridge');

    await fireEvent.click(within(section(container, 'Relations')!).getByRole('button', { name: 'crosses' }));
    await flush();
    expect(container.querySelector('.omv-tab.active')?.textContent).toContain('Properties');
    expect(container.querySelector('.pp-table tr.active')?.getAttribute('data-prop-iri')).toBe(EX + 'crosses');

    // Back to the class, then to its shape.
    await fireEvent.click([...container.querySelectorAll<HTMLButtonElement>('.omv-tab')][0]);
    await flush();
    await selectInTree(container, 'Bridge');
    await fireEvent.click(within(section(container, 'Constraints from shapes')!).getByRole('button', { name: /BridgeShape$/ }));
    await flush();
    expect(container.querySelector('.omv-tab.active')?.textContent).toContain('SHACL');
    expect(container.querySelector('.shape-card.focused')?.getAttribute('data-shape-iri')).toBe(EX + 'BridgeShape');
  });

  it('looks up data instances once per selection and ignores a stale answer', async () => {
    const first = deferred<unknown>();
    const second = deferred<unknown>();
    mockedQuery.mockReturnValueOnce(first.promise as never).mockReturnValueOnce(second.promise as never);
    const { container } = await mount();

    await selectInTree(container, 'Bridge');
    expect(mockedQuery).toHaveBeenCalledTimes(1);
    expect(String(mockedQuery.mock.calls[0][0])).toContain(`<${EX}Bridge>`);
    expect(within(section(container, 'Instances')!).getByText('Looking up instances…')).toBeTruthy();

    await selectInTree(container, 'River');
    expect(mockedQuery).toHaveBeenCalledTimes(2);
    expect(String(mockedQuery.mock.calls[1][0])).toContain(`<${EX}River>`);
    // The earlier request was cancelled.
    expect((mockedQuery.mock.calls[0][1] as RequestInit).signal?.aborted).toBe(true);

    second.resolve(answer(1, [['http://example.org/data/r1', 'Fresh river']]));
    await flush();
    first.resolve(answer(7, [['http://example.org/data/b9', 'Stale bridge']]));
    await flush();

    const inst = section(container, 'Instances')!;
    expect(within(inst).getByText('Fresh river')).toBeTruthy();
    expect(within(inst).queryByText('Stale bridge')).toBeNull();
    expect(within(inst).getByText('Showing 1 of 1')).toBeTruthy();
    const link = within(inst).getByText('Fresh river').closest('a')!;
    expect(link.getAttribute('href')).toContain('/resource?iri=' + encodeURIComponent('http://example.org/data/r1'));
    expect(mockedQuery).toHaveBeenCalledTimes(2);
  });

  it('shows a quiet message when the lookup fails, without hiding the rest', async () => {
    mockedQuery.mockRejectedValue(new Error('boom'));
    const { container } = await mount();
    await selectInTree(container, 'Bridge');

    const inst = section(container, 'Instances')!;
    expect(within(inst).getByText('Could not load instances.')).toBeTruthy();
    expect(within(inst).getByText('First bridge')).toBeTruthy();
    expect(section(container, 'Attributes')).toBeTruthy();
  });
});
