// Canonical role / permission vocabularies for the UI.
//
// SINGLE SOURCE OF TRUTH for the frontend. These mirror the Rust enums in
// `src/auth/models.rs` (SystemRole, Role, ResourceRole, ApiScope, Visibility,
// graph-ACL permission, ACL principal types). Keep them in sync with the
// backend; dropdowns and gates should import from here rather than hardcoding
// string lists, so the two sides can't drift.

export type Option = { value: string; label: string };

/** System account roles — `SystemRole` (`super_admin` is assignable only by a super-admin). */
export const SYSTEM_ROLES: Option[] = [
  { value: 'user', label: 'User' },
  { value: 'admin', label: 'Admin' },
  { value: 'super_admin', label: 'Super Admin' },
];

/** Org / group membership roles — `Role`. */
export const MEMBERSHIP_ROLES: Option[] = [
  { value: 'admin', label: 'Admin' },
  { value: 'member', label: 'Member' },
  { value: 'viewer', label: 'Viewer' },
];

/** Per-resource grant levels — `ResourceRole`. Ordered weakest→strongest. */
export const RESOURCE_ROLES: Option[] = [
  { value: 'viewer', label: 'Viewer' },
  { value: 'editor', label: 'Editor' },
  { value: 'admin', label: 'Admin' },
];

/** Numeric rank for comparing resource roles (higher = stronger). */
export const RESOURCE_ROLE_RANK: Record<string, number> = { viewer: 1, editor: 2, admin: 3 };

/** API-token scopes — `ApiScope`. */
export const TOKEN_SCOPES: Option[] = [
  { value: 'read', label: 'read' },
  { value: 'write', label: 'write' },
  { value: 'admin', label: 'admin' },
];

/** Named-graph ACL permissions — graph-ACL `permission`. */
export const GRAPH_PERMISSIONS: Option[] = [
  { value: 'read', label: 'Read only' },
  { value: 'write', label: 'Write (includes read)' },
  { value: 'admin', label: 'Admin (includes write)' },
];

/** ACL principal types (endpoint & graph ACLs). `public` applies to graph ACLs only. */
export const ACL_PRINCIPAL_TYPES: Option[] = [
  { value: 'user', label: 'User' },
  { value: 'role', label: 'Role' },
  { value: 'organisation', label: 'Organisation' },
  { value: 'group', label: 'Group' },
];

/** Dataset visibility — `Visibility`. */
export const VISIBILITIES: Option[] = [
  { value: 'public', label: 'Public' },
  { value: 'members', label: 'Members only' },
  { value: 'private', label: 'Private' },
];

/** value → label lookup for visibility. */
export const VISIBILITY_LABEL: Record<string, string> =
  Object.fromEntries(VISIBILITIES.map((v) => [v.value, v.label]));
