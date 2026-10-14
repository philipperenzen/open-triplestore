<script lang="ts">
  // Reasoning without a dataset: one-off SWRL runs over the default graph,
  // OWL 2 DL checks of a Turtle premise, the QL rewriting of a query, the
  // shared entailment graphs and the caller's background jobs.
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Brain, Loader2 } from 'lucide-svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import SectionNav from '../components/SectionNav.svelte';
  import SwrlRunner from '../components/reasoning/SwrlRunner.svelte';
  import ConsistencyCheck from '../components/reasoning/ConsistencyCheck.svelte';
  import QlRewrite from '../components/reasoning/QlRewrite.svelte';
  import ReasoningJobs from '../components/reasoning/ReasoningJobs.svelte';
  import { getReasoningStatus } from '../lib/api';
  import { Link } from '../lib/router/index.js';
  import '../components/reasoning/reasoning.css';

  let status: { graphs: Record<string, number>; dl_backend: string | null } | null = null;
  let statusError = '';
  let dlBackend: string | null = null;
  let jobsKey = 0;

  onMount(async () => {
    try {
      status = await getReasoningStatus();
      dlBackend = status.dl_backend;
    } catch (e) {
      statusError = (e as Error)?.message || String(e);
    }
  });
</script>

<PageHeader title={$t('pages.reasoning.title')} icon={Brain} />
<p class="page-intro">{$t('pages.reasoning.intro')} <Link to="/datasets">{$t('pages.reasoning.datasetsLink')}</Link></p>

<SectionNav sections={[
  { id: 'status', label: $t('pages.reasoning.navStatus') },
  { id: 'swrl-run', label: $t('pages.datasetReasoning.navSwrl') },
  { id: 'dl-check', label: $t('pages.datasetReasoning.navCheck') },
  { id: 'rewrite', label: $t('pages.datasetReasoning.navRewrite') },
  { id: 'jobs', label: $t('pages.datasetReasoning.navJobs') },
]} />

<div class="stack">
  <div class="card rz-card" id="status">
    <h3 class="rz-label">{$t('pages.reasoning.statusHeading')}</h3>
    {#if statusError}
      <p class="rz-msg rz-msg-bad">{statusError}</p>
    {:else if !status}
      <p class="rz-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
    {:else}
      <table class="rz-table">
        <thead><tr><th>{$t('pages.reasoning.graph')}</th><th class="rz-num">{$t('pages.reasoning.triples')}</th></tr></thead>
        <tbody>{#each Object.entries(status.graphs) as [g, n]}<tr><td><code>{g}</code></td><td class="rz-num">{n}</td></tr>{/each}</tbody>
      </table>
      <p class="rz-muted">{$t('components.reasoning.dlBackend')}: {#if status.dl_backend}<code>{status.dl_backend}</code>{:else}{$t('components.reasoning.dlBackendNone')}{/if}</p>
    {/if}
  </div>
  <SwrlRunner />
  <ConsistencyCheck {dlBackend} on:queued={() => (jobsKey += 1)} />
  <QlRewrite />
  <ReasoningJobs refreshKey={jobsKey} />
</div>

<style>
  .page-intro { margin: 0 0 1rem; color: var(--ink-500); font-size: 0.9rem; }
  .stack { display: grid; gap: 1rem; margin-top: 1rem; }
</style>
