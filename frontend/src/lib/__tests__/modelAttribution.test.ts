// The bundled vocabularies the server seeds as reference models keep their
// publishers' licences, and the store keeps no comment headers, so the model
// pages show the licence record `/api/models` serves (`attribution`), and the
// term card's source pill links to the bundled files' NOTICE.md.
import { describe, it, expect, beforeAll } from 'vitest';
import { render } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import nl from '../i18n/nl.json';
import ModelAttributionCard from '../../components/ModelAttributionCard.svelte';
import TermDefinitionCard from '../../components/ontology/TermDefinitionCard.svelte';
import type { ModelAttribution } from '../api';
import { emptyTermMeta } from '../ontology/termTypes';

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

const RDF: ModelAttribution = {
  file: 'rdf.ttl',
  licenses: [{ name: 'W3C Document License (2023)', uri: 'https://www.w3.org/copyright/document-license-2023/' }],
  copyright: ['Copyright © 2019 World Wide Web Consortium. https://www.w3.org/copyright/document-license-2023/'],
  notice:
    'Copyright © 2023 W3C®. This software or document includes material copied from or derived from The RDF Concepts Vocabulary (RDF), https://www.w3.org/1999/02/22-rdf-syntax-ns.ttl.',
  status: 'Defined in RDF 1.1 Concepts and Abstract Syntax, W3C Recommendation 25 February 2014, https://www.w3.org/TR/2014/REC-rdf11-concepts-20140225/',
  source_url: 'https://www.w3.org/1999/02/22-rdf-syntax-ns.ttl',
  specification_url: 'https://www.w3.org/TR/rdf11-concepts/',
  changes: 'Changes by Open Triplestore: this comment header was added.',
  stored_copy: 'The store holds the triples of the bundled file vocab/rdf.ttl, unchanged.',
  unchanged: true,
  remarks: null,
  no_derivatives: false,
  header: 'The RDF Concepts Vocabulary (RDF) — the RDF Schema\nfor the terms in the RDF namespace.',
  notice_url: 'http://localhost:7878/vocab/NOTICE.md',
};

const IMBOR: ModelAttribution = {
  ...RDF,
  file: 'imbor.ttl',
  licenses: [
    { name: 'CC BY 4.0', uri: 'https://creativecommons.org/licenses/by/4.0/' },
    { name: 'Open Data Commons Attribution License (ODC-BY) 1.0', uri: 'https://opendatacommons.org/licenses/by/1-0/' },
  ],
  copyright: ['© Stichting CROW'],
  notice: null,
  status: null,
  header: null,
  no_derivatives: true,
};

describe('ModelAttributionCard', () => {
  it('shows the licence, copyright, notice, status, source and the full notice link', () => {
    const { container, getByText } = render(ModelAttributionCard, { attribution: RDF });
    const card = container.querySelector('[data-testid="model-attribution"]');
    expect(card).toBeTruthy();
    const licence = container.querySelector('a[href="https://www.w3.org/copyright/document-license-2023/"]');
    expect(licence?.textContent).toBe('W3C Document License (2023)');
    expect(licence?.getAttribute('rel')).toContain('license');
    expect(getByText(RDF.copyright[0])).toBeTruthy();
    expect(getByText(RDF.notice as string)).toBeTruthy();
    expect(getByText(RDF.status as string)).toBeTruthy();
    expect(container.querySelector(`a[href="${RDF.source_url}"]`)).toBeTruthy();
    expect(container.querySelector(`a[href="${RDF.specification_url}"]`)).toBeTruthy();
    expect(getByText(RDF.stored_copy)).toBeTruthy();
    // The file's own header, verbatim.
    expect(container.querySelector('details pre')?.textContent).toBe(RDF.header);
    // The NOTICE link points at the copy this UI ships, whatever the server's base URL.
    const notice = getByText(en.pages.modelDetail.attribution.fullNotice);
    expect(notice.getAttribute('href')).toBe('/vocab/NOTICE.md');
  });

  it('marks content that allows no altered copies', () => {
    const { container, getAllByText } = render(ModelAttributionCard, { attribution: IMBOR });
    expect(getAllByText(en.pages.modelDetail.attribution.noDerivatives).length).toBe(1);
    expect(container.querySelector('a[href="https://opendatacommons.org/licenses/by/1-0/"]')).toBeTruthy();
    expect(container.querySelector('details')).toBeNull();
  });

  it('renders a one-line credit in the compact variant', () => {
    const { container } = render(ModelAttributionCard, { attribution: RDF, variant: 'compact' });
    const line = container.querySelector('[data-testid="model-attribution-compact"]');
    expect(line?.textContent).toContain('W3C Document License (2023)');
    expect(line?.textContent).toContain('Copyright © 2019 World Wide Web Consortium.');
    expect(line?.querySelector('a[href="/vocab/NOTICE.md"]')).toBeTruthy();
  });

  it('says a copy may have been modified unless the server checked it', () => {
    const copy = {
      ...RDF,
      unchanged: false,
      stored_copy:
        'Copied in this registry from version 1.1. Its content derives from the bundled file vocab/rdf.ttl; it may have been modified in this registry, so it is not that file.',
    };
    for (const variant of ['full', 'compact'] as const) {
      const { container, unmount } = render(ModelAttributionCard, { attribution: copy, variant });
      const badge = container.querySelector('[data-testid="model-attribution-modified"]');
      expect(badge?.textContent).toBe(en.pages.modelDetail.attribution.possiblyModified);
      unmount();
    }
    // A checked copy, and a record from a server that predates the flag, carry no badge.
    for (const attribution of [RDF, { ...RDF, unchanged: undefined }]) {
      const { container, unmount } = render(ModelAttributionCard, { attribution });
      expect(container.querySelector('[data-testid="model-attribution-modified"]')).toBeNull();
      unmount();
    }
  });

  it('renders nothing without a licence record', () => {
    const { container } = render(ModelAttributionCard, { attribution: null });
    expect(container.textContent?.trim()).toBe('');
  });

  it('never turns a non-web source into a link', () => {
    const { container } = render(ModelAttributionCard, {
      attribution: { ...RDF, source_url: 'javascript:alert(1)', specification_url: 'javascript:alert(2)' },
    });
    expect(container.querySelector('a[href^="javascript:"]')).toBeNull();
  });
});

describe('TermDefinitionCard source pill', () => {
  it('links the bundled vocabulary to its licence and attribution', () => {
    const meta = { ...emptyTermMeta('http://www.w3.org/ns/dcat#Dataset', 'dcat'), termType: 'owl:Class' as const };
    meta.labels = [{ lang: 'en', value: 'Dataset' }];
    const { container } = render(TermDefinitionCard, { iri: meta.iri, meta, variant: 'rich' });
    const pill = container.querySelector('a.tdc-src');
    expect(pill?.textContent).toBe('dcat');
    expect(pill?.getAttribute('href')).toBe('/vocab/NOTICE.md');
    expect(pill?.getAttribute('title')).toBe('Bundled vocabulary dcat.ttl: licence and attribution');
  });
});

describe('attribution translations', () => {
  it('has every key in both locales', () => {
    const enKeys = Object.keys(en.pages.modelDetail.attribution).sort();
    const nlKeys = Object.keys((nl as typeof en).pages.modelDetail.attribution).sort();
    expect(nlKeys).toEqual(enKeys);
    expect((nl as typeof en).components.termDefinitionCard.sourceNotice).toContain('{file}');
  });
});
