<script lang="ts">
  // A dataset's reasoning: its entailment regime and the last run, runs and
  // checks on request (with their background jobs), its stored SWRL rules,
  // one-off SWRL runs with a regime, and the OWL 2 QL rewriting of a query.
  // Everything here was API-only (docs/reasoning.md, docs/swrl.md).
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Brain, Lock } from 'lucide-svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import SectionNav from '../components/SectionNav.svelte';
  import EntailmentSettings from '../components/reasoning/EntailmentSettings.svelte';
  import MaterialisePanel from '../components/reasoning/MaterialisePanel.svelte';
  import ConsistencyCheck from '../components/reasoning/ConsistencyCheck.svelte';
  import ReasoningJobs from '../components/reasoning/ReasoningJobs.svelte';
  import DatasetRules from '../components/reasoning/DatasetRules.svelte';
  import SwrlRunner from '../components/reasoning/SwrlRunner.svelte';
  import QlRewrite from '../components/reasoning/QlRewrite.svelte';
  import { getDataset, type EntailmentState } from '../lib/api';
  import { isAuthenticated } from '../lib/stores';
  import { Link } from '../lib/router/index.js';

  export let id: string;

  let dataset: { name?: string; can_write?: boolean } | null = null;
  let error = '';
  let entailment: EntailmentState | null = null;
  let jobsKey = 0;
  let settingsKey = 0;

  onMount(async () => {
    try {
      dataset = await getDataset(id);
    } catch (e) {
      error = (e as Error)?.message || String(e);
    }
  });

  $: canWrite = !!dataset?.can_write && $isAuthenticated;
  $: dlBackend = entailment?.dl_backend ?? null;
  $: breadcrumbs = [
    { label: $t('pages.apiServices.crumbDatasets'), href: '/datasets' },
    { label: dataset?.name ?? id, href: `/datasets/${id}` },
    { label: $t('pages.datasetReasoning.title') },
  ];
  const queued = () => { jobsKey += 1; };
</script>

<PageHeader title={$t('pages.datasetReasoning.title')} icon={Brain} {breadcrumbs} />
<p class="page-intro">{$t('pages.datasetReasoning.intro')} <Link to="/docs/reasoning">{$t('pages.datasetReasoning.docs')}</Link></p>

{#if error}
  <p class="error">{error}</p>
{:else}
  <SectionNav sections={[
    { id: 'entailment-settings', label: $t('pages.datasetReasoning.navSettings') },
    { id: 'materialise', label: $t('pages.datasetReasoning.navRun'), visible: $isAuthenticated },
    { id: 'dl-check', label: $t('pages.datasetReasoning.navCheck'), visible: $isAuthenticated },
    { id: 'jobs', label: $t('pages.datasetReasoning.navJobs'), visible: $isAuthenticated },
    { id: 'rules', label: $t('pages.datasetReasoning.navRules') },
    { id: 'swrl-run', label: $t('pages.datasetReasoning.navSwrl'), visible: $isAuthenticated },
    { id: 'rewrite', label: $t('pages.datasetReasoning.navRewrite'), visible: $isAuthenticated },
  ]} />

  <div class="stack">
    <EntailmentSettings datasetId={id} {canWrite} refreshKey={settingsKey} on:loaded={(e) => (entailment = e.detail)} />
    {#if $isAuthenticated}
      <MaterialisePanel datasetId={id} {canWrite} {dlBackend} eqRefDefault={!!entailment?.eq_ref} on:queued={queued} />
      <ConsistencyCheck datasetId={id} {dlBackend} on:queued={queued} />
      <ReasoningJobs datasetId={id} refreshKey={jobsKey} />
    {/if}
    <DatasetRules datasetId={id} {canWrite} on:changed={() => (settingsKey += 1)} />
    {#if $isAuthenticated}
      <SwrlRunner datasetId={id} {canWrite} inferenceGraph={entailment?.inference_graph ?? ''} />
      <QlRewrite datasetId={id} />
    {:else}
      <p class="signin"><Lock size={13} /> {$t('pages.datasetReasoning.signIn')}</p>
    {/if}
  </div>
{/if}

<style>
  .page-intro { margin: 0 0 1rem; color: var(--ink-500); font-size: 0.9rem; }
  .stack { display: grid; gap: 1rem; margin-top: 1rem; }
  .signin { display: flex; align-items: center; gap: 0.4rem; color: var(--ink-500); font-size: 0.85rem; }
</style>
