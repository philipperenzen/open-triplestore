import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { copyToClipboard } from '../clipboard.js';

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
