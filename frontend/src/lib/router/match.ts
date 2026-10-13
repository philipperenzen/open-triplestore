/**
 * Match a route pattern (`/datasets/:id/viewer`) against an app path. Returns
 * the named parameters, or null when the path does not match. Shared by
 * `<Route>` (render when matched) and `<Fallback>` (render when nothing is).
 */
export function matchPath(pattern: string, pathname: string): Record<string, string> | null {
  const paramNames: string[] = [];
  const regexStr =
    '^' +
    pattern
      .replace(/[.+*?^${}()|[\]\\]/g, '\\$&')
      .replace(/:([a-zA-Z_][a-zA-Z0-9_]*)/g, (_, name: string) => {
        paramNames.push(name);
        return '([^/]+)';
      }) +
    '$';
  const match = pathname.match(new RegExp(regexStr));
  if (!match) return null;
  const params: Record<string, string> = {};
  paramNames.forEach((name, i) => { params[name] = match[i + 1]; });
  return params;
}
