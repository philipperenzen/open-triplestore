// The dataset's event-stream settings: readers see the stream, writers get
// the form; a bad duration is caught before the request; a good form sends
// the body PUT /api/datasets/:id/ldes expects; turning the stream off asks
// first, in the app's own dialog.
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { render, cleanup, fireEvent, waitFor } from '@testing-library/svelte';
import { init, addMessages, locale } from 'svelte-i18n';
import en from '../i18n/en.json';
import nl from '../i18n/nl.json';

const api = vi.hoisted(() => ({ getLdesConfig: vi.fn(), putLdesStream: vi.fn() }));
vi.mock('../api', () => api);
const confirm = vi.hoisted(() => ({ askConfirm: vi.fn() }));
vi.mock('../confirm', () => confirm);

import LdesPublishPanel from '../../components/exchange/LdesPublishPanel.svelte';

const OFF = {
  dataset_id: 'd1', enabled: false, configured: false, page_size: 100, polling_interval: 60, retention: null,
  members: 0, sealed_nodes: 0, created_at: null, stream: 'https://x.test/api/datasets/d1/ldes', can_write: true,
};
const ON = { ...OFF, enabled: true, configured: true, members: 1234, sealed_nodes: 12, retention: { version_amount: 2 } };

beforeAll(() => {
  addMessages('en', en as never);
  addMessages('nl', nl as never);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  locale.set('en');
});

const label = (container: HTMLElement, text: string) =>
  [...container.querySelectorAll('label.xc-field')].find((l) => l.textContent?.includes(text))!.querySelector('input')!;

describe('LdesPublishPanel', () => {
  it('shows a publishing stream with its URL, members and pages', async () => {
    api.getLdesConfig.mockResolvedValue({ ...ON, can_write: false });
    const { findByText, container } = render(LdesPublishPanel, { datasetId: 'd1' });
    await findByText('Publishing');
    expect(container.textContent).toContain('https://x.test/api/datasets/d1/ldes');
    expect(container.textContent).toContain('1,234');
    expect(container.textContent).toContain('Policy declared');
    expect(container.querySelector('form')).toBeNull();
  });

  it('catches a malformed duration without a request', async () => {
    api.getLdesConfig.mockResolvedValue(OFF);
    const { findByText, container, getByRole } = render(LdesPublishPanel, { datasetId: 'd1' });
    await findByText('Not published');
    await fireEvent.input(label(container, 'Keep everything for'), { target: { value: '30 days' } });
    await fireEvent.click(getByRole('button', { name: /Save stream settings/ }));
    expect(container.textContent).toContain(en.components.exchange.ldes.errors.fullLogDuration);
    expect(api.putLdesStream).not.toHaveBeenCalled();
  });

  it('enables the stream with its fragmentation and retention', async () => {
    api.getLdesConfig.mockResolvedValueOnce(OFF).mockResolvedValue(ON);
    api.putLdesStream.mockResolvedValue({ members_seeded: 7 });
    const { findByText, container, getByRole } = render(LdesPublishPanel, { datasetId: 'd1' });
    await findByText('Not published');
    await fireEvent.click(container.querySelector('input[type="checkbox"]')!);
    expect(container.textContent).toContain('first members');
    await fireEvent.input(label(container, 'Members per page'), { target: { value: '50' } });
    await fireEvent.input(label(container, 'Versions per entity'), { target: { value: '2' } });
    await fireEvent.input(label(container, 'Keep deletions for'), { target: { value: 'P7D' } });
    await fireEvent.click(getByRole('button', { name: /Save stream settings/ }));
    await waitFor(() => expect(api.putLdesStream).toHaveBeenCalled());
    expect(api.putLdesStream).toHaveBeenCalledWith('d1', {
      enabled: true, page_size: 50, polling_interval: 60,
      retention: { version_amount: 2, version_delete_duration: 'P7D' },
    });
    await findByText('Publishing');
  });

  it('asks before it stops publishing, and does nothing on cancel', async () => {
    api.getLdesConfig.mockResolvedValue(ON);
    confirm.askConfirm.mockResolvedValue(false);
    const { findByText, container, getByRole } = render(LdesPublishPanel, { datasetId: 'd1' });
    await findByText('Publishing');
    await fireEvent.click(container.querySelector('input[type="checkbox"]')!);
    await fireEvent.click(getByRole('button', { name: /Save stream settings/ }));
    await waitFor(() => expect(confirm.askConfirm).toHaveBeenCalled());
    expect(api.putLdesStream).not.toHaveBeenCalled();
  });

  it('speaks Dutch', async () => {
    locale.set('nl');
    api.getLdesConfig.mockResolvedValue(ON);
    const { findByText } = render(LdesPublishPanel, { datasetId: 'd1' });
    await findByText('Gepubliceerd');
    await findByText(nl.components.exchange.ldes.save);
  });
});
