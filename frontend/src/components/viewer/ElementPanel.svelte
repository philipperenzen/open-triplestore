<script>
  // Linked-data panel for the dataset viewer: shows the selected element's RDF
  // (label, types, outgoing properties) via the existing browse API, with terms
  // rendered by RdfTerm so IRIs stay navigable. Selecting in the map or the 3D
  // canvas drives this panel.
  import { t as i18nT } from 'svelte-i18n';
  import { browseResource } from '../../lib/api.js';
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import { Link } from '../../lib/router/index.js';
  import RdfTerm from '../RdfTerm.svelte';

  export let iri = '';
  export let datasetId = '';
  /** The feed element (label/types/files), shown while RDF loads. */
  export let element = null;

  let data = null;
  let loading = false;
  let error = '';

  async function load(target) {
    if (!target) {
      data = null;
      return;
    }
    loading = true;
    error = '';
    try {
      data = await browseResource(target, { dataset_id: datasetId });
    } catch (e) {
      error = e?.message || 'failed';
      data = null;
    } finally {
      loading = false;
    }
  }

  $: load(iri);
</script>

<div class="element-panel">
  {#if !iri}
    <p class="hint">{$i18nT('pages.datasetViewer.selectHint')}</p>
  {:else}
    <header>
      <h3>{element?.label || shortenIRI(iri)}</h3>
      <Link to={`/resource?iri=${encodeURIComponent(iri)}`} class="btn btn-sm">
        {$i18nT('pages.datasetViewer.openResource')}
      </Link>
    </header>
    {#if element?.types?.length}
      <div class="types">
        {#each element.types as t}
          <span class="type-chip" title={t}>{shortenIRI(t)}</span>
        {/each}
      </div>
    {/if}
    {#if element?.gltf_url || element?.ifc_url}
      <div class="files">
        {#if element.gltf_url}<a href={element.gltf_url} target="_blank" rel="noreferrer">glTF</a>{/if}
        {#if element.ifc_url}<a href={element.ifc_url} target="_blank" rel="noreferrer">IFC</a>{/if}
        {#if element.ifc_guid}<code title="IFC GlobalId">{element.ifc_guid}</code>{/if}
      </div>
    {/if}

    {#if loading}
      <p class="hint">…</p>
    {:else if error}
      <p class="hint error">{error}</p>
    {:else if data}
      <table class="props">
        <tbody>
          {#each data.outgoing || [] as row}
            <tr>
              <td class="pred" title={row.p?.value}>{shortenIRI(row.p?.value || '')}</td>
              <td><RdfTerm term={row.o} graph="" /></td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  {/if}
</div>

<style>
  .element-panel {
    height: 100%;
    overflow: auto;
    padding: 0.75rem;
  }
  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }
  h3 {
    margin: 0;
    font-size: 1rem;
    overflow-wrap: anywhere;
  }
  .types {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    margin: 0.4rem 0;
  }
  .type-chip {
    font-size: 0.72rem;
    padding: 1px 8px;
    border-radius: 99px;
    background: var(--surface-2, #eef1f4);
    color: var(--text-2, #555);
  }
  .files {
    display: flex;
    gap: 0.6rem;
    align-items: center;
    margin: 0.3rem 0 0.6rem;
    font-size: 0.85rem;
  }
  .files code {
    font-size: 0.75rem;
    opacity: 0.8;
  }
  .props {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }
  .props td {
    padding: 3px 6px;
    vertical-align: top;
    border-top: 1px solid var(--border, #e3e7ea);
  }
  .props .pred {
    white-space: nowrap;
    color: var(--text-2, #666);
  }
  .hint {
    color: var(--text-2, #777);
    font-size: 0.9rem;
  }
  .hint.error {
    color: #c0392b;
  }
</style>
