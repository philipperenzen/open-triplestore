// prettySparql is LAYOUT ONLY: an LLM emits its query on one line, and the chat
// card should read like the SPARQL workspace beside it. These tests pin the two
// things that make such a re-indenter safe — the token stream is preserved, and
// punctuation inside literals/IRIs is never treated as structure.
import { describe, it, expect } from 'vitest';
import { prettySparql } from '../resultHighlight.js';

/** Whitespace-insensitive token stream, for "nothing was added or lost". */
const tokens = (s) => s.replace(/\s+/g, ' ').trim();

describe('prettySparql', () => {
  it('lays out a one-line query', () => {
    const out = prettySparql(
      'PREFIX bh: <https://ex.org/b#> SELECT ?o ?score WHERE { ?o bh:score ?score . FILTER(?score >= 3) } ORDER BY DESC(?score) LIMIT 10',
    );
    expect(out.split('\n')).toEqual([
      'PREFIX bh: <https://ex.org/b#>',
      'SELECT ?o ?score',
      'WHERE {',
      '  ?o bh:score ?score .',
      '  FILTER(?score >= 3)',
      '}',
      'ORDER BY DESC(?score)',
      'LIMIT 10',
    ]);
  });

  it('indents nested groups', () => {
    expect(prettySparql('SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g').split('\n')).toEqual([
      'SELECT (COUNT(*) AS ?n)',
      'WHERE {',
      '  GRAPH ?g {',
      '    ?s ?p ?o',
      '  }',
      '}',
      'GROUP BY ?g',
    ]);
  });

  it('never breaks inside a literal, however much punctuation it holds', () => {
    const q = 'SELECT ?s WHERE { ?s rdfs:label "a . b ; c { } WHERE" }';
    const out = prettySparql(q);
    expect(out).toContain('?s rdfs:label "a . b ; c { } WHERE"');
    expect(tokens(out)).toBe(tokens(q));
  });

  it('keeps a PREFIX on one line with its IRI', () => {
    expect(prettySparql('PREFIX ex: <https://ex.org/> ASK { ?s ?p ?o }')).toContain('PREFIX ex: <https://ex.org/>');
  });

  it('preserves the token stream (layout only)', () => {
    const q = 'SELECT ?s ?p WHERE { ?s a ex:Thing ; ex:p ?p . OPTIONAL { ?s ex:q ?q } } LIMIT 5';
    expect(tokens(prettySparql(q))).toBe(tokens(q));
  });

  it('leaves an already laid-out query, and empty input, alone', () => {
    const already = 'SELECT ?s\nWHERE { ?s ?p ?o }';
    expect(prettySparql(already)).toBe(already);
    expect(prettySparql('')).toBe('');
    expect(prettySparql(null)).toBe('');
  });
});
