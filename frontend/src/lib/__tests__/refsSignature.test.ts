import { describe, it, expect } from 'vitest';
import { digestGuids, guidsSignature, refsSignature } from '../viewer/refsSignature';

const wall = {
  id: 'https://data.example.nl/id/bim/Wall-01',
  url: 'https://files.example/schependomlaan.ifc',
  format: 'ifc',
  slot: [0, 0] as [number, number],
  guids: ['0aBc', '1dEf', '2gHi'],
};

describe('refsSignature', () => {
  it('is stable across re-allocated array literals with identical contents', () => {
    // This is the whole point: a host that inlines `refs={[{ … }]}` hands us a
    // brand-new array on every reactive pass, and that must NOT reload the model.
    expect(refsSignature([{ ...wall }])).toBe(refsSignature([{ ...wall }]));
    expect(refsSignature([])).toBe(refsSignature([]));
  });

  it('changes when any field the loader consumes changes', () => {
    const base = refsSignature([wall]);
    expect(refsSignature([{ ...wall, url: 'https://files.example/other.ifc' }])).not.toBe(base);
    expect(refsSignature([{ ...wall, format: 'gltf' }])).not.toBe(base);
    expect(refsSignature([{ ...wall, upAxis: 'Z' }])).not.toBe(base);
    expect(refsSignature([{ ...wall, slot: [2, 0] }])).not.toBe(base);
    expect(refsSignature([{ ...wall, id: 'other' }])).not.toBe(base);
    expect(refsSignature([{ ...wall, guids: ['0aBc'] }])).not.toBe(base);
    expect(refsSignature([wall, wall])).not.toBe(base);
  });

  it('ignores the order of a guid set but not the order of the refs', () => {
    expect(refsSignature([{ ...wall, guids: ['2gHi', '0aBc', '1dEf'] }])).toBe(
      refsSignature([wall]),
    );
    const other = { ...wall, id: 'b', slot: [2, 0] as [number, number] };
    expect(refsSignature([wall, other])).not.toBe(refsSignature([other, wall]));
  });

  it('does not confuse adjacent fields or refs', () => {
    // Separators matter: without them `{id:'ab', url:''}` and `{id:'a', url:'b'}`
    // would hash to the same string.
    expect(refsSignature([{ id: 'ab', url: '' }])).not.toBe(refsSignature([{ id: 'a', url: 'b' }]));
    expect(refsSignature([{ id: 'a' }, { id: 'b' }])).not.toBe(refsSignature([{ id: 'ab' }]));
  });

  it('tolerates nullish input', () => {
    expect(refsSignature(null)).toBe('');
    expect(refsSignature(undefined)).toBe('');
    expect(refsSignature([null as never])).toBe('~');
  });
});

describe('digestGuids / guidsSignature', () => {
  it('is order-independent and separator-safe', () => {
    expect(digestGuids(['a', 'b'])).toBe(digestGuids(['b', 'a']));
    expect(digestGuids(['ab', 'c'])).not.toBe(digestGuids(['a', 'bc']));
  });

  it('reports an empty signature for no guids', () => {
    expect(guidsSignature([])).toBe('');
    expect(guidsSignature(null)).toBe('');
    expect(guidsSignature(['a'])).toBe(`1~${digestGuids(['a'])}`);
  });
});
