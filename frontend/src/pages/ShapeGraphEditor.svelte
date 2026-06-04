<script>
  import { onMount } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { sanitizeHtml } from '../lib/ontology/sanitizeHtml.js';
  import { getShapeGraph, updateShapeGraph, listShapeGraphRevisions, getShapeGraphRevision, restoreShapeGraphRevision,
    validateShapeGraph, stageShapeGraph, publishShapeGraph, deprecateShapeGraph, listBindingsForShapeGraph,
    getShapeGraphTurtle } from '../lib/api.js';
  import { ArrowLeft, History, Lock, Users, Globe, Save, Edit3, X, RotateCcw, Loader2, Sparkles, Database, ShieldCheck, Send, Archive, Check, Plus, Link2, ExternalLink } from 'lucide-svelte';
  import { Link, navigate } from '../lib/router/index.js';
  import { openPendingViewerTab, showShapesInViewer, viewerConfigured } from '../lib/graphViewer.ts';
  import ShaclStudioNav from '../components/ShaclStudioNav.svelte';
  import ShapesEditor from '../components/ShapesEditor.svelte';
  import ShapesCatalog from '../components/ShapesCatalog.svelte';
  import CommitHistory from '../components/CommitHistory.svelte';
  import Select from '../components/Select.svelte';
  import { isAuthenticated, authInitialized } from '../lib/stores.js';
  import { toastError, toastSuccess } from '../lib/toast.ts';

  export let id = '';

  let set = null;
  let loading = true;
  let error = '';

  let editingMeta = false;
  let editName = '';
  let editDescription = '';
  let editVisibility = 'private';
  let saving = false;

  let showHistory = false;
  let revisions = [];
  let revLoading = false;

  // Meta-validation (SHACL-of-SHACL) + lifecycle.
  let showMeta = false;
  let metaReport = null;
  let validatingMeta = false;
  let transitioning = false;

  // Compose ("add existing shapes") + impact ("what data this is applied on").
  let showAddShapes = false;
  let impact = [];
  let editorReloadToken = 0;

  // "Open in graph viewer" — hand the compiled shapes to an external viewer's
  // SHACL sandbox. Only shown when VITE_GRAPH_VIEWER_URL is configured.
  let openingViewer = false;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAuthenticated) navigate('/login');
  }

  onMount(load);

  $: if (id) { load(); }

  async function load() {
    if (!id) return;
    loading = true;
    error = '';
    try {
      set = await getShapeGraph(id);
      editName = set.name;
      editDescription = set.description || '';
      editVisibility = set.visibility;
      try { const imp = await listBindingsForShapeGraph(id); impact = imp?.targets || []; } catch { impact = []; }
    } catch (e) {
      error = e.message;
    } finally {
      loading = false;
    }
  }

  async function saveMeta() {
    saving = true;
    try {
      const updated = await updateShapeGraph(id, {
        name: editName.trim(),
        description: editDescription.trim() || undefined,
        visibility: editVisibility,
        tags: set.tags || [],
      });
      set = updated;
      editingMeta = false;
      toastSuccess($i18nT('pages.shapeGraphEditor.toastUpdated'));
    } catch (e) {
      toastError(e.message);
    } finally {
      saving = false;
    }
  }

  async function loadHistory() {
    showHistory = true;
    revLoading = true;
    try {
      revisions = await listShapeGraphRevisions(id);
    } catch (e) {
      toastError(e.message);
    } finally {
      revLoading = false;
    }
  }

  async function previewRevision(rev) {
    try {
      const r = await getShapeGraphRevision(id, rev);
      // Show in a simple alert window — Phase 4 brings a proper diff viewer.
      const win = window.open('', '_blank');
      if (win) {
        win.document.title = $i18nT('pages.shapeGraphEditor.revisionTitle', { values: { rev } });
        win.document.body.style.font = '0.85rem monospace';
        win.document.body.textContent = r.turtle || '';
      }
    } catch (e) { toastError(e.message); }
  }

  async function restoreRevision(rev) {
    if (!confirm($i18nT('pages.shapeGraphEditor.confirmRestore', { values: { rev } }))) return;
    try {
      const res = await restoreShapeGraphRevision(id, rev);
      toastSuccess($i18nT('pages.shapeGraphEditor.toastRestored', { values: { version: res.version } }));
      await load();
      await loadHistory();
    } catch (e) { toastError(e.message); }
  }

  async function runMetaValidation() {
    showMeta = true;
    validatingMeta = true;
    metaReport = null;
    try {
      metaReport = await validateShapeGraph(id);
    } catch (e) {
      toastError(e.message);
      showMeta = false;
    } finally {
      validatingMeta = false;
    }
  }

  async function transition(fn, label) {
    transitioning = true;
    try {
      const res = await fn(id);
      set = { ...set, status: res.status };
      toastSuccess($i18nT('pages.shapeGraphEditor.toastTransitioned', { values: { label } }));
    } catch (e) {
      toastError(e.message);
    } finally {
      transitioning = false;
    }
  }

  function onShapesImported() {
    showAddShapes = false;
    editorReloadToken += 1; // remount the editor so it reloads the new Turtle
    load();                 // refresh shape_count / impact
  }

  async function openInViewer() {
    // Open the tab synchronously (inside the click gesture) so the browser
    // doesn't treat it as a blocked popup; navigate it once the Turtle loads.
    const win = openPendingViewerTab();
    openingViewer = true;
    try {
      const ttl = await getShapeGraphTurtle(id);
      if (!ttl || !ttl.trim()) {
        toastError($i18nT('pages.shapeGraphEditor.noShapesToOpen'));
        win?.close();
        return;
      }
      showShapesInViewer(win, ttl);
    } catch (e) {
      toastError(e.message);
      win?.close();
    } finally {
      openingViewer = false;
    }
  }

  function relativeTime(iso) {
    if (!iso) return '';
    const sec = Math.round((Date.now() - new Date(iso).getTime()) / 1000);
    if (sec < 60) return $i18nT('pages.shapeGraphEditor.justNow');
    const min = Math.round(sec / 60); if (min < 60) return $i18nT('pages.shapeGraphEditor.minutesAgo', { values: { count: min } });
    const hr = Math.round(min / 60); if (hr < 24) return $i18nT('pages.shapeGraphEditor.hoursAgo', { values: { count: hr } });
    const day = Math.round(hr / 24); if (day < 30) return $i18nT('pages.shapeGraphEditor.daysAgo', { values: { count: day } });
    return $i18nT('pages.shapeGraphEditor.monthsAgo', { values: { count: Math.round(day / 30) } });
  }

  function shortIRI(iri) {
    const m = String(iri).match(/[^#/]+$/);
    return m ? m[0] : iri;
  }
</script>

<div class="editor-page">
  <ShaclStudioNav />

  {#if loading}
    <div class="card placeholder"><Loader2 size={28} class="spin" /><p>{$i18nT('pages.shapeGraphEditor.loadingShapeGraph')}</p></div>
  {:else if error}
    <div class="error">{error}</div>
  {:else if set}
    <div class="card meta-card">
      <Link to="/shacl/shapes" class="back"><ArrowLeft size={13} /> {$i18nT('pages.shapeGraphEditor.libraryLink')}</Link>
      {#if editingMeta}
        <form class="meta-edit" on:submit|preventDefault={saveMeta}>
          <input bind:value={editName} class="name-input" />
          <textarea bind:value={editDescription} placeholder={$i18nT('pages.shapeGraphEditor.descriptionPlaceholder')} rows="2"></textarea>
          <div class="meta-edit-row">
            <Select bind:value={editVisibility} class="grow" options={[
              { value: 'private', label: $i18nT('pages.shapeGraphEditor.visibilityPrivate') },
              { value: 'members', label: $i18nT('pages.shapeGraphEditor.visibilityMembers') },
              { value: 'public', label: $i18nT('pages.shapeGraphEditor.visibilityPublic') },
            ]} />
            <button type="button" class="btn btn-sm btn-ghost" on:click={() => (editingMeta = false)}><X size={12} /> {$i18nT('system.cancel')}</button>
            <button type="submit" class="btn btn-sm" disabled={saving}>
              {#if saving}<Loader2 size={12} class="spin" />{:else}<Save size={12} />{/if} {$i18nT('system.save')}
            </button>
          </div>
        </form>
      {:else}
        <div class="meta-row">
          <div class="meta-main">
            <h2>{set.name}</h2>
            {#if set.description}<p class="meta-desc">{set.description}</p>{/if}
            <div class="meta-chips">
              {#if set.status}<span class="chip status-{set.status}">{set.status}</span>{/if}
              {#if set.visibility === 'public'}<span class="chip"><Globe size={10} /> {$i18nT('pages.shapeGraphEditor.chipPublic')}</span>{:else if set.visibility === 'members'}<span class="chip"><Users size={10} /> {$i18nT('pages.shapeGraphEditor.chipMembers')}</span>{:else}<span class="chip"><Lock size={10} /> {$i18nT('pages.shapeGraphEditor.chipPrivate')}</span>{/if}
              {#if set.source && set.source !== 'manual'}
                <span class="chip chip-source-{set.source}">{#if set.source === 'ai'}<Sparkles size={10} />{/if} {set.source}</span>
              {/if}
              <span class="chip">v{set.version}</span>
              <span class="chip"><strong>{set.shape_count}</strong> {set.shape_count === 1 ? $i18nT('pages.shapeGraphEditor.shapeSingular') : $i18nT('pages.shapeGraphEditor.shapePlural')}</span>
              <span class="chip dim">{$i18nT('pages.shapeGraphEditor.updatedPrefix', { values: { time: relativeTime(set.updated_at) } })}</span>
            </div>
            {#if (set.target_classes || []).length}
              <div class="targets">
                {#each set.target_classes as tc}<span class="chip chip-target"><Database size={10} /> {shortIRI(tc)}</span>{/each}
              </div>
            {/if}
            {#if impact.length}
              <div class="impact">
                <Link2 size={11} /> <span class="impact-label">{impact.length === 1 ? $i18nT('pages.shapeGraphEditor.appliedToTarget', { values: { count: impact.length } }) : $i18nT('pages.shapeGraphEditor.appliedToTargets', { values: { count: impact.length } })}</span>
                {#each impact.slice(0, 5) as t}<span class="chip chip-applied" title={t}>{shortIRI(t)}</span>{/each}
                {#if impact.length > 5}<span class="chip">+{impact.length - 5}</span>{/if}
              </div>
            {/if}
          </div>
          <div class="meta-actions">
            {#if viewerConfigured()}
            <button class="btn btn-sm btn-ghost" on:click={openInViewer} disabled={openingViewer} title={$i18nT('pages.shapeGraphEditor.openInViewerTitle')}>
              {#if openingViewer}<Loader2 size={13} class="spin" />{:else}<ExternalLink size={13} />{/if} {$i18nT('pages.shapeGraphEditor.openInViewerButton')}
            </button>
            {/if}
            <button class="btn btn-sm btn-ghost" on:click={() => (showAddShapes = true)} title={$i18nT('pages.shapeGraphEditor.addShapesTitle')}><Plus size={13} /> {$i18nT('pages.shapeGraphEditor.addShapesButton')}</button>
            <button class="btn btn-sm btn-ghost" on:click={runMetaValidation} disabled={validatingMeta} title={$i18nT('pages.shapeGraphEditor.validateTitle')}>
              {#if validatingMeta}<Loader2 size={13} class="spin" />{:else}<ShieldCheck size={13} />{/if} {$i18nT('pages.shapeGraphEditor.validateButton')}
            </button>
            {#if set.status === 'draft'}
              <button class="btn btn-sm btn-ghost" on:click={() => transition(stageShapeGraph, $i18nT('pages.shapeGraphEditor.labelStaged'))} disabled={transitioning}><Check size={13} /> {$i18nT('pages.shapeGraphEditor.stageButton')}</button>
            {/if}
            {#if set.status === 'draft' || set.status === 'staged'}
              <button class="btn btn-sm btn-ghost" on:click={() => transition(publishShapeGraph, $i18nT('pages.shapeGraphEditor.labelPublished'))} disabled={transitioning}><Send size={13} /> {$i18nT('pages.shapeGraphEditor.publishButton')}</button>
            {/if}
            {#if set.status && set.status !== 'deprecated'}
              <button class="btn btn-sm btn-ghost" on:click={() => transition(deprecateShapeGraph, $i18nT('pages.shapeGraphEditor.labelDeprecated'))} disabled={transitioning}><Archive size={13} /> {$i18nT('pages.shapeGraphEditor.deprecateButton')}</button>
            {/if}
            <button class="btn btn-sm btn-ghost" on:click={loadHistory}><History size={13} /> {$i18nT('pages.shapeGraphEditor.historyButton')}</button>
            <button class="btn btn-sm btn-ghost" on:click={() => (editingMeta = true)}><Edit3 size={13} /> {$i18nT('pages.shapeGraphEditor.editDetailsButton')}</button>
          </div>
        </div>
      {/if}
    </div>

    <div class="card editor-host">
      {#key editorReloadToken}
        <ShapesEditor shapeGraphId={id} height="calc(100vh - 360px)" />
      {/key}
    </div>

    <CommitHistory kind="shape-graph" {id} />
  {/if}

  {#if showAddShapes && set}
    <div class="modal-backdrop" on:click={() => (showAddShapes = false)} role="presentation">
      <div class="modal modal-wide" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" tabindex="-1">
        <header class="modal-head">
          <h3><Plus size={14} /> {$i18nT('pages.shapeGraphEditor.addShapesHeading')}</h3>
          <button class="icon-btn" on:click={() => (showAddShapes = false)}><X size={14} /></button>
        </header>
        <div class="modal-body">
          <!-- eslint-disable-next-line svelte/no-at-html-tags -- DOMPurify-sanitized -->
          <p class="add-hint">{@html sanitizeHtml($i18nT('pages.shapeGraphEditor.addShapesHint', { values: { name: `<strong>${set.name}</strong>` } }))}</p>
          <ShapesCatalog picker targetGraphId={id} excludeGraphIri={set.graph_iri} on:imported={onShapesImported} />
        </div>
      </div>
    </div>
  {/if}

  {#if showMeta}
    <div class="modal-backdrop" on:click={() => (showMeta = false)} role="presentation">
      <div class="modal" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" tabindex="-1">
        <header class="modal-head">
          <h3><ShieldCheck size={14} /> {$i18nT('pages.shapeGraphEditor.metaValidationHeading')}</h3>
          <button class="icon-btn" on:click={() => (showMeta = false)}><X size={14} /></button>
        </header>
        <div class="modal-body">
          {#if validatingMeta}
            <div class="placeholder"><Loader2 size={22} class="spin" /> {$i18nT('pages.shapeGraphEditor.validatingShapes')}</div>
          {:else if metaReport}
            <p class="meta-verdict" class:ok={metaReport.conforms}>
              {#if metaReport.conforms}<Check size={15} /> {$i18nT('pages.shapeGraphEditor.shapesWellFormed')}{:else}<X size={15} /> {metaReport.results_count === 1 ? $i18nT('pages.shapeGraphEditor.issuesFound', { values: { count: metaReport.results_count } }) : $i18nT('pages.shapeGraphEditor.issuesFoundPlural', { values: { count: metaReport.results_count } })}{/if}
            </p>
            {#if (metaReport.results || []).length}
              <table class="meta-table">
                <thead><tr><th>{$i18nT('pages.shapeGraphEditor.colSeverity')}</th><th>{$i18nT('pages.shapeGraphEditor.colFocus')}</th><th>{$i18nT('pages.shapeGraphEditor.colPath')}</th><th>{$i18nT('pages.shapeGraphEditor.colMessage')}</th></tr></thead>
                <tbody>
                  {#each metaReport.results as r}
                    <tr>
                      <td><span class="sev sev-{r.severity}">{r.severity}</span></td>
                      <td><code>{shortIRI(r.focus_node)}</code></td>
                      <td>{r.path ? shortIRI(r.path) : '—'}</td>
                      <td>{r.message}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            {/if}
          {/if}
        </div>
      </div>
    </div>
  {/if}

  {#if showHistory}
    <div class="modal-backdrop" on:click={() => (showHistory = false)} role="presentation">
      <div class="modal" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" tabindex="-1">
        <header class="modal-head">
          <h3><History size={14} /> {$i18nT('pages.shapeGraphEditor.revisionHistoryHeading')}</h3>
          <button class="icon-btn" on:click={() => (showHistory = false)}><X size={14} /></button>
        </header>
        <div class="modal-body">
          {#if revLoading}
            <div class="placeholder"><Loader2 size={22} class="spin" /></div>
          {:else if revisions.length === 0}
            <p class="dim">{$i18nT('pages.shapeGraphEditor.noRevisions')}</p>
          {:else}
            <ul class="rev-list">
              {#each revisions as r}
                <li class="rev-row">
                  <span class="rev-num">v{r.revision}</span>
                  <span class="rev-note">{r.note || $i18nT('pages.shapeGraphEditor.noNote')}</span>
                  <span class="rev-time">{relativeTime(r.created_at)}</span>
                  <button class="btn btn-xs btn-ghost" on:click={() => previewRevision(r.revision)}>{$i18nT('pages.shapeGraphEditor.previewButton')}</button>
                  {#if r.revision !== set.version}
                    <button class="btn btn-xs btn-ghost" on:click={() => restoreRevision(r.revision)}><RotateCcw size={11} /> {$i18nT('pages.shapeGraphEditor.restoreButton')}</button>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .editor-page { display: flex; flex-direction: column; }
  .error { color: #dc2626; background: #fef2f2; border: 1px solid #fecaca; padding: 0.6rem 0.8rem; border-radius: 10px; font-size: 0.85rem; }
  .placeholder { display: flex; align-items: center; justify-content: center; gap: 0.5rem; padding: 3rem; color: #94a3b8; }
  .placeholder p { margin: 0; }

  .meta-card { padding: 0.85rem 1.1rem !important; margin-bottom: 0.85rem; }
  :global(.editor-page .back) { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.78rem; color: #2F7A8C; text-decoration: none; margin-bottom: 0.45rem; }
  :global(.editor-page .back:hover) { text-decoration: underline; }
  .meta-row { display: flex; gap: 1rem; align-items: flex-start; justify-content: space-between; }
  .meta-main { flex: 1; min-width: 0; }
  .meta-main h2 { margin: 0 0 0.2rem; font-size: 1.15rem; }
  .meta-desc { margin: 0 0 0.5rem; color: #64748b; font-size: 0.85rem; }
  .meta-chips, .targets { display: flex; gap: 0.25rem; flex-wrap: wrap; }
  .targets { margin-top: 0.45rem; }
  .impact { display: flex; align-items: center; gap: 0.3rem; flex-wrap: wrap; margin-top: 0.5rem; font-size: 0.76rem; color: #475569; }
  .impact-label { font-weight: 600; }
  .chip-applied { background: #f0fdf4; color: #15803d; font-family: 'IBM Plex Mono', monospace; font-weight: 500; text-transform: none; }
  .add-hint { margin: 0 0 0.6rem; font-size: 0.82rem; color: #64748b; }
  .chip { display: inline-flex; align-items: center; gap: 0.2rem; font-size: 0.7rem; padding: 2px 7px; border-radius: 999px; background: #f1f5f9; color: #475569; font-weight: 600; text-transform: capitalize; }
  .chip.dim { background: transparent; color: #94a3b8; }
  .chip strong { color: #1e293b; }
  .chip-target { background: #ecfeff; color: #0e7490; font-family: 'IBM Plex Mono', monospace; font-weight: 500; text-transform: none; }
  .chip-source-derived { background: #fef3c7; color: #92400e; }
  .chip-source-ai { background: #fce7f3; color: #9d174d; }
  .chip-source-imported { background: #dbeafe; color: #1d4ed8; }
  .status-draft { background: #f3f4f6; color: #6b7280; }
  .status-staged { background: #fef3c7; color: #92400e; }
  .status-published { background: #dcfce7; color: #166534; }
  .status-deprecated { background: #fee2e2; color: #991b1b; }
  .meta-actions { display: flex; gap: 0.4rem; flex-shrink: 0; flex-wrap: wrap; justify-content: flex-end; }

  .meta-verdict { display: inline-flex; align-items: center; gap: 0.4rem; font-weight: 600; font-size: 0.9rem; color: #991b1b; margin: 0 0 0.6rem; }
  .meta-verdict.ok { color: #166534; }
  .meta-table { width: 100%; border-collapse: collapse; font-size: 0.78rem; }
  .meta-table th, .meta-table td { text-align: left; padding: 0.3rem 0.45rem; border-bottom: 1px solid var(--line-soft, #e5e7eb); vertical-align: top; }
  .sev { padding: 0.05rem 0.35rem; border-radius: 4px; font-size: 0.7rem; text-transform: capitalize; }
  .sev-violation { background: #fee2e2; color: #991b1b; }
  .sev-warning { background: #fef3c7; color: #92400e; }
  .sev-info { background: #dbeafe; color: #1e40af; }

  .meta-edit { display: flex; flex-direction: column; gap: 0.5rem; }
  .meta-edit input, .meta-edit textarea { padding: 0.4rem 0.55rem; font-size: 0.88rem; border: 1px solid var(--line-soft); border-radius: 8px; }
  .name-input { font-weight: 600; font-size: 1rem; }
  .meta-edit-row { display: flex; gap: 0.5rem; align-items: center; }

  .editor-host { padding: 1rem !important; }

  .modal-backdrop { position: fixed; inset: 0; background: rgba(15,23,42,0.45); display: grid; place-items: center; z-index: 100; }
  .modal { background: #fff; border-radius: 14px; width: min(560px, 92vw); max-height: 80vh; display: flex; flex-direction: column; box-shadow: 0 20px 50px rgba(15,23,42,0.25); }
  .modal-wide { width: min(820px, 94vw); max-height: 86vh; }
  .modal-head { display: flex; align-items: center; justify-content: space-between; padding: 0.85rem 1rem; border-bottom: 1px solid var(--line-soft); }
  .modal-head h3 { margin: 0; font-size: 1rem; display: inline-flex; align-items: center; gap: 0.4rem; }
  .modal-body { padding: 0.6rem 1rem 1rem; overflow: auto; }
  .icon-btn { display: grid; place-items: center; width: 26px; height: 26px; border-radius: 7px; border: 1px solid transparent; background: transparent; color: #64748b; cursor: pointer; }
  .icon-btn:hover { background: #f1f5f9; color: #334155; }
  .rev-list { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 0.35rem; }
  .rev-row { display: flex; align-items: center; gap: 0.6rem; padding: 0.45rem 0.55rem; border: 1px solid var(--line-soft); border-radius: 8px; }
  .rev-num { font-family: 'IBM Plex Mono', monospace; font-weight: 700; color: #2F7A8C; min-width: 2.4rem; }
  .rev-note { flex: 1; font-size: 0.85rem; color: #334155; }
  .rev-time { font-size: 0.75rem; color: #94a3b8; }
  .btn-xs { font-size: 0.72rem; padding: 0.2rem 0.5rem; }
  .dim { color: #94a3b8; }

  :global(:is([data-theme="dark"], .dark)) .error { color: #fca5a5; background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark) .editor-page .back) { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .chip { background: rgba(255,255,255,0.06); color: var(--ink-400); }
  :global(:is([data-theme="dark"], .dark)) .chip strong { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .chip-target { background: var(--brand-100); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .impact { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .chip-applied { background: rgba(34,197,94,0.18); color: #86efac; }
  :global(:is([data-theme="dark"], .dark)) .chip-source-derived { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .chip-source-ai { background: rgba(236,72,153,0.2); color: #f9a8d4; }
  :global(:is([data-theme="dark"], .dark)) .chip-source-imported { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .modal { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn:hover { background: rgba(255,255,255,0.06); color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .rev-num { color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .rev-note { color: var(--ink-800); }
</style>
