/**
 * Shareable viewer deep links (`?focus=<iri>`).
 *
 * The read side already existed (DatasetViewer applies `?focus` on load); these
 * cover the write side — building a link to hand over, and the address-bar sync.
 */
import { describe, it, expect } from 'vitest';
import { viewerShareUrl, focusUrlUpdate, FOCUS_PARAM } from '../viewer/shareLink';

const loc = (search = '', pathname = '/datasets/d1/viewer') => ({
  origin: 'https://ots.example',
  pathname,
  search,
});

describe('viewerShareUrl', () => {
  it('adds the focus IRI to a clean viewer URL', () => {
    expect(viewerShareUrl(loc(), 'https://ex.org/wall-7')).toBe(
      'https://ots.example/datasets/d1/viewer?focus=https%3A%2F%2Fex.org%2Fwall-7',
    );
  });

  it('replaces an existing focus rather than appending a second one', () => {
    const url = viewerShareUrl(loc('?focus=old'), 'new');
    expect(url).toBe('https://ots.example/datasets/d1/viewer?focus=new');
  });

  it('preserves other query parameters', () => {
    const url = viewerShareUrl(loc('?tab=3d'), 'urn:x');
    expect(url).toContain('tab=3d');
    expect(url).toContain(`${FOCUS_PARAM}=urn%3Ax`);
  });

  it('drops the focus for an empty IRI', () => {
    expect(viewerShareUrl(loc('?focus=old'), '')).toBe('https://ots.example/datasets/d1/viewer');
  });
});

describe('focusUrlUpdate', () => {
  it('returns a path+query to write, without the origin', () => {
    expect(focusUrlUpdate(loc(), 'urn:a')).toBe('/datasets/d1/viewer?focus=urn%3Aa');
  });

  it('returns null when the URL already names that element', () => {
    expect(focusUrlUpdate(loc('?focus=urn%3Aa'), 'urn:a')).toBeNull();
    expect(focusUrlUpdate(loc(''), '')).toBeNull();
  });

  it('clears the parameter when the selection is dropped', () => {
    expect(focusUrlUpdate(loc('?focus=urn%3Aa'), '')).toBe('/datasets/d1/viewer');
  });

  it('keeps unrelated parameters when clearing', () => {
    expect(focusUrlUpdate(loc('?tab=3d&focus=urn%3Aa'), '')).toBe('/datasets/d1/viewer?tab=3d');
  });
});
