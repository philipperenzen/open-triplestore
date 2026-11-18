/**
 * Calls that present the in-memory access token as a Bearer header.
 *
 * The server answers a bearer it no longer honours with 401, also on routes
 * anyone may read. These calls must then renew the session and try again, and
 * fall back to the anonymous view when the session is over, instead of
 * surfacing "Invalid or expired token" on a page the visitor may open.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { setTokens, clearTokens, sparqlQuery, fetchWithToken, isLoggedIn } from '../api.js';

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

const rejected = () =>
  new Response('Invalid or expired token', {
    status: 401,
    headers: { 'WWW-Authenticate': 'Bearer error="invalid_token"' },
  });

const authorization = (call: unknown[]) =>
  ((call[1] as RequestInit).headers as Record<string, string>)['Authorization'];

describe('a rejected bearer token', () => {
  let fetchSpy;
  // How often the app was told the session is over.
  let expired = 0;
  const onExpired = () => {
    expired += 1;
  };

  beforeEach(() => {
    fetchSpy = vi.spyOn(globalThis, 'fetch');
    expired = 0;
    window.addEventListener('auth-expired', onExpired);
  });

  afterEach(() => {
    window.removeEventListener('auth-expired', onExpired);
    fetchSpy.mockRestore();
    clearTokens();
  });

  it('is renewed and the call made again', async () => {
    setTokens('stale', 'refresh');
    fetchSpy
      .mockResolvedValueOnce(rejected())
      .mockResolvedValueOnce(json({ access_token: 'fresh', refresh_token: 'refresh-2' }))
      .mockResolvedValueOnce(json({ boolean: true }));

    await expect(sparqlQuery('ASK {}')).resolves.toEqual({ boolean: true });

    const [first, refresh, second] = fetchSpy.mock.calls;
    expect(authorization(first)).toBe('Bearer stale');
    expect(String(refresh[0])).toContain('/api/auth/refresh');
    expect(authorization(second)).toBe('Bearer fresh');
    expect(expired).toBe(0);
  });

  it('falls back to the anonymous view when the session is over', async () => {
    setTokens('stale', 'refresh');
    fetchSpy
      .mockResolvedValueOnce(rejected())
      .mockResolvedValueOnce(new Response('Unauthorized', { status: 401 }))
      .mockResolvedValueOnce(json({ boolean: false }));

    await expect(sparqlQuery('ASK {}')).resolves.toEqual({ boolean: false });

    const last = fetchSpy.mock.calls[2];
    expect(authorization(last)).toBeUndefined();
    expect(expired).toBe(1);
    expect(isLoggedIn()).toBe(false);
  });

  it('is not renewed when no token was sent', async () => {
    fetchSpy.mockResolvedValueOnce(new Response('Authentication required', { status: 401 }));

    const res = await fetchWithToken('/api/llm/health');

    expect(res.status).toBe(401);
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    expect(authorization(fetchSpy.mock.calls[0])).toBeUndefined();
    expect(expired).toBe(0);
  });

  it('leaves any other answer alone', async () => {
    setTokens('good', 'refresh');
    fetchSpy.mockResolvedValueOnce(new Response('Forbidden', { status: 403 }));

    const res = await fetchWithToken('/api/llm/health');

    expect(res.status).toBe(403);
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    expect(authorization(fetchSpy.mock.calls[0])).toBe('Bearer good');
  });
});
