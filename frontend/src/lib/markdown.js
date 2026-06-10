// Markdown rendering for the in-app documentation (`/api/docs` bodies).
//
// Parses CommonMark/GFM with `marked`, sanitizes the result with DOMPurify,
// then post-processes it to:
//   1. give every heading a stable slug `id` so the "on this page" rail can
//      scroll-spy and deep-link to it (#anchor), and
//   2. rewrite the repo-relative `*.md` links the built-in docs use (e.g.
//      `dcat.md#frag`) into in-app `/docs/<slug>` routes.
// Returns the clean HTML plus a flat heading list (the table of contents).

import { marked } from 'marked';
import DOMPurify from 'dompurify';
import { highlightRdf, highlightJson, highlightXml } from './resultHighlight.js';

marked.setOptions({ gfm: true, breaks: false });

// SPARQL keywords the (Turtle-oriented) RDF highlighter should also flag.
const SPARQL_KEYWORDS = new Set([
  'SELECT', 'CONSTRUCT', 'ASK', 'DESCRIBE', 'WHERE', 'FROM', 'NAMED', 'PREFIX',
  'BASE', 'OPTIONAL', 'UNION', 'MINUS', 'GRAPH', 'SERVICE', 'FILTER', 'BIND',
  'VALUES', 'UNDEF', 'ORDER', 'BY', 'GROUP', 'HAVING', 'LIMIT', 'OFFSET',
  'DISTINCT', 'REDUCED', 'AS', 'ASC', 'DESC', 'INSERT', 'DELETE', 'DATA',
  'WITH', 'USING', 'CLEAR', 'DROP', 'CREATE', 'LOAD', 'COPY', 'MOVE', 'ADD',
  'INTO', 'SILENT', 'DEFAULT', 'ALL', 'TO', 'NOT', 'EXISTS', 'IN', 'TRUE', 'FALSE',
]);

// Map a fenced-code language tag to a highlighter, or null to leave it plain.
function highlightFor(lang) {
  switch (lang) {
    case 'turtle': case 'ttl': case 'trig': case 'nt': case 'ntriples':
    case 'n-triples': case 'nq': case 'nquads': case 'n-quads': case 'rdf':
    case 'shacl': case 'shaclc':
      return (src) => highlightRdf(src);
    case 'sparql': case 'rq':
      return (src) => highlightRdf(src, SPARQL_KEYWORDS);
    case 'json': case 'jsonld': case 'json-ld':
      return highlightJson;
    case 'xml': case 'rdfxml': case 'rdf-xml': case 'owl':
      return highlightXml;
    default:
      return null;
  }
}

/** Highlight a SPARQL string (the RDF highlighter plus SPARQL keywords). Shared
 * with the chat's runnable query blocks so they match fenced ```sparql code. */
export function highlightSparql(src) {
  return highlightRdf(src, SPARQL_KEYWORDS);
}

/** Turn heading text into a URL-safe slug used as an `id` / hash anchor. */
export function slugify(text) {
  return String(text || '')
    .toLowerCase()
    .replace(/[^\w\s-]/g, '') // drop punctuation
    .trim()
    .replace(/[\s_]+/g, '-') // spaces / underscores → hyphen
    .replace(/-+/g, '-') // collapse repeats
    .replace(/^-+|-+$/g, ''); // trim leading / trailing hyphens
}

// A repo-relative doc link (`shacl.md`, `./dcat.md#frag`, `sub/x.md`) becomes
// `/docs/<basename>`. Absolute URLs, root-absolute paths, schemes (mailto:,
// http:) and bare `#fragment` links are left untouched.
function rewriteDocHref(href) {
  if (!href) return href;
  if (/^([a-z][a-z0-9+.-]*:|\/\/|\/|#)/i.test(href)) return href;
  const m = href.match(/^(?:\.\/)?(?:.*\/)?([^/]+?)\.md(#.*)?$/i);
  return m ? `/docs/${m[1]}${m[2] || ''}` : href;
}

/**
 * Render a markdown string to sanitized HTML and a heading TOC.
 * @param {string} md - markdown source
 * @param {{ breaks?: boolean }} [opts] - `breaks: true` renders a single newline as
 *   a line break (chat-friendly); the default (false) is standard markdown for docs.
 * @returns {{ html: string, headings: Array<{ id: string, text: string, level: number }> }}
 */
export function renderMarkdown(md, opts = {}) {
  const source = String(md || '');
  if (!source.trim()) return { html: '', headings: [] };

  const dirty = marked.parse(source, { breaks: !!opts.breaks });
  const clean = DOMPurify.sanitize(dirty, { USE_PROFILES: { html: true }, ADD_ATTR: ['target'] });

  // Outside a DOM (SSR / non-jsdom): return sanitized HTML without a TOC.
  if (typeof DOMParser === 'undefined') return { html: clean, headings: [] };

  const doc = new DOMParser().parseFromString(clean, 'text/html');
  const headings = [];
  const used = new Map();

  doc.querySelectorAll('h1, h2, h3, h4').forEach((el) => {
    const text = (el.textContent || '').trim();
    let id = slugify(text);
    if (!id) return;
    const seen = used.get(id) || 0;
    used.set(id, seen + 1);
    if (seen > 0) id = `${id}-${seen}`;
    el.id = id;
    headings.push({ id, text, level: Number(el.tagName[1]) });
  });

  // Make built-in docs' relative `.md` links navigate in-app, and harden any
  // link that opens a new tab.
  doc.querySelectorAll('a[href]').forEach((a) => {
    const href = a.getAttribute('href');
    const rewritten = rewriteDocHref(href);
    if (rewritten !== href) a.setAttribute('href', rewritten);
    if (a.getAttribute('target') === '_blank') a.setAttribute('rel', 'noopener noreferrer');
  });

  // Syntax-highlight fenced code blocks for known RDF/SPARQL/JSON/XML languages.
  // The highlighters escape every source character and emit only safe
  // <span class="tok-*"> wrappers (see resultHighlight.js), so injecting their
  // output here is safe.
  doc.querySelectorAll('pre > code[class]').forEach((code) => {
    const m = /(?:language|lang)-([\w-]+)/.exec(code.className);
    const fn = m && highlightFor(m[1].toLowerCase());
    if (fn) code.innerHTML = fn(code.textContent || '');
  });

  return { html: doc.body.innerHTML, headings };
}
