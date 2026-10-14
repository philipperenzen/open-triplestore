<script lang="ts">
  // The dataset's Linked Data Event Stream: whether it publishes, its
  // fragmentation (members per page; a full page is sealed and immutable),
  // its polling interval and its retention policy, over
  // GET /api/datasets/:id/ldes/config and PUT /api/datasets/:id/ldes.
  // Readers see the settings and the stream URL; writers can change them.
  import { t } from 'svelte-i18n';
  import { Radio, Loader2, Copy } from 'lucide-svelte';
  import { getLdesConfig, putLdesStream, type LdesConfig } from '../../lib/api';
  import { retentionToForm, formToRetention, keepsEverything, type RetentionForm } from '../../lib/exchange';
  import { askConfirm } from '../../lib/confirm';
  import { copyToClipboard } from '../../lib/clipboard.js';
  import { toastError, toastSuccess } from '../../lib/toast';
  import './exchange.css';

  export let datasetId: string;

  let config: LdesConfig | null = null;
  let loading = true;
  let loadError = '';
  let saving = false;
  let formError = '';

  let enabled = false;
  let pageSize = '100';
  let pollingInterval = '60';
  let retention: RetentionForm = retentionToForm(null);

  async function load(id: string) {
    loading = true;
    loadError = '';
    try {
      config = await getLdesConfig(id);
      reset();
    } catch (e) {
      loadError = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
  }

  function reset() {
    if (!config) return;
    enabled = config.enabled;
    pageSize = String(config.page_size);
    pollingInterval = String(config.polling_interval);
    retention = retentionToForm(config.retention);
    formError = '';
  }

  $: if (datasetId) load(datasetId);

  async function save() {
    if (!config) return;
    formError = '';
    const size = Number(pageSize);
    if (!Number.isInteger(size) || size < 1 || size > 10000) {
      formError = $t('components.exchange.ldes.errors.pageSize');
      return;
    }
    const poll = Number(pollingInterval);
    if (!Number.isInteger(poll) || poll < 1) {
      formError = $t('components.exchange.ldes.errors.pollingInterval');
      return;
    }
    const r = formToRetention(retention);
    if ('error' in r) {
      formError = $t(`components.exchange.ldes.errors.${r.error}`);
      return;
    }
    if (config.enabled && !enabled) {
      const ok = await askConfirm({
        title: $t('components.exchange.ldes.disableTitle'),
        message: $t('components.exchange.ldes.disableMessage'),
        confirmLabel: $t('components.exchange.ldes.disableConfirm'),
        variant: 'warning',
      });
      if (!ok) return;
    }
    saving = true;
    try {
      const res = await putLdesStream(datasetId, {
        enabled,
        page_size: size,
        polling_interval: poll,
        retention: r.policy,
      });
      toastSuccess(
        res?.members_seeded
          ? $t('components.exchange.ldes.savedSeeded', { values: { n: res.members_seeded } })
          : $t('components.exchange.ldes.saved'),
      );
      await load(datasetId);
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    } finally {
      saving = false;
    }
  }

  async function copyStream() {
    if (config && (await copyToClipboard(config.stream))) toastSuccess($t('components.exchange.copied'));
  }
</script>

<section class="xc-sub" data-testid="ldes-panel">
  <div class="xc-head">
    <Radio size={15} />
    <h4>{$t('components.exchange.ldes.heading')}</h4>
    {#if config}
      <span class="xc-badge" class:xc-ok={config.enabled}>
        {config.enabled ? $t('components.exchange.ldes.publishing') : $t('components.exchange.ldes.notPublishing')}
      </span>
    {/if}
  </div>
  <p class="xc-intro">{$t('components.exchange.ldes.intro')}</p>

  {#if loading && !config}
    <p class="xc-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
  {:else if loadError}
    <p class="xc-error">{loadError}</p>
  {:else if config}
    {#if config.enabled}
      <dl class="xc-facts">
        <div class="xc-wide">
          <dt>{$t('components.exchange.ldes.streamUrl')}</dt>
          <dd class="xc-row">
            <code class="xc-code">{config.stream}</code>
            <button class="icon-btn" on:click={copyStream} title={$t('components.exchange.copy')} aria-label={$t('components.exchange.copy')}><Copy size={13} /></button>
          </dd>
        </div>
        <div><dt>{$t('components.exchange.ldes.members')}</dt><dd>{config.members.toLocaleString()}</dd></div>
        <div><dt>{$t('components.exchange.ldes.sealedPages')}</dt><dd>{config.sealed_nodes.toLocaleString()}</dd></div>
        <div>
          <dt>{$t('components.exchange.ldes.retentionNow')}</dt>
          <dd>{keepsEverything(config.retention) ? $t('components.exchange.ldes.keepsEverything') : $t('components.exchange.ldes.policyDeclared')}</dd>
        </div>
      </dl>
    {/if}

    {#if config.can_write}
      <form class="xc-form" on:submit|preventDefault={save}>
        <label class="xc-check">
          <input type="checkbox" bind:checked={enabled} disabled={saving} />
          {$t('components.exchange.ldes.enable')}
        </label>
        {#if !config.enabled && enabled}
          <p class="xc-muted">{$t('components.exchange.ldes.seedHint')}</p>
        {/if}
        <div class="xc-grid">
          <label class="xc-field">
            <span>{$t('components.exchange.ldes.pageSize')}</span>
            <input type="number" min="1" max="10000" bind:value={pageSize} disabled={saving} />
            <small>{$t('components.exchange.ldes.pageSizeHint')}</small>
          </label>
          <label class="xc-field">
            <span>{$t('components.exchange.ldes.pollingInterval')}</span>
            <input type="number" min="1" bind:value={pollingInterval} disabled={saving} />
            <small>{$t('components.exchange.ldes.pollingIntervalHint')}</small>
          </label>
        </div>
        <fieldset class="xc-fieldset">
          <legend class="xc-label">{$t('components.exchange.ldes.retention')}</legend>
          <p class="xc-muted">{$t('components.exchange.ldes.retentionHint')}</p>
          <div class="xc-grid">
            <label class="xc-field">
              <span>{$t('components.exchange.ldes.fullLogDuration')}</span>
              <input type="text" placeholder="P30D" bind:value={retention.full_log_duration} disabled={saving} />
              <small>{$t('components.exchange.ldes.fullLogDurationHint')}</small>
            </label>
            <label class="xc-field">
              <span>{$t('components.exchange.ldes.versionAmount')}</span>
              <input type="number" min="1" placeholder="2" bind:value={retention.version_amount} disabled={saving} />
              <small>{$t('components.exchange.ldes.versionAmountHint')}</small>
            </label>
            <label class="xc-field">
              <span>{$t('components.exchange.ldes.versionDuration')}</span>
              <input type="text" placeholder="P1Y" bind:value={retention.version_duration} disabled={saving} />
              <small>{$t('components.exchange.ldes.versionDurationHint')}</small>
            </label>
            <label class="xc-field">
              <span>{$t('components.exchange.ldes.versionDeleteDuration')}</span>
              <input type="text" placeholder="P7D" bind:value={retention.version_delete_duration} disabled={saving} />
              <small>{$t('components.exchange.ldes.versionDeleteDurationHint')}</small>
            </label>
            <label class="xc-field">
              <span>{$t('components.exchange.ldes.startingFrom')}</span>
              <input type="text" placeholder="2026-01-01T00:00:00Z" bind:value={retention.starting_from} disabled={saving} />
              <small>{$t('components.exchange.ldes.startingFromHint')}</small>
            </label>
          </div>
        </fieldset>
        {#if formError}<p class="xc-error" role="alert">{formError}</p>{/if}
        <div class="xc-row">
          <button type="submit" class="btn btn-sm" disabled={saving}>
            {#if saving}<Loader2 size={13} class="animate-spin" />{/if}
            {$t('components.exchange.ldes.save')}
          </button>
          <button type="button" class="btn btn-sm btn-ghost" on:click={reset} disabled={saving}>{$t('components.exchange.reset')}</button>
        </div>
      </form>
    {:else if !config.enabled}
      <p class="xc-muted">{$t('components.exchange.ldes.readOnlyOff')}</p>
    {/if}
  {/if}
</section>

<style>
  .xc-fieldset { border: 1px solid var(--line-soft); border-radius: 0.6rem; padding: 0.5rem 0.75rem 0.75rem; margin: 0; }
  .xc-fieldset legend { padding: 0 0.25rem; }
</style>
