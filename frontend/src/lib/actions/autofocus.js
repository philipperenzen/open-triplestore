/**
 * Svelte action: focus the node when it mounts.
 *
 * Use this instead of the `autofocus` HTML attribute. Programmatic
 * focus-on-mount gives the same UX (e.g. focusing a modal's first field)
 * without tripping the `a11y_autofocus` rule, which flags the static
 * attribute because it can disorient assistive-tech users on page load.
 * In a modal/popover that opens on demand, moving focus in is expected.
 */
export function autofocus(node) {
  node.focus();
}
