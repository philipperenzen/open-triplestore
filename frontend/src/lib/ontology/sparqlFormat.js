// A small, dependency-free, idempotent SPARQL pretty-printer.
//
// Why not sparqljs Parser->Generator? That round-trip throws on any query that
// doesn't fully parse (an undeclared prefix, a half-typed pattern -- i.e. the
// normal state of a query you're editing) and it silently discards comments and
// rewrites expressions. A formatter you press mid-edit must never do either.
//
// This formatter only normalises *layout*: indentation and line breaks driven by
// block braces `{}` and the statement punctuation `. ; ,`. Token text -- comments,
// strings, IRIs, property paths, expressions -- is preserved verbatim, so it never
// changes a query's meaning and is idempotent: format(format(x)) === format(x).
//
// MIRRORED FILE: keep in sync with any companion app's SPARQL formatter.

const INDENT = '  ';
// IRIREF: '<' then any non-space, non-angle chars, then '>'. Loose vs the full
// SPARQL grammar but exact enough to tell an IRI from a `<` comparison operator
// (which is always followed by whitespace or '=') while keeping '-', '#', ':' etc.
const IRIREF = /^<[^\s<>]*>/;
const NUMBER = /^(?:\d+\.\d+|\.\d+|\d+)(?:[eE][+-]?\d+)?/;
// PN_CHARS-ish: enough to recognise prefixed-name / variable characters so we can
// tell a decimal point or a dotted local name from a statement terminator.
const isNameChar = (c) => !!c && (/[A-Za-z0-9_·À-￿]/.test(c) || c === '-');

function tokenize(src) {
  const toks = [];
  const n = src.length;
  let i = 0;
  let sp = false; // whitespace seen since the previous token

  const pushAtom = (v) => { toks.push({ t: 'atom', v, sp }); sp = false; };

  while (i < n) {
    const c = src[i];

    if (c === ' ' || c === '\t' || c === '\r' || c === '\n') { sp = true; i++; continue; }

    if (c === '#') {
      let j = i + 1;
      while (j < n && src[j] !== '\n') j++;
      toks.push({ t: 'comment', v: src.slice(i, j).replace(/\s+$/, ''), sp });
      sp = false; i = j; continue;
    }

    if (src.startsWith('"""', i) || src.startsWith("'''", i)) {
      const q = src.slice(i, i + 3);
      let j = i + 3;
      while (j < n && !src.startsWith(q, j)) { if (src[j] === '\\') j++; j++; }
      j = Math.min(n, j + 3);
      pushAtom(src.slice(i, j)); i = j; continue;
    }

    if (c === '"' || c === "'") {
      let j = i + 1;
      while (j < n && src[j] !== c && src[j] !== '\n') { if (src[j] === '\\') j++; j++; }
      j = Math.min(n, j + 1);
      pushAtom(src.slice(i, j)); i = j; continue;
    }

    if (c === '<') {
      const m = IRIREF.exec(src.slice(i));
      if (m) { pushAtom(m[0]); i += m[0].length; continue; }
    }

    if (c === '.' || (c >= '0' && c <= '9')) {
      const m = NUMBER.exec(src.slice(i));
      if (m && (c !== '.' || /\d/.test(src[i + 1] ?? ''))) { pushAtom(m[0]); i += m[0].length; continue; }
    }

    if (c === '{') { toks.push({ t: 'lbrace' }); sp = false; i++; continue; }
    if (c === '}') { toks.push({ t: 'rbrace' }); sp = false; i++; continue; }
    if (c === ';') { toks.push({ t: 'semi' }); sp = false; i++; continue; }
    if (c === ',') { toks.push({ t: 'comma' }); sp = false; i++; continue; }

    if (c === '.') {
      // Glued between name chars => part of a dotted local name (ex:a.b), not a terminator.
      if (!sp && isNameChar(src[i - 1]) && isNameChar(src[i + 1])) {
        const last = toks[toks.length - 1];
        if (last && last.t === 'atom') { last.v += '.'; sp = false; i++; continue; }
      }
      toks.push({ t: 'dot' }); sp = false; i++; continue;
    }

    // Fallback: a run of non-space, non-structural chars. Keeps prefixed names,
    // variables, operators and property paths (a/b, ^a, a|b, a+) intact.
    let j = i;
    while (j < n) {
      const d = src[j];
      if (/\s/.test(d) || d === '#' || d === '"' || d === "'") break;
      if (d === '{' || d === '}' || d === ';' || d === ',') break;
      if (d === '.' && !(isNameChar(src[j - 1]) && isNameChar(src[j + 1]))) break;
      if (d === '<' && IRIREF.test(src.slice(j))) break;
      j++;
    }
    if (j === i) j++;
    pushAtom(src.slice(i, j));
    i = j;
  }
  return toks;
}

const isDirectiveLine = (s) => /^(PREFIX|BASE)\b/i.test(s);
const isIri = (s) => /^<[^\s<>]*>$/.test(s);

export function formatSparql(input) {
  if (!input || !input.trim()) return input ?? '';
  try {
    const toks = tokenize(input);
    const out = [];
    let line = '';
    let lineIndent = '';
    let depth = 0;       // brace-block nesting
    let cont = 0;        // predicate-object continuation after ';' (reset by '.')
    let forceSpace = false;
    let prevDirective = false;

    const indentFor = () => INDENT.repeat(Math.max(0, depth + cont));
    const pushLine = (text, directive = false) => {
      if (prevDirective && !directive && text !== '') out.push('');
      out.push(text);
      prevDirective = directive;
    };
    const flush = () => {
      if (!line.length) return;
      pushLine(lineIndent + line, isDirectiveLine(line));
      line = '';
    };
    // Capture a line's indent when its first token lands, not when it flushes:
    // a trailing ';' or '.' changes `cont` for the *next* line, not this one.
    const append = (s, space) => {
      if (!line.length) { lineIndent = indentFor(); line = s; }
      else line += (space ? ' ' : '') + s;
    };

    for (const tk of toks) {
      switch (tk.t) {
        case 'lbrace':
          append('{', line.length > 0); flush(); depth++; cont = 0; forceSpace = false; break;
        case 'rbrace':
          flush(); depth = Math.max(0, depth - 1); cont = 0; forceSpace = false;
          pushLine(INDENT.repeat(depth) + '}'); break;
        case 'dot':
          append('.', line.length > 0); flush(); cont = 0; forceSpace = false; break;
        case 'semi':
          append(';', line.length > 0); flush(); cont = 1; forceSpace = false; break;
        case 'comma':
          append(',', false); forceSpace = true; break;
        case 'comment':
          if (line.length) { line += ' ' + tk.v; flush(); }
          else pushLine(indentFor() + tk.v);
          forceSpace = false;
          break;
        case 'atom':
          append(tk.v, line.length > 0 && (tk.sp || forceSpace));
          forceSpace = false;
          // A PREFIX/BASE directive ends at its IRI -> break the line there.
          if (isIri(tk.v) && isDirectiveLine(line)) flush();
          break;
      }
    }
    flush();
    return out.join('\n').replace(/\n{3,}/g, '\n\n').replace(/\s+$/, '') + '\n';
  } catch {
    return input;
  }
}
