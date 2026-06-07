import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { parseTurtle } from '../loader';
import { indexStore, VOCAB_FILES } from '../termDictionary';

// Validates the bundled vocabulary assets (public/vocab/*.ttl) the same way the
// app does at runtime — parse with n3, index with indexStore — so a corrupt or
// unparseable download fails CI rather than silently showing no definitions.
const VOCAB_DIR = resolve(process.cwd(), 'public/vocab');
const files = readdirSync(VOCAB_DIR).filter((f) => f.endsWith('.ttl'));

const load = async (file: string) =>
  indexStore((await parseTurtle(readFileSync(resolve(VOCAB_DIR, file), 'utf8'))).store, 'x');

describe('bundled vocabulary assets', () => {
  it('ships a file on disk for every VOCAB_FILES entry', () => {
    for (const { file } of Object.values(VOCAB_FILES)) {
      expect(files, `${file} is referenced by VOCAB_FILES but missing from public/vocab`).toContain(file);
    }
  });

  it.each(files)('%s parses with n3 and indexes ≥1 term', async (file) => {
    const idx = await load(file);
    expect(idx.size, `${file} produced no terms`).toBeGreaterThan(0);
  });

  it('resolves canonical anchor terms across the bundle', async () => {
    const dcat = await load('dcat.ttl');
    const mediaType = dcat.get('http://www.w3.org/ns/dcat#mediaType');
    expect(mediaType?.termType).toBe('owl:ObjectProperty');
    expect(mediaType?.labels.length, 'dcat:mediaType is multi-language').toBeGreaterThan(1);
    expect(mediaType?.definitions.length).toBeGreaterThan(0);

    expect((await load('skos.ttl')).get('http://www.w3.org/2004/02/skos/core#definition')).toBeTruthy();
    expect((await load('foaf.ttl')).get('http://xmlns.com/foaf/0.1/Person')).toBeTruthy();
    expect((await load('void.ttl')).get('http://rdfs.org/ns/void#triples')).toBeTruthy();
    expect((await load('xsd.ttl')).get('http://www.w3.org/2001/XMLSchema#string')).toBeTruthy();
    expect((await load('owl.ttl')).size).toBeGreaterThan(0);

    // schema.org terms are bundled under BOTH http and https IRIs (data uses either).
    const schema = await load('schema.ttl');
    expect(schema.get('https://schema.org/Person'), 'https schema.org/Person').toBeTruthy();
    expect(schema.get('http://schema.org/Person'), 'http schema.org/Person').toBeTruthy();
  });
});
