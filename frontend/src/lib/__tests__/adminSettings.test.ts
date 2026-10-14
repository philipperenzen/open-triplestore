import { describe, it, expect } from 'vitest';
import { controlFor, groupSettings, isDirty, filterConfig, type ConfigSetting } from '../adminSettings';

describe('controlFor', () => {
  it('edits a boolean with a toggle, an enum with a select, anything else as text', () => {
    expect(controlFor({ kind: 'bool' })).toBe('toggle');
    expect(controlFor({ kind: 'enum', choices: ['session', 'scoped', 'full'] })).toBe('select');
    expect(controlFor({ kind: 'enum', choices: [] })).toBe('text');
    expect(controlFor({ kind: 'capabilities' })).toBe('text');
  });
});

describe('groupSettings', () => {
  it('keeps the server order of groups and members', () => {
    const g = groupSettings([
      { group: 'auth', name: 'A' },
      { group: 'llm', name: 'B' },
      { group: 'auth', name: 'C' },
    ]);
    expect(g.map((x) => x.group)).toEqual(['auth', 'llm']);
    expect(g[0].settings.map((s) => s.name)).toEqual(['A', 'C']);
  });
});

describe('isDirty', () => {
  it('is a change only for a non-blank value that differs', () => {
    expect(isDirty({ value: 'session' }, 'scoped')).toBe(true);
    expect(isDirty({ value: 'session' }, ' session ')).toBe(false);
    expect(isDirty({ value: 'session' }, '   ')).toBe(false);
    expect(isDirty({ value: null }, 'x')).toBe(true);
    expect(isDirty({ value: 'x' }, undefined)).toBe(false);
  });
});

describe('filterConfig', () => {
  const row = (name: string, source: ConfigSetting['source'], value: string | null = null): ConfigSetting => ({
    name, group: 'g', kind: 'text', value, default: null, source, secret: false, runtime: false, doc: '',
  });
  const all = [row('OTS_BASE_URL', 'env', 'https://data.example.org'), row('OTS_PORT', 'default', '7878')];

  it('matches name or value, case-insensitively', () => {
    expect(filterConfig(all, 'base').map((s) => s.name)).toEqual(['OTS_BASE_URL']);
    expect(filterConfig(all, 'EXAMPLE').map((s) => s.name)).toEqual(['OTS_BASE_URL']);
    expect(filterConfig(all, '').length).toBe(2);
  });

  it('can show only what is not at its built-in default', () => {
    expect(filterConfig(all, '', true).map((s) => s.name)).toEqual(['OTS_BASE_URL']);
  });
});
