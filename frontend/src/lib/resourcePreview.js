// Hover previews for resources: given an IRI, fetch a small slice of its
// triples and distill the card-worthy facts (label, types, description, how
// much more there is). One shared floating card (ResourceHoverCard.svelte,
// mounted once in App.svelte) renders whatever `hoverCard` holds; the helpers
// here own scheduling, fetching and caching so every surface that shows IRIs —
// RdfTerm cells, chat answers, result tables — gets the same behaviour by
// calling two functions.
//
// The fetch is /api/browse/triples?subject=<iri> (one bounded query, ACL-scoped
// server-side), so the card can never show more than the caller may read, and
// an IRI the store doesn't know simply yields an "external" preview.

import { writable } from 'svelte/store';
import { get } from 'svelte/store';
import { locale } from 'svelte-i18n';
import { browseTriples } from './api.ts';
import { shortenIRI } from './rdf-utils.js';

/** How long a fetched preview stays fresh. Hover cards tolerate mild staleness;
 *  refetching on every hover would hammer the endpoint from result tables. */
const CACHE_TTL_MS = 5 * 60 * 1000;
/** Triples fetched per preview — enough for labels in several languages plus
 *  types and a description, small enough to stay a sub-10ms query. */
const FETCH_LIMIT = 40;
/** Hover intent delays: show late enough not to flicker while the pointer
 *  crosses a table, hide late enough to survive the gap to a neighbour cell. */
const SHOW_DELAY_MS = 350;
const HIDE_DELAY_MS = 150;

const LABEL_PREDICATES = new Set([
  'http://www.w3.org/2000/01/rdf-schema#label',
  'http://www.w3.org/2004/02/skos/core#prefLabel',
  'http://purl.org/dc/terms/title',
  'http://purl.org/dc/elements/1.1/title',
  'http://xmlns.com/foaf/0.1/name',
  'https://schema.org/name',
  'http://schema.org/name',
]);
const DESC_PREDICATES = new Set([
  'http://www.w3.org/2000/01/rdf-schema#comment',
  'http://www.w3.org/2004/02/skos/core#definition',
  'http://purl.org/dc/terms/description',
  'http://purl.org/dc/elements/1.1/description',
  'https://schema.org/description',
  'http://schema.org/description',
]);
const TYPE_PREDICATE = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#type';

/** What the floating card renders. Null = hidden.
 *  {iri, graph, x, y, state: 'loading'|'ready', preview?} */
export const hoverCard = writable(null);

/** iri → {at, preview} */
const cache = new Map();
/** iri → in-flight promise, so a hover storm issues one request per IRI. */
const inflight = new Map();

let showTimer = null;
let hideTimer = null;

/** Pick the best literal from `byLang` for the UI locale: exact locale first,
 *  then English, then a language-less literal, then anything. */
function pickLiteral(byLang) {
  if (!byLang.size) return null;
  const loc = (get(locale) || 'en').split('-')[0];
  return (
    byLang.get(loc) ?? byLang.get('en') ?? byLang.get('') ?? byLang.values().next().value
  );
}

/**
 * Distill a /api/browse/triples response into card facts.
 * Exported for tests.
 * @returns {{known: boolean, label: string|null, types: string[], description: string|null, facts: number, more: boolean}}
 */
export function summarizeTriples(res) {
  const triples = res?.triples || [];
  const labels = new Map();
  const descs = new Map();
  const types = [];
  for (const t of triples) {
    const p = t.predicate?.value;
    const o = t.object;
    if (!p || !o) continue;
    if (o.type === 'literal') {
      const lang = (o.language || o['xml:lang'] || '').split('-')[0];
      if (LABEL_PREDICATES.has(p) && !labels.has(lang)) labels.set(lang, o.value);
      if (DESC_PREDICATES.has(p) && !descs.has(lang)) descs.set(lang, o.value);
    } else if (p === TYPE_PREDICATE && (o.type === 'uri' || o.type === 'iri')) {
      const short = shortenIRI(o.value);
      if (!types.includes(short)) types.push(short);
    }
  }
  return {
    known: triples.length > 0,
    label: pickLiteral(labels),
    types: types.slice(0, 3),
    description: pickLiteral(descs),
    facts: triples.length,
    more: !!res?.hasMore,
  };
}

/**
 * Fetch (or serve from cache) the preview for `iri`.
 * Never throws: a failed fetch resolves to an unknown-resource preview, so a
 * hover can't surface an error toast.
 */
export function fetchResourcePreview(iri) {
  const hit = cache.get(iri);
  if (hit && Date.now() - hit.at < CACHE_TTL_MS) return Promise.resolve(hit.preview);
  const pending = inflight.get(iri);
  if (pending) return pending;
  const p = browseTriples({ subject: iri, limit: String(FETCH_LIMIT) })
    .then((res) => summarizeTriples(res))
    .catch(() => ({ known: false, label: null, types: [], description: null, facts: 0, more: false }))
    .then((preview) => {
      cache.set(iri, { at: Date.now(), preview });
      inflight.delete(iri);
      return preview;
    });
  inflight.set(iri, p);
  return p;
}

/** Is this preview already cached and known to the store? Undefined = not yet
 *  fetched. Used by click handlers that must decide synchronously. */
export function cachedKnown(iri) {
  const hit = cache.get(iri);
  return hit ? hit.preview.known : undefined;
}

function cardPosition(anchorEl) {
  const r = anchorEl.getBoundingClientRect();
  return { x: r.left, y: r.bottom };
}

/** Begin hover intent for `anchorEl` referencing `iri`. */
export function scheduleShow(anchorEl, iri, graph = '') {
  clearTimeout(showTimer);
  clearTimeout(hideTimer);
  showTimer = setTimeout(() => {
    const { x, y } = cardPosition(anchorEl);
    hoverCard.set({ iri, graph, x, y, state: 'loading' });
    fetchResourcePreview(iri).then((preview) => {
      hoverCard.update((c) =>
        c && c.iri === iri ? { ...c, state: 'ready', preview } : c
      );
    });
  }, SHOW_DELAY_MS);
}

/** End hover intent (pointer left the anchor). */
export function scheduleHide() {
  clearTimeout(showTimer);
  clearTimeout(hideTimer);
  hideTimer = setTimeout(() => hoverCard.set(null), HIDE_DELAY_MS);
}

/** Hide immediately (scroll, click, navigation). */
export function hideNow() {
  clearTimeout(showTimer);
  clearTimeout(hideTimer);
  hoverCard.set(null);
}

/** Svelte action: hover previews on one element that references one IRI.
 *  Usage: <span use:resourceHover={{ iri: term.value, graph }}> */
export function resourceHover(node, params) {
  let iri = params?.iri;
  let graph = params?.graph || '';
  const enter = () => iri && scheduleShow(node, iri, graph);
  const leave = () => scheduleHide();
  node.addEventListener('mouseenter', enter);
  node.addEventListener('mouseleave', leave);
  return {
    update(next) {
      iri = next?.iri;
      graph = next?.graph || '';
    },
    destroy() {
      node.removeEventListener('mouseenter', enter);
      node.removeEventListener('mouseleave', leave);
      scheduleHide();
    },
  };
}

/** The IRI an element inside a delegate container references, if any:
 *  a `.chat-iri-link` chip (decorateIriLinks) carries it in data-iri; an
 *  ordinary absolute http(s) link is its own reference. Relative hrefs are app
 *  routes and API chips are runnable calls — neither is a resource. */
function referencedIri(el) {
  const chip = el?.closest?.('.chat-iri-link');
  if (chip?.dataset?.iri) return { el: chip, iri: chip.dataset.iri };
  const a = el?.closest?.('a[href]');
  if (!a) return null;
  const href = a.getAttribute('href') || '';
  return /^https?:\/\//i.test(href) ? { el: a, iri: href } : null;
}

/** Svelte action for a container of sanitized answer HTML: every resource
 *  reference inside (IRI chips, external links) gains the hover preview.
 *  Clicks stay with the container's own handlers (the chat already opens
 *  chips on its resource page). Event delegation, so re-rendered content
 *  needs no re-wiring.
 *  Usage: <div use:resourceLinkDelegate> */
export function resourceLinkDelegate(node) {
  const over = (e) => {
    const ref = referencedIri(e.target);
    if (ref && node.contains(ref.el)) scheduleShow(ref.el, ref.iri);
  };
  const out = (e) => {
    if (referencedIri(e.target)) scheduleHide();
  };
  const click = () => hideNow();
  node.addEventListener('mouseover', over);
  node.addEventListener('mouseout', out);
  node.addEventListener('click', click);
  return {
    destroy() {
      node.removeEventListener('mouseover', over);
      node.removeEventListener('mouseout', out);
      node.removeEventListener('click', click);
    },
  };
}
