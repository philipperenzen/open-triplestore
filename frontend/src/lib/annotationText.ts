/**
 * Parser for vocabulary annotation prose (rdfs:comment, skos:definition,
 * dct:description …).
 *
 * Published vocabularies routinely put light markup in these literals — most
 * visibly schema.org, whose comments look like:
 *
 *   "An offer to transfer some rights to an item…\n\nNote: As the
 *    [[businessFunction]] property, which identifies the form of offer…,
 *    defaults to http://purl.org/goodrelations/v1#Sell; …\n\nFor
 *    [GTIN](http://www.gs1.org/barcodes/technical/idkeys/gtin)-related fields,
 *    see [Check Digit calculator](http://www.gs1.org/…) …"
 *
 * Rendered as raw text that's an unreadable wall with visible brackets. This
 * turns it into paragraphs of typed segments the UI can render as real links:
 *
 *   - `\n\n` (or a literal backslash-n-n, which some exports emit) → paragraphs
 *   - `[label](url)`          → external link
 *   - bare `http(s)://…`      → external link
 *   - `[[termName]]`          → link to a sibling term in the same vocabulary
 *
 * Deliberately NOT a markdown implementation: no emphasis, lists or headings.
 * Vocabulary annotations use links and paragraphs, and anything more would
 * mangle prose that contains stray asterisks or underscores.
 */

export type AnnotationSegment =
  | { kind: 'text'; text: string }
  /** An absolute http(s) link, either markdown-style or a bare URL. */
  | { kind: 'link'; text: string; href: string }
  /** A `[[name]]` reference to another term in the same vocabulary. */
  | { kind: 'term'; text: string; name: string };

/** One paragraph = an ordered run of segments. */
export type AnnotationParagraph = AnnotationSegment[];

// [[term]] | [label](url) | bare url
const TOKEN =
  /\[\[([^\]|]+?)(?:\|([^\]]+?))?\]\]|\[([^\]]*?)\]\((https?:\/\/[^)\s]+)\)|(https?:\/\/[^\s<>"']+)/g;

/** Trailing punctuation that is almost always sentence punctuation, not URL. */
function trimUrlTail(url: string): { href: string; tail: string } {
  let end = url.length;
  while (end > 0) {
    const ch = url[end - 1];
    if ('.,;:!?'.includes(ch)) {
      end--;
      continue;
    }
    // A ')' only belongs to the URL if it closes a '(' inside it.
    if (ch === ')') {
      const slice = url.slice(0, end);
      const opens = (slice.match(/\(/g) || []).length;
      const closes = (slice.match(/\)/g) || []).length;
      if (closes > opens) {
        end--;
        continue;
      }
    }
    break;
  }
  return { href: url.slice(0, end), tail: url.slice(end) };
}

/** Normalise line endings, including the escaped `\n` some exports emit. */
function normalise(raw: string): string {
  return raw
    .replace(/\r\n?/g, '\n')
    .replace(/\\r\\n|\\n/g, '\n')
    .trim();
}

function parseParagraph(text: string): AnnotationParagraph {
  const out: AnnotationParagraph = [];
  let last = 0;
  let m: RegExpExecArray | null;
  TOKEN.lastIndex = 0;

  const pushText = (s: string) => {
    if (!s) return;
    const prev = out[out.length - 1];
    if (prev && prev.kind === 'text') prev.text += s;
    else out.push({ kind: 'text', text: s });
  };

  while ((m = TOKEN.exec(text)) !== null) {
    pushText(text.slice(last, m.index));
    last = m.index + m[0].length;

    if (m[1] !== undefined) {
      // [[name]] or [[name|label]]
      const name = m[1].trim();
      out.push({ kind: 'term', text: (m[2] || name).trim(), name });
    } else if (m[4] !== undefined) {
      // [label](url)
      out.push({ kind: 'link', text: m[3] || m[4], href: m[4] });
    } else if (m[5] !== undefined) {
      // bare url — sentence punctuation stays as text
      const { href, tail } = trimUrlTail(m[5]);
      if (href) out.push({ kind: 'link', text: href, href });
      pushText(tail);
    }
  }
  pushText(text.slice(last));
  return out;
}

/**
 * Split annotation prose into paragraphs of segments.
 * Returns `[]` for empty/blank input.
 */
export function parseAnnotation(raw: string | null | undefined): AnnotationParagraph[] {
  const text = normalise(raw || '');
  if (!text) return [];
  return text
    .split(/\n{2,}/)
    .map((p) => p.trim())
    .filter(Boolean)
    .map(parseParagraph);
}

/** True when `raw` contains markup worth rendering (links or term refs). */
export function hasAnnotationMarkup(raw: string | null | undefined): boolean {
  return parseAnnotation(raw).some((p) => p.some((s) => s.kind !== 'text'));
}

/**
 * Resolve a `[[name]]` reference to an absolute IRI using the namespace of the
 * term the annotation belongs to — schema.org's `[[businessFunction]]` inside
 * `https://schema.org/Offer` means `https://schema.org/businessFunction`.
 * Returns '' when there's no usable base.
 */
export function resolveTermRef(name: string, baseIri: string | null | undefined): string {
  if (!name || !baseIri) return '';
  const hash = baseIri.lastIndexOf('#');
  const slash = baseIri.lastIndexOf('/');
  const cut = Math.max(hash, slash);
  if (cut < 0) return '';
  return baseIri.slice(0, cut + 1) + name;
}
