<script lang="ts">
  // RDF Patch for one dataset: apply a patch (and record it in the patch log),
  // and browse, view and download the log (docs/versioning.md).
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { FileDiff } from 'lucide-svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import PatchApply from '../components/reasoning/PatchApply.svelte';
  import PatchLog from '../components/reasoning/PatchLog.svelte';
  import { getDataset, listDatasetGraphs, type PatchLog as Log } from '../lib/api';
  import { isAuthenticated } from '../lib/stores';

  export let id: string;
  let dataset: { name?: string; can_write?: boolean } | null = null;
  let graphs: string[] = [];
  let error = '';
  let logKey = 0;
  let log: Log | null = null;

  onMount(async () => {
    try {
      dataset = await getDataset(id);
      graphs = (await listDatasetGraphs(id)).map((g) => g.graph_iri);
    } catch (e) {
      error = (e as Error)?.message || String(e);
    }
  });

  $: canWrite = !!dataset?.can_write && $isAuthenticated;
  $: breadcrumbs = [
    { label: $t('pages.apiServices.crumbDatasets'), href: '/datasets' },
    { label: dataset?.name ?? id, href: `/datasets/${id}` },
    { label: $t('pages.datasetPatch.title') },
  ];
</script>

<PageHeader title={$t('pages.datasetPatch.title')} icon={FileDiff} {breadcrumbs} />
{#if error}
  <p class="error">{error}</p>
{:else}
  <div class="stack">
    <PatchApply datasetId={id} {canWrite} {graphs} latestId={log?.latest?.id ?? null} on:applied={() => (logKey += 1)} />
    <PatchLog datasetId={id} refreshKey={logKey} bind:log />
  </div>
{/if}

<style>
  .stack { display: grid; gap: 1rem; }
</style>
