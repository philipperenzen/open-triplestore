// A datasource profile (`GET /api/sources/{id}/profile`, Turtle) as the
// structure the Explore screen renders: tables, their columns with the
// statistics the store computed, code lists, keys and foreign keys.
//
// Everything here is metadata and aggregates — the profile carries no row
// content beyond the values of a code list, which is the point of profiling
// in the store rather than in the proposer.
import { Parser } from 'n3';
import type { Quad, Term } from 'n3';

const CSVW = 'http://www.w3.org/ns/csvw#';
const VOID = 'http://rdfs.org/ns/void#';
const PROF = 'https://w3id.org/open-triplestore/profile#';
const RDF = 'http://www.w3.org/1999/02/22-rdf-syntax-ns#';
const PROV = 'http://www.w3.org/ns/prov#';
const DCT = 'http://purl.org/dc/terms/';
const DS = 'https://w3id.org/open-triplestore/datasource#';

export interface TopValue { value: string; count: number; rank: number }

export interface ProfileColumn {
  iri: string;
  name: string;
  number: number;
  required: boolean;
  /** Local name of the XSD datatype (`integer`, `string`), when known. */
  datatype: string | null;
  nativeType: string;
  distinct: number | null;
  nulls: number | null;
  cardinalityRatio: number | null;
  meanLength: number | null;
  numeric: { min: number; max: number; mean: number; p50: number; p99: number } | null;
  /** `email`, `iri`, `uuid`, `date`, `code`, `phone` — a claim about lexical shape only. */
  pattern: string | null;
  patternConfidence: number | null;
  /** The complete value set of a code list; empty for every other column. */
  topValues: TopValue[];
}

export interface ProfileForeignKey { columns: string[]; refTable: string; refColumns: string[] }

export interface ProfileTable {
  iri: string;
  name: string;
  rows: number | null;
  isView: boolean;
  structuralHash: string;
  sampledRows: number;
  primaryKey: string[];
  columns: ProfileColumn[];
  foreignKeys: ProfileForeignKey[];
}

export interface ProfileActivity {
  version: number | null;
  startedAt: string | null;
  endedAt: string | null;
  durationMs: number | null;
  actor: string | null;
}

export interface SourceProfile { tables: ProfileTable[]; activity: ProfileActivity }

type Index = Map<string, Map<string, Term[]>>;

function index(quads: Quad[]): Index {
  const out: Index = new Map();
  for (const q of quads) {
    const s = q.subject.value;
    let preds = out.get(s);
    if (!preds) { preds = new Map(); out.set(s, preds); }
    const objs = preds.get(q.predicate.value);
    if (objs) objs.push(q.object); else preds.set(q.predicate.value, [q.object]);
  }
  return out;
}

const first = (ix: Index, s: string, p: string): Term | undefined => ix.get(s)?.get(p)?.[0];
const all = (ix: Index, s: string, p: string): Term[] => ix.get(s)?.get(p) ?? [];
const str = (ix: Index, s: string, p: string): string | null => first(ix, s, p)?.value ?? null;
const num = (ix: Index, s: string, p: string): number | null => {
  const v = str(ix, s, p);
  if (v == null) return null;
  const n = Number(v);
  return Number.isFinite(n) ? n : null;
};
const local = (iri: string): string => iri.slice(Math.max(iri.lastIndexOf('#'), iri.lastIndexOf('/')) + 1);

/** Parse a served profile document. A document that is not one yields no tables. */
export function parseSourceProfile(turtle: string): SourceProfile {
  const quads = new Parser().parse(String(turtle || ''));
  const ix = index(quads);
  const tableIris: string[] = [];
  for (const [s, preds] of ix) {
    if ((preds.get(`${RDF}type`) ?? []).some((t) => t.value === `${CSVW}Table`)) tableIris.push(s);
  }
  const nameOf = new Map<string, string>();
  for (const t of tableIris) nameOf.set(t, str(ix, t, `${DCT}title`) ?? local(t));

  const tables: ProfileTable[] = tableIris.map((t) => {
    const schema = str(ix, t, `${CSVW}tableSchema`) ?? '';
    const columns: ProfileColumn[] = all(ix, schema, `${CSVW}column`).map((c) => {
      const ci = c.value;
      const topValues: TopValue[] = all(ix, ci, `${PROF}topValue`)
        .map((v) => ({
          value: str(ix, v.value, `${RDF}value`) ?? '',
          count: num(ix, v.value, `${PROF}occurrences`) ?? 0,
          rank: num(ix, v.value, `${PROF}rank`) ?? 0,
        }))
        .sort((a, b) => a.rank - b.rank);
      const min = num(ix, ci, `${PROF}min`);
      const max = num(ix, ci, `${PROF}max`);
      const mean = num(ix, ci, `${PROF}mean`);
      const p50 = num(ix, ci, `${PROF}p50`);
      const p99 = num(ix, ci, `${PROF}p99`);
      const numeric = min != null && max != null && mean != null
        ? { min, max, mean, p50: p50 ?? min, p99: p99 ?? max }
        : null;
      const dt = str(ix, ci, `${CSVW}datatype`);
      const pattern = str(ix, ci, `${PROF}pattern`);
      return {
        iri: ci,
        name: str(ix, ci, `${CSVW}name`) ?? local(ci),
        number: num(ix, ci, `${CSVW}number`) ?? 0,
        required: str(ix, ci, `${CSVW}required`) === 'true',
        datatype: dt ? local(dt) : null,
        nativeType: str(ix, ci, `${PROF}nativeType`) ?? '',
        distinct: num(ix, ci, `${PROF}distinctValues`),
        nulls: num(ix, ci, `${PROF}nullCount`),
        cardinalityRatio: num(ix, ci, `${PROF}cardinalityRatio`),
        meanLength: num(ix, ci, `${PROF}meanLength`),
        numeric,
        pattern: pattern ? local(pattern).toLowerCase() : null,
        patternConfidence: num(ix, ci, `${PROF}patternConfidence`),
        topValues,
      };
    });
    columns.sort((a, b) => a.number - b.number);
    const foreignKeys: ProfileForeignKey[] = all(ix, schema, `${CSVW}foreignKey`).map((fk) => {
      const ref = str(ix, fk.value, `${CSVW}reference`) ?? '';
      const resource = str(ix, ref, `${CSVW}resource`) ?? '';
      return {
        columns: all(ix, fk.value, `${CSVW}columnReference`).map((c) => c.value),
        refTable: nameOf.get(resource) ?? local(resource),
        refColumns: all(ix, ref, `${CSVW}columnReference`).map((c) => c.value),
      };
    });
    return {
      iri: t,
      name: nameOf.get(t) ?? local(t),
      rows: num(ix, t, `${VOID}entities`),
      isView: str(ix, t, `${PROF}isView`) === 'true',
      structuralHash: str(ix, t, `${PROF}structuralHash`) ?? '',
      sampledRows: num(ix, t, `${PROF}sampledRows`) ?? 0,
      primaryKey: all(ix, schema, `${CSVW}primaryKey`).map((k) => k.value),
      columns,
      foreignKeys,
    };
  });
  tables.sort((a, b) => a.name.localeCompare(b.name));

  // The activity and the version entity travel with the profile.
  let activity: ProfileActivity = { version: null, startedAt: null, endedAt: null, durationMs: null, actor: null };
  for (const [s, preds] of ix) {
    const types = (preds.get(`${RDF}type`) ?? []).map((t) => t.value);
    if (types.includes(`${PROF}Profiling`)) {
      activity = {
        ...activity,
        startedAt: str(ix, s, `${PROV}startedAtTime`),
        endedAt: str(ix, s, `${PROV}endedAtTime`),
        durationMs: num(ix, s, `${DS}durationMs`),
        actor: str(ix, s, `${PROV}wasAssociatedWith`),
      };
    }
    if (types.includes(`${PROF}SourceProfile`)) {
      activity.version = num(ix, s, `${DS}version`);
    }
  }
  return { tables, activity };
}
