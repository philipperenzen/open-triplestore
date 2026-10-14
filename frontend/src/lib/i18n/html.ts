import DOMPurify from 'dompurify';
import { derived } from 'svelte/store';
import { t } from 'svelte-i18n';

// Translations that carry inline markup (`<strong>`, `<em>`, `<code>`) are
// rendered with {@html}. Two rules keep that safe:
//   1. every interpolated value is HTML-escaped, so a dataset, organisation or
//      shape-graph name can never inject markup — the markup a message needs
//      lives in the translation string itself (`delete <strong>{name}</strong>`);
//   2. the formatted message passes a sanitizer that keeps only the few inline
//      tags translations use, with no attributes at all.
// Use `{@html $tHtml('key', { values })}` instead of `{@html $t(...)}`.

const ESCAPES: Record<string, string> = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };

/** Escape a value for use as HTML text. */
export function escapeHtml(value: unknown): string {
  return String(value ?? '').replace(/[&<>"']/g, (ch) => ESCAPES[ch]);
}

/** Tags a translation may use; everything else is stripped (its text kept). */
export const TRANSLATION_TAGS = ['strong', 'em', 'b', 'i', 'code', 'kbd', 'br'];

/** Sanitize a formatted translation: inline formatting tags only, no attributes. */
export function sanitizeTranslation(html: string): string {
  return DOMPurify.sanitize(String(html ?? ''), { ALLOWED_TAGS: TRANSLATION_TAGS, ALLOWED_ATTR: [] });
}

type FormatOptions = { values?: Record<string, unknown>; [k: string]: unknown };

/** Format `key` with escaped values and return sanitized HTML for {@html}. */
export const tHtml = derived(t, ($t) => (key: string, options: FormatOptions = {}): string => {
  const values = options.values
    ? Object.fromEntries(Object.entries(options.values).map(([k, v]) => [k, escapeHtml(v)]))
    : undefined;
  const formatted = $t(key, (values ? { ...options, values } : options) as Parameters<typeof $t>[1]);
  return sanitizeTranslation(formatted);
});
