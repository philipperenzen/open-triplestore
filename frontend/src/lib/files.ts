// File-manager domain logic: asset kind detection, folder-path utilities and
// the folder/file listing derivations the FileBrowser renders. Pure functions,
// no DOM/fetch — unit-tested in __tests__/files.test.ts.
//
// The kind taxonomy mirrors the backend's authoritative classifier
// (src/assets/mod.rs `AssetKind`) so icons/filters agree with the RDF typing,
// with one UI-side refinement: plain text/RDF/code files (which the backend
// buckets as Generic) get a `text` kind so they read as previewable documents.

export type FileKind =
  | 'image'
  | 'video'
  | 'audio'
  | 'model3d'
  | 'document'
  | 'geodata'
  | 'pointcloud'
  | 'cad'
  | 'archive'
  | 'spreadsheet'
  | 'text'
  | 'generic';

export interface AssetEntry {
  id: string;
  dataset_id?: string;
  filename: string;
  content_type: string;
  size_bytes: number;
  created_at: string;
  updated_at?: string | null;
  title?: string | null;
  description?: string | null;
  public?: boolean;
  folder?: string;
  iri?: string;
}

export interface FolderInfo {
  path: string;
  asset_count?: number;
  total_bytes?: number;
}

const EXT_KINDS: Record<string, FileKind> = {};
function reg(kind: FileKind, exts: string[]) {
  for (const e of exts) EXT_KINDS[e] = kind;
}
reg('image', ['png', 'jpg', 'jpeg', 'gif', 'svg', 'webp', 'bmp', 'ico', 'tif', 'tiff', 'avif', 'heic']);
reg('video', ['mp4', 'webm', 'ogv', 'mov', 'mkv', 'avi', 'm4v']);
reg('audio', ['mp3', 'ogg', 'oga', 'wav', 'flac', 'm4a', 'aac', 'opus']);
reg('model3d', ['glb', 'gltf', 'obj', 'stl', 'ply', 'fbx', 'dae', '3ds', 'usdz', 'usd']);
reg('document', ['pdf', 'doc', 'docx', 'odt', 'rtf', 'ppt', 'pptx', 'odp']);
reg('geodata', ['geojson', 'kml', 'gpx', 'kmz', 'cityjson', 'citygml', 'gml']);
reg('pointcloud', ['las', 'laz', 'e57', 'pcd', 'xyz', 'pts']);
reg('cad', ['dwg', 'dxf', 'ifc', 'ifczip', 'rvt', 'step', 'stp']);
reg('archive', ['zip', 'gz', 'tgz', '7z', 'tar', 'rar', 'bz2']);
reg('spreadsheet', ['csv', 'tsv', 'xls', 'xlsx', 'ods']);
reg('text', ['txt', 'md', 'ttl', 'nt', 'n3', 'trig', 'nq', 'rdf', 'jsonld', 'json', 'xml', 'yaml', 'yml', 'toml', 'sparql', 'rq', 'log']);

function extOf(filename: string): string {
  const name = (filename || '').toLowerCase();
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1) : '';
}

/** Classify a file by declared MIME + extension (no bytes client-side). */
export function fileKind(contentType: string | null | undefined, filename: string): FileKind {
  const m = (contentType || '').toLowerCase().split(';')[0].trim();
  const ext = extOf(filename);
  // Extension first for the formats whose MIME is usually a generic
  // octet-stream/JSON (3D, CAD, point clouds, geo) — mirrors the backend order.
  const byExt = EXT_KINDS[ext];
  if (byExt && byExt !== 'text') return byExt;
  if (m.startsWith('image/')) return 'image';
  if (m.startsWith('video/')) return 'video';
  if (m.startsWith('audio/')) return 'audio';
  if (m.startsWith('model/')) return 'model3d';
  if (m === 'application/pdf' || m === 'application/msword' || m === 'application/rtf') return 'document';
  if (m.startsWith('application/vnd.openxmlformats-officedocument.spreadsheetml') || m === 'text/csv') return 'spreadsheet';
  if (m.startsWith('application/vnd.openxmlformats-officedocument') || m.startsWith('application/vnd.oasis.opendocument')) return 'document';
  if (m === 'application/geo+json' || m === 'application/vnd.google-earth.kml+xml' || m === 'application/gpx+xml') return 'geodata';
  if (m === 'application/zip' || m === 'application/gzip' || m === 'application/x-7z-compressed' || m === 'application/x-tar') return 'archive';
  if (m.startsWith('text/') || m === 'application/json' || m === 'application/ld+json' || m.endsWith('+xml')) return 'text';
  return byExt || 'generic';
}

/** Chip-filter groups: a coarser view over kinds for the filter bar. */
export type KindGroup = 'images' | 'documents' | 'models' | 'geo' | 'data' | 'media' | 'other';

export const KIND_GROUPS: Record<FileKind, KindGroup> = {
  image: 'images',
  document: 'documents',
  text: 'documents',
  spreadsheet: 'data',
  archive: 'data',
  model3d: 'models',
  cad: 'models',
  pointcloud: 'geo',
  geodata: 'geo',
  video: 'media',
  audio: 'media',
  generic: 'other',
};

export const ALL_GROUPS: KindGroup[] = ['images', 'documents', 'models', 'geo', 'data', 'media', 'other'];

// ── Folder paths ─────────────────────────────────────────────────────────────

/** Client-side mirror of the server's folder-path normalization (never throws:
 *  invalid segments are dropped, matching what the server would reject). */
export function normalizePath(path: string | null | undefined): string {
  if (!path) return '';
  return String(path)
    .split(/[\\/]/)
    .map((s) => s.trim())
    .filter((s) => s && s !== '.' && s !== '..' && !s.startsWith('.'))
    .join('/');
}

export function parentOf(path: string): string {
  const i = path.lastIndexOf('/');
  return i === -1 ? '' : path.slice(0, i);
}

export function nameOf(path: string): string {
  const i = path.lastIndexOf('/');
  return i === -1 ? path : path.slice(i + 1);
}

export function joinPath(base: string, name: string): string {
  return base ? `${base}/${name}` : name;
}

/** Breadcrumb segments for a path: [{name, path}] from root child downwards. */
export function crumbsOf(path: string): { name: string; path: string }[] {
  if (!path) return [];
  const out: { name: string; path: string }[] = [];
  let prefix = '';
  for (const seg of path.split('/')) {
    prefix = prefix ? `${prefix}/${seg}` : seg;
    out.push({ name: seg, path: prefix });
  }
  return out;
}

/** Is `path` equal to or below `ancestor`? (`ancestor === ''` is the root.) */
export function isWithin(path: string, ancestor: string): boolean {
  if (!ancestor) return true;
  return path === ancestor || path.startsWith(`${ancestor}/`);
}

// ── Thumbnails ───────────────────────────────────────────────────────────────

/** Server-generated thumbnails are sibling assets named `<parentId>-thumb.png`.
 *  Returns the parent asset id when this asset IS such a thumbnail. */
export function thumbParentId(asset: Pick<AssetEntry, 'filename'>): string | null {
  const m = /^(.+)-thumb\.png$/.exec(asset.filename || '');
  return m ? m[1] : null;
}

/** Map parentId → thumbnail asset from a full asset list. */
export function thumbnailIndex(assets: AssetEntry[]): Map<string, AssetEntry> {
  const byId = new Set(assets.map((a) => a.id));
  const map = new Map<string, AssetEntry>();
  for (const a of assets) {
    const parent = thumbParentId(a);
    if (parent && byId.has(parent)) map.set(parent, a);
  }
  return map;
}

// ── Listing derivations ──────────────────────────────────────────────────────

export interface SubfolderRow {
  path: string;
  name: string;
  /** Files anywhere below this folder (recursive). */
  fileCount: number;
  /** Bytes anywhere below this folder (recursive). */
  totalBytes: number;
}

export interface FolderListing {
  subfolders: SubfolderRow[];
  files: AssetEntry[];
}

/** Assets that should not appear as rows: server-derived thumbnails. */
export function visibleAssets(assets: AssetEntry[]): AssetEntry[] {
  const thumbs = thumbnailIndex(assets);
  const thumbIds = new Set([...thumbs.values()].map((t) => t.id));
  return assets.filter((a) => !thumbIds.has(a.id));
}

/**
 * What lives directly inside `path`: its immediate subfolders (from explicit
 * folder paths ∪ folders implied by asset locations, with recursive counts)
 * and the files whose `folder` is exactly `path`.
 */
export function listChildren(
  assets: AssetEntry[],
  folderPaths: string[],
  path: string
): FolderListing {
  const shown = visibleAssets(assets);
  const allFolders = new Set<string>(folderPaths.filter(Boolean));
  for (const a of shown) {
    let prefix = '';
    for (const seg of (a.folder || '').split('/').filter(Boolean)) {
      prefix = prefix ? `${prefix}/${seg}` : seg;
      allFolders.add(prefix);
    }
  }

  const directChildNames = new Set<string>();
  for (const f of allFolders) {
    if (!isWithin(f, path) || f === path) continue;
    const rest = path ? f.slice(path.length + 1) : f;
    directChildNames.add(rest.split('/')[0]);
  }

  const subfolders: SubfolderRow[] = [...directChildNames]
    .sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' }))
    .map((name) => {
      const sub = joinPath(path, name);
      let fileCount = 0;
      let totalBytes = 0;
      for (const a of shown) {
        if (isWithin(a.folder || '', sub)) {
          fileCount += 1;
          totalBytes += a.size_bytes || 0;
        }
      }
      return { path: sub, name, fileCount, totalBytes };
    });

  const files = shown.filter((a) => (a.folder || '') === path);
  return { subfolders, files };
}

export type SortKey = 'name' | 'size' | 'modified' | 'kind';

export function sortFiles(files: AssetEntry[], key: SortKey, dir: 1 | -1 = 1): AssetEntry[] {
  const label = (a: AssetEntry) => (a.title || a.filename || '').toLowerCase();
  const stamp = (a: AssetEntry) => a.updated_at || a.created_at || '';
  return [...files].sort((a, b) => {
    let cmp = 0;
    if (key === 'size') cmp = (a.size_bytes || 0) - (b.size_bytes || 0);
    else if (key === 'modified') cmp = stamp(a).localeCompare(stamp(b));
    else if (key === 'kind')
      cmp = fileKind(a.content_type, a.filename).localeCompare(fileKind(b.content_type, b.filename));
    if (cmp === 0) cmp = label(a).localeCompare(label(b), undefined, { numeric: true, sensitivity: 'base' });
    return cmp * dir;
  });
}

/** Case-insensitive match on filename/title/description. */
export function matchesQuery(asset: AssetEntry, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return [asset.filename, asset.title, asset.description]
    .some((v) => (v || '').toLowerCase().includes(q));
}

export function formatBytes(b: number | null | undefined): string {
  if (b == null || Number.isNaN(b)) return '';
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  if (b < 1024 * 1024 * 1024) return `${(b / 1024 / 1024).toFixed(1)} MB`;
  return `${(b / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

// ── Drag & drop upload traversal ─────────────────────────────────────────────

export interface PendingUpload {
  file: File;
  /** Destination folder path, relative paths of dropped directories included. */
  folder: string;
}

/**
 * Resolve a DataTransferItemList drop into files + destination folders under
 * `basePath`, recursing into dropped directories (webkitGetAsEntry). Falls back
 * to plain `dataTransfer.files` when the entry API is unavailable.
 */
export async function collectDroppedFiles(
  dataTransfer: DataTransfer,
  basePath: string
): Promise<PendingUpload[]> {
  const out: PendingUpload[] = [];
  const items = dataTransfer.items;
  const entries: unknown[] = [];
  if (items) {
    for (let i = 0; i < items.length; i++) {
      const it = items[i] as DataTransferItem & { webkitGetAsEntry?: () => unknown };
      const entry = it.webkitGetAsEntry?.();
      if (entry) entries.push(entry);
    }
  }
  if (!entries.length) {
    for (let i = 0; i < (dataTransfer.files?.length || 0); i++) {
      out.push({ file: dataTransfer.files[i], folder: basePath });
    }
    return out;
  }
  await Promise.all(entries.map((e) => walkEntry(e, basePath, out)));
  return out;
}

interface FsEntryLike {
  isFile: boolean;
  isDirectory: boolean;
  name: string;
  file: (ok: (f: File) => void, err: (e: unknown) => void) => void;
  createReader: () => { readEntries: (ok: (es: FsEntryLike[]) => void, err: (e: unknown) => void) => void };
}

async function walkEntry(entry: unknown, folder: string, out: PendingUpload[]): Promise<void> {
  const e = entry as FsEntryLike;
  if (e.isFile) {
    const file = await new Promise<File | null>((resolve) => e.file(resolve, () => resolve(null)));
    if (file) out.push({ file, folder });
    return;
  }
  if (e.isDirectory) {
    const sub = joinPath(folder, normalizePath(e.name) || e.name);
    const reader = e.createReader();
    // readEntries returns batches of ≤100; drain until empty.
    for (;;) {
      const batch = await new Promise<FsEntryLike[]>((resolve) =>
        reader.readEntries(resolve, () => resolve([]))
      );
      if (!batch.length) break;
      for (const child of batch) await walkEntry(child, sub, out);
    }
  }
}
