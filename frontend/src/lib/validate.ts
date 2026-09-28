// Client-side account-field validation, mirroring the server rules in
// src/auth/validate.rs (the server remains authoritative).

/** Pragmatic email check: dot-atom local part @ dotted domain with a TLD. */
export function validateEmail(email: string): string | null {
  const e = email.trim();
  if (!e) return 'required';
  if (e.length > 254) return 'tooLong';
  const at = e.lastIndexOf('@');
  if (at < 1 || at === e.length - 1) return 'format';
  const local = e.slice(0, at);
  const domain = e.slice(at + 1);
  if (local.length > 64) return 'tooLong';
  if (local.startsWith('.') || local.endsWith('.') || local.includes('..')) return 'format';
  if (!/^[A-Za-z0-9!#$%&'*+/=?^_`{|}~.-]+$/.test(local)) return 'format';
  const labels = domain.split('.');
  if (labels.length < 2) return 'domain';
  for (const label of labels) {
    if (!label || label.length > 63) return 'domain';
    if (label.startsWith('-') || label.endsWith('-')) return 'domain';
    if (!/^[A-Za-z0-9-]+$/.test(label)) return 'domain';
  }
  const tld = labels[labels.length - 1];
  if (tld.length < 2 || /^[0-9]+$/.test(tld)) return 'domain';
  return null;
}

/** 3–50 chars, letters/digits plus . _ -, starting with a letter or digit. */
export function validateUsername(username: string): string | null {
  if (username.length < 3 || username.length > 50) return 'length';
  if (!/^[A-Za-z0-9]/.test(username)) return 'start';
  if (!/^[A-Za-z0-9._-]+$/.test(username)) return 'charset';
  return null;
}

/** 8–1024 characters. */
export function validatePassword(password: string): string | null {
  if (password.length < 8) return 'tooShort';
  if (password.length > 1024) return 'tooLong';
  return null;
}

// ── Prefix overrides ─────────────────────────────────────────────────────────
//
// Mirrors `is_valid_label` / `is_valid_iri` in src/prefixes/mod.rs so an
// administrator hears about a malformed label before spending a round trip on
// it. The server still decides: these checks only shorten the feedback loop,
// and a label this function accepts can still be refused with a 400.

/** The server's MAX_LABEL_LENGTH. */
export const PREFIX_LABEL_MAX_LENGTH = 64;

/**
 * A prefix label: an ASCII letter, then letters, digits, `_` or `-`.
 *
 * Spaces are excluded by the charset rather than checked separately — a label
 * goes into a `PREFIX label: <IRI>` declaration, where a space would end it.
 */
export function validatePrefixLabel(label: string): string | null {
  if (!label) return 'required';
  if (label.length > PREFIX_LABEL_MAX_LENGTH) return 'tooLong';
  if (!/^[A-Za-z]/.test(label)) return 'start';
  if (!/^[A-Za-z][A-Za-z0-9_-]*$/.test(label)) return 'charset';
  return null;
}

/**
 * A namespace: an absolute `http` or `https` IRI.
 *
 * Parsed rather than pattern-matched, because the schemes that matter to
 * exclude — `javascript:`, `data:`, `file:` — are exactly the ones a regex over
 * "looks like a URL" tends to let through. No base is passed, so a relative
 * reference fails to parse and is refused, which is what we want: a namespace
 * that only resolves against whatever page happened to be open is not a
 * namespace.
 */
export function validatePrefixNamespace(namespace: string): string | null {
  if (!namespace) return 'required';
  let url: URL;
  try {
    url = new URL(namespace);
  } catch {
    return 'format';
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') return 'scheme';
  return null;
}
