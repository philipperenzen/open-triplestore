<script lang="ts">
  // Rules in one of the six SWRL syntaxes the server reads (docs/swrl.md),
  // with what that syntax needs: prefixes for SWRLAPI, an RDF serialisation
  // and base IRI for the RDF syntax.
  import { t } from 'svelte-i18n';
  import Select from '../Select.svelte';
  import type { SwrlFormat } from '../../lib/api';
  import './reasoning.css';

  export let format: SwrlFormat = 'swrlapi';
  export let rules = '';
  export let prefixText = '';
  export let rdfFormat = 'turtle';
  export let baseIri = '';
  export let disabled = false;
  /** Prefix for element ids, so two editors on one page do not clash. */
  export let idPrefix = 'swrl';

  const FORMATS: SwrlFormat[] = ['swrlapi', 'text', 'functional', 'xml', 'rdf', 'ruleml'];
  const RDF_FORMATS = ['turtle', 'ntriples', 'trig', 'rdfxml', 'jsonld', 'n3'];

  $: formatOptions = FORMATS.map((f) => ({ value: f, label: $t(`components.swrl.format.${f}`) }));
  const rdfOptions = RDF_FORMATS.map((f) => ({ value: f, label: f }));
  $: placeholder = $t(`components.swrl.placeholder.${format}`);
</script>

<div class="rz-grid">
  <div class="rz-field">
    <span class="rz-label">{$t('components.swrl.formatLabel')}</span>
    <Select bind:value={format} options={formatOptions} ariaLabel={$t('components.swrl.formatLabel')} {disabled} />
  </div>
  {#if format === 'rdf'}
    <div class="rz-field">
      <span class="rz-label">{$t('components.swrl.rdfFormat')}</span>
      <Select bind:value={rdfFormat} options={rdfOptions} ariaLabel={$t('components.swrl.rdfFormat')} {disabled} />
    </div>
    <div class="rz-field">
      <label for="{idPrefix}-base">{$t('components.swrl.baseIri')}</label>
      <input id="{idPrefix}-base" bind:value={baseIri} placeholder="https://example.org/" {disabled} />
    </div>
  {/if}
</div>
{#if format === 'swrlapi'}
  <div class="rz-field">
    <label for="{idPrefix}-prefixes">{$t('components.swrl.prefixes')}</label>
    <textarea id="{idPrefix}-prefixes" class="rz-code" style="min-height: 3.5rem" bind:value={prefixText} placeholder="ex: https://example.org/" {disabled}></textarea>
    <span class="rz-muted">{$t('components.swrl.prefixesHint')}</span>
  </div>
{/if}
<div class="rz-field">
  <label for="{idPrefix}-rules">{$t('components.swrl.rules')}</label>
  <textarea id="{idPrefix}-rules" class="rz-code" bind:value={rules} {placeholder} {disabled}></textarea>
</div>
