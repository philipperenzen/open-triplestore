// Deep-link integration with an external graph viewer — an optional companion app
// that renders RDF / SHACL shapes. We hand the viewer a shape graph's (or
// dataset's) Turtle in a URL query param and let it seed its SHACL sandbox /
// canvas. Turtle is UTF-8 encoded then base64url'd so it survives the URL
// round-trip cleanly.
//
// The integration is OPTIONAL and OFF by default. Set `VITE_GRAPH_VIEWER_URL` to a
// viewer's base URL to enable the "Open in graph viewer" actions; leave it unset
// and the UI hides them. Any viewer that decodes the same scheme works — base64url
// of the UTF-8 bytes (no padding) on a `/viewer?view=…&data=…` URL.

// Base URL of the external graph viewer. Empty/unset disables the integration.
const VIEWER_URL = String((import.meta as any).env?.VITE_GRAPH_VIEWER_URL ?? '')
  .trim()
  .replace(/\/+$/, '');

/** Base URL of the configured external graph viewer (`''` when none is configured). */
export function graphViewerBaseUrl(): string {
  return VIEWER_URL;
}

/** Whether an external graph viewer is configured — gates the "Open in viewer" UI. */
export function viewerConfigured(): boolean {
  return VIEWER_URL.length > 0;
}

function bytesToB64url(bytes: Uint8Array): string {
  let bin = '';
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

/** base64url-encode a Turtle string for a viewer deep-link query param. */
export function encodeTurtleParam(turtle: string): string {
  return bytesToB64url(new TextEncoder().encode(String(turtle || '')));
}

function buildViewerUrl(view: string, payloadParam: string, turtle: string): string {
  const u = new URL(`${VIEWER_URL}/viewer`);
  u.searchParams.set('view', view);
  u.searchParams.set(payloadParam, encodeTurtleParam(turtle));
  return u.toString();
}

/** URL that opens the viewer's SHACL sandbox with these shapes preloaded. */
export function buildShapesViewerUrl(turtle: string): string {
  return buildViewerUrl('shaclSandbox', 'shapesData', turtle);
}

/** URL that opens the viewer canvas with this RDF data preloaded. */
export function buildDataViewerUrl(turtle: string): string {
  return buildViewerUrl('viewer', 'data', turtle);
}

// Pre-open a blank tab SYNCHRONOUSLY inside a click handler, then navigate it
// once the (async) Turtle fetch resolves. Calling window.open *after* an await
// is treated as a programmatic popup and blocked by browsers — which is why an
// earlier version silently did nothing. Returns the handle (or null if blocked).
export function openPendingViewerTab(): Window | null {
  const win = window.open('', '_blank');
  if (win) {
    try { win.opener = null; } catch { /* cross-origin guard */ }
    try { win.document.write('<title>Graph viewer</title><p style="font:14px system-ui;padding:1.5rem;color:#475569">Opening the graph viewer…</p>'); } catch { /* ignore */ }
  }
  return win;
}

/** Send a pre-opened tab (or a fresh one) to the viewer's SHACL sandbox. */
export function showShapesInViewer(win: Window | null, turtle: string): void {
  const url = buildShapesViewerUrl(turtle);
  if (win && !win.closed) win.location.href = url;
  else window.open(url, '_blank');
}

/** Send a pre-opened tab (or a fresh one) to the viewer canvas with RDF data. */
export function showDataInViewer(win: Window | null, turtle: string): void {
  const url = buildDataViewerUrl(turtle);
  if (win && !win.closed) win.location.href = url;
  else window.open(url, '_blank');
}
