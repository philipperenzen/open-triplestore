// The frontend's role vocabulary calls itself the single source of truth and
// claims to mirror the Rust enums in src/auth/models.rs. Nothing checked that
// claim, and the two had drifted: SystemRole gained `Guest` and this side did
// not. Read the Rust source and compare, so the drift fails a test next time.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { MEMBERSHIP_ROLES, SYSTEM_ROLES } from '../permissions';

const MODELS_RS = resolve(__dirname, '../../../../src/auth/models.rs');

/** Variant names of `pub enum <name> { … }` in models.rs, as snake_case. */
function rustEnumVariants(name: string): string[] {
  const src = readFileSync(MODELS_RS, 'utf8');
  const m = src.match(new RegExp(`pub enum ${name}\\s*\\{([^}]*)\\}`));
  if (!m) throw new Error(`enum ${name} not found in ${MODELS_RS}`);
  return m[1]
    .split('\n')
    .map((l) => l.replace(/\/\/.*$/, '').trim())
    .filter((l) => /^[A-Z][A-Za-z0-9]*,?$/.test(l))
    .map((l) => l.replace(/,$/, ''))
    .map((v) => v.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase());
}

describe('role vocabularies mirror the Rust enums', () => {
  it('SYSTEM_ROLES matches SystemRole', () => {
    const rust = rustEnumVariants('SystemRole').sort();
    const ts = SYSTEM_ROLES.map((o) => o.value).sort();
    expect(ts).toEqual(rust);
  });

  it('MEMBERSHIP_ROLES matches Role', () => {
    const rust = rustEnumVariants('Role').sort();
    const ts = MEMBERSHIP_ROLES.map((o) => o.value).sort();
    expect(ts).toEqual(rust);
  });
});
