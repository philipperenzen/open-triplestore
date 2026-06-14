// Pure helpers for the SHACL validation UI.
//
// POST /api/datasets/:id/validate returns an envelope `{ report, run_id,
// ran_at }` (plus `test: true` for dry runs); stored runs from
// /validation/latest and /validation/runs/:id carry the report under the same
// `report` key but use `{ id, run_timestamp }` for identity/time. Components
// must never treat the envelope itself as the report — these helpers normalize
// both shapes into one.

export interface ValidationResultRow {
  severity?: string;
  focus_node?: string;
  path?: string | null;
  message?: string;
  [key: string]: unknown;
}

export interface ValidationReport {
  conforms: boolean;
  results_count: number;
  results: ValidationResultRow[];
}

export interface NormalizedValidationRun {
  report: ValidationReport | null;
  runId: string | null;
  ranAt: string | null;
  test: boolean;
}

function looksLikeReport(o: unknown): o is Record<string, unknown> {
  return !!o && typeof o === 'object' && 'conforms' in (o as object);
}

function normalizeReport(r: Record<string, unknown>): ValidationReport {
  const results = Array.isArray(r.results) ? (r.results as ValidationResultRow[]) : [];
  return {
    conforms: !!r.conforms,
    results_count: typeof r.results_count === 'number' ? r.results_count : results.length,
    results,
  };
}

/**
 * Unwrap a validate response or stored-run record into `{ report, runId,
 * ranAt, test }`. Accepts the envelope (`report`/`run_id`/`ran_at`), a stored
 * run (`report`/`id`/`run_timestamp`) and — defensively — a bare report.
 * Returns a null report when none is present.
 */
export function unwrapValidationRun(res: unknown): NormalizedValidationRun {
  if (!res || typeof res !== 'object') {
    return { report: null, runId: null, ranAt: null, test: false };
  }
  const r = res as Record<string, unknown>;
  const report = looksLikeReport(r.report)
    ? normalizeReport(r.report as Record<string, unknown>)
    : looksLikeReport(r)
      ? normalizeReport(r)
      : null;
  return {
    report,
    runId:
      (typeof r.run_id === 'string' && r.run_id) ||
      (typeof r.id === 'string' && r.id) ||
      null,
    ranAt:
      (typeof r.ran_at === 'string' && r.ran_at) ||
      (typeof r.run_timestamp === 'string' && r.run_timestamp) ||
      null,
    test: r.test === true,
  };
}

/**
 * User-facing message for a failed validate call. The backend's 4xx body is
 * actionable ("No shapes graph configured …"), so prefer it; fall back to the
 * caller-supplied (i18n'd) generic message otherwise.
 */
export function validationErrorMessage(err: unknown, fallback = 'Validation failed'): string {
  if (err && typeof err === 'object') {
    const msg = (err as { message?: unknown }).message;
    if (typeof msg === 'string' && msg.trim()) return msg.trim();
  }
  return fallback;
}
