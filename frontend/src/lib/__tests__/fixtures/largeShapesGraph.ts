// Synthetic shape graph shaped like the large ones a store hands back from
// GET /api/shacl/shape-graphs/{id}/turtle: every blank node is a labelled
// `_:hex` subject on its own lines (never an inline `[ … ]`), the SHACL prefix
// is `shacl:` rather than `sh:`, targets are SPARQL-based (`shacl:target _:b`
// where `_:b` carries `shacl:select`), lists are long (`shacl:in`,
// `shacl:ignoredProperties`, `shacl:or`), names and descriptions carry language
// tags and messages are long. No shape uses `shacl:targetClass`.
//
// At `nodeShapes: 4100` the output is roughly 20 MB, the size that used to hang
// the editor page. Output is deterministic for a given size.

export interface LargeShapesGraph {
  turtle: string;
  nodeShapes: number;
  namedPropertyShapes: number;
  /** IRI of every node shape, in the order they were written. */
  shapeIris: string[];
}

const EX = 'http://example.org/';

export function generateLargeShapesGraph(nodeShapes: number): LargeShapesGraph {
  // Small LCG: the same size always yields the same document.
  let seed = 0x2f6e2b1;
  const rnd = (n: number) => {
    seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
    return seed % n;
  };
  let bseq = 0;
  // Long, server-style labels: 24 hex digits.
  const bnode = () => `_:b${(++bseq).toString(16).padStart(23, '0')}`;

  const namedPropertyShapes = Math.max(1, Math.round(nodeShapes * 0.139));
  const out: string[] = [
    '@prefix shacl: <http://www.w3.org/ns/shacl#> .',
    '@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .',
    '@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .',
    '@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .',
    '',
  ];
  const pad = (i: number) => String(i).padStart(5, '0');
  const shapeIri = (i: number) => `${EX}shape/Shape${pad(i)}`;
  const propIri = (i: number) => `${EX}shape/property/Prop${pad(i)}`;
  const defIri = (i: number) => `${EX}def/property${pad(i)}`;
  const classIri = (i: number) => `${EX}def/Class${pad(i)}`;
  const message = (i: number) =>
    `A value of this property must satisfy the rule for item ${i}. ` +
    'The value is checked against the allowed values of the code list and against the expected datatype; ' +
    'when it does not match, the record is reported so the data owner can correct it before publication.';

  const list = (items: string[]): string => {
    if (!items.length) return 'rdf:nil';
    const cells = items.map(() => bnode());
    for (let i = 0; i < items.length; i++) {
      out.push(`${cells[i]} rdf:first ${items[i]} ;\n  rdf:rest ${i + 1 < items.length ? cells[i + 1] : 'rdf:nil'} .`);
    }
    return cells[0];
  };

  // Named property shapes, shared between node shapes.
  for (let k = 0; k < namedPropertyShapes; k++) {
    out.push(
      `<${propIri(k)}> a shacl:PropertyShape ;\n` +
        `  shacl:path <${defIri(90000 + k)}> ;\n` +
        `  shacl:datatype xsd:string ;\n` +
        `  shacl:maxCount 1 ;\n` +
        `  shacl:name "Shared property ${k}"@en , "Gedeelde eigenschap ${k}"@nl .`,
    );
  }

  const shapeIris: string[] = [];
  for (let i = 0; i < nodeShapes; i++) {
    const iri = shapeIri(i);
    shapeIris.push(iri);
    const target = bnode();
    out.push(
      `${target} a shacl:SPARQLTarget ;\n` +
        `  shacl:select """SELECT ?this WHERE { ?this a <${classIri(i)}> . FILTER NOT EXISTS { ?this <${EX}def/retired> true } }""" .`,
    );

    const props: string[] = [];
    const nProps = 3 + rnd(4); // 3..6, about 4.5 on average
    for (let j = 0; j < nProps; j++) {
      const p = bnode();
      props.push(p);
      const lines = [
        `shacl:path <${defIri(rnd(20000))}>`,
        `shacl:name "property ${i}.${j}"@nl , "property ${i}.${j}"@en`,
        `shacl:description "Description of property ${j} of shape ${i}."@en`,
        `shacl:message "${message(i * 10 + j)}"`,
      ];
      if (j % 2 === 0) lines.push('shacl:minCount 1', 'shacl:datatype xsd:string');
      if (j === 1) {
        const values = Array.from({ length: 12 + rnd(10) }, (_, v) => `<${EX}def/code/c${pad(i)}-${v}>`);
        lines.push(`shacl:in ${list(values)}`);
      }
      if (j === 2) lines.push(`shacl:class <${classIri(rnd(nodeShapes))}>`, 'shacl:nodeKind shacl:IRI');
      out.push(`${p} ${lines.join(' ;\n  ')} .`);
    }
    if (i % 2 === 0) props.push(`<${propIri(rnd(namedPropertyShapes))}>`);

    const parts = [
      'a shacl:NodeShape',
      `shacl:name "Shape ${i}"@en , "Vorm ${i}"@nl`,
      `shacl:description "Node shape number ${i} of the generated graph."@en`,
      `shacl:target ${target}`,
    ];
    if (i % 3 === 0) {
      parts.push('shacl:closed true');
      const ignored = Array.from({ length: 6 }, (_, v) => `<${defIri(v)}>`);
      parts.push(`shacl:ignoredProperties ${list(ignored)}`);
    }
    // About a quarter of the shapes carry an `shacl:or` of two class shapes.
    if (i % 4 === 1) {
      const alts = [bnode(), bnode()];
      for (const a of alts) out.push(`${a} shacl:class <${classIri(rnd(nodeShapes))}> .`);
      parts.push(`shacl:or ${list(alts)}`);
    }
    parts.push(`shacl:property ${props.join(' , ')}`);
    out.push(`<${iri}> ${parts.join(' ;\n  ')} .`);
  }

  return { turtle: out.join('\n') + '\n', nodeShapes, namedPropertyShapes, shapeIris };
}
