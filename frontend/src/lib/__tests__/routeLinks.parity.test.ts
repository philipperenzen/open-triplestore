/**
 * Every in-app <Link to="…"> must hit a <Route> declared in App.svelte.
 *
 * The bug this caught: the dataset page linked its effective shape graphs to
 * `/shacl/shape-graphs/{id}` — the *API* path — while the declared route is
 * `/shacl/shapes/:id`. App.svelte has no catch-all Route, so the click landed
 * on a silently blank page. Nothing in the type system or the linter can see
 * that mismatch, and it is the kind that reappears whenever someone copies a
 * path out of lib/api.ts, so assert it from the source.
 */
import { describe, it, expect } from 'vitest';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

const SRC = resolve(__dirname, '../..') + '/';

function svelteFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) svelteFiles(full, out);
    else if (entry.endsWith('.svelte')) out.push(full);
  }
  return out;
}

/** `/shacl/shapes/:id` → /^\/shacl\/shapes\/[^/]+$/ */
function routeMatcher(path: string): RegExp {
  const body = path
    .split('/')
    .map((seg) => (seg.startsWith(':') ? '[^/]+' : seg.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')))
    .join('/');
  return new RegExp(`^${body}$`);
}

const declaredRoutes = [
  ...readFileSync(join(SRC, 'App.svelte'), 'utf8').matchAll(/<Route\s+path="([^"]+)"/g),
].map((m) => m[1]);

const matchers = declaredRoutes.map(routeMatcher);

/** Link targets are templates; a `${…}` slot stands for any single segment. */
function isRoutable(target: string): boolean {
  const concrete = target.split('?')[0].split('#')[0].replace(/\$\{[^}]*\}/g, 'x');
  return matchers.some((re) => re.test(concrete));
}

const linkTargets = svelteFiles(SRC).flatMap((file) =>
  [...readFileSync(file, 'utf8').matchAll(/\bto=(?:"([^"]+)"|\{`([^`]+)`\})/g)]
    .map((m) => ({ file: file.slice(SRC.length), target: m[1] ?? m[2] }))
    // Only in-app absolute paths: relative and external targets are not routed.
    .filter(({ target }) => target.startsWith('/'))
);

describe('router link ↔ route parity', () => {
  it('finds the declared routes and the links to check', () => {
    expect(declaredRoutes).toContain('/shacl/shapes/:id');
    expect(linkTargets.length).toBeGreaterThan(20);
  });

  it('has no catch-all route, so an unmatched path renders nothing', () => {
    expect(declaredRoutes.filter((p) => p === '*' || p === '/*')).toEqual([]);
  });

  it('routes every <Link to> target', () => {
    const unroutable = linkTargets.filter(({ target }) => !isRoutable(target));
    expect(unroutable).toEqual([]);
  });

  it('rejects the API path that caused the blank page', () => {
    // Guards the matcher itself: if this ever "routes", the test above is inert.
    expect(isRoutable('/shacl/shape-graphs/${s.id}')).toBe(false);
    expect(isRoutable('/shacl/shapes/${s.id}')).toBe(true);
  });
});
