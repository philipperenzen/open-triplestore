<script lang="ts">
  // Admin maintenance: backups (status with the S3 copy, list, create, verify,
  // and a restore staged for the next start, confirmed in the app's dialog)
  // and the full-text index (status and a full rebuild). Backups are a
  // super_admin matter on the server; the page says so to other admins
  // instead of showing them a 403.
  import { t, locale } from 'svelte-i18n';
  import {
    HardDrive, Loader2, RefreshCw, Plus, ShieldCheck, ArchiveRestore, X as XIcon, Search, CircleAlert, CheckCircle2, CloudUpload,
  } from 'lucide-svelte';
  import { isAdmin, isSuperAdmin, authInitialized } from '../lib/stores.js';
  import { navigate } from '../lib/router/index.js';
  import {
    adminBackupStatus, adminListBackups, adminCreateBackup, adminVerifyBackup, adminStageRestore, adminCancelRestore,
    textSearchStatus, textSearchReindex,
  } from '../lib/api';
  import { askConfirm } from '../lib/confirm';
  import { toastError, toastSuccess, toastWarn } from '../lib/toast';
  import ConfirmModal from '../components/ConfirmModal.svelte';
  import '../components/exchange/exchange.css';

  type Manifest = {
    id: string; created_at: string; software_version: string; rdf_quad_count: number; encrypted: boolean;
  };

  let status: any = null;
  let backups: Manifest[] = [];
  let backupError = '';
  let loadingBackups = false;
  let creating = false;
  let verifying: Record<string, boolean | 'busy'> = {};
  let restoreTarget: Manifest | null = null;
  let staging = false;

  let index: any = null;
  let indexError = '';
  let reindexing = false;

  let _guardChecked = false;
  $: if ($authInitialized && !_guardChecked) {
    _guardChecked = true;
    if (!$isAdmin) navigate('/');
    else {
      if ($isSuperAdmin) loadBackups();
      loadIndex();
    }
  }

  $: lang = $locale || 'en';
  const when = (iso: string | null | undefined) => (iso ? new Date(iso).toLocaleString(lang) : '—');

  async function loadBackups() {
    loadingBackups = true;
    backupError = '';
    try {
      status = await adminBackupStatus();
      backups = status?.configured ? await adminListBackups() : [];
    } catch (e) {
      backupError = (e as Error)?.message || String(e);
    } finally {
      loadingBackups = false;
    }
  }

  async function createBackup() {
    creating = true;
    try {
      const m = await adminCreateBackup();
      if (m?.s3 && m.s3.uploaded === false) {
        toastWarn($t('pages.adminMaintenance.backups.createdS3Failed', { values: { id: m.id, error: m.s3.error } }));
      } else {
        toastSuccess($t('pages.adminMaintenance.backups.created', { values: { id: m.id } }));
      }
      await loadBackups();
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    } finally {
      creating = false;
    }
  }

  async function verify(id: string) {
    verifying = { ...verifying, [id]: 'busy' };
    try {
      const r = await adminVerifyBackup(id);
      verifying = { ...verifying, [id]: !!r?.verified };
      if (r?.verified) toastSuccess($t('pages.adminMaintenance.backups.verifiedOk', { values: { id } }));
      else toastError($t('pages.adminMaintenance.backups.verifiedBad', { values: { id } }));
    } catch (e) {
      verifying = Object.fromEntries(Object.entries(verifying).filter(([k]) => k !== id));
      toastError((e as Error)?.message || String(e));
    }
  }

  async function stageRestore() {
    if (!restoreTarget) return;
    staging = true;
    try {
      await adminStageRestore(restoreTarget.id);
      toastSuccess($t('pages.adminMaintenance.backups.staged', { values: { id: restoreTarget.id } }));
      restoreTarget = null;
      await loadBackups();
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    } finally {
      staging = false;
    }
  }

  async function cancelRestore() {
    const ok = await askConfirm({
      title: $t('pages.adminMaintenance.backups.cancelTitle'),
      message: $t('pages.adminMaintenance.backups.cancelMessage', { values: { id: status?.pending_restore?.id } }),
      confirmLabel: $t('pages.adminMaintenance.backups.cancelConfirm'),
      variant: 'warning',
    });
    if (!ok) return;
    try {
      await adminCancelRestore();
      toastSuccess($t('pages.adminMaintenance.backups.cancelled'));
      await loadBackups();
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    }
  }

  async function loadIndex() {
    indexError = '';
    try {
      index = await textSearchStatus();
    } catch (e) {
      indexError = (e as Error)?.message || String(e);
    }
  }

  async function reindex() {
    const ok = await askConfirm({
      title: $t('pages.adminMaintenance.index.confirmTitle'),
      message: $t('pages.adminMaintenance.index.confirmMessage'),
      confirmLabel: $t('pages.adminMaintenance.index.reindex'),
      variant: 'warning',
    });
    if (!ok) return;
    reindexing = true;
    try {
      const r = await textSearchReindex();
      toastSuccess($t('pages.adminMaintenance.index.done', { values: { n: (r?.indexed ?? 0).toLocaleString() } }));
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    } finally {
      reindexing = false;
      await loadIndex();
    }
  }

  $: s3 = status?.s3;
  $: s3State = !s3 ? '' : !s3.enabled ? 'off' : !s3.object_store_configured ? 'noStore' : 'on';
</script>

<div class="admin-maint">
  <div class="header-row">
    <div>
      <h2><HardDrive size={20} /> {$t('pages.adminMaintenance.title')}</h2>
      <p class="subtitle">{$t('pages.adminMaintenance.detail')}</p>
    </div>
  </div>

  <!-- ── Backups ───────────────────────────────────────────────────────── -->
  <section class="card" data-testid="backups-card">
    <div class="xc-head">
      <h3>{$t('pages.adminMaintenance.backups.title')}</h3>
      <span class="xc-spacer"></span>
      {#if $isSuperAdmin && status?.configured}
        <button class="btn btn-sm btn-ghost" on:click={loadBackups} disabled={loadingBackups}>
          {#if loadingBackups}<Loader2 size={13} class="animate-spin" />{:else}<RefreshCw size={13} />{/if}
          {$t('pages.adminMaintenance.refresh')}
        </button>
        <button class="btn btn-sm" on:click={createBackup} disabled={creating}>
          {#if creating}<Loader2 size={13} class="animate-spin" />{:else}<Plus size={13} />{/if}
          {$t('pages.adminMaintenance.backups.create')}
        </button>
      {/if}
    </div>

    {#if !$isSuperAdmin}
      <p class="xc-muted">{$t('pages.adminMaintenance.backups.superAdminOnly')}</p>
    {:else if backupError}
      <p class="xc-error"><CircleAlert size={14} /> {backupError}</p>
    {:else if !status}
      <p class="xc-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
    {:else if !status.configured}
      <p class="xc-muted">{$t('pages.adminMaintenance.backups.notConfigured')}</p>
    {:else}
      <dl class="xc-facts">
        <div class="xc-wide"><dt>{$t('pages.adminMaintenance.backups.dir')}</dt><dd><code class="xc-code">{status.dir}</code></dd></div>
        <div><dt>{$t('pages.adminMaintenance.backups.schedule')}</dt><dd>{$t('pages.adminMaintenance.backups.everyHours', { values: { n: Math.round((status.schedule_secs / 3600) * 100) / 100 } })}</dd></div>
        <div><dt>{$t('pages.adminMaintenance.backups.retention')}</dt><dd>{$t('pages.adminMaintenance.backups.keepN', { values: { n: status.retention } })}</dd></div>
        <div>
          <dt>{$t('pages.adminMaintenance.backups.encryption')}</dt>
          <dd>{status.encrypted ? $t('pages.adminMaintenance.backups.encryptedYes') : $t('pages.adminMaintenance.backups.encryptedNo')}</dd>
        </div>
        <div>
          <dt>{$t('pages.adminMaintenance.backups.s3')}</dt>
          <dd data-testid="s3-status">
            <span class="xc-badge" class:xc-ok={s3State === 'on'} class:xc-warn={s3State === 'noStore'}>
              <CloudUpload size={12} /> {$t(`pages.adminMaintenance.backups.s3State.${s3State}`)}
            </span>
          </dd>
        </div>
      </dl>
      {#if s3?.last_upload}
        <p class="xc-muted">
          {s3.last_upload.ok
            ? $t('pages.adminMaintenance.backups.s3LastOk', { values: { id: s3.last_upload.id, when: when(s3.last_upload.at) } })
            : $t('pages.adminMaintenance.backups.s3LastFailed', { values: { id: s3.last_upload.id, when: when(s3.last_upload.at), error: s3.last_upload.error } })}
        </p>
      {/if}

      {#if status.pending_restore}
        <div class="xc-callout xc-warn" role="status" data-testid="pending-restore">
          <ArchiveRestore size={15} />
          <div>
            <strong>{$t('pages.adminMaintenance.backups.pendingTitle', { values: { id: status.pending_restore.id } })}</strong>
            <p class="xc-muted">{$t('pages.adminMaintenance.backups.pendingHint', { values: { when: when(status.pending_restore.requested_at) } })}</p>
            <button class="btn btn-sm btn-ghost" on:click={cancelRestore}><XIcon size={13} /> {$t('pages.adminMaintenance.backups.cancelRestore')}</button>
          </div>
        </div>
      {/if}
      {#if status.last_restore}
        <div class="xc-callout" class:xc-ok={status.last_restore.ok} class:xc-bad={!status.last_restore.ok} role="status">
          {#if status.last_restore.ok}<CheckCircle2 size={15} />{:else}<CircleAlert size={15} />{/if}
          <div>
            {status.last_restore.ok
              ? $t('pages.adminMaintenance.backups.lastRestoreOk', { values: { id: status.last_restore.id, when: when(status.last_restore.finished_at), quads: (status.last_restore.quad_count ?? 0).toLocaleString() } })
              : $t('pages.adminMaintenance.backups.lastRestoreFailed', { values: { id: status.last_restore.id, when: when(status.last_restore.finished_at), error: status.last_restore.error } })}
          </div>
        </div>
      {/if}

      {#if backups.length === 0}
        <p class="xc-muted">{$t('pages.adminMaintenance.backups.none')}</p>
      {:else}
        <div class="xc-table-wrap">
          <table class="xc-table" data-testid="backups-table">
            <thead>
              <tr>
                <th>{$t('pages.adminMaintenance.backups.id')}</th>
                <th>{$t('pages.adminMaintenance.backups.createdAt')}</th>
                <th>{$t('pages.adminMaintenance.backups.quads')}</th>
                <th>{$t('pages.adminMaintenance.backups.version')}</th>
                <th>{$t('pages.adminMaintenance.backups.checksums')}</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {#each backups as b (b.id)}
                <tr>
                  <td>
                    <code>{b.id}</code>
                    {#if b.encrypted}<span class="xc-badge">{$t('pages.adminMaintenance.backups.encryptedBadge')}</span>{/if}
                    {#if status.pending_restore?.id === b.id}<span class="xc-badge xc-warn">{$t('pages.adminMaintenance.backups.stagedBadge')}</span>{/if}
                  </td>
                  <td>{when(b.created_at)}</td>
                  <td>{b.rdf_quad_count.toLocaleString()}</td>
                  <td>{b.software_version}</td>
                  <td>
                    {#if verifying[b.id] === 'busy'}
                      <Loader2 size={12} class="animate-spin" />
                    {:else if verifying[b.id] === true}
                      <span class="xc-badge xc-ok">{$t('pages.adminMaintenance.backups.intact')}</span>
                    {:else if verifying[b.id] === false}
                      <span class="xc-badge xc-bad">{$t('pages.adminMaintenance.backups.corrupt')}</span>
                    {:else}
                      <span class="xc-muted">—</span>
                    {/if}
                  </td>
                  <td class="row-actions">
                    <button class="btn btn-xs btn-ghost" on:click={() => verify(b.id)} disabled={verifying[b.id] === 'busy'}>
                      <ShieldCheck size={12} /> {$t('pages.adminMaintenance.backups.verify')}
                    </button>
                    <button class="btn btn-xs btn-ghost" on:click={() => (restoreTarget = b)}>
                      <ArchiveRestore size={12} /> {$t('pages.adminMaintenance.backups.restore')}
                    </button>
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {/if}
  </section>

  <!-- ── Full-text index ───────────────────────────────────────────────── -->
  <section class="card" data-testid="index-card">
    <div class="xc-head">
      <Search size={15} />
      <h3>{$t('pages.adminMaintenance.index.title')}</h3>
      <span class="xc-spacer"></span>
      <button class="btn btn-sm btn-ghost" on:click={loadIndex}><RefreshCw size={13} /> {$t('pages.adminMaintenance.refresh')}</button>
      {#if index?.available}
        <button class="btn btn-sm" on:click={reindex} disabled={reindexing || index.reindexing}>
          {#if reindexing || index.reindexing}<Loader2 size={13} class="animate-spin" />{:else}<RefreshCw size={13} />{/if}
          {$t('pages.adminMaintenance.index.reindex')}
        </button>
      {/if}
    </div>
    <p class="xc-intro">{$t('pages.adminMaintenance.index.intro')}</p>
    {#if indexError}
      <p class="xc-error"><CircleAlert size={14} /> {indexError}</p>
    {:else if !index}
      <p class="xc-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
    {:else if !index.available}
      <p class="xc-muted">{$t('pages.adminMaintenance.index.unavailable')} ({index.reason})</p>
    {:else}
      <dl class="xc-facts" data-testid="index-status">
        <div><dt>{$t('pages.adminMaintenance.index.documents')}</dt><dd>{index.documents.toLocaleString()}</dd></div>
        <div>
          <dt>{$t('pages.adminMaintenance.index.freshness')}</dt>
          <dd>
            <span class="xc-badge" class:xc-ok={!index.stale} class:xc-warn={index.stale}>
              {index.stale ? $t('pages.adminMaintenance.index.stale') : $t('pages.adminMaintenance.index.fresh')}
            </span>
          </dd>
        </div>
        <div>
          <dt>{$t('pages.adminMaintenance.index.substring')}</dt>
          <dd>{index.substring_pushdown ? $t('pages.adminMaintenance.index.substringOn') : $t('pages.adminMaintenance.index.substringOff')}</dd>
        </div>
        <div>
          <dt>{$t('pages.adminMaintenance.index.lastReindex')}</dt>
          <dd>
            {#if index.reindexing || reindexing}
              {$t('pages.adminMaintenance.index.running')}
            {:else if index.last_reindex}
              {#if index.last_reindex.error}
                <span class="xc-badge xc-bad">{$t('pages.adminMaintenance.index.failed')}</span> {index.last_reindex.error}
              {:else}
                {$t('pages.adminMaintenance.index.lastDone', { values: { when: when(index.last_reindex.finished_at), n: (index.last_reindex.indexed ?? 0).toLocaleString(), secs: (index.last_reindex.duration_ms / 1000).toFixed(1) } })}
              {/if}
            {:else}
              {$t('pages.adminMaintenance.index.never')}
            {/if}
          </dd>
        </div>
      </dl>
    {/if}
  </section>
</div>

{#if restoreTarget}
  <ConfirmModal
    title={$t('pages.adminMaintenance.backups.restoreTitle', { values: { id: restoreTarget.id } })}
    message={$t('pages.adminMaintenance.backups.restoreMessage', { values: { when: when(restoreTarget.created_at), quads: restoreTarget.rdf_quad_count.toLocaleString() } })}
    confirmLabel={$t('pages.adminMaintenance.backups.restoreConfirm')}
    cancelLabel={$t('system.cancel')}
    confirmVariant="danger"
    requirePhrase={restoreTarget.id}
    loading={staging}
    on:confirm={stageRestore}
    on:cancel={() => (restoreTarget = null)}
  />
{/if}

<style>
  .admin-maint { display: grid; gap: 1rem; max-width: 72rem; }
  .header-row { display: flex; justify-content: space-between; align-items: flex-start; gap: 1rem; flex-wrap: wrap; }
  .header-row h2 { display: flex; align-items: center; gap: 0.5rem; margin: 0; }
  .subtitle { margin: 0.25rem 0 0; color: var(--ink-500); font-size: 0.9rem; }
  .row-actions { white-space: nowrap; text-align: right; }
</style>
