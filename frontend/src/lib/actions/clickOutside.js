// Svelte action: invoke `callback` when a click lands outside `node`.
// Uses capture phase so it fires before inner click handlers stop propagation.
// Shared by floating panels (term popover, resource detail menus, …).
export function clickOutside(node, callback) {
  const handle = (e) => { if (!node.contains(e.target)) callback(); };
  document.addEventListener('click', handle, true);
  return {
    update(next) { callback = next; },
    destroy() { document.removeEventListener('click', handle, true); },
  };
}
