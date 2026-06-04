<script>
  // Documentation viewer. Renders the in-app docs served by `/api/docs`
  // (admin-only docs are filtered server-side). The doc list drives a
  // category-grouped index; the selected doc's markdown body is rendered with
  // an "on this page" scroll-spy rail. Deep-linkable at /docs/:slug#heading.
  import { onMount, tick } from 'svelte';
  import { listDocs, getDoc } from '../lib/api.js';
  import { isAdmin } from '../lib/stores.js';
  import { Link, navigate } from '../lib/router/index.js';
  import { renderMarkdown } from '../lib/markdown.js';
  import { BookOpen, Pencil, AlertTriangle, FileQuestion } from 'lucide-svelte';
  import { t } from 'svelte-i18n';

  // Doc slug from the /docs/:slug route. Undefined on the bare /docs index, in
  // which case we fall back to the first doc in the list.
  export let slug = undefined;

  let docs = [];
  let listLoading = true;
  let listError = '';

  let current = null;
  let docLoading = false;
  let docError = '';
  let notFound = false;

  let rendered = { html: '', headings: [] };
  let activeId = '';
  let contentEl;

  // Distance below the viewport top that marks the "active" line for scroll-spy.
  const ACTIVE_OFFSET = 110;

  $: effectiveSlug = slug || docs[0]?.slug || '';
  $: groups = groupByCategory(docs);
  $: tocHeadings = rendered.headings.filter((h) => h.level === 2 || h.level === 3);

  function groupByCategory(list) {
    const map = new Map();
    for (const d of list) {
      const cat = d.category || 'General';
      if (!map.has(cat)) map.set(cat, []);
      map.get(cat).push(d);
    }
    const out = [...map.entries()].map(([category, items]) => ({
      category,
      items: items.slice().sort((a, b) => (a.sort_order - b.sort_order) || a.title.localeCompare(b.title)),
      minSort: Math.min(...items.map((i) => i.sort_order ?? 100)),
    }));
    out.sort((a, b) => a.minSort - b.minSort || a.category.localeCompare(b.category));
    return out;
  }

  async function loadList() {
    listLoading = true;
    listError = '';
    try {
      docs = await listDocs();
    } catch (e) {
      listError = e?.message || String(e);
    } finally {
      listLoading = false;
    }
  }

  // Load the selected doc whenever the effective slug changes.
  let lastLoaded = null;
  $: if (effectiveSlug && effectiveSlug !== lastLoaded) {
    lastLoaded = effectiveSlug;
    loadDoc(effectiveSlug);
  }

  async function loadDoc(s) {
    docLoading = true;
    docError = '';
    notFound = false;
    try {
      const doc = await getDoc(s);
      current = doc;
      rendered = renderMarkdown(doc.body_md || '');
      await tick();
      applyHash();
      updateActive();
    } catch (e) {
      current = null;
      rendered = { html: '', headings: [] };
      const msg = e?.message || String(e);
      if (/\b404\b|not found/i.test(msg)) notFound = true;
      else docError = msg;
    } finally {
      docLoading = false;
    }
  }

  // Honour a #heading deep link once a doc has rendered; otherwise reset scroll.
  function applyHash() {
    const hash = decodeURIComponent((window.location.hash || '').slice(1));
    if (!hash) {
      window.scrollTo({ top: 0 });
      return;
    }
    const el = document.getElementById(hash);
    if (el) {
      activeId = hash;
      tick().then(() => el.scrollIntoView({ block: 'start' }));
    }
  }

  function updateActive() {
    if (!contentEl) return;
    const hs = Array.from(contentEl.querySelectorAll('h2[id], h3[id]'));
    if (!hs.length) {
      activeId = '';
      return;
    }
    // At the bottom of the page the final headings can never reach the offset
    // line — activate the last one so the rail doesn't stall.
    if (window.scrollY + window.innerHeight >= document.documentElement.scrollHeight - 4) {
      activeId = hs[hs.length - 1].id;
      return;
    }
    let active = hs[0].id;
    for (const h of hs) {
      if (h.getBoundingClientRect().top - ACTIVE_OFFSET <= 0) active = h.id;
      else break;
    }
    activeId = active;
  }

  function scrollToId(id) {
    const el = document.getElementById(id);
    if (!el) return;
    activeId = id;
    el.scrollIntoView({ behavior: 'smooth', block: 'start' });
    history.replaceState(null, '', `#${id}`);
  }

  function onTocClick(e, id) {
    e.preventDefault();
    scrollToId(id);
  }

  // Delegate clicks inside rendered markdown: same-page #anchors scroll
  // smoothly, other in-app links route via the SPA, external links keep their
  // default behaviour.
  function onContentClick(e) {
    const a = e.target.closest?.('a[href]');
    if (!a || !contentEl?.contains(a)) return;
    if (a.target === '_blank') return; // let new-tab links open natively
    if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey || e.button !== 0) return;
    const raw = a.getAttribute('href') || '';
    let url;
    try {
      url = new URL(raw, window.location.href);
    } catch {
      return;
    }
    if (url.origin !== window.location.origin) return; // external → default
    e.preventDefault();
    if (url.pathname === window.location.pathname && url.hash) {
      scrollToId(decodeURIComponent(url.hash.slice(1)));
    } else {
      navigate(url.pathname + url.search + url.hash);
    }
  }

  onMount(() => {
    loadList();
    const onScroll = () => updateActive();
    window.addEventListener('scroll', onScroll, { passive: true });
    window.addEventListener('resize', onScroll);
    return () => {
      window.removeEventListener('scroll', onScroll);
      window.removeEventListener('resize', onScroll);
    };
  });
</script>

<div class="docs-layout">
  <!-- Left: doc index, grouped by category -->
  <aside class="docs-side" aria-label={$t('pages.documentation.indexAriaLabel')}>
    <div class="docs-side-inner">
      {#if listLoading}
        <div class="side-skeleton">
          {#each Array(7) as _, i}<div class="sk-row" style={`width:${70 + ((i * 13) % 30)}%`}></div>{/each}
        </div>
      {:else if listError}
        <p class="side-error"><AlertTriangle size={14} /> {listError}</p>
      {:else}
        {#each groups as g}
          <div class="side-group">
            <p class="side-group-h">{g.category}</p>
            {#each g.items as d}
              <Link to={`/docs/${d.slug}`} class={`side-link ${effectiveSlug === d.slug ? 'side-link-active' : ''}`}>
                <span class="side-link-t">{d.title}</span>
                {#if d.admin_only}<span class="side-tag">{$t('pages.documentation.adminTag')}</span>{/if}
              </Link>
            {/each}
          </div>
        {/each}
      {/if}
    </div>
  </aside>

  <!-- Middle: rendered doc -->
  <main class="docs-main">
    {#if current}
      <div class="doc-head">
        {#if current.category}<span class="doc-kicker">{current.category}</span>{/if}
        {#if $isAdmin}
          <Link to="/admin/docs" class="doc-edit"><Pencil size={13} /> {$t('system.edit')}</Link>
        {/if}
      </div>
    {/if}

    {#if docLoading && !current}
      <div class="doc-loading">{$t('system.loading')}</div>
    {:else if notFound}
      <div class="doc-empty">
        <FileQuestion size={30} />
        <h2>{$t('pages.documentation.notFoundTitle')}</h2>
        <p>{$t('pages.documentation.notFoundBefore')}<code>{slug}</code>{$t('pages.documentation.notFoundAfter')}</p>
        <Link to="/docs" class="doc-empty-link">{$t('pages.documentation.backToDocs')}</Link>
      </div>
    {:else if docError}
      <div class="doc-empty">
        <AlertTriangle size={30} />
        <h2>{$t('pages.documentation.loadErrorTitle')}</h2>
        <p>{docError}</p>
      </div>
    {:else if !listLoading && docs.length === 0}
      <div class="doc-empty">
        <BookOpen size={30} />
        <h2>{$t('pages.documentation.emptyTitle')}</h2>
        <p>{$t('pages.documentation.emptyBody')}</p>
        {#if $isAdmin}<Link to="/admin/docs" class="doc-empty-link">{$t('pages.documentation.createFirst')}</Link>{/if}
      </div>
    {/if}

    <!-- Rendered markdown is sanitized in renderMarkdown() (DOMPurify). Click is
         delegated to route in-app links; keyboard users still get native <a>s. -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions a11y_click_events_have_key_events -->
    <article class="docs-article" bind:this={contentEl} on:click={onContentClick}>
      <!-- eslint-disable-next-line svelte/no-at-html-tags -->
      {@html rendered.html}
    </article>
  </main>

  <!-- Right: on this page -->
  {#if tocHeadings.length > 1}
    <nav class="docs-toc" aria-label={$t('pages.documentation.onThisPage')}>
      <p class="toc-h">{$t('pages.documentation.onThisPage')}</p>
      <div class="toc-inner">
        {#each tocHeadings as h}
          <a
            href={`#${h.id}`}
            class={`toc-link toc-l${h.level} ${activeId === h.id ? 'toc-link-active' : ''}`}
            on:click={(e) => onTocClick(e, h.id)}
          >{h.text}</a>
        {/each}
      </div>
    </nav>
  {/if}
</div>

<style>
  /* ── Layout ─────────────────────────────────────────────────────────────── */
  .docs-layout {
    display: flex;
    align-items: flex-start;
    gap: 1.75rem;
    padding-bottom: 3rem;
  }
  .docs-side {
    width: 14rem;
    flex-shrink: 0;
    position: sticky;
    top: 1.25rem;
    max-height: calc(100vh - 2.5rem);
    overflow-y: auto;
  }
  .docs-side-inner {
    display: flex;
    flex-direction: column;
    gap: 1.1rem;
  }
  .docs-main {
    flex: 1;
    min-width: 0;
  }
  .docs-toc {
    width: 13rem;
    flex-shrink: 0;
    position: sticky;
    top: 1.25rem;
    max-height: calc(100vh - 2.5rem);
    overflow-y: auto;
  }

  /* ── Side index ─────────────────────────────────────────────────────────── */
  .side-group {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }
  .side-group-h {
    font-size: 0.7rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ink-400);
    margin: 0 0 0.3rem;
    padding: 0 0.65rem;
  }
  :global(.side-link) {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.32rem 0.65rem;
    border-radius: 0.45rem;
    font-size: 0.83rem;
    font-weight: 500;
    color: var(--ink-600);
    text-decoration: none;
    border-left: 2px solid transparent;
    line-height: 1.35;
  }
  .side-link-t {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  :global(.side-link:hover) {
    background: var(--bg-soft);
    color: var(--brand-600);
  }
  :global(.side-link-active) {
    background: var(--brand-50, #eef2ff);
    color: var(--brand-600);
    border-left-color: var(--brand-500);
    font-weight: 600;
  }
  .side-tag {
    margin-left: auto;
    font-size: 0.62rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    background: var(--bg-soft);
    color: var(--ink-400);
    padding: 0.05rem 0.3rem;
    border-radius: 0.3rem;
  }
  .side-skeleton {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 0.5rem;
  }
  .sk-row {
    height: 0.85rem;
    border-radius: 0.3rem;
    background: linear-gradient(90deg, var(--bg-soft) 0%, var(--line-soft) 50%, var(--bg-soft) 100%);
    background-size: 200% 100%;
    animation: sk 1.2s ease-in-out infinite;
  }
  @keyframes sk {
    0% { background-position: 200% 0; }
    100% { background-position: -200% 0; }
  }
  .side-error {
    color: #b00020;
    font-size: 0.8rem;
    display: flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.5rem;
  }

  /* ── Doc header ─────────────────────────────────────────────────────────── */
  .doc-head {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 1.1rem;
  }
  .doc-kicker {
    font-size: 0.72rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--brand-600);
  }
  :global(.doc-edit) {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    font-size: 0.78rem;
    color: var(--ink-500);
    text-decoration: none;
    border: 1px solid var(--line-soft);
    padding: 0.25rem 0.6rem;
    border-radius: 0.45rem;
  }
  :global(.doc-edit:hover) {
    color: var(--brand-600);
    border-color: var(--brand-500);
  }

  /* ── Empty / loading states ─────────────────────────────────────────────── */
  .doc-loading {
    padding: 3rem 0;
    color: var(--ink-400);
    font-size: 0.9rem;
  }
  .doc-empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    gap: 0.4rem;
    padding: 4rem 1rem;
    color: var(--ink-500);
  }
  .doc-empty h2 {
    font-size: 1.1rem;
    font-weight: 700;
    color: var(--ink-800);
    margin: 0.5rem 0 0;
  }
  .doc-empty p {
    font-size: 0.88rem;
    margin: 0;
  }
  :global(.doc-empty-link) {
    margin-top: 0.75rem;
    color: var(--brand-600);
    text-decoration: underline;
  }

  /* ── On this page ───────────────────────────────────────────────────────── */
  .toc-h {
    font-size: 0.7rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--ink-400);
    margin: 0 0 0.5rem;
  }
  .toc-inner {
    display: flex;
    flex-direction: column;
    gap: 0.05rem;
    border-left: 1px solid var(--line-soft);
  }
  .toc-link {
    display: block;
    padding: 0.25rem 0.75rem;
    font-size: 0.8rem;
    color: var(--ink-500);
    text-decoration: none;
    border-left: 2px solid transparent;
    margin-left: -1px;
    line-height: 1.4;
  }
  .toc-link:hover {
    color: var(--brand-600);
  }
  .toc-link-active {
    color: var(--brand-600);
    border-left-color: var(--brand-500);
    font-weight: 600;
  }
  .toc-l3 {
    padding-left: 1.5rem;
    font-size: 0.76rem;
  }

  /* ── Rendered markdown content ──────────────────────────────────────────── */
  .docs-article {
    max-width: 48rem;
  }
  .docs-article :global(h1) {
    font-size: 1.6rem;
    font-weight: 800;
    line-height: 1.25;
    margin: 0 0 1rem;
    color: var(--ink-900);
  }
  .docs-article :global(h2) {
    font-size: 1.2rem;
    font-weight: 700;
    margin: 2rem 0 0.75rem;
    padding-bottom: 0.35rem;
    border-bottom: 1px solid var(--line-soft);
    color: var(--ink-900);
    scroll-margin-top: 5rem;
  }
  .docs-article :global(h3) {
    font-size: 1rem;
    font-weight: 700;
    margin: 1.5rem 0 0.5rem;
    color: var(--ink-800);
    scroll-margin-top: 5rem;
  }
  .docs-article :global(h4) {
    font-size: 0.9rem;
    font-weight: 600;
    margin: 1.25rem 0 0.4rem;
    color: var(--ink-800);
    scroll-margin-top: 5rem;
  }
  .docs-article :global(h1:first-child),
  .docs-article :global(h2:first-child) {
    margin-top: 0;
  }
  .docs-article :global(p) {
    font-size: 0.9rem;
    line-height: 1.7;
    color: var(--ink-700);
    margin: 0 0 1rem;
  }
  .docs-article :global(ul),
  .docs-article :global(ol) {
    margin: 0 0 1rem;
    padding-left: 1.4rem;
    font-size: 0.9rem;
    color: var(--ink-700);
    line-height: 1.7;
  }
  .docs-article :global(li) {
    margin: 0.25rem 0;
  }
  .docs-article :global(li::marker) {
    color: var(--ink-400);
  }
  .docs-article :global(a) {
    color: var(--brand-600);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .docs-article :global(a:hover) {
    opacity: 0.85;
  }
  .docs-article :global(strong) {
    color: var(--ink-900);
    font-weight: 600;
  }
  .docs-article :global(code) {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.82em;
    background: var(--bg-soft, #f3f4f6);
    padding: 0.12em 0.36em;
    border-radius: 0.3rem;
    color: var(--ink-800);
  }
  .docs-article :global(pre) {
    background: #1e1e2e;
    color: #cdd6f4;
    padding: 1rem 1.15rem;
    border-radius: 0.7rem;
    overflow-x: auto;
    margin: 0 0 1.15rem;
    line-height: 1.55;
  }
  .docs-article :global(pre code) {
    background: none;
    padding: 0;
    color: inherit;
    font-size: 0.82rem;
  }
  /* Syntax-highlight tokens (resultHighlight.js) on the dark code blocks. */
  .docs-article :global(pre .tok-comment) { color: #7f849c; font-style: italic; }
  .docs-article :global(pre .tok-iri)     { color: #89b4fa; }
  .docs-article :global(pre .tok-pname)   { color: #f5c2e7; }
  .docs-article :global(pre .tok-kw)      { color: #cba6f7; font-weight: 600; }
  .docs-article :global(pre .tok-str)     { color: #a6e3a1; }
  .docs-article :global(pre .tok-num)     { color: #fab387; }
  .docs-article :global(pre .tok-punct)   { color: #9399b2; }
  .docs-article :global(pre .tok-key)     { color: #89dceb; }
  .docs-article :global(pre .tok-tag)     { color: #89b4fa; }
  .docs-article :global(pre .tok-attr)    { color: #f9e2af; }
  .docs-article :global(pre .tok-meta)    { color: #7f849c; }
  .docs-article :global(blockquote) {
    margin: 0 0 1.15rem;
    padding: 0.6rem 0.9rem;
    border-left: 3px solid var(--brand-500);
    background: var(--bg-soft);
    border-radius: 0 0.5rem 0.5rem 0;
    color: var(--ink-700);
    font-size: 0.88rem;
  }
  .docs-article :global(blockquote p) {
    margin: 0;
  }
  .docs-article :global(table) {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.84rem;
    margin: 0 0 1.2rem;
    display: block;
    overflow-x: auto;
  }
  .docs-article :global(th) {
    text-align: left;
    padding: 0.5rem 0.7rem;
    font-size: 0.72rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--ink-400);
    border-bottom: 1px solid var(--line-soft);
    white-space: nowrap;
  }
  .docs-article :global(td) {
    padding: 0.5rem 0.7rem;
    border-bottom: 1px solid var(--line-soft);
    color: var(--ink-700);
    vertical-align: top;
  }
  .docs-article :global(tr:hover td) {
    background: var(--bg-soft);
  }
  .docs-article :global(hr) {
    border: none;
    border-top: 1px solid var(--line-soft);
    margin: 1.75rem 0;
  }
  .docs-article :global(img) {
    max-width: 100%;
    border-radius: 0.5rem;
  }

  /* ── Responsive ─────────────────────────────────────────────────────────── */
  @media (max-width: 1280px) {
    .docs-toc {
      display: none;
    }
  }
  @media (max-width: 1024px) {
    .docs-layout {
      flex-direction: column;
      gap: 1rem;
    }
    .docs-side {
      position: static;
      width: 100%;
      max-height: none;
    }
    .docs-side-inner {
      flex-flow: row wrap;
      gap: 0.75rem 1.25rem;
    }
    .docs-article {
      max-width: none;
    }
  }
</style>
