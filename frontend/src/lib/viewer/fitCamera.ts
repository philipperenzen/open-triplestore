/**
 * Exact camera framing for the 3D viewer.
 *
 * Framing used to fit a BOUNDING SPHERE sized by the model's largest single
 * axis. For the shape most of this data actually has — a wide, shallow building
 * footprint — that parks the camera far enough back that the model fills well
 * under half the frame, which is the "too zoomed out" complaint.
 *
 * Instead, project the eight corners of the bounding box onto the camera's own
 * basis and take the smallest distance that keeps every corner inside BOTH
 * frustum planes. That frames tightly and correctly for any viewport aspect.
 *
 * Pure, so the framing is unit-testable without a WebGL context.
 */
import { Box3, Vector3 } from 'three';

const WORLD_UP = new Vector3(0, 1, 0);

export interface FitOpts {
  /** Vertical field of view, in DEGREES (three's `camera.fov`). */
  fov: number;
  /** Viewport aspect (width / height). */
  aspect: number;
  /** Unit direction from the model centre toward the camera. */
  dir: Vector3;
  /** Slack around the model. 1 = corners exactly on the frustum edges. */
  padding?: number;
}

/**
 * Distance from the box centre at which the whole box fits the frustum.
 * Returns a small positive number for a degenerate (empty/point) box.
 */
export function fitDistance(box: Box3, opts: FitOpts): number {
  const { fov, aspect, dir, padding = 1.06 } = opts;
  if (box.isEmpty()) return 0.001;

  const center = box.getCenter(new Vector3());
  const right = new Vector3().crossVectors(WORLD_UP, dir).normalize();
  // A straight-down view leaves `right` undefined; fall back to a fixed axis.
  if (!Number.isFinite(right.x) || right.lengthSq() < 1e-8) right.set(1, 0, 0);
  const up = new Vector3().crossVectors(dir, right).normalize();

  const halfV = (fov * Math.PI) / 360;
  const tanV = Math.tan(halfV);
  const tanH = Math.tan(Math.atan(tanV * (aspect || 1)));

  let dist = 0;
  const v = new Vector3();
  for (let i = 0; i < 8; i++) {
    v.set(
      i & 1 ? box.max.x : box.min.x,
      i & 2 ? box.max.y : box.min.y,
      i & 4 ? box.max.z : box.min.z,
    ).sub(center);
    const depth = v.dot(dir);
    dist = Math.max(
      dist,
      depth + Math.abs(v.dot(up)) / tanV,
      depth + Math.abs(v.dot(right)) / tanH,
    );
  }
  return Math.max(dist, 0.001) * padding;
}
