<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { navigate } from '../lib/router/index.js';
  import { ArrowLeft, Loader2 } from 'lucide-svelte';
  import { getVocabulary, listVocabularyVersions, getVocabularyVersionDataUrl } from '../lib/api.js';
  import OntologyBrowserPanel from '../components/OntologyBrowserPanel.svelte';
  import ContentKindWarning from '../components/ContentKindWarning.svelte';
  import Select from '../components/Select.svelte';

  export let id;
  export let versionId = '';

  let vocabulary = null;
  let versions = [];
  let selectedVersion = null;
  let loading = true;
  let error = '';

  onMount(async () => {
    try {
      [vocabulary, versions] = await Promise.all([getVocabulary(id), listVocabularyVersions(id)]);
      selectedVersion = versionId
        ? versions.find(v => v.version === versionId) || versions[0]
        : (versions.find(v => v.status === 'published') || versions[0]);
      if (!selectedVersion) {
        error = $t('pages.vocabularyViewer.noVersions');
      }
    } catch (e) {
      error = e?.message || $t('pages.vocabularyViewer.loadFailed');
    }
    loading = false;
  });

  function selectVersion(ver) {
    selectedVersion = ver;
    navigate(`/vocabularies/${id}/viewer/${ver.version}`);
  }
</script>

<div class="space-y-4">
  <div class="flex items-center gap-3 flex-wrap">
    <button class="btn btn-ghost btn-sm" on:click={() => navigate(`/vocabularies/${id}`)}>
      <ArrowLeft size={16} /> {$t('pages.vocabularyViewer.backToVocabulary')}
    </button>
    {#if vocabulary}
      <h2 class="text-xl font-semibold m-0">{vocabulary.title}</h2>
    {/if}
    {#if versions.length > 1}
      <div class="ml-auto flex items-center gap-2">
        <label class="text-sm text-[var(--ink-400)]" for="ver-select">{$t('pages.vocabularyViewer.version')}</label>
        <Select
          id="ver-select"
          value={selectedVersion?.version || ''}
          options={versions.map(ver => ({ value: ver.version, label: `v${ver.version} (${ver.status})` }))}
          on:change={e => {
            const v = versions.find(x => x.version === e.detail);
            if (v) selectVersion(v);
          }}
        />
      </div>
    {/if}
  </div>

  {#if loading}
    <div class="flex items-center justify-center py-16 text-[var(--ink-400)]">
      <Loader2 size={24} class="animate-spin mr-2" /> {$t('system.loading')}
    </div>
  {:else if error}
    <div class="p-4 rounded-xl bg-red-50 border border-red-200 text-red-700 text-sm">{error}</div>
  {:else if selectedVersion}
    <ContentKindWarning
      graphs={[selectedVersion.graph_iri, ...(selectedVersion.sub_graphs || [])]}
      expected="vocabulary"
      contextName={vocabulary?.title}
    />
    <OntologyBrowserPanel
      graphIri={selectedVersion.graph_iri}
      subGraphs={selectedVersion.sub_graphs || []}
      versionLabel={selectedVersion.version}
      rawDataUrl={getVocabularyVersionDataUrl(id, selectedVersion.version, 'turtle', 'all')}
    />
  {/if}
</div>

<style>
  .btn { display: inline-flex; align-items: center; gap: 0.375rem; padding: 0.5rem 1rem; border-radius: 0.75rem; font-size: 0.875rem; font-weight: 500; cursor: pointer; border: none; transition: all 0.15s; background: transparent; color: var(--ink-600, #475569); }
  .btn-ghost:hover { background: var(--bg-soft, #f1f5f9); }
  .btn-sm { padding: 0.375rem 0.75rem; font-size: 0.8125rem; }
</style>
