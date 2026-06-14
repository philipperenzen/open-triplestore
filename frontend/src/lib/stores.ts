import { writable, derived } from 'svelte/store';
import { getHealth, tryRefreshToken } from './api.js';

export interface User {
  id: string;
  username: string;
  email: string;
  email_verified: boolean;
  totp_enabled: boolean;
  /** New email address awaiting confirmation (from GET /api/auth/me). */
  email_pending?: string;
  role: string;
  is_active: boolean;
  can_publish: boolean;
  [key: string]: unknown;
}

export interface ServiceStatus {
  ok: boolean;
  [key: string]: unknown;
}

export interface HealthStatus {
  status: 'ok' | 'degraded' | null;
  version?: string;
  services: {
    triplestore: ServiceStatus & { triples?: number; graphs?: number };
    database: ServiceStatus;
    object_storage: ServiceStatus & { configured: boolean };
    backup: ServiceStatus & { enabled: boolean };
  } | null;
}

// null = not yet checked
export const backendHealth = writable<HealthStatus | null>(null);

// Derived single boolean for code that only needs online/offline
export const backendOnline = derived(
  backendHealth,
  ($h) => $h === null ? null : $h.status !== null,
);

export async function checkBackend(): Promise<void> {
  try {
    const data = await getHealth();
    backendHealth.set({
      status: data.status === 'ok' ? 'ok' : 'degraded',
      version: data.version,
      services: data.services ?? null,
    });
  } catch {
    backendHealth.set({ status: null, services: null });
  }
}

export const user = writable<User | null>(null);
export const isAuthenticated = writable(false);
export const authInitialized = writable(false);

export const userRole = derived(user, ($user) => $user?.role || 'user');
export const isAdmin = derived(userRole, ($role) => $role === 'admin' || $role === 'super_admin');
export const isSuperAdmin = derived(userRole, ($role) => $role === 'super_admin');

// Shared filter state — written by TripleBrowser, read by GraphVisualizer
export interface BrowseFilters {
  subject: string;
  predicate: string;
  object: string;
  graph: string;
}
export const browseFilters = writable<BrowseFilters>({ subject: '', predicate: '', object: '', graph: '' });

export async function refreshUser(): Promise<void> {
  // Restore the session from the HttpOnly auth cookies (sent automatically via
  // credentials:'include'). A raw fetch is used rather than the request() wrapper
  // so a genuinely-anonymous 401 stays silent (no auth-expired event / redirect).
  const fetchMe = () => fetch('/api/auth/me', {
    credentials: 'include',
    headers: { Accept: 'application/json' },
  });
  try {
    let res = await fetchMe();
    // A 401 means the short-lived access-token cookie is missing or expired. Mint
    // a fresh one from the long-lived refresh-token cookie and retry before giving
    // up — otherwise every page reload past the access-token TTL silently logs the
    // user out even though a valid session still exists server-side.
    if (res.status === 401 && (await tryRefreshToken())) {
      res = await fetchMe();
    }
    if (res.ok) {
      const me = await res.json();
      user.set(me);
      isAuthenticated.set(true);
    } else {
      user.set(null);
      isAuthenticated.set(false);
    }
  } catch {
    user.set(null);
    isAuthenticated.set(false);
  } finally {
    authInitialized.set(true);
  }
}
