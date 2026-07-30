<script>
  // The file manager: folder tree + breadcrumbs, grid/list views, multi-select,
  // drag-drop uploads (including dropped directories), move/rename/delete for
  // files and folders, visibility toggles, previews and linked-data copy
  // actions. Reused by the dataset page (compact), the /files page (full) and
  // FileBrowserModal (manage or pick mode).
  //
  // Props:
  //   datasetId          – dataset whose file manager to show (required)
  //   canWrite           – enables every mutating affordance
  //   mode               – 'manage' | 'pick' (pick: selection + Choose emits 'select')
  //   compact            – embedded variant: no tree sidebar, tighter chrome
  //   initialPath        – folder to open first
  //   datasetVisibility  – 'public' | 'members' | 'private' (pre-flight for asset visibility)
  // Events: pathchange(path), select(asset) [pick mode]
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import {
    listAssets, listAssetFolders, uploadAsset, deleteAsset, moveAsset,
    createAssetFolder, renameAssetFolder, deleteAssetFolder,
    updateAssetVisibility, updateAssetMetadata, fetchAssetContent,
  } from '../../lib/api.js';
  import {
    fileKind, KIND_GROUPS, ALL_GROUPS, normalizePath, parentOf, nameOf, joinPath,
    crumbsOf, isWithin, thumbnailIndex, visibleAssets, listChildren, sortFiles,
    matchesQuery, formatBytes, collectDroppedFiles,
  } from '../../lib/files';
  import { copyToClipboard } from '../../lib/clipboard';
  import AssetPreview from '../AssetPreview.svelte';
  import ContextMenu from '../ContextMenu.svelte';
  import ConfirmModal from '../ConfirmModal.svelte';
  import FileThumb from './FileThumb.svelte';
  import {
    Folder, FolderOpen, FolderPlus, FolderInput, HardDrive, Upload, Download,
    Trash2, Pencil, Eye, Link as LinkIcon, Clipboard, Lock, Globe, LayoutGrid,
    List as ListIcon, Search, ChevronRight, ChevronDown, X as XIcon, Check,
    CheckCheck, Loader2, MoreHorizontal, ArrowUpDown, RefreshCw, FileText,
  } from 'lucide-svelte';

  export let datasetId;
  export let canWrite = false;
  export let mode = 'manage';
  export let compact = false;
  export let initialPath = '';
  export let datasetVisibility = null;

  const dispatch = createEventDispatcher();

  // ── Data ──────────────────────────────────────────────────────────────────
  let assets = [];
  let folderPaths = [];
  let loading = true;
  let error = '';
  let loadedFor = null;

  // ── View state ────────────────────────────────────────────────────────────
  let path = normalizePath(initialPath);
  let view = compact ? 'list' : 'grid';
  let sortKey = 'name';
  let sortDir = 1;
  let query = '';
  let activeGroups = new Set();
  let selected = new Set();
  let anchorId = null;
  let treeExpanded = new Set();
  let sortMenuOpen = false;
  let copiedId = null;
  let rootEl;

  // ── Transients ────────────────────────────────────────────────────────────
  let uploads = [];
  let dropActive = false;
  let dragCount = 0;
  let dropFolderTarget = null;
  let ctx = { visible: false, x: 0, y: 0 };
  let ctxTarget = null; // { type: 'file', asset } | { type: 'folder', row }

  // ── Modals ────────────────────────────────────────────────────────────────
  let newFolderOpen = false;
  let newFolderName = '';
  let newFolderBusy = false;
  let renameTarget = null; // { type, asset?|folderPath?, value }
  let renameBusy = false;
  let moveTarget = null; // { assetIds?: string[], folderPath?: string }
  let movePick = '';
  let moveBusy = false;
  let deleteTarget = null; // { assets?: AssetEntry[], folder?: SubfolderRow }
  let deleteBusy = false;
  let editingAsset = null;
  let editTitle = '';
  let editDesc = '';
  let editBusy = false;
  let previewAsset = null;

  $: if (datasetId && datasetId !== loadedFor) load();

  async function load() {
    loadedFor = datasetId;
    loading = true;
    error = '';
    selected = new Set();
    try {
      const [assetList, folderRes] = await Promise.all([
        listAssets(datasetId),
        listAssetFolders(datasetId).catch(() => ({ folders: [] })),
      ]);
      assets = assetList;
      folderPaths = (folderRes.folders || []).map((f) => f.path);
      // Recompute existence locally — the derived `folderSet` refreshes only on
      // the next flush (Svelte tracks statically visible assignments only).
      const known = new Set(folderPaths);
      for (const a of assetList) {
        let prefix = '';
        for (const seg of (a.folder || '').split('/').filter(Boolean)) {
          prefix = prefix ? `${prefix}/${seg}` : seg;
          known.add(prefix);
        }
      }
      if (path && !known.has(path)) path = '';
    } catch (e) {
      error = e?.message || $t('components.fileBrowser.loadFailed');
      assets = [];
      folderPaths = [];
    } finally {
      loading = false;
    }
  }

  // ── Derivations ───────────────────────────────────────────────────────────
  $: thumbIndex = thumbnailIndex(assets);
  $: folderSet = (() => {
    const set = new Set(folderPaths.filter(Boolean));
    for (const a of visibleAssets(assets)) {
      let prefix = '';
      for (const seg of (a.folder || '').split('/').filter(Boolean)) {
        prefix = prefix ? `${prefix}/${seg}` : seg;
        set.add(prefix);
      }
    }
    return set;
  })();
  $: searching = query.trim().length > 0;
  $: listing = listChildren(assets, folderPaths, path);
  $: groupOf = (a) => KIND_GROUPS[fileKind(a.content_type, a.filename)];
  $: baseFiles = searching ? visibleAssets(assets).filter((a) => matchesQuery(a, query)) : listing.files;
  $: fileRows = sortFiles(
    activeGroups.size ? baseFiles.filter((a) => activeGroups.has(groupOf(a))) : baseFiles,
    sortKey,
    sortDir
  );
  $: folderRows = searching ? [] : listing.subfolders;
  $: crumbs = crumbsOf(path);
  $: totalShownBytes = fileRows.reduce((s, a) => s + (a.size_bytes || 0), 0);
  $: presentGroups = ALL_GROUPS.filter((g) => visibleAssets(assets).some((a) => groupOf(a) === g));
  $: selectedAssets = fileRows.filter((a) => selected.has(a.id));
  $: isEmptyDataset = !loading && visibleAssets(assets).length === 0 && folderSet.size === 0;

  // Flattened sidebar tree (root-level folders + expanded descendants).
  $: treeRows = (() => {
    const all = [...folderSet].sort((a, b) => a.localeCompare(b, undefined, { sensitivity: 'base' }));
    const childrenOf = (p) => all.filter((f) => parentOf(f) === p);
    const out = [];
    const walk = (p, depth) => {
      for (const f of childrenOf(p)) {
        const kids = childrenOf(f);
        out.push({ path: f, name: nameOf(f), depth, hasChildren: kids.length > 0, expanded: treeExpanded.has(f) });
        if (treeExpanded.has(f)) walk(f, depth + 1);
      }
    };
    walk('', 0);
    return out;
  })();

  function setPath(p) {
    if (p === path) return;
    path = p;
    selected = new Set();
    query = '';
    // Reveal the target in the sidebar tree.
    let prefix = '';
    for (const seg of p.split('/').filter(Boolean)) {
      prefix = prefix ? `${prefix}/${seg}` : seg;
      treeExpanded.add(prefix);
    }
    treeExpanded = treeExpanded;
    dispatch('pathchange', p);
  }

  function toggleTree(p) {
    if (treeExpanded.has(p)) treeExpanded.delete(p); else treeExpanded.add(p);
    treeExpanded = treeExpanded;
  }

  function toggleGroup(g) {
    if (activeGroups.has(g)) activeGroups.delete(g); else activeGroups.add(g);
    activeGroups = activeGroups;
  }

  // ── Selection ─────────────────────────────────────────────────────────────
  function clickFile(e, asset) {
    if (mode === 'pick') {
      selected = new Set([asset.id]);
      return;
    }
    if (e.shiftKey && anchorId) {
      const ids = fileRows.map((a) => a.id);
      const a = ids.indexOf(anchorId);
      const b = ids.indexOf(asset.id);
      if (a !== -1 && b !== -1) {
        selected = new Set(ids.slice(Math.min(a, b), Math.max(a, b) + 1));
        return;
      }
    }
    if (e.ctrlKey || e.metaKey) {
      if (selected.has(asset.id)) selected.delete(asset.id); else selected.add(asset.id);
      selected = new Set(selected);
    } else {
      selected = new Set([asset.id]);
    }
    anchorId = asset.id;
  }

  function dblClickFile(asset) {
    if (mode === 'pick') { dispatch('select', asset); return; }
    previewAsset = asset;
  }

  function toggleSelectAll() {
    selected = selected.size === fileRows.length
      ? new Set()
      : new Set(fileRows.map((a) => a.id));
  }

  function onKeydown(e) {
    if (e.key === 'Escape') {
      if (ctx.visible) { ctx = { ...ctx, visible: false }; return; }
      selected = new Set();
      sortMenuOpen = false;
    }
    if (e.key === 'Delete' && canWrite && selected.size && !deleteTarget && mode === 'manage') {
      e.preventDefault();
      deleteTarget = { assets: selectedAssets };
    }
  }

  // ── Context menu ──────────────────────────────────────────────────────────
  function openCtx(e, target) {
    e.preventDefault();
    e.stopPropagation();
    ctxTarget = target;
    if (target.type === 'file' && !selected.has(target.asset.id)) {
      selected = new Set([target.asset.id]);
      anchorId = target.asset.id;
    }
    ctx = { visible: true, x: e.clientX, y: e.clientY };
  }

  $: ctxItems = (() => {
    if (!ctxTarget) return [];
    if (ctxTarget.type === 'folder') {
      const items = [{ label: $t('components.fileBrowser.open'), icon: FolderOpen, action: 'open-folder' }];
      if (canWrite && mode === 'manage') {
        items.push(
          { label: $t('components.fileBrowser.rename'), icon: Pencil, action: 'rename-folder' },
          { label: $t('components.fileBrowser.moveTo'), icon: FolderInput, action: 'move-folder' },
          { divider: true },
          { label: $t('components.fileBrowser.delete'), icon: Trash2, action: 'delete-folder', danger: true },
        );
      }
      return items;
    }
    const many = selected.size > 1;
    const items = [];
    if (!many) {
      items.push(
        { label: $t('components.fileBrowser.preview'), icon: Eye, action: 'preview' },
        { label: $t('components.fileBrowser.download'), icon: Download, action: 'download' },
        { label: $t('components.fileBrowser.copyIri'), icon: LinkIcon, action: 'copy-iri' },
        { label: $t('components.fileBrowser.copyTurtle'), icon: Clipboard, action: 'copy-turtle' },
      );
    }
    if (canWrite && mode === 'manage') {
      if (items.length) items.push({ divider: true });
      if (!many) {
        items.push(
          { label: $t('components.fileBrowser.rename'), icon: Pencil, action: 'rename-file' },
          { label: $t('components.fileBrowser.editMetadata'), icon: FileText, action: 'edit-meta' },
        );
      }
      items.push(
        { label: $t('components.fileBrowser.moveTo'), icon: FolderInput, action: 'move-files' },
        { divider: true },
        {
          label: many
            ? $t('components.fileBrowser.deleteCount', { values: { count: selected.size } })
            : $t('components.fileBrowser.delete'),
          icon: Trash2, action: 'delete-files', danger: true,
        },
      );
    }
    return items;
  })();

  function onCtxAction(e) {
    const action = e.detail;
    const target = ctxTarget;
    if (!target) return;
    if (target.type === 'folder') {
      const row = target.row;
      if (action === 'open-folder') setPath(row.path);
      else if (action === 'rename-folder') renameTarget = { type: 'folder', folderPath: row.path, value: row.name };
      else if (action === 'move-folder') { moveTarget = { folderPath: row.path }; movePick = ''; }
      else if (action === 'delete-folder') deleteTarget = { folder: row };
      return;
    }
    const asset = target.asset;
    if (action === 'preview') previewAsset = asset;
    else if (action === 'download') downloadFile(asset);
    else if (action === 'copy-iri') copyIri(asset);
    else if (action === 'copy-turtle') copyTurtle(asset);
    else if (action === 'rename-file') renameTarget = { type: 'file', asset, value: asset.filename };
    else if (action === 'edit-meta') openEdit(asset);
    else if (action === 'move-files') { moveTarget = { assetIds: [...selected] }; movePick = ''; }
    else if (action === 'delete-files') deleteTarget = { assets: selectedAssets.length ? selectedAssets : [asset] };
  }

  // ── File actions ──────────────────────────────────────────────────────────
  function assetIri(asset) {
    return asset.iri || `${window.location.origin}/datasets/${datasetId}/assets/${asset.id}`;
  }

  async function downloadFile(asset) {
    try {
      const res = await fetchAssetContent(datasetId, asset.id);
      if (!res.ok) { error = $t('components.fileBrowser.downloadFailed', { values: { status: res.status } }); return; }
      const blob = await res.blob();
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = asset.filename;
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 5000);
    } catch (e) {
      error = e?.message || $t('components.fileBrowser.downloadFailedPlain');
    }
  }

  async function copyIri(asset) {
    if (await copyToClipboard(assetIri(asset))) {
      copiedId = asset.id;
      setTimeout(() => { copiedId = null; }, 2000);
    }
  }

  function copyTurtle(asset) {
    const iri = assetIri(asset);
    const title = asset.filename.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
    const turtle = `<${iri}> a <http://www.w3.org/ns/dcat#Distribution> ;\n    <http://purl.org/dc/terms/title> "${title}" ;\n    <http://www.w3.org/ns/dcat#mediaType> "${asset.content_type}" ;\n    <http://www.w3.org/ns/dcat#downloadURL> <${iri}> .`;
    copyToClipboard(turtle);
  }

  async function toggleVisibility(asset) {
    const makingPublic = !asset.public;
    if (makingPublic && datasetVisibility && datasetVisibility !== 'public') {
      error = $t('components.fileBrowser.cannotMakePublic');
      return;
    }
    try {
      await updateAssetVisibility(datasetId, asset.id, makingPublic);
      assets = assets.map((a) => a.id === asset.id ? { ...a, public: makingPublic } : a);
      error = '';
    } catch (e) {
      error = e?.message || $t('components.fileBrowser.actionFailed');
    }
  }

  function openEdit(asset) {
    editingAsset = asset;
    editTitle = asset.title || '';
    editDesc = asset.description || '';
  }

  async function saveEdit() {
    if (!editingAsset) return;
    editBusy = true;
    try {
      const updated = await updateAssetMetadata(datasetId, editingAsset.id, {
        title: editTitle || null,
        description: editDesc || null,
      });
      assets = assets.map((a) => a.id === updated.id ? { ...updated } : a);
      editingAsset = null;
      error = '';
    } catch (e) {
      error = e?.message || $t('components.fileBrowser.actionFailed');
    } finally {
      editBusy = false;
    }
  }

  async function confirmRename() {
    if (!renameTarget || !renameTarget.value.trim()) return;
    renameBusy = true;
    const value = renameTarget.value.trim();
    try {
      if (renameTarget.type === 'file') {
        const updated = await moveAsset(datasetId, renameTarget.asset.id, { filename: value });
        assets = assets.map((a) => a.id === updated.id ? { ...updated } : a);
      } else {
        const from = renameTarget.folderPath;
        const to = joinPath(parentOf(from), normalizePath(value) || value);
        await renameAssetFolder(datasetId, from, to);
        if (path === from || isWithin(path, from)) path = to + path.slice(from.length);
        await load();
      }
      renameTarget = null;
      error = '';
    } catch (e) {
      error = e?.message || $t('components.fileBrowser.actionFailed');
    } finally {
      renameBusy = false;
    }
  }

  async function confirmMove() {
    if (!moveTarget) return;
    moveBusy = true;
    const dest = movePick;
    try {
      if (moveTarget.assetIds) {
        for (const id of moveTarget.assetIds) {
          const updated = await moveAsset(datasetId, id, { folder: dest });
          assets = assets.map((a) => a.id === updated.id ? { ...updated } : a);
        }
        if (dest) folderPaths = [...new Set([...folderPaths, dest])];
        selected = new Set();
      } else if (moveTarget.folderPath) {
        const from = moveTarget.folderPath;
        const to = joinPath(dest, nameOf(from));
        await renameAssetFolder(datasetId, from, to);
        await load();
      }
      moveTarget = null;
      error = '';
    } catch (e) {
      error = e?.message || $t('components.fileBrowser.actionFailed');
    } finally {
      moveBusy = false;
    }
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    deleteBusy = true;
    try {
      if (deleteTarget.assets) {
        for (const asset of deleteTarget.assets) {
          await deleteAsset(datasetId, asset.id);
          const thumb = thumbIndex.get(asset.id);
          assets = assets.filter((a) => a.id !== asset.id && (!thumb || a.id !== thumb.id));
        }
        selected = new Set();
      } else if (deleteTarget.folder) {
        await deleteAssetFolder(datasetId, deleteTarget.folder.path, deleteTarget.folder.fileCount > 0);
        if (isWithin(path, deleteTarget.folder.path)) path = parentOf(deleteTarget.folder.path);
        await load();
      }
      deleteTarget = null;
      error = '';
    } catch (e) {
      error = e?.message || $t('components.fileBrowser.actionFailed');
    } finally {
      deleteBusy = false;
    }
  }

  async function confirmNewFolder() {
    const name = normalizePath(newFolderName);
    if (!name) { newFolderOpen = false; return; }
    newFolderBusy = true;
    try {
      const full = joinPath(path, name);
      await createAssetFolder(datasetId, full);
      folderPaths = [...new Set([...folderPaths, full])];
      newFolderOpen = false;
      newFolderName = '';
      error = '';
    } catch (e) {
      error = e?.message || $t('components.fileBrowser.actionFailed');
    } finally {
      newFolderBusy = false;
    }
  }

  // ── Uploads ───────────────────────────────────────────────────────────────
  function friendlyUploadError(e) {
    if (e?.status === 503) return $t('components.fileBrowser.storageNotConfigured');
    if (e?.status === 403) return $t('components.fileBrowser.noWriteAccess');
    if (e?.status === 413) return $t('components.fileBrowser.fileTooLarge');
    return e?.message || $t('components.fileBrowser.uploadFailed');
  }

  async function enqueueUploads(items) {
    if (!items.length) return;
    const entries = items.map((it) => ({
      id: `${Date.now()}-${Math.random().toString(36).slice(2)}`,
      name: it.file.name,
      folder: it.folder,
      size: it.file.size,
      file: it.file,
      progress: 0,
      status: 'uploading',
      error: '',
    }));
    uploads = [...uploads, ...entries];
    let next = 0;
    const worker = async () => {
      for (;;) {
        const idx = next++;
        if (idx >= entries.length) return;
        const entry = entries[idx];
        try {
          const created = await uploadAsset(
            datasetId,
            entry.file,
            (p) => { entry.progress = p; uploads = uploads; },
            entry.folder
          );
          assets = [...assets, created];
          if (entry.folder) {
            const acc = [];
            let prefix = '';
            for (const seg of entry.folder.split('/')) {
              prefix = prefix ? `${prefix}/${seg}` : seg;
              acc.push(prefix);
            }
            folderPaths = [...new Set([...folderPaths, ...acc])];
          }
          entry.status = 'done';
        } catch (e) {
          entry.status = 'error';
          entry.error = friendlyUploadError(e);
        }
        uploads = uploads;
      }
    };
    await Promise.all(Array.from({ length: Math.min(3, entries.length) }, worker));
    setTimeout(() => { uploads = uploads.filter((u) => u.status !== 'done'); }, 4000);
  }

  function onPickFiles(e) {
    const files = [...(e.target.files || [])].map((file) => ({ file, folder: path }));
    e.target.value = '';
    enqueueUploads(files);
  }

  // ── Drag & drop ───────────────────────────────────────────────────────────
  const ASSET_DND = 'application/x-ots-asset-ids';

  function hasExternalFiles(e) {
    return [...(e.dataTransfer?.types || [])].includes('Files');
  }

  function onDragEnter(e) {
    if (!canWrite || !hasExternalFiles(e)) return;
    e.preventDefault();
    dragCount += 1;
    dropActive = true;
  }
  function onDragOver(e) {
    if (!canWrite) return;
    if (hasExternalFiles(e) || [...(e.dataTransfer?.types || [])].includes(ASSET_DND)) {
      e.preventDefault();
    }
  }
  function onDragLeave(_e) {
    if (!dropActive) return;
    dragCount = Math.max(0, dragCount - 1);
    if (dragCount === 0) dropActive = false;
  }
  async function onDrop(e) {
    if (!canWrite) return;
    e.preventDefault();
    dragCount = 0;
    dropActive = false;
    dropFolderTarget = null;
    if (hasExternalFiles(e)) {
      const items = await collectDroppedFiles(e.dataTransfer, path);
      enqueueUploads(items);
    }
  }

  function dragStartFile(e, asset) {
    if (!canWrite || mode !== 'manage') { e.preventDefault(); return; }
    if (!selected.has(asset.id)) { selected = new Set([asset.id]); anchorId = asset.id; }
    e.dataTransfer.setData(ASSET_DND, JSON.stringify([...selected]));
    e.dataTransfer.effectAllowed = 'move';
  }

  function folderDragOver(e, folderPath) {
    if (![...(e.dataTransfer?.types || [])].includes(ASSET_DND)) return;
    e.preventDefault();
    e.stopPropagation();
    dropFolderTarget = folderPath;
    e.dataTransfer.dropEffect = 'move';
  }

  async function folderDrop(e, folderPath) {
    if (![...(e.dataTransfer?.types || [])].includes(ASSET_DND)) return;
    e.preventDefault();
    e.stopPropagation();
    dropFolderTarget = null;
    let ids = [];
    try { ids = JSON.parse(e.dataTransfer.getData(ASSET_DND)) || []; } catch { /* not ours */ }
    for (const id of ids) {
      const asset = assets.find((a) => a.id === id);
      if (!asset || (asset.folder || '') === folderPath) continue;
      try {
        const updated = await moveAsset(datasetId, id, { folder: folderPath });
        assets = assets.map((a) => a.id === updated.id ? { ...updated } : a);
      } catch (err) {
        error = err?.message || $t('components.fileBrowser.actionFailed');
      }
    }
    if (folderPath) folderPaths = [...new Set([...folderPaths, folderPath])];
    selected = new Set();
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="fb"
  class:fb-compact={compact}
  bind:this={rootEl}
  tabindex="-1"
  on:keydown={onKeydown}
  on:dragenter={onDragEnter}
  on:dragover={onDragOver}
  on:dragleave={onDragLeave}
  on:drop={onDrop}
>
  <!-- ── Toolbar ─────────────────────────────────────────────────────────── -->
  <div class="fb-toolbar">
    <nav class="fb-crumbs" aria-label={$t('components.fileBrowser.breadcrumbs')}>
      <button
        class="fb-crumb"
        class:fb-crumb-current={!path && !searching}
        class:fb-drop-hint={dropFolderTarget === ''}
        on:click={() => setPath('')}
        on:dragover={(e) => folderDragOver(e, '')}
        on:drop={(e) => folderDrop(e, '')}
        on:dragleave={() => { if (dropFolderTarget === '') dropFolderTarget = null; }}
      >
        <HardDrive size={13} /> {$t('components.fileBrowser.allFiles')}
      </button>
      {#each crumbs as crumb, i}
        <ChevronRight size={12} class="fb-crumb-sep" />
        <button
          class="fb-crumb"
          class:fb-crumb-current={i === crumbs.length - 1 && !searching}
          class:fb-drop-hint={dropFolderTarget === crumb.path}
          on:click={() => setPath(crumb.path)}
          on:dragover={(e) => folderDragOver(e, crumb.path)}
          on:drop={(e) => folderDrop(e, crumb.path)}
          on:dragleave={() => { if (dropFolderTarget === crumb.path) dropFolderTarget = null; }}
        >{crumb.name}</button>
      {/each}
      {#if searching}
        <ChevronRight size={12} class="fb-crumb-sep" />
        <span class="fb-crumb fb-crumb-current">{$t('components.fileBrowser.searchResults')}</span>
      {/if}
    </nav>

    <div class="fb-tools">
      <div class="fb-search">
        <Search size={13} />
        <input
          type="search"
          placeholder={$t('components.fileBrowser.searchPlaceholder')}
          bind:value={query}
          aria-label={$t('components.fileBrowser.searchPlaceholder')}
        />
        {#if query}
          <button class="fb-icon-btn" on:click={() => query = ''} title={$t('system.clear')}><XIcon size={12} /></button>
        {/if}
      </div>

      <div class="fb-sort">
        <button class="fb-tool-btn" on:click={() => sortMenuOpen = !sortMenuOpen} title={$t('components.fileBrowser.sortBy')}>
          <ArrowUpDown size={13} />
          <span class="fb-tool-label">{$t(`components.fileBrowser.sort_${sortKey}`)}</span>
        </button>
        {#if sortMenuOpen}
          <div class="fb-sort-menu" role="menu">
            {#each ['name', 'kind', 'size', 'modified'] as key}
              <button
                role="menuitem"
                class:active={sortKey === key}
                on:click={() => {
                  if (sortKey === key) sortDir = sortDir === 1 ? -1 : 1; else { sortKey = key; sortDir = 1; }
                  sortMenuOpen = false;
                }}
              >
                {$t(`components.fileBrowser.sort_${key}`)}
                {#if sortKey === key}<span class="fb-sort-dir">{sortDir === 1 ? '↑' : '↓'}</span>{/if}
              </button>
            {/each}
          </div>
        {/if}
      </div>

      <div class="fb-view-toggle" role="group" aria-label={$t('components.fileBrowser.viewMode')}>
        <button class:active={view === 'grid'} on:click={() => view = 'grid'} title={$t('components.fileBrowser.gridView')}><LayoutGrid size={14} /></button>
        <button class:active={view === 'list'} on:click={() => view = 'list'} title={$t('components.fileBrowser.listView')}><ListIcon size={14} /></button>
      </div>

      <button class="fb-tool-btn" on:click={load} title={$t('system.refresh')} disabled={loading}>
        <RefreshCw size={13} />
      </button>

      {#if canWrite && mode === 'manage'}
        <button class="btn btn-sm btn-ghost" on:click={() => { newFolderOpen = true; newFolderName = ''; }}>
          <FolderPlus size={13} /> {$t('components.fileBrowser.newFolder')}
        </button>
        <label class="btn btn-sm">
          <Upload size={13} /> {$t('components.fileBrowser.upload')}
          <input type="file" multiple style="display:none" on:change={onPickFiles} />
        </label>
      {/if}
    </div>
  </div>

  {#if presentGroups.length > 1}
    <div class="fb-chips" role="group" aria-label={$t('components.fileBrowser.filterByKind')}>
      {#each presentGroups as g}
        <button class="fb-chip" class:active={activeGroups.has(g)} on:click={() => toggleGroup(g)}>
          {$t(`components.fileBrowser.group_${g}`)}
        </button>
      {/each}
      {#if activeGroups.size}
        <button class="fb-chip fb-chip-clear" on:click={() => activeGroups = new Set()}>
          <XIcon size={11} /> {$t('system.clear')}
        </button>
      {/if}
    </div>
  {/if}

  {#if error}
    <div class="fb-error" role="alert">
      <span>{error}</span>
      <button class="fb-icon-btn" on:click={() => error = ''}><XIcon size={12} /></button>
    </div>
  {/if}

  <div class="fb-body">
    <!-- ── Folder tree (full mode) ─────────────────────────────────────── -->
    {#if !compact}
      <aside class="fb-tree" aria-label={$t('components.fileBrowser.folders')}>
        <button
          class="fb-tree-row"
          class:selected={!path}
          class:fb-drop-hint={dropFolderTarget === ''}
          on:click={() => setPath('')}
          on:dragover={(e) => folderDragOver(e, '')}
          on:drop={(e) => folderDrop(e, '')}
        >
          <span class="fb-tree-indent"></span>
          <HardDrive size={14} />
          <span class="fb-tree-name">{$t('components.fileBrowser.allFiles')}</span>
        </button>
        {#each treeRows as row (row.path)}
          <div class="fb-tree-line" style="--depth: {row.depth}">
            {#if row.hasChildren}
              <button class="fb-tree-caret" on:click={() => toggleTree(row.path)} aria-label={row.expanded ? $t('components.fileBrowser.collapse') : $t('components.fileBrowser.expand')}>
                {#if row.expanded}<ChevronDown size={12} />{:else}<ChevronRight size={12} />{/if}
              </button>
            {:else}
              <span class="fb-tree-caret"></span>
            {/if}
            <button
              class="fb-tree-row"
              class:selected={path === row.path}
              class:fb-drop-hint={dropFolderTarget === row.path}
              on:click={() => setPath(row.path)}
              on:dragover={(e) => folderDragOver(e, row.path)}
              on:drop={(e) => folderDrop(e, row.path)}
            >
              {#if path === row.path}<FolderOpen size={14} />{:else}<Folder size={14} />{/if}
              <span class="fb-tree-name">{row.name}</span>
            </button>
          </div>
        {/each}
      </aside>
    {/if}

    <!-- ── Content ─────────────────────────────────────────────────────── -->
    <div class="fb-content">
      {#if loading}
        <div class="fb-state"><Loader2 size={20} class="animate-spin" /> {$t('system.loading')}</div>
      {:else if isEmptyDataset}
        <div class="fb-state fb-empty-state">
          <FolderOpen size={40} />
          <p class="fb-empty-title">{$t('components.fileBrowser.emptyTitle')}</p>
          <p class="fb-empty-sub">
            {canWrite ? $t('components.fileBrowser.emptyHintWrite') : $t('components.fileBrowser.emptyHintRead')}
          </p>
          {#if canWrite && mode === 'manage'}
            <label class="btn btn-sm">
              <Upload size={13} /> {$t('components.fileBrowser.uploadFirst')}
              <input type="file" multiple style="display:none" on:change={onPickFiles} />
            </label>
          {/if}
        </div>
      {:else if !folderRows.length && !fileRows.length}
        <div class="fb-state">
          {searching
            ? $t('components.fileBrowser.noSearchResults')
            : $t('components.fileBrowser.emptyFolder')}
        </div>
      {:else if view === 'grid'}
        <div class="fb-grid">
          {#each folderRows as row (row.path)}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="fb-tile fb-tile-folder"
              class:fb-drop-hint={dropFolderTarget === row.path}
              role="button"
              tabindex="0"
              on:click={() => setPath(row.path)}
              on:contextmenu={(e) => openCtx(e, { type: 'folder', row })}
              on:dragover={(e) => folderDragOver(e, row.path)}
              on:drop={(e) => folderDrop(e, row.path)}
              on:dragleave={() => { if (dropFolderTarget === row.path) dropFolderTarget = null; }}
            >
              <div class="fb-tile-preview"><Folder size={34} class="fb-folder-icon" /></div>
              <div class="fb-tile-caption">
                <span class="fb-tile-name" title={row.name}>{row.name}</span>
                <span class="fb-tile-sub">{$t('components.fileBrowser.fileCount', { values: { count: row.fileCount } })}</span>
              </div>
              {#if canWrite && mode === 'manage'}
                <button class="fb-tile-menu" on:click|stopPropagation={(e) => openCtx(e, { type: 'folder', row })} title={$t('components.fileBrowser.actions')}>
                  <MoreHorizontal size={14} />
                </button>
              {/if}
            </div>
          {/each}
          {#each fileRows as asset (asset.id)}
            <!-- svelte-ignore a11y_click_events_have_key_events -->
            <div
              class="fb-tile"
              class:selected={selected.has(asset.id)}
              role="button"
              tabindex="0"
              draggable={canWrite && mode === 'manage'}
              on:click={(e) => clickFile(e, asset)}
              on:dblclick={() => dblClickFile(asset)}
              on:contextmenu={(e) => openCtx(e, { type: 'file', asset })}
              on:dragstart={(e) => dragStartFile(e, asset)}
            >
              <div class="fb-tile-preview">
                <FileThumb {datasetId} {asset} thumbAsset={thumbIndex.get(asset.id) || null} iconSize={30} />
              </div>
              <div class="fb-tile-caption">
                <span class="fb-tile-name" title={asset.title || asset.filename}>{asset.title || asset.filename}</span>
                <span class="fb-tile-sub">
                  {formatBytes(asset.size_bytes)}
                  {#if searching && asset.folder} · {asset.folder}{/if}
                </span>
              </div>
              {#if selected.has(asset.id)}
                <span class="fb-tile-check"><Check size={12} /></span>
              {/if}
              {#if !asset.public}
                <span class="fb-tile-lock" title={$t('components.fileBrowser.privateFile')}><Lock size={11} /></span>
              {/if}
              <button class="fb-tile-menu" on:click|stopPropagation={(e) => openCtx(e, { type: 'file', asset })} title={$t('components.fileBrowser.actions')}>
                <MoreHorizontal size={14} />
              </button>
            </div>
          {/each}
        </div>
      {:else}
        <table class="fb-table">
          <thead>
            <tr>
              {#if mode === 'manage'}
                <th class="fb-col-check">
                  <button class="fb-check" class:checked={fileRows.length > 0 && selected.size === fileRows.length} on:click={toggleSelectAll} title={$t('components.fileBrowser.selectAll')}>
                    {#if fileRows.length > 0 && selected.size === fileRows.length}<Check size={11} />{/if}
                  </button>
                </th>
              {/if}
              <th>{$t('components.fileBrowser.colName')}</th>
              <th class="fb-col-kind">{$t('components.fileBrowser.colKind')}</th>
              <th class="fb-col-size">{$t('components.fileBrowser.colSize')}</th>
              <th class="fb-col-date">{$t('components.fileBrowser.colModified')}</th>
              <th class="fb-col-vis"></th>
              <th class="fb-col-actions"></th>
            </tr>
          </thead>
          <tbody>
            {#each folderRows as row (row.path)}
              <tr
                class="fb-row fb-row-folder"
                class:fb-drop-hint={dropFolderTarget === row.path}
                on:click={() => setPath(row.path)}
                on:contextmenu={(e) => openCtx(e, { type: 'folder', row })}
                on:dragover={(e) => folderDragOver(e, row.path)}
                on:drop={(e) => folderDrop(e, row.path)}
                on:dragleave={() => { if (dropFolderTarget === row.path) dropFolderTarget = null; }}
              >
                {#if mode === 'manage'}<td class="fb-col-check"></td>{/if}
                <td>
                  <span class="fb-name-cell">
                    <Folder size={16} class="fb-folder-icon" />
                    <span class="fb-file-name">{row.name}</span>
                  </span>
                </td>
                <td class="fb-col-kind"><span class="fb-kind-chip fb-kind-folder">{$t('components.fileBrowser.kindFolder')}</span></td>
                <td class="fb-col-size">{$t('components.fileBrowser.fileCount', { values: { count: row.fileCount } })}</td>
                <td class="fb-col-date"></td>
                <td class="fb-col-vis"></td>
                <td class="fb-col-actions">
                  {#if canWrite && mode === 'manage'}
                    <button class="fb-icon-btn" on:click|stopPropagation={(e) => openCtx(e, { type: 'folder', row })} title={$t('components.fileBrowser.actions')}><MoreHorizontal size={14} /></button>
                  {/if}
                </td>
              </tr>
            {/each}
            {#each fileRows as asset (asset.id)}
              {@const kind = fileKind(asset.content_type, asset.filename)}
              <tr
                class="fb-row"
                class:selected={selected.has(asset.id)}
                draggable={canWrite && mode === 'manage'}
                on:click={(e) => clickFile(e, asset)}
                on:dblclick={() => dblClickFile(asset)}
                on:contextmenu={(e) => openCtx(e, { type: 'file', asset })}
                on:dragstart={(e) => dragStartFile(e, asset)}
              >
                {#if mode === 'manage'}
                  <td class="fb-col-check">
                    <button
                      class="fb-check"
                      class:checked={selected.has(asset.id)}
                      on:click|stopPropagation={() => clickFile({ ctrlKey: true }, asset)}
                      title={$t('components.fileBrowser.select')}
                    >
                      {#if selected.has(asset.id)}<Check size={11} />{/if}
                    </button>
                  </td>
                {/if}
                <td>
                  <span class="fb-name-cell">
                    <span class="fb-row-thumb"><FileThumb {datasetId} {asset} thumbAsset={thumbIndex.get(asset.id) || null} iconSize={16} /></span>
                    <span class="fb-name-stack">
                      <span class="fb-file-name" title={asset.filename}>{asset.title || asset.filename}</span>
                      {#if searching && asset.folder}
                        <span class="fb-file-sub">{asset.folder}</span>
                      {:else if asset.title && asset.title !== asset.filename}
                        <span class="fb-file-sub">{asset.filename}</span>
                      {/if}
                    </span>
                  </span>
                </td>
                <td class="fb-col-kind"><span class="fb-kind-chip fb-kind-{kind}">{$t(`components.fileBrowser.kind_${kind}`)}</span></td>
                <td class="fb-col-size">{formatBytes(asset.size_bytes)}</td>
                <td class="fb-col-date">{new Date(asset.updated_at || asset.created_at).toLocaleDateString()}</td>
                <td class="fb-col-vis">
                  {#if canWrite && mode === 'manage'}
                    <button
                      class="fb-vis-btn"
                      class:public={asset.public}
                      on:click|stopPropagation={() => toggleVisibility(asset)}
                      title={asset.public ? $t('components.fileBrowser.makePrivate') : $t('components.fileBrowser.makePublic')}
                    >
                      {#if asset.public}<Globe size={11} /> {$t('components.fileBrowser.public')}{:else}<Lock size={11} /> {$t('components.fileBrowser.private')}{/if}
                    </button>
                  {:else}
                    <span class="fb-vis-badge" class:public={asset.public}>
                      {#if asset.public}<Globe size={11} /> {$t('components.fileBrowser.public')}{:else}<Lock size={11} /> {$t('components.fileBrowser.private')}{/if}
                    </span>
                  {/if}
                </td>
                <td class="fb-col-actions">
                  <span class="fb-row-actions">
                    <button class="fb-icon-btn" on:click|stopPropagation={() => previewAsset = asset} title={$t('components.fileBrowser.preview')}><Eye size={13} /></button>
                    <button class="fb-icon-btn" on:click|stopPropagation={() => downloadFile(asset)} title={$t('components.fileBrowser.download')}><Download size={13} /></button>
                    <button class="fb-icon-btn" on:click|stopPropagation={() => copyIri(asset)} title={$t('components.fileBrowser.copyIri')}>
                      {#if copiedId === asset.id}<CheckCheck size={13} />{:else}<LinkIcon size={13} />{/if}
                    </button>
                    <button class="fb-icon-btn" on:click|stopPropagation={(e) => openCtx(e, { type: 'file', asset })} title={$t('components.fileBrowser.actions')}><MoreHorizontal size={13} /></button>
                  </span>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
    </div>
  </div>

  <!-- ── Footer: status / bulk actions / pick actions ────────────────────── -->
  <div class="fb-footer">
    {#if mode === 'pick'}
      <span class="fb-footer-info">
        {selected.size ? (fileRows.find((a) => selected.has(a.id))?.filename || '') : $t('components.fileBrowser.pickHint')}
      </span>
      <span class="fb-footer-actions">
        <button class="btn btn-sm btn-ghost" on:click={() => dispatch('close')}>{$t('system.cancel')}</button>
        <button
          class="btn btn-sm"
          disabled={!selected.size}
          on:click={() => { const a = fileRows.find((x) => selected.has(x.id)); if (a) dispatch('select', a); }}
        >
          <Check size={13} /> {$t('components.fileBrowser.choose')}
        </button>
      </span>
    {:else if selected.size}
      <span class="fb-footer-info">{$t('components.fileBrowser.selectedCount', { values: { count: selected.size } })}</span>
      <span class="fb-footer-actions">
        {#if canWrite}
          <button class="btn btn-xs btn-ghost" on:click={() => { moveTarget = { assetIds: [...selected] }; movePick = ''; }}>
            <FolderInput size={12} /> {$t('components.fileBrowser.move')}
          </button>
          <button class="btn btn-xs btn-ghost btn-danger" on:click={() => deleteTarget = { assets: selectedAssets }}>
            <Trash2 size={12} /> {$t('components.fileBrowser.delete')}
          </button>
        {/if}
        <button class="btn btn-xs btn-ghost" on:click={() => selected = new Set()}>{$t('system.clear')}</button>
      </span>
    {:else}
      <span class="fb-footer-info">
        {$t('components.fileBrowser.itemSummary', { values: { files: fileRows.length, folders: folderRows.length } })}
        {#if totalShownBytes} · {formatBytes(totalShownBytes)}{/if}
      </span>
    {/if}
  </div>

  <!-- ── Upload queue ────────────────────────────────────────────────────── -->
  {#if uploads.length}
    <div class="fb-uploads" aria-live="polite">
      {#each uploads as u (u.id)}
        <div class="fb-upload-row" class:error={u.status === 'error'}>
          {#if u.status === 'uploading'}
            <Loader2 size={12} class="animate-spin" />
          {:else if u.status === 'done'}
            <Check size={12} />
          {:else}
            <XIcon size={12} />
          {/if}
          <span class="fb-upload-name" title={u.name}>{u.folder ? `${u.folder}/` : ''}{u.name}</span>
          {#if u.status === 'uploading'}
            <span class="fb-upload-bar"><span class="fb-upload-fill" style="width: {Math.round(u.progress * 100)}%"></span></span>
          {:else if u.status === 'error'}
            <span class="fb-upload-err">{u.error}</span>
          {/if}
        </div>
      {/each}
    </div>
  {/if}

  <!-- ── Drop overlay ────────────────────────────────────────────────────── -->
  {#if dropActive}
    <div class="fb-drop-overlay">
      <Upload size={30} />
      <p>{path
        ? $t('components.fileBrowser.dropIntoFolder', { values: { folder: path } })
        : $t('components.fileBrowser.dropToUpload')}</p>
    </div>
  {/if}
</div>

<ContextMenu bind:visible={ctx.visible} x={ctx.x} y={ctx.y} items={ctxItems} on:action={onCtxAction} />

<!-- ── New-folder modal ──────────────────────────────────────────────────── -->
{#if newFolderOpen}
  <div class="fb-modal-backdrop" role="presentation" on:click={() => newFolderOpen = false} on:keydown={(e) => e.key === 'Escape' && (newFolderOpen = false)}>
    <div class="fb-modal" role="dialog" aria-modal="true" aria-label={$t('components.fileBrowser.newFolder')} on:click|stopPropagation on:keydown|stopPropagation tabindex="-1">
      <h3 class="fb-modal-title"><FolderPlus size={15} /> {$t('components.fileBrowser.newFolder')}</h3>
      <p class="fb-modal-sub">{path ? $t('components.fileBrowser.insideFolder', { values: { folder: path } }) : $t('components.fileBrowser.atRoot')}</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="fb-modal-input"
        autofocus
        placeholder={$t('components.fileBrowser.folderNamePlaceholder')}
        bind:value={newFolderName}
        on:keydown={(e) => e.key === 'Enter' && confirmNewFolder()}
      />
      <div class="fb-modal-actions">
        <button class="btn btn-ghost" on:click={() => newFolderOpen = false}>{$t('system.cancel')}</button>
        <button class="btn" disabled={newFolderBusy || !normalizePath(newFolderName)} on:click={confirmNewFolder}>
          {#if newFolderBusy}<Loader2 size={13} class="animate-spin" />{/if}
          {$t('components.fileBrowser.create')}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- ── Rename modal ──────────────────────────────────────────────────────── -->
{#if renameTarget}
  <div class="fb-modal-backdrop" role="presentation" on:click={() => renameTarget = null} on:keydown={(e) => e.key === 'Escape' && (renameTarget = null)}>
    <div class="fb-modal" role="dialog" aria-modal="true" aria-label={$t('components.fileBrowser.rename')} on:click|stopPropagation on:keydown|stopPropagation tabindex="-1">
      <h3 class="fb-modal-title"><Pencil size={15} /> {$t('components.fileBrowser.rename')}</h3>
      <p class="fb-modal-sub">
        {renameTarget.type === 'file' ? renameTarget.asset.filename : renameTarget.folderPath}
      </p>
      <!-- svelte-ignore a11y_autofocus -->
      <input
        class="fb-modal-input"
        autofocus
        bind:value={renameTarget.value}
        on:keydown={(e) => e.key === 'Enter' && confirmRename()}
      />
      <div class="fb-modal-actions">
        <button class="btn btn-ghost" on:click={() => renameTarget = null}>{$t('system.cancel')}</button>
        <button class="btn" disabled={renameBusy || !renameTarget.value.trim()} on:click={confirmRename}>
          {#if renameBusy}<Loader2 size={13} class="animate-spin" />{/if}
          {$t('components.fileBrowser.rename')}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- ── Move modal (destination picker) ───────────────────────────────────── -->
{#if moveTarget}
  <div class="fb-modal-backdrop" role="presentation" on:click={() => moveTarget = null} on:keydown={(e) => e.key === 'Escape' && (moveTarget = null)}>
    <div class="fb-modal" role="dialog" aria-modal="true" aria-label={$t('components.fileBrowser.moveTo')} on:click|stopPropagation on:keydown|stopPropagation tabindex="-1">
      <h3 class="fb-modal-title"><FolderInput size={15} /> {$t('components.fileBrowser.moveTo')}</h3>
      <p class="fb-modal-sub">
        {moveTarget.assetIds
          ? $t('components.fileBrowser.moveFilesSub', { values: { count: moveTarget.assetIds.length } })
          : $t('components.fileBrowser.moveFolderSub', { values: { folder: moveTarget.folderPath } })}
      </p>
      <div class="fb-move-list" role="listbox" aria-label={$t('components.fileBrowser.folders')}>
        <button class="fb-move-row" class:active={movePick === ''} role="option" aria-selected={movePick === ''} on:click={() => movePick = ''}>
          <HardDrive size={14} /> {$t('components.fileBrowser.allFiles')}
        </button>
        {#each [...folderSet].sort() as f (f)}
          {@const disabled = moveTarget.folderPath && (f === moveTarget.folderPath || isWithin(f, moveTarget.folderPath))}
          <button
            class="fb-move-row"
            class:active={movePick === f}
            style="--depth: {f.split('/').length - 1}"
            disabled={disabled}
            role="option"
            aria-selected={movePick === f}
            on:click={() => movePick = f}
          >
            <Folder size={14} /> {nameOf(f)}
          </button>
        {/each}
      </div>
      <div class="fb-modal-actions">
        <button class="btn btn-ghost" on:click={() => moveTarget = null}>{$t('system.cancel')}</button>
        <button class="btn" disabled={moveBusy} on:click={confirmMove}>
          {#if moveBusy}<Loader2 size={13} class="animate-spin" />{/if}
          {$t('components.fileBrowser.move')}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- ── Edit metadata modal ───────────────────────────────────────────────── -->
{#if editingAsset}
  <div class="fb-modal-backdrop" role="presentation" on:click={() => editingAsset = null} on:keydown={(e) => e.key === 'Escape' && (editingAsset = null)}>
    <div class="fb-modal" role="dialog" aria-modal="true" aria-label={$t('components.fileBrowser.editMetadata')} on:click|stopPropagation on:keydown|stopPropagation tabindex="-1">
      <h3 class="fb-modal-title"><FileText size={15} /> {$t('components.fileBrowser.editMetadata')}</h3>
      <p class="fb-modal-sub">{editingAsset.filename} · {formatBytes(editingAsset.size_bytes)}</p>
      <label class="fb-modal-label" for="fb-meta-title">{$t('components.fileBrowser.titleLabel')}</label>
      <input id="fb-meta-title" class="fb-modal-input" bind:value={editTitle} placeholder={editingAsset.filename} />
      <label class="fb-modal-label" for="fb-meta-desc">{$t('components.fileBrowser.descriptionLabel')}</label>
      <textarea id="fb-meta-desc" class="fb-modal-input fb-modal-textarea" rows="3" bind:value={editDesc}></textarea>
      <div class="fb-modal-actions">
        <button class="btn btn-ghost" on:click={() => editingAsset = null}>{$t('system.cancel')}</button>
        <button class="btn" disabled={editBusy} on:click={saveEdit}>
          {#if editBusy}<Loader2 size={13} class="animate-spin" />{/if}
          {$t('system.save')}
        </button>
      </div>
    </div>
  </div>
{/if}

<!-- ── Delete confirms ───────────────────────────────────────────────────── -->
{#if deleteTarget?.assets}
  <ConfirmModal
    title={$t('components.fileBrowser.deleteFilesTitle', { values: { count: deleteTarget.assets.length } })}
    message={deleteTarget.assets.length === 1
      ? deleteTarget.assets[0].filename
      : $t('components.fileBrowser.deleteFilesMsg', { values: { count: deleteTarget.assets.length } })}
    confirmLabel={$t('components.fileBrowser.delete')}
    loading={deleteBusy}
    on:confirm={confirmDelete}
    on:cancel={() => deleteTarget = null}
  />
{:else if deleteTarget?.folder}
  <ConfirmModal
    title={$t('components.fileBrowser.deleteFolderTitle')}
    message={deleteTarget.folder.fileCount > 0
      ? $t('components.fileBrowser.deleteFolderMsgFiles', { values: { folder: deleteTarget.folder.path, count: deleteTarget.folder.fileCount } })
      : $t('components.fileBrowser.deleteFolderMsgEmpty', { values: { folder: deleteTarget.folder.path } })}
    confirmLabel={$t('components.fileBrowser.delete')}
    loading={deleteBusy}
    on:confirm={confirmDelete}
    on:cancel={() => deleteTarget = null}
  />
{/if}

{#if previewAsset}
  <AssetPreview asset={previewAsset} {datasetId} on:close={() => previewAsset = null} />
{/if}

<style>
  .fb {
    position: relative;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 14px;
    background: var(--bg, #fff);
    outline: none;
  }
  .fb-compact { border: none; background: transparent; }

  /* ── Toolbar ── */
  .fb-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.6rem;
    flex-wrap: wrap;
    padding: 0.6rem 0.75rem;
    border-bottom: 1px solid var(--line-soft, #eef2f6);
  }
  .fb-compact .fb-toolbar { padding: 0.45rem 0.15rem; }

  .fb-crumbs { display: flex; align-items: center; gap: 0.15rem; min-width: 0; flex-wrap: wrap; }
  .fb-crumb {
    display: inline-flex; align-items: center; gap: 0.3rem;
    border: none; background: transparent; cursor: pointer;
    padding: 0.25rem 0.45rem; border-radius: 8px;
    font-size: 0.82rem; color: var(--ink-500, #64748b);
    max-width: 180px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    transition: background 0.12s, color 0.12s;
  }
  .fb-crumb:hover { background: var(--bg-soft, #f1f5f9); color: var(--ink-800, #1e293b); }
  .fb-crumb-current { color: var(--ink-900, #0f172a); font-weight: 700; }
  .fb :global(.fb-crumb-sep) { color: var(--ink-300, #cbd5e1); flex-shrink: 0; }

  .fb-tools { display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; }

  .fb-search {
    display: flex; align-items: center; gap: 0.35rem;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 9px;
    padding: 0.28rem 0.5rem;
    color: var(--ink-400, #94a3b8);
    background: var(--bg, #fff);
  }
  .fb-search input {
    border: none; outline: none; background: transparent;
    font-size: 0.8rem; width: 150px; color: var(--ink-900, #0f172a);
  }
  .fb-search:focus-within { border-color: var(--brand-300, #93c5fd); }

  .fb-tool-btn {
    display: inline-flex; align-items: center; gap: 0.32rem;
    border: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg, #fff);
    border-radius: 9px; padding: 0.3rem 0.5rem;
    font-size: 0.76rem; color: var(--ink-600, #475569); cursor: pointer;
    transition: background 0.12s, border-color 0.12s;
  }
  .fb-tool-btn:hover { background: var(--bg-soft, #f8fafc); border-color: var(--brand-200, #bfdbfe); }
  .fb-tool-btn:disabled { opacity: 0.5; cursor: default; }
  .fb-tool-label { max-width: 90px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .fb-sort { position: relative; }
  .fb-sort-menu {
    position: absolute; top: calc(100% + 4px); right: 0; z-index: 30;
    min-width: 140px;
    background: var(--bg-strong, #fff);
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 10px; padding: 4px;
    box-shadow: 0 8px 24px rgba(15, 23, 42, 0.12);
  }
  .fb-sort-menu button {
    display: flex; align-items: center; justify-content: space-between; gap: 0.5rem;
    width: 100%; text-align: left; padding: 0.4rem 0.6rem;
    border: none; background: transparent; border-radius: 7px;
    font-size: 0.8rem; color: var(--ink-700, #334155); cursor: pointer;
  }
  .fb-sort-menu button:hover { background: var(--bg-soft, #f1f5f9); }
  .fb-sort-menu button.active { font-weight: 700; color: var(--brand-600, #2563eb); }
  .fb-sort-dir { font-size: 0.75rem; }

  .fb-view-toggle {
    display: inline-flex; border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 9px; overflow: hidden;
  }
  .fb-view-toggle button {
    display: grid; place-items: center;
    border: none; background: transparent; cursor: pointer;
    padding: 0.32rem 0.5rem; color: var(--ink-400, #94a3b8);
    transition: background 0.12s, color 0.12s;
  }
  .fb-view-toggle button.active { background: var(--brand-50, #eff6ff); color: var(--brand-600, #2563eb); }

  /* ── Filter chips ── */
  .fb-chips { display: flex; align-items: center; gap: 0.35rem; flex-wrap: wrap; padding: 0.45rem 0.75rem 0; }
  .fb-compact .fb-chips { padding: 0.45rem 0.15rem 0; }
  .fb-chip {
    border: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg, #fff);
    border-radius: 999px; padding: 0.2rem 0.65rem;
    font-size: 0.72rem; font-weight: 600; color: var(--ink-500, #64748b);
    cursor: pointer; transition: all 0.12s;
    display: inline-flex; align-items: center; gap: 0.25rem;
  }
  .fb-chip:hover { border-color: var(--brand-300, #93c5fd); color: var(--ink-800, #1e293b); }
  .fb-chip.active {
    background: var(--brand-600, #2563eb); border-color: var(--brand-600, #2563eb); color: #fff;
  }
  .fb-chip-clear { border-style: dashed; }

  .fb-error {
    display: flex; align-items: center; justify-content: space-between; gap: 0.5rem;
    margin: 0.5rem 0.75rem 0;
    padding: 0.45rem 0.65rem;
    background: rgba(220, 38, 38, 0.07);
    border: 1px solid rgba(220, 38, 38, 0.25);
    border-radius: 10px;
    color: #b91c1c; font-size: 0.8rem;
  }

  /* ── Body layout ── */
  .fb-body { display: flex; min-height: 0; flex: 1; }
  .fb-compact .fb-body { max-height: 430px; }

  .fb-tree {
    width: 200px; flex-shrink: 0;
    border-right: 1px solid var(--line-soft, #eef2f6);
    padding: 0.5rem 0.4rem;
    overflow-y: auto;
    display: flex; flex-direction: column; gap: 1px;
  }
  .fb-tree-line { display: flex; align-items: center; padding-left: calc(var(--depth) * 0.85rem); }
  .fb-tree-caret {
    width: 18px; height: 18px; flex-shrink: 0;
    display: grid; place-items: center;
    border: none; background: transparent; color: var(--ink-400, #94a3b8);
    cursor: pointer; border-radius: 5px; padding: 0;
  }
  .fb-tree-caret:hover { background: var(--bg-soft, #f1f5f9); }
  .fb-tree-indent { width: 18px; flex-shrink: 0; }
  .fb-tree-row {
    display: flex; align-items: center; gap: 0.4rem;
    flex: 1; min-width: 0;
    border: none; background: transparent; cursor: pointer;
    padding: 0.3rem 0.45rem; border-radius: 8px;
    font-size: 0.8rem; color: var(--ink-600, #475569);
    transition: background 0.12s, color 0.12s;
  }
  .fb-tree-row:hover { background: var(--bg-soft, #f1f5f9); }
  .fb-tree-row.selected { background: var(--brand-50, #eff6ff); color: var(--brand-700, #1d4ed8); font-weight: 700; }
  .fb-tree-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .fb-content { flex: 1; min-width: 0; overflow: auto; padding: 0.6rem 0.75rem; }
  .fb-compact .fb-content { padding: 0.5rem 0.15rem; }

  .fb-state {
    display: flex; flex-direction: column; align-items: center; justify-content: center;
    gap: 0.5rem; min-height: 140px;
    color: var(--ink-400, #94a3b8); font-size: 0.85rem; text-align: center;
    padding: 1.5rem;
  }
  .fb-empty-state :global(svg:first-child) { opacity: 0.35; }
  .fb-empty-title { margin: 0; font-weight: 700; color: var(--ink-700, #334155); }
  .fb-empty-sub { margin: 0; font-size: 0.8rem; max-width: 380px; }

  /* ── Grid view ── */
  .fb-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(136px, 1fr));
    gap: 0.6rem;
  }
  .fb-tile {
    position: relative;
    display: flex; flex-direction: column;
    border: 1px solid var(--line-soft, #e8edf3);
    border-radius: 12px;
    background: var(--bg, #fff);
    cursor: pointer;
    overflow: hidden;
    transition: border-color 0.12s, box-shadow 0.12s, transform 0.12s;
  }
  .fb-tile:hover { border-color: var(--brand-200, #bfdbfe); box-shadow: 0 3px 12px rgba(15, 23, 42, 0.07); }
  .fb-tile.selected { border-color: var(--brand-500, #3b82f6); box-shadow: 0 0 0 2px rgba(59, 130, 246, 0.25); }
  .fb-tile-preview {
    height: 84px;
    display: flex; align-items: center; justify-content: center;
    background: var(--bg-soft, #f8fafc);
    border-bottom: 1px solid var(--line-soft, #eef2f6);
  }
  .fb-tile-folder .fb-tile-preview { background: transparent; border-bottom-color: transparent; }
  .fb :global(.fb-folder-icon) { color: #eab308; fill: rgba(234, 179, 8, 0.22); }
  .fb-tile-caption { display: flex; flex-direction: column; gap: 0.1rem; padding: 0.45rem 0.55rem 0.5rem; min-width: 0; }
  .fb-tile-name {
    font-size: 0.78rem; font-weight: 600; color: var(--ink-800, #1e293b);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .fb-tile-sub { font-size: 0.68rem; color: var(--ink-400, #94a3b8); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .fb-tile-check {
    position: absolute; top: 6px; left: 6px;
    width: 18px; height: 18px; border-radius: 6px;
    display: grid; place-items: center;
    background: var(--brand-600, #2563eb); color: #fff;
  }
  .fb-tile-lock {
    position: absolute; top: 6px; right: 6px;
    display: grid; place-items: center;
    width: 18px; height: 18px; border-radius: 6px;
    background: rgba(15, 23, 42, 0.55); color: #fff;
  }
  .fb-tile-menu {
    position: absolute; bottom: 6px; right: 6px;
    width: 22px; height: 22px; border-radius: 7px;
    display: grid; place-items: center;
    border: none; cursor: pointer;
    background: transparent; color: var(--ink-400, #94a3b8);
    opacity: 0; transition: opacity 0.12s, background 0.12s;
  }
  .fb-tile:hover .fb-tile-menu { opacity: 1; }
  .fb-tile-menu:hover { background: var(--bg-soft, #eef2f6); color: var(--ink-700, #334155); }

  /* ── List view ── */
  .fb-table { width: 100%; border-collapse: collapse; font-size: 0.82rem; }
  .fb-table th {
    text-align: left; font-size: 0.68rem; font-weight: 700;
    text-transform: uppercase; letter-spacing: 0.06em;
    color: var(--ink-400, #94a3b8);
    padding: 0.3rem 0.5rem; border-bottom: 1px solid var(--line-soft, #eef2f6);
    white-space: nowrap;
  }
  .fb-row { cursor: pointer; transition: background 0.1s; }
  .fb-row:hover { background: var(--bg-soft, #f8fafc); }
  .fb-row.selected { background: var(--brand-50, #eff6ff); }
  .fb-row td { padding: 0.38rem 0.5rem; border-bottom: 1px solid var(--line-soft, #f2f5f9); vertical-align: middle; }
  .fb-col-check { width: 30px; }
  .fb-col-kind { width: 105px; }
  .fb-col-size { width: 90px; white-space: nowrap; color: var(--ink-500, #64748b); }
  .fb-col-date { width: 100px; white-space: nowrap; color: var(--ink-500, #64748b); }
  .fb-col-vis { width: 92px; }
  .fb-col-actions { width: 130px; text-align: right; }

  .fb-check {
    width: 16px; height: 16px; border-radius: 5px;
    border: 1.5px solid var(--line-strong, #cbd5e1);
    background: var(--bg, #fff); cursor: pointer;
    display: grid; place-items: center; padding: 0;
    color: #fff; transition: all 0.12s;
  }
  .fb-check.checked { background: var(--brand-600, #2563eb); border-color: var(--brand-600, #2563eb); }

  .fb-name-cell { display: flex; align-items: center; gap: 0.5rem; min-width: 0; }
  .fb-row-thumb {
    width: 28px; height: 28px; flex-shrink: 0;
    border-radius: 7px; overflow: hidden;
    background: var(--bg-soft, #f1f5f9);
    display: grid; place-items: center;
  }
  .fb-name-stack { display: flex; flex-direction: column; min-width: 0; }
  .fb-file-name {
    font-weight: 600; color: var(--ink-800, #1e293b);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 380px;
  }
  .fb-file-sub { font-size: 0.7rem; color: var(--ink-400, #94a3b8); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  .fb-kind-chip {
    display: inline-block;
    font-size: 0.66rem; font-weight: 700;
    padding: 0.14rem 0.5rem; border-radius: 999px;
    background: var(--bg-soft, #f1f5f9); color: var(--ink-500, #64748b);
    white-space: nowrap;
  }
  .fb-kind-image { background: #e0f2fe; color: #0369a1; }
  .fb-kind-video { background: #ede9fe; color: #6d28d9; }
  .fb-kind-audio { background: #fae8ff; color: #a21caf; }
  .fb-kind-model3d { background: #fef3c7; color: #b45309; }
  .fb-kind-cad { background: #ffedd5; color: #c2410c; }
  .fb-kind-pointcloud { background: #d1fae5; color: #047857; }
  .fb-kind-geodata { background: #d1fae5; color: #065f46; }
  .fb-kind-document { background: #dbeafe; color: #1d4ed8; }
  .fb-kind-spreadsheet { background: #dcfce7; color: #15803d; }
  .fb-kind-archive { background: #fef9c3; color: #854d0e; }
  .fb-kind-folder { background: #fef3c7; color: #a16207; }

  .fb-vis-btn, .fb-vis-badge {
    display: inline-flex; align-items: center; gap: 0.25rem;
    font-size: 0.68rem; font-weight: 700;
    padding: 0.16rem 0.5rem; border-radius: 999px;
    border: 1px solid var(--line-soft, #e2e8f0);
    background: var(--bg-soft, #f8fafc); color: var(--ink-500, #64748b);
    white-space: nowrap;
  }
  .fb-vis-btn { cursor: pointer; transition: all 0.12s; }
  .fb-vis-btn:hover { border-color: var(--brand-300, #93c5fd); }
  .fb-vis-btn.public, .fb-vis-badge.public { background: #dcfce7; border-color: #bbf7d0; color: #15803d; }

  .fb-row-actions { display: inline-flex; gap: 0.15rem; opacity: 0; transition: opacity 0.12s; }
  .fb-row:hover .fb-row-actions, .fb-row.selected .fb-row-actions { opacity: 1; }
  .fb-icon-btn {
    width: 24px; height: 24px; border-radius: 7px;
    display: inline-grid; place-items: center;
    border: none; background: transparent; cursor: pointer;
    color: var(--ink-400, #94a3b8);
    transition: background 0.12s, color 0.12s;
  }
  .fb-icon-btn:hover { background: var(--bg-soft, #eef2f6); color: var(--ink-700, #334155); }

  /* ── Footer ── */
  .fb-footer {
    display: flex; align-items: center; justify-content: space-between; gap: 0.75rem;
    padding: 0.45rem 0.75rem;
    border-top: 1px solid var(--line-soft, #eef2f6);
    font-size: 0.74rem; color: var(--ink-400, #94a3b8);
    min-height: 38px;
  }
  .fb-compact .fb-footer { padding: 0.45rem 0.15rem 0; }
  .fb-footer-info { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .fb-footer-actions { display: inline-flex; align-items: center; gap: 0.35rem; flex-shrink: 0; }

  /* ── Upload queue ── */
  .fb-uploads {
    position: absolute; right: 10px; bottom: 46px; z-index: 20;
    width: min(320px, calc(100% - 20px));
    display: flex; flex-direction: column; gap: 4px;
    background: var(--bg-strong, #fff);
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 12px; padding: 8px;
    box-shadow: 0 10px 30px rgba(15, 23, 42, 0.14);
    max-height: 220px; overflow-y: auto;
  }
  .fb-upload-row {
    display: flex; align-items: center; gap: 0.4rem;
    font-size: 0.74rem; color: var(--ink-600, #475569);
  }
  .fb-upload-row.error { color: #b91c1c; }
  .fb-upload-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .fb-upload-bar {
    width: 80px; height: 4px; flex-shrink: 0;
    border-radius: 999px; background: var(--bg-soft, #eef2f6); overflow: hidden;
  }
  .fb-upload-fill { display: block; height: 100%; background: var(--brand-500, #3b82f6); transition: width 0.15s; }
  .fb-upload-err { font-size: 0.68rem; max-width: 140px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  /* ── Drag & drop ── */
  .fb-drop-overlay {
    position: absolute; inset: 0; z-index: 40;
    display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 0.5rem;
    background: rgba(37, 99, 235, 0.08);
    border: 2px dashed var(--brand-400, #60a5fa);
    border-radius: inherit;
    color: var(--brand-700, #1d4ed8); font-weight: 700; font-size: 0.9rem;
    pointer-events: none;
  }
  .fb-drop-overlay p { margin: 0; }
  .fb-drop-hint { outline: 2px dashed var(--brand-400, #60a5fa); outline-offset: -2px; background: var(--brand-50, #eff6ff) !important; }

  /* ── Modals ── */
  .fb-modal-backdrop {
    position: fixed; inset: 0; z-index: 40000;
    background: rgba(15, 23, 42, 0.45);
    display: flex; align-items: center; justify-content: center;
    padding: 1rem;
  }
  .fb-modal {
    width: min(430px, 100%);
    background: var(--bg-strong, #fff);
    border-radius: 14px;
    padding: 1.1rem 1.2rem 1rem;
    box-shadow: 0 20px 60px rgba(0, 0, 0, 0.22);
    display: flex; flex-direction: column; gap: 0.5rem;
    outline: none;
  }
  .fb-modal-title { display: flex; align-items: center; gap: 0.4rem; margin: 0; font-size: 0.95rem; font-weight: 700; color: var(--ink-900, #0f172a); }
  .fb-modal-sub { margin: 0; font-size: 0.78rem; color: var(--ink-400, #94a3b8); word-break: break-all; }
  .fb-modal-label { font-size: 0.72rem; font-weight: 700; color: var(--ink-500, #64748b); margin-top: 0.25rem; }
  .fb-modal-input {
    width: 100%; box-sizing: border-box;
    border: 1px solid var(--line-soft, #e2e8f0);
    border-radius: 9px; padding: 0.5rem 0.65rem;
    font-size: 0.85rem; color: var(--ink-900, #0f172a);
    background: var(--bg, #fff);
    outline: none;
  }
  .fb-modal-input:focus { border-color: var(--brand-400, #60a5fa); box-shadow: 0 0 0 3px rgba(59, 130, 246, 0.14); }
  .fb-modal-textarea { resize: vertical; font-family: inherit; }
  .fb-modal-actions { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.5rem; }

  .fb-move-list {
    display: flex; flex-direction: column; gap: 1px;
    max-height: 240px; overflow-y: auto;
    border: 1px solid var(--line-soft, #eef2f6);
    border-radius: 10px; padding: 4px;
  }
  .fb-move-row {
    display: flex; align-items: center; gap: 0.45rem;
    padding: 0.35rem 0.5rem; padding-left: calc(0.5rem + var(--depth, 0) * 0.9rem);
    border: none; background: transparent; border-radius: 7px;
    font-size: 0.8rem; color: var(--ink-700, #334155);
    cursor: pointer; text-align: left;
  }
  .fb-move-row:hover:not(:disabled) { background: var(--bg-soft, #f1f5f9); }
  .fb-move-row.active { background: var(--brand-50, #eff6ff); color: var(--brand-700, #1d4ed8); font-weight: 700; }
  .fb-move-row:disabled { opacity: 0.4; cursor: default; }

  /* ── Dark mode ── */
  :global(:is([data-theme="dark"], .dark)) .fb { background: var(--bg-strong); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .fb-compact { background: transparent; }
  :global(:is([data-theme="dark"], .dark)) .fb-search,
  :global(:is([data-theme="dark"], .dark)) .fb-tool-btn,
  :global(:is([data-theme="dark"], .dark)) .fb-chip,
  :global(:is([data-theme="dark"], .dark)) .fb-view-toggle { background: rgba(255,255,255,0.04); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .fb-chip.active { background: var(--brand-600, #2563eb); border-color: var(--brand-600, #2563eb); }
  :global(:is([data-theme="dark"], .dark)) .fb-tile,
  :global(:is([data-theme="dark"], .dark)) .fb-modal,
  :global(:is([data-theme="dark"], .dark)) .fb-sort-menu,
  :global(:is([data-theme="dark"], .dark)) .fb-uploads { background: var(--bg-strong); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .fb-tile-preview { background: rgba(255,255,255,0.04); border-color: var(--line-soft); }
  :global(:is([data-theme="dark"], .dark)) .fb-row:hover { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .fb-row.selected,
  :global(:is([data-theme="dark"], .dark)) .fb-tree-row.selected,
  :global(:is([data-theme="dark"], .dark)) .fb-move-row.active { background: rgba(59,130,246,0.16); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .fb-kind-chip { background: rgba(255,255,255,0.07); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .fb-vis-btn,
  :global(:is([data-theme="dark"], .dark)) .fb-vis-badge { background: rgba(255,255,255,0.05); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .fb-vis-btn.public,
  :global(:is([data-theme="dark"], .dark)) .fb-vis-badge.public { background: rgba(34,197,94,0.14); border-color: rgba(34,197,94,0.3); color: #86efac; }
  :global(:is([data-theme="dark"], .dark)) .fb-modal-input { background: rgba(255,255,255,0.05); border-color: var(--line-strong); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .fb-error { background: rgba(220,38,38,0.12); border-color: rgba(220,38,38,0.35); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .fb-drop-overlay { background: rgba(59,130,246,0.12); }

  @media (max-width: 760px) {
    .fb-tree { display: none; }
    .fb-col-date, .fb-col-kind { display: none; }
    .fb-file-name { max-width: 180px; }
  }
</style>
