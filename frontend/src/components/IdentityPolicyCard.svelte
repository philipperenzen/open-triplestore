<script lang="ts">
  // The owl:sameAs identity policy of a dataset or an organisation, over
  // GET/PUT/DELETE /api/{datasets|organisations}/:id/identity (docs/reasoning.md,
  // "Identity policy"). Readers see the policy in force and where it comes from;
  // `canManage` adds the select. The card hides itself when the server will not
  // show the setting to this user (401/403/404).
  import { t } from 'svelte-i18n';
  import { Fingerprint, Loader2 } from 'lucide-svelte';
  import Select from './Select.svelte';
  import { Link } from '../lib/router/index.js';
  import {
    getIdentityPolicy, setIdentityPolicy, clearIdentityPolicy,
    type IdentityPolicyName, type IdentityPolicyState, type IdentityScope,
  } from '../lib/api';
  import { toastError, toastSuccess } from '../lib/toast';

  export let scope: IdentityScope;
  export let id: string;
  export let canManage = false;

  const POLICIES: IdentityPolicyName[] = ['sameas-off', 'sameas-narrow', 'sameas-full'];
  /** The select's value for "no own setting". */
  const INHERIT = '';

  let state: IdentityPolicyState | null = null;
  let hidden = false;
  let loadError = '';
  let loading = true;
  let saving = false;
  let choice: string = INHERIT;

  async function load(target: string) {
    loading = true;
    loadError = '';
    try {
      state = await getIdentityPolicy(scope, target);
      hidden = false;
      choice = state.setting ?? INHERIT;
    } catch (e) {
      const status = (e as { status?: number })?.status;
      if (status === 401 || status === 403 || status === 404) hidden = true;
      else loadError = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
  }

  $: if (id) load(id);

  $: options = [
    { value: INHERIT, label: scope === 'dataset' ? $t('components.identityPolicy.inheritDataset') : $t('components.identityPolicy.inheritOrganisation') },
    ...POLICIES.map((p) => ({ value: p, label: $t(`components.identityPolicy.option.${p}`) })),
  ];
  $: dirty = !!state && choice !== (state.setting ?? INHERIT);

  async function save() {
    if (!state || !dirty) return;
    saving = true;
    try {
      state = choice === INHERIT
        ? await clearIdentityPolicy(scope, id)
        : await setIdentityPolicy(scope, id, choice as IdentityPolicyName);
      choice = state.setting ?? INHERIT;
      toastSuccess($t('components.identityPolicy.saved'));
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    } finally {
      saving = false;
    }
  }
</script>

{#if !hidden}
<div class="card identity-card" id="identity">
  <div class="ip-head">
    <Fingerprint size={15} />
    <h3>{$t('components.identityPolicy.heading')}</h3>
  </div>
  <p class="ip-intro">
    {scope === 'dataset' ? $t('components.identityPolicy.introDataset') : $t('components.identityPolicy.introOrganisation')}
    <Link to="/docs/reasoning" class="ip-doc-link">{$t('components.identityPolicy.docsLink')}</Link>
  </p>

  {#if loading && !state}
    <p class="ip-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
  {:else if loadError}
    <p class="ip-error">{loadError}</p>
  {:else if state}
    <dl class="ip-effective">
      <dt>{$t('components.identityPolicy.inForce')}</dt>
      <dd>
        <span class="ip-badge ip-{state.policy}">{$t(`components.identityPolicy.option.${state.policy}`)}</span>
        <span class="ip-source">{$t(`components.identityPolicy.source.${scope === 'organisation' && state.source === 'organisation' ? 'organisationOwn' : state.source}`)}</span>
      </dd>
      <dd class="ip-desc">{$t(`components.identityPolicy.description.${state.policy}`)}</dd>
    </dl>

    {#if canManage}
      <div class="ip-form">
        <label class="ip-label" for="identity-policy-{scope}">{$t('components.identityPolicy.selectLabel')}</label>
        <div class="ip-row">
          <Select id="identity-policy-{scope}" bind:value={choice} {options} disabled={saving} ariaLabel={$t('components.identityPolicy.selectLabel')} />
          <button class="btn btn-sm" on:click={save} disabled={!dirty || saving}>
            {#if saving}<Loader2 size={13} class="animate-spin" />{/if}
            {$t('system.save')}
          </button>
        </div>
        {#if scope === 'dataset'}
          <p class="ip-muted">{$t('components.identityPolicy.rematerialiseHint')}</p>
        {:else}
          <p class="ip-muted">{$t('components.identityPolicy.orgInheritHint')}</p>
        {/if}
      </div>
    {:else}
      <p class="ip-muted">{$t('components.identityPolicy.readOnly')}</p>
    {/if}

    {#if state.never_identity?.length}
      <p class="ip-muted ip-never">
        {$t('components.identityPolicy.neverIdentity')}
        {#each state.never_identity as p}<code>{p}</code>{/each}
      </p>
    {/if}
  {/if}
</div>
{/if}

<style>
  .ip-head { display: flex; align-items: center; gap: 0.5rem; margin-bottom: 0.4rem; }
  .ip-head h3 { margin: 0; font-size: 0.95rem; font-weight: 600; }
  .ip-intro { margin: 0 0 0.75rem; font-size: 0.85rem; color: var(--ink-500); line-height: 1.45; }
  .ip-intro :global(.ip-doc-link) { white-space: nowrap; }
  .ip-effective { margin: 0 0 0.75rem; display: grid; gap: 0.25rem; }
  .ip-effective dt { font-size: 0.75rem; font-weight: 600; text-transform: uppercase; letter-spacing: 0.03em; color: var(--ink-400); }
  .ip-effective dd { margin: 0; display: flex; flex-wrap: wrap; align-items: center; gap: 0.5rem; }
  .ip-desc { font-size: 0.85rem; color: var(--ink-600); }
  .ip-badge { font-size: 0.78rem; font-weight: 600; padding: 0.1rem 0.5rem; border-radius: 999px; background: #e0e7ff; color: #3730a3; }
  .ip-sameas-off { background: #f1f5f9; color: #475569; }
  .ip-sameas-full { background: #fef3c7; color: #92400e; }
  .ip-source { font-size: 0.8rem; color: var(--ink-500); }
  .ip-form { display: grid; gap: 0.35rem; margin-bottom: 0.5rem; }
  .ip-label { font-size: 0.8rem; font-weight: 600; }
  .ip-row { display: flex; flex-wrap: wrap; gap: 0.5rem; align-items: center; }
  .ip-row :global(.sel-trigger) { min-width: 16rem; }
  .ip-muted { margin: 0.25rem 0 0; font-size: 0.8rem; color: var(--ink-500); display: flex; flex-wrap: wrap; align-items: center; gap: 0.35rem; }
  .ip-never code { font-size: 0.75rem; background: var(--surface-2, #f1f5f9); padding: 0 0.3rem; border-radius: 0.25rem; }
  .ip-error { color: #b91c1c; font-size: 0.85rem; margin: 0; }
  :global(:is([data-theme="dark"], .dark)) .ip-badge { background: rgba(99,102,241,0.2); color: #c7d2fe; }
  :global(:is([data-theme="dark"], .dark)) .ip-sameas-off { background: rgba(255,255,255,0.06); color: var(--ink-600); }
  :global(:is([data-theme="dark"], .dark)) .ip-sameas-full { background: rgba(245,158,11,0.2); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .ip-error { color: #fca5a5; }
</style>
