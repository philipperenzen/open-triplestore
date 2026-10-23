<script>
  // Readable summary of SHACL validation findings in a chat answer: whether the
  // data conforms, counts per severity, the paths / shapes / focus nodes with
  // the most results, and the worst few results with their messages. Findings
  // come from chatViz (a report queried as triples, a SELECT over results, or
  // the platform's JSON report) — the counts are of the rows returned, which
  // the note says when the query was capped.
  import { t } from 'svelte-i18n';
  import { summarizeShacl } from '../../lib/chatViz';
  import { shortenIRI } from '../../lib/rdf-utils.js';
  import { formatShaclPath } from '../../lib/validationReport';
  import { navigate } from '../../lib/router/index.js';
  import { ShieldCheck, ShieldAlert } from 'lucide-svelte';

  /** @type {import('../../lib/chatViz').ShaclFindings} */
  export let findings;
  /** True when the underlying query hit its row cap. */
  export let truncated = false;

  $: summary = summarizeShacl(findings || { conforms: null, issues: [] });
  $: ok = summary.conforms === true || (summary.conforms === null && summary.total === 0);

  const isIri = (s) => /^(https?:\/\/|urn:)/i.test(String(s || ''));
  const short = (s) => (isIri(s) ? shortenIRI(s) : String(s || ''));
  const sevClass = (s) => (s === 'Violation' ? 'violation' : s === 'Warning' ? 'warning' : 'info');
  function sevLabel(s) {
    const key = { Violation: 'violation', Warning: 'warning', Info: 'info' }[s];
    return key ? $t(`components.chat.shacl.${key}`) : s;
  }
  function open(iri) {
    if (isIri(iri)) navigate(`/resource?iri=${encodeURIComponent(iri)}`);
  }
</script>

<div class="shacl" class:ok data-testid="chat-shacl">
  <div class="verdict">
    {#if ok}<ShieldCheck size={16} />{:else}<ShieldAlert size={16} />{/if}
    <strong>
      {#if summary.conforms === true}{$t('components.chat.shacl.conforms')}
      {:else if summary.conforms === false}{$t('components.chat.shacl.doesNotConform')}
      {:else}{$t('components.chat.shacl.results', { values: { count: summary.total } })}{/if}
    </strong>
    {#if summary.conforms !== null && summary.total}
      <span class="sub">· {$t('components.chat.shacl.results', { values: { count: summary.total } })}{#if truncated}+{/if}</span>
    {/if}
  </div>

  {#if summary.bySeverity.length}
    <ul class="sev-row">
      {#each summary.bySeverity as s}
        <li class="sev {sevClass(s.key)}"><span class="n">{s.count}</span> {sevLabel(s.key)}</li>
      {/each}
    </ul>
  {/if}

  {#if summary.total}
    <div class="groups">
      {#each [['byPath', summary.byPath], ['byShape', summary.byShape], ['byFocus', summary.byFocus]] as [key, rows]}
        {#if rows.length}
          <div class="group">
            <p class="group-title">{$t(`components.chat.shacl.${key}`)}</p>
            <ul>
              {#each rows as r}
                <li>
                  {#if key === 'byFocus' && isIri(r.key)}
                    <button class="link" title={r.key} on:click={() => open(r.key)}>{short(r.key)}</button>
                  {:else}
                    <span class="key" title={r.key}>{key === 'byPath' ? formatShaclPath(r.key) : short(r.key)}</span>
                  {/if}
                  <span class="count">{r.count}</span>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      {/each}
    </div>

    <p class="group-title">{$t('components.chat.shacl.samples')}</p>
    <ul class="samples">
      {#each summary.samples as s}
        <li>
          {#if s.severity}<span class="sev-dot {sevClass(s.severity)}" title={sevLabel(s.severity)}></span>{/if}
          <span class="msg">{s.message || $t('components.chat.shacl.noMessage')}</span>
          <span class="where" title={s.focus}>{short(s.focus)}{#if s.path} · {formatShaclPath(s.path)}{/if}</span>
        </li>
      {/each}
    </ul>
  {/if}
  {#if truncated}<p class="note">{$t('components.chat.shacl.truncated')}</p>{/if}
</div>

<style>
  .shacl {
    margin: 0 0 0.55rem; padding: 0.6rem 0.75rem; border-radius: 10px;
    border: 1px solid #fecaca; background: #fff8f8;
  }
  .shacl.ok { border-color: #bbf7d0; background: #f0fdf4; }
  .verdict { display: flex; align-items: center; gap: 0.4rem; font-size: 0.85rem; color: #b91c1c; }
  .shacl.ok .verdict { color: #15803d; }
  .sub { font-size: 0.76rem; color: var(--ink-500); font-weight: 400; }
  .sev-row { list-style: none; display: flex; flex-wrap: wrap; gap: 0.35rem; margin: 0.45rem 0 0; padding: 0; }
  .sev { font-size: 0.74rem; padding: 0.15rem 0.55rem; border-radius: 999px; border: 1px solid; }
  .sev .n { font-weight: 700; }
  .sev.violation { color: #b91c1c; border-color: #fca5a5; background: #fee2e2; }
  .sev.warning { color: #b45309; border-color: #fcd34d; background: #fef3c7; }
  .sev.info { color: #1d4ed8; border-color: #93c5fd; background: #dbeafe; }
  .groups { display: grid; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); gap: 0.5rem 1rem; margin-top: 0.5rem; }
  .group ul, .samples { list-style: none; margin: 0; padding: 0; }
  .group li { display: flex; justify-content: space-between; gap: 0.5rem; font-size: 0.75rem; color: var(--ink-600); min-width: 0; }
  .group-title { margin: 0.5rem 0 0.2rem; font-size: 0.68rem; font-weight: 700; letter-spacing: 0.4px; text-transform: uppercase; color: var(--ink-400); }
  .key, .link { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .count { color: var(--ink-400); font-variant-numeric: tabular-nums; }
  .link {
    background: none; border: none; cursor: pointer; padding: 0; font-size: 0.75rem; text-align: left;
    color: #4f46e5; text-decoration: underline; text-decoration-color: rgba(79,70,229,0.35);
  }
  .samples li { display: flex; align-items: baseline; gap: 0.4rem; flex-wrap: wrap; font-size: 0.76rem; padding: 0.15rem 0; }
  .sev-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; display: inline-block; }
  .sev-dot.violation { background: #dc2626; }
  .sev-dot.warning { background: #f59e0b; }
  .sev-dot.info { background: #3b82f6; }
  .msg { color: var(--ink-700); }
  .where { color: var(--ink-400); font-size: 0.7rem; overflow: hidden; text-overflow: ellipsis; }
  .note { margin: 0.4rem 0 0; font-size: 0.7rem; color: var(--ink-400); font-style: italic; }
  :global(:is([data-theme="dark"], .dark)) .shacl { background: rgba(220,38,38,0.1); border-color: rgba(220,38,38,0.35); }
  :global(:is([data-theme="dark"], .dark)) .shacl.ok { background: rgba(16,185,129,0.1); border-color: rgba(16,185,129,0.3); }
  :global(:is([data-theme="dark"], .dark)) .verdict { color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .shacl.ok .verdict { color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .link { color: #a5b4fc; }
</style>
