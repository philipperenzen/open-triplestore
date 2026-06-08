const API_BASE = '';

// In-memory token storage (M-2: avoids localStorage XSS exposure).
// The server sets HttpOnly cookies for the actual auth; these in-memory copies
// are only used to know whether we are logged in and to populate UI state.
let _accessToken: string | null = null;
let _refreshToken: string | null = null;

class ApiError extends Error {
  status?: number;
}

function getAccessToken(): string | null {
  return _accessToken;
}

function getRefreshToken(): string | null {
  return _refreshToken;
}

export function setTokens(accessToken: string | null, refreshToken: string | null): void {
  _accessToken = accessToken || null;
  if (refreshToken) _refreshToken = refreshToken;
}

export function clearTokens(): void {
  _accessToken = null;
  _refreshToken = null;
}

// Backward compat
export function setToken(token: string | null): void { setTokens(token, null); }
export function clearToken(): void { clearTokens(); }

function authHeaders() {
  const headers = { 'Content-Type': 'application/json' };
  // Do NOT set Authorization header — the HttpOnly cookie is sent automatically
  // by the browser when credentials: 'include' is set (M-2).
  return headers;
}

async function extractErrorMessage(res) {
  try {
    const text = await res.text();
    try {
      const json = JSON.parse(text);
      const msg = json.message || json.error || json.detail;
      if (msg) return msg;
    } catch {}
    const stripped = text.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
    return stripped || res.statusText;
  } catch {
    return res.statusText;
  }
}

let isRefreshing = false;
let refreshPromise = null;

export async function tryRefreshToken() {
  if (isRefreshing) {
    return refreshPromise;
  }

  isRefreshing = true;
  refreshPromise = (async () => {
    try {
      // M-2: send credentials so the HttpOnly refresh_token cookie is included;
      // no explicit refresh_token body is required.
      const res = await fetch(`${API_BASE}/api/auth/refresh`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        credentials: 'include',
        body: '{}',
      });
      if (!res.ok) return false;
      const data = await res.json();
      setTokens(data.access_token, data.refresh_token);
      return true;
    } catch {
      return false;
    } finally {
      isRefreshing = false;
      refreshPromise = null;
    }
  })();
  return refreshPromise;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// Compute backoff delay for a 429 response, honoring Retry-After when present.
function backoffDelayMs(res: Response, attempt: number): number {
  const ra = res.headers.get('Retry-After');
  if (ra) {
    const secs = Number(ra);
    if (!Number.isNaN(secs) && secs >= 0) return Math.min(secs * 1000, 5000);
  }
  return Math.min(500 * Math.pow(2, attempt), 4000);
}

async function request(method, path, body = null) {
  const opts: RequestInit = { method, headers: authHeaders(), credentials: 'include' };
  if (body) opts.body = JSON.stringify(body);
  let res = await fetch(`${API_BASE}${path}`, opts);

  // Transparent retry on 429 (rate limited).
  for (let attempt = 0; attempt < 3 && res.status === 429; attempt++) {
    await sleep(backoffDelayMs(res, attempt));
    res = await fetch(`${API_BASE}${path}`, opts);
  }

  if (res.status === 401 && true /* always try refresh if 401 */) {
    const wasAuthenticated = _accessToken !== null;
    const refreshed = await tryRefreshToken();
    if (refreshed) {
      opts.headers = authHeaders();
      res = await fetch(`${API_BASE}${path}`, opts);
    } else {
      clearTokens();
      // Only redirect to login if the user was previously authenticated.
      // Anonymous users hitting a protected endpoint should not be redirected.
      if (wasAuthenticated) {
        window.dispatchEvent(new CustomEvent('auth-expired'));
      }
    }
  }

  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    err.status = res.status;
    throw err;
  }
  const ct = res.headers.get('content-type') || '';
  if (ct.includes('application/json')) return res.json();
  return res.text();
}

// Like `request`, but sends a raw text body with a custom Content-Type (e.g.
// Turtle). Uses the same cookie-based auth + 429/401 handling as `request`.
async function requestRaw(method, path, body, contentType) {
  const makeOpts = (): RequestInit => ({ method, headers: { 'Content-Type': contentType }, credentials: 'include', body });
  let res = await fetch(`${API_BASE}${path}`, makeOpts());
  for (let attempt = 0; attempt < 3 && res.status === 429; attempt++) {
    await sleep(backoffDelayMs(res, attempt));
    res = await fetch(`${API_BASE}${path}`, makeOpts());
  }
  if (res.status === 401) {
    const wasAuthenticated = _accessToken !== null;
    const refreshed = await tryRefreshToken();
    if (refreshed) {
      res = await fetch(`${API_BASE}${path}`, makeOpts());
    } else {
      clearTokens();
      if (wasAuthenticated) window.dispatchEvent(new CustomEvent('auth-expired'));
    }
  }
  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    err.status = res.status;
    throw err;
  }
  const ct = res.headers.get('content-type') || '';
  if (ct.includes('application/json')) return res.json();
  return res.text();
}

// Auth
export const login = (username, password) =>
  request('POST', '/api/auth/login', { username, password });

export const register = (username, email, password) =>
  request('POST', '/api/auth/register', { username, email, password });

export const refreshAccessToken = () => {
  const refreshToken = getRefreshToken();
  return request('POST', '/api/auth/refresh', { refresh_token: refreshToken });
};

export const logout = async () => {
  try {
    // M-2: send credentials so the HttpOnly cookies are cleared server-side
    await fetch(`${API_BASE}/api/auth/logout`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'include',
      body: '{}',
    });
  } catch {}
  clearTokens();
};

export const getMe = () => request('GET', '/api/auth/me');
export const updateMe = (data) => request('PUT', '/api/auth/me', data);
export const changePassword = (current_password, new_password) =>
  request('POST', '/api/auth/change-password', { current_password, new_password });

// API Tokens
export const listApiTokens = () => request('GET', '/api/auth/tokens');
export const createApiToken = (data) => request('POST', '/api/auth/tokens', data);
export const revokeApiToken = (tokenId) => request('DELETE', `/api/auth/tokens/${tokenId}`);

// Self-service account management
export const selfDeactivate = (password) =>
  request('DELETE', '/api/auth/account', { password });
export const selfPurge = (password) =>
  request('POST', '/api/auth/account/purge', { password });

// Admin user management
export const adminListUsers = (params = {}) => {
  const qs = new URLSearchParams(params).toString();
  return request('GET', `/api/admin/users?${qs}`);
};
export const adminCreateUser = (data) => request('POST', '/api/admin/users', data);
export const adminGetUser = (id) => request('GET', `/api/admin/users/${id}`);
export const adminUpdateUser = (id, data) => request('PUT', `/api/admin/users/${id}`, data);
export const adminDeleteUser = (id) => request('DELETE', `/api/admin/users/${id}`);
export const adminResetPassword = (id, new_password) =>
  request('POST', `/api/admin/users/${id}/reset-password`, { new_password });
export const adminPurgeUser = (id) => request('POST', `/api/admin/users/${id}/purge`);

// Public user directory (no auth required — returns only users with is_public=true)
export const listPublicUsers = () => request('GET', '/api/users/public');

// Organisations
export const listOrganisations = () => request('GET', '/api/organisations');
export const createOrganisation = (data) => request('POST', '/api/organisations', data);
export const getOrganisation = (id) => request('GET', `/api/organisations/${id}`);
export const updateOrganisation = (id, data) => request('PUT', `/api/organisations/${id}`, data);
export const deleteOrganisation = (id) => request('DELETE', `/api/organisations/${id}`);
export const listOrgMembers = (orgId) => request('GET', `/api/organisations/${orgId}/members`);
export const addOrgMember = (orgId, data) => request('POST', `/api/organisations/${orgId}/members`, data);
export const removeOrgMember = (orgId, userId) => request('DELETE', `/api/organisations/${orgId}/members/${userId}`);
export const updateOrgMemberRole = (orgId, userId, role: string) => request('PUT', `/api/organisations/${orgId}/members/${userId}`, { role });

// Groups
export const listGroups = (orgId) => request('GET', `/api/organisations/${orgId}/groups`);
export const listGroupMembers = (orgId, groupId) =>
  request('GET', `/api/organisations/${orgId}/groups/${groupId}/members`);
export const createGroup = (orgId, data) => request('POST', `/api/organisations/${orgId}/groups`, data);
export const getGroup = (orgId, id) => request('GET', `/api/organisations/${orgId}/groups/${id}`);
export const updateGroup = (orgId, id, data) => request('PUT', `/api/organisations/${orgId}/groups/${id}`, data);
export const deleteGroup = (orgId, id) => request('DELETE', `/api/organisations/${orgId}/groups/${id}`);
export const addGroupMember = (orgId, groupId, data: { user_id: string; role: string }) =>
  request('POST', `/api/organisations/${orgId}/groups/${groupId}/members`, data);
export const removeGroupMember = (orgId, groupId, userId) =>
  request('DELETE', `/api/organisations/${orgId}/groups/${groupId}/members/${userId}`);

// Datasets
export const listDatasets = () => request('GET', '/api/datasets');
export const createDataset = (data) => request('POST', '/api/datasets', data);
export const getDataset = (id) => request('GET', `/api/datasets/${id}`);
export const updateDataset = (id, data) => request('PUT', `/api/datasets/${id}`, data);
export const deleteDataset = (id) => request('DELETE', `/api/datasets/${id}`);
export const listDatasetAccess = (id) => request('GET', `/api/datasets/${id}/access`);
export const grantDatasetAccess = (id, userId: string) => request('POST', `/api/datasets/${id}/access`, { user_id: userId });
export const revokeDatasetAccess = (id, userId: string) => request('DELETE', `/api/datasets/${id}/access/${userId}`);

// Role-based per-resource grants (viewer | editor | admin) for a principal
// (user | group). These override the role a user derives from org/group
// membership, and can both elevate and restrict.
export type ResourceGrant = {
  id: string;
  resource_type: string;
  resource_id: string;
  principal_type: 'user' | 'group';
  principal_id: string;
  role: 'viewer' | 'editor' | 'admin';
};
export const listDatasetGrants = (id): Promise<ResourceGrant[]> => request('GET', `/api/datasets/${id}/grants`);
export const setDatasetGrant = (id, data: { principal_type: 'user' | 'group'; principal_id: string; role: 'viewer' | 'editor' | 'admin' }) =>
  request('PUT', `/api/datasets/${id}/grants`, data);
export const revokeDatasetGrant = (id, principalType: 'user' | 'group', principalId: string) =>
  request('DELETE', `/api/datasets/${id}/grants/${principalType}/${principalId}`);
export const updateDatasetShacl = (id, data) => request('PUT', `/api/datasets/${id}/shacl`, data);
export const updateDatasetRole = (id, graph_role) => request('PUT', `/api/datasets/${id}/role`, { graph_role });
export const detectShapes = (graphIri: string) => request('GET', `/api/shacl/detect-shapes?graph=${encodeURIComponent(graphIri)}`);
export const listAccessibleShapeGraphs = () => request('GET', '/api/shacl/dataset-shape-graphs');
export type DatasetGraph = { graph_iri: string; graph_role?: string | null; private?: boolean; triple_count?: number };

export const listDatasetGraphs = (id): Promise<DatasetGraph[]> => request('GET', `/api/datasets/${id}/graphs`);
export const getDatasetCommits = (id) => request('GET', `/api/datasets/${id}/commits`);
export const addDatasetGraph = (id, data: { graph_iri: string; graph_role?: string }) => request('POST', `/api/datasets/${id}/graphs`, data);
export const removeDatasetGraph = (id, data) => request('DELETE', `/api/datasets/${id}/graphs`, data);
export const updateGraphRole = (datasetId: string, graphIri: string, graphRole: string | null) =>
  request('PATCH', `/api/datasets/${datasetId}/graphs`, { graph_iri: graphIri, graph_role: graphRole });
export const setGraphPrivacy = (datasetId: string, graphIri: string, isPrivate: boolean) =>
  request('PATCH', `/api/datasets/${datasetId}/graphs`, { graph_iri: graphIri, private: isPrivate });

// ── Saved / versioned SPARQL queries ──────────────────────────────────────────
// `scope` is the URL segment: 'datasets' | 'organisations' | 'groups'.
export type SavedQueryScope = 'datasets' | 'organisations' | 'groups';
export type SavedQueryParam = {
  name: string;
  type?: 'iri' | 'string' | 'integer' | 'decimal' | 'boolean' | 'date' | 'dateTime';
  required?: boolean;
  default?: string | null;
  description?: string | null;
};
export type SavedQuery = {
  id: string;
  scope: 'dataset' | 'organisation' | 'group';
  owner_id: string;
  name: string;
  slug: string;
  description?: string | null;
  current_revision: number;
  parameters: SavedQueryParam[];
  test_parameters?: Record<string, string> | null;
  visibility?: string | null;
  is_active: boolean;
  sparql?: string | null;
  created_at: string;
  updated_at: string;
  // Attached by the list endpoint (not on get/create): where the service reads
  // its data from, and whether the current caller may edit this specific service.
  reads_from?: SavedQueryReadsFrom | null;
  can_write?: boolean;
};
export type SavedQueryReadsFrom = {
  kind: 'dataset' | 'organisation' | 'group';
  dataset_id?: string;
  dataset_name?: string | null;
  datasets?: { id: string; name: string }[];
};
export type SavedQueryRevision = {
  revision: number;
  // Optional commit-style title for this revision (mirrors dataset-version naming).
  name?: string | null;
  sparql: string;
  note?: string | null;
  origin: string; // 'manual' | 'llm_repair' | 'import'
  created_by?: string | null;
  created_at: string;
};
export type SavedQueryTest = {
  id: string;
  revision: number;
  dataset_version: string;
  prev_version?: string | null;
  status: 'ok' | 'changed' | 'error';
  result_rowcount?: number | null;
  error_message?: string | null;
  acknowledged: boolean;
  acknowledged_by?: string | null;
  created_at: string;
};

const sqBase = (scope: SavedQueryScope, ownerId: string) =>
  `/api/${scope}/${encodeURIComponent(ownerId)}/api-services`;

export const listSavedQueries = (scope: SavedQueryScope, ownerId: string): Promise<{ queries: SavedQuery[]; can_write: boolean }> =>
  request('GET', sqBase(scope, ownerId));
export const getSavedQuery = (scope: SavedQueryScope, ownerId: string, slug: string): Promise<SavedQuery> =>
  request('GET', `${sqBase(scope, ownerId)}/${encodeURIComponent(slug)}`);
export const createSavedQuery = (scope: SavedQueryScope, ownerId: string, data): Promise<SavedQuery> =>
  request('POST', sqBase(scope, ownerId), data);
export const updateSavedQuery = (scope: SavedQueryScope, ownerId: string, slug: string, data): Promise<SavedQuery> =>
  request('PUT', `${sqBase(scope, ownerId)}/${encodeURIComponent(slug)}`, data);
export const deleteSavedQuery = (scope: SavedQueryScope, ownerId: string, slug: string) =>
  request('DELETE', `${sqBase(scope, ownerId)}/${encodeURIComponent(slug)}`);
export const listSavedQueryRevisions = (scope: SavedQueryScope, ownerId: string, slug: string): Promise<{ revisions: SavedQueryRevision[] }> =>
  request('GET', `${sqBase(scope, ownerId)}/${encodeURIComponent(slug)}/revisions`);
export const listSavedQueryTests = (scope: SavedQueryScope, ownerId: string, slug: string): Promise<{ tests: SavedQueryTest[] }> =>
  request('GET', `${sqBase(scope, ownerId)}/${encodeURIComponent(slug)}/tests`);
export const acknowledgeSavedQueryTest = (scope: SavedQueryScope, ownerId: string, slug: string, testId: string) =>
  request('POST', `${sqBase(scope, ownerId)}/${encodeURIComponent(slug)}/tests/${encodeURIComponent(testId)}/ack`);
export const repairSavedQuery = (scope: SavedQueryScope, ownerId: string, slug: string, data): Promise<{ sparql: string; model: string; savedRevision: number | null }> =>
  request('POST', `${sqBase(scope, ownerId)}/${encodeURIComponent(slug)}/repair`, data);
export const savedQueryOpenApiUrl = (scope: SavedQueryScope, ownerId: string) =>
  `/api/${scope}/${encodeURIComponent(ownerId)}/openapi.json`;

/// Run a saved query as an API. Returns parsed SPARQL-results JSON plus the
/// dataset version that served it (from the `x-ots-dataset-version` header).
export const runSavedQuery = async (
  scope: SavedQueryScope,
  ownerId: string,
  slug: string,
  params: Record<string, string> = {},
  version: string | null = null,
  accept: string = 'application/sparql-results+json',
): Promise<{ data: any; versionServed: string | null; raw: string; contentType: string }> => {
  const qs = new URLSearchParams();
  if (version) qs.set('version', version);
  for (const [k, v] of Object.entries(params)) if (v !== '' && v != null) qs.set(k, String(v));
  const suffix = qs.toString() ? `?${qs.toString()}` : '';
  const res = await fetch(`${API_BASE}${sqBase(scope, ownerId)}/${encodeURIComponent(slug)}/run${suffix}`, {
    headers: { Accept: accept },
    credentials: 'include',
  });
  const text = await res.text();
  if (!res.ok) throw new Error(text || res.statusText);
  const versionServed = res.headers.get('x-ots-dataset-version');
  const contentType = res.headers.get('content-type') || '';
  // Only SPARQL-results JSON is rendered as a table; every other negotiated
  // format (CSV, Turtle, JSON-LD, …) is returned verbatim for display/download.
  let data: any;
  if (contentType.includes('sparql-results+json')) {
    try { data = JSON.parse(text); } catch { data = { raw: text }; }
  } else {
    data = { raw: text };
  }
  return { data, versionServed, raw: text, contentType };
};

// ── Dataset versioning (snapshots + branches) ─────────────────────────────────

export type DatasetGraphMapping = { snapshot_graph: string; source_graph: string };
export type DatasetVersion = {
  dataset_id: string;
  version: string;
  status: 'published' | 'staged' | 'draft' | 'deprecated';
  graph_iri: string;
  snapshot_graphs: string[];
  source_map?: DatasetGraphMapping[];
  created_at: string;
  created_by?: string | null;
  derived_from?: string | null;
  notes?: string | null;
  branch?: string | null;
};

export const listDatasetVersions = (id): Promise<DatasetVersion[]> => request('GET', `/api/datasets/${id}/versions`);
export const getDatasetVersion = (id, ver): Promise<DatasetVersion> => request('GET', `/api/datasets/${id}/versions/${ver}`);
export const createDatasetVersion = (id, data: { version: string; notes?: string; branch?: string; graphs?: string[] }) =>
  request('POST', `/api/datasets/${id}/versions`, data);
export const updateDatasetVersionNotes = (id, ver, notes) => request('PATCH', `/api/datasets/${id}/versions/${ver}`, { notes });
export const stageDatasetVersion = (id, ver) => request('POST', `/api/datasets/${id}/versions/${ver}/stage`);
export const publishDatasetVersion = (id, ver) => request('POST', `/api/datasets/${id}/versions/${ver}/publish`);
export const deprecateDatasetVersion = (id, ver) => request('POST', `/api/datasets/${id}/versions/${ver}/deprecate`);
export const restoreDatasetVersion = (id, ver) => request('POST', `/api/datasets/${id}/versions/${ver}/restore`);
export const getDatasetBranches = (id) => request('GET', `/api/datasets/${id}/branches`);
export const createDatasetBranch = (id, branch, fromVersion, targetVersion?) =>
  request('POST', `/api/datasets/${id}/branches`, { branch, from_version: fromVersion, target_version: targetVersion || undefined });
export function getDatasetVersionDataUrl(id, ver, format?, graph?) {
  let url = `/api/datasets/${id}/versions/${ver}/data?format=${format || 'trig'}`;
  if (graph) url += `&graph=${encodeURIComponent(graph)}`;
  return url;
}

export async function analyzeImport(file: File): Promise<{ total_triples: number; splits: { role: string; triple_count: number; suggested_suffix: string }[]; is_mixed: boolean }> {
  const fd = new FormData();
  fd.append('file', file, file.name);
  const headers: Record<string, string> = {};
  const token = getAccessToken();
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const res = await fetch(`${API_BASE}/api/import/analyze`, { method: 'POST', headers, body: fd, credentials: 'include' });
  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    err.status = res.status;
    throw err;
  }
  return res.json();
}

// Services
export const listServices = (datasetId) => request('GET', `/api/datasets/${datasetId}/services`);
export const createService = (datasetId, data) => request('POST', `/api/datasets/${datasetId}/services`, data);
export const getService = (datasetId, id) => request('GET', `/api/datasets/${datasetId}/services/${id}`);
export const updateService = (datasetId, id, data) => request('PUT', `/api/datasets/${datasetId}/services/${id}`, data);
export const deleteService = (datasetId, id) => request('DELETE', `/api/datasets/${datasetId}/services/${id}`);

// Service Graphs
export const listServiceGraphs = (datasetId, serviceId) => request('GET', `/api/datasets/${datasetId}/services/${serviceId}/graphs`);
export const addServiceGraph = (datasetId, serviceId, data) => request('POST', `/api/datasets/${datasetId}/services/${serviceId}/graphs`, data);
export const removeServiceGraph = (datasetId, serviceId, data) => request('DELETE', `/api/datasets/${datasetId}/services/${serviceId}/graphs`, data);

// Browse
export const browseGraphs = () => request('GET', '/api/browse/graphs');
export const browseSuggest = (
  field,
  prefix,
  limit = 30,
  opts: { dataset?: string | null; predicate?: string | null } = {},
) => {
  const qs = new URLSearchParams({
    field,
    ...(prefix ? { prefix } : {}),
    limit: String(limit),
    ...(opts.dataset ? { dataset: opts.dataset } : {}),
    ...(opts.predicate ? { predicate: opts.predicate } : {}),
  }).toString();
  return request('GET', `/api/browse/suggest?${qs}`);
};
export const browseTriples = (params) => {
  const qs = new URLSearchParams(params).toString();
  return request('GET', `/api/browse/triples?${qs}`);
};
// `opts` may be a bare graph IRI (back-compat) or an object carrying the same
// scope params as browseTriples: { graph, dataset_id, dataset_ids, org_id, versions }.
// Scope lets the graph view expand a resource within the active browse scope
// (dataset/org + version pins) instead of the broad accessible set.
export const browseResource = (iri, opts = {}) => {
  const qs = new URLSearchParams({ iri });
  const o = typeof opts === 'string' ? { graph: opts } : (opts || {});
  for (const k of ['graph', 'dataset_id', 'dataset_ids', 'org_id', 'versions']) {
    if (o[k]) qs.set(k, o[k]);
  }
  return request('GET', `/api/browse/resource?${qs.toString()}`);
};
export const browseStats = () => request('GET', '/api/browse/stats');
// Classes / properties / graphs present in the current scope, with counts.
// Accepts the same scope params as browseTriples (dataset_id, dataset_ids,
// org_id, versions, graph). The chip `filters` JSON may also be passed through.
export const browseFacets = (params) => {
  const qs = new URLSearchParams(params).toString();
  return request('GET', `/api/browse/facets?${qs}`);
};

// SPARQL

/**
 * Detect whether a SPARQL query is a graph-producing query (CONSTRUCT/DESCRIBE)
 * vs a tabular/boolean query (SELECT/ASK).
 */
function isGraphQuery(query) {
  // Strip comments and string literals, then check for CONSTRUCT/DESCRIBE keyword
  const stripped = query
    .replace(/#[^\n]*/g, '')           // line comments
    .replace(/'[^']*'|"[^"]*"/g, '')   // string literals
    .replace(/<[^>]*>/g, '');           // IRIs (avoid matching keywords inside IRIs)
  return /\b(CONSTRUCT|DESCRIBE)\b/i.test(stripped);
}

async function executeSparql(url, query) {
  const token = getAccessToken();
  const graphQ = isGraphQuery(query);
  const headers = {
    'Content-Type': 'application/sparql-query',
    'Accept': graphQ ? 'application/n-triples' : 'application/sparql-results+json',
  };
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const res = await fetch(url, { method: 'POST', headers, body: query });
  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    err.status = res.status;
    throw err;
  }
  if (graphQ) {
    const text = await res.text();
    return { _graphResult: true, ntriples: text };
  }
  return res.json();
}

export const sparqlQuery = (query) =>
  executeSparql(`${API_BASE}/sparql`, query);

/** Query a dataset service, optionally scoped to a version snapshot. A version
 *  of '', 'live', 'latest' or 'current' means live data. */
export const datasetSparqlQuery = (datasetId, serviceSlug, query, version = null) => {
  let url = `${API_BASE}/api/datasets/${encodeURIComponent(datasetId)}/services/${encodeURIComponent(serviceSlug)}/sparql`;
  if (version && !['live', 'latest', 'current'].includes(version)) {
    url += `?version=${encodeURIComponent(version)}`;
  }
  return executeSparql(url, query);
};

// Natural-language → SPARQL via any OpenAI-compatible LLM endpoint.
// Generation only: the returned query is run through the normal scoped SPARQL endpoint,
// so it passes the same authorization boundary as any user-typed query.
export async function nlToSparql(question, schemaHint) {
  const token = getAccessToken();
  const headers = { 'Content-Type': 'application/json' };
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const res = await fetch(`${API_BASE}/api/llm/sparql`, {
    method: 'POST',
    headers,
    body: JSON.stringify({ question, schema_hint: schemaHint || null }),
  });
  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    err.status = res.status;
    throw err;
  }
  return res.json(); // { sparql, model }
}

// Is the NL→SPARQL LLM service reachable? Used to show LLM availability in service health.
export async function llmHealth() {
  try {
    const token = getAccessToken();
    const headers = token ? { Authorization: `Bearer ${token}` } : {};
    const res = await fetch(`${API_BASE}/api/llm/health`, { headers });
    if (!res.ok) return { reachable: false };
    return res.json(); // { gateway, reachable, detail }
  } catch {
    return { reachable: false };
  }
}

// Best-effort training feedback (approve / edit / reject) for a generated SPARQL query.
// Forwarded by the backend to the gateway's /v1/signals; never throws.
export async function sendLlmFeedback(signal) {
  try {
    const token = getAccessToken();
    const headers = { 'Content-Type': 'application/json' };
    if (token) headers['Authorization'] = `Bearer ${token}`;
    await fetch(`${API_BASE}/api/llm/feedback`, {
      method: 'POST',
      headers,
      body: JSON.stringify(signal),
      keepalive: true,
    });
  } catch {
    // feedback is best-effort telemetry
  }
}

// Grounded knowledge-graph chat. Sends the running conversation; the backend grounds
// it in the platform state THIS caller can see (datasets, API services, named graphs)
// and may run a scoped SPARQL query to answer. Returns the assistant turn:
//   { answer, model, ran_query, sparql?, columns?, rows?, truncated }
export async function llmChat(messages, model = null) {
  const token = getAccessToken();
  const headers = { 'Content-Type': 'application/json' };
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const res = await fetch(`${API_BASE}/api/llm/chat`, {
    method: 'POST',
    headers,
    credentials: 'include',
    body: JSON.stringify({ messages, model }),
  });
  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    err.status = res.status;
    throw err;
  }
  return res.json();
}

// SHACL
// Returns { report, run_id, ran_at } — the run is persisted server-side.
// Pass { test: true } for a dry run that validates but is NOT recorded.
export const validateDataset = (datasetId, data = {}, opts: { test?: boolean } = {}) =>
  request('POST', `/api/datasets/${datasetId}/validate${opts.test ? '?test=true' : ''}`, data);
export const getShapes = (datasetId) =>
  request('GET', `/api/datasets/${datasetId}/shapes`);
// Shapes are Turtle text — send as text/turtle via the shared cookie auth path.
export const putShapes = (datasetId, turtle) =>
  requestRaw('PUT', `/api/datasets/${datasetId}/shapes`, turtle, 'text/turtle');

// Validation run history
export const getLatestValidationRun = (datasetId) =>
  request('GET', `/api/datasets/${datasetId}/validation/latest`);
export const getValidationHistory = (datasetId, limit = 50) =>
  request('GET', `/api/datasets/${datasetId}/validation/history?limit=${limit}`);
export const getValidationRun = (datasetId, runId) =>
  request('GET', `/api/datasets/${datasetId}/validation/runs/${encodeURIComponent(runId)}`);
export const listLatestValidationRuns = (datasetIds) =>
  request('POST', `/api/shacl/validation/latest`, { dataset_ids: datasetIds });

// The caller's own dataset usage (count + last_used per dataset), most-used
// first. Powers "recently used / use a lot" ranking. Private telemetry: this
// only ever returns the current user's own footprint.
export const myDatasetUsage = () =>
  request('GET', `/api/me/dataset-usage`);
// Cross-user usage aggregate — super_admin only (server enforces).
export const adminDatasetUsage = (opts: { since?: string; limit?: number } = {}) => {
  const p = new URLSearchParams();
  if (opts.since) p.set('since', opts.since);
  if (opts.limit) p.set('limit', String(opts.limit));
  const qs = p.toString();
  return request('GET', `/api/admin/dataset-usage${qs ? `?${qs}` : ''}`);
};

// Infer (SHACL-AF)
export const inferDataset = (datasetId) =>
  request('POST', `/api/datasets/${datasetId}/infer`, {});

// ─── SHACL Studio: shape graphs (the Library) ────────────────────────────────
export const listShapeGraphs = () => request('GET', '/api/shacl/shape-graphs');
export const createShapeGraph = (body) => request('POST', '/api/shacl/shape-graphs', body);
export const getShapeGraph = (id) => request('GET', `/api/shacl/shape-graphs/${id}`);
export const updateShapeGraph = (id, body) => request('PUT', `/api/shacl/shape-graphs/${id}`, body);
export const deleteShapeGraph = (id) => request('DELETE', `/api/shacl/shape-graphs/${id}`);
export const cloneShapeGraph = (id, body = {}) => request('POST', `/api/shacl/shape-graphs/${id}/clone`, body);
export const listShapeGraphRevisions = (id) => request('GET', `/api/shacl/shape-graphs/${id}/revisions`);
export const getShapeGraphRevision = (id, rev) => request('GET', `/api/shacl/shape-graphs/${id}/revisions/${rev}`);
export const restoreShapeGraphRevision = (id, rev) => request('POST', `/api/shacl/shape-graphs/${id}/restore/${rev}`, {});

// Meta-validation: validate the shape graph's own shapes against the built-in
// SHACL-SHACL meta shapes. Returns a validation report like a pipeline run.
export const validateShapeGraph = (id) => request('POST', `/api/shacl/shape-graphs/${id}/validate`, {});

// ─── Shapes catalog & compose ───────────────────────────────────────────────
// Graph-first shapes catalog. Without a graph → a cheap summary of the graphs
// that hold shapes (`{ graphs: [{graph, node_count, property_count, total,
// registered, shape_graph_id, shape_graph_name}] }`). With a graph → that one
// graph's shapes (`{ graph, shapes: [...] }`). Scales to stores with tens of
// thousands of shapes (e.g. a large information model).
export const listShapesCatalog = (graph?: string) =>
  request('GET', graph ? `/api/shacl/shapes?graph=${encodeURIComponent(graph)}` : '/api/shacl/shapes');
// Copy existing shapes (each with its full closure) into a shape graph — the
// "add existing shapes" / "compose" primitive. Copy semantics.
export const importShapesIntoGraph = (
  id: string,
  shapes: { source_graph: string; shape: string }[],
  note?: string,
) => request('POST', `/api/shacl/shape-graphs/${id}/import-shapes`, { shapes, note });
// Adopt an existing shapes-bearing named graph as a first-class shape graph, in
// place (no copy). Idempotent — returns the existing record if already known.
export const registerShapeGraph = (body: {
  graph_iri: string;
  name: string;
  description?: string;
  visibility?: string;
  tags?: string[];
}) => request('POST', '/api/shacl/register-shape-graph', body);

// Lifecycle transitions (draft → staged → published → deprecated).
export const stageShapeGraph = (id) => request('POST', `/api/shacl/shape-graphs/${id}/stage`, {});
export const publishShapeGraph = (id) => request('POST', `/api/shacl/shape-graphs/${id}/publish`, {});
export const deprecateShapeGraph = (id) => request('POST', `/api/shacl/shape-graphs/${id}/deprecate`, {});

// The shape graph's slice of the shared commit trail (kind = Shapes).
export const getShapeGraphCommits = (id, limit = 50) =>
  request('GET', `/api/shacl/shape-graphs/${id}/commits?limit=${limit}`);

/** Fetch the shape graph's Turtle (or SHACLC via `?format=shaclc`). */
export async function getShapeGraphTurtle(id: string, format: 'turtle' | 'shaclc' = 'turtle'): Promise<string> {
  const url = `/api/shacl/shape-graphs/${id}/turtle${format === 'shaclc' ? '?format=shaclc' : ''}`;
  const res = await fetch(url, { credentials: 'include' });
  if (!res.ok) throw new Error(`Failed to load shape graph turtle: ${res.status}`);
  return res.text();
}

/** Save the shape graph's Turtle (or SHACLC via `contentType="text/shaclc"`). */
export async function putShapeGraphTurtle(id: string, body: string, contentType = 'text/turtle'): Promise<{ version: number }> {
  const res = await fetch(`/api/shacl/shape-graphs/${id}/turtle`, {
    method: 'PUT',
    credentials: 'include',
    headers: { 'Content-Type': contentType },
    body,
  });
  if (!res.ok) throw new Error(`Failed to save shape graph turtle: ${res.status} ${await res.text()}`);
  return res.json();
}

// ─── SHACL Studio: validation pipelines ─────────────────────────────────────
export const listPipelines = () => request('GET', '/api/shacl/pipelines');
export const createPipeline = (body) => request('POST', '/api/shacl/pipelines', body);
export const getPipeline = (id) => request('GET', `/api/shacl/pipelines/${id}`);
export const updatePipeline = (id, body) => request('PUT', `/api/shacl/pipelines/${id}`, body);
export const deletePipeline = (id) => request('DELETE', `/api/shacl/pipelines/${id}`);

// ── In-app documentation (admin-editable; admin-only docs filtered server-side) ──
export const listDocs = () => request('GET', '/api/docs');
export const getDoc = (slug) => request('GET', `/api/docs/${slug}`);
export const saveDoc = (slug, body) => request('PUT', `/api/docs/${slug}`, body);
export const deleteDoc = (slug) => request('DELETE', `/api/docs/${slug}`);
// Pass { test: true } for a dry run that validates but is NOT recorded.
export const runPipeline = (id, opts: { test?: boolean } = {}) =>
  request('POST', `/api/shacl/pipelines/${id}/run${opts.test ? '?test=true' : ''}`, {});
export const listPipelineRuns = (id, limit = 50) => request('GET', `/api/shacl/pipelines/${id}/runs?limit=${limit}`);
export const getPipelineRun = (pipelineId, runId) => request('GET', `/api/shacl/pipelines/${pipelineId}/runs/${runId}`);
export const listLatestPipelineRuns = (pipelineIds) =>
  request('POST', '/api/shacl/pipelines/latest', { pipeline_ids: pipelineIds });

// ─── SHACL Studio: the validation layer (shape ↔ target bindings) ───────────
// A binding records `<target> ots:validatedBy <shapeGraph>` as RDF in a system
// graph. Target kind is 'dataset' | 'graph' | 'shapegraph' (these are the wire
// values the backend deserialises). For a graph the id is the graph IRI; for a
// dataset or shape graph the id is the entity id.
export type ValidationTarget = { kind: 'dataset' | 'graph' | 'shapegraph'; id: string };
export const listBindingsForTarget = (kind: string, id: string) =>
  request('GET', `/api/shacl/bindings?target_kind=${encodeURIComponent(kind)}&target_id=${encodeURIComponent(id)}`);
export const listBindingsForShapeGraph = (shapeGraphId: string) =>
  request('GET', `/api/shacl/bindings?shape_graph_id=${encodeURIComponent(shapeGraphId)}`);
export const createBinding = (target: ValidationTarget, shapeGraphId: string) =>
  request('POST', '/api/shacl/bindings', { target, shape_graph_id: shapeGraphId });
export const deleteBinding = (target: ValidationTarget, shapeGraphId: string) =>
  request('DELETE', '/api/shacl/bindings', { target, shape_graph_id: shapeGraphId });
// The shape graphs that effectively apply to a dataset = its own bindings ∪ each
// contained graph's bindings (dynamic inheritance from graph-attached shapes).
export const getDatasetEffectiveShapes = (datasetId: string) =>
  request('GET', `/api/datasets/${datasetId}/effective-shapes`);

// ─── SHACL Studio: tooling ──────────────────────────────────────────────────
/** Real classes + properties present in the selected scope — feeds the visual builder + autocomplete. */
export const getModelContext = (params: { dataset?: string; graphs?: string[] }) => {
  const q = params.dataset
    ? `?dataset=${encodeURIComponent(params.dataset)}`
    : params.graphs && params.graphs.length
      ? `?graphs=${encodeURIComponent(params.graphs.join(','))}`
      : '';
  return request('GET', `/api/shacl/model-context${q}`);
};

/** Induce a draft SHACL shapes Turtle from existing instance data. */
export const deriveShapes = (body: { dataset_id?: string; graphs?: string[]; target_classes?: string[] }) =>
  request('POST', '/api/shacl/derive', body);

/** Form-manifest contract: dataset + attached shapes + endpoints, ready for an external form platform to load. */
export const getFormManifest = (datasetId: string) =>
  request('GET', `/api/datasets/${datasetId}/form-manifest`);

/**
 * SHACL Studio AI assistant. `task` is one of:
 *  - `'draft'`   — generate SHACL Turtle from a NL description (uses `modelContext` for real IRIs)
 *  - `'explain'` — describe what existing `turtle` validates
 *  - `'improve'` — suggest refinements to existing `turtle`
 * Returns `{ task, model, turtle?, explanation? }`.
 */
export async function aiShacl({ task, description, turtle, modelContext, model }: {
  task: 'draft' | 'explain' | 'improve';
  description?: string;
  turtle?: string;
  modelContext?: unknown;
  model?: string;
}) {
  const token = getAccessToken();
  const headers: Record<string, string> = { 'Content-Type': 'application/json' };
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const res = await fetch(`${API_BASE}/api/llm/shacl`, {
    method: 'POST',
    headers,
    credentials: 'include',
    body: JSON.stringify({ task, description, turtle, model_context: modelContext, model }),
  });
  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    (err as any).status = res.status;
    throw err;
  }
  return res.json();
}

// Health check
export const getHealth = () => request('GET', '/health');

// Graph Store Protocol (raw fetch — needs binary/text body)
export function deleteGraph(graphIri) {
  const token = getAccessToken();
  const headers = {};
  if (token) headers['Authorization'] = `Bearer ${token}`;
  return fetch(`/store?graph=${encodeURIComponent(graphIri)}`, { method: 'DELETE', headers }).then(async (res) => {
    if (!res.ok) throw new Error(await res.text());
    return true;
  });
}

export async function uploadToGraph(graphIri, body, contentType, replace = false) {
  const token = getAccessToken();
  const headers = { 'Content-Type': contentType };
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const method = replace ? 'PUT' : 'POST';
  const url = `/store?graph=${encodeURIComponent(graphIri)}`;
  let res = await fetch(url, { method, headers, body });
  for (let attempt = 0; attempt < 3 && res.status === 429; attempt++) {
    await sleep(backoffDelayMs(res, attempt));
    res = await fetch(url, { method, headers, body });
  }
  if (!res.ok) throw new Error(await res.text());
  return true;
}

/**
 * Bulk multi-file import — uploads N RDF files in a single multipart POST.
 *
 * `entries`: each is one file plus its routing decision.
 *   - `file`: the File/Blob (filename and content-type are read from it)
 *   - `targetGraph`: target graph IRI for triple-format files; for quad
 *      formats this only takes effect when `merge` is true.
 *   - `graphRemap`: for quad formats (merge off), `{ embeddedIri: newTargetIri }`
 *      re-homing each embedded graph to a different write target — used to move
 *      embedded graphs under the dataset namespace so the server's per-graph
 *      write boundary admits them (applied at write time, not a post-import MOVE).
 * `opts`:
 *   - `datasetId`: if set, every touched graph is registered with this dataset.
 *   - `replace`: drop touched graphs before insertion.
 *   - `merge`: force every quad into its file's `targetGraph` even for `.nq`/`.trig`.
 *   - `defaultTargetGraph`: fallback when an entry has no `targetGraph`.
 *
 * Returns the server's BulkResponse (per-file status + aggregate counts).
 */
export async function bulkImport(
  entries: { file: File | Blob; filename?: string; targetGraph?: string; autoSplit?: boolean; replace?: boolean; graphRole?: string; graphRemap?: Record<string, string> }[],
  opts: { datasetId?: string; replace?: boolean; merge?: boolean; defaultTargetGraph?: string; versionBump?: string } = {},
) {
  const fd = new FormData();
  const targets: Record<string, string> = {};
  const autoSplitFiles: string[] = [];
  const replaceFiles: string[] = [];
  const graphRoles: Record<string, string> = {};
  const graphRemap: Record<string, Record<string, string>> = {};
  for (const e of entries) {
    const fname = e.filename || (e.file as File).name || 'upload.bin';
    fd.append('file', e.file, fname);
    if (e.targetGraph) targets[fname] = e.targetGraph;
    if (e.autoSplit) autoSplitFiles.push(fname);
    if (e.replace) replaceFiles.push(fname);
    if (e.graphRole) graphRoles[fname] = e.graphRole;
    if (e.graphRemap && Object.keys(e.graphRemap).length) graphRemap[fname] = e.graphRemap;
  }
  fd.append('meta', JSON.stringify({
    dataset_id: opts.datasetId,
    replace: !!opts.replace,
    merge: !!opts.merge,
    default_target_graph: opts.defaultTargetGraph,
    targets,
    auto_split_files: autoSplitFiles,
    replace_files: replaceFiles,
    graph_roles: graphRoles,
    graph_remap: graphRemap,
    version_bump: opts.versionBump,
  }));

  const headers: Record<string, string> = {};
  const token = getAccessToken();
  if (token) headers['Authorization'] = `Bearer ${token}`;

  const url = `${API_BASE}/api/import/bulk`;
  let res = await fetch(url, { method: 'POST', headers, body: fd });
  for (let attempt = 0; attempt < 3 && res.status === 429; attempt++) {
    await sleep(backoffDelayMs(res, attempt));
    res = await fetch(url, { method: 'POST', headers, body: fd });
  }
  if (res.status === 401 && getRefreshToken()) {
    const refreshed = await tryRefreshToken();
    if (refreshed) {
      const h2 = { ...headers, Authorization: `Bearer ${getAccessToken()}` };
      res = await fetch(url, { method: 'POST', headers: h2, body: fd });
    }
  }
  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    err.status = res.status;
    throw err;
  }
  return res.json();
}

export function exportGraph(graphIri) {
  const token = getAccessToken();
  const headers = { 'Accept': 'text/turtle' };
  if (token) headers['Authorization'] = `Bearer ${token}`;
  return fetch(`/store?graph=${encodeURIComponent(graphIri)}`, { method: 'GET', headers }).then(async (res) => {
    if (!res.ok) throw new Error(await res.text());
    return res.text();
  });
}

// SPARQL Update
export async function sparqlUpdate(update) {
  function buildHeaders() {
    const h = { 'Content-Type': 'application/sparql-update' };
    const token = getAccessToken();
    if (token) h['Authorization'] = `Bearer ${token}`;
    return h;
  }
  let res = await fetch('/sparql', { method: 'POST', headers: buildHeaders(), body: update });
  if (res.status === 401 && getRefreshToken()) {
    const refreshed = await tryRefreshToken();
    if (refreshed) {
      res = await fetch('/sparql', { method: 'POST', headers: buildHeaders(), body: update });
    } else {
      clearTokens();
      window.dispatchEvent(new CustomEvent('auth-expired'));
    }
  }
  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    err.status = res.status;
    throw err;
  }
  return true;
}

// Users (admin) — legacy
export const listUsers = () => request('GET', '/api/users');
export const getUser = (id) => request('GET', `/api/users/${id}`);
export const deleteUser = (id) => request('DELETE', `/api/users/${id}`);

// Image uploads (multipart/form-data)
async function uploadImage(path, file) {
  const token = getAccessToken();
  const formData = new FormData();
  formData.append('file', file);
  const headers = {};
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const res = await fetch(`${API_BASE}${path}`, { method: 'PUT', headers, body: formData });
  if (!res.ok) {
    const msg = await extractErrorMessage(res);
    const err = new ApiError(msg);
    err.status = res.status;
    throw err;
  }
  const ct = res.headers.get('content-type') || '';
  return ct.includes('application/json') ? res.json() : res.text();
}

export const uploadUserAvatar = (file) => uploadImage('/api/users/me/avatar', file);
export const getUserAvatarUrl = (userId) => `${API_BASE}/api/users/${userId}/avatar`;

export const uploadOrgImage = (orgId, file) => uploadImage(`/api/organisations/${orgId}/image`, file);
export const getOrgImageUrl = (orgId) => `${API_BASE}/api/organisations/${orgId}/image`;

export const uploadDatasetImage = (datasetId, file) => uploadImage(`/api/datasets/${datasetId}/image`, file);
export const getDatasetImageUrl = (datasetId) => `${API_BASE}/api/datasets/${datasetId}/image`;

export const uploadOrgBanner = (orgId, file) => uploadImage(`/api/organisations/${orgId}/banner`, file);
export const getOrgBannerUrl = (orgId) => `${API_BASE}/api/organisations/${orgId}/banner`;

export const uploadDatasetBanner = (datasetId, file) => uploadImage(`/api/datasets/${datasetId}/banner`, file);
export const getDatasetBannerUrl = (datasetId) => `${API_BASE}/api/datasets/${datasetId}/banner`;

// Banner presets — select a built-in animated/gradient banner instead of
// uploading. Stored server-side as `banner_key = "preset:<id>"`; passing a
// null/empty preset clears the banner.
export const setOrgBannerPreset = (orgId, preset) =>
  request('PUT', `/api/organisations/${orgId}/banner-preset`, { preset: preset ?? null });
export const clearOrgBanner = (orgId) => setOrgBannerPreset(orgId, null);

export const setDatasetBannerPreset = (datasetId, preset) =>
  request('PUT', `/api/datasets/${datasetId}/banner-preset`, { preset: preset ?? null });
export const clearDatasetBanner = (datasetId) => setDatasetBannerPreset(datasetId, null);

// Assets
export const listAssets = (datasetId) =>
  request('GET', `/api/datasets/${datasetId}/assets`);

export function uploadAsset(datasetId, file, onProgress) {
  const token = getAccessToken();
  const formData = new FormData();
  formData.append('file', file);
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', `/api/datasets/${datasetId}/assets`);
    if (token) xhr.setRequestHeader('Authorization', `Bearer ${token}`);
    if (onProgress) {
      xhr.upload.addEventListener('progress', (e) => {
        if (e.lengthComputable) onProgress(e.loaded / e.total);
      });
    }
    xhr.onload = () => {
      if (xhr.status === 201) {
        resolve(JSON.parse(xhr.responseText));
      } else {
        reject(new Error(xhr.responseText || 'Upload failed'));
      }
    };
    xhr.onerror = () => reject(new Error('Network error'));
    xhr.send(formData);
  });
}

export const deleteAsset = (datasetId, assetId) =>
  request('DELETE', `/api/datasets/${datasetId}/assets/${assetId}`);

export const updateAssetVisibility = (datasetId, assetId, isPublic) =>
  request('PUT', `/api/datasets/${datasetId}/assets/${assetId}/visibility`, { public: isPublic });

export const updateAssetMetadata = (datasetId, assetId, data) =>
  request('PATCH', `/api/datasets/${datasetId}/assets/${assetId}`, data);

// The asset's typed metadata (dimensions, duration, page/point/sheet/row counts, panorama, sha256,
// bbox, thumbnail IRI, schema_class) the triplestore derived on upload. Optional-auth, like the
// linked-data endpoint, so it works for public assets without a token.
export const assetMetadata = (datasetId, assetId) =>
  request('GET', `/api/datasets/${datasetId}/assets/${assetId}/metadata`);

// Fetch raw asset content with auth headers via the API endpoint. Returns a Response.
export function fetchAssetContent(datasetId, assetId) {
  const token = getAccessToken();
  const headers = {};
  if (token) headers['Authorization'] = `Bearer ${token}`;
  return fetch(`/api/datasets/${datasetId}/assets/${assetId}`, { headers });
}

// Auth state
export function isLoggedIn() {
  return !!getAccessToken();
}

// ── Model Registry (OWL/RDFS ontologies and SKOS vocabularies) ─────────────────
// Each entry carries a `kind` ("data-model" | "vocabulary"), auto-detected on upload.

export const listDataModels = () => request('GET', '/api/models');

// A prefix candidate derived from an on-platform registered model/vocabulary.
// Shape lines up with PrefixCandidate in prefixService so the search panel can
// merge these straight into its result list.
export type PlatformPrefix = {
  prefix: string;
  namespace: string;
  title?: string;
  description?: string;
  source: 'platform';
  kind?: 'data-model' | 'vocabulary';
};

// Turn a model title (or id) into a short, valid prefix label: lowercase,
// alnum/underscore, must start with a letter. Falls back to "ns" if nothing usable.
function slugifyPrefix(raw: string): string {
  const base = (raw || '').toLowerCase().replace(/[^a-z0-9]+/g, '');
  const trimmed = base.replace(/^[^a-z]+/, '');
  return (trimmed || base || 'ns').slice(0, 24);
}

/**
 * On-platform prefixes derived from the model registry. For every registered
 * model/vocabulary that declares a `namespace`, yields a prefix candidate whose
 * label is slugified from the model title (falling back to its id). Best-effort:
 * returns [] if the registry can't be loaded (e.g. anonymous user). De-duplicates
 * by namespace and disambiguates colliding prefix labels with a numeric suffix.
 */
export async function listPlatformPrefixes(): Promise<PlatformPrefix[]> {
  let models: any[];
  try {
    models = await listDataModels();
  } catch {
    return [];
  }
  if (!Array.isArray(models)) return [];

  const out: PlatformPrefix[] = [];
  const seenNs = new Set<string>();
  const usedPrefixes = new Set<string>();
  for (const m of models) {
    const namespace = (m?.namespace || '').trim();
    if (!namespace || seenNs.has(namespace)) continue;
    seenNs.add(namespace);

    let prefix = slugifyPrefix(m?.title || m?.id || '');
    if (usedPrefixes.has(prefix)) {
      let i = 2;
      while (usedPrefixes.has(`${prefix}${i}`)) i++;
      prefix = `${prefix}${i}`;
    }
    usedPrefixes.add(prefix);

    out.push({
      prefix,
      namespace,
      title: m?.title || undefined,
      description: m?.description || undefined,
      source: 'platform',
      kind: m?.kind === 'vocabulary' ? 'vocabulary' : 'data-model',
    });
  }
  return out;
}

export const createDataModel = (data) => request('POST', '/api/models', data);
export const getDataModel = (id) => request('GET', `/api/models/${id}`);
export const deleteDataModel = (id) => request('DELETE', `/api/models/${id}`);
export const updateDataModel = (id, data) => request('PATCH', `/api/models/${id}`, data);

export const listDataModelVersions = (id) => request('GET', `/api/models/${id}/versions`);
export const getDataModelVersion = (id, ver) => request('GET', `/api/models/${id}/versions/${ver}`);
export const deleteDataModelVersion = (id, ver) => request('DELETE', `/api/models/${id}/versions/${ver}`);
export const updateDataModelVersionNotes = (id, ver, notes) => request('PATCH', `/api/models/${id}/versions/${ver}`, { notes });
export const stageDataModelVersion = (id, ver) => request('POST', `/api/models/${id}/versions/${ver}/stage`);
export const publishDataModelVersion = (id, ver) => request('POST', `/api/models/${id}/versions/${ver}/publish`);
export const createDataModelDraft = (id, fromVer, targetVer) =>
  request('POST', `/api/models/${id}/versions/${fromVer}/draft`, { target_version: targetVer });
export const getDataModelDiff = (id, from, to) =>
  request('GET', `/api/models/${id}/diff?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`);
export const getDataModelCollaborators = (id) => request('GET', `/api/models/${id}/collaborators`);
export const getDataModelBranches = (id) => request('GET', `/api/models/${id}/branches`);
export const getDataModelCommits = (id, branch) =>
  request('GET', `/api/models/${id}/commits${branch ? `?branch=${encodeURIComponent(branch)}` : ''}`);
export const createDataModelBranch = (id, branch, fromVersion, targetVersion) =>
  request('POST', `/api/models/${id}/branches`, { branch, from_version: fromVersion, target_version: targetVersion || undefined });
export const previewDataModelMerge = (id, from, into) =>
  request('GET', `/api/models/${id}/merge/preview?from=${encodeURIComponent(from)}&into=${encodeURIComponent(into)}`);
export const mergeDataModel = (id, body) => request('POST', `/api/models/${id}/merge`, body);
/** Phase 6 — stage/publish/deprecate a single subgraph of a version. */
export const subgraphActionDataModel = (id, ver, action: 'stage' | 'publish' | 'deprecate', graph) =>
  request('POST', `/api/models/${id}/versions/${ver}/subgraph/${action}`, { graph });

export function uploadDataModelVersion(id, file, versionOverride, notes, merge, isPublic) {
  const token = getAccessToken();
  const headers = {};
  if (token) headers['Authorization'] = `Bearer ${token}`;
  const form = new FormData();
  form.append('file', file);
  if (versionOverride) form.append('version', versionOverride);
  if (notes) form.append('notes', notes);
  if (merge) form.append('merge', 'true');
  form.append('is_public', isPublic ? 'true' : 'false');
  return fetch(`/api/models/${id}/versions`, { method: 'POST', headers, body: form })
    .then(async (res) => {
      if (!res.ok) {
        const msg = await res.text().then(t => {
          try { return JSON.parse(t).message || t; } catch { return t; }
        });
        const err = new ApiError(msg);
        err.status = res.status;
        throw err;
      }
      return res.json();
    });
}

export function getDataModelVersionDataUrl(id, ver, format, graph) {
  let url = `/api/models/${id}/versions/${ver}/data?format=${format || 'trig'}`;
  if (graph) url += `&graph=${encodeURIComponent(graph)}`;
  return url;
}

// ─── OAuth / SSO ──────────────────────────────────────────────────────────────

/** List active SSO providers (public — no auth required). */
export const getOauthProviders = () =>
  request('GET', '/api/auth/oauth/providers');

// ─── Admin: OAuth provider management ────────────────────────────────────────

export const adminListOauthProviders = () =>
  request('GET', '/api/admin/oauth/providers');

export const adminGetOauthProvider = (id) =>
  request('GET', `/api/admin/oauth/providers/${id}`);

export const adminCreateOauthProvider = (data) =>
  request('POST', '/api/admin/oauth/providers', data);

export const adminUpdateOauthProvider = (id, data) =>
  request('PUT', `/api/admin/oauth/providers/${id}`, data);

export const adminDeleteOauthProvider = (id) =>
  request('DELETE', `/api/admin/oauth/providers/${id}`);

// ─── Admin: Endpoint ACL ──────────────────────────────────────────────────────

export const listEndpointAclRules = () =>
  request('GET', '/api/admin/acl/endpoints');

export const createEndpointAclRule = (data) =>
  request('POST', '/api/admin/acl/endpoints', data);

export const updateEndpointAclRule = (id, data) =>
  request('PUT', `/api/admin/acl/endpoints/${id}`, data);

export const deleteEndpointAclRule = (id) =>
  request('DELETE', `/api/admin/acl/endpoints/${id}`);

// ─── Admin: Graph ACL ─────────────────────────────────────────────────────────

export const listGraphAclRules = (graphIri) => {
  const qs = graphIri ? `?graph_iri=${encodeURIComponent(graphIri)}` : '';
  return request('GET', `/api/admin/acl/graphs${qs}`);
};

export const grantGraphPermission = (data) =>
  request('POST', '/api/admin/acl/graphs', data);

export const revokeGraphPermission = (id) =>
  request('DELETE', `/api/admin/acl/graphs/${id}`);

// ─── Admin: Triple Security Labels ───────────────────────────────────────────

export const listTripleSecurityLabels = (graphIri) => {
  const qs = graphIri ? `?graph_iri=${encodeURIComponent(graphIri)}` : '';
  return request('GET', `/api/admin/acl/triples${qs}`);
};

export const createTripleSecurityLabel = (data) =>
  request('POST', '/api/admin/acl/triples', data);

export const deleteTripleSecurityLabel = (id) =>
  request('DELETE', `/api/admin/acl/triples/${id}`);

