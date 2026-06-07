// Map a BCP-47 language tag to a flag emoji, used to annotate language-tagged
// literals and multi-language vocabulary definitions. Extracted from
// RdfTerm.svelte so other components (e.g. TermDefinitionCard) reuse one copy.

// Map a 2-letter ISO country code to its flag emoji (regional-indicator pair).
function flagEmoji(cc: string): string {
  if (!cc || !/^[a-zA-Z]{2}$/.test(cc)) return '';
  const up = cc.toUpperCase();
  return String.fromCodePoint(up.charCodeAt(0) + 0x1f1a5, up.charCodeAt(1) + 0x1f1a5);
}

// Default country for a primary language subtag, used when the BCP-47 tag has
// no explicit region (e.g. "nl" → NL flag). A region subtag in the tag itself
// (e.g. "en-GB", "nl-BE") always wins over this fallback.
const LANG_TO_COUNTRY: Record<string, string> = {
  en: 'GB', nl: 'NL', de: 'DE', fr: 'FR', es: 'ES', it: 'IT', pt: 'PT',
  ru: 'RU', zh: 'CN', ja: 'JP', ko: 'KR', ar: 'SA', hi: 'IN', pl: 'PL',
  sv: 'SE', no: 'NO', nb: 'NO', nn: 'NO', da: 'DK', fi: 'FI', cs: 'CZ',
  sk: 'SK', hu: 'HU', ro: 'RO', el: 'GR', tr: 'TR', uk: 'UA', he: 'IL',
  th: 'TH', vi: 'VN', id: 'ID', ms: 'MY', ga: 'IE', cy: 'GB', ca: 'ES',
  eu: 'ES', gl: 'ES', hr: 'HR', sr: 'RS', sl: 'SI', bg: 'BG', et: 'EE',
  lv: 'LV', lt: 'LT', is: 'IS', fa: 'IR', ur: 'PK', bn: 'BD', ta: 'IN',
  af: 'ZA', sw: 'KE', fy: 'NL',
};

export function langToFlag(lang: string): string {
  if (!lang) return '';
  const parts = lang.split('-');
  // An explicit 2-letter region subtag (anything past the primary) wins.
  for (let i = parts.length - 1; i >= 1; i--) {
    if (/^[a-zA-Z]{2}$/.test(parts[i])) return flagEmoji(parts[i]);
  }
  return flagEmoji(LANG_TO_COUNTRY[parts[0].toLowerCase()] || '');
}
