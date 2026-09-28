/**
 * Scope helpers for the Triple Browser.
 *
 * The browse scope — the datasets and organisations the user has picked, plus
 * their per-dataset version pins — drives three separate requests (the result
 * page, the exact count and the facet rail). They stay consistent only because
 * they all go through `buildScopeParams()`, which is why it lives here rather
 * than inside the 2 700-line page: the page cannot be mounted in a unit test.
 *
 * The same selection is also what the user expects to find again after closing
 * the browser, so the localStorage snapshot lives alongside it.
 */

export interface ScopeItem {
  type: 'dataset' | 'org';
  id: string;
  name?: string;
}

export interface RememberedScope {
  items: ScopeItem[];
  versions: Record<string, string>;
}

/** A version selector meaning "whatever is live", i.e. no snapshot pin. */
const LIVE_SENTINELS = ['', 'live', 'latest', 'current'];
export const isPinnedVersion = (v: string | undefined | null): boolean =>
  !!v && !LIVE_SENTINELS.includes(v);

/**
 * Scope (dataset/org) + version params shared by every fetch path.
 *
 * `scopeItems` is the user's literal selection; `datasetIds` is that selection
 * with org items already expanded to the datasets they own, which is the set the
 * version pins apply to.
 *
 * The two single-selection shapes (`dataset_id`, `org_id`) are what the endpoint
 * has always been sent and stay exactly as they were. Anything wider goes out as
 * `dataset_ids` + `org_ids`, which the backend unions. `org_id` is repeated
 * alongside `org_ids` so a backend that predates the plural parameter still sees
 * the first organisation instead of losing the organisation half of the scope
 * altogether — which is the bug this fixes: mixing datasets with an organisation
 * used to send `dataset_ids` plus an `org_id` that the backend then ignored.
 */
export function buildScopeParams({
  scopeItems = [] as ScopeItem[],
  datasetIds = [] as string[],
  versions = {} as Record<string, string>,
} = {}): Record<string, string> {
  const params: Record<string, string> = {};
  const dsIds = scopeItems.filter((s) => s.type === 'dataset').map((s) => s.id);
  const orgIds = scopeItems.filter((s) => s.type === 'org').map((s) => s.id);

  if (dsIds.length === 1 && orgIds.length === 0) {
    params.dataset_id = dsIds[0];
  } else if (dsIds.length === 0 && orgIds.length === 1) {
    params.org_id = orgIds[0];
  } else {
    if (dsIds.length) params.dataset_ids = dsIds.join(',');
    if (orgIds.length) {
      params.org_ids = orgIds.join(',');
      params.org_id = orgIds[0];
    }
  }

  const verPairs = datasetIds
    .filter((id) => isPinnedVersion(versions[id]))
    .map((id) => `${id}:${versions[id]}`);
  if (verPairs.length) params.versions = verPairs.join(',');
  return params;
}

/**
 * Where the remembered scope lives. Versioned: a snapshot written by an older
 * build is ignored rather than fed to a page that no longer understands it.
 */
export const SCOPE_STORAGE_KEY = 'ots:browse:scope:v1';
const SCOPE_VERSION = 1;

/** Anything Storage-shaped; `null` when the environment has no storage at all. */
export interface ScopeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

function defaultStorage(): ScopeStorage | null {
  try {
    return typeof localStorage !== 'undefined' ? localStorage : null;
  } catch {
    return null; // a locked-down browser throws on the property access itself
  }
}

/**
 * Remember the current scope. An empty selection is a deliberate choice ("show
 * me everything"), so it is written out like any other and NOT treated as
 * "nothing saved" on the way back in.
 */
export function saveScope(
  items: ScopeItem[],
  versions: Record<string, string> = {},
  storage: ScopeStorage | null | undefined = defaultStorage(),
): void {
  if (!storage) return;
  try {
    // Only the pins that are actually pinned are worth keeping; a live selector
    // restores as live anyway.
    const pins: Record<string, string> = {};
    for (const [id, v] of Object.entries(versions || {})) if (isPinnedVersion(v)) pins[id] = v;
    storage.setItem(
      SCOPE_STORAGE_KEY,
      JSON.stringify({
        v: SCOPE_VERSION,
        items: (items || []).map((s) => ({ type: s.type, id: s.id, name: s.name })),
        versions: pins,
      }),
    );
  } catch { /* private windows and full quotas throw; forgetting the scope is not fatal */ }
}

/** The remembered scope, or null when there is none we can trust. */
export function loadScope(
  storage: ScopeStorage | null | undefined = defaultStorage(),
): RememberedScope | null {
  if (!storage) return null;
  try {
    const raw = storage.getItem(SCOPE_STORAGE_KEY);
    if (!raw) return null;
    const snap = JSON.parse(raw);
    if (!snap || snap.v !== SCOPE_VERSION || !Array.isArray(snap.items)) return null;
    const items = snap.items
      .filter((s) => s && (s.type === 'dataset' || s.type === 'org') && s.id)
      .map((s) => ({ type: s.type, id: String(s.id), name: s.name ?? String(s.id) }));
    return { items, versions: snap.versions && typeof snap.versions === 'object' ? snap.versions : {} };
  } catch {
    return null;
  }
}

/**
 * Drop remembered scope items the user can no longer see — a deleted dataset, or
 * one that belongs to an organisation they have since left. Silently: a stale
 * pick should cost the user nothing, not an error or an empty result page.
 *
 * Call this only with inventories that actually loaded; an empty list because a
 * request failed would otherwise wipe a perfectly good scope.
 */
export function reconcileScope(
  scope: RememberedScope,
  inventory: { datasets?: Array<{ id: string }>; orgs?: Array<{ id: string }> },
): RememberedScope {
  const known = {
    dataset: new Set((inventory.datasets || []).map((d) => String(d.id))),
    org: new Set((inventory.orgs || []).map((o) => String(o.id))),
  };
  const items = (scope.items || []).filter((s) => known[s.type]?.has(String(s.id)));
  const versions: Record<string, string> = {};
  for (const [id, v] of Object.entries(scope.versions || {})) {
    if (known.dataset.has(String(id))) versions[id] = v;
  }
  return { items, versions };
}
