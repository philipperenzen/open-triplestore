// Global preview-overlay state: any surface that renders RDF terms (the triple
// table, the graph explorer, resource panels) can pop a floating 3D-model or
// map preview without owning the component. PreviewOverlay.svelte (mounted once
// in App.svelte) renders whatever is requested here.

import { writable } from 'svelte/store';
import type { ModelFormat } from './models';

export type PreviewRequest =
  | { kind: 'model'; url: string; format: ModelFormat; title: string }
  // `sources`: where the geometry came from (graph or resource IRIs), so the
  // preview can show the credit a licensed source requires.
  | { kind: 'geo'; wkts: string[]; title: string; sources?: string[] };

export const preview = writable<PreviewRequest | null>(null);

export function openPreview(req: PreviewRequest): void {
  preview.set(req);
}

export function closePreview(): void {
  preview.set(null);
}
