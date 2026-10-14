<script lang="ts">
  // The dataset page's "Exchange" section: the formats a dataset is
  // published in or brought in from beyond plain RDF files — its event
  // stream (LDES), linked-document containers (ICDD), CityJSON, and the IDS
  // check of a building's IFC import. Each panel reads its own API; this
  // only lays them out and passes "graphs changed" up to the page.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { ArrowLeftRight } from 'lucide-svelte';
  import LdesPublishPanel from './LdesPublishPanel.svelte';
  import ContainerPanel from './ContainerPanel.svelte';
  import CityJsonPanel from './CityJsonPanel.svelte';
  import IdsCheckPanel from './IdsCheckPanel.svelte';
  import { idsProjectionGraphs } from '../../lib/exchange';
  import './exchange.css';

  export let datasetId: string;
  export let datasetName = '';
  export let canWrite = false;
  export let authenticated = false;
  export let graphs: (string | { graph_iri: string })[] = [];

  const dispatch = createEventDispatcher<{ changed: void }>();
  const changed = () => dispatch('changed');

  $: hasProjection = idsProjectionGraphs(graphs).length > 0;
</script>

<div class="card" id="exchange" data-testid="dataset-exchange">
  <div class="xc-head">
    <ArrowLeftRight size={15} />
    <h3>{$t('components.exchange.heading')}</h3>
  </div>
  <p class="xc-intro">{$t('components.exchange.intro')}</p>

  <LdesPublishPanel {datasetId} />
  <ContainerPanel {datasetId} {canWrite} {authenticated} on:changed={changed} />
  {#if canWrite}
    <CityJsonPanel {datasetId} on:changed={changed} />
  {/if}
  {#if authenticated && hasProjection}
    <IdsCheckPanel {datasetId} {graphs} {datasetName} />
  {/if}
</div>
