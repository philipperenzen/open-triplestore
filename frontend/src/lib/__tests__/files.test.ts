import { describe, it, expect } from 'vitest';
import {
  fileKind,
  KIND_GROUPS,
  normalizePath,
  parentOf,
  nameOf,
  joinPath,
  crumbsOf,
  isWithin,
  thumbParentId,
  thumbnailIndex,
  visibleAssets,
  listChildren,
  sortFiles,
  matchesQuery,
  formatBytes,
  type AssetEntry,
} from '../files';

function asset(over: Partial<AssetEntry> = {}): AssetEntry {
  return {
    id: over.id || Math.random().toString(36).slice(2),
    filename: 'file.txt',
    content_type: 'text/plain',
    size_bytes: 10,
    created_at: '2026-07-01T00:00:00Z',
    folder: '',
    ...over,
  };
}

describe('fileKind', () => {
  it('classifies by extension for opaque binary formats', () => {
    expect(fileKind('application/octet-stream', 'model.glb')).toBe('model3d');
    expect(fileKind('', 'survey.las')).toBe('pointcloud');
    expect(fileKind('', 'plan.dwg')).toBe('cad');
    expect(fileKind('', 'building.ifc')).toBe('cad');
    expect(fileKind('application/json', 'area.geojson')).toBe('geodata');
  });

  it('classifies by MIME when extension is unknown', () => {
    expect(fileKind('image/png', 'photo')).toBe('image');
    expect(fileKind('video/mp4', 'clip')).toBe('video');
    expect(fileKind('application/pdf', 'doc')).toBe('document');
    expect(fileKind('model/gltf-binary', 'thing')).toBe('model3d');
  });

  it('mirrors the backend container-format traps', () => {
    // xlsx is a ZIP container — must stay spreadsheet, not archive.
    expect(fileKind('application/zip', 'book.xlsx')).toBe('spreadsheet');
    expect(fileKind('application/zip', 'bundle.zip')).toBe('archive');
    expect(
      fileKind('application/vnd.openxmlformats-officedocument.wordprocessingml.document', 'r.docx')
    ).toBe('document');
  });

  it('treats plain text/RDF as text and unknowns as generic', () => {
    expect(fileKind('text/turtle', 'g.ttl')).toBe('text');
    expect(fileKind('', 'notes.md')).toBe('text');
    expect(fileKind('application/octet-stream', 'blob.bin')).toBe('generic');
  });

  it('every kind maps to a filter group', () => {
    for (const kind of ['image', 'video', 'audio', 'model3d', 'document', 'geodata', 'pointcloud', 'cad', 'archive', 'spreadsheet', 'text', 'generic'] as const) {
      expect(KIND_GROUPS[kind]).toBeTruthy();
    }
  });
});

describe('path utilities', () => {
  it('normalizes separators, blanks and traversal', () => {
    expect(normalizePath('/docs/reports/')).toBe('docs/reports');
    expect(normalizePath('docs\\reports')).toBe('docs/reports');
    expect(normalizePath('a//b')).toBe('a/b');
    expect(normalizePath('a/../b')).toBe('a/b');
    expect(normalizePath('.hidden/x')).toBe('x');
    expect(normalizePath('')).toBe('');
  });

  it('parent/name/join/crumbs agree', () => {
    expect(parentOf('a/b/c')).toBe('a/b');
    expect(parentOf('a')).toBe('');
    expect(nameOf('a/b/c')).toBe('c');
    expect(joinPath('', 'x')).toBe('x');
    expect(joinPath('a', 'x')).toBe('a/x');
    expect(crumbsOf('a/b')).toEqual([
      { name: 'a', path: 'a' },
      { name: 'b', path: 'a/b' },
    ]);
    expect(crumbsOf('')).toEqual([]);
  });

  it('isWithin covers root, self and descendants only', () => {
    expect(isWithin('a/b', '')).toBe(true);
    expect(isWithin('a/b', 'a')).toBe(true);
    expect(isWithin('a', 'a')).toBe(true);
    expect(isWithin('ab', 'a')).toBe(false); // no prefix-string false positive
    expect(isWithin('a', 'a/b')).toBe(false);
  });
});

describe('thumbnails', () => {
  it('detects and indexes -thumb.png siblings', () => {
    const parent = asset({ id: 'p1', filename: 'photo.jpg', content_type: 'image/jpeg' });
    const thumb = asset({ id: 't1', filename: 'p1-thumb.png', content_type: 'image/png' });
    expect(thumbParentId(thumb)).toBe('p1');
    expect(thumbParentId(parent)).toBeNull();
    const idx = thumbnailIndex([parent, thumb]);
    expect(idx.get('p1')?.id).toBe('t1');
    // Orphan thumb (parent deleted) is NOT hidden or indexed.
    const orphan = asset({ id: 't2', filename: 'gone-thumb.png' });
    expect(thumbnailIndex([orphan]).size).toBe(0);
    expect(visibleAssets([parent, thumb]).map((a) => a.id)).toEqual(['p1']);
    expect(visibleAssets([orphan]).map((a) => a.id)).toEqual(['t2']);
  });
});

describe('listChildren', () => {
  const assets = [
    asset({ id: 'a', filename: 'root.txt', folder: '' }),
    asset({ id: 'b', filename: 'r1.txt', folder: 'docs/reports', size_bytes: 100 }),
    asset({ id: 'c', filename: 'r2.txt', folder: 'docs/reports', size_bytes: 50 }),
    asset({ id: 'd', filename: 'img.png', folder: 'docs', size_bytes: 7, content_type: 'image/png' }),
  ];

  it('derives implicit folders and recursive counts at the root', () => {
    const { subfolders, files } = listChildren(assets, ['empty'], '');
    expect(files.map((f) => f.id)).toEqual(['a']);
    expect(subfolders.map((s) => s.path)).toEqual(['docs', 'empty']);
    const docs = subfolders.find((s) => s.path === 'docs')!;
    expect(docs.fileCount).toBe(3);
    expect(docs.totalBytes).toBe(157);
    expect(subfolders.find((s) => s.path === 'empty')!.fileCount).toBe(0);
  });

  it('lists a nested level with only direct files', () => {
    const { subfolders, files } = listChildren(assets, [], 'docs');
    expect(subfolders.map((s) => s.path)).toEqual(['docs/reports']);
    expect(files.map((f) => f.id)).toEqual(['d']);
  });

  it('does not leak sibling-prefix folders (docs vs docs2)', () => {
    const extra = [...assets, asset({ id: 'e', folder: 'docs2' })];
    const { subfolders } = listChildren(extra, [], 'docs');
    expect(subfolders.map((s) => s.path)).toEqual(['docs/reports']);
  });
});

describe('sorting + filtering', () => {
  it('sorts by size, date, kind with name tiebreak', () => {
    const files = [
      asset({ id: '1', filename: 'b.txt', size_bytes: 5 }),
      asset({ id: '2', filename: 'a.txt', size_bytes: 5 }),
      asset({ id: '3', filename: 'c.png', content_type: 'image/png', size_bytes: 99 }),
    ];
    expect(sortFiles(files, 'size').map((f) => f.id)).toEqual(['2', '1', '3']);
    expect(sortFiles(files, 'size', -1)[0].id).toBe('3');
    expect(sortFiles(files, 'kind')[0].content_type).toBe('image/png');
    expect(sortFiles(files, 'name').map((f) => f.id)).toEqual(['2', '1', '3']);
  });

  it('matchesQuery searches name, title and description', () => {
    const a = asset({ filename: 'Bridge-Survey.las', title: 'North span', description: 'LiDAR sweep' });
    expect(matchesQuery(a, 'bridge')).toBe(true);
    expect(matchesQuery(a, 'north')).toBe(true);
    expect(matchesQuery(a, 'lidar')).toBe(true);
    expect(matchesQuery(a, 'zzz')).toBe(false);
    expect(matchesQuery(a, '  ')).toBe(true);
  });

  it('formats byte sizes', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(2048)).toBe('2.0 KB');
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB');
    expect(formatBytes(3 * 1024 * 1024 * 1024)).toBe('3.00 GB');
  });
});
