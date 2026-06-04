<script>
  // No-code SHACL builder. Renders the parsed shapes model as editable cards
  // with model-driven pickers (target/path datalists fed by the dataset's real
  // classes + properties) and typed constraint controls. Every edit mutates the
  // structured model and re-serialises to Turtle, so the source view stays in
  // sync. When the document uses SHACL the model can't losslessly round-trip
  // (`canRoundTrip === false`) the cards drop to read-only and Turtle stays the
  // single source of truth.
  import {
    parseShapesGraph,
    serializeShapesGraph,
    makeCurie,
    propChips,
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
    {#if !editable}
      <div class="banner">
        <Lock size={14} />
        <!-- eslint-disable-next-line svelte/no-at-html-tags -- trusted static i18n string -->
        <span>{@html $i18nT('components.shapeBuilder.readonlyBanner')}</span>
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
          <span class="badge badge-node" title={$i18nT('components.shapeBuilder.nodeShapeBadge')}>N</span>
          {#if editable}
            <input
              class="iri-input"
              value={disp(shape.iri)}
              on:change={(e) => { shape.iri = expand(e.currentTarget.value); touch(); }}
              title={$i18nT('components.shapeBuilder.shapeIriTitle')}
            />
          {:else}
            <span class="iri-static">{disp(shape.iri)}</span>
          {/if}
          <span class="spacer"></span>
          {#if editable}
            <label class="mini-toggle" title={$i18nT('components.shapeBuilder.closedTitle')}>
              <input type="checkbox" checked={!!shape.closed} on:change={(e) => { shape.closed = e.currentTarget.checked; touch(); }} />
              <Lock size={11} /> {$i18nT('components.shapeBuilder.closed')}
            </label>
            <button class="icon-btn danger" on:click={() => deleteShape(si)} title={$i18nT('components.shapeBuilder.deleteShape')}><Trash2 size={14} /></button>
          {/if}
        </header>

        <!-- Targets -->
        <div class="targets">
          {#each shape.targets as tgt, ti}
            <div class="target-row">
              {#if editable}
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
              {:else}
                <span class="target-chip">
                  {#if tgt.kind === 'class'}<Target size={11} />{:else}<Crosshair size={11} />{/if}
                  {tgt.kind === 'class' ? '' : tgt.kind + ' '}{disp(tgt.value)}
                </span>
              {/if}
            </div>
          {/each}
          {#if editable}
            <button class="link-btn" on:click={() => addTarget(shape)}><Plus size={11} /> {$i18nT('components.shapeBuilder.target')}</button>
          {/if}
        </div>

        <!-- Properties -->
        <div class="props">
          {#each shape.properties as p, pi (p._id)}
            <div class="prop" class:complex={p.complex}>
              {#if editable}
                <div class="prop-main">
                  <Combobox
                    class="sb-path"
                    suggestions={propSuggestions}
                    value={disp(p.path)}
                    placeholder="ex:propertyPath"
                    on:change={(e) => { p.path = expand(e.detail); touch(); }}
                    title={$i18nT('components.shapeBuilder.propertyPathTitle')}
                  />
                  <Select class="sb-sel" size="sm" bind:value={p._vt} on:change={() => applyValueType(p)} title={$i18nT('components.shapeBuilder.valueType')}
                    options={[{ value: 'any', label: $i18nT('components.shapeBuilder.valueAny') }, { value: 'literal', label: $i18nT('components.shapeBuilder.valueLiteral') }, { value: 'class', label: $i18nT('components.shapeBuilder.valueClass') }, { value: 'iri', label: 'IRI' }, { value: 'blank', label: $i18nT('components.shapeBuilder.valueBlankNode') }]} />
                  {#if p._vt === 'literal'}
                    <Select class="sb-sel" size="sm" value={dtValue(p.c.datatype)} on:change={(e) => { p.c.datatype = expand(e.detail); touch(); }} title={$i18nT('components.shapeBuilder.datatype')} options={DATATYPES} />
                  {:else if p._vt === 'class'}
                    <Combobox class="sb-grow" suggestions={classSuggestions} value={disp(p.c.class)} placeholder="ex:SomeClass" on:change={(e) => setIri(p.c, 'class', e.detail)} title={$i18nT('components.shapeBuilder.requiredClassTitle')} />
                  {/if}
                  <label class="mini-toggle" title={$i18nT('components.shapeBuilder.requiredTitle')}>
                    <input type="checkbox" checked={(p.c.minCount || 0) >= 1} on:change={(e) => toggleRequired(p, e.currentTarget.checked)} /> {$i18nT('components.shapeBuilder.required')}
                  </label>
                  <label class="mini-toggle" title={$i18nT('components.shapeBuilder.singleTitle')}>
                    <input type="checkbox" checked={p.c.maxCount === 1} on:change={(e) => toggleSingle(p, e.currentTarget.checked)} /> {$i18nT('components.shapeBuilder.single')}
                  </label>
                  <button class="icon-btn" class:active={expanded[p._id]} on:click={() => toggleAdvanced(p._id)} title={$i18nT('components.shapeBuilder.moreConstraints')}>
                    <SlidersHorizontal size={13} />
                  </button>
                  <button class="icon-btn danger" on:click={() => removeProperty(shape, pi)} title={$i18nT('components.shapeBuilder.removeProperty')}><Trash2 size={13} /></button>
                </div>

                {#if expanded[p._id]}
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
                          options={[{ value: '', label: $i18nT('components.shapeBuilder.severityInherit') }, { value: 'Violation', label: $i18nT('components.shapeBuilder.severityViolation') }, { value: 'Warning', label: $i18nT('components.shapeBuilder.severityWarning') }, { value: 'Info', label: $i18nT('components.shapeBuilder.severityInfo') }]} />
                      </label>
                      <label class="full">{$i18nT('components.shapeBuilder.messageLabel')}<input value={p.message ?? ''} placeholder={$i18nT('components.shapeBuilder.messagePlaceholder')} on:change={(e) => setStr(p, 'message', e.currentTarget.value)} /></label>
                      <label class="full">{$i18nT('components.shapeBuilder.description')}<input value={p.description ?? ''} on:change={(e) => setStr(p, 'description', e.currentTarget.value)} /></label>
                    </div>
                  </div>
                {/if}
              {:else}
                <!-- read-only row -->
                <div class="prop-ro">
                  <span class="badge badge-prop">P</span>
                  <span class="ro-path">{p.pathComplex ? $i18nT('components.shapeBuilder.pathExpression') : disp(p.path)}</span>
                  <span class="ro-chips">
                    {#each propChips(p, curie) as chip}<span class="chip chip-{chip.k}">{chip.v}</span>{/each}
                    {#if p.complex}<span class="chip chip-adv">{$i18nT('components.shapeBuilder.advanced')}</span>{/if}
                  </span>
                  {#if p.message}<span class="ro-msg" title={p.message}>“{p.message}”</span>{/if}
                </div>
              {/if}
            </div>
          {/each}
          {#if editable}
            <button class="link-btn add-prop" on:click={() => addProperty(shape)}><Plus size={12} /> {$i18nT('components.shapeBuilder.property')}</button>
          {/if}
        </div>
      </article>
    {/each}

    {#if editable && model.shapes.length > 0}
      <button class="btn btn-ghost add-shape" on:click={addShape}><Plus size={14} /> {$i18nT('components.shapeBuilder.addNodeShape')}</button>
    {/if}
  {/if}
</div>

<style>
  .builder { display: flex; flex-direction: column; gap: 0.7rem; }
  .state { display: flex; align-items: center; justify-content: center; gap: 0.5rem; padding: 2rem; color: #94a3b8; font-size: 0.88rem; }
  .state-warn { color: #92400e; }
  .empty { flex-direction: column; gap: 0.5rem; border: 1px dashed var(--line-soft); border-radius: 12px; }
  .empty h4 { margin: 0; color: #334155; }
  .empty p { margin: 0; text-align: center; max-width: 30rem; font-size: 0.84rem; }

  .banner { display: flex; align-items: flex-start; gap: 0.45rem; background: #fffbeb; border: 1px solid #fde68a; color: #92400e; border-radius: 10px; padding: 0.55rem 0.7rem; font-size: 0.8rem; line-height: 1.4; }

  .shape { border: 1px solid var(--line-soft); border-radius: 12px; background: #fff; overflow: hidden; }
  .shape.ro { background: #fcfcfd; }
  .shape-head { display: flex; align-items: center; gap: 0.5rem; padding: 0.55rem 0.7rem; background: linear-gradient(90deg, #f8fafc, #fff); border-bottom: 1px solid var(--line-soft); }
  .spacer { flex: 1; }
  .badge { width: 20px; height: 20px; border-radius: 5px; color: #fff; font-size: 0.68rem; font-weight: 700; display: grid; place-items: center; flex-shrink: 0; }
  .badge-node { background: #4a90d9; }
  .badge-prop { background: #6a5acd; }

  .iri-input { font-family: 'IBM Plex Mono', monospace; font-size: 0.82rem; padding: 0.28rem 0.45rem; border: 1px solid var(--line-soft); border-radius: 7px; background: #fff; color: #1e293b; min-width: 0; }
  .iri-input.path { flex: 1 1 160px; }
  .iri-input.grow { flex: 1 1 140px; }
  /* Flex sizing for migrated Select/Combobox in inline rows (child-scoped
     classes need :global to reach the trigger/input). */
  :global(.builder .sb-sel) { width: auto; flex: 0 0 auto; }
  :global(.builder .sb-grow) { flex: 1 1 140px; }
  :global(.builder .sb-path) { flex: 1 1 160px; }
  .iri-input:focus { outline: 2px solid #bae6fd; outline-offset: -1px; }
  .iri-static { font-family: 'IBM Plex Mono', monospace; font-size: 0.86rem; font-weight: 600; color: #1e293b; }

  .targets { display: flex; flex-wrap: wrap; align-items: center; gap: 0.4rem; padding: 0.5rem 0.7rem; border-bottom: 1px dashed #eef2f7; }
  .target-row { display: inline-flex; align-items: center; gap: 0.3rem; }
  .target-chip { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.74rem; color: #1d4ed8; background: #dbeafe; padding: 1px 8px; border-radius: 999px; }

  .props { display: flex; flex-direction: column; }
  .prop { border-top: 1px solid #f1f5f9; padding: 0.45rem 0.7rem; }
  .prop:first-child { border-top: none; }
  .prop.complex { background: #fafafa; }
  .prop-main { display: flex; align-items: center; gap: 0.4rem; flex-wrap: wrap; }
  .mini-toggle { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.74rem; color: #475569; white-space: nowrap; cursor: pointer; }
  .mini-toggle input { margin: 0; }

  .icon-btn { display: grid; place-items: center; width: 28px; height: 28px; border-radius: 7px; border: 1px solid var(--line-soft); background: #fff; color: #64748b; cursor: pointer; flex-shrink: 0; }
  .icon-btn:hover { background: #f1f5f9; }
  .icon-btn.active { background: #ecfeff; color: #0e7490; border-color: #7ED6D0; }
  .icon-btn.danger:hover { background: #fef2f2; color: #b91c1c; border-color: #fecaca; }

  .adv { margin-top: 0.5rem; padding: 0.55rem; background: #f8fafc; border: 1px solid var(--line-soft); border-radius: 9px; display: flex; flex-direction: column; gap: 0.5rem; }
  .adv-grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(130px, 1fr)); gap: 0.45rem; }
  .adv-grid label { display: flex; flex-direction: column; gap: 0.18rem; font-size: 0.7rem; font-weight: 600; color: #64748b; }
  .adv-grid label.wide { grid-column: span 2; }
  .adv-grid label.full { grid-column: 1 / -1; }
  .adv-grid label.check-wide { grid-column: span 2; flex-direction: row; align-items: center; gap: 0.35rem; }
  .adv-grid input { font-size: 0.8rem; padding: 0.28rem 0.4rem; border: 1px solid var(--line-soft); border-radius: 6px; background: #fff; color: #1e293b; }

  .link-btn { display: inline-flex; align-items: center; gap: 0.25rem; font-size: 0.74rem; color: #0e7490; background: none; border: 1px dashed #cbd5e1; border-radius: 7px; padding: 0.25rem 0.5rem; cursor: pointer; align-self: flex-start; }
  .link-btn:hover { background: #ecfeff; border-color: #7ED6D0; }
  .add-prop { margin: 0.45rem 0.7rem 0.6rem; }
  .add-shape { align-self: flex-start; }

  /* read-only property row */
  .prop-ro { display: flex; align-items: center; gap: 0.5rem; flex-wrap: wrap; }
  .ro-path { font-family: 'IBM Plex Mono', monospace; font-size: 0.8rem; color: #334155; font-weight: 500; }
  .ro-chips { display: inline-flex; gap: 0.3rem; flex-wrap: wrap; }
  .ro-msg { font-size: 0.74rem; color: #94a3b8; font-style: italic; margin-left: auto; max-width: 16rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .chip { font-size: 0.68rem; padding: 1px 7px; border-radius: 6px; white-space: nowrap; }
  .chip-card { background: #ede9fe; color: #5b21b6; }
  .chip-type { background: #dcfce7; color: #166534; }
  .chip-range { background: #fef3c7; color: #92400e; }
  .chip-str { background: #e0f2fe; color: #075985; }
  .chip-shape { background: #fce7f3; color: #9d174d; }
  .chip-sev { background: #f1f5f9; color: #475569; }
  .chip-adv { background: #fee2e2; color: #991b1b; }

  /* ---- Dark mode overrides (scoped rules out-specify global theme.css) ---- */
  :global(:is([data-theme="dark"], .dark)) .state-warn { color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .empty h4,
  :global(:is([data-theme="dark"], .dark)) .ro-path { color: var(--ink-800); }
  :global(:is([data-theme="dark"], .dark)) .banner { background: rgba(245,158,11,0.12); border-color: rgba(245,158,11,0.35); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .shape { background: var(--bg-strong); }
  :global(:is([data-theme="dark"], .dark)) .shape.ro { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .shape-head { background: linear-gradient(90deg, var(--bg-soft), var(--bg-strong)); }
  :global(:is([data-theme="dark"], .dark)) .iri-input,
  :global(:is([data-theme="dark"], .dark)) .icon-btn,
  :global(:is([data-theme="dark"], .dark)) .adv-grid input { background: var(--bg-soft); color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .iri-static { color: var(--ink-900); }
  :global(:is([data-theme="dark"], .dark)) .target-chip { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .prop.complex { background: rgba(255,255,255,0.03); }
  :global(:is([data-theme="dark"], .dark)) .mini-toggle { color: var(--ink-700); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn:hover { background: rgba(255,255,255,0.06); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn.active { background: var(--brand-100); color: var(--brand-700); border-color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .icon-btn.danger:hover { background: rgba(239,68,68,0.14); color: #fca5a5; border-color: rgba(239,68,68,0.4); }
  :global(:is([data-theme="dark"], .dark)) .adv { background: var(--bg-soft); }
  :global(:is([data-theme="dark"], .dark)) .adv-grid label { color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .link-btn { color: var(--brand-700); border-color: var(--line-strong); }
  :global(:is([data-theme="dark"], .dark)) .link-btn:hover { background: var(--brand-100); border-color: var(--brand-300); }
  :global(:is([data-theme="dark"], .dark)) .chip-card { background: rgba(139,92,246,0.2); color: #c4b5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip-type { background: rgba(16,185,129,0.18); color: #6ee7b7; }
  :global(:is([data-theme="dark"], .dark)) .chip-range { background: rgba(245,158,11,0.18); color: #fcd34d; }
  :global(:is([data-theme="dark"], .dark)) .chip-str { background: rgba(59,130,246,0.2); color: #93c5fd; }
  :global(:is([data-theme="dark"], .dark)) .chip-shape { background: rgba(236,72,153,0.2); color: #f9a8d4; }
  :global(:is([data-theme="dark"], .dark)) .chip-sev { background: rgba(255,255,255,0.06); color: var(--ink-500); }
  :global(:is([data-theme="dark"], .dark)) .chip-adv { background: rgba(239,68,68,0.18); color: #fca5a5; }
</style>
