// Shared number/size formatting for page stats and file listings.

/** 12 345 678 → "12.3M" (locale-aware compact notation). */
export function compactNumber(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '0';
  return new Intl.NumberFormat(undefined, {
    notation: 'compact',
    maximumFractionDigits: 1,
  }).format(n);
}

/** Bytes → human-readable size ("47.0 MB"). */
export function formatBytes(b: number | null | undefined): string {
  if (!b || b <= 0) return '0 B';
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  if (b < 1024 * 1024 * 1024) return `${(b / 1024 / 1024).toFixed(1)} MB`;
  return `${(b / 1024 / 1024 / 1024).toFixed(2)} GB`;
}
