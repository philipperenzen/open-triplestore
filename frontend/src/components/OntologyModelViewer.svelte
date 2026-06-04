<script lang="ts">
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import {
    Layers, Shapes, Puzzle, AlertTriangle, Network, RefreshCw, Tag, Globe2, GitBranch
  } from 'lucide-svelte';
  import { navigate } from '../lib/router/index.js';
  import { shortenIRI } from '../lib/rdf-utils';
  import { extractSchema } from '../lib/ontology/schema-model';
  import type { SchemaModel } from '../lib/ontology/schema-model';
  import { loadOntologyGraph } from '../lib/ontology/loader';
  import { validateSemantics } from '../lib/ontology/semanticValidator';
  import { applyFilter, emptyFilter } from '../lib/ontology/filters';
  import type { FilterState } from '../lib/ontology/filters';
  import FilterBar from './ontology/FilterBar.svelte';
  import ClassAxiomPanel from './ontology/ClassAxiomPanel.svelte';
  import PropertyAxiomPanel from './ontology/PropertyAxiomPanel.svelte';
  import SkosPanel from './ontology/SkosPanel.svelte';
  import NamespacePanel from './ontology/NamespacePanel.svelte';
  import DiagnosticsPanel from './ontology/DiagnosticsPanel.svelte';

  /** Primary named graph for this ontology version. */
  export let graphIri: string = '';
  /** Optional extra sub-graphs to merge into the query scope. */
  export let subGraphs: string[] = [];
  /** Optional pre-loaded N3 Store — skips SPARQL fetch when provided. */
  export let preloadedStore: any = null;

  type Tab = 'classes' | 'properties' | 'axioms' | 'shapes' | 'skos' | 'namespaces' | 'diagnostics';
  export let initialTab: Tab = 'classes';
  let activeTab: Tab = initialTab;
  $: if (initialTab) activeTab = initialTab;

  let loading = false;
  let error = '';
  let model: SchemaModel | null = null;
  // The parsed N3 store backing `model` — kept so we can run full semantic
  // validation (validateSemantics) over the raw triples, not just the view.
  let store: any = null;
  let filter: FilterState = emptyFilter();

  $: scopeGraphs = [graphIri, ...(subGraphs || [])].filter(Boolean);
  $: view = model ? applyFilter(model, filter) : null;

  async function load() {
    if (!scopeGraphs.length && !preloadedStore) return;
    loading = true;
    error = '';
    try {
      if (preloadedStore) {
        store = preloadedStore;
        model = extractSchema(store);
      } else {
        const loaded = await loadOntologyGraph(scopeGraphs);
        store = loaded.store;
        model = extractSchema(store);
      }
    } catch (e: any) {
      error = e?.message || $t('components.ontologyModelViewer.loadError');
    } finally {
      loading = false;
    }
  }

  onMount(load);

  // Re-run when preloadedStore arrives (async from parent)
  $: if (preloadedStore && !model) load();

  function viewResource(iri: string) {
    const qs = new URLSearchParams({ iri });
    if (graphIri) qs.set('graph', graphIri);
    navigate(`/resource?${qs.toString()}`);
  }
  function viewInGraph() {
    navigate(`/browse?view=graph&graph=${encodeURIComponent(graphIri)}`);
  }

  // Diagnostics — full semantic validation over the loaded ontology store:
  // subclass cycles, unknown domain/range classes, property-kind conflicts,
  // SHACL path/target/min-max/datatype conflicts, missing labels, orphan
  // classes, literal-where-IRI-expected, … (see lib/ontology/semanticValidator).
  $: diagnostics = store ? validateSemantics(store) : [];
  $: issueCount = diagnostics.filter(d => d.severity === 'error' || d.severity === 'warning').length;

  $: shapesArr = model ? [...model.shapes.values()] : [];
</script>

<div class="omv">
  <div class="omv-toolbar">
    <div class="omv-tabs">
      <button class="omv-tab" class:active={activeTab === 'classes'} on:click={() => (activeTab = 'classes')}>
        <Layers size={13} /> {$t('components.ontologyModelViewer.tabClasses')} <span class="count">{view?.classes.length ?? 0}</span>
      </button>
      <button class="omv-tab" class:active={activeTab === 'properties'} on:click={() => (activeTab = 'properties')}>
        <Puzzle size={13} /> {$t('components.ontologyModelViewer.tabProperties')} <span class="count">{view?.properties.length ?? 0}</span>
      </button>
      <button class="omv-tab" class:active={activeTab === 'axioms'} on:click={() => (activeTab = 'axioms')}>
        <GitBranch size={13} /> {$t('components.ontologyModelViewer.tabAxioms')}
      </button>
      <button class="omv-tab" class:active={activeTab === 'shapes'} on:click={() => (activeTab = 'shapes')}>
        <Shapes size={13} /> SHACL <span class="count">{shapesArr.length}</span>
      </button>
      <button class="omv-tab" class:active={activeTab === 'skos'} on:click={() => (activeTab = 'skos')}>
        <Tag size={13} /> SKOS <span class="count">{view?.concepts.length ?? 0}</span>
      </button>
      <button class="omv-tab" class:active={activeTab === 'namespaces'} on:click={() => (activeTab = 'namespaces')}>
        <Globe2 size={13} /> {$t('components.ontologyModelViewer.tabNamespaces')} <span class="count">{model ? model.namespaces.size : 0}</span>
      </button>
      <button class="omv-tab" class:active={activeTab === 'diagnostics'} on:click={() => (activeTab = 'diagnostics')}>
        <AlertTriangle size={13} /> {$t('components.ontologyModelViewer.tabDiagnostics')}
        {#if issueCount > 0}<span class="count warn">{issueCount}</span>{/if}
      </button>
    </div>
    <div class="omv-actions">
      <button class="btn btn-sm btn-ghost" on:click={viewInGraph} title={$t('components.ontologyModelViewer.visualizeTitle')}>
        <Network size={12} /> {$t('components.ontologyModelViewer.visualize')}
      </button>
      <button class="btn btn-sm btn-ghost" on:click={load} disabled={loading} title={$t('components.ontologyModelViewer.reload')}>
        <RefreshCw size={12} /> {$t('components.ontologyModelViewer.reload')}
      </button>
    </div>
  </div>

  <FilterBar state={filter} onChange={(s) => (filter = s)} />

  {#if loading}
    <div class="omv-loading">{$t('components.ontologyModelViewer.loadingModel')}</div>
  {:else if error}
    <div class="omv-error">{error}</div>
  {:else if !model || !view}
    <div class="omv-empty">{$t('components.ontologyModelViewer.noGraphLoaded')}</div>
  {:else}
    {#if activeTab === 'classes'}
      <ClassAxiomPanel
        classes={view.classes}
        onOpen={viewResource}
      />
    {:else if activeTab === 'properties' || activeTab === 'axioms'}
      <PropertyAxiomPanel properties={view.properties} onOpen={viewResource} />
      {#if activeTab === 'axioms'}
        <div class="ax-classes">
          <h3 class="ax-h">{$t('components.ontologyModelViewer.classAxioms')}</h3>
          <ClassAxiomPanel classes={view.classes} onOpen={viewResource} />
        </div>
      {/if}
    {:else if activeTab === 'shapes'}
      {#if shapesArr.length === 0}
        <div class="omv-empty">{$t('components.ontologyModelViewer.noNodeShapes')}</div>
      {:else}
        <div class="shapes-list">
          {#each shapesArr as s}
            <div class="shape-card">
              <div class="shape-head">
                <button class="linky" on:click={() => viewResource(s.iri)} title={s.iri}>
                  <strong>{shortenIRI(s.iri)}</strong>
                </button>
                <div class="shape-targets">
                  {#each s.targetClass as tc}<span class="target-chip">targetClass: {shortenIRI(tc)}</span>{/each}
                  {#each s.targetNode as tn}<span class="target-chip">targetNode: {shortenIRI(tn)}</span>{/each}
                </div>
              </div>
              {#if s.properties.length > 0}
                <table class="omv-table sub">
                  <thead><tr><th>{$t('components.ontologyModelViewer.thPath')}</th><th>{$t('components.ontologyModelViewer.thCardinality')}</th><th>{$t('components.ontologyModelViewer.thDatatypeClass')}</th><th>{$t('components.ontologyModelViewer.thPattern')}</th></tr></thead>
                  <tbody>
                    {#each s.properties as pp}
                      <tr>
                        <td><code>{shortenIRI(pp.path)}</code>{#if pp.name}<span class="muted small"> — {pp.name}</span>{/if}</td>
                        <td class="mono small">{pp.minCount ?? '0'}…{pp.maxCount ?? '*'}</td>
                        <td>
                          {#if pp.datatype}<span class="chip">{shortenIRI(pp.datatype)}</span>{/if}
                          {#if pp.cls}<span class="chip">{shortenIRI(pp.cls)}</span>{/if}
                          {#if !pp.datatype && !pp.cls}<span class="muted small">—</span>{/if}
                        </td>
                        <td class="mono small">{pp.pattern || ''}</td>
                      </tr>
                    {/each}
                  </tbody>
                </table>
              {:else}
                <div class="muted small" style="padding:6px 10px">{$t('components.ontologyModelViewer.noPropertyDeclarations')}</div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    {:else if activeTab === 'skos'}
      <SkosPanel concepts={view.concepts} {model} onOpen={viewResource} />
    {:else if activeTab === 'namespaces'}
      <NamespacePanel {model} onOpen={viewResource} />
    {:else if activeTab === 'diagnostics'}
      <DiagnosticsPanel issues={diagnostics} on:navigate={(e) => viewResource(e.detail.iri)} />
    {/if}
  {/if}
</div>

<style>
  .omv { display: flex; flex-direction: column; gap: 0.55rem; }
  .omv-toolbar { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 0.5rem; }
  .omv-tabs { display: flex; gap: 0.25rem; padding: 0.3rem; border-radius: 12px; background: #f8fafc; border: 1px solid #e2e8f0; flex-wrap: wrap; }
  .omv-tab { display: inline-flex; align-items: center; gap: 0.35rem; padding: 0.35rem 0.7rem; border: none; background: transparent; border-radius: 8px; font-size: 0.8rem; color: #475569; cursor: pointer; }
  .omv-tab:hover { background: #e2e8f0; }
  .omv-tab.active { background: #1565c0; color: #fff; }
  .omv-tab .count { background: rgba(255,255,255,0.3); border-radius: 999px; padding: 0 0.4rem; font-size: 0.68rem; font-weight: 700; }
  .omv-tab:not(.active) .count { background: #e2e8f0; color: #475569; }
  .omv-tab .count.warn { background: #f59e0b; color: #fff; }
  .omv-actions { display: flex; gap: 0.35rem; }

  .omv-loading, .omv-empty { padding: 1rem; color: #64748b; font-size: 0.85rem; text-align: center; }
  .omv-error { padding: 0.75rem; color: #b91c1c; background: #fef2f2; border: 1px solid #fecaca; border-radius: 10px; font-size: 0.85rem; }

  .ax-classes { margin-top: 0.6rem; }
  .ax-h { font-size: 0.85rem; color: #475569; font-weight: 700; margin: 0.4rem 0 0.3rem; }

  .shapes-list { display: flex; flex-direction: column; gap: 0.75rem; }
  .shape-card { border: 1px solid #e2e8f0; border-radius: 12px; overflow: hidden; }
  .shape-head { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; padding: 0.55rem 0.75rem; background: #f8fafc; border-bottom: 1px solid #e2e8f0; flex-wrap: wrap; }
  .shape-targets { display: flex; gap: 0.3rem; flex-wrap: wrap; }
  .target-chip { font-size: 0.72rem; padding: 1px 8px; border-radius: 999px; background: #ede9fe; color: #6d28d9; border: 1px solid #ddd6fe; }
  .omv-table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
  .omv-table th { text-align: left; background: #f8fafc; padding: 0.45rem 0.65rem; border-bottom: 1px solid #e2e8f0; font-weight: 600; color: #475569; }
  .omv-table td { padding: 0.4rem 0.65rem; border-bottom: 1px solid #f1f5f9; vertical-align: top; }
  .omv-table.sub th { background: #fbfbfe; }
  .linky { background: none; border: none; cursor: pointer; color: #2563eb; padding: 0; text-align: left; }
  .linky:hover { text-decoration: underline; }
  .chip { display: inline-block; margin: 1px 2px 1px 0; padding: 1px 7px; border-radius: 999px; background: #eef5ff; color: #1565c0; border: 1px solid #bbdefb; font-size: 0.72rem; cursor: pointer; }


  .muted { color: #94a3b8; }
  .small { font-size: 0.75rem; }
  .mono { font-family: monospace; }

  :global(:is([data-theme="dark"], .dark)) .omv-tabs { background: var(--bg-soft); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .omv-tab:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .omv-tab:not(.active) .count { background: rgba(255,255,255,0.1); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .omv-error { color: #fca5a5; background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .shape-card { border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .shape-head { background: var(--bg-soft); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .target-chip { background: rgba(139,92,246,0.2); color: #c4b5fd; border-color: rgba(139,92,246,0.35); }
  :global(:is([data-theme="dark"], .dark)) .omv-table th { background: var(--bg-soft); border-color: var(--line-strong); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .omv-table td { border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .omv-table.sub th { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .linky { color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip { background: rgba(59,130,246,0.18); color: #93c5fd; border-color: rgba(59,130,246,0.3); }
</style>
