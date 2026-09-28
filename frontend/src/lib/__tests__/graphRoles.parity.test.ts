// The frontend's graph-role vocabulary (rdf-utils.ts) mirrors the backend's
// `GraphKind` enum in src/auth/models.rs. When the layered-graph convention
// added domain-values / linkset / provenance / catalog, nothing would have told
// this side; read the Rust source (serde renames included) and compare.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { GRAPH_ROLE_LABELS, normalizeGraphRole } from '../rdf-utils';

const MODELS_RS = resolve(__dirname, '../../../../src/auth/models.rs');

/** The serde tokens of `pub enum GraphKind`: `#[serde(rename = "…")]` wins, else lowercase. */
function rustGraphKindTokens(): string[] {
  const src = readFileSync(MODELS_RS, 'utf8');
  const m = src.match(/pub enum GraphKind\s*\{([^}]*)\}/);
  if (!m) throw new Error(`enum GraphKind not found in ${MODELS_RS}`);
  const tokens: string[] = [];
  let rename: string | null = null;
  for (const raw of m[1].split('\n')) {
    const line = raw.replace(/\/\/.*$/, '').trim();
    const r = line.match(/^#\[serde\(rename = "([^"]+)"\)\]$/);
    if (r) {
      rename = r[1];
      continue;
    }
    if (/^[A-Z][A-Za-z0-9]*,?$/.test(line)) {
      tokens.push(rename ?? line.replace(/,$/, '').toLowerCase());
      rename = null;
    }
  }
  return tokens;
}

describe('graph roles mirror the Rust GraphKind enum', () => {
  it('GRAPH_ROLE_LABELS has exactly the backend tokens', () => {
    expect(Object.keys(GRAPH_ROLE_LABELS).sort()).toEqual(rustGraphKindTokens().sort());
  });

  it('every backend token normalises to itself, and the convention aliases fold', () => {
    for (const t of rustGraphKindTokens()) expect(normalizeGraphRole(t)).toBe(t);
    expect(normalizeGraphRole('ontology')).toBe('model');
    expect(normalizeGraphRole('domain_values')).toBe('domain-values');
    expect(normalizeGraphRole('bogus')).toBeNull();
  });
});
