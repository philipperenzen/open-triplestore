<script>
  import { onMount } from 'svelte';
  import { t } from 'svelte-i18n';
  import { listDocs, getDoc, saveDoc, deleteDoc } from '../lib/api.js';

  let docs = [];
  let loading = true;
  let error = '';
  let status = '';

  // Editing state
  let slug = '';
  let title = '';
  let category = '';
  let bodyMd = '';
  let adminOnly = false;
  let sortOrder = 100;
  let isNew = false;
  let saving = false;

  async function refresh() {
    loading = true;
    error = '';
    try {
      docs = await listDocs();
    } catch (e) {
      error = e.message || String(e);
    } finally {
      loading = false;
    }
  }

  onMount(refresh);

  async function edit(d) {
    status = '';
    error = '';
    try {
      const full = await getDoc(d.slug);
      slug = full.slug;
      title = full.title;
      category = full.category || '';
      bodyMd = full.body_md || '';
      adminOnly = !!full.admin_only;
      sortOrder = full.sort_order ?? 100;
      isNew = false;
    } catch (e) {
      error = e.message || String(e);
    }
  }

  function startNew() {
    slug = '';
    title = '';
    category = '';
    bodyMd = '';
    adminOnly = false;
    sortOrder = 100;
    isNew = true;
    status = '';
    error = '';
  }

  async function save() {
    if (!slug.trim()) {
      error = $t('pages.docEditor.slugRequired');
      return;
    }
    saving = true;
    error = '';
    status = '';
    try {
      await saveDoc(slug.trim(), {
        title: title.trim() || slug.trim(),
        category: category.trim() || null,
        body_md: bodyMd,
        admin_only: adminOnly,
        sort_order: Number(sortOrder) || 100,
      });
      status = $t('pages.docEditor.savedToast');
      isNew = false;
      await refresh();
    } catch (e) {
      error = e.message || String(e);
    } finally {
      saving = false;
    }
  }

  async function remove() {
    if (!slug || isNew) return;
    if (!confirm($t('pages.docEditor.deleteConfirm', { values: { slug } }))) return;
    error = '';
    try {
      await deleteDoc(slug);
      status = $t('pages.docEditor.deletedToast');
      startNew();
      await refresh();
    } catch (e) {
      error = e.message || String(e);
    }
  }
</script>

<div class="doc-editor">
  <h1>{$t('pages.docEditor.heading')}</h1>
  <p class="hint">
    {$t('pages.docEditor.hintBefore')}<strong>{$t('pages.docEditor.hintAdminOnly')}</strong>{$t('pages.docEditor.hintAfter')}
  </p>

  {#if error}<div class="banner err">{error}</div>{/if}
  {#if status}<div class="banner ok">{status}</div>{/if}

  <div class="layout">
    <aside class="list">
      <button class="new" on:click={startNew}>{$t('pages.docEditor.newDoc')}</button>
      {#if loading}
        <p>{$t('system.loading')}</p>
      {:else}
        <ul>
          {#each docs as d}
            <li>
              <button class:active={d.slug === slug && !isNew} on:click={() => edit(d)}>
                <span class="t">{d.title}</span>
                <span class="meta">
                  <code>{d.slug}</code>
                  {#if d.admin_only}<span class="badge">{$t('pages.docEditor.badgeAdmin')}</span>{/if}
                  {#if d.source === 'builtin'}<span class="badge builtin">{$t('pages.docEditor.badgeBuiltin')}</span>{/if}
                </span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </aside>

    <section class="form">
      <label>
        <span>{$t('pages.docEditor.slugLabel')}</span>
        <input type="text" bind:value={slug} disabled={!isNew} placeholder={$t('pages.docEditor.slugPlaceholder')} />
      </label>
      <label>
        <span>{$t('pages.docEditor.titleLabel')}</span>
        <input type="text" bind:value={title} placeholder={$t('pages.docEditor.titlePlaceholder')} />
      </label>
      <label>
        <span>{$t('pages.docEditor.categoryLabel')}</span>
        <input type="text" bind:value={category} placeholder={$t('pages.docEditor.categoryPlaceholder')} />
      </label>
      <div class="row">
        <label class="check">
          <input type="checkbox" bind:checked={adminOnly} />
          <span>{$t('pages.docEditor.adminOnlyLabel')}</span>
        </label>
        <label class="small">
          <span>{$t('pages.docEditor.sortLabel')}</span>
          <input type="number" bind:value={sortOrder} min="0" />
        </label>
      </div>
      <label>
        <span>{$t('pages.docEditor.bodyLabel')}</span>
        <textarea bind:value={bodyMd} rows="20" placeholder={$t('pages.docEditor.bodyPlaceholder')}></textarea>
      </label>
      <div class="actions">
        <button class="primary" on:click={save} disabled={saving}>{saving ? $t('pages.docEditor.saving') : $t('system.save')}</button>
        {#if !isNew}<button class="danger" on:click={remove}>{$t('system.delete')}</button>{/if}
      </div>
    </section>
  </div>
</div>

<style>
  .doc-editor { padding: 1.5rem; max-width: 1100px; margin: 0 auto; }
  .hint { color: var(--text-muted, #666); font-size: 0.9rem; }
  .banner { padding: 0.5rem 0.75rem; border-radius: 6px; margin: 0.5rem 0; }
  .banner.err { background: #fdecea; color: #b00020; }
  .banner.ok { background: #e7f6ec; color: #1e7e34; }
  .layout { display: grid; grid-template-columns: 280px 1fr; gap: 1.25rem; margin-top: 1rem; }
  .list .new { width: 100%; margin-bottom: 0.5rem; padding: 0.4rem; }
  .list ul { list-style: none; margin: 0; padding: 0; }
  .list li button { width: 100%; text-align: left; padding: 0.45rem 0.5rem; background: none; border: 1px solid transparent; border-radius: 6px; cursor: pointer; }
  .list li button.active { border-color: var(--accent, #4f46e5); background: #f4f4ff; }
  .list .t { display: block; font-weight: 600; font-size: 0.9rem; }
  .list .meta { display: flex; gap: 0.35rem; align-items: center; font-size: 0.72rem; color: #888; }
  .badge { background: #eee; border-radius: 4px; padding: 0 0.3rem; }
  .badge.builtin { background: #eef; }
  .form label { display: block; margin-bottom: 0.6rem; }
  .form label > span { display: block; font-size: 0.8rem; font-weight: 600; margin-bottom: 0.2rem; }
  .form input[type=text], .form input[type=number], .form textarea { width: 100%; padding: 0.4rem; font-family: inherit; box-sizing: border-box; }
  .form textarea { font-family: ui-monospace, monospace; font-size: 0.85rem; }
  .row { display: flex; gap: 1rem; align-items: center; }
  .check { display: flex; align-items: center; gap: 0.4rem; }
  .check input { width: auto; }
  .small { width: 80px; }
  .actions { display: flex; gap: 0.5rem; margin-top: 0.5rem; }
  .actions .primary { background: var(--accent, #4f46e5); color: #fff; border: none; padding: 0.45rem 1rem; border-radius: 6px; cursor: pointer; }
  .actions .danger { background: #fff; color: #b00020; border: 1px solid #b00020; padding: 0.45rem 1rem; border-radius: 6px; cursor: pointer; }
</style>
