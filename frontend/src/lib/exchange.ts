// Pure helpers behind the exchange-format and maintenance views: LDES stream
// settings, container validation, CityJSON results and IDS check summaries.
// Kept free of Svelte and of the API module so they test on their own.

import type { LdesRetention } from './api';

// ─── LDES retention ──────────────────────────────────────────────────────────

/**
 * The xsd:duration subset the server accepts: `PnYnMnWnDTnHnMnS`, every part
 * optional but at least one, fractions on seconds only. Mirrors
 * `parse_xsd_duration` in src/ldes/store.rs closely enough to catch typos
 * before a round trip; the server stays the judge.
 */
const DURATION = /^P(?!$)(\d+Y)?(\d+M)?(\d+W)?(\d+D)?(T(?=\d)(\d+H)?(\d+M)?(\d+(\.\d+)?S)?)?$/;

export function isXsdDuration(s: string): boolean {
  return DURATION.test(s.trim());
}

/** An xsd:dateTime with a timezone (the server refuses one without). */
export function isZonedDateTime(s: string): boolean {
  return /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2}(\.\d+)?)?(Z|[+-]\d{2}:\d{2})$/.test(s.trim());
}

/** The retention form's fields, as text inputs hold them. */
export interface RetentionForm {
  full_log_duration: string;
  version_amount: string;
  version_duration: string;
  version_delete_duration: string;
  starting_from: string;
}

export function retentionToForm(r: LdesRetention | null | undefined): RetentionForm {
  return {
    full_log_duration: r?.full_log_duration ?? '',
    version_amount: r?.version_amount != null ? String(r.version_amount) : '',
    version_duration: r?.version_duration ?? '',
    version_delete_duration: r?.version_delete_duration ?? '',
    starting_from: r?.starting_from ?? '',
  };
}

export type RetentionError =
  | 'fullLogDuration'
  | 'versionAmount'
  | 'versionDuration'
  | 'versionDurationNeedsAmount'
  | 'versionDeleteDuration'
  | 'startingFrom';

/**
 * The policy to send for a form, or the first field that is wrong. An empty
 * form is `{}`, which clears the policy (the stream keeps every member).
 */
export function formToRetention(f: RetentionForm): { policy: LdesRetention } | { error: RetentionError } {
  const policy: LdesRetention = {};
  // A number input bound with bind:value hands back a number (or null), not text.
  const text = (v: unknown) => (v === null || v === undefined ? '' : String(v)).trim();
  const full = text(f.full_log_duration);
  if (full) {
    if (!isXsdDuration(full)) return { error: 'fullLogDuration' };
    policy.full_log_duration = full;
  }
  const amount = text(f.version_amount);
  if (amount) {
    const n = Number(amount);
    if (!Number.isInteger(n) || n < 1) return { error: 'versionAmount' };
    policy.version_amount = n;
  }
  const vd = text(f.version_duration);
  if (vd) {
    if (!isXsdDuration(vd)) return { error: 'versionDuration' };
    if (policy.version_amount == null) return { error: 'versionDurationNeedsAmount' };
    policy.version_duration = vd;
  }
  const del = text(f.version_delete_duration);
  if (del) {
    if (!isXsdDuration(del)) return { error: 'versionDeleteDuration' };
    policy.version_delete_duration = del;
  }
  const from = text(f.starting_from);
  if (from) {
    if (!isZonedDateTime(from)) return { error: 'startingFrom' };
    policy.starting_from = from;
  }
  return { policy };
}

/** Whether a policy keeps every member (nothing declared). */
export function keepsEverything(r: LdesRetention | null | undefined): boolean {
  return !r || Object.values(r).every((v) => v === undefined || v === null || v === '');
}

// ─── Container validation ────────────────────────────────────────────────────

export interface ContainerFinding {
  severity: 'violation' | 'warning' | 'info';
  code: string;
  message: string;
  focus?: string;
  path?: string;
  file?: string;
  shape?: string;
}

export interface ContainerReport {
  profile: string;
  conforms: boolean;
  violations: number;
  warnings: number;
  findings: ContainerFinding[];
}

/** The finding code an export carries when the ISO ontology files are not configured. */
export const ONTOLOGY_MISSING = 'ontology-resource-missing';

/**
 * The export's own validation, from its response headers:
 * `X-Container-Conforms: true|false`, `X-Container-Validation:
 * violations=N; warnings=M` and `X-Container-Findings: code, code`.
 */
export function parseContainerHeaders(h: {
  conforms: string | null; validation: string | null; findings: string | null;
}): { conforms: boolean | null; violations: number | null; warnings: number | null; findings: string[] } {
  const num = (key: string) => {
    const m = new RegExp(`${key}=(\\d+)`).exec(h.validation ?? '');
    return m ? Number(m[1]) : null;
  };
  return {
    conforms: h.conforms === 'true' ? true : h.conforms === 'false' ? false : null,
    violations: num('violations'),
    warnings: num('warnings'),
    findings: (h.findings ?? '').split(',').map((s) => s.trim()).filter(Boolean),
  };
}

/** Findings ordered violations first, then warnings, then info. */
export function sortFindings(findings: ContainerFinding[]): ContainerFinding[] {
  const rank = { violation: 0, warning: 1, info: 2 } as const;
  return [...findings].sort((a, b) => (rank[a.severity] ?? 3) - (rank[b.severity] ?? 3));
}

// ─── IDS check ───────────────────────────────────────────────────────────────

/** Graphs that hold an IFC import's IDS projection (`{building}/ids`). */
export function idsProjectionGraphs(graphs: (string | { graph_iri: string })[]): string[] {
  return graphs
    .map((g) => (typeof g === 'string' ? g : g.graph_iri))
    .filter((iri) => typeof iri === 'string' && /\/ids$/.test(iri));
}

/** The building graph a projection belongs to: `…/building/ids` → `…/building`. */
export function buildingOf(idsGraph: string): string {
  return idsGraph.replace(/\/ids$/, '');
}

/** Whether a shape graph came from an IDS import (the importer tags it `ids`). */
export function isIdsShapeGraph(sg: { tags?: string[] }): boolean {
  return Array.isArray(sg.tags) && sg.tags.includes('ids');
}

const unbracket = (s: string) => (s || '').replace(/^<(.*)>$/, '$1');

export interface IdsSpecSummary {
  shape: string;
  name: string;
  /** Distinct entities that fail one of the specification's requirements. */
  failing: string[];
  /** The model has no applicable entity although the specification requires one. */
  missing: boolean;
  messages: string[];
  passed: boolean;
}

/**
 * A run's results grouped by IDS specification. Each specification is a node
 * shape, plus `<shape>-exists` on the model node when it is required; a
 * result belongs to the specification whose shape (or existence shape)
 * produced it. Specifications with no result passed. Results from shapes not
 * in `specs` are grouped under their own shape IRI, so nothing is dropped.
 */
export function summarizeIdsRun(
  specs: { shape: string; label?: string | null }[],
  results: { source_shape: string; focus_node: string; message: string }[],
): IdsSpecSummary[] {
  const bySpec = new Map<string, IdsSpecSummary>();
  const order: string[] = [];
  const add = (shape: string, name: string) => {
    if (!bySpec.has(shape)) {
      bySpec.set(shape, { shape, name, failing: [], missing: false, messages: [], passed: true });
      order.push(shape);
    }
    return bySpec.get(shape)!;
  };
  for (const s of specs) {
    const iri = unbracket(s.shape);
    if (iri.endsWith('-exists')) continue;
    add(iri, s.label || iri);
  }
  for (const r of results) {
    const src = unbracket(r.source_shape);
    const exists = src.endsWith('-exists');
    const shape = exists ? src.slice(0, -'-exists'.length) : src;
    const spec = add(shape, shape);
    spec.passed = false;
    if (exists) spec.missing = true;
    else {
      const focus = unbracket(r.focus_node);
      if (!spec.failing.includes(focus)) spec.failing.push(focus);
    }
    if (r.message && !spec.messages.includes(r.message)) spec.messages.push(r.message);
  }
  return order.map((s) => bySpec.get(s)!);
}

/** A pipeline that runs `shapeGraphId` over exactly `graphIri` of `datasetId`, if one exists. */
export function findIdsPipeline<P extends {
  dataset_ids?: string[]; graph_iris?: string[]; shape_graph_ids?: string[];
}>(pipelines: P[], datasetId: string, graphIri: string, shapeGraphId: string): P | undefined {
  const same = (a: string[] | undefined, v: string) => Array.isArray(a) && a.length === 1 && a[0] === v;
  return pipelines.find((p) =>
    same(p.dataset_ids, datasetId) && same(p.graph_iris, graphIri) && same(p.shape_graph_ids, shapeGraphId));
}

// ─── Downloads and sizes ─────────────────────────────────────────────────────

/** Save a blob under `filename` through a temporary object URL. */
export function downloadBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}

export function fmtBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—';
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

/** A percentage for a 0–1 progress fraction. */
export function pctOf(fraction: number | null | undefined): number {
  if (fraction == null || !Number.isFinite(fraction)) return 0;
  return Math.max(0, Math.min(100, Math.round(fraction * 100)));
}
