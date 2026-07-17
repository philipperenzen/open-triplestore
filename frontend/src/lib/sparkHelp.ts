// Shared "Ask Spark" handoff: stash a plain-language explain prompt for a
// linked-data term in sessionStorage, then navigate to the Spark chat route.
// LlmChat.svelte reads + removes 'ots_spark_prompt' on mount and auto-sends it,
// mirroring SparqlEditor's 'ots_sparql_load' handoff.
import { navigate } from './router/index.js';

/** sessionStorage key the Spark chat page reads (and removes) on mount. */
export const SPARK_PROMPT_KEY = 'ots_spark_prompt';

/**
 * Open the Spark chat with a pre-filled prompt asking it to explain a term.
 * Pass whatever is known — an IRI, a friendly label, and/or a kind hint.
 */
export function openSparkExplain(
  { iri, label, kind }: { iri?: string; label?: string; kind?: string } = {},
): void {
  const name = label || iri || '';
  const kindHint = kind ? ` (a ${kind})` : '';
  const prompt =
    `Explain the linked-data term "${name}"${iri ? ` (<${iri}>)` : ''}${kindHint}. ` +
    'What does it mean in plain language, what is it typically used for, and ' +
    'give a small concrete example. Keep it concise.';
  try {
    sessionStorage.setItem(SPARK_PROMPT_KEY, prompt);
  } catch {
    // Private-mode / disabled storage: navigate anyway, just without the prefill.
  }
  navigate('/chat');
}
