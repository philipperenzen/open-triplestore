// Dependency-free pretty-printing + syntax highlighting for SPARQL result bodies
// (JSON, XML, RDF/Turtle/N-Triples). Returns HTML where every piece of source
// text is HTML-escaped and only safe <span class="tok-*"> wrappers are added, so
// the output is safe to render with {@html} even though result bodies are
// attacker-influenced (a literal could contain "<script>").

function esc(s) {
  return String(s ?? '').replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c]));
}
const span = (cls, text) => `<span class="tok-${cls}">${esc(text)}</span>`;

// ── Pretty-printers ──────────────────────────────────────────────────────────

export function prettyJson(raw) {
  try { return JSON.stringify(JSON.parse(raw), null, 2); } catch { return raw; }
}

// Re-indent XML by inserting line breaks between tags. Layout only — never
// rewrites tag/attribute text, so it can't change the document's meaning.
export function prettyXml(raw) {
  const src = String(raw ?? '');
  if (!src.trim()) return src;
  // Already multi-line and indented? Leave it alone (idempotent-ish).
  const withBreaks = src.replace(/>\s*</g, '>\n<').trim();
  const lines = withBreaks.split('\n');
  const out = [];
  let depth = 0;
  for (let line of lines) {
    line = line.trim();
    if (!line) continue;
    const isClosing = /^<\//.test(line);
    const isSelfContained = /^<[^!?][^>]*\/>\s*$/.test(line) // <x/>
      || /^<([\w:.-]+)(\s[^>]*)?>.*<\/\1>\s*$/.test(line)    // <x>text</x>
      || /^<[!?]/.test(line);                                 // <!-- --> / <?xml?>
    if (isClosing) depth = Math.max(0, depth - 1);
    out.push('  '.repeat(depth) + line);
    const isOpening = /^<[^/!?][^>]*[^/]>$/.test(line) || /^<[^/!?][^>]*[^/?]>$/.test(line);
    if (isOpening && !isClosing && !isSelfContained) depth++;
  }
  return out.join('\n');
}

// ── Highlighters (input is treated as raw text; all text is escaped) ─────────

export function highlightJson(src) {
  const s = String(src ?? '');
  let out = '';
  let i = 0;
  const n = s.length;
  const isWs = (c) => c === ' ' || c === '\t' || c === '\n' || c === '\r';
  while (i < n) {
    const c = s[i];
    if (isWs(c)) { out += esc(c); i++; continue; }
    if (c === '"') {
      let j = i + 1;
      while (j < n) { if (s[j] === '\\') j += 2; else if (s[j] === '"') { j++; break; } else j++; }
      const text = s.slice(i, j);
      // A string immediately followed by ':' is a key.
      let k = j; while (k < n && isWs(s[k])) k++;
      out += span(s[k] === ':' ? 'key' : 'str', text);
      i = j; continue;
    }
    if (c === '{' || c === '}' || c === '[' || c === ']' || c === ',' || c === ':') {
      out += span('punct', c); i++; continue;
    }
    if (c === '-' || (c >= '0' && c <= '9')) {
      const m = /^-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?/.exec(s.slice(i));
      if (m) { out += span('num', m[0]); i += m[0].length; continue; }
    }
    const kw = /^(?:true|false|null)\b/.exec(s.slice(i));
    if (kw) { out += span('kw', kw[0]); i += kw[0].length; continue; }
    out += esc(c); i++;
  }
  return out;
}

export function highlightXml(src) {
  const s = String(src ?? '');
  let out = '';
  let i = 0;
  const n = s.length;
  while (i < n) {
    if (s[i] === '<') {
      // Comment / CDATA / processing instruction / doctype.
      if (s.startsWith('<!--', i)) {
        const end = s.indexOf('-->', i); const j = end < 0 ? n : end + 3;
        out += span('comment', s.slice(i, j)); i = j; continue;
      }
      if (s.startsWith('<![CDATA[', i)) {
        const end = s.indexOf(']]>', i); const j = end < 0 ? n : end + 3;
        out += span('comment', s.slice(i, j)); i = j; continue;
      }
      if (s[i + 1] === '?' || s[i + 1] === '!') {
        const end = s.indexOf('>', i); const j = end < 0 ? n : end + 1;
        out += span('meta', s.slice(i, j)); i = j; continue;
      }
      const end = s.indexOf('>', i);
      const j = end < 0 ? n : end + 1;
      out += highlightTag(s.slice(i, j));
      i = j; continue;
    }
    // Text node up to the next tag.
    const next = s.indexOf('<', i);
    const j = next < 0 ? n : next;
    out += esc(s.slice(i, j));
    i = j;
  }
  return out;
}

function highlightTag(tag) {
  // tag includes the angle brackets, e.g. <ns:foo a="b"> or </foo> or <x/>
  const m = /^(<\/?)([\w:.-]+)([\s\S]*?)(\/?>)$/.exec(tag);
  if (!m) return esc(tag);
  const [, open, name, attrs, close] = m;
  let out = span('punct', open) + span('tag', name);
  // Highlight attribute name="value" pairs; escape everything else.
  let rest = attrs;
  const attrRe = /([\w:.-]+)(\s*=\s*)("[^"]*"|'[^']*')|(\s+)|([^\s]+)/g;
  let am;
  while ((am = attrRe.exec(rest))) {
    if (am[1]) out += span('attr', am[1]) + esc(am[2]) + span('str', am[3]);
    else if (am[4]) out += esc(am[4]);
    else out += esc(am[5]);
  }
  out += span('punct', close);
  return out;
}

// Turtle / N-Triples / TriG / SPARQL. Line/token oriented; safe for partial
// documents. Pass `keywords` (an upper-cased Set) to also flag SPARQL keywords.
export function highlightRdf(src, keywords) {
  const s = String(src ?? '');
  let out = '';
  let i = 0;
  const n = s.length;
  const isWs = (c) => c === ' ' || c === '\t' || c === '\n' || c === '\r';
  while (i < n) {
    const c = s[i];
    if (isWs(c)) { out += esc(c); i++; continue; }
    if (c === '#') {
      let j = i; while (j < n && s[j] !== '\n') j++;
      out += span('comment', s.slice(i, j)); i = j; continue;
    }
    if (c === '<') {
      const m = /^<[^\s<>]*>/.exec(s.slice(i));
      if (m) { out += span('iri', m[0]); i += m[0].length; continue; }
    }
    if (c === '"' || c === "'") {
      // Triple-quoted or single-quoted literal.
      const triple = s.startsWith(c.repeat(3), i);
      const q = triple ? c.repeat(3) : c;
      let j = i + q.length;
      while (j < n && !s.startsWith(q, j)) { if (s[j] === '\\') j++; j++; }
      j = Math.min(n, j + q.length);
      // Include a trailing language tag or ^^datatype if present.
      out += span('str', s.slice(i, j)); i = j; continue;
    }
    if (c === '@') {
      const m = /^@[\w-]+/.exec(s.slice(i));
      if (m) { out += span('kw', m[0]); i += m[0].length; continue; }
    }
    if (c === '.' || c === ';' || c === ',' || c === '(' || c === ')' ||
        c === '[' || c === ']' || c === '{' || c === '}') {
      out += span('punct', c); i++; continue;
    }
    // PREFIX/BASE keywords, the bare predicate `a`, numbers, prefixed names,
    // and (when `keywords` is supplied) SPARQL keywords.
    const word = /^[^\s<>"'#.;,()[\]{}]+/.exec(s.slice(i));
    if (word) {
      const w = word[0];
      if (/^(?:@?prefix|@?base|PREFIX|BASE)$/i.test(w)) out += span('kw', w);
      else if (w === 'a') out += span('kw', w);
      else if (/^[+-]?\d/.test(w)) out += span('num', w);
      else if (/^[\w-]*:/.test(w)) out += span('pname', w);
      else if (keywords && keywords.has(w.toUpperCase())) out += span('kw', w);
      else out += esc(w);
      i += w.length; continue;
    }
    out += esc(c); i++;
  }
  return out;
}

// Dispatch by syntax family. `lang` is one of 'json' | 'xml' | 'rdf' | 'text'.
export function highlight(lang, src) {
  switch (lang) {
    case 'json': return highlightJson(src);
    case 'xml': return highlightXml(src);
    case 'rdf': return highlightRdf(src);
    default: return esc(src);
  }
}
