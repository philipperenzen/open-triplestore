<script lang="ts">
  // A dataset's stored SWRL rules (GET/POST/PUT/DELETE /api/datasets/:id/rules):
  // the rule sets kept in its managed rules graph, editable by its writers,
  // and the rules in its other rule graphs, read-only. Every change runs the
  // rules again; the report of that run is shown.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Loader2, Pencil, Play, Plus, ScrollText, Trash2 } from 'lucide-svelte';
  import SwrlRuleEditor from './SwrlRuleEditor.svelte';
  import RulesReportView from './RulesReportView.svelte';
  import {
    listDatasetRules, createDatasetRules, updateDatasetRules, deleteDatasetRules, runDatasetRules,
    type DatasetRules, type RulesRun, type SwrlFormat, type SwrlRuleSet,
  } from '../../lib/api';
  import { parsePrefixLines, prefixLines } from '../../lib/reasoning';
  import { askConfirm } from '../../lib/confirm';
  import { toastError, toastInfo, toastSuccess } from '../../lib/toast';
  import './reasoning.css';

  export let datasetId: string;
  export let canWrite = false;

  /** `changed`: the stored rules (and what they derive) changed. */
  const dispatch = createEventDispatcher<{ changed: void }>();

  let data: DatasetRules | null = null;
  let loading = true; // until the first answer
  let loadError = '';
  let unavailable = false;

  // The form: null closed, '' a new set, else the id of the set being edited.
  let editing: string | null = null;
  let label = '';
  let format: SwrlFormat = 'swrlapi';
  let rules = '';
  let prefixText = '';
  let rdfFormat = 'turtle';
  let baseIri = '';
  let saving = false;
  let formError = '';
  let lastRun: RulesRun | null = null;
  let running = false;

  async function load(target = datasetId) {
    try {
      data = await listDatasetRules(target);
      loadError = '';
    } catch (e) {
      const status = (e as { status?: number })?.status;
      if (status === 503) unavailable = true;
      else loadError = (e as Error)?.message || String(e);
    } finally {
      loading = false;
    }
  }

  $: load(datasetId);
  $: writable = canWrite && (data?.can_write ?? false);

  function openNew() {
    editing = '';
    label = '';
    format = 'swrlapi';
    rules = '';
    prefixText = '';
    rdfFormat = 'turtle';
    baseIri = '';
    formError = '';
  }

  function openEdit(set: SwrlRuleSet) {
    editing = set.id;
    label = set.label ?? '';
    format = (set.format ?? 'functional') as SwrlFormat;
    rules = set.source ?? set.rules.map((r) => r.functional ?? '').join('\n');
    prefixText = prefixLines(set.options?.prefixes);
    rdfFormat = set.options?.rdf_format ?? 'turtle';
    baseIri = set.options?.base_iri ?? '';
    formError = '';
  }

  async function save() {
    const { prefixes, bad } = parsePrefixLines(prefixText);
    if (format === 'swrlapi' && bad.length) {
      formError = $t('components.swrl.badPrefixLines', { values: { lines: bad.join(', ') } });
      return;
    }
    saving = true;
    formError = '';
    try {
      const body = {
        label: label.trim() || undefined,
        rules,
        format,
        prefixes: format === 'swrlapi' ? prefixes : undefined,
        rdf_format: format === 'rdf' ? rdfFormat : undefined,
        base_iri: format === 'rdf' && baseIri.trim() ? baseIri.trim() : undefined,
      };
      const res = editing ? await updateDatasetRules(datasetId, editing, body) : await createDatasetRules(datasetId, body);
      lastRun = res.run;
      toastSuccess($t('components.swrl.saved', { values: { count: res.rules } }));
      editing = null;
      dispatch('changed');
      await load();
    } catch (e) {
      formError = (e as Error)?.message || String(e);
    } finally {
      saving = false;
    }
  }

  async function remove(set: SwrlRuleSet) {
    const ok = await askConfirm({
      title: $t('components.swrl.deleteTitle'),
      message: $t('components.swrl.deleteMessage', { values: { name: set.label || set.id, count: set.rules.length } }),
      confirmLabel: $t('system.delete'),
      variant: 'danger',
    });
    if (!ok) return;
    try {
      const res = await deleteDatasetRules(datasetId, set.id);
      lastRun = res.run;
      toastSuccess($t('components.swrl.deleted'));
      dispatch('changed');
      await load();
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    }
  }

  async function runNow() {
    running = true;
    try {
      lastRun = await runDatasetRules(datasetId);
      if (lastRun.queued) toastInfo($t('components.reasoning.queued'));
      dispatch('changed');
      await load();
    } catch (e) {
      toastError((e as Error)?.message || String(e));
    } finally {
      running = false;
    }
  }

  function fmt(ts: string | null) {
    if (!ts) return '';
    const d = new Date(ts);
    return Number.isNaN(d.getTime()) ? ts : d.toLocaleString();
  }
</script>

<div class="card rz-card" id="rules">
  <div class="rz-head">
    <div class="rz-head-left"><ScrollText size={15} /><h3>{$t('components.swrl.storedHeading')}</h3></div>
    {#if writable}
      <div class="rz-row">
        <button class="btn btn-sm btn-ghost" on:click={runNow} disabled={running}>
          {#if running}<Loader2 size={13} class="animate-spin" />{:else}<Play size={13} />{/if}
          {$t('components.swrl.runNow')}
        </button>
        <button class="btn btn-sm" on:click={openNew} disabled={editing !== null}><Plus size={13} /> {$t('components.swrl.addRules')}</button>
      </div>
    {/if}
  </div>
  <p class="rz-intro">{$t('components.swrl.storedIntro')}</p>

  {#if loading && !data}
    <p class="rz-muted"><Loader2 size={12} class="animate-spin" /> {$t('system.loading')}</p>
  {:else if unavailable}
    <p class="rz-msg rz-msg-warn">{$t('components.swrl.unavailable')}</p>
  {:else if loadError}
    <p class="rz-msg rz-msg-bad">{loadError}</p>
  {:else if data}
    {#if editing !== null}
      <div class="rz-set" data-testid="rule-form">
        <div class="rz-field">
          <label for="rules-label">{$t('components.swrl.label')}</label>
          <input id="rules-label" bind:value={label} placeholder={$t('components.swrl.labelPlaceholder')} disabled={saving} />
        </div>
        <SwrlRuleEditor bind:format bind:rules bind:prefixText bind:rdfFormat bind:baseIri disabled={saving} idPrefix="stored" />
        {#if formError}<p class="rz-msg rz-msg-bad" role="alert">{formError}</p>{/if}
        <div class="rz-row">
          <button class="btn btn-sm" on:click={save} disabled={saving || !rules.trim()}>
            {#if saving}<Loader2 size={13} class="animate-spin" />{/if}
            {editing ? $t('components.swrl.saveChanges') : $t('components.swrl.store')}
          </button>
          <button class="btn btn-sm btn-ghost" on:click={() => (editing = null)} disabled={saving}>{$t('system.cancel')}</button>
          <span class="rz-muted">{$t('components.swrl.storeHint')}</span>
        </div>
      </div>
    {/if}

    {#if lastRun}
      {#if lastRun.error}<p class="rz-msg rz-msg-bad" role="alert">{lastRun.error}</p>{/if}
      {#if lastRun.queued}<p class="rz-msg rz-msg-ok">{$t('components.swrl.runQueued')}</p>{/if}
    {/if}

    {#if !data.sets.length && !data.other.length}
      <p class="rz-muted">{$t('components.swrl.noRules')}</p>
    {/if}

    {#each data.sets as set (set.id)}
      <div class="rz-set" data-testid="rule-set">
        <div class="rz-set-head">
          <div>
            <span class="rz-set-title">{set.label || $t('components.swrl.untitled')}</span>
            <span class="rz-badge">{set.format ? $t(`components.swrl.format.${set.format}`) : '—'}</span>
            <span class="rz-muted">{[$t('components.swrl.ruleCount', { values: { count: set.rules.length } }), set.updated_by, fmt(set.updated_at)].filter(Boolean).join(' · ')}</span>
          </div>
          {#if writable}
            <div class="rz-row">
              <button class="tbl-btn" on:click={() => openEdit(set)} title={$t('system.edit')} aria-label={$t('components.swrl.editSet', { values: { name: set.label || set.id } })} disabled={editing !== null}><Pencil size={13} /></button>
              <button class="tbl-btn danger" on:click={() => remove(set)} title={$t('system.delete')} aria-label={$t('components.swrl.deleteSet', { values: { name: set.label || set.id } })}><Trash2 size={13} /></button>
            </div>
          {/if}
        </div>
        {#if set.source}<pre class="rz-pre">{set.source}</pre>{/if}
        <details class="rz-details">
          <summary>{$t('components.swrl.asStored')}</summary>
          <ul class="rz-list">{#each set.rules as r}<li><code>{r.functional ?? r.iri}</code></li>{/each}</ul>
        </details>
      </div>
    {/each}

    {#if data.other.length}
      <h4 class="rz-label">{$t('components.swrl.otherGraphs')}</h4>
      {#each data.other as g}
        <div class="rz-set">
          <div class="rz-set-head"><code>{g.graph}</code><span class="rz-muted">{$t('components.swrl.ruleCount', { values: { count: g.rules.length } })}</span></div>
          {#if g.error}<p class="rz-msg rz-msg-bad">{g.error}</p>{/if}
          {#if g.rules.length}
            <ul class="rz-list">{#each g.rules as r}<li><code>{r.functional}</code></li>{/each}</ul>
          {/if}
        </div>
      {/each}
    {/if}

    {#if lastRun?.report ?? data.last_run}
      <h4 class="rz-label">{$t('components.swrl.lastRun')}</h4>
      <RulesReportView report={lastRun?.report ?? data.last_run} />
    {/if}
    <p class="rz-muted">{$t('components.swrl.inferenceGraph')} <code>{data.inference_graph}</code></p>
  {/if}
</div>
