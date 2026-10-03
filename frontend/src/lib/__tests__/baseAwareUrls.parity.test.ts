/**
 * No root-absolute URL may leave the app without the sub-path base.
 *
 * Under OTS_BASE_PATH=/ots/ a plain `fetch('/api/…')`, `<a href="/login">` or
 * `window.location.href = '/…'` goes to the host root, outside the deployment,
 * and the reverse proxy in front of it never sees the request. In-app
 * navigation goes through <Link>/navigate() (which add the base); everything
 * else must spell it through lib/basePath.ts (withBase / absoluteUrl) or
 * lib/api.ts's API_BASE. This scans the source for the raw forms.
 */
import { describe, it, expect } from 'vitest';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

const SRC = resolve(__dirname, '../..') + '/';

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir)) {
    const full = join(dir, entry);
    if (statSync(full).isDirectory()) {
      if (entry !== '__tests__') sourceFiles(full, out);
    } else if (/\.(svelte|ts|js)$/.test(entry)) out.push(full);
  }
  return out;
}

// A quote or template-literal backtick, then a root-absolute path (not `//`).
const ROOT = String.raw`['"\`]\/(?!\/)`;
const PATTERNS: [string, RegExp][] = [
  ['fetch() of a root path', new RegExp(String.raw`\bfetch(?:Retry429)?\(\s*` + ROOT)],
  ['XHR open of a root path', new RegExp(String.raw`\.open\(\s*['"][A-Z]+['"]\s*,\s*` + ROOT)],
  ['EventSource on a root path', new RegExp(String.raw`new EventSource\(\s*` + ROOT)],
  ['href/src attribute with a root path', /\b(?:href|src)=(?:"|\{`)\/(?!\/)/],
  ['window.location set to a root path', new RegExp(String.raw`location\.(?:href\s*=|assign\(|replace\()\s*` + ROOT)],
  ['window.open of a root path', new RegExp(String.raw`window\.open\(\s*` + ROOT)],
  ['origin + root path', /location\.origin\s*\+\s*['"`]\/|\$\{(?:window\.)?location\.origin\}\//],
];

const offenders = sourceFiles(SRC).flatMap((file) => {
  const rel = file.slice(SRC.length);
  return readFileSync(file, 'utf8')
    .split('\n')
    .flatMap((line, i) => {
      const code = line.trim();
      if (code.startsWith('//') || code.startsWith('*')) return [];
      return PATTERNS.filter(([, re]) => re.test(line)).map(([what]) => `${rel}:${i + 1} ${what}: ${code}`);
    });
});

describe('root-absolute URLs carry the sub-path base', () => {
  it('scans the app source', () => {
    expect(sourceFiles(SRC).length).toBeGreaterThan(100);
  });

  it('has no fetch, link, redirect or popup to a bare root path', () => {
    expect(offenders).toEqual([]);
  });

  it('catches the forms it is meant to catch', () => {
    const samples = [
      "fetch('/api/auth/me')",
      'fetch(`/sparql?query=${q}`)',
      "xhr.open('POST', `/api/datasets/${id}/assets`)",
      '<a href="/login">',
      '<a href={`/resource?iri=${x}`}>',
      "window.location.href = `/api/auth/oauth/${slug}/authorize`",
      'const u = `${window.location.origin}/api/x`',
    ];
    for (const s of samples) expect(PATTERNS.some(([, re]) => re.test(s)), s).toBe(true);
    for (const s of ["fetch(withBase('/api/x'))", '<a href={withBase(`/x`)}>', "fetch('//cdn.example.org/x')", "fetch(url)"]) {
      expect(PATTERNS.some(([, re]) => re.test(s)), s).toBe(false);
    }
  });
});
