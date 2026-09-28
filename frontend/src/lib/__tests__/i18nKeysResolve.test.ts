// Every translation key the app asks for must exist in both dictionaries.
//
// `svelte-i18n` renders a missing key as the key itself, so a typo or a
// namespace that was never created ships as literal `common.loading` on screen
// — which is how four `$t('common.loading')` calls survived in AdminSecurity
// and OAuthAuthorize: there is no `common` namespace in either locale file, and
// nothing failed. It is not the sort of thing a component test notices either,
// because a component test asserts what it expects to see rather than reading
// what is there.
//
// This scans the source for literal key references and checks each one
// resolves. Dynamic keys (`$t(variable)` or a template literal) are invisible
// to it by construction, so it is a floor, not a proof.
import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import en from '../i18n/en.json';
import nl from '../i18n/nl.json';

const SRC = path.resolve(__dirname, '../..');

/** Every leaf path in a dictionary, dotted. */
function leaves(obj: unknown, prefix = '', out = new Set<string>()): Set<string> {
  if (obj && typeof obj === 'object' && !Array.isArray(obj)) {
    for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
      const key = prefix ? `${prefix}.${k}` : k;
      if (v && typeof v === 'object' && !Array.isArray(v)) leaves(v, key, out);
      else out.add(key);
    }
  }
  return out;
}

function sourceFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === '__tests__' || entry.name === 'node_modules') continue;
      sourceFiles(full, acc);
    } else if (/\.(svelte|ts|js)$/.test(entry.name)) {
      acc.push(full);
    }
  }
  return acc;
}

/**
 * Literal key references: `$t('a.b')`, `$i18nT("a.b")`, `t('a.b')`.
 *
 * Deliberately only single/double-quoted literals — a template literal or a
 * variable cannot be checked without running the app. The trailing `[,)]` is
 * what keeps a *built* key out: `$t('pages.import.role.' + role)` starts with
 * a literal that is not itself a key, and matching it reported two bugs that
 * were not there.
 */
const CALL = /\$?(?:i18nT|t)\(\s*(['"])([a-zA-Z][\w.]*\.[\w.]+)\1\s*[,)]/g;

describe('every literal translation key resolves', () => {
  const enKeys = leaves(en);
  const nlKeys = leaves(nl);
  const files = sourceFiles(SRC);

  it('finds enough call sites to be worth trusting', () => {
    // A regex that silently stops matching would make every assertion below
    // vacuously true, which is the failure mode this guards.
    let found = 0;
    for (const f of files) {
      const text = fs.readFileSync(f, 'utf8');
      found += [...text.matchAll(CALL)].length;
    }
    expect(found).toBeGreaterThan(500);
  });

  it('resolves in English', () => {
    const missing: string[] = [];
    for (const f of files) {
      const text = fs.readFileSync(f, 'utf8');
      for (const [, , key] of text.matchAll(CALL)) {
        if (!enKeys.has(key)) missing.push(`${path.relative(SRC, f)}: ${key}`);
      }
    }
    expect(
      [...new Set(missing)],
      'these keys render as their own name on screen',
    ).toEqual([]);
  });

  it('resolves in Dutch too', () => {
    // A key present in one dictionary and not the other falls back to English
    // silently, which reads as an untranslated app rather than a bug.
    const missing: string[] = [];
    for (const f of files) {
      const text = fs.readFileSync(f, 'utf8');
      for (const [, , key] of text.matchAll(CALL)) {
        if (enKeys.has(key) && !nlKeys.has(key)) {
          missing.push(`${path.relative(SRC, f)}: ${key}`);
        }
      }
    }
    expect([...new Set(missing)]).toEqual([]);
  });

  it('keeps the two dictionaries the same shape', () => {
    expect([...enKeys].filter((k) => !nlKeys.has(k))).toEqual([]);
    expect([...nlKeys].filter((k) => !enKeys.has(k))).toEqual([]);
  });
});
