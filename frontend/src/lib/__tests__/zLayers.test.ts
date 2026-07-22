import { describe, it, expect } from 'vitest';
import { previewZ, Z_PREVIEW, Z_DOCK, Z_INSPECTOR_BASE } from '../viewer/zLayers';

describe('previewZ', () => {
  it('rests at the documented preview band when nothing else floats', () => {
    expect(previewZ(0)).toBe(Z_PREVIEW);
  });

  it('ignores inspector windows that are already below the preview band', () => {
    expect(previewZ(Z_INSPECTOR_BASE)).toBe(Z_PREVIEW);
    expect(previewZ(Z_PREVIEW - 1)).toBe(Z_PREVIEW);
  });

  it('climbs one step above the topmost inspector window', () => {
    // This is the reported symptom: the viewer raises the focused window with an
    // unbounded counter, so a long session walked it straight past a hard-coded
    // 1200 and the preview opened invisibly underneath.
    expect(previewZ(Z_PREVIEW)).toBe(Z_PREVIEW + 1);
    expect(previewZ(Z_PREVIEW + 40)).toBe(Z_PREVIEW + 41);
  });

  it('never reaches the dock, however far the counter has leaked', () => {
    expect(previewZ(Z_DOCK)).toBe(Z_DOCK - 1);
    expect(previewZ(999999)).toBe(Z_DOCK - 1);
  });

  it('falls back to the resting band for non-finite input', () => {
    expect(previewZ(Number.NaN)).toBe(Z_PREVIEW);
    expect(previewZ(Number.POSITIVE_INFINITY)).toBe(Z_PREVIEW);
  });

  it('keeps the bands ordered: inspector < preview < dock', () => {
    expect(Z_INSPECTOR_BASE).toBeLessThan(Z_PREVIEW);
    expect(Z_PREVIEW).toBeLessThan(Z_DOCK);
  });
});
