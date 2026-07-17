// Ontology-aware CodeMirror autocompletion for SPARQL.
// - Built-in namespaces + vocab terms always available (works without ontology)
// - Auto-inserts PREFIX declarations when a known prefix is used but not declared
// - Looks up unknown prefixes via prefix.cc (async, cached)

import { autocompletion } from '@codemirror/autocomplete';
import { shortenIRI } from '../rdf-utils.js';
import { NAMESPACES, VOCAB, allBuiltinTerms } from './vocabularies.js';
import {
  lookupPrefixSync, lookupPrefix, warmPrefix, extractDeclaredPrefixes,
} from './prefixService.js';

const KEYWORDS = [
  'SELECT', 'DISTINCT', 'REDUCED', 'WHERE', 'FILTER', 'OPTIONAL', 'UNION',
  'GRAPH', 'NAMED', 'FROM', 'ASK', 'CONSTRUCT', 'DESCRIBE', 'INSERT', 'DELETE',
  'PREFIX', 'BASE', 'LIMIT', 'OFFSET', 'ORDER BY', 'GROUP BY', 'HAVING',
  'VALUES', 'BIND', 'SERVICE', 'MINUS', 'NOT EXISTS', 'EXISTS', 'AS',
  'COUNT', 'SUM', 'MIN', 'MAX', 'AVG', 'SAMPLE', 'GROUP_CONCAT',
  'STR', 'LANG', 'LANGMATCHES', 'DATATYPE', 'BOUND', 'IRI', 'URI',
  'CONCAT', 'STRLEN', 'SUBSTR', 'UCASE', 'LCASE', 'CONTAINS',
  'STRSTARTS', 'STRENDS', 'REGEX', 'REPLACE', 'NOW', 'YEAR',
  'MONTH', 'DAY', 'IF', 'COALESCE', 'ISIRI', 'ISLITERAL', 'ISBLANK',
];

function kindToCMType(kind: string): string {
  switch (kind) {
    case 'object':
    case 'datatype':
    case 'annotation':
    case 'property': return 'property';
    case 'class': return 'class';
    default: return 'variable';
  }
}

import type { EditorView } from '@codemirror/view';
import type { Completion } from '@codemirror/autocomplete';

function applyWithPrefix(prefix: string, ns: string, insertText: string) {
  return (view: EditorView, _completion: Completion, from: number, to: number) => {
    const doc = view.state.doc.toString();
    const declared = extractDeclaredPrefixes(doc);
    const changes = [{ from, to, insert: insertText }];
    let selPos = from + insertText.length;
    if (!declared[prefix]) {
      const line = `PREFIX ${prefix}: <${ns}>\n`;
      // Insert after any existing PREFIX/BASE block at top, else at position 0.
      const m = /^(\s*(?:PREFIX\s+[^\n]*\n|BASE\s+[^\n]*\n)+)/i.exec(doc);
      const insPos = m ? m[0].length : 0;
      changes.unshift({ from: insPos, to: insPos, insert: line });
      selPos += line.length;
    }
    view.dispatch({ changes, selection: { anchor: selPos } });
  };
}

interface OntologyTerm {
  iri: string;
  label?: string;
  comment?: string;
  kind?: string;
  _ns?: string;
}

export function ontologyAwareAutocomplete({ prefixes = {} as Record<string, string>, terms = [] as OntologyTerm[] } = {}): ReturnType<typeof autocompletion> {
  const builtinTerms = allBuiltinTerms();
  const byNs = new Map();
  for (const t of [...builtinTerms, ...terms]) {
    if (!byNs.has(t._ns || guessNs(t.iri))) byNs.set(t._ns || guessNs(t.iri), []);
    byNs.get(t._ns || guessNs(t.iri)).push(t);
  }

  return autocompletion({
    override: [async (context) => {
      const doc = context.state.doc.toString();
      const line = context.state.doc.lineAt(context.pos);
      const before = doc.slice(line.from, context.pos);
      const declared = extractDeclaredPrefixes(doc);
      const prefixMap = { ...NAMESPACES, ...prefixes, ...declared };

      // --- Case 1: inside <...>  → full IRI completion
      const iriOpen = before.lastIndexOf('<');
      const iriClose = before.lastIndexOf('>');
      if (iriOpen > iriClose && !/\s/.test(before.slice(iriOpen))) {
        const fragment = before.slice(iriOpen + 1).toLowerCase();
        const pool = [...builtinTerms, ...terms];
        const options = pool
          .filter(t => !fragment || t.iri.toLowerCase().includes(fragment))
          .slice(0, 200)
          .map(t => ({
            label: t.iri,
            displayLabel: t.label ? `${t.label} · ${shortenIRI(t.iri)}` : shortenIRI(t.iri),
            detail: t.kind || '',
            info: t.comment || '',
            apply: `${t.iri}>`,
            type: kindToCMType(t.kind),
          }));
        return {
          from: line.from + iriOpen + 1,
          to: context.pos,
          options,
          validFor: /^[^\s>]*$/,
        };
      }

      // --- Case 2: typing  prefix:local  → vocab terms (+ auto-insert PREFIX)
      const prefMatch = /([a-zA-Z_][\w-]*):([a-zA-Z_][\w-]*)?$/.exec(before);
      if (prefMatch) {
        const prefix = prefMatch[1];
        const localLen = prefMatch[2]?.length || 0;
        const from = context.pos - localLen;

        // Resolve namespace: declared > built-in/extra > prefix.cc (async)
        let ns = prefixMap[prefix] || lookupPrefixSync(prefix);
        if (!ns) {
          if (context.explicit) ns = await lookupPrefix(prefix);
          else { warmPrefix(prefix); return null; }
        }
        if (!ns) return null;

        const pool = VOCAB[prefix] || byNs.get(ns) || [];
        const options = pool.slice(0, 500).map(t => {
          const local = t.iri.slice(ns.length);
          const insert = local;
          return {
            label: local,
            displayLabel: t.label && t.label !== local ? `${local} — ${t.label}` : local,
            detail: t.kind || prefix,
            info: t.comment || t.iri,
            type: kindToCMType(t.kind),
            apply: applyWithPrefix(prefix, ns, insert),
          };
        });

        // Also offer the raw "insert PREFIX" completion when local part is empty
        if (!localLen && !declared[prefix]) {
          options.unshift({
            label: `PREFIX ${prefix}: <${ns}>`,
            displayLabel: `Insert PREFIX ${prefix}:`,
            detail: ns,
            type: 'namespace',
            boost: 99,
            apply: applyWithPrefix(prefix, ns, ''),
          });
        }
        return { from, to: context.pos, options, validFor: /^[\w-]*$/ };
      }

      // --- Case 3: word position — keywords, variables, prefix names
      const word = context.matchBefore(/[?$]?[\w-]+/);
      if (!word && !context.explicit) return null;
      const w = (word?.text || '').toLowerCase();

      const varNames = new Set();
      for (const m of doc.matchAll(/[?$]([a-zA-Z_][\w-]*)/g)) varNames.add(m[1]);

      const prefixNames = new Set([
        ...Object.keys(NAMESPACES),
        ...Object.keys(prefixes),
        ...Object.keys(declared),
      ]);

      const options = [
        ...KEYWORDS.map(k => ({ label: k, type: 'keyword', boost: 5 })),
        ...[...prefixNames].map(p => {
          const ns = prefixMap[p] || lookupPrefixSync(p);
          return {
            label: `${p}:`,
            displayLabel: `${p}:`,
            detail: ns ? shortenIRI(ns) : 'prefix',
            type: 'namespace',
            info: ns ? `PREFIX ${p}: <${ns}>` : '',
            boost: 8,
          };
        }),
        ...[...varNames].map(v => ({ label: `?${v}`, type: 'variable', boost: 7 })),
      ];

      const from = word ? word.from : context.pos;
      return {
        from,
        options: options.filter(o =>
          !w || o.label.toLowerCase().startsWith(w) || o.label.toLowerCase().includes(w)
        ),
        validFor: /^[?$]?[\w-:]*$/,
      };
    }],
  });
}

function guessNs(iri: string): string {
  const i = Math.max(iri.lastIndexOf('#'), iri.lastIndexOf('/'));
  return i >= 0 ? iri.slice(0, i + 1) : iri;
}

export function buildPrefixBlock(prefixes: Record<string, string>): string {
  return Object.entries(prefixes).map(([p, iri]) => `PREFIX ${p}: <${iri}>`).join('\n');
}
