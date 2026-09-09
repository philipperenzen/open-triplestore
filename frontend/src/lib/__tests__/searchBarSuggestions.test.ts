/**
 * The ⌘K palette's autosuggestions.
 *
 * They used to come from a hardcoded `mockDatasets` array, so every instance
 * offered the same three invented dataset names. These drive the real component
 * through a real input event — the suggestions are computed in a `$:` block fed
 * by an async fetch, and a pure unit test of the filter would not catch a
 * reactivity break between the two.
 */
import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { render, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import SearchBar from '../../components/SearchBar.svelte';

const listDatasets = vi.fn();
vi.mock('../api.js', () => ({ listDatasets: () => listDatasets() }));

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  listDatasets.mockReset();
  localStorage.clear();
});

const DATASETS = [
  { id: 'waalbrug-bridges', name: 'Waalbrug Bridges' },
  { id: 'geo-basis', name: 'Geo Basisregistratie' },
  { id: 'unrelated', name: 'Personnel Records' },
];

/** Suggestion rows currently rendered, as `[kind, label]` pairs. */
const rows = (container: HTMLElement) =>
  [...container.querySelectorAll('[data-suggestion-kind]')].map((n) => [
    n.getAttribute('data-suggestion-kind'),
    n.textContent?.trim(),
  ]);

async function type(container: HTMLElement, value: string) {
  const input = container.querySelector('#global-search') as HTMLInputElement;
  await fireEvent.input(input, { target: { value } });
  return input;
}

describe('SearchBar dataset suggestions', () => {
  it('suggests datasets returned by the API', async () => {
    listDatasets.mockResolvedValue(DATASETS);
    const { container } = render(SearchBar);

    await type(container, 'waal');

    await waitFor(() => {
      expect(rows(container)).toEqual([['dataset', 'Waalbrug Bridges']]);
    });
    expect(listDatasets).toHaveBeenCalled();
  });

  it('matches the dataset id as well as its name, folding case', async () => {
    listDatasets.mockResolvedValue(DATASETS);
    const { container } = render(SearchBar);

    // Lower-case query against a capitalised name, and an id-only fragment:
    // the old hardcoded list only ever matched verbatim lower-case names.
    await type(container, 'basisregistratie');
    await waitFor(() => {
      expect(rows(container)).toEqual([['dataset', 'Geo Basisregistratie']]);
    });

    await type(container, 'geo-basis');
    await waitFor(() => {
      expect(rows(container)).toEqual([['dataset', 'Geo Basisregistratie']]);
    });
  });

  it('shows nothing for a query no dataset matches', async () => {
    listDatasets.mockResolvedValue(DATASETS);
    const { container } = render(SearchBar);

    await type(container, 'zzzznomatch');
    await waitFor(() => expect(listDatasets).toHaveBeenCalled());
    expect(rows(container)).toEqual([]);
  });

  it('debounces to a single request across a burst of keystrokes', async () => {
    listDatasets.mockResolvedValue(DATASETS);
    const { container } = render(SearchBar);

    await type(container, 'w');
    await type(container, 'wa');
    await type(container, 'waa');
    await type(container, 'waal');

    await waitFor(() => expect(rows(container).length).toBe(1));
    expect(listDatasets).toHaveBeenCalledTimes(1);
  });

  it('does not fetch until the user types', async () => {
    listDatasets.mockResolvedValue(DATASETS);
    render(SearchBar);

    // Opening the palette alone must not cost a request.
    await new Promise((r) => setTimeout(r, 300));
    expect(listDatasets).not.toHaveBeenCalled();
  });

  it('stays usable on an API failure, falling back to recent searches', async () => {
    localStorage.setItem('recentSearches', JSON.stringify(['waalbrug']));
    listDatasets.mockRejectedValue(new Error('403 Forbidden'));
    const { container } = render(SearchBar);

    await type(container, 'waal');

    await waitFor(() => expect(listDatasets).toHaveBeenCalled());
    // The rejection is swallowed: the recent search still shows, and the input
    // still accepts typing.
    expect(rows(container)).toEqual([['recent', 'waalbrug']]);

    const input = await type(container, 'waalb');
    expect(input.value).toBe('waalb');
  });

  it('lists recent searches alongside datasets without repeating one', async () => {
    localStorage.setItem('recentSearches', JSON.stringify(['Waalbrug Bridges', 'waalbrug deck']));
    listDatasets.mockResolvedValue(DATASETS);
    const { container } = render(SearchBar);

    await type(container, 'waal');

    await waitFor(() => expect(listDatasets).toHaveBeenCalled());
    await waitFor(() => {
      // "Waalbrug Bridges" is both a recent search and a dataset name; it must
      // appear once, under whichever source came first.
      expect(rows(container)).toEqual([
        ['recent', 'Waalbrug Bridges'],
        ['recent', 'waalbrug deck'],
      ]);
    });
  });

  it('drops recent searches that do not match the query', async () => {
    localStorage.setItem('recentSearches', JSON.stringify(['payroll', 'holiday rota']));
    listDatasets.mockResolvedValue(DATASETS);
    const { container } = render(SearchBar);

    await type(container, 'waal');

    // An unrelated recent must not take a slot from a real dataset match.
    await waitFor(() => {
      expect(rows(container)).toEqual([['dataset', 'Waalbrug Bridges']]);
    });
  });

  it('keeps arrow-key navigation over the suggestion list', async () => {
    localStorage.setItem('recentSearches', JSON.stringify(['waalbrug deck']));
    listDatasets.mockResolvedValue(DATASETS);
    const { container } = render(SearchBar);

    const input = await type(container, 'waal');
    await waitFor(() => expect(rows(container).length).toBe(2));

    const highlighted = () =>
      [...container.querySelectorAll('[data-suggestion-kind]')].findIndex((n) =>
        n.classList.contains('bg-brand-50'),
      );

    expect(highlighted()).toBe(-1);
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    expect(highlighted()).toBe(0);
    await fireEvent.keyDown(input, { key: 'ArrowDown' });
    expect(highlighted()).toBe(1);
    await fireEvent.keyDown(input, { key: 'ArrowUp' });
    expect(highlighted()).toBe(0);
  });
});
