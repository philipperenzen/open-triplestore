// The app's own dialogs (lib/confirm.ts + ConfirmHost), which replaced 11
// window.confirm() and 3 window.prompt() calls; 31 window.alert() calls became
// error toasts. Browser dialogs block the page, ignore the app's language and
// theme, and embedding contexts may suppress them, so none may come back.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { render, cleanup, fireEvent } from '@testing-library/svelte';
import { tick } from 'svelte';
import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { askConfirm, askText } from '../confirm';
import ConfirmHost from '../../components/ConfirmHost.svelte';

const SRC = path.resolve(__dirname, '../..');

function sourceFiles(dir: string, acc: string[] = []): string[] {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === '__tests__' || entry.name === 'node_modules') continue;
      sourceFiles(full, acc);
    } else if (/\.(svelte|ts|js)$/.test(entry.name)) {
      acc.push(full);
    }
  }
  return acc;
}

beforeAll(() => {
  addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
  init({ fallbackLocale: 'en', initialLocale: 'en' });
});

afterEach(cleanup);

describe('askConfirm', () => {
  it('resolves true when the user confirms', async () => {
    const view = render(ConfirmHost);
    const answer = askConfirm({ title: 'Delete it?', message: 'Gone for good.', confirmLabel: 'Delete' });
    await tick();
    expect(view.getByRole('dialog').textContent).toContain('Gone for good.');
    await fireEvent.click(view.getByRole('button', { name: 'Delete' }));
    expect(await answer).toBe(true);
    await tick();
    expect(view.queryByRole('dialog')).toBeNull();
  });

  it('resolves false on Cancel, with a translated cancel label', async () => {
    const view = render(ConfirmHost);
    const answer = askConfirm({ title: 'Restore?' });
    await tick();
    await fireEvent.click(view.getByRole('button', { name: 'Cancel' }));
    expect(await answer).toBe(false);
  });

  it('answers an open request "no" when a second one replaces it', async () => {
    render(ConfirmHost);
    const first = askConfirm({ title: 'First?' });
    const second = askConfirm({ title: 'Second?' });
    expect(await first).toBe(false);
    void second;
  });
});

describe('askText', () => {
  it('returns the trimmed answer, prefilled with the default', async () => {
    const view = render(ConfirmHost);
    const answer = askText({ title: 'Name?', label: 'Name', defaultValue: 'draft', confirmLabel: 'Create' });
    await tick();
    const input = view.getByLabelText('Name') as HTMLInputElement;
    expect(input.value).toBe('draft');
    await fireEvent.input(input, { target: { value: '  shapes v2  ' } });
    await fireEvent.click(view.getByRole('button', { name: 'Create' }));
    expect(await answer).toBe('shapes v2');
  });

  it('treats Enter as confirm and an empty answer or Cancel as no answer', async () => {
    const view = render(ConfirmHost);
    const enter = askText({ title: 'Version?', label: 'Version' });
    await tick();
    const input = view.getByLabelText('Version');
    await fireEvent.input(input, { target: { value: '2.0.0' } });
    await fireEvent.keyDown(input, { key: 'Enter' });
    expect(await enter).toBe('2.0.0');

    const empty = askText({ title: 'Version?', label: 'Version', confirmLabel: 'OK' });
    await tick();
    await fireEvent.click(view.getByRole('button', { name: 'OK' }));
    expect(await empty).toBeNull();

    const cancelled = askText({ title: 'Version?', defaultValue: 'x' });
    await tick();
    await fireEvent.click(view.getByRole('button', { name: 'Cancel' }));
    expect(await cancelled).toBeNull();
  });
});

describe('browser dialogs', () => {
  it('are not used anywhere in the app', () => {
    const offenders: string[] = [];
    for (const f of sourceFiles(SRC)) {
      const lines = fs.readFileSync(f, 'utf8').split('\n');
      lines.forEach((line, i) => {
        const code = line.replace(/\/\/.*$/, '').replace(/^\s*\*.*$/, '');
        if (/(^|[^\w.$])(?<!function )(window\.)?(alert|confirm|prompt)\(/.test(code)) {
          offenders.push(`${path.relative(SRC, f)}:${i + 1}: ${line.trim()}`);
        }
      });
    }
    expect(offenders).toEqual([]);
  });
});
