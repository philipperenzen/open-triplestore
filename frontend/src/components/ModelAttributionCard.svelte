<script lang="ts">
  // Licence and attribution of a registry entry's or version's content — the
  // record the server keeps for the bundled standard vocabularies it seeds
  // (GET /api/models…, `attribution`). The store keeps triples only, so the
  // bundled file's own notice header lives on in this record; this component is
  // where the model pages show it. `variant='full'` is the entry's card;
  // `variant='compact'` is the one-line credit under a version.
  import { t } from 'svelte-i18n';
  import { Scale } from 'lucide-svelte';
  import type { ModelAttribution } from '../lib/api';

  let {
    attribution = null,
    variant = 'full',
  }: { attribution?: ModelAttribution | null; variant?: 'full' | 'compact' } = $props();

  /** Only http(s) URLs become links; anything else renders as text. */
  function isWebUrl(u: string | null | undefined): u is string {
    return !!u && /^https?:\/\//i.test(u);
  }

  function uiBase(): string {
    try {
      return (import.meta as { env?: { BASE_URL?: string } }).env?.BASE_URL || '/';
    } catch {
      return '/';
    }
  }

  // A copy the server has not checked to be the bundled file, unchanged: a
  // draft, branch, merge or rebase, or a seeded copy that was edited or
  // differs from the file. Its downloads say it may have been modified.
  const possiblyModified = $derived(attribution?.unchanged === false);

  // The server gives the NOTICE link against its base URL, which need not be
  // the origin this UI is served from; the UI ships the same file at
  // /vocab/NOTICE.md, so link that.
  const noticeHref = $derived(
    attribution?.notice_url?.endsWith('/vocab/NOTICE.md')
      ? `${uiBase()}vocab/NOTICE.md`
      : attribution?.notice_url || `${uiBase()}vocab/NOTICE.md`,
  );
</script>

{#if attribution}
  {#if variant === 'compact'}
    <div class="ma-compact" data-testid="model-attribution-compact">
      <Scale size={11} class="ma-icon" />
      {#if attribution.licenses.length}
        {#each attribution.licenses as l, i (l.uri)}
          {#if i > 0}<span class="ma-sep">·</span>{/if}
          <a href={l.uri} target="_blank" rel="noopener noreferrer license">{l.name}</a>
        {/each}
      {:else}
        <span>{$t('pages.modelDetail.attribution.noLicence')}</span>
      {/if}
      {#if attribution.copyright.length}
        <span class="ma-sep">·</span><span class="ma-credit">{attribution.copyright[0]}</span>
      {/if}
      {#if attribution.no_derivatives}
        <span class="ma-badge" title={$t('pages.modelDetail.attribution.noDerivativesHint')}>{$t('pages.modelDetail.attribution.noDerivatives')}</span>
      {/if}
      {#if possiblyModified}
        <span class="ma-badge ma-badge-modified" data-testid="model-attribution-modified" title={$t('pages.modelDetail.attribution.possiblyModifiedHint')}>{$t('pages.modelDetail.attribution.possiblyModified')}</span>
      {/if}
      <span class="ma-sep">·</span>
      <a href={noticeHref} target="_blank" rel="noopener">{$t('pages.modelDetail.attribution.fullNotice')}</a>
    </div>
  {:else}
    <section class="ma-card" data-testid="model-attribution" aria-labelledby="ma-title">
      <div class="ma-head">
        <Scale size={15} class="ma-icon" />
        <h3 id="ma-title" class="ma-title">{$t('pages.modelDetail.attribution.title')}</h3>
        {#if attribution.no_derivatives}
          <span class="ma-badge" title={$t('pages.modelDetail.attribution.noDerivativesHint')}>{$t('pages.modelDetail.attribution.noDerivatives')}</span>
        {/if}
        {#if possiblyModified}
          <span class="ma-badge ma-badge-modified" data-testid="model-attribution-modified" title={$t('pages.modelDetail.attribution.possiblyModifiedHint')}>{$t('pages.modelDetail.attribution.possiblyModified')}</span>
        {/if}
      </div>
      <p class="ma-intro">{$t('pages.modelDetail.attribution.intro')}</p>
      <dl class="ma-list">
        <dt>{$t('pages.modelDetail.attribution.licence')}</dt>
        <dd>
          {#if attribution.licenses.length}
            {#each attribution.licenses as l, i (l.uri)}
              {#if i > 0}<span class="ma-sep">·</span>{/if}
              <a href={l.uri} target="_blank" rel="noopener noreferrer license">{l.name}</a>
            {/each}
          {:else}
            {$t('pages.modelDetail.attribution.noLicence')}
          {/if}
        </dd>
        {#if attribution.copyright.length}
          <dt>{$t('pages.modelDetail.attribution.copyright')}</dt>
          <dd>
            {#each attribution.copyright as c}<div>{c}</div>{/each}
          </dd>
        {/if}
        {#if attribution.notice}
          <dt>{$t('pages.modelDetail.attribution.notice')}</dt>
          <dd class="ma-notice">{attribution.notice}</dd>
        {/if}
        {#if attribution.status}
          <dt>{$t('pages.modelDetail.attribution.status')}</dt>
          <dd>{attribution.status}</dd>
        {/if}
        <dt>{$t('pages.modelDetail.attribution.source')}</dt>
        <dd class="ma-url">
          {#if isWebUrl(attribution.source_url)}
            <a href={attribution.source_url} target="_blank" rel="noopener noreferrer">{attribution.source_url}</a>
          {:else}
            {attribution.source_url}
          {/if}
        </dd>
        {#if isWebUrl(attribution.specification_url)}
          <dt>{$t('pages.modelDetail.attribution.specification')}</dt>
          <dd class="ma-url">
            <a href={attribution.specification_url} target="_blank" rel="noopener noreferrer">{attribution.specification_url}</a>
          </dd>
        {/if}
        {#if attribution.changes}
          <dt>{$t('pages.modelDetail.attribution.changes')}</dt>
          <dd>{attribution.changes}</dd>
        {/if}
        <dt>{$t('pages.modelDetail.attribution.storedCopy')}</dt>
        <dd>{attribution.stored_copy}</dd>
        {#if attribution.remarks}
          <dt>{$t('pages.modelDetail.attribution.remarks')}</dt>
          <dd>{attribution.remarks}</dd>
        {/if}
      </dl>
      {#if attribution.header}
        <details class="ma-header">
          <summary>{$t('pages.modelDetail.attribution.header', { values: { file: attribution.file } })}</summary>
          <pre>{attribution.header}</pre>
        </details>
      {/if}
      <a class="ma-notice-link" href={noticeHref} target="_blank" rel="noopener">{$t('pages.modelDetail.attribution.fullNotice')}</a>
    </section>
  {/if}
{/if}

<style>
  .ma-card { background: var(--bg-soft, #f8fafc); border: 1px solid var(--border, #e2e8f0); border-radius: 0.75rem; padding: 0.875rem 1rem; font-size: 0.8rem; color: var(--ink-700, #334155); }
  .ma-head { display: flex; align-items: center; gap: 0.4rem; margin-bottom: 0.35rem; }
  :global(.ma-icon) { color: var(--brand-500, #6366f1); flex-shrink: 0; }
  .ma-title { margin: 0; font-size: 0.7rem; font-weight: 700; letter-spacing: 0.06em; text-transform: uppercase; color: var(--ink-400, #94a3b8); }
  .ma-intro { margin: 0 0 0.5rem; color: var(--ink-500, #64748b); }
  .ma-list { display: grid; grid-template-columns: max-content 1fr; gap: 0.3rem 0.9rem; margin: 0; }
  .ma-list dt { font-weight: 600; color: var(--ink-500, #64748b); }
  .ma-list dd { margin: 0; min-width: 0; line-height: 1.45; }
  @media (max-width: 600px) { .ma-list { grid-template-columns: 1fr; } .ma-list dd { margin-bottom: 0.3rem; } }
  .ma-url { word-break: break-all; }
  .ma-notice { font-style: italic; }
  .ma-list a, .ma-compact a, .ma-notice-link { color: var(--brand-600, #4f46e5); text-decoration: none; }
  .ma-list a:hover, .ma-compact a:hover, .ma-notice-link:hover { text-decoration: underline; }
  .ma-header { margin-top: 0.6rem; }
  .ma-header summary { cursor: pointer; color: var(--ink-500, #64748b); font-weight: 600; }
  .ma-header pre { margin: 0.4rem 0 0; padding: 0.5rem 0.65rem; white-space: pre-wrap; word-break: break-word; font-size: 0.72rem; background: var(--bg, #fff); border: 1px solid var(--border, #e2e8f0); border-radius: 0.4rem; max-height: 22rem; overflow: auto; }
  .ma-notice-link { display: inline-block; margin-top: 0.55rem; font-weight: 600; }
  .ma-badge { font-size: 0.64rem; font-weight: 700; padding: 1px 7px; border-radius: 999px; background: #fef3c7; color: #92400e; white-space: nowrap; }
  .ma-sep { color: var(--ink-300, #cbd5e1); margin: 0 0.1rem; }
  .ma-compact { display: flex; align-items: center; flex-wrap: wrap; gap: 0.3rem; font-size: 0.72rem; color: var(--ink-500, #64748b); margin-top: 0.25rem; }
  .ma-credit { overflow-wrap: anywhere; }

  .ma-badge-modified { background: #e0e7ff; color: #3730a3; }

  :global(:is([data-theme="dark"], .dark)) .ma-badge { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .ma-badge-modified { background: rgba(99,102,241,0.2); color: #c7d2fe; }
</style>
