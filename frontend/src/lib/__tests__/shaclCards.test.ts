import { describe, it, expect } from 'vitest';
import { Parser } from 'n3';
import { parseShapesGraph, serializeShapesGraph } from '../shaclModel.ts';
import { SHACL_CONSTRAINT_CARDS } from '../shaclConstraints.ts';

// Every palette card must produce SHACL the model can round-trip without any
// quad loss; cards the visual builder can't model are marked sourceOnly and
// still survive verbatim via extraQuads.

const PFX = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <http://example.org/> .
`;

/** Wrap a fragment template into a full shapes document, like the editor does. */
function docFor(template: string): string {
  if (template.includes('sh:NodeShape')) return PFX + template;
  return PFX + 'ex:CardShape a sh:NodeShape ;\n' + template.trim().replace(/;$/, '.');
}

function djb2(s: string): string {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h * 33) ^ s.charCodeAt(i)) >>> 0;
  return h.toString(36);
}

function canonQuads(ttl: string): string[] {
  const quads = new Parser().parse(ttl) as unknown as {
    subject: { termType: string; value: string };
    predicate: { value: string };
    object: { termType: string; value: string; language?: string; datatype?: { value: string } };
  }[];
  const bnodes = new Set<string>();
  for (const q of quads) {
    if (q.subject.termType === 'BlankNode') bnodes.add(q.subject.value);
    if (q.object.termType === 'BlankNode') bnodes.add(q.object.value);
  }
  let sig = new Map<string, string>();
  for (const b of bnodes) sig.set(b, 'b');
  const tstr = (t: { termType: string; value: string; language?: string; datatype?: { value: string } }): string =>
    t.termType === 'BlankNode'
      ? '_:' + sig.get(t.value)
      : t.termType === 'Literal'
        ? `"${t.value}"${t.language ? '@' + t.language : ''}^^${t.datatype?.value || ''}`
        : `<${t.value}>`;
  for (let round = 0; round < 8; round++) {
    const next = new Map<string, string>();
    for (const b of bnodes) {
      const parts: string[] = [sig.get(b)!];
      for (const q of quads) {
        if (q.subject.termType === 'BlankNode' && q.subject.value === b)
          parts.push(`S|${q.predicate.value}|${tstr(q.object)}`);
        if (q.object.termType === 'BlankNode' && q.object.value === b)
          parts.push(`O|${q.predicate.value}|${tstr(q.subject as never)}`);
      }
      parts.sort();
      next.set(b, djb2(parts.join('')));
    }
    sig = next;
  }
  return quads.map((q) => `${tstr(q.subject as never)} <${q.predicate.value}> ${tstr(q.object)}`).sort();
}

describe('SHACL constraint cards', () => {
  it.each(SHACL_CONSTRAINT_CARDS.map((c) => [c.id, c] as const))(
    'card "%s" round-trips quad-identically through the model',
    (_id, card) => {
      const doc = docFor(card.template);
      const g = parseShapesGraph(doc);
      expect(g.parseError).toBeFalsy();
      expect(g.canRoundTrip).toBe(true);
      const out = serializeShapesGraph(g);
      expect(canonQuads(out)).toEqual(canonQuads(doc));
    },
  );

  it.each(SHACL_CONSTRAINT_CARDS.map((c) => [c.id, c] as const))(
    'card "%s" is modelled exactly when it is not sourceOnly',
    (_id, card) => {
      const g = parseShapesGraph(docFor(card.template));
      expect(!!g.hasUnsupported).toBe(!!card.sourceOnly);
    },
  );

  it('marks exactly the SPARQL-based cards as sourceOnly', () => {
    const sourceOnly = SHACL_CONSTRAINT_CARDS.filter((c) => c.sourceOnly).map((c) => c.id);
    expect(sourceOnly.sort()).toEqual(['ruleSparql', 'sparqlConstraint']);
  });
});
