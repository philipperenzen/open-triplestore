<script lang="ts">
  // Why a reasoning run or check failed, from the API's error body: an
  // inconsistency (rule and detail), no fixed point, input outside OWL 2 DL
  // (violations), a DL backend that is missing (503) or timed out (504).
  import { t } from 'svelte-i18n';
  import './reasoning.css';

  export let status: number | null = null;
  export let message = '';
  export let body: Record<string, unknown> | null = null;

  $: violations = (Array.isArray(body?.violations) ? body?.violations : []) as { rule: string; detail: string }[];
  $: hint = status === 503 ? 'unavailable' : status === 504 ? 'timeout' : status === 413 ? 'tooLarge' : body?.consistent === false ? 'inconsistent' : body?.converged === false ? 'notConverged' : body?.in_profile === false ? 'notInProfile' : null;
</script>

<div class="rz-msg rz-msg-bad" role="alert" data-testid="run-failure">
  {#if hint}<strong>{$t(`components.reasoning.failure.${hint}`)}</strong><br />{/if}
  {message}
  {#if typeof body?.rule === 'string'}
    <div><code>{body.rule}</code>{#if typeof body?.detail === 'string'} — {body.detail}{/if}</div>
  {/if}
  {#if violations.length}
    <ul class="rz-list">{#each violations as v}<li><code>{v.rule}</code> {v.detail}</li>{/each}</ul>
  {/if}
</div>
