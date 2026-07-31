import { describe, it, expect } from 'vitest';
import { groupElements, formatBadges, GROUPINGS } from '../viewer/grouping';

const el = (id: string, extra: Record<string, unknown> = {}) => ({
  id: `http://ex.org/${id}`,
  label: id,
  ...extra,
});

const place = (...names: string[]) =>
  names.map((n, i) => ({ id: `http://ex.org/place/${n}`, label: n, level: String(i) }));

describe('viewer sidebar grouping', () => {
  it('offers structure plus the three re-bucketing lenses', () => {
    expect(GROUPINGS).toEqual(['structure', 'location', 'format', 'type']);
  });

  it('nests location groups country → region → city when opened', () => {
    const rows = groupElements(
      [
        el('Haus', { place: place('Germany', 'Baden-Württemberg', 'Karlsruhe') }),
        el('Smiley', { place: place('Germany', 'Baden-Württemberg', 'Karlsruhe') }),
        el('Schep', { place: place('Netherlands', 'Gelderland', 'Nijmegen') }),
      ],
      'location',
      {
        expanded: new Set([
          'Germany',
          'Germany Baden-Württemberg',
          'Germany Baden-Württemberg Karlsruhe',
          'Netherlands',
          'Netherlands Gelderland',
          'Netherlands Gelderland Nijmegen',
        ]),
      },
    );
    const shape = rows.map((r) => `${'  '.repeat(r.depth)}${r.header ?? r.el!.label}`);
    expect(shape).toEqual([
      'Germany',
      '  Baden-Württemberg',
      '    Karlsruhe',
      '      Haus',
      '      Smiley',
      'Netherlands',
      '  Gelderland',
      '    Nijmegen',
      '      Schep',
    ]);
  });

  it('counts every descendant on a header, not just direct children', () => {
    const rows = groupElements(
      [
        el('a', { place: place('Germany', 'Bayern') }),
        el('b', { place: place('Germany', 'Hessen') }),
      ],
      'location',
    );
    expect(rows.find((r) => r.header === 'Germany')!.count).toBe(2);
  });

  it('groups by model format and keeps unmodelled elements in a trailing bucket', () => {
    const rows = groupElements(
      [
        el('bridge', { files: [['Stl', 'https://x.test/a.stl']] }),
        el('house', { ifc_url: 'https://x.test/a.ifc' }),
        el('dot', {}),
      ],
      'format',
      { unknown: 'Ungrouped' },
    );
    const headers = rows.filter((r) => r.header).map((r) => r.header);
    // Alphabetical, with the catch-all pinned last however it sorts.
    expect(headers).toEqual(['IFC', 'STL', 'Ungrouped']);
  });

  it('groups start closed, so a huge dataset costs one row per group', () => {
    const els = Array.from({ length: 500 }, (_, i) =>
      el(`w${i}`, { place: place('Germany', 'Bayern') }),
    );
    const shut = groupElements(els, 'location');
    // 500 elements, one visible row — this bound is what keeps regrouping instant.
    expect(shut).toHaveLength(1);
    expect(shut[0].header).toBe('Germany');
    expect(shut[0].count).toBe(500);
    expect(shut[0].open).toBe(false);

    const open = groupElements(els, 'location', { expanded: new Set(['Germany']) });
    expect(open).toHaveLength(2); // Germany > Bayern (still closed)
    expect(open[0].open).toBe(true);

    const deep = groupElements(els, 'location', {
      expanded: new Set(['Germany', 'Germany Bayern']),
    });
    expect(deep).toHaveLength(2 + 500);
  });

  it('groups by primary rdf:type, shortened', () => {
    const rows = groupElements(
      [el('w', { types: ['https://w3id.org/bot#Element', 'http://ex.org/Other'] })],
      'type',
    );
    expect(rows[0].header).toBe('bot:Element');
  });

  it('badges every format an element offers, preferred first', () => {
    expect(
      formatBadges(
        el('multi', {
          gltf_url: 'https://x.test/a.glb',
          files: [['Ifc_v4', 'https://x.test/a.ifc']],
        }),
      ),
    ).toEqual(['glTF', 'IFC']);
    expect(formatBadges(el('none'))).toEqual([]);
  });

  it('structure grouping is handled by the tree, so it yields no rows here', () => {
    expect(groupElements([el('a')], 'structure')).toEqual([]);
  });
});
