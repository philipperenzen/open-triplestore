// Off-thread parsing for the shapes editor.
//
// `parseShapesGraph` is synchronous and, on a 20 MB shapes graph, takes well
// over a second on a fast machine: long enough that a page running it on the
// main thread never even paints its loading state. Large documents are parsed
// in a module worker instead (see shapesParse.worker.ts); small ones stay
// inline, where a worker round trip would cost more than the parse itself.

import { parseShapesGraph, type ShapesGraph } from './shaclModel';

/**
 * Documents at least this long are parsed off the main thread. Measured on
 * generated graphs: 0.56 MB parses in about 36 ms and 1.7 MB in about 90 ms,
 * while starting a worker and cloning its result back costs a few tens of
 * milliseconds of its own. Half a megabyte is where the worker starts to pay.
 */
export const WORKER_PARSE_MIN_CHARS = 512 * 1024;

export function isLargeShapesDocument(ttl: string | null | undefined): boolean {
  return typeof ttl === 'string' && ttl.length >= WORKER_PARSE_MIN_CHARS;
}

/** The worker could not be started at all; only this warrants an inline parse. */
class WorkerUnavailable extends Error {}

/**
 * Resolves after the browser has had a chance to paint, so a loading state set
 * just before a long synchronous step is on screen while it runs.
 */
export function nextPaint(): Promise<void> {
  return new Promise((resolve) => {
    const later = () => setTimeout(resolve, 0);
    if (typeof requestAnimationFrame === 'function') requestAnimationFrame(later);
    else later();
  });
}

let workerBroken = false;

type Job = { worker: Worker; settle: (g: ShapesGraph | null) => void };

function parseInWorker(ttl: string, owner: { current: Job | null }): Promise<ShapesGraph | null> {
  return new Promise((resolve, reject) => {
    let worker: Worker;
    try {
      worker = new Worker(new URL('./shapesParse.worker.ts', import.meta.url), { type: 'module' });
    } catch (e) {
      reject(new WorkerUnavailable(e instanceof Error ? e.message : String(e)));
      return;
    }
    // One worker per parse, terminated as soon as it answers: the parse leaves
    // a heap several times the document's size behind, and terminating is the
    // quickest way to hand that memory back.
    const job: Job = { worker, settle: resolve };
    owner.current = job;
    let loaded = false;
    const done = () => {
      worker.terminate();
      if (owner.current === job) owner.current = null;
    };
    worker.onerror = (e) => {
      done();
      // Before the worker said it loaded, the script itself is what failed
      // (blocked bundle, CSP), so an inline parse is the way out. After that
      // it is the parse that failed, and repeating it inline would only block
      // the page doing the same thing.
      const message = e.message || 'shapes parse worker failed';
      reject(loaded ? new Error(message) : new WorkerUnavailable(message));
    };
    worker.onmessage = (ev: MessageEvent<{ ready?: boolean; graph?: ShapesGraph; error?: string }>) => {
      const msg = ev.data || {};
      if (msg.ready) {
        loaded = true;
        return;
      }
      done();
      if (msg.graph) resolve(msg.graph);
      else reject(new Error(msg.error || 'shapes parse failed'));
    };
    worker.postMessage({ ttl });
  });
}

/**
 * A parser for one editor. Each `parse` supersedes the one before it: the
 * worker still busy with older text is terminated, and that call resolves to
 * `null`. Without Worker support (tests, very old browsers) the parse runs
 * inline, but only after the browser has painted.
 */
export function createShapesParser() {
  const owner: { current: Job | null } = { current: null };
  const cancel = () => {
    const job = owner.current;
    if (!job) return;
    owner.current = null;
    job.worker.terminate();
    job.settle(null);
  };
  const parse = async (ttl: string): Promise<ShapesGraph | null> => {
    cancel();
    if (!workerBroken && typeof Worker !== 'undefined') {
      try {
        return await parseInWorker(ttl, owner);
      } catch (e) {
        if (!(e instanceof WorkerUnavailable)) throw e;
        workerBroken = true;
      }
    }
    await nextPaint();
    return parseShapesGraph(ttl);
  };
  return { parse, cancel };
}
