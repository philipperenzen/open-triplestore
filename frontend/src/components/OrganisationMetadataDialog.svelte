<script>
  import { createEventDispatcher, tick } from 'svelte';
  import { t } from 'svelte-i18n';
  import { sanitizeHtml } from '../lib/ontology/sanitizeHtml.js';
  import { Check, X as XIcon, Loader2, Info, Image as ImageIcon, ImagePlus, Trash2, AlertTriangle } from 'lucide-svelte';
  import Select from './Select.svelte';

  export let open = false;
  /** @type {Record<string, any> | null} */
  export let organisation = null;
  export let saving = false;
  export let error = '';
  /** All organisations the user can see — used to populate the parent selector. */
  export let organisations = [];
  /** Appearance: current icon/logo + banner state, owned by the parent. */
  export let imageUrl = '';
  export let hasImage = false;
  export let uploadingImage = false;
  export let bannerUrl = '';
  export let hasBanner = false;
  export let uploadingBanner = false;
  /** Set by the parent while a delete is in flight. */
  export let deleting = false;

  const dispatch = createEventDispatcher();

  // Danger zone: a deliberately friction-y delete that requires retyping the name.
  let deleteArmed = false;
  let deleteConfirmText = '';
  let imageInputEl;
  let bannerInputEl;
  $: deleteName = organisation?.name || '';
  $: canDelete = deleteArmed && deleteConfirmText.trim() === deleteName && !deleting;

  function onImagePick(e) {
    const f = e.target.files?.[0];
    if (f) dispatch('uploadImage', { file: f });
    if (imageInputEl) imageInputEl.value = '';
  }
  function onBannerPick(e) {
    const f = e.target.files?.[0];
    if (f) dispatch('uploadBanner', { file: f });
    if (bannerInputEl) bannerInputEl.value = '';
  }
  function requestDelete() {
    if (canDelete) dispatch('delete');
  }

  let name = '';
  let description = '';
  let orgType = 'FormalOrganization';
  let identifier = '';
  let contactName = '';
  let contactEmail = '';
  let contactUrl = '';
  let homepage = '';
  let parentOrgId = '';

  // Candidate parents: every other organisation (exclude self). The backend
  // additionally rejects descendants to prevent cycles.
  $: parentCandidates = (organisations || [])
    .filter(o => o.id !== organisation?.id)
    .sort((a, b) => (a.name || '').localeCompare(b.name || ''));

  let dialogEl;
  let prevOpen = false;

  // Re-seed from organisation every time the dialog transitions to open.
  // Keep the read and write of `prevOpen` in ONE reactive block: a split
  // `$: prevOpen = open` gets topologically sorted before the guard (which
  // depends on prevOpen), so prevOpen would already be true and seed() would
  // never run — leaving the form blank and Save disabled.
  $: {
    if (open && !prevOpen && organisation) seed(organisation);
    prevOpen = open;
  }

  function seed(o) {
    name        = o.name        || '';
    description = o.description || '';
    orgType     = o.org_type    || 'FormalOrganization';
    identifier  = o.identifier  || '';
    contactName = o.contact_name  || '';
    contactEmail = o.contact_email || '';
    contactUrl  = o.contact_url  || '';
    homepage    = o.homepage    || '';
    parentOrgId = o.parent_org_id || '';
    deleteArmed = false;
    deleteConfirmText = '';
    tick().then(() => dialogEl?.focus());
  }

  function close() {
    if (saving || deleting) return;
    deleteArmed = false;
    deleteConfirmText = '';
    dispatch('close');
  }

  function onBackdropKey(e) {
    if (e.key === 'Escape') close();
  }

  function save() {
    dispatch('save', {
      name,
      description:   description   || null,
      org_type:      orgType        || null,
      identifier:    identifier     || null,
      contact_name:  contactName    || null,
      contact_email: contactEmail   || null,
      contact_url:   contactUrl     || null,
      homepage:      homepage       || null,
      parent_org_id: parentOrgId    || null,
    });
  }
</script>

{#if open}
  <div class="modal-backdrop" on:click={close} on:keydown={onBackdropKey} role="presentation">
    <div
      class="modal-box"
      bind:this={dialogEl}
      on:click|stopPropagation
      on:keydown|stopPropagation={onBackdropKey}
      role="dialog"
      aria-modal="true"
      aria-label={$t('components.organisationMetadataDialog.title')}
      tabindex="-1"
    >
      <div class="modal-header">
        <h3>{$t('components.organisationMetadataDialog.title')}</h3>
        <button class="icon-btn" on:click={close} aria-label={$t('system.close')}><XIcon size={16} /></button>
      </div>

      <div class="modal-body">
        {#if error}<p class="err">{error}</p>{/if}

        <!-- Basics -->
        <section class="section">
          <h4 class="section-title">{$t('components.organisationMetadataDialog.basics')}</h4>

          <div class="form-row">
            <label for="om-name">
              {$t('components.organisationMetadataDialog.name')}
              <span class="help" title={$t('components.organisationMetadataDialog.nameHelp')}><Info size={12} /></span>
            </label>
            <input id="om-name" bind:value={name} placeholder={$t('components.organisationMetadataDialog.namePlaceholder')} />
          </div>

          <div class="form-row">
            <label for="om-slug">
              {$t('components.organisationMetadataDialog.slug')}
              <span class="help" title={$t('components.organisationMetadataDialog.slugHelp')}><Info size={12} /></span>
            </label>
            <input
              id="om-slug"
              value={organisation?.slug || ''}
              disabled
              class="input-disabled"
              aria-describedby="om-slug-hint"
            />
            <p id="om-slug-hint" class="field-hint-text">{$t('components.organisationMetadataDialog.slugHint')}</p>
          </div>

          <div class="form-row">
            <label for="om-desc">
              {$t('components.organisationMetadataDialog.description')}
              <span class="help" title={$t('components.organisationMetadataDialog.descriptionHelp')}><Info size={12} /></span>
            </label>
            <textarea id="om-desc" bind:value={description} rows="3" placeholder={$t('components.organisationMetadataDialog.descriptionPlaceholder')}></textarea>
          </div>
        </section>

        <!-- Appearance -->
        <section class="section">
          <h4 class="section-title">{$t('components.organisationMetadataDialog.appearance')}</h4>

          <div class="form-row">
            <span class="img-label">
              {$t('components.organisationMetadataDialog.iconLogo')}
              <span class="help" title={$t('components.organisationMetadataDialog.iconLogoHelp')}><Info size={12} /></span>
            </span>
            <div class="img-setting">
              <div class="img-thumb img-thumb-icon">
                {#if hasImage && imageUrl}
                  <img src={imageUrl} alt={$t('components.organisationMetadataDialog.logoAlt')} />
                {:else}
                  <ImageIcon size={20} />
                {/if}
              </div>
              <label class="btn btn-sm btn-ghost img-pick">
                {#if uploadingImage}<Loader2 size={13} class="animate-spin" />{:else}<ImagePlus size={13} />{/if}
                {hasImage ? $t('components.organisationMetadataDialog.replaceLogo') : $t('components.organisationMetadataDialog.uploadLogo')}
                <input bind:this={imageInputEl} type="file" accept="image/*" on:change={onImagePick} style="display:none" />
              </label>
            </div>
          </div>

          <div class="form-row">
            <span class="img-label">
              {$t('components.organisationMetadataDialog.banner')}
              <span class="help" title={$t('components.organisationMetadataDialog.bannerHelp')}><Info size={12} /></span>
            </span>
            <div class="img-setting img-setting-banner">
              <div class="img-thumb img-thumb-banner">
                {#if hasBanner && bannerUrl}
                  <img src={bannerUrl} alt={$t('components.organisationMetadataDialog.bannerAlt')} />
                {:else}
                  <ImageIcon size={20} />
                {/if}
              </div>
              <label class="btn btn-sm btn-ghost img-pick">
                {#if uploadingBanner}<Loader2 size={13} class="animate-spin" />{:else}<ImagePlus size={13} />{/if}
                {hasBanner ? $t('components.organisationMetadataDialog.replaceBanner') : $t('components.organisationMetadataDialog.uploadBanner')}
                <input bind:this={bannerInputEl} type="file" accept="image/*" on:change={onBannerPick} style="display:none" />
              </label>
            </div>
          </div>
        </section>

        <!-- Classification -->
        <section class="section">
          <h4 class="section-title">{$t('components.organisationMetadataDialog.classification')}</h4>

          <div class="form-row">
            <label for="om-type">
              {$t('components.organisationMetadataDialog.orgType')}
              <span class="help" title={$t('components.organisationMetadataDialog.orgTypeHelp')}><Info size={12} /></span>
            </label>
            <Select id="om-type" bind:value={orgType} options={[
              { value: 'FormalOrganization', label: $t('components.organisationMetadataDialog.orgTypeFormal') },
              { value: 'OrganizationalUnit', label: $t('components.organisationMetadataDialog.orgTypeUnit') },
              { value: 'Organization', label: $t('components.organisationMetadataDialog.orgTypeGeneral') },
            ]} />
          </div>

          <div class="form-row">
            <label for="om-id">
              {$t('components.organisationMetadataDialog.identifier')}
              <span class="help" title={$t('components.organisationMetadataDialog.identifierHelp')}><Info size={12} /></span>
            </label>
            <input id="om-id" bind:value={identifier} placeholder={$t('components.organisationMetadataDialog.identifierPlaceholder')} />
          </div>

          <div class="form-row">
            <label for="om-parent">
              {$t('components.organisationMetadataDialog.parentOrg')}
              <span class="help" title={$t('components.organisationMetadataDialog.parentOrgHelp')}><Info size={12} /></span>
            </label>
            <Select
              id="om-parent"
              bind:value={parentOrgId}
              options={[{ value: '', label: $t('components.organisationMetadataDialog.parentOrgNone') }, ...parentCandidates.map(o => ({ value: o.id, label: o.name }))]}
            />
            <p class="field-hint-text">{$t('components.organisationMetadataDialog.parentOrgHint')}</p>
          </div>
        </section>

        <!-- Contact & links -->
        <section class="section">
          <h4 class="section-title">{$t('components.organisationMetadataDialog.contactLinks')}</h4>

          <div class="form-row">
            <label for="om-homepage">
              {$t('components.organisationMetadataDialog.homepage')}
              <span class="help" title={$t('components.organisationMetadataDialog.homepageHelp')}><Info size={12} /></span>
            </label>
            <input id="om-homepage" bind:value={homepage} placeholder="https://www.example.org" />
          </div>

          <div class="form-row two-col">
            <div>
              <label for="om-cname">
                {$t('components.organisationMetadataDialog.contactName')}
                <span class="help" title={$t('components.organisationMetadataDialog.contactNameHelp')}><Info size={12} /></span>
              </label>
              <input id="om-cname" bind:value={contactName} placeholder={$t('components.organisationMetadataDialog.contactNamePlaceholder')} />
            </div>
            <div>
              <label for="om-cemail">
                {$t('components.organisationMetadataDialog.contactEmail')}
                <span class="help" title={$t('components.organisationMetadataDialog.contactEmailHelp')}><Info size={12} /></span>
              </label>
              <input id="om-cemail" type="email" bind:value={contactEmail} placeholder="info@example.org" />
            </div>
          </div>

          <div class="form-row">
            <label for="om-curl">
              {$t('components.organisationMetadataDialog.contactUrl')}
              <span class="help" title={$t('components.organisationMetadataDialog.contactUrlHelp')}><Info size={12} /></span>
            </label>
            <input id="om-curl" bind:value={contactUrl} placeholder="https://www.example.org/contact" />
          </div>
        </section>

        <!-- Danger zone -->
        <section class="section danger-zone">
          <h4 class="section-title danger-title"><AlertTriangle size={13} /> {$t('components.organisationMetadataDialog.dangerZone')}</h4>
          {#if !deleteArmed}
            <div class="danger-row">
              <div class="danger-copy">
                <strong>{$t('components.organisationMetadataDialog.deleteThis')}</strong>
                <p>{$t('components.organisationMetadataDialog.deleteDesc')}</p>
              </div>
              <button type="button" class="btn btn-sm btn-danger" on:click={() => deleteArmed = true} disabled={saving || deleting}>
                <Trash2 size={13} /> {$t('components.organisationMetadataDialog.deleteOrg')}
              </button>
            </div>
          {:else}
            <div class="danger-confirm">
              <p class="danger-warn">
                <!-- eslint-disable-next-line svelte/no-at-html-tags -- DOMPurify-sanitized -->
                {@html sanitizeHtml($t('components.organisationMetadataDialog.deleteConfirmWarn', { values: { name: `<strong>${deleteName}</strong>` } }))}
              </p>
              <input
                class="danger-input"
                bind:value={deleteConfirmText}
                placeholder={deleteName}
                autocomplete="off"
                spellcheck="false"
                aria-label={$t('components.organisationMetadataDialog.deleteConfirmAria')}
              />
              <div class="danger-actions">
                <button type="button" class="btn btn-sm btn-ghost" on:click={() => { deleteArmed = false; deleteConfirmText = ''; }} disabled={deleting}>{$t('system.cancel')}</button>
                <button type="button" class="btn btn-sm btn-danger" on:click={requestDelete} disabled={!canDelete}>
                  {#if deleting}<Loader2 size={13} class="animate-spin" /> {$t('components.organisationMetadataDialog.deleting')}{:else}<Trash2 size={13} /> {$t('components.organisationMetadataDialog.deletePermanently')}{/if}
                </button>
              </div>
            </div>
          {/if}
        </section>
      </div>

      <div class="modal-footer">
        <button class="btn btn-ghost" on:click={close} disabled={saving}>{$t('system.cancel')}</button>
        <button class="btn" on:click={save} disabled={saving || !name.trim()}>
          {#if saving}<Loader2 size={14} class="animate-spin" /> {$t('components.organisationMetadataDialog.saving')}{:else}<Check size={14} /> {$t('system.save')}{/if}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed; inset: 0;
    background: rgba(0,0,0,0.4);
    display: flex; align-items: center; justify-content: center;
    z-index: 200;
    padding: 1.5rem;
  }
  .modal-box {
    background: white;
    border-radius: 1rem;
    width: min(600px, calc(100vw - 2rem));
    max-height: min(90vh, 820px);
    display: flex; flex-direction: column;
    box-shadow: 0 20px 60px rgba(0,0,0,0.18);
    overflow: hidden;
    outline: none;
  }
  .modal-header {
    display: flex; align-items: center; justify-content: space-between;
    padding: 1rem 1.25rem 0.75rem;
    border-bottom: 1px solid var(--line-soft, #e5e7eb);
    flex-shrink: 0;
  }
  .modal-header h3 { margin: 0; font-size: 1rem; }
  .modal-body { padding: 1rem 1.25rem; overflow-y: auto; flex: 1; }
  .modal-footer {
    display: flex; justify-content: flex-end; gap: 0.5rem;
    padding: 0.75rem 1.25rem;
    border-top: 1px solid var(--line-soft, #e5e7eb);
    flex-shrink: 0;
  }
  .icon-btn {
    background: transparent; border: none; cursor: pointer;
    color: var(--ink-400, #888); padding: 0.25rem; border-radius: 4px;
    display: inline-flex; align-items: center;
  }
  .icon-btn:hover { background: var(--bg-subtle, #f3f4f6); color: var(--ink-700, #333); }

  .err {
    background: #fef2f2; color: #991b1b;
    border: 1px solid #fecaca; border-radius: 6px;
    padding: 0.5rem 0.75rem; font-size: 0.85rem; margin: 0 0 0.75rem;
  }

  .section { margin-bottom: 1.25rem; }
  .section + .section { padding-top: 1rem; border-top: 1px dashed var(--line-soft, #e5e7eb); }
  .section-title {
    font-size: 0.78rem; font-weight: 700;
    color: var(--ink-400, #888);
    text-transform: uppercase; letter-spacing: 0.06em;
    margin: 0 0 0.6rem;
  }

  .form-row { display: flex; flex-direction: column; gap: 0.3rem; margin-bottom: 0.75rem; }
  .form-row label {
    font-size: 0.85rem; font-weight: 600; color: var(--ink-700, #333);
    display: inline-flex; align-items: center; gap: 0.3rem;
  }
  .form-row input, .form-row textarea {
    width: 100%; box-sizing: border-box;
    font-size: 0.88rem;
    padding: 0.45rem 0.6rem;
    border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 6px;
    background: white;
  }
  .form-row textarea { resize: vertical; min-height: 72px; }

  .input-disabled {
    background: var(--bg-subtle, #f3f4f6) !important;
    color: var(--ink-400, #9ca3af) !important;
    cursor: not-allowed;
  }

  .two-col { display: grid; grid-template-columns: 1fr 1fr; gap: 0.6rem; }
  .two-col > div { display: flex; flex-direction: column; gap: 0.3rem; }

  .help {
    display: inline-flex; align-items: center; justify-content: center;
    color: var(--ink-400, #999); cursor: help;
  }

  .field-hint-text { font-size: 0.78rem; color: var(--ink-400, #888); margin: 0.25rem 0 0; }

  /* Appearance (icon + banner) */
  .img-label {
    font-size: 0.85rem; font-weight: 600; color: var(--ink-700, #333);
    display: inline-flex; align-items: center; gap: 0.3rem;
  }
  .img-setting { display: flex; align-items: center; gap: 0.7rem; }
  .img-setting-banner { align-items: stretch; }
  .img-thumb {
    flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    background: var(--bg-subtle, #f3f4f6);
    border: 1px solid var(--line-soft, #e5e7eb);
    border-radius: 8px;
    color: var(--ink-400, #9ca3af);
    overflow: hidden;
  }
  .img-thumb img { width: 100%; height: 100%; object-fit: cover; }
  .img-thumb-icon { width: 48px; height: 48px; }
  .img-thumb-banner { width: 140px; height: 48px; }
  .img-pick { white-space: nowrap; }

  /* Danger zone */
  .danger-zone { border-top: 1px solid #fecaca; }
  .danger-title { color: #b91c1c; display: inline-flex; align-items: center; gap: 0.35rem; }
  .danger-row { display: flex; align-items: center; justify-content: space-between; gap: 1rem; }
  .danger-copy strong { font-size: 0.88rem; color: var(--ink-700, #333); }
  .danger-copy p { margin: 0.15rem 0 0; font-size: 0.8rem; color: var(--ink-500, #6b7280); }
  .danger-confirm {
    background: #fef2f2; border: 1px solid #fecaca; border-radius: 8px;
    padding: 0.75rem 0.85rem;
  }
  .danger-warn { margin: 0 0 0.5rem; font-size: 0.82rem; color: #991b1b; line-height: 1.4; }
  .danger-input {
    width: 100%; box-sizing: border-box; font-size: 0.88rem;
    padding: 0.45rem 0.6rem; border: 1px solid #fca5a5; border-radius: 6px; background: white;
  }
  .danger-input:focus { outline: none; border-color: #ef4444; box-shadow: 0 0 0 3px rgba(239,68,68,0.15); }
  .danger-actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.6rem; }

  /* .btn / .btn-ghost / .btn-danger styles come from global app.css */

  :global(:is([data-theme="dark"], .dark)) .modal-box { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .form-row input,
  :global(:is([data-theme="dark"], .dark)) .form-row textarea { background: var(--bg-soft); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .err { background: rgba(220,38,38,0.12); color: #fca5a5; border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .danger-zone { border-top-color: rgba(239,68,68,0.35); }
  :global(:is([data-theme="dark"], .dark)) .danger-title { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .danger-confirm { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .danger-warn { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .danger-input { background: var(--bg-soft); color: var(--ink-900); border-color: rgba(239,68,68,0.5); }
</style>
