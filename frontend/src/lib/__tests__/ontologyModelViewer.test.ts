// The model ("taxonomy") viewer rendered an empty class tree / "Classes 0" for
// model-registry versions.
//
// Mechanism (measured against a seeded backend): the viewer ran TWO loaders for the
// same version. The parent (OntologyBrowserPanel) fetches the version's Turtle from
// the model-registry endpoint and passes it down as `preloadedStore`; meanwhile the
// viewer's own onMount `load()` issued `CONSTRUCT { ?s ?p ?o } WHERE { GRAPH <g> …}`
// over /sparql. Model-registry version graphs are never inserted into
// `dataset_graphs`, so `scope_query_to_authorized` excludes them for every principal
// and that CONSTRUCT returns HTTP 200 with ZERO bytes — verified for schema, bot,
// rdfs, time, otl and org, all of which serve 8–126 KB over the registry endpoint.
// Whichever loader resolved LAST won. The empty one usually did, clobbering a good
// model with an empty one.
//
// The fix makes the SPARQL fallback opt-in (`allowSparqlFallback`, default off) and
// re-checks `preloadedStore` after the await so a late CONSTRUCT can never clobber.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { init, addMessages } from 'svelte-i18n';
import { Store, DataFactory } from 'n3';
import en from '../i18n/en.json';
import OntologyModelViewer from '../../components/OntologyModelViewer.svelte';

const { namedNode, literal, quad } = DataFactory;
const RDFS = 'http://www.w3.org/2000/01/rdf-schema#';
const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const OWL = 'http://www.w3.org/2002/07/owl#';
const GRAPH = 'https://example.org/data-model/ex/version/1.0';

beforeAll(() => {
  addMessages('en', en as Record<string, unknown>);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

/** A minimal ontology: 2 classes, 1 object property. */
function ontologyStore(): Store {
  const s = new Store();
  const EX = 'https://example.org/ns#';
  for (const c of ['Bridge', 'Tunnel']) {
    s.addQuad(quad(namedNode(EX + c), namedNode(RDF + 'type'), namedNode(OWL + 'Class')));
    s.addQuad(quad(namedNode(EX + c), namedNode(RDFS + 'label'), literal(c, 'en')));
  }
  s.addQuad(quad(namedNode(EX + 'crosses'), namedNode(RDF + 'type'), namedNode(OWL + 'ObjectProperty')));
  s.addQuad(quad(namedNode(EX + 'crosses'), namedNode(RDFS + 'domain'), namedNode(EX + 'Bridge')));
  return s;
}

/** Numeric badges on the tab buttons, in tab order (classes, properties, …). */
function tabCounts(container: HTMLElement): number[] {
  return [...container.querySelectorAll('.omv-tab')]
    .map((b) => b.querySelector('.count')?.textContent?.trim())
    .filter((v): v is string => v != null && v !== '')
    .map((v) => Number(v));
}

/**
 * Stub /sparql the way the real server answers a model-registry version graph:
 * 200 OK, `text/turtle`, empty body — after `delayMs`, so the CONSTRUCT resolves
 * AFTER the parent's store has already arrived.
 */
function stubEmptyConstruct(delayMs: number, seen: string[]) {
  globalThis.fetch = ((input: RequestInfo | URL) => {
    seen.push(String(input));
    return new Promise((resolve) =>
      setTimeout(
        () => resolve(new Response('', { status: 200, headers: { 'content-type': 'text/turtle' } })),
        delayMs,
      ),
    );
  }) as typeof fetch;
}

describe('OntologyModelViewer', () => {
  it('shows the real class/property counts when the store arrives as a prop', async () => {
    const { container, rerender } = render(OntologyModelViewer, {
      graphIri: '',
      subGraphs: [],
      preloadedStore: null,
    });
    await rerender({ graphIri: '', subGraphs: [], preloadedStore: ontologyStore() });
    await tick();

    const counts = tabCounts(container);
    expect(counts[0]).toBe(2); // Classes
    expect(counts[1]).toBe(1); // Properties
    expect(container.textContent).toContain('Bridge');
  });

  // THE REGRESSION GUARD. Fails on the pre-fix component: the late empty CONSTRUCT
  // overwrites the good model and the tree empties out.
  it('a late empty CONSTRUCT must not clobber a model built from preloadedStore', async () => {
    const seen: string[] = [];
    stubEmptyConstruct(40, seen);

    // Mount exactly as OntologyBrowserPanel did: a real graph IRI, store not yet
    // fetched — so the old code starts its CONSTRUCT here.
    const { container, rerender } = render(OntologyModelViewer, {
      graphIri: GRAPH,
      subGraphs: [],
      preloadedStore: null,
    });

    // The registry fetch wins the race and delivers a good store.
    await rerender({ graphIri: GRAPH, subGraphs: [], preloadedStore: ontologyStore() });
    await tick();
    expect(tabCounts(container)[0]).toBe(2);

    // Now let the slow, empty CONSTRUCT land.
    await new Promise((r) => setTimeout(r, 120));
    await tick();

    expect(tabCounts(container)[0]).toBe(2);
    expect(tabCounts(container)[1]).toBe(1);
    expect(container.textContent).toContain('Bridge');
  });

  it('issues no /sparql request at all when a preloadedStore is supplied', async () => {
    const seen: string[] = [];
    stubEmptyConstruct(0, seen);

    render(OntologyModelViewer, {
      graphIri: GRAPH,
      subGraphs: [],
      preloadedStore: ontologyStore(),
    });
    await tick();
    await new Promise((r) => setTimeout(r, 60));

    expect(seen.filter((u) => u.includes('sparql'))).toHaveLength(0);
  });

  it('still uses the SPARQL path when explicitly opted in and no store is supplied', async () => {
    const seen: string[] = [];
    stubEmptyConstruct(0, seen);

    render(OntologyModelViewer, {
      graphIri: GRAPH,
      subGraphs: [],
      preloadedStore: null,
      allowSparqlFallback: true,
    });
    await tick();
    await new Promise((r) => setTimeout(r, 60));

    expect(seen.filter((u) => u.includes('sparql')).length).toBeGreaterThan(0);
  });
});
