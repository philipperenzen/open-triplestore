// Translations with inline markup are rendered with {@html}. They go through
// `$tHtml`, which escapes every interpolated value (a dataset, organisation or
// shape-graph name is user data) and keeps only inline formatting tags. Before,
// most sites passed `$t(...)` straight to {@html}, and the rest wrapped a value
// in `<strong>${name}</strong>` before a broad HTML sanitizer — so a name such
// as `<a href=…>` still became a link.
import { describe, it, expect, beforeAll } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { get } from 'svelte/store';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import nl from '../i18n/nl.json';
import { tHtml, escapeHtml, sanitizeTranslation } from '../i18n/html';

const SRC = path.resolve(__dirname, '../..');

function sourceFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === '__tests__' || entry.name === 'node_modules') continue;
      sourceFiles(full, acc);
    } else if (entry.name.endsWith('.svelte')) {
      acc.push(full);
    }
  }
  return acc;
}

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

describe('escapeHtml', () => {
  it('escapes the five HTML metacharacters', () => {
    expect(escapeHtml(`<a href="x">&'</a>`)).toBe('&lt;a href=&quot;x&quot;&gt;&amp;&#39;&lt;/a&gt;');
    expect(escapeHtml(undefined)).toBe('');
  });
});

describe('sanitizeTranslation', () => {
  it('keeps inline formatting and drops everything else', () => {
    expect(sanitizeTranslation('<strong>a</strong> <em>b</em> <code>c</code>')).toBe('<strong>a</strong> <em>b</em> <code>c</code>');
    expect(sanitizeTranslation('<a href="https://example.org">x</a><img src=x onerror=alert(1)>')).toBe('x');
    expect(sanitizeTranslation('<strong onclick="alert(1)" class="c">a</strong>')).toBe('<strong>a</strong>');
  });
});

describe('$tHtml', () => {
  it('renders a translation\'s own markup', () => {
    const html = get(tHtml)('components.uploadVersionDialog.versionHintRequired');
    expect(html).toContain('<code>owl:versionInfo</code>');
  });

  it('escapes interpolated values, so a name cannot inject markup', () => {
    const name = '<a href="https://example.org/x">click</a><img src=x onerror=alert(1)>';
    const html = get(tHtml)('components.organisationMetadataDialog.deleteConfirmWarn', { values: { name } });
    expect(html).toContain('<strong>&lt;a href="https://example.org/x"&gt;click&lt;/a&gt;&lt;img src=x onerror=alert(1)&gt;</strong>');
    const div = document.createElement('div');
    div.innerHTML = html;
    expect(div.querySelector('a, img')).toBeNull();
    expect(div.querySelector('strong')?.textContent).toBe(name);
  });
});

describe('translated {@html} sites', () => {
  it('all go through $tHtml', () => {
    const offenders: string[] = [];
    for (const f of sourceFiles(SRC)) {
      const text = fs.readFileSync(f, 'utf8');
      for (const m of text.matchAll(/\{@html\s+(?:sanitizeHtml\()?\$(?:t|i18nT)\(/g)) {
        offenders.push(`${path.relative(SRC, f)} @ ${text.slice(0, m.index).split('\n').length}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it('interpolated values sit inside the translation\'s markup, in both languages', () => {
    // These used to wrap the value in <strong> at the call site.
    const keys = [
      ['components', 'attachShapesDialog', 'intro', '<strong>{target}</strong>'],
      ['components', 'organisationMetadataDialog', 'deleteConfirmWarn', '<strong>{name}</strong>'],
      ['pages', 'shapeGraphEditor', 'addShapesHint', '<strong>{name}</strong>'],
    ] as const;
    for (const dict of [en, nl] as Record<string, any>[]) {
      for (const [a, b, c, needle] of keys) expect(dict[a][b][c]).toContain(needle);
    }
  });
});
