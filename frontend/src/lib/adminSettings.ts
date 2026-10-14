/**
 * Pure helpers behind the admin Settings page (`/admin/settings`): the
 * runtime settings an administrator can change without a restart
 * (`GET/PUT /api/admin/settings`) and the read-only effective configuration
 * (`GET /api/admin/config`). No DOM, no fetch — tested as functions.
 */

/** Where a value came from, as the server reports it. */
export type SettingSource = 'cli' | 'env' | 'file' | 'profile' | 'default' | 'admin';

/** One runtime setting (`GET /api/admin/settings`). */
export interface RuntimeSetting {
  name: string;
  group: string;
  kind: string;
  doc: string;
  choices?: string[];
  value: string | null;
  source: SettingSource;
  /** What applies without the override (environment, profile or default). */
  configured: string | null;
  configured_source: SettingSource;
  overridden: boolean;
}

/** One setting of the effective configuration (`GET /api/admin/config`). */
export interface ConfigSetting {
  name: string;
  group: string;
  kind: string;
  value: string | null;
  default: string | null;
  source: SettingSource;
  secret: boolean;
  runtime: boolean;
  doc: string;
  choices?: string[];
}

/** How a runtime setting is edited. */
export type Control = 'toggle' | 'select' | 'text';

export function controlFor(s: Pick<RuntimeSetting, 'kind' | 'choices'>): Control {
  if (s.kind === 'bool') return 'toggle';
  if (s.kind === 'enum' && Array.isArray(s.choices) && s.choices.length > 0) return 'select';
  return 'text';
}

/** Settings grouped by their `group`, groups and members in server order. */
export function groupSettings<T extends { group: string }>(settings: T[]): { group: string; settings: T[] }[] {
  const groups: { group: string; settings: T[] }[] = [];
  for (const s of settings) {
    let g = groups.find((x) => x.group === s.group);
    if (!g) {
      g = { group: s.group, settings: [] };
      groups.push(g);
    }
    g.settings.push(s);
  }
  return groups;
}

/** Whether `draft` differs from the setting's effective value. A blank draft
 *  is never a change: an empty value is not a value (reset instead). */
export function isDirty(s: Pick<RuntimeSetting, 'value'>, draft: string | null | undefined): boolean {
  if (draft === null || draft === undefined) return false;
  const d = draft.trim();
  return d !== '' && d !== (s.value ?? '');
}

/** The effective configuration narrowed to settings whose name, group,
 *  value or documentation contains `query` (case-insensitive); with
 *  `changedOnly`, only those not at their built-in default. */
export function filterConfig(settings: ConfigSetting[], query: string, changedOnly = false): ConfigSetting[] {
  const q = query.trim().toLowerCase();
  return settings.filter((s) => {
    if (changedOnly && s.source === 'default') return false;
    if (!q) return true;
    return [s.name, s.group, s.value ?? '', s.doc].some((x) => x.toLowerCase().includes(q));
  });
}
