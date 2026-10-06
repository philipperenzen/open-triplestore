<script lang="ts">
  // ShEx validation of one dataset (POST /api/datasets/:id/shex/validate).
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { ShieldCheck } from 'lucide-svelte';
  import PageHeader from '../components/PageHeader.svelte';
  import ShexValidator from '../components/reasoning/ShexValidator.svelte';
  import { getDataset } from '../lib/api';
  import { isAuthenticated } from '../lib/stores';

  export let id: string;
  let name = id;

  onMount(async () => {
    try { name = (await getDataset(id))?.name ?? id; } catch { /* the validator reports access errors */ }
  });

  $: breadcrumbs = [
    { label: $t('pages.apiServices.crumbDatasets'), href: '/datasets' },
    { label: name, href: `/datasets/${id}` },
    { label: $t('pages.shex.title') },
  ];
</script>

<PageHeader title={$t('pages.shex.title')} icon={ShieldCheck} {breadcrumbs} />
{#if $isAuthenticated}
  <ShexValidator datasetId={id} />
{:else}
  <p class="error">{$t('pages.shex.signIn')}</p>
{/if}
