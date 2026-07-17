<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { probeContentKind } from '../lib/content-kind.js';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { AlertTriangle, X, MoveRight, ArrowRight, Loader2 } from 'lucide-svelte';
  import { navigate } from '../lib/router/index.js';
  import { updateDatasetRole } from '../lib/api.js';

  /**
   * @type {{ graphs?: string[], expected?: string, contextName?: string, datasetId?: string|null, declaredRole?: string|null, onresolved?: (detail: {role: string}) => void }}
   */
  let { graphs = [], expected = 'model', contextName = '', datasetId = null, declaredRole = null, onresolved } = $props();

  let probe = $state(null);
  let dismissed = $state(false);
  let showModal = $state(false);
  let converting = $state(false);
  let convertError = $state('');

  async function run() {
    if (!graphs?.length) return;
    try {
      probe = await probeContentKind(graphs);
    } catch {
      // probe stays null — no warning shown
    }
  }

  onMount(run);

  function computeMismatch(p, dis, exp, role) {
    if (!p || dis) return null;
    if (p.verdict === 'empty') return null;
    const isModelRole = role === 'model';
    const isVocabRole = role === 'vocabulary';
    const isShapesRole = role === 'shapes';
    const isEntailmentRole = role === 'entailment';
    if ((exp === 'model' || exp === 'vocabulary') && (p.verdict === 'instances' || p.verdict === 'mixed') && p.instanceCount > 0) {
      return 'instance-in-model';
    }
    if (exp === 'dataset' && p.verdict === 'model') return isModelRole ? null : 'model-in-dataset';
    if (exp === 'dataset' && p.verdict === 'vocabulary') return isVocabRole ? null : 'vocabulary-in-dataset';
    if (exp === 'dataset' && p.verdict === 'shapes') return isShapesRole ? null : 'shapes-in-dataset';
    if (exp === 'dataset' && p.verdict === 'entailment') return isEntailmentRole ? null : 'entailment-in-dataset';
    if (exp === 'dataset' && p.verdict === 'mixed'
        && (p.classCount + p.propertyCount + p.shapeCount + (p.skosSchemeCount || 0) + (p.skosConceptCount || 0)) > 3) {
      const skosSignal = (p.skosSchemeCount || 0) + (p.skosConceptCount || 0);
      const schemaSignal = p.classCount + p.propertyCount + p.shapeCount;
      if (skosSignal > schemaSignal) return isVocabRole ? null : 'vocabulary-in-dataset';
      return isModelRole ? null : 'model-in-dataset';
    }
    return null;
  }

  let mismatch = $derived(computeMismatch(probe, dismissed, expected, declaredRole));

  function openModal() { showModal = true; convertError = ''; }
  function closeModal() { showModal = false; }

  async function doConvert(role) {
    if (!datasetId) return;
    converting = true;
    convertError = '';
    try {
      await updateDatasetRole(datasetId, role);
      closeModal();
      dismissed = true;
      onresolved?.({ role });
    } catch (e) {
      convertError = e.message ? `${e.message} (${e.status ?? $t('components.contentKindWarning.noStatus')})` : $t('components.contentKindWarning.convertFailed');
    } finally {
      converting = false;
    }
  }
</script>

{#if mismatch && !dismissed}
  <div class="ck-warning" class:dataset={expected === 'dataset'}>
    <AlertTriangle size={16} />
    <div class="ck-body">
      {#if mismatch === 'instance-in-model'}
        <strong>{expected === 'vocabulary' ? $t('components.contentKindWarning.bannerInstanceInVocabHeading') : $t('components.contentKindWarning.bannerInstanceInModelHeading')}</strong>
        <p class="ck-sub">
          {expected === 'vocabulary' ? $t('components.contentKindWarning.bannerInstanceInVocabBody') : $t('components.contentKindWarning.bannerInstanceInModelBody')}
          {$t('components.contentKindWarning.weDetected')} <b>{probe.instanceCount}</b> {$t('components.contentKindWarning.instancesLabel')}{#if probe.sampleTypes.length}
            {probe.sampleTypes.length === 1 ? $t('components.contentKindWarning.ofType') : $t('components.contentKindWarning.ofTypes')}
            {#each probe.sampleTypes as s, i}<code>{shortenIRI(s.cls)}</code>{i < probe.sampleTypes.length - 1 ? ', ' : ''}{/each}{/if}.
        </p>
      {:else if mismatch === 'vocabulary-in-dataset'}
        <strong>{$t('components.contentKindWarning.bannerVocabInDatasetHeading')}</strong>
        <p class="ck-sub">
          {$t('components.contentKindWarning.datasetsHoldInstanceData')} {$t('components.contentKindWarning.weDetected')}
          <b>{probe.skosSchemeCount}</b> {$t('components.contentKindWarning.conceptSchemesLabel')} {$t('components.contentKindWarning.and')}
          <b>{probe.skosConceptCount}</b> {$t('components.contentKindWarning.conceptsLabel')} — {$t('components.contentKindWarning.vocabBelongPrefix')} <b>{$t('components.contentKindWarning.vocabularyRegistry')}</b>
          {$t('components.contentKindWarning.belongSuffix')}
        </p>
      {:else if mismatch === 'shapes-in-dataset'}
        <strong>{$t('components.contentKindWarning.bannerShapesInDatasetHeading')}</strong>
        <p class="ck-sub">
          {$t('components.contentKindWarning.weDetected')} <b>{probe.shapeCount}</b> {$t('components.contentKindWarning.shaclShapesLabel')}. {$t('components.contentKindWarning.bannerShapesInDatasetBodyPrefix')}
          <b>{$t('components.contentKindWarning.shapesGraph')}</b> — {$t('components.contentKindWarning.bannerShapesInDatasetBodyMid')} <em>{$t('components.contentKindWarning.roleShapes')}</em> {$t('components.contentKindWarning.bannerShapesInDatasetBodySuffix')}
        </p>
      {:else if mismatch === 'entailment-in-dataset'}
        <strong>{$t('components.contentKindWarning.bannerEntailmentInDatasetHeading')}</strong>
        <p class="ck-sub">
          {$t('components.contentKindWarning.weDetected')} <b>{probe.entailmentCount}</b> {$t('components.contentKindWarning.swrlSpinRulesLabel')}. {$t('components.contentKindWarning.bannerEntailmentInDatasetBodyPrefix')}
          <b>{$t('components.contentKindWarning.entailmentGraph')}</b> — {$t('components.contentKindWarning.bannerEntailmentInDatasetBodyMid')} <em>{$t('components.contentKindWarning.roleEntailment')}</em>.
        </p>
      {:else}
        <strong>{$t('components.contentKindWarning.bannerModelInDatasetHeading')}</strong>
        <p class="ck-sub">
          {$t('components.contentKindWarning.datasetsHoldInstanceData')} {$t('components.contentKindWarning.weDetected')}
          <b>{probe.classCount}</b> {$t('components.contentKindWarning.classDefinitionsLabel')},
          <b>{probe.propertyCount}</b> {$t('components.contentKindWarning.propertyDefinitionsLabel')} {$t('components.contentKindWarning.and')}
          <b>{probe.shapeCount}</b> {$t('components.contentKindWarning.shaclShapesLabel')} — {$t('components.contentKindWarning.modelBelongPrefix')} <b>{$t('components.contentKindWarning.modelRegistry')}</b>
          {$t('components.contentKindWarning.belongSuffix')}
        </p>
      {/if}
    </div>
    <div class="ck-actions">
      {#if datasetId && (mismatch === 'model-in-dataset' || mismatch === 'vocabulary-in-dataset' || mismatch === 'shapes-in-dataset' || mismatch === 'entailment-in-dataset')}
        {@const convertRole = mismatch === 'vocabulary-in-dataset' ? 'vocabulary' : mismatch === 'shapes-in-dataset' ? 'shapes' : mismatch === 'entailment-in-dataset' ? 'entailment' : 'model'}
        {@const convertLabel = mismatch === 'vocabulary-in-dataset' ? $t('components.contentKindWarning.convertToVocabulary') : mismatch === 'shapes-in-dataset' ? $t('components.contentKindWarning.setRoleShapes') : mismatch === 'entailment-in-dataset' ? $t('components.contentKindWarning.setRoleEntailment') : $t('components.contentKindWarning.convertToModel')}
        <button class="btn btn-sm btn-primary" onclick={() => doConvert(convertRole)} disabled={converting}>
          {#if converting}<Loader2 size={13} class="animate-spin" />{:else}<MoveRight size={13} />{/if}
          {convertLabel}
        </button>
        <button class="btn btn-sm btn-ghost" onclick={openModal} title={$t('components.contentKindWarning.seeOptions')}>
          <ArrowRight size={13} />
        </button>
      {:else}
        <button class="btn btn-sm btn-primary" onclick={openModal}>
          <MoveRight size={13} /> {$t('components.contentKindWarning.resolve')}
        </button>
      {/if}
      <button class="btn btn-sm btn-ghost" onclick={() => (dismissed = true)} title={$t('components.contentKindWarning.dismiss')}>
        <X size={13} />
      </button>
    </div>
    {#if convertError}<p class="ck-convert-error-inline">{convertError}</p>{/if}
  </div>
{/if}

{#if showModal}
  <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
  <div class="ck-backdrop" onclick={(e) => { if (e.target === e.currentTarget) closeModal(); }}>
    <div class="ck-modal">
      <div class="ck-modal-head">
        <h3>{mismatch === 'instance-in-model' ? $t('components.contentKindWarning.modalTitleInstanceInModel') : mismatch === 'vocabulary-in-dataset' ? $t('components.contentKindWarning.modalTitleVocabInDataset') : mismatch === 'shapes-in-dataset' ? $t('components.contentKindWarning.modalTitleShapesInDataset') : mismatch === 'entailment-in-dataset' ? $t('components.contentKindWarning.modalTitleEntailmentInDataset') : $t('components.contentKindWarning.modalTitleModelInDataset')}</h3>
        <button class="btn btn-ghost btn-sm" onclick={closeModal}><X size={14} /></button>
      </div>
      <div class="ck-modal-body">
        {#if mismatch === 'instance-in-model'}
          <p>
            {expected === 'vocabulary' ? $t('components.contentKindWarning.modalInstanceVocabIntroPrefix') : $t('components.contentKindWarning.modalInstanceModelIntroPrefix')} <b>{$t('components.contentKindWarning.knowledgeModel')}</b> {$t('components.contentKindWarning.modalInstanceIntroMid')}{expected === 'vocabulary' ? $t('components.contentKindWarning.modalInstanceVocabIntroSuffix') : $t('components.contentKindWarning.modalInstanceModelIntroSuffix')} {$t('components.contentKindWarning.modalInstanceIntroEnd')}
          </p>
          <ol>
            <li>{$t('components.contentKindWarning.stepCreateDatasetPrefix')} <b>{$t('components.contentKindWarning.datasetWord')}</b> {$t('components.contentKindWarning.stepCreateDatasetSuffix')}</li>
            <li>{$t('components.contentKindWarning.stepImportInstancePortion')}</li>
            <li>{$t('components.contentKindWarning.stepSetConformsPrefix')} <em>{$t('components.contentKindWarning.conformsTo')}</em> {$t('components.contentKindWarning.stepSetConformsTo')} <b>{contextName || (expected === 'vocabulary' ? $t('components.contentKindWarning.thisVocabulary') : $t('components.contentKindWarning.thisModel'))}</b> {$t('components.contentKindWarning.stepSetConformsSuffix')}</li>
          </ol>
          <div class="ck-counts">
            {#if expected === 'vocabulary'}
              <div>{$t('components.contentKindWarning.countConceptSchemes')} <b>{probe.skosSchemeCount || 0}</b></div>
              <div>{$t('components.contentKindWarning.countConcepts')} <b>{probe.skosConceptCount || 0}</b></div>
            {:else}
              <div>{$t('components.contentKindWarning.countClasses')} <b>{probe.classCount}</b></div>
              <div>{$t('components.contentKindWarning.countProperties')} <b>{probe.propertyCount}</b></div>
              <div>{$t('components.contentKindWarning.countShaclShapes')} <b>{probe.shapeCount}</b></div>
            {/if}
            <div class="accent">{$t('components.contentKindWarning.countInstances')} <b>{probe.instanceCount}</b></div>
          </div>
          <div class="ck-modal-actions">
            <button class="btn btn-primary btn-sm" onclick={() => { closeModal(); navigate('/datasets'); }}>
              <ArrowRight size={13} /> {$t('components.contentKindWarning.goToDatasets')}
            </button>
            <button class="btn btn-ghost btn-sm" onclick={() => { closeModal(); navigate(`/import?graph=${encodeURIComponent(graphs[0] || '')}&target=dataset`); }}>
              {$t('components.contentKindWarning.openImportWizard')}
            </button>
          </div>
        {:else if mismatch === 'vocabulary-in-dataset'}
          <p>
            {$t('components.contentKindWarning.modalDatasetsHoldPrefix')} <b>{$t('components.contentKindWarning.instanceData')}</b>. {$t('components.contentKindWarning.modalVocabBodyPrefix')} <b>{$t('components.contentKindWarning.vocabularyRegistry')}</b> — {$t('components.contentKindWarning.modalVocabBodySuffix')} <em>{$t('components.contentKindWarning.conformsTo')}</em> {$t('components.contentKindWarning.fieldWord')}
          </p>
          {#if datasetId}
            <p>{$t('components.contentKindWarning.convertInPlaceVocabPrefix')} <b>{$t('components.contentKindWarning.vocabularyWord')}</b> {$t('components.contentKindWarning.convertInPlaceSuffix')}</p>
          {:else}
            <ol>
              <li>{$t('components.contentKindWarning.stepCreateEntryPrefix')} <b>{$t('components.contentKindWarning.vocabularyRegistry')}</b>.</li>
              <li>{$t('components.contentKindWarning.stepUploadVocabPortion')}</li>
              <li>{$t('components.contentKindWarning.stepComeBackPrefix')} <em>{$t('components.contentKindWarning.conformsTo')}</em> {$t('components.contentKindWarning.stepComeBackSuffix')}</li>
            </ol>
          {/if}
          <div class="ck-counts">
            <div class="accent">{$t('components.contentKindWarning.countConceptSchemes')} <b>{probe.skosSchemeCount || 0}</b></div>
            <div class="accent">{$t('components.contentKindWarning.countConcepts')} <b>{probe.skosConceptCount || 0}</b></div>
            <div class="accent">{$t('components.contentKindWarning.countClasses')} <b>{probe.classCount}</b></div>
            <div>{$t('components.contentKindWarning.countInstances')} <b>{probe.instanceCount}</b></div>
          </div>
          <div class="ck-modal-actions">
            {#if datasetId}
              <button class="btn btn-primary btn-sm" onclick={() => doConvert('vocabulary')} disabled={converting}>
                {#if converting}<Loader2 size={13} class="animate-spin" />{:else}<ArrowRight size={13} />{/if}
                {$t('components.contentKindWarning.convertToVocabulary')}
              </button>
              <button class="btn btn-ghost btn-sm" onclick={() => { closeModal(); navigate('/models'); }}>
                {$t('components.contentKindWarning.goToVocabularyRegistry')}
              </button>
            {:else}
              <button class="btn btn-primary btn-sm" onclick={() => { closeModal(); navigate('/models'); }}>
                <ArrowRight size={13} /> {$t('components.contentKindWarning.goToVocabularyRegistry')}
              </button>
              <button class="btn btn-ghost btn-sm" onclick={() => { closeModal(); navigate(`/import?graph=${encodeURIComponent(graphs[0] || '')}&target=vocabulary`); }}>
                {$t('components.contentKindWarning.openImportWizard')}
              </button>
            {/if}
          </div>
          {#if convertError}<p class="ck-convert-error">{convertError}</p>{/if}
        {:else if mismatch === 'shapes-in-dataset'}
          <p>
            {$t('components.contentKindWarning.modalShapesBodyPrefix')} <b>{$t('components.contentKindWarning.shaclShapesPlain')}</b> — {$t('components.contentKindWarning.modalShapesBodyMid')} <em>{$t('components.contentKindWarning.roleShapes')}</em> {$t('components.contentKindWarning.modalShapesBodySuffix')}
          </p>
          <div class="ck-counts">
            <div class="accent">{$t('components.contentKindWarning.countShaclShapes')} <b>{probe.shapeCount}</b></div>
            <div>{$t('components.contentKindWarning.countInstances')} <b>{probe.instanceCount}</b></div>
          </div>
          <div class="ck-modal-actions">
            {#if datasetId}
              <button class="btn btn-primary btn-sm" onclick={() => doConvert('shapes')} disabled={converting}>
                {#if converting}<Loader2 size={13} class="animate-spin" />{:else}<ArrowRight size={13} />{/if}
                {$t('components.contentKindWarning.setRoleShapes')}
              </button>
            {/if}
            <button class="btn btn-ghost btn-sm" onclick={closeModal}>{$t('components.contentKindWarning.dismiss')}</button>
          </div>
          {#if convertError}<p class="ck-convert-error">{convertError}</p>{/if}
        {:else if mismatch === 'entailment-in-dataset'}
          <p>
            {$t('components.contentKindWarning.modalEntailmentBodyPrefix')} <b>{$t('components.contentKindWarning.entailmentRulesPlain')}</b> (SWRL/SPIN). {$t('components.contentKindWarning.modalEntailmentBodyMid')}
            <em>{$t('components.contentKindWarning.roleEntailment')}</em> {$t('components.contentKindWarning.modalEntailmentBodySuffix')}
          </p>
          <div class="ck-counts">
            <div class="accent">{$t('components.contentKindWarning.countEntailmentRules')} <b>{probe.entailmentCount || 0}</b></div>
            <div>{$t('components.contentKindWarning.countInstances')} <b>{probe.instanceCount}</b></div>
          </div>
          <div class="ck-modal-actions">
            {#if datasetId}
              <button class="btn btn-primary btn-sm" onclick={() => doConvert('entailment')} disabled={converting}>
                {#if converting}<Loader2 size={13} class="animate-spin" />{:else}<ArrowRight size={13} />{/if}
                {$t('components.contentKindWarning.setRoleEntailment')}
              </button>
            {/if}
            <button class="btn btn-ghost btn-sm" onclick={closeModal}>{$t('components.contentKindWarning.dismiss')}</button>
          </div>
          {#if convertError}<p class="ck-convert-error">{convertError}</p>{/if}
        {:else}
          <p>
            {$t('components.contentKindWarning.modalDatasetsHoldPrefix')} <b>{$t('components.contentKindWarning.instanceData')}</b>. {$t('components.contentKindWarning.modalModelBodyPrefix')} <b>{$t('components.contentKindWarning.modelRegistry')}</b> — {$t('components.contentKindWarning.modalModelBodySuffix')} <em>{$t('components.contentKindWarning.conformsTo')}</em> {$t('components.contentKindWarning.fieldWord')}
          </p>
          {#if datasetId}
            <p>{$t('components.contentKindWarning.convertInPlaceModelPrefix')} <b>{$t('components.contentKindWarning.modelWord')}</b> {$t('components.contentKindWarning.convertInPlaceSuffix')}</p>
          {:else}
            <ol>
              <li>{$t('components.contentKindWarning.stepCreateEntryPrefix')} <b>{$t('components.contentKindWarning.modelRegistry')}</b>.</li>
              <li>{$t('components.contentKindWarning.stepUploadModelPortion')}</li>
              <li>{$t('components.contentKindWarning.stepComeBackPrefix')} <em>{$t('components.contentKindWarning.conformsTo')}</em> {$t('components.contentKindWarning.stepComeBackSuffix')}</li>
            </ol>
          {/if}
          <div class="ck-counts">
            <div class="accent">{$t('components.contentKindWarning.countClasses')} <b>{probe.classCount}</b></div>
            <div class="accent">{$t('components.contentKindWarning.countProperties')} <b>{probe.propertyCount}</b></div>
            <div class="accent">{$t('components.contentKindWarning.countShaclShapes')} <b>{probe.shapeCount}</b></div>
            <div>{$t('components.contentKindWarning.countInstances')} <b>{probe.instanceCount}</b></div>
          </div>
          <div class="ck-modal-actions">
            {#if datasetId}
              <button class="btn btn-primary btn-sm" onclick={() => doConvert('model')} disabled={converting}>
                {#if converting}<Loader2 size={13} class="animate-spin" />{:else}<ArrowRight size={13} />{/if}
                {$t('components.contentKindWarning.convertToModel')}
              </button>
              <button class="btn btn-ghost btn-sm" onclick={() => { closeModal(); navigate('/models'); }}>
                {$t('components.contentKindWarning.goToModelRegistry')}
              </button>
            {:else}
              <button class="btn btn-primary btn-sm" onclick={() => { closeModal(); navigate('/models'); }}>
                <ArrowRight size={13} /> {$t('components.contentKindWarning.goToModelRegistry')}
              </button>
              <button class="btn btn-ghost btn-sm" onclick={() => { closeModal(); navigate(`/import?graph=${encodeURIComponent(graphs[0] || '')}&target=model`); }}>
                {$t('components.contentKindWarning.openImportWizard')}
              </button>
            {/if}
          </div>
          {#if convertError}<p class="ck-convert-error">{convertError}</p>{/if}
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .ck-warning {
    display: flex; align-items: flex-start; gap: 0.6rem;
    padding: 0.75rem 0.9rem;
    border: 1px solid #fde68a; background: #fffbeb; color: #78350f;
    border-radius: 12px;
  }
  .ck-warning.dataset {
    border-color: #bfdbfe; background: #eff6ff; color: #1e3a8a;
  }
  .ck-body { flex: 1; min-width: 0; }
  .ck-body strong { display: block; font-size: 0.9rem; margin-bottom: 0.2rem; }
  .ck-sub { margin: 0; font-size: 0.82rem; line-height: 1.45; }
  .ck-sub code { font-size: 0.75rem; background: rgba(0,0,0,0.05); padding: 1px 5px; border-radius: 4px; }
  .ck-actions { display: flex; gap: 0.35rem; flex-shrink: 0; }

  .ck-backdrop {
    position: fixed; inset: 0; z-index: 100;
    background: rgba(15, 23, 42, 0.55);
    display: flex; align-items: center; justify-content: center;
    padding: 1rem;
  }
  .ck-modal {
    background: #fff; border-radius: 16px; max-width: 560px; width: 100%;
    max-height: 90vh; overflow: auto; box-shadow: 0 20px 50px rgba(0,0,0,0.25);
  }
  .ck-modal-head { display: flex; align-items: center; justify-content: space-between; padding: 0.85rem 1rem; border-bottom: 1px solid #e2e8f0; }
  .ck-modal-head h3 { margin: 0; font-size: 1rem; font-weight: 700; color: #1e293b; }
  .ck-modal-body { padding: 1rem; display: flex; flex-direction: column; gap: 0.75rem; font-size: 0.88rem; color: #334155; line-height: 1.5; }
  .ck-modal-body p, .ck-modal-body ol { margin: 0; }
  .ck-modal-body ol { padding-left: 1.25rem; display: flex; flex-direction: column; gap: 0.25rem; }
  .ck-counts { display: grid; grid-template-columns: repeat(2, 1fr); gap: 0.4rem; padding: 0.6rem; border: 1px solid #e2e8f0; border-radius: 10px; background: #f8fafc; font-size: 0.82rem; }
  .ck-counts .accent { color: #b45309; font-weight: 600; }
  .ck-warning.dataset ~ * .ck-counts .accent, .ck-modal-body .accent { color: inherit; font-weight: 600; }
  .ck-modal-actions { display: flex; gap: 0.4rem; justify-content: flex-end; padding-top: 0.5rem; border-top: 1px dashed #e2e8f0; }
  .ck-convert-error { margin: 0.3rem 0 0; font-size: 0.78rem; color: #dc2626; }
  .ck-convert-error-inline { margin: 0.3rem 0.9rem 0; font-size: 0.78rem; color: #dc2626; }

  /* Dark theme: this banner + modal hardcode light amber/blue/white surfaces. */
  :global(html.dark) .ck-warning {
    border-color: rgba(251, 191, 36, 0.38); background: rgba(251, 191, 36, 0.12); color: #fcd980;
  }
  :global(html.dark) .ck-warning.dataset {
    border-color: rgba(96, 165, 250, 0.38); background: rgba(59, 130, 246, 0.13); color: #bfdbfe;
  }
  :global(html.dark) .ck-sub code { background: rgba(255, 255, 255, 0.1); }
  :global(html.dark) .ck-modal { background: #111827; box-shadow: 0 20px 50px rgba(0, 0, 0, 0.55); }
  :global(html.dark) .ck-modal-head { border-bottom-color: rgba(226, 232, 240, 0.12); }
  :global(html.dark) .ck-modal-head h3 { color: #f1f5f9; }
  :global(html.dark) .ck-modal-body { color: #cbd5e1; }
  :global(html.dark) .ck-counts { border-color: rgba(226, 232, 240, 0.12); background: rgba(255, 255, 255, 0.04); }
  :global(html.dark) .ck-counts .accent { color: #fbbf24; }
  :global(html.dark) .ck-modal-actions { border-top-color: rgba(226, 232, 240, 0.14); }
  :global(html.dark) .ck-convert-error,
  :global(html.dark) .ck-convert-error-inline { color: #f87171; }
</style>
