// Admin maintenance: backups with the S3 status, verify, a restore staged
// only after typing the backup id in the app's dialog, the staged restore
// and its cancel; the text index status and a confirmed rebuild. Backups are
// super_admin-only on the server, so a plain admin is told so.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { user, authInitialized } from '../stores';

const api = vi.hoisted(() => ({
  adminBackupStatus: vi.fn(),
  adminListBackups: vi.fn(),
  adminCreateBackup: vi.fn(),
  adminVerifyBackup: vi.fn(),
  adminStageRestore: vi.fn(),
  adminCancelRestore: vi.fn(),
  textSearchStatus: vi.fn(),
  textSearchReindex: vi.fn(),
}));
vi.mock('../api', () => api);
vi.mock('../api.js', () => api);
const confirm = vi.hoisted(() => ({ askConfirm: vi.fn() }));
vi.mock('../confirm', () => confirm);

import AdminMaintenance from '../../pages/AdminMaintenance.svelte';

const B1 = { id: 'backup-20261006T080000Z', created_at: '2026-10-06T08:00:00Z', software_version: '0.7.0', rdf_quad_count: 4200, encrypted: false };
const B0 = { ...B1, id: 'backup-20261005T080000Z', created_at: '2026-10-05T08:00:00Z' };
const STATUS = {
  configured: true, dir: '/data/backups', retention: 7, encrypted: false, decrypt_identity_configured: false,
  schedule_secs: 86400, count: 2, latest: B1,
  s3: { enabled: true, object_store_configured: true, last_upload: { id: B1.id, at: '2026-10-06T08:00:05Z', ok: true, error: null } },
  pending_restore: null, last_restore: null,
};
const INDEX = { available: true, documents: 98765, stale: true, substring_pushdown: true, reindexing: false, last_reindex: null };

beforeAll(() => {
  addMessages('en', en as never);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  user.set({ id: 'u1', username: 'root', role: 'super_admin' } as never);
  authInitialized.set(true);
  api.adminBackupStatus.mockResolvedValue(STATUS);
  api.adminListBackups.mockResolvedValue([B1, B0]);
  api.textSearchStatus.mockResolvedValue(INDEX);
});

describe('AdminMaintenance', () => {
  it('lists backups with the schedule, retention and S3 copy', async () => {
    const { findByTestId, getByTestId } = render(AdminMaintenance);
    const table = await findByTestId('backups-table');
    expect(table.querySelectorAll('tbody tr')).toHaveLength(2);
    expect(getByTestId('backups-card').textContent).toContain('Every 24 hour(s)');
    expect(getByTestId('backups-card').textContent).toContain('The newest 7');
    expect(getByTestId('s3-status').textContent).toContain('On');
    expect(getByTestId('backups-card').textContent).toContain(`Last S3 copy: ${B1.id}`);
  });

  it('says when S3 is on but there is no object storage', async () => {
    api.adminBackupStatus.mockResolvedValue({ ...STATUS, s3: { enabled: true, object_store_configured: false, last_upload: null } });
    const { findByTestId } = render(AdminMaintenance);
    expect((await findByTestId('s3-status')).textContent).toContain('no object storage');
  });

  it('creates a backup and warns when its S3 copy failed', async () => {
    api.adminCreateBackup.mockResolvedValue({ ...B1, s3: { uploaded: false, error: 'bucket gone' } });
    const { findByTestId, getByRole } = render(AdminMaintenance);
    await findByTestId('backups-table');
    await fireEvent.click(getByRole('button', { name: /Back up now/ }));
    await waitFor(() => expect(api.adminCreateBackup).toHaveBeenCalled());
    await waitFor(() => expect(api.adminBackupStatus).toHaveBeenCalledTimes(2));
  });

  it('verifies a backup', async () => {
    api.adminVerifyBackup.mockResolvedValue({ id: B1.id, verified: false });
    const { findByTestId, getAllByRole } = render(AdminMaintenance);
    const table = await findByTestId('backups-table');
    await fireEvent.click(getAllByRole('button', { name: /Verify/ })[0]);
    await waitFor(() => expect(table.textContent).toContain('Mismatch'));
    expect(api.adminVerifyBackup).toHaveBeenCalledWith(B1.id);
  });

  it('stages a restore only once the backup id is typed', async () => {
    api.adminStageRestore.mockResolvedValue({ id: B0.id });
    const { findByTestId, getAllByRole, getByRole } = render(AdminMaintenance);
    await findByTestId('backups-table');
    await fireEvent.click(getAllByRole('button', { name: /Restore…/ })[1]);
    const stage = getByRole('button', { name: /Stage the restore/ }) as HTMLButtonElement;
    expect(document.body.textContent).toContain('replaces ALL data');
    expect(stage.disabled).toBe(true);
    const phrase = document.getElementById('confirm-phrase-input') as HTMLInputElement;
    expect(phrase.labels?.[0]?.textContent).toBe(`Type ${B0.id} to confirm`);
    await fireEvent.input(phrase, { target: { value: B0.id } });
    expect(stage.disabled).toBe(false);
    await fireEvent.click(stage);
    await waitFor(() => expect(api.adminStageRestore).toHaveBeenCalledWith(B0.id));
  });

  it('shows a staged restore and cancels it after asking', async () => {
    api.adminBackupStatus.mockResolvedValue({ ...STATUS, pending_restore: { id: B0.id, requested_by: 'u1', requested_at: '2026-10-06T09:00:00Z' } });
    confirm.askConfirm.mockResolvedValue(true);
    api.adminCancelRestore.mockResolvedValue('');
    const { findByTestId, getByRole } = render(AdminMaintenance);
    const pending = await findByTestId('pending-restore');
    expect(pending.textContent).toContain(`Restore of ${B0.id} staged`);
    await fireEvent.click(getByRole('button', { name: /Cancel the restore/ }));
    await waitFor(() => expect(api.adminCancelRestore).toHaveBeenCalled());
  });

  it('shows how the last restore went', async () => {
    api.adminBackupStatus.mockResolvedValue({ ...STATUS, last_restore: {
      id: B0.id, requested_by: 'u1', requested_at: '2026-10-06T09:00:00Z', finished_at: '2026-10-06T09:05:00Z',
      ok: false, error: 'checksum verification failed', quad_count: null,
    } });
    const { findByText } = render(AdminMaintenance);
    await findByText(/checksum verification failed/);
  });

  it('shows the index status and rebuilds it after confirming', async () => {
    confirm.askConfirm.mockResolvedValue(true);
    api.textSearchReindex.mockResolvedValue({ indexed: 100000 });
    const { findByTestId, getByRole } = render(AdminMaintenance);
    const status = await findByTestId('index-status');
    expect(status.textContent).toContain('98,765');
    expect(status.textContent).toContain('Writes waiting');
    await fireEvent.click(getByRole('button', { name: /Rebuild index/ }));
    await waitFor(() => expect(api.textSearchReindex).toHaveBeenCalled());
    await waitFor(() => expect(api.textSearchStatus).toHaveBeenCalledTimes(2));
  });

  it('tells a plain admin that backups are for super admins, and still shows the index', async () => {
    user.set({ id: 'u2', username: 'ops', role: 'admin' } as never);
    const { findByTestId, getByTestId } = render(AdminMaintenance);
    await findByTestId('index-status');
    expect(getByTestId('backups-card').textContent).toContain('super admins');
    expect(api.adminBackupStatus).not.toHaveBeenCalled();
  });
});
