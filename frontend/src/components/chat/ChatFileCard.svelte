<script>
  // Compact attachment card for ```file blocks: extension-matched icon, name,
  // Open / Download actions and an inline preview for images, audio and video.
  // chatRich.parseFileSpec already gated the URL through the shared scheme
  // allowlist — a blocked spec arrives with url:'' + blocked:true and renders
  // an inert card. The component re-gates anyway (defence in depth), so no
  // model-controlled string reaches href/src unchecked. SVG previews go
  // through <img> only, which cannot execute embedded scripts.
  import { t } from 'svelte-i18n';
  import {
    File, FileText, FileImage, FileAudio, FileVideo, FileArchive,
    FileCode, FileSpreadsheet, Boxes, Ban, Download, ExternalLink,
  } from 'lucide-svelte';
  import { safeExternalUrl, safeImageUrl } from '../../lib/safeUrl.js';

  /** @type {{label?: string, url: string, filename?: string, blocked?: boolean}} */
  export let file;

  const IMAGE_EXT = new Set(['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'avif', 'bmp']);
  const AUDIO_EXT = new Set(['mp3', 'wav', 'ogg', 'oga', 'm4a', 'flac', 'opus', 'aac', 'weba']);
  const VIDEO_EXT = new Set(['mp4', 'webm', 'ogv', 'mov', 'm4v']);
  const ARCHIVE_EXT = new Set(['zip', 'gz', 'tgz', 'tar', '7z', 'rar']);
  const SHEET_EXT = new Set(['csv', 'tsv', 'xlsx', 'xls', 'ods']);
  const TEXT_EXT = new Set(['pdf', 'txt', 'md', 'doc', 'docx', 'rtf', 'odt']);
  const CODE_EXT = new Set([
    'js', 'ts', 'py', 'rs', 'java', 'sh', 'sql', 'html', 'css',
    'json', 'jsonld', 'xml', 'yml', 'yaml', 'toml',
    'ttl', 'rq', 'nt', 'nq', 'trig', 'rdf', 'owl', 'shacl',
  ]);
  const MODEL_EXT = new Set(['glb', 'gltf', 'stl', 'ifc', 'obj', 'cityjson', 'citygml', 'gml']);

  function extOf(s) {
    const clean = String(s || '').split(/[?#]/)[0];
    const m = /\.([a-z0-9]{1,8})$/i.exec(clean);
    return m ? m[1].toLowerCase() : '';
  }

  $: url = file?.blocked ? undefined : safeExternalUrl(file?.url);
  $: name = file?.filename || file?.label || '';
  $: ext = extOf(file?.filename) || extOf(url);
  $: preview = IMAGE_EXT.has(ext) ? 'image' : AUDIO_EXT.has(ext) ? 'audio' : VIDEO_EXT.has(ext) ? 'video' : '';
  $: imgSrc = preview === 'image' ? safeImageUrl(url) : undefined;

  // The download attribute only works same-origin — external URLs get Open only.
  function isSameOrigin(u) {
    if (!u) return false;
    try {
      return new URL(u, window.location.href).origin === window.location.origin;
    } catch {
      return false;
    }
  }
  $: sameOrigin = isSameOrigin(url);

  $: Icon = file?.blocked
    ? Ban
    : IMAGE_EXT.has(ext) ? FileImage
    : AUDIO_EXT.has(ext) ? FileAudio
    : VIDEO_EXT.has(ext) ? FileVideo
    : ARCHIVE_EXT.has(ext) ? FileArchive
    : SHEET_EXT.has(ext) ? FileSpreadsheet
    : CODE_EXT.has(ext) ? FileCode
    : MODEL_EXT.has(ext) ? Boxes
    : TEXT_EXT.has(ext) ? FileText
    : File;

  let imgFailed = false;
  $: if (imgSrc) imgFailed = false;
</script>

<div class="file-card">
  <div class="row">
    <span class="icon" class:blocked={file?.blocked} title={$t('components.chat.fileTitle')}>
      <svelte:component this={Icon} size={20} />
    </span>
    <div class="meta">
      {#if file?.label && file.label !== name}
        <p class="label">{file.label}</p>
        {#if name}<p class="filename">{name}</p>{/if}
      {:else}
        <p class="label">{name || file?.label || $t('components.chat.fileTitle')}</p>
      {/if}
      {#if !url}
        <p class="note blocked-note">{$t('components.chat.fileBlockedUrl')}</p>
      {/if}
    </div>
    {#if url}
      <div class="actions">
        <a class="action" href={url} target="_blank" rel="noopener noreferrer">
          <ExternalLink size={12} /> {$t('components.chat.fileOpen')}
        </a>
        {#if sameOrigin}
          <a class="action" href={url} download={file?.filename || ''}>
            <Download size={12} /> {$t('components.chat.download')}
          </a>
        {/if}
      </div>
    {/if}
  </div>
  {#if url}
    {#if preview === 'image' && imgSrc && !imgFailed}
      <img class="preview-img" src={imgSrc} alt={name} loading="lazy" on:error={() => (imgFailed = true)} />
    {:else if preview === 'audio'}
      <audio class="preview-media" controls preload="none" src={url}></audio>
    {:else if preview === 'video'}
      <!-- model-referenced files carry no caption track -->
      <!-- svelte-ignore a11y-media-has-caption -->
      <video class="preview-media" controls preload="metadata" src={url}></video>
    {:else}
      <p class="note">{$t('components.chat.filePreviewUnavailable')}</p>
    {/if}
  {/if}
</div>

<style>
  .file-card {
    margin: 0 0 0.55rem; padding: 0.55rem 0.7rem;
    border: 1px solid var(--line-soft); border-radius: 10px;
    background: var(--bg-strong);
  }
  .row { display: flex; align-items: center; gap: 0.6rem; }
  .icon { display: flex; color: #6d4ad9; flex-shrink: 0; }
  .icon.blocked { color: #dc2626; }
  .meta { flex: 1; min-width: 0; }
  .label {
    margin: 0; font-size: 0.82rem; font-weight: 600; color: var(--ink-800);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .filename {
    margin: 0.1rem 0 0; font-family: 'SF Mono', ui-monospace, monospace;
    font-size: 0.7rem; color: var(--ink-400);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .note { margin: 0.35rem 0 0; font-size: 0.7rem; color: var(--ink-400); font-style: italic; }
  .blocked-note { margin: 0.1rem 0 0; color: #dc2626; }
  .actions { display: flex; gap: 0.35rem; flex-shrink: 0; }
  .action {
    display: inline-flex; align-items: center; gap: 0.25rem;
    padding: 0.2rem 0.55rem; border: 1px solid var(--line-strong);
    border-radius: 7px; font-size: 0.72rem; font-weight: 600;
    color: var(--ink-600); text-decoration: none; white-space: nowrap;
    transition: background 0.12s, border-color 0.12s, color 0.12s;
  }
  .action:hover { background: var(--bg-soft, #f1f5f9); color: var(--ink-800); }
  .preview-img {
    display: block; margin-top: 0.5rem; max-width: 100%; max-height: 240px;
    border-radius: 8px; border: 1px solid var(--line-soft);
  }
  .preview-media { display: block; margin-top: 0.5rem; width: 100%; border-radius: 8px; }
  video.preview-media { max-height: 280px; background: #000; }
  :global(:is([data-theme="dark"], .dark)) .icon { color: #c4b5fd; }
</style>
