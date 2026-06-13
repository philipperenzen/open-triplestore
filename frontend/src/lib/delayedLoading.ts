import { writable, type Readable } from 'svelte/store';

/**
 * A loading flag that only flips to `true` once a load has been running for
 * `delay` ms — so a sub-half-second navigation/pagination never flashes a
 * spinner or skeleton (the "blank page for a microsecond" problem). It flips
 * back to `false` immediately when loading ends.
 *
 * Usage in a component:
 *   const busy = delayedLoading();        // 480ms by default
 *   const showLoading = busy.show;        // a readable store
 *   $: busy.set(loading);                 // mirror your own `loading` flag
 *   // template: {#if $showLoading}<Skeleton/>{:else}<Rows/>{/if}
 *
 * Remember to call `busy.cancel()` in onDestroy if the component can unmount
 * mid-load (cancels the pending timer).
 */
export interface DelayedLoading {
  /** Readable<boolean>: true only after `delay` ms of continuous loading. */
  show: Readable<boolean>;
  /** Mirror your own loading flag here (idempotent — repeats are ignored). */
  set(loading: boolean): void;
  /** Cancel any pending timer and force the flag off. */
  cancel(): void;
}

export function delayedLoading(delay = 480): DelayedLoading {
  const show = writable(false);
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pending = false;

  function clear() {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
  }

  function set(loading: boolean) {
    if (loading === pending) return;
    pending = loading;
    clear();
    if (loading) {
      timer = setTimeout(() => {
        timer = null;
        show.set(true);
      }, delay);
    } else {
      show.set(false);
    }
  }

  function cancel() {
    clear();
    pending = false;
    show.set(false);
  }

  return { show, set, cancel };
}
