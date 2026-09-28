// An RML mapping (Turtle) as the matrix the Map screen renders: one block per
// triples map — what it reads, what subject it mints, which classes it
// asserts — and one row per predicate-object map, with the object described
// in a word: a column, a template, a constant, a join to another map, or a
// function.
import { Parser } from 'n3';
import type { Quad, Term } from 'n3';

const RR = 'http://www.w3.org/ns/r2rml#';
const RML = 'http://semweb.mmlab.be/ns/rml#';
const FNML = 'http://semweb.mmlab.be/ns/fnml#';
const FNO = 'https://w3id.org/function/ontology#';
const OTSFN = 'https://w3id.org/open-triplestore/fn#';
const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';

export type ObjectKind = 'column' | 'template' | 'constant' | 'parent' | 'function' | 'unknown';

export interface MatrixRow {
  predicate: string;
  kind: ObjectKind;
  /** The column, the template, the constant, the parent map's name or the function call. */
  detail: string;
  datatype: string | null;
  language: string | null;
  /** For a join: the parent map's IRI and the `child = parent` conditions. */
  parentMap: string | null;
  joins: Array<{ child: string; parent: string }>;
}

export interface MatrixMap {
  iri: string;
  name: string;
  table: string | null;
  query: string | null;
  subjectKind: 'template' | 'column' | 'constant' | 'function' | 'unknown';
  subject: string;
  classes: string[];
  rows: MatrixRow[];
}

type Index = Map<string, Map<string, Term[]>>;

function index(quads: Quad[]): Index {
  const out: Index = new Map();
  for (const q of quads) {
    let preds = out.get(q.subject.value);
    if (!preds) { preds = new Map(); out.set(q.subject.value, preds); }
    const objs = preds.get(q.predicate.value);
    if (objs) objs.push(q.object); else preds.set(q.predicate.value, [q.object]);
  }
  return out;
}

const first = (ix: Index, s: string, p: string): Term | undefined => ix.get(s)?.get(p)?.[0];
const all = (ix: Index, s: string, p: string): Term[] => ix.get(s)?.get(p) ?? [];
const str = (ix: Index, s: string, p: string): string | null => first(ix, s, p)?.value ?? null;
const local = (iri: string): string => iri.slice(Math.max(iri.lastIndexOf('#'), iri.lastIndexOf('/')) + 1);

/** `otsfn:mapValue(status)` — the function's local name and its row inputs. */
function describeFunction(ix: Index, fv: string): string {
  let name = 'function';
  const inputs: string[] = [];
  for (const pom of all(ix, fv, `${RR}predicateObjectMap`)) {
    const p = str(ix, pom.value, `${RR}predicate`) ?? '';
    const constant = str(ix, pom.value, `${RR}object`);
    const om = str(ix, pom.value, `${RR}objectMap`);
    const column = om ? str(ix, om, `${RR}column`) ?? str(ix, om, `${RML}reference`) : null;
    if (p === `${FNO}executes` && constant) {
      name = constant.startsWith(OTSFN) ? `otsfn:${local(constant)}` : local(constant);
    } else if (column) {
      inputs.push(column);
    } else if (p === `${OTSFN}template` && constant) {
      inputs.push(constant);
    }
  }
  return `${name}(${inputs.join(', ')})`;
}

function describeObject(ix: Index, pom: string): Omit<MatrixRow, 'predicate'> {
  const base = { datatype: null, language: null, parentMap: null, joins: [] as MatrixRow['joins'] };
  const constant = first(ix, pom, `${RR}object`);
  if (constant) return { ...base, kind: 'constant', detail: constant.value };
  const om = str(ix, pom, `${RR}objectMap`);
  if (!om) return { ...base, kind: 'unknown', detail: '' };
  const datatype = str(ix, om, `${RR}datatype`);
  const language = str(ix, om, `${RR}language`);
  const typed = { ...base, datatype: datatype ? local(datatype) : null, language };
  const parent = str(ix, om, `${RR}parentTriplesMap`);
  if (parent) {
    const joins = all(ix, om, `${RR}joinCondition`).map((jc) => ({
      child: str(ix, jc.value, `${RR}child`) ?? '',
      parent: str(ix, jc.value, `${RR}parent`) ?? '',
    }));
    return { ...typed, kind: 'parent', detail: local(parent), parentMap: parent, joins };
  }
  const fv = str(ix, om, `${FNML}functionValue`);
  if (fv) return { ...typed, kind: 'function', detail: describeFunction(ix, fv) };
  const column = str(ix, om, `${RR}column`) ?? str(ix, om, `${RML}reference`);
  if (column) return { ...typed, kind: 'column', detail: column };
  const template = str(ix, om, `${RR}template`);
  if (template) return { ...typed, kind: 'template', detail: template };
  const c = str(ix, om, `${RR}constant`);
  if (c) return { ...typed, kind: 'constant', detail: c };
  return { ...typed, kind: 'unknown', detail: '' };
}

/** Parse a mapping document into its matrix. Unparseable input is an empty matrix. */
export function parseRmlMatrix(turtle: string): MatrixMap[] {
  let quads: Quad[];
  try {
    quads = new Parser().parse(String(turtle || ''));
  } catch {
    return [];
  }
  const ix = index(quads);
  const maps: MatrixMap[] = [];
  for (const [s, preds] of ix) {
    const types = (preds.get(`${RDF}type`) ?? []).map((t) => t.value);
    const isMap = types.includes(`${RR}TriplesMap`) || preds.has(`${RML}logicalSource`) || preds.has(`${RR}logicalTable`);
    if (!isMap) continue;
    const ls = str(ix, s, `${RML}logicalSource`) ?? str(ix, s, `${RR}logicalTable`) ?? '';
    const sm = str(ix, s, `${RR}subjectMap`);
    let subjectKind: MatrixMap['subjectKind'] = 'unknown';
    let subject = '';
    if (sm) {
      const fv = str(ix, sm, `${FNML}functionValue`);
      const template = str(ix, sm, `${RR}template`);
      const column = str(ix, sm, `${RR}column`) ?? str(ix, sm, `${RML}reference`);
      const constant = str(ix, sm, `${RR}constant`);
      if (fv) { subjectKind = 'function'; subject = describeFunction(ix, fv); }
      else if (template) { subjectKind = 'template'; subject = template; }
      else if (column) { subjectKind = 'column'; subject = column; }
      else if (constant) { subjectKind = 'constant'; subject = constant; }
    } else {
      const constant = str(ix, s, `${RR}subject`);
      if (constant) { subjectKind = 'constant'; subject = constant; }
    }
    const classes = [
      ...(sm ? all(ix, sm, `${RR}class`) : []),
      ...all(ix, s, `${RR}class`),
    ].map((c) => c.value);
    const rows: MatrixRow[] = all(ix, s, `${RR}predicateObjectMap`).map((pom) => {
      const predicate = str(ix, pom.value, `${RR}predicate`)
        ?? (str(ix, pom.value, `${RR}predicateMap`) ? str(ix, str(ix, pom.value, `${RR}predicateMap`)!, `${RR}constant`) : null)
        ?? '';
      return { predicate, ...describeObject(ix, pom.value) };
    });
    rows.sort((a, b) => a.predicate.localeCompare(b.predicate));
    maps.push({
      iri: s,
      name: local(s),
      table: str(ix, ls, `${RR}tableName`),
      query: str(ix, ls, `${RML}query`) ?? str(ix, ls, `${RR}sqlQuery`),
      subjectKind,
      subject,
      classes: [...new Set(classes)].sort(),
      rows,
    });
  }
  maps.sort((a, b) => a.name.localeCompare(b.name));
  return maps;
}
