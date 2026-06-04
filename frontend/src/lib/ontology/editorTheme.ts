// Shared light/dark theme + syntax highlighting for the CodeMirror SPARQL/Turtle
// editor. Mirrored in spirit (same palette) with the companion graph viewer's editor theme so the
// two apps' editors look consistent. One highlight style serves both SPARQL and
// Turtle since their StreamLanguages emit the same lezer tags.

import { EditorView } from '@codemirror/view';
import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import type { Extension } from '@codemirror/state';

export type ThemePref = 'auto' | 'light' | 'dark';

const lightHighlight = HighlightStyle.define([
  { tag: tags.keyword, color: '#7c3aed', fontWeight: '600' },
  { tag: tags.variableName, color: '#c05000' },
  { tag: tags.string, color: '#166534' },
  { tag: tags.regexp, color: '#1e3a5f' },
  { tag: tags.namespace, color: '#7c3aed' },
  { tag: tags.number, color: '#0550ae', fontWeight: '500' },
  { tag: tags.bool, color: '#0550ae', fontWeight: '500' },
  { tag: tags.comment, color: '#6b7280', fontStyle: 'italic' },
  { tag: tags.operator, color: '#374151' },
  { tag: tags.meta, color: '#b45309' },
]);

const darkHighlight = HighlightStyle.define([
  { tag: tags.keyword, color: '#c4b5fd', fontWeight: '600' },
  { tag: tags.variableName, color: '#fdba74' },
  { tag: tags.string, color: '#86efac' },
  { tag: tags.regexp, color: '#93c5fd' },
  { tag: tags.namespace, color: '#c4b5fd' },
  { tag: tags.number, color: '#7dd3fc', fontWeight: '500' },
  { tag: tags.bool, color: '#7dd3fc', fontWeight: '500' },
  { tag: tags.comment, color: '#94a3b8', fontStyle: 'italic' },
  { tag: tags.operator, color: '#cbd5e1' },
  { tag: tags.meta, color: '#fcd34d' },
]);

// Tooltip styling is shared across light/dark (it is a dark chip on both).
const tooltipTheme = {
  '.cm-ontology-tt': {
    background: '#1f2937', color: '#f9fafb', padding: '6px 8px', borderRadius: '4px',
    fontSize: '11px', maxWidth: '320px', boxShadow: '0 4px 12px rgba(0,0,0,0.35)',
  },
  '.cm-ontology-tt .tt-label': { fontWeight: '600', marginBottom: '2px' },
  '.cm-ontology-tt .tt-iri': { opacity: 0.7, fontFamily: 'monospace', fontSize: '10px', marginBottom: '2px', wordBreak: 'break-all' },
  '.cm-ontology-tt .tt-kind': { color: '#fbbf24', fontSize: '10px', textTransform: 'uppercase', marginBottom: '4px' },
  '.cm-ontology-tt .tt-comment': { opacity: 0.85, fontStyle: 'italic', whiteSpace: 'pre-wrap', marginTop: '4px' },
  '.cm-ontology-tt .tt-row': { display: 'flex', alignItems: 'center', gap: '4px', flexWrap: 'wrap', margin: '3px 0', fontSize: '10px' },
  '.cm-ontology-tt .tt-k': { color: '#9ca3af', textTransform: 'uppercase', fontSize: '9px', letterSpacing: '0.04em' },
  '.cm-ontology-tt .tt-chip': { background: '#374151', padding: '1px 5px', borderRadius: '8px', fontFamily: 'monospace' },
  '.cm-ontology-tt .tt-loading': { marginTop: '4px', opacity: 0.6, fontSize: '10px' },
} as const;

function lightTheme(height: string) {
  return EditorView.theme({
    '&': { height, fontSize: '13px', fontFamily: "'SF Mono', 'Fira Code', monospace" },
    '.cm-scroller': { overflow: 'auto' },
    '.cm-content': { padding: '8px 0', caretColor: '#1a1a2e' },
    '.cm-line': { padding: '0 8px' },
    '.cm-focused': { outline: 'none' },
    '&.cm-editor': { border: '1px solid #d0d7de', borderRadius: '6px', background: '#fff' },
    '&.cm-editor.cm-focused': { borderColor: '#4a90d9', boxShadow: '0 0 0 3px rgba(74,144,217,0.15)' },
    '.cm-gutters': { background: '#f6f8fa', borderRight: '1px solid #d0d7de', color: '#8c959f' },
    '.cm-activeLineGutter': { background: '#eaf5fe' },
    '.cm-activeLine': { background: 'rgba(74,144,217,0.07)' },
    '.cm-selectionBackground': { background: 'rgba(74,144,217,0.30) !important' },
    '&.cm-focused .cm-selectionBackground': { background: 'rgba(74,144,217,0.45) !important' },
    '.cm-selectionMatch': { background: '#dbeafe', outline: '1px solid #93c5fd' },
    '::selection': { backgroundColor: 'rgba(74,144,217,0.40)' },
    ...tooltipTheme,
  });
}

function darkTheme(height: string) {
  return EditorView.theme({
    '&': { height, fontSize: '13px', fontFamily: "'SF Mono', 'Fira Code', monospace", color: '#e2e8f0' },
    '.cm-scroller': { overflow: 'auto' },
    '.cm-content': { padding: '8px 0', caretColor: '#7dd3fc' },
    '.cm-line': { padding: '0 8px' },
    '.cm-focused': { outline: 'none' },
    '&.cm-editor': { border: '1px solid #334155', borderRadius: '6px', background: '#0f172a' },
    '&.cm-editor.cm-focused': { borderColor: '#3b82f6', boxShadow: '0 0 0 3px rgba(59,130,246,0.25)' },
    '.cm-gutters': { background: '#111827', borderRight: '1px solid #334155', color: '#64748b' },
    '.cm-activeLineGutter': { background: '#1e293b' },
    '.cm-activeLine': { background: 'rgba(59,130,246,0.10)' },
    '.cm-selectionBackground': { background: 'rgba(59,130,246,0.32) !important' },
    '&.cm-focused .cm-selectionBackground': { background: 'rgba(59,130,246,0.45) !important' },
    '.cm-selectionMatch': { background: '#1e3a5f', outline: '1px solid #2563eb' },
    '::selection': { backgroundColor: 'rgba(59,130,246,0.40)' },
    ...tooltipTheme,
  }, { dark: true });
}

/** Theme + highlight extension for the given resolved mode. */
export function buildEditorTheme(isDark: boolean, height = '280px'): Extension {
  return isDark
    ? [darkTheme(height), syntaxHighlighting(darkHighlight)]
    : [lightTheme(height), syntaxHighlighting(lightHighlight)];
}

/** Resolve a theme preference to a concrete light/dark boolean, consulting the
 *  document (`.dark` class / `data-theme`) then the OS preference. */
export function resolveDark(pref: ThemePref = 'auto'): boolean {
  if (pref === 'light') return false;
  if (pref === 'dark') return true;
  if (typeof document !== 'undefined') {
    const el = document.documentElement;
    if (el.classList.contains('dark') || el.getAttribute('data-theme') === 'dark') return true;
    if (el.classList.contains('light') || el.getAttribute('data-theme') === 'light') return false;
    // Respect an app that declares a fixed `color-scheme` (e.g. a light-only app)
    // so the editor doesn't flip dark on a light page just because the OS is dark.
    if (typeof getComputedStyle !== 'undefined') {
      const cs = getComputedStyle(el).colorScheme || '';
      if (/\bdark\b/.test(cs) && !/\blight\b/.test(cs)) return true;
      if (/\blight\b/.test(cs) && !/\bdark\b/.test(cs)) return false;
    }
  }
  if (typeof window !== 'undefined' && window.matchMedia) {
    return window.matchMedia('(prefers-color-scheme: dark)').matches;
  }
  return false;
}

/** Subscribe to environment theme changes (OS preference + html class/attr).
 *  Returns an unsubscribe function. No-op outside the browser. */
export function onThemeChange(cb: () => void): () => void {
  const cleanups: Array<() => void> = [];
  if (typeof window !== 'undefined' && window.matchMedia) {
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const handler = () => cb();
    mq.addEventListener?.('change', handler);
    cleanups.push(() => mq.removeEventListener?.('change', handler));
  }
  if (typeof document !== 'undefined' && typeof MutationObserver !== 'undefined') {
    const obs = new MutationObserver(cb);
    obs.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'data-theme'] });
    cleanups.push(() => obs.disconnect());
  }
  return () => cleanups.forEach((c) => c());
}
