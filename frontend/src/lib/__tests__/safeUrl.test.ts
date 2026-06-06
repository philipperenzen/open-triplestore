import { describe, it, expect } from 'vitest';
import { safeExternalUrl, safeImageUrl } from '../safeUrl.js';

describe('safeExternalUrl', () => {
  it('passes http/https URLs through unchanged', () => {
    expect(safeExternalUrl('http://example.com/a')).toBe('http://example.com/a');
    expect(safeExternalUrl('https://example.com/a?b=1#c')).toBe('https://example.com/a?b=1#c');
  });

  it('allows mailto: links', () => {
    expect(safeExternalUrl('mailto:alice@example.com')).toBe('mailto:alice@example.com');
  });

  it('treats relative references as safe', () => {
    expect(safeExternalUrl('/data-models/foo')).toBe('/data-models/foo');
    expect(safeExternalUrl('relative/path')).toBe('relative/path');
  });

  it('rejects javascript: URLs (the core stored-XSS vector)', () => {
    expect(safeExternalUrl('javascript:alert(1)')).toBeUndefined();
    expect(safeExternalUrl('javascript:alert(document.cookie)')).toBeUndefined();
  });

  it('rejects data: URLs', () => {
    expect(safeExternalUrl('data:text/html,<script>alert(1)</script>')).toBeUndefined();
    expect(safeExternalUrl('data:text/plain,hi')).toBeUndefined();
  });

  it('rejects other dangerous schemes', () => {
    expect(safeExternalUrl('vbscript:msgbox(1)')).toBeUndefined();
    expect(safeExternalUrl('file:///etc/passwd')).toBeUndefined();
  });

  it('is not fooled by case, whitespace, or embedded control chars', () => {
    expect(safeExternalUrl('JavaScript:alert(1)')).toBeUndefined();
    expect(safeExternalUrl('  javascript:alert(1)  ')).toBeUndefined();
    // The URL parser strips ASCII tab/newline before the scheme is read.
    expect(safeExternalUrl('java\tscript:alert(1)')).toBeUndefined();
    expect(safeExternalUrl('java\nscript:alert(1)')).toBeUndefined();
  });

  it('returns undefined for empty/nullish input', () => {
    expect(safeExternalUrl('')).toBeUndefined();
    expect(safeExternalUrl('   ')).toBeUndefined();
    expect(safeExternalUrl(null)).toBeUndefined();
    expect(safeExternalUrl(undefined)).toBeUndefined();
  });
});

describe('safeImageUrl', () => {
  it('allows http/https image sources', () => {
    expect(safeImageUrl('https://cdn.example.com/p.png')).toBe('https://cdn.example.com/p.png');
    expect(safeImageUrl('http://example.com/p.jpg')).toBe('http://example.com/p.jpg');
  });

  it('rejects javascript: and data: image sources', () => {
    expect(safeImageUrl('javascript:alert(1)')).toBeUndefined();
    // data: images are an exfiltration/phishing vector here, so block them too.
    expect(safeImageUrl('data:image/svg+xml,<svg onload=alert(1)>')).toBeUndefined();
  });

  it('does not treat mailto: as a valid image source', () => {
    expect(safeImageUrl('mailto:alice@example.com')).toBeUndefined();
  });
});
