// How an IRI is labelled on screen: the prefixed name (`rdf:type`) or the IRI
// itself. The prefixed name is the default because it is what makes a table of
// triples readable at all; the switch exists because the prefixed form is a
// lossy rendering of the thing you came to read, and the only way to see the
// IRI was to hover a tooltip — which needs a pointer, cannot be reached from a
// keyboard, and shows nothing at all on a touch screen.
//
// The preference is app-wide and persisted, so it is asked for once rather than
// per surface. It never changes what a copy control copies: that is the IRI
// either way, and always was.

import { writable, get } from 'svelte/store';

export type IriDisplay = 'curie' | 'full';

const STORAGE_KEY = 'ots-iri-display';

/** The current labelling. Subscribe in components; `$iriDisplay === 'full'`. */
export const iriDisplay = writable<IriDisplay>('curie');

function read(): IriDisplay | null {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return v === 'curie' || v === 'full' ? v : null;
  } catch {
    // Private mode, blocked site data, a sandboxed frame.
    return null;
  }
}

/** Apply the persisted preference. Call once, early in app boot. */
export function initIriDisplay(): void {
  iriDisplay.set(read() ?? 'curie');
}

export function setIriDisplay(mode: IriDisplay): void {
  try {
    localStorage.setItem(STORAGE_KEY, mode);
  } catch {
    // Storage is a convenience here; the choice still applies to this session.
  }
  iriDisplay.set(mode);
}

export function toggleIriDisplay(): void {
  setIriDisplay(get(iriDisplay) === 'full' ? 'curie' : 'full');
}
