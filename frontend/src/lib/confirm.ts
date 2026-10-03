import { writable } from 'svelte/store';

// The app's own dialogs, as promises, in place of the browser's blocking
// `window.confirm()` / `window.prompt()`:
//   if (!(await askConfirm({ title, message }))) return;
//   const name = await askText({ title, defaultValue });   // null on cancel
// <ConfirmHost> (mounted once in App.svelte) renders the open request with
// ConfirmModal and settles the promise.

export interface ConfirmRequest {
  title: string;
  message?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** 'danger' (red, the default) for deletes; 'warning' (amber) for overwrites; 'primary' for plain questions. */
  variant?: 'danger' | 'warning' | 'primary';
}

export interface TextRequest extends ConfirmRequest {
  /** The text field's label (defaults to the title). */
  label?: string;
  defaultValue?: string;
}

interface Pending extends TextRequest {
  /** Set for askText: the dialog shows a text field. */
  text: boolean;
  resolve: (answer: string | null) => void;
}

export const pendingConfirm = writable<Pending | null>(null);

let current: Pending | null = null;
pendingConfirm.subscribe((p) => { current = p; });

function open(req: TextRequest, text: boolean): Promise<string | null> {
  // A second request while one is open answers the first with "no".
  current?.resolve(null);
  return new Promise<string | null>((resolve) => {
    pendingConfirm.set({ variant: text ? 'primary' : 'danger', ...req, text, resolve });
  });
}

/** Ask the user to confirm. Resolves true on confirm, false on cancel/Escape. */
export async function askConfirm(req: ConfirmRequest): Promise<boolean> {
  return (await open(req, false)) !== null;
}

/** Ask the user for a line of text. Resolves the trimmed text, or null on cancel or an empty answer. */
export async function askText(req: TextRequest): Promise<string | null> {
  const answer = await open(req, true);
  const trimmed = answer?.trim() ?? '';
  return trimmed ? trimmed : null;
}

/** Settle the open request (called by ConfirmHost): a string confirms, null cancels. */
export function settleConfirm(answer: string | null): void {
  const p = current;
  pendingConfirm.set(null);
  p?.resolve(answer);
}
