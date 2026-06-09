// Robust clipboard copy.
//
// The async Clipboard API (`navigator.clipboard`) is only available in a *secure
// context*: HTTPS, or `http://localhost`. A self-hosted instance reached over plain
// HTTP on a LAN/IP — a very common deployment here — has
// `navigator.clipboard === undefined`, so a naive `navigator.clipboard.writeText(…)`
// throws a TypeError and the copy silently fails (this is why "I can no longer copy
// my API token" happens once you stop using localhost).
//
// `copyToClipboard` tries the modern API first, then falls back to a hidden
// `<textarea>` + `document.execCommand('copy')`, which still works in insecure
// contexts and older browsers. It never throws; it resolves to whether the copy
// succeeded so callers can show accurate feedback.

/** Copy `text` to the clipboard. Resolves `true` on success, `false` otherwise. */
export async function copyToClipboard(text: string): Promise<boolean> {
  const value = String(text ?? '');

  // Preferred path: the async Clipboard API (secure contexts only).
  if (typeof navigator !== 'undefined' && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(value);
      return true;
    } catch {
      // Permission denied, document not focused, or insecure context — fall back.
    }
  }

  return legacyCopy(value);
}

/** Fallback copy via a transient off-screen textarea + `execCommand('copy')`. */
function legacyCopy(value: string): boolean {
  if (typeof document === 'undefined') return false;
  const textarea = document.createElement('textarea');
  try {
    textarea.value = value;
    textarea.setAttribute('readonly', '');
    // Keep it off-screen and non-disruptive (no scroll jump).
    textarea.style.position = 'fixed';
    textarea.style.top = '-9999px';
    textarea.style.left = '-9999px';
    textarea.style.opacity = '0';
    document.body.appendChild(textarea);
    textarea.focus();
    textarea.select();
    textarea.setSelectionRange(0, value.length);
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    if (textarea.parentNode) textarea.parentNode.removeChild(textarea);
  }
}
