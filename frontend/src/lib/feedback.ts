// The feedback dialog is mounted once in App.svelte; anything can open it
// (sidebar button, docs help card) and preselect what kind of report it is.
import { writable } from 'svelte/store';

export type FeedbackKind = 'bug' | 'feature' | 'question' | 'other';
export type FeedbackStatus = 'open' | 'in_progress' | 'resolved' | 'closed';

export const FEEDBACK_KINDS: FeedbackKind[] = ['bug', 'feature', 'question', 'other'];
export const FEEDBACK_STATUSES: FeedbackStatus[] = ['open', 'in_progress', 'resolved', 'closed'];

// Mirrors the server's limits in src/feedback/mod.rs.
export const FEEDBACK_TITLE_MAX = 200;
export const FEEDBACK_BODY_MAX = 10_000;

export const feedbackDialog = writable<{ open: boolean; kind: FeedbackKind | null }>({
  open: false,
  kind: null,
});

export function openFeedback(kind: FeedbackKind | null = null): void {
  feedbackDialog.set({ open: true, kind });
}

export function closeFeedback(): void {
  feedbackDialog.update((s) => ({ ...s, open: false }));
}

/**
 * The page a report is sent from, as the reporter's path only: the query
 * string is left out because it can carry a whole SPARQL query or a token.
 */
export function currentPage(): string {
  return window.location.pathname;
}

/**
 * A stored page is linkable only as an in-app path. Anything else (a scheme,
 * a protocol-relative `//host`) is shown as text, never followed.
 */
export function isInAppPath(page: string | null | undefined): boolean {
  return !!page && page.startsWith('/') && !page.startsWith('//') && !page.startsWith('/\\');
}
