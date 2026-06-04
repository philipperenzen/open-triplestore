// Shared SPARQL diagnostics catalog.
//
// This module is the single source of wording + behaviour for every SPARQL
// editor diagnostic (errors, warnings, hints). It is intended to be mirrored
// VERBATIM with any companion app's SPARQL editor so that the same query reports
// identically in every UI. Keep copies in sync — the only intended difference is
// the `sparqljs` import style.

import { linter, type Diagnostic } from '@codemirror/lint';
import type { EditorView } from '@codemirror/view';
import sparqljs from 'sparqljs';

const Parser = sparqljs.Parser;

export type Severity = 'error' | 'warning' | 'info' | 'hint';

export interface DiagnosticContext {
  /** Resolve a prefix to a namespace IRI (sync) for the "add PREFIX" quick-fix. */
  resolvePrefix?: (prefix: string) => string | null;
  /** Known ontology IRIs, enables the "possible typo" check when supplied. */
  knownIris?: Set<string>;
  /** Disable the no-LIMIT hint (e.g. for constraint/rule editors). */
  noLimitCheck?: boolean;
}

interface PrefixFix {
  kind: 'add-prefix';
  prefix: string;
  namespace: string;
}

export interface SparqlFinding {
  from: number;
  to: number;
  code: string;
  severity: Severity;
  message: string;
  /** One-line remediation shown under the message. */
  hint?: string;
  fix?: PrefixFix;
}

// ── Message catalog ─────────────────────────────────────────────────────────
// Edit copy here to change it across every editor in every app. `code` values
// are stable identifiers surfaced faintly in the lint tooltip (Diagnostic.source).
export const SPARQL_DIAGNOSTICS = {
  syntax: (detail: string) => ({
    code: 'sparql/syntax',
    severity: 'error' as const,
    message: detail ? `Syntax error — ${detail}` : 'Syntax error in query.',
    hint: 'Check matching brackets, “.” between triples, and that every prefix is declared.',
  }),
  undeclaredPrefix: (prefix: string, resolvable: boolean) => ({
    code: 'sparql/undeclared-prefix',
    severity: 'warning' as const,
    message: `Prefix “${prefix}:” is used but not declared.`,
    hint: resolvable
      ? 'A matching namespace is known — use the quick-fix to add the PREFIX line.'
      : `Add a “PREFIX ${prefix}: <…>” declaration above the query.`,
  }),
  unusedPrefix: (prefix: string) => ({
    code: 'sparql/unused-prefix',
    severity: 'info' as const,
    message: `Prefix “${prefix}:” is declared but never used.`,
    hint: 'You can remove this PREFIX line to keep the query tidy.',
  }),
  noLimit: () => ({
    code: 'sparql/no-limit',
    severity: 'info' as const,
    message: 'This query has no LIMIT.',
    hint: 'Add a LIMIT to avoid accidentally fetching a very large result set.',
  }),
  unknownIri: () => ({
    code: 'sparql/unknown-iri',
    severity: 'warning' as const,
    message: 'IRI not found in the loaded ontology — possible typo.',
    hint: 'Double-check the local name against the ontology vocabulary.',
  }),
};

const RESERVED_PSEUDO = new Set(['http', 'https', 'urn', 'mailto', 'tel', 'ftp', 'file', 'data', 'bnode']);
const PREFIX_DECL_RE = /PREFIX\s+([A-Za-z_][\w-]*)\s*:\s*<([^>]*)>/gi;
const QUERY_FORM_RE = /\b(SELECT|CONSTRUCT|DESCRIBE|ASK)\b/i;

interface PrefixUse { prefix: string; from: number; to: number; }

/** Locate `prefix:local` uses, skipping IRIs, string literals, comments and the
 *  PREFIX/BASE declarations themselves, so we only flag genuine *uses*. */
function findPrefixUses(text: string): PrefixUse[] {
  const uses: PrefixUse[] = [];
  const n = text.length;
  let i = 0;
  while (i < n) {
    const ch = text[i];
    // Line comment
    if (ch === '#') { while (i < n && text[i] !== '\n') i++; continue; }
    // IRI ref vs. the `<`/`<=` comparison operator
    if (ch === '<') {
      const next = text[i + 1];
      if (next === undefined || next === ' ' || next === '\t' || next === '=' || next === '\n') { i++; continue; }
      i++;
      while (i < n && text[i] !== '>' && text[i] !== '\n') i++;
      if (i < n && text[i] === '>') i++;
      continue;
    }
    // String literals (single + triple quoted)
    if (ch === '"' || ch === "'") {
      const triple = text.slice(i, i + 3);
      if (triple === ch + ch + ch) {
        i += 3;
        while (i < n && text.slice(i, i + 3) !== ch + ch + ch) i++;
        i += 3;
      } else {
        i++;
        while (i < n && text[i] !== ch && text[i] !== '\n') { if (text[i] === '\\') i++; i++; }
        i++;
      }
      continue;
    }
    // Identifiers / prefixed names / keywords
    if (/[A-Za-z_]/.test(ch)) {
      const start = i;
      while (i < n && /[\w-]/.test(text[i])) i++;
      const word = text.slice(start, i);
      if (word.toUpperCase() === 'PREFIX' || word.toUpperCase() === 'BASE') {
        // Consume the whole declaration so its label isn't seen as a "use".
        while (i < n && text[i] !== '>' && text[i] !== '\n') i++;
        if (i < n && text[i] === '>') i++;
        continue;
      }
      if (text[i] === ':') {
        const colonAt = i;
        i++; // ':'
        while (i < n && /[\w.\-%]/.test(text[i])) i++;
        if (!RESERVED_PSEUDO.has(word.toLowerCase())) {
          uses.push({ prefix: word, from: start, to: colonAt + 1 });
        }
      }
      continue;
    }
    i++;
  }
  return uses;
}

function lineRange(text: string, line1: number): { from: number; to: number } {
  const lines = text.split('\n');
  const idx = Math.max(1, Math.min(line1, lines.length)) - 1;
  let from = 0;
  for (let i = 0; i < idx; i++) from += lines[i].length + 1;
  return { from, to: from + (lines[idx]?.length ?? 0) };
}

function locateParseError(text: string, e: any): { from: number; to: number; detail: string } {
  const msg = String(e?.message || 'parse error');
  const lm = /line\s+(\d+)/i.exec(msg);
  const line = lm ? parseInt(lm[1], 10) : 1;
  // Pull the human-readable tail (jison/sparqljs prefix the message with a code snippet).
  let detail = '';
  const m = msg.match(/(Expecting[\s\S]*|Unexpected[\s\S]*|Lexical error[\s\S]*|Unknown prefix[\s\S]*)/i);
  if (m) detail = m[1];
  else detail = msg.split('\n').filter((l) => l.trim() && !/\^/.test(l)).pop() || msg;
  detail = detail.replace(/\s+/g, ' ').trim();
  if (detail.length > 180) detail = detail.slice(0, 177) + '…';
  return { ...lineRange(text, line), detail };
}

/** Pure analysis: text → findings. Framework-agnostic and unit-testable. */
export function analyzeSparql(doc: string, ctx: DiagnosticContext = {}): SparqlFinding[] {
  const findings: SparqlFinding[] = [];
  const text = doc;
  if (!text.trim()) return findings;

  // 1) Syntax (sparqljs parse)
  let parseOk = false;
  try {
    new (Parser as any)().parse(text);
    parseOk = true;
  } catch (e) {
    const { from, to, detail } = locateParseError(text, e);
    findings.push({ from, to, ...SPARQL_DIAGNOSTICS.syntax(detail) });
  }

  // 2) Prefix hygiene
  const declared = new Map<string, { ns: string; from: number; to: number }>();
  for (const m of text.matchAll(PREFIX_DECL_RE)) {
    declared.set(m[1], { ns: m[2], from: m.index!, to: m.index! + m[0].length });
  }
  const uses = findPrefixUses(text);
  const usedSet = new Set(uses.map((u) => u.prefix));

  for (const u of uses) {
    if (declared.has(u.prefix)) continue;
    const ns = ctx.resolvePrefix?.(u.prefix) || null;
    const base = SPARQL_DIAGNOSTICS.undeclaredPrefix(u.prefix, !!ns);
    findings.push({ from: u.from, to: u.to, ...base, fix: ns ? { kind: 'add-prefix', prefix: u.prefix, namespace: ns } : undefined });
  }
  for (const [p, d] of declared) {
    if (!usedSet.has(p)) findings.push({ from: d.from, to: d.to, ...SPARQL_DIAGNOSTICS.unusedPrefix(p) });
  }

  // 3) Missing LIMIT (valid SELECT/CONSTRUCT/DESCRIBE with a body, not ASK)
  if (parseOk && !ctx.noLimitCheck) {
    const fm = QUERY_FORM_RE.exec(text);
    const form = fm?.[1]?.toUpperCase();
    if (form && form !== 'ASK' && /\{/.test(text) && !/\bLIMIT\s+\d+/i.test(text)) {
      findings.push({ from: fm!.index, to: fm!.index + fm![1].length, ...SPARQL_DIAGNOSTICS.noLimit() });
    }
  }

  // 4) Possible-typo IRI (only with a known-IRI set)
  if (ctx.knownIris && ctx.knownIris.size) {
    for (const m of text.matchAll(/<([^>\s]+)>/g)) {
      const iri = m[1];
      if (!/^https?:/.test(iri) || ctx.knownIris.has(iri)) continue;
      for (const known of ctx.knownIris) {
        const ns = known.replace(/[^#/]*$/, '');
        if (iri.startsWith(ns) && iri !== known) {
          findings.push({ from: m.index!, to: m.index! + iri.length + 2, ...SPARQL_DIAGNOSTICS.unknownIri() });
          break;
        }
      }
    }
  }

  return findings;
}

/** Build a CodeMirror linter extension backed by the shared catalog. */
export function sparqlLinter(ctx: DiagnosticContext = {}) {
  return linter((view) => {
    const len = view.state.doc.length;
    return analyzeSparql(view.state.doc.toString(), ctx).map((f): Diagnostic => {
      const d: Diagnostic = {
        from: Math.max(0, Math.min(f.from, len)),
        to: Math.max(0, Math.min(Math.max(f.from, f.to), len)),
        severity: f.severity,
        message: f.hint ? `${f.message}\n${f.hint}` : f.message,
        source: f.code,
      };
      if (f.fix?.kind === 'add-prefix') {
        const fix = f.fix;
        d.actions = [{
          name: `Add PREFIX ${fix.prefix}:`,
          apply(v: EditorView) {
            const text = v.state.doc.toString();
            const block = /^(\s*(?:PREFIX\s+[^\n]*\n|BASE\s+[^\n]*\n)+)/i.exec(text);
            const at = block ? block[0].length : 0;
            v.dispatch({ changes: { from: at, to: at, insert: `PREFIX ${fix.prefix}: <${fix.namespace}>\n` } });
          },
        }];
      }
      return d;
    });
  });
}
