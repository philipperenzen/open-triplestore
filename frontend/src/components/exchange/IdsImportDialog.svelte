<script lang="ts">
  // Import a buildingSMART IDS 1.0 document as SHACL: check it first (the
  // server converts without storing), see the specifications it holds and
  // what could not be carried, or — when it is refused — the audit report,
  // problem by problem; then create a shape graph from it.
  import { createEventDispatcher } from 'svelte';
  import { t } from 'svelte-i18n';
  import { Loader2, ShieldCheck, Plus, CircleAlert, CheckCircle2 } from 'lucide-svelte';
  import { importSpec } from '../../lib/api';
  import { fmtBytes } from '../../lib/exchange';
  import ExchangeDialog from './ExchangeDialog.svelte';
  import './exchange.css';

  const dispatch = createEventDispatcher<{ close: void; created: { id: string; name: string } }>();

  type Spec = { name: string; shape: string; target_classes: string[]; requirements: number };
  type Checked = { title: string; description?: string | null; shape_count: number; specifications: Spec[]; warnings: string[] };
  type Refusal = { stage: 'xml' | 'schema' | 'audit'; problems: string[]; error: string };

  let file: File | null = null;
  let xml = '';
  let busy: '' | 'check' | 'create' = '';
  let checked: Checked | null = null;
  let refusal: Refusal | null = null;
  let error = '';
  let name = '';
  let visibility = 'private';

  async function onFile(e: Event) {
    file = (e.target as HTMLInputElement).files?.[0] ?? null;
    checked = null;
    refusal = null;
    error = '';
    xml = file ? await file.text() : '';
  }

  function fail(e: unknown) {
    const err = e as Error & { status?: number; body?: any };
    if (err.status === 422 && Array.isArray(err.body?.problems)) {
      refusal = { stage: err.body.stage, problems: err.body.problems, error: err.body.error };
    } else {
      error = err.message || String(e);
    }
  }

  async function check() {
    if (!xml) return;
    busy = 'check';
    checked = null;
    refusal = null;
    error = '';
    try {
      checked = await importSpec('ids', xml);
      if (!name) name = checked?.title || '';
    } catch (e) {
      fail(e);
    } finally {
      busy = '';
    }
  }

  async function create() {
    if (!xml || !checked) return;
    busy = 'create';
    error = '';
    try {
      const res = await importSpec('ids', xml, { create: true, name: name.trim() || undefined, visibility });
      if (res?.shape_graph) dispatch('created', res.shape_graph);
    } catch (e) {
      fail(e);
    } finally {
      busy = '';
    }
  }
</script>

<ExchangeDialog title={$t('components.exchange.idsImport.title')} busy={!!busy} on:close={() => dispatch('close')}>
  <p class="xc-intro">{$t('components.exchange.idsImport.intro')}</p>
  <label class="xc-field">
    <span>{$t('components.exchange.idsImport.file')}</span>
    <input class="xc-file" type="file" accept=".ids,.xml,application/xml,text/xml" on:change={onFile} disabled={!!busy} data-testid="ids-file" />
    {#if file}<small>{file.name} · {fmtBytes(file.size)}</small>{/if}
  </label>

  {#if error}<p class="xc-error" role="alert">{error}</p>{/if}

  {#if refusal}
    <div class="xc-callout xc-bad" role="alert" data-testid="ids-audit-report">
      <CircleAlert size={15} />
      <div>
        <strong>{$t(`components.exchange.idsImport.stage.${refusal.stage}`)}</strong>
        <p class="xc-muted">{$t('components.exchange.idsImport.problemCount', { values: { n: refusal.problems.length } })}</p>
        <ol class="xc-list">{#each refusal.problems as p}<li>{p}</li>{/each}</ol>
      </div>
    </div>
  {/if}

  {#if checked}
    <div class="xc-callout xc-ok" role="status" data-testid="ids-checked">
      <CheckCircle2 size={15} />
      <div>
        <strong>{$t('components.exchange.idsImport.passed')}</strong>
        <p class="xc-muted">
          {$t('components.exchange.idsImport.summary', { values: { title: checked.title, specs: checked.specifications.length, shapes: checked.shape_count } })}
        </p>
      </div>
    </div>
    {#if checked.specifications.length}
      <div class="xc-table-wrap">
        <table class="xc-table">
          <thead>
            <tr>
              <th>{$t('components.exchange.ids.specification')}</th>
              <th>{$t('components.exchange.idsImport.targets')}</th>
              <th>{$t('components.exchange.idsImport.requirements')}</th>
            </tr>
          </thead>
          <tbody>
            {#each checked.specifications as s (s.shape)}
              <tr><td>{s.name}</td><td>{s.target_classes.length}</td><td>{s.requirements}</td></tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    {#if checked.warnings.length}
      <div class="xc-callout xc-warn" role="note">
        <CircleAlert size={15} />
        <div>
          <strong>{$t('components.exchange.idsImport.warnings')}</strong>
          <ul class="xc-list">{#each checked.warnings as w}<li>{w}</li>{/each}</ul>
        </div>
      </div>
    {/if}
    <div class="xc-grid">
      <label class="xc-field">
        <span>{$t('components.exchange.idsImport.name')}</span>
        <input type="text" bind:value={name} disabled={!!busy} />
      </label>
      <label class="xc-field">
        <span>{$t('components.exchange.idsImport.visibility')}</span>
        <select bind:value={visibility} disabled={!!busy}>
          <option value="private">{$t('components.exchange.idsImport.private')}</option>
          <option value="public">{$t('components.exchange.idsImport.public')}</option>
        </select>
      </label>
    </div>
  {/if}

  <svelte:fragment slot="footer">
    <button class="btn btn-ghost btn-sm" on:click={() => dispatch('close')} disabled={!!busy}>{$t('system.cancel')}</button>
    <button class="btn btn-sm btn-ghost" on:click={check} disabled={!xml || !!busy}>
      {#if busy === 'check'}<Loader2 size={13} class="animate-spin" />{:else}<ShieldCheck size={13} />{/if}
      {$t('components.exchange.idsImport.check')}
    </button>
    <button class="btn btn-sm" on:click={create} disabled={!checked || !!busy}>
      {#if busy === 'create'}<Loader2 size={13} class="animate-spin" />{:else}<Plus size={13} />{/if}
      {$t('components.exchange.idsImport.create')}
    </button>
  </svelte:fragment>
</ExchangeDialog>
