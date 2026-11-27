<script lang="ts">
  // Where a resource belongs, on its own page: the model that defines it, the
  // classes it is an instance of and their models, the dataset that holds it
  // and the model that dataset conforms to, and, for a class, where its
  // instances are. Every line is a link, so a term of a model leads to the
  // model and to the data, and an instance leads to its class and its model.
  //
  // The data is the answer of GET /api/browse/resource/context, which only
  // names what the caller may read. Nothing to show renders nothing.
  import { t } from 'svelte-i18n';
  import { BookOpen, Tag, Database, Boxes, ShieldCheck } from 'lucide-svelte';
  import { Link } from '../lib/router/index.js';
  import { shortenIRI } from '../lib/rdf-utils.js';

  type ModelRef = {
    model_id: string;
    title?: string | null;
    kind?: string | null;
    version: string;
    status?: string | null;
    other_versions?: number;
  };
  type DatasetRef = {
    id: string;
    name: string;
    conforms_to?: { model_id: string; version?: string | null; title?: string | null } | null;
  };
  type Context = {
    iri?: string;
    defined_in?: ModelRef[];
    instance_of?: { iri: string; defined_in: ModelRef[] }[];
    datasets?: DatasetRef[];
    conforming_datasets?: { id: string; name: string; version?: string | null }[];
    instances?: {
      total: number;
      elsewhere?: number;
      sources: { dataset?: DatasetRef; model?: ModelRef; count: number }[];
    };
  };

  let {
    context = null,
    iri = '',
    onOpenResource = (_iri: string) => {},
    onShowInstances = null,
  }: {
    context?: Context | null;
    /** The resource the page is about: a model link opens the model on it. */
    iri?: string;
    onOpenResource?: (_iri: string) => void;
    /** Shows the resources typed with this one, when the page can. */
    onShowInstances?: (() => void) | null;
  } = $props();

  let definedIn = $derived(context?.defined_in ?? []);
  let instanceOf = $derived(context?.instance_of ?? []);
  let datasets = $derived(context?.datasets ?? []);
  let conforming = $derived(context?.conforming_datasets ?? []);
  let instances = $derived(context?.instances ?? { total: 0, sources: [] });
  let anything = $derived(
    definedIn.length > 0 || instanceOf.length > 0 || datasets.length > 0 || conforming.length > 0 || instances.total > 0,
  );

  // The model viewer on the version, opened on a term when there is one.
  function modelHref(model: { model_id: string; version?: string | null }, term = ''): string {
    const base = model.version
      ? `/models/${encodeURIComponent(model.model_id)}/viewer/${encodeURIComponent(model.version)}`
      : `/models/${encodeURIComponent(model.model_id)}`;
    return term && model.version ? `${base}?term=${encodeURIComponent(term)}` : base;
  }
  const datasetHref = (id: string) => `/datasets/${encodeURIComponent(id)}`;
  const modelName = (m: { model_id: string; title?: string | null }) => m.title || m.model_id;
</script>

{#snippet model(m: ModelRef, term: string)}
  <Link to={modelHref(m, term)} class="rc-link" title={m.model_id}>
    <BookOpen size={12} /> {modelName(m)} <span class="rc-version">{m.version}</span>
  </Link>
  {#if m.status && m.status !== 'published'}<span class="rc-chip">{m.status}</span>{/if}
  {#if (m.other_versions ?? 0) > 0}
    <span class="rc-note">{$t('components.resourceContext.otherVersions', { values: { count: m.other_versions } })}</span>
  {/if}
{/snippet}

{#if anything}
  <section class="card rc" aria-labelledby="rc-title">
    <h3 id="rc-title"><ShieldCheck size={14} /> {$t('components.resourceContext.title')}</h3>
    <dl>
      {#if definedIn.length > 0}
        <dt>{$t('components.resourceContext.definedIn')}</dt>
        <dd>
          <ul>
            {#each definedIn as m (m.model_id)}
              <li>{@render model(m, iri)}</li>
            {/each}
          </ul>
        </dd>
      {/if}

      {#if instanceOf.length > 0}
        <dt>{$t('components.resourceContext.instanceOf')}</dt>
        <dd>
          <ul>
            {#each instanceOf as cls (cls.iri)}
              <li>
                <button type="button" class="rc-class" title={cls.iri} onclick={() => onOpenResource(cls.iri)}>
                  <Tag size={11} /> {shortenIRI(cls.iri)}
                </button>
                {#if cls.defined_in.length > 0}
                  <span class="rc-note">{$t('components.resourceContext.classDefinedIn')}</span>
                  {#each cls.defined_in as m (m.model_id)}{@render model(m, cls.iri)}{/each}
                {:else}
                  <span class="rc-note">{$t('components.resourceContext.classNotInAModel')}</span>
                {/if}
              </li>
            {/each}
          </ul>
        </dd>
      {/if}

      {#if datasets.length > 0}
        <dt>{$t('components.resourceContext.inDataset')}</dt>
        <dd>
          <ul>
            {#each datasets as ds (ds.id)}
              <li>
                <Link to={datasetHref(ds.id)} class="rc-link" title={ds.id}><Database size={12} /> {ds.name}</Link>
                {#if ds.conforms_to}
                  <span class="rc-note">{$t('components.resourceContext.conformsTo')}</span>
                  <Link to={modelHref(ds.conforms_to)} class="rc-link" title={ds.conforms_to.model_id}>
                    <BookOpen size={12} /> {modelName(ds.conforms_to)}
                    {#if ds.conforms_to.version}<span class="rc-version">{ds.conforms_to.version}</span>{/if}
                  </Link>
                {:else}
                  <span class="rc-note">{$t('components.resourceContext.noModelDeclared')}</span>
                {/if}
              </li>
            {/each}
          </ul>
        </dd>
      {/if}

      {#if instances.total > 0}
        <dt>{$t('components.resourceContext.instances')}</dt>
        <dd>
          <ul>
            {#each instances.sources as src (src.dataset?.id ?? `model:${src.model?.model_id}`)}
              <li>
                <span class="rc-count"><Boxes size={12} /> {src.count.toLocaleString()}</span>
                <span class="rc-note">{$t('components.resourceContext.instancesIn')}</span>
                {#if src.dataset}
                  <Link to={datasetHref(src.dataset.id)} class="rc-link" title={src.dataset.id}><Database size={12} /> {src.dataset.name}</Link>
                  {#if src.dataset.conforms_to}
                    <span class="rc-note">{$t('components.resourceContext.conformsTo')}</span>
                    <Link to={modelHref(src.dataset.conforms_to)} class="rc-link" title={src.dataset.conforms_to.model_id}>
                      <BookOpen size={12} /> {modelName(src.dataset.conforms_to)}
                      {#if src.dataset.conforms_to.version}<span class="rc-version">{src.dataset.conforms_to.version}</span>{/if}
                    </Link>
                  {/if}
                {:else if src.model}
                  {@render model(src.model, iri)}
                {/if}
              </li>
            {/each}
            {#if (instances.elsewhere ?? 0) > 0}
              <li>
                <span class="rc-count"><Boxes size={12} /> {(instances.elsewhere ?? 0).toLocaleString()}</span>
                <span class="rc-note">{$t('components.resourceContext.instancesElsewhere')}</span>
              </li>
            {/if}
          </ul>
          {#if onShowInstances}
            <button type="button" class="rc-show" onclick={() => onShowInstances?.()}>
              {$t('components.resourceContext.showInstances')}
            </button>
          {/if}
        </dd>
      {/if}

      {#if conforming.length > 0}
        <dt>{$t('components.resourceContext.conformingDatasets')}</dt>
        <dd>
          <ul>
            {#each conforming as ds (ds.id)}
              <li>
                <Link to={datasetHref(ds.id)} class="rc-link" title={ds.id}><Database size={12} /> {ds.name}</Link>
                {#if ds.version}<span class="rc-note">{$t('components.resourceContext.conformsToVersion', { values: { version: ds.version } })}</span>{/if}
              </li>
            {/each}
          </ul>
        </dd>
      {/if}
    </dl>
  </section>
{/if}

<style>
  .rc h3 { display: flex; align-items: center; gap: 0.4rem; margin: 0 0 0.6rem; font-size: 0.9rem; }
  dl { display: grid; grid-template-columns: minmax(8rem, max-content) 1fr; gap: 0.5rem 1rem; margin: 0; }
  dt { font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.06em; color: var(--ink-500); padding-top: 0.2rem; }
  dd { margin: 0; min-width: 0; }
  ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.3rem; }
  li { display: flex; flex-wrap: wrap; align-items: center; gap: 0.3rem 0.45rem; font-size: 0.85rem; }
  .rc :global(.rc-link) { display: inline-flex; align-items: center; gap: 0.3rem; color: var(--brand-600); text-decoration: none; font-weight: 500; }
  .rc :global(.rc-link:hover) { text-decoration: underline; }
  .rc-version { font-family: var(--font-mono); font-size: 0.75rem; color: var(--ink-500); font-weight: 400; }
  .rc-note { font-size: 0.78rem; color: var(--ink-500); }
  .rc-chip { font-size: 0.68rem; padding: 1px 6px; border-radius: 999px; border: 1px solid var(--line-strong); color: var(--ink-600); text-transform: lowercase; }
  .rc-class { display: inline-flex; align-items: center; gap: 0.28rem; padding: 2px 9px; border-radius: 999px; border: 1px solid var(--line-strong); background: var(--bg-soft); color: inherit; font-size: 0.78rem; cursor: pointer; }
  .rc-class:hover { border-color: var(--brand-500); color: var(--brand-600); }
  .rc-count { display: inline-flex; align-items: center; gap: 0.3rem; font-variant-numeric: tabular-nums; font-weight: 600; }
  .rc-show { margin-top: 0.4rem; padding: 0; border: 0; background: none; color: var(--brand-600); font-size: 0.8rem; cursor: pointer; }
  .rc-show:hover { text-decoration: underline; }
  .rc-class:focus-visible, .rc-show:focus-visible { outline: 2px solid var(--brand-500); outline-offset: 2px; }
  @media (max-width: 40rem) {
    dl { grid-template-columns: 1fr; gap: 0.15rem 0; }
    dd { margin-bottom: 0.5rem; }
  }
</style>
