// Display helpers for multi-language term metadata (see termTypes.ts).
import type { LangValue } from './termTypes';

const base = (lang: string): string => (lang || '').toLowerCase().split('-')[0];

/**
 * The active UI language, mirrored from svelte-i18n's `$locale` (see main.ts).
 *
 * RDF parsing code picks labels far away from any Svelte component — inside
 * loader.ts / schema-model.ts, which run on plain N3 stores — so it reads the
 * language from here rather than threading a parameter through every call site.
 * Note that already-parsed models keep the labels they were parsed with; a
 * language switch applies to models loaded after it.
 */
let activeLang = 'en';

/** Mirror the UI locale here. Accepts full tags ('nl-BE') or bare ones ('nl'). */
export function setUiLang(tag: string | null | undefined): void {
  activeLang = (tag || 'en').toLowerCase();
}

/** The active UI language tag, lower-cased. */
export function uiLang(): string {
  return activeLang;
}

/**
 * Rank a literal's language tag for display against `ui` — LOWER IS BETTER:
 *   0 exact tag · 1 same primary subtag · 2 English · 3 untagged · 4 other.
 * This is the single ordering behind `pickLang` and the label picks in the
 * parsers, so "prefer Dutch when the UI is Dutch" means the same thing
 * everywhere.
 */
export function langRank(lang: string | null | undefined, ui: string = activeLang): number {
  const l = (lang || '').toLowerCase();
  const want = (ui || '').toLowerCase();
  if (want && l === want) return 0;
  if (want && l && base(l) === base(want)) return 1;
  if (base(l) === 'en') return 2;
  if (!l) return 3;
  return 4;
}

/**
 * True when a literal tagged `candidate` should replace one tagged `current`.
 * Ties keep the incumbent, so the first literal seen wins among equals.
 */
export function isBetterLang(
  candidate: string | null | undefined,
  current: string | null | undefined,
  ui: string = activeLang,
): boolean {
  return langRank(candidate, ui) < langRank(current, ui);
}

/**
 * Pick the best single value for the active UI language:
 *   exact tag → same primary subtag → English → no language tag → first available.
 * Returns '' when the list is empty.
 */
export function pickLang(values: LangValue[], uiLang = 'en'): string {
  if (!values || !values.length) return '';
  let best = values[0];
  let bestRank = langRank(best.lang, uiLang);
  for (let i = 1; i < values.length; i++) {
    const r = langRank(values[i].lang, uiLang);
    if (r < bestRank) {
      best = values[i];
      bestRank = r;
    }
  }
  return best.value;
}

/**
 * Order values for a grouped, multi-language display: the active UI language
 * first, then English, then other languages alphabetically, with untagged
 * values last. Non-mutating.
 */
export function groupByLang(values: LangValue[], uiLang = 'en'): LangValue[] {
  if (!values) return [];
  const wantBase = base(uiLang);
  const rank = (lang: string): number => {
    const l = base(lang);
    if (wantBase && l === wantBase) return 0;
    if (l === 'en') return 1;
    if (!l) return 3;
    return 2;
  };
  return [...values].sort((a, b) => {
    const ra = rank(a.lang);
    const rb = rank(b.lang);
    if (ra !== rb) return ra - rb;
    return (a.lang || '').localeCompare(b.lang || '');
  });
}
