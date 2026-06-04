import { writable } from 'svelte/store';

export type ToastType = 'info' | 'success' | 'error' | 'warning';

export interface Toast {
  id: number;
  message: string;
  type: ToastType;
}

let _id = 0;
export const toasts = writable<Toast[]>([]);

export function toast(message: string, type: ToastType = 'info', duration = 4000): number {
  const id = ++_id;
  toasts.update(ts => [...ts, { id, message, type }]);
  if (duration > 0) setTimeout(() => dismiss(id), duration);
  return id;
}

export function dismiss(id: number): void {
  toasts.update(ts => ts.filter(t => t.id !== id));
}

export const toastSuccess = (msg: string, dur?: number) => toast(msg, 'success', dur);
export const toastError   = (msg: string, dur?: number) => toast(msg, 'error',   dur ?? 6000);
export const toastInfo    = (msg: string, dur?: number) => toast(msg, 'info',    dur);
export const toastWarn    = (msg: string, dur?: number) => toast(msg, 'warning', dur ?? 5000);
