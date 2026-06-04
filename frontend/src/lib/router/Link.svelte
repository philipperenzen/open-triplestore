<script lang="ts">
  import { navigate } from '../locationStore.js';

  export let to: string;
  export let replace: boolean = false;

  function handleClick(e: MouseEvent) {
    // Let browser handle modified clicks (new tab, etc.)
    if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey || e.button !== 0) return;
    e.preventDefault();
    navigate(to, { replace });
  }
</script>

<a href={to} on:click={handleClick} {...$$restProps}><slot /></a>
