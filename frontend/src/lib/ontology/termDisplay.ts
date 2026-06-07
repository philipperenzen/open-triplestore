// Display helpers for multi-language term metadata (see termTypes.ts).
import type { LangValue } from './termTypes';

const base = (lang: string): string => (lang || '').toLowerCase().split('-')[0];

/**
 * Pick the best single value for the active UI language:
 *   exact tag → same primary subtag → English → no language tag → first available.
 * Returns '' when the list is empty.
 */
export function pickLang(values: LangValue[], uiLang = 'en'): string {
  if (!values || !values.length) return '';
  const want = (uiLang || '').toLowerCase();
  const wantBase = base(uiLang);
  const exact = values.find((v) => v.lang.toLowerCase() === want);
  if (exact) return exact.value;
  if (wantBase) {
    const byBase = values.find((v) => base(v.lang) === wantBase);
    if (byBase) return byBase.value;
  }
  const en = values.find((v) => base(v.lang) === 'en');
  if (en) return en.value;
  const noLang = values.find((v) => !v.lang);
  if (noLang) return noLang.value;
  return values[0].value;
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
