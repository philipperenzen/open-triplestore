import { describe, it, expect } from 'vitest';
import { sanitizeHtml } from '../ontology/sanitizeHtml.js';

// Regression guard (rdf:HTML stored-XSS hardening): rdf:HTML literals are
// attacker-controllable and rendered with {@html}, so they must be DOMPurified.
// These payloads all bypassed the previous regex-only sanitizer.
describe('sanitizeHtml (rdf:HTML literal XSS defense)', () => {
  const xssPayloads = [
    '<img src=x onerror="window.__xss=1">',
    '<img src=x onerror=window.__xss=1>',          // unquoted handler
    '<svg onload="window.__xss=1"></svg>',
    '<scr<script>ipt>window.__xss=1</scr</script>ipt>',
    '<a href="javascript:window.__xss=1">click</a>',
    '<iframe src="javascript:window.__xss=1"></iframe>',
    '<body onload=window.__xss=1>',
  ];

  for (const payload of xssPayloads) {
    it(`neutralizes: ${payload.slice(0, 40)}`, () => {
      const clean = sanitizeHtml(payload);
      expect(clean.toLowerCase()).not.toContain('onerror');
      expect(clean.toLowerCase()).not.toContain('onload');
      expect(clean.toLowerCase()).not.toContain('<script');
      expect(clean.toLowerCase()).not.toContain('javascript:');
    });
  }

  it('preserves benign formatting', () => {
    const clean = sanitizeHtml('<b>bold</b> and <a href="https://example.org">link</a>');
    expect(clean).toContain('<b>');
    expect(clean).toContain('bold');
    expect(clean).toContain('href="https://example.org"');
  });

  it('handles nullish input safely', () => {
    expect(sanitizeHtml(null)).toBe('');
    expect(sanitizeHtml(undefined)).toBe('');
  });
});
