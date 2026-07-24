<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Link } from '../lib/router/index.js';
  import {
    Search, Loader2, BookOpen, Sparkles, Tag, Download, Check, ExternalLink,
    Library, ChevronRight, Info, X,
  } from 'lucide-svelte';
  import {
    searchVocabularies, searchVocabTerms, suggestVocabTerms, recommendVocabularies,
    installVocabulary, vocabStatus, vocabTags,
  } from '../lib/api.ts';
  import { isAdmin } from '../lib/stores.js';
  import { safeExternalUrl } from '../lib/safeUrl.ts';
  import { toastSuccess, toastError } from '../lib/toast.ts';

  // ── Tabs ──────────────────────────────────────────────────────────────────
  let tab = 'terms'; // 'terms' | 'vocabs' | 'recommend'

  // ── Shared status ─────────────────────────────────────────────────────────
  let status = null;
  onMount(async () => {
    const params = new URLSearchParams(window.location.search);
    const deepTab = params.get('tab');
    if (deepTab === 'vocabs' || deepTab === 'recommend') tab = deepTab;
    const q = params.get('q');
    if (q) {
      termQuery = q;
      vocabQuery = q;
    }
    try { status = await vocabStatus(); } catch {}
    try { allTags = ((await vocabTags()) || []).slice(0, 30); } catch {}
    runTermSearch(); // with a deep-linked q, or the initial popularity browse
    runVocabSearch();
  });

  // ── Term search ───────────────────────────────────────────────────────────
  let termQuery = '';
  let termTypes = new Set(['class', 'property']);
  let termSource = '';
  let termVocab = '';
  let termLoading = false;
  let termError = '';
  let termResult = null;
  let suggestions = [];
  let termPage = 1;
  let termTimer;
  let termSeq = 0;

  const TYPE_OPTIONS = ['class', 'property', 'datatype', 'instance'];

  function toggleType(tp) {
    if (termTypes.has(tp)) termTypes.delete(tp);
    else termTypes.add(tp);
    termTypes = termTypes;
    termPage = 1;
    runTermSearch();
  }

  function onTermInput() {
    clearTimeout(termTimer);
    termPage = 1;
    termTimer = setTimeout(runTermSearch, 250);
  }

  async function runTermSearch() {
    const seq = ++termSeq;
    termLoading = true;
    termError = '';
    suggestions = [];
    // Empty selection means "no type restriction" — send the full list so
    // the backend doesn't fall back to its class,property default.
    const types = termTypes.size ? [...termTypes].join(',') : TYPE_OPTIONS.join(',');
    try {
      const result = await searchVocabTerms(termQuery, {
        types,
        vocab: termVocab || undefined,
        source: termSource || undefined,
        page: termPage,
        pageSize: 15,
      });
      if (seq !== termSeq) return; // a newer search superseded this one
      termResult = result;
      if (termQuery && termResult && termResult.total_results === 0) {
        try {
          const s = await suggestVocabTerms(termQuery);
          if (seq === termSeq) suggestions = s?.suggestions || [];
        } catch {}
      }
    } catch (e) {
      if (seq !== termSeq) return;
      termError = e.message;
      termResult = null;
    }
    if (seq === termSeq) termLoading = false;
  }

  function pickVocabFacet(prefix) {
    termVocab = termVocab === prefix ? '' : prefix;
    termPage = 1;
    runTermSearch();
  }

  function firstLine(s) {
    return (s || '').split('\n')[0];
  }

  // ── Vocabulary search ─────────────────────────────────────────────────────
  let vocabQuery = '';
  let vocabTag = '';
  let vocabLoading = false;
  let vocabError = '';
  let vocabResult = null;
  let vocabPage = 1;
  let vocabTimer;
  let vocabSeq = 0;
  let allTags = [];
  let installing = new Set();

  function onVocabInput() {
    clearTimeout(vocabTimer);
    vocabPage = 1;
    vocabTimer = setTimeout(runVocabSearch, 250);
  }

  async function runVocabSearch() {
    const seq = ++vocabSeq;
    vocabLoading = true;
    vocabError = '';
    try {
      const result = await searchVocabularies(vocabQuery, {
        tag: vocabTag || undefined,
        page: vocabPage,
        pageSize: 12,
      });
      if (seq !== vocabSeq) return;
      vocabResult = result;
    } catch (e) {
      if (seq !== vocabSeq) return;
      vocabError = e.message;
      vocabResult = null;
    }
    if (seq === vocabSeq) vocabLoading = false;
  }

  function pickTag(tag) {
    vocabTag = vocabTag === tag ? '' : tag;
    vocabPage = 1;
    runVocabSearch();
  }

  function title(entry) {
    const en = entry.titles?.find((x) => x.lang === 'en');
    return (en || entry.titles?.[0])?.value || entry.prefix;
  }

  function description(entry) {
    const en = entry.descriptions?.find((x) => x.lang === 'en');
    const d = (en || entry.descriptions?.[0])?.value || '';
    return d.length > 220 ? d.slice(0, 220) + '…' : d;
  }

  async function install(entry) {
    installing.add(entry.prefix); installing = installing;
    try {
      const out = await installVocabulary(entry.prefix);
      toastSuccess($t('pages.vocabularySearch.installed', { values: { id: out.model_id, version: out.version } }));
      await runVocabSearch();
      try { status = await vocabStatus(); } catch {}
    } catch (e) {
      toastError(e.message);
    }
    installing.delete(entry.prefix); installing = installing;
  }

  // ── Recommender ───────────────────────────────────────────────────────────
  let recInput = '';
  let recCategory = 'all';
  let recLoading = false;
  let recError = '';
  let recResult = null;

  async function runRecommend() {
    const terms = recInput
      .split(/[\n,;]+/)
      .map((s) => s.trim())
      .filter(Boolean)
      .slice(0, 50)
      .map((term) => ({ term, category: recCategory }));
    if (!terms.length) return;
    recLoading = true;
    recError = '';
    try {
      recResult = await recommendVocabularies({ terms });
    } catch (e) {
      recError = e.message;
      recResult = null;
    }
    recLoading = false;
  }
</script>

<div class="vs-page">
  <header class="vs-header">
    <div class="vs-title">
      <Library size={22} />
      <div>
        <h1>{$t('pages.vocabularySearch.title')}</h1>
        <p class="vs-sub">{$t('pages.vocabularySearch.subtitle')}</p>
      </div>
    </div>
    {#if status}
      <div class="vs-status" title={$t('pages.vocabularySearch.statusTitle')}>
        <span>{$t('pages.vocabularySearch.statVocabs', { values: { n: status.catalog_vocabularies } })}</span>
        <span>{$t('pages.vocabularySearch.statPrefixes', { values: { n: status.prefix_dataset_size } })}</span>
        {#if status.engine}
          <span>{$t('pages.vocabularySearch.statTerms', { values: { n: (status.engine.lov_terms || 0) + (status.engine.platform_terms || 0) } })}</span>
          {#if !status.engine.lov_index_ready && status.corpus_available}
            <span class="vs-building"><Loader2 size={12} class="vs-spin" /> {$t('pages.vocabularySearch.indexBuilding')}</span>
          {/if}
        {/if}
      </div>
    {/if}
  </header>

  <nav class="vs-tabs">
    <button class:active={tab === 'terms'} on:click={() => (tab = 'terms')}>
      <Search size={14} /> {$t('pages.vocabularySearch.tabTerms')}
    </button>
    <button class:active={tab === 'vocabs'} on:click={() => (tab = 'vocabs')}>
      <BookOpen size={14} /> {$t('pages.vocabularySearch.tabVocabs')}
    </button>
    <button class:active={tab === 'recommend'} on:click={() => (tab = 'recommend')}>
      <Sparkles size={14} /> {$t('pages.vocabularySearch.tabRecommend')}
    </button>
  </nav>

  {#if tab === 'terms'}
    <section class="vs-panel">
      <div class="vs-searchbar">
        <Search size={16} class="vs-search-icon" />
        <input
          type="text"
          placeholder={$t('pages.vocabularySearch.termPlaceholder')}
          bind:value={termQuery}
          on:input={onTermInput}
          aria-label={$t('pages.vocabularySearch.tabTerms')}
        />
        {#if termLoading}<Loader2 size={16} class="vs-spin" />{/if}
      </div>

      <div class="vs-filters">
        {#each TYPE_OPTIONS as tp}
          <button class="vs-chip" class:on={termTypes.has(tp)} on:click={() => toggleType(tp)}>
            {$t(`pages.vocabularySearch.type_${tp}`)}
          </button>
        {/each}
        <span class="vs-filter-sep"></span>
        <button class="vs-chip" class:on={termSource === ''} on:click={() => { termSource = ''; termPage = 1; runTermSearch(); }}>
          {$t('pages.vocabularySearch.sourceAll')}
        </button>
        <button class="vs-chip" class:on={termSource === 'platform'} on:click={() => { termSource = 'platform'; termPage = 1; runTermSearch(); }}>
          {$t('pages.vocabularySearch.sourcePlatform')}
        </button>
        <button class="vs-chip" class:on={termSource === 'lov'} on:click={() => { termSource = 'lov'; termPage = 1; runTermSearch(); }}>
          LOV
        </button>
        {#if termVocab}
          <button class="vs-chip on" on:click={() => pickVocabFacet(termVocab)}>
            {termVocab} <X size={12} />
          </button>
        {/if}
      </div>

      {#if termError}
        <p class="vs-error">{termError}</p>
      {:else if termResult}
        <div class="vs-columns">
          <div class="vs-results">
            <p class="vs-count">
              {$t('pages.vocabularySearch.termCount', { values: { n: termResult.total_results } })}
              {#if termResult.lov_index_ready === false}
                <span class="vs-building">({$t('pages.vocabularySearch.indexBuilding')})</span>
              {/if}
            </p>
            {#if suggestions.length}
              <p class="vs-suggest">
                {$t('pages.vocabularySearch.didYouMean')}
                {#each suggestions as s}
                  <button class="vs-suggest-btn" on:click={() => { termQuery = s.text; runTermSearch(); }}>{s.text}</button>
                {/each}
              </p>
            {/if}
            {#each termResult.results || [] as hit (hit.iri)}
              <article class="vs-hit">
                <div class="vs-hit-head">
                  <code class="vs-prefixed">{hit.prefixed}</code>
                  <span class="vs-badge vs-type-{hit.ttype}">{$t(`pages.vocabularySearch.type_${hit.ttype}`)}</span>
                  <span class="vs-badge vs-src-{hit.source}">{hit.source === 'platform' ? $t('pages.vocabularySearch.sourcePlatform') : 'LOV'}</span>
                  <span class="vs-score" title={$t('pages.vocabularySearch.scoreTitle')}>
                    <span class="vs-score-bar" style={`width:${Math.round(Math.min(1, hit.score) * 100)}%`}></span>
                  </span>
                </div>
                {#if firstLine(hit.labels)}
                  <div class="vs-hit-label">{firstLine(hit.labels)}</div>
                {/if}
                {#if firstLine(hit.secondary)}
                  <p class="vs-hit-desc">{firstLine(hit.secondary)}</p>
                {/if}
                <div class="vs-hit-foot">
                  {#if safeExternalUrl(hit.iri)}
                    <a class="vs-iri" href={safeExternalUrl(hit.iri)} target="_blank" rel="noopener noreferrer">{hit.iri} <ExternalLink size={11} /></a>
                  {:else}
                    <code class="vs-iri">{hit.iri}</code>
                  {/if}
                  {#if hit.model_id}
                    <Link to={`/models/${hit.model_id}`} class="vs-model-link">
                      {$t('pages.vocabularySearch.openInRegistry')} <ChevronRight size={12} />
                    </Link>
                  {/if}
                </div>
              </article>
            {/each}
            {#if termResult.total_results > 15}
              <div class="vs-pager">
                <button disabled={termPage <= 1} on:click={() => { termPage -= 1; runTermSearch(); }}>‹</button>
                <span>{termPage}</span>
                <button disabled={termPage * 15 >= termResult.total_results} on:click={() => { termPage += 1; runTermSearch(); }}>›</button>
              </div>
            {/if}
          </div>
          <aside class="vs-facets">
            {#if termResult.aggregations?.vocabs?.length}
              <h3>{$t('pages.vocabularySearch.facetVocabs')}</h3>
              <ul>
                {#each termResult.aggregations.vocabs.slice(0, 12) as [prefix, count]}
                  <li>
                    <button class:on={termVocab === prefix} on:click={() => pickVocabFacet(prefix)}>
                      <span class="vs-facet-name">{prefix}</span>
                      <span class="vs-facet-count">{count}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
            {#if termResult.aggregations?.types?.length}
              <h3>{$t('pages.vocabularySearch.facetTypes')}</h3>
              <ul>
                {#each termResult.aggregations.types as [tp, count]}
                  <li><span class="vs-facet-name">{$t(`pages.vocabularySearch.type_${tp}`)}</span><span class="vs-facet-count">{count}</span></li>
                {/each}
              </ul>
            {/if}
          </aside>
        </div>
      {:else if termLoading}
        <p class="vs-loading"><Loader2 size={18} class="vs-spin" /></p>
      {/if}
    </section>
  {:else if tab === 'vocabs'}
    <section class="vs-panel">
      <div class="vs-searchbar">
        <Search size={16} class="vs-search-icon" />
        <input
          type="text"
          placeholder={$t('pages.vocabularySearch.vocabPlaceholder')}
          bind:value={vocabQuery}
          on:input={onVocabInput}
          aria-label={$t('pages.vocabularySearch.tabVocabs')}
        />
        {#if vocabLoading}<Loader2 size={16} class="vs-spin" />{/if}
      </div>

      {#if allTags.length}
        <div class="vs-filters vs-tagcloud">
          <Tag size={13} />
          {#each allTags.slice(0, 18) as tc}
            <button class="vs-chip" class:on={vocabTag === tc.tag} on:click={() => pickTag(tc.tag)}>
              {tc.tag}
            </button>
          {/each}
        </div>
      {/if}

      {#if vocabError}
        <p class="vs-error">{vocabError}</p>
      {:else if vocabResult}
        <p class="vs-count">{$t('pages.vocabularySearch.vocabCount', { values: { n: vocabResult.total_results } })}</p>
        <div class="vs-cards">
          {#each vocabResult.results || [] as entry (entry.prefix + entry.nsp)}
            <article class="vs-card">
              <div class="vs-card-head">
                <code class="vs-prefixed">{entry.prefix}</code>
                {#if entry.model_id}
                  <span class="vs-badge vs-src-platform"><Check size={11} /> {$t('pages.vocabularySearch.installedBadge')}</span>
                {:else}
                  <span class="vs-badge vs-src-lov">LOV</span>
                {/if}
              </div>
              <h4>{title(entry)}</h4>
              {#if description(entry)}<p class="vs-card-desc">{description(entry)}</p>{/if}
              <div class="vs-card-tags">
                {#each (entry.tags || []).slice(0, 4) as tg}
                  <span class="vs-tag">{tg}</span>
                {/each}
              </div>
              <div class="vs-card-metrics">
                {#if entry.metrics?.incoming_links}
                  <span title={$t('pages.vocabularySearch.metricIncoming')}>↧ {entry.metrics.incoming_links}</span>
                {/if}
                {#if entry.metrics?.reused_by_datasets}
                  <span title={$t('pages.vocabularySearch.metricReuse')}>◈ {entry.metrics.reused_by_datasets}</span>
                {/if}
                {#if entry.versions?.length}
                  <span>{$t('pages.vocabularySearch.versionCount', { values: { n: entry.versions.length } })}</span>
                {/if}
              </div>
              <div class="vs-card-actions">
                <code class="vs-nsp" title={entry.nsp}>{entry.nsp}</code>
                {#if entry.model_id}
                  <Link to={`/models/${entry.model_id}`} class="vs-model-link">
                    {$t('pages.vocabularySearch.openInRegistry')} <ChevronRight size={12} />
                  </Link>
                {:else if entry.installable && $isAdmin}
                  <button class="vs-install" disabled={installing.has(entry.prefix)} on:click={() => install(entry)}>
                    {#if installing.has(entry.prefix)}<Loader2 size={13} class="vs-spin" />{:else}<Download size={13} />{/if}
                    {$t('pages.vocabularySearch.install')}
                  </button>
                {/if}
              </div>
            </article>
          {/each}
        </div>
        {#if vocabResult.total_results > 12}
          <div class="vs-pager">
            <button disabled={vocabPage <= 1} on:click={() => { vocabPage -= 1; runVocabSearch(); }}>‹</button>
            <span>{vocabPage}</span>
            <button disabled={vocabPage * 12 >= vocabResult.total_results} on:click={() => { vocabPage += 1; runVocabSearch(); }}>›</button>
          </div>
        {/if}
      {:else if vocabLoading}
        <p class="vs-loading"><Loader2 size={18} class="vs-spin" /></p>
      {/if}
    </section>
  {:else}
    <section class="vs-panel">
      <p class="vs-rec-intro"><Info size={14} /> {$t('pages.vocabularySearch.recommendIntro')}</p>
      <textarea
        class="vs-rec-input"
        rows="4"
        placeholder={$t('pages.vocabularySearch.recommendPlaceholder')}
        bind:value={recInput}
      ></textarea>
      <div class="vs-rec-controls">
        <select bind:value={recCategory}>
          <option value="all">{$t('pages.vocabularySearch.categoryAll')}</option>
          <option value="class">{$t('pages.vocabularySearch.type_class')}</option>
          <option value="property">{$t('pages.vocabularySearch.type_property')}</option>
        </select>
        <button class="vs-rec-run" disabled={recLoading || !recInput.trim()} on:click={runRecommend}>
          {#if recLoading}<Loader2 size={14} class="vs-spin" />{:else}<Sparkles size={14} />{/if}
          {$t('pages.vocabularySearch.recommendRun')}
        </button>
      </div>
      {#if recError}<p class="vs-error">{recError}</p>{/if}
      {#if recResult}
        <div class="vs-rec-result">
          <h3>{$t('pages.vocabularySearch.recommendSet')}</h3>
          <div class="vs-rec-vocabs">
            {#each recResult.homogeneous_vocabs as v}
              <button class="vs-chip on" on:click={() => { tab = 'vocabs'; vocabQuery = v; runVocabSearch(); }}>{v}</button>
            {/each}
          </div>
          <table class="vs-rec-table">
            <thead>
              <tr>
                <th>{$t('pages.vocabularySearch.recommendTerm')}</th>
                <th>{$t('pages.vocabularySearch.recommendBest')}</th>
                <th>{$t('pages.vocabularySearch.recommendAlternatives')}</th>
              </tr>
            </thead>
            <tbody>
              {#each recResult.terms as tr, i (i)}
                <tr>
                  <td class="vs-rec-term">{tr.search_term}</td>
                  <td>
                    {#if tr.homogeneous_best}
                      <code class="vs-prefixed">{tr.homogeneous_best.prefixed}</code>
                      {#if tr.homogeneous_best.label}<span class="vs-rec-label">{tr.homogeneous_best.label}</span>{/if}
                    {:else}
                      <span class="vs-rec-none">{$t('pages.vocabularySearch.recommendNone')}</span>
                    {/if}
                  </td>
                  <td class="vs-rec-alt">
                    {#each tr.results.slice(0, 3) as r (r.iri)}
                      <code>{r.prefixed}</code>
                    {/each}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </section>
  {/if}

  <footer class="vs-attribution">
    {$t('pages.vocabularySearch.attribution')}
    <a href="https://lov.linkeddata.es/" target="_blank" rel="noopener noreferrer">Linked Open Vocabularies</a>
    (CC BY 4.0) · prefix.cc
  </footer>
</div>

<style>
  .vs-page { max-width: 1100px; margin: 0 auto; padding: 1.25rem 1.5rem 3rem; }
  .vs-header { display: flex; justify-content: space-between; align-items: flex-start; gap: 1rem; flex-wrap: wrap; }
  .vs-title { display: flex; gap: 0.75rem; align-items: flex-start; }
  .vs-title h1 { margin: 0; font-size: 1.35rem; }
  .vs-sub { margin: 0.15rem 0 0; color: var(--text-muted, #64748b); font-size: 0.85rem; }
  .vs-status { display: flex; gap: 0.75rem; font-size: 0.75rem; color: var(--text-muted, #64748b); flex-wrap: wrap; align-items: center; }
  .vs-building { display: inline-flex; align-items: center; gap: 0.25rem; color: var(--accent, #0d9488); }

  .vs-tabs { display: flex; gap: 0.25rem; margin: 1rem 0 0.75rem; border-bottom: 1px solid var(--border, #e2e8f0); }
  .vs-tabs button { display: inline-flex; align-items: center; gap: 0.4rem; padding: 0.5rem 0.9rem; border: none; background: none; cursor: pointer; font-size: 0.85rem; color: var(--text-muted, #64748b); border-bottom: 2px solid transparent; }
  .vs-tabs button.active { color: var(--accent, #0d9488); border-bottom-color: var(--accent, #0d9488); font-weight: 600; }

  .vs-searchbar { position: relative; display: flex; align-items: center; gap: 0.5rem; margin: 0.5rem 0; }
  .vs-searchbar input { flex: 1; padding: 0.55rem 0.75rem 0.55rem 2.1rem; border: 1px solid var(--border, #cbd5e1); border-radius: 8px; font-size: 0.9rem; background: var(--bg-input, #fff); color: inherit; }
  :global(.vs-search-icon) { position: absolute; left: 0.7rem; color: var(--text-muted, #94a3b8); }
  :global(.vs-spin) { animation: vs-rot 0.9s linear infinite; }
  @keyframes vs-rot { to { transform: rotate(360deg); } }

  .vs-filters { display: flex; gap: 0.35rem; flex-wrap: wrap; align-items: center; margin-bottom: 0.75rem; }
  .vs-filter-sep { width: 1px; height: 1.1rem; background: var(--border, #e2e8f0); margin: 0 0.25rem; }
  .vs-chip { padding: 0.2rem 0.6rem; border-radius: 999px; border: 1px solid var(--border, #cbd5e1); background: none; font-size: 0.75rem; cursor: pointer; color: inherit; display: inline-flex; align-items: center; gap: 0.25rem; }
  .vs-chip.on { background: var(--accent, #0d9488); border-color: var(--accent, #0d9488); color: #fff; }

  .vs-columns { display: grid; grid-template-columns: 1fr 220px; gap: 1.25rem; }
  @media (max-width: 800px) { .vs-columns { grid-template-columns: 1fr; } }
  .vs-count { font-size: 0.78rem; color: var(--text-muted, #64748b); margin: 0.25rem 0 0.5rem; }
  .vs-error { color: #dc2626; font-size: 0.85rem; }
  .vs-loading { display: flex; justify-content: center; padding: 2rem; }

  .vs-suggest { font-size: 0.82rem; }
  .vs-suggest-btn { border: none; background: none; color: var(--accent, #0d9488); cursor: pointer; text-decoration: underline; padding: 0 0.25rem; }

  .vs-hit { border: 1px solid var(--border, #e2e8f0); border-radius: 10px; padding: 0.65rem 0.85rem; margin-bottom: 0.55rem; background: var(--bg-card, #fff); }
  .vs-hit-head { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .vs-prefixed { font-weight: 600; font-size: 0.88rem; color: var(--accent, #0d9488); }
  .vs-badge { font-size: 0.65rem; padding: 0.1rem 0.45rem; border-radius: 999px; text-transform: uppercase; letter-spacing: 0.02em; }
  .vs-type-class { background: #dbeafe; color: #1d4ed8; }
  .vs-type-property { background: #dcfce7; color: #15803d; }
  .vs-type-datatype { background: #fef9c3; color: #a16207; }
  .vs-type-instance { background: #f1f5f9; color: #475569; }
  .vs-src-platform { background: #dcfce7; color: #15803d; display: inline-flex; align-items: center; gap: 0.2rem; }
  .vs-src-lov { background: #ffedd5; color: #c2410c; }
  .vs-score { flex: 0 0 70px; margin-left: auto; height: 5px; border-radius: 3px; background: var(--border, #e2e8f0); overflow: hidden; }
  .vs-score-bar { display: block; height: 100%; background: var(--accent, #0d9488); }
  .vs-hit-label { font-size: 0.85rem; font-weight: 500; margin-top: 0.25rem; }
  .vs-hit-desc { font-size: 0.8rem; color: var(--text-muted, #64748b); margin: 0.2rem 0 0.3rem; }
  .vs-hit-foot { display: flex; gap: 0.75rem; align-items: center; flex-wrap: wrap; }
  .vs-iri { font-size: 0.72rem; color: var(--text-muted, #94a3b8); text-decoration: none; display: inline-flex; align-items: center; gap: 0.2rem; word-break: break-all; }
  .vs-iri:hover { text-decoration: underline; }
  :global(.vs-model-link) { font-size: 0.75rem; color: var(--accent, #0d9488); display: inline-flex; align-items: center; gap: 0.1rem; text-decoration: none; }

  .vs-facets h3 { font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.05em; color: var(--text-muted, #94a3b8); margin: 0.75rem 0 0.3rem; }
  .vs-facets ul { list-style: none; margin: 0; padding: 0; }
  .vs-facets li { display: flex; }
  .vs-facets li > button, .vs-facets li > .vs-facet-name { flex: 1; display: flex; justify-content: space-between; }
  .vs-facets button { border: none; background: none; cursor: pointer; padding: 0.2rem 0.3rem; font-size: 0.8rem; border-radius: 6px; color: inherit; width: 100%; }
  .vs-facets button.on { background: var(--accent-soft, #ccfbf1); color: var(--accent, #0f766e); }
  .vs-facet-count { color: var(--text-muted, #94a3b8); font-size: 0.72rem; }
  .vs-facets li { padding: 0.1rem 0.3rem; font-size: 0.8rem; justify-content: space-between; }

  .vs-cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(300px, 1fr)); gap: 0.75rem; }
  .vs-card { border: 1px solid var(--border, #e2e8f0); border-radius: 10px; padding: 0.75rem 0.9rem; background: var(--bg-card, #fff); display: flex; flex-direction: column; gap: 0.35rem; }
  .vs-card-head { display: flex; justify-content: space-between; align-items: center; }
  .vs-card h4 { margin: 0; font-size: 0.92rem; }
  .vs-card-desc { font-size: 0.78rem; color: var(--text-muted, #64748b); margin: 0; }
  .vs-card-tags { display: flex; gap: 0.3rem; flex-wrap: wrap; }
  .vs-tag { font-size: 0.68rem; background: var(--bg-muted, #f1f5f9); border-radius: 999px; padding: 0.08rem 0.5rem; color: var(--text-muted, #475569); }
  .vs-card-metrics { display: flex; gap: 0.75rem; font-size: 0.72rem; color: var(--text-muted, #94a3b8); }
  .vs-card-actions { margin-top: auto; display: flex; justify-content: space-between; align-items: center; gap: 0.5rem; }
  .vs-nsp { font-size: 0.68rem; color: var(--text-muted, #94a3b8); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 55%; }
  .vs-install { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.75rem; padding: 0.3rem 0.7rem; border-radius: 7px; border: 1px solid var(--accent, #0d9488); color: var(--accent, #0d9488); background: none; cursor: pointer; }
  .vs-install:hover:not(:disabled) { background: var(--accent, #0d9488); color: #fff; }
  .vs-install:disabled { opacity: 0.6; cursor: default; }

  .vs-pager { display: flex; gap: 0.5rem; justify-content: center; align-items: center; margin-top: 0.9rem; font-size: 0.85rem; }
  .vs-pager button { border: 1px solid var(--border, #cbd5e1); background: none; border-radius: 6px; padding: 0.15rem 0.6rem; cursor: pointer; color: inherit; }
  .vs-pager button:disabled { opacity: 0.4; cursor: default; }

  .vs-rec-intro { display: flex; align-items: center; gap: 0.4rem; font-size: 0.82rem; color: var(--text-muted, #64748b); }
  .vs-rec-input { width: 100%; border: 1px solid var(--border, #cbd5e1); border-radius: 8px; padding: 0.6rem 0.8rem; font-size: 0.88rem; font-family: inherit; background: var(--bg-input, #fff); color: inherit; resize: vertical; }
  .vs-rec-controls { display: flex; gap: 0.5rem; margin: 0.5rem 0 1rem; align-items: center; }
  .vs-rec-controls select { padding: 0.35rem 0.5rem; border: 1px solid var(--border, #cbd5e1); border-radius: 7px; font-size: 0.82rem; background: var(--bg-input, #fff); color: inherit; }
  .vs-rec-run { display: inline-flex; align-items: center; gap: 0.35rem; padding: 0.4rem 0.9rem; border-radius: 7px; border: none; background: var(--accent, #0d9488); color: #fff; font-size: 0.85rem; cursor: pointer; }
  .vs-rec-run:disabled { opacity: 0.5; cursor: default; }
  .vs-rec-result h3 { font-size: 0.9rem; margin: 0.5rem 0 0.4rem; }
  .vs-rec-vocabs { display: flex; gap: 0.35rem; flex-wrap: wrap; margin-bottom: 0.75rem; }
  .vs-rec-table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
  .vs-rec-table th { text-align: left; font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.04em; color: var(--text-muted, #94a3b8); padding: 0.3rem 0.5rem; border-bottom: 1px solid var(--border, #e2e8f0); }
  .vs-rec-table td { padding: 0.4rem 0.5rem; border-bottom: 1px solid var(--border, #f1f5f9); vertical-align: top; }
  .vs-rec-term { font-weight: 600; }
  .vs-rec-label { margin-left: 0.4rem; color: var(--text-muted, #64748b); font-size: 0.78rem; }
  .vs-rec-none { color: var(--text-muted, #94a3b8); font-style: italic; }
  .vs-rec-alt code { display: inline-block; margin-right: 0.5rem; font-size: 0.75rem; color: var(--text-muted, #64748b); }

  .vs-attribution { margin-top: 2rem; font-size: 0.72rem; color: var(--text-muted, #94a3b8); text-align: center; }
  .vs-attribution a { color: inherit; }

  .vs-tagcloud { align-items: center; color: var(--text-muted, #94a3b8); }
</style>
