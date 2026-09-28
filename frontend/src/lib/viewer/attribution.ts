/**
 * Data credits the viewers show beside licensed third-party content.
 *
 * The demo's real city block is a 3DBAG excerpt, licensed CC BY 4.0. 3DBAG
 * fixes the credit wording, requires digital media to link its copyright page,
 * and wants the credit in the bottom-right corner of a browsable map
 * (https://docs.3dbag.nl/en/copyright/). CC BY 4.0 §3(a)(1) adds the licence
 * URI and a note that the material was modified. The wording is the rights
 * holder's, so it is fixed text, never translated.
 *
 * Pure and DOM-free, so it's testable. The HTML it builds goes to MapLibre's
 * attribution control and to Cesium credits, both of which render HTML.
 */

export interface DataCredit {
  /** The credit line, worded exactly as the rights holder requires. */
  text: string;
  /** The rights holder's copyright page, which the credit links to. */
  url?: string;
  license?: string;
  licenseUrl?: string;
  /** The shown content is an adaptation (CC BY 4.0 §3(a)(1)(B)). */
  modified?: boolean;
}

export const THREEDBAG_CREDIT: DataCredit = {
  text: '© 3DBAG by tudelft3d and 3DGI',
  url: 'https://docs.3dbag.nl/en/copyright/',
  license: 'CC BY 4.0',
  licenseUrl: 'https://creativecommons.org/licenses/by/4.0/',
  // The bundled excerpt is pruned and re-levelled, and every viewer meshes or
  // reprojects it further.
  modified: true,
};

const ESCAPES: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
const escapeHtml = (s: string) => s.replace(/[&<>"']/g, (ch) => ESCAPES[ch]);
/** Only absolute http(s) links make it into a credit. */
const httpUrl = (u: unknown) => (typeof u === 'string' && /^https?:\/\//i.test(u) ? u : '');

function link(href: string | undefined, label: string): string {
  const safe = httpUrl(href);
  return safe
    ? `<a href="${escapeHtml(safe)}" target="_blank" rel="noopener noreferrer">${escapeHtml(label)}</a>`
    : escapeHtml(label);
}

/** One credit as static HTML: the linked credit line, then licence and "modified". */
export function creditHtml(credit: DataCredit): string {
  const notes = [credit.license ? link(credit.licenseUrl, credit.license) : '', credit.modified ? 'modified' : ''];
  const tail = notes.filter(Boolean).join(', ');
  return tail ? `${link(credit.url, credit.text)} (${tail})` : link(credit.url, credit.text);
}

/** A link to 3DBAG data: their own hosts, or a bundled `…3dbag…` file. */
export function is3dbagUrl(url: string | null | undefined): boolean {
  return /3dbag/i.test(url || '');
}

/**
 * Does any viewer-feed element or model ref show 3DBAG content? Feed elements
 * carry their model links in `files` ([key, url] pairs); Model3D refs in `url`.
 */
export function carries3dbag(items: { files?: [string, string][]; url?: string }[] | null | undefined): boolean {
  return (items || []).some(
    (it) => is3dbagUrl(it?.url) || (it?.files || []).some(([, url]) => is3dbagUrl(url)),
  );
}

/** Extra map attribution HTML for a set of feed elements ('' when none is owed). */
export function mapAttributionFor(elements: { files?: [string, string][] }[] | null | undefined): string {
  return carries3dbag(elements) ? creditHtml(THREEDBAG_CREDIT) : '';
}

/**
 * Credits a served 3D Tiles tileset declares. 3D Tiles 1.1 has no copyright
 * member, so the server puts them in `asset.extras.credits` (src/tiles3d).
 */
export function tilesetCredits(asset: unknown): DataCredit[] {
  const list = (asset as { extras?: { credits?: unknown } } | null | undefined)?.extras?.credits;
  if (!Array.isArray(list)) return [];
  return list
    .filter((c) => c && typeof c.text === 'string' && c.text.trim())
    .map((c) => ({
      text: c.text,
      url: httpUrl(c.url) || undefined,
      license: typeof c.license === 'string' ? c.license : undefined,
      licenseUrl: httpUrl(c.licenseUrl) || undefined,
      modified: c.modified === true,
    }));
}
