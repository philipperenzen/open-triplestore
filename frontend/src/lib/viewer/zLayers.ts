// One place that decides how the app's floating surfaces stack.
//
// The viewer can have four independent things floating at once: the inspector
// windows (each with its own z, raised on focus), the singleton preview overlay
// (PreviewOverlay.svelte), the bottom dock of minimised windows, and the
// first-person walkthrough. Those numbers used to live as bare literals in four
// stylesheets, which is how the preview overlay ended up *underneath* an
// inspector window: the viewer raises the focused window with an unbounded
// `++zTop` counter, and after enough clicks it simply walked past the overlay's
// hard-coded 1200.
//
// Naming the bands here makes the invariant checkable —
//   inspector windows < preview overlay < dock < walkthrough
// — and `previewZ()` keeps the overlay above whatever the window system is
// currently using while never climbing over the dock.

import { writable, type Readable } from 'svelte/store';

/** Lowest z-index an inspector window is allowed to occupy. */
export const Z_INSPECTOR_BASE = 1100;
/** Resting z-index of the singleton preview overlay. */
export const Z_PREVIEW = 1200;
/** The dock of minimised windows: always reachable, so it sits above the rest. */
export const Z_DOCK = 1300;
/** Immersive walkthrough: takes over the screen, so it is the top band. */
export const Z_WALKTHROUGH = 1400;

/**
 * The z-index the preview overlay should render at, given the highest z-index
 * currently claimed by an inspector-window system.
 *
 * Pure so the stacking invariant can be unit-tested: the result is never below
 * `Z_PREVIEW` (an idle page keeps the documented resting value) and never at or
 * above `Z_DOCK` (the dock must stay clickable even if a window system leaks an
 * absurd counter).
 */
export function previewZ(inspectorTopZ: number): number {
  const claimed = Number.isFinite(inspectorTopZ) ? Math.floor(inspectorTopZ) + 1 : Z_PREVIEW;
  return Math.min(Math.max(Z_PREVIEW, claimed), Z_DOCK - 1);
}

const topZ = writable(0);

/**
 * Highest z-index currently claimed by an inspector-window system, or 0 when no
 * such system is mounted. Read-only for consumers — only `reportInspectorTopZ`
 * may write it, so a stray `.set()` cannot bury the overlay.
 */
export const inspectorTopZ: Readable<number> = { subscribe: topZ.subscribe };

/**
 * Called by whatever owns floating inspector windows whenever its topmost
 * window changes, and with 0 when it unmounts. Optional: with no reporter the
 * overlay simply rests at `Z_PREVIEW`, which is the pre-existing behaviour.
 */
export function reportInspectorTopZ(z: number): void {
  topZ.set(Number.isFinite(z) ? z : 0);
}
