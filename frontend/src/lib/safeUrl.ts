/**
 * Safe-URL helpers for rendering attacker-controlled values as links.
 *
 * RDF object IRIs and DCAT/org metadata (landing_page, license, spatial,
 * contact_url, homepage…) are stored verbatim — there is no scheme allowlist on
 * RDF ingest — and then served to other, possibly anonymous, users on public
 * datasets. A value such as `javascript:alert(document.cookie)` therefore
 * round-trips straight into an `<a href>`. These helpers gate every such href to
 * a small allowlist so an unsafe scheme renders as an inert anchor (no `href`)
 * instead of executing.
 *
 * Usage: `href={safeExternalUrl(iri)}` for links, `src={safeImageUrl(iri)}` for
 * `<img>` / resource loads. When the value is unsafe the helper returns
 * `undefined`, and Svelte then omits the attribute — the surrounding text still
 * renders, just not as a working link.
 */

// Schemes allowed in an <a href>. `mailto:` covers contact links; relative
// references (which carry no scheme of their own) resolve against the current
// document's http(s) origin and so pass through.
const LINK_SCHEMES = new Set(['http:', 'https:', 'mailto:']);

// <img src> / resource loads have to fetch over the network, so only the web
// schemes are valid here — `mailto:` is meaningless as an image source.
const IMAGE_SCHEMES = new Set(['http:', 'https:']);

function safeUrl(value: string | null | undefined, allowed: Set<string>): string | undefined {
  if (value == null) return undefined;
  const url = String(value).trim();
  if (!url) return undefined;
  try {
    // Resolve against the page so a relative URL inherits its safe http(s)
    // scheme; an absolute URL keeps its own (the base is then ignored). The URL
    // parser also strips embedded tabs/newlines, defeating `java\tscript:` style
    // obfuscation, and lower-cases the scheme so the allowlist check is robust.
    const { protocol } = new URL(url, window.location.href);
    return allowed.has(protocol) ? url : undefined;
  } catch {
    // Malformed input (a bare `:`, stray control bytes, …) is not safe.
    return undefined;
  }
}

/**
 * Returns `value` if it is an http/https/mailto URL or a relative reference,
 * otherwise `undefined`. Use for `<a href>` bound to RDF IRIs or DCAT URL
 * metadata so `javascript:`, `data:`, `vbscript:`, `file:` … never reach href.
 */
export function safeExternalUrl(value: string | null | undefined): string | undefined {
  return safeUrl(value, LINK_SCHEMES);
}

/**
 * Like {@link safeExternalUrl} but allows only http/https — for `<img src>` and
 * other resource loads, where `mailto:` makes no sense and a real fetch is
 * required.
 */
export function safeImageUrl(value: string | null | undefined): string | undefined {
  return safeUrl(value, IMAGE_SCHEMES);
}
