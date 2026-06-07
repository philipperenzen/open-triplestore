import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { parseTurtle } from '../loader';
import { indexStore } from '../termDictionary';

// Validates the hand-authored OTS vocabulary asset so authoring mistakes (a typo'd
// IRI, a missing label/definition) fail CI without needing a running server.
const ttl = readFileSync(resolve(process.cwd(), 'public/vocab/ots.ttl'), 'utf8');

const O = 'https://opentriplestore.org/ontology/';
const NS = 'https://opentriplestore.org/ns#';
const A = 'https://opentriplestore.org/ns/asset#';

describe('public/vocab/ots.ttl', () => {
  it('parses and indexes every OTS model term with a label and a definition', async () => {
    const { store } = await parseTurtle(ttl);
    const idx = indexStore(store, 'ots');
    const terms = [
      O + 'GraphRole', O + 'graphRole', O + 'auditStatus',
      O + 'Instances', O + 'Model', O + 'Vocabulary', O + 'Shapes', O + 'Entailment', O + 'System',
      NS + 'Standard', NS + 'conformance',
      A + 'rowCount', A + 'pointCount',
    ];
    for (const iri of terms) {
      const m = idx.get(iri);
      expect(m, `missing term ${iri}`).toBeTruthy();
      if (!m) continue;
      expect(m.labels.length, `${iri} needs a label`).toBeGreaterThan(0);
      expect(
        m.definitions.length || m.comments.length,
        `${iri} needs a definition or comment`,
      ).toBeGreaterThan(0);
    }
  });

  it('captures multi-language (en + nl) labels', async () => {
    const { store } = await parseTurtle(ttl);
    const idx = indexStore(store, 'ots');
    expect(idx.get(O + 'Instances')?.labels.map((l) => l.lang).sort()).toEqual(['en', 'nl']);
  });

  it('resolves OWL types for the standards-registry terms', async () => {
    const { store } = await parseTurtle(ttl);
    const idx = indexStore(store, 'ots');
    expect(idx.get(NS + 'Standard')?.termType).toBe('owl:Class');
    expect(idx.get(NS + 'conformance')?.termType).toBe('owl:ObjectProperty');
    expect(idx.get(A + 'rowCount')?.termType).toBe('owl:DatatypeProperty');
  });
});
