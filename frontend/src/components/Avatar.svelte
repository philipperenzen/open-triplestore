<script>
  import { getUserAvatarUrl, getOrgImageUrl, getDatasetImageUrl } from '../lib/api.js';

  /** 'user' | 'organisation' | 'dataset' */
  export let kind = 'user';
  export let id = '';
  /** Passed straight through: when falsy (and not a placeholder), renders initials. */
  export let hasImage = false;
  export let name = '';
  export let size = 24;
  export let cacheKey = '';

  $: url = !id ? null
    : kind === 'user' ? getUserAvatarUrl(id)
    : kind === 'organisation' ? getOrgImageUrl(id)
    : kind === 'dataset' ? getDatasetImageUrl(id)
    : null;
  $: src = url ? `${url}${cacheKey ? `?v=${cacheKey}` : ''}` : null;

  $: initials = (name || id || '?')
    .split(/[\s_-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map(s => s[0]?.toUpperCase() ?? '')
    .join('') || '?';

  let errored = false;
</script>

{#if hasImage && src && !errored}
  <img
    class="avatar"
    style="width:{size}px; height:{size}px;"
    {src}
    alt={name || id}
    title={name || id}
    on:error={() => (errored = true)}
  />
{:else}
  <span
    class="avatar avatar-fallback"
    style="width:{size}px; height:{size}px; font-size:{Math.round(size * 0.42)}px;"
    title={name || id}
  >{initials}</span>
{/if}

<style>
  .avatar {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    border-radius: 50%;
    object-fit: cover;
    background: #e2e8f0;
    color: #475569;
    font-weight: 600;
    vertical-align: middle;
    border: 1px solid #e2e8f0;
  }
  .avatar-fallback { user-select: none; }
</style>
