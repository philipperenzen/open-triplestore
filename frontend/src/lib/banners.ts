// Banner presets — the set of built-in "default banners" a user can pick instead
// of uploading their own image. Animated presets reuse the canvas
// `LinkedDataBackground` (the same drifting-nodes motif as the landing hero),
// recoloured per theme; gradient presets are a calm static backdrop.
//
// Persistence: a chosen preset is stored in the dataset/organisation `banner_key`
// column as the sentinel string `preset:<id>` (no DB migration — the column is
// free-text). An uploaded image stores an object-store key instead, so
// `parseBanner` distinguishes the two. The backend only validates the slug shape;
// this registry is the single source of truth for what each id renders as.

export type BannerKind = 'animated' | 'gradient';

export interface BannerPreset {
  /** URL-safe slug; the value stored as `preset:<id>`. */
  id: string;
  /** Short human label shown in the picker. */
  name: string;
  kind: BannerKind;
  /** RGB triplet (no `rgb()` wrapper) for the animated nodes/edges. */
  color: string;
  /** Base opacity multiplier for the animated layer (0–1). */
  intensity: number;
  /** CSS backdrop painted behind the (optional) animation. */
  gradient: string;
}

/** The teal motif used by the landing hero today — also the fallback. */
export const DEFAULT_BANNER_PRESET_ID = 'aurora-teal';

export const BANNER_PRESETS: BannerPreset[] = [
  {
    id: 'aurora-teal',
    name: 'Teal',
    kind: 'animated',
    color: '126, 214, 208',
    intensity: 0.9,
    gradient: 'linear-gradient(135deg, #0f2a33 0%, #1e5663 55%, #2f7a8c 100%)',
  },
  {
    id: 'aurora-cyan',
    name: 'Cyan',
    kind: 'animated',
    color: '120, 220, 235',
    intensity: 0.9,
    gradient: 'linear-gradient(135deg, #0f3033 0%, #1e5d63 55%, #2f8c8c 100%)',
  },
  {
    id: 'aurora-emerald',
    name: 'Emerald',
    kind: 'animated',
    color: '120, 220, 170',
    intensity: 0.9,
    gradient: 'linear-gradient(135deg, #0e2b25 0%, #185046 55%, #2f8c6e 100%)',
  },
  {
    id: 'aurora-azure',
    name: 'Azure',
    kind: 'animated',
    color: '125, 175, 250',
    intensity: 0.9,
    gradient: 'linear-gradient(135deg, #0e2336 0%, #1c3f63 55%, #2f6aa0 100%)',
  },
  {
    id: 'aurora-indigo',
    name: 'Indigo',
    kind: 'animated',
    color: '155, 162, 245',
    intensity: 0.9,
    gradient: 'linear-gradient(135deg, #171c33 0%, #2a3363 55%, #4f5a9c 100%)',
  },
  {
    id: 'aurora-violet',
    name: 'Violet',
    kind: 'animated',
    color: '194, 150, 245',
    intensity: 0.9,
    gradient: 'linear-gradient(135deg, #1f1733 0%, #3a2a63 55%, #6a4f9c 100%)',
  },
  {
    id: 'aurora-rose',
    name: 'Rose',
    kind: 'animated',
    color: '245, 140, 172',
    intensity: 0.9,
    gradient: 'linear-gradient(135deg, #331720 0%, #632a3d 55%, #9c4f63 100%)',
  },
  {
    id: 'aurora-amber',
    name: 'Amber',
    kind: 'animated',
    color: '240, 196, 120',
    intensity: 0.9,
    gradient: 'linear-gradient(135deg, #332a17 0%, #63502a 55%, #9c7a3f 100%)',
  },
  {
    id: 'gradient-slate',
    name: 'Slate',
    kind: 'gradient',
    color: '150, 180, 200',
    intensity: 0,
    gradient: 'linear-gradient(135deg, #1a2630 0%, #2c3e4f 55%, #46637a 100%)',
  },
  {
    id: 'gradient-dusk',
    name: 'Dusk',
    kind: 'gradient',
    color: '210, 160, 180',
    intensity: 0,
    gradient: 'linear-gradient(135deg, #2a1f33 0%, #633a4f 55%, #b06a7a 100%)',
  },
];

const PRESETS_BY_ID: Record<string, BannerPreset> = Object.fromEntries(
  BANNER_PRESETS.map((p) => [p.id, p]),
);

/** Animated presets only — used for the deterministic per-page default. */
export const ANIMATED_PRESETS: BannerPreset[] = BANNER_PRESETS.filter((p) => p.kind === 'animated');

export type BannerSource = 'upload' | 'preset' | 'none';

export interface ParsedBanner {
  type: BannerSource;
  /** The preset id when `type === 'preset'`, else null. */
  presetId: string | null;
}

/**
 * Classify a stored `banner_key` value.
 *  - `null`/empty            → no banner set
 *  - `preset:<id>`           → a built-in preset
 *  - anything else (a key)   → an uploaded image
 */
export function parseBanner(bannerKey: unknown): ParsedBanner {
  if (!bannerKey) return { type: 'none', presetId: null };
  // Tolerate the legacy `bannerKey === true` flag a page sets right after an
  // upload: any truthy non-preset value is an uploaded image.
  if (typeof bannerKey === 'string' && bannerKey.startsWith('preset:')) {
    return { type: 'preset', presetId: bannerKey.slice('preset:'.length) };
  }
  return { type: 'upload', presetId: null };
}

/** Look up a preset by id, falling back to the default teal preset. */
export function getPreset(id: string | null | undefined): BannerPreset {
  return (id && PRESETS_BY_ID[id]) || PRESETS_BY_ID[DEFAULT_BANNER_PRESET_ID];
}

/**
 * Pick a stable animated preset from a seed string (e.g. a dataset id or name),
 * so a page with no banner still gets a pleasant, consistent themed backdrop.
 */
export function defaultPresetFor(seed: string | null | undefined): BannerPreset {
  const list = ANIMATED_PRESETS.length ? ANIMATED_PRESETS : BANNER_PRESETS;
  if (!seed) return list[0];
  let h = 0;
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0;
  return list[h % list.length];
}
