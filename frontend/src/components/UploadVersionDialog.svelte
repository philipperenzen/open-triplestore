<script>
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { sanitizeHtml } from '../lib/ontology/sanitizeHtml.js';
  import { Loader2, Upload, X, FileText, Eye, EyeOff, AlertCircle, CheckCircle2, Globe, Lock } from 'lucide-svelte';

  export let id;
  /** Function with signature (id, file, versionOverride, notes, merge, isPublic) => Promise */
  export let uploadFn;
  /** 'vocabulary' | 'model' | 'ontology' — controls dialog title */
  export let kind = 'ontology';

  $: kindLabel = kind === 'vocabulary' ? $t('components.uploadVersionDialog.kindVocabulary') : kind === 'model' ? $t('components.uploadVersionDialog.kindModel') : $t('components.uploadVersionDialog.kindOntology');

  const dispatch = createEventDispatcher();

  let file = null;
  let fileText = '';
  let detectedVersion = null;
  let versionOverride = '';
  let notes = '';
  let merge = false;
  let isPublic = false;
  let showPreview = false;
  let loading = false;
  let error = '';

  const MAX_PREVIEW_CHARS = 2000;

  function extractVersionFromRdf(text) {
    // Turtle / TriG: owl:versionInfo "1.0.0"
    let m = text.match(/owl:versionInfo\s+"([^"]+)"/);
    if (m) return m[1].trim();
    // RDF/XML: <owl:versionInfo>1.0.0</owl:versionInfo>
    m = text.match(/<owl:versionInfo[^>]*>\s*([^<]+?)\s*<\/owl:versionInfo>/);
    if (m) return m[1].trim();
    // JSON-LD: "owl:versionInfo": "1.0.0"
    m = text.match(/"owl:versionInfo"\s*:\s*"([^"]+)"/);
    if (m) return m[1].trim();
    return null;
  }

  async function handleFile(e) {
    const selected = e.target.files?.[0] || null;
    file = selected;
    fileText = '';
    detectedVersion = null;
    versionOverride = '';
    if (!selected) return;

    try {
      const text = await selected.text();
      fileText = text;
      detectedVersion = extractVersionFromRdf(text);
      if (detectedVersion) versionOverride = detectedVersion;
    } catch {
      // Binary or unreadable file — skip preview / version parsing
    }
  }

  $: versionRequired = file && !detectedVersion;
  $: canSubmit = file && (!versionRequired || versionOverride.trim().length > 0);

  async function handleSubmit() {
    if (!file) { error = $t('components.uploadVersionDialog.errorNoFile'); return; }
    if (versionRequired && !versionOverride.trim()) { error = $t('components.uploadVersionDialog.errorVersionRequired'); return; }
    error = '';
    loading = true;
    try {
      await uploadFn(id, file, versionOverride.trim() || null, notes.trim() || null, merge, isPublic);
      dispatch('uploaded');
    } catch (e) {
      error = e.message;
    }
    loading = false;
  }

  function formatBytes(n) {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / (1024 * 1024)).toFixed(1)} MB`;
  }

  $: previewText = fileText ? fileText.slice(0, MAX_PREVIEW_CHARS) : '';
  $: previewTruncated = fileText.length > MAX_PREVIEW_CHARS;
</script>

<div class="modal-backdrop" on:click={() => dispatch('cancel')} role="presentation" on:keydown={(e) => e.key === 'Escape' && dispatch('cancel')}>
  <div class="modal-box" on:click|stopPropagation on:keydown|stopPropagation role="dialog" aria-modal="true" aria-label={$t('components.uploadVersionDialog.dialogAriaLabel', { values: { kind: kindLabel.toLowerCase() } })} tabindex="-1">

    <!-- Header -->
    <div class="modal-header">
      <div>
        <h3 class="modal-title">{$t('components.uploadVersionDialog.title', { values: { kind: kindLabel } })}</h3>
        <p class="modal-subtitle">{$t('components.uploadVersionDialog.subtitle', { values: { kind: kindLabel.toLowerCase() } })}</p>
      </div>
      <button class="icon-btn" type="button" on:click={() => dispatch('cancel')} aria-label={$t('system.close')}>
        <X size={18} />
      </button>
    </div>

    <form on:submit|preventDefault={handleSubmit} class="modal-body">

      <!-- ① File picker -->
      <div class="field">
        <label class="label" for="upload-file">{$t('components.uploadVersionDialog.fileLabel')} <span class="req">*</span></label>
        <label class="dropzone" class:dropzone-filled={!!file}>
          <input
            id="upload-file"
            type="file"
            accept=".ttl,.nt,.nq,.trig,.rdf,.owl,.jsonld"
            on:change={handleFile}
            class="visually-hidden"
          />
          {#if file}
            <FileText size={20} class="dropzone-icon text-brand-500" />
            <span class="dropzone-name">{file.name}</span>
            <span class="dropzone-meta">{$t('components.uploadVersionDialog.dropzoneReplace', { values: { size: formatBytes(file.size) } })}</span>
          {:else}
            <Upload size={20} class="dropzone-icon text-ink-300" />
            <span class="dropzone-name text-ink-400">{$t('components.uploadVersionDialog.dropzonePrompt')}</span>
            <span class="dropzone-meta">Turtle · N-Triples · N-Quads · TriG · RDF/XML · OWL/XML · JSON-LD</span>
          {/if}
        </label>
      </div>

      <!-- ② File preview + version detection — only after file selected -->
      {#if file}
        <!-- Version detection banner -->
        <div class="version-banner" class:version-found={!!detectedVersion} class:version-missing={!detectedVersion}>
          {#if detectedVersion}
            <CheckCircle2 size={15} class="shrink-0" />
            <!-- eslint-disable-next-line svelte/no-at-html-tags -- DOMPurify-sanitized -->
            <span>{@html sanitizeHtml($t('components.uploadVersionDialog.versionDetected', { values: { version: detectedVersion } }))}</span>
          {:else}
            <AlertCircle size={15} class="shrink-0" />
            <span>{$t('components.uploadVersionDialog.versionNotDetected')}</span>
          {/if}
        </div>

        <!-- Preview toggle -->
        {#if previewText}
          <div class="preview-toggle-row">
            <button type="button" class="preview-toggle-btn" on:click={() => showPreview = !showPreview}>
              {#if showPreview}<EyeOff size={13} class="shrink-0" />{:else}<Eye size={13} class="shrink-0" />{/if}
              {showPreview ? $t('components.uploadVersionDialog.hideContents') : $t('components.uploadVersionDialog.previewContents')}
            </button>
          </div>
          {#if showPreview}
            <div class="preview-box">
              <pre class="preview-pre">{previewText}</pre>
              {#if previewTruncated}
                <div class="preview-truncated">{$t('components.uploadVersionDialog.previewTruncated', { values: { count: MAX_PREVIEW_CHARS } })}</div>
              {/if}
            </div>
          {/if}
        {/if}

        <!-- ③ Version field -->
        <div class="field">
          <label class="label" for="upload-ver">
            {$t('components.uploadVersionDialog.versionLabel')}
            {#if detectedVersion}
              <span class="label-hint">{$t('components.uploadVersionDialog.versionOverrideHint')}</span>
            {:else}
              <span class="req">*</span>
            {/if}
          </label>
          <input
            id="upload-ver"
            type="text"
            class="input"
            class:input-required={versionRequired && !versionOverride.trim()}
            bind:value={versionOverride}
            placeholder={detectedVersion ? $t('components.uploadVersionDialog.versionPlaceholderDetected', { values: { version: detectedVersion } }) : $t('components.uploadVersionDialog.versionPlaceholder')}
            required={versionRequired}
          />
          <p class="field-hint">
            {#if detectedVersion}
              {$t('components.uploadVersionDialog.versionHintDetected')}
            {:else}
              <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
              {@html $t('components.uploadVersionDialog.versionHintRequired')}
            {/if}
          </p>
        </div>

        <!-- ④ Release notes -->
        <div class="field">
          <label class="label" for="upload-notes">{$t('components.uploadVersionDialog.notesLabel')} <span class="label-hint">{$t('components.uploadVersionDialog.optional')}</span></label>
          <textarea
            id="upload-notes"
            class="input resize-none"
            rows="2"
            bind:value={notes}
            placeholder={$t('components.uploadVersionDialog.notesPlaceholder')}
          ></textarea>
        </div>

        <!-- ⑤ Toggles -->
        <div class="toggles-grid">
          <div class="toggle-row">
            <div class="toggle-info">
              <span class="toggle-label">{$t('components.uploadVersionDialog.mergeLabel')}</span>
              <span class="toggle-desc">{$t('components.uploadVersionDialog.mergeDesc')}</span>
            </div>
            <button
              type="button"
              class="ios-toggle"
              class:ios-toggle-on={merge}
              role="switch"
              aria-checked={merge}
              on:click={() => merge = !merge}
              aria-label={$t('components.uploadVersionDialog.mergeLabel')}
            >
              <span class="ios-thumb"></span>
            </button>
          </div>

          <div class="toggle-row">
            <div class="toggle-info">
              <span class="toggle-label">
                {#if isPublic}
                  <Globe size={13} class="inline-icon text-green-600" /> {$t('components.uploadVersionDialog.publishedLabel')}
                {:else}
                  <Lock size={13} class="inline-icon text-ink-400" /> {$t('components.uploadVersionDialog.draftLabel')}
                {/if}
              </span>
              <span class="toggle-desc">
                {isPublic ? $t('components.uploadVersionDialog.publishedDesc') : $t('components.uploadVersionDialog.draftDesc')}
              </span>
            </div>
            <button
              type="button"
              class="ios-toggle"
              class:ios-toggle-on={isPublic}
              role="switch"
              aria-checked={isPublic}
              on:click={() => isPublic = !isPublic}
              aria-label={$t('components.uploadVersionDialog.makePublicAria')}
            >
              <span class="ios-thumb"></span>
            </button>
          </div>
        </div>
      {/if}

      {#if error}
        <div class="error-row">
          <AlertCircle size={14} class="shrink-0" />
          {error}
        </div>
      {/if}

      <!-- Footer -->
      <div class="modal-footer">
        <button type="button" class="btn btn-ghost" on:click={() => dispatch('cancel')}>{$t('system.cancel')}</button>
        <button type="submit" class="btn btn-primary" disabled={loading || !canSubmit}>
          {#if loading}<Loader2 size={14} class="animate-spin shrink-0" />{:else}<Upload size={14} class="shrink-0" />{/if}
          {isPublic ? $t('components.uploadVersionDialog.uploadAndPublish') : $t('components.uploadVersionDialog.uploadAsDraft')}
        </button>
      </div>
    </form>
  </div>
</div>

<style>
  /* ── Layout ── */
  .modal-backdrop {
    position: fixed; inset: 0;
    background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 50;
    padding: 1rem;
  }
  .modal-box {
    background: white;
    border-radius: 1.25rem;
    width: min(560px, 100%);
    max-height: 90vh;
    overflow-y: auto;
    box-shadow: 0 24px 64px rgba(0,0,0,0.18);
    display: flex; flex-direction: column;
  }
  .modal-header {
    display: flex; align-items: flex-start; justify-content: space-between; gap: 1rem;
    padding: 1.5rem 1.5rem 0;
  }
  .modal-title { font-size: 1.1rem; font-weight: 700; margin: 0; color: var(--ink-900, #0f172a); }
  .modal-subtitle { font-size: 0.8rem; color: var(--ink-400, #94a3b8); margin: 0.2rem 0 0; }
  .modal-body { display: flex; flex-direction: column; gap: 1.25rem; padding: 1.25rem 1.5rem; }
  .modal-footer {
    display: flex; gap: 0.75rem; justify-content: flex-end;
    padding-top: 0.25rem;
    border-top: 1px solid var(--line-soft, #e2e8f0);
  }

  /* ── Fields ── */
  .field { display: flex; flex-direction: column; gap: 0.3rem; }
  .label { font-size: 0.825rem; font-weight: 600; color: var(--ink-700, #334155); }
  .label-hint { font-weight: 400; color: var(--ink-400, #94a3b8); margin-left: 0.25rem; }
  .req { color: #ef4444; }
  .field-hint { font-size: 0.75rem; color: var(--ink-400, #94a3b8); margin: 0; }
  .input {
    width: 100%; padding: 0.5rem 0.75rem;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 0.75rem; font-size: 0.875rem;
    box-sizing: border-box; font-family: inherit;
    transition: border-color 0.15s;
    outline: none;
  }
  .input:focus { border-color: var(--brand-400, #818cf8); box-shadow: 0 0 0 3px rgba(99,102,241,0.12); }
  .input-required { border-color: #fca5a5; }
  .input-required:focus { border-color: #ef4444; box-shadow: 0 0 0 3px rgba(239,68,68,0.1); }

  /* ── Dropzone ── */
  .dropzone {
    display: flex; flex-direction: column; align-items: center; gap: 0.35rem;
    padding: 1.5rem 1rem;
    border: 2px dashed var(--line-soft, #e2e8f0);
    border-radius: 0.875rem;
    background: var(--bg-soft, #f8fafc);
    cursor: pointer;
    transition: all 0.15s;
    text-align: center;
  }
  .dropzone:hover, .dropzone-filled { border-color: var(--brand-300, #a5b4fc); background: var(--bg-accent-soft, #f0f4ff); }
  :global(.dropzone-icon) { margin-bottom: 0.1rem; }
  .dropzone-name { font-size: 0.875rem; font-weight: 500; color: var(--ink-700, #334155); }
  .dropzone-meta { font-size: 0.7rem; color: var(--ink-400, #94a3b8); }
  .visually-hidden { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0,0,0,0); }

  /* ── Version banner ── */
  .version-banner {
    display: flex; align-items: center; gap: 0.5rem;
    padding: 0.6rem 0.875rem; border-radius: 0.75rem;
    font-size: 0.8rem;
  }
  .version-found { background: #dcfce7; color: #15803d; border: 1px solid #bbf7d0; }
  .version-missing { background: #fef9c3; color: #854d0e; border: 1px solid #fde68a; }

  /* ── Preview ── */
  .preview-toggle-row { display: flex; }
  .preview-toggle-btn {
    display: inline-flex; align-items: center; gap: 0.375rem;
    font-size: 0.775rem; color: var(--brand-600, #4f46e5); font-weight: 500;
    background: none; border: none; cursor: pointer; padding: 0;
  }
  .preview-toggle-btn:hover { text-decoration: underline; }
  .preview-box {
    border: 1px solid var(--line-soft, #e2e8f0); border-radius: 0.75rem;
    background: #fafafa; overflow: hidden; max-height: 220px; overflow-y: auto;
  }
  .preview-pre {
    margin: 0; padding: 0.875rem 1rem;
    font-size: 0.72rem; line-height: 1.5;
    font-family: ui-monospace, 'Cascadia Code', 'Fira Code', monospace;
    color: var(--ink-700, #334155);
    white-space: pre-wrap; word-break: break-all;
  }
  .preview-truncated {
    padding: 0.4rem 1rem; font-size: 0.7rem; color: var(--ink-400, #94a3b8);
    background: var(--bg-soft, #f8fafc); border-top: 1px solid var(--line-soft, #e2e8f0);
    font-style: italic;
  }

  /* ── iOS Toggles ── */
  .toggles-grid {
    display: flex; flex-direction: column; gap: 0.75rem;
    padding: 0.875rem 1rem; border-radius: 0.875rem;
    background: var(--bg-soft, #f8fafc);
    border: 1px solid var(--line-soft, #e2e8f0);
  }
  .toggle-row {
    display: flex; align-items: center; justify-content: space-between; gap: 1rem;
  }
  .toggle-info { display: flex; flex-direction: column; gap: 0.15rem; min-width: 0; flex: 1; }
  .toggle-label { font-size: 0.85rem; font-weight: 500; color: var(--ink-800, #1e293b); display: flex; align-items: center; gap: 0.3rem; }
  .toggle-desc { font-size: 0.73rem; color: var(--ink-400, #94a3b8); line-height: 1.35; }

  .ios-toggle {
    position: relative; display: block;
    width: 44px; min-width: 44px; height: 26px;
    border-radius: 13px; border: none; cursor: pointer;
    background: #cbd5e1;
    transition: background 0.22s ease;
    padding: 0; flex-shrink: 0;
  }
  .ios-toggle-on { background: #22c55e; }
  .ios-toggle:focus-visible { outline: 2px solid var(--brand-400, #818cf8); outline-offset: 2px; }

  .ios-thumb {
    position: absolute; top: 3px; left: 3px;
    width: 20px; height: 20px; border-radius: 50%;
    background: white;
    box-shadow: 0 1px 4px rgba(0,0,0,0.25);
    transition: left 0.22s ease;
  }
  .ios-toggle-on .ios-thumb { left: 21px; }

  :global(.inline-icon) { display: inline-block; vertical-align: middle; margin-top: -2px; }

  /* ── Error / Buttons ── */
  .error-row {
    display: flex; align-items: center; gap: 0.5rem;
    padding: 0.6rem 0.875rem; border-radius: 0.75rem;
    background: #fef2f2; border: 1px solid #fecaca;
    font-size: 0.82rem; color: #dc2626;
  }

  .btn {
    display: inline-flex; align-items: center; gap: 0.4rem;
    padding: 0.5rem 1.1rem; border-radius: 0.75rem;
    font-size: 0.875rem; font-weight: 500;
    cursor: pointer; border: none; transition: all 0.15s;
  }
  .btn-primary { background: var(--brand-500, #6366f1); color: white; }
  .btn-primary:hover:not(:disabled) { background: var(--brand-600, #4f46e5); transform: translateY(-1px); }
  .btn-ghost { background: transparent; color: var(--ink-600, #475569); }
  .btn-ghost:hover { background: var(--bg-soft, #f1f5f9); }
  .btn:disabled { opacity: 0.5; cursor: not-allowed; transform: none !important; }
  .icon-btn {
    display: flex; align-items: center; justify-content: center;
    width: 2rem; height: 2rem; border-radius: 0.625rem;
    border: none; cursor: pointer; background: transparent;
    color: var(--ink-400, #94a3b8); transition: all 0.15s;
    flex-shrink: 0;
  }
  .icon-btn:hover { background: var(--bg-soft, #f1f5f9); color: var(--ink-700, #334155); }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .modal-box { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .req { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .version-found { background: rgba(16,185,129,0.18); color: #6ee7b7; border-color: rgba(16,185,129,0.4); }
  :global(:is([data-theme="dark"], .dark)) .version-missing { background: rgba(245,158,11,0.18); color: #fcd34d; border-color: rgba(245,158,11,0.4); }
  :global(:is([data-theme="dark"], .dark)) .preview-box { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .ios-toggle { background: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .error-row { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
</style>
