<script>
  // No-code SHACL builder. Renders the parsed shapes model as editable cards
  // with model-driven pickers (target/path datalists fed by the dataset's real
  // classes + properties) and typed constraint controls. Every edit mutates the
  // structured model and re-serialises to Turtle, so the source view stays in
  // sync. SHACL the model doesn't edit (SPARQL constraints, shared structures,
  // …) is retained verbatim (`extraQuads`) and flagged via `hasUnsupported`,
  // so the cards stay editable for the modelled constructs while everything
  // else round-trips untouched.
  import {
    parseShapesGraph,
    serializeShapesGraph,
    makeCurie,
    renderPath,
    shortLocal,
    SEVERITY_VIOLATION,
    SEVERITY_WARNING,
    SEVERITY_INFO,
    SH,
  } from '../lib/shaclModel.ts';
  import {
    Plus,
    Trash2,
    Target,
    Crosshair,
    Lock,
    AlertTriangle,
    FileCode,
    SlidersHorizontal,
    Box,
  } from 'lucide-svelte';
  import Select from './Select.svelte';
  import Combobox from './Combobox.svelte';
  import { t as i18nT } from 'svelte-i18n';

  /** Turtle source (two-way: parent binds shapesContent here). */
  export let turtle = '';
  /** @type {{ classes: Array<{iri: string, label?: string, count?: number}>, properties: Array<{iri: string, label?: string, count?: number}> } | null} */
  export let modelContext = null;
  /** Called with regenerated Turtle whenever the model is edited. */
  export let onChange = (_ttl) => {};
  export let loading = false;

  const XSD = 'http://www.w3.org/2001/XMLSchema#';
  const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
  const DATATYPES = [
    'xsd:string', 'xsd:boolean', 'xsd:integer', 'xsd:decimal', 'xsd:double',
    'xsd:float', 'xsd:date', 'xsd:dateTime', 'xsd:time', 'xsd:gYear',
    'xsd:anyURI', 'rdf:langString',
  ];
  $: NODE_KINDS = [
    { v: SH + 'IRI', label: 'IRI' },
    { v: SH + 'BlankNode', label: $i18nT('components.shapeBuilder.nodeKind.blankNode') },
    { v: SH + 'Literal', label: $i18nT('components.shapeBuilder.nodeKind.literal') },
    { v: SH + 'BlankNodeOrIRI', label: $i18nT('components.shapeBuilder.nodeKind.blankNodeOrIri') },
    { v: SH + 'IRIOrLiteral', label: $i18nT('components.shapeBuilder.nodeKind.iriOrLiteral') },
    { v: SH + 'BlankNodeOrLiteral', label: $i18nT('components.shapeBuilder.nodeKind.blankNodeOrLiteral') },
  ];

  let model = { prefixes: {}, shapes: [], canRoundTrip: true };
  let _lastTurtle = null;
  let _seq = 0;

  // Re-parse only when the Turtle changed outside this component.
  $: if (turtle !== _lastTurtle) {
    model = withIds(parseShapesGraph(turtle));
    _lastTurtle = turtle;
  }
  $: curie = makeCurie(model.prefixes || {});
  $: editable = model.canRoundTrip && !model.parseError;
  $: classOptions = modelContext?.classes || [];
  $: propOptions = modelContext?.properties || [];
  $: classSuggestions = classOptions.map((c) => ({ value: disp(c.iri), label: c.label || disp(c.iri), hint: c.count ? `${disp(c.iri)} · ${c.count}` : disp(c.iri) }));
  $: propSuggestions = propOptions.map((p) => ({ value: disp(p.iri), label: p.label || disp(p.iri), hint: p.count ? `${disp(p.iri)} · ${p.count}` : disp(p.iri) }));

  function withIds(g) {
    for (const s of g.shapes) {
      if (s._id == null) s._id = ++_seq;
      for (const p of s.properties) {
        if (p._id == null) p._id = ++_seq;
        if (p._vt == null) p._vt = valueTypeOf(p.c);
      }
    }
    return g;
  }

  function commit() {
    const ttl = serializeShapesGraph(model);
    _lastTurtle = ttl; // set before onChange so the parent's echo won't re-parse
    onChange(ttl);
  }
  function touch() {
    model = model;
    commit();
  }

  // ── IRI display / expansion ────────────────────────────────────────────────
  function disp(iri) {
    if (!iri) return '';
    const c = curie(iri);
    return c.startsWith('<') ? iri : c;
  }
  function expand(token) {
    if (!token) return '';
    const t = token.trim();
    if (/^(https?:|urn:|mailto:)/.test(t)) return t;
    const m = t.match(/^([A-Za-z][\w.-]*):(.*)$/);
    if (m && model.prefixes[m[1]]) return model.prefixes[m[1]] + m[2];
    return t;
  }

  // ── Value type (literal / class / iri / blank / any) ────────────────────────
  function valueTypeOf(c) {
    if (c.datatype) return 'literal';
    if (c.class) return 'class';
    if (c.nodeKind === SH + 'IRI') return 'iri';
    if (c.nodeKind === SH + 'BlankNode') return 'blank';
    return 'any';
  }
  function applyValueType(p) {
    const c = p.c;
    delete c.datatype;
    delete c.class;
    delete c.nodeKind;
    if (p._vt === 'literal') c.datatype = XSD + 'string';
    else if (p._vt === 'iri') c.nodeKind = SH + 'IRI';
    else if (p._vt === 'blank') c.nodeKind = SH + 'BlankNode';
    // 'class' leaves class empty until the picker fills it; 'any' clears all.
    touch();
  }

  // ── Field setters (delete when emptied) ─────────────────────────────────────
  function setStr(obj, key, value) {
    if (value == null || value === '') delete obj[key];
    else obj[key] = value;
    touch();
  }
  function setNum(obj, key, value) {
    if (value === '' || value == null || Number.isNaN(Number(value))) delete obj[key];
    else obj[key] = Number(value);
    touch();
  }
  function setIri(obj, key, value) {
    const iri = expand(value);
    if (!iri) delete obj[key];
    else obj[key] = iri;
    touch();
  }
  function toggleRequired(p, on) {
    if (on) p.c.minCount = 1;
    else delete p.c.minCount;
    touch();
  }
  function toggleSingle(p, on) {
    if (on) p.c.maxCount = 1;
    else delete p.c.maxCount;
    touch();
  }

  // ── Structural edits ────────────────────────────────────────────────────────
  function baseNs() {
    const std = new Set([
      SH, RDF,
      'http://www.w3.org/2000/01/rdf-schema#',
      XSD,
      'http://www.w3.org/2002/07/owl#',
      'http://www.w3.org/2004/02/skos/core#',
      'http://purl.org/dc/terms/',
    ]);
    for (const [, ns] of Object.entries(model.prefixes)) if (!std.has(ns)) return ns;
    model.prefixes = { ...model.prefixes, ex: 'http://example.org/' };
    return 'http://example.org/';
  }
  function addShape() {
    const ns = baseNs();
    let n = 1;
    let iri;
    do {
      iri = `${ns}Shape${n > 1 ? n : ''}`;
      n++;
    } while (model.shapes.some((s) => s.iri === iri));
    model.shapes = [
      ...model.shapes,
      { _id: ++_seq, iri, declared: true, targets: [{ kind: 'class', value: '' }], properties: [] },
    ];
    touch();
  }
  function deleteShape(i) {
    model.shapes.splice(i, 1);
    touch();
  }
  function addTarget(shape) {
    shape.targets = [...shape.targets, { kind: 'class', value: '' }];
    touch();
  }
  function removeTarget(shape, i) {
    shape.targets.splice(i, 1);
    touch();
  }
  function addProperty(shape) {
    shape.properties = [...shape.properties, { _id: ++_seq, _vt: 'literal', path: '', c: { datatype: XSD + 'string' } }];
    touch();
  }
  function removeProperty(shape, i) {
    shape.properties.splice(i, 1);
    touch();
  }

  let expanded = {}; // property _id → advanced section open
  function toggleAdvanced(id) {
    expanded = { ...expanded, [id]: !expanded[id] };
  }

  const STD_SEVERITIES = [SEVERITY_VIOLATION, SEVERITY_WARNING, SEVERITY_INFO];

  // Compact summary of logical operators on a shape, e.g. "or(2) not(1)".
  function logicSummary(logic) {
    const parts = [];
    for (const op of ['and', 'or', 'xone', 'not']) {
      if (logic?.[op]?.length) parts.push(`sh:${op}(${logic[op].length})`);
    }
    return parts.join(' ');
  }

  function sevClass(severity) {
    if (severity === SEVERITY_WARNING) return 'chip-sev-warning';
    if (severity === SEVERITY_INFO) return 'chip-sev-info';
    if (severity === SEVERITY_VIOLATION) return 'chip-sev-violation';
    return '';
  }

  // Read-only target chip text, e.g. "targets ex:Person".
  function targetChipText(tgt, t) {
    const v = disp(tgt.value);
    const key = {
      class: 'targetsClass',
      node: 'targetsNode',
      subjectsOf: 'targetsSubjectsOf',
      objectsOf: 'targetsObjectsOf',
    }[tgt.kind] || 'targetsClass';
    return t(`components.shapeBuilder.${key}`, { values: { value: v } });
  }
  function targetTitle(tgt) {
    const prop = { class: 'sh:targetClass', node: 'sh:targetNode', subjectsOf: 'sh:targetSubjectsOf', objectsOf: 'sh:targetObjectsOf' }[tgt.kind];
    return `${prop} <${tgt.value}>`;
  }

  // Explicit cardinality summary for the property header ("0..1", "2..*").
  // Omitted when the "required" chip already says it all (1..*).
  function cardText(c) {
    if (c.minCount == null && c.maxCount == null) return '';
    if (c.maxCount == null && (c.minCount ?? 0) <= 1) return '';
    return `${c.minCount ?? 0}..${c.maxCount ?? '*'}`;
  }
  function cardTitle(c) {
    const parts = [];
    if (c.minCount != null) parts.push(`sh:minCount ${c.minCount}`);
    if (c.maxCount != null) parts.push(`sh:maxCount ${c.maxCount}`);
    return parts.join(' · ');
  }

  /**
   * Labelled constraint chips for a property row. In editable mode the
   * value-type + cardinality constraints have dedicated controls, so they are
   * skipped (`includeType=false`); read-only rows show everything. Each chip
   * carries the raw SHACL in its tooltip.
   */
  function constraintChips(p, includeType, t) {
    const L = (k) => t(`components.shapeBuilder.${k}`);
    const c = p.c || {};
    const out = [];
    if (includeType) {
      if (c.datatype) out.push({ cls: 'chip-type', label: L('chipDatatype'), value: disp(c.datatype), title: `sh:datatype <${c.datatype}>` });
      if (c.class) out.push({ cls: 'chip-type', label: L('chipClass'), value: disp(c.class), title: `sh:class <${c.class}>` });
      if (c.nodeKind) out.push({ cls: 'chip-type', label: L('chipKind'), value: shortLocal(c.nodeKind), title: `sh:nodeKind <${c.nodeKind}>` });
    }
    if (c.minInclusive != null) out.push({ cls: 'chip-range', label: L('chipMin'), value: String(c.minInclusive), title: `sh:minInclusive ${c.minInclusive}` });
    if (c.maxInclusive != null) out.push({ cls: 'chip-range', label: L('chipMax'), value: String(c.maxInclusive), title: `sh:maxInclusive ${c.maxInclusive}` });
    if (c.minExclusive != null) out.push({ cls: 'chip-range', label: L('chipMin'), value: `> ${c.minExclusive}`, title: `sh:minExclusive ${c.minExclusive}` });
    if (c.maxExclusive != null) out.push({ cls: 'chip-range', label: L('chipMax'), value: `< ${c.maxExclusive}`, title: `sh:maxExclusive ${c.maxExclusive}` });
    if (c.minLength != null) out.push({ cls: 'chip-range', label: L('chipLength'), value: `≥ ${c.minLength}`, title: `sh:minLength ${c.minLength}` });
    if (c.maxLength != null) out.push({ cls: 'chip-range', label: L('chipLength'), value: `≤ ${c.maxLength}`, title: `sh:maxLength ${c.maxLength}` });
    if (c.pattern) out.push({ cls: 'chip-str', label: L('chipPattern'), value: trunc(c.pattern + (c.flags ? ` /${c.flags}` : ''), 28), title: `sh:pattern "${c.pattern}"${c.flags ? ` · sh:flags "${c.flags}"` : ''}` });
    if (c.in && c.in.length) {
      const vals = c.in.map((it) => (it.type === 'iri' ? disp(it.value) : `"${it.value}"`));
      out.push({ cls: 'chip-str', label: L('chipIn'), value: trunc(vals.join(', '), 36), title: `sh:in (${vals.join(' ')})` });
    }
    if (c.hasValue) out.push({ cls: 'chip-str', label: L('chipEquals'), value: c.hasValue.type === 'iri' ? disp(c.hasValue.value) : `"${c.hasValue.value}"`, title: 'sh:hasValue' });
    if (c.languageIn && c.languageIn.length) out.push({ cls: 'chip-str', label: L('chipLang'), value: c.languageIn.join(' '), title: `sh:languageIn (${c.languageIn.join(' ')})` });
    if (c.uniqueLang) out.push({ cls: 'chip-str', label: '', value: L('chipUniqueLang'), title: 'sh:uniqueLang true' });
    if (c.node) out.push({ cls: 'chip-shape', label: L('chipShape'), value: disp(c.node), title: `sh:node <${c.node}>` });
    if (p.logic) {
      for (const op of ['and', 'or', 'xone', 'not']) {
        const n = p.logic[op]?.length;
        if (n) out.push({ cls: 'chip-shape', label: '', value: `sh:${op}(${n})`, title: `sh:${op} — ${n}` });
      }
    }
    if (p.qualified) out.push({ cls: 'chip-shape', label: '', value: 'qualified', title: 'sh:qualifiedValueShape' });
    if (p.severity) out.push({ cls: `chip-sev ${sevClass(p.severity)}`, label: L('chipSeverity'), value: shortLocal(p.severity), title: `sh:severity <${p.severity}>` });
    return out;
  }
  function trunc(s, n) {
    return s.length > n ? s.slice(0, n - 1) + '…' : s;
  }

  function dtValue(iri) {
    // Normalize the common datatype namespaces even when their prefix isn't
    // declared, so the picker matches an option instead of showing blank.
    let c = disp(iri);
    if (iri && iri.startsWith(XSD)) c = 'xsd:' + iri.slice(XSD.length);
    else if (iri && iri.startsWith(RDF)) c = 'rdf:' + iri.slice(RDF.length);
    return DATATYPES.includes(c) ? c : c || 'xsd:string';
  }

  // Parse the free-text enum / language inputs into model values.
  function setEnum(p, text) {
    const toks = (text || '').match(/"[^"]*"(@[\w-]+)?|\S+/g) || [];
    const vals = toks.map((tok) => {
      const lit = tok.match(/^"([^"]*)"(?:@([\w-]+))?$/);
      if (lit) return { type: 'literal', value: lit[1], ...(lit[2] ? { lang: lit[2] } : {}) };
      return { type: 'iri', value: expand(tok) };
    });
    if (vals.length) p.c.in = vals;
    else delete p.c.in;
    touch();
  }
  function setLangs(p, text) {
    const langs = (text || '').split(/[\s,]+/).filter(Boolean);
    if (langs.length) p.c.languageIn = langs;
    else delete p.c.languageIn;
    touch();
  }
</script>

<div class="builder">
  {#if loading}
    <div class="state"><FileCode size={20} /> {$i18nT('system.loading')}</div>
  {:else if model.parseError}
    <div class="state state-warn">
      <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
      <AlertTriangle size={18} /> {@html $i18nT('components.shapeBuilder.parseError')}
    </div>
  {:else}
    {#if model.hasUnsupported}
      <div class="banner">
        <Lock size={14} />
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <span>{@html $i18nT('components.shapeBuilder.preservedBanner')}</span>
      </div>
    {/if}

    {#if model.shapes.length === 0}
      <div class="state empty">
        <FileCode size={30} strokeWidth={1.2} />
        <h4>{$i18nT('components.shapeBuilder.emptyTitle')}</h4>
        <p>{$i18nT('components.shapeBuilder.emptyHint')}</p>
        {#if editable}<button class="btn" on:click={addShape}><Plus size={14} /> {$i18nT('components.shapeBuilder.addNodeShape')}</button>{/if}
      </div>
    {/if}

    {#each model.shapes as shape, si (shape._id)}
      <article class="shape" class:ro={!editable}>
        <header class="shape-head">
          <div class="shape-title-row">
            <span class="kind-tag" title={$i18nT('components.shapeBuilder.nodeShapeBadge')}>
              <Box size={11} /> {$i18nT('components.shapeBuilder.nodeShapeBadge')}
            </span>
            {#if editable}
              <input
                class="iri-input name-input"
                value={disp(shape.iri)}
                on:change={(e) => { shape.iri = expand(e.currentTarget.value); touch(); }}
                title={$i18nT('components.shapeBuilder.shapeIriTitle')}
              />
            {:else}
              <span class="shape-name" title={shape.iri}>{shape.name || disp(shape.iri)}</span>
            {/if}
            {#if shape.name && editable}
              <span class="shape-alias" title="sh:name">{shape.name}</span>
            {/if}
            <span class="spacer"></span>
            {#if shape.severity}
              <span class="chip chip-sev {sevClass(shape.severity)}" title={`sh:severity <${shape.severity}>`}>
                {$i18nT('components.shapeBuilder.chipSeverity')} {shortLocal(shape.severity)}
              </span>
            {/if}
            {#if !editable && shape.closed}
              <span class="chip chip-closed" title={$i18nT('components.shapeBuilder.closedTitle')}><Lock size={10} /> {$i18nT('components.shapeBuilder.closed')}</span>
            {/if}
            {#if shape.hasUnsupported}
              <span class="chip chip-adv" title={$i18nT('components.shapeBuilder.preservedTitle')}><AlertTriangle size={10} /> {$i18nT('components.shapeBuilder.advanced')}</span>
            {/if}
            {#if editable}
              <label class="mini-toggle" title={$i18nT('components.shapeBuilder.closedTitle')}>
                <input type="checkbox" checked={!!shape.closed} on:change={(e) => { if (e.currentTarget.checked) shape.closed = true; else delete shape.closed; touch(); }} />
                <Lock size={11} /> {$i18nT('components.shapeBuilder.closed')}
              </label>
              <button class="icon-btn danger" on:click={() => deleteShape(si)} title={$i18nT('components.shapeBuilder.deleteShape')}><Trash2 size={14} /></button>
            {/if}
          </div>

          <!-- Targets -->
          <div class="targets">
            <span class="micro-label">{$i18nT('components.shapeBuilder.targetsLabel')}</span>
            {#if editable}
              {#each shape.targets as tgt, ti}
                <div class="target-row">
                  <Select
                    class="sb-sel"
                    size="sm"
                    bind:value={tgt.kind}
                    on:change={touch}
                    title={$i18nT('components.shapeBuilder.targetSelector')}
                    options={[{ value: 'class', label: $i18nT('components.shapeBuilder.targetClass') }, { value: 'node', label: $i18nT('components.shapeBuilder.targetNode') }, { value: 'subjectsOf', label: $i18nT('components.shapeBuilder.targetSubjectsOf') }, { value: 'objectsOf', label: $i18nT('components.shapeBuilder.targetObjectsOf') }]}
                  />
                  <Combobox
                    class="sb-grow"
                    suggestions={tgt.kind === 'class' ? classSuggestions : propSuggestions}
                    value={disp(tgt.value)}
                    placeholder={tgt.kind === 'class' ? 'ex:SomeClass' : 'ex:someProperty'}
                    on:change={(e) => { tgt.value = expand(e.detail); touch(); }}
                  />
                  <button class="icon-btn" on:click={() => removeTarget(shape, ti)} title={$i18nT('components.shapeBuilder.removeTarget')}><Trash2 size={12} /></button>
                </div>
              {/each}
            {:else}
              {#each shape.targets as tgt}
                <span class="target-chip" title={targetTitle(tgt)}>
                  {#if tgt.kind === 'class'}<Target size={11} />{:else}<Crosshair size={11} />{/if}
                  {targetChipText(tgt, $i18nT)}
                </span>
              {/each}
            {/if}
            {#if shape.logic}
              <span class="target-chip" title={logicSummary(shape.logic)}>{logicSummary(shape.logic)}</span>
            {/if}
            {#if editable}
              <button class="btn btn-xs btn-ghost add-target" on:click={() => addTarget(shape)}><Plus size={11} /> {$i18nT('components.shapeBuilder.target')}</button>
            {/if}
          </div>
        </header>

        <!-- Properties -->
        <div class="props">
          {#if shape.properties.length}
            <div class="props-head">
              <span class="micro-label">{$i18nT('components.shapeBuilder.propertiesLabel')} ({shape.properties.length})</span>
            </div>
          {/if}
          {#each shape.properties as p, pi (p._id)}
            {@const chips = constraintChips(p, !editable, $i18nT)}
            <div class="prop" class:complex={p.hasUnsupported}>
              <!-- Line 1: path + name, cardinality summary right-aligned -->
              <div class="prop-line1">
                {#if editable && !p.pathExpr}
                  <Combobox
                    class="sb-path"
                    suggestions={propSuggestions}
                    value={disp(p.path)}
                    placeholder="ex:propertyPath"
                    on:change={(e) => { p.path = expand(e.detail); touch(); }}
                    title={$i18nT('components.shapeBuilder.propertyPathTitle')}
                  />
                {:else}
                  <span class="ro-path" title={p.pathExpr ? $i18nT('components.shapeBuilder.propertyPathTitle') : (p.path || '')}>
                    {p.pathExpr ? renderPath(p.pathExpr, curie) : disp(p.path)}
                  </span>
                {/if}
                {#if p.name}<span class="prop-name" title="sh:name">{p.name}</span>{/if}
                <span class="spacer"></span>
                {#if (p.c.minCount || 0) >= 1}
                  <span class="chip chip-card" title={`sh:minCount ${p.c.minCount}`}>{$i18nT('components.shapeBuilder.required')}</span>
                {/if}
                {#if cardText(p.c)}
                  <span class="chip chip-card" title={cardTitle(p.c)}>{cardText(p.c)}</span>
                {/if}
                {#if p.hasUnsupported}
                  <span class="chip chip-adv" title={$i18nT('components.shapeBuilder.preservedTitle')}><AlertTriangle size={10} /> {$i18nT('components.shapeBuilder.advanced')}</span>
                {/if}
                {#if editable}
                  <button class="icon-btn" class:active={expanded[p._id]} on:click={() => toggleAdvanced(p._id)} title={$i18nT('components.shapeBuilder.moreConstraints')}>
                    <SlidersHorizontal size={13} />
                  </button>
                  <button class="icon-btn danger" on:click={() => removeProperty(shape, pi)} title={$i18nT('components.shapeBuilder.removeProperty')}><Trash2 size={13} /></button>
                {/if}
              </div>

              <!-- Line 2: typed controls (edit) + labelled constraint chips -->
              {#if editable || chips.length || p.message}
              <div class="prop-line2">
                {#if editable}
                  <div class="ctl-group">
                    <span class="ctl-label">{$i18nT('components.shapeBuilder.valueGroupLabel')}</span>
                    <Select class="sb-sel" size="sm" bind:value={p._vt} on:change={() => applyValueType(p)} title={$i18nT('components.shapeBuilder.valueType')}
                      options={[{ value: 'any', label: $i18nT('components.shapeBuilder.valueAny') }, { value: 'literal', label: $i18nT('components.shapeBuilder.valueLiteral') }, { value: 'class', label: $i18nT('components.shapeBuilder.valueClass') }, { value: 'iri', label: 'IRI' }, { value: 'blank', label: $i18nT('components.shapeBuilder.valueBlankNode') }]} />
                    {#if p._vt === 'literal'}
                      <Select class="sb-sel" size="sm" value={dtValue(p.c.datatype)} on:change={(e) => { p.c.datatype = expand(e.detail); touch(); }} title={$i18nT('components.shapeBuilder.datatype')} options={DATATYPES} />
                    {:else if p._vt === 'class'}
                      <Combobox class="sb-grow" suggestions={classSuggestions} value={disp(p.c.class)} placeholder="ex:SomeClass" on:change={(e) => setIri(p.c, 'class', e.detail)} title={$i18nT('components.shapeBuilder.requiredClassTitle')} />
                    {/if}
                  </div>
                  <div class="ctl-group">
                    <span class="ctl-label">{$i18nT('components.shapeBuilder.cardinalityGroupLabel')}</span>
                    <label class="mini-toggle" title={$i18nT('components.shapeBuilder.requiredTitle')}>
                      <input type="checkbox" checked={(p.c.minCount || 0) >= 1} on:change={(e) => toggleRequired(p, e.currentTarget.checked)} /> {$i18nT('components.shapeBuilder.required')}
                    </label>
                    <label class="mini-toggle" title={$i18nT('components.shapeBuilder.singleTitle')}>
                      <input type="checkbox" checked={p.c.maxCount === 1} on:change={(e) => toggleSingle(p, e.currentTarget.checked)} /> {$i18nT('components.shapeBuilder.single')}
                    </label>
                  </div>
                {/if}
                {#each chips as ch}
                  <span class="chip {ch.cls}" title={ch.title}>{#if ch.label}<b class="chip-key">{ch.label}</b>{/if}{ch.value}</span>
                {/each}
                {#if !editable && p.message}<span class="ro-msg" title={p.message}>“{p.message}”</span>{/if}
              </div>
              {/if}

              {#if editable && expanded[p._id]}
                <div class="adv">
                  <div class="adv-grid">
                    <label>{$i18nT('components.shapeBuilder.minCount')}<input type="number" min="0" value={p.c.minCount ?? ''} on:change={(e) => setNum(p.c, 'minCount', e.currentTarget.value)} /></label>
                    <label>{$i18nT('components.shapeBuilder.maxCount')}<input type="number" min="0" value={p.c.maxCount ?? ''} on:change={(e) => setNum(p.c, 'maxCount', e.currentTarget.value)} /></label>
                    {#if p._vt === 'iri' || p._vt === 'blank' || p._vt === 'any'}
                      <label class="wide">{$i18nT('components.shapeBuilder.nodeKindLabel')}
                        <Select size="sm" value={p.c.nodeKind || ''} on:change={(e) => setStr(p.c, 'nodeKind', e.detail)}
                          options={[{ value: '', label: $i18nT('components.shapeBuilder.anyParen') }, ...NODE_KINDS.map((nk) => ({ value: nk.v, label: nk.label }))]} />
                      </label>
                    {/if}
                    <label>{$i18nT('components.shapeBuilder.minInclusive')}<input value={p.c.minInclusive ?? ''} on:change={(e) => setStr(p.c, 'minInclusive', e.currentTarget.value)} /></label>
                    <label>{$i18nT('components.shapeBuilder.maxInclusive')}<input value={p.c.maxInclusive ?? ''} on:change={(e) => setStr(p.c, 'maxInclusive', e.currentTarget.value)} /></label>
                    <label>{$i18nT('components.shapeBuilder.minExclusive')}<input value={p.c.minExclusive ?? ''} on:change={(e) => setStr(p.c, 'minExclusive', e.currentTarget.value)} /></label>
                    <label>{$i18nT('components.shapeBuilder.maxExclusive')}<input value={p.c.maxExclusive ?? ''} on:change={(e) => setStr(p.c, 'maxExclusive', e.currentTarget.value)} /></label>
                    <label>{$i18nT('components.shapeBuilder.minLength')}<input type="number" min="0" value={p.c.minLength ?? ''} on:change={(e) => setNum(p.c, 'minLength', e.currentTarget.value)} /></label>
                    <label>{$i18nT('components.shapeBuilder.maxLength')}<input type="number" min="0" value={p.c.maxLength ?? ''} on:change={(e) => setNum(p.c, 'maxLength', e.currentTarget.value)} /></label>
                    <label class="wide">{$i18nT('components.shapeBuilder.pattern')}<input value={p.c.pattern ?? ''} placeholder="^[A-Z]{'{2}'}$" on:change={(e) => setStr(p.c, 'pattern', e.currentTarget.value)} /></label>
                    <label>{$i18nT('components.shapeBuilder.flags')}<input value={p.c.flags ?? ''} placeholder="i" on:change={(e) => setStr(p.c, 'flags', e.currentTarget.value)} /></label>
                    <label class="wide">{$i18nT('components.shapeBuilder.nodeShapeLabel')}<Combobox suggestions={classSuggestions} value={disp(p.c.node)} placeholder="ex:AddressShape" on:change={(e) => setIri(p.c, 'node', e.detail)} /></label>
                  </div>

                  <div class="adv-grid">
                    <label class="wide">{$i18nT('components.shapeBuilder.allowedValues')}
                      <input value={(p.c.in || []).map((it) => (it.type === 'iri' ? disp(it.value) : `"${it.value}"`)).join(' ')}
                        placeholder={$i18nT('components.shapeBuilder.allowedValuesPlaceholder')}
                        on:change={(e) => setEnum(p, e.currentTarget.value)} />
                    </label>
                    <label class="wide">{$i18nT('components.shapeBuilder.languages')}
                      <input value={(p.c.languageIn || []).join(' ')} placeholder="en nl"
                        on:change={(e) => setLangs(p, e.currentTarget.value)} />
                    </label>
                    <label class="check-wide"><input type="checkbox" checked={!!p.c.uniqueLang} on:change={(e) => { if (e.currentTarget.checked) p.c.uniqueLang = true; else delete p.c.uniqueLang; touch(); }} /> {$i18nT('components.shapeBuilder.uniqueLanguage')}</label>
                  </div>

                  <div class="adv-grid">
                    <label class="wide">{$i18nT('components.shapeBuilder.labelName')}<input value={p.name ?? ''} on:change={(e) => setStr(p, 'name', e.currentTarget.value)} /></label>
                    <label>{$i18nT('components.shapeBuilder.severity')}
                      <Select size="sm" value={p.severity ?? ''} on:change={(e) => setStr(p, 'severity', e.detail)}
                        options={[
                          { value: '', label: $i18nT('components.shapeBuilder.severityInherit') },
                          { value: SEVERITY_VIOLATION, label: $i18nT('components.shapeBuilder.severityViolation') },
                          { value: SEVERITY_WARNING, label: $i18nT('components.shapeBuilder.severityWarning') },
                          { value: SEVERITY_INFO, label: $i18nT('components.shapeBuilder.severityInfo') },
                          ...(p.severity && !STD_SEVERITIES.includes(p.severity) ? [{ value: p.severity, label: shortLocal(p.severity) }] : []),
                        ]} />
                    </label>
                    <label class="full">{$i18nT('components.shapeBuilder.messageLabel')}<input value={p.message ?? ''} placeholder={$i18nT('components.shapeBuilder.messagePlaceholder')} on:change={(e) => setStr(p, 'message', e.currentTarget.value)} /></label>
                    <label class="full">{$i18nT('components.shapeBuilder.description')}<input value={p.description ?? ''} on:change={(e) => setStr(p, 'description', e.currentTarget.value)} /></label>
                  </div>
                </div>
              {/if}
            </div>
          {/each}
          {#if editable}
            <div class="props-foot">
              <button class="btn btn-sm btn-ghost" on:click={() => addProperty(shape)}><Plus size={13} /> {$i18nT('components.shapeBuilder.property')}</button>
            </div>
          {/if}
        </div>
      </article>
    {/each}

    {#if editable && model.shapes.length > 0}
      <button class="btn btn-sm btn-ghost add-shape" on:click={addShape}><Plus size={14} /> {$i18nT('components.shapeBuilder.addNodeShape')}</button>
    {/if}
  {/if}
</div>

<style>
  .builder { display: flex; flex-direction: column; gap: 0.85rem; }
  .state { display: flex; align-items: center; justify-content: center; gap: 0.5rem; padding: 2rem; color: var(--ink-400); font-size: 0.88rem; }
  .state-warn { color: #92400e; }
  .empty { flex-direction: column; gap: 0.5rem; border: 1px dashed var(--line-soft); border-radius: 12px; }
  .empty h4 { margin: 0; color: var(--ink-700); }
  .empty p { margin: 0; text-align: center; max-width: 30rem; font-size: 0.84rem; }

  .banner { display: flex; align-items: flex-start; gap: 0.45rem; background: #fffbeb; border: 1px solid #fde68a; color: #92400e; border-radius: 10px; padding: 0.55rem 0.7rem; font-size: 0.8rem; line-height: 1.4; }

  .micro-label { font-size: 0.62rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.08em; color: var(--ink-400); flex-shrink: 0; }

  .shape { border: 1px solid var(--line-soft); border-radius: 12px; background: var(--bg-strong); overflow: hidden; box-shadow: var(--shadow-xs); }
  .shape.ro { background: var(--bg-soft); }
  .shape-head { display: flex; flex-direction: column; gap: 0.45rem; padding: 0.7rem 0.85rem 0.6rem; background: linear-gradient(180deg, var(--bg-soft), transparent); border-bottom: 1px solid var(--line-soft); }
  .shape-title-row { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .spacer { flex: 1; }
  .kind-tag { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.62rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.07em; color: var(--brand-700); background: var(--brand-100); border: 1px solid var(--brand-200); padding: 2px 7px; border-radius: 6px; flex-shrink: 0; }

  .iri-input { font-family: 'IBM Plex Mono', monospace; font-size: 0.82rem; padding: 0.28rem 0.45rem; border: 1px solid var(--line-soft); border-radius: 7px; background: var(--bg-strong); color: var(--ink-800); min-width: 0; }
  .name-input { font-size: 0.92rem; font-weight: 600; flex: 1 1 200px; max-width: 26rem; }
  /* Flex sizing for migrated Select/Combobox in inline rows (child-scoped
     classes need :global to reach the trigger/input). */
  :global(.builder .sb-sel) { width: auto; flex: 0 0 auto; }
  :global(.builder .sb-grow) { flex: 1 1 140px; }
  /* The Combobox places its class on the <input> itself. */
  :global(.builder .sb-path) { flex: 1 1 180px; max-width: 24rem; font-family: 'IBM Plex Mono', monospace; font-weight: 600; }
  .iri-input:focus { outline: 2px solid var(--brand-200); outline-offset: -1px; }
  .shape-name { font-family: 'IBM Plex Mono', monospace; font-size: 0.95rem; font-weight: 700; color: var(--ink-900); overflow-wrap: anywhere; }
  .shape-alias { font-size: 0.78rem; color: var(--ink-500); font-style: italic; }

  .targets { display: flex; flex-wrap: wrap; align-items: center; gap: 0.4rem; }
  .target-row { display: inline-flex; align-items: center; gap: 0.3rem; }
  .target-chip { display: inline-flex; align-items: center; gap: 0.3rem; font-size: 0.75rem; font-weight: 600; color: #1d4ed8; background: #dbeafe; padding: 2px 9px; border-radius: 999px; }

  .props { display: flex; flex-direction: column; }
  .props-head { padding: 0.55rem 0.85rem 0.1rem; }
  .props-foot { padding: 0.5rem 0.85rem 0.7rem; }
  .prop { border-top: 1px solid var(--line-soft); padding: 0.55rem 0.85rem 0.6rem; display: flex; flex-direction: column; gap: 0.35rem; transition: background 0.12s; }
  .prop:first-of-type { border-top: none; }
  .prop:hover { background: color-mix(in srgb, var(--brand-100) 28%, transparent); }
  .prop.complex { background: var(--bg-soft); }
  .prop-line1 { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .prop-line2 { display: flex; align-items: center; gap: 0.55rem; flex-wrap: wrap; min-height: 1.4rem; }
  .prop-name { font-size: 0.8rem; color: var(--ink-600); font-weight: 500; }
  .ctl-group { display: inline-flex; align-items: center; gap: 0.4rem; padding: 0.2rem 0.45rem; border: 1px dashed var(--line-soft); border-radius: 8px; }
  .ctl-label { font-size: 0.62rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.07em; color: var(--ink-400); }
  .mini-toggle { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.74rem; color: var(--ink-600); white-space: nowrap; cursor: pointer; }
  .mini-toggle input { margin: 0; }

  .icon-btn { display: grid; place-items: center; width: 28px; height: 28px; border-radius: 7px; border: 1px solid var(--line-soft); background: var(--bg-strong); color: var(--ink-500); cursor: pointer; flex-shrink: 0; transition: background 0.12s, color 0.12s, border-color 0.12s; }
  .icon-btn:hover { background: var(--bg-soft); border-color: var(--line-strong); }
  .icon-btn:focus-visible { outline: 2px solid var(--brand-300); outline-offset: 1px; }
  .icon-btn.active { background: var(--brand-100); color: var(--brand-700); border-color: var(--brand-300); }
  .icon-btn.danger:hover { background: #fef2f2; color: #b91c1c; border-color: #fecaca; }

  .adv { margin-top: 0.25rem; padding: 0.55rem; background: var(--bg-soft); border: 1px solid var(--line-soft); border-radius: 9px; display: flex; flex-direction: column; gap: 0.5rem; }
  .adv-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(130px, 1fr)); gap: 0.45rem; }
  .adv-grid label { display: flex; flex-direction: column; gap: 0.18rem; font-size: 0.7rem; font-weight: 600; color: var(--ink-500); }
  .adv-grid label.wide { grid-column: span 2; }
  .adv-grid label.full { grid-column: 1 / -1; }
  .adv-grid label.check-wide { grid-column: span 2; flex-direction: row; align-items: center; gap: 0.35rem; }
  .adv-grid input { font-size: 0.8rem; padding: 0.28rem 0.4rem; border: 1px solid var(--line-soft); border-radius: 6px; background: var(--bg-strong); color: var(--ink-800); }

  .btn-xs { min-height: 0; font-size: 0.72rem; padding: 0.2rem 0.55rem; border-radius: 7px; }
  .add-target { align-self: center; }
  .add-shape { align-self: flex-start; }

  /* path + message in display mode */
  .ro-path { font-family: 'IBM Plex Mono', monospace; font-size: 0.84rem; color: var(--ink-800); font-weight: 700; overflow-wrap: anywhere; }
  .ro-msg { font-size: 0.74rem; color: var(--ink-400); font-style: italic; margin-left: auto; max-width: 18rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

  /* Constraint chips — one hue per constraint family (value-type green,
     cardinality violet, range amber, string blue, shape/logic pink). */
  .chip { display: inline-flex; align-items: center; gap: 0.28rem; font-size: 0.7rem; font-weight: 600; padding: 1.5px 8px; border-radius: 6px; white-space: nowrap; }
  .chip-key { font-weight: 700; opacity: 0.7; text-transform: uppercase; font-size: 0.6rem; letter-spacing: 0.04em; }
  .chip-card { background: #ede9fe; color: #5b21b6; }
  .chip-type { background: #dcfce7; color: #166534; }
  .chip-range { background: #fef3c7; color: #92400e; }
  .chip-str { background: #e0f2fe; color: #075985; }
  .chip-shape { background: #fce7f3; color: #9d174d; }
  .chip-sev { background: #f1f5f9; color: #475569; }
  .chip-sev-violation { background: #fee2e2; color: #991b1b; }
  .chip-sev-warning { background: #fef3c7; color: #92400e; }
  .chip-sev-info { background: #dbeafe; color: #1e40af; }
  .chip-adv { background: #fee2e2; color: #991b1b; }
  .chip-closed { background: #f1f5f9; color: #475569; }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .state-warn { color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .banner { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.35); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .shape-head { background: linear-gradient(180deg, rgba(255,255,255,0.04), transparent); }
  :global(:is([data-theme="dark"], .dark)) .kind-tag { background: var(--brand-100); border-color: var(--brand-200); color: var(--brand-700); }
  :global(:is([data-theme="dark"], .dark)) .iri-input,
  :global(:is([data-theme="dark"], .dark)) .adv-grid input { background: var(--bg-soft); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn { background: var(--bg-soft); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn.active { background: var(--brand-100); color: var(--brand-700); border-color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn.danger:hover { background: rgba(239,68,68,0.14); color: #fca5a5; border-color: rgba(239,68,68,0.4); }
  :global(:is([data-theme="dark"], .dark)) .target-chip { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .prop:hover { background: rgba(255,255,255,0.04); }
  :global(:is([data-theme="dark"], .dark)) .prop.complex { background: rgba(255,255,255,0.03); }
  :global(:is([data-theme="dark"], .dark)) .adv { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .chip-card { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip-type { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .chip-range { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .chip-str { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip-shape { background: rgba(236,72,153,0.2); color: #f9a8d4; }
  :global(:is([data-theme="dark"], .dark)) .chip-sev,
  :global(:is([data-theme="dark"], .dark)) .chip-closed { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .chip-sev-violation { background: rgba(239,68,68,0.18); color: #fca5a5; }
  :global(:is([data-theme="dark"], .dark)) .chip-sev-warning { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .chip-sev-info { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip-adv { background: rgba(239,68,68,0.18); color: #fca5a5; }
</style>
