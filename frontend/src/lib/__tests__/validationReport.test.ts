import { describe, it, expect } from 'vitest';
import { unwrapValidationRun, validationErrorMessage } from '../validationReport.js';

const fullReport = {
  conforms: false,
  results_count: 2,
  results: [
    { severity: 'Violation', focus_node: 'http://ex.org/a', path: 'http://ex.org/p', message: 'too few values' },
    { severity: 'Warning', focus_node: 'http://ex.org/b', path: null, message: 'deprecated' },
  ],
};

describe('unwrapValidationRun', () => {
  it('unwraps the full validate envelope { report, run_id, ran_at }', () => {
    const run = unwrapValidationRun({
      report: fullReport,
      run_id: 'run-123',
      ran_at: '2026-06-11T10:00:00Z',
    });
    expect(run.report).not.toBeNull();
    expect(run.report!.conforms).toBe(false);
    expect(run.report!.results_count).toBe(2);
    expect(run.report!.results).toHaveLength(2);
    expect(run.runId).toBe('run-123');
    expect(run.ranAt).toBe('2026-06-11T10:00:00Z');
    expect(run.test).toBe(false);
  });

  it('flags test (dry-run) envelopes', () => {
    const run = unwrapValidationRun({
      report: { conforms: true, results_count: 0, results: [] },
      run_id: null,
      ran_at: '2026-06-11T10:00:00Z',
      test: true,
    });
    expect(run.test).toBe(true);
    expect(run.report!.conforms).toBe(true);
    expect(run.runId).toBeNull();
  });

  it('returns a null report when the envelope has no report', () => {
    expect(unwrapValidationRun({ run_id: 'x', ran_at: 'y' }).report).toBeNull();
    expect(unwrapValidationRun(null).report).toBeNull();
    expect(unwrapValidationRun(undefined).report).toBeNull();
    expect(unwrapValidationRun('oops').report).toBeNull();
    expect(unwrapValidationRun({ report: 'not-a-report' }).report).toBeNull();
  });

  it('accepts stored-run records ({ id, report, run_timestamp })', () => {
    const run = unwrapValidationRun({
      id: 'stored-1',
      report: fullReport,
      run_timestamp: '2026-06-10T09:00:00Z',
      conforms: false,
      results_count: 2,
    });
    expect(run.report!.results_count).toBe(2);
    expect(run.runId).toBe('stored-1');
    expect(run.ranAt).toBe('2026-06-10T09:00:00Z');
  });

  it('defensively accepts a bare report (legacy response shape)', () => {
    const run = unwrapValidationRun(fullReport);
    expect(run.report!.conforms).toBe(false);
    expect(run.report!.results).toHaveLength(2);
    expect(run.runId).toBeNull();
    expect(run.ranAt).toBeNull();
  });

  it('normalizes missing results / results_count', () => {
    const run = unwrapValidationRun({ report: { conforms: true } });
    expect(run.report!.results).toEqual([]);
    expect(run.report!.results_count).toBe(0);

    const counted = unwrapValidationRun({
      report: { conforms: false, results: [{ severity: 'Violation' }] },
    });
    expect(counted.report!.results_count).toBe(1);
  });
});

describe('validationErrorMessage', () => {
  it('surfaces the backend 400 message (no-shapes case)', () => {
    const err = Object.assign(
      new Error('No shapes graph configured. Attach shapes in SHACL Studio, set a graph role to "shapes", or pass shapes_graph.'),
      { status: 400 },
    );
    expect(validationErrorMessage(err, 'fallback')).toMatch(/No shapes graph configured/);
  });

  it('falls back when the error has no usable message', () => {
    expect(validationErrorMessage(new Error(''), 'Validation failed.')).toBe('Validation failed.');
    expect(validationErrorMessage(null, 'Validation failed.')).toBe('Validation failed.');
    expect(validationErrorMessage({ status: 500 }, 'Validation failed.')).toBe('Validation failed.');
    expect(validationErrorMessage({ message: '   ' }, 'Validation failed.')).toBe('Validation failed.');
  });

  it('uses the built-in default fallback when none is given', () => {
    expect(validationErrorMessage(null)).toBe('Validation failed');
  });
});
