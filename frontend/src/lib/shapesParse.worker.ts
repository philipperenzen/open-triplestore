/// <reference lib="webworker" />
// Shape graph parse worker. A multi-megabyte shapes graph takes over a second
// to turn into the builder's model, which used to freeze the page while it
// ran. The model is plain data (objects, arrays, strings, numbers), so it
// crosses back to the page by structured clone, shared named property shapes
// included: a clone keeps two references to one object as one object.
//
// Only parsing runs here. Serialising stays on the page because it shortens
// IRIs with the prefix table the page loads at runtime, which a worker would
// not have.

import { parseShapesGraph } from './shaclModel';

const scope = self as unknown as DedicatedWorkerGlobalScope;

scope.onmessage = (ev: MessageEvent<{ ttl: string }>) => {
  try {
    scope.postMessage({ graph: parseShapesGraph(ev.data?.ttl ?? '') });
  } catch (e) {
    scope.postMessage({ error: e instanceof Error ? e.message : String(e) });
  }
};

// Tells the page the script loaded, so a later error is known to come from
// the parse rather than from a worker that never started.
scope.postMessage({ ready: true });
