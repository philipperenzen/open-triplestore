// Window/tab state for the dataset viewer's floating inspectors.
//
// The explorer used to model an inspector as "one panel = one element", which
// made every in-modal link spawn another window and left no place to express a
// browser-like tab group. Here a WINDOW is the container (position, stacking,
// minimised/full state, one 3D budget slot) and a TAB is a subject shown inside
// it — either a feed element or an arbitrary RDF resource reached from a link.
//
// This module is deliberately pure: no DOM, no Svelte, no clocks and no random
// ids, so every transition is reproducible in a unit test and the components
// stay thin. Every operation returns a NEW state (the Svelte components rely on
// assignment invalidation, and identity equality is how they detect a no-op).

export type TabKind = 'element' | 'resource';

export interface WindowTab {
  /** Stable de-duplication key — `${kind}:${id}`. */
  key: string;
  kind: TabKind;
  /** Element IRI (feed) or resource IRI. */
  id: string;
  label: string;
}

export interface WindowPos {
  x: number;
  y: number;
}

export interface InspectorWindow {
  wid: string;
  tabs: WindowTab[];
  activeKey: string;
  /** Stacking order; the highest is on top. */
  z: number;
  pos: WindowPos;
  minimized: boolean;
  full: boolean;
  /** Whether this window is allowed to mount the heavy 3D viewer. */
  loadModel: boolean;
  /** Monotonic open counter — drives the cascade offset of a new window. */
  seq: number;
}

export interface WindowState {
  windows: InspectorWindow[];
  zTop: number;
  seq: number;
  widSeq: number;
}

/** Windows open at once before the lowest-stacked one is evicted. */
export const MAX_WINDOWS = 5;
/** Tabs per window before the oldest inactive one is dropped. */
export const MAX_TABS_PER_WINDOW = 8;
/** Live 3D viewers allowed at once (WebGL contexts are a scarce resource). */
export const MODEL_CAP = 2;
/** Pixel step between cascaded new windows. */
export const CASCADE = 30;
/**
 * Origin of the stacking counter. `z` is an ORDER, not a CSS value: it only ever
 * grows, so the host must map it onto a bounded band before rendering (the
 * dataset viewer ranks the windows into `Z_INSPECTOR_BASE + i`). The origin
 * mirrors `Z_INSPECTOR_BASE` in ./zLayers, which owns the band layout —
 * duplicated as a literal only to keep this module free of any import.
 */
export const Z_BASE = 1100;

/** Nominal window box, mirroring the .element-modal rule in ElementModal.svelte. */
const WINDOW_W = 720;
const WINDOW_H = 580;
/** How much of a window must stay on screen after a drop. */
const KEEP_VISIBLE = 120;

export const tabKey = (kind: TabKind, id: string): string => `${kind}:${id}`;

/**
 * Svelte context key an inspector window sets so descendants (RdfTerm) can turn
 * an RDF link into a tab of that window instead of a full page navigation. The
 * value is `(req: { iri: string; graph?: string }) => boolean` — returning false
 * lets the caller fall back to the router, so every OTHER page is untouched.
 * Declared here (a plain string) to keep the components free of a shared
 * component-only module.
 */
export const OPEN_RESOURCE_CONTEXT = 'ots:viewer:openResource';

export function createState(): WindowState {
  return { windows: [], zTop: Z_BASE, seq: 0, widSeq: 0 };
}

export function activeTabOf(w: InspectorWindow | null | undefined): WindowTab | null {
  if (!w) return null;
  return w.tabs.find((t) => t.key === w.activeKey) ?? w.tabs[0] ?? null;
}

/** Locate a tab anywhere in the state — the basis of global de-duplication. */
export function findTab(
  state: WindowState,
  key: string
): { wid: string; tab: WindowTab } | null {
  for (const w of state.windows) {
    const tab = w.tabs.find((t) => t.key === key);
    if (tab) return { wid: w.wid, tab };
  }
  return null;
}

/** Free 3D slots. Minimised windows are unmounted, so they don't count. */
export function modelSlotsFree(state: WindowState): number {
  const live = state.windows.filter((w) => w.loadModel && !w.minimized).length;
  return Math.max(0, MODEL_CAP - live);
}

function patch(
  state: WindowState,
  wid: string,
  fn: (w: InspectorWindow) => InspectorWindow,
  extra: Partial<WindowState> = {}
): WindowState {
  return {
    ...state,
    ...extra,
    windows: state.windows.map((w) => (w.wid === wid ? fn(w) : w)),
  };
}

/** Revoke 3D from the lowest-stacked live holders until `keepWid` fits the cap. */
function enforceModelCap(state: WindowState, keepWid: string): WindowState {
  const live = state.windows.filter(
    (w) => w.loadModel && !w.minimized && w.wid !== keepWid
  );
  if (live.length < MODEL_CAP) return state;
  const victims = new Set(
    [...live].sort((a, b) => a.z - b.z).slice(0, live.length - MODEL_CAP + 1).map((w) => w.wid)
  );
  return {
    ...state,
    windows: state.windows.map((w) =>
      victims.has(w.wid) ? { ...w, loadModel: false } : w
    ),
  };
}

/** Bring a window to the front; restores it first when it was minimised. */
function reveal(state: WindowState, wid: string): WindowState {
  const w = state.windows.find((x) => x.wid === wid);
  if (!w) return state;
  return w.minimized ? restoreWindow(state, wid) : focusWindow(state, wid);
}

export function focusWindow(state: WindowState, wid: string): WindowState {
  const w = state.windows.find((x) => x.wid === wid);
  // Already on top: return the SAME object so callers can skip the re-render
  // (a pointerdown anywhere in a focused window used to refetch its RDF).
  if (!w || (w.z === state.zTop && !w.minimized)) return state;
  const zTop = state.zTop + 1;
  return patch(state, wid, (x) => ({ ...x, z: zTop, minimized: false }), { zTop });
}

export function setActiveTab(state: WindowState, wid: string, key: string): WindowState {
  const w = state.windows.find((x) => x.wid === wid);
  if (!w || w.activeKey === key || !w.tabs.some((t) => t.key === key)) return state;
  return patch(state, wid, (x) => ({ ...x, activeKey: key }));
}

export function moveWindow(state: WindowState, wid: string, pos: WindowPos): WindowState {
  const w = state.windows.find((x) => x.wid === wid);
  if (!w || (w.pos.x === pos.x && w.pos.y === pos.y)) return state;
  return patch(state, wid, (x) => ({ ...x, pos: { x: pos.x, y: pos.y } }));
}

export function toggleFull(state: WindowState, wid: string): WindowState {
  if (!state.windows.some((x) => x.wid === wid)) return state;
  return patch(state, wid, (x) => ({ ...x, full: !x.full }));
}

export function minimizeWindow(state: WindowState, wid: string): WindowState {
  const w = state.windows.find((x) => x.wid === wid);
  if (!w || w.minimized) return state;
  // `loadModel` is kept so a restore can claim its viewer back; it simply stops
  // counting against the cap while the body is unmounted.
  return patch(state, wid, (x) => ({ ...x, minimized: true }));
}

export function restoreWindow(state: WindowState, wid: string): WindowState {
  const w = state.windows.find((x) => x.wid === wid);
  if (!w) return state;
  const zTop = state.zTop + 1;
  const next = patch(state, wid, (x) => ({ ...x, minimized: false, z: zTop }), { zTop });
  // A restored 3D window re-enters the budget and may evict a live holder.
  return w.loadModel ? enforceModelCap(next, wid) : next;
}

/**
 * Grant this window a live 3D viewer, revoking the lowest-stacked one if full.
 *
 * Deliberately does NOT touch `z`: granting a slot must not re-order the stack.
 * It used to, and since a revoked window asks for its slot back, the eviction
 * cascade shuffled the window the user was reading to the bottom of a 30 px
 * cascade — i.e. out of sight. Raising a window is focusWindow's job.
 */
export function requestModel(state: WindowState, wid: string): WindowState {
  const w = state.windows.find((x) => x.wid === wid);
  if (!w || w.loadModel) return state;
  const next = patch(state, wid, (x) => ({ ...x, loadModel: true }));
  return w.minimized ? next : enforceModelCap(next, wid);
}

export function closeWindow(state: WindowState, wid: string): WindowState {
  if (!state.windows.some((x) => x.wid === wid)) return state;
  return { ...state, windows: state.windows.filter((w) => w.wid !== wid) };
}

export function closeAll(state: WindowState): WindowState {
  if (!state.windows.length) return state;
  return { ...state, windows: [] };
}

export function closeTab(state: WindowState, wid: string, key: string): WindowState {
  const w = state.windows.find((x) => x.wid === wid);
  if (!w) return state;
  const idx = w.tabs.findIndex((t) => t.key === key);
  if (idx === -1) return state;
  const tabs = w.tabs.filter((t) => t.key !== key);
  if (!tabs.length) return closeWindow(state, wid);
  // Browser convention: focus moves to the right-hand neighbour, else the left.
  const activeKey =
    w.activeKey === key ? (tabs[idx] ?? tabs[idx - 1] ?? tabs[0]).key : w.activeKey;
  return patch(state, wid, (x) => ({ ...x, tabs, activeKey }));
}

/** Trim a window back to MAX_TABS_PER_WINDOW, dropping the oldest inactive tab. */
function trimTabs(w: InspectorWindow): InspectorWindow {
  if (w.tabs.length <= MAX_TABS_PER_WINDOW) return w;
  const tabs = [...w.tabs];
  while (tabs.length > MAX_TABS_PER_WINDOW) {
    const victim = tabs.findIndex((t) => t.key !== w.activeKey);
    tabs.splice(victim === -1 ? 0 : victim, 1);
  }
  return { ...w, tabs };
}

export interface OpenOpts {
  wantsModel?: boolean;
  pos?: WindowPos;
}

/**
 * Open a subject in a NEW window — what a pick in the side list or on the map
 * does. A subject already open anywhere is revealed instead of duplicated.
 */
export function openInNewWindow(
  state: WindowState,
  tab: WindowTab,
  opts: OpenOpts = {}
): WindowState {
  const existing = findTab(state, tab.key);
  if (existing) return setActiveTab(reveal(state, existing.wid), existing.wid, tab.key);

  const zTop = state.zTop + 1;
  const seq = state.seq;
  const win: InspectorWindow = {
    wid: `w${state.widSeq + 1}`,
    tabs: [tab],
    activeKey: tab.key,
    z: zTop,
    pos: opts.pos ?? { x: (seq % 6) * CASCADE, y: (seq % 6) * CASCADE },
    minimized: false,
    full: false,
    loadModel: !!opts.wantsModel && modelSlotsFree(state) > 0,
    seq,
  };
  let windows = [...state.windows, win];
  if (windows.length > MAX_WINDOWS) {
    // Evict by stacking order, not insertion order: the window the user last
    // looked at must never be the one that disappears.
    const oldest = [...windows].sort((a, b) => a.z - b.z)[0];
    windows = windows.filter((w) => w.wid !== oldest.wid);
  }
  return { ...state, windows, zTop, seq: seq + 1, widSeq: state.widSeq + 1 };
}

/**
 * Open a subject as a TAB of an existing window — what a link inside a window
 * does. De-duplication is global: if the subject is already open in ANOTHER
 * window that window is revealed rather than the tab being copied.
 */
export function openTabInWindow(
  state: WindowState,
  wid: string,
  tab: WindowTab
): WindowState {
  const existing = findTab(state, tab.key);
  if (existing) return setActiveTab(reveal(state, existing.wid), existing.wid, tab.key);
  if (!state.windows.some((w) => w.wid === wid)) return openInNewWindow(state, tab);

  const zTop = state.zTop + 1;
  return patch(
    state,
    wid,
    (w) => trimTabs({ ...w, tabs: [...w.tabs, tab], activeKey: tab.key, z: zTop, minimized: false }),
    { zTop }
  );
}

/**
 * Move a tab into another window (the drag-and-drop group gesture). Within the
 * same window an explicit `index` reorders; without one it is a no-op.
 */
export function moveTabToWindow(
  state: WindowState,
  fromWid: string,
  key: string,
  toWid: string,
  index?: number
): WindowState {
  const from = state.windows.find((w) => w.wid === fromWid);
  const tab = from?.tabs.find((t) => t.key === key);
  if (!from || !tab) return state;
  if (!state.windows.some((w) => w.wid === toWid)) return state;

  if (fromWid === toWid) {
    if (index == null) return state;
    const tabs = from.tabs.filter((t) => t.key !== key);
    tabs.splice(Math.max(0, Math.min(index, tabs.length)), 0, tab);
    return patch(state, fromWid, (w) => ({ ...w, tabs, activeKey: key }));
  }
  // The subject may already live in the target — then it is a plain close+focus.
  const dupe = state.windows.find((w) => w.wid === toWid)!.tabs.some((t) => t.key === key);
  const zTop = state.zTop + 1;
  const windows = state.windows
    .map((w) => {
      if (w.wid === fromWid) {
        const tabs = w.tabs.filter((t) => t.key !== key);
        const activeKey =
          w.activeKey === key ? (tabs[0]?.key ?? '') : w.activeKey;
        return { ...w, tabs, activeKey };
      }
      if (w.wid === toWid) {
        const tabs = dupe ? w.tabs : [...w.tabs];
        if (!dupe) {
          const at = index == null ? tabs.length : Math.max(0, Math.min(index, tabs.length));
          tabs.splice(at, 0, tab);
        }
        return trimTabs({ ...w, tabs, activeKey: key, z: zTop, minimized: false });
      }
      return w;
    })
    .filter((w) => w.tabs.length > 0); // an emptied source window closes itself
  return { ...state, windows, zTop };
}

/** Tear a tab out into a window of its own (drop outside any tab strip). */
export function detachTabToNewWindow(
  state: WindowState,
  fromWid: string,
  key: string,
  pos?: WindowPos
): WindowState {
  const from = state.windows.find((w) => w.wid === fromWid);
  const tab = from?.tabs.find((t) => t.key === key);
  if (!from || !tab) return state;
  // A lone tab is already its own window — just move it to the drop point.
  if (from.tabs.length === 1) return pos ? moveWindow(state, fromWid, pos) : state;

  const stripped = patch(state, fromWid, (w) => {
    const tabs = w.tabs.filter((t) => t.key !== key);
    const activeKey = w.activeKey === key ? tabs[0].key : w.activeKey;
    return { ...w, tabs, activeKey };
  });
  return openInNewWindow(stripped, tab, { pos });
}

/**
 * Viewport drop point → the window's translate() offset.
 *
 * The inspector is centred by CSS (left/top 50% with negative margins that
 * mirror `margin-left: min(-360px, -45vw)`), so a raw client point has to be
 * re-expressed relative to that origin. The result is clamped so a dropped
 * window always keeps its header reachable on screen.
 */
export function dropPos(
  point: { x: number; y: number },
  viewport: { width: number; height: number }
): WindowPos {
  const w = Math.min(WINDOW_W, viewport.width * 0.9);
  const originX = viewport.width / 2 + Math.min(-WINDOW_W / 2, -0.45 * viewport.width);
  const originY = viewport.height / 2 - WINDOW_H / 2;
  const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
  return {
    x: Math.round(
      clamp(point.x - originX, KEEP_VISIBLE - w - originX, viewport.width - KEEP_VISIBLE - originX)
    ),
    y: Math.round(clamp(point.y - originY, -originY, viewport.height - 60 - originY)),
  };
}
