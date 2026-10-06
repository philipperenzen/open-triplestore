<script lang="ts">
  import { navigate } from '../locationStore.js';
  import { withBase } from '../basePath';

  export let to: string;
  export let replace: boolean = false;

  function handleClick(e: MouseEvent) {
    // Let browser handle modified clicks (new tab, etc.)
    if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey || e.button !== 0) return;
    e.preventDefault();
    navigate(to, { replace });
  }
</script>

<!-- `to` is an app path; the rendered href carries the deployment base so a
     new-tab / copy-link / no-JS visit lands on the same page. -->
<a href={withBase(to)} on:click={handleClick} {...$$restProps}><slot /></a>
