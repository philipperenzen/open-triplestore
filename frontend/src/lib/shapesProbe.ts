// Aggregation helpers for the post-import SHACL shapes probe.
//
// A bulk import can produce several graphs per file (auto-split routes shapes
// into a '{target}/shapes' subgraph, quad files carry their own graphs), so
// every IRI in file_results[].graph_iris must be probed with
// /api/shacl/detect-shapes — not just the first graph of the first file.

export interface SuggestedDataset {
  id: string;
  name: string;
  has_shapes?: boolean;
}

export interface DetectShapesResponse {
  shapes_detected?: boolean;
  shape_count?: number;
  suggested_datasets?: SuggestedDataset[];
}

export interface ImportFileResult {
  status?: string;
  /** First (primary) graph the server wrote for this file. */
  graphIri?: string;
  /** Every graph the server wrote for this file (file_results[].graph_iris). */
  graphIris?: string[];
}

/** Unique graph IRIs across all successful file results, in import order. */
export function collectGraphIris(fileResults: ImportFileResult[]): string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const fr of fileResults || []) {
    if (!fr || fr.status !== 'ok') continue;
    const iris = (fr.graphIris && fr.graphIris.length ? fr.graphIris : [fr.graphIri])
      .filter((iri): iri is string => !!iri);
    for (const iri of iris) {
      if (!seen.has(iri)) {
        seen.add(iri);
        out.push(iri);
      }
    }
  }
  return out;
}

export interface ShapesProbe {
  graphIri: string;
  /** null when the probe call failed — the graph is simply skipped. */
  result: DetectShapesResponse | null;
}

export interface ShapesProbeAggregate {
  shapesDetected: boolean;
  totalShapeCount: number;
  /** Every probed graph that holds shapes, with its shape count. */
  shapeGraphs: { graphIri: string; shapeCount: number }[];
  /** Union of dataset suggestions across probes, de-duplicated by id. */
  suggestedDatasets: SuggestedDataset[];
}

/** Merge per-graph detect-shapes results into one prompt-ready summary. */
export function aggregateShapesProbe(probes: ShapesProbe[]): ShapesProbeAggregate {
  const shapeGraphs: { graphIri: string; shapeCount: number }[] = [];
  const suggestedDatasets: SuggestedDataset[] = [];
  const seenDatasets = new Set<string>();
  let totalShapeCount = 0;
  for (const probe of probes || []) {
    const res = probe?.result;
    if (!res) continue;
    if (res.shapes_detected) {
      const count = typeof res.shape_count === 'number' ? res.shape_count : 0;
      shapeGraphs.push({ graphIri: probe.graphIri, shapeCount: count });
      totalShapeCount += count;
    }
    for (const ds of res.suggested_datasets || []) {
      const key = String(ds.id);
      if (!seenDatasets.has(key)) {
        seenDatasets.add(key);
        suggestedDatasets.push(ds);
      }
    }
  }
  return {
    shapesDetected: shapeGraphs.length > 0,
    totalShapeCount,
    shapeGraphs,
    suggestedDatasets,
  };
}
