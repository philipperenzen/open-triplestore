/**
 * Camera framing for the 3D viewer.
 *
 * The bug these pin down: the old bounding-sphere fit sized by the largest
 * single axis left wide, shallow models (i.e. most building footprints) filling
 * a small fraction of the frame.
 */
import { describe, it, expect } from 'vitest';
import { Box3, Vector3 } from 'three';
import { fitDistance } from '../viewer/fitCamera';

const DIR = new Vector3(0.7, 0.55, 1).normalize();
const opts = (over = {}) => ({ fov: 50, aspect: 720 / 420, dir: DIR, ...over });

/**
 * How much of the frustum the box actually uses, measured per corner AT ITS OWN
 * DEPTH — a near corner is closer to the camera and so covers more of the frame
 * than the same offset at the centre plane. 1 = a corner exactly touching an
 * edge, >1 = clipped, well under 1 = the "too zoomed out" symptom.
 */
function frustumFill(box: Box3, dist: number, fov: number, aspect: number, dir: Vector3) {
  const center = box.getCenter(new Vector3());
  const right = new Vector3().crossVectors(new Vector3(0, 1, 0), dir).normalize();
  if (right.lengthSq() < 1e-8) right.set(1, 0, 0);
  const up = new Vector3().crossVectors(dir, right).normalize();
  const tanV = Math.tan((fov * Math.PI) / 360);
  const tanH = tanV * aspect;

  let fill = 0;
  const v = new Vector3();
  for (let i = 0; i < 8; i++) {
    v.set(
      i & 1 ? box.max.x : box.min.x,
      i & 2 ? box.max.y : box.min.y,
      i & 4 ? box.max.z : box.min.z,
    ).sub(center);
    // Distance from the camera to this corner's depth plane.
    const dz = dist - v.dot(dir);
    if (dz <= 0) return Infinity; // behind the camera — never acceptable
    fill = Math.max(fill, Math.abs(v.dot(up)) / (tanV * dz), Math.abs(v.dot(right)) / (tanH * dz));
  }
  return fill;
}

describe('fitDistance', () => {
  it('frames a cube so every corner is inside the frustum', () => {
    const box = new Box3(new Vector3(-1, -1, -1), new Vector3(1, 1, 1));
    expect(frustumFill(box, fitDistance(box, opts()), 50, 720 / 420, DIR)).toBeLessThanOrEqual(1);
  });

  it('fills the frame instead of leaving the model a speck', () => {
    // A wide, shallow footprint: the case the old sphere fit handled worst. With
    // padding 1.06 the binding corner should sit at ~1/1.06 of the frustum edge.
    const box = new Box3(new Vector3(-25, 0, -15), new Vector3(25, 3, 15));
    const fill = frustumFill(box, fitDistance(box, opts()), 50, 720 / 420, DIR);
    expect(fill).toBeGreaterThan(0.85);
    expect(fill).toBeLessThanOrEqual(1);
  });

  it('touches the frustum edge exactly at padding 1', () => {
    const box = new Box3(new Vector3(-25, 0, -15), new Vector3(25, 3, 15));
    const d = fitDistance(box, opts({ padding: 1 }));
    expect(frustumFill(box, d, 50, 720 / 420, DIR)).toBeCloseTo(1, 6);
  });

  it('frames a tall, narrow model without clipping it', () => {
    const tower = new Box3(new Vector3(-4, 0, -4), new Vector3(4, 60, 4));
    const fill = frustumFill(tower, fitDistance(tower, opts()), 50, 720 / 420, DIR);
    expect(fill).toBeLessThanOrEqual(1);
    expect(fill).toBeGreaterThan(0.85);
  });

  it('pulls back further for a narrower viewport', () => {
    const box = new Box3(new Vector3(-10, 0, -10), new Vector3(10, 2, 10));
    const wide = fitDistance(box, opts({ aspect: 2 }));
    const narrow = fitDistance(box, opts({ aspect: 0.6 }));
    expect(narrow).toBeGreaterThan(wide);
  });

  it('scales linearly with model size', () => {
    const small = new Box3(new Vector3(-1, -1, -1), new Vector3(1, 1, 1));
    const big = new Box3(new Vector3(-10, -10, -10), new Vector3(10, 10, 10));
    expect(fitDistance(big, opts())).toBeCloseTo(fitDistance(small, opts()) * 10, 5);
  });

  it('respects the padding factor', () => {
    const box = new Box3(new Vector3(-2, -2, -2), new Vector3(2, 2, 2));
    const tight = fitDistance(box, opts({ padding: 1 }));
    const loose = fitDistance(box, opts({ padding: 1.5 }));
    expect(loose).toBeCloseTo(tight * 1.5, 5);
  });

  it('survives a degenerate box instead of returning 0 or NaN', () => {
    expect(fitDistance(new Box3(), opts())).toBeGreaterThan(0);
    const point = new Box3(new Vector3(1, 1, 1), new Vector3(1, 1, 1));
    expect(fitDistance(point, opts())).toBeGreaterThan(0);
  });

  it('handles a top-down direction, where the up axis is degenerate', () => {
    const box = new Box3(new Vector3(-1, -1, -1), new Vector3(1, 1, 1));
    const d = fitDistance(box, opts({ dir: new Vector3(0, 1, 0) }));
    expect(Number.isFinite(d)).toBe(true);
    expect(d).toBeGreaterThan(0);
  });
});
