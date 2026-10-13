import { describe, it, expect } from 'vitest';
import { normaliseBasePath } from './base-path.mjs';

describe('normaliseBasePath (OTS_BASE_PATH → Vite base)', () => {
  it('is the root when unset, empty or a bare slash', () => {
    for (const raw of [undefined, null, '', '  ', '/', '//']) expect(normaliseBasePath(raw)).toBe('/');
  });

  it('adds the leading and trailing slash Vite needs', () => {
    expect(normaliseBasePath('ots')).toBe('/ots/');
    expect(normaliseBasePath('/ots')).toBe('/ots/');
    expect(normaliseBasePath('/ots/')).toBe('/ots/');
    expect(normaliseBasePath(' /tools/ots/ ')).toBe('/tools/ots/');
  });

  it('refuses something that is not a plain path', () => {
    for (const raw of ['https://example.org/ots/', '/ots?x=1', '/o ts/', '/a/../b', '/a//b', '/%2e/']) {
      expect(() => normaliseBasePath(raw)).toThrow(/plain URL path/);
    }
  });

  it('refuses a prefix that starts like one of the app\'s own paths', () => {
    for (const raw of ['/api/', '/sparql', '/datasets/x/', 'embed']) {
      expect(() => normaliseBasePath(raw)).toThrow(/a path the app itself uses/);
    }
    // A segment that merely starts with the same letters is fine.
    expect(normaliseBasePath('/apis/')).toBe('/apis/');
  });
});
