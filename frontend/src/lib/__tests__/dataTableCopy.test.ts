/**
 * Copying IRIs out of the triple table.
 *
 * The report was "in the table view i cannot copy iris for predicates sub or
 * obj": the subject/object copy button existed but sat at opacity 0 inside a
 * `td` with `overflow: hidden` and `tabindex="-1"`, and the predicate and graph
 * cells had no copy control at all. The visible text is the prefixed CURIE, so
 * hand-selecting a cell never yields the IRI either. These tests drive the real
 * component, because the defect was entirely in what the markup offers a user.
 */
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';

// The implementations name their argument so the recorded calls are typed
// `[string]`; an argument-less `vi.fn` records `[]`, and reading `calls[0][0]`
// is then a type error rather than the assertion it looks like.
const clipboard = vi.hoisted(() => ({
  copyOrWarn: vi.fn(async (_text: string) => true),
  copyToClipboard: vi.fn(async (_text: string) => true),
}));
vi.mock('../clipboard.js', () => clipboard);

const router = vi.hoisted(() => ({ navigate: vi.fn() }));
vi.mock('../router/index.js', () => router);

import DataTable from '../../components/DataTable.svelte';

const SUBJECT = 'https://data.example/reasoning/Heart';
const TYPE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';
const OBJECT = 'https://data.example/reasoning/Organ';
const LABEL = 'http://www.w3.org/2000/01/rdf-schema#label';
const GRAPH = 'https://data.example/graph/instances';

const triples = [
  {
    subject: { type: 'uri', value: SUBJECT },
    predicate: { type: 'uri', value: TYPE },
    object: { type: 'uri', value: OBJECT },
    graph: { type: 'uri', value: GRAPH },
  },
  {
    subject: { type: 'uri', value: SUBJECT },
    predicate: { type: 'uri', value: LABEL },
    object: { type: 'literal', value: 'Heart' },
    graph: { type: 'uri', value: GRAPH },
  },
];

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  clipboard.copyOrWarn.mockClear();
  clipboard.copyToClipboard.mockClear();
  router.navigate.mockClear();
});

afterEach(() => cleanup());

function rows(container: HTMLElement) {
  return [...container.querySelectorAll('tr.triple-row')] as HTMLElement[];
}

/** The copy controls of one row, in column order, with their accessible names. */
function copyControls(row: HTMLElement) {
  return [...row.querySelectorAll('button.copy-iri')] as HTMLButtonElement[];
}

describe('DataTable triple copy controls', () => {
  it('offers a copy control in each of the four term columns', () => {
    const { container } = render(DataTable, { mode: 'triples', triples });
    const controls = copyControls(rows(container)[0]);

    expect(controls).toHaveLength(4);
    expect(controls.map((b) => b.getAttribute('aria-label'))).toEqual([
      'Copy IRI',
      'Copy IRI',
      'Copy IRI',
      'Copy IRI',
    ]);
  });

  it('copies the full IRI, not the prefixed form shown in the cell', async () => {
    const { container } = render(DataTable, { mode: 'triples', triples });
    const controls = copyControls(rows(container)[0]);

    for (const button of controls) await fireEvent.click(button);

    expect(clipboard.copyOrWarn.mock.calls.map((c) => c[0])).toEqual([
      SUBJECT,
      TYPE,
      OBJECT,
      GRAPH,
    ]);
  });

  it('copies a literal object by value, not as an IRI', async () => {
    const { container } = render(DataTable, { mode: 'triples', triples });
    // Subject, predicate, object, graph — the third is the literal object.
    const [, , literal] = copyControls(rows(container)[1]);

    expect(literal.getAttribute('aria-label')).toBe('Copy value');

    await fireEvent.click(literal);
    expect(clipboard.copyOrWarn).toHaveBeenCalledWith('Heart');
    expect(clipboard.copyOrWarn).not.toHaveBeenCalledWith(LABEL);
  });

  it('keeps the controls reachable — visible and in the tab order', () => {
    const { container } = render(DataTable, { mode: 'triples', triples });

    for (const button of copyControls(rows(container)[0])) {
      expect(button.getAttribute('tabindex')).not.toBe('-1');
      expect(button.hasAttribute('hidden')).toBe(false);
    }
  });

  it('copying does not navigate away from the table', async () => {
    const { container } = render(DataTable, { mode: 'triples', triples });
    await fireEvent.click(copyControls(rows(container)[0])[0]);

    expect(router.navigate).not.toHaveBeenCalled();
  });

  it('shows no copy control for the default graph, which has no IRI', () => {
    const defaultGraph = [{ ...triples[0], graph: undefined }];
    const { container } = render(DataTable, { mode: 'triples', triples: defaultGraph });

    expect(copyControls(rows(container)[0])).toHaveLength(3);
  });

  it('marks the copy as done so the click is not silent', async () => {
    const { container } = render(DataTable, { mode: 'triples', triples });
    const button = copyControls(rows(container)[0])[1];

    await fireEvent.click(button);
    await waitFor(() => expect(button.classList.contains('copied')).toBe(true));
  });
});
