import { StreamLanguage, foldService } from '@codemirror/language';
import { autocompletion } from '@codemirror/autocomplete';
import { linter } from '@codemirror/lint';
import { tags } from '@lezer/highlight';
import type { Extension, EditorState } from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import type { Completion } from '@codemirror/autocomplete';
import { NAMESPACES, VOCAB, allBuiltinTerms } from './ontology/vocabularies.js';
import { SHACL_CONSTRAINT_CARDS } from './shaclConstraints.js';

const NS: Record<string, string> = NAMESPACES;

// Turtle/N3 StreamLanguage tokenizer
export const turtleLanguage = StreamLanguage.define({
  name: 'turtle',

  startState() {
    return { inString: false, stringChar: null, tripleQuoted: false, inIri: false };
  },

  token(stream, state) {
    if (state.inIri) {
      if (stream.match(/[^>]*/)) {
        if (stream.eat('>')) { state.inIri = false; }
        return 'string';
      }
      stream.next();
      return 'string';
    }

    if (state.inString) {
      if (state.tripleQuoted) {
        const end = state.stringChar.repeat(3);
        if (stream.match(end)) {
          state.inString = false;
          state.tripleQuoted = false;
          return 'string2';
        }
        stream.next();
        return 'string2';
      }
      if (stream.match(state.stringChar)) {
        state.inString = false;
        return 'string2';
      }
      if (stream.eat('\\')) stream.next();
      else stream.next();
      return 'string2';
    }

    if (stream.eatSpace()) return null;

    // Comment
    if (stream.eat('#')) {
      stream.skipToEnd();
      return 'comment';
    }

    // Triple-quoted strings
    if (stream.match('"""')) {
      state.inString = true; state.stringChar = '"'; state.tripleQuoted = true;
      return 'string2';
    }
    if (stream.match("'''")) {
      state.inString = true; state.stringChar = "'"; state.tripleQuoted = true;
      return 'string2';
    }

    // String literals
    if (stream.eat('"')) {
      state.inString = true; state.stringChar = '"'; state.tripleQuoted = false;
      return 'string2';
    }
    if (stream.eat("'")) {
      state.inString = true; state.stringChar = "'"; state.tripleQuoted = false;
      return 'string2';
    }

    // IRI
    if (stream.eat('<')) {
      if (stream.peek() && stream.peek() !== ' ' && stream.peek() !== '=') {
        state.inIri = true;
        return 'string';
      }
      return 'operator';
    }

    // @prefix / @base directives
    if (stream.match(/@(prefix|base|forSome|forAll)/)) return 'keyword';

    // Numbers
    if (stream.match(/[+-]?[0-9]+(\.[0-9]*)?([eE][+-]?[0-9]+)?/)) return 'number';

    // Language tag
    if (stream.match(/@[a-zA-Z]+(-[a-zA-Z0-9]+)*/)) return 'meta';

    // ^^datatype
    if (stream.match('^^')) return 'operator';

    // Blank nodes
    if (stream.match(/_:[a-zA-Z0-9_-]+/)) return 'variableName';

    // Keywords: a, true, false
    if (stream.match(/\b(true|false)\b/)) return 'bool';
    if (stream.match(/\ba\b/)) return 'keyword';

    // Prefixed names and prefix declarations
    if (stream.match(/[a-zA-Z_][a-zA-Z0-9_-]*/)) {
      if (stream.peek() === ':') {
        stream.eat(':');
        stream.match(/[a-zA-Z0-9_\-.]*/);
        return 'namespace';
      }
      return 'variableName';
    }

    // Bare colon
    if (stream.eat(':')) {
      stream.match(/[a-zA-Z0-9_\-.]*/);
      return 'namespace';
    }

    if (stream.match(/[{}[\]().,;!^]/)) return 'operator';

    stream.next();
    return null;
  },

  languageData: {
    commentTokens: { line: '#' },
  },

  tokenTable: {
    string2: tags.regexp,
    namespace: tags.namespace,
  },
});

// ─── Prefix declarations ────────────────────────────────────────────────────

/**
 * Prefixes declared in a Turtle document. Turtle 1.1 accepts both `@prefix p:
 * <ns> .` and the SPARQL-style `PREFIX p: <ns>`, so both count — matching only
 * `@prefix` would make the completer re-declare a prefix that is already live.
 */
export function extractTurtlePrefixes(doc: string): Record<string, string> {
  const out: Record<string, string> = {};
  const re = /(?:^|\s)@?(?:prefix)\s+([a-zA-Z_][\w-]*)?\s*:\s*<([^>]*)>/gi;
  let m: RegExpExecArray | null;
  while ((m = re.exec(String(doc || '')))) out[m[1] || ''] = m[2];
  return out;
}

/**
 * Offset at which a new `@prefix` line belongs: after the document's existing
 * directive block, or below its header comment when there is none.
 */
export function prefixInsertOffset(doc: string): number {
  const text = String(doc || '');
  const isDirective = (line: string) => /^@?(?:prefix|base)\b/i.test(line);
  let offset = 0;
  let afterDirectives = -1;
  let afterHeader = 0;
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    const next = Math.min(offset + raw.length + 1, text.length);
    if (isDirective(line)) afterDirectives = next;
    else if (line === '' || line.startsWith('#')) { if (afterDirectives < 0) afterHeader = next; }
    else break;
    offset += raw.length + 1;
  }
  return afterDirectives >= 0 ? afterDirectives : afterHeader;
}

/** Namespaces a shapes author reaches for, seeded into every new shape graph. */
export const TEMPLATE_PREFIXES = ['sh', 'rdf', 'rdfs', 'xsd', 'owl', 'skos', 'dct', 'schema'];

/**
 * A correct, empty Turtle shapes graph. Emitted in Turtle's own `@prefix … .`
 * form: the SPARQL-style `PREFIX` header this used to produce is not Turtle, so
 * the first save from the visual builder silently rewrote the user's document.
 */
export function emptyShapesTemplate(exampleNs = 'http://example.org/'): string {
  const names = [...TEMPLATE_PREFIXES, 'ex'];
  const width = Math.max(...names.map((p) => p.length)) + 2;
  const decl = (p: string, ns: string) => `@prefix ${`${p}:`.padEnd(width)}<${ns}> .`;
  const lines = TEMPLATE_PREFIXES.map((p) => decl(p, NS[p]));
  lines.push(decl('ex', exampleNs));
  return `# SHACL shapes graph\n${lines.join('\n')}\n`;
}

// ─── Parse-error positions ──────────────────────────────────────────────────

/**
 * N3.js reports the offending line only inside the message text ("… on line
 * 7."); its structured `error.context` is gone by the time the shapes model
 * hands us a plain string, so the position is recovered from the text.
 */
export function parseTurtleErrorPosition(message: string | null | undefined): number | null {
  if (!message) return null;
  const m = /\bon line (\d+)/i.exec(String(message));
  if (!m) return null;
  const line = Number(m[1]);
  return Number.isFinite(line) && line > 0 ? line : null;
}

export interface TurtleErrorRange { from: number; to: number; line: number; message: string }

/**
 * Map a parse-error message onto a character range in `doc`. Falls back to the
 * first line when the message carries no position, and walks back over blank
 * lines so an "unexpected eof" reported past the last statement still lands on
 * content the user can see.
 */
export function turtleErrorRange(doc: string, message: string | null | undefined): TurtleErrorRange | null {
  if (!message) return null;
  const text = String(doc ?? '');
  const lines = text.split('\n');
  let idx = (parseTurtleErrorPosition(message) ?? 1) - 1;
  idx = Math.max(0, Math.min(idx, lines.length - 1));
  while (idx > 0 && lines[idx].trim() === '') idx--;

  let from = 0;
  for (let i = 0; i < idx; i++) from += lines[i].length + 1;
  const raw = lines[idx];
  const trimmed = raw.trim();
  const lead = trimmed ? raw.length - raw.trimStart().length : 0;
  return {
    from: from + lead,
    to: from + (trimmed ? lead + trimmed.length : raw.length),
    line: idx + 1,
    message: String(message),
  };
}

/**
 * Lint extension surfacing a Turtle parse error as a gutter marker and a
 * squiggle. Rebuild it through a compartment whenever the message changes.
 */
export function turtleDiagnostics(message: string | null | undefined): Extension {
  return linter((view) => {
    const hit = turtleErrorRange(view.state.doc.toString(), message);
    if (!hit) return [];
    return [{ from: hit.from, to: hit.to, severity: 'error' as const, source: 'turtle', message: hit.message }];
  }, { delay: 120 });
}

// ─── Folding ────────────────────────────────────────────────────────────────

/** Index just past the string literal starting at `i`. */
function skipString(text: string, i: number): number {
  const q = text[i];
  if (text.slice(i, i + 3) === q.repeat(3)) {
    const end = text.indexOf(q.repeat(3), i + 3);
    return end < 0 ? text.length : end + 3;
  }
  for (let j = i + 1; j < text.length; j++) {
    if (text[j] === '\\') { j++; continue; }
    if (text[j] === q || text[j] === '\n') return j + 1;
  }
  return text.length;
}

/**
 * Index just past a comment, string literal or `<IRI>` starting at `i`, or -1
 * when `i` is none of those. Every scan below skips these so their punctuation
 * cannot be mistaken for structure.
 */
function skipAtomic(text: string, i: number): number {
  const c = text[i];
  if (c === '#') { const nl = text.indexOf('\n', i); return nl < 0 ? text.length : nl; }
  if (c === '"' || c === "'") return skipString(text, i);
  if (c === '<') {
    const gt = text.indexOf('>', i + 1);
    const nl = text.indexOf('\n', i + 1);
    if (gt > 0 && (nl < 0 || gt < nl)) return gt + 1;
  }
  return -1;
}

/** Index of the first bracket opened in [from, to) and not closed there, else -1. */
function unclosedOpener(text: string, from: number, to: number): number {
  const stack: number[] = [];
  let i = from;
  while (i < to) {
    const skip = skipAtomic(text, i);
    if (skip >= 0) { i = Math.max(skip, i + 1); continue; }
    const c = text[i];
    if (c === '[' || c === '(') stack.push(i);
    else if (c === ']' || c === ')') stack.pop();
    i++;
  }
  return stack.length ? stack[0] : -1;
}

/** Index of the bracket closing the one at `openIdx`, or -1. */
function matchingClose(text: string, openIdx: number): number {
  let depth = 0;
  let i = openIdx;
  while (i < text.length) {
    const skip = skipAtomic(text, i);
    if (skip >= 0) { i = Math.max(skip, i + 1); continue; }
    const c = text[i];
    if (c === '[' || c === '(') depth++;
    else if (c === ']' || c === ')') { depth--; if (depth === 0) return i; }
    i++;
  }
  return -1;
}

/** Index just past the `.` terminating the statement that starts at `from`. */
function statementEnd(text: string, from: number): number {
  let depth = 0;
  let i = from;
  while (i < text.length) {
    const skip = skipAtomic(text, i);
    if (skip >= 0) { i = Math.max(skip, i + 1); continue; }
    const c = text[i];
    if (c === '[' || c === '(') depth++;
    else if (c === ']' || c === ')') depth--;
    else if (c === '.' && depth === 0 && (i + 1 >= text.length || /[\s#]/.test(text[i + 1]))) return i + 1;
    i++;
  }
  return text.length;
}

/**
 * Foldable range for the line spanning [lineFrom, lineTo): a bracket left open
 * on the line folds to its match, otherwise a statement starting flush at
 * column 0 folds to its terminating dot. StreamLanguage yields a flat syntax
 * tree, so CodeMirror's own fold service finds nothing here and this heuristic
 * stands in for it — hence the column-0 rule, which is both Turtle convention
 * and what the shapes serializer emits.
 */
export function turtleFoldRange(text: string, lineFrom: number, lineTo: number): { from: number; to: number } | null {
  const open = unclosedOpener(text, lineFrom, lineTo);
  if (open >= 0) {
    const close = matchingClose(text, open);
    if (close > lineTo) return { from: lineTo, to: close };
    if (close < 0) return null;
  }
  if (!/^[A-Za-z_<:]/.test(text.slice(lineFrom, lineTo))) return null;
  const end = statementEnd(text, lineFrom) - 1;
  return end > lineTo ? { from: lineTo, to: end } : null;
}

// foldGutter re-runs the service for every visible line on each viewport
// update; hold the stringified doc so a long shapes file is serialised once.
let foldDocKey: unknown = null;
let foldDocText = '';
function docText(state: EditorState): string {
  if (state.doc !== foldDocKey) {
    foldDocKey = state.doc;
    foldDocText = state.doc.toString();
  }
  return foldDocText;
}

/** Fold service backing `foldGutter()` in Turtle mode. */
export const turtleFolding: Extension = foldService.of((state, lineFrom, lineTo) =>
  turtleFoldRange(docText(state), lineFrom, lineTo));

// ─── Autocompletion ─────────────────────────────────────────────────────────

export interface TurtleCompletionOption {
  label: string;
  displayLabel?: string;
  detail?: string;
  info?: string;
  type?: string;
  boost?: number;
  /** Text inserted in place of `label`, when the two differ. */
  insert?: string;
  /** Namespace that has to be declared before `insert` resolves. */
  requiresPrefix?: { prefix: string; ns: string };
}

export interface TurtleCompletionResult {
  /** Characters immediately before the cursor that the completion replaces. */
  replaceLen: number;
  options: TurtleCompletionOption[];
  validFor?: RegExp;
}

const TURTLE_DIRECTIVES: TurtleCompletionOption[] = [
  { label: '@prefix', detail: 'directive', info: 'Bind a prefix to a namespace IRI.', type: 'keyword', boost: 20 },
  { label: '@base', detail: 'directive', info: 'Set the base IRI for relative references.', type: 'keyword', boost: 18 },
];

const TURTLE_KEYWORDS: TurtleCompletionOption[] = [
  { label: 'a', detail: 'rdf:type', info: 'Turtle shorthand for rdf:type.', type: 'keyword', boost: 10 },
  { label: 'true', type: 'constant', boost: 6 },
  { label: 'false', type: 'constant', boost: 6 },
];

// The constraint palette is the single catalogue of SHACL snippets; the
// completer offers the same templates the "+ Constraint" popover inserts.
const SNIPPETS: TurtleCompletionOption[] = SHACL_CONSTRAINT_CARDS.map((c) => ({
  label: c.label,
  detail: 'snippet',
  info: c.what,
  type: 'text',
  boost: -10,
  insert: c.template,
}));

const kindToCMType = (kind?: string): string => (kind === 'class' ? 'class' : kind ? 'property' : 'variable');

/**
 * Candidate generation for the Turtle/SHACL completer, kept free of CodeMirror
 * so it can be unit-tested. `before` is the current line up to the cursor.
 */
export function turtleCompletionsFor(
  doc: string,
  before: string,
  { explicit = false }: { explicit?: boolean } = {},
): TurtleCompletionResult | null {
  const text = String(doc || '');
  const declared = extractTurtlePrefixes(text);
  const known: Record<string, string> = { ...NS, ...declared };

  // --- A bare `@…` → the directives themselves, plus ready-made declarations.
  const atMatch = /^\s*@([a-zA-Z]*)$/.exec(before);
  if (atMatch) {
    const typed = atMatch[1].toLowerCase();
    const options: TurtleCompletionOption[] = [
      ...TURTLE_DIRECTIVES,
      ...Object.keys(NS)
        .filter((p) => !declared[p])
        .map((p) => ({
          label: `@prefix ${p}: <${NS[p]}> .`,
          displayLabel: `@prefix ${p}:`,
          detail: NS[p],
          type: 'namespace',
          boost: 4,
        })),
    ];
    return {
      replaceLen: typed.length + 1,
      options: options.filter((o) => !typed || o.label.slice(1).toLowerCase().startsWith(typed)),
      validFor: /^@[a-zA-Z]*$/,
    };
  }

  // --- Anywhere on a directive line: complete the namespace, not vocabulary.
  const directive = /^\s*@?(prefix|base)\b/i.exec(before);
  if (directive) {
    const iriOpen = before.lastIndexOf('<');
    if (iriOpen > before.lastIndexOf('>')) {
      const fragment = before.slice(iriOpen + 1).toLowerCase();
      const options = Object.keys(NS)
        .filter((p) => !fragment || NS[p].toLowerCase().includes(fragment))
        .map((p) => ({
          label: NS[p],
          displayLabel: `${p}: — ${NS[p]}`,
          detail: 'namespace',
          type: 'namespace',
          insert: `${NS[p]}>`,
        }));
      return { replaceLen: fragment.length, options, validFor: /^[^\s>]*$/ };
    }
    const named = /(?:^|\s)([a-zA-Z_][\w-]*)?:([^\S\n]*)$/.exec(before);
    if (named) {
      const ns = NS[named[1] || ''];
      if (!ns) return null;
      return {
        replaceLen: named[2].length,
        options: [{ label: `<${ns}> .`, detail: named[1] || 'default', type: 'namespace', insert: ` <${ns}> .`, boost: 30 }],
        validFor: /^$/,
      };
    }
    if (directive[1].toLowerCase() === 'base') return null;
    const typed = /([a-zA-Z_][\w-]*)?$/.exec(before)?.[1] || '';
    const options = Object.keys(NS)
      .filter((p) => !declared[p])
      .map((p) => ({ label: `${p}: <${NS[p]}> .`, displayLabel: `${p}:`, detail: NS[p], type: 'namespace' }));
    return options.length ? { replaceLen: typed.length, options, validFor: /^[\w-]*$/ } : null;
  }

  // --- Inside <…> → full IRI completion over the built-in vocabularies.
  const iriOpen = before.lastIndexOf('<');
  if (iriOpen > before.lastIndexOf('>') && !/[\s<]/.test(before.slice(iriOpen + 1))) {
    const fragment = before.slice(iriOpen + 1).toLowerCase();
    const options = allBuiltinTerms()
      .filter((t) => !fragment || t.iri.toLowerCase().includes(fragment))
      .slice(0, 200)
      .map((t) => ({
        label: t.iri,
        displayLabel: t.label ? `${t.label} · ${t.iri}` : t.iri,
        detail: t.kind || '',
        info: t.comment || '',
        insert: `${t.iri}>`,
        type: kindToCMType(t.kind),
      }));
    return { replaceLen: fragment.length, options, validFor: /^[^\s>]*$/ };
  }

  // --- `prefix:local` → vocabulary terms, declaring the prefix if it is new.
  // What may be offered as a prefixed name's local part: it has to survive
  // being pasted into the document, so no '/', '#' or ':'.
  const PN_LOCAL_OFFERABLE = /^[A-Za-z_][\w.-]*$/;
  const prefMatch = /(?:^|[\s;,([])([a-zA-Z_][\w-]*)?:([a-zA-Z_][\w-]*)?$/.exec(before);
  if (prefMatch) {
    const prefix = prefMatch[1] || '';
    const local = prefMatch[2] || '';
    const ns = known[prefix];
    if (ns) {
      // A prefix name is only an alias: the document may bind `schema:` to the
      // https form, or rebind `sh:` to its own namespace. VOCAB is keyed by the
      // built-in prefix name, so trusting it here sliced the built-in
      // namespace's length off a foreign IRI and offered — and inserted —
      // garbage locals ("hing" for "Thing"). Match on the namespace, and keep
      // only terms the document's own `ns` actually prefixes.
      const byName = NS[prefix] === ns ? VOCAB[prefix] : undefined;
      // The remainder also has to BE a local name. A document namespace that is
      // a proper prefix of a built-in one ("@prefix purl: <http://purl.org/>")
      // otherwise offers "dc/terms/title", which splices a '/' into the
      // document and makes it unparseable.
      const pool = (byName || allBuiltinTerms()).filter(
        (t) => t.iri.startsWith(ns) && PN_LOCAL_OFFERABLE.test(t.iri.slice(ns.length)),
      );
      const options: TurtleCompletionOption[] = pool.slice(0, 500).map((t) => {
        const term = t.iri.slice(ns.length);
        return {
          label: term,
          displayLabel: t.label && t.label !== term ? `${term} — ${t.label}` : term,
          detail: t.kind || prefix,
          info: t.comment || t.iri,
          type: kindToCMType(t.kind),
          ...(declared[prefix] === undefined ? { requiresPrefix: { prefix, ns } } : {}),
        };
      });
      // A known prefix with nothing to offer completes to NOTHING. Falling
      // through would reach the bare-word branch still carrying the local part's
      // replaceLen, so accepting a constraint snippet there would splice a
      // multi-line template into the middle of a CURIE.
      return options.length
        ? { replaceLen: local.length, options, validFor: /^[\w-]*$/ }
        : null;
    }
  }

  // --- Bare word → prefixes, Turtle keywords, directives, constraint snippets.
  const typed = /([A-Za-z_][\w-]*)?$/.exec(before)?.[1] || '';
  if (!typed && !explicit) return null;
  const prefixNames = [...new Set([...Object.keys(NS), ...Object.keys(declared)])].filter(Boolean);
  const options: TurtleCompletionOption[] = [
    ...TURTLE_KEYWORDS,
    ...TURTLE_DIRECTIVES,
    ...prefixNames.map((p) => ({
      label: `${p}:`,
      detail: known[p] || 'prefix',
      info: declared[p] !== undefined
        ? `@prefix ${p}: <${known[p]}> .`
        : `Inserts @prefix ${p}: <${known[p]}> .`,
      type: 'namespace',
      boost: declared[p] !== undefined ? 9 : 7,
      ...(declared[p] === undefined && known[p] ? { requiresPrefix: { prefix: p, ns: known[p] } } : {}),
    })),
    ...SNIPPETS,
  ];
  return { replaceLen: typed.length, options, validFor: /^[\w-]*$/ };
}

/** Insert an option's text, prepending an `@prefix` declaration when missing. */
function applyTurtle(option: TurtleCompletionOption) {
  return (view: EditorView, _c: Completion, from: number, to: number) => {
    const insert = option.insert ?? option.label;
    const doc = view.state.doc.toString();
    const changes = [{ from, to, insert }];
    let anchor = from + insert.length;
    const need = option.requiresPrefix;
    if (need && extractTurtlePrefixes(doc)[need.prefix] === undefined) {
      const decl = `@prefix ${need.prefix}: <${need.ns}> .\n`;
      const at = prefixInsertOffset(doc);
      changes.unshift({ from: at, to: at, insert: decl });
      if (at <= from) anchor += decl.length;
    }
    view.dispatch({ changes, selection: { anchor } });
  };
}

/**
 * CodeMirror autocompletion for Turtle shape graphs. Replaces the SPARQL
 * completer that used to serve this mode and only ever offered SELECT/WHERE and
 * `PREFIX` declarations — none of which are Turtle.
 */
export function turtleAutocomplete(): Extension {
  return autocompletion({
    override: [(context) => {
      const doc = context.state.doc.toString();
      const line = context.state.doc.lineAt(context.pos);
      const before = doc.slice(line.from, context.pos);
      const result = turtleCompletionsFor(doc, before, { explicit: context.explicit });
      if (!result || !result.options.length) return null;
      return {
        from: context.pos - result.replaceLen,
        to: context.pos,
        validFor: result.validFor,
        options: result.options.map((o) => ({
          label: o.label,
          displayLabel: o.displayLabel,
          detail: o.detail,
          info: o.info,
          type: o.type,
          boost: o.boost,
          ...(o.insert || o.requiresPrefix ? { apply: applyTurtle(o) } : {}),
        })),
      };
    }],
  });
}
