// Pure helpers for the reasoning, SWRL, ShEx, RDF Patch and version-diff views.
// Kept apart from the components so the parsing and the retention preview are
// unit-tested on their own.

import type { IgnoredConstruct, ShexStatus } from './api';

/**
 * The axioms a run left out, in one shape. The API reports EL's as
 * `ignored: [{construct, count, example}]` and (until the shapes are unified)
 * QL's as `ignored_axioms` plus `ignored_sample`; either becomes rows here.
 */
export function normaliseIgnored(report: {
  ignored?: IgnoredConstruct[] | null;
  ignored_axioms?: number | null;
  ignored_sample?: string[] | null;
} | null | undefined): IgnoredConstruct[] {
  if (!report) return [];
  if (Array.isArray(report.ignored) && report.ignored.length) {
    return report.ignored.map((r) => ({ construct: r.construct, count: r.count, example: r.example ?? null }));
  }
  const n = report.ignored_axioms ?? 0;
  if (n > 0) {
    const sample = report.ignored_sample ?? [];
    return [{ construct: 'outside the profile', count: n, example: sample[0] ?? null }];
  }
  return [];
}

/** The sample axioms of the old QL shape, for display under the table. */
export function ignoredSample(report: { ignored_sample?: string[] | null } | null | undefined): string[] {
  return report?.ignored_sample ?? [];
}

/** Badge tone of an entailment run status or a job status. */
export function statusTone(status: string | null | undefined): 'ok' | 'busy' | 'warn' | 'bad' | 'idle' {
  switch (status) {
    case 'ok':
    case 'succeeded':
      return 'ok';
    case 'queued':
    case 'running':
      return 'busy';
    case 'not_converged':
    case 'timeout':
    case 'too_large':
    case 'unavailable':
      return 'warn';
    case 'inconsistent':
    case 'not_in_profile':
    case 'failed':
      return 'bad';
    default:
      return 'idle';
  }
}

/** A ShEx result's verdict and reason, from the serde enum shape. */
export function shexVerdict(status: ShexStatus): { ok: boolean; reason: string } {
  if (status === 'Conformant') return { ok: true, reason: '' };
  if (status && typeof status === 'object' && 'NonConformant' in status) {
    return { ok: false, reason: String(status.NonConformant ?? '') };
  }
  return { ok: false, reason: String(status) };
}

/**
 * Prefix declarations typed one per line (`ex: http://example.org/` or
 * `ex http://…`, a trailing `:` optional, `:` alone the default prefix).
 * Lines that are not a label and an IRI are reported, not dropped silently.
 */
export function parsePrefixLines(text: string): { prefixes: Record<string, string>; bad: string[] } {
  const prefixes: Record<string, string> = {};
  const bad: string[] = [];
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const m = line.match(/^(?:@?prefix\s+)?([A-Za-z_][\w.-]*)?:?\s+<?([^\s<>]+?)>?\s*\.?$/i);
    if (!m || !/^[a-z][a-z0-9+.-]*:/i.test(m[2])) {
      bad.push(line);
      continue;
    }
    prefixes[m[1] ?? ''] = m[2];
  }
  return { prefixes, bad };
}

/** Prefixes back into lines, for editing a stored rule set. */
export function prefixLines(prefixes: Record<string, string> | null | undefined): string {
  return Object.entries(prefixes ?? {}).map(([k, v]) => `${k}: ${v}`).join('\n');
}

/** One change of an RDF Patch: a quad added or removed, or a prefix row. */
export interface PatchRow {
  op: 'A' | 'D' | 'PA' | 'PD';
  /** The rest of the row as written, without the trailing ` .`. */
  terms: string;
  /** The graph term of a quad row, when it has one. */
  graph: string | null;
}

/** Split an N-Triples-like row into terms: IRIs, blank nodes, literals (with tags) and bare words. */
export function splitTerms(text: string): string[] {
  const out: string[] = [];
  const re = /<[^>]*>|_:[^\s]+|"(?:[^"\\]|\\.)*"(?:@[A-Za-z0-9-]+|\^\^<[^>]*>|\^\^[^\s]+)?|[^\s]+/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) out.push(m[0]);
  return out;
}

/** The change rows of an RDF Patch (`A`, `D`, `PA`, `PD`); headers and transactions are skipped. */
export function parsePatchRows(patch: string): PatchRow[] {
  const rows: PatchRow[] = [];
  for (const raw of patch.split('\n')) {
    const line = raw.trim();
    const m = line.match(/^(A|D|PA|PD)\s+(.*?)\s*\.?$/);
    if (!m) continue;
    const op = m[1] as PatchRow['op'];
    const terms = m[2];
    let graph: string | null = null;
    if (op === 'A' || op === 'D') {
      const parts = splitTerms(terms);
      if (parts.length >= 4) graph = parts[3];
    }
    rows.push({ op, terms, graph });
  }
  return rows;
}

/** The patch's `H id`, if it has one. */
export function patchHeader(patch: string, name: string): string | null {
  for (const raw of patch.split('\n')) {
    const m = raw.trim().match(/^H\s+(\S+)\s+(.*?)\s*\.?$/);
    if (m && m[1] === name) return m[2].replace(/^<|>$/g, '');
  }
  return null;
}

/**
 * What the patch log needs before it appends a patch: one `H id` the log does
 * not hold and an `H prev` naming its latest entry (none on an empty log).
 * Adds whichever header is missing; a header already there is kept as written.
 */
export function withLogHeaders(patch: string, latestId: string | null, newId: () => string): string {
  const head: string[] = [];
  if (!patchHeader(patch, 'id')) head.push(`H id <${newId()}> .`);
  if (latestId && !patchHeader(patch, 'prev')) head.push(`H prev <${latestId}> .`);
  return head.length ? `${head.join('\n')}\n${patch}` : patch;
}

export interface VersionLike { version: string; status: string; created_at: string }

/**
 * The versions `POST …/versions/gc {keep}` would delete: every unpublished
 * version but the newest `keep`, newest first by creation time with the label
 * breaking ties — the server's own order (dataset_versions::gc_versions).
 */
export function gcPreview<T extends VersionLike>(versions: T[], keep: number): T[] {
  const candidates = versions
    .filter((v) => v.status !== 'published')
    .slice()
    .sort((a, b) => (b.created_at < a.created_at ? -1 : b.created_at > a.created_at ? 1 : b.version < a.version ? -1 : b.version > a.version ? 1 : 0));
  return candidates.slice(Math.max(0, Math.floor(keep)));
}

/** A short form of an IRI for tables: the part after the last `#` or `/`. */
export function shortIri(iri: string): string {
  const raw = iri.replace(/^<|>$/g, '');
  const cut = Math.max(raw.lastIndexOf('#'), raw.lastIndexOf('/'));
  return cut > 0 && cut < raw.length - 1 ? raw.slice(cut + 1) : raw;
}
