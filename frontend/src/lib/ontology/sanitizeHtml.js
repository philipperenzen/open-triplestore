import DOMPurify from 'dompurify';

/**
 * Sanitize an `rdf:HTML` literal before it is injected into the DOM via `{@html}`.
 *
 * `rdf:HTML` values are attacker-controllable — any user who can write a triple
 * can store one, and it is then rendered for every viewer (including admins and,
 * for public datasets, anonymous users). This uses DOMPurify's HTML profile,
 * which strips `<script>`, inline event handlers (`onerror`, `onload`, …), and
 * `javascript:` URLs.
 *
 * It replaces an earlier hand-rolled regex sanitizer that only removed `<script>`
 * blocks and quoted `on*=` attributes and was trivially bypassable
 * (`<img src=x onerror=…>`, `<svg onload=…>`, unquoted handlers, …).
 * Hardening: closes a stored-XSS vector via attacker-controlled rdf:HTML.
 *
 * @param {unknown} html
 * @returns {string} sanitized HTML safe to inject
 */
export function sanitizeHtml(html) {
  return DOMPurify.sanitize(String(html ?? ''), { USE_PROFILES: { html: true } });
}
