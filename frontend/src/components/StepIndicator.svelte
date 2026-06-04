<script>
  import { t } from 'svelte-i18n';
  import { Check } from 'lucide-svelte';

  export let steps = [];
  export let current = 0;
</script>

<!-- Desktop: horizontal stepper -->
<div class="hidden sm:flex items-center gap-0 w-full">
  {#each steps as step, i}
    <div class="flex items-center gap-2.5 {i < steps.length - 1 ? 'flex-1' : ''}">
      <div class="flex items-center gap-2 min-w-0">
        <span
          class="flex items-center justify-center w-8 h-8 rounded-full text-sm font-bold shrink-0 transition-all
            {i < current ? 'bg-[var(--brand-500)] text-white' : i === current ? 'bg-[var(--brand-500)] text-white ring-4 ring-[var(--brand-300)]/30' : 'bg-[var(--line-soft)] text-[var(--ink-500)]'}"
        >
          {#if i < current}<Check size={16} />{:else}{i + 1}{/if}
        </span>
        <span class="text-sm font-medium truncate {i <= current ? 'text-[var(--ink-900)]' : 'text-[var(--ink-500)]'}">
          {step}
        </span>
      </div>
      {#if i < steps.length - 1}
        <div class="flex-1 h-px mx-2 {i < current ? 'bg-[var(--brand-500)]' : 'bg-[var(--line-soft)]'} transition-colors"></div>
      {/if}
    </div>
  {/each}
</div>

<!-- Mobile: compact progress -->
<div class="flex sm:hidden items-center gap-3">
  <span class="flex items-center justify-center w-8 h-8 rounded-full bg-[var(--brand-500)] text-white text-sm font-bold">{current + 1}</span>
  <div class="flex-1 min-w-0">
    <div class="text-sm font-semibold truncate">{steps[current] || ''}</div>
    <div class="text-xs text-[var(--ink-500)]">{$t('pages.tripleBrowser.page')} {current + 1} / {steps.length}</div>
  </div>
  <div class="flex gap-1">
    {#each steps as _, i}
      <span class="w-2 h-2 rounded-full {i <= current ? 'bg-[var(--brand-500)]' : 'bg-[var(--line-soft)]'} transition-colors"></span>
    {/each}
  </div>
</div>
