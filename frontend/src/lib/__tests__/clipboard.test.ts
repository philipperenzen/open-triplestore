import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// The toast is the half of `copyOrWarn` under test; i18n is initialised so the
// assertion sees the real sentence rather than the key.
const toasts = vi.hoisted(() => ({
  toastWarn: vi.fn(),
  toast: vi.fn(),
  toastSuccess: vi.fn(),
  toastError: vi.fn(),
  toastInfo: vi.fn(),
  dismiss: vi.fn(),
  toasts: { subscribe: () => () => {} },
}));
vi.mock('../toast', () => toasts);

import { init, addMessages } from 'svelte-i18n';
import en from '../i18n/en.json';
import { copyToClipboard, copyOrWarn } from '../clipboard.js';

addMessages('en', en as unknown as Parameters<typeof addMessages>[1]);
init({ fallbackLocale: 'en', initialLocale: 'en' });

// jsdom does not implement document.execCommand, so we assign a stub directly
// (vi.spyOn can't attach to a missing property) and restore it afterwards.
const hadExec = 'execCommand' in document;
const originalExec = (document as { execCommand?: unknown }).execCommand;

function stubExec(result: boolean) {
  const fn = vi.fn().mockReturnValue(result);
  (document as unknown as { execCommand: unknown }).execCommand = fn;
  return fn;
}

describe('copyToClipboard', () => {
  beforeEach(() => {
    delete (document as { execCommand?: unknown }).execCommand;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    if (hadExec) {
      (document as unknown as { execCommand: unknown }).execCommand = originalExec;
    } else {
      delete (document as { execCommand?: unknown }).execCommand;
    }
  });

  it('uses navigator.clipboard.writeText in a secure context', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal('navigator', { clipboard: { writeText } });

    const ok = await copyToClipboard('ots_secret_token');

    expect(ok).toBe(true);
    expect(writeText).toHaveBeenCalledWith('ots_secret_token');
  });

  it('falls back to execCommand when navigator.clipboard is missing (insecure HTTP/LAN context)', async () => {
    // Plain HTTP on a LAN: navigator exists but has no clipboard — the regression.
    vi.stubGlobal('navigator', {});
    const exec = stubExec(true);

    const ok = await copyToClipboard('ots_secret_token');

    expect(ok).toBe(true);
    expect(exec).toHaveBeenCalledWith('copy');
    // The transient textarea must be cleaned up.
    expect(document.querySelector('textarea')).toBeNull();
  });

  it('falls back to execCommand when writeText rejects (permission denied / not focused)', async () => {
    const writeText = vi.fn().mockRejectedValue(new Error('NotAllowedError'));
    vi.stubGlobal('navigator', { clipboard: { writeText } });
    const exec = stubExec(true);

    const ok = await copyToClipboard('value');

    expect(ok).toBe(true);
    expect(exec).toHaveBeenCalledWith('copy');
  });

  it('returns false (no throw) when both methods fail', async () => {
    vi.stubGlobal('navigator', {});
    stubExec(false);

    await expect(copyToClipboard('value')).resolves.toBe(false);
  });
});

// A copy can fail for reasons the user cannot see — an unfocused document, a
// store reached over plain HTTP — and a `false` nobody surfaces reads as a
// dead button. `copyOrWarn` keeps the boolean and adds the sentence.
describe('copyOrWarn', () => {
  beforeEach(() => {
    toasts.toastWarn.mockClear();
    delete (document as { execCommand?: unknown }).execCommand;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('says nothing when the copy works', async () => {
    vi.stubGlobal('navigator', { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });

    await expect(copyOrWarn('urn:x')).resolves.toBe(true);

    expect(toasts.toastWarn).not.toHaveBeenCalled();
  });

  it('tells the user what to do instead when every path fails', async () => {
    vi.stubGlobal('navigator', {});
    (document as unknown as { execCommand: unknown }).execCommand = vi.fn().mockReturnValue(false);

    await expect(copyOrWarn('urn:x')).resolves.toBe(false);

    expect(toasts.toastWarn).toHaveBeenCalledTimes(1);
    const msg = String(toasts.toastWarn.mock.calls[0][0]);
    expect(msg).toContain('Ctrl/Cmd');
    expect(msg).not.toBe('system.copyFailed'); // the sentence, not the key
  });
});
