// SPARQL linter — thin wrapper kept for backwards-compatible imports.
// The implementation now lives in the shared diagnostics catalog
// (./sparqlDiagnostics) so that this app and the companion graph viewer report
// identical errors, warnings and hints. See that file to edit the wording.

export { sparqlLinter, analyzeSparql, SPARQL_DIAGNOSTICS } from './sparqlDiagnostics.js';
export type { DiagnosticContext, SparqlFinding, Severity } from './sparqlDiagnostics.js';
