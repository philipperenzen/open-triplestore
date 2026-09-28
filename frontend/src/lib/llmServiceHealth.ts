/**
 * Pure helper behind the "LLM services" section of the Service health
 * popover. It turns a `GET /api/llm/health` body into dots, labels and words
 * the popover can show, and knows nothing about the DOM or fetch, so it is
 * tested as a function.
 *
 * LLM state is informational: it never feeds the sidebar badge or the
 * degraded banner, which describe the server itself.
 */
import type { LlmHealth, LlmServiceId } from './api.js';

/** A dot colour: `pending` = not known yet, or not knowable (neutral grey).
 *  There is no error colour: red in the popover means the server itself is
 *  failing, and an absent or unreachable LLM is not that. */
export type LlmHealthState = 'ok' | 'warn' | 'pending';

export interface LlmGatewayRow {
  state: LlmHealthState;
  /** i18n key for the right-hand status word. */
  detailKey: string;
  /** Gateway base URL for a tooltip, when the server reported one. */
  address: string | null;
}

export interface LlmServiceRow {
  id: LlmServiceId;
  /** i18n key naming the feature. */
  labelKey: string;
  /** The model the feature uses; shown as the detail text. */
  model: string;
  state: LlmHealthState;
  /** i18n key for a short note explaining a non-ok state, shown as text under
   *  the row (not only as a dot colour or tooltip), or `null`. */
  noteKey: string | null;
}

export interface LlmServiceHealthView {
  gateway: LlmGatewayRow;
  services: LlmServiceRow[];
}

/** The features the server reports on, in display order, with their names. */
export const LLM_SERVICES: ReadonlyArray<{ id: LlmServiceId; labelKey: string }> = [
  { id: 'chat', labelKey: 'nav.llmServiceChat' },
  { id: 'sparql', labelKey: 'nav.llmServiceSparql' },
  { id: 'shacl', labelKey: 'nav.llmServiceShacl' },
];

const MODEL_NOT_LISTED_KEY = 'nav.llmModelNotListed';

/**
 * - Nothing yet (`null` / `undefined`): the gateway row reads "checking…" and
 *   there are no feature rows.
 * - The request itself failed (`llmHealth()`'s `error: true` fallback): the
 *   gateway row reads "unknown" with a neutral dot and there are no feature
 *   rows — the server's LLM configuration was not read, so nothing is claimed.
 * - Reachable: gateway ok; one row per feature, `warn` with a "not listed"
 *   note when the gateway's model list does not name its model
 *   (`listed === false`), else ok.
 * - Unreachable but configured: gateway "unreachable" and every feature row
 *   `warn` — the features are expected to work and do not. Yellow, never red:
 *   the LLM is optional, and the bundled docker-compose.yml sets
 *   `LLM_GATEWAY_URL` even when its `ollama` service was never started.
 * - Unreachable and not configured (or an older server that does not say):
 *   gateway `warn` ("not configured") and no feature rows — nothing is broken,
 *   there is just no LLM.
 *
 * A missing or malformed `services` array yields no feature rows; unknown ids
 * are ignored, and the order is always chat, sparql, shacl.
 */
export function llmServiceHealthView(status: LlmHealth | null | undefined): LlmServiceHealthView {
  if (!status) {
    return { gateway: { state: 'pending', detailKey: 'nav.llmChecking', address: null }, services: [] };
  }
  if (status.error === true) {
    return { gateway: { state: 'pending', detailKey: 'nav.llmUnknown', address: null }, services: [] };
  }
  const address = typeof status.gateway === 'string' && status.gateway ? status.gateway : null;

  if (status.reachable === true) {
    return {
      gateway: { state: 'ok', detailKey: 'nav.llmReachable', address },
      services: serviceRows(status, (listed) =>
        listed === false ? ['warn', MODEL_NOT_LISTED_KEY] : ['ok', null],
      ),
    };
  }
  if (status.configured === true) {
    return {
      gateway: { state: 'warn', detailKey: 'nav.llmUnreachable', address },
      services: serviceRows(status, () => ['warn', null]),
    };
  }
  return { gateway: { state: 'warn', detailKey: 'nav.llmNotConfigured', address }, services: [] };
}

function serviceRows(
  status: LlmHealth,
  classify: (listed: boolean | null) => [LlmHealthState, string | null],
): LlmServiceRow[] {
  const reported = Array.isArray(status.services) ? status.services : null;
  if (!reported) return [];
  const rows: LlmServiceRow[] = [];
  for (const { id, labelKey } of LLM_SERVICES) {
    const entry = reported.find((s) => s && typeof s === 'object' && s.id === id);
    if (!entry) continue;
    const listed = typeof entry.listed === 'boolean' ? entry.listed : null;
    const [state, noteKey] = classify(listed);
    rows.push({
      id,
      labelKey,
      model: typeof entry.model === 'string' ? entry.model : '',
      state,
      noteKey,
    });
  }
  return rows;
}
