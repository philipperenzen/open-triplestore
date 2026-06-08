import { describe, it, expect } from 'vitest';
import {
  parseBanner,
  getPreset,
  defaultPresetFor,
  DEFAULT_BANNER_PRESET_ID,
  BANNER_PRESETS,
  ANIMATED_PRESETS,
} from '../banners';

describe('parseBanner', () => {
  it('classifies empty values as none', () => {
    expect(parseBanner(null)).toEqual({ type: 'none', presetId: null });
    expect(parseBanner(undefined)).toEqual({ type: 'none', presetId: null });
    expect(parseBanner('')).toEqual({ type: 'none', presetId: null });
  });

  it('classifies the preset sentinel', () => {
    expect(parseBanner('preset:aurora-rose')).toEqual({ type: 'preset', presetId: 'aurora-rose' });
  });

  it('classifies an object key as an upload', () => {
    expect(parseBanner('dataset-banners/abc.png')).toEqual({ type: 'upload', presetId: null });
  });

  it('treats the legacy boolean flag as an upload', () => {
    // Pages set `bannerKey = true` right after an upload before reload.
    expect(parseBanner(true).type).toBe('upload');
  });
});

describe('getPreset', () => {
  it('returns the matching preset', () => {
    expect(getPreset('aurora-rose').id).toBe('aurora-rose');
  });

  it('falls back to the default for unknown or empty ids', () => {
    expect(getPreset('does-not-exist').id).toBe(DEFAULT_BANNER_PRESET_ID);
    expect(getPreset(null).id).toBe(DEFAULT_BANNER_PRESET_ID);
    expect(getPreset(undefined).id).toBe(DEFAULT_BANNER_PRESET_ID);
  });
});

describe('defaultPresetFor', () => {
  it('is deterministic for a given seed', () => {
    expect(defaultPresetFor('spatial').id).toBe(defaultPresetFor('spatial').id);
  });

  it('always returns an animated preset', () => {
    for (const seed of ['', 'spatial', 'reasoning', 'a', 'zzz']) {
      expect(defaultPresetFor(seed).kind).toBe('animated');
    }
  });
});

describe('registry invariants', () => {
  it('has unique slug ids matching the backend slug shape', () => {
    const ids = BANNER_PRESETS.map((p) => p.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids) expect(id).toMatch(/^[a-z0-9-]{1,40}$/);
  });

  it('includes the default preset and at least one animated preset', () => {
    expect(BANNER_PRESETS.some((p) => p.id === DEFAULT_BANNER_PRESET_ID)).toBe(true);
    expect(ANIMATED_PRESETS.length).toBeGreaterThan(0);
  });
});
