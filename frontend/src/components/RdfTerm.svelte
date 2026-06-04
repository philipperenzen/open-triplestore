<script>
  import { t as i18nT } from 'svelte-i18n';
  import { shortenIRI } from '../lib/rdf-utils.js';
  import { navigate } from '../lib/router/index.js';
  import { isDark } from '../lib/theme.js';
  import { Check, Copy } from 'lucide-svelte';

  /**
   * @typedef {Object} RdfTermLike
   * @property {string} [type]
   * @property {any} [value]
   * @property {string} [language]
   * @property {string} [datatype]
   */

  /** @type {RdfTermLike | null} */
  export let term = null;
  export let navigable = true;
  /** Optional graph scope, forwarded when navigating (esp. for graph-local bnodes). */
  export let graph = '';
  /** True when this term is rendered inside a quoted triple — suppresses the copy
   *  button so nested terms stay compact. */
  export let nested = false;

  let copied = false;

  // Theme-aware term colours. The light defaults are kept close to the originals;
  // the dark variants are brightened so URIs/literals/bnodes stay legible on a
  // dark surface (the old #4a90d9 / #2e8b57 / #888 were too dim).
  // Map a 2-letter ISO country code to its flag emoji (regional-indicator pair).
  function flagEmoji(cc) {
    if (!cc || !/^[a-zA-Z]{2}$/.test(cc)) return '';
    const up = cc.toUpperCase();
    return String.fromCodePoint(up.charCodeAt(0) + 0x1f1a5, up.charCodeAt(1) + 0x1f1a5);
  }

  // Default country for a primary language subtag, used when the BCP-47 tag has
  // no explicit region (e.g. "nl" → NL flag). A region subtag in the tag itself
  // (e.g. "en-GB", "nl-BE") always wins over this fallback.
  const LANG_TO_COUNTRY = {
    en: 'GB', nl: 'NL', de: 'DE', fr: 'FR', es: 'ES', it: 'IT', pt: 'PT',
    ru: 'RU', zh: 'CN', ja: 'JP', ko: 'KR', ar: 'SA', hi: 'IN', pl: 'PL',
    sv: 'SE', no: 'NO', nb: 'NO', nn: 'NO', da: 'DK', fi: 'FI', cs: 'CZ',
    sk: 'SK', hu: 'HU', ro: 'RO', el: 'GR', tr: 'TR', uk: 'UA', he: 'IL',
    th: 'TH', vi: 'VN', id: 'ID', ms: 'MY', ga: 'IE', cy: 'GB', ca: 'ES',
    eu: 'ES', gl: 'ES', hr: 'HR', sr: 'RS', sl: 'SI', bg: 'BG', et: 'EE',
    lv: 'LV', lt: 'LT', is: 'IS', fa: 'IR', ur: 'PK', bn: 'BD', ta: 'IN',
    af: 'ZA', sw: 'KE', fy: 'NL',
  };

  function langToFlag(lang) {
    if (!lang) return '';
    const parts = lang.split('-');
    // An explicit 2-letter region subtag (anything past the primary) wins.
    for (let i = parts.length - 1; i >= 1; i--) {
      if (/^[a-zA-Z]{2}$/.test(parts[i])) return flagEmoji(parts[i]);
    }
    return flagEmoji(LANG_TO_COUNTRY[parts[0].toLowerCase()] || '');
  }

  function colorFor(t, dark) {
    if (!t) return dark ? '#94a3b8' : '#9ca3af';
    switch (t.type) {
      case 'uri':
      case 'iri':     return dark ? '#7db4f0' : '#2f6fb3';
      case 'literal': return dark ? '#5fd39a' : '#1f7a44';
      case 'bnode':   return dark ? '#c4b5fd' : '#7c5fce';
      case 'triple':  return dark ? '#cbd5e1' : '#475569';
      default:        return dark ? '#cbd5e1' : '#475569';
    }
  }

  // Parse a stringified quoted triple "<<<s> <p> <o>>>" (the form the
  // /api/browse/triples endpoint returns for RDF-star terms) into three terms.
  function parseQuoted(value) {
    if (typeof value !== 'string') return null;
    const v = value.trim();
    if (!v.startsWith('<<') || !v.endsWith('>>')) return null;
    const inner = v.slice(2, -2).trim();
    const terms = [];
    let i = 0;
    while (i < inner.length && terms.length < 3) {
      const c = inner[i];
      if (c === ' ' || c === '\t') { i++; continue; }
      if (c === '<') {
        const end = inner.indexOf('>', i);
        if (end === -1) break;
        terms.push({ type: 'uri', value: inner.slice(i + 1, end) });
        i = end + 1;
      } else if (c === '_' && inner[i + 1] === ':') {
        let j = i + 2;
        while (j < inner.length && /\S/.test(inner[j])) j++;
        terms.push({ type: 'bnode', value: inner.slice(i + 2, j) });
        i = j;
      } else if (c === '"') {
        let j = i + 1;
        while (j < inner.length) {
          if (inner[j] === '\\') { j += 2; continue; }
          if (inner[j] === '"') break;
          j++;
        }
        const lex = inner.slice(i + 1, j).replace(/\\"/g, '"').replace(/\\\\/g, '\\');
        j++;
        let lang = '';
        let dt = '';
        if (inner[j] === '@') {
          let s = j + 1, e = s;
          while (e < inner.length && /[a-zA-Z0-9-]/.test(inner[e])) e++;
          lang = inner.slice(s, e); j = e;
        } else if (inner[j] === '^' && inner[j + 1] === '^') {
          j += 2;
          if (inner[j] === '<') {
            const e = inner.indexOf('>', j);
            if (e !== -1) { dt = inner.slice(j + 1, e); j = e + 1; }
          }
        }
        const lit = { type: 'literal', value: lex };
        if (lang) lit.language = lang;
        if (dt) lit.datatype = dt;
        terms.push(lit);
        i = j;
      } else {
        i++;
      }
    }
    if (terms.length === 3) {
      return { subject: terms[0], predicate: terms[1], object: terms[2] };
    }
    return null;
  }

  // Quoted-triple structure (RDF-star), from either the structured SPARQL-JSON
  // shape { type:'triple', value:{subject,predicate,object} } or the stringified
  // "<<<…>>>" form the browse endpoint emits as type 'unknown'.
  $: quoted = (() => {
    if (!term) return null;
    if (term.type === 'triple' && term.value && typeof term.value === 'object') {
      const v = term.value;
      if (v.subject && v.predicate && v.object) {
        return { subject: v.subject, predicate: v.predicate, object: v.object };
      }
    }
    if (typeof term?.value === 'string') {
      const t = term.value.trim();
      if (t.startsWith('<<') && t.endsWith('>>')) return parseQuoted(t);
    }
    return null;
  })();

  function handleClick(e) {
    if (!navigable || !isClickable) return;
    if (e) e.stopPropagation();
    const t = term?.type;
    if (t === 'uri' || t === 'iri') {
      navigate(`/resource?iri=${encodeURIComponent(term.value)}${graph ? `&graph=${encodeURIComponent(graph)}` : ''}`);
    } else if (t === 'bnode') {
      // Blank-node identity is graph-local; carry the graph scope so the resource
      // view can resolve it within the right graph.
      navigate(`/resource?iri=${encodeURIComponent(`_:${term.value}`)}${graph ? `&graph=${encodeURIComponent(graph)}` : ''}`);
    }
  }

  function copyValue(e) {
    e.stopPropagation();
    const val = typeof term?.value === 'string' ? term.value : (display || '');
    navigator.clipboard.writeText(val).then(() => {
      copied = true;
      setTimeout(() => (copied = false), 1500);
    }).catch(() => {});
  }

  $: display = (() => {
    if (!term) return '—';
    if (term.type === 'uri' || term.type === 'iri') return shortenIRI(term.value);
    if (term.type === 'literal') return `"${term.value}"`;
    if (term.type === 'bnode') return `_:${term.value}`;
    return term.value || '—';
  })();

  // Lang/datatype badge shown beside a literal value, like the predicate chip.
  // xsd:string is the implicit default for plain literals, so it's suppressed.
  $: literalMeta = (() => {
    if (!term || term.type !== 'literal') return null;
    if (term.language) {
      return { kind: 'lang', label: term.language.toUpperCase(), flag: langToFlag(term.language), title: $i18nT('components.rdfTerm.languageTitle', { values: { lang: term.language } }) };
    }
    if (term.datatype && term.datatype !== 'http://www.w3.org/2001/XMLSchema#string') {
      return { kind: 'datatype', label: shortenIRI(term.datatype), flag: '', title: $i18nT('components.rdfTerm.datatypeTitle', { values: { datatype: term.datatype } }) };
    }
    return null;
  })();

  $: color = colorFor(term, $isDark);
  $: isClickable = navigable && (term?.type === 'uri' || term?.type === 'iri' || term?.type === 'bnode');
  $: tooltip = typeof term?.value === 'string' ? term?.value : display;
</script>

{#if quoted}
  <span class="term-wrap">
    <span class="rdf-term star" style="color: {color}" title={tooltip}>
      <span class="star-br">«</span><svelte:self
        term={quoted.subject} {navigable} {graph} nested /><span class="star-sep"></span><svelte:self
        term={quoted.predicate} {navigable} {graph} nested /><span class="star-sep"></span><svelte:self
        term={quoted.object} {navigable} {graph} nested /><span class="star-br">»</span>
    </span>
    {#if !nested}
      <button
        class="copy-btn"
        class:copied
        title={copied ? $i18nT('system.copied') : $i18nT('components.rdfTerm.copyValue')}
        on:click={copyValue}
        tabindex="-1"
        aria-label={$i18nT('system.copy')}
      >{#if copied}<Check size={11} />{:else}<Copy size={11} />{/if}</button>
    {/if}
  </span>
{:else if term}
  <span class="term-wrap">
    {#if isClickable}
      <span
        class="rdf-term type-{term.type} clickable"
        style="color: {color}"
        title={tooltip}
        on:click={handleClick}
        on:keypress={(e) => e.key === 'Enter' && handleClick(e)}
        role="link"
        tabindex="0"
      >{display}</span>
    {:else}
      <span
        class="rdf-term type-{term.type}"
        style="color: {color}"
        title={tooltip}
      >{display}</span>
    {/if}
    {#if literalMeta}
      <span
        class="lit-meta"
        class:lit-lang={literalMeta.kind === 'lang'}
        class:lit-dt={literalMeta.kind === 'datatype'}
        title={literalMeta.title}
      >{#if literalMeta.flag}<span class="lit-flag">{literalMeta.flag}</span>{/if}{literalMeta.label}</span>
    {/if}
    {#if !nested}
      <button
        class="copy-btn"
        class:copied
        title={copied ? $i18nT('system.copied') : $i18nT('components.rdfTerm.copyValue')}
        on:click={copyValue}
        tabindex="-1"
        aria-label={$i18nT('system.copy')}
      >{#if copied}<Check size={11} />{:else}<Copy size={11} />{/if}</button>
    {/if}
  </span>
{:else}
  <span class="rdf-term unbound">—</span>
{/if}

<style>
  .term-wrap {
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
  }

  .term-wrap:not(:hover) .copy-btn { opacity: 0; pointer-events: none; }
  .term-wrap:hover .copy-btn { opacity: 1; }

  .rdf-term {
    font-size: 0.875rem;
    word-break: break-word;
  }

  .clickable {
    cursor: pointer;
    text-decoration: underline;
    text-decoration-color: transparent;
    transition: text-decoration-color 0.15s;
  }
  .clickable:hover { text-decoration-color: currentColor; }

  .unbound { color: #9ca3af; }
  :global(html.dark) .unbound { color: #64748b; }

  .type-literal {
    font-family: 'SF Mono', 'Fira Code', monospace;
    font-size: 0.82rem;
  }

  .type-bnode { font-style: italic; }

  /* Language / datatype badge beside a literal — mirrors the predicate chip. */
  .lit-meta {
    font-size: 0.7rem;
    font-weight: 600;
    padding: 1px 5px;
    border-radius: 4px;
    white-space: nowrap;
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    line-height: 1.4;
    flex-shrink: 0;
  }
  .lit-lang {
    color: #1e40af;
    background: #dbeafe;
    text-transform: uppercase;
    letter-spacing: 0.02em;
  }
  .lit-dt {
    color: #92400e;
    background: #fef3c7;
    font-family: 'SF Mono', 'Fira Code', monospace;
  }
  .lit-flag { font-size: 0.85rem; line-height: 1; }
  :global(html.dark) .lit-lang { color: #93c5fd; background: rgba(59, 130, 246, 0.18); }
  :global(html.dark) .lit-dt { color: #fcd34d; background: rgba(245, 158, 11, 0.18); }

  /* Quoted triple (RDF-star): a bracketed, tinted inline group so the nested
     subject/predicate/object read as one term instead of a wall of raw IRIs. */
  .star {
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.15rem 0.3rem;
    padding: 0.05rem 0.3rem;
    border: 1px solid rgba(100, 116, 139, 0.35);
    border-radius: 6px;
    background: rgba(100, 116, 139, 0.06);
  }
  :global(html.dark) .star {
    border-color: rgba(148, 163, 184, 0.35);
    background: rgba(148, 163, 184, 0.1);
  }
  .star-br {
    font-weight: 700;
    opacity: 0.7;
  }
  .star-sep { display: inline-block; width: 0; }

  .copy-btn {
    background: none;
    border: none;
    cursor: pointer;
    color: #9ca3af;
    font-size: 0.72rem;
    padding: 0 2px;
    line-height: 1;
    transition: color 0.1s, opacity 0.1s;
    flex-shrink: 0;
  }
  .copy-btn:hover { color: #4a90d9; }
  .copy-btn.copied { color: #4caf50; }
  :global(html.dark) .copy-btn { color: #64748b; }
  :global(html.dark) .copy-btn:hover { color: #7db4f0; }
  :global(html.dark) .copy-btn.copied { color: #5fd39a; }
</style>
