#!/usr/bin/env python3
"""Build the vendored LOV vocabulary catalog from a lov.nq.gz dump.

The Linked Open Vocabularies (LOV) N-Quads dump contains one named graph per
vocabulary (latest version) plus the LOV metadata graph
<https://lov.linkeddata.es/dataset/lov> describing every vocabulary (titles,
descriptions, tags, namespaces, prefixes, versions, VOAF relations and reuse
metrics).  This script distils that metadata graph into the compact JSON
catalog the backend embeds (assets/vocab/lov-catalog.json.gz), so vocabulary
search works without the full 18 MB corpus at hand.

Usage:
    python scripts/build_lov_catalog.py <lov.nq.gz> <out-catalog.json.gz> \
        [--snapshot-date YYYY-MM-DD] [--source-url URL]

The LOV dataset is licensed CC BY 4.0 (see NOTICE); the generated catalog
records that attribution.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import sys
from collections import defaultdict

META_GRAPHS = {
    "<https://lov.linkeddata.es/dataset/lov>",
    "<http://lov.okfn.org/dataset/lov>",
}

LINE_RE = re.compile(r"^(\S+)\s+(<[^>]+>)\s+(.*?)\s+(<[^>]+>)\s*\.\s*$")
LIT_RE = re.compile(r'^"(.*)"(?:@([a-zA-Z-]+)|\^\^<[^>]+>)?$', re.DOTALL)

VOAF = "http://purl.org/vocommons/voaf#"
DCT = "http://purl.org/dc/terms/"
VANN = "http://purl.org/vocab/vann/"
DCAT = "http://www.w3.org/ns/dcat#"
FOAF = "http://xmlns.com/foaf/0.1/"
RDF_TYPE = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>"

# Relations whose presence counts toward a vocabulary's incoming links.
VOAF_RELATIONS = [
    "metadataVoc",
    "specializes",
    "generalizes",
    "extends",
    "hasEquivalencesWith",
    "hasDisjunctionsWith",
]


UNICODE_ESC_RE = re.compile(r"\\u([0-9a-fA-F]{4})|\\U([0-9a-fA-F]{8})")


def unescape_literal(raw: str) -> str:
    """Unescape an N-Triples literal body (incl. \\uXXXX / \\UXXXXXXXX)."""
    s = (
        raw.replace("\\\\", "\x00")
        .replace('\\"', '"')
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
    )
    s = UNICODE_ESC_RE.sub(lambda m: chr(int(m.group(1) or m.group(2), 16)), s)
    return s.replace("\x00", "\\")


def parse_object(obj: str):
    """Return ('iri', value) | ('lit', value, lang) | None."""
    if obj.startswith("<") and obj.endswith(">"):
        return ("iri", obj[1:-1], None)
    m = LIT_RE.match(obj)
    if m:
        return ("lit", unescape_literal(m.group(1)), m.group(2))
    if obj.startswith("_:"):
        return ("bnode", obj, None)
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("dump")
    ap.add_argument("out")
    ap.add_argument("--snapshot-date", default="unknown")
    ap.add_argument("--source-url", default="https://lov.linkeddata.es/lov.nq.gz")
    args = ap.parse_args()

    sha = hashlib.sha256(open(args.dump, "rb").read()).hexdigest()

    # subject -> predicate -> [(kind, value, lang)] for the metadata graph only
    meta = defaultdict(lambda: defaultdict(list))
    graph_quads = defaultdict(int)

    with gzip.open(args.dump, "rt", encoding="utf-8", errors="replace") as f:
        for line in f:
            m = LINE_RE.match(line)
            if not m:
                continue
            s, p, o, g = m.groups()
            graph_quads[g[1:-1]] += 1
            if g in META_GRAPHS:
                po = parse_object(o)
                if po is not None:
                    meta[s][p[1:-1]].append(po)

    def values(subj, pred, kind=None):
        out = []
        for k, v, lang in meta.get(subj, {}).get(pred, []):
            if kind is None or k == kind:
                out.append((v, lang))
        return out

    def first(subj, pred, kind=None):
        vals = values(subj, pred, kind)
        return vals[0][0] if vals else None

    def as_int(subj, pred):
        v = first(subj, pred, "lit")
        try:
            return int(v) if v is not None else 0
        except ValueError:
            return 0

    # Vocabulary subjects: everything typed voaf:Vocabulary in the metadata graph
    vocab_subjects = [
        s
        for s, preds in meta.items()
        if any(
            kind == "iri" and v == VOAF + "Vocabulary"
            for kind, v, _ in preds.get(RDF_TYPE[1:-1], [])
        )
    ]

    # Incoming links per vocab URI across VOAF relations
    incoming = defaultdict(int)
    for s, preds in meta.items():
        for rel in VOAF_RELATIONS:
            for k, v, _ in preds.get(VOAF + rel, []):
                if k == "iri":
                    incoming[v] += 1

    def lang_strings(subj, pred):
        return [
            {"value": v, "lang": lang}
            for v, lang in values(subj, pred, "lit")
        ]

    def agent_names(subj, pred):
        # Subject keys keep IRI angle brackets; parsed object IRIs do not.
        names = []
        for k, v, _ in meta.get(subj, {}).get(pred, []):
            if k in ("iri", "bnode"):
                agent_subj = v if k == "bnode" else f"<{v}>"
                name = first(agent_subj, FOAF + "name", "lit")
                if name:
                    names.append(name)
        return names

    vocabularies = []
    for subj in sorted(vocab_subjects):
        uri = subj[1:-1] if subj.startswith("<") else subj
        prefix = first(subj, VANN + "preferredNamespacePrefix", "lit")
        nsp = first(subj, VANN + "preferredNamespaceUri", "lit") or first(
            subj, VANN + "preferredNamespaceUri", "iri"
        )
        if not prefix:
            continue

        versions = []
        for k, dist, _ in meta.get(subj, {}).get(DCAT + "distribution", []):
            dsub = dist if k == "bnode" else f"<{dist}>"
            name = first(dsub, DCT + "title", "lit")
            versions.append(
                {
                    "name": name,
                    "issued": first(dsub, DCT + "issued", "lit"),
                    "class_count": as_int(dsub, VOAF + "classNumber"),
                    "property_count": as_int(dsub, VOAF + "propertyNumber"),
                    "datatype_count": as_int(dsub, VOAF + "datatypeNumber"),
                    "instance_count": as_int(dsub, VOAF + "instanceNumber"),
                }
            )
        versions.sort(key=lambda v: v.get("issued") or "", reverse=True)

        vocabularies.append(
            {
                "prefix": prefix,
                "uri": uri,
                "nsp": nsp or uri,
                "titles": lang_strings(subj, DCT + "title"),
                "descriptions": lang_strings(subj, DCT + "description"),
                "tags": sorted(v for v, _ in values(subj, DCAT + "keyword", "lit")),
                "homepage": first(subj, FOAF + "homepage", "iri"),
                "is_defined_by": first(subj, "http://www.w3.org/2000/01/rdf-schema#isDefinedBy", "iri"),
                "issued": first(subj, DCT + "issued", "lit"),
                "modified": first(subj, DCT + "modified", "lit"),
                "langs": sorted(
                    {v.rsplit("/", 1)[-1] for v, _ in values(subj, DCT + "language", "iri")}
                ),
                "creators": agent_names(subj, DCT + "creator"),
                "contributors": agent_names(subj, DCT + "contributor"),
                "publishers": agent_names(subj, DCT + "publisher"),
                "versions": versions,
                "metrics": {
                    "occurrences_in_datasets": as_int(subj, VOAF + "occurrencesInDatasets"),
                    "reused_by_datasets": as_int(subj, VOAF + "reusedByDatasets"),
                    "reused_by_vocabularies": as_int(subj, VOAF + "reusedByVocabularies"),
                    "incoming_links": incoming.get(uri, 0),
                },
                "graph_quads": graph_quads.get(uri, 0),
            }
        )

    # Term-level LOD reuse metrics (sparse; ranking falls back to vocab metrics)
    term_metrics = {}
    vocab_uri_set = {f"<{v['uri']}>" for v in vocabularies}
    for s, preds in meta.items():
        occ = preds.get(VOAF + "occurrencesInDatasets")
        reused = preds.get(VOAF + "reusedByDatasets")
        if (occ or reused) and s not in vocab_uri_set and s.startswith("<"):
            def geti(vals):
                if not vals:
                    return 0
                try:
                    return int(vals[0][1])
                except (ValueError, TypeError):
                    return 0
            term_metrics[s[1:-1]] = [geti(occ), geti(reused)]

    catalog = {
        "format_version": 1,
        "source": {
            "url": args.source_url,
            "snapshot_date": args.snapshot_date,
            "sha256": sha,
            "license": "CC BY 4.0",
            "attribution": "Linked Open Vocabularies (LOV), https://lov.linkeddata.es/",
        },
        "vocabularies": vocabularies,
        "term_metrics": term_metrics,
    }

    payload = json.dumps(catalog, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    with gzip.open(args.out, "wb", compresslevel=9) as f:
        f.write(payload)

    print(f"vocabularies: {len(vocabularies)}")
    print(f"term metrics: {len(term_metrics)}")
    print(f"raw json: {len(payload)/1e6:.2f} MB, gz: {__import__('os').path.getsize(args.out)/1e6:.2f} MB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
