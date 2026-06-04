<script>
  // AI assistant panel for the SHACL editor. Three actions:
  //  · Draft  — generate Turtle from a natural-language description (uses the
  //    selected model context so real class/property IRIs are picked up).
  //  · Explain — describe what the current Turtle validates.
  //  · Improve — suggest concrete refinements.
  //
  // Output for `draft` can be inserted at the editor cursor or replace the whole
  // source. `explain` / `improve` render as markdown alongside the input.
  import { aiShacl, deriveShapes, getModelContext } from '../lib/api.js';
  import { Sparkles, Wand2, BookOpenCheck, Lightbulb, Loader2, ChevronDown, Database } from 'lucide-svelte';
  import { toastError } from '../lib/toast.ts';
  import { t } from 'svelte-i18n';

  /** Current Turtle source — used as input for explain / improve and shown beside draft. */
  export let turtle = '';
  /** Optional dataset whose model + data context (classes + properties) feed the prompt + the
   *  "draft from data" action. */
  export let scopeDatasetId = '';
  /** Callbacks the host wires up to apply the assistant's Turtle output. */
  export let onInsert = (_text) => {};
  export let onReplace = (_text) => {};

  /** @type {'draft' | 'explain' | 'improve'} */
  let task = 'draft';
  let description = '';
  let loading = false;
  let output = '';
  let model = '';
  let mode = 'draft'; // 'draft' (Turtle preview) | 'prose' (explain/improve)
  let contextLoaded = false;
  let modelContext = null;

  async function loadContext() {
    if (contextLoaded || !scopeDatasetId) return;
    try {
      modelContext = await getModelContext({ dataset: scopeDatasetId });
      contextLoaded = true;
    } catch {
      // The assistant degrades gracefully without model context.
    }
  }

  async function run() {
    if (loading) return;
    if (task === 'draft' && !description.trim()) return;
    if ((task === 'explain' || task === 'improve') && !turtle.trim()) {
      toastError($t('components.aiAssistPanel.errorNoTurtle'));
      return;
    }
    loading = true;
    output = '';
    try {
      await loadContext();
      const body = { task };
      if (task === 'draft') body.description = description;
      if (task !== 'draft') body.turtle = turtle;
      if (task === 'improve' && description.trim()) body.description = description;
      if (modelContext) body.modelContext = modelContext;
      const res = await aiShacl(body);
      model = res.model;
      if (res.turtle) {
        output = res.turtle;
        mode = 'draft';
      } else {
        output = res.explanation || '';
        mode = 'prose';
      }
    } catch (e) {
      toastError(e.message);
    } finally {
      loading = false;
    }
  }

  async function draftFromData() {
    if (!scopeDatasetId) {
      toastError($t('components.aiAssistPanel.errorNoScope'));
      return;
    }
    loading = true;
    output = '';
    try {
      const res = await deriveShapes({ dataset_id: scopeDatasetId });
      output = res.turtle || '';
      model = 'shape-induction';
      mode = 'draft';
    } catch (e) {
      toastError(e.message);
    } finally {
      loading = false;
    }
  }

  function insert() { if (output) onInsert(output); }
  function replace() { if (output) onReplace(output); }
</script>

<aside class="ai-panel">
  <header class="ai-head">
    <Sparkles size={14} />
    <span>{$t('components.aiAssistPanel.title')}</span>
    {#if model}<span class="model-pill" title={$t('components.aiAssistPanel.modelPillTitle')}>{model}</span>{/if}
  </header>

  <div class="task-row">
    <button class="task" class:active={task === 'draft'} on:click={() => (task = 'draft')}>
      <Wand2 size={12} /> {$t('components.aiAssistPanel.taskDraft')}
    </button>
    <button class="task" class:active={task === 'explain'} on:click={() => (task = 'explain')}>
      <BookOpenCheck size={12} /> {$t('components.aiAssistPanel.taskExplain')}
    </button>
    <button class="task" class:active={task === 'improve'} on:click={() => (task = 'improve')}>
      <Lightbulb size={12} /> {$t('components.aiAssistPanel.taskImprove')}
    </button>
  </div>

  {#if task === 'draft' || task === 'improve'}
    <label class="prompt-label">
      <span>{task === 'draft' ? $t('components.aiAssistPanel.describeLabel') : $t('components.aiAssistPanel.focusLabel')}</span>
      <textarea
        bind:value={description}
        rows="3"
        placeholder={task === 'draft'
          ? $t('components.aiAssistPanel.describePlaceholder')
          : $t('components.aiAssistPanel.focusPlaceholder')}
      ></textarea>
    </label>
  {/if}

  <div class="ai-actions">
    <button class="btn btn-sm" on:click={run} disabled={loading}>
      {#if loading}<Loader2 size={12} class="spin" /> {$t('components.aiAssistPanel.working')}{:else}<Sparkles size={12} /> {task === 'draft' ? $t('components.aiAssistPanel.taskDraft') : task === 'explain' ? $t('components.aiAssistPanel.taskExplain') : $t('components.aiAssistPanel.suggestImprovements')}{/if}
    </button>
    {#if task === 'draft' && scopeDatasetId}
      <button class="btn btn-sm btn-ghost" on:click={draftFromData} disabled={loading} title={$t('components.aiAssistPanel.draftFromDataTitle')}>
        <Database size={12} /> {$t('components.aiAssistPanel.draftFromData')}
      </button>
    {/if}
  </div>

  {#if output}
    <div class="ai-output">
      <header class="out-head">
        <span>{mode === 'draft' ? $t('components.aiAssistPanel.generatedTurtle') : $t('components.aiAssistPanel.analysis')}</span>
        {#if mode === 'draft'}
          <div class="out-actions">
            <button class="btn btn-xs btn-ghost" on:click={insert}><ChevronDown size={11} /> {$t('components.aiAssistPanel.insertAtCursor')}</button>
            <button class="btn btn-xs" on:click={replace}>{$t('components.aiAssistPanel.replaceSource')}</button>
          </div>
        {/if}
      </header>
      <pre class="out-body" class:prose={mode === 'prose'}>{output}</pre>
    </div>
  {/if}
</aside>

<style>
  .ai-panel { display: flex; flex-direction: column; gap: 0.55rem; padding: 0.7rem; background: linear-gradient(180deg, #fdf4ff, #ffffff); border: 1px solid #f3e8ff; border-radius: 12px; }
  .ai-head { display: flex; align-items: center; gap: 0.35rem; font-size: 0.8rem; font-weight: 700; color: #6d28d9; }
  .model-pill { margin-left: auto; font-size: 0.62rem; padding: 1px 5px; border-radius: 999px; background: #ede9fe; color: #5b21b6; font-weight: 600; }
  .task-row { display: inline-flex; border: 1px solid #e9d5ff; border-radius: 8px; background: #fff; overflow: hidden; align-self: flex-start; }
  .task { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.75rem; padding: 0.3rem 0.6rem; border: none; background: transparent; cursor: pointer; color: #6d28d9; }
  .task:hover { background: #faf5ff; }
  .task.active { background: #ede9fe; color: #5b21b6; font-weight: 700; }
  .prompt-label { display: flex; flex-direction: column; gap: 0.25rem; font-size: 0.78rem; color: #475569; font-weight: 600; }
  .prompt-label textarea { font-family: inherit; font-size: 0.85rem; padding: 0.4rem 0.55rem; border: 1px solid #e9d5ff; border-radius: 8px; background: #fff; resize: vertical; }
  .prompt-label textarea:focus { outline: 2px solid #ddd6fe; outline-offset: -1px; }
  .ai-actions { display: flex; gap: 0.4rem; flex-wrap: wrap; }
  .btn-xs { font-size: 0.7rem; padding: 0.18rem 0.45rem; }
  .ai-output { margin-top: 0.2rem; background: #fff; border: 1px solid #e9d5ff; border-radius: 10px; overflow: hidden; }
  .out-head { display: flex; align-items: center; justify-content: space-between; padding: 0.4rem 0.55rem; background: #faf5ff; border-bottom: 1px solid #e9d5ff; font-size: 0.72rem; font-weight: 700; color: #6d28d9; text-transform: uppercase; letter-spacing: 0.06em; }
  .out-actions { display: inline-flex; gap: 0.25rem; }
  .out-body { margin: 0; padding: 0.55rem 0.7rem; font-family: 'IBM Plex Mono', monospace; font-size: 0.78rem; color: #1e293b; max-height: 320px; overflow: auto; white-space: pre-wrap; word-break: break-word; }
  .out-body.prose { font-family: inherit; }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .ai-panel { background: linear-gradient(180deg, rgba(139,92,246,0.12), transparent); border-color: rgba(139,92,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .ai-head,
  :global(:is([data-theme="dark"], .dark)) .task,
  :global(:is([data-theme="dark"], .dark)) .out-head { color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .model-pill,
  :global(:is([data-theme="dark"], .dark)) .task.active { background: rgba(139,92,246,0.25); color: #d8b4fe; }
  :global(:is([data-theme="dark"], .dark)) .task-row,
  :global(:is([data-theme="dark"], .dark)) .prompt-label textarea,
  :global(:is([data-theme="dark"], .dark)) .ai-output { background: var(--bg-soft); border-color: rgba(139,92,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .task:hover { background: rgba(139,92,246,0.14); }
  :global(:is([data-theme="dark"], .dark)) .prompt-label { color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .out-head { background: rgba(139,92,246,0.14); border-color: rgba(139,92,246,0.3); }
  :global(:is([data-theme="dark"], .dark)) .out-body { color: var(--ink-900); }
</style>
