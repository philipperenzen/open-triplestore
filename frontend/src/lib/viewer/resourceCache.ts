// A small read-through cache in front of `/api/browse/resource`.
//
// The viewer's inspector windows hammer that endpoint: opening a resource in a
// second window, re-activating a tab and (before it was fixed) merely clicking
// inside a window all issued a fresh lookup, and each one costs the backend
// three SPARQL queries. Worse, two lookups racing in the same window resolved
// last-write-wins, so a slow answer for the previous resource could land on top
// of the current one.
//
// Three guarantees, in the order they matter:
//   * in-flight de-duplication — N callers asking for the same resource at the
//     same time share ONE request;
//   * a short TTL — re-opening something you just looked at is free;
//   * cancellation — a caller passing an AbortSignal drops out, and the shared
//     request is only cancelled once the *last* interested caller has gone.
//
// The TTL is deliberately short: `/api/browse/resource` is read-only, but a user
// editing the graph in another tab should not be stuck with stale properties for
// long. `invalidate()` and `clear()` exist for the surfaces that know better.

import { browseResource } from '../api';

/** The scope params `browseResource` understands; all optional. */
export interface ResourceScope {
  graph?: string;
  dataset_id?: string;
  dataset_ids?: string;
  org_id?: string;
  versions?: string;
}

export interface ResourceFetchInit {
  signal?: AbortSignal;
}

export type ResourceFetcher<T = unknown> = (
  iri: string,
  scope: ResourceScope,
  init: ResourceFetchInit,
) => Promise<T> | T;

export interface ResourceCacheOptions<T = unknown> {
  fetcher: ResourceFetcher<T>;
  /** How long a resolved entry stays fresh. Default 60s. */
  ttlMs?: number;
  /** Maximum resolved entries kept; least-recently-used are evicted. Default 100. */
  max?: number;
  /** Injectable clock, so the TTL can be tested without timers. */
  now?: () => number;
}

export interface ResourceCacheStats {
  entries: number;
  inflight: number;
}

export interface ResourceCache<T = unknown> {
  get(iri: string, scope?: ResourceScope, init?: ResourceFetchInit): Promise<T>;
  /** Drops cached answers for one IRI (any scope), or all of them when omitted. */
  invalidate(iri?: string): void;
  clear(): void;
  stats(): ResourceCacheStats;
}

// Scope params are part of the identity of an answer: the same IRI resolved
// against one dataset and against the full accessible set genuinely returns
// different triples. NUL separates the fields because it cannot occur in an IRI
// or in any of the id values, so no combination of values can forge another key.
const SCOPE_KEYS: (keyof ResourceScope)[] = ['graph', 'dataset_id', 'dataset_ids', 'org_id', 'versions'];

export function resourceCacheKey(iri: string, scope: ResourceScope = {}): string {
  const s = scope || {};
  return [...SCOPE_KEYS.map((k) => s[k] || ''), iri].join('\u0000');
}

interface Flight<T> {
  promise: Promise<T>;
  controller: AbortController | null;
  /** Callers still interested in this request; at 0 the shared request is cancelled. */
  refs: number;
}

interface Entry<T> {
  at: number;
  value: T;
  iri: string;
}

function abortError(): Error {
  // DOMException is what a real fetch abort throws; keep the name identical so
  // callers can do the usual `err.name === 'AbortError'` check either way.
  if (typeof DOMException === 'function') return new DOMException('Aborted', 'AbortError');
  const err = new Error('Aborted');
  err.name = 'AbortError';
  return err;
}

export function createResourceCache<T = unknown>(opts: ResourceCacheOptions<T>): ResourceCache<T> {
  const { fetcher } = opts;
  const ttlMs = opts.ttlMs ?? 60_000;
  const max = opts.max ?? 100;
  const now = opts.now ?? (() => Date.now());

  // Map preserves insertion order, which is all an LRU needs: a hit deletes and
  // re-inserts (moving the key to the end), so the oldest key is always first.
  const entries = new Map<string, Entry<T>>();
  const inflight = new Map<string, Flight<T>>();

  function store(key: string, iri: string, value: T): void {
    entries.delete(key);
    entries.set(key, { at: now(), value, iri });
    while (entries.size > max) {
      const oldest = entries.keys().next();
      if (oldest.done) break;
      entries.delete(oldest.value);
    }
  }

  /**
   * Attach one caller to a shared in-flight request. The caller's own promise
   * rejects as soon as *its* signal aborts, but the underlying request keeps
   * running for everybody else still waiting on the same resource.
   */
  function join(flight: Flight<T>, signal?: AbortSignal): Promise<T> {
    flight.refs += 1;
    return new Promise<T>((resolve, reject) => {
      let left = false;
      const leave = () => {
        if (left) return;
        left = true;
        flight.refs -= 1;
        if (signal) signal.removeEventListener('abort', onAbort);
      };
      function onAbort() {
        leave();
        if (flight.refs <= 0 && flight.controller) flight.controller.abort();
        reject(abortError());
      }
      if (signal) {
        if (signal.aborted) {
          onAbort();
          return;
        }
        signal.addEventListener('abort', onAbort);
      }
      flight.promise.then(
        (value) => {
          leave();
          resolve(value);
        },
        (err) => {
          leave();
          reject(err);
        },
      );
    });
  }

  function start(key: string, iri: string, scope: ResourceScope): Flight<T> {
    const controller = typeof AbortController === 'function' ? new AbortController() : null;
    const flight: Flight<T> = { controller, refs: 0, promise: null as unknown as Promise<T> };
    flight.promise = Promise.resolve(fetcher(iri, scope, controller ? { signal: controller.signal } : {})).then(
      (value) => {
        inflight.delete(key);
        store(key, iri, value);
        return value;
      },
      (err) => {
        // Never cache a rejection. A transient network blip or a 429 must not
        // pin a dead entry for the whole TTL — the next click should retry.
        inflight.delete(key);
        throw err;
      },
    );
    inflight.set(key, flight);
    return flight;
  }

  return {
    get(iri, scope = {}, init = {}) {
      const key = resourceCacheKey(iri, scope);
      const hit = entries.get(key);
      if (hit) {
        if (now() - hit.at <= ttlMs) {
          // Re-insert to mark it as most-recently-used.
          entries.delete(key);
          entries.set(key, hit);
          return Promise.resolve(hit.value);
        }
        entries.delete(key);
      }
      return join(inflight.get(key) ?? start(key, iri, scope), init.signal);
    },
    invalidate(iri) {
      if (iri === undefined) {
        entries.clear();
        return;
      }
      for (const [key, entry] of entries) {
        if (entry.iri === iri) entries.delete(key);
      }
    },
    clear() {
      entries.clear();
    },
    stats() {
      return { entries: entries.size, inflight: inflight.size };
    },
  };
}

/**
 * The instance the viewer uses. Shared on purpose: two inspector windows showing
 * the same element must cost one request, not two.
 */
export const resourceCache = createResourceCache({
  fetcher: (iri, scope, init) => browseResource(iri, scope, init),
});
