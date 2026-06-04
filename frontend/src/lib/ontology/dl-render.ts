// Description-Logic-ish rendering for OWL class expressions and restrictions.
// Pure: takes ClassExpr / Restriction shapes from schema-model.ts and
// returns short strings using `shortenIRI` for prefixed display.

import { shortenIRI } from '../rdf-utils';

export type Restriction = {
  onProperty: string;
  kind: 'some' | 'all' | 'value' | 'min' | 'max' | 'exact' | 'qmin' | 'qmax' | 'qexact';
  filler?: ClassExpr | { iri?: string; literal?: string; datatype?: string; lang?: string };
  n?: number;
  onClass?: ClassExpr | string;
};

export type ClassExpr =
  | { type: 'named'; iri: string }
  | { type: 'union'; parts: ClassExpr[] }
  | { type: 'intersection'; parts: ClassExpr[] }
  | { type: 'complement'; of: ClassExpr }
  | { type: 'oneOf'; members: string[] }
  | { type: 'restriction'; restriction: Restriction }
  | { type: 'unknown'; bnode?: string };

const short = (iri: string) => (iri ? shortenIRI(iri) : '?');

function renderFiller(filler: Restriction['filler']): string {
  if (!filler) return '⊤';
  if ('type' in (filler as any)) return renderClassExpr(filler as ClassExpr);
  const f = filler as { iri?: string; literal?: string; datatype?: string };
  if (f.iri) return short(f.iri);
  if (f.literal !== undefined) {
    const dt = f.datatype && f.datatype !== 'http://www.w3.org/2001/XMLSchema#string'
      ? `^^${short(f.datatype)}` : '';
    return `"${f.literal}"${dt}`;
  }
  return '?';
}

export function renderRestriction(r: Restriction): string {
  const p = short(r.onProperty);
  const onCls = r.onClass
    ? typeof r.onClass === 'string' ? short(r.onClass) : renderClassExpr(r.onClass)
    : '';
  switch (r.kind) {
    case 'some':   return `∃ ${p}.${renderFiller(r.filler)}`;
    case 'all':    return `∀ ${p}.${renderFiller(r.filler)}`;
    case 'value':  return `${p} ∋ ${renderFiller(r.filler)}`;
    case 'min':    return `≥ ${r.n ?? 0} ${p}`;
    case 'max':    return `≤ ${r.n ?? 0} ${p}`;
    case 'exact':  return `= ${r.n ?? 0} ${p}`;
    case 'qmin':   return `≥ ${r.n ?? 0} ${p}.${onCls || '⊤'}`;
    case 'qmax':   return `≤ ${r.n ?? 0} ${p}.${onCls || '⊤'}`;
    case 'qexact': return `= ${r.n ?? 0} ${p}.${onCls || '⊤'}`;
  }
}

export function renderClassExpr(e: ClassExpr): string {
  if (!e) return '?';
  switch (e.type) {
    case 'named':       return short(e.iri);
    case 'union':       return `(${e.parts.map(renderClassExpr).join(' ⊔ ')})`;
    case 'intersection':return `(${e.parts.map(renderClassExpr).join(' ⊓ ')})`;
    case 'complement':  return `¬${renderClassExpr(e.of)}`;
    case 'oneOf':       return `{${e.members.map(short).join(', ')}}`;
    case 'restriction': return renderRestriction(e.restriction);
    case 'unknown':     return e.bnode ? `_:${e.bnode}` : '?';
  }
}

export function renderChain(chain: string[]): string {
  return chain.map(short).join(' ∘ ');
}
