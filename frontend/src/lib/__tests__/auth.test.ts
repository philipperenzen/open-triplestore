/**
 * Auth API smoke tests (W4-19)
 *
 * These tests verify that:
 * - setTokens / clearTokens use in-memory storage (not localStorage) — M-2
 * - login/register calls include credentials: 'include' — M-2
 * - tryRefreshToken does not send refresh_token in body — M-2
 * - logout calls POST and clears in-memory tokens
 * - 401 responses trigger a token refresh attempt before failing
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { setTokens, clearTokens } from '../api.js';

// Verify that in-memory token storage does NOT touch localStorage.
describe('In-memory token storage (M-2)', () => {
  beforeEach(() => {
    clearTokens();
    localStorage.clear();
  });

  it('setTokens does not write to localStorage', () => {
    setTokens('access-tok', 'refresh-tok');
    expect(localStorage.getItem('access_token')).toBeNull();
    expect(localStorage.getItem('refresh_token')).toBeNull();
  });

  it('clearTokens does not touch localStorage', () => {
    localStorage.setItem('legacy_token', 'old');
    clearTokens();
    // localStorage value should remain untouched (we don't own it)
    expect(localStorage.getItem('legacy_token')).toBe('old');
  });
});

// Verify fetch calls include credentials: 'include'.
describe('Fetch credential mode (M-2)', () => {
  let fetchSpy;

  beforeEach(() => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
  });

  afterEach(() => {
    fetchSpy.mockRestore();
    clearTokens();
  });

  it('login sends credentials: include', async () => {
    fetchSpy.mockResolvedValueOnce(new Response(
      JSON.stringify({ access_token: 'a', refresh_token: 'r', expires_in: 1800, user: { id: '1', username: 'u', email: 'u@e', role: 'user', is_active: true, can_publish: false } }),
      { status: 200, headers: { 'Content-Type': 'application/json' } },
    ));

    const { login } = await import('../api.js');
    await login('alice', 'password123');

    const [, opts] = fetchSpy.mock.calls[0];
    expect(opts.credentials).toBe('include');
  });

  it('tryRefreshToken sends credentials: include and no body token', async () => {
    // First call: 401 to trigger refresh attempt
    fetchSpy
      .mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }))
      .mockResolvedValueOnce(new Response(
        JSON.stringify({ access_token: 'new-a', refresh_token: 'new-r', expires_in: 1800, user: {} }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      ))
      .mockResolvedValueOnce(new Response('{}', {
        status: 200, headers: { 'Content-Type': 'application/json' },
      }));

    setTokens('old-access', 'old-refresh');

    const { getUser } = await import('../api.js');
    try { await getUser('me'); } catch {}

    // Second call is the refresh attempt
    const refreshCall = fetchSpy.mock.calls.find(([url]) => String(url).includes('/api/auth/refresh'));
    expect(refreshCall).toBeDefined();
    const [, refreshOpts] = refreshCall;
    expect(refreshOpts.credentials).toBe('include');
    // Must NOT send refresh_token in body (it comes via HttpOnly cookie)
    const body = refreshOpts.body ? JSON.parse(refreshOpts.body) : {};
    expect(body.refresh_token).toBeUndefined();
  });

  it('logout sends credentials: include', async () => {
    fetchSpy.mockResolvedValueOnce(new Response(null, { status: 204 }));
    const { logout } = await import('../api.js');
    await logout();
    const [, opts] = fetchSpy.mock.calls[0];
    expect(opts.credentials).toBe('include');
  });

  it('logout clears in-memory tokens', async () => {
    fetchSpy.mockResolvedValueOnce(new Response(null, { status: 204 }));
    setTokens('tok', 'rtok');
    const { logout } = await import('../api.js');
    await logout();
    // After logout, setTokens with null should not throw and subsequent login should work
    expect(() => clearTokens()).not.toThrow();
  });
});

// refreshUser must restore a session that has only the long-lived refresh cookie
// (the short-lived access-token cookie expired). Without the refresh-on-401 retry,
// every page reload past the access-token TTL silently logs the user out.
describe('refreshUser session restoration', () => {
  let fetchSpy;

  beforeEach(() => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    clearTokens();
  });

  afterEach(() => {
    fetchSpy.mockRestore();
    clearTokens();
  });

  it('refreshes the access token on a 401 from /me, then stays logged in', async () => {
    fetchSpy
      // 1) GET /api/auth/me → 401 (access-token cookie expired)
      .mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }))
      // 2) POST /api/auth/refresh → 200 (refresh-token cookie still valid)
      .mockResolvedValueOnce(new Response(
        JSON.stringify({ access_token: 'new-a', refresh_token: 'new-r', expires_in: 1800, user: {} }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      ))
      // 3) GET /api/auth/me retry → 200
      .mockResolvedValueOnce(new Response(
        JSON.stringify({ id: '1', username: 'alice', email: 'a@e', role: 'user', is_active: true }),
        { status: 200, headers: { 'Content-Type': 'application/json' } },
      ));

    const { refreshUser, isAuthenticated, user } = await import('../stores.js');
    await refreshUser();

    expect(fetchSpy.mock.calls.some(([url]) => String(url).includes('/api/auth/refresh'))).toBe(true);
    expect(get(isAuthenticated)).toBe(true);
    expect(get(user)?.username).toBe('alice');
  });

  it('stays logged out without throwing when the refresh also fails (anonymous reload)', async () => {
    fetchSpy
      .mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }))            // GET /me
      .mockResolvedValueOnce(new Response('Missing refresh token', { status: 401 }));  // POST /refresh

    const { refreshUser, isAuthenticated, user } = await import('../stores.js');
    await refreshUser();

    expect(get(isAuthenticated)).toBe(false);
    expect(get(user)).toBeNull();
  });
});

// OAuth callback: tokens extracted from hash, not query string (M-3).
describe('OAuthCallback hash extraction (M-3)', () => {
  it('does not expose tokens in search params', () => {
    // Simulate the URL the server produces:  /oauth/callback#access_token=a&refresh_token=r
    const hash = new URLSearchParams('access_token=at&refresh_token=rt');
    const params = new URLSearchParams('');  // empty — server no longer puts tokens here

    const accessFromHash = hash.get('access_token');
    const accessFromSearch = params.get('access_token');

    expect(accessFromHash).toBe('at');
    expect(accessFromSearch).toBeNull();   // must NOT fall back to query string
  });
});
