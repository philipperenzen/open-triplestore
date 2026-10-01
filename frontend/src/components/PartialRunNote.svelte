<script>
  import { t } from 'svelte-i18n';
  import { EyeOff } from 'lucide-svelte';

  /**
   * Says that a run left out graphs its caller may not read: the server
   * answers `partial: true` then. A partial validation run is a test run (not
   * recorded, the dataset's status unchanged); a partial inference run did not
   * apply the rules of the shapes graphs it left out.
   * @type {{ kind?: 'validate' | 'infer' }}
   */
  let { kind = 'validate' } = $props();
</script>

<p class="partial-run-note" role="note">
  <EyeOff size={14} />
  <span>{kind === 'infer' ? $t('components.partialRunNote.infer') : $t('components.partialRunNote.validate')}</span>
</p>

<style>
  .partial-run-note { display: flex; align-items: flex-start; gap: 0.45rem; margin: 0.5rem 0; padding: 0.45rem 0.7rem; border-radius: 8px; font-size: 0.8rem; line-height: 1.4; background: #fef3c7; color: #92400e; }
  .partial-run-note :global(svg) { flex-shrink: 0; margin-top: 0.1rem; }
  :global(:is([data-theme="dark"], .dark)) .partial-run-note { background: rgba(217, 119, 6, 0.18); color: #fcd34d; }
</style>
