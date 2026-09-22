import { describe, it, expect } from 'vitest';
import { parseSourceProfile } from '../sourceProfile.ts';

// The shape `GET /api/sources/{id}/profile` serves: prefixed Turtle, one
// table with a code list, a numeric column and a foreign key, plus the
// profiling activity and the version entity.
const PROFILE = `
@prefix csvw: <http://www.w3.org/ns/csvw#> .
@prefix void: <http://rdfs.org/ns/void#> .
@prefix dsprof: <https://w3id.org/open-triplestore/profile#> .
@prefix prov: <http://www.w3.org/ns/prov#> .
@prefix dct: <http://purl.org/dc/terms/> .
@prefix ds: <https://w3id.org/open-triplestore/datasource#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

<urn:source:s:table:entry> a csvw:Table, void:Dataset ;
  dct:title "entry" ; ds:source <urn:source:s> ;
  csvw:tableSchema <urn:source:s:table:entry:schema> ;
  dsprof:structuralHash "abc123" ; dsprof:sampledRows 12 ; void:entities 12 .
<urn:source:s:table:entry:schema> a csvw:Schema ;
  csvw:column <urn:source:s:table:entry:column:state>, <urn:source:s:table:entry:column:amount>, <urn:source:s:table:entry:column:entry_id> ;
  csvw:primaryKey "entry_id" ;
  csvw:foreignKey <urn:source:s:table:entry:foreignKey:1> .
<urn:source:s:table:entry:foreignKey:1> a csvw:ForeignKey ; csvw:columnReference "lookup_id" ;
  csvw:reference <urn:source:s:table:entry:foreignKey:1:reference> .
<urn:source:s:table:entry:foreignKey:1:reference> a csvw:TableReference ;
  csvw:resource <urn:source:s:table:lookup> ; csvw:columnReference "lookup_id" .
<urn:source:s:table:lookup> a csvw:Table ; dct:title "lookup" ; dsprof:structuralHash "h2" ; dsprof:sampledRows 3 ;
  csvw:tableSchema <urn:source:s:table:lookup:schema> .
<urn:source:s:table:lookup:schema> a csvw:Schema .
<urn:source:s:table:entry:column:entry_id> a csvw:Column ; csvw:name "entry_id" ; csvw:number 1 ;
  csvw:required true ; dsprof:nativeType "INTEGER" ; csvw:datatype xsd:integer ; dsprof:distinctValues 12 ; dsprof:nullCount 0 .
<urn:source:s:table:entry:column:state> a csvw:Column ; csvw:name "state" ; csvw:number 2 ;
  csvw:required true ; dsprof:nativeType "TEXT" ; csvw:datatype xsd:string ;
  dsprof:distinctValues 3 ; dsprof:nullCount 0 ; dsprof:cardinalityRatio 0.25 ; dsprof:meanLength 4.5 ;
  dsprof:pattern dsprof:Code ; dsprof:patternConfidence 1.0 ;
  dsprof:topValue <urn:source:s:table:entry:column:state:value:2>, <urn:source:s:table:entry:column:state:value:1> .
<urn:source:s:table:entry:column:state:value:1> rdf:value "alpha" ; dsprof:occurrences 5 ; dsprof:rank 1 .
<urn:source:s:table:entry:column:state:value:2> rdf:value "beta" ; dsprof:occurrences 4 ; dsprof:rank 2 .
<urn:source:s:table:entry:column:amount> a csvw:Column ; csvw:name "amount" ; csvw:number 3 ;
  csvw:required false ; dsprof:nativeType "REAL" ; csvw:datatype xsd:double ;
  dsprof:min 1.5 ; dsprof:max 12.5 ; dsprof:mean 7.0 ; dsprof:p50 7.5 ; dsprof:p99 12.5 .
<urn:source:s:profile:version:2:activity> a prov:Activity, dsprof:Profiling ;
  prov:startedAtTime "2026-09-17T08:00:00Z"^^xsd:dateTime ; prov:endedAtTime "2026-09-17T08:00:01Z"^^xsd:dateTime ;
  ds:durationMs 900 ; prov:wasAssociatedWith <http://x/users/adm> .
<urn:source:s:profile:version:2> a dsprof:SourceProfile ; ds:version 2 .
`;

describe('parseSourceProfile', () => {
  const profile = parseSourceProfile(PROFILE);

  it('lists every table, sorted, with its counts and keys', () => {
    expect(profile.tables.map((t) => t.name)).toEqual(['entry', 'lookup']);
    const entry = profile.tables[0];
    expect(entry.rows).toBe(12);
    expect(entry.structuralHash).toBe('abc123');
    expect(entry.sampledRows).toBe(12);
    expect(entry.primaryKey).toEqual(['entry_id']);
    expect(entry.isView).toBe(false);
    expect(profile.tables[1].rows).toBeNull();
  });

  it('orders columns by position and carries every statistic', () => {
    const [id, state, amount] = profile.tables[0].columns;
    expect([id.name, state.name, amount.name]).toEqual(['entry_id', 'state', 'amount']);
    expect(id.datatype).toBe('integer');
    expect(id.required).toBe(true);
    expect(state.distinct).toBe(3);
    expect(state.cardinalityRatio).toBe(0.25);
    expect(state.meanLength).toBe(4.5);
    expect(state.pattern).toBe('code');
    expect(state.patternConfidence).toBe(1);
    expect(state.numeric).toBeNull();
    expect(amount.numeric).toEqual({ min: 1.5, max: 12.5, mean: 7, p50: 7.5, p99: 12.5 });
    expect(amount.required).toBe(false);
    expect(amount.pattern).toBeNull();
  });

  it('keeps a code list ranked and gives every other column none', () => {
    const [id, state] = profile.tables[0].columns;
    expect(state.topValues).toEqual([
      { value: 'alpha', count: 5, rank: 1 },
      { value: 'beta', count: 4, rank: 2 },
    ]);
    expect(id.topValues).toEqual([]);
  });

  it('resolves a foreign key to the referenced table by name', () => {
    expect(profile.tables[0].foreignKeys).toEqual([
      { columns: ['lookup_id'], refTable: 'lookup', refColumns: ['lookup_id'] },
    ]);
  });

  it('reads the activity and the version entity', () => {
    expect(profile.activity).toEqual({
      version: 2,
      startedAt: '2026-09-17T08:00:00Z',
      endedAt: '2026-09-17T08:00:01Z',
      durationMs: 900,
      actor: 'http://x/users/adm',
    });
  });

  it('yields no tables for an empty or unrelated document', () => {
    expect(parseSourceProfile('').tables).toEqual([]);
    expect(parseSourceProfile('<urn:x> <urn:p> "o" .').tables).toEqual([]);
  });
});
