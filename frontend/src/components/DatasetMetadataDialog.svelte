<script>
  import { createEventDispatcher, tick } from 'svelte';
  import { t as i18nT } from 'svelte-i18n';
  import { Check, X as XIcon, Loader2, Info, ExternalLink, Tag, Image as ImageIcon, ImagePlus, Trash2, AlertTriangle } from 'lucide-svelte';
  import { LICENSES, LICENSE_CATEGORY_LABEL, findLicense, searchLicenses } from '../lib/vocab/licenses';
  import { findTheme, searchThemes, ADMS_STATUSES, findAdmsStatus } from '../lib/vocab/themes';
  import Select from './Select.svelte';
  import BannerPicker from './BannerPicker.svelte';

  export let open = false;
  /** @type {Record<string, any> | null} */
  export let dataset = null;
  export let saving = false;
  export let error = '';
  /** Appearance: current icon/cover + banner state, owned by the parent. */
  export let imageUrl = '';
  export let hasImage = false;
  export let uploadingImage = false;
  export let bannerUrl = '';
  export let uploadingBanner = false;
  /** Live banner_key so the picker can highlight the active preset/upload. */
  export let bannerKey = null;
  /** Set by the parent while a delete is in flight. */
  export let deleting = false;

  const dispatch = createEventDispatcher();

  // Danger zone: a deliberately friction-y delete that requires retyping the name.
  let deleteArmed = false;
  let deleteConfirmText = '';
  let imageInputEl;
  $: deleteName = dataset?.name || '';
  $: canDelete = deleteArmed && deleteConfirmText.trim() === deleteName && !deleting;

  function onImagePick(e) {
    const f = e.target.files?.[0];
    if (f) dispatch('uploadImage', { file: f });
    if (imageInputEl) imageInputEl.value = '';
  }
  function onBannerUpload(e) {
    dispatch('uploadBanner', { file: e.detail.file });
  }
  function onBannerPreset(e) {
    dispatch('selectBannerPreset', { preset: e.detail.preset });
  }
  function onBannerClear() {
    dispatch('clearBanner');
  }
  function requestDelete() {
    if (canDelete) dispatch('delete');
  }

  let name = '';
  let description = '';
  let visibility = 'public';
  let licenseIri = '';
  let licenseQuery = '';
  let licenseFocused = false;
  let themes = [];
  let themeQuery = '';
  let themeFocused = false;
  let keywords = [];
  let keywordInput = '';
  let admsStatus = '';
  let admsCustom = false;
  let admsCustomIri = '';
  let versionNotes = '';
  let contactName = '';
  let contactEmail = '';
  let contactUrl = '';
  let spatial = '';
  let landingPage = '';

  let dialogEl;
  let prevOpen = false;

  // Re-seed from dataset every time the dialog transitions to open.
  // Keep the read and write of `prevOpen` in ONE reactive block: a split
  // `$: prevOpen = open` gets topologically sorted before the guard (which
  // depends on prevOpen), so prevOpen would already be true and seed() would
  // never run — leaving the form blank and Save disabled.
  $: {
    if (open && !prevOpen && dataset) seed(dataset);
    prevOpen = open;
  }

  function parseJsonList(s) {
    if (!s) return [];
    try { const v = JSON.parse(s); return Array.isArray(v) ? v : []; } catch { return []; }
  }

  function seed(d) {
    name = d.name || '';
    description = d.description || '';
    visibility = d.visibility || 'public';
    licenseIri = d.license || '';
    licenseQuery = licenseIri ? (findLicense(licenseIri)?.label || licenseIri) : '';
    themes = parseJsonList(d.themes);
    themeQuery = '';
    keywords = parseJsonList(d.keywords);
    keywordInput = '';
    const adms = d.adms_status || '';
    if (adms && findAdmsStatus(adms)) {
      admsStatus = adms;
      admsCustom = false;
      admsCustomIri = '';
    } else if (adms) {
      admsStatus = '__custom__';
      admsCustom = true;
      admsCustomIri = adms;
    } else {
      admsStatus = '';
      admsCustom = false;
      admsCustomIri = '';
    }
    versionNotes = d.version_notes || '';
    contactName = d.contact_name || '';
    contactEmail = d.contact_email || '';
    contactUrl = d.contact_url || '';
    spatial = d.spatial || '';
    landingPage = d.landing_page || '';
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

  $: licenseMatches = searchLicenses(licenseQuery).slice(0, 10);
  $: selectedLicense = findLicense(licenseIri);
  $: themeMatches = searchThemes(themeQuery).filter(t => !themes.includes(t.iri)).slice(0, 10);

  function pickLicense(opt) {
    licenseIri = opt.iri;
    licenseQuery = opt.label;
    licenseFocused = false;
  }
  function clearLicense() {
    licenseIri = '';
    licenseQuery = '';
  }
  function onLicenseInput(e) {
    licenseQuery = e.target.value;
    // Accept whatever the user typed as a custom IRI if no exact match.
    const exact = LICENSES.find(l => l.label === licenseQuery || l.iri === licenseQuery);
    licenseIri = exact ? exact.iri : licenseQuery;
  }

  function addTheme(iri) {
    if (!iri) return;
    if (!themes.includes(iri)) themes = [...themes, iri];
    themeQuery = '';
  }
  function removeTheme(iri) {
    themes = themes.filter(t => t !== iri);
  }
  function onThemeKey(e) {
    if (e.key === 'Enter' && themeQuery.trim()) {
      e.preventDefault();
      const match = themeMatches[0];
      addTheme(match ? match.iri : themeQuery.trim());
    } else if (e.key === 'Backspace' && !themeQuery && themes.length) {
      themes = themes.slice(0, -1);
    }
  }

  function addKeyword() {
    const v = keywordInput.trim();
    if (!v) return;
    if (!keywords.includes(v)) keywords = [...keywords, v];
    keywordInput = '';
  }
  function removeKeyword(k) {
    keywords = keywords.filter(x => x !== k);
  }
  function onKeywordKey(e) {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      addKeyword();
    } else if (e.key === 'Backspace' && !keywordInput && keywords.length) {
      keywords = keywords.slice(0, -1);
    }
  }

  function onAdmsChange(v) {
    if (v === '__custom__') {
      admsCustom = true;
      admsStatus = '__custom__';
    } else {
      admsCustom = false;
      admsStatus = v;
      admsCustomIri = '';
    }
  }

  function save() {
    const themesOut = themes.length ? themes : null;
    const keywordsOut = keywords.length ? keywords : null;
    let admsOut = null;
    if (admsCustom) admsOut = admsCustomIri.trim() || null;
    else if (admsStatus) admsOut = admsStatus;
    dispatch('save', {
      name,
      description: description || null,
      visibility,
      license: licenseIri || null,
      themes: themesOut,
      keywords: keywordsOut,
      contact_name: contactName || null,
      contact_email: contactEmail || null,
      contact_url: contactUrl || null,
      adms_status: admsOut,
      version_notes: versionNotes || null,
      spatial: spatial || null,
      landing_page: landingPage || null,
    });
  }
</script>

{#if open}
  <div class="modal-backdrop" on:click={close} on:keydown={onBackdropKey} role="presentation">
    <div
      class="modal-box modal-lg"
      bind:this={dialogEl}
      on:click|stopPropagation
      on:keydown|stopPropagation={onBackdropKey}
      role="dialog"
      aria-modal="true"
      aria-label={$i18nT('components.datasetMetadataDialog.dialogTitle')}
      tabindex="-1"
    >
      <div class="modal-header">
        <h3>{$i18nT('components.datasetMetadataDialog.dialogTitle')}</h3>
        <button class="icon-btn" on:click={close} aria-label={$i18nT('system.close')}><XIcon size={16} /></button>
      </div>
      <div class="modal-body">
        {#if error}<p class="err">{error}</p>{/if}

        <!-- Basics -->
        <section class="section">
          <h4 class="section-title">{$i18nT('components.datasetMetadataDialog.sectionBasics')}</h4>
          <div class="form-row">
            <label for="md-name">
              {$i18nT('components.datasetMetadataDialog.nameLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.nameHelp')}><Info size={12} /></span>
            </label>
            <input id="md-name" bind:value={name} placeholder={$i18nT('components.datasetMetadataDialog.namePlaceholder')} />
          </div>
          <div class="form-row">
            <label for="md-desc">
              {$i18nT('components.datasetMetadataDialog.descriptionLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.descriptionHelp')}><Info size={12} /></span>
            </label>
            <textarea id="md-desc" bind:value={description} rows="2" placeholder={$i18nT('components.datasetMetadataDialog.descriptionPlaceholder')}></textarea>
          </div>
          <div class="form-row">
            <label for="md-vis">
              {$i18nT('components.datasetMetadataDialog.visibilityLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.visibilityHelp')}><Info size={12} /></span>
            </label>
            <Select id="md-vis" bind:value={visibility} options={[
              { value: 'public', label: $i18nT('components.datasetMetadataDialog.visibilityPublic') },
              { value: 'members', label: $i18nT('components.datasetMetadataDialog.visibilityMembers') },
              { value: 'private', label: $i18nT('components.datasetMetadataDialog.visibilityPrivate') },
            ]} />
          </div>
        </section>

        <!-- Appearance -->
        <section class="section">
          <h4 class="section-title">{$i18nT('components.datasetMetadataDialog.sectionAppearance')}</h4>

          <div class="form-row">
            <span class="img-label">
              {$i18nT('components.datasetMetadataDialog.iconLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.iconHelp')}><Info size={12} /></span>
            </span>
            <div class="img-setting">
              <div class="img-thumb img-thumb-icon">
                {#if hasImage && imageUrl}
                  <img src={imageUrl} alt={$i18nT('components.datasetMetadataDialog.iconAlt')} />
                {:else}
                  <ImageIcon size={20} />
                {/if}
              </div>
              <label class="btn btn-sm btn-ghost img-pick">
                {#if uploadingImage}<Loader2 size={13} class="animate-spin" />{:else}<ImagePlus size={13} />{/if}
                {hasImage ? $i18nT('components.datasetMetadataDialog.replaceIcon') : $i18nT('components.datasetMetadataDialog.uploadIcon')}
                <input bind:this={imageInputEl} type="file" accept="image/*" on:change={onImagePick} style="display:none" />
              </label>
            </div>
          </div>

          <div class="form-row">
            <span class="img-label">
              {$i18nT('components.datasetMetadataDialog.bannerLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.bannerHelp')}><Info size={12} /></span>
            </span>
            <BannerPicker
              bannerKey={bannerKey}
              imageUrl={bannerUrl}
              uploading={uploadingBanner}
              on:selectPreset={onBannerPreset}
              on:upload={onBannerUpload}
              on:clear={onBannerClear}
            />
          </div>
        </section>

        <!-- Licensing & rights -->
        <section class="section">
          <h4 class="section-title">{$i18nT('components.datasetMetadataDialog.sectionLicensing')}</h4>

          <div class="form-row">
            <label for="md-license">
              {$i18nT('components.datasetMetadataDialog.licenseLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.licenseHelp')}><Info size={12} /></span>
            </label>
            <div class="combo">
              <input
                id="md-license"
                bind:value={licenseQuery}
                on:input={onLicenseInput}
                on:focus={() => licenseFocused = true}
                on:blur={() => setTimeout(() => licenseFocused = false, 150)}
                placeholder={$i18nT('components.datasetMetadataDialog.licensePlaceholder')}
                autocomplete="off"
              />
              {#if licenseIri}
                <button class="combo-clear" on:click={clearLicense} aria-label={$i18nT('components.datasetMetadataDialog.clearLicense')}><XIcon size={12} /></button>
              {/if}
              {#if licenseFocused && licenseMatches.length}
                <ul class="combo-list" role="listbox">
                  {#each licenseMatches as opt (opt.iri)}
                    <li>
                      <button type="button" class="combo-item" on:mousedown|preventDefault={() => pickLicense(opt)}>
                        <span class="combo-label">{opt.label}</span>
                        <span class="combo-cat">{LICENSE_CATEGORY_LABEL[opt.category]}</span>
                        <span class="combo-summary">{opt.summary}</span>
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
            {#if selectedLicense}
              <div class="info-card">
                <div class="info-card-head">
                  <strong>{selectedLicense.label}</strong>
                  <span class="info-card-tag">{LICENSE_CATEGORY_LABEL[selectedLicense.category]}</span>
                  {#if selectedLicense.url}
                    <a href={selectedLicense.url} target="_blank" rel="noopener" class="info-card-link" title={$i18nT('components.datasetMetadataDialog.openLicensePage')}>
                      <ExternalLink size={12} />
                    </a>
                  {/if}
                </div>
                <p class="info-card-summary">{selectedLicense.summary}</p>
              </div>
            {:else if licenseIri}
              <div class="info-card info-card-custom">
                <div class="info-card-head">
                  <strong>{$i18nT('components.datasetMetadataDialog.customLicense')}</strong>
                  <span class="info-card-tag">{$i18nT('components.datasetMetadataDialog.unknown')}</span>
                </div>
                <p class="info-card-summary"><code>{licenseIri}</code></p>
              </div>
            {/if}
          </div>

          <div class="form-row">
            <label for="md-keywords">
              {$i18nT('components.datasetMetadataDialog.keywordsLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.keywordsHelp')}><Info size={12} /></span>
            </label>
            <div class="chips-input">
              {#each keywords as kw}
                <span class="chip">
                  {kw}
                  <button type="button" class="chip-x" on:click={() => removeKeyword(kw)} aria-label={$i18nT('components.datasetMetadataDialog.removeKeyword')}><XIcon size={10} /></button>
                </span>
              {/each}
              <input
                id="md-keywords"
                bind:value={keywordInput}
                on:keydown={onKeywordKey}
                on:blur={addKeyword}
                placeholder={keywords.length ? '' : $i18nT('components.datasetMetadataDialog.keywordsPlaceholder')}
              />
            </div>
          </div>
        </section>

        <!-- Classification -->
        <section class="section">
          <h4 class="section-title">{$i18nT('components.datasetMetadataDialog.sectionClassification')}</h4>

          <div class="form-row">
            <label for="md-themes">
              {$i18nT('components.datasetMetadataDialog.themesLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.themesHelp')}><Info size={12} /></span>
            </label>
            <div class="combo chips-input">
              {#each themes as iri}
                {@const theme = findTheme(iri)}
                <span class="chip" title={theme?.summary || iri}>
                  <Tag size={10} />
                  {theme ? theme.label : iri}
                  {#if !theme}<span class="chip-tag">{$i18nT('components.datasetMetadataDialog.customTag')}</span>{/if}
                  <button type="button" class="chip-x" on:click={() => removeTheme(iri)} aria-label={$i18nT('components.datasetMetadataDialog.removeTheme')}><XIcon size={10} /></button>
                </span>
              {/each}
              <input
                id="md-themes"
                bind:value={themeQuery}
                on:keydown={onThemeKey}
                on:focus={() => themeFocused = true}
                on:blur={() => setTimeout(() => themeFocused = false, 150)}
                placeholder={themes.length ? '' : $i18nT('components.datasetMetadataDialog.themesPlaceholder')}
                autocomplete="off"
              />
              {#if themeFocused && themeMatches.length}
                <ul class="combo-list" role="listbox">
                  {#each themeMatches as opt (opt.iri)}
                    <li>
                      <button type="button" class="combo-item" on:mousedown|preventDefault={() => addTheme(opt.iri)}>
                        <span class="combo-label">{opt.label}</span>
                        <span class="combo-summary">{opt.summary}</span>
                      </button>
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
          </div>

          <div class="form-row">
            <label for="md-adms">
              {$i18nT('components.datasetMetadataDialog.admsLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.admsHelp')}><Info size={12} /></span>
            </label>
            <Select
              id="md-adms"
              value={admsStatus}
              on:change={e => onAdmsChange(e.detail)}
              options={[
                { value: '', label: $i18nT('components.datasetMetadataDialog.admsNotSpecified') },
                ...ADMS_STATUSES.map(opt => ({ value: opt.iri, label: opt.label })),
                { value: '__custom__', label: $i18nT('components.datasetMetadataDialog.admsCustomIri') },
              ]}
            />
            {#if admsCustom}
              <input
                class="follow-up"
                bind:value={admsCustomIri}
                placeholder="http://example.org/status/..."
              />
            {/if}
            {#if admsStatus && !admsCustom}
              {@const s = findAdmsStatus(admsStatus)}
              {#if s}<p class="field-hint-text">{s.summary}</p>{/if}
            {/if}
          </div>

          <div class="form-row">
            <label for="md-vnotes">
              {$i18nT('components.datasetMetadataDialog.versionNotesLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.versionNotesHelp')}><Info size={12} /></span>
            </label>
            <input id="md-vnotes" bind:value={versionNotes} placeholder={$i18nT('components.datasetMetadataDialog.versionNotesPlaceholder')} />
          </div>
        </section>

        <!-- Contact & links -->
        <section class="section">
          <h4 class="section-title">{$i18nT('components.datasetMetadataDialog.sectionContact')}</h4>

          <div class="form-row two-col">
            <div>
              <label for="md-cname">
                {$i18nT('components.datasetMetadataDialog.contactNameLabel')}
                <span class="help" title={$i18nT('components.datasetMetadataDialog.contactNameHelp')}><Info size={12} /></span>
              </label>
              <input id="md-cname" bind:value={contactName} placeholder={$i18nT('components.datasetMetadataDialog.contactNamePlaceholder')} />
            </div>
            <div>
              <label for="md-cemail">
                {$i18nT('components.datasetMetadataDialog.contactEmailLabel')}
                <span class="help" title={$i18nT('components.datasetMetadataDialog.contactEmailHelp')}><Info size={12} /></span>
              </label>
              <input id="md-cemail" type="email" bind:value={contactEmail} placeholder="data@example.org" />
            </div>
          </div>

          <div class="form-row">
            <label for="md-curl">
              {$i18nT('components.datasetMetadataDialog.contactUrlLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.contactUrlHelp')}><Info size={12} /></span>
            </label>
            <input id="md-curl" bind:value={contactUrl} placeholder="https://example.org/contact" />
          </div>

          <div class="form-row">
            <label for="md-spatial">
              {$i18nT('components.datasetMetadataDialog.spatialLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.spatialHelp')}><Info size={12} /></span>
            </label>
            <input id="md-spatial" bind:value={spatial} placeholder="http://www.geonames.org/2635167/united-kingdom.html" />
          </div>

          <div class="form-row">
            <label for="md-landing">
              {$i18nT('components.datasetMetadataDialog.landingPageLabel')}
              <span class="help" title={$i18nT('components.datasetMetadataDialog.landingPageHelp')}><Info size={12} /></span>
            </label>
            <input id="md-landing" bind:value={landingPage} placeholder="https://data.example.org/datasets/xyz" />
          </div>
        </section>

        <!-- Danger zone -->
        <section class="section danger-zone">
          <h4 class="section-title danger-title"><AlertTriangle size={13} /> {$i18nT('components.datasetMetadataDialog.dangerZone')}</h4>
          {#if !deleteArmed}
            <div class="danger-row">
              <div class="danger-copy">
                <strong>{$i18nT('components.datasetMetadataDialog.deleteHeading')}</strong>
                <p>{$i18nT('components.datasetMetadataDialog.deleteDescription')}</p>
              </div>
              <button type="button" class="btn btn-sm btn-danger" on:click={() => deleteArmed = true} disabled={saving || deleting}>
                <Trash2 size={13} /> {$i18nT('components.datasetMetadataDialog.deleteDataset')}
              </button>
            </div>
          {:else}
            <div class="danger-confirm">
              <p class="danger-warn">
                {$i18nT('components.datasetMetadataDialog.deleteConfirmWarning', { values: { name: deleteName } })}
              </p>
              <input
                class="danger-input"
                bind:value={deleteConfirmText}
                placeholder={deleteName}
                autocomplete="off"
                spellcheck="false"
                aria-label={$i18nT('components.datasetMetadataDialog.deleteConfirmAria')}
              />
              <div class="danger-actions">
                <button type="button" class="btn btn-sm btn-ghost" on:click={() => { deleteArmed = false; deleteConfirmText = ''; }} disabled={deleting}>{$i18nT('system.cancel')}</button>
                <button type="button" class="btn btn-sm btn-danger" on:click={requestDelete} disabled={!canDelete}>
                  {#if deleting}<Loader2 size={13} class="animate-spin" /> {$i18nT('components.datasetMetadataDialog.deleting')}{:else}<Trash2 size={13} /> {$i18nT('components.datasetMetadataDialog.deletePermanently')}{/if}
                </button>
              </div>
            </div>
          {/if}
        </section>
      </div>

      <div class="modal-footer">
        <button class="btn btn-ghost" on:click={close} disabled={saving}>{$i18nT('system.cancel')}</button>
        <button class="btn" on:click={save} disabled={saving || !name.trim()}>
          {#if saving}<Loader2 size={14} class="animate-spin" /> {$i18nT('components.datasetMetadataDialog.saving')}{:else}<Check size={14} /> {$i18nT('system.save')}{/if}
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
    width: min(680px, calc(100vw - 2rem));
    max-height: min(90vh, 860px);
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
  .form-row textarea { resize: vertical; min-height: 60px; }
  .form-row .follow-up { margin-top: 0.4rem; }

  .two-col { display: grid; grid-template-columns: 1fr 1fr; gap: 0.6rem; }
  .two-col > div { display: flex; flex-direction: column; gap: 0.3rem; }

  .help {
    display: inline-flex; align-items: center; justify-content: center;
    color: var(--ink-400, #999); cursor: help;
  }

  .field-hint-text { font-size: 0.78rem; color: var(--ink-400, #888); margin: 0.25rem 0 0; }

  /* Combobox */
  .combo { position: relative; }
  .combo-clear {
    position: absolute; right: 0.4rem; top: 50%;
    transform: translateY(-50%);
    background: transparent; border: none; cursor: pointer;
    color: var(--ink-400, #888); padding: 2px; border-radius: 4px;
    display: inline-flex; align-items: center;
  }
  .combo-clear:hover { background: var(--bg-subtle, #f3f4f6); }
  .combo-list {
    position: absolute; left: 0; right: 0; top: calc(100% + 4px);
    list-style: none; margin: 0; padding: 0.25rem;
    background: white;
    border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 8px;
    box-shadow: 0 10px 30px rgba(0,0,0,0.10);
    max-height: 240px; overflow-y: auto;
    z-index: 10;
  }
  .combo-item {
    display: grid;
    grid-template-columns: 1fr auto;
    grid-template-rows: auto auto;
    gap: 0.1rem 0.5rem;
    width: 100%;
    text-align: left;
    background: transparent;
    border: none;
    padding: 0.45rem 0.55rem;
    border-radius: 6px;
    cursor: pointer;
  }
  .combo-item:hover { background: var(--bg-subtle, #f3f4f6); }
  .combo-label { font-size: 0.86rem; font-weight: 600; color: var(--ink-700, #333); }
  .combo-cat {
    font-size: 0.7rem; color: var(--ink-400, #888);
    background: var(--bg-subtle, #f3f4f6); border-radius: 4px;
    padding: 0.05rem 0.4rem; align-self: center;
  }
  .combo-summary {
    grid-column: 1 / -1;
    font-size: 0.78rem; color: var(--ink-500, #6b7280);
    line-height: 1.3;
  }

  /* Chip input (keywords & themes) */
  .chips-input {
    display: flex; flex-wrap: wrap; align-items: center; gap: 0.3rem;
    padding: 0.35rem 0.45rem;
    border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 6px;
    background: white;
    min-height: 38px;
  }
  .chips-input input {
    flex: 1; min-width: 120px;
    border: none !important; padding: 0.15rem 0.2rem !important;
    font-size: 0.86rem;
    background: transparent;
    outline: none;
  }
  .chip {
    display: inline-flex; align-items: center; gap: 0.25rem;
    background: var(--bg-subtle, #eef2f7);
    color: var(--ink-700, #333);
    border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 999px;
    padding: 0.15rem 0.45rem 0.15rem 0.55rem;
    font-size: 0.78rem;
    max-width: 100%;
  }
  .chip-tag {
    font-size: 0.65rem; text-transform: uppercase; letter-spacing: 0.05em;
    color: var(--ink-400, #888);
    border: 1px solid var(--line-soft, #d1d5db);
    padding: 0 0.25rem; border-radius: 4px;
  }
  .chip-x {
    background: transparent; border: none; cursor: pointer;
    color: var(--ink-400, #888);
    padding: 1px; border-radius: 999px;
    display: inline-flex; align-items: center;
  }
  .chip-x:hover { background: rgba(0,0,0,0.08); color: var(--ink-700, #333); }

  /* Combo + chips combined (themes) */
  .combo.chips-input { padding-right: 0.5rem; }

  /* Selected license info card */
  .info-card {
    margin-top: 0.45rem;
    background: var(--bg-subtle, #f8fafc);
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 8px;
    padding: 0.5rem 0.65rem;
  }
  .info-card-custom { border-style: dashed; }
  .info-card-head {
    display: flex; align-items: center; gap: 0.4rem;
    font-size: 0.85rem;
  }
  .info-card-tag {
    font-size: 0.68rem; text-transform: uppercase; letter-spacing: 0.05em;
    background: white; border: 1px solid var(--line-soft, #d1d5db);
    border-radius: 4px; padding: 0.05rem 0.35rem;
    color: var(--ink-400, #888);
  }
  .info-card-link {
    color: var(--brand-600, #4a90d9);
    display: inline-flex; align-items: center;
  }
  .info-card-summary {
    margin: 0.3rem 0 0;
    font-size: 0.8rem; color: var(--ink-600, #475569);
    line-height: 1.4;
  }
  .info-card-summary code {
    font-size: 0.78rem; word-break: break-all;
    background: white; padding: 0.05rem 0.3rem; border-radius: 4px;
    border: 1px solid var(--line-soft, #e5e7eb);
  }

  /* Appearance (icon + banner) */
  .img-label {
    font-size: 0.85rem; font-weight: 600; color: var(--ink-700, #333);
    display: inline-flex; align-items: center; gap: 0.3rem;
  }
  .img-setting { display: flex; align-items: center; gap: 0.7rem; }
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
  :global(:is([data-theme="dark"], .dark)) .err { background: rgba(220,38,38,0.12); color: #fca5a5; border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .form-row input,
  :global(:is([data-theme="dark"], .dark)) .form-row textarea { background: var(--bg-soft); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .combo-list { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .chips-input { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .chip-x:hover { background: rgba(255,255,255,0.1); }
  :global(:is([data-theme="dark"], .dark)) .info-card-tag { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .info-card-summary code { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .danger-zone { border-top-color: rgba(239,68,68,0.35); }
  :global(:is([data-theme="dark"], .dark)) .danger-title { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .danger-confirm { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .danger-warn { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .danger-input { background: var(--bg-soft); color: var(--ink-900); border-color: rgba(239,68,68,0.5); }
</style>
