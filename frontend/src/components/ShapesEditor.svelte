<script>
  import { tick } from 'svelte';
  import { autofocus } from '../lib/actions/autofocus.js';
  import { getShapes, putShapes, inferDataset, getShapeGraphTurtle, putShapeGraphTurtle, getModelContext } from '../lib/api.js';
  import SparqlEditorCM from './SparqlEditorCM.svelte';
  import { t } from 'svelte-i18n';
  import { Save, Zap, Loader2, Check, RefreshCw, Lightbulb, Plus, Search, X, Code, LayoutGrid, Sparkles, CornerDownLeft } from 'lucide-svelte';
  import { toastError } from '../lib/toast.ts';
  import { SHACL_CONSTRAINT_CARDS, CONSTRAINT_GROUPS } from '../lib/shaclConstraints.ts';
  import { parseShapesGraph } from '../lib/shaclModel.ts';
  import AiAssistPanel from './AiAssistPanel.svelte';
  import ShapeBuilder from './ShapeBuilder.svelte';

  export let datasetId = '';
  /**
   * When set, the editor operates on a SHACL Studio Shape Graph (Library mode)
   * instead of a dataset's legacy shapes graph. Provide exactly one of
   * `datasetId` or `shapeGraphId`.
   */
  export let shapeGraphId = '';
  export let height = 'calc(100vh - 320px)';

  let editorRef;
  let shapesContent = '';
  /** Snapshot of the last loaded/saved Turtle — powers the dirty indicator. */
  let savedContent = '';
  let loading = false;
  let saving = false;
  let inferring = false;
  let error = '';
  let saveSuccess = false;
  let inferResult = null;

  let view = 'visual'; // 'visual' | 'source'
  let aiOpen = false;

  // Real classes + properties of the dataset's data, used to drive the visual
  // builder's target/path pickers. Library mode (no datasetId) degrades to free
  // text. Loaded once per dataset.
  let modelContext = null;
  let _ctxFor = null;
  $: if (datasetId && datasetId !== _ctxFor) {
    _ctxFor = datasetId;
    loadModelContext();
  }
  async function loadModelContext() {
    try {
      modelContext = await getModelContext({ dataset: datasetId });
    } catch {
      modelContext = null;
    }
  }

  // Palette / constraint modal state
  let paletteOpen = false;
  let paletteSearch = '';
  $: filteredCards = SHACL_CONSTRAINT_CARDS.filter((c) => {
    if (!paletteSearch.trim()) return true;
    const q = paletteSearch.toLowerCase();
    return c.label.toLowerCase().includes(q) || c.what.toLowerCase().includes(q) || c.id.toLowerCase().includes(q);
  });

  // Lightweight parse powering the toolbar's shape count + invalid-Turtle
  // indicator. The builder runs its own (richer) parse for editing.
  $: graph = parseShapesGraph(shapesContent);
  $: parseError = graph.parseError || null;

  const EMPTY_TEMPLATE = `# SHACL Shapes Graph
PREFIX sh: <http://www.w3.org/ns/shacl#>
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
PREFIX ex: <http://example.org/>
`;

  // Single source-of-truth identifier across both modes — the editor watches
  // it and reloads when it changes.
  $: targetKey = shapeGraphId ? `set:${shapeGraphId}` : datasetId ? `ds:${datasetId}` : '';
  let _loadedFor = null;
  $: if (targetKey && targetKey !== _loadedFor) {
    _loadedFor = targetKey;
    loadShapes();
  }

  async function loadShapes() {
    if (!targetKey) return;
    loading = true;
    error = '';
    inferResult = null;
    try {
      const content = shapeGraphId
        ? await getShapeGraphTurtle(shapeGraphId)
        : await getShapes(datasetId);
      shapesContent = typeof content === 'string' ? content : JSON.stringify(content, null, 2);
    } catch (_) {
      shapesContent = EMPTY_TEMPLATE;
    } finally {
      savedContent = shapesContent;
      loading = false;
    }
  }

  $: dirty = !loading && shapesContent !== savedContent;

  async function saveShapes() {
    if (!targetKey) return;
    saving = true;
    saveSuccess = false;
    error = '';
    try {
      if (shapeGraphId) await putShapeGraphTurtle(shapeGraphId, shapesContent);
      else await putShapes(datasetId, shapesContent);
      savedContent = shapesContent;
      saveSuccess = true;
      setTimeout(() => (saveSuccess = false), 3000);
    } catch (e) {
      error = e.message;
      toastError(e.message);
    } finally {
      saving = false;
    }
  }

  async function runInfer() {
    // SHACL-AF rule inference materialises triples against a dataset; not
    // meaningful for a standalone shape graph in the Library.
    if (!datasetId) return;
    inferring = true;
    inferResult = null;
    error = '';
    try {
      inferResult = await inferDataset(datasetId);
    } catch (e) {
      error = e.message;
      toastError(e.message);
    } finally {
      inferring = false;
    }
  }

  function insertCard(card) {
    if (view !== 'source') view = 'source';
    paletteOpen = false;
    paletteSearch = '';
    tick().then(() => editorRef?.insertAtCursor(card.template));
  }
</script>

<div class="shapes-editor">
  <div class="editor-toolbar">
    <div class="tb-left">
      <span class="section-label">{$t('pages.shaclShapes.shapes')} ({graph.shapes.length})</span>
      {#if parseError}<span class="parse-bad" title={parseError}><X size={12} /> {$t('pages.shaclShapes.invalidTurtle')}</span>{/if}
      <div class="view-toggle">
        <button class="vt" class:active={view === 'visual'} on:click={() => (view = 'visual')} title={$t('pages.shaclShapes.visualBuilderTitle')}><LayoutGrid size={13} /> {$t('pages.shaclShapes.build')}</button>
        <button class="vt" class:active={view === 'source'} on:click={() => (view = 'source')} title={$t('pages.shaclShapes.turtleSourceTitle')}><Code size={13} /> {$t('pages.shaclShapes.source')}</button>
      </div>
    </div>
    <div class="toolbar-actions">
      {#if dirty}
        <span class="dirty-chip" title={$t('components.shapeBuilder.unsavedChanges')}><span class="dirty-dot"></span> {$t('components.shapeBuilder.unsavedChanges')}</span>
      {:else if saveSuccess}
        <span class="save-success"><Check size={14} /> {$t('pages.shaclShapes.saved')}</span>
      {/if}
      {#if inferResult !== null}
        <span class="infer-result">{$t('pages.shaclShapes.inferResult', { values: { count: inferResult.inferred_triples ?? inferResult.inferred_count ?? inferResult } })}</span>
      {/if}
      <div class="palette-anchor">
        <button class="btn btn-sm btn-ghost" on:click={() => (paletteOpen = !paletteOpen)} title={$t('pages.shaclShapes.insertConstraintTitle')} class:active={paletteOpen}>
          <Plus size={14} /> {$t('pages.shaclShapes.constraint')}
        </button>
        {#if paletteOpen}
          <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
          <div class="palette" on:click|stopPropagation>
            <div class="palette-head">
              <div class="input-icon"><Search size={13} /><input placeholder={$t('pages.shaclShapes.searchConstraints')} bind:value={paletteSearch} use:autofocus /></div>
              <button class="icon-btn" on:click={() => (paletteOpen = false)} title={$t('system.close')}><X size={14} /></button>
            </div>
            <div class="palette-list">
              {#each CONSTRAINT_GROUPS as g}
                {@const cards = filteredCards.filter((c) => c.group === g.id)}
                {#if cards.length}
                  <div class="palette-group">{g.label}</div>
                  {#each cards as card (card.id)}
                    <button class="card" on:click={() => insertCard(card)} title={card.example}>
                      <div class="card-top">
                        <span class="card-label">{card.label}</span>
                        {#if card.sourceOnly}<span class="card-source-only">{$t('pages.shaclShapes.sourceOnly')}</span>{/if}
                        <span class="card-insert"><CornerDownLeft size={11} /> {$t('pages.shaclShapes.insert')}</span>
                      </div>
                      <div class="card-what">{card.what}</div>
                    </button>
                  {/each}
                {/if}
              {/each}
              {#if filteredCards.length === 0}
                <div class="palette-empty">{$t('pages.shaclShapes.noConstraintsMatch', { values: { query: paletteSearch } })}</div>
              {/if}
            </div>
          </div>
        {/if}
      </div>
      <button class="btn btn-sm btn-ghost" on:click={loadShapes} title={$t('system.refresh')} disabled={loading}><RefreshCw size={13} /></button>
      <button class="btn btn-sm btn-ghost ai-toggle" class:active={aiOpen} on:click={() => (aiOpen = !aiOpen)} title={$t('pages.shaclShapes.aiAssistantTitle')}>
        <Sparkles size={13} /> AI
      </button>
      {#if datasetId}
        <button class="btn btn-sm btn-ghost" on:click={runInfer} disabled={inferring || !datasetId}>
          {#if inferring}<Loader2 size={14} class="spin" /> {$t('pages.shaclShapes.inferring')}{:else}<Zap size={14} /> {$t('pages.shaclShapes.infer')}{/if}
        </button>
      {/if}
      <button class="btn btn-sm save-btn" class:has-dirty={dirty} on:click={saveShapes} disabled={saving || !targetKey} title={dirty ? $t('components.shapeBuilder.unsavedChanges') : undefined}>
        {#if saving}<Loader2 size={14} class="spin" /> {$t('pages.shaclShapes.saving')}{:else}<Save size={14} /> {$t('system.save')}{/if}
        {#if dirty}<span class="dirty-dot on-btn"></span>{/if}
      </button>
    </div>
  </div>

  {#if error}<p class="error">{error}</p>{/if}

  <div class="editor-grid" class:has-ai={aiOpen}>
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="content" on:click={() => (paletteOpen = false)} style="--editor-h:{height}">
    <!-- Visual builder -->
    <div class="cards" class:hidden={view !== 'visual'}>
      <ShapeBuilder
        turtle={shapesContent}
        {modelContext}
        {loading}
        onChange={(ttl) => (shapesContent = ttl)}
      />
    </div>

    <!-- Turtle source (kept mounted so jump-to-source works) -->
    <div class="source" class:hidden={view !== 'source'}>
      {#if loading}
        <div class="editor-loading">{$t('system.loading')}</div>
      {:else}
        <SparqlEditorCM bind:this={editorRef} bind:query={shapesContent} mode="turtle" {height} />
        <div class="editor-footer">
          <span class="editor-hint"><Lightbulb size={13} /> {$t('pages.shaclShapes.editorHint')}</span>
        </div>
      {/if}
    </div>
  </div>

    {#if aiOpen}
      <aside class="ai-side">
        <AiAssistPanel
          turtle={shapesContent}
          scopeDatasetId={datasetId}
          onInsert={(text) => { view = 'source'; tick().then(() => editorRef?.insertAtCursor('\n' + text + '\n')); }}
          onReplace={(text) => { view = 'source'; shapesContent = text; }}
        />
      </aside>
    {/if}
  </div>
</div>

<style>
  .shapes-editor { display: flex; flex-direction: column; gap: 0.6rem; }
  .editor-grid { display: grid; grid-template-columns: minmax(0, 1fr); gap: 0.75rem; align-items: stretch; }
  .editor-grid.has-ai { grid-template-columns: minmax(0, 1fr) minmax(280px, 360px); }
  .ai-side { min-width: 0; }
  .ai-toggle.active { background: #faf5ff; color: #6d28d9; border-color: #ddd6fe; }
  @media (max-width: 960px) {
    .editor-grid.has-ai { grid-template-columns: 1fr; }
  }
  .editor-toolbar { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; flex-wrap: wrap; }
  .tb-left { display: flex; align-items: center; gap: 0.6rem; flex-wrap: wrap; }
  .section-label { font-size: 0.75rem; font-weight: 700; color: #64748b; text-transform: uppercase; letter-spacing: 0.06em; }
  .parse-bad { display: inline-flex; align-items: center; gap: 0.2rem; font-size: 0.72rem; font-weight: 600; color: #b91c1c; background: #fef2f2; border: 1px solid #fecaca; padding: 1px 7px; border-radius: 999px; }
  .toolbar-actions { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .save-success { color: #15803d; font-size: 0.82rem; font-weight: 600; display: inline-flex; align-items: center; gap: 0.25rem; }
  .infer-result { font-size: 0.82rem; color: #6f42c1; background: #f5f0ff; padding: 0.2rem 0.6rem; border-radius: 6px; }
  .error { color: #dc2626; background: #fef2f2; border: 1px solid #fecaca; padding: 0.5rem 0.7rem; border-radius: 8px; font-size: 0.82rem; margin: 0; }
  .btn-ghost.active { background: #ecfeff; color: #0e7490; border-color: #7ED6D0; }

  .view-toggle { display: inline-flex; gap: 2px; border: 1px solid var(--line-soft); border-radius: 9px; padding: 2px; background: var(--bg-soft); }
  .vt { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.78rem; font-weight: 600; padding: 0.3rem 0.7rem; border: 1px solid transparent; border-radius: 7px; background: transparent; color: #64748b; cursor: pointer; transition: background 0.12s, color 0.12s; }
  .vt:hover { background: rgba(255,255,255,0.7); color: #334155; }
  .vt.active { background: #fff; color: #0e7490; font-weight: 700; border-color: var(--brand-300, #7ED6D0); box-shadow: var(--shadow-xs); }

  .dirty-chip { display: inline-flex; align-items: center; gap: 0.35rem; font-size: 0.74rem; font-weight: 600; color: #92400e; background: #fef3c7; border: 1px solid #fde68a; padding: 2px 9px; border-radius: 999px; white-space: nowrap; }
  .dirty-dot { width: 7px; height: 7px; border-radius: 50%; background: #d97706; flex-shrink: 0; }
  .dirty-dot.on-btn { background: #fff; box-shadow: 0 0 0 2px rgba(217,119,6,0.85); margin-left: 0.1rem; }
  .save-btn { position: relative; }

  /* Palette */
  .palette-anchor { position: relative; }
  .palette { position: absolute; right: 0; top: calc(100% + 6px); z-index: 60; width: 380px; max-width: 90vw; max-height: 62vh; display: flex; flex-direction: column; background: #fff; border: 1px solid var(--line-soft); border-radius: 12px; box-shadow: 0 12px 32px rgba(15,23,42,0.18); }
  .palette-head { display: flex; align-items: center; gap: 0.4rem; padding: 0.6rem; border-bottom: 1px solid var(--line-soft); }
  .input-icon { display: flex; align-items: center; gap: 0.4rem; padding: 0.35rem 0.6rem; border: 1px solid var(--line-soft); border-radius: 8px; background: #fff; flex: 1; color: #64748b; }
  .input-icon input { border: none; outline: none; background: transparent; font-size: 0.82rem; color: #1e293b; flex: 1; min-width: 0; }
  .icon-btn { display: grid; place-items: center; width: 28px; height: 28px; border-radius: 8px; border: 1px solid var(--line-soft); background: #fff; color: #64748b; cursor: pointer; }
  .icon-btn:hover { background: #f1f5f9; }
  .palette-list { overflow: auto; padding: 0.4rem; }
  .palette-group { font-size: 0.68rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: #94a3b8; padding: 0.5rem 0.4rem 0.2rem; }
  .card { display: block; width: 100%; text-align: left; background: #fff; border: 1px solid transparent; border-radius: 8px; padding: 0.45rem 0.55rem; cursor: pointer; }
  .card:hover { background: #f0fdfa; border-color: #7ED6D0; }
  .card-top { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; }
  .card-label { font-size: 0.84rem; font-weight: 600; color: #1e293b; }
  .card-insert { display: inline-flex; align-items: center; gap: 0.2rem; font-size: 0.68rem; color: #0e7490; opacity: 0; }
  .card-source-only { font-size: 0.64rem; font-weight: 600; color: #92400e; background: #fef3c7; border-radius: 999px; padding: 0 6px; white-space: nowrap; }
  :global(:is([data-theme="dark"], .dark)) .card-source-only { background: rgba(245,158,11,0.18); color: #fcd34d; }
  .card:hover .card-insert { opacity: 1; }
  .card-what { font-size: 0.74rem; color: #64748b; line-height: 1.35; margin-top: 0.1rem; }
  .palette-empty { padding: 1rem; text-align: center; color: #94a3b8; font-size: 0.82rem; }

  .content { min-height: 0; }
  .hidden { display: none !important; }

  /* Visual builder wrapper (cards rendered by ShapeBuilder) */
  .cards { display: flex; flex-direction: column; gap: 0.7rem; max-height: var(--editor-h); overflow: auto; padding-right: 0.2rem; }

  .source { display: flex; flex-direction: column; }
  .editor-loading { display: flex; align-items: center; justify-content: center; color: #94a3b8; border: 1px solid var(--line-soft); border-radius: 10px; padding: 2rem; }
  .editor-footer { margin-top: 0.4rem; font-size: 0.75rem; color: #94a3b8; }
  .editor-hint { display: inline-flex; align-items: center; gap: 0.3rem; }
  :global(.spin) { animation: spin 0.9s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }

  :global(:is([data-theme="dark"], .dark)) .ai-toggle.active { background: rgba(139,92,246,0.2); color: #c4b5fd; border-color: rgba(139,92,246,0.35); }
  :global(:is([data-theme="dark"], .dark)) .parse-bad { color: #fca5a5; background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .save-success { color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .infer-result { color: #c4b5fd; background: rgba(139,92,246,0.2); }
  :global(:is([data-theme="dark"], .dark)) .error { color: #fca5a5; background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .btn-ghost.active { background: var(--brand-100); color: var(--brand-700); border-color: var(--brand-200); }
  :global(:is([data-theme="dark"], .dark)) .view-toggle { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .vt { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .vt:hover { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .vt.active { background: var(--brand-100); color: var(--brand-700); border-color: var(--brand-300); box-shadow: none; }
  :global(:is([data-theme="dark"], .dark)) .dirty-chip { background: rgba(245,158,11,0.14); border-color: rgba(245,158,11,0.35); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .dirty-dot { background: #fbbf24; }
  :global(:is([data-theme="dark"], .dark)) .dirty-dot.on-btn { background: #fff; box-shadow: 0 0 0 2px rgba(251,191,36,0.85); }
  :global(:is([data-theme="dark"], .dark)) .palette { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .input-icon { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .input-icon input { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .card { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .card:hover { background: var(--bg-accent-soft); border-color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .card-label { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .card-insert { color: var(--brand-700); }
</style>
