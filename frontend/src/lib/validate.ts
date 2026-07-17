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
