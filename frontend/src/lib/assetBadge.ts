// File-format badge classification for asset listings. 3D model formats reuse
// the viewer's detector (single source of truth for what the platform can
// render); everything else falls back to extension buckets.
import { fileResourceKind, FORMAT_LABELS } from './viewer/detect';

export type BadgeTone = 'model3d' | 'rdf' | 'image' | 'json' | 'neutral';

export interface AssetBadge {
  /** Short uppercase-ish label shown in the badge tile (≤5 chars ideally). */
  label: string;
  tone: BadgeTone;
  /** Model format when the file is renderable in 3D (drives "View in 3D"). */
  model3dFormat: string | null;
}

const RDF_EXTS = new Set(['ttl', 'nt', 'nq', 'trig', 'rdf', 'owl', 'jsonld', 'n3']);
const IMAGE_EXTS = new Set(['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp', 'bmp', 'ico']);
const JSON_EXTS = new Set(['json', 'geojson']);

export function assetBadge(filename: string | null | undefined): AssetBadge {
  const name = String(filename || '').toLowerCase();
  const ext = name.includes('.') ? name.split('.').pop() || '' : '';

  const detected = fileResourceKind(name.startsWith('/') ? name : `/${name}`);
  if (detected?.kind === 'model3d' && detected.format) {
    return {
      label: FORMAT_LABELS[detected.format] || ext.toUpperCase(),
      tone: 'model3d',
      model3dFormat: detected.format,
    };
  }
  const label = ext ? ext.toUpperCase().slice(0, 5) : 'FILE';
  if (RDF_EXTS.has(ext)) return { label, tone: 'rdf', model3dFormat: null };
  if (IMAGE_EXTS.has(ext)) return { label, tone: 'image', model3dFormat: null };
  if (JSON_EXTS.has(ext)) return { label, tone: 'json', model3dFormat: null };
  return { label, tone: 'neutral', model3dFormat: null };
}
