import { describe, it, expect } from 'vitest';
import {
  MAX_TABS_PER_WINDOW,
  MAX_WINDOWS,
  MODEL_CAP,
  activeTabOf,
  closeTab,
  closeWindow,
  createState,
  detachTabToNewWindow,
  dropPos,
  findTab,
  focusWindow,
  minimizeWindow,
  modelSlotsFree,
  moveTabToWindow,
  openInNewWindow,
  openTabInWindow,
  requestModel,
  restoreWindow,
  setActiveTab,
  tabKey,
} from '../viewer/windows';
import type { WindowState, WindowTab } from '../viewer/windows';

const el = (id: string): WindowTab => ({
  key: tabKey('element', id),
  kind: 'element',
  id,
  label: id,
});
const res = (iri: string): WindowTab => ({
  key: tabKey('resource', iri),
  kind: 'resource',
  id: iri,
  label: iri,
});

/** Open n element windows named e1…en, returning the state. */
function withWindows(n: number): WindowState {
  let s = createState();
  for (let i = 1; i <= n; i++) s = openInNewWindow(s, el(`e${i}`));
  return s;
}

describe('viewer window/tab state', () => {
  it('opens a subject once — a second open focuses the existing window', () => {
    let s = openInNewWindow(createState(), el('e1'));
    s = openInNewWindow(s, el('e2'));
    const beforeZ = s.windows[0].z;

    s = openInNewWindow(s, el('e1'));
    expect(s.windows).toHaveLength(2);
    expect(s.windows[0].z).toBeGreaterThan(beforeZ);
    expect(s.windows[0].z).toBe(s.zTop);
  });

  it('openTabInWindow appends a tab and activates it', () => {
    let s = openInNewWindow(createState(), el('e1'));
    const wid = s.windows[0].wid;
    s = openTabInWindow(s, wid, res('http://x/1'));

    expect(s.windows).toHaveLength(1);
    expect(s.windows[0].tabs.map((t) => t.id)).toEqual(['e1', 'http://x/1']);
    expect(activeTabOf(s.windows[0])?.id).toBe('http://x/1');
  });

  it('openTabInWindow focuses a subject already open in ANOTHER window', () => {
    let s = withWindows(2);
    const [w1, w2] = s.windows.map((w) => w.wid);
    s = focusWindow(s, w1);

    s = openTabInWindow(s, w1, el('e2'));
    expect(s.windows).toHaveLength(2);
    expect(s.windows.find((w) => w.wid === w1)!.tabs).toHaveLength(1);
    expect(s.windows.find((w) => w.wid === w2)!.tabs).toHaveLength(1);
    // …and that other window came to the front instead of the tab being copied.
    expect(s.windows.find((w) => w.wid === w2)!.z).toBe(s.zTop);
  });

  it('closeTab activates the right-hand neighbour, then the left', () => {
    let s = openInNewWindow(createState(), el('a'));
    const wid = s.windows[0].wid;
    s = openTabInWindow(s, wid, el('b'));
    s = openTabInWindow(s, wid, el('c'));
    s = setActiveTab(s, wid, tabKey('element', 'b'));

    s = closeTab(s, wid, tabKey('element', 'b'));
    expect(activeTabOf(s.windows[0])?.id).toBe('c');

    s = closeTab(s, wid, tabKey('element', 'c'));
    expect(activeTabOf(s.windows[0])?.id).toBe('a');
  });

  it('closing the last tab closes the window', () => {
    let s = openInNewWindow(createState(), el('a'));
    s = closeTab(s, s.windows[0].wid, tabKey('element', 'a'));
    expect(s.windows).toHaveLength(0);
  });

  it('moveTabToWindow joins the target, activates the tab and drops an emptied source', () => {
    let s = withWindows(2);
    const [w1, w2] = s.windows.map((w) => w.wid);

    s = moveTabToWindow(s, w1, tabKey('element', 'e1'), w2);
    expect(s.windows).toHaveLength(1);
    expect(s.windows[0].wid).toBe(w2);
    expect(s.windows[0].tabs.map((t) => t.id)).toEqual(['e2', 'e1']);
    expect(activeTabOf(s.windows[0])?.id).toBe('e1');
  });

  it('detachTabToNewWindow places the new window at the drop point', () => {
    let s = openInNewWindow(createState(), el('a'));
    const wid = s.windows[0].wid;
    s = openTabInWindow(s, wid, el('b'));

    s = detachTabToNewWindow(s, wid, tabKey('element', 'b'), { x: 210, y: 90 });
    expect(s.windows).toHaveLength(2);
    expect(s.windows[0].tabs.map((t) => t.id)).toEqual(['a']);
    const detached = s.windows[1];
    expect(detached.pos).toEqual({ x: 210, y: 90 });
    expect(detached.tabs.map((t) => t.id)).toEqual(['b']);
  });

  it('detaching a lone tab just moves its window (never spawns an empty one)', () => {
    let s = openInNewWindow(createState(), el('a'));
    const wid = s.windows[0].wid;
    s = detachTabToNewWindow(s, wid, tabKey('element', 'a'), { x: 40, y: 40 });
    expect(s.windows).toHaveLength(1);
    expect(s.windows[0].pos).toEqual({ x: 40, y: 40 });
  });

  it('minimised windows do not consume a 3D slot, and restoring reclaims one', () => {
    let s = createState();
    s = openInNewWindow(s, el('m1'), { wantsModel: true });
    s = openInNewWindow(s, el('m2'), { wantsModel: true });
    const [w1, w2] = s.windows.map((w) => w.wid);
    expect(modelSlotsFree(s)).toBe(0);

    s = minimizeWindow(s, w1);
    expect(modelSlotsFree(s)).toBe(MODEL_CAP - 1);

    // A third window can now take the freed slot without evicting anyone.
    s = openInNewWindow(s, el('m3'));
    const w3 = s.windows.find((w) => w.wid !== w1 && w.wid !== w2)!.wid;
    s = requestModel(s, w3);
    expect(s.windows.find((w) => w.wid === w2)!.loadModel).toBe(true);
    expect(s.windows.find((w) => w.wid === w3)!.loadModel).toBe(true);

    // Restoring the minimised holder pushes the budget over: the lowest-stacked
    // live viewer (w2) is the one revoked.
    s = restoreWindow(s, w1);
    expect(s.windows.find((w) => w.wid === w1)!.loadModel).toBe(true);
    expect(s.windows.find((w) => w.wid === w2)!.loadModel).toBe(false);
    expect(s.windows.find((w) => w.wid === w3)!.loadModel).toBe(true);
  });

  it('granting a 3D slot never re-stacks the window', () => {
    let s = createState();
    s = openInNewWindow(s, el('a'));
    s = openInNewWindow(s, el('b'));
    const [w1, w2] = s.windows.map((w) => w.wid);
    const zBefore = s.windows.map((w) => w.z);

    s = requestModel(s, w1);
    expect(s.windows.map((w) => w.z)).toEqual(zBefore);
    // …so the window the user is reading (w2, on top) stays on top.
    expect([...s.windows].sort((a, b) => b.z - a.z)[0].wid).toBe(w2);
    expect(s.windows.find((w) => w.wid === w1)!.loadModel).toBe(true);
  });

  it('evicts the lowest-stacked window at the cap, not the first opened', () => {
    let s = withWindows(MAX_WINDOWS);
    const first = s.windows[0].wid;
    const second = s.windows[1].wid;
    s = focusWindow(s, first); // the user just looked at this one

    s = openInNewWindow(s, el('extra'));
    expect(s.windows).toHaveLength(MAX_WINDOWS);
    expect(s.windows.some((w) => w.wid === first)).toBe(true);
    expect(s.windows.some((w) => w.wid === second)).toBe(false);
  });

  it('drops the oldest inactive tab past the per-window cap', () => {
    let s = openInNewWindow(createState(), el('t0'));
    const wid = s.windows[0].wid;
    for (let i = 1; i <= MAX_TABS_PER_WINDOW; i++) s = openTabInWindow(s, wid, el(`t${i}`));

    const w = s.windows[0];
    expect(w.tabs).toHaveLength(MAX_TABS_PER_WINDOW);
    expect(w.tabs.map((t) => t.id)).not.toContain('t0');
    expect(activeTabOf(w)?.id).toBe(`t${MAX_TABS_PER_WINDOW}`);
  });

  it('never mutates the previous state', () => {
    const before = withWindows(2);
    const snapshot = JSON.stringify(before);

    const after = openTabInWindow(before, before.windows[0].wid, res('http://x/9'));
    expect(after).not.toBe(before);
    expect(after.windows).not.toBe(before.windows);
    expect(JSON.stringify(before)).toBe(snapshot);
  });

  it('returns the SAME state for no-ops so callers can skip a re-render', () => {
    const s = withWindows(1);
    const wid = s.windows[0].wid;
    expect(focusWindow(s, wid)).toBe(s); // already on top
    expect(focusWindow(s, 'nope')).toBe(s);
    expect(setActiveTab(s, wid, s.windows[0].activeKey)).toBe(s);
    expect(closeWindow(s, 'nope')).toBe(s);
    expect(moveTabToWindow(s, wid, s.windows[0].activeKey, wid)).toBe(s);
  });

  it('keeps z strictly monotonic and puts the focused window on top', () => {
    let s = withWindows(3);
    const zs = s.windows.map((w) => w.z);
    expect(zs).toEqual([...zs].sort((a, b) => a - b));

    const first = s.windows[0].wid;
    s = focusWindow(s, first);
    expect(s.windows.find((w) => w.wid === first)!.z).toBe(s.zTop);
    expect(Math.max(...s.windows.map((w) => w.z))).toBe(s.zTop);
  });

  it('findTab locates a tab across windows', () => {
    let s = withWindows(2);
    s = openTabInWindow(s, s.windows[1].wid, res('http://x/7'));
    expect(findTab(s, tabKey('resource', 'http://x/7'))?.wid).toBe(s.windows[1].wid);
    expect(findTab(s, tabKey('resource', 'http://x/nope'))).toBeNull();
  });

  it('dropPos re-anchors a viewport point and keeps the window on screen', () => {
    const viewport = { width: 1440, height: 900 };
    // The CSS origin at 1440×900 is (1440/2 − 648, 900/2 − 290) = (72, 160).
    expect(dropPos({ x: 372, y: 260 }, viewport)).toEqual({ x: 300, y: 100 });
    // Far off-screen drops are clamped back into view.
    const far = dropPos({ x: 5000, y: 5000 }, viewport);
    expect(72 + far.x).toBeLessThanOrEqual(viewport.width - 120);
    expect(160 + far.y).toBeLessThanOrEqual(viewport.height - 60);
    const near = dropPos({ x: -5000, y: -5000 }, viewport);
    expect(160 + near.y).toBeGreaterThanOrEqual(0);
  });
});
