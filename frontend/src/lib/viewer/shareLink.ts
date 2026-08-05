/**
 * Shareable deep links into the map / 3D viewer.
 *
 * The viewer already understands `?focus=<iri>` on load (DatasetViewer applies
 * it as soon as the element shows up in the feed). This module is the other
 * half: building that URL so a view can be handed to someone else, and keeping
 * the address bar in sync so "copy what's in the URL bar" also works.
 *
 * The link carries NO credentials — it names a dataset and an IRI, nothing
 * more. Whoever opens it still goes through the same dataset access checks, so
 * a recipient without access gets the usual sign-in / forbidden response rather
 * than the data.
 *
 * Pure and DOM-free apart from the explicit `loc` argument, so it's testable.
 */

export interface ShareLocation {
  origin: string;
  pathname: string;
  search: string;
}

/** The query parameter the viewer reads on load. */
export const FOCUS_PARAM = 'focus';

/**
 * Absolute URL that reopens `loc` framed on `iri`.
 * An empty `iri` yields the same view with the focus removed.
 */
export function viewerShareUrl(loc: ShareLocation, iri: string): string {
  const params = new URLSearchParams(loc.search || '');
  if (iri) params.set(FOCUS_PARAM, iri);
  else params.delete(FOCUS_PARAM);
  const qs = params.toString();
  return `${loc.origin}${loc.pathname}${qs ? `?${qs}` : ''}`;
}

/**
 * The path+query to push into the address bar for `iri`, or null when the URL
 * already says that — callers skip the history write in that case so selecting
 * the same element repeatedly doesn't churn history state.
 */
export function focusUrlUpdate(loc: ShareLocation, iri: string): string | null {
  const params = new URLSearchParams(loc.search || '');
  if ((params.get(FOCUS_PARAM) || '') === (iri || '')) return null;
  if (iri) params.set(FOCUS_PARAM, iri);
  else params.delete(FOCUS_PARAM);
  const qs = params.toString();
  return `${loc.pathname}${qs ? `?${qs}` : ''}`;
}
